use super::*;

impl crate::agent::TurnEngine {
    pub(super) async fn recall_memory_context(
        &self,
        effective_prompt: &str,
        context_results: &mut Vec<ContextResult>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> (
        Vec<crate::durable_memory::DurableMemoryRecallIdentity>,
        Vec<(String, String, f32)>,
    ) {
        let mut recalled = Vec::new();
        for record in self.config.outcome_ledger.active_recall() {
            recalled.push((record.change_digest.clone(), record.constraint.clone(), 1.0));
        }
        let Some(ref memory) = self.config.memory else {
            self.emit_recalled(event_tx, &recalled, effective_prompt)
                .await;
            return (Vec::new(), Vec::new());
        };

        let mut v1_items = match memory.recall_similar(effective_prompt, 5).await {
            Ok(items) => items,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to recall memory context");
                Vec::new()
            }
        };

        let mut durable_batch = None;
        let mut durable_identities = Vec::new();
        if let Some(binding) = memory.durable_memory() {
            let cancellation = self
                .bound_invocation
                .as_ref()
                .map(|invocation| invocation.cancellation().clone())
                .unwrap_or_default();
            match binding
                .query_active_context_with_cancellation(effective_prompt, cancellation)
                .await
            {
                Ok(batch) if !batch.result.is_empty() => {
                    let active_content = batch
                        .result
                        .items
                        .iter()
                        .map(|item| normalized_memory_context(&item.content))
                        .collect::<std::collections::HashSet<_>>();
                    v1_items.retain(|item| {
                        !active_content.contains(&normalized_memory_context(&item.content))
                    });
                    durable_batch = Some(batch);
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "Failed to query active V2 memory context");
                }
            }
        }

        if !v1_items.is_empty() {
            recalled.extend(v1_items.iter().map(|item| {
                (
                    item.id.clone(),
                    item.content.clone(),
                    item.relevance_score(),
                )
            }));
            context_results.push(crate::memory::memory_items_to_context_result(
                "memory", v1_items,
            ));
        }
        if let Some(batch) = durable_batch {
            for (identity, item) in batch.identities.iter().zip(&batch.result.items) {
                recalled.push((
                    identity.node_id.clone(),
                    item.content.clone(),
                    item.relevance,
                ));
            }
            durable_identities = batch.identities;
            context_results.push(batch.result);
        }

        self.emit_recalled(event_tx, &recalled, effective_prompt)
            .await;
        (durable_identities, recalled)
    }

    /// Record lexical usage telemetry for one turn's recalled memories.
    ///
    /// Fire-and-forget by contract: callers keep this off the turn-critical
    /// path and every failure is traced instead of propagated.
    pub(crate) async fn record_recalled_memory_usage(
        &self,
        recalled: &[(String, String, f32)],
        final_text: &str,
    ) {
        let Some(memory) = self.config.memory.as_ref() else {
            return;
        };
        record_memory_usage_with_semantics(
            memory.store().as_ref(),
            recalled,
            final_text,
            memory.semantic_usage_judge.as_deref(),
        )
        .await;
    }

    async fn emit_recalled(
        &self,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        recalled: &[(String, String, f32)],
        effective_prompt: &str,
    ) {
        if let Some(tx) = event_tx {
            for (memory_id, content, relevance) in recalled {
                tx.send(AgentEvent::MemoryRecalled {
                    memory_id: memory_id.clone(),
                    content: content.clone(),
                    relevance: *relevance,
                })
                .await
                .ok();
            }
            if !recalled.is_empty() {
                tx.send(AgentEvent::MemoriesSearched {
                    query: Some(effective_prompt.to_string()),
                    tags: Vec::new(),
                    result_count: recalled.len(),
                })
                .await
                .ok();
            }
        }
    }
}

fn normalized_memory_context(content: &str) -> String {
    content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// V1 usage-telemetry counter kept in a store item's `metadata` map. The key
/// and decimal format are the contract shared with the
/// `v1_usage_consolidation` maintenance job in
/// `core/src/memory/maintenance.rs`.
const MEMORY_USAGE_COUNT_METADATA: &str = "usage_count";

/// Bump the usage counter of every recalled memory whose distinctive token
/// appears (case-insensitively) in the turn's final assistant text.
///
/// The signal is lexical, not semantic: one distinctive word per memory, so
/// a common long word can count as use while a faithful paraphrase cannot.
/// Ids that do not resolve to a V1 store item (durable node ids, ledger
/// digests) are skipped silently, and a repeated id counts once per call.
#[cfg(test)]
pub(super) async fn record_memory_usage(
    store: &dyn a3s_memory::MemoryStore,
    recalled: &[(String, String, f32)],
    final_text: &str,
) {
    record_memory_usage_with_semantics(store, recalled, final_text, None).await
}

/// Hybrid usage telemetry: the lexical arm is free and always runs; a
/// semantic judge, when the host attached one, gets the memories the
/// lexical arm could not decide (no qualifying token, or a token absent
/// from the final text) — a paraphrase then still counts as use.
pub(super) async fn record_memory_usage_with_semantics(
    store: &dyn a3s_memory::MemoryStore,
    recalled: &[(String, String, f32)],
    final_text: &str,
    semantic: Option<&crate::memory::SemanticUsageJudge>,
) {
    let final_text_lower = final_text.to_lowercase();
    let mut counted = std::collections::HashSet::new();
    let mut semantic_budget = SEMANTIC_USAGE_JUDGE_BUDGET;
    let mut undecided: Vec<&(String, String, f32)> = Vec::new();
    for entry @ (memory_id, content, _relevance) in recalled {
        if !counted.insert(memory_id.clone()) {
            continue;
        }
        let lexically_used = distinctive_usage_token(content)
            .map(|token| final_text_lower.contains(&token))
            .unwrap_or(false);
        if lexically_used {
            bump_memory_usage(store, memory_id).await;
        } else {
            undecided.push(entry);
        }
    }
    let Some(judge) = semantic else {
        return;
    };
    for (memory_id, content, _relevance) in undecided {
        if semantic_budget == 0 {
            break;
        }
        semantic_budget -= 1;
        // A miss or an embedding failure adds nothing — telemetry
        // degrades to the lexical arm, never to a wrong bump.
        if let Some(true) = judge.used(content, final_text).await {
            bump_memory_usage(store, memory_id).await;
        }
    }
}

/// Semantic evaluations per settled turn. The recalled set is already
/// bounded by the recall budget; this keeps the embedding bill bounded
/// even if a future caller widens it.
const SEMANTIC_USAGE_JUDGE_BUDGET: usize = 8;

async fn bump_memory_usage(store: &dyn a3s_memory::MemoryStore, memory_id: &str) {
    match store.retrieve(memory_id).await {
        Ok(Some(mut item)) => {
            add_memory_usage_count(&mut item, 1);
            if let Err(error) = store.store(item).await {
                tracing::warn!(
                    memory_id,
                    %error,
                    "Failed to persist memory usage telemetry"
                );
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                memory_id,
                %error,
                "Failed to load memory for usage telemetry"
            )
        }
    }
}

/// The longest purely alphanumeric word of at least eight characters in the
/// content, lowercased — the distinctive token usage telemetry matches on.
pub(super) fn distinctive_usage_token(content: &str) -> Option<String> {
    content
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.chars().count() >= 8)
        .max_by_key(|word| word.chars().count())
        .map(|word| word.to_lowercase())
}

/// Read a memory item's V1 usage counter (metadata-backed, default 0).
pub(super) fn memory_usage_count(item: &a3s_memory::MemoryItem) -> u32 {
    item.metadata
        .get(MEMORY_USAGE_COUNT_METADATA)
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(0)
}

/// Add `increment` to a memory item's V1 usage counter.
pub(super) fn add_memory_usage_count(item: &mut a3s_memory::MemoryItem, increment: u32) {
    let updated = memory_usage_count(item).saturating_add(increment);
    item.metadata
        .insert(MEMORY_USAGE_COUNT_METADATA.to_string(), updated.to_string());
}
