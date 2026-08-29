use super::*;

impl AgentLoop {
    pub(super) async fn recall_memory_context(
        &self,
        effective_prompt: &str,
        context_results: &mut Vec<ContextResult>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> Vec<crate::durable_memory::DurableMemoryRecallIdentity> {
        let Some(ref memory) = self.config.memory else {
            return Vec::new();
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

        let mut recalled = Vec::new();
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

        if let Some(tx) = event_tx {
            for (memory_id, content, relevance) in &recalled {
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
        durable_identities
    }
}

fn normalized_memory_context(content: &str) -> String {
    content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}
