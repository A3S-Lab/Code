//! Bridge handlers for observable filesystem-first daemon lifecycle operations.

use super::*;

pub(super) async fn start(state: &BridgeState, params: &Value) -> Result<Value, BridgeFailure> {
    let agent_id: String = required(params, "agent_id")?;
    let dir: String = required(params, "dir")?;
    let workspace: String = required(params, "workspace")?;
    let options = state
        .optional_session_options(optional::<BridgeSessionOptions>(params, "options")?)
        .await?;
    let agent_dir = a3s_code_core::config::AgentDir::load(&dir)
        .map_err(|error| BridgeFailure::new("CONFIG_ERROR", error.to_string()))?;
    let agent = state.agent(&agent_id).await?;
    let handle = spawn_agent_dir_daemon(agent, agent_dir, workspace, options)?;
    if let Err(error) = handle.wait_ready().await {
        return Err(serve_failure(&handle, error));
    }
    let serve_handle = state.handle("serve");
    state
        .serve_handles
        .write()
        .await
        .insert(serve_handle.clone(), handle);
    Ok(json!({ "serve_handle": serve_handle }))
}

pub(super) async fn status(state: &BridgeState, params: &Value) -> Result<Value, BridgeFailure> {
    let serve_handle: String = required(params, "serve_handle")?;
    let handles = state.serve_handles.read().await;
    let handle = handles.get(&serve_handle).ok_or_else(|| {
        BridgeFailure::new(
            "NOT_FOUND",
            format!("serve handle {serve_handle:?} was not found"),
        )
    })?;
    let status = handle.status();
    Ok(json!({
        "phase": status.phase.as_str(),
        "failure_code": handle.failure_code(),
        "ready": handle.is_ready(),
        "stopped": handle.is_stopped(),
    }))
}

pub(super) async fn stop(state: &BridgeState, params: &Value) -> Result<Value, BridgeFailure> {
    let serve_handle: String = required(params, "serve_handle")?;
    let handle = state.serve_handles.read().await.get(&serve_handle).cloned();
    let stopped = match handle {
        Some(handle) => {
            if let Err(error) = handle.stop().await {
                return Err(serve_failure(&handle, error));
            }
            state.serve_handles.write().await.remove(&serve_handle);
            true
        }
        None => false,
    };
    Ok(json!({ "stopped": stopped }))
}
