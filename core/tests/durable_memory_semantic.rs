use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingNormalization,
    EmbeddingProvider, EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::{
    DurableMemoryRecallChannel, DurableMemoryRecallPolicy, DurableMemorySemanticRecall,
    DurableMemorySemanticRecallPolicy, DurableMemorySession,
    DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION,
};
use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, InMemoryRepository, MemoryChangeSet,
    MemoryNamespace, MemoryNodeDraft, MemoryOperation, MemoryRepository, MemoryStatus,
    RevisionMode,
};
use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const CONTENT: &str = "Rotate the amber signing key before restoring the gateway.";
const QUERY: &str = "如何恢复网关凭证？";

#[derive(Clone)]
struct FixtureEmbeddingProvider {
    descriptor: EmbeddingProviderDescriptor,
    vectors: Arc<HashMap<String, Vec<f32>>>,
}

impl FixtureEmbeddingProvider {
    fn new() -> Self {
        Self {
            descriptor: EmbeddingProviderDescriptor::new("fixture", "cross-language-v1", 2)
                .with_revision("fixture-r1")
                .with_normalization(EmbeddingNormalization::Unit),
            vectors: Arc::new(HashMap::from([
                (CONTENT.to_string(), vec![1.0, 0.0]),
                (QUERY.to_string(), vec![1.0, 0.0]),
            ])),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for FixtureEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if cancellation.is_cancelled() {
            return Err(EmbeddingProviderError::Cancelled);
        }
        let vectors = request
            .inputs()
            .iter()
            .map(|input| {
                self.vectors
                    .get(input.text())
                    .cloned()
                    .map(|values| EmbeddingVector::new(input.id(), values))
                    .ok_or(EmbeddingProviderError::InvalidRequest)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EmbeddingBatchResponse::new(
            self.descriptor.clone(),
            vectors,
        ))
    }
}

fn time(second: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 30, 0, 0, second)
        .single()
        .unwrap()
}

fn evidence(uri: &str) -> EvidenceRef {
    EvidenceRef::try_new(
        uri,
        format!("sha256:{}", "a".repeat(64)),
        EvidenceKind::Verification,
        time(1),
    )
    .unwrap()
}

async fn seed_node(
    repository: &InMemoryRepository,
    namespace: &MemoryNamespace,
    key: &str,
    node_id: &str,
    status: MemoryStatus,
    content: &str,
) {
    repository
        .apply(MemoryChangeSet::new(
            key,
            namespace.clone(),
            time(1),
            vec![MemoryOperation::Create {
                node: MemoryNodeDraft::new(
                    node_id,
                    namespace.clone(),
                    DurableMemoryKind::Procedural,
                    status,
                    content,
                    vec![evidence(&format!("a3s://semantic/{key}"))],
                    time(1),
                ),
            }],
        ))
        .await
        .unwrap();
}

fn semantic_recall(index: Arc<dyn VectorIndex>) -> DurableMemorySemanticRecall {
    semantic_recall_with_authority(index, 'b')
}

fn semantic_recall_with_authority(
    index: Arc<dyn VectorIndex>,
    authority_marker: char,
) -> DurableMemorySemanticRecall {
    DurableMemorySemanticRecall::new(
        format!("sha256:{}", authority_marker.to_string().repeat(64)),
        Arc::new(FixtureEmbeddingProvider::new()),
        EmbeddingExecutorConfig::default(),
        index,
        DurableMemorySemanticRecallPolicy::try_new(8, 0.8).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn typed_semantic_recall_crosses_languages_without_weakening_repository_authority() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DurableMemorySemanticRecall>();

    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let repository = Arc::new(InMemoryRepository::new());
    repository
        .apply(MemoryChangeSet::new(
            "seed-semantic",
            namespace.clone(),
            time(1),
            vec![
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "active",
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Active,
                        CONTENT,
                        vec![evidence("a3s://semantic/active")],
                        time(1),
                    ),
                },
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "candidate",
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Candidate,
                        "Unverified candidate content",
                        vec![evidence("a3s://semantic/candidate")],
                        time(1),
                    ),
                },
            ],
        ))
        .await
        .unwrap();

    let recall_policy = DurableMemoryRecallPolicy::try_new(3, 0.2).unwrap();
    let lexical_only =
        DurableMemorySession::active_recall(repository.clone(), namespace.clone(), recall_policy);
    assert!(lexical_only
        .preview_recall(QUERY)
        .await
        .unwrap()
        .hits
        .is_empty());

    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let semantic = semantic_recall(index);
    let active = repository.get(&namespace, "active").await.unwrap().unwrap();
    semantic
        .replace_namespace(&namespace, vec![active], CancellationToken::new())
        .await
        .unwrap();
    let candidate = repository
        .get(&namespace, "candidate")
        .await
        .unwrap()
        .unwrap();
    assert!(semantic
        .replace_namespace(&namespace, vec![candidate], CancellationToken::new())
        .await
        .is_err());

    let durable = lexical_only.with_semantic_recall(semantic).unwrap();
    let preview = durable.preview_recall(QUERY).await.unwrap();
    assert_eq!(preview.hits.len(), 1);
    assert_eq!(preview.hits[0].node_id, "active");
    assert_eq!(
        preview.hits[0].channel,
        DurableMemoryRecallChannel::Semantic
    );
    assert_eq!(
        durable.binding().schema_version(),
        DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION
    );
    assert!(durable.binding().semantic_recall().is_some());
}

#[tokio::test]
async fn hybrid_fusion_deduplicates_one_current_node() {
    let namespace = MemoryNamespace::try_new("tenant", "principal", "hybrid").unwrap();
    let repository = Arc::new(InMemoryRepository::new());
    seed_node(
        repository.as_ref(),
        &namespace,
        "seed-hybrid",
        "active",
        MemoryStatus::Active,
        CONTENT,
    )
    .await;
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let semantic = semantic_recall(index);
    semantic
        .replace_namespace(
            &namespace,
            vec![repository.get(&namespace, "active").await.unwrap().unwrap()],
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let session = DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(3, 0.2).unwrap(),
    )
    .with_semantic_recall(semantic)
    .unwrap();
    let preview = session.preview_recall(CONTENT).await.unwrap();

    assert_eq!(preview.hits.len(), 1);
    assert_eq!(preview.hits[0].node_id, "active");
    assert_eq!(preview.hits[0].channel, DurableMemoryRecallChannel::Hybrid);
}

#[tokio::test]
async fn semantic_failure_preserves_the_exact_lexical_result() {
    let namespace = MemoryNamespace::try_new("tenant", "principal", "fallback").unwrap();
    let repository = Arc::new(InMemoryRepository::new());
    seed_node(
        repository.as_ref(),
        &namespace,
        "seed-fallback",
        "active",
        MemoryStatus::Active,
        CONTENT,
    )
    .await;
    let lexical = DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(3, 0.2).unwrap(),
    );
    let query = "amber signing key";
    let expected = lexical.preview_recall(query).await.unwrap();
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let hybrid = lexical
        .with_semantic_recall(semantic_recall(index))
        .unwrap();

    assert_eq!(hybrid.preview_recall(query).await.unwrap(), expected);
}

#[tokio::test]
async fn stale_semantic_revision_is_reverified_and_dropped() {
    let namespace = MemoryNamespace::try_new("tenant", "principal", "stale").unwrap();
    let repository = Arc::new(InMemoryRepository::new());
    seed_node(
        repository.as_ref(),
        &namespace,
        "seed-stale",
        "active",
        MemoryStatus::Active,
        CONTENT,
    )
    .await;
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let semantic = semantic_recall(index);
    semantic
        .replace_namespace(
            &namespace,
            vec![repository.get(&namespace, "active").await.unwrap().unwrap()],
            CancellationToken::new(),
        )
        .await
        .unwrap();
    repository
        .apply(MemoryChangeSet::new(
            "revise-stale",
            namespace.clone(),
            time(2),
            vec![MemoryOperation::Revise {
                node_id: "active".to_string(),
                expected_revision: 1,
                content: "Use the blue recovery credential instead.".to_string(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence("a3s://semantic/revise-stale")],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .unwrap();
    let session = DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(3, 0.2).unwrap(),
    )
    .with_semantic_recall(semantic)
    .unwrap();

    assert!(session.preview_recall(QUERY).await.unwrap().hits.is_empty());
}

#[tokio::test]
async fn shared_index_partitions_the_same_namespace_by_exact_semantic_generation() {
    let namespace = MemoryNamespace::try_new("tenant", "principal", "shared-index").unwrap();
    let repository = Arc::new(InMemoryRepository::new());
    seed_node(
        repository.as_ref(),
        &namespace,
        "seed-shared-generation",
        "active",
        MemoryStatus::Active,
        CONTENT,
    )
    .await;
    let node = repository.get(&namespace, "active").await.unwrap().unwrap();
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let first = semantic_recall_with_authority(index.clone(), 'b');
    let second = semantic_recall_with_authority(index, 'c');

    first
        .replace_namespace(&namespace, vec![node.clone()], CancellationToken::new())
        .await
        .unwrap();
    second
        .replace_namespace(&namespace, vec![node], CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(first.index_status().partition_count, 2);
}

#[tokio::test]
async fn foreign_partition_and_local_candidate_never_cross_active_authority() {
    let namespace = MemoryNamespace::try_new("tenant", "principal", "local").unwrap();
    let foreign = MemoryNamespace::try_new("tenant", "principal", "foreign").unwrap();
    let source = Arc::new(InMemoryRepository::new());
    let target = Arc::new(InMemoryRepository::new());
    seed_node(
        source.as_ref(),
        &namespace,
        "seed-source-active",
        "shared",
        MemoryStatus::Active,
        CONTENT,
    )
    .await;
    seed_node(
        target.as_ref(),
        &namespace,
        "seed-target-candidate",
        "shared",
        MemoryStatus::Candidate,
        CONTENT,
    )
    .await;
    seed_node(
        source.as_ref(),
        &foreign,
        "seed-foreign-active",
        "foreign",
        MemoryStatus::Active,
        CONTENT,
    )
    .await;
    let index: Arc<dyn VectorIndex> =
        Arc::new(InMemoryVectorIndex::new(VectorIndexDescriptor::new(2)).unwrap());
    let semantic = semantic_recall(index);
    semantic
        .replace_namespace(
            &namespace,
            vec![source.get(&namespace, "shared").await.unwrap().unwrap()],
            CancellationToken::new(),
        )
        .await
        .unwrap();
    semantic
        .replace_namespace(
            &foreign,
            vec![source.get(&foreign, "foreign").await.unwrap().unwrap()],
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let session = DurableMemorySession::active_recall(
        target,
        namespace,
        DurableMemoryRecallPolicy::try_new(3, 0.2).unwrap(),
    )
    .with_semantic_recall(semantic)
    .unwrap();

    assert!(session.preview_recall(QUERY).await.unwrap().hits.is_empty());
}
