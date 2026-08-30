#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::{DurableMemorySemanticError, DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1};
use a3s_memory::repository::{
    InMemoryRepository, MemoryChangeSet, MemoryNamespace, MemoryOperation, MemoryRepository,
    MemoryStatus, RevisionMode, MEMORY_NAMESPACE_SNAPSHOT_PROFILE_V1,
};
use a3s_memory::vector::{
    InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorIndexError,
    VectorMutationConsistency, VectorRevision,
};
use async_trait::async_trait;
use refresh_support::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn repository_refresh_publishes_one_complete_active_snapshot_and_receipt() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<a3s_code_core::DurableMemorySemanticRefreshReceipt>();

    let namespace = namespace("complete");
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
        "create-candidate",
        "candidate",
        MemoryStatus::Candidate,
        BETA,
        2,
    )
    .await;
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let session = session(
        repository,
        namespace,
        semantic(
            Arc::new(FixtureProvider),
            EmbeddingExecutorConfig::default(),
            index,
        ),
    );

    let receipt = session
        .refresh_semantic_recall_requiring(
            VectorMutationConsistency::IndexRevisionCas,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(
        receipt.profile(),
        DURABLE_MEMORY_SEMANTIC_REFRESH_PROFILE_V1
    );
    assert_eq!(
        receipt.source_snapshot_profile(),
        MEMORY_NAMESPACE_SNAPSHOT_PROFILE_V1
    );
    assert!(receipt.source_snapshot_digest().starts_with("sha256:"));
    assert!(receipt.source_snapshot_bytes() > 0);
    assert!(receipt.serving_generation_digest().starts_with("sha256:"));
    assert_eq!(receipt.active_node_count(), 1);
    assert_eq!(
        receipt.mutation_consistency(),
        VectorMutationConsistency::IndexRevisionCas
    );
    assert_eq!(receipt.index_status().record_count, 1);
    let preview = session.preview_recall(ALPHA_QUERY).await.unwrap();
    assert_eq!(preview.hits.len(), 1);
    assert_eq!(preview.hits[0].node_id, "alpha");
}

#[tokio::test]
async fn repeated_refresh_replaces_revised_and_inactive_nodes() {
    let namespace = namespace("lifecycle");
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
        2,
    )
    .await;
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let session = session(
        repository.clone(),
        namespace.clone(),
        semantic(
            Arc::new(FixtureProvider),
            EmbeddingExecutorConfig::default(),
            index,
        ),
    );
    let first = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first.active_node_count(), 2);

    repository
        .apply(MemoryChangeSet::new(
            "refresh-lifecycle",
            namespace,
            time(3),
            vec![
                MemoryOperation::Revise {
                    node_id: "alpha".into(),
                    expected_revision: 1,
                    content: GAMMA.into(),
                    mode: RevisionMode::Correction,
                    evidence: vec![evidence("correct-alpha", 3)],
                    confidence: None,
                    importance: None,
                },
                MemoryOperation::SetStatus {
                    node_id: "beta".into(),
                    expected_revision: 1,
                    status: MemoryStatus::Tombstoned,
                },
            ],
        ))
        .await
        .unwrap();
    let second = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(second.active_node_count(), 1);
    assert_eq!(second.index_status().record_count, 1);
    assert!(second.index_status().revision > first.index_status().revision);
    let stale = session.preview_recall(ALPHA_QUERY).await.unwrap();
    assert!(
        stale.hits.is_empty(),
        "unexpected stale hits: {:?}",
        stale.hits
    );
    let current = session.preview_recall(GAMMA_QUERY).await.unwrap();
    assert_eq!(current.hits.len(), 1);
    assert_eq!(current.hits[0].node_id, "alpha");
    assert_eq!(current.hits[0].node_revision, 2);
}

struct DriftingProvider {
    repository: Arc<InMemoryRepository>,
    namespace: MemoryNamespace,
    changed: AtomicBool,
}

#[async_trait]
impl EmbeddingProvider for DriftingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        FixtureProvider.descriptor()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        _cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if !self.changed.swap(true, Ordering::SeqCst) {
            self.repository
                .apply(MemoryChangeSet::new(
                    "concurrent-revision",
                    self.namespace.clone(),
                    time(2),
                    vec![MemoryOperation::Revise {
                        node_id: "alpha".into(),
                        expected_revision: 1,
                        content: GAMMA.into(),
                        mode: RevisionMode::Correction,
                        evidence: vec![evidence("concurrent-revision", 2)],
                        confidence: None,
                        importance: None,
                    }],
                ))
                .await
                .unwrap();
        }
        Ok(EmbeddingBatchResponse::new(
            self.descriptor(),
            request
                .inputs()
                .iter()
                .map(|input| EmbeddingVector::new(input.id(), vec![1.0, 0.0]))
                .collect(),
        ))
    }
}

#[tokio::test]
async fn repository_drift_during_refresh_invalidates_the_published_partition() {
    let namespace = namespace("drift");
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
    let session = session(
        repository.clone(),
        namespace.clone(),
        semantic(
            Arc::new(DriftingProvider {
                repository,
                namespace,
                changed: AtomicBool::new(false),
            }),
            EmbeddingExecutorConfig::default(),
            index,
        ),
    );

    let error = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap_err();

    assert_eq!(
        error,
        DurableMemorySemanticError::RepositoryChangedDuringRefresh
    );
    let status = session.semantic_recall().unwrap().index_status();
    assert_eq!(status.partition_count, 0);
    assert_eq!(status.record_count, 0);
}

struct GatedProvider {
    first_started: Arc<Notify>,
    release_first: Arc<Notify>,
    calls: AtomicUsize,
}

#[async_trait]
impl EmbeddingProvider for GatedProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        FixtureProvider.descriptor()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        _cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.first_started.notify_one();
            self.release_first.notified().await;
        }
        let vectors = request
            .inputs()
            .iter()
            .map(|input| {
                let values = match input.text() {
                    ALPHA | ALPHA_QUERY => vec![1.0, 0.0],
                    GAMMA | GAMMA_QUERY => vec![0.0, 1.0],
                    _ => return Err(EmbeddingProviderError::InvalidRequest),
                };
                Ok(EmbeddingVector::new(input.id(), values))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

#[tokio::test]
async fn cloned_sessions_serialize_refresh_publication_and_drift_cleanup() {
    let namespace = namespace("concurrent-refresh");
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
    let first_started = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());
    let provider = Arc::new(GatedProvider {
        first_started: first_started.clone(),
        release_first: release_first.clone(),
        calls: AtomicUsize::new(0),
    });
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let session = session(
        repository.clone(),
        namespace.clone(),
        semantic(provider, EmbeddingExecutorConfig::default(), index),
    );

    let first_session = session.clone();
    let first = tokio::spawn(async move {
        first_session
            .refresh_semantic_recall(CancellationToken::new())
            .await
    });
    first_started.notified().await;
    repository
        .apply(MemoryChangeSet::new(
            "concurrent-refresh-revision",
            namespace,
            time(2),
            vec![MemoryOperation::Revise {
                node_id: "alpha".into(),
                expected_revision: 1,
                content: GAMMA.into(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence("concurrent-refresh-revision", 2)],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .unwrap();

    let second_completed = Arc::new(Notify::new());
    let second_completed_task = second_completed.clone();
    let second_session = session.clone();
    let second = tokio::spawn(async move {
        let result = second_session
            .refresh_semantic_recall(CancellationToken::new())
            .await;
        second_completed_task.notify_one();
        result
    });
    let completed_before_release = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        second_completed.notified(),
    )
    .await
    .is_ok();
    release_first.notify_one();

    assert_eq!(
        first.await.unwrap().unwrap_err(),
        DurableMemorySemanticError::RepositoryChangedDuringRefresh
    );
    let second = second.await.unwrap().unwrap();
    assert!(
        !completed_before_release,
        "a cloned session published a second refresh while the first still owned cleanup"
    );
    assert_eq!(second.active_node_count(), 1);
    assert_eq!(second.index_status().record_count, 1);
    let current = session.preview_recall(GAMMA_QUERY).await.unwrap();
    assert_eq!(current.hits.len(), 1);
    assert_eq!(current.hits[0].node_revision, 2);
}

#[tokio::test]
async fn independent_sessions_cannot_overwrite_or_clean_up_a_newer_refresh() {
    let namespace = namespace("independent-refresh");
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
    let first_started = Arc::new(Notify::new());
    let release_first = Arc::new(Notify::new());
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let delayed_session = session(
        repository.clone(),
        namespace.clone(),
        semantic(
            Arc::new(GatedProvider {
                first_started: first_started.clone(),
                release_first: release_first.clone(),
                calls: AtomicUsize::new(0),
            }),
            EmbeddingExecutorConfig::default(),
            index.clone(),
        ),
    );
    let current_session = session(
        repository.clone(),
        namespace.clone(),
        semantic(
            Arc::new(FixtureProvider),
            EmbeddingExecutorConfig::default(),
            index,
        ),
    );

    let delayed_task = tokio::spawn(async move {
        delayed_session
            .refresh_semantic_recall_requiring(
                VectorMutationConsistency::IndexRevisionCas,
                CancellationToken::new(),
            )
            .await
    });
    first_started.notified().await;
    repository
        .apply(MemoryChangeSet::new(
            "independent-refresh-revision",
            namespace,
            time(2),
            vec![MemoryOperation::Revise {
                node_id: "alpha".into(),
                expected_revision: 1,
                content: GAMMA.into(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence("independent-refresh-revision", 2)],
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
    release_first.notify_one();

    let error = delayed_task.await.unwrap().unwrap_err();
    assert!(matches!(
        error,
        DurableMemorySemanticError::Vector(VectorIndexError::RevisionConflict {
            expected,
            actual,
        }) if expected == VectorRevision::new(0) && actual == current.index_status().revision
    ));
    assert_eq!(
        current_session.semantic_recall().unwrap().index_status(),
        *current.index_status()
    );
    let preview = current_session.preview_recall(GAMMA_QUERY).await.unwrap();
    assert_eq!(preview.hits.len(), 1);
    assert_eq!(preview.hits[0].node_revision, 2);
}
