#[cfg(test)]
use super::agent_loop_runtime::pin_and_admit_runtime_projection;
use super::{agent_loop_runtime::PinnedRuntimeProjection, AgentSession};
use crate::agent::{AgentEvent, AgentResult};
use crate::capability::{CapabilityRuntimeError, ScopeCloseReport, SessionCapabilityRun};
use crate::commands::CommandContext;
#[cfg(test)]
use crate::commands::CommandRegistry;
use crate::error::read_or_recover;
#[cfg(test)]
use crate::error::Result;
use crate::llm::Message;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
#[cfg(test)]
use tokio::task::JoinHandle;

/// Original presentation values, captured before the canonical worker starts.
/// This value neither admits a capability nor proves original owner binding.
pub(super) struct OwnedCommandInputs {
    session_id: String,
    workspace: String,
    model: String,
    history: Arc<RwLock<Vec<Message>>>,
    security_provider: Option<Arc<dyn crate::security::SecurityProvider>>,
    usage: crate::llm::TokenUsage,
}

impl OwnedCommandInputs {
    pub(super) fn capture_original(session: &AgentSession) -> Self {
        Self {
            session_id: session.session_id.clone(),
            workspace: session.workspace.display().to_string(),
            model: session.model_name.clone(),
            history: Arc::clone(&session.history),
            security_provider: session.config.security_provider.clone(),
            usage: super::SessionView::from_session(session).recorded_token_usage(),
        }
    }

    pub(super) fn blocking_result(&self, text: String, history: Option<&[Message]>) -> AgentResult {
        command_result(
            text,
            history
                .map(|history| history.to_vec())
                .unwrap_or_else(|| read_or_recover(&self.history).clone()),
            crate::llm::TokenUsage::default(),
        )
    }

    fn sanitize_text(&self, text: &str) -> String {
        self.security_provider
            .as_deref()
            .map(|provider| crate::security::sanitize_text(provider, text))
            .unwrap_or_else(|| text.to_string())
    }
}

/// The actual admitted guard and its actual close result remain independent
/// of the output. An absent output is never a NotAttempted/cleanup witness.
pub(super) struct OwnedCommandPreparation {
    capability_run: SessionCapabilityRun,
    actual_close: std::result::Result<ScopeCloseReport, CapabilityRuntimeError>,
    text: Option<String>,
}

impl OwnedCommandPreparation {
    pub(super) fn into_original_observation(
        self,
    ) -> (
        SessionCapabilityRun,
        std::result::Result<ScopeCloseReport, CapabilityRuntimeError>,
        Option<String>,
    ) {
        (self.capability_run, self.actual_close, self.text)
    }
}

/// Execute and close an already admitted original command in the calling
/// worker. The caller probes is_command before admission and records the real
/// attempt before awaiting admission; this helper creates neither of them.
/// It never spawns output work or grants readiness/release authority.
pub(super) async fn prepare_owned(
    inputs: &OwnedCommandInputs,
    prompt: &str,
    runtime: &PinnedRuntimeProjection,
    capability_run: SessionCapabilityRun,
) -> OwnedCommandPreparation {
    let ctx = build_command_context(inputs, runtime.tool_executor());
    let output = runtime.command_registry().dispatch(prompt, &ctx);
    let actual_close = capability_run.close().await;
    let text = if actual_close.is_ok() {
        output.map(|output| inputs.sanitize_text(&output.text))
    } else {
        None
    };
    OwnedCommandPreparation {
        capability_run,
        actual_close,
        text,
    }
}

#[cfg(test)]
pub(super) async fn dispatch_blocking(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
) -> Result<Option<AgentResult>> {
    if !CommandRegistry::is_command(prompt) {
        return Ok(None);
    }

    let (runtime, capability_run) = pin_and_admit_runtime_projection(session).await?;
    let inputs = OwnedCommandInputs::capture_original(session);
    let (_capability_run, actual_close, text) =
        prepare_owned(&inputs, prompt, &runtime, capability_run)
            .await
            .into_original_observation();
    actual_close?;
    let Some(text) = text else {
        return Ok(None);
    };

    Ok(Some(inputs.blocking_result(text, history)))
}

fn command_result(
    text: String,
    messages: Vec<Message>,
    usage: crate::llm::TokenUsage,
) -> AgentResult {
    AgentResult {
        text,
        messages,
        tool_calls_count: 0,
        usage,
        verification_reports: Vec::new(),
        completion: crate::harness_loop::CompletionTerminal::Narrative,
        run_admission: "ordinary".to_string(),
        forced_stop: None,
    }
}

#[cfg(test)]
pub(super) async fn dispatch_streaming(
    session: &AgentSession,
    prompt: &str,
) -> Result<Option<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)>> {
    if !CommandRegistry::is_command(prompt) {
        return Ok(None);
    }

    let (runtime, capability_run) = pin_and_admit_runtime_projection(session).await?;
    let inputs = OwnedCommandInputs::capture_original(session);
    let (_capability_run, actual_close, text) =
        prepare_owned(&inputs, prompt, &runtime, capability_run)
            .await
            .into_original_observation();
    actual_close?;
    let Some(text) = text else {
        return Ok(None);
    };
    let (tx, rx) = mpsc::channel(256);

    let handle = tokio::spawn(async move {
        send_text_output(&tx, text).await;
    });

    Ok(Some((rx, handle)))
}

fn build_command_context(
    inputs: &OwnedCommandInputs,
    tool_executor: &crate::tools::ToolExecutor,
) -> CommandContext {
    let history = read_or_recover(&inputs.history);
    let tool_names: Vec<String> = tool_executor
        .definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect();

    let mut mcp_map = std::collections::HashMap::new();
    for name in &tool_names {
        if let Some(rest) = name.strip_prefix("mcp__") {
            if let Some((server, _)) = rest.split_once("__") {
                *mcp_map.entry(server.to_string()).or_default() += 1;
            }
        }
    }
    let mut mcp_servers: Vec<(String, usize)> = mcp_map.into_iter().collect();
    mcp_servers.sort_by(|a, b| a.0.cmp(&b.0));

    let context_report = crate::fact_control::tool_result_weights(
        std::path::Path::new(&inputs.workspace),
        &crate::fact_control::thread_for_session(&inputs.session_id),
    )
    .ok()
    .map(|weights| super::SessionContextReport::from_parts(inputs.usage.clone(), weights).render());
    CommandContext {
        session_id: inputs.session_id.clone(),
        workspace: inputs.workspace.clone(),
        model: inputs.model.clone(),
        history_len: history.len(),
        total_tokens: inputs.usage.total_tokens as u64,
        total_cost: 0.0,
        tool_names,
        mcp_servers,
        context_report,
    }
}

pub(super) async fn send_text_output(tx: &mpsc::Sender<AgentEvent>, text: String) {
    let _ = tx.send(AgentEvent::TextDelta { text: text.clone() }).await;
    send_end(tx, text).await;
}

async fn send_end(tx: &mpsc::Sender<AgentEvent>, text: String) {
    let _ = tx
        .send(AgentEvent::End {
            text,
            usage: crate::llm::TokenUsage::default(),
            verification_summary: empty_verification_summary(),
            completion_terminal: Default::default(),
            forced_stop: None,
            meta: None,
        })
        .await;
}

fn empty_verification_summary() -> Box<crate::verification::VerificationSummary> {
    Box::new(crate::verification::VerificationSummary::from_reports(&[]))
}

#[cfg(test)]
mod owned_command_tests;
