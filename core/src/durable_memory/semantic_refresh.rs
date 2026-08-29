use super::{
    DurableMemorySemanticError, DurableMemorySemanticRecall,
    DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1,
};
use a3s_memory::repository::{
    MemoryNamespace, MemoryRepository, MemorySnapshotRequest, MemoryStatus, MAX_SNAPSHOT_BYTES,
    MAX_SNAPSHOT_NODES,
};
use a3s_memory::vector::VectorIndexStatus;
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

    pub fn index_status(&self) -> &VectorIndexStatus {
        &self.index_status
    }
}

impl DurableMemorySemanticRecall {
    pub(super) async fn refresh_repository_namespace(
        &self,
        repository: &dyn MemoryRepository,
        namespace: &MemoryNamespace,
        cancellation: CancellationToken,
    ) -> Result<DurableMemorySemanticRefreshReceipt, DurableMemorySemanticError> {
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
        let source_snapshot_profile = before.profile().to_string();
        let source_snapshot_digest = before.digest().to_string();
        let source_snapshot_bytes = before.byte_count();
        let active_node_count = before.nodes().len();

        let index_status = self
            .replace_namespace_locked(namespace, before.into_nodes(), cancellation)
            .await?;

        // Publication is the commit point. Finish source verification even if
        // the caller cancels after the atomic index replacement completed.
        let after = match repository.snapshot_namespace(request.clone()).await {
            Ok(after) => after,
            Err(error) => {
                self.invalidate_namespace(namespace).await?;
                return Err(error.into());
            }
        };
        if let Err(error) = after.verify(&request) {
            self.invalidate_namespace(namespace).await?;
            return Err(error.into());
        }
        if after.digest() != source_snapshot_digest {
            self.invalidate_namespace(namespace).await?;
            return Err(DurableMemorySemanticError::RepositoryChangedDuringRefresh);
        }

        Ok(DurableMemorySemanticRefreshReceipt {
            profile: DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1.to_string(),
            source_snapshot_profile,
            source_snapshot_digest,
            source_snapshot_bytes,
            semantic_binding_schema: DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1.to_string(),
            serving_generation_digest: self.serving_generation_digest().to_string(),
            active_node_count,
            index_status,
        })
    }
}
