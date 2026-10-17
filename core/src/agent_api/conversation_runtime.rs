//! Conversation execution facade for a session.
//!
//! This module owns the public conversation contract: slash-command dispatch,
//! blocking sends, streaming sends, and attachment handling.
//! Lower-level runtime modules own run lifecycle and event forwarding.

#[cfg(test)]
use super::{
    agent_loop_runtime::{build_pinned_agent_loop, cognitive_binding_for_projection},
    runtime_checkpoints::runtime_checkpoint_channel,
};
use super::{
    command_runtime, execution_coordinator::ExecutionCoordinator, run_admission,
    run_lifecycle::RunControlState, AgentRunSpawn, AgentRunStream, AgentSession,
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

mod host_events;
mod owned_fact;
mod owned_launch;
mod owned_worker;
mod recovery_support;
mod turn_outcome;
use host_events::*;
use owned_fact::*;
use owned_launch::*;
use owned_worker::*;
pub(crate) use recovery_support::exact_run_replay;
use recovery_support::*;
use turn_outcome::*;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExactRecoveryError {
    #[error(transparent)]
    Checkpoint(#[from] SessionCheckpointError),
    #[error(transparent)]
    Code(#[from] CodeError),
}

pub(crate) enum ExactRecoveryPreparation {
    Replayed(Box<AgentRunSpawn>),
    Ready(Box<PreparedExactRecovery>),
}

pub(crate) struct PreparedExactRecovery {
    checkpoint: LoopCheckpoint,
    evidence: SessionLogicalResumeEvidenceV1,
    run_id: String,
    prompt: String,
    lease: run_admission::RunAdmissionLease,
}

/// Typed automatic origin; the original FactRun re-validates it before admission.
pub(crate) type GoalTickOrigin = (
    crate::goal_control::GoalControlCommandV1,
    crate::goal_control::ContinuationSource,
);

pub(super) async fn send(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
) -> Result<AgentResult> {
    let lease = ExecutionCoordinator::admit(session, "send").await?;
    observe_owned_blocking(launch_owned_conversation(
        session,
        lease,
        OwnedConversationRequest::Prompt {
            prompt: prompt.to_owned(),
            attachments: Vec::new(),
            history: history.map(<[Message]>::to_vec),
            command_capable: true,
            exact_run_id: None,
            goal_tick: None,
        },
        run_admission::ExecutionMode::Blocking,
        true,
    )?)
    .await
}

pub(super) async fn send_with_attachments(
    session: &AgentSession,
    prompt: &str,
    attachments: &[Attachment],
    history: Option<&[Message]>,
) -> Result<AgentResult> {
    reject_oversized_attachments(attachments)?;
    let lease = ExecutionCoordinator::admit(session, "send-with-attachments").await?;
    observe_owned_blocking(launch_owned_conversation(
        session,
        lease,
        OwnedConversationRequest::Prompt {
            prompt: prompt.to_owned(),
            attachments: attachments.to_vec(),
            history: history.map(<[Message]>::to_vec),
            command_capable: false,
            exact_run_id: None,
            goal_tick: None,
        },
        run_admission::ExecutionMode::Blocking,
        false,
    )?)
    .await
}

pub(super) async fn stream_with_attachments(
    session: &AgentSession,
    prompt: &str,
    attachments: &[Attachment],
    history: Option<&[Message]>,
) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
    reject_oversized_attachments(attachments)?;
    let lease = ExecutionCoordinator::admit(session, "stream-with-attachments").await?;
    let actual = observe_owned_stream(launch_owned_conversation(
        session,
        lease,
        OwnedConversationRequest::Prompt {
            prompt: prompt.to_owned(),
            attachments: attachments.to_vec(),
            history: history.map(<[Message]>::to_vec),
            command_capable: false,
            exact_run_id: None,
            goal_tick: None,
        },
        run_admission::ExecutionMode::Streaming,
        false,
    )?)
    .await?;
    Ok((actual.events, actual.worker))
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
) -> Result<AgentRunStream> {
    let lease = ExecutionCoordinator::admit(session, "stream").await?;
    observe_owned_stream(launch_owned_conversation(
        session,
        lease,
        OwnedConversationRequest::Prompt {
            prompt: prompt.to_owned(),
            attachments: Vec::new(),
            history: history.map(<[Message]>::to_vec),
            command_capable: true,
            exact_run_id: None,
            goal_tick: None,
        },
        run_admission::ExecutionMode::Streaming,
        true,
    )?)
    .await
}

/// Start one detached Code run at an exact host-selected identity.
pub(super) async fn spawn_run_with_id(
    session: &AgentSession,
    run_id: &str,
    prompt: &str,
) -> Result<AgentRunSpawn> {
    spawn_exact_run(session, run_id, prompt, None).await
}

/// Start the one original Run an automatic tick may admit, named by its operation.
/// The prompt names the step so earlier Runs in the history cannot stand in for it.
pub(super) async fn spawn_goal_tick(
    session: &AgentSession,
    command: crate::goal_control::GoalControlCommandV1,
    source: crate::goal_control::ContinuationSource,
    step_id: &str,
) -> Result<AgentRunSpawn> {
    let run_id = command.operation_id.clone();
    let prompt = goal_tick_prompt(step_id);
    spawn_exact_run(session, &run_id, &prompt, Some(Box::new((command, source)))).await
}

/// The same detached goal slice as [`spawn_goal_tick`]; its events reach the
/// host instead of being drained, so it can render and answer confirmations.
pub(super) async fn stream_goal_tick(
    session: &AgentSession,
    command: crate::goal_control::GoalControlCommandV1,
    source: crate::goal_control::ContinuationSource,
    step_id: &str,
) -> Result<super::GoalTickStream> {
    let run_id = command.operation_id.clone();
    let prompt = goal_tick_prompt(step_id);
    match launch_exact_run(session, &run_id, &prompt, Some(Box::new((command, source)))).await? {
        Ok(observation) => observe_owned_stream(observation)
            .await
            .map(super::GoalTickStream::Started),
        Err(snapshot) => Ok(super::GoalTickStream::Replayed(snapshot)),
    }
}

fn goal_tick_prompt(step_id: &str) -> String {
    format!("Execute only the current admitted step `{step_id}` using fresh evidence.")
}

async fn spawn_exact_run(
    session: &AgentSession,
    run_id: &str,
    prompt: &str,
    goal_tick: Option<Box<GoalTickOrigin>>,
) -> Result<AgentRunSpawn> {
    match launch_exact_run(session, run_id, prompt, goal_tick).await? {
        Ok(observation) => observe_owned_detached(observation).await,
        Err(snapshot) => Ok(AgentRunSpawn::Replayed { snapshot }),
    }
}

/// Launch a detached exact Run, or return the snapshot it already replays.
async fn launch_exact_run(
    session: &AgentSession,
    run_id: &str,
    prompt: &str,
    goal_tick: Option<Box<GoalTickOrigin>>,
) -> Result<std::result::Result<OwnedObservation, crate::run::RunSnapshot>> {
    if let Some(AgentRunSpawn::Replayed { snapshot } | AgentRunSpawn::Started { snapshot, .. }) =
        exact_run_replay(session, run_id, prompt).await?
    {
        return Ok(Err(snapshot));
    }
    let lease = ExecutionCoordinator::admit(session, "spawn-run").await?;
    launch_owned_conversation(
        session,
        lease,
        OwnedConversationRequest::Prompt {
            prompt: prompt.to_owned(),
            attachments: Vec::new(),
            history: None,
            command_capable: false,
            exact_run_id: Some(run_id.to_owned()),
            goal_tick,
        },
        run_admission::ExecutionMode::Detached,
        false,
    )
    .map(Ok)
}

/// Resume one durable loop checkpoint into an exact fresh run identity.
pub(super) async fn spawn_recovery_with_run_id(
    session: &AgentSession,
    checkpoint_run_id: &str,
    run_id: &str,
) -> Result<AgentRunSpawn> {
    ensure_distinct_recovery_target(checkpoint_run_id, run_id)?;
    let prompt = format!("<resume run={checkpoint_run_id}>");
    if let Some(replay) = exact_run_replay(session, run_id, &prompt).await? {
        return Ok(replay);
    }
    let lease = ExecutionCoordinator::admit(session, "spawn-recovery").await?;
    observe_owned_detached(launch_owned_conversation(
        session,
        lease,
        OwnedConversationRequest::ExactRecovery {
            source: checkpoint_run_id.to_owned(),
            target: run_id.to_owned(),
            prompt,
        },
        run_admission::ExecutionMode::Detached,
        false,
    )?)
    .await
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
    ensure_distinct_recovery_target(&evidence.source_run_id, run_id)?;

    let prompt = exact_recovery_prompt(checkpoint_identity);
    if let Some(replay) = exact_run_replay(session, run_id, &prompt).await? {
        return Ok(ExactRecoveryPreparation::Replayed(Box::new(replay)));
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
    super::session_persistence::SessionPersistenceContext::from_session(session)
        .validate_checkpoint_source(&checkpoint)?;
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
    Ok(ExactRecoveryPreparation::Ready(Box::new(
        PreparedExactRecovery {
            checkpoint,
            evidence: evidence.clone(),
            run_id: run_id.to_string(),
            prompt,
            lease,
        },
    )))
}

/// Admit a previously validated immutable recovery plan and start its worker.
pub(super) async fn spawn_prepared_recovery(
    session: &AgentSession,
    prepared: PreparedExactRecovery,
) -> std::result::Result<AgentRunSpawn, ExactRecoveryError> {
    if !prepared.lease.belongs_to(&session.run_admission)
        || prepared.evidence.session_id != session.session_id
    {
        return Err(CodeError::Session(
            "prepared recovery belongs to another original Session admission".into(),
        )
        .into());
    }
    let PreparedExactRecovery {
        checkpoint,
        evidence,
        run_id,
        prompt,
        lease,
    } = prepared;
    ensure_distinct_recovery_target(&checkpoint.run_id, &run_id)?;
    checkpoint
        .ensure_owned_by(&evidence.source_run_id, &session.session_id)
        .map_err(|error| {
            SessionCheckpointError::InvalidPayload(format!(
                "refusing prepared checkpoint: {error:#}"
            ))
        })?;
    evidence.validate_for(&checkpoint)?;
    super::session_persistence::SessionPersistenceContext::from_session(session)
        .validate_checkpoint_source(&checkpoint)?;
    if let Some(binding) = &checkpoint.capability_binding {
        super::agent_loop_runtime::validate_run_capability_binding(session, binding).map_err(|error| match error {
            crate::capability::RunCapabilityBindingError::ContentDrift { .. } => SessionCheckpointError::ContentDrift(format!("the source Run capability generation is unavailable on this Session: {error}")),
            _ => SessionCheckpointError::InvalidPayload(format!("the source Run capability binding is invalid: {error}")),
        })?;
    }
    Ok(observe_owned_detached(launch_owned_conversation(
        session,
        lease,
        OwnedConversationRequest::PreparedRecovery {
            checkpoint: Box::new(checkpoint),
            target: run_id,
            prompt,
        },
        run_admission::ExecutionMode::Detached,
        false,
    )?)
    .await?)
}

/// Resume the session fact log.
///
/// `checkpoint_run_id` is not the source of the next model call. A quiescent
/// log resumes with zero steps.
pub(super) async fn resume_run(
    session: &AgentSession,
    checkpoint_run_id: &str,
) -> Result<AgentResult> {
    let lease = ExecutionCoordinator::admit(session, "resume-run").await?;
    observe_owned_blocking(launch_owned_conversation(
        session,
        lease,
        OwnedConversationRequest::OrdinaryResume {
            source: checkpoint_run_id.to_owned(),
        },
        run_admission::ExecutionMode::Blocking,
        false,
    )?)
    .await
}

/// Build the owed-work directive for a turn carrying unsettled mutations,
/// or `None` when the ledger is clean. Shared by the checkpoint resume
/// (appended to the replayed prompt) and the normal send stream (injected
/// as guidance before the first model call) so every path into a debt
/// carrying turn states the same facts: what is owed, that narrative
/// completion will be refused, and the EXACT declared suite commands —
/// a suffixed variant does not bind.
fn owed_work_directive(
    owed: &[String],
    contracts: &[crate::verification::VerificationContractV1],
) -> Option<String> {
    if owed.is_empty() {
        return None;
    }
    let mut paths = owed.to_vec();
    paths.sort_unstable();
    paths.dedup();
    let suite_hint = if contracts.is_empty() {
        String::new()
    } else {
        let commands = contracts
            .iter()
            .map(|contract| format!("`{}`", contract.command))
            .collect::<Vec<_>>()
            .join(" or ");
        format!(
            " Run exactly {commands} (a modified or suffixed variant does not count) and let it exit 0."
        )
    };
    Some(format!(
        "[owed work] Earlier turns left workspace mutations that are not \
         verified yet: {}. A narrative claim of completion will be refused —\
         re-run the project's verification{suite_hint} before answering.",
        paths.join(", ")
    ))
}

/// Restore the interrupted run's mutation ledger and verification reports
/// onto the resumed surface.
///
/// The fact spine rebuilds the conversation from the workspace log, but the
/// mutation ledger is process-local surface state: without this seed a
/// resumed run's completion gate is blind to the mutations the interrupted
/// run already landed, and a resumed long task could settle `Narrative`
/// over a workspace effect it never accounted for.
#[cfg(test)]
fn seed_resumed_mutation_state(
    pinned: &mut PinnedFact,
    checkpoint: &LoopCheckpoint,
) -> anyhow::Result<()> {
    seed_resumed_mutation_parts(&pinned.parts, checkpoint)
}

fn seed_resumed_mutation_parts(
    parts: &FactSession,
    checkpoint: &LoopCheckpoint,
) -> anyhow::Result<()> {
    let Some(surface) = parts.surface.as_ref() else {
        return Ok(());
    };
    let coverage = crate::fact_control::restore_checkpoint_mutation_cache(surface, checkpoint);
    if !checkpoint.verification_reports.is_empty() {
        let mut reports = surface
            .reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for report in &checkpoint.verification_reports {
            if !reports.contains(report) {
                reports.push(report.clone());
            }
        }
    }
    if coverage == crate::fact_control::CheckpointMutationCacheCoverage::Unproved {
        let run = parts.open().map_err(|error| {
            anyhow::anyhow!("tool_effect_recovery_unproved_append_failed: {error:#}")
        })?;
        run.append_recovery_unproved(checkpoint, &surface.session_id)?;
        crate::fact_control::replay_unsettled_mutations(surface);
    }
    // Cached reports remain diagnostic. The negative host input, when
    // required, is committed before recovery can dispatch a model or tool.
    Ok(())
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
    client: Option<Arc<dyn crate::llm::LlmClient>>,
    executor: Arc<crate::tools::ToolExecutor>,
    permission: crate::permissions::PermissionPolicy,
    yolo_lanes: Vec<crate::queue::SessionLane>,
    max_tool_rounds: usize,
    surface: Option<crate::fact_control::SessionSurface>,
    original_model: Option<super::OriginalModelCallPort>,
    fact_log: Arc<std::sync::Mutex<Option<Arc<a3s_effect::FileLog>>>>,
}

impl FactSession {
    pub(crate) fn from(session: &AgentSession) -> Self {
        let permission = session
            .runtime_permission_policy()
            .or_else(|| session.config.permission_policy.clone())
            .unwrap_or_default();
        Self::from_permission(session, permission)
    }

    fn from_permission(
        session: &AgentSession,
        permission: crate::permissions::PermissionPolicy,
    ) -> Self {
        Self {
            workspace: session.workspace.clone(),
            session_id: session.session_id.clone(),
            client: Some(Arc::clone(&session.llm_client)),
            executor: Arc::clone(&session.tool_executor),
            permission,
            yolo_lanes: session
                .config
                .confirmation_policy
                .as_ref()
                .map(|policy| policy.yolo_lanes.iter().copied().collect())
                .unwrap_or_default(),
            max_tool_rounds: session.config.max_tool_rounds,
            surface: None,
            original_model: None,
            fact_log: session.run_admission.fact_log_slot(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_surface(mut self, surface: crate::fact_control::SessionSurface) -> Self {
        self.surface = Some(surface);
        self
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

    #[cfg(test)]
    pub(crate) fn with_executor(mut self, executor: Arc<crate::tools::ToolExecutor>) -> Self {
        self.executor = executor;
        self
    }

    fn retire_fact_host_events(&mut self) {
        self.client.take();
        if let Some(surface) = &mut self.surface {
            surface.agent.retire_fact_host_events();
        }
    }

    fn retire_checkpoint_senders(&mut self) {
        if let Some(surface) = &mut self.surface {
            surface.checkpoint.take();
            surface.agent.checkpoint_sink.take();
        }
    }

    pub(crate) fn open(&self) -> Result<crate::fact_control::FactRun> {
        let client = self.client.as_ref().ok_or_else(|| {
            CodeError::Session("completed fact model facade has been retired".into())
        })?;
        if let Some(surface) = &self.surface {
            return Ok(
                crate::fact_control::FactRun::open_projected_with_original_model(
                    &self.workspace,
                    &self.session_id,
                    Arc::clone(client),
                    Arc::clone(&self.executor),
                    (
                        self.permission.clone(),
                        &self.yolo_lanes,
                        self.max_tool_rounds,
                    ),
                    surface.clone(),
                    self.original_model.clone(),
                )?,
            );
        }
        Ok(crate::fact_control::FactRun::open_session(
            &self.workspace,
            &self.session_id,
            Arc::clone(client),
            Arc::clone(&self.executor),
            self.permission.clone(),
            &self.yolo_lanes,
            self.max_tool_rounds,
        )?)
    }
}

#[cfg(test)]
struct PinnedFact {
    parts: FactSession,
    capability_run: crate::capability::SessionCapabilityRun,
    committer: Option<JoinHandle<()>>,
}

struct TurnFinish {
    run_store: Arc<crate::run::InMemoryRunStore>,
    hook: Option<Arc<dyn crate::hooks::HookExecutor>>,
    session_id: String,
    persistence: super::session_persistence::SessionPersistenceContext,
    #[cfg(test)]
    cancel_slot: Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    #[cfg(test)]
    current_run_id: Arc<tokio::sync::Mutex<Option<String>>>,
    #[cfg(test)]
    cancellation: tokio_util::sync::CancellationToken,
    #[cfg(test)]
    run_id: Option<String>,
}

impl TurnFinish {
    fn from_session(
        session: &AgentSession,
        _cancellation: tokio_util::sync::CancellationToken,
        _run_id: Option<String>,
    ) -> Self {
        Self {
            run_store: Arc::clone(&session.run_store),
            hook: session.hook_executor.clone(),
            session_id: session.session_id.clone(),
            persistence: super::session_persistence::SessionPersistenceContext::from_session(
                session,
            ),
            #[cfg(test)]
            cancel_slot: Arc::clone(&session.cancel_token),
            #[cfg(test)]
            current_run_id: Arc::clone(&session.current_run_id),
            #[cfg(test)]
            cancellation: _cancellation,
            #[cfg(test)]
            run_id: _run_id,
        }
    }

    #[cfg(test)]
    async fn publish(
        &self,
        prompt: &str,
        outcome: &std::result::Result<AgentResult, anyhow::Error>,
        update_history: bool,
    ) {
        if let Some(run_id) = self.run_id.as_deref() {
            let _ = prompt;
            // The admitted invocation owns this token. The mutable Session
            // slot may already be cleared, and answer wording has no control
            // authority. Preserve cancellation even when cleanup returns Err.
            let cancelled = self.cancellation.is_cancelled();
            if cancelled {
                self.run_store
                    .settle_terminal(run_id, crate::run::RunTerminalTransition::Cancelled)
                    .await;
            }
            match outcome {
                Ok(_) if cancelled => {}
                Ok(result) => {
                    let end = AgentEvent::End {
                        text: result.text.clone(),
                        usage: result.usage.clone(),
                        verification_summary: Box::new(
                            crate::verification::VerificationSummary::from_reports(
                                &result.verification_reports,
                            ),
                        ),
                        completion_terminal: result.completion.clone(),
                        forced_stop: result.forced_stop,
                        meta: None,
                    };
                    self.run_store.record_event(run_id, end.clone()).await;
                    self.note(run_id, &end).await;
                }
                Err(error) => {
                    let event = AgentEvent::Error {
                        message: error.to_string(),
                    };
                    self.run_store.record_event(run_id, event.clone()).await;
                    self.note(run_id, &event).await;
                }
            }
            self.persistence.clear_loop_checkpoint(run_id).await;
        }
        if let Ok(result) = outcome {
            self.persistence.record_usage(&result.usage);
        }
        if let Ok(result) = outcome {
            if update_history && !result.messages.is_empty() {
                let mut messages = result.messages.clone();
                strip_attachment_annotation(&mut messages);
                self.persistence.record_messages(messages);
            }
            self.persistence.auto_save_if_enabled().await;
        }
        *self.cancel_slot.lock().await = None;
        let mut current = self.current_run_id.lock().await;
        if current.as_deref() == self.run_id.as_deref() {
            *current = None;
        }
    }

    async fn note(&self, run_id: &str, event: &AgentEvent) {
        if let Some(hook) = &self.hook {
            hook.record_agent_event(event, run_id, &self.session_id)
                .await;
        }
    }
}

#[cfg(test)]
impl PinnedFact {
    async fn close(self) {
        let Self {
            parts,
            capability_run,
            committer,
            ..
        } = self;
        drop(parts);
        let _ = capability_run.close().await;
        if let Some(committer) = committer {
            let _ = committer.await;
        }
    }
}

#[cfg(test)]
mod receipt_restore_tests;

#[cfg(test)]
mod typed_interruption_tests;

#[cfg(test)]
mod memory_retention_tests;

#[cfg(test)]
mod owner_checkpoint_tests;

#[cfg(test)]
mod owner_lifetime_tests;

#[cfg(test)]
mod steer_terminal_tests;

#[cfg(test)]
mod steer_lifecycle_tests;

#[cfg(test)]
mod stream_usage_tests;

#[cfg(test)]
mod host_event_tests;
