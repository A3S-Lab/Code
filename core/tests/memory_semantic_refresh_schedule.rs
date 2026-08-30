#[allow(dead_code)]
#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

use a3s_code_core::llm::{LlmResponse, Message, StreamEvent, ToolDefinition};
use a3s_code_core::memory::{
    MemoryMaintenanceOptions, MemoryMaintenancePhase, ScheduledSemanticRefresh,
    SEMANTIC_REFRESH_JOB_NAME,
};
use a3s_code_core::{
    Agent, AgentSession, CodeConfig, CodeError, LlmClient, SessionBuildResource, SessionOptions,
};
use a3s_memory::repository::{
    InMemoryRepository, MemoryChangeSet, MemoryOperation, MemoryRepository, MemoryStatus,
    RevisionMode,
};
use a3s_memory::vector::{
    InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorIndexStatus,
    VectorMutationConsistency, VectorRecord, VectorResult, VectorSearchRequest, VectorSearchResult,
};
use a3s_memory::InMemoryStore;
use async_trait::async_trait;
use refresh_support::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
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
        anyhow::bail!("scheduled semantic refresh must not call the conversation model")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("scheduled semantic refresh must not call the conversation model")
    }
}

struct PartitionAtomicOnlyIndex {
    inner: InMemoryVectorIndex,
}

struct PostPublicationGateIndex {
    inner: InMemoryVectorIndex,
    published: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl VectorIndex for PostPublicationGateIndex {
    fn descriptor(&self) -> &VectorIndexDescriptor {
        self.inner.descriptor()
    }

    fn status(&self) -> VectorIndexStatus {
        self.inner.status()
    }

    fn mutation_consistency(&self) -> VectorMutationConsistency {
        VectorMutationConsistency::IndexRevisionCas
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
        expected_revision: a3s_memory::vector::VectorRevision,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        let status = self
            .inner
            .replace_partition_if_revision(partition, expected_revision, records)
            .await?;
        self.published.notify_one();
        self.release.notified().await;
        Ok(status)
    }

    async fn remove_partition(&self, partition: &str) -> VectorResult<VectorIndexStatus> {
        self.inner.remove_partition(partition).await
    }

    async fn remove_partition_if_revision(
        &self,
        partition: &str,
        expected_revision: a3s_memory::vector::VectorRevision,
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

#[async_trait]
impl VectorIndex for PartitionAtomicOnlyIndex {
    fn descriptor(&self) -> &VectorIndexDescriptor {
        self.inner.descriptor()
    }

    fn status(&self) -> VectorIndexStatus {
        self.inner.status()
    }

    async fn replace_partition(
        &self,
        partition: &str,
        records: Vec<VectorRecord>,
    ) -> VectorResult<VectorIndexStatus> {
        self.inner.replace_partition(partition, records).await
    }

    async fn remove_partition(&self, partition: &str) -> VectorResult<VectorIndexStatus> {
        self.inner.remove_partition(partition).await
    }

    async fn search(&self, request: VectorSearchRequest) -> VectorResult<VectorSearchResult> {
        self.inner.search(request).await
    }

    async fn clear(&self) -> VectorResult<VectorIndexStatus> {
        self.inner.clear().await
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

async fn advance_until_runs(session: &AgentSession, interval: Duration, expected_runs: u64) {
    settle_workers().await;
    tokio::time::advance(interval).await;
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
async fn scheduled_semantic_refresh_publishes_revisions_retains_receipts_and_stops_on_close() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ScheduledSemanticRefresh>();

    let namespace = namespace("scheduled-refresh");
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
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let durable = session(
        repository.clone(),
        namespace.clone(),
        semantic(Arc::new(FixtureProvider), Default::default(), index.clone()),
    );
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(5)).unwrap();
    assert_eq!(schedule.interval(), Duration::from_secs(5));
    assert_eq!(
        schedule.required_consistency(),
        VectorMutationConsistency::IndexRevisionCas
    );
    assert!(schedule.last_receipt().is_none());

    let agent = Agent::from_config(CodeConfig::default()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let agent_session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(
                "scheduled-semantic-refresh",
                durable.clone(),
                schedule.clone(),
            )),
        )
        .await
        .unwrap();

    let initial = agent_session.memory_maintenance_health();
    assert_eq!(initial.phase, MemoryMaintenancePhase::Running);
    assert_eq!(initial.jobs.len(), 1);
    assert_eq!(initial.jobs[0].name, SEMANTIC_REFRESH_JOB_NAME);
    assert_eq!(index.status(), VectorIndexStatus::default());

    let duplicate_error = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(
                "scheduled-semantic-refresh-duplicate",
                durable.clone(),
                schedule.clone(),
            )),
        )
        .await
        .expect_err("one schedule handle must not have two active owners");
    assert!(matches!(
        duplicate_error,
        CodeError::SessionInitialization {
            resource: SessionBuildResource::MemoryMaintenance,
            ..
        }
    ));
    assert!(duplicate_error
        .to_string()
        .contains("semantic refresh schedule already has an active maintenance owner"));

    advance_until_runs(&agent_session, Duration::from_secs(5), 1).await;

    let first = schedule.last_receipt().expect("first retained receipt");
    assert_eq!(first.active_node_count(), 1);
    assert_eq!(
        first.mutation_consistency(),
        VectorMutationConsistency::IndexRevisionCas
    );
    assert_eq!(first.index_status(), &index.status());
    let alpha = durable.preview_recall(ALPHA_QUERY).await.unwrap();
    assert_eq!(alpha.hits.len(), 1);
    assert_eq!(alpha.hits[0].node_revision, 1);

    repository
        .apply(MemoryChangeSet::new(
            "scheduled-revision",
            namespace,
            time(2),
            vec![MemoryOperation::Revise {
                node_id: "alpha".into(),
                expected_revision: 1,
                content: GAMMA.into(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence("scheduled-revision", 2)],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .unwrap();
    advance_until_runs(&agent_session, Duration::from_secs(5), 2).await;

    let second = schedule.last_receipt().expect("second retained receipt");
    assert_ne!(
        second.source_snapshot_digest(),
        first.source_snapshot_digest()
    );
    assert!(second.index_status().revision > first.index_status().revision);
    let gamma = durable.preview_recall(GAMMA_QUERY).await.unwrap();
    assert_eq!(gamma.hits.len(), 1);
    assert_eq!(gamma.hits[0].node_revision, 2);
    let running = agent_session.memory_maintenance_health();
    assert_eq!(running.jobs[0].successful_runs, 2);
    assert_eq!(running.jobs[0].total_affected_items, 2);

    agent_session.close().await;
    let closed_revision = index.status().revision;
    assert_eq!(
        agent_session.memory_maintenance_health().phase,
        MemoryMaintenancePhase::Closed
    );
    tokio::time::advance(Duration::from_secs(15)).await;
    settle_workers().await;
    assert_eq!(index.status().revision, closed_revision);
    assert_eq!(schedule.last_receipt(), Some(second));

    let replacement = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(
                "scheduled-semantic-refresh-replacement",
                durable,
                schedule,
            )),
        )
        .await
        .expect("close must release the schedule ownership claim");
    assert_eq!(
        replacement.memory_maintenance_health().phase,
        MemoryMaintenancePhase::Running
    );
    replacement.close().await;
}

#[tokio::test(start_paused = true)]
async fn close_waits_for_post_publication_verification_and_retains_the_receipt() {
    let namespace = namespace("scheduled-close-after-publication");
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
    let published = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let gated_index = Arc::new(PostPublicationGateIndex {
        inner: InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap(),
        published: published.clone(),
        release: release.clone(),
    });
    let index: Arc<dyn VectorIndex> = gated_index.clone();
    let durable = session(
        repository,
        namespace,
        semantic(Arc::new(FixtureProvider), Default::default(), index),
    );
    let schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(5)).unwrap();
    let agent = Agent::from_config(CodeConfig::default()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let agent_session = Arc::new(
        agent
            .session_async(
                workspace.path().display().to_string(),
                Some(session_options(
                    "scheduled-close-after-publication",
                    durable,
                    schedule.clone(),
                )),
            )
            .await
            .unwrap(),
    );

    settle_workers().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    published.notified().await;
    assert_eq!(gated_index.status().revision.value(), 1);
    assert!(schedule.last_receipt().is_none());

    let closing_session = agent_session.clone();
    let close = tokio::spawn(async move {
        closing_session.close().await;
    });
    settle_workers().await;
    assert!(
        !close.is_finished(),
        "close discarded post-publication verification"
    );

    release.notify_one();
    close.await.unwrap();
    let receipt = schedule.last_receipt().expect("verified close receipt");
    assert_eq!(receipt.index_status().revision.value(), 1);
    assert_eq!(
        agent_session.memory_maintenance_health().phase,
        MemoryMaintenancePhase::Closed
    );
}

#[tokio::test]
async fn scheduled_semantic_refresh_rejects_missing_or_weak_bindings_before_start() {
    assert!(ScheduledSemanticRefresh::try_new(Duration::ZERO).is_err());
    let agent = Agent::from_config(CodeConfig::default()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let llm: Arc<dyn LlmClient> = Arc::new(UnusedLlmClient);
    let missing_schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(5)).unwrap();
    let missing = SessionOptions::new()
        .with_session_id("scheduled-semantic-missing")
        .with_llm_client(llm)
        .with_memory(Arc::new(InMemoryStore::new()))
        .with_memory_maintenance(
            MemoryMaintenanceOptions::new().with_semantic_refresh(missing_schedule.clone()),
        );
    let error = agent
        .session_async(workspace.path().display().to_string(), Some(missing))
        .await
        .expect_err("missing durable binding must fail before worker start");
    assert!(matches!(
        error,
        CodeError::SessionInitialization {
            resource: SessionBuildResource::MemoryMaintenance,
            ..
        }
    ));
    assert!(missing_schedule.last_receipt().is_none());

    let namespace = namespace("scheduled-weak-index");
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
    let weak_index: Arc<dyn VectorIndex> = Arc::new(PartitionAtomicOnlyIndex {
        inner: InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap(),
    });
    let durable = session(
        repository,
        namespace,
        semantic(
            Arc::new(FixtureProvider),
            Default::default(),
            weak_index.clone(),
        ),
    );
    let weak_schedule = ScheduledSemanticRefresh::try_new(Duration::from_secs(5)).unwrap();
    let error = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(
                "scheduled-semantic-weak-index",
                durable,
                weak_schedule.clone(),
            )),
        )
        .await
        .expect_err("weak mutation consistency must fail before worker start");
    assert!(matches!(
        error,
        CodeError::SessionInitialization {
            resource: SessionBuildResource::MemoryMaintenance,
            ..
        }
    ));
    assert_eq!(weak_index.status(), VectorIndexStatus::default());
    assert!(weak_schedule.last_receipt().is_none());
}
