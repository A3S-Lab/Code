//! Agent loop construction for a session.
//!
//! The public session facade should not know how hooks, live tool definitions,
//! and optional queue adapters are threaded into an `AgentLoop`. This module is
//! the runtime seam for constructing the executable loop from session state.

use super::AgentSession;
use crate::agent::AgentLoop;
use std::sync::Arc;

pub(super) fn build_agent_loop(session: &AgentSession) -> AgentLoop {
    let mut config = session.config.clone();
    config.hook_engine = Some(match &session.hook_executor {
        Some(executor) => executor.clone(),
        None => Arc::clone(&session.hook_engine) as Arc<dyn crate::hooks::HookExecutor>,
    });

    // Dynamic MCP additions mutate the executor after session construction, so
    // every run snapshots live definitions instead of using the stale config copy.
    config.tools = session.tool_executor.definitions();

    // Runtime budget-guard override (set via AgentSession::set_budget_guard)
    // takes precedence over the value baked in at session-build time.
    // Used by Node SDK where the JS callable cannot live inside
    // value-typed SessionOptions.
    if let Some(runtime_guard) = session.budget_guard() {
        config.budget_guard = Some(runtime_guard);
    }

    let mut agent_loop = AgentLoop::new(
        session.llm_client.clone(),
        session.tool_executor.clone(),
        session.tool_context.clone(),
        config,
    );
    if let Some(queue) = &session.command_queue {
        agent_loop = agent_loop.with_queue(Arc::clone(queue));
    }
    // Wire per-tool-round checkpointing when the session has a store.
    // The run id is bound later by the caller via
    // `AgentLoop::set_checkpoint_run` once `start_run` returns.
    if let Some(store) = &session.session_store {
        let sink = std::sync::Arc::new(crate::loop_checkpoint::SessionStoreCheckpointSink::new(
            std::sync::Arc::clone(store),
        ));
        agent_loop = agent_loop.with_checkpoint_sink(sink);
    }
    agent_loop
}
