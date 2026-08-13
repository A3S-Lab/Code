//! Host-injected embedding provider contract and bounded batch execution.
//!
//! This module does not own workspace traversal, vector storage, or session
//! lifecycle. It validates provider output before a later retrieval runtime can
//! publish vectors into a session-owned index.

mod executor;
mod provider;
mod types;

pub use executor::{EmbeddingExecutor, EmbeddingExecutorConfig};
pub use provider::EmbeddingProvider;
pub use types::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingError, EmbeddingExecution,
    EmbeddingFailureKind, EmbeddingInput, EmbeddingNormalization, EmbeddingProviderDescriptor,
    EmbeddingProviderError, EmbeddingResult, EmbeddingVector,
};

#[cfg(test)]
mod tests;
