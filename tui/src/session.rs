//! One TUI prompt submitted through the a3s-code 9.0.0 fact-log session.

use std::path::Path;
use std::sync::{Arc, Mutex};

use a3s_code_core::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use a3s_code_core::{Agent, PlanningMode, SessionOptions};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::model::resolve_acl_model;

/// Client that records the ACL model id on every completion the session makes.
struct BoundClient {
    inner: Arc<dyn LlmClient>,
    model_id: String,
    invocations: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl LlmClient for BoundClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.invocations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(self.model_id.clone());
        self.inner.complete(messages, system, tools).await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.invocations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(self.model_id.clone());
        self.inner
            .complete_streaming(messages, system, tools, cancel_token)
            .await
    }
}

/// Submit one prompt the way the TUI does on Enter.
///
/// The model id comes from ACL `default_model`. The turn is `session.send`,
/// which appends a fact-log `user.message`. When `responder` is set, that
/// client handles the model call and each invocation records the ACL model id.
pub async fn submit_configured_turn(
    workspace: &Path,
    acl: &str,
    responder: Option<Arc<dyn LlmClient>>,
    invocations: Arc<Mutex<Vec<String>>>,
    prompt: &str,
) -> anyhow::Result<String> {
    let resolved = resolve_acl_model(acl).map_err(anyhow::Error::msg)?;
    let config = a3s_code_core::CodeConfig::from_acl(acl)?;
    let agent = Agent::from_config(config).await?;
    let mut options = SessionOptions::new()
        .with_session_id("tui-turn")
        .with_model(resolved.model_id.clone())
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false);
    if let Some(responder) = responder {
        options = options.with_llm_client(Arc::new(BoundClient {
            inner: responder,
            model_id: resolved.model_id,
            invocations,
        }));
    }
    let session = agent
        .session_async(workspace.to_string_lossy().to_string(), Some(options))
        .await?;
    let result = session.send(prompt, None).await?;
    Ok(result.text)
}
