use super::*;

// A concrete request remains in the original worker through preparation. It is
// not a future/job hosting port and carries no Session or admission authority.
pub(super) enum OwnedConversationRequest {
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

pub(super) struct OwnedConversationCapture {
    pub(super) preparation: super::super::agent_loop_runtime::OwnedPreparationCapture,
    pub(super) coordinator: super::super::execution_coordinator::OwnedCoordinatorInputs,
    pub(super) controls: RunControlState,
    pub(super) cleanup: super::super::runtime_events::RunCleanupState,
    pub(super) finish: TurnFinish,
    pub(super) parts: FactSession,
    pub(super) config: crate::agent::AgentConfig,
    pub(super) commands: command_runtime::OwnedCommandInputs,
    pub(super) init_warning: Option<String>,
    /// Host reports already on the session when this turn was captured.
    /// Each send builds a fresh fact surface, and the completion gate reads
    /// that surface. A `record_verification_reports` between turns is visible
    /// only when this snapshot seeds `surface.reports`.
    pub(super) recorded_reports: Vec<crate::verification::VerificationReport>,
}

impl OwnedConversationCapture {
    pub(super) fn capture(session: &AgentSession, command_capable: bool) -> Result<Self> {
        let cancellation = session.session_cancel.child_token();
        let preparation = super::super::agent_loop_runtime::capture_owned_preparation(
            session,
            cancellation.clone(),
            command_capable,
        )?;
        // Do not reread the mutable permission override after the original
        // capture boundary; use its exact facet in the passive constructor.
        let parts = FactSession::from_permission(session, preparation.permission.clone());
        Ok(Self {
            preparation,
            coordinator:
                super::super::execution_coordinator::OwnedCoordinatorInputs::capture_original(
                    session,
                ),
            controls: RunControlState::from_session(session),
            cleanup: super::super::runtime_events::RunCleanupState::from_session(session, ""),
            finish: TurnFinish::from_session(session, cancellation, None),
            parts,
            config: session.config.clone(),
            commands: command_runtime::OwnedCommandInputs::capture_original(session),
            init_warning: session.init_warning.clone(),
            recorded_reports: session.verification_reports(),
        })
    }
}

pub(super) enum OwnedReady {
    Fact {
        run: crate::run::RunHandle,
        snapshot: crate::run::RunSnapshot,
    },
    Command,
    Replayed(crate::run::RunSnapshot),
}

pub(super) struct OwnedObservation {
    pub(super) ready: tokio::sync::oneshot::Receiver<Result<OwnedReady>>,
    pub(super) result: tokio::sync::oneshot::Receiver<Result<AgentResult>>,
    pub(super) events: mpsc::Receiver<AgentEvent>,
    pub(super) guardian: JoinHandle<()>,
    pub(super) disposition: tokio::sync::oneshot::Receiver<run_admission::OwnerDisposition>,
    pub(super) cancellation: tokio_util::sync::CancellationToken,
}

pub(super) struct CancelBlockingObserver(Option<tokio_util::sync::CancellationToken>);
impl Drop for CancelBlockingObserver {
    fn drop(&mut self) {
        if let Some(actual) = self.0.take() {
            actual.cancel();
        }
    }
}

pub(super) fn launch_owned_conversation(
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

pub(super) async fn observe_owned_blocking(observation: OwnedObservation) -> Result<AgentResult> {
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

pub(super) async fn observe_owned_stream(observation: OwnedObservation) -> Result<AgentRunStream> {
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

pub(super) async fn observe_owned_detached(observation: OwnedObservation) -> Result<AgentRunSpawn> {
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
