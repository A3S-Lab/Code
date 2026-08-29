use super::fixture::Fixture;
use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingNormalization, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(super) struct FixtureEmbeddingProvider {
    descriptor: EmbeddingProviderDescriptor,
    vectors: Arc<HashMap<String, Vec<f32>>>,
}

impl FixtureEmbeddingProvider {
    pub(super) fn new(fixture: &Fixture) -> Self {
        let mut vectors = HashMap::new();
        for node in &fixture.nodes {
            vectors.insert(node.content.clone(), node.embedding.clone());
        }
        vectors.insert(
            fixture.candidate_node.content.clone(),
            fixture.candidate_node.embedding.clone(),
        );
        vectors.insert(
            fixture.foreign_node.content.clone(),
            fixture.foreign_node.embedding.clone(),
        );
        vectors.insert(
            fixture.stale_node.indexed_content.clone(),
            fixture.stale_node.embedding.clone(),
        );
        for query in &fixture.queries {
            vectors.insert(query.query.clone(), query.embedding.clone());
        }
        for query in &fixture.negative_queries {
            vectors.insert(query.query.clone(), query.embedding.clone());
        }
        Self {
            descriptor: EmbeddingProviderDescriptor::new(
                &fixture.embedding.provider,
                &fixture.embedding.model,
                fixture.embedding.dimension,
            )
            .with_revision(&fixture.embedding.revision)
            .with_normalization(EmbeddingNormalization::Unit),
            vectors: Arc::new(vectors),
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
