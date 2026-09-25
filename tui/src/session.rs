//! One TUI prompt submitted through the a3s-code 9.0.0 fact-log session.

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
    let merged = merge_launch_layers(layers).map_err(anyhow::Error::msg)?;
    let provider_names = merged.provider_names.clone();
    let agent = Agent::from_config(merged.config).await?;
    let mut options = SessionOptions::new()
        .with_session_id("tui-turn")
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
