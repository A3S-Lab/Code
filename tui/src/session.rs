//! One prompt from the hybrid scaffold. This crate is not the `a3s code`
//! product path. The session id `tui-turn` is only for this scaffold.

use std::sync::Arc;

use a3s_code_core::llm::LlmClient;
use a3s_code_core::{Agent, PlanningMode, SessionOptions};

use crate::model::{merge_launch_layers, LaunchLayers};

/// Result of one Enter submission.
pub struct ConfiguredTurn {
    pub text: String,
    /// [`a3s_code_core::AgentSession::model_name`] after the session is built
    /// from the merged CLI layers.
    pub model_name: String,
    /// Provider names on the config passed to `Agent::from_config`.
    pub provider_names: Vec<String>,
}

/// The scaffold's default fact-log / transcript thread. `a3s code
/// --continue` resolves here too: the scaffold always continues this
/// thread, so the flag is a no-op by construction, not a dropped arg
/// (CLI issue #149).
pub const DEFAULT_SESSION_ID: &str = "tui-turn";

/// Submit one prompt the way the TUI does on Enter.
///
/// The model and providers come from the CLI layer stack: explicit config, or
/// the user layer then the workspace layer, then `A3S_DEFAULT_MODEL`. The turn
/// is `session.send`, which appends a fact-log `user.message`.
pub async fn submit_configured_turn(
    layers: &LaunchLayers,
    responder: Option<Arc<dyn LlmClient>>,
    prompt: &str,
) -> anyhow::Result<ConfiguredTurn> {
    submit_configured_turn_in_session(layers, responder, prompt, DEFAULT_SESSION_ID).await
}

/// [`submit_configured_turn`] pinned to an explicit session id — the
/// `--resume <id>` path.
pub async fn submit_configured_turn_in_session(
    layers: &LaunchLayers,
    responder: Option<Arc<dyn LlmClient>>,
    prompt: &str,
    session_id: &str,
) -> anyhow::Result<ConfiguredTurn> {
    let merged = merge_launch_layers(layers).map_err(anyhow::Error::msg)?;
    let provider_names = merged.provider_names.clone();
    let agent = Agent::from_config(merged.config).await?;
    let mut options = SessionOptions::new()
        .with_session_id(session_id)
        .with_model(merged.model_id)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false);
    if let Some(responder) = responder {
        options = options.with_llm_client(responder);
    }
    let session = agent
        .session_async(
            layers.workspace.to_string_lossy().to_string(),
            Some(options),
        )
        .await?;
    let model_name = session.model_name().to_string();
    let result = session.send(prompt, None).await?;
    Ok(ConfiguredTurn {
        text: result.text,
        model_name,
        provider_names,
    })
}
