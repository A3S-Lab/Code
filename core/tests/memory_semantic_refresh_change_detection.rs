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
}

impl CountingProvider {
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
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

    advance_until_runs(&agent_session, 2).await;
    assert_eq!(
        provider.call_count(),
        1,
        "an unchanged verified snapshot must not be embedded again"
    );
    assert_eq!(index.status(), first_index_status);
    assert_eq!(schedule.last_receipt(), Some(first_receipt.clone()));
    let unchanged_health = agent_session.memory_maintenance_health();
    assert_eq!(unchanged_health.jobs[0].successful_runs, 2);
    assert_eq!(unchanged_health.jobs[0].total_affected_items, 1);
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
    assert_eq!(provider.call_count(), 2);
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
            namespace,
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
    assert_eq!(provider.call_count(), 3);
    let source_drift_receipt = schedule.last_receipt().expect("source rebuild receipt");
    assert_ne!(
        source_drift_receipt.source_snapshot_digest(),
        index_drift_receipt.source_snapshot_digest()
    );
    let running = agent_session.memory_maintenance_health();
    assert_eq!(running.jobs[0].successful_runs, 4);
    assert_eq!(running.jobs[0].total_affected_items, 3);
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
    assert_eq!(replacement_index.status().revision.value(), 1);
    replacement.close().await;
}
