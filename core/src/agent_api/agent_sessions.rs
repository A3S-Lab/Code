//! Agent-to-session factory operations.
//!
//! `Agent` is workspace-independent; this module owns the transition from an
//! agent config/runtime to a workspace-bound `AgentSession`, including resume.

use super::{
    agent_binding, session_builder, session_config, session_persistence, Agent, AgentSession,
    SessionOptions,
};
use crate::error::Result;

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
