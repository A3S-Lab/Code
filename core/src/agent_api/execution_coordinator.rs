//! Shared run identity and invocation assembly for Agent execution.
//!
//! This is the first incremental boundary of the execution coordinator. It
//! owns the pieces that must be identical for blocking and streaming runs:
//! attaching the run control inbox, binding checkpoint identity, and deriving
//! an invocation from one run-owned cancellation token. Event collection and
//! terminal cleanup remain in the mode-specific lifecycle adapters until their
//! state machines can be migrated safely.

use super::{run_lifecycle::RunControlState, AgentSession};
use crate::agent::{AgentEvent, AgentLoop, InvocationContext};
use crate::run_control::RunControlInbox;
use crate::tools::AgentEventBarrier;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

/// Shared identity boundary for one admitted Agent Run.
///
/// A coordinator is created after the Run has been reserved and before any
/// provider call is started. Both blocking and streaming execution paths use
/// this value, so run control, checkpoint identity, and cancellation cannot
/// silently diverge between the two modes.
pub(super) struct ExecutionCoordinator {
    session_id: String,
    run_id: String,
    cancellation: CancellationToken,
    run_control: Arc<RunControlInbox>,
}

impl ExecutionCoordinator {
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
        };

        assert_eq!(coordinator.identity(), ("session-1", "run-1"));
    }
}
