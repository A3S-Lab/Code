use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingNormalization,
    EmbeddingProvider, EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::{
    DurableMemoryRecallPolicy, DurableMemorySemanticRecall, DurableMemorySemanticRecallPolicy,
    DurableMemorySession,
};
use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, InMemoryRepository, MemoryChangeSet,
    MemoryNamespace, MemoryNodeDraft, MemoryOperation, MemoryRepository, MemoryStatus,
};
use a3s_memory::vector::VectorIndex;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub const ALPHA: &str = "Rotate the amber signing key before gateway recovery.";
pub const BETA: &str = "Archive the blue deployment ledger after verification.";
pub const GAMMA: &str = "Use the green recovery credential after correction.";
pub const ALPHA_QUERY: &str = "legacy amber gateway procedure";
pub const BETA_QUERY: &str = "Which deployment record should be archived?";
pub const GAMMA_QUERY: &str = "What credential is valid after the correction?";

pub fn time(second: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 30, 1, 0, second)
        .single()
        .unwrap()
}

pub fn namespace(scope: &str) -> MemoryNamespace {
    MemoryNamespace::try_new("tenant", "principal", scope).unwrap()
}

pub fn evidence(name: &str, second: u32) -> EvidenceRef {
    EvidenceRef::try_new(
        format!("a3s://semantic-refresh/{name}"),
        format!("sha256:{name:0>64}"),
        EvidenceKind::Verification,
        time(second),
    )
    .unwrap()
}

pub async fn create_node(
    repository: &InMemoryRepository,
    namespace: &MemoryNamespace,
    key: &str,
    id: &str,
    status: MemoryStatus,
    content: &str,
    second: u32,
) {
    repository
        .apply(MemoryChangeSet::new(
            key,
            namespace.clone(),
            time(second),
            vec![MemoryOperation::Create {
                node: MemoryNodeDraft::new(
                    id,
                    namespace.clone(),
                    DurableMemoryKind::Procedural,
                    status,
                    content,
                    vec![evidence(key, second)],
                    time(second),
                ),
            }],
        ))
        .await
        .unwrap();
}

#[derive(Clone)]
pub struct FixtureProvider;

impl FixtureProvider {
    fn vector(text: &str) -> Option<Vec<f32>> {
        match text {
            ALPHA | ALPHA_QUERY => Some(vec![1.0, 0.0]),
            BETA | BETA_QUERY => Some(vec![0.0, 1.0]),
            GAMMA | GAMMA_QUERY => Some(vec![0.0, 1.0]),
            _ => None,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for FixtureProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new("fixture", "refresh-v1", 2)
            .with_revision("fixture-r1")
            .with_normalization(EmbeddingNormalization::Unit)
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
                Self::vector(input.text())
                    .map(|values| EmbeddingVector::new(input.id(), values))
                    .ok_or(EmbeddingProviderError::InvalidRequest)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

pub fn semantic(
    provider: Arc<dyn EmbeddingProvider>,
    config: EmbeddingExecutorConfig,
    index: Arc<dyn VectorIndex>,
) -> DurableMemorySemanticRecall {
    DurableMemorySemanticRecall::new(
        format!("sha256:{}", "d".repeat(64)),
        provider,
        config,
        index,
        DurableMemorySemanticRecallPolicy::try_new(8, 0.8).unwrap(),
    )
    .unwrap()
}

pub fn session(
    repository: Arc<InMemoryRepository>,
    namespace: MemoryNamespace,
    semantic: DurableMemorySemanticRecall,
) -> DurableMemorySession {
    DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(4, 0.2).unwrap(),
    )
    .with_semantic_recall(semantic)
    .unwrap()
}
