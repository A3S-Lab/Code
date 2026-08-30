#[allow(dead_code)]
#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

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
    MemoryNamespaceChangeToken, MemoryNamespaceSnapshot, MemoryNode, MemoryOperation, MemoryQuery,
    MemoryQueryResult, MemoryRepository, MemoryRepositoryError, MemorySnapshotRequest,
    MemoryStatus, MemoryUsageSummary, RevisionMode,
};
use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor};
use a3s_memory::InMemoryStore;
use async_trait::async_trait;
use refresh_support::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct CountingProvider {
    calls: AtomicUsize,
    inputs: AtomicUsize,
}

impl CountingProvider {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn inputs(&self) -> usize {
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

struct ObservedRepository {
    inner: Arc<InMemoryRepository>,
    expose_change_token: bool,
    snapshots: AtomicUsize,
    token_calls: AtomicUsize,
    mutate_on_token_call: usize,
    pending_change: Mutex<Option<MemoryChangeSet>>,
}

impl ObservedRepository {
    fn new(inner: Arc<InMemoryRepository>, expose_change_token: bool) -> Self {
        Self {
            inner,
            expose_change_token,
            snapshots: AtomicUsize::new(0),
            token_calls: AtomicUsize::new(0),
            mutate_on_token_call: 0,
            pending_change: Mutex::new(None),
        }
    }

    fn with_token_call_mutation(mut self, call: usize, change: MemoryChangeSet) -> Self {
        self.mutate_on_token_call = call;
        self.pending_change = Mutex::new(Some(change));
        self
    }

    fn snapshot_count(&self) -> usize {
        self.snapshots.load(Ordering::SeqCst)
    }

    fn token_call_count(&self) -> usize {
        self.token_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl MemoryRepository for ObservedRepository {
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
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        self.inner.snapshot_namespace(request).await
    }

    async fn namespace_change_token(
        &self,
        namespace: &MemoryNamespace,
    ) -> Result<Option<MemoryNamespaceChangeToken>, MemoryRepositoryError> {
        let call = self.token_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.mutate_on_token_call {
            let change = self
                .pending_change
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(change) = change {
                self.inner.apply(change).await?;
            }
        }
        if self.expose_change_token {
            self.inner.namespace_change_token(namespace).await
        } else {
            Ok(None)
        }
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

fn revision(namespace: &MemoryNamespace, key: &str) -> MemoryChangeSet {
    MemoryChangeSet::new(
        key,
        namespace.clone(),
        time(2),
        vec![MemoryOperation::Revise {
            node_id: "alpha".into(),
            expected_revision: 1,
            content: GAMMA.into(),
            mode: RevisionMode::Correction,
            evidence: vec![evidence(key, 2)],
            confidence: None,
            importance: None,
        }],
    )
}

fn durable(
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

fn start_runtime(
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

async fn advance_until(runtime: &MemoryMaintenanceRuntime, successes: u64, failures: u64) {
    tokio::time::sleep(Duration::from_secs(1)).await;
    for _ in 0..4096 {
        let health = runtime.health();
        if health.jobs[0].successful_runs >= successes && health.jobs[0].failed_runs >= failures {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("semantic refresh did not reach success={successes} failure={failures}");
}

#[tokio::test(start_paused = true)]
async fn unsupported_repositories_keep_the_verified_full_snapshot_fallback() {
    let namespace = namespace("token-fallback");
    let inner = Arc::new(InMemoryRepository::new());
    create_node(
        inner.as_ref(),
        &namespace,
        "fallback-create",
        "alpha",
        MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let repository = Arc::new(ObservedRepository::new(inner, false));
    let provider = Arc::new(CountingProvider::default());
    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let runtime = start_runtime(
        "token-fallback-owner",
        durable(repository.clone(), namespace, provider.clone(), index),
        schedule.clone(),
    );

    advance_until(runtime.as_ref(), 1, 0).await;
    assert_eq!(repository.snapshot_count(), 2);
    assert_eq!(repository.token_call_count(), 1);
    assert!(schedule
        .last_receipt()
        .expect("fallback receipt")
        .source_change_token()
        .is_none());

    advance_until(runtime.as_ref(), 2, 0).await;
    assert_eq!(repository.snapshot_count(), 3);
    assert_eq!(repository.token_call_count(), 2);
    assert_eq!(provider.calls(), 1);
    assert_eq!(provider.inputs(), 1);
    let metrics = schedule.metrics();
    assert_eq!(metrics.total_source_change_token_requests(), 2);
    assert_eq!(metrics.total_source_change_token_observations(), 0);
    assert_eq!(metrics.total_source_snapshot_requests(), 3);
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn inactive_only_changes_advance_the_receipt_without_republishing_active_vectors() {
    let namespace = namespace("token-inactive-change");
    let inner = Arc::new(InMemoryRepository::new());
    create_node(
        inner.as_ref(),
        &namespace,
        "active-create",
        "alpha",
        MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let repository = Arc::new(ObservedRepository::new(inner.clone(), true));
    let provider = Arc::new(CountingProvider::default());
    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let vector_index: Arc<dyn VectorIndex> = index.clone();
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let runtime = start_runtime(
        "token-inactive-owner",
        durable(
            repository.clone(),
            namespace.clone(),
            provider.clone(),
            vector_index,
        ),
        schedule.clone(),
    );

    advance_until(runtime.as_ref(), 1, 0).await;
    assert_eq!(
        schedule
            .last_receipt()
            .expect("initial receipt")
            .source_change_token()
            .expect("initial token")
            .sequence(),
        1
    );
    create_node(
        inner.as_ref(),
        &namespace,
        "candidate-create",
        "candidate",
        MemoryStatus::Candidate,
        BETA,
        2,
    )
    .await;

    advance_until(runtime.as_ref(), 2, 0).await;
    let advanced = schedule.last_receipt().expect("advanced receipt");
    assert_eq!(
        advanced
            .source_change_token()
            .expect("advanced token")
            .sequence(),
        2
    );
    assert_eq!(provider.calls(), 1);
    assert_eq!(index.status().revision.value(), 1);
    assert_eq!(repository.snapshot_count(), 2);
    assert_eq!(repository.token_call_count(), 5);

    advance_until(runtime.as_ref(), 3, 0).await;
    assert_eq!(repository.snapshot_count(), 2);
    assert_eq!(repository.token_call_count(), 6);
    assert_eq!(provider.calls(), 1);
    assert_eq!(index.status().revision.value(), 1);
    let metrics = schedule.metrics();
    assert_eq!(metrics.published_runs(), 1);
    assert_eq!(metrics.unchanged_runs(), 2);
    assert_eq!(metrics.total_source_snapshot_requests(), 2);
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn token_drift_around_the_snapshot_fails_before_embedding_or_publication() {
    let namespace = namespace("token-pre-publication-drift");
    let inner = Arc::new(InMemoryRepository::new());
    create_node(
        inner.as_ref(),
        &namespace,
        "pre-drift-create",
        "alpha",
        MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let repository = Arc::new(
        ObservedRepository::new(inner, true)
            .with_token_call_mutation(2, revision(&namespace, "pre-drift-revision")),
    );
    let provider = Arc::new(CountingProvider::default());
    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let vector_index: Arc<dyn VectorIndex> = index.clone();
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let runtime = start_runtime(
        "token-pre-drift-owner",
        durable(
            repository.clone(),
            namespace,
            provider.clone(),
            vector_index,
        ),
        schedule.clone(),
    );

    advance_until(runtime.as_ref(), 0, 1).await;
    assert_eq!(repository.snapshot_count(), 1);
    assert_eq!(repository.token_call_count(), 2);
    assert_eq!(provider.calls(), 0);
    assert_eq!(index.status().revision.value(), 0);
    assert_eq!(index.status().record_count, 0);
    assert!(schedule.last_receipt().is_none());
    let metrics = schedule.metrics();
    assert_eq!(metrics.total_source_change_token_requests(), 2);
    assert_eq!(metrics.total_source_change_token_observations(), 2);
    assert_eq!(metrics.total_publication_attempts(), 0);
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn token_drift_after_publication_invalidates_before_a_receipt_is_promoted() {
    let namespace = namespace("token-post-publication-drift");
    let inner = Arc::new(InMemoryRepository::new());
    create_node(
        inner.as_ref(),
        &namespace,
        "post-drift-create",
        "alpha",
        MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let repository = Arc::new(
        ObservedRepository::new(inner, true)
            .with_token_call_mutation(3, revision(&namespace, "post-drift-revision")),
    );
    let provider = Arc::new(CountingProvider::default());
    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let vector_index: Arc<dyn VectorIndex> = index.clone();
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let runtime = start_runtime(
        "token-post-drift-owner",
        durable(
            repository.clone(),
            namespace,
            provider.clone(),
            vector_index,
        ),
        schedule.clone(),
    );

    advance_until(runtime.as_ref(), 0, 1).await;
    assert_eq!(repository.snapshot_count(), 1);
    assert_eq!(repository.token_call_count(), 3);
    assert_eq!(provider.calls(), 1);
    assert_eq!(provider.inputs(), 1);
    assert_eq!(index.status().revision.value(), 2);
    assert_eq!(index.status().record_count, 0);
    assert!(schedule.last_receipt().is_none());
    let failed = schedule.metrics();
    assert_eq!(failed.total_source_change_token_requests(), 3);
    assert_eq!(failed.total_source_change_token_observations(), 3);
    assert_eq!(failed.total_source_snapshot_requests(), 1);
    assert_eq!(failed.total_publication_attempts(), 1);

    advance_until(runtime.as_ref(), 1, 1).await;
    assert_eq!(repository.snapshot_count(), 2);
    assert_eq!(repository.token_call_count(), 6);
    assert_eq!(provider.calls(), 2);
    assert_eq!(index.status().record_count, 1);
    let receipt = schedule.last_receipt().expect("recovery receipt");
    assert_eq!(
        receipt
            .source_change_token()
            .expect("recovered source token")
            .sequence(),
        2
    );
    let encoded = serde_json::to_string(&receipt).unwrap();
    assert!(!encoded.contains("token-post-publication-drift"));
    assert!(!encoded.contains(GAMMA));
    runtime.close().await;
}
