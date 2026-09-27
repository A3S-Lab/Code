//! Prepares one cache-aligned, image-budgeted compaction request history.

use std::num::NonZeroU64;

use a3s_code_sampling_types::ConversationItem;
use a3s_chat_state::compaction_utils::ModelRequestHistory;
use a3s_chat_state::image_budget::{
    ImageBudgetOutcome, apply_image_budget_with_limits, image_budget_limits,
};

use super::session_compact::build_compaction_prompt;

/// The exact owned history sent by a compaction attempt and persisted in its artifact.
pub(crate) struct PreparedCompactionHistory {
    /// Items ending with the summarization prompt, after the one image-budget transformation.
    pub(crate) items: Vec<ConversationItem>,
    /// Exact accounting for the transformation applied to `items`.
    pub(crate) image_budget: ImageBudgetOutcome,
}

/// Raw direct-call history or history already fully transformed for sampling.
pub(crate) enum CompactionHistoryInput {
    Raw(Vec<ConversationItem>),
    Prepared(PreparedCompactionHistory),
}

impl From<Vec<ConversationItem>> for CompactionHistoryInput {
    fn from(items: Vec<ConversationItem>) -> Self {
        Self::Raw(items)
    }
}

impl From<PreparedCompactionHistory> for CompactionHistoryInput {
    fn from(history: PreparedCompactionHistory) -> Self {
        Self::Prepared(history)
    }
}

impl CompactionHistoryInput {
    /// Budget raw direct-call history once; preserve an already-prepared history verbatim.
    pub(crate) fn prepare(
        self,
        max_request_bytes: Option<NonZeroU64>,
        compaction_tool_tokens: u64,
    ) -> PreparedCompactionHistory {
        match self {
            Self::Raw(items) => prepare_items(items, max_request_bytes, compaction_tool_tokens),
            Self::Prepared(history) => history,
        }
    }
}

/// Append the summarization prompt, then apply the compaction request's image budget once.
pub(crate) fn build_compaction_chat_history(
    mut chat_history: Vec<ConversationItem>,
    user_context: Option<&str>,
    use_short_prompt: bool,
    max_request_bytes: Option<NonZeroU64>,
    compaction_tool_tokens: u64,
) -> PreparedCompactionHistory {
    let prompt = build_compaction_prompt(user_context, use_short_prompt);
    chat_history.push(ConversationItem::user(prompt));
    prepare_items(chat_history, max_request_bytes, compaction_tool_tokens)
}

fn prepare_items(
    items: Vec<ConversationItem>,
    max_request_bytes: Option<NonZeroU64>,
    compaction_tool_tokens: u64,
) -> PreparedCompactionHistory {
    let (trigger_bytes, reclaim_target_bytes) =
        effective_image_budget_limits(max_request_bytes, compaction_tool_tokens);
    let items = ModelRequestHistory::from_raw(items).into_items();
    let budgeted = apply_image_budget_with_limits(items, trigger_bytes, reclaim_target_bytes);
    PreparedCompactionHistory {
        items: budgeted.items,
        image_budget: budgeted.outcome,
    }
}

fn effective_image_budget_limits(
    max_request_bytes: Option<NonZeroU64>,
    compaction_tool_tokens: u64,
) -> (usize, usize) {
    let (trigger_bytes, reclaim_target_bytes) = image_budget_limits(max_request_bytes);
    // The existing tool estimate is bytes/4; invert that same heuristic here.
    // Saturation is conservative: an unrepresentable reserve leaves no image budget.
    let reserved_bytes =
        usize::try_from(a3s_token_estimation::estimate_chars(compaction_tool_tokens))
            .unwrap_or(usize::MAX);
    (
        trigger_bytes.saturating_sub(reserved_bytes),
        reclaim_target_bytes.saturating_sub(reserved_bytes),
    )
}

#[cfg(test)]
#[path = "prepared_compaction_history_tests.rs"]
mod tests;
