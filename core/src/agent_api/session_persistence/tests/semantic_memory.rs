use super::*;
use crate::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingNormalization,
    EmbeddingProvider, EmbeddingProviderDescriptor, EmbeddingProviderError,
};
use crate::error::CodeError;
use a3s_memory::repository::{InMemoryRepository, MemoryNamespace};
use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct DescriptorOnlyProvider {
    model: &'static str,
}

#[async_trait::async_trait]
impl EmbeddingProvider for DescriptorOnlyProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new("fixture", self.model, 2)
            .with_revision("fixture-r1")
            .with_normalization(EmbeddingNormalization::Unit)
    }

    async fn embed(
        &self,
        _request: EmbeddingBatchRequest,
        _cancellation: CancellationToken,
    ) -> std::result::Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        Err(EmbeddingProviderError::InvalidRequest)
    }
}

#[derive(Clone, Copy)]
struct SemanticFixture {
    authority_marker: char,
    model: &'static str,
    max_records: usize,
    min_score: f32,
    max_retries: u32,
}

impl Default for SemanticFixture {
    fn default() -> Self {
        Self {
            authority_marker: 'a',
            model: "semantic-resume-v1",
            max_records: 128,
            min_score: 0.7,
            max_retries: 2,
        }
    }
}

fn semantic_session(fixture: SemanticFixture) -> crate::durable_memory::DurableMemorySession {
    let namespace = MemoryNamespace::try_new("tenant", "principal", "semantic-resume").unwrap();
    let repository = Arc::new(InMemoryRepository::new());
    let descriptor = VectorIndexDescriptor::new(2).with_max_records(fixture.max_records);
    let index: Arc<dyn VectorIndex> = Arc::new(InMemoryVectorIndex::new(descriptor).unwrap());
    let execution = EmbeddingExecutorConfig {
        max_retries: fixture.max_retries,
        ..EmbeddingExecutorConfig::default()
    };
    let semantic = crate::durable_memory::DurableMemorySemanticRecall::new(
        format!("sha256:{}", fixture.authority_marker.to_string().repeat(64)),
        Arc::new(DescriptorOnlyProvider {
            model: fixture.model,
        }),
        execution,
        index,
        crate::durable_memory::DurableMemorySemanticRecallPolicy::try_new(8, fixture.min_score)
            .unwrap(),
    )
    .unwrap();
    crate::durable_memory::DurableMemorySession::active_recall(
        repository,
        namespace,
        crate::durable_memory::DurableMemoryRecallPolicy::try_new(4, 0.2).unwrap(),
    )
    .with_semantic_recall(semantic)
    .unwrap()
}

#[test]
fn persisted_semantic_memory_requires_the_exact_serving_generation() {
    let fixture = SemanticFixture::default();
    let exact = semantic_session(fixture);
    let mut data = persisted_data(Some("openai/gpt-4o"), None);
    data.durable_memory_binding = Some(exact.binding());

    let missing = apply_persisted_runtime_options(SessionOptions::new(), &data).unwrap_err();
    assert!(matches!(
        missing,
        CodeError::SessionConfiguration {
            field: "durable_memory",
            ..
        }
    ));

    for drifted in [
        semantic_session(SemanticFixture {
            model: "semantic-resume-v2",
            ..fixture
        }),
        semantic_session(SemanticFixture {
            authority_marker: 'b',
            ..fixture
        }),
        semantic_session(SemanticFixture {
            max_records: 64,
            ..fixture
        }),
        semantic_session(SemanticFixture {
            min_score: 0.8,
            ..fixture
        }),
        semantic_session(SemanticFixture {
            max_retries: 1,
            ..fixture
        }),
    ] {
        let error = apply_persisted_runtime_options(
            SessionOptions::new().with_durable_memory(drifted),
            &data,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            CodeError::SessionConfiguration {
                field: "durable_memory",
                ..
            }
        ));
    }

    let restored = apply_persisted_runtime_options(
        SessionOptions::new().with_durable_memory(exact.clone()),
        &data,
    )
    .unwrap();
    assert_eq!(
        restored
            .durable_memory
            .as_ref()
            .map(crate::durable_memory::DurableMemorySession::binding),
        Some(exact.binding())
    );
}
