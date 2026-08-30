use super::{
    check_cancellation, digest, semantic_partition_id, semantic_record_id,
    DurableMemorySemanticRecall, LABEL_CONTENT_DIGEST, LABEL_NODE_ID, LABEL_NODE_REVISION,
    LABEL_SCHEMA, RECORD_SCHEMA_V1,
};
use crate::durable_memory::semantic_binding::{invalid, DurableMemorySemanticError};
use crate::embedding::{EmbeddingError, EmbeddingInput, EmbeddingProviderRequestMetrics};
use a3s_memory::repository::{MemoryNamespace, MemoryNode, MemoryStatus};
use a3s_memory::vector::{
    VectorIndexStatus, VectorMutationConsistency, VectorRecord, VectorRevision,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug)]
pub(in crate::durable_memory) struct SemanticIndexPublication {
    consistency: VectorMutationConsistency,
    expected_revision: Option<VectorRevision>,
}

/// One schedule-ownership epoch's bounded, text-free semantic vectors.
///
/// Keys are complete semantic record IDs, so reuse remains bound to the exact
/// namespace, serving generation, node revision, and content digest.
pub(crate) struct SemanticRefreshEmbeddingCache {
    partition: String,
    embeddings: HashMap<String, Arc<[f32]>>,
}

impl SemanticRefreshEmbeddingCache {
    fn new(partition: String, embeddings: HashMap<String, Arc<[f32]>>) -> Self {
        Self {
            partition,
            embeddings,
        }
    }

    fn get(&self, partition: &str, record_id: &str, dimension: usize) -> Option<Arc<[f32]>> {
        if self.partition != partition {
            return None;
        }
        self.embeddings
            .get(record_id)
            .filter(|embedding| embedding.len() == dimension)
            .cloned()
    }
}

impl std::fmt::Debug for SemanticRefreshEmbeddingCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let vector_bytes = self.embeddings.values().fold(0usize, |total, embedding| {
            total.saturating_add(embedding.len().saturating_mul(std::mem::size_of::<f32>()))
        });
        formatter
            .debug_struct("SemanticRefreshEmbeddingCache")
            .field("record_count", &self.embeddings.len())
            .field("vector_bytes", &vector_bytes)
            .finish_non_exhaustive()
    }
}

pub(in crate::durable_memory) struct SemanticIndexReplacement {
    pub(in crate::durable_memory) status: VectorIndexStatus,
    pub(in crate::durable_memory) embedding_cache: SemanticRefreshEmbeddingCache,
}

#[derive(Default)]
pub(in crate::durable_memory) struct SemanticIndexWork {
    pub(in crate::durable_memory) embedding_cache_hits: usize,
    pub(in crate::durable_memory) embedding_inputs: usize,
    pub(in crate::durable_memory) embedding_input_bytes: usize,
    pub(in crate::durable_memory) provider_requests: usize,
    pub(in crate::durable_memory) provider_inputs: usize,
    pub(in crate::durable_memory) provider_input_bytes: usize,
    pub(in crate::durable_memory) publication_attempts: usize,
    pub(in crate::durable_memory) publication_records: usize,
}

pub(in crate::durable_memory) struct SemanticIndexReplacementAttempt {
    pub(in crate::durable_memory) result:
        Result<SemanticIndexReplacement, DurableMemorySemanticError>,
    pub(in crate::durable_memory) work: SemanticIndexWork,
}

struct SemanticIndexReuseRequest<'a> {
    namespace: &'a MemoryNamespace,
    nodes: Vec<MemoryNode>,
    previous_cache: Option<&'a SemanticRefreshEmbeddingCache>,
    cancellation: CancellationToken,
    publication: SemanticIndexPublication,
    provider_metrics: &'a EmbeddingProviderRequestMetrics,
}

struct PreparedSemanticRecord {
    node_id: String,
    node_revision: u64,
    content_digest: String,
    record_id: String,
    input: EmbeddingInput,
}

impl PreparedSemanticRecord {
    fn into_vector_record(self, embedding: Vec<f32>) -> VectorRecord {
        VectorRecord::new(self.record_id, embedding)
            .with_label(LABEL_SCHEMA, RECORD_SCHEMA_V1)
            .with_label(LABEL_NODE_ID, self.node_id)
            .with_label(LABEL_NODE_REVISION, self.node_revision.to_string())
            .with_label(LABEL_CONTENT_DIGEST, self.content_digest)
    }
}

impl SemanticIndexPublication {
    pub(in crate::durable_memory) fn consistency(self) -> VectorMutationConsistency {
        self.consistency
    }

    pub(in crate::durable_memory) fn expected_revision(self) -> Option<VectorRevision> {
        self.expected_revision
    }

    pub(in crate::durable_memory) fn after_publication(self, revision: VectorRevision) -> Self {
        Self {
            consistency: self.consistency,
            expected_revision: self.expected_revision.map(|_| revision),
        }
    }
}

impl DurableMemorySemanticRecall {
    pub(in crate::durable_memory) fn refresh_lock(&self) -> Arc<Mutex<()>> {
        self.refresh_lock.clone()
    }

    pub(in crate::durable_memory) fn refresh_node_limit(
        &self,
    ) -> Result<usize, DurableMemorySemanticError> {
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

    pub(in crate::durable_memory) fn refresh_snapshot_byte_limit(&self) -> usize {
        self.executor.config().max_request_text_bytes
    }

    pub(in crate::durable_memory) fn begin_index_publication(
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

    pub(in crate::durable_memory) async fn invalidate_namespace(
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

    pub(in crate::durable_memory) async fn replace_namespace_locked(
        &self,
        namespace: &MemoryNamespace,
        nodes: Vec<MemoryNode>,
        cancellation: CancellationToken,
        publication: SemanticIndexPublication,
    ) -> Result<VectorIndexStatus, DurableMemorySemanticError> {
        check_cancellation(&cancellation)?;
        let (partition, prepared) = self.prepare_namespace_records(namespace, nodes)?;
        if prepared.is_empty() {
            return self
                .publish_partition(&partition, Vec::new(), publication)
                .await;
        }
        let inputs = prepared.iter().map(|record| record.input.clone()).collect();
        let execution = self.executor.embed(inputs, cancellation.clone()).await?;
        check_cancellation(&cancellation)?;
        let records = prepared
            .into_iter()
            .zip(execution.vectors)
            .map(|(record, vector)| record.into_vector_record(vector.values))
            .collect();
        self.publish_partition(&partition, records, publication)
            .await
    }

    pub(in crate::durable_memory) async fn replace_namespace_locked_reusing(
        &self,
        namespace: &MemoryNamespace,
        nodes: Vec<MemoryNode>,
        previous_cache: Option<&SemanticRefreshEmbeddingCache>,
        cancellation: CancellationToken,
        publication: SemanticIndexPublication,
    ) -> SemanticIndexReplacementAttempt {
        let provider_metrics = EmbeddingProviderRequestMetrics::default();
        let mut work = SemanticIndexWork::default();
        let result = self
            .replace_namespace_locked_reusing_inner(
                SemanticIndexReuseRequest {
                    namespace,
                    nodes,
                    previous_cache,
                    cancellation,
                    publication,
                    provider_metrics: &provider_metrics,
                },
                &mut work,
            )
            .await;
        work.provider_requests = provider_metrics.requests();
        work.provider_inputs = provider_metrics.inputs();
        work.provider_input_bytes = provider_metrics.input_bytes();
        SemanticIndexReplacementAttempt { result, work }
    }

    async fn replace_namespace_locked_reusing_inner(
        &self,
        request: SemanticIndexReuseRequest<'_>,
        work: &mut SemanticIndexWork,
    ) -> Result<SemanticIndexReplacement, DurableMemorySemanticError> {
        let SemanticIndexReuseRequest {
            namespace,
            nodes,
            previous_cache,
            cancellation,
            publication,
            provider_metrics,
        } = request;
        check_cancellation(&cancellation)?;
        let (partition, prepared) = self.prepare_namespace_records(namespace, nodes)?;
        if prepared.is_empty() {
            work.publication_attempts = 1;
            let status = self
                .publish_partition(&partition, Vec::new(), publication)
                .await?;
            return Ok(SemanticIndexReplacement {
                status,
                embedding_cache: SemanticRefreshEmbeddingCache::new(partition, HashMap::new()),
            });
        }

        let dimension = self.executor.descriptor().dimension;
        let mut resolved = Vec::with_capacity(prepared.len());
        let mut missing_inputs = Vec::new();
        for record in &prepared {
            let cached = previous_cache
                .and_then(|cache| cache.get(&partition, &record.record_id, dimension));
            match &cached {
                Some(_) => {
                    work.embedding_cache_hits = work.embedding_cache_hits.saturating_add(1);
                }
                None => missing_inputs.push(record.input.clone()),
            }
            resolved.push(cached);
        }

        work.embedding_inputs = missing_inputs.len();
        work.embedding_input_bytes = missing_inputs.iter().fold(0usize, |total, input| {
            total.saturating_add(input.text_bytes())
        });

        let mut generated = HashMap::with_capacity(missing_inputs.len());
        if !missing_inputs.is_empty() {
            let execution = self
                .executor
                .embed_observed(missing_inputs, cancellation.clone(), provider_metrics)
                .await?;
            check_cancellation(&cancellation)?;
            for vector in execution.vectors {
                generated.insert(vector.id.to_string(), Arc::<[f32]>::from(vector.values));
            }
        }

        let mut embeddings = HashMap::with_capacity(prepared.len());
        let mut records = Vec::with_capacity(prepared.len());
        for (record, cached) in prepared.into_iter().zip(resolved) {
            let embedding = match cached {
                Some(embedding) => embedding,
                None => generated.remove(&record.record_id).ok_or_else(|| {
                    invalid(
                        "refresh.embeddingOutput",
                        "validated embedding output did not resolve one requested record",
                    )
                })?,
            };
            embeddings.insert(record.record_id.clone(), Arc::clone(&embedding));
            records.push(record.into_vector_record(embedding.to_vec()));
        }
        if !generated.is_empty() {
            return Err(invalid(
                "refresh.embeddingOutput",
                "validated embedding output retained an unrequested record",
            ));
        }
        check_cancellation(&cancellation)?;
        work.publication_attempts = 1;
        work.publication_records = records.len();
        let status = self
            .publish_partition(&partition, records, publication)
            .await?;
        Ok(SemanticIndexReplacement {
            status,
            embedding_cache: SemanticRefreshEmbeddingCache::new(partition, embeddings),
        })
    }

    fn prepare_namespace_records(
        &self,
        namespace: &MemoryNamespace,
        nodes: Vec<MemoryNode>,
    ) -> Result<(String, Vec<PreparedSemanticRecord>), DurableMemorySemanticError> {
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
        let mut prepared = Vec::with_capacity(nodes.len());
        for node in nodes {
            let content_digest = digest(&node.content);
            let record_id =
                semantic_record_id(&partition, &node.id, node.revision, &content_digest);
            prepared.push(PreparedSemanticRecord {
                node_id: node.id,
                node_revision: node.revision,
                content_digest,
                input: EmbeddingInput::new(record_id.clone(), node.content),
                record_id,
            });
        }
        Ok((partition, prepared))
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
}
