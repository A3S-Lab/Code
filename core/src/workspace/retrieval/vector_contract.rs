//! Code-owned vector contract for session-local workspace retrieval.
//!
//! The workspace runtime must not know which storage implementation supplies
//! vectors.  The compatibility Memory implementation and the A3S Vec adapter
//! are kept behind [`WorkspaceVectorIndex`].  The public value types are
//! re-exported from the compatibility crate for now so the migration remains
//! wire- and error-compatible; only this module is allowed to bind the
//! workspace contract to that dependency's trait.

pub(super) use a3s_memory::vector::{
    VectorBudgetResource, VectorIndexChangeToken, VectorIndexDescriptor, VectorIndexError,
    VectorIndexObservation, VectorIndexStatus, VectorMetric, VectorMutationConsistency,
    VectorNormalization, VectorRecord, VectorResult, VectorRevision, VectorSearchHit,
    VectorSearchRequest, VectorSearchResult,
};

/// Session-owned vector index used by workspace semantic retrieval.
///
/// Implementations publish complete partition generations atomically.  The
/// selected implementation is authoritative for one session; any other
/// implementation is used only as a differential shadow.
// The contract intentionally mirrors the complete lifecycle surface even
// though some observation/CAS methods are currently exercised only by the
// compatibility tests and future migration callers.
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
