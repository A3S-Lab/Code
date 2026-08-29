use super::{DurableMemoryMode, DurableMemorySession};
use crate::context::{ContextAssembly, ContextItem, ContextResult, ContextType};
use a3s_memory::repository::{
    DurableMemoryKind, MemoryAccessEvent, MemoryQuery, MemoryRepositoryError,
};
use chrono::{DateTime, Utc};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

const PROVIDER: &str = "durable_memory_v2";

#[derive(Debug, Clone)]
pub(crate) struct DurableMemoryRecallIdentity {
    pub(crate) item_id: String,
    source: String,
    pub(crate) node_id: String,
    pub(crate) node_revision: u64,
    content_digest: String,
}

impl DurableMemoryRecallIdentity {
    fn matches(&self, item: &ContextItem) -> bool {
        item.id == self.item_id
            && item.source.as_deref() == Some(self.source.as_str())
            && digest(&item.content) == self.content_digest
    }
}

pub(crate) struct DurableMemoryContextBatch {
    pub(crate) result: ContextResult,
    pub(crate) identities: Vec<DurableMemoryRecallIdentity>,
}

impl DurableMemorySession {
    pub(crate) async fn query_active_context(
        &self,
        text: &str,
    ) -> Result<DurableMemoryContextBatch, MemoryRepositoryError> {
        let Some(policy) = self.recall_policy() else {
            return Ok(empty_batch());
        };
        if self.mode() != DurableMemoryMode::ActiveRecall
            || !text.chars().any(char::is_alphanumeric)
        {
            return Ok(empty_batch());
        }

        let query = MemoryQuery::new(self.namespace().clone())
            .with_text(text)
            .with_limit(policy.max_results());
        let hits = self.repository().query(query).await?.hits;
        let mut result = ContextResult::new(PROVIDER);
        let mut identities = Vec::new();
        for hit in hits
            .into_iter()
            .filter(|hit| hit.score.total >= policy.min_lexical_score())
        {
            let node = hit.node;
            let encoded_id = utf8_percent_encode(&node.id, NON_ALPHANUMERIC);
            let source = format!("a3s-memory://{encoded_id}?revision={}", node.revision);
            let item_id = format!("a3s-memory-v2:{}:r{}", node.id, node.revision);
            let content_digest = digest(&node.content);
            let token_count = (node.content.len() / 4).max(1);
            let item = ContextItem::new(&item_id, ContextType::Memory, &node.content)
                .with_relevance(hit.score.total)
                .with_token_count(token_count)
                .with_source(&source)
                .with_metadata("memory_node_id", serde_json::json!(node.id))
                .with_metadata("memory_node_revision", serde_json::json!(node.revision))
                .with_metadata("memory_kind", serde_json::json!(kind_label(node.kind)))
                .with_metadata("evidence_count", serde_json::json!(node.evidence.len()))
                .with_provenance(PROVIDER)
                .with_priority(0.4)
                .with_trust(0.8)
                .with_freshness(0.6);
            identities.push(DurableMemoryRecallIdentity {
                item_id,
                source,
                node_id: node.id,
                node_revision: node.revision,
                content_digest,
            });
            result.add_item(item);
        }
        Ok(DurableMemoryContextBatch { result, identities })
    }

    pub(crate) async fn admit_selected_context(
        &self,
        assembly: &mut ContextAssembly,
        identities: &[DurableMemoryRecallIdentity],
        context_id: &str,
        occurred_at: Option<DateTime<Utc>>,
    ) -> usize {
        if identities.is_empty() {
            return 0;
        }
        let mut admitted = HashSet::new();
        if let Some(occurred_at) = occurred_at {
            for item in &assembly.items {
                let Some(identity) = identities.iter().find(|identity| identity.matches(item))
                else {
                    continue;
                };
                let event_id = admission_id(context_id, &identity.node_id, identity.node_revision);
                let event = MemoryAccessEvent::new(
                    event_id,
                    self.namespace().clone(),
                    &identity.node_id,
                    identity.node_revision,
                    occurred_at,
                )
                .with_context_id(context_id);
                match self.repository().record_admission(event).await {
                    Ok(()) => {
                        admitted.insert(identity.item_id.clone());
                    }
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            memory_id = %identity.node_id,
                            memory_revision = identity.node_revision,
                            "Dropping V2 memory that could not be admitted"
                        );
                    }
                }
            }
        } else {
            tracing::warn!("Dropping V2 memory context because host time is invalid");
        }

        assembly.items.retain(|item| {
            let recalled = identities.iter().any(|identity| identity.matches(item));
            !recalled || admitted.contains(&item.id)
        });
        assembly.total_tokens = assembly
            .items
            .iter()
            .map(|item| {
                if item.token_count > 0 {
                    item.token_count
                } else {
                    item.content.split_whitespace().count().max(1)
                }
            })
            .sum();
        admitted.len()
    }
}

fn empty_batch() -> DurableMemoryContextBatch {
    DurableMemoryContextBatch {
        result: ContextResult::new(PROVIDER),
        identities: Vec::new(),
    }
}

fn kind_label(kind: DurableMemoryKind) -> &'static str {
    match kind {
        DurableMemoryKind::Episodic => "episodic",
        DurableMemoryKind::Semantic => "semantic",
        DurableMemoryKind::Procedural => "procedural",
    }
}

fn digest(content: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(content.as_bytes()))
}

fn admission_id(context_id: &str, node_id: &str, node_revision: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"a3s.code.memory.admission.v1\0");
    hasher.update(context_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(node_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(node_revision.to_le_bytes());
    format!("a3s-code-admission-{:x}", hasher.finalize())
}
