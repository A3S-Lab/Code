//! Context compaction logic
//!
//! Summarizes old conversation messages to reduce context size while
//! preserving key information.

use crate::llm::{ContentBlock, LlmClient, Message};
use anyhow::{Context, Result};
use std::sync::Arc;

/// Number of recent messages to keep intact during compaction
pub(crate) const KEEP_RECENT_MESSAGES: usize = 20;

/// Minimum message count before compaction is triggered
pub(crate) const MIN_MESSAGES_FOR_COMPACTION: usize = 30;

/// Number of initial messages to keep (usually system context)
pub(crate) const KEEP_INITIAL_MESSAGES: usize = 2;

/// Compact messages by summarizing old conversation turns.
///
/// Returns `Some(new_messages)` if compaction was performed, or `None` if
/// the message count is below the compaction threshold.
pub(crate) async fn compact_messages(
    session_id: &str,
    messages: &[Message],
    llm_client: &Arc<dyn LlmClient>,
) -> Result<Option<Vec<Message>>> {
    if messages.len() <= MIN_MESSAGES_FOR_COMPACTION {
        tracing::debug!(
            "Session {} has {} messages, no compaction needed (threshold: {})",
            session_id,
            messages.len(),
            MIN_MESSAGES_FOR_COMPACTION
        );
        return Ok(None);
    }

    tracing::info!(
        "Compacting session {} with {} messages",
        session_id,
        messages.len()
    );

    let total = messages.len();
    let summarize_start = KEEP_INITIAL_MESSAGES;
    let summarize_end = total.saturating_sub(KEEP_RECENT_MESSAGES);

    // If there's nothing to summarize, just keep recent messages
    if summarize_end <= summarize_start {
        tracing::debug!(
            "Not enough messages to summarize, keeping last {}",
            KEEP_RECENT_MESSAGES
        );
        let recent = messages[total.saturating_sub(KEEP_RECENT_MESSAGES)..].to_vec();
        return Ok(Some(recent));
    }

    let initial_messages = messages[..summarize_start].to_vec();
    let messages_to_summarize = &messages[summarize_start..summarize_end];
    let recent_messages = messages[summarize_end..].to_vec();

    tracing::debug!(
        "Compaction split: {} initial, {} to summarize, {} recent",
        initial_messages.len(),
        messages_to_summarize.len(),
        recent_messages.len()
    );

    // Build summarization prompt
    let conversation_text = messages_to_summarize
        .iter()
        .map(|msg| {
            let role = &msg.role;
            let text = msg.text();
            format!("{}: {}", role, text)
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let summarization_prompt = crate::prompts::render(
        crate::prompts::CONTEXT_COMPACT,
        &[("conversation", &conversation_text)],
    );

    // Call LLM to generate summary
    let summary_message = Message::user(&summarization_prompt);
    let response = llm_client
        .complete(&[summary_message], None, &[])
        .await
        .context("Failed to generate conversation summary")?;

    let summary_text = response.text();
    tracing::debug!("Generated summary: {} chars", summary_text.len());

    let summary_message = Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: format!("{}{}", crate::prompts::CONTEXT_SUMMARY_PREFIX, summary_text),
        }],
        reasoning_content: None,
    };

    let mut new_messages = initial_messages;
    new_messages.push(summary_message);
    new_messages.extend(recent_messages);

    tracing::info!(
        "Compaction complete: {} messages -> {} messages",
        messages.len(),
        new_messages.len()
    );

    Ok(Some(new_messages))
}
