use crate::refresh_support::{semantic, FixtureProvider};
use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError,
};
use a3s_code_core::memory::{
    AgentMemory, MemoryConfig, MemoryMaintenanceOptions, MemoryMaintenanceRuntime,
    ScheduledSemanticRefresh,
};
use a3s_code_core::{DurableMemoryRecallPolicy, DurableMemorySession};
use a3s_memory::repository::{
    InMemoryRepository, MemoryAccessEvent, MemoryChangeResult, MemoryChangeSet, MemoryNamespace,
    MemoryNamespaceSnapshot, MemoryNode, MemoryQuery, MemoryQueryResult, MemoryRepository,
    MemoryRepositoryError, MemorySnapshotRequest, MemoryUsageSummary,
};
use a3s_memory::vector::{
    InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorIndexStatus,
    VectorMutationConsistency, VectorRecord, VectorResult, VectorRevision, VectorSearchRequest,
    VectorSearchResult,
};
use a3s_memory::InMemoryStore;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct CountingProvider {
    calls: AtomicUsize,
    inputs: AtomicUsize,
}

impl CountingProvider {
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn inputs(&self) -> usize {
        self.inputs.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EmbeddingProvider for CountingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        FixtureProvider.descriptor()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs
            .fetch_add(request.inputs().len(), Ordering::SeqCst);
        FixtureProvider.embed(request, cancellation).await
    }
}

pub struct SnapshotOnlyRepository {
    inner: Arc<InMemoryRepository>,
    fail_next_snapshot: AtomicBool,
}

impl SnapshotOnlyRepository {
    pub fn new(inner: Arc<InMemoryRepository>) -> Self {
        Self {
            inner,
            fail_next_snapshot: AtomicBool::new(false),
        }
    }

    pub fn failing_once(inner: Arc<InMemoryRepository>) -> Self {
        Self {
            inner,
            fail_next_snapshot: AtomicBool::new(true),
        }
    }
}

#[async_trait]
impl MemoryRepository for SnapshotOnlyRepository {
    async fn apply(
        &self,
        change_set: MemoryChangeSet,
    ) -> Result<MemoryChangeResult, MemoryRepositoryError> {
        self.inner.apply(change_set).await
    }

    async fn get(
        &self,
        namespace: &MemoryNamespace,
        node_id: &str,
    ) -> Result<Option<MemoryNode>, MemoryRepositoryError> {
        self.inner.get(namespace, node_id).await
    }

    async fn query(&self, query: MemoryQuery) -> Result<MemoryQueryResult, MemoryRepositoryError> {
        self.inner.query(query).await
    }

    async fn snapshot_namespace(
        &self,
        request: MemorySnapshotRequest,
    ) -> Result<MemoryNamespaceSnapshot, MemoryRepositoryError> {
        if self.fail_next_snapshot.swap(false, Ordering::SeqCst) {
            return Err(MemoryRepositoryError::Persistence {
                operation: "snapshot_namespace".to_string(),
                message: "injected transient failure".to_string(),
            });
        }
        self.inner.snapshot_namespace(request).await
    }

    async fn record_admission(
        &self,
        event: MemoryAccessEvent,
    ) -> Result<(), MemoryRepositoryError> {
        self.inner.record_admission(event).await
    }

    async fn record_use(&self, event: MemoryAccessEvent) -> Result<(), MemoryRepositoryError> {
        self.inner.record_use(event).await
    }

    async fn usage_summary(
        &self,
        namespace: &MemoryNamespace,
        node_id: &str,
    ) -> Result<MemoryUsageSummary, MemoryRepositoryError> {
        self.inner.usage_summary(namespace, node_id).await
    }
}

pub struct NoChangeTokenVectorIndex {
    inner: Arc<InMemoryVectorIndex>,
}

impl NoChangeTokenVectorIndex {
    pub fn new(inner: Arc<InMemoryVectorIndex>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl VectorIndex for NoChangeTokenVectorIndex {
    fn descriptor(&self) -> &VectorIndexDescriptor {
        self.inner.descriptor()
    }

    fn status(&self) -> VectorIndexStatus {
        self.inner.status()
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

pub fn durable(
    repository: Arc<dyn MemoryRepository>,
    namespace: MemoryNamespace,
    provider: Arc<dyn EmbeddingProvider>,
    index: Arc<dyn VectorIndex>,
) -> DurableMemorySession {
    DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(4, 0.2).unwrap(),
    )
    .with_semantic_recall(semantic(
        provider,
        EmbeddingExecutorConfig::default(),
        index,
    ))
    .unwrap()
}

pub fn start_runtime(
    owner_id: &str,
    durable: DurableMemorySession,
    schedule: ScheduledSemanticRefresh,
) -> Arc<MemoryMaintenanceRuntime> {
    let memory = Arc::new(AgentMemory::with_config_observers_and_durable(
        Arc::new(InMemoryStore::new()),
        MemoryConfig::default(),
        Vec::new(),
        Some(durable),
    ));
    MemoryMaintenanceRuntime::start(
        owner_id,
        memory,
        MemoryMaintenanceOptions::new().with_semantic_refresh(schedule),
    )
    .unwrap()
}

pub async fn advance_until(runtime: &MemoryMaintenanceRuntime, successful_runs: u64) {
    // Awaiting a timer lets Tokio poll the newly spawned worker and arm its
    // interval before the paused clock advances.
    tokio::time::sleep(Duration::from_secs(1)).await;
    for _ in 0..4096 {
        if runtime.health().jobs[0].successful_runs >= successful_runs {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("semantic refresh did not reach {successful_runs} successful runs");
}

pub async fn advance_until_failure(runtime: &MemoryMaintenanceRuntime, failed_runs: u64) {
    tokio::time::sleep(Duration::from_secs(1)).await;
    for _ in 0..4096 {
        if runtime.health().jobs[0].failed_runs >= failed_runs {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("semantic refresh did not reach {failed_runs} failed runs");
}
