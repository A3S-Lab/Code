//! Adapter for the session-local A3S Memory vector implementation.
//!
//! Workspace retrieval talks only to [`WorkspaceVectorIndex`]. Keeping the
//! dependency-owned trait import in this one module keeps the semantic runtime
//! independent of Memory-specific implementation details.

use super::vector_contract::{
    VectorIndexChangeToken, VectorIndexDescriptor, VectorIndexObservation, VectorIndexStatus,
    VectorMutationConsistency, VectorRecord, VectorResult, VectorRevision, VectorSearchRequest,
    VectorSearchResult, WorkspaceVectorIndex,
};
use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex as MemoryVectorIndex};

#[derive(Debug)]
pub(super) struct MemoryVectorIndexAdapter {
    inner: InMemoryVectorIndex,
}

impl MemoryVectorIndexAdapter {
    pub(super) fn new(descriptor: VectorIndexDescriptor) -> VectorResult<Self> {
        Ok(Self {
            inner: InMemoryVectorIndex::new(descriptor)?,
        })
    }
}

#[async_trait::async_trait]
impl WorkspaceVectorIndex for MemoryVectorIndexAdapter {
    fn descriptor(&self) -> &VectorIndexDescriptor {
        self.inner.descriptor()
    }

    fn status(&self) -> VectorIndexStatus {
        self.inner.status()
    }

    fn change_token(&self) -> Option<VectorIndexChangeToken> {
        self.inner.change_token()
    }

    async fn observe(&self) -> VectorResult<VectorIndexObservation> {
        self.inner.observe().await
    }

    fn mutation_consistency(&self) -> VectorMutationConsistency {
        self.inner.mutation_consistency()
    }

    async fn replace_partition(
        &self,
        partition: &str,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        self.inner.replace_partition(partition, records).await
    }

    async fn replace_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        self.inner
            .replace_partition_if_revision(partition, expected_revision, records)
            .await
    }

    async fn remove_partition(&self, partition: &str) -> VectorResult<VectorIndexStatus> {
        self.inner.remove_partition(partition).await
    }

    async fn remove_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
    ) -> VectorResult<VectorIndexStatus> {
        self.inner
            .remove_partition_if_revision(partition, expected_revision)
            .await
    }

    async fn search(&self, request: VectorSearchRequest) -> VectorResult<VectorSearchResult> {
        self.inner.search(request).await
    }

    async fn clear(&self) -> VectorResult<VectorIndexStatus> {
        self.inner.clear().await
    }
}
