//! Conversation execution facade for a session.
//!
//! This module owns the public conversation contract: slash-command dispatch,
//! blocking sends, streaming sends, and attachment handling.
//! Lower-level runtime modules own run lifecycle and event forwarding.

use super::{
    agent_loop_runtime::{build_pinned_agent_loop, cognitive_binding_for_projection},
    command_runtime,
    execution_coordinator::ExecutionCoordinator,
    run_admission,
    run_lifecycle::RunControlState,
    runtime::ConversationInput,
    runtime_checkpoints::runtime_checkpoint_channel,
    AgentRunSpawn, AgentSession,
};
use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::{AgentEvent, AgentResult};
use crate::error::{CodeError, Result};
use crate::llm::{Attachment, Message};
use crate::loop_checkpoint::LoopCheckpoint;
use crate::session_checkpoint::{SessionCheckpointError, SessionLogicalResumeEvidenceV1};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExactRecoveryError {
    #[error(transparent)]
    Checkpoint(#[from] SessionCheckpointError),
    #[error(transparent)]
    Code(#[from] CodeError),
}

pub(crate) enum ExactRecoveryPreparation {
    Replayed(AgentRunSpawn),
    Ready(PreparedExactRecovery),
}

pub(crate) struct PreparedExactRecovery {
    checkpoint: LoopCheckpoint,
    evidence: SessionLogicalResumeEvidenceV1,
    run_id: String,
    prompt: String,
    lease: run_admission::RunAdmissionLease,
}

pub(super) async fn send(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
) -> Result<AgentResult> {
    // Admission must precede command dispatch and internal-history reads.
    let _lease = ExecutionCoordinator::admit(session, "send").await?;

    if let Some(result) = command_runtime::dispatch_blocking(session, prompt, history).await? {
        return Ok(result);
    }

    warn_deferred_init(session);
    settle_prompt(session, prompt, history).await
}

pub(super) async fn send_with_attachments(
    session: &AgentSession,
    prompt: &str,
    attachments: &[Attachment],
    history: Option<&[Message]>,
) -> Result<AgentResult> {
    reject_oversized_attachments(attachments)?;
    let _lease = ExecutionCoordinator::admit(session, "send-with-attachments").await?;
    let prompt = prompt_with_attachments(prompt, attachments);
    settle_prompt(session, &prompt, history).await
}

pub(super) async fn stream_with_attachments(
    session: &AgentSession,
    prompt: &str,
    attachments: &[Attachment],
    history: Option<&[Message]>,
) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
    reject_oversized_attachments(attachments)?;
    let lease = ExecutionCoordinator::admit(session, "stream-with-attachments").await?;
    let prompt = prompt_with_attachments(prompt, attachments);
    spawn_fact_stream(session, &prompt, history, lease).await
}

fn reject_oversized_attachments(attachments: &[Attachment]) -> Result<()> {
    for attachment in attachments {
        if attachment.data.len() > crate::llm::MAX_ATTACHMENT_BYTES {
            return Err(CodeError::SessionConfiguration {
                field: "attachments",
                message: format!(
                    "attachment exceeds the {}-byte limit",
                    crate::llm::MAX_ATTACHMENT_BYTES
                ),
            });
        }
    }
    Ok(())
}

pub(super) async fn stream(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
    // Slash commands share admission because they read and may mutate the same
    // session state as model-backed operations.
    let lease = ExecutionCoordinator::admit(session, "stream").await?;

    if let Some((rx, handle)) = command_runtime::dispatch_streaming(session, prompt).await? {
        let worker_abort = handle.abort_handle();
        return Ok((
            rx,
            ExecutionCoordinator::supervise_stream(handle, vec![worker_abort], lease),
        ));
    }

    spawn_fact_stream(session, prompt, history, lease).await
}

/// Start one detached Code run at an exact host-selected identity.
pub(super) async fn spawn_run_with_id(
    session: &AgentSession,
    run_id: &str,
    prompt: &str,
) -> Result<AgentRunSpawn> {
    if let Some(replay) = exact_run_replay(session, run_id, prompt).await? {
        return Ok(replay);
    }
    let lease = ExecutionCoordinator::admit(session, "spawn-run").await?;
    let input = ConversationInput::from_history(session, None);
    let run_control = RunControlState::from_session(session);
    let reservation = run_control.reserve_run_with_id(run_id, prompt).await?;
    let mut snapshot = reservation.snapshot().clone();
    if reservation.replayed() {
        return Ok(AgentRunSpawn::Replayed { snapshot });
    }

    let _ = input;
    snapshot = run_control.snapshot(run_id).await.ok_or_else(|| {
        CodeError::Session(format!(
            "newly admitted Run '{run_id}' disappeared before execution"
        ))
    })?;
    let mut pinned = match prepare_pinned(session, prompt, false).await {
        Ok(pinned) => pinned,
        Err(error) => {
            run_control.fail_reserved_run_start(run_id, &error).await;
            return Err(error);
        }
    };
    bind_external_run(session, &mut pinned, run_id).await;
    let (events, worker) = spawn_fact_worker(pinned, prompt, false);
    let abort = worker.abort_handle();
    let worker = ExecutionCoordinator::supervise_stream(worker, vec![abort], lease);
    Ok(AgentRunSpawn::Started {
        snapshot,
        worker: drain_detached_events(events, worker),
    })
}

/// Resume one durable loop checkpoint into an exact fresh run identity.
pub(super) async fn spawn_recovery_with_run_id(
    session: &AgentSession,
    checkpoint_run_id: &str,
    run_id: &str,
) -> Result<AgentRunSpawn> {
    let prompt = format!("<resume run={checkpoint_run_id}>");
    if let Some(replay) = exact_run_replay(session, run_id, &prompt).await? {
        return Ok(replay);
    }

    let lease = ExecutionCoordinator::admit(session, "spawn-recovery").await?;
    let _checkpoint = load_resume_checkpoint(session, checkpoint_run_id).await?;
    let run_control = RunControlState::from_session(session);
    let reservation = match run_control.reserve_run_with_id(run_id, &prompt).await {
        Ok(reservation) => reservation,
        Err(error) => return Err(error),
    };
    let mut snapshot = reservation.snapshot().clone();
    if reservation.replayed() {
        return Ok(AgentRunSpawn::Replayed { snapshot });
    }
    snapshot = run_control.snapshot(run_id).await.ok_or_else(|| {
        CodeError::Session(format!(
            "newly admitted recovery Run '{run_id}' disappeared before execution"
        ))
    })?;
    let mut pinned = match prepare_pinned(session, &prompt, false).await {
        Ok(pinned) => pinned,
        Err(error) => {
            run_control.fail_reserved_run_start(run_id, &error).await;
            return Err(error);
        }
    };
    bind_external_run(session, &mut pinned, run_id).await;
    let (events, worker) = spawn_fact_worker(pinned, &prompt, true);
    let abort = worker.abort_handle();
    let worker = ExecutionCoordinator::supervise_stream(worker, vec![abort], lease);
    Ok(AgentRunSpawn::Started {
        snapshot,
        worker: drain_detached_events(events, worker),
    })
}

async fn spawn_fact_stream(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
    lease: run_admission::RunAdmissionLease,
) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
    let (tx, rx) = mpsc::channel(8);
    let mut pinned = prepare_pinned(session, prompt, true).await?;
    pinned.parts.set_events(tx.clone());
    let prompt = prompt.to_string();
    let update_history = history.is_none();
    let history = history.map(|messages| messages.to_vec());
    let handle = tokio::spawn(async move {
        let PinnedFact {
            mut parts,
            capability_run,
            committer,
            finish,
        } = pinned;
        let settled = async {
            let run = parts.open()?;
            if let Some(history) = history.as_deref() {
                run.seed_history(history)?;
            }
            prepare_pre_analysis(&parts, &prompt).await?;
            let settled = run.user_text(&prompt).await?;
            settle_confirmations(&run, settled, &parts).await
        }
        .await;
        let messages = parts.transcript();
        let usage = parts.usage();
        let mapped = settled
            .as_ref()
            .map(|settled| agent_result(settled, messages, usage))
            .map_err(|error| anyhow::anyhow!("{error:#}"))
            .and_then(|result| gate_result(&parts, result));
        emit_gated(&tx, &settled, &mapped).await;
        // Channel close is host settlement. Extraction is post-turn work.
        drop(tx);
        if let Some(surface) = parts.surface.as_mut() {
            surface.events.take();
        }
        extract_settled_memory(&parts, &prompt, &mapped).await;
        finish.publish(&prompt, &mapped, update_history).await;
        drop(parts);
        let _ = capability_run.close().await;
        if let Some(committer) = committer {
            let _ = committer.await;
        }
    });
    let abort = handle.abort_handle();
    Ok((
        rx,
        ExecutionCoordinator::supervise_stream(handle, vec![abort], lease),
    ))
}

fn spawn_fact_worker(
    pinned: PinnedFact,
    prompt: &str,
    resume_only: bool,
) -> (mpsc::Receiver<AgentEvent>, JoinHandle<()>) {
    let (tx, events) = mpsc::channel(8);
    let prompt = prompt.to_string();
    let worker = tokio::spawn(async move {
        let PinnedFact {
            mut parts,
            capability_run,
            committer,
            finish,
        } = pinned;
        let settled = async {
            let run = parts.open()?;
            if resume_only {
                run.resume_limit(32).await
            } else {
                prepare_pre_analysis(&parts, &prompt).await?;
                let settled = run.user_text(&prompt).await?;
                settle_confirmations(&run, settled, &parts).await
            }
        }
        .await;
        let messages = parts.transcript();
        let usage = parts.usage();
        let mapped = settled
            .as_ref()
            .map(|settled| agent_result(settled, messages, usage))
            .map_err(|error| anyhow::anyhow!("{error:#}"))
            .and_then(|result| gate_result(&parts, result));
        emit_gated(&tx, &settled, &mapped).await;
        // Channel close is host settlement. Extraction is post-turn work.
        drop(tx);
        if let Some(surface) = parts.surface.as_mut() {
            surface.events.take();
        }
        extract_settled_memory(&parts, &prompt, &mapped).await;
        finish.publish(&prompt, &mapped, false).await;
        drop(parts);
        let _ = capability_run.close().await;
        if let Some(committer) = committer {
            let _ = committer.await;
        }
    });
    (events, worker)
}

async fn extract_settled_memory(
    parts: &FactSession,
    prompt: &str,
    mapped: &std::result::Result<AgentResult, anyhow::Error>,
) {
    let Ok(result) = mapped else {
        return;
    };
    let Some(surface) = parts.surface.as_ref() else {
        return;
    };
    surface
        .agent
        .fact_extract_turn_memory(prompt, &result.text, &surface.session_id, &surface.cancel)
        .await;
}

async fn emit_gated(
    tx: &mpsc::Sender<AgentEvent>,
    settled: &anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>>,
    mapped: &std::result::Result<AgentResult, anyhow::Error>,
) {
    if let Err(error) = mapped {
        let _ = tx
            .send(AgentEvent::Error {
                message: error.to_string(),
            })
            .await;
        return;
    }
    emit_settled(tx, settled).await;
}

async fn emit_settled(
    tx: &mpsc::Sender<AgentEvent>,
    settled: &anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>>,
) {
    match settled {
        Ok(settled) => {
            let text = settled.view.assistant.clone().unwrap_or_default();
            let _ = tx.send(AgentEvent::TextDelta { text: text.clone() }).await;
            let _ = tx
                .send(AgentEvent::End {
                    text,
                    usage: crate::llm::TokenUsage::default(),
                    verification_summary: Box::new(
                        crate::verification::VerificationSummary::from_reports(&[]),
                    ),
                    meta: None,
                })
                .await;
        }
        Err(error) => {
            let _ = tx
                .send(AgentEvent::Error {
                    message: error.to_string(),
                })
                .await;
        }
    }
}

/// Validate and pin one content-addressed recovery boundary before a host
/// captures workspace baseline evidence or admits the target Run.
pub(super) async fn prepare_recovery_with_evidence(
    session: &AgentSession,
    evidence: &SessionLogicalResumeEvidenceV1,
    checkpoint_identity: &str,
    run_id: &str,
) -> std::result::Result<ExactRecoveryPreparation, ExactRecoveryError> {
    prepare_exact_recovery(session, evidence, checkpoint_identity, run_id, None).await
}

/// Validate and pin a logical boundary supplied by the same already-validated
/// portable checkpoint payload, without first splitting it into SessionStore
/// fragment writes.
pub(super) async fn prepare_recovery_from_checkpoint(
    session: &AgentSession,
    evidence: &SessionLogicalResumeEvidenceV1,
    checkpoint_identity: &str,
    run_id: &str,
    checkpoint: LoopCheckpoint,
) -> std::result::Result<ExactRecoveryPreparation, ExactRecoveryError> {
    prepare_exact_recovery(
        session,
        evidence,
        checkpoint_identity,
        run_id,
        Some(checkpoint),
    )
    .await
}

async fn prepare_exact_recovery(
    session: &AgentSession,
    evidence: &SessionLogicalResumeEvidenceV1,
    checkpoint_identity: &str,
    run_id: &str,
    supplied_checkpoint: Option<LoopCheckpoint>,
) -> std::result::Result<ExactRecoveryPreparation, ExactRecoveryError> {
    evidence.validate()?;
    if evidence.session_id != session.session_id {
        return Err(SessionCheckpointError::InvalidDescriptor(
            "logical-resume evidence belongs to another Session".into(),
        )
        .into());
    }

    let prompt = exact_recovery_prompt(checkpoint_identity);
    if let Some(replay) = exact_run_replay(session, run_id, &prompt).await? {
        return Ok(ExactRecoveryPreparation::Replayed(replay));
    }

    let lease = ExecutionCoordinator::admit(session, "prepare-exact-recovery").await?;
    let checkpoint = match supplied_checkpoint {
        Some(checkpoint) => checkpoint,
        None => load_resume_checkpoint(session, &evidence.source_run_id).await?,
    };
    checkpoint
        .ensure_owned_by(&evidence.source_run_id, &session.session_id)
        .map_err(|error| {
            SessionCheckpointError::InvalidPayload(format!(
                "refusing supplied logical-resume checkpoint: {error:#}"
            ))
        })?;
    evidence.validate_for(&checkpoint)?;
    if let Some(binding) = &checkpoint.capability_binding {
        super::agent_loop_runtime::validate_run_capability_binding(session, binding).map_err(
            |error| match error {
                crate::capability::RunCapabilityBindingError::ContentDrift { .. } => {
                    SessionCheckpointError::ContentDrift(format!(
                        "the source Run capability generation is unavailable on this Session: {error}"
                    ))
                }
                _ => SessionCheckpointError::InvalidPayload(format!(
                    "the source Run capability binding is invalid: {error}"
                )),
            },
        )?;
    }
    Ok(ExactRecoveryPreparation::Ready(PreparedExactRecovery {
        checkpoint,
        evidence: evidence.clone(),
        run_id: run_id.to_string(),
        prompt,
        lease,
    }))
}

/// Admit a previously validated immutable recovery plan and start its worker.
pub(super) async fn spawn_prepared_recovery(
    session: &AgentSession,
    prepared: PreparedExactRecovery,
) -> std::result::Result<AgentRunSpawn, ExactRecoveryError> {
    let PreparedExactRecovery {
        checkpoint,
        evidence,
        run_id,
        prompt,
        lease,
    } = prepared;
    evidence.validate_for(&checkpoint)?;
    if let Some(binding) = &checkpoint.capability_binding {
        super::agent_loop_runtime::validate_run_capability_binding(session, binding).map_err(
            |error| match error {
                crate::capability::RunCapabilityBindingError::ContentDrift { .. } => {
                    SessionCheckpointError::ContentDrift(format!(
                        "the source Run capability generation is unavailable on this Session: {error}"
                    ))
                }
                _ => SessionCheckpointError::InvalidPayload(format!(
                    "the source Run capability binding is invalid: {error}"
                )),
            },
        )?;
    }

    let run_control = RunControlState::from_session(session);
    let reservation = run_control.reserve_run_with_id(&run_id, &prompt).await?;
    let mut snapshot = reservation.snapshot().clone();
    if reservation.replayed() {
        return Ok(AgentRunSpawn::Replayed { snapshot });
    }
    if let Some(binding) = checkpoint.capability_binding.clone() {
        if let Err(error) = run_control
            .bind_capability_generation(&run_id, binding)
            .await
        {
            run_control.fail_reserved_run_start(&run_id, &error).await;
            return Err(error.into());
        }
    }
    snapshot = run_control.snapshot(&run_id).await.ok_or_else(|| {
        CodeError::Session(format!(
            "newly admitted exact recovery Run '{run_id}' disappeared before execution"
        ))
    })?;
    let mut pinned = match prepare_pinned(session, &prompt, false).await {
        Ok(pinned) => pinned,
        Err(error) => {
            run_control.fail_reserved_run_start(&run_id, &error).await;
            return Err(error.into());
        }
    };
    bind_external_run(session, &mut pinned, &run_id).await;
    let (events, worker) = spawn_fact_worker(pinned, &prompt, true);
    let abort = worker.abort_handle();
    let worker = ExecutionCoordinator::supervise_stream(worker, vec![abort], lease);
    Ok(AgentRunSpawn::Started {
        snapshot,
        worker: drain_detached_events(events, worker),
    })
}

/// Resume the session fact log.
///
/// `checkpoint_run_id` is not the source of the next model call. A quiescent
/// log resumes with zero steps.
pub(super) async fn resume_run(
    session: &AgentSession,
    checkpoint_run_id: &str,
) -> Result<crate::agent::AgentResult> {
    let _lease = ExecutionCoordinator::admit(session, "resume-run").await?;
    let Some(checkpoint) = load_resume_checkpoint_if_present(session, checkpoint_run_id).await?
    else {
        let pinned = prepare_pinned(session, "", false).await?;
        let settled = async {
            let run = pinned.parts.open()?;
            run.resume_limit(32).await
        }
        .await;
        let messages = pinned.parts.transcript();
        let usage = pinned.parts.usage();
        pinned.close().await;
        let settled = settled?;
        return Ok(agent_result(&settled, messages, usage));
    };
    let resume_prompt = checkpoint
        .messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| message.text())
        .unwrap_or_default();
    let pinned = prepare_pinned(session, &resume_prompt, true).await?;
    let settled = async {
        let run = pinned.parts.open()?;
        let mut seed = checkpoint.messages.clone();
        if seed
            .last()
            .is_some_and(|message| message.role == "assistant")
        {
            seed.pop();
        }
        run.reset_thread_log()?;
        run.seed_history(&seed)?;
        run.resume_limit(32).await
    }
    .await;
    let messages = pinned.parts.transcript();
    let mut usage = pinned.parts.usage();
    usage.prompt_tokens = usage
        .prompt_tokens
        .saturating_add(checkpoint.total_usage.prompt_tokens);
    usage.completion_tokens = usage
        .completion_tokens
        .saturating_add(checkpoint.total_usage.completion_tokens);
    usage.total_tokens = usage
        .total_tokens
        .saturating_add(checkpoint.total_usage.total_tokens);
    let settled = settled?;
    let mut result = agent_result(&settled, messages, usage);
    result.tool_calls_count = result
        .tool_calls_count
        .saturating_add(checkpoint.tool_calls_count);
    let gated = gate_result(&pinned.parts, result).map_err(|error| anyhow::anyhow!("{error:#}"));
    pinned.finish.publish(&resume_prompt, &gated, false).await;
    pinned.close().await;
    gated.map_err(|error| CodeError::Session(error.to_string()))
}

async fn load_resume_checkpoint_if_present(
    session: &AgentSession,
    checkpoint_run_id: &str,
) -> Result<Option<crate::loop_checkpoint::LoopCheckpoint>> {
    let Some(store) = session.session_store.as_ref() else {
        return Ok(None);
    };
    let Some(checkpoint) = store
        .load_loop_checkpoint(checkpoint_run_id)
        .await
        .map_err(|error| {
            CodeError::Session(format!(
                "load_loop_checkpoint('{checkpoint_run_id}') failed: {error}"
            ))
        })?
    else {
        return Ok(None);
    };
    checkpoint
        .ensure_owned_by(checkpoint_run_id, &session.session_id)
        .map_err(|error| {
            CodeError::Session(format!(
                "refusing to resume checkpoint '{checkpoint_run_id}': {error:#}"
            ))
        })?;
    Ok(Some(checkpoint))
}

async fn load_resume_checkpoint(
    session: &AgentSession,
    checkpoint_run_id: &str,
) -> Result<crate::loop_checkpoint::LoopCheckpoint> {
    let store = session.session_store.as_ref().ok_or_else(|| {
        CodeError::Session("resume_run requires a session_store on this session".to_string())
    })?;
    let checkpoint = store
        .load_loop_checkpoint(checkpoint_run_id)
        .await
        .map_err(|error| {
            CodeError::Session(format!(
                "load_loop_checkpoint('{checkpoint_run_id}') failed: {error}"
            ))
        })?
        .ok_or_else(|| {
            CodeError::Session(format!(
                "no loop checkpoint found for run '{checkpoint_run_id}'"
            ))
        })?;
    checkpoint
        .ensure_owned_by(checkpoint_run_id, &session.session_id)
        .map_err(|error| {
            CodeError::Session(format!(
                "refusing to resume checkpoint '{checkpoint_run_id}': {error:#}"
            ))
        })?;
    Ok(checkpoint)
}

pub(crate) struct FactSession {
    workspace: PathBuf,
    session_id: String,
    client: Arc<dyn crate::llm::LlmClient>,
    executor: Arc<crate::tools::ToolExecutor>,
    permission: crate::permissions::PermissionPolicy,
    yolo_lanes: Vec<crate::queue::SessionLane>,
    max_tool_rounds: usize,
    surface: Option<crate::fact_control::SessionSurface>,
}

impl FactSession {
    pub(crate) fn from(session: &AgentSession) -> Self {
        Self {
            workspace: session.workspace.clone(),
            session_id: session.session_id.clone(),
            client: Arc::clone(&session.llm_client),
            executor: Arc::clone(&session.tool_executor),
            permission: session.config.permission_policy.clone().unwrap_or_default(),
            yolo_lanes: session
                .config
                .confirmation_policy
                .as_ref()
                .map(|policy| policy.yolo_lanes.iter().copied().collect())
                .unwrap_or_default(),
            max_tool_rounds: session.config.max_tool_rounds,
            surface: None,
        }
    }

    pub(crate) fn with_surface(mut self, surface: crate::fact_control::SessionSurface) -> Self {
        self.surface = Some(surface);
        self
    }

    pub(crate) fn set_events(&mut self, sender: mpsc::Sender<AgentEvent>) {
        if let Some(surface) = &mut self.surface {
            surface.events = Some(sender);
        }
    }

    fn transcript(&self) -> Vec<Message> {
        self.surface
            .as_ref()
            .map(|surface| {
                surface
                    .transcript
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            })
            .unwrap_or_default()
    }

    fn usage(&self) -> crate::llm::TokenUsage {
        self.surface
            .as_ref()
            .map(|surface| {
                surface
                    .usage
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            })
            .unwrap_or_default()
    }

    pub(crate) fn with_executor(mut self, executor: Arc<crate::tools::ToolExecutor>) -> Self {
        self.executor = executor;
        self
    }

    pub(crate) fn open(&self) -> Result<crate::fact_control::FactRun> {
        if let Some(surface) = &self.surface {
            return Ok(crate::fact_control::FactRun::open_projected(
                &self.workspace,
                &self.session_id,
                Arc::clone(&self.client),
                Arc::clone(&self.executor),
                self.permission.clone(),
                &self.yolo_lanes,
                self.max_tool_rounds,
                surface.clone(),
            )?);
        }
        Ok(crate::fact_control::FactRun::open_session(
            &self.workspace,
            &self.session_id,
            Arc::clone(&self.client),
            Arc::clone(&self.executor),
            self.permission.clone(),
            &self.yolo_lanes,
            self.max_tool_rounds,
        )?)
    }
}

struct PinnedFact {
    parts: FactSession,
    capability_run: crate::capability::SessionCapabilityRun,
    committer: Option<JoinHandle<()>>,
    finish: TurnFinish,
}

struct TurnFinish {
    run_store: Arc<crate::run::InMemoryRunStore>,
    hook: Option<Arc<dyn crate::hooks::HookExecutor>>,
    session_id: String,
    persistence: super::session_persistence::SessionPersistenceContext,
    cancel_slot: Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    current_run_id: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl TurnFinish {
    fn from_session(session: &AgentSession) -> Self {
        Self {
            run_store: Arc::clone(&session.run_store),
            hook: session.hook_executor.clone(),
            session_id: session.session_id.clone(),
            persistence: super::session_persistence::SessionPersistenceContext::from_session(
                session,
            ),
            cancel_slot: Arc::clone(&session.cancel_token),
            current_run_id: Arc::clone(&session.current_run_id),
        }
    }

    async fn publish(
        &self,
        prompt: &str,
        outcome: &std::result::Result<AgentResult, anyhow::Error>,
        update_history: bool,
    ) {
        if let Some(run_id) = self.current_run_id.lock().await.clone() {
            let _ = prompt;
            match outcome {
                Ok(result) if result.text.contains("interrupted") => {
                    self.run_store.mark_cancelled(&run_id).await;
                }
                Ok(result) => {
                    let end = AgentEvent::End {
                        text: result.text.clone(),
                        usage: result.usage.clone(),
                        verification_summary: Box::new(
                            crate::verification::VerificationSummary::from_reports(&[]),
                        ),
                        meta: None,
                    };
                    self.run_store.record_event(&run_id, end.clone()).await;
                    self.note(&run_id, &end).await;
                }
                Err(error) => {
                    let event = AgentEvent::Error {
                        message: error.to_string(),
                    };
                    self.run_store.record_event(&run_id, event.clone()).await;
                    self.note(&run_id, &event).await;
                }
            }
            self.persistence.clear_loop_checkpoint(&run_id).await;
        }
        if let Ok(result) = outcome {
            self.persistence.record_usage(&result.usage);
        }
        if update_history {
            if let Ok(result) = outcome {
                if !result.messages.is_empty() {
                    let mut messages = result.messages.clone();
                    strip_attachment_annotation(&mut messages);
                    self.persistence.record_messages(messages);
                    self.persistence.auto_save_if_enabled().await;
                }
            }
        }
        *self.cancel_slot.lock().await = None;
        *self.current_run_id.lock().await = None;
    }

    async fn note(&self, run_id: &str, event: &AgentEvent) {
        if let Some(hook) = &self.hook {
            hook.record_agent_event(event, run_id, &self.session_id)
                .await;
        }
    }
}

impl PinnedFact {
    async fn close(self) {
        let Self {
            parts,
            capability_run,
            committer,
            finish: _,
        } = self;
        drop(parts);
        let _ = capability_run.close().await;
        if let Some(committer) = committer {
            let _ = committer.await;
        }
    }

    async fn conclude(
        self,
        prompt: &str,
        outcome: std::result::Result<a3s_effect::Settlement<a3s_effect::CodingView>, anyhow::Error>,
        update_history: bool,
    ) -> Result<AgentResult> {
        let messages = self.parts.transcript();
        let usage = self.parts.usage();
        let mapped = outcome
            .as_ref()
            .map(|settled| agent_result(settled, messages, usage))
            .map_err(|error| anyhow::anyhow!("{error:#}"))
            .and_then(|result| gate_result(&self.parts, result));
        if let Ok(result) = &mapped {
            if let Some(surface) = self.parts.surface.as_ref() {
                surface
                    .agent
                    .fact_extract_turn_memory(
                        prompt,
                        &result.text,
                        &surface.session_id,
                        &surface.cancel,
                    )
                    .await;
            }
        }
        self.finish.publish(prompt, &mapped, update_history).await;
        let Self {
            parts,
            capability_run,
            committer,
            finish: _,
        } = self;
        drop(parts);
        let _ = capability_run.close().await;
        if let Some(committer) = committer {
            let _ = committer.await;
        }
        let mut result = mapped.map_err(|error| CodeError::Session(error.to_string()))?;
        if update_history {
            strip_attachment_annotation(&mut result.messages);
        }
        Ok(result)
    }
}

fn strip_attachment_annotation(messages: &mut [Message]) {
    let Some(message) = messages.first_mut() else {
        return;
    };
    if message.role != "user" {
        return;
    }
    let text = message.text();
    let Some(visible) = text.split("\n[attachment ").next() else {
        return;
    };
    if visible != text {
        *message = Message::user(visible);
    }
}

fn tool_results_since_last_user_message(log: &[a3s_effect::Fact]) -> usize {
    let start = log
        .iter()
        .rposition(|fact| fact.kind == "user.message")
        .unwrap_or(0);
    log[start..]
        .iter()
        .filter(|fact| fact.kind == "tool.result")
        .count()
}

fn gate_result(
    parts: &FactSession,
    mut result: AgentResult,
) -> std::result::Result<AgentResult, anyhow::Error> {
    let Some(surface) = &parts.surface else {
        return Ok(result);
    };
    let ledger = surface
        .ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let reports = surface
        .reports
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    result.verification_reports = reports.clone();
    match surface.agent.fact_completion_gate(&ledger, &reports) {
        crate::harness_loop::CompletionGate::Allow(terminal) => {
            result.completion = terminal;
            Ok(result)
        }
        crate::harness_loop::CompletionGate::Incomplete { message }
        | crate::harness_loop::CompletionGate::Continue { message } => {
            Err(anyhow::anyhow!(message))
        }
    }
}

fn agent_result(
    settled: &a3s_effect::Settlement<a3s_effect::CodingView>,
    messages: Vec<Message>,
    usage: crate::llm::TokenUsage,
) -> AgentResult {
    AgentResult {
        text: settled.view.assistant.clone().unwrap_or_default(),
        messages,
        usage,
        tool_calls_count: tool_results_since_last_user_message(&settled.log),
        verification_reports: Vec::new(),
        completion: crate::harness_loop::CompletionTerminal::Narrative,
        run_admission: "ordinary".into(),
    }
}

fn prompt_with_attachments(prompt: &str, attachments: &[Attachment]) -> String {
    if attachments.is_empty() {
        return prompt.to_string();
    }
    let mut text = prompt.to_string();
    for (index, attachment) in attachments.iter().enumerate() {
        text.push_str(&format!(
            "\n[attachment {} {} {} bytes]",
            index + 1,
            attachment.media_type,
            attachment.data.len()
        ));
    }
    text
}

async fn bind_external_run(session: &AgentSession, pinned: &mut PinnedFact, run_id: &str) {
    if let Some(surface) = pinned.parts.surface.as_mut() {
        surface.run_store = Some(Arc::clone(&session.run_store));
        surface.run_id = Some(run_id.to_string());
        *session.cancel_token.lock().await = Some(surface.cancel.clone());
    }
    *session.current_run_id.lock().await = Some(run_id.to_string());
}

async fn prepare_pinned(
    session: &AgentSession,
    prompt: &str,
    owned_run: bool,
) -> Result<PinnedFact> {
    let cancellation = session.session_cancel.child_token();
    let (mut agent_loop, capability_run) =
        build_pinned_agent_loop(session, cancellation.clone()).await?;
    let mut committer = None;
    let mut checkpoint = None;
    let mut active_run_id = None;
    let mut run_control = None;
    if owned_run {
        let Some(binding) = agent_loop.checkpoint_capability_binding().cloned() else {
            let _ = capability_run.close().await;
            return Err(CodeError::Session(
                "scoped Agent Run has no capability binding".into(),
            ));
        };
        let cognitive = cognitive_binding_for_projection(session, capability_run.projection());
        let run = match RunControlState::from_session(session)
            .start_run_with_bindings(prompt, cognitive.clone(), binding.clone())
            .await
        {
            Ok(run) => {
                active_run_id = Some(run.id().to_string());
                run
            }
            Err(error) => {
                let _ = capability_run.close().await;
                return Err(error);
            }
        };
        if let Some(binding) = cognitive {
            session
                .run_store
                .record_event(
                    run.id(),
                    crate::agent::AgentEvent::CognitiveContextBound { binding },
                )
                .await;
        }
        let started = AgentEvent::Start {
            prompt: prompt.to_string(),
        };
        session
            .run_store
            .record_event(run.id(), started.clone())
            .await;
        if let Some(hook) = &session.hook_executor {
            hook.record_agent_event(&started, run.id(), &session.session_id)
                .await;
        }
        let (checkpoint_sink, mut checkpoints) = runtime_checkpoint_channel(session);
        if let Some(sink) = checkpoint_sink {
            checkpoint = Some(crate::fact_control::CheckpointEmit {
                sink,
                run_id: run.id().to_string(),
                session_id: session.session_id.clone(),
                capability_binding: Some(binding),
            });
            committer = Some(tokio::spawn(async move {
                while let Some(boundary) = checkpoints.recv().await {
                    checkpoints.commit(boundary).await;
                }
            }));
        } else {
            drop(checkpoints);
        }
        let prepared =
            ExecutionCoordinator::prepare(session, run.id(), &mut agent_loop, cancellation.clone())
                .await;
        prepared.run_control().update_turn(1).await;
        run_control = Some(prepared.run_control());
    }
    let parts = FactSession::from(session)
        .with_executor(agent_loop.tool_executor_handle())
        .with_surface(crate::fact_control::SessionSurface {
            agent: agent_loop,
            session_id: session.session_id.clone(),
            checkpoint,
            events: None,
            cancel: cancellation,
            transcript: Arc::new(std::sync::Mutex::new(Vec::new())),
            usage: Arc::new(std::sync::Mutex::new(crate::llm::TokenUsage::default())),
            confirmation: session.config.confirmation_manager.clone(),
            run_store: active_run_id
                .as_ref()
                .map(|_| Arc::clone(&session.run_store)),
            run_id: active_run_id,
            ledger: Arc::new(std::sync::Mutex::new(
                crate::harness_loop::MutationLedger::default(),
            )),
            reports: Arc::new(std::sync::Mutex::new(Vec::new())),
            run_control,
        });
    if owned_run {
        if let Some(cancel) = parts.surface.as_ref().map(|surface| surface.cancel.clone()) {
            *session.cancel_token.lock().await = Some(cancel);
        }
    }
    Ok(PinnedFact {
        parts,
        capability_run,
        committer,
        finish: TurnFinish::from_session(session),
    })
}

async fn settle_prompt(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
) -> Result<AgentResult> {
    let update_history = history.is_none();
    let pinned = prepare_pinned(session, prompt, true).await?;
    let outcome = async {
        let run = pinned.parts.open()?;
        if let Some(history) = history {
            run.seed_history(history)?;
        }
        prepare_pre_analysis(&pinned.parts, prompt).await?;
        let settled = run.user_text(prompt).await?;
        settle_confirmations(&run, settled, &pinned.parts).await
    }
    .await;
    pinned.conclude(prompt, outcome, update_history).await
}

async fn prepare_pre_analysis(parts: &FactSession, prompt: &str) -> anyhow::Result<()> {
    let Some(surface) = &parts.surface else {
        return Ok(());
    };
    surface
        .agent
        .fact_pre_analysis(prompt, &surface.session_id, &surface.cancel)
        .await
        .map_err(pre_analysis_error)?;
    surface
        .agent
        .fact_publish_plan(
            prompt,
            &surface.session_id,
            &surface.events,
            &surface.cancel,
            surface.run_store.as_deref(),
            surface.run_id.as_deref(),
        )
        .await
        .map_err(pre_analysis_error)
}

fn pre_analysis_error(error: anyhow::Error) -> anyhow::Error {
    if let Some(message) = crate::llm::non_retryable_llm_error_message(&error) {
        anyhow::anyhow!(message.to_string())
    } else {
        error
    }
}

async fn settle_confirmations(
    run: &crate::fact_control::FactRun,
    mut settled: a3s_effect::Settlement<a3s_effect::CodingView>,
    parts: &FactSession,
) -> anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>> {
    let Some(surface) = &parts.surface else {
        return Ok(settled);
    };
    while settled.view.phase == a3s_effect::CodingPhase::Confirm {
        let Some(pending) = settled.view.pending_confirmation.clone() else {
            break;
        };
        let approved = await_host_confirmation(surface, &pending).await;
        if surface.cancel.is_cancelled() {
            let _ = run.confirm(&pending.tool_call_id, false).await;
            break;
        }
        settled = run.confirm(&pending.tool_call_id, approved).await?;
    }
    while settled.view.phase == a3s_effect::CodingPhase::Question {
        let Some(pending) = settled.view.pending_question.clone() else {
            break;
        };
        if let Some(sender) = &surface.events {
            let _ = sender
                .send(crate::agent::AgentEvent::UserQuestion {
                    question_id: pending.question_id.clone(),
                    question: pending.question.clone(),
                    options: pending.options.clone(),
                    allow_free_text: pending.allow_free_text,
                })
                .await;
        }
        let Some(text) =
            crate::ask_user::wait_for_answer(&pending.question_id, &surface.cancel).await
        else {
            break;
        };
        let output = serde_json::json!({
            "schema": crate::ask_user::USER_ANSWER_SCHEMA,
            "question_id": pending.question_id,
            "status": "answered",
            "answer": text,
            "permission_grant": false,
            "wrote_files": false,
        })
        .to_string();
        if let Some(sender) = &surface.events {
            let _ = sender
                .send(crate::agent::AgentEvent::ToolEnd {
                    id: pending.question_id.clone(),
                    name: "ask_user".into(),
                    args: Some(serde_json::json!({
                        "question": pending.question,
                        "options": pending.options,
                        "allow_free_text": pending.allow_free_text,
                    })),
                    output: output.clone(),
                    exit_code: 0,
                    metadata: Some(serde_json::json!({
                        "permission_grant": false,
                        "wrote_files": false,
                    })),
                    error_kind: None,
                })
                .await;
        }
        settled = run.answer(&output).await?;
    }
    Ok(settled)
}

async fn await_host_confirmation(
    surface: &crate::fact_control::SessionSurface,
    pending: &a3s_effect::PendingConfirmation,
) -> bool {
    let Some(manager) = &surface.confirmation else {
        return false;
    };
    let receiver = manager
        .request_confirmation(&pending.tool_call_id, &pending.name, &pending.args)
        .await;
    if let Some(sender) = &surface.events {
        let _ = sender
            .send(AgentEvent::ConfirmationRequired {
                tool_id: pending.tool_call_id.clone(),
                tool_name: pending.name.clone(),
                args: pending.args.clone(),
                timeout_ms: 0,
            })
            .await;
    }
    let reason = Some("Confirmation cancelled".to_string());
    tokio::select! {
        biased;
        _ = surface.cancel.cancelled() => {
            let _ = manager
                .confirm(&pending.tool_call_id, false, reason.clone())
                .await;
            if let Some(sender) = &surface.events {
                let _ = sender
                    .send(AgentEvent::ConfirmationReceived {
                        tool_id: pending.tool_call_id.clone(),
                        approved: false,
                        reason,
                    })
                    .await;
            }
            false
        }
        response = receiver => {
            let response = response.unwrap_or(crate::hitl::ConfirmationResponse {
                approved: false,
                reason: reason.clone(),
            });
            if let Some(sender) = &surface.events {
                let _ = sender
                    .send(AgentEvent::ConfirmationReceived {
                        tool_id: pending.tool_call_id.clone(),
                        approved: response.approved,
                        reason: response.reason.clone(),
                    })
                    .await;
            }
            response.approved
        }
    }
}

fn exact_recovery_prompt(checkpoint_identity: &str) -> String {
    format!("<resume exact checkpoint={checkpoint_identity}>")
}

fn drain_detached_events(
    mut events: mpsc::Receiver<AgentEvent>,
    worker: JoinHandle<()>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while events.recv().await.is_some() {}
        let _ = worker.await;
    })
}

async fn exact_run_replay(
    session: &AgentSession,
    run_id: &str,
    prompt: &str,
) -> Result<Option<AgentRunSpawn>> {
    let Some(snapshot) = session.run_snapshot(run_id).await else {
        return Ok(None);
    };
    if snapshot.session_id != session.session_id || snapshot.prompt != prompt {
        return Err(CodeError::RunIdentityConflict {
            run_id: run_id.to_string(),
        });
    }
    Ok(Some(AgentRunSpawn::Replayed { snapshot }))
}

fn warn_deferred_init(session: &AgentSession) {
    if let Some(warning) = &session.init_warning {
        tracing::warn!(
            session_id = %session.session_id,
            "Session init warning: {}", warning
        );
    }
}
