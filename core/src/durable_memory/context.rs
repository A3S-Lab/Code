use super::{
    DurableMemoryMode, DurableMemoryRecallChannel, DurableMemoryRecallHit,
    DurableMemoryRecallPreview, DurableMemorySession,
};
use crate::context::{ContextAssembly, ContextItem, ContextResult, ContextType};
use a3s_memory::repository::{
    DurableMemoryKind, MemoryAccessEvent, MemoryNode, MemoryQuery, MemoryRelationKind,
    MemoryRepositoryError, MemoryStatus,
};
use chrono::{DateTime, Utc};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

const PROVIDER: &str = "durable_memory_v2";
const RELATED_SCORE_FACTOR: f32 = 0.75;

#[derive(Clone)]
struct RecallCandidate {
    node: MemoryNode,
    score: f32,
    channel: DurableMemoryRecallChannel,
    related_from: Option<String>,
}

impl RecallCandidate {
    fn into_preview_hit(self) -> DurableMemoryRecallHit {
        DurableMemoryRecallHit {
            node_id: self.node.id,
            node_revision: self.node.revision,
            kind: self.node.kind,
            content: self.node.content,
            score: self.score,
            channel: self.channel,
            related_from: self.related_from,
        }
    }
}

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
    /// Run the same pure active-only retrieval used before context assembly.
    /// This diagnostic does not record admission or use and does not inject
    /// any returned content into a model prompt.
    pub async fn preview_recall(
        &self,
        text: &str,
    ) -> Result<DurableMemoryRecallPreview, MemoryRepositoryError> {
        Ok(DurableMemoryRecallPreview {
            hits: self
                .query_recall_candidates(text)
                .await?
                .into_iter()
                .map(RecallCandidate::into_preview_hit)
                .collect(),
        })
    }

    pub(crate) async fn query_active_context(
        &self,
        text: &str,
    ) -> Result<DurableMemoryContextBatch, MemoryRepositoryError> {
        let hits = self.query_recall_candidates(text).await?;
        let mut result = ContextResult::new(PROVIDER);
        let mut identities = Vec::new();
        for hit in hits {
            let node = hit.node;
            let encoded_id = utf8_percent_encode(&node.id, NON_ALPHANUMERIC);
            let source = format!("a3s-memory://{encoded_id}?revision={}", node.revision);
            let item_id = format!("a3s-memory-v2:{}:r{}", node.id, node.revision);
            let content_digest = digest(&node.content);
            let token_count = (node.content.len() / 4).max(1);
            let item = ContextItem::new(&item_id, ContextType::Memory, &node.content)
                .with_relevance(hit.score)
                .with_token_count(token_count)
                .with_source(&source)
                .with_metadata("memory_node_id", serde_json::json!(node.id))
                .with_metadata("memory_node_revision", serde_json::json!(node.revision))
                .with_metadata("memory_kind", serde_json::json!(kind_label(node.kind)))
                .with_metadata("evidence_count", serde_json::json!(node.evidence.len()))
                .with_metadata(
                    "retrieval_channel",
                    serde_json::json!(channel_label(hit.channel)),
                )
                .with_provenance(PROVIDER)
                .with_priority(0.4)
                .with_trust(0.8)
                .with_freshness(0.6);
            let item = match hit.related_from {
                Some(source_id) => item.with_metadata("related_from", serde_json::json!(source_id)),
                None => item,
            };
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

    async fn query_recall_candidates(
        &self,
        text: &str,
    ) -> Result<Vec<RecallCandidate>, MemoryRepositoryError> {
        let Some(policy) = self.recall_policy() else {
            return Ok(Vec::new());
        };
        if self.mode() != DurableMemoryMode::ActiveRecall
            || !text.chars().any(char::is_alphanumeric)
        {
            return Ok(Vec::new());
        }

        let query = MemoryQuery::new(self.namespace().clone())
            .with_text(text)
            .with_limit(policy.max_results());
        let lexical = self
            .repository()
            .query(query)
            .await?
            .hits
            .into_iter()
            .filter(|hit| hit.score.total >= policy.min_lexical_score())
            .map(|hit| RecallCandidate {
                node: hit.node,
                score: hit.score.total,
                channel: DurableMemoryRecallChannel::Lexical,
                related_from: None,
            })
            .collect::<Vec<_>>();
        let mut candidates = lexical;
        if policy.max_related_lookups() == 0 {
            return Ok(candidates);
        }
        let mut known_ids = candidates
            .iter()
            .map(|candidate| candidate.node.id.clone())
            .collect::<HashSet<_>>();
        let mut looked_up = HashSet::new();
        let mut lookup_count = 0;
        let lexical_seeds = candidates.clone();
        'seeds: for seed in &lexical_seeds {
            for relation in &seed.node.relations {
                if relation.kind != MemoryRelationKind::RelatedTo
                    || known_ids.contains(&relation.target_id)
                    || !looked_up.insert(relation.target_id.clone())
                {
                    continue;
                }
                if lookup_count >= policy.max_related_lookups() {
                    break 'seeds;
                }
                lookup_count += 1;
                let Some(node) = self
                    .repository()
                    .get(self.namespace(), &relation.target_id)
                    .await?
                else {
                    continue;
                };
                if node.status != MemoryStatus::Active {
                    continue;
                }
                known_ids.insert(node.id.clone());
                candidates.push(RecallCandidate {
                    node,
                    score: (seed.score * RELATED_SCORE_FACTOR).clamp(0.0, 1.0),
                    channel: DurableMemoryRecallChannel::Related,
                    related_from: Some(seed.node.id.clone()),
                });
            }
        }
        candidates.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| channel_rank(left.channel).cmp(&channel_rank(right.channel)))
                .then_with(|| right.node.updated_at.cmp(&left.node.updated_at))
                .then_with(|| left.node.id.cmp(&right.node.id))
        });
        candidates.truncate(policy.max_results());
        Ok(candidates)
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

fn kind_label(kind: DurableMemoryKind) -> &'static str {
    match kind {
        DurableMemoryKind::Episodic => "episodic",
        DurableMemoryKind::Semantic => "semantic",
        DurableMemoryKind::Procedural => "procedural",
    }
}

fn channel_label(channel: DurableMemoryRecallChannel) -> &'static str {
    match channel {
        DurableMemoryRecallChannel::Lexical => "lexical",
        DurableMemoryRecallChannel::Related => "related",
    }
}

fn channel_rank(channel: DurableMemoryRecallChannel) -> u8 {
    match channel {
        DurableMemoryRecallChannel::Lexical => 0,
        DurableMemoryRecallChannel::Related => 1,
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
