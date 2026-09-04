//! Code-owned vector contract for session-local workspace retrieval.
//!
//! The workspace runtime must not know which storage implementation supplies
//! vectors. The Memory adapter is kept behind [`WorkspaceVectorIndex`] so the
//! semantic projection can remain independent of storage details. The public
//! value types are re-exported from the Memory crate through this narrow
//! contract boundary.

pub(super) use a3s_memory::vector::{
    VectorIndexChangeToken, VectorIndexDescriptor, VectorIndexError, VectorIndexObservation,
    VectorIndexStatus, VectorMutationConsistency, VectorRecord, VectorResult, VectorRevision,
    VectorSearchRequest, VectorSearchResult,
};

/// Session-owned vector index used by workspace semantic retrieval.
///
/// Implementations publish complete partition generations atomically and use
/// revision-fenced mutations so delayed embedding work cannot overwrite a
/// newer catalog generation.
#[allow(dead_code)]
#[async_trait::async_trait]
pub(super) trait WorkspaceVectorIndex: Send + Sync {
    fn descriptor(&self) -> &VectorIndexDescriptor;

    fn status(&self) -> VectorIndexStatus;

    fn change_token(&self) -> Option<VectorIndexChangeToken> {
        None
    }

    async fn observe(&self) -> VectorResult<VectorIndexObservation> {
        let observation = VectorIndexObservation {
            status: self.status(),
            change_token: None,
        };
        observation.verify()?;
        Ok(observation)
    }

    fn mutation_consistency(&self) -> VectorMutationConsistency {
        VectorMutationConsistency::PartitionAtomic
    }

    async fn replace_partition(
        &self,
        partition: &str,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus>;

    async fn replace_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus>;

    async fn remove_partition(&self, partition: &str) -> VectorResult<VectorIndexStatus>;

    async fn remove_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: VectorRevision,
    ) -> VectorResult<VectorIndexStatus>;

    async fn search(&self, request: VectorSearchRequest) -> VectorResult<VectorSearchResult>;

    async fn clear(&self) -> VectorResult<VectorIndexStatus>;
}
