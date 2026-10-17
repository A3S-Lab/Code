//! Session compaction for the fact-log turn engine.

use super::*;

pub(super) struct UsageStamp {
    pub(super) inner: Arc<dyn Completion>,
    pub(super) prompt_tokens: Arc<AtomicU64>,
}

impl Completion for UsageStamp {
    fn complete(
        &self,
        request: CompletionRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<ModelDecision, ActorError>> {
        let inner = Arc::clone(&self.inner);
        let prompt_tokens = Arc::clone(&self.prompt_tokens);
        Box::pin(async move {
            let decision = inner.complete(request, cancel).await?;
            let reported = inner.reported_prompt_tokens().unwrap_or(0);
            prompt_tokens.store(reported, Ordering::SeqCst);
            Ok(decision)
        })
    }

    fn reported_prompt_tokens(&self) -> Option<u64> {
        self.inner.reported_prompt_tokens()
    }
}

pub(super) struct SessionCompact {
    pub(super) client: Arc<dyn LlmClient>,
    pub(super) source: Option<crate::store::SessionFactSourceV1>,
    pub(super) events: Option<tokio::sync::mpsc::Sender<crate::agent::AgentEvent>>,
    pub(super) window_tokens: u64,
    pub(super) prompt_tokens: Arc<AtomicU64>,
}

impl Compactor for SessionCompact {
    fn compact(
        &self,
        messages: &[String],
    ) -> a3s_effect::coding::BoxFuture<Result<String, ActorError>> {
        let client = Arc::clone(&self.client);
        let source = self.source.clone();
        let events = self.events.clone();
        let window = self.window_tokens;
        let used = self.prompt_tokens.load(Ordering::SeqCst);
        let messages = messages.to_vec();
        Box::pin(async move {
            if let Some(events) = &events {
                let _ = events
                    .send(crate::agent::AgentEvent::AutoCompact {
                        phase: "started".into(),
                        tokens_used: used,
                        context_window: window,
                        tokens_after: 0,
                    })
                    .await;
            }
            let summary =
                compact_handoff(client.as_ref(), &messages, window, source.as_ref()).await;
            let tokens_after = (summary.len() as u64).div_ceil(4).max(1);
            if let Some(events) = &events {
                let _ = events
                    .send(crate::agent::AgentEvent::AutoCompact {
                        phase: "completed".into(),
                        tokens_used: used,
                        context_window: window,
                        tokens_after,
                    })
                    .await;
            }
            Ok(summary)
        })
    }
}

/// Model handoff for a prefix the fold already split. A failed model call
/// still shrinks the next prompt.
pub(super) async fn compact_handoff(
    client: &dyn LlmClient,
    messages: &[String],
    window_tokens: u64,
    source: Option<&crate::store::SessionFactSourceV1>,
) -> String {
    match crate::compaction::summarize_folded_prefix(
        client,
        messages,
        window_tokens as usize,
        source,
    )
    .await
    {
        Ok(summary) => summary,
        Err(_) => {
            let mut carried = crate::compaction_carry::obligations_from_folded(messages);
            carried.durable_source = source.cloned();
            crate::compaction_carry::cap_fallback_goals(&mut carried);
            crate::compaction_carry::seal_summary(&bounded_summary(messages), &carried)
        }
    }
}

/// Keep a short handoff when the prefix is long. A joined transcript would
/// put the same tokens back into the next prompt.
pub(super) fn bounded_summary(messages: &[String]) -> String {
    const MAX_CHARS: usize = 32_000;
    let joined = messages.join("\n");
    if joined.len() <= MAX_CHARS {
        return joined;
    }
    let head_end = char_boundary_at(&joined, 8_000);
    let tail_start = char_boundary_before(&joined, joined.len().saturating_sub(8_000));
    format!(
        "{}\n\n[earlier transcript compacted]\n\n{}",
        &joined[..head_end],
        &joined[tail_start..]
    )
}

pub(super) fn char_boundary_at(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    (index..=text.len())
        .find(|cursor| text.is_char_boundary(*cursor))
        .unwrap_or(text.len())
}

pub(super) fn char_boundary_before(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    (0..=index)
        .rev()
        .find(|cursor| text.is_char_boundary(*cursor))
        .unwrap_or(0)
}
