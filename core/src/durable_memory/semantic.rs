mod indexing;

pub(crate) use indexing::SemanticRefreshEmbeddingCache;

use super::semantic_binding::{
    invalid, DurableMemorySemanticBindingV1, DurableMemorySemanticError,
    DurableMemorySemanticRecallPolicy,
};
use crate::embedding::{
    EmbeddingError, EmbeddingExecutor, EmbeddingExecutorConfig, EmbeddingInput, EmbeddingProvider,
};
use a3s_memory::repository::{MemoryNamespace, MemoryNode, MemoryRepository, MemoryStatus};
use a3s_memory::vector::{
    VectorIndex, VectorIndexChangeToken, VectorIndexStatus, VectorMutationConsistency,
    VectorSearchRequest,
};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const PARTITION_ID_DOMAIN: &str = "a3s.code.memory.semantic-partition.v1";
const RECORD_ID_DOMAIN: &str = "a3s.code.memory.semantic-record.v1";
const RECORD_SCHEMA_V1: &str = "a3s.code.memory.semantic-record.v1";
const QUERY_ID: &str = "durable-memory-semantic-query";
const LABEL_SCHEMA: &str = "a3s.memory.semantic.schema";
const LABEL_NODE_ID: &str = "a3s.memory.semantic.node_id";
const LABEL_NODE_REVISION: &str = "a3s.memory.semantic.node_revision";
const LABEL_CONTENT_DIGEST: &str = "a3s.memory.semantic.content_digest";

/// Explicit host-owned embedding generation plus caller-owned vector index.
///
/// Construction and namespace replacement are inert with respect to background
/// work. The host decides whether to invoke refresh directly or install an
/// owned schedule; Code owns bounded query execution and candidate verification.
#[derive(Clone)]
pub struct DurableMemorySemanticRecall {
    binding: DurableMemorySemanticBindingV1,
    serving_generation_digest: String,
    executor: EmbeddingExecutor,
    index: Arc<dyn VectorIndex>,
    refresh_lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for DurableMemorySemanticRecall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DurableMemorySemanticRecall")
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}

impl DurableMemorySemanticRecall {
    pub fn new(
        authority_digest: impl Into<String>,
        provider: Arc<dyn EmbeddingProvider>,
        embedding_config: EmbeddingExecutorConfig,
        index: Arc<dyn VectorIndex>,
        policy: DurableMemorySemanticRecallPolicy,
    ) -> Result<Self, DurableMemorySemanticError> {
        let executor = EmbeddingExecutor::new(provider, embedding_config)?;
        let binding = DurableMemorySemanticBindingV1::new(
            authority_digest.into(),
            executor.descriptor().clone(),
            embedding_config,
            index.descriptor().clone(),
            policy,
        )?;
        let serving_generation_digest = binding.serving_generation_digest()?;
        Ok(Self {
            binding,
            serving_generation_digest,
            executor,
            index,
            refresh_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn binding(&self) -> &DurableMemorySemanticBindingV1 {
        &self.binding
    }

    pub fn index_status(&self) -> VectorIndexStatus {
        self.index.status()
    }

    pub(super) fn index_change_token(&self) -> Option<VectorIndexChangeToken> {
        self.index.change_token()
    }

    /// Return the strongest partition-mutation ordering advertised by the
    /// injected vector backend.
    pub fn mutation_consistency(&self) -> VectorMutationConsistency {
        self.index.mutation_consistency()
    }

    pub(super) fn serving_generation_digest(&self) -> &str {
        &self.serving_generation_digest
    }

    pub(super) async fn query_verified(
        &self,
        repository: &dyn MemoryRepository,
        namespace: &MemoryNamespace,
        text: &str,
        cancellation: CancellationToken,
    ) -> Result<Vec<SemanticRecallCandidate>, DurableMemorySemanticError> {
        check_cancellation(&cancellation)?;
        let execution = self
            .executor
            .embed(
                vec![EmbeddingInput::new(QUERY_ID, text.to_string())],
                cancellation.clone(),
            )
            .await?;
        let vector = execution
            .vectors
            .into_iter()
            .next()
            .ok_or_else(|| invalid("query.embedding", "provider returned no vector"))?;
        check_cancellation(&cancellation)?;

        let partition = semantic_partition_id(namespace, &self.serving_generation_digest);
        let observed_revision = self.index.status().revision;
        let request =
            VectorSearchRequest::new(vector.values, self.binding.policy().candidate_limit())
                .with_partition(&partition)
                .with_label(LABEL_SCHEMA, RECORD_SCHEMA_V1);
        let result = tokio::select! {
            result = self.index.search(request) => result?,
            _ = cancellation.cancelled() => return Err(EmbeddingError::Cancelled.into()),
        };
        if result.status.revision != observed_revision
            || self.index.status().revision != observed_revision
        {
            return Err(DurableMemorySemanticError::IndexRevisionChanged);
        }

        let mut known_nodes = HashSet::new();
        let mut candidates = Vec::new();
        for hit in result.hits {
            check_cancellation(&cancellation)?;
            if hit.partition != partition
                || !hit.score.is_finite()
                || !(-1.0..=1.0).contains(&hit.score)
                || hit.score < self.binding.policy().min_score()
            {
                continue;
            }
            let Some(node_id) = hit.labels.get(LABEL_NODE_ID) else {
                continue;
            };
            let Some(node_revision) = hit
                .labels
                .get(LABEL_NODE_REVISION)
                .and_then(|value| value.parse::<u64>().ok())
            else {
                continue;
            };
            let Some(content_digest) = hit.labels.get(LABEL_CONTENT_DIGEST) else {
                continue;
            };
            if hit.labels.get(LABEL_SCHEMA).map(String::as_str) != Some(RECORD_SCHEMA_V1)
                || hit.id != semantic_record_id(&partition, node_id, node_revision, content_digest)
                || !known_nodes.insert(node_id.clone())
            {
                continue;
            }
            let node = tokio::select! {
                result = repository.get(namespace, node_id) => result?,
                _ = cancellation.cancelled() => return Err(EmbeddingError::Cancelled.into()),
            };
            let Some(node) = node else {
                continue;
            };
            if node.status != MemoryStatus::Active
                || node.revision != node_revision
                || digest(&node.content) != *content_digest
            {
                continue;
            }
            candidates.push(SemanticRecallCandidate {
                node,
                score: hit.score,
            });
        }
        check_cancellation(&cancellation)?;
        if self.index.status().revision != observed_revision {
            return Err(DurableMemorySemanticError::IndexRevisionChanged);
        }
        candidates.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| right.node.updated_at.cmp(&left.node.updated_at))
                .then_with(|| left.node.id.cmp(&right.node.id))
        });
        candidates.truncate(self.binding.policy().candidate_limit());
        Ok(candidates)
    }
}

#[derive(Clone)]
pub(super) struct SemanticRecallCandidate {
    pub(super) node: MemoryNode,
    pub(super) score: f32,
}

fn semantic_partition_id(namespace: &MemoryNamespace, serving_generation_digest: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(PARTITION_ID_DOMAIN.as_bytes());
    for (label, value) in [
        (b"tenant".as_slice(), namespace.tenant_id()),
        (b"principal".as_slice(), namespace.principal_id()),
        (b"scope".as_slice(), namespace.scope_id()),
    ] {
        hasher.update([0]);
        hasher.update(label);
        hasher.update([0]);
        hasher.update(Sha256::digest(value.as_bytes()));
    }
    hasher.update([0]);
    hasher.update(b"serving-generation");
    hasher.update([0]);
    hasher.update(serving_generation_digest.as_bytes());
    format!("a3s-memory-semantic-{:x}", hasher.finalize())
}

fn semantic_record_id(
    partition: &str,
    node_id: &str,
    node_revision: u64,
    content_digest: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(RECORD_ID_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(partition.as_bytes());
    hasher.update([0]);
    hasher.update(Sha256::digest(node_id.as_bytes()));
    hasher.update([0]);
    hasher.update(node_revision.to_le_bytes());
    hasher.update([0]);
    hasher.update(content_digest.as_bytes());
    format!("a3s-memory-semantic-record-{:x}", hasher.finalize())
}

fn digest(content: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(content.as_bytes()))
}

fn check_cancellation(cancellation: &CancellationToken) -> Result<(), DurableMemorySemanticError> {
    if cancellation.is_cancelled() {
        Err(EmbeddingError::Cancelled.into())
    } else {
        Ok(())
    }
}
