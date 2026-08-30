#[allow(dead_code)]
#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError,
};
use a3s_code_core::llm::{LlmResponse, Message, StreamEvent, ToolDefinition};
use a3s_code_core::memory::{MemoryMaintenanceOptions, ScheduledSemanticRefresh};
use a3s_code_core::{Agent, AgentSession, CodeConfig, LlmClient, SessionOptions};
use a3s_memory::repository::{
    InMemoryRepository, MemoryChangeSet, MemoryOperation, MemoryRepository, MemoryStatus,
    RevisionMode,
};
use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorRecord};
use a3s_memory::InMemoryStore;
use async_trait::async_trait;
use refresh_support::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct UnusedLlmClient;

#[async_trait]
impl LlmClient for UnusedLlmClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("semantic refresh must not call the conversation model")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("semantic refresh must not call the conversation model")
    }
}

#[derive(Default)]
struct CountingProvider {
    calls: AtomicUsize,
    inputs: AtomicUsize,
    interference_index: Option<Arc<InMemoryVectorIndex>>,
    interfere_next: std::sync::atomic::AtomicBool,
}

impl CountingProvider {
    fn with_interference(index: Arc<InMemoryVectorIndex>) -> Self {
        Self {
            interference_index: Some(index),
            ..Self::default()
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn input_count(&self) -> usize {
        self.inputs.load(Ordering::SeqCst)
    }

    fn interfere_once(&self) {
        self.interfere_next.store(true, Ordering::SeqCst);
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
        if self.interfere_next.swap(false, Ordering::SeqCst) {
            self.interference_index
                .as_ref()
                .expect("interference index")
                .replace_partition(
                    "provider-interference",
                    vec![VectorRecord::new("interference", vec![0.0, 1.0])],
                )
                .await
                .expect("publish provider interference");
        }
        FixtureProvider.embed(request, cancellation).await
    }
}

fn session_options(
    session_id: &str,
    durable_memory: a3s_code_core::DurableMemorySession,
    schedule: ScheduledSemanticRefresh,
) -> SessionOptions {
    let llm: Arc<dyn LlmClient> = Arc::new(UnusedLlmClient);
    SessionOptions::new()
        .with_session_id(session_id)
        .with_llm_client(llm)
        .with_memory(Arc::new(InMemoryStore::new()))
        .with_durable_memory(durable_memory)
        .with_memory_maintenance(MemoryMaintenanceOptions::new().with_semantic_refresh(schedule))
}

async fn settle_workers() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

async fn advance_until_runs(session: &AgentSession, expected_runs: u64) {
    settle_workers().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    for _ in 0..256 {
        if session
            .memory_maintenance_health()
            .jobs
            .iter()
            .any(|job| job.successful_runs >= expected_runs)
        {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("semantic refresh did not complete run {expected_runs}");
}

async fn advance_until_failures(session: &AgentSession, expected_failures: u64) {
    settle_workers().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    for _ in 0..256 {
        if session
            .memory_maintenance_health()
            .jobs
            .iter()
            .any(|job| job.failed_runs >= expected_failures)
        {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("semantic refresh did not record failure {expected_failures}");
}

#[tokio::test(start_paused = true)]
async fn unchanged_ticks_skip_embedding_but_source_or_index_drift_rebuilds() {
    let namespace = namespace("scheduled-change-detection");
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
    let provider = Arc::new(CountingProvider::default());
    let index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let vector_index: Arc<dyn VectorIndex> = index.clone();
    let durable = session(
        repository.clone(),
        namespace.clone(),
        semantic(
            provider.clone(),
            EmbeddingExecutorConfig::default(),
            vector_index,
        ),
    );
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let agent = Agent::from_config(CodeConfig::default()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let agent_session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(
                "scheduled-change-detection",
                durable,
                schedule.clone(),
            )),
        )
        .await
        .unwrap();

    advance_until_runs(&agent_session, 1).await;
    let first_receipt = schedule.last_receipt().expect("initial refresh receipt");
    let first_index_status = index.status();
    assert_eq!(provider.call_count(), 1);
    assert_eq!(provider.input_count(), 2);

    advance_until_runs(&agent_session, 2).await;
    assert_eq!(
        provider.call_count(),
        1,
        "an unchanged verified snapshot must not be embedded again"
    );
    assert_eq!(provider.input_count(), 2);
    assert_eq!(index.status(), first_index_status);
    assert_eq!(schedule.last_receipt(), Some(first_receipt.clone()));
    let unchanged_health = agent_session.memory_maintenance_health();
    assert_eq!(unchanged_health.jobs[0].successful_runs, 2);
    assert_eq!(unchanged_health.jobs[0].total_affected_items, 2);
    assert_eq!(unchanged_health.jobs[0].last_affected_items, Some(0));

    index
        .replace_partition(
            "independent-partition",
            vec![VectorRecord::new("independent", vec![0.0, 1.0])],
        )
        .await
        .unwrap();
    let independent_status = index.status();
    advance_until_runs(&agent_session, 3).await;
    assert_eq!(
        provider.call_count(),
        1,
        "index drift must republish verified cached vectors without provider egress"
    );
    assert_eq!(provider.input_count(), 2);
    assert!(index.status().revision > independent_status.revision);
    let index_drift_receipt = schedule
        .last_receipt()
        .expect("index-drift rebuild receipt");
    assert_eq!(
        index_drift_receipt.source_snapshot_digest(),
        first_receipt.source_snapshot_digest()
    );
    assert!(index_drift_receipt.index_status().revision > first_receipt.index_status().revision);

    repository
        .apply(MemoryChangeSet::new(
            "scheduled-source-change",
            namespace.clone(),
            time(2),
            vec![MemoryOperation::Revise {
                node_id: "alpha".into(),
                expected_revision: 1,
                content: GAMMA.into(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence("scheduled-source-change", 2)],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .unwrap();
    advance_until_runs(&agent_session, 4).await;
    assert_eq!(provider.call_count(), 2);
    assert_eq!(
        provider.input_count(),
        3,
        "only the changed Active node may leave the process for embedding"
    );
    let source_drift_receipt = schedule.last_receipt().expect("source rebuild receipt");
    assert_ne!(
        source_drift_receipt.source_snapshot_digest(),
        index_drift_receipt.source_snapshot_digest()
    );

    repository
        .apply(MemoryChangeSet::new(
            "scheduled-active-removal",
            namespace,
            time(3),
            vec![MemoryOperation::SetStatus {
                node_id: "beta".into(),
                expected_revision: 1,
                status: MemoryStatus::Tombstoned,
            }],
        ))
        .await
        .unwrap();
    advance_until_runs(&agent_session, 5).await;
    assert_eq!(provider.call_count(), 2);
    assert_eq!(
        provider.input_count(),
        3,
        "removing an Active node must rebuild entirely from retained verified vectors"
    );
    let removal_receipt = schedule.last_receipt().expect("removal rebuild receipt");
    assert_eq!(removal_receipt.active_node_count(), 1);
    assert_ne!(
        removal_receipt.source_snapshot_digest(),
        source_drift_receipt.source_snapshot_digest()
    );
    assert_eq!(index.status().record_count, 2);
    let running = agent_session.memory_maintenance_health();
    assert_eq!(running.jobs[0].successful_runs, 5);
    assert_eq!(running.jobs[0].total_affected_items, 7);
    assert_eq!(running.jobs[0].last_affected_items, Some(1));
    agent_session.close().await;
}

#[tokio::test(start_paused = true)]
async fn a_new_schedule_owner_discards_the_previous_process_local_receipt() {
    let namespace = namespace("scheduled-owner-epoch");
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
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let agent = Agent::from_config(CodeConfig::default()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();

    let first_provider = Arc::new(CountingProvider::default());
    let first_index = Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let first_vector_index: Arc<dyn VectorIndex> = first_index;
    let first = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(
                "scheduled-owner-epoch-first",
                session(
                    repository.clone(),
                    namespace.clone(),
                    semantic(
                        first_provider,
                        EmbeddingExecutorConfig::default(),
                        first_vector_index,
                    ),
                ),
                schedule.clone(),
            )),
        )
        .await
        .unwrap();
    advance_until_runs(&first, 1).await;
    first.close().await;
    assert!(schedule.last_receipt().is_some());

    let replacement_provider = Arc::new(CountingProvider::default());
    let replacement_index =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let replacement_vector_index: Arc<dyn VectorIndex> = replacement_index.clone();
    let replacement = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(
                "scheduled-owner-epoch-replacement",
                session(
                    repository,
                    namespace,
                    semantic(
                        replacement_provider.clone(),
                        EmbeddingExecutorConfig::default(),
                        replacement_vector_index,
                    ),
                ),
                schedule.clone(),
            )),
        )
        .await
        .unwrap();
    assert!(
        schedule.last_receipt().is_none(),
        "a process-local receipt must not authorize skipping on a new backend owner"
    );
    advance_until_runs(&replacement, 1).await;
    assert_eq!(replacement_provider.call_count(), 1);
    assert_eq!(replacement_provider.input_count(), 1);
    assert_eq!(replacement_index.status().revision.value(), 1);
    replacement.close().await;
}

#[tokio::test(start_paused = true)]
async fn failed_cas_publication_does_not_promote_prepared_embeddings() {
    let namespace = namespace("scheduled-cache-publication");
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
    let provider = Arc::new(CountingProvider::with_interference(index.clone()));
    let vector_index: Arc<dyn VectorIndex> = index.clone();
    let durable = session(
        repository.clone(),
        namespace.clone(),
        semantic(
            provider.clone(),
            EmbeddingExecutorConfig::default(),
            vector_index,
        ),
    );
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(1)).unwrap();
    let agent = Agent::from_config(CodeConfig::default()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let agent_session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(
                "scheduled-cache-publication",
                durable,
                schedule.clone(),
            )),
        )
        .await
        .unwrap();

    advance_until_runs(&agent_session, 1).await;
    let first_receipt = schedule.last_receipt().expect("initial refresh receipt");
    assert_eq!(provider.input_count(), 2);

    repository
        .apply(MemoryChangeSet::new(
            "scheduled-cache-source-change",
            namespace,
            time(2),
            vec![MemoryOperation::Revise {
                node_id: "alpha".into(),
                expected_revision: 1,
                content: GAMMA.into(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence("scheduled-cache-source-change", 2)],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .unwrap();
    provider.interfere_once();
    advance_until_failures(&agent_session, 1).await;
    assert_eq!(provider.input_count(), 3);
    assert_eq!(schedule.last_receipt(), Some(first_receipt.clone()));
    let failed = agent_session.memory_maintenance_health();
    assert_eq!(failed.jobs[0].successful_runs, 1);
    assert_eq!(failed.jobs[0].failed_runs, 1);
    assert_eq!(failed.jobs[0].total_affected_items, 2);

    advance_until_runs(&agent_session, 2).await;
    assert_eq!(provider.call_count(), 3);
    assert_eq!(
        provider.input_count(),
        4,
        "the changed node must be embedded again after its prepared CAS publication lost"
    );
    let recovered = schedule.last_receipt().expect("recovered refresh receipt");
    assert_ne!(
        recovered.source_snapshot_digest(),
        first_receipt.source_snapshot_digest()
    );
    let healthy = agent_session.memory_maintenance_health();
    assert_eq!(healthy.jobs[0].successful_runs, 2);
    assert_eq!(healthy.jobs[0].failed_runs, 1);
    assert_eq!(healthy.jobs[0].total_affected_items, 4);
    agent_session.close().await;
}
