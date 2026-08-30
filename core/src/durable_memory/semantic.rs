use super::semantic_binding::{
    invalid, DurableMemorySemanticBindingV1, DurableMemorySemanticError,
    DurableMemorySemanticRecallPolicy,
};
use crate::embedding::{
    EmbeddingError, EmbeddingExecutor, EmbeddingExecutorConfig, EmbeddingInput, EmbeddingProvider,
};
use a3s_memory::repository::{MemoryNamespace, MemoryNode, MemoryRepository, MemoryStatus};
use a3s_memory::vector::{
    VectorIndex, VectorIndexStatus, VectorMutationConsistency, VectorRecord, VectorRevision,
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

#[derive(Clone, Copy, Debug)]
pub(super) struct SemanticIndexPublication {
    consistency: VectorMutationConsistency,
    expected_revision: Option<VectorRevision>,
}

impl SemanticIndexPublication {
    pub(super) fn consistency(self) -> VectorMutationConsistency {
        self.consistency
    }

    pub(super) fn after_publication(self, revision: VectorRevision) -> Self {
        Self {
            consistency: self.consistency,
            expected_revision: self.expected_revision.map(|_| revision),
        }
    }
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

    /// Return the strongest partition-mutation ordering advertised by the
    /// injected vector backend.
    pub fn mutation_consistency(&self) -> VectorMutationConsistency {
        self.index.mutation_consistency()
    }

    pub(super) fn serving_generation_digest(&self) -> &str {
        &self.serving_generation_digest
    }

    pub(super) fn refresh_lock(&self) -> Arc<Mutex<()>> {
        self.refresh_lock.clone()
    }

    pub(super) fn refresh_node_limit(&self) -> Result<usize, DurableMemorySemanticError> {
        let execution = self.executor.config();
        let descriptor = self.index.descriptor();
        let vector_bytes = descriptor
            .dimension
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or_else(|| invalid("refresh.vectorBytes", "vector byte size overflowed"))?;
        let by_execution_vector_bytes = execution.max_request_vector_bytes / vector_bytes;
        let limit = execution
            .max_request_inputs
            .min(descriptor.max_records)
            .min(by_execution_vector_bytes);
        if limit == 0 {
            return Err(invalid(
                "refresh.maxNodes",
                "configured budgets cannot admit one semantic record",
            ));
        }
        Ok(limit)
    }

    pub(super) fn refresh_snapshot_byte_limit(&self) -> usize {
        self.executor.config().max_request_text_bytes
    }

    pub(super) fn begin_index_publication(
        &self,
        required_consistency: VectorMutationConsistency,
    ) -> Result<SemanticIndexPublication, DurableMemorySemanticError> {
        let consistency = self.index.mutation_consistency();
        let sufficient = match required_consistency {
            VectorMutationConsistency::PartitionAtomic => matches!(
                consistency,
                VectorMutationConsistency::PartitionAtomic
                    | VectorMutationConsistency::IndexRevisionCas
            ),
            VectorMutationConsistency::IndexRevisionCas => {
                consistency == VectorMutationConsistency::IndexRevisionCas
            }
            _ => false,
        };
        if !sufficient {
            return Err(
                DurableMemorySemanticError::MutationConsistencyInsufficient {
                    required: required_consistency,
                    actual: consistency,
                },
            );
        }
        let expected_revision = match consistency {
            VectorMutationConsistency::PartitionAtomic => None,
            VectorMutationConsistency::IndexRevisionCas => Some(self.index.status().revision),
            _ => {
                return Err(invalid(
                    "vectorIndex.mutationConsistency",
                    "is unsupported by this Code generation",
                ));
            }
        };
        Ok(SemanticIndexPublication {
            consistency,
            expected_revision,
        })
    }

    pub(super) async fn invalidate_namespace(
        &self,
        namespace: &MemoryNamespace,
        publication: SemanticIndexPublication,
    ) -> Result<VectorIndexStatus, DurableMemorySemanticError> {
        let partition = semantic_partition_id(namespace, &self.serving_generation_digest);
        match publication.expected_revision {
            Some(expected_revision) => self
                .index
                .remove_partition_if_revision(&partition, expected_revision)
                .await
                .map_err(Into::into),
            None => self
                .index
                .remove_partition(&partition)
                .await
                .map_err(Into::into),
        }
    }

    /// Atomically replace one exact namespace partition with current Active nodes.
    pub async fn replace_namespace(
        &self,
        namespace: &MemoryNamespace,
        nodes: Vec<MemoryNode>,
        cancellation: CancellationToken,
    ) -> Result<VectorIndexStatus, DurableMemorySemanticError> {
        check_cancellation(&cancellation)?;
        let refresh_lock = self.refresh_lock();
        let _refresh_guard = tokio::select! {
            guard = refresh_lock.lock() => guard,
            _ = cancellation.cancelled() => {
                return Err(EmbeddingError::Cancelled.into());
            }
        };
        let publication =
            self.begin_index_publication(VectorMutationConsistency::PartitionAtomic)?;
        self.replace_namespace_locked(namespace, nodes, cancellation, publication)
            .await
    }

    pub(super) async fn replace_namespace_locked(
        &self,
        namespace: &MemoryNamespace,
        nodes: Vec<MemoryNode>,
        cancellation: CancellationToken,
        publication: SemanticIndexPublication,
    ) -> Result<VectorIndexStatus, DurableMemorySemanticError> {
        check_cancellation(&cancellation)?;
        let mut node_ids = HashSet::with_capacity(nodes.len());
        for node in &nodes {
            if &node.namespace != namespace {
                return Err(invalid(
                    "nodes.namespace",
                    "every node must belong to the replaced namespace",
                ));
            }
            if node.status != MemoryStatus::Active {
                return Err(invalid(
                    "nodes.status",
                    "only current Active nodes may enter a semantic partition",
                ));
            }
            if !node_ids.insert(node.id.clone()) {
                return Err(invalid("nodes.id", "node identifiers must be unique"));
            }
        }

        let execution_config = self.executor.config();
        if nodes.len() > execution_config.max_request_inputs {
            return Err(EmbeddingError::BudgetExceeded {
                resource: "request input count",
                requested: nodes.len(),
                limit: execution_config.max_request_inputs,
            }
            .into());
        }
        let mut total_text_bytes = 0usize;
        for node in &nodes {
            if node.content.len() > execution_config.max_input_text_bytes {
                return Err(EmbeddingError::BudgetExceeded {
                    resource: "input text byte",
                    requested: node.content.len(),
                    limit: execution_config.max_input_text_bytes,
                }
                .into());
            }
            total_text_bytes = total_text_bytes.saturating_add(node.content.len());
        }
        if total_text_bytes > execution_config.max_request_text_bytes {
            return Err(EmbeddingError::BudgetExceeded {
                resource: "request text byte",
                requested: total_text_bytes,
                limit: execution_config.max_request_text_bytes,
            }
            .into());
        }

        let partition = semantic_partition_id(namespace, &self.serving_generation_digest);
        if nodes.is_empty() {
            return self
                .publish_partition(&partition, Vec::new(), publication)
                .await;
        }
        let mut prepared = Vec::with_capacity(nodes.len());
        let mut inputs = Vec::with_capacity(nodes.len());
        for node in nodes {
            let content_digest = digest(&node.content);
            let record_id =
                semantic_record_id(&partition, &node.id, node.revision, &content_digest);
            inputs.push(EmbeddingInput::new(record_id.clone(), node.content));
            prepared.push((node.id, node.revision, content_digest, record_id));
        }
        let execution = self.executor.embed(inputs, cancellation.clone()).await?;
        check_cancellation(&cancellation)?;
        let records = prepared
            .into_iter()
            .zip(execution.vectors)
            .map(
                |((node_id, node_revision, content_digest, record_id), vector)| {
                    VectorRecord::new(record_id, vector.values)
                        .with_label(LABEL_SCHEMA, RECORD_SCHEMA_V1)
                        .with_label(LABEL_NODE_ID, node_id)
                        .with_label(LABEL_NODE_REVISION, node_revision.to_string())
                        .with_label(LABEL_CONTENT_DIGEST, content_digest)
                },
            )
            .collect();
        self.publish_partition(&partition, records, publication)
            .await
    }

    async fn publish_partition(
        &self,
        partition: &str,
        records: Vec<VectorRecord>,
        publication: SemanticIndexPublication,
    ) -> Result<VectorIndexStatus, DurableMemorySemanticError> {
        match publication.expected_revision {
            Some(expected_revision) => self
                .index
                .replace_partition_if_revision(partition, expected_revision, records)
                .await
                .map_err(Into::into),
            None => self
                .index
                .replace_partition(partition, records)
                .await
                .map_err(Into::into),
        }
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
