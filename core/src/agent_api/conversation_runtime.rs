//! Conversation execution facade for a session.
//!
//! This module owns the public conversation contract: slash-command dispatch,
//! blocking sends, streaming sends, and attachment handling.
//! Lower-level runtime modules own run lifecycle and event forwarding.

use super::{
    command_runtime, runtime::BlockingRunContext, runtime::ConversationInput,
    runtime::StreamRunContext, AgentSession,
};
use crate::agent::{AgentEvent, AgentResult};
use crate::error::Result;
use crate::llm::{Attachment, Message};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub(super) async fn send(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
) -> Result<AgentResult> {
    if let Some(result) = command_runtime::dispatch_blocking(session, prompt, history).await? {
        return Ok(result);
    }

    warn_deferred_init(session);
    let input = ConversationInput::from_history(session, history);
    let blocking_run = BlockingRunContext::start(session, prompt, input.persistence).await;
    blocking_run
        .execute_with_prompt(&input.messages, prompt, &session.session_id)
        .await
}

pub(super) async fn send_with_attachments(
    session: &AgentSession,
    prompt: &str,
    attachments: &[Attachment],
    history: Option<&[Message]>,
) -> Result<AgentResult> {
    // Build one user message containing text and images, then execute from the
    // resulting message list so the loop does not append a duplicate prompt.
    let input = ConversationInput::with_attachments(session, history, prompt, attachments);
    let blocking_run = BlockingRunContext::start(session, prompt, input.persistence).await;
    blocking_run
        .execute_from_messages(input.messages, &session.session_id)
        .await
}

pub(super) async fn stream_with_attachments(
    session: &AgentSession,
    prompt: &str,
    attachments: &[Attachment],
    history: Option<&[Message]>,
) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
    let input = ConversationInput::with_attachments(session, history, prompt, attachments);
    let stream_run = StreamRunContext::start(session, prompt, input.persistence).await;
    Ok(stream_run.spawn_from_messages(input.messages))
}

pub(super) async fn stream(
    session: &AgentSession,
    prompt: &str,
    history: Option<&[Message]>,
) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
    if let Some(stream) = command_runtime::dispatch_streaming(session, prompt).await {
        return Ok(stream);
    }

    let input = ConversationInput::from_history(session, history);
    let stream_run = StreamRunContext::start(session, prompt, input.persistence).await;
    Ok(stream_run.spawn_with_prompt(input.messages, prompt.to_string()))
}

fn warn_deferred_init(session: &AgentSession) {
    if let Some(warning) = &session.init_warning {
        tracing::warn!(
            session_id = %session.session_id,
            "Session init warning: {}", warning
        );
    }
}
