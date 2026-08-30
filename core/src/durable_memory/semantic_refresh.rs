use super::{
    DurableMemorySemanticError, DurableMemorySemanticRecall, DurableMemorySession,
    DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1,
};
use a3s_memory::repository::{
    MemoryNamespace, MemoryNamespaceSnapshot, MemoryRepository, MemorySnapshotRequest,
    MemoryStatus, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_NODES,
};
use a3s_memory::vector::{VectorIndexStatus, VectorMutationConsistency, VectorRevision};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

/// Stable identity of the verified full-snapshot refresh algorithm.
pub const DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1: &str =
    "a3s.code.memory.semantic-refresh.full-snapshot.v1";

/// Secret-free evidence that one exact Active snapshot was published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableMemorySemanticRefreshReceipt {
    profile: String,
    source_snapshot_profile: String,
    source_snapshot_digest: String,
    source_snapshot_bytes: usize,
    semantic_binding_schema: String,
    serving_generation_digest: String,
    active_node_count: usize,
    mutation_consistency: VectorMutationConsistency,
    index_status: VectorIndexStatus,
}

impl DurableMemorySemanticRefreshReceipt {
    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn source_snapshot_profile(&self) -> &str {
        &self.source_snapshot_profile
    }

    pub fn source_snapshot_digest(&self) -> &str {
        &self.source_snapshot_digest
    }

    pub fn source_snapshot_bytes(&self) -> usize {
        self.source_snapshot_bytes
    }

    pub fn semantic_binding_schema(&self) -> &str {
        &self.semantic_binding_schema
    }

    pub fn serving_generation_digest(&self) -> &str {
        &self.serving_generation_digest
    }

    pub fn active_node_count(&self) -> usize {
        self.active_node_count
    }

    pub fn mutation_consistency(&self) -> VectorMutationConsistency {
        self.mutation_consistency
    }

    pub fn index_status(&self) -> &VectorIndexStatus {
        &self.index_status
    }

    fn matches_current(
        &self,
        semantic: &DurableMemorySemanticRecall,
        snapshot: &MemoryNamespaceSnapshot,
        consistency: VectorMutationConsistency,
        expected_revision: Option<VectorRevision>,
        index_status: &VectorIndexStatus,
    ) -> bool {
        self.profile == DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1
            && self.source_snapshot_profile == snapshot.profile()
            && self.source_snapshot_digest == snapshot.digest()
            && self.source_snapshot_bytes == snapshot.byte_count()
            && self.semantic_binding_schema == DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1
            && self.serving_generation_digest == semantic.serving_generation_digest()
            && self.active_node_count == snapshot.nodes().len()
            && self.mutation_consistency == consistency
            && expected_revision == Some(self.index_status.revision)
            && self.index_status == *index_status
    }
}

pub(crate) enum DurableMemorySemanticRefreshRun {
    Published(DurableMemorySemanticRefreshReceipt),
    Unchanged(DurableMemorySemanticRefreshReceipt),
}

impl DurableMemorySemanticRefreshRun {
    pub(crate) fn into_receipt(self) -> DurableMemorySemanticRefreshReceipt {
        match self {
            Self::Published(receipt) | Self::Unchanged(receipt) => receipt,
        }
    }
}

impl DurableMemorySession {
    pub(crate) async fn refresh_semantic_recall_if_stale_requiring(
        &self,
        previous: &DurableMemorySemanticRefreshReceipt,
        required_consistency: VectorMutationConsistency,
        cancellation: CancellationToken,
    ) -> Result<DurableMemorySemanticRefreshRun, DurableMemorySemanticError> {
        let semantic = self.semantic_recall.as_ref().ok_or_else(|| {
            DurableMemorySemanticError::InvalidConfiguration {
                field: "semanticRecall",
                reason: "refresh requires an attached semantic recall generation".to_string(),
            }
        })?;
        semantic
            .refresh_repository_namespace_if_stale(
                self.repository.as_ref(),
                &self.namespace,
                required_consistency,
                Some(previous),
                cancellation,
            )
            .await
    }
}

impl DurableMemorySemanticRecall {
    pub(super) async fn refresh_repository_namespace(
        &self,
        repository: &dyn MemoryRepository,
        namespace: &MemoryNamespace,
        required_consistency: VectorMutationConsistency,
        cancellation: CancellationToken,
    ) -> Result<DurableMemorySemanticRefreshReceipt, DurableMemorySemanticError> {
        self.refresh_repository_namespace_if_stale(
            repository,
            namespace,
            required_consistency,
            None,
            cancellation,
        )
        .await
        .map(DurableMemorySemanticRefreshRun::into_receipt)
    }

    pub(super) async fn refresh_repository_namespace_if_stale(
        &self,
        repository: &dyn MemoryRepository,
        namespace: &MemoryNamespace,
        required_consistency: VectorMutationConsistency,
        previous: Option<&DurableMemorySemanticRefreshReceipt>,
        cancellation: CancellationToken,
    ) -> Result<DurableMemorySemanticRefreshRun, DurableMemorySemanticError> {
        if cancellation.is_cancelled() {
            return Err(crate::embedding::EmbeddingError::Cancelled.into());
        }
        let refresh_lock = self.refresh_lock();
        let _refresh_guard = tokio::select! {
            guard = refresh_lock.lock() => guard,
            _ = cancellation.cancelled() => {
                return Err(crate::embedding::EmbeddingError::Cancelled.into());
            }
        };
        let publication = self.begin_index_publication(required_consistency)?;
        let request = MemorySnapshotRequest::new(
            namespace.clone(),
            self.refresh_node_limit()?.min(MAX_SNAPSHOT_NODES),
            self.refresh_snapshot_byte_limit().min(MAX_SNAPSHOT_BYTES),
        )
        .with_statuses([MemoryStatus::Active]);
        let before = tokio::select! {
            result = repository.snapshot_namespace(request.clone()) => result?,
            _ = cancellation.cancelled() => {
                return Err(crate::embedding::EmbeddingError::Cancelled.into());
            }
        };
        before.verify(&request)?;
        if let Some(previous) = previous {
            // CAS captures the index revision before the source snapshot, and
            // this status read observes it afterward. Exact equality proves
            // there was one interval where both source and index still matched
            // the receipt; weaker partition-atomic ordering never skips.
            let current_index_status = self.index_status();
            if previous.matches_current(
                self,
                &before,
                publication.consistency(),
                publication.expected_revision(),
                &current_index_status,
            ) {
                return Ok(DurableMemorySemanticRefreshRun::Unchanged(previous.clone()));
            }
        }
        let source_snapshot_profile = before.profile().to_string();
        let source_snapshot_digest = before.digest().to_string();
        let source_snapshot_bytes = before.byte_count();
        let active_node_count = before.nodes().len();

        let index_status = self
            .replace_namespace_locked(namespace, before.into_nodes(), cancellation, publication)
            .await?;
        let cleanup_publication = publication.after_publication(index_status.revision);

        // Publication is the commit point. Finish source verification even if
        // the caller cancels after the atomic index replacement completed.
        let after = match repository.snapshot_namespace(request.clone()).await {
            Ok(after) => after,
            Err(error) => {
                self.invalidate_namespace(namespace, cleanup_publication)
                    .await?;
                return Err(error.into());
            }
        };
        if let Err(error) = after.verify(&request) {
            self.invalidate_namespace(namespace, cleanup_publication)
                .await?;
            return Err(error.into());
        }
        if after.digest() != source_snapshot_digest {
            self.invalidate_namespace(namespace, cleanup_publication)
                .await?;
            return Err(DurableMemorySemanticError::RepositoryChangedDuringRefresh);
        }
        if self.index_status().revision != index_status.revision {
            return Err(DurableMemorySemanticError::IndexRevisionChanged);
        }

        Ok(DurableMemorySemanticRefreshRun::Published(
            DurableMemorySemanticRefreshReceipt {
                profile: DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1.to_string(),
                source_snapshot_profile,
                source_snapshot_digest,
                source_snapshot_bytes,
                semantic_binding_schema: DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1.to_string(),
                serving_generation_digest: self.serving_generation_digest().to_string(),
                active_node_count,
                mutation_consistency: publication.consistency(),
                index_status,
            },
        ))
    }
}
