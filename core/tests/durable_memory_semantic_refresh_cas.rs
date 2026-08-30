#[allow(dead_code)]
#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

use a3s_code_core::embedding::EmbeddingExecutorConfig;
use a3s_code_core::{DurableMemoryRecallPolicy, DurableMemorySemanticError, DurableMemorySession};
use a3s_memory::repository::{
    InMemoryRepository, MemoryAccessEvent, MemoryChangeResult, MemoryChangeSet, MemoryNamespace,
    MemoryNamespaceSnapshot, MemoryNode, MemoryOperation, MemoryQuery, MemoryQueryResult,
    MemoryRepository, MemoryRepositoryError, MemorySnapshotRequest, MemoryUsageSummary,
    RevisionMode,
};
use a3s_memory::vector::{
    InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorIndexError,
    VectorMutationConsistency, VectorRevision,
};
use async_trait::async_trait;
use refresh_support::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct PostSnapshotGateRepository {
    inner: Arc<InMemoryRepository>,
    snapshot_calls: AtomicUsize,
    post_snapshot_started: Arc<Notify>,
    release_post_snapshot: Arc<Notify>,
}

#[async_trait]
impl MemoryRepository for PostSnapshotGateRepository {
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
        if self.snapshot_calls.fetch_add(1, Ordering::SeqCst) == 1 {
            self.post_snapshot_started.notify_one();
            self.release_post_snapshot.notified().await;
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

fn wrapped_session(
    repository: Arc<PostSnapshotGateRepository>,
    namespace: MemoryNamespace,
    index: Arc<dyn VectorIndex>,
) -> DurableMemorySession {
    DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(4, 0.2).unwrap(),
    )
    .with_semantic_recall(semantic(
        Arc::new(FixtureProvider),
        EmbeddingExecutorConfig::default(),
        index,
    ))
    .unwrap()
}

#[tokio::test]
async fn delayed_drift_cleanup_cannot_remove_a_newer_independent_refresh() {
    let namespace = namespace("cleanup-cas");
    let inner = Arc::new(InMemoryRepository::new());
    create_node(
        inner.as_ref(),
        &namespace,
        "create-alpha",
        "alpha",
        a3s_memory::repository::MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let post_snapshot_started = Arc::new(Notify::new());
    let release_post_snapshot = Arc::new(Notify::new());
    let repository = Arc::new(PostSnapshotGateRepository {
        inner: inner.clone(),
        snapshot_calls: AtomicUsize::new(0),
        post_snapshot_started: post_snapshot_started.clone(),
        release_post_snapshot: release_post_snapshot.clone(),
    });
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let delayed_session = wrapped_session(repository.clone(), namespace.clone(), index.clone());
    let current_session = wrapped_session(repository, namespace.clone(), index);

    let delayed_task = tokio::spawn(async move {
        delayed_session
            .refresh_semantic_recall_requiring(
                VectorMutationConsistency::IndexRevisionCas,
                CancellationToken::new(),
            )
            .await
    });
    post_snapshot_started.notified().await;
    inner
        .apply(MemoryChangeSet::new(
            "cleanup-cas-revision",
            namespace,
            time(2),
            vec![MemoryOperation::Revise {
                node_id: "alpha".into(),
                expected_revision: 1,
                content: GAMMA.into(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence("cleanup-cas-revision", 2)],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .unwrap();
    let current = current_session
        .refresh_semantic_recall_requiring(
            VectorMutationConsistency::IndexRevisionCas,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    release_post_snapshot.notify_one();

    let error = delayed_task.await.unwrap().unwrap_err();
    assert!(matches!(
        error,
        DurableMemorySemanticError::Vector(VectorIndexError::RevisionConflict {
            expected,
            actual,
        }) if expected == VectorRevision::new(1) && actual == current.index_status().revision
    ));
    assert_eq!(
        current_session.semantic_recall().unwrap().index_status(),
        *current.index_status()
    );
    let preview = current_session.preview_recall(GAMMA_QUERY).await.unwrap();
    assert_eq!(preview.hits.len(), 1);
    assert_eq!(preview.hits[0].node_revision, 2);
}
