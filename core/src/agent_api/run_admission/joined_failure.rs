//! The original guardian's narrow failed-input/Error continuation.
//!
//! This port is frozen before worker spawn and dies before child/committer joins.
//! It cannot append facts, close capabilities, clean slots or release admission.

use super::*;
use crate::agent::AgentEvent;
use crate::hooks::HookExecutor;
use crate::run_control::RunControlInbox;
use tokio::sync::mpsc;

pub(in crate::agent_api) struct OriginalFailurePublisher {
    run_store: Arc<crate::run::InMemoryRunStore>,
    hook: Option<Arc<dyn HookExecutor>>,
    session_id: String,
    events: Option<mpsc::Sender<AgentEvent>>,
}

/// Actual continuation result; a panic payload is retained, never a release.
pub(super) struct OriginalFailureContinuation {
    pub(super) actual: std::thread::Result<Result<()>>,
}
impl OriginalFailureContinuation {
    pub(super) fn unproved(&self) -> bool {
        !matches!(&self.actual, Ok(Ok(())))
    }
}
impl std::fmt::Debug for OriginalFailureContinuation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.actual {
            Ok(Ok(())) => f.write_str("Returned(Ok)"),
            Ok(Err(error)) => f.debug_tuple("Returned").field(error).finish(),
            Err(_) => f.write_str("Panicked"),
        }
    }
}

impl OriginalFailurePublisher {
    pub(in crate::agent_api) fn freeze(
        run_store: Arc<crate::run::InMemoryRunStore>,
        hook: Option<Arc<dyn HookExecutor>>,
        session_id: String,
        events: Option<mpsc::Sender<AgentEvent>>,
    ) -> Self {
        Self {
            run_store,
            hook,
            session_id,
            events,
        }
    }
}

diagnostic_set!(OriginalFailureWire, "original failure wire", {
    NotApplicable => "not_applicable", NotAttempted => "not_attempted",
    Queued => "queued", Full => "full", Closed => "closed",
});

impl OriginalExecutionContext {
    /// Separate authority: never relax the canonical worker-only validator.
    fn joined_guardian_state(&self) -> Result<std::sync::MutexGuard<'_, OriginalOwnerState>> {
        let state = self
            .state
            .lock()
            .map_err(|_| owner_error("poisoned joined owner"))?;
        if state.phase != EffectPhase::WorkerFinished
            || state.worker_join.is_none()
            || state.worker_id.is_none()
            || state.guardian_id.is_none()
            || state.guardian_id != tokio::task::try_id()
            || state.guardian_id == state.worker_id
            || state.release.retired
        {
            return Err(owner_error(
                "failure continuation requires the actual original joined guardian",
            ));
        }
        Ok(state)
    }

    fn joined_failure_inbox(
        &self,
        publisher: &OriginalFailurePublisher,
    ) -> Result<Arc<RunControlInbox>> {
        let state = self.joined_guardian_state()?;
        let OriginalRunBinding::Created { snapshot, .. } = &state.run else {
            return Err(owner_error("joined failure has no original Created Run"));
        };
        let inbox = state
            .release
            .inbox
            .as_ref()
            .ok_or_else(|| owner_error("joined failure has no original inbox"))?;
        if publisher.session_id != self.session_id
            || snapshot.session_id != self.session_id
            || inbox.snapshot_run_id() != snapshot.id
            || state.release.hooks_configured != Some(publisher.hook.is_some())
        {
            return Err(owner_error("foreign frozen failure publisher or inbox"));
        }
        Ok(Arc::clone(inbox))
    }

    fn begin_joined_failure(
        &self,
        publisher: &OriginalFailurePublisher,
    ) -> Result<Option<(OriginalPublicationTarget, OriginalPublicationObservation)>> {
        let mut state = self.joined_guardian_state()?;
        // Even Entered/unknown prior publication is never retried or replaced.
        if state.release.sealed
            || state.release.publication.is_some()
            || state.release.publication_complete
        {
            return Ok(None);
        }
        let OriginalRunBinding::Created { snapshot, .. } = &state.run else {
            return Err(owner_error("missing joined failure publication target"));
        };
        let run_id = snapshot.id.clone();
        state.release.cancelled_at_seal = self.original_cancel.is_cancelled();
        state.release.sealed = true;
        state.release.failure_wire = Some(if publisher.events.is_some() {
            OriginalFailureWire::NotAttempted
        } else {
            OriginalFailureWire::NotApplicable
        });
        let target = OriginalPublicationTarget {
            session_id: self.session_id.clone(),
            run_id: Some(run_id),
            cancelled: state.release.cancelled_at_seal,
            cancel_origin: state.release.cancel_origin,
        };
        let actual = OriginalPublicationObservation {
            kind: OriginalPublicationKind::Error,
            terminal: OriginalStoreObservation::NotAttempted,
            diagnostic: if target.cancelled {
                OriginalStoreObservation::NotAttempted
            } else {
                OriginalStoreObservation::NotApplicable
            },
            cancellation_hook: if target.cancel_origin.is_some() && publisher.hook.is_some() {
                OriginalHookObservation::NotAttempted
            } else {
                OriginalHookObservation::NotApplicable
            },
            event_hook: if publisher.hook.is_some() {
                OriginalHookObservation::NotAttempted
            } else {
                OriginalHookObservation::NotApplicable
            },
        };
        retain_publication_progress(&mut state, &actual)?;
        Ok(Some((target, actual)))
    }

    fn retain_joined_publication(&self, actual: &OriginalPublicationObservation) -> Result<()> {
        let mut state = self.joined_guardian_state()?;
        retain_publication_progress(&mut state, actual)
    }

    pub(super) async fn continue_joined_failure(
        &self,
        mut publisher: OriginalFailurePublisher,
    ) -> Result<()> {
        {
            let state = self.joined_guardian_state()?;
            if publisher.session_id != self.session_id
                || state.release.hooks_configured != Some(publisher.hook.is_some())
            {
                return Err(owner_error("foreign frozen failure publisher"));
            }
            if matches!(state.run, OriginalRunBinding::NoRun)
                && state.release.no_inbox
                && state.release.inbox.is_none()
                && publication_proved(&state, &self.session_id)
            {
                // A positively completed NoRun has no control or Error work.
                // Returning drops the transient publisher before child joins.
                return Ok(());
            }
        }
        let inbox = self.joined_failure_inbox(&publisher)?;
        let now_ms = super::super::session_clock::now_ms();
        let decided = inbox
            .persist_pending(true, now_ms, |_| {
                Err(a3s_effect::ActorError::Defect(
                    "original execution owner ended before input retention".into(),
                ))
            })
            .await;
        {
            let mut state = self.joined_guardian_state()?;
            if !state
                .release
                .inbox
                .as_ref()
                .is_some_and(|actual| Arc::ptr_eq(actual, &inbox))
            {
                return Err(owner_error("foreign final original inbox observation"));
            }
            // The final counts come directly from that exact transaction.
            state.release.controls = Some(decided.pending);
        }
        let publication = self.begin_joined_failure(&publisher)?;
        let event = AgentEvent::Error { message: "original execution worker ended before terminal settlement; unretained controls failed".into() };
        if let Some((target, mut actual)) = publication {
            let run_id = target
                .run_id
                .as_deref()
                .ok_or_else(|| owner_error("missing failed Run"))?;
            actual.terminal = OriginalStoreObservation::Entered;
            self.retain_joined_publication(&actual)?;
            let returned = if target.cancelled {
                publisher
                    .run_store
                    .settle_terminal(run_id, crate::run::RunTerminalTransition::Cancelled)
                    .await
            } else {
                publisher
                    .run_store
                    .record_event(run_id, event.clone())
                    .await
            };
            actual.terminal = OriginalStoreObservation::Returned(Box::new(returned));
            self.retain_joined_publication(&actual)?;
            if target.cancelled {
                actual.diagnostic = OriginalStoreObservation::Entered;
                self.retain_joined_publication(&actual)?;
                actual.diagnostic = OriginalStoreObservation::Returned(Box::new(
                    publisher
                        .run_store
                        .record_event(run_id, event.clone())
                        .await,
                ));
                self.retain_joined_publication(&actual)?;
            }
            // Store-before-wire; no wire await, retry or delivery fabrication.
            if let Some(sender) = publisher.events.take() {
                let wire = match sender.try_send(event.clone()) {
                    Ok(()) => OriginalFailureWire::Queued,
                    Err(mpsc::error::TrySendError::Full(_)) => OriginalFailureWire::Full,
                    Err(mpsc::error::TrySendError::Closed(_)) => OriginalFailureWire::Closed,
                };
                drop(sender);
                self.joined_guardian_state()?.release.failure_wire = Some(wire);
            }
            // Drop the strong sender before ANY observational await, including
            // receipt post-hooks. A blocked observer cannot prevent host EOF.
            drop(publisher.events.take());
            inbox.observe_decisions(&decided.decisions).await;
            if let Some(hook) = &publisher.hook {
                if target.cancel_origin.is_some() {
                    actual.cancellation_hook = OriginalHookObservation::Entered;
                    self.retain_joined_publication(&actual)?;
                    hook.record_run_cancelled(
                        run_id,
                        &target.session_id,
                        Some("cancelled original Run"),
                    )
                    .await;
                    actual.cancellation_hook = OriginalHookObservation::Returned;
                    self.retain_joined_publication(&actual)?;
                }
                actual.event_hook = OriginalHookObservation::Entered;
                self.retain_joined_publication(&actual)?;
                hook.record_agent_event(&event, run_id, &target.session_id)
                    .await;
                actual.event_hook = OriginalHookObservation::Returned;
                self.retain_joined_publication(&actual)?;
            }
            let mut state = self.joined_guardian_state()?;
            retain_publication_progress(&mut state, &actual)?;
            state.release.publication_complete = true;
        } else {
            // Original worker already began/published; never duplicate it.
            drop(publisher.events.take());
            inbox.observe_decisions(&decided.decisions).await;
        }
        drop(publisher);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
