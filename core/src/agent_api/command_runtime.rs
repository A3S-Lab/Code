use super::{agent_loop_runtime::pin_and_admit_runtime_projection, AgentSession};
use crate::agent::{AgentEvent, AgentResult};
use crate::commands::{CommandContext, CommandRegistry};
use crate::error::{read_or_recover, Result};
use crate::llm::Message;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub(super) async fn dispatch_blocking(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
) -> Result<Option<AgentResult>> {
    if !CommandRegistry::is_command(prompt) {
        return Ok(None);
    }

    let (runtime, capability_run) = pin_and_admit_runtime_projection(session).await?;
    let ctx = build_command_context(session, runtime.tool_executor());
    let output = runtime.command_registry().dispatch(prompt, &ctx);
    capability_run.close().await?;
    let Some(output) = output else {
        return Ok(None);
    };

    Ok(Some(command_result(
        sanitize_text(session, &output.text),
        command_messages(session, history),
        crate::llm::TokenUsage::default(),
    )))
}

fn command_messages(session: &AgentSession, history: Option<&[Message]>) -> Vec<Message> {
    history
        .map(|h| h.to_vec())
        .unwrap_or_else(|| read_or_recover(&session.history).clone())
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
    }
}

pub(super) async fn dispatch_streaming(
    session: &AgentSession,
    prompt: &str,
) -> Result<Option<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)>> {
    if !CommandRegistry::is_command(prompt) {
        return Ok(None);
    }

    let (runtime, capability_run) = pin_and_admit_runtime_projection(session).await?;
    let ctx = build_command_context(session, runtime.tool_executor());
    let output = runtime.command_registry().dispatch(prompt, &ctx);
    capability_run.close().await?;
    let Some(output) = output else {
        return Ok(None);
    };
    let (tx, rx) = mpsc::channel(256);

    let text = sanitize_text(session, &output.text);
    let handle = tokio::spawn(async move {
        send_text_output(&tx, text).await;
    });

    Ok(Some((rx, handle)))
}

fn sanitize_text(session: &AgentSession, text: &str) -> String {
    session
        .config
        .security_provider
        .as_deref()
        .map(|provider| provider.sanitize_output(text))
        .unwrap_or_else(|| text.to_string())
}

fn build_command_context(
    session: &AgentSession,
    tool_executor: &crate::tools::ToolExecutor,
) -> CommandContext {
    let history = read_or_recover(&session.history);
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

    CommandContext {
        session_id: session.session_id.clone(),
        workspace: session.workspace.display().to_string(),
        model: session.model_name.clone(),
        history_len: history.len(),
        total_tokens: 0,
        total_cost: 0.0,
        tool_names,
        mcp_servers,
    }
}

async fn send_text_output(tx: &mpsc::Sender<AgentEvent>, text: String) {
    let _ = tx.send(AgentEvent::TextDelta { text: text.clone() }).await;
    send_end(tx, text).await;
}

async fn send_end(tx: &mpsc::Sender<AgentEvent>, text: String) {
    let _ = tx
        .send(AgentEvent::End {
            text,
            usage: crate::llm::TokenUsage::default(),
            verification_summary: empty_verification_summary(),
            meta: None,
        })
        .await;
}

fn empty_verification_summary() -> Box<crate::verification::VerificationSummary> {
    Box::new(crate::verification::VerificationSummary::from_reports(&[]))
}
