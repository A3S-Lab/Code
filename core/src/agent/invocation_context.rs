//! Immutable identity, cancellation, event, and governance scope for one run.

use super::{AgentEvent, AgentLoop};
use crate::budget::BudgetGuard;
use crate::tools::{AgentEventBarrier, ToolContext};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

/// Governance resources snapshotted when a run starts.
///
/// Runtime overrides are resolved before this value is created. Keeping the
/// snapshot beside the run identity prevents helper LLM calls from silently
/// consulting a different budget scope halfway through a run.
#[derive(Clone, Default)]
pub(crate) struct InvocationGovernance {
    budget_guard: Option<Arc<dyn BudgetGuard>>,
}

impl InvocationGovernance {
    pub(crate) fn budget_guard(&self) -> Option<&Arc<dyn BudgetGuard>> {
        self.budget_guard.as_ref()
    }
}

/// The single source of truth for metadata shared by every operation in a run.
#[derive(Clone)]
pub(crate) struct InvocationContext {
    run_id: Arc<str>,
    session_id: Arc<str>,
    cancellation: CancellationToken,
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    agent_event_tx: Option<broadcast::Sender<AgentEvent>>,
    agent_event_barrier: Option<AgentEventBarrier>,
    governance: InvocationGovernance,
}

impl std::fmt::Debug for InvocationContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvocationContext")
            .field("run_id", &self.run_id)
            .field("session_id", &self.session_id)
            .field("cancelled", &self.cancellation.is_cancelled())
            .field("has_event_tx", &self.event_tx.is_some())
            .field("has_agent_event_tx", &self.agent_event_tx.is_some())
            .field(
                "has_agent_event_barrier",
                &self.agent_event_barrier.is_some(),
            )
            .field("has_budget_guard", &self.governance.budget_guard.is_some())
            .finish()
    }
}

impl InvocationContext {
    pub(crate) fn new(
        run_id: impl Into<Arc<str>>,
        session_id: impl Into<Arc<str>>,
        cancellation: CancellationToken,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        governance: InvocationGovernance,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            session_id: session_id.into(),
            cancellation,
            event_tx,
            agent_event_tx: None,
            agent_event_barrier: None,
            governance,
        }
    }

    /// Bind high-level tool events to the run that owns this invocation.
    ///
    /// The session-level channel remains available for direct session tools,
    /// but agent-run tools must use this sender so a background child from an
    /// earlier run cannot be attributed to a later run in the same session.
    pub(crate) fn with_agent_events(
        mut self,
        tx: broadcast::Sender<AgentEvent>,
        barrier: AgentEventBarrier,
    ) -> Self {
        self.agent_event_tx = Some(tx);
        self.agent_event_barrier = Some(barrier);
        self
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(crate) fn session_id_option(&self) -> Option<&str> {
        (!self.session_id.is_empty()).then_some(self.session_id())
    }

    pub(crate) fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub(crate) fn event_tx(&self) -> &Option<mpsc::Sender<AgentEvent>> {
        &self.event_tx
    }

    pub(crate) fn governance(&self) -> &InvocationGovernance {
        &self.governance
    }

    /// Install run identity and cancellation into a tool context before any
    /// direct, queued, nested, or delegated tool invocation begins.
    pub(crate) fn bind_tool_context(&self, mut context: ToolContext) -> ToolContext {
        if !self.session_id.is_empty() {
            context = context.with_session_id(self.session_id.to_string());
        }
        if let Some(tx) = &self.agent_event_tx {
            context = context.with_agent_event_tx(tx.clone());
        }
        if let Some(barrier) = &self.agent_event_barrier {
            context = context.with_agent_event_barrier(barrier.clone());
        }
        context.with_cancellation(self.cancellation.clone())
    }

    /// Clone an agent loop and install this invocation's run-owned tool scope.
    /// All planning, model-tool, nested-tool, and queue paths spawned from the
    /// clone inherit the same event sender and acknowledgement barrier.
    pub(crate) fn bind_agent_loop(&self, agent: &AgentLoop) -> AgentLoop {
        let mut scoped = agent.clone();
        scoped.tool_context = self.bind_tool_context(scoped.tool_context);
        scoped
    }
}

impl AgentLoop {
    pub(crate) fn invocation_context(
        &self,
        run_id: impl Into<Arc<str>>,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        cancellation: CancellationToken,
    ) -> InvocationContext {
        InvocationContext::new(
            run_id,
            Arc::<str>::from(session_id.unwrap_or("")),
            cancellation,
            event_tx,
            InvocationGovernance {
                budget_guard: self.config.budget_guard.clone(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn binding_installs_run_cancellation_and_session_identity() {
        let token = CancellationToken::new();
        let context = InvocationContext::new(
            Arc::<str>::from("run-1"),
            Arc::<str>::from("session-1"),
            token.clone(),
            None,
            InvocationGovernance::default(),
        );
        let tool_context = context.bind_tool_context(ToolContext::new(PathBuf::from(".")));

        assert_eq!(tool_context.session_id.as_deref(), Some("session-1"));
        assert!(!tool_context.is_cancelled());
        token.cancel();
        assert!(tool_context.is_cancelled());
    }
}
