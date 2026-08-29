#[path = "durable_memory_semantic_refresh/support.rs"]
mod refresh_support;

use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingNormalization,
    EmbeddingProvider, EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::{DurableMemoryRecallPolicy, DurableMemorySemanticError, DurableMemorySession};
use a3s_memory::repository::{
    InMemoryRepository, MemoryAccessEvent, MemoryChangeResult, MemoryChangeSet, MemoryNamespace,
    MemoryNamespaceSnapshot, MemoryNode, MemoryOperation, MemoryQuery, MemoryQueryResult,
    MemoryRepository, MemoryRepositoryError, MemorySnapshotRequest, MemoryStatus,
    MemoryUsageSummary, RevisionMode,
};
use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor};
use async_trait::async_trait;
use refresh_support::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn over_node_budget_snapshot_preserves_the_previous_partition() {
    let namespace = namespace("overflow");
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
    let config = EmbeddingExecutorConfig {
        max_batch_inputs: 1,
        max_request_inputs: 1,
        ..EmbeddingExecutorConfig::default()
    };
    let session = session(
        repository.clone(),
        namespace.clone(),
        semantic(Arc::new(FixtureProvider), config, index),
    );
    let first = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap();
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

    let error = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        DurableMemorySemanticError::Repository(MemoryRepositoryError::LimitExceeded {
            resource,
            limit: 1,
            actual: 2,
        }) if resource == "namespace snapshot nodes"
    ));
    assert_eq!(
        session.semantic_recall().unwrap().index_status(),
        *first.index_status()
    );
}

#[tokio::test]
async fn over_byte_budget_snapshot_preserves_the_previous_partition() {
    let namespace = namespace("byte-overflow");
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
    let config = EmbeddingExecutorConfig {
        max_input_text_bytes: 1_024,
        max_batch_text_bytes: 1_024,
        max_request_text_bytes: 2_048,
        ..EmbeddingExecutorConfig::default()
    };
    let session = session(
        repository.clone(),
        namespace.clone(),
        semantic(Arc::new(FixtureProvider), config, index),
    );
    let first = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap();
    repository
        .apply(MemoryChangeSet::new(
            "revise-over-byte-budget",
            namespace,
            time(2),
            vec![MemoryOperation::Revise {
                node_id: "alpha".into(),
                expected_revision: 1,
                content: "x".repeat(4_096),
                mode: RevisionMode::Correction,
                evidence: vec![evidence("revise-over-byte-budget", 2)],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .unwrap();

    let error = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        DurableMemorySemanticError::Repository(MemoryRepositoryError::LimitExceeded {
            resource,
            limit: 2_048,
            actual,
        }) if resource == "namespace snapshot bytes" && actual > 2_048
    ));
    assert_eq!(
        session.semantic_recall().unwrap().index_status(),
        *first.index_status()
    );
}

struct FailAfterFirstProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl EmbeddingProvider for FailAfterFirstProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new("fixture", "refresh-failure-v1", 2)
            .with_revision("fixture-r1")
            .with_normalization(EmbeddingNormalization::Unit)
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        _cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
            return Err(EmbeddingProviderError::InvalidRequest);
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
async fn embedding_failure_preserves_the_previous_complete_partition() {
    let namespace = namespace("provider-failure");
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
            Arc::new(FailAfterFirstProvider {
                calls: AtomicUsize::new(0),
            }),
            EmbeddingExecutorConfig::default(),
            index,
        ),
    );
    let first = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap();
    repository
        .apply(MemoryChangeSet::new(
            "revise-before-provider-failure",
            namespace,
            time(2),
            vec![MemoryOperation::Revise {
                node_id: "alpha".into(),
                expected_revision: 1,
                content: GAMMA.into(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence("revise-before-provider-failure", 2)],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .unwrap();

    let error = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap_err();

    assert!(matches!(error, DurableMemorySemanticError::Embedding(_)));
    assert_eq!(
        session.semantic_recall().unwrap().index_status(),
        *first.index_status()
    );
}

struct TamperingRepository {
    inner: Arc<InMemoryRepository>,
    tamper: AtomicBool,
}

#[async_trait]
impl MemoryRepository for TamperingRepository {
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
        let snapshot = self.inner.snapshot_namespace(request).await?;
        if !self.tamper.load(Ordering::SeqCst) {
            return Ok(snapshot);
        }
        let mut encoded = serde_json::to_value(snapshot).unwrap();
        encoded["digest"] = serde_json::json!(format!("sha256:{}", "e".repeat(64)));
        Ok(serde_json::from_value(encoded).unwrap())
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

#[tokio::test]
async fn forged_snapshot_identity_is_rejected_before_partition_mutation() {
    let namespace = namespace("tampered-snapshot");
    let inner = Arc::new(InMemoryRepository::new());
    create_node(
        inner.as_ref(),
        &namespace,
        "create-alpha",
        "alpha",
        MemoryStatus::Active,
        ALPHA,
        1,
    )
    .await;
    let repository = Arc::new(TamperingRepository {
        inner,
        tamper: AtomicBool::new(false),
    });
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let session = DurableMemorySession::active_recall(
        repository.clone(),
        namespace,
        DurableMemoryRecallPolicy::try_new(4, 0.2).unwrap(),
    )
    .with_semantic_recall(semantic(
        Arc::new(FixtureProvider),
        EmbeddingExecutorConfig::default(),
        index,
    ))
    .unwrap();
    let first = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap();
    repository.tamper.store(true, Ordering::SeqCst);

    let error = session
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        DurableMemorySemanticError::Repository(MemoryRepositoryError::InvariantViolation { .. })
    ));
    assert_eq!(
        session.semantic_recall().unwrap().index_status(),
        *first.index_status()
    );
}
