//! Shared run identity and invocation assembly for Agent execution.
//!
//! This is the first incremental boundary of the execution coordinator. It
//! owns the pieces that must be identical for blocking and streaming runs:
//! attaching the run control inbox, binding checkpoint identity, and deriving
//! an invocation from one run-owned cancellation token. Event collection and
//! terminal cleanup remain in the mode-specific lifecycle adapters until their
//! state machines can be migrated safely.

use super::{
    run_admission::{self, RunAdmissionLease},
    run_lifecycle::RunControlState,
    AgentSession,
};
use crate::agent::{AgentEvent, AgentLoop, InvocationContext};
use crate::error::{CodeError, Result};
use crate::run_control::RunControlInbox;
use crate::tools::AgentEventBarrier;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;

/// Terminal state selected exactly once for an admitted Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RunTerminalTransition {
    /// Agent execution returned a result without cancellation.
    Completed,
    /// Cancellation won the race with a result or an execution error.
    Cancelled,
    /// Execution failed and the Run can retain a bounded error message.
    Failed(String),
}

/// Shared identity boundary for one admitted Agent Run.
///
/// A coordinator is created after the Run has been reserved and before any
/// provider call is started. Both blocking and streaming execution paths use
/// this value, so run control, checkpoint identity, and cancellation cannot
/// silently diverge between the two modes.
#[derive(Clone)]
pub(super) struct ExecutionCoordinator {
    session_id: String,
    run_id: String,
    cancellation: CancellationToken,
    run_control: Arc<RunControlInbox>,
    run_store: Arc<crate::run::InMemoryRunStore>,
    terminal_settled: Arc<std::sync::atomic::AtomicBool>,
}

impl ExecutionCoordinator {
    /// Acquire a task-scheduler lease using the coordinator's canonical error
    /// mapping. Direct Tool calls use this primitive without taking the
    /// transcript's single-flight Run lease.
    pub(super) async fn acquire_task(
        scheduler: &crate::task_scheduler::TaskScheduler,
        priority: crate::task_scheduler::TaskPriority,
        label: String,
        session_id: &str,
        cancellation: &CancellationToken,
        closed: &AtomicBool,
    ) -> Result<crate::task_scheduler::TaskLease> {
        scheduler
            .acquire(priority, label, cancellation)
            .await
            .map_err(|error| match error {
                crate::task_scheduler::TaskSchedulerError::Cancelled
                    if closed.load(Ordering::Acquire) =>
                {
                    CodeError::SessionClosed {
                        session_id: session_id.to_owned(),
                    }
                }
                crate::task_scheduler::TaskSchedulerError::Cancelled => {
                    CodeError::TaskAdmissionCancelled {
                        session_id: session_id.to_owned(),
                    }
                }
                crate::task_scheduler::TaskSchedulerError::Closed => CodeError::TaskSchedulerClosed,
                crate::task_scheduler::TaskSchedulerError::InvalidConfig(message) => {
                    CodeError::Config(message)
                }
            })
    }

    /// Acquire an optional scheduler lease for control-plane operations that
    /// may be constructed without a Session-owned scheduler (for example,
    /// low-level direct-tool tests).
    pub(super) async fn acquire_optional_task(
        scheduler: Option<&crate::task_scheduler::TaskScheduler>,
        priority: crate::task_scheduler::TaskPriority,
        label: String,
        session_id: &str,
        cancellation: &CancellationToken,
        closed: &AtomicBool,
    ) -> Result<Option<crate::task_scheduler::TaskLease>> {
        match scheduler {
            Some(scheduler) => {
                Self::acquire_task(scheduler, priority, label, session_id, cancellation, closed)
                    .await
                    .map(Some)
            }
            None => Ok(None),
        }
    }

    /// Admit one session operation before it can inspect or mutate run state.
    ///
    /// Admission is deliberately part of the coordinator boundary so every
    /// public execution entrypoint uses the same close check, task-scheduler
    /// lease, and single-flight semantics.
    pub(super) async fn admit(
        session: &AgentSession,
        operation: &'static str,
    ) -> Result<RunAdmissionLease> {
        if session.is_closed() {
            return Err(CodeError::SessionClosed {
                session_id: session.session_id.clone(),
            });
        }
        let lease = session.run_admission.try_acquire(&session.session_id)?;
        let label = format!("{}:{operation}", session.session_id);
        let task_lease = Self::acquire_task(
            &session.task_scheduler,
            session.task_priority,
            label,
            &session.session_id,
            &session.session_cancel,
            &session.closed,
        )
        .await?;
        if session.is_closed() {
            return Err(CodeError::SessionClosed {
                session_id: session.session_id.clone(),
            });
        }
        Ok(lease.attach_task_lease(task_lease))
    }

    /// Keep a stream's admission lease until its worker and event forwarder
    /// have both settled. The public stream handle may be dropped without
    /// accidentally admitting a second operation while detached work remains.
    pub(super) fn supervise_stream(
        handle: JoinHandle<()>,
        worker_aborts: Vec<AbortHandle>,
        lease: RunAdmissionLease,
    ) -> JoinHandle<()> {
        run_admission::guard_stream_handle(handle, worker_aborts, lease)
    }

    /// Attach one run's control and checkpoint identity to its pinned loop.
    pub(super) async fn prepare(
        session: &AgentSession,
        run_id: impl Into<String>,
        agent_loop: &mut AgentLoop,
        cancellation: CancellationToken,
    ) -> Self {
        let run_id = run_id.into();
        let run_control = RunControlInbox::new_with_hook_executor(
            session.session_id.clone(),
            run_id.clone(),
            cancellation.clone(),
            agent_loop.hook_executor(),
        );
        RunControlState::from_session(session)
            .attach_run_control(&run_id, run_control.clone())
            .await;
        agent_loop.set_checkpoint_run(&run_id);

        Self {
            session_id: session.session_id.clone(),
            run_id,
            cancellation,
            run_control,
            run_store: Arc::clone(&session.run_store),
            terminal_settled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub(super) fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Select the terminal state for one execution result.
    ///
    /// Cancellation is sampled from the coordinator-owned token before any
    /// lifecycle adapter clears the Session's active-token slot. This keeps
    /// blocking and streaming paths identical when cancellation races with a
    /// provider result or error.
    pub(super) fn terminal_for(
        &self,
        succeeded: bool,
        error: Option<String>,
    ) -> RunTerminalTransition {
        if self.cancellation.is_cancelled() {
            RunTerminalTransition::Cancelled
        } else if succeeded {
            RunTerminalTransition::Completed
        } else {
            RunTerminalTransition::Failed(error.unwrap_or_else(|| "execution failed".to_owned()))
        }
    }

    /// Apply the one terminal RunStore transition. Successful Runs are already
    /// finalized by the normal `End` event path, so `Completed` intentionally
    /// performs no second write. The atomic guard makes accidental duplicate
    /// cleanup calls harmless and prevents a late failure from rewriting a
    /// terminal cancellation.
    pub(super) async fn settle_terminal(&self, transition: RunTerminalTransition) {
        if self
            .terminal_settled
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            tracing::debug!(run_id = %self.run_id, "Ignored duplicate Run terminal transition");
            return;
        }
        match transition {
            RunTerminalTransition::Completed => {}
            RunTerminalTransition::Cancelled => {
                let _ = self.run_store.mark_cancelled(&self.run_id).await;
            }
            RunTerminalTransition::Failed(error) => {
                let _ = self.run_store.mark_failed(&self.run_id, error).await;
            }
        }
    }

    /// Build the invocation context used by blocking and streaming workers.
    pub(super) fn invocation(
        &self,
        agent_loop: &AgentLoop,
        runtime_tx: Option<mpsc::Sender<AgentEvent>>,
        agent_event_tx: broadcast::Sender<AgentEvent>,
        agent_event_barrier: AgentEventBarrier,
    ) -> InvocationContext {
        agent_loop
            .invocation_context(
                self.run_id.clone(),
                Some(&self.session_id),
                runtime_tx,
                self.cancellation.clone(),
            )
            .with_agent_events(agent_event_tx, agent_event_barrier)
            .with_run_control(self.run_control.clone())
    }

    #[cfg(test)]
    fn identity(&self) -> (&str, &str) {
        (&self.session_id, &self.run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_stores_one_session_and_run_identity() {
        let cancellation = CancellationToken::new();
        let run_control = RunControlInbox::new(
            "session-1".to_owned(),
            "run-1".to_owned(),
            cancellation.clone(),
        );
        let coordinator = ExecutionCoordinator {
            session_id: "session-1".to_owned(),
            run_id: "run-1".to_owned(),
            cancellation,
            run_control,
            run_store: Arc::new(crate::run::InMemoryRunStore::new()),
            terminal_settled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

        assert_eq!(coordinator.identity(), ("session-1", "run-1"));
    }

    #[test]
    fn terminal_transition_is_deterministic_and_cancellation_wins() {
        let cancellation = CancellationToken::new();
        let run_control = RunControlInbox::new(
            "session-1".to_owned(),
            "run-1".to_owned(),
            cancellation.clone(),
        );
        let coordinator = ExecutionCoordinator {
            session_id: "session-1".to_owned(),
            run_id: "run-1".to_owned(),
            cancellation: cancellation.clone(),
            run_control,
            run_store: Arc::new(crate::run::InMemoryRunStore::new()),
            terminal_settled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

        assert_eq!(
            coordinator.terminal_for(true, None),
            RunTerminalTransition::Completed
        );
        assert_eq!(
            coordinator.terminal_for(false, Some("boom".to_owned())),
            RunTerminalTransition::Failed("boom".to_owned())
        );
        cancellation.cancel();
        assert_eq!(
            coordinator.terminal_for(true, None),
            RunTerminalTransition::Cancelled
        );
    }

    #[tokio::test]
    async fn terminal_transition_updates_the_run_store_once() {
        let cancellation = CancellationToken::new();
        let run_control = RunControlInbox::new(
            "session-1".to_owned(),
            "run-1".to_owned(),
            cancellation.clone(),
        );
        let run_store = Arc::new(crate::run::InMemoryRunStore::new());
        run_store
            .create_run_with_id("run-1".to_owned(), "session-1", "prompt")
            .await;
        let coordinator = ExecutionCoordinator {
            session_id: "session-1".to_owned(),
            run_id: "run-1".to_owned(),
            cancellation,
            run_control,
            run_store: Arc::clone(&run_store),
            terminal_settled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };

        coordinator
            .settle_terminal(RunTerminalTransition::Failed("boom".to_owned()))
            .await;
        coordinator
            .settle_terminal(RunTerminalTransition::Cancelled)
            .await;

        let snapshot = run_store.snapshot("run-1").await.unwrap();
        assert_eq!(snapshot.status, crate::run::RunStatus::Failed);
        assert_eq!(snapshot.error.as_deref(), Some("boom"));
    }
}
