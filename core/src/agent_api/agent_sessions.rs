//! Agent-to-session factory operations.
//!
//! `Agent` is workspace-independent; this module owns the transition from an
//! agent config/runtime to a workspace-bound `AgentSession`, including resume.
//! It also implements the agent-side session registry: every newly built
//! session registers a `Weak<SessionCloseHandle>` so `Agent::close_session`
//! and `Agent::close` can reach in and tear it down.

use super::{
    agent_binding, session_builder, session_close::SessionCloseHandle, session_config,
    session_persistence, Agent, AgentSession, SessionOptions,
};
use crate::error::{CodeError, Result};
use std::sync::{Arc, Weak};

pub(super) async fn refresh_mcp_tools(agent: &Agent) -> Result<()> {
    if let Some(mcp) = &agent.global_mcp {
        let fresh = mcp.get_all_tools().await;
        *agent
            .global_mcp_tools
            .lock()
            .expect("global_mcp_tools lock poisoned") = fresh;
    }
    Ok(())
}

pub(super) fn create_session(
    agent: &Agent,
    workspace: impl Into<String>,
    options: Option<SessionOptions>,
) -> Result<AgentSession> {
    bail_if_agent_closed(agent)?;

    let merged_opts = session_builder::prepare_session_options(agent, options.unwrap_or_default());
    let session_id = merged_opts
        .session_id
        .as_deref()
        .expect("prepare_session_options assigns session_id");
    let llm_client = session_config::resolve_session_llm_client(
        &agent.code_config,
        &merged_opts,
        Some(session_id),
    )?;

    session_builder::build_agent_session(agent, workspace.into(), llm_client, &merged_opts)
}

/// Register a freshly built session's close handle into the parent agent's
/// registry. Called by `session_builder::build_agent_session` immediately
/// after the handle is constructed.
///
/// Uses `Weak` so the registry doesn't keep the handle alive; when the
/// caller drops their `AgentSession`, the handle's `Arc` count goes to
/// zero, the handle drops, and the `Weak` in the registry becomes
/// dangling. Dead entries are pruned lazily on the next
/// [`list_sessions`] / [`close_session`] access.
pub(super) fn register_session(agent: &Agent, handle: &Arc<SessionCloseHandle>) {
    let weak = Arc::downgrade(handle);
    let id = handle.session_id.clone();
    let mut sessions = agent
        .sessions
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    sessions.insert(id, weak);
}

fn bail_if_agent_closed(agent: &Agent) -> Result<()> {
    if agent.closed.load(std::sync::atomic::Ordering::Acquire) {
        return Err(CodeError::SessionClosed {
            session_id: "<agent-closed>".to_string(),
        });
    }
    Ok(())
}

pub(super) async fn list_sessions(agent: &Agent) -> Vec<String> {
    let mut sessions = agent
        .sessions
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    sessions.retain(|_, weak| weak.strong_count() > 0);
    let mut ids: Vec<String> = sessions.keys().cloned().collect();
    ids.sort();
    ids
}

pub(super) async fn close_session(agent: &Agent, session_id: &str) -> bool {
    let handle: Option<Arc<SessionCloseHandle>> = {
        let mut sessions = agent
            .sessions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        sessions.retain(|_, weak| weak.strong_count() > 0);
        sessions.get(session_id).and_then(Weak::upgrade)
    };
    match handle {
        Some(handle) => {
            let was_open = !handle.is_closed();
            handle.close().await;
            was_open
        }
        None => false,
    }
}

pub(super) async fn close_agent(agent: &Agent) {
    // Mark closed *before* iterating so concurrent `session()` calls fail fast.
    if agent.closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }

    // Snapshot live handles so we can close them outside the registry lock.
    // Also prune dead `Weak` entries here: a high-churn create-and-drop
    // workload that never calls `list_sessions`/`close_session` would
    // otherwise leave dangling entries in the registry until agent close.
    let handles: Vec<Arc<SessionCloseHandle>> = {
        let mut sessions = agent
            .sessions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        sessions.retain(|_, weak| weak.strong_count() > 0);
        sessions.values().filter_map(Weak::upgrade).collect()
    };
    for handle in handles {
        handle.close().await;
    }

    // Tear down global MCP connections so background workers exit.
    if let Some(mcp) = &agent.global_mcp {
        for name in mcp.list_connected().await {
            if let Err(e) = mcp.disconnect(&name).await {
                tracing::warn!(
                    server = %name,
                    error = %e,
                    "Failed to disconnect MCP server during Agent::close"
                );
            }
        }
    }
}

pub(super) fn create_session_for_agent(
    agent: &Agent,
    workspace: impl Into<String>,
    def: &crate::subagent::AgentDefinition,
    extra: Option<SessionOptions>,
) -> Result<AgentSession> {
    let opts = agent_binding::apply_agent_definition(extra.unwrap_or_default(), def);
    create_session(agent, workspace, Some(opts))
}

pub(super) fn resume_session(
    agent: &Agent,
    session_id: &str,
    options: SessionOptions,
) -> Result<AgentSession> {
    bail_if_agent_closed(agent)?;

    let store = options.session_store.clone().ok_or_else(|| {
        crate::error::CodeError::Session(
            "resume_session requires a session_store in SessionOptions".to_string(),
        )
    })?;

    let data = session_persistence::load_session_data(&store, session_id)?;
    let opts = session_persistence::apply_persisted_runtime_options(options, &data);
    let opts = session_builder::prepare_session_options(agent, opts);
    let session_id = data.id.clone();
    let workspace = data.config.workspace.clone();
    let llm_client =
        session_config::resolve_session_llm_client(&agent.code_config, &opts, Some(&session_id))?;
    let session = session_builder::build_agent_session(agent, workspace, llm_client, &opts)?;
    session_persistence::restore_persisted_session_state(&session, &store, data)?;

    Ok(session)
}
