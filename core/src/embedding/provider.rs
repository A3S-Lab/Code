use super::{EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingProviderDescriptor};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

/// Host-owned source of embedding vectors.
///
/// Implementations receive only inputs already admitted by the caller. They
/// must honor cancellation and return one vector for each input identifier.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Descriptor captured when a Code embedding generation is constructed.
    fn descriptor(&self) -> EmbeddingProviderDescriptor;

    /// Embed one executor-bounded batch.
    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, super::EmbeddingProviderError>;
}
