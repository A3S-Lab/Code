#[allow(dead_code)]
#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError,
};
use a3s_code_core::memory::{
    AgentMemory, MemoryConfig, MemoryMaintenanceOptions, MemoryMaintenanceRuntime,
    ScheduledSemanticRefresh, SemanticRefreshRunMetrics, SemanticRefreshRunOutcome,
};
use a3s_memory::repository::{
    InMemoryRepository, MemoryChangeSet, MemoryOperation, MemoryRepository, MemoryStatus,
    RevisionMode,
};
use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorRecord};
use a3s_memory::InMemoryStore;
use async_trait::async_trait;
use refresh_support::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct ObservedProvider {
    calls: AtomicUsize,
    fail_next: AtomicBool,
    interfere_next: AtomicBool,
    interference_index: Option<Arc<InMemoryVectorIndex>>,
}

impl ObservedProvider {
    fn with_interference(index: Arc<InMemoryVectorIndex>) -> Self {
        Self {
            interference_index: Some(index),
            ..Self::default()
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn fail_once(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    fn interfere_once(&self) {
        self.interfere_next.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl EmbeddingProvider for ObservedProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        FixtureProvider.descriptor()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(EmbeddingProviderError::Unavailable { retry_after: None });
        }
        if self.interfere_next.swap(false, Ordering::SeqCst) {
            self.interference_index
                .as_ref()
                .expect("interference index")
                .replace_partition(
                    "metrics-interference",
                    vec![VectorRecord::new("interference", vec![0.0, 1.0])],
                )
                .await
                .expect("publish provider interference");
        }
        FixtureProvider.embed(request, cancellation).await
    }
}

fn start_runtime(
    owner_id: &str,
    durable: a3s_code_core::DurableMemorySession,
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

async fn advance_until(runtime: &MemoryMaintenanceRuntime, successful_runs: u64, failed_runs: u64) {
    tokio::time::sleep(Duration::from_secs(1)).await;
    for _ in 0..4096 {
        let health = runtime.health();
        if health.jobs[0].successful_runs >= successful_runs
            && health.jobs[0].failed_runs >= failed_runs
        {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("semantic refresh did not reach success={successful_runs} failure={failed_runs}");
}

fn assert_send_sync<T: Send + Sync>() {}

#[tokio::test(start_paused = true)]
async fn metrics_quantify_snapshot_cache_provider_and_publication_work_per_epoch() {
    assert_send_sync::<a3s_code_core::memory::SemanticRefreshMetrics>();
    assert_send_sync::<SemanticRefreshRunMetrics>();
    assert_send_sync::<SemanticRefreshRunOutcome>();

    let namespace = namespace("scheduled-metrics");
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
    create_node(
        repository.as_ref(),
        &namespace,
        "create-beta",
        "beta",
        MemoryStatus::Active,
        BETA,
        1,
    )
    .await;
    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let provider = Arc::new(ObservedProvider::with_interference(index.clone()));
    let vector_index: Arc<dyn VectorIndex> = index.clone();
    let durable = session(
        repository.clone(),
        namespace.clone(),
        semantic(
            provider.clone(),
            EmbeddingExecutorConfig {
                max_retries: 0,
                ..EmbeddingExecutorConfig::default()
            },
            vector_index,
        ),
    );
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    assert_eq!(schedule.metrics().ownership_epoch(), 0);
    assert_eq!(schedule.metrics().attempted_runs(), 0);

    let runtime = start_runtime("metrics-owner-1", durable.clone(), schedule.clone());
    assert_eq!(schedule.metrics().ownership_epoch(), 1);
    assert_eq!(schedule.metrics().attempted_runs(), 0);

    advance_until(runtime.as_ref(), 1, 0).await;
    let first_receipt = schedule.last_receipt().expect("initial receipt");
    assert_eq!(
        first_receipt
            .source_change_token()
            .expect("built-in repository token")
            .sequence(),
        2
    );
    let first = schedule.metrics();
    assert_eq!(first.attempted_runs(), 1);
    assert_eq!(first.published_runs(), 1);
    assert_eq!(first.unchanged_runs(), 0);
    assert_eq!(first.failed_runs(), 0);
    assert_eq!(first.total_source_change_token_requests(), 3);
    assert_eq!(first.total_source_change_token_observations(), 3);
    assert_eq!(first.total_source_snapshot_requests(), 1);
    assert_eq!(first.total_source_snapshot_node_reads(), 2);
    assert_eq!(
        first.total_source_snapshot_bytes(),
        u64::try_from(first_receipt.source_snapshot_bytes()).unwrap()
    );
    assert_eq!(first.total_embedding_cache_hits(), 0);
    assert_eq!(first.total_embedding_inputs(), 2);
    assert_eq!(
        first.total_embedding_input_bytes(),
        u64::try_from(ALPHA.len() + BETA.len()).unwrap()
    );
    assert_eq!(first.total_provider_requests(), 1);
    assert_eq!(first.total_provider_inputs(), 2);
    assert_eq!(
        first.total_provider_input_bytes(),
        u64::try_from(ALPHA.len() + BETA.len()).unwrap()
    );
    assert_eq!(first.total_publication_attempts(), 1);
    assert_eq!(first.total_publication_records(), 2);
    assert_eq!(first.recent_runs().len(), 1);
    assert_eq!(
        first.last_run().expect("first run").outcome(),
        SemanticRefreshRunOutcome::Published
    );

    advance_until(runtime.as_ref(), 2, 0).await;
    let unchanged = schedule.metrics();
    assert_eq!(unchanged.attempted_runs(), 2);
    assert_eq!(unchanged.unchanged_runs(), 1);
    assert_eq!(unchanged.total_source_change_token_requests(), 4);
    assert_eq!(unchanged.total_source_change_token_observations(), 4);
    assert_eq!(unchanged.total_source_snapshot_requests(), 1);
    assert_eq!(unchanged.total_source_snapshot_node_reads(), 2);
    assert_eq!(unchanged.total_embedding_inputs(), 2);
    assert_eq!(unchanged.total_provider_requests(), 1);
    assert_eq!(unchanged.total_provider_inputs(), 2);
    assert_eq!(unchanged.total_publication_attempts(), 1);
    assert_eq!(
        unchanged.last_run().expect("unchanged run").outcome(),
        SemanticRefreshRunOutcome::Unchanged
    );
    assert_eq!(
        unchanged
            .last_run()
            .expect("unchanged run")
            .source_change_token_requests(),
        1
    );
    assert_eq!(
        unchanged
            .last_run()
            .expect("unchanged run")
            .source_snapshot_requests(),
        0
    );

    index
        .replace_partition(
            "metrics-independent",
            vec![VectorRecord::new("independent", vec![1.0, 0.0])],
        )
        .await
        .unwrap();
    advance_until(runtime.as_ref(), 3, 0).await;
    let index_drift = schedule.metrics();
    assert_eq!(index_drift.published_runs(), 2);
    assert_eq!(index_drift.total_source_change_token_requests(), 7);
    assert_eq!(index_drift.total_source_change_token_observations(), 7);
    assert_eq!(index_drift.total_source_snapshot_requests(), 2);
    assert_eq!(index_drift.total_embedding_cache_hits(), 2);
    assert_eq!(index_drift.total_embedding_inputs(), 2);
    assert_eq!(index_drift.total_provider_requests(), 1);
    assert_eq!(index_drift.total_provider_inputs(), 2);
    assert_eq!(index_drift.total_publication_attempts(), 2);

    repository
        .apply(MemoryChangeSet::new(
            "metrics-source-change",
            namespace,
            time(2),
            vec![MemoryOperation::Revise {
                node_id: "alpha".into(),
                expected_revision: 1,
                content: GAMMA.into(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence("metrics-source-change", 2)],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .unwrap();
    provider.interfere_once();
    advance_until(runtime.as_ref(), 3, 1).await;
    let failed = schedule.metrics();
    assert_eq!(failed.attempted_runs(), 4);
    assert_eq!(failed.failed_runs(), 1);
    assert_eq!(failed.total_source_change_token_requests(), 9);
    assert_eq!(failed.total_source_change_token_observations(), 9);
    assert_eq!(failed.total_source_snapshot_requests(), 3);
    assert_eq!(failed.total_embedding_cache_hits(), 3);
    assert_eq!(failed.total_embedding_inputs(), 3);
    assert_eq!(failed.total_provider_requests(), 2);
    assert_eq!(failed.total_provider_inputs(), 3);
    assert_eq!(
        failed.total_provider_input_bytes(),
        u64::try_from(ALPHA.len() + BETA.len() + GAMMA.len()).unwrap()
    );
    assert_eq!(failed.total_publication_attempts(), 3);
    assert_eq!(
        failed.last_run().expect("failed run").outcome(),
        SemanticRefreshRunOutcome::Failed
    );

    advance_until(runtime.as_ref(), 4, 1).await;
    let recovered = schedule.metrics();
    assert_eq!(recovered.attempted_runs(), 5);
    assert_eq!(recovered.published_runs(), 3);
    assert_eq!(recovered.unchanged_runs(), 1);
    assert_eq!(recovered.failed_runs(), 1);
    assert_eq!(recovered.total_source_change_token_requests(), 12);
    assert_eq!(recovered.total_source_change_token_observations(), 12);
    assert_eq!(recovered.total_source_snapshot_requests(), 4);
    assert_eq!(recovered.total_embedding_cache_hits(), 4);
    assert_eq!(recovered.total_embedding_inputs(), 4);
    assert_eq!(recovered.total_provider_requests(), 3);
    assert_eq!(recovered.total_provider_inputs(), 4);
    assert_eq!(
        recovered.total_provider_input_bytes(),
        u64::try_from(ALPHA.len() + BETA.len() + GAMMA.len() * 2).unwrap()
    );
    assert_eq!(recovered.total_publication_attempts(), 4);
    assert_eq!(recovered.total_publication_records(), 8);
    assert_eq!(recovered.recent_runs().len(), 5);
    assert_eq!(recovered.recent_runs()[3].sequence(), 4);
    assert_eq!(recovered.recent_runs()[3].provider_requests(), 1);
    assert_eq!(recovered.recent_runs()[3].provider_inputs(), 1);
    assert_eq!(provider.calls(), 3);

    let encoded = serde_json::to_string(&recovered).unwrap();
    let debug = format!("{recovered:?}");
    for secret in [ALPHA, BETA, GAMMA, "alpha", "beta", "sha256:"] {
        assert!(!encoded.contains(secret));
        assert!(!debug.contains(secret));
    }

    runtime.close().await;
    assert_eq!(schedule.metrics(), recovered);
    let replacement = start_runtime("metrics-owner-2", durable, schedule.clone());
    let reset = schedule.metrics();
    assert_eq!(reset.ownership_epoch(), 2);
    assert_eq!(reset.attempted_runs(), 0);
    assert!(reset.recent_runs().is_empty());
    assert!(schedule.last_receipt().is_none());
    replacement.close().await;
}

#[tokio::test(start_paused = true)]
async fn failed_provider_requests_are_observable_without_content_or_error_bodies() {
    let namespace = namespace("scheduled-provider-metrics");
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
    let provider = Arc::new(ObservedProvider::default());
    provider.fail_once();
    let vector_index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let durable = session(
        repository,
        namespace,
        semantic(
            provider.clone(),
            EmbeddingExecutorConfig {
                max_retries: 0,
                ..EmbeddingExecutorConfig::default()
            },
            vector_index,
        ),
    );
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let runtime = start_runtime("provider-metrics-owner", durable, schedule.clone());

    advance_until(runtime.as_ref(), 0, 1).await;
    let failed = schedule.metrics();
    assert_eq!(failed.attempted_runs(), 1);
    assert_eq!(failed.failed_runs(), 1);
    assert_eq!(failed.total_source_change_token_requests(), 2);
    assert_eq!(failed.total_source_change_token_observations(), 2);
    assert_eq!(failed.total_source_snapshot_requests(), 1);
    assert_eq!(failed.total_embedding_inputs(), 1);
    assert_eq!(failed.total_provider_requests(), 1);
    assert_eq!(failed.total_provider_inputs(), 1);
    assert_eq!(
        failed.total_provider_input_bytes(),
        u64::try_from(ALPHA.len()).unwrap()
    );
    assert_eq!(failed.total_publication_attempts(), 0);
    assert_eq!(provider.calls(), 1);

    advance_until(runtime.as_ref(), 1, 1).await;
    let recovered = schedule.metrics();
    assert_eq!(recovered.attempted_runs(), 2);
    assert_eq!(recovered.published_runs(), 1);
    assert_eq!(recovered.failed_runs(), 1);
    assert_eq!(recovered.total_source_change_token_requests(), 5);
    assert_eq!(recovered.total_source_change_token_observations(), 5);
    assert_eq!(recovered.total_source_snapshot_requests(), 2);
    assert_eq!(recovered.total_embedding_inputs(), 2);
    assert_eq!(recovered.total_provider_requests(), 2);
    assert_eq!(recovered.total_provider_inputs(), 2);
    assert_eq!(
        recovered.total_provider_input_bytes(),
        u64::try_from(ALPHA.len() * 2).unwrap()
    );
    assert_eq!(recovered.total_publication_attempts(), 1);
    runtime.close().await;
}
