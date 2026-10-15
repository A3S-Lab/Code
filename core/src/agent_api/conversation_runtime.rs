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

// A concrete request remains in the original worker through preparation. It is
// not a future/job hosting port and carries no Session or admission authority.
enum OwnedConversationRequest {
    Prompt {
        prompt: String,
        attachments: Vec<Attachment>,
        history: Option<Vec<Message>>,
        command_capable: bool,
        exact_run_id: Option<String>,
        goal_tick: Option<Box<GoalTickOrigin>>,
    },
    ExactRecovery {
        source: String,
        target: String,
        prompt: String,
    },
    PreparedRecovery {
        checkpoint: Box<LoopCheckpoint>,
        target: String,
        prompt: String,
    },
    OrdinaryResume {
        source: String,
    },
}

struct OwnedConversationCapture {
    preparation: super::agent_loop_runtime::OwnedPreparationCapture,
    coordinator: super::execution_coordinator::OwnedCoordinatorInputs,
    controls: RunControlState,
    cleanup: super::runtime_events::RunCleanupState,
    finish: TurnFinish,
    parts: FactSession,
    config: crate::agent::AgentConfig,
    commands: command_runtime::OwnedCommandInputs,
    init_warning: Option<String>,
    /// Host reports already on the session when this turn was captured.
    /// Each send builds a fresh fact surface, and the completion gate reads
    /// that surface. A `record_verification_reports` between turns is visible
    /// only when this snapshot seeds `surface.reports`.
    recorded_reports: Vec<crate::verification::VerificationReport>,
}

impl OwnedConversationCapture {
    fn capture(session: &AgentSession, command_capable: bool) -> Result<Self> {
        let cancellation = session.session_cancel.child_token();
        let preparation = super::agent_loop_runtime::capture_owned_preparation(
            session,
            cancellation.clone(),
            command_capable,
        )?;
        // Do not reread the mutable permission override after the original
        // capture boundary; use its exact facet in the passive constructor.
        let parts = FactSession::from_permission(session, preparation.permission.clone());
        Ok(Self {
            preparation,
            coordinator: super::execution_coordinator::OwnedCoordinatorInputs::capture_original(
                session,
            ),
            controls: RunControlState::from_session(session),
            cleanup: super::runtime_events::RunCleanupState::from_session(session, ""),
            finish: TurnFinish::from_session(session, cancellation, None),
            parts,
            config: session.config.clone(),
            commands: command_runtime::OwnedCommandInputs::capture_original(session),
            init_warning: session.init_warning.clone(),
            recorded_reports: session.verification_reports(),
        })
    }
}

enum OwnedReady {
    Fact {
        run: crate::run::RunHandle,
        snapshot: crate::run::RunSnapshot,
    },
    Command,
    Replayed(crate::run::RunSnapshot),
}

struct OwnedObservation {
    ready: tokio::sync::oneshot::Receiver<Result<OwnedReady>>,
    result: tokio::sync::oneshot::Receiver<Result<AgentResult>>,
    events: mpsc::Receiver<AgentEvent>,
    guardian: JoinHandle<()>,
    disposition: tokio::sync::oneshot::Receiver<run_admission::OwnerDisposition>,
    cancellation: tokio_util::sync::CancellationToken,
}

struct CancelBlockingObserver(Option<tokio_util::sync::CancellationToken>);
impl Drop for CancelBlockingObserver {
    fn drop(&mut self) {
        if let Some(actual) = self.0.take() {
            actual.cancel();
        }
    }
}

fn launch_owned_conversation(
    session: &AgentSession,
    lease: run_admission::RunAdmissionLease,
    request: OwnedConversationRequest,
    mode: run_admission::ExecutionMode,
    command_capable: bool,
) -> Result<OwnedObservation> {
    let mut captured = OwnedConversationCapture::capture(session, command_capable)?;
    let cancellation = captured.preparation.cancellation.clone();
    let (handoff, start, original) =
        ExecutionCoordinator::prepare_original_handoff(session, lease, mode, cancellation.clone())?;
    captured.controls = captured.controls.bind_original(Arc::clone(&original))?;
    let (ready, ready_rx) = tokio::sync::oneshot::channel();
    let (result, result_rx) = tokio::sync::oneshot::channel();
    let (events, event_rx) = mpsc::channel(8);
    let (disposition, observed) = tokio::sync::oneshot::channel();
    let handoff = handoff.with_failure_publisher(run_admission::OriginalFailurePublisher::freeze(
        Arc::clone(&captured.finish.run_store),
        captured.finish.hook.clone(),
        captured.finish.session_id.clone(),
        (mode != run_admission::ExecutionMode::Blocking).then(|| events.clone()),
    ));
    let worker = tokio::spawn(async move {
        match start.wait().await {
            Ok(true) => {
                Box::pin(owned_conversation_worker(
                    captured, request, mode, original, ready, result, events,
                ))
                .await
            }
            Ok(false) => {
                let _ = ready.send(Err(CodeError::Session(
                    "original worker start was cancelled".into(),
                )));
            }
            Err(error) => {
                let _ = ready.send(Err(error));
            }
        }
    });
    // No await, fallible operation, or observer handoff between the real spawn
    // and the existing guardian taking this same actual JoinHandle/TaskLease.
    let aborts = if mode == run_admission::ExecutionMode::Blocking {
        Vec::new()
    } else {
        vec![worker.abort_handle()]
    };
    let guardian = handoff.supervise(worker, aborts, disposition);
    Ok(OwnedObservation {
        ready: ready_rx,
        result: result_rx,
        events: event_rx,
        guardian,
        disposition: observed,
        cancellation,
    })
}

async fn observe_owned_blocking(observation: OwnedObservation) -> Result<AgentResult> {
    let OwnedObservation {
        ready,
        result,
        events,
        guardian,
        mut disposition,
        cancellation,
    } = observation;
    let mut cancel = CancelBlockingObserver(Some(cancellation.clone()));
    // Blocking's actual invocation has no host event sender.
    drop(events);
    ready
        .await
        .map_err(|_| CodeError::Session("original worker ended before readiness".into()))??;
    let outcome = result
        .await
        .map_err(|_| CodeError::Session("original worker ended without result".into()))?;
    if outcome.is_ok() && !cancellation.is_cancelled() {
        // A normal completed call observes actual retirement before a following
        // send. RetainedUnproved is delivered before the guardian parks: never
        // await that permanently retained proxy or turn it into release proof.
        match disposition.await {
            Ok(
                run_admission::OwnerDisposition::Released
                | run_admission::OwnerDisposition::NeverStarted,
            ) => {
                guardian.await.map_err(|error| {
                    CodeError::Session(format!("original guardian observer failed: {error}"))
                })?;
            }
            Ok(run_admission::OwnerDisposition::RetainedUnproved) => {
                drop(guardian);
            }
            Err(_) => {
                drop(guardian);
                return Err(CodeError::Session(
                    "original guardian ended without a disposition".into(),
                ));
            }
        }
    } else {
        // Error/cancellation may have an outstanding original model callback.
        // The caller stops observing; the same guardian keeps its actual lease.
        match disposition.try_recv() {
            Ok(
                run_admission::OwnerDisposition::Released
                | run_admission::OwnerDisposition::NeverStarted,
            ) => {
                guardian.await.map_err(|error| {
                    CodeError::Session(format!("original guardian observer failed: {error}"))
                })?;
            }
            _ => drop(guardian),
        }
    }
    cancel.0 = None;
    outcome
}

async fn observe_owned_stream(observation: OwnedObservation) -> Result<AgentRunStream> {
    let OwnedObservation {
        ready,
        events,
        guardian,
        ..
    } = observation;
    let run = match ready
        .await
        .map_err(|_| CodeError::Session("original worker ended before readiness".into()))??
    {
        OwnedReady::Fact { run, .. } => Some(run),
        OwnedReady::Command => None,
        OwnedReady::Replayed(_) => {
            return Err(CodeError::Session(
                "ordinary stream cannot replay an exact Run".into(),
            ))
        }
    };
    Ok(AgentRunStream {
        run,
        events,
        worker: guardian,
    })
}

async fn observe_owned_detached(observation: OwnedObservation) -> Result<AgentRunSpawn> {
    let OwnedObservation {
        ready,
        events,
        guardian,
        ..
    } = observation;
    let started = ready
        .await
        .map_err(|_| CodeError::Session("original worker ended before readiness".into()))??;
    match started {
        OwnedReady::Fact { snapshot, .. } => Ok(AgentRunSpawn::Started {
            snapshot,
            worker: drain_detached_events(events, guardian),
        }),
        OwnedReady::Replayed(snapshot) => {
            drop(events);
            drop(guardian);
            Ok(AgentRunSpawn::Replayed { snapshot })
        }
        OwnedReady::Command => Err(CodeError::Session(
            "exact Run cannot be a slash command".into(),
        )),
    }
}

struct OwnedPinnedFact {
    run: crate::run::RunHandle,
    snapshot: crate::run::RunSnapshot,
    parts: FactSession,
    guard: crate::capability::SessionCapabilityRun,
    finish: TurnFinish,
    cleanup: super::runtime_events::RunCleanupState,
    controls: RunControlState,
    original: Arc<run_admission::OriginalExecutionContext>,
}

// Preparation uses only concrete values captured before the start-gated
// worker. Actual capability and Run returns are recorded before the next await.
async fn prepare_owned_fact(
    captured: OwnedConversationCapture,
    original: Arc<run_admission::OriginalExecutionContext>,
    prompt: &str,
    external: Option<crate::run::RunHandle>,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
) -> Result<OwnedPinnedFact> {
    use run_admission::OriginalScopeKind as Scope;
    let OwnedConversationCapture {
        preparation,
        coordinator,
        controls,
        cleanup,
        finish,
        mut parts,
        config,
        recorded_reports,
        ..
    } = captured;
    let super::agent_loop_runtime::OwnedPreparationCapture {
        fact,
        construction,
        checkpoint,
        cognitive_binding,
        cancellation,
        session_id,
        boundary,
        ..
    } = preparation;
    let cleanup = match external.as_ref() {
        Some(actual) => cleanup.with_run_id(actual.id().to_owned()),
        None => cleanup,
    };
    original.mark_scope_attempted(Scope::Fact)?;
    let (mut agent, guard) = match super::agent_loop_runtime::build_owned_pinned_agent_loop(
        fact,
        construction,
        boundary,
        cancellation.clone(),
    )
    .await
    {
        Ok(actual) => actual,
        Err(failure) => {
            let (error, actual) = failure.into_original_observation();
            if let Some((guard, close)) = actual {
                original.record_scope_returned(Scope::Fact, &guard)?;
                original.record_scope_close(Scope::Fact, &close)?;
            }
            // No Run/inbox/committer construction was reached on this branch.
            if external.is_none() {
                original.record_no_run_branch()?;
            }
            original.record_no_inbox()?;
            original.record_no_committer()?;
            original.seal_original_controls().await?;
            controls.fail_reserved_run_start(&original, &error).await?;
            return Err(finish_failed_admission(&cleanup, &original, error).await);
        }
    };
    original.record_scope_returned(Scope::Fact, &guard)?;
    let binding = match agent.checkpoint_capability_binding().cloned() {
        Some(binding) => binding,
        None => {
            let close = guard.close().await;
            original.record_scope_close(Scope::Fact, &close)?;
            return Err(CodeError::Session(
                "scoped Agent Run has no capability binding".into(),
            ));
        }
    };
    let run_result = if let Some(run) = external {
        let id = run.id().to_owned();
        async {
            controls
                .bind_capability_generation(&id, binding.clone())
                .await?;
            if let Some(value) = cognitive_binding.clone() {
                controls.bind_cognitive_package(&id, value).await?;
            }
            Ok(run)
        }
        .await
    } else {
        controls
            .start_run_with_bindings(prompt, cognitive_binding.clone(), binding.clone())
            .await
    };
    let run = match run_result {
        Ok(run) => run,
        Err(error) => {
            let close = guard.close().await;
            original.record_scope_close(Scope::Fact, &close)?;
            // reserve may already have returned Created; absence is not proof.
            // T005 preserves any unknown binding and refuses false release.
            original.record_no_inbox()?;
            original.record_no_committer()?;
            original.seal_original_controls().await?;
            controls.fail_reserved_run_start(&original, &error).await?;
            return Err(finish_failed_admission(&cleanup, &original, error).await);
        }
    };
    let run_id = run.id().to_owned();
    let cleanup = cleanup.with_run_id(run_id.clone());
    if let Some(binding) = cognitive_binding {
        finish
            .run_store
            .record_event(&run_id, AgentEvent::CognitiveContextBound { binding })
            .await;
    }
    let started = AgentEvent::Start {
        prompt: prompt.to_owned(),
    };
    finish
        .run_store
        .record_event(&run_id, started.clone())
        .await;
    finish.note(&run_id, &started).await;
    let (sink, mut checkpoints) = super::runtime_checkpoints::runtime_checkpoint_channel_owned(
        checkpoint.store,
        checkpoint.export,
        checkpoint.persistence,
    );
    let checkpoint = match sink {
        Some(sink) => {
            let install = original.reserve_committer_install()?;
            let actual = tokio::spawn(async move {
                while let Some(boundary) = checkpoints.recv().await {
                    checkpoints.commit(boundary).await;
                }
            });
            install.install(actual); // No await before the actual handle is retained.
            Some(crate::fact_control::CheckpointEmit {
                sink,
                run_id: run_id.clone(),
                session_id: session_id.clone(),
                capability_binding: Some(binding),
            })
        }
        None => {
            drop(checkpoints);
            original.record_no_committer()?;
            None
        }
    };
    let prepared = match ExecutionCoordinator::prepare_owned(
        coordinator,
        controls.clone(),
        &original,
        run_id.clone(),
        &mut agent,
        cancellation.clone(),
    )
    .await
    {
        Ok(actual) => actual,
        Err(error) => {
            drop(checkpoint);
            drop(agent);
            let close = guard.close().await;
            original.record_scope_close(Scope::Fact, &close)?;
            observe_owned_committer(&original).await?;
            original.seal_original_controls().await?;
            controls.fail_reserved_run_start(&original, &error).await?;
            return Err(finish_failed_admission(&cleanup, &original, error).await);
        }
    };
    prepared.run_control().update_turn(1).await;
    let (lane, _) = tokio::sync::broadcast::channel::<AgentEvent>(1024);
    agent = agent.with_agent_event_tx(lane);
    // The fact actor relays the broadcast lane per tool call. There is no
    // actual barrier receiver here. Select the final runtime lane before
    // creating/binding the one invocation and its matched governed client.
    let invocation = prepared.fact_invocation(&agent, event_tx.clone());
    agent = invocation.bind_agent_loop(&agent);
    parts.client =
        Some(agent.scoped_llm_client_for_parts(Some(&session_id), &event_tx, &cancellation));
    parts.executor = agent.tool_executor_handle();
    parts.original_model = Some(original.model_call_port());
    parts.surface = Some(crate::fact_control::SessionSurface {
        agent,
        session_id,
        checkpoint,
        events: event_tx,
        pending_memory_telemetry: Arc::new(std::sync::Mutex::new(Vec::new())),
        fact_log: Arc::clone(&parts.fact_log),
        streamed_text: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        forced_stop: Arc::new(std::sync::Mutex::new(None)),
        cancel: cancellation,
        transcript: Arc::new(std::sync::Mutex::new(Vec::new())),
        usage: Arc::new(std::sync::Mutex::new(crate::llm::TokenUsage::default())),
        confirmation: config.confirmation_manager,
        run_store: Some(Arc::clone(&finish.run_store)),
        run_id: Some(run_id.clone()),
        goal_tick: None,
        ledger: Arc::new(std::sync::Mutex::new(
            crate::harness_loop::MutationLedger::default(),
        )),
        reports: Arc::new(std::sync::Mutex::new(recorded_reports)),
        run_control: Some(prepared.run_control()),
        targeted_paths: Arc::new(std::sync::Mutex::new(Vec::new())),
        harness: config.harness,
        host_harness_registry: config.host_harness_registry,
        host_harness_assembler: config.host_harness_assembler,
    });
    if let Some(surface) = &parts.surface {
        crate::fact_control::replay_unsettled_mutations(surface);
    }
    let snapshot = match controls.snapshot(&run_id).await {
        Some(actual) => actual,
        None => {
            return Err(fail_owned_components(
                parts,
                guard,
                controls,
                cleanup,
                original,
                CodeError::Session(format!(
                    "bound exact Run '{run_id}' disappeared before execution"
                )),
            )
            .await)
        }
    };
    Ok(OwnedPinnedFact {
        run,
        snapshot,
        parts,
        guard,
        finish,
        cleanup,
        controls,
        original,
    })
}

impl TurnFinish {
    async fn publish_owned(
        &self,
        original: &Arc<run_admission::OriginalExecutionContext>,
        outcome: &std::result::Result<AgentResult, anyhow::Error>,
        update_history: bool,
    ) -> Result<run_admission::OriginalPublicationKind> {
        use run_admission::{
            OriginalHookObservation as Hook, OriginalPublicationKind as Kind,
            OriginalPublicationObservation, OriginalStoreObservation as Store,
        };
        let sealed = original.seal_publication()?;
        let target = original.publication_target(&sealed)?;
        let mut actual = OriginalPublicationObservation {
            kind: if target.run_id.is_none() {
                Kind::NoRun
            } else if outcome.is_err() {
                Kind::Error
            } else if target.cancelled {
                Kind::Cancelled
            } else {
                Kind::End
            },
            terminal: Store::NotApplicable,
            diagnostic: Store::NotApplicable,
            cancellation_hook: Hook::NotApplicable,
            event_hook: Hook::NotApplicable,
        };
        if let Some(run_id) = target.run_id.as_deref() {
            actual.terminal = Store::NotAttempted;
            if target.cancelled && outcome.is_err() {
                actual.diagnostic = Store::NotAttempted;
            }
            if target.cancel_origin.is_some() && self.hook.is_some() {
                actual.cancellation_hook = Hook::NotAttempted;
            }
            if self.hook.is_some() && (!target.cancelled || outcome.is_err()) {
                actual.event_hook = Hook::NotAttempted;
            }
            original.record_publication_progress(&sealed, &actual)?;
            if target.cancelled {
                actual.terminal = Store::Entered;
                original.record_publication_progress(&sealed, &actual)?;
                actual.terminal = Store::Returned(Box::new(
                    self.run_store
                        .settle_terminal(run_id, crate::run::RunTerminalTransition::Cancelled)
                        .await,
                ));
                original.record_publication_progress(&sealed, &actual)?;
                if target.cancel_origin.is_some() {
                    if let Some(hook) = &self.hook {
                        actual.cancellation_hook = Hook::Entered;
                        original.record_publication_progress(&sealed, &actual)?;
                        hook.record_run_cancelled(
                            run_id,
                            &target.session_id,
                            Some("cancelled original Run"),
                        )
                        .await;
                        actual.cancellation_hook = Hook::Returned;
                        original.record_publication_progress(&sealed, &actual)?;
                    }
                }
            }
            let event = match outcome {
                Ok(_) if target.cancelled => None,
                Ok(result) => Some(AgentEvent::End {
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
                }),
                Err(error) => Some(AgentEvent::Error {
                    message: error.to_string(),
                }),
            };
            if let Some(event) = event {
                if target.cancelled {
                    actual.diagnostic = Store::Entered;
                } else {
                    actual.terminal = Store::Entered;
                }
                original.record_publication_progress(&sealed, &actual)?;
                let returned = self.run_store.record_event(run_id, event.clone()).await;
                if target.cancelled {
                    actual.diagnostic = Store::Returned(Box::new(returned));
                } else {
                    actual.terminal = Store::Returned(Box::new(returned));
                }
                original.record_publication_progress(&sealed, &actual)?;
                if let Some(hook) = &self.hook {
                    actual.event_hook = Hook::Entered;
                    original.record_publication_progress(&sealed, &actual)?;
                    hook.record_agent_event(&event, run_id, &target.session_id)
                        .await;
                    actual.event_hook = Hook::Returned;
                    original.record_publication_progress(&sealed, &actual)?;
                }
            }
            self.persistence.clear_loop_checkpoint(run_id).await;
        }
        if let Ok(result) = outcome {
            self.persistence.record_usage(&result.usage);
            if update_history && !result.messages.is_empty() {
                let mut messages = result.messages.clone();
                strip_attachment_annotation(&mut messages);
                self.persistence.record_messages(messages);
            }
            self.persistence.auto_save_if_enabled().await;
        }
        // Project the same sealed decision to the caller/public stream. A
        // later token poll could disagree with the terminal already retained.
        let kind = actual.kind;
        original.record_publication(sealed, actual)?;
        Ok(kind)
    }
}

async fn observe_owned_committer(
    original: &Arc<run_admission::OriginalExecutionContext>,
) -> Result<()> {
    match original.wait_committer().await? {
        run_admission::ActualCommitterJoin::PositivelyDisabled
        | run_admission::ActualCommitterJoin::Joined(
            run_admission::ActualJoinObservation::Completed,
        ) => Ok(()),
        actual => Err(CodeError::Session(format!(
            "original checkpoint committer did not complete: {actual:?}"
        ))),
    }
}

async fn fail_owned_prepared(pinned: OwnedPinnedFact, primary: CodeError) -> CodeError {
    let OwnedPinnedFact {
        parts,
        guard,
        controls,
        cleanup,
        original,
        ..
    } = pinned;
    fail_owned_components(parts, guard, controls, cleanup, original, primary).await
}

async fn fail_owned_components(
    mut parts: FactSession,
    guard: crate::capability::SessionCapabilityRun,
    controls: RunControlState,
    cleanup: super::runtime_events::RunCleanupState,
    original: Arc<run_admission::OriginalExecutionContext>,
    mut primary: CodeError,
) -> CodeError {
    let closed = guard.close().await;
    if let Err(error) = &closed {
        primary = CodeError::Session(format!(
            "{primary}; capability Run cleanup also failed: {error}"
        ));
    }
    if let Err(error) = original.record_scope_close(run_admission::OriginalScopeKind::Fact, &closed)
    {
        return CodeError::Session(format!(
            "{primary}; original close observation failed: {error}"
        ));
    }
    parts.retire_fact_host_events();
    parts.retire_checkpoint_senders();
    if let Err(error) = observe_owned_committer(&original).await {
        return CodeError::Session(format!("{primary}; {error}"));
    }
    if let Some(surface) = &parts.surface {
        if let Err(error) = crate::fact_control::settle_terminal_run_controls(surface).await {
            primary = CodeError::Session(format!(
                "{primary}; terminal input retention failed: {error}"
            ));
        }
        if let Err(error) = crate::fact_control::retained_transcript(surface) {
            primary = CodeError::Session(format!("{primary}; terminal transcript failed: {error}"));
        }
    }
    if let Err(error) = original.seal_original_controls().await {
        return CodeError::Session(format!(
            "{primary}; original controls remain unproved: {error}"
        ));
    }
    drop(parts);
    if let Err(error) = controls.fail_reserved_run_start(&original, &primary).await {
        return CodeError::Session(format!(
            "{primary}; original admission publication failed: {error}"
        ));
    }
    if let Err(error) = cleanup.finish_owned(&original).await {
        return CodeError::Session(format!(
            "{primary}; original cleanup remains unproved: {error}"
        ));
    }
    primary
}

async fn finish_failed_admission(
    cleanup: &super::runtime_events::RunCleanupState,
    original: &Arc<run_admission::OriginalExecutionContext>,
    primary: CodeError,
) -> CodeError {
    if let Err(cleanup_error) = cleanup.finish_owned(original).await {
        // The original context retains its incomplete observations and lease.
        // Preserve the typed admission cause for callers; a failed cleanup
        // observation cannot replace it or authorize original-owner release.
        tracing::error!(
            primary_error = %primary,
            error = %cleanup_error,
            "Admission failed and original cleanup remains unproved"
        );
    }
    primary
}

fn combine_original_failure(primary: CodeError, observed: Result<()>) -> CodeError {
    match observed {
        Ok(()) => primary,
        Err(error) => CodeError::Session(format!(
            "{primary}; original cleanup remains unproved: {error}"
        )),
    }
}

async fn owned_no_run_finish(
    capture: &OwnedConversationCapture,
    original: &Arc<run_admission::OriginalExecutionContext>,
) -> Result<()> {
    original.record_no_run_branch()?;
    original.record_no_inbox()?;
    original.record_no_committer()?;
    original.seal_original_controls().await?;
    capture
        .finish
        .publish_owned(
            original,
            &Err(anyhow::anyhow!("no fact Run constructed")),
            false,
        )
        .await?;
    capture.cleanup.finish_owned(original).await
}

async fn owned_load_checkpoint(
    store: Option<&Arc<dyn crate::store::SessionStore>>,
    session_id: &str,
    source: &str,
) -> Result<Option<LoopCheckpoint>> {
    let Some(store) = store else {
        return Ok(None);
    };
    let actual = store.load_loop_checkpoint(source).await.map_err(|error| {
        CodeError::Session(format!("load_loop_checkpoint('{source}') failed: {error}"))
    })?;
    if let Some(checkpoint) = &actual {
        checkpoint
            .ensure_owned_by(source, session_id)
            .map_err(|error| {
                CodeError::Session(format!(
                    "refusing to resume checkpoint '{source}': {error:#}"
                ))
            })?;
    }
    Ok(actual)
}

async fn owned_conversation_worker(
    mut captured: OwnedConversationCapture,
    request: OwnedConversationRequest,
    mode: run_admission::ExecutionMode,
    original: Arc<run_admission::OriginalExecutionContext>,
    ready: tokio::sync::oneshot::Sender<Result<OwnedReady>>,
    result: tokio::sync::oneshot::Sender<Result<AgentResult>>,
    events: mpsc::Sender<AgentEvent>,
) {
    use run_admission::OriginalScopeKind as Scope;
    let mut ready = Some(ready);
    let bind_source_generation =
        matches!(&request, OwnedConversationRequest::PreparedRecovery { .. });
    let mut request = request;
    let goal_tick = match &mut request {
        OwnedConversationRequest::Prompt { goal_tick, .. } => goal_tick.take(),
        _ => None,
    };
    let resolved: Result<_> = async {
        let request_inputs: Result<_> = async {
            let resolved = match request {
                OwnedConversationRequest::Prompt {
                    prompt,
                    attachments,
                    history,
                    command_capable,
                    exact_run_id,
                    ..
                } => (
                    prompt,
                    attachments,
                    history,
                    command_capable,
                    exact_run_id,
                    None,
                    false,
                ),
                OwnedConversationRequest::ExactRecovery {
                    source,
                    target,
                    prompt,
                } => {
                    let checkpoint = owned_load_checkpoint(
                        captured.preparation.checkpoint.store.as_ref(),
                        &captured.preparation.session_id,
                        &source,
                    )
                    .await?
                    .ok_or_else(|| {
                        CodeError::Session(format!("no loop checkpoint found for run '{source}'"))
                    })?;
                    (
                        prompt,
                        Vec::new(),
                        None,
                        false,
                        Some(target),
                        Some(checkpoint),
                        false,
                    )
                }
                OwnedConversationRequest::PreparedRecovery {
                    checkpoint,
                    target,
                    prompt,
                } => (
                    prompt,
                    Vec::new(),
                    None,
                    false,
                    Some(target),
                    Some(*checkpoint),
                    false,
                ),
                OwnedConversationRequest::OrdinaryResume { source } => {
                    let checkpoint = owned_load_checkpoint(
                        captured.preparation.checkpoint.store.as_ref(),
                        &captured.preparation.session_id,
                        &source,
                    )
                    .await?;
                    if checkpoint.is_none() {
                        let thread = crate::fact_control::thread_for_session(
                            &captured.preparation.session_id,
                        );
                        let empty = crate::fact_control::read_workspace_facts(
                            &captured.preparation.workspace,
                            &thread,
                        )
                        .map(|facts| facts.is_empty())
                        .unwrap_or(true);
                        if empty {
                            return Err(CodeError::Session(
                                if captured.preparation.checkpoint.store.is_none() {
                                    "resume_run requires a session_store on this session".to_owned()
                                } else {
                                    format!("no loop checkpoint found for run '{source}'")
                                },
                            ));
                        }
                    }
                    let prompt = checkpoint
                        .as_ref()
                        .and_then(|checkpoint| {
                            checkpoint
                                .messages
                                .iter()
                                .find(|message| message.role == "user")
                        })
                        .map(Message::text)
                        .unwrap_or_default();
                    (prompt, Vec::new(), None, false, None, checkpoint, true)
                }
            };
            if let Some(checkpoint) = &resolved.5 {
                captured
                    .preparation
                    .checkpoint
                    .persistence
                    .validate_checkpoint_source(checkpoint)?;
            }
            Ok(resolved)
        }
        .await;
        let (mut prompt, attachments, history, command_capable, exact, checkpoint, ordinary) =
            match request_inputs {
                Ok(actual) => actual,
                Err(error) => {
                    // Only read-only request resolution occurred; neither scope,
                    // Run, inbox nor committer was constructed on this branch.
                    original.record_scope_not_attempted(Scope::Command)?;
                    original.record_scope_not_attempted(Scope::Fact)?;
                    owned_no_run_finish(&captured, &original).await?;
                    return Err(error);
                }
            };
        if command_capable {
            let is_command = crate::commands::CommandRegistry::is_command(&prompt);
            if is_command {
                let command = captured.preparation.command.take().ok_or_else(|| {
                    CodeError::Session("original command projection missing".into())
                })?;
                original.mark_scope_attempted(Scope::Command)?;
                let (runtime, guard, _) =
                    match super::agent_loop_runtime::admit_captured_runtime_projection(
                        command,
                        &captured.preparation.boundary,
                        captured.preparation.cancellation.clone(),
                        &captured.preparation.workspace,
                    )
                    .await
                    {
                        Ok(actual) => actual,
                        Err(failure) => {
                            let (error, actual) = failure.into_original_observation();
                            if let Some((guard, close)) = actual {
                                original.record_scope_returned(Scope::Command, &guard)?;
                                original.record_scope_close(Scope::Command, &close)?;
                            }
                            original.record_scope_not_attempted(Scope::Fact)?;
                            let cleanup = owned_no_run_finish(&captured, &original).await;
                            return Err(combine_original_failure(error, cleanup));
                        }
                    };
                original.record_scope_returned(Scope::Command, &guard)?;
                let prepared =
                    command_runtime::prepare_owned(&captured.commands, &prompt, &runtime, guard)
                        .await;
                let (guard, closed, text) = prepared.into_original_observation();
                original.record_scope_close(Scope::Command, &closed)?;
                drop(guard);
                if let Err(error) = closed {
                    original.record_scope_not_attempted(Scope::Fact)?;
                    let cleanup = owned_no_run_finish(&captured, &original).await;
                    return Err(combine_original_failure(error.into(), cleanup));
                }
                if let Some(text) = text {
                    original.record_scope_not_attempted(Scope::Fact)?;
                    owned_no_run_finish(&captured, &original).await?;
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Ok(OwnedReady::Command));
                    }
                    let answer = captured
                        .commands
                        .blocking_result(text.clone(), history.as_deref());
                    if mode != run_admission::ExecutionMode::Blocking {
                        command_runtime::send_text_output(&events, text).await;
                    }
                    return Ok(Some(answer));
                }
            } else {
                original.record_scope_not_attempted(Scope::Command)?;
            }
        } else {
            original.record_scope_not_attempted(Scope::Command)?;
        }
        if mode == run_admission::ExecutionMode::Blocking && command_capable {
            if let Some(warning) = &captured.init_warning {
                tracing::warn!(
                    session_id = %captured.preparation.session_id,
                    "Session init warning: {}", warning
                );
            }
        }
        // Debt is read from the original workspace inside the owned worker.
        if mode == run_admission::ExecutionMode::Streaming || ordinary {
            let thread = crate::fact_control::thread_for_session(&captured.preparation.session_id);
            let owed =
                crate::fact_control::read_workspace_facts(&captured.preparation.workspace, &thread)
                    .map(|facts| crate::fact_control::unsettled_mutation_paths(&facts))
                    .unwrap_or_default();
            if let Some(directive) =
                owed_work_directive(&owed, &captured.config.verification_contracts)
            {
                prompt = format!("{prompt}\n\n{directive}");
            }
        }
        // Exact reservation precedes capability admission, as in the original
        // ingress. Existing is a passive replay and cannot mint another owner.
        let external = if let Some(id) = exact.as_deref() {
            let reservation = match captured.controls.reserve_run_with_id(id, &prompt).await {
                Ok(actual) => actual,
                Err(error @ CodeError::RunIdentityConflict { .. }) => {
                    original.record_scope_not_attempted(Scope::Fact)?;
                    owned_no_run_finish(&captured, &original).await?;
                    return Err(error);
                }
                Err(error) => return Err(error), // Unknown Created/owner failure remains unproved.
            };
            if reservation.replayed() {
                original.record_scope_not_attempted(Scope::Fact)?;
                let snapshot = reservation.snapshot().clone();
                owned_no_run_finish(&captured, &original).await?;
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Ok(OwnedReady::Replayed(snapshot)));
                }
                return Ok(None);
            }
            let actual = original.created_handle()?;
            if bind_source_generation {
                if let Some(binding) = checkpoint
                    .as_ref()
                    .and_then(|value| value.capability_binding.clone())
                {
                    captured
                        .controls
                        .bind_capability_generation(id, binding)
                        .await?;
                }
            }
            Some(actual)
        } else {
            None
        };
        let event_tx = if mode == run_admission::ExecutionMode::Blocking {
            None
        } else {
            Some(events.clone())
        };
        let mut pinned =
            prepare_owned_fact(captured, Arc::clone(&original), &prompt, external, event_tx)
                .await?;
        if let Some(surface) = pinned.parts.surface.as_mut() {
            surface.goal_tick = goal_tick;
        }
        if let Some(checkpoint) = checkpoint.as_ref() {
            let seeded = seed_resumed_mutation_parts(&pinned.parts, checkpoint).and_then(|()| {
                if ordinary {
                    Ok(())
                } else {
                    seed_exact_recovery_parts(&pinned.parts, &checkpoint.messages)
                }
            });
            if let Err(error) = seeded {
                return Err(
                    fail_owned_prepared(pinned, CodeError::Session(format!("{error:#}"))).await,
                );
            }
        }
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(OwnedReady::Fact {
                run: pinned.run.clone(),
                snapshot: pinned.snapshot.clone(),
            }));
        }
        settle_owned_fact(
            pinned,
            OwnedFactExecution {
                prompt,
                attachments,
                history,
                checkpoint,
                ordinary,
                exact,
                mode,
            },
            events,
        )
        .await
    }
    .await;
    match resolved {
        Ok(Some(answer)) => {
            let _ = result.send(Ok(answer));
        }
        Ok(None) => {}
        Err(error) => {
            if let Some(ready) = ready.take() {
                let _ = ready.send(Err(error));
            } else {
                let _ = result.send(Err(error));
            }
        }
    }
}

struct OwnedFactExecution {
    prompt: String,
    attachments: Vec<Attachment>,
    history: Option<Vec<Message>>,
    checkpoint: Option<LoopCheckpoint>,
    ordinary: bool,
    exact: Option<String>,
    mode: run_admission::ExecutionMode,
}

async fn settle_owned_fact(
    mut pinned: OwnedPinnedFact,
    execution: OwnedFactExecution,
    events: mpsc::Sender<AgentEvent>,
) -> Result<Option<AgentResult>> {
    let original = Arc::clone(&pinned.original);
    let host_cancellation = pinned
        .parts
        .surface
        .as_ref()
        .map(|surface| surface.cancel.clone());
    let OwnedFactExecution {
        prompt,
        attachments,
        history,
        mut checkpoint,
        ordinary,
        exact,
        mode,
    } = execution;
    let mut current_goal = None;
    let settled = async {
        let fact = pinned.parts.open()?;
        current_goal = fact.current_goal().cloned();
        if ordinary {
            if let Some(checkpoint) = checkpoint.as_ref() {
                let mut seed = checkpoint.messages.clone();
                if seed
                    .last()
                    .is_some_and(|message| message.role == "assistant")
                {
                    seed.pop();
                }
                fact.seed_recovery_history(&seed, &prompt)?;
            }
            fact.resume_limit(32).await
        } else if checkpoint.is_some() {
            fact.resume_limit(32).await
        } else {
            if let Some(history) = history.as_deref() {
                fact.seed_history(history)?;
            }
            prepare_pre_analysis(&pinned.parts, &prompt, fact.current_goal()).await?;
            let settled = fact
                .user_text_with_attachments(&prompt, &attachments)
                .await?;
            settle_confirmations(&fact, settled, &pinned.parts).await
        }
    }
    .await;
    let mut usage = pinned.parts.usage();
    let messages = pinned.parts.transcript();
    if ordinary {
        if let Some(checkpoint) = checkpoint.as_ref() {
            usage.prompt_tokens = usage
                .prompt_tokens
                .saturating_add(checkpoint.total_usage.prompt_tokens);
            usage.completion_tokens = usage
                .completion_tokens
                .saturating_add(checkpoint.total_usage.completion_tokens);
            usage.total_tokens = usage
                .total_tokens
                .saturating_add(checkpoint.total_usage.total_tokens);
        }
    }
    let mut mapped = settled
        .as_ref()
        .map(|settled| agent_result(settled, messages, usage))
        .map_err(|error| anyhow::anyhow!("{error:#}"));
    if ordinary {
        if let (Ok(result), Some(checkpoint)) = (&mut mapped, checkpoint.take()) {
            result.tool_calls_count = result
                .tool_calls_count
                .saturating_add(checkpoint.tool_calls_count);
        }
    }
    // Capture turn-local evidence before terminal Steer adds another user fact.
    if let Ok(result) = &mut mapped {
        result.forced_stop = captured_forced_stop(&pinned.parts);
    }
    let mut accepted_step = None;
    let mut goal_attestation = None;
    if let (Some(surface), Some(goal), Ok(result)) =
        (&pinned.parts.surface, &current_goal, &mut mapped)
    {
        let ledger = surface
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut fresh = Vec::new();
        surface.agent.merge_completion_attestor(&ledger, &mut fresh);
        match crate::fact_control::assess_goal_step(surface, goal, result, fresh.first()).await {
            Ok((prepared, usage)) => {
                accepted_step = prepared;
                result.usage.accumulate(&usage);
            }
            Err(error) => mapped = Err(error),
        }
        goal_attestation = Some(fresh);
    }
    mapped = settle_owned_capability_run(&pinned, mapped).await;
    pinned.parts.retire_fact_host_events();
    pinned.parts.retire_checkpoint_senders();
    // A late checkpoint cannot overwrite the terminal retained input/history.
    let committer = observe_owned_committer(&original).await;
    if let Err(error) = committer {
        mapped = combine_retention_failure(mapped, "checkpoint committer", error);
    } else if let Some(surface) = &pinned.parts.surface {
        if let Err(error) = crate::fact_control::settle_terminal_run_controls(surface).await {
            mapped = combine_retention_failure(mapped, "terminal input retention", error);
        }
        match crate::fact_control::retained_transcript(surface) {
            Ok(messages) => {
                if let Ok(result) = &mut mapped {
                    result.messages = messages;
                }
            }
            Err(error) => {
                mapped = combine_retention_failure(mapped, "terminal transcript", error);
            }
        }
    }
    original.seal_original_controls().await?;
    mapped =
        mapped.and_then(|result| gate_result(&pinned.parts, result, goal_attestation.as_deref()));
    if mapped.is_ok() {
        if let (Some(surface), Some(prepared)) = (&pinned.parts.surface, accepted_step) {
            if let Err(error) = crate::fact_control::commit_goal_step(
                surface,
                prepared,
                &original.model_call_port(),
            ) {
                mapped = Err(error);
            }
        }
    }
    if let (Some(surface), Some(goal)) = (&pinned.parts.surface, &current_goal) {
        let usage = mapped.as_ref().ok().map(|result| result.usage.clone());
        if let Err(error) = crate::fact_control::settle_goal_slice(
            surface,
            goal,
            usage,
            &original.model_call_port(),
        ) {
            mapped = combine_retention_failure(mapped, "goal slice settlement", error);
        }
    }
    let update_history = history.is_none() && !ordinary && exact.is_none();
    // Keep each original route's memory/End/committer ordering.
    let admitted = if ordinary {
        None
    } else {
        admit_settled_memory(&pinned.parts, &prompt, &mapped)
    };
    // The completed fact can no longer open another main model facade. Drop
    // its strong host senders before telemetry or a retained memory tail can
    // clone the original agent; the passive channel identity remains bound.
    pinned.parts.retire_fact_host_events();
    let publication = if mode == run_admission::ExecutionMode::Blocking && !ordinary {
        if let Some(ticket) = admitted {
            extract_admitted_memory(&pinned.parts, &prompt, &mapped, ticket).await;
        }
        let publication = pinned
            .finish
            .publish_owned(&original, &mapped, update_history)
            .await?;
        drop(pinned.parts);
        publication
    } else if mode == run_admission::ExecutionMode::Streaming {
        let streamed = pinned.parts.surface.as_ref().is_some_and(|surface| {
            surface
                .streamed_text
                .load(std::sync::atomic::Ordering::Relaxed)
        });
        if let Some(surface) = &mut pinned.parts.surface {
            surface.events.take();
            surface.checkpoint.take();
        }
        let publication = pinned
            .finish
            .publish_owned(&original, &mapped, update_history)
            .await?;
        let telemetry =
            emit_memory_telemetry(&events, &pinned.parts, host_cancellation.as_ref()).await;
        spawn_recalled_memory_usage(&pinned.parts, telemetry, &mapped);
        emit_gated(
            &events,
            &settled,
            &mapped,
            streamed,
            publication,
            pinned.run.id(),
            host_cancellation.as_ref(),
        )
        .await;
        drop(events);
        let parts = pinned.parts;
        tokio::spawn(async move {
            if let Some(ticket) = admitted {
                extract_admitted_memory(&parts, &prompt, &mapped, ticket).await;
            }
        });
        pinned.cleanup.finish_owned(&original).await?;
        return Ok(None);
    } else {
        let publication = pinned
            .finish
            .publish_owned(&original, &mapped, false)
            .await?;
        if mode != run_admission::ExecutionMode::Blocking {
            let streamed = pinned.parts.surface.as_ref().is_some_and(|surface| {
                surface
                    .streamed_text
                    .load(std::sync::atomic::Ordering::Relaxed)
            });
            emit_gated(
                &events,
                &settled,
                &mapped,
                streamed,
                publication,
                pinned.run.id(),
                host_cancellation.as_ref(),
            )
            .await;
        }
        drop(events);
        if let Some(surface) = &mut pinned.parts.surface {
            surface.events.take();
        }
        if let Some(ticket) = admitted {
            extract_admitted_memory(&pinned.parts, &prompt, &mapped, ticket).await;
        }
        drop(pinned.parts);
        publication
    };
    if let Err(cleanup) = pinned.cleanup.finish_owned(&original).await {
        return Err(match mapped {
            Ok(_) => cleanup,
            Err(primary) => CodeError::Session(format!(
                "{primary:#}; original cleanup remains unproved: {cleanup}"
            )),
        });
    }
    if publication == run_admission::OriginalPublicationKind::Cancelled {
        return Err(CodeError::run_cancelled(pinned.run.id().to_owned()));
    }
    let mut answer = mapped.map_err(|error| CodeError::Session(format!("{error:#}")))?;
    if update_history {
        strip_attachment_annotation(&mut answer.messages);
    }
    Ok(Some(answer))
}

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

fn admit_settled_memory(
    parts: &FactSession,
    prompt: &str,
    mapped: &std::result::Result<AgentResult, anyhow::Error>,
) -> Option<crate::memory::MemoryExtractionTicket> {
    let result = mapped.as_ref().ok()?;
    let surface = parts.surface.as_ref()?;
    surface
        .agent
        .admit_fact_memory_extraction(prompt, &result.text, &surface.cancel)
}

async fn extract_admitted_memory(
    parts: &FactSession,
    prompt: &str,
    mapped: &std::result::Result<AgentResult, anyhow::Error>,
    ticket: crate::memory::MemoryExtractionTicket,
) {
    let Ok(result) = mapped else {
        return;
    };
    let Some(surface) = parts.surface.as_ref() else {
        return;
    };
    surface
        .agent
        .finish_admitted_fact_extraction(
            prompt,
            &result.text,
            &surface.session_id,
            &surface.cancel,
            ticket,
        )
        .await;
}

/// Drain the recall telemetry the actor parked on the surface and emit it
/// while the worker owns the host channel — always before `End`, never from
/// inside the state machine. Returns the drained batches so post-turn usage
/// telemetry can consume the same recalled set.
async fn emit_memory_telemetry(
    tx: &mpsc::Sender<AgentEvent>,
    parts: &FactSession,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> Vec<crate::fact_control::MemoryTelemetryBatch> {
    let Some(surface) = parts.surface.as_ref() else {
        return Vec::new();
    };
    let batches = std::mem::take(
        &mut *surface
            .pending_memory_telemetry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for batch in &batches {
        let result_count = batch.items.len();
        for (memory_id, content, relevance) in batch.items.clone() {
            send_host_event(
                tx,
                AgentEvent::MemoryRecalled {
                    memory_id,
                    content,
                    relevance,
                },
                cancellation,
            )
            .await;
        }
        send_host_event(
            tx,
            AgentEvent::MemoriesSearched {
                query: Some(batch.query.clone()),
                tags: Vec::new(),
                result_count,
            },
            cancellation,
        )
        .await;
    }
    batches
}

/// Fire-and-forget lexical usage telemetry for the turn's recalled memories.
///
/// Runs on a detached task after the turn settled: store round-trips never
/// delay `End`, and the recorder itself traces every error.
fn spawn_recalled_memory_usage(
    parts: &FactSession,
    batches: Vec<crate::fact_control::MemoryTelemetryBatch>,
    mapped: &std::result::Result<AgentResult, anyhow::Error>,
) {
    let Some(surface) = parts.surface.as_ref() else {
        return;
    };
    let Ok(result) = mapped else {
        return;
    };
    let recalled: Vec<(String, String, f32)> = batches
        .iter()
        .flat_map(|batch| batch.items.iter().cloned())
        .collect();
    if recalled.is_empty() || result.text.trim().is_empty() {
        return;
    }
    let agent = surface.agent.clone();
    let final_text = result.text.clone();
    tokio::spawn(async move {
        agent
            .record_recalled_memory_usage(&recalled, &final_text)
            .await;
    });
}

// Cancellation ends host observation, never original execution ownership.
// A ready send keeps its place in the bounded FIFO; a full send is abandoned
// only after the same Run cancellation. Authoritative terminal/control events
// are retained by their caller before this best-effort host projection.
async fn send_host_event(
    tx: &mpsc::Sender<AgentEvent>,
    event: AgentEvent,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) {
    if let Some(cancellation) = cancellation {
        tokio::select! {
            biased;
            _ = tx.send(event) => {},
            _ = cancellation.cancelled() => {},
        }
    } else {
        let _ = tx.send(event).await;
    }
}

async fn emit_interactive_host_event(
    surface: &crate::fact_control::SessionSurface,
    event: AgentEvent,
) {
    if let (Some(store), Some(run_id)) = (&surface.run_store, &surface.run_id) {
        store.record_event(run_id, event.clone()).await;
    }
    if let Some(sender) = &surface.events {
        send_host_event(sender, event, Some(&surface.cancel)).await;
    }
}

async fn emit_gated(
    tx: &mpsc::Sender<AgentEvent>,
    settled: &anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>>,
    mapped: &std::result::Result<AgentResult, anyhow::Error>,
    streamed_text: bool,
    publication: run_admission::OriginalPublicationKind,
    run_id: &str,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) {
    if let Err(error) = mapped {
        send_host_event(
            tx,
            AgentEvent::Error {
                message: error.to_string(),
            },
            cancellation,
        )
        .await;
        return;
    }
    // Only the original sealed publication can authorize a successful End.
    // Cancellation is an existing Error-frame diagnostic; RunStatus remains
    // the typed status authority. Preserve a real mapped error above rather
    // than replacing it with this clean-cancellation diagnostic.
    let error = match publication {
        run_admission::OriginalPublicationKind::End => {
            emit_settled_to_host(
                tx,
                settled,
                mapped.as_ref().ok(),
                streamed_text,
                cancellation,
            )
            .await;
            return;
        }
        run_admission::OriginalPublicationKind::Cancelled => {
            CodeError::run_cancelled(run_id.to_owned())
        }
        _ => CodeError::Session(format!(
            "Original publication for Run '{run_id}' does not authorize successful End"
        )),
    };
    send_host_event(
        tx,
        AgentEvent::Error {
            message: error.to_string(),
        },
        cancellation,
    )
    .await;
}

#[cfg(test)]
async fn emit_settled(
    tx: &mpsc::Sender<AgentEvent>,
    settled: &anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>>,
    mapped: Option<&AgentResult>,
    streamed_text: bool,
) {
    emit_settled_to_host(tx, settled, mapped, streamed_text, None).await;
}

async fn emit_settled_to_host(
    tx: &mpsc::Sender<AgentEvent>,
    settled: &anyhow::Result<a3s_effect::Settlement<a3s_effect::CodingView>>,
    mapped: Option<&AgentResult>,
    streamed_text: bool,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) {
    match settled {
        Ok(settled) => {
            let text = settled.view.assistant.clone().unwrap_or_default();
            // The streamed deltas already carried this text to the host; the
            // settled replay only adds the End marker. Re-sending the full
            // text as another delta duplicated every message.
            if !streamed_text {
                send_host_event(
                    tx,
                    AgentEvent::TextDelta { text: text.clone() },
                    cancellation,
                )
                .await;
            }
            // The gate already merged the turn's real verification reports
            // into the mapped result; the End marker must carry them (an
            // empty summary hid Verified/Waived outcomes from every client).
            let verification_summary = match mapped {
                Some(result) => crate::verification::VerificationSummary::from_reports(
                    &result.verification_reports,
                ),
                None => crate::verification::VerificationSummary::from_reports(&[]),
            };
            send_host_event(
                tx,
                AgentEvent::End {
                    text,
                    usage: mapped
                        .map(|result| result.usage.clone())
                        .unwrap_or_default(),
                    verification_summary: Box::new(verification_summary),
                    completion_terminal: mapped
                        .map(|result| result.completion.clone())
                        .unwrap_or_default(),
                    forced_stop: mapped.and_then(|result| result.forced_stop),
                    meta: None,
                },
                cancellation,
            )
            .await;
        }
        Err(error) => {
            send_host_event(
                tx,
                AgentEvent::Error {
                    message: error.to_string(),
                },
                cancellation,
            )
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

async fn settle_owned_capability_run(
    pinned: &OwnedPinnedFact,
    outcome: std::result::Result<AgentResult, anyhow::Error>,
) -> std::result::Result<AgentResult, anyhow::Error> {
    let cancelled_before_cleanup = pinned
        .parts
        .surface
        .as_ref()
        .is_some_and(|surface| surface.cancel.is_cancelled());
    let closed = pinned.guard.close().await;
    if let Err(error) = pinned
        .original
        .record_scope_close(run_admission::OriginalScopeKind::Fact, &closed)
    {
        return Err(anyhow::anyhow!("{error}; original result: {outcome:?}"));
    }
    apply_capability_close(&pinned.parts, outcome, cancelled_before_cleanup, closed).await
}

async fn apply_capability_close(
    parts: &FactSession,
    outcome: std::result::Result<AgentResult, anyhow::Error>,
    cancelled_before_cleanup: bool,
    closed: std::result::Result<
        crate::capability::ScopeCloseReport,
        crate::capability::CapabilityRuntimeError,
    >,
) -> std::result::Result<AgentResult, anyhow::Error> {
    let cleanup_succeeded = closed.is_ok();
    let outcome = match (outcome, closed) {
        (Ok(result), Ok(_)) => Ok(result),
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(error), Err(close)) => Err(anyhow::anyhow!(
            "{error:#}; capability Run cleanup also failed: {close}"
        )),
    };
    if let Some(surface) = parts.surface.as_ref().filter(|_| cleanup_succeeded) {
        crate::fact_control::replay_unsettled_mutations(surface);
        let mut ledger = surface
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let observation_cancel = tokio_util::sync::CancellationToken::new();
        let incomplete = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::harness_loop::absorb_open_workspace_children(
                &mut ledger,
                parts.workspace.as_path(),
                &observation_cancel,
            ),
        )
        .await
        .unwrap_or(true);
        if incomplete {
            ledger.set_observation_incomplete(true);
        }
        *surface
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ledger;
        // Local child observation cannot clear durable execution debt. Once
        // children are settled, re-project their exact committed terminals.
        crate::fact_control::replay_unsettled_mutations(surface);
    }
    match outcome {
        Ok(_)
            if !cancelled_before_cleanup
                && parts
                    .surface
                    .as_ref()
                    .is_some_and(|surface| surface.cancel.is_cancelled()) =>
        {
            Err(anyhow::anyhow!(
                "Agent Run interrupted during capability cleanup"
            ))
        }
        Ok(result) => Ok(result),
        Err(error) => Err(error),
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

/// The effect machine mints `budget.denied` when the tool-round budget
/// denies the next call. Exact fact-kind lookup on this turn's slice (since
/// the last `user.message`) — never a text-shape match on the answer.
fn budget_denied_this_turn(parts: &FactSession) -> Option<crate::harness_loop::ForcedStop> {
    let thread = crate::fact_control::thread_for_session(&parts.session_id);
    let facts = crate::fact_control::read_workspace_facts(&parts.workspace, &thread).ok()?;
    let start = facts
        .iter()
        .rposition(|fact| fact.kind == "user.message")
        .unwrap_or(0);
    facts[start..]
        .iter()
        .any(|fact| fact.kind == "budget.denied")
        .then_some(crate::harness_loop::ForcedStop::ToolBudget)
}

fn captured_forced_stop(parts: &FactSession) -> Option<crate::harness_loop::ForcedStop> {
    parts
        .surface
        .as_ref()
        .and_then(|surface| {
            surface
                .forced_stop
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        })
        .or_else(|| budget_denied_this_turn(parts))
}

fn combine_retention_failure(
    outcome: std::result::Result<AgentResult, anyhow::Error>,
    stage: &str,
    error: impl std::fmt::Display,
) -> std::result::Result<AgentResult, anyhow::Error> {
    match outcome {
        Ok(_) => Err(anyhow::anyhow!("{stage} failed: {error}")),
        Err(primary) => Err(anyhow::anyhow!("{primary:#}; {stage} also failed: {error}")),
    }
}

fn gate_result(
    parts: &FactSession,
    mut result: AgentResult,
    goal_attestation: Option<&[crate::verification::VerificationReport]>,
) -> std::result::Result<AgentResult, anyhow::Error> {
    let Some(surface) = &parts.surface else {
        return Ok(result);
    };
    let ledger = surface
        .ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let mut reports = surface
        .reports
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(fresh) = goal_attestation {
        if !ledger.observes_current_workspace(&surface.agent.tool_context_handle().workspace) {
            anyhow::bail!("goal.evidence_stale: workspace effects changed after judgment");
        }
        reports.extend_from_slice(fresh);
    } else {
        surface
            .agent
            .merge_completion_attestor(&ledger, &mut reports);
    }
    if let Ok(mut lock) = surface.reports.lock() {
        *lock = reports.clone();
    }
    result.verification_reports = reports.clone();
    match surface.agent.decide_fact_completion_gate(&ledger, &reports) {
        crate::harness_loop::CompletionGate::Allow(terminal) => {
            // The run closed accountable: mark the log position so the next
            // send's ledger replay starts here instead of inheriting this
            // run's already-verified mutations.
            if let Some(surface) = &parts.surface {
                crate::fact_control::append_turn_complete_fact(surface, &terminal)?;
            }
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
        forced_stop: None,
    }
}

#[cfg(test)]
async fn prepare_pinned(
    session: &AgentSession,
    prompt: &str,
    owned_run: bool,
) -> Result<PinnedFact> {
    // Same capture point as `OwnedConversationCapture`: host reports recorded
    // before this turn must already be on the surface the gate reads.
    let recorded_reports = session.verification_reports();
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
    let streamed_text = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let forced_stop = std::sync::Arc::new(std::sync::Mutex::new(None));
    let _ = &forced_stop;
    let parts = FactSession::from(session)
        .with_executor(agent_loop.tool_executor_handle())
        .with_surface(crate::fact_control::SessionSurface {
            agent: agent_loop,
            session_id: session.session_id.clone(),
            checkpoint,
            events: None,
            pending_memory_telemetry: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            fact_log: session.run_admission.fact_log_slot(),
            streamed_text: streamed_text.clone(),
            forced_stop: forced_stop.clone(),
            cancel: cancellation.clone(),
            transcript: Arc::new(std::sync::Mutex::new(Vec::new())),
            usage: Arc::new(std::sync::Mutex::new(crate::llm::TokenUsage::default())),
            confirmation: session.config.confirmation_manager.clone(),
            run_store: active_run_id
                .as_ref()
                .map(|_| Arc::clone(&session.run_store)),
            run_id: active_run_id.clone(),
            goal_tick: None,
            ledger: Arc::new(std::sync::Mutex::new(
                crate::harness_loop::MutationLedger::default(),
            )),
            reports: Arc::new(std::sync::Mutex::new(recorded_reports)),
            run_control,
            targeted_paths: Arc::new(std::sync::Mutex::new(Vec::new())),
            harness: session.config.harness.clone(),
            host_harness_registry: session.config.host_harness_registry.clone(),
            host_harness_assembler: session.config.host_harness_assembler.clone(),
        });
    // Inherit the mutations of runs that never settled accountable (a
    // graceful interrupt or a gate refusal): the fresh surface must not
    // open with a blank ledger over a changed workspace. A checkpoint
    // resume conservatively merges its cached projection into this seed next.
    if let Some(surface) = parts.surface.as_ref() {
        crate::fact_control::replay_unsettled_mutations(surface);
    }
    if owned_run {
        if let Some(cancel) = parts.surface.as_ref().map(|surface| surface.cancel.clone()) {
            *session.cancel_token.lock().await = Some(cancel);
        }
    }
    Ok(PinnedFact {
        parts,
        capability_run,
        committer,
    })
}

async fn prepare_pre_analysis(
    parts: &FactSession,
    prompt: &str,
    current_goal: Option<&crate::goal_control::DurableGoalDefinitionV1>,
) -> anyhow::Result<()> {
    let Some(surface) = &parts.surface else {
        return Ok(());
    };
    // Pre-analysis is a structured model call. The fact loop discards its
    // result, so running it holds every prompt until that call (and its
    // repair attempts) finish. Explicit planning still publishes below.
    surface
        .agent
        .fact_publish_plan(
            prompt,
            &surface.session_id,
            &surface.events,
            &surface.cancel,
            (surface.run_store.as_deref(), surface.run_id.as_deref()),
            current_goal,
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
        emit_interactive_host_event(
            surface,
            crate::agent::AgentEvent::UserQuestion {
                question_id: pending.question_id.clone(),
                question: pending.question.clone(),
                options: pending.options.clone(),
                allow_free_text: pending.allow_free_text,
                form: None,
            },
        )
        .await;
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
        emit_interactive_host_event(
            surface,
            crate::agent::AgentEvent::ToolEnd {
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
            },
        )
        .await;
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
    emit_interactive_host_event(
        surface,
        AgentEvent::ConfirmationRequired {
            tool_id: pending.tool_call_id.clone(),
            tool_name: pending.name.clone(),
            args: pending.args.clone(),
            timeout_ms: 0,
        },
    )
    .await;
    let reason = Some("Confirmation cancelled".to_string());
    tokio::select! {
        biased;
        _ = surface.cancel.cancelled() => {
            let _ = manager
                .confirm(&pending.tool_call_id, false, reason.clone())
                .await;
            emit_interactive_host_event(surface, AgentEvent::ConfirmationReceived {
                        tool_id: pending.tool_call_id.clone(),
                        approved: false,
                        reason,
                    }).await;
            false
        }
        response = receiver => {
            let response = response.unwrap_or(crate::hitl::ConfirmationResponse {
                approved: false,
                reason: reason.clone(),
            });
            emit_interactive_host_event(surface, AgentEvent::ConfirmationReceived {
                        tool_id: pending.tool_call_id.clone(),
                        approved: response.approved,
                        reason: response.reason.clone(),
                    }).await;
            response.approved
        }
    }
}

#[cfg(test)]
fn seed_exact_recovery(pinned: &PinnedFact, history: &[Message]) -> anyhow::Result<()> {
    seed_exact_recovery_parts(&pinned.parts, history)
}

fn seed_exact_recovery_parts(parts: &FactSession, history: &[Message]) -> anyhow::Result<()> {
    let run = parts.open()?;
    let mut seed = history.to_vec();
    if seed
        .last()
        .is_some_and(|message| message.role == "assistant")
    {
        seed.pop();
    }
    let prompt = history
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(Message::text)
        .unwrap_or_default();
    run.seed_recovery_history(&seed, &prompt)?;
    Ok(())
}

fn exact_recovery_prompt(checkpoint_identity: &str) -> String {
    format!("<resume exact checkpoint={checkpoint_identity}>")
}

fn ensure_distinct_recovery_target(source_run_id: &str, target_run_id: &str) -> Result<()> {
    if source_run_id == target_run_id {
        return Err(CodeError::RunIdentityConflict {
            run_id: target_run_id.to_string(),
        });
    }
    Ok(())
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

pub(crate) async fn exact_run_replay(
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
