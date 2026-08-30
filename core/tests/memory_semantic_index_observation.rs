#[allow(dead_code)]
#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

use a3s_code_core::embedding::EmbeddingExecutorConfig;
use a3s_memory::repository::{InMemoryRepository, MemoryStatus};
use a3s_memory::vector::{
    InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorIndexObservation,
    VectorIndexStatus, VectorMutationConsistency, VectorRecord, VectorResult, VectorRevision,
    VectorSearchRequest, VectorSearchResult,
};
use async_trait::async_trait;
use refresh_support::*;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct ObservationOnlyIndex {
    inner: Arc<InMemoryVectorIndex>,
}

#[async_trait]
impl VectorIndex for ObservationOnlyIndex {
    fn descriptor(&self) -> &VectorIndexDescriptor {
        self.inner.descriptor()
    }

    fn status(&self) -> VectorIndexStatus {
        VectorIndexStatus::default()
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

#[tokio::test]
async fn semantic_correctness_uses_exact_async_observations_not_sync_status_hints() {
    let namespace = namespace("async-index-observation");
    let repository = Arc::new(InMemoryRepository::new());
    create_node(
        repository.as_ref(),
        &namespace,
        "create-alpha",
        "alpha",
        MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let inner = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let index: Arc<dyn VectorIndex> = Arc::new(ObservationOnlyIndex {
        inner: inner.clone(),
    });
    let durable = refresh_support::session(
        repository,
        namespace,
        semantic(
            Arc::new(FixtureProvider),
            EmbeddingExecutorConfig::default(),
            index,
        ),
    );

    let receipt = durable
        .refresh_semantic_recall_requiring(
            VectorMutationConsistency::IndexRevisionCas,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(receipt.index_status().revision.value(), 1);
    assert_eq!(inner.status().revision.value(), 1);

    let preview = durable.preview_recall(ALPHA_QUERY).await.unwrap();
    assert_eq!(preview.hits.len(), 1);
    assert_eq!(preview.hits[0].node_id, "alpha");
}
