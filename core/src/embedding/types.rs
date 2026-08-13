use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Vector normalization promised by an embedding provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingNormalization {
    /// The provider returns finite vectors without a unit-length promise.
    None,
    /// The provider returns unit-L2-normalized vectors.
    Unit,
}

/// Immutable identity and output shape for one embedding generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingProviderDescriptor {
    pub provider: String,
    pub model: String,
    pub revision: Option<String>,
    pub dimension: usize,
    pub normalization: EmbeddingNormalization,
}

impl EmbeddingProviderDescriptor {
    pub fn new(provider: impl Into<String>, model: impl Into<String>, dimension: usize) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            revision: None,
            dimension,
            normalization: EmbeddingNormalization::None,
        }
    }

    pub fn with_revision(mut self, revision: impl Into<String>) -> Self {
        self.revision = Some(revision.into());
        self
    }

    pub fn with_normalization(mut self, normalization: EmbeddingNormalization) -> Self {
        self.normalization = normalization;
        self
    }
}

/// One caller-identified text input.
#[derive(Clone, Eq, PartialEq)]
pub struct EmbeddingInput {
    id: Arc<str>,
    text: Arc<str>,
}

impl EmbeddingInput {
    pub fn new(id: impl Into<Arc<str>>, text: impl Into<Arc<str>>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn text_bytes(&self) -> usize {
        self.text.len()
    }
}

impl fmt::Debug for EmbeddingInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmbeddingInput")
            .field("id_bytes", &self.id.len())
            .field("text_bytes", &self.text.len())
            .finish()
    }
}

/// One bounded request passed to an [`super::EmbeddingProvider`].
#[derive(Clone)]
pub struct EmbeddingBatchRequest {
    inputs: Arc<[EmbeddingInput]>,
    text_bytes: usize,
}

impl EmbeddingBatchRequest {
    pub(crate) fn new(inputs: Vec<EmbeddingInput>) -> Self {
        let text_bytes = inputs.iter().fold(0usize, |total, input| {
            total.saturating_add(input.text_bytes())
        });
        Self {
            inputs: Arc::from(inputs),
            text_bytes,
        }
    }

    pub fn inputs(&self) -> &[EmbeddingInput] {
        &self.inputs
    }

    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }
}

impl fmt::Debug for EmbeddingBatchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmbeddingBatchRequest")
            .field("input_count", &self.inputs.len())
            .field("text_bytes", &self.text_bytes)
            .finish()
    }
}

/// One provider-returned embedding vector.
#[derive(Clone, PartialEq)]
pub struct EmbeddingVector {
    pub id: Arc<str>,
    pub values: Vec<f32>,
}

impl EmbeddingVector {
    pub fn new(id: impl Into<Arc<str>>, values: Vec<f32>) -> Self {
        Self {
            id: id.into(),
            values,
        }
    }
}

impl fmt::Debug for EmbeddingVector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmbeddingVector")
            .field("id_bytes", &self.id.len())
            .field("dimension", &self.values.len())
            .finish()
    }
}

/// Provider response for one batch.
#[derive(Clone)]
pub struct EmbeddingBatchResponse {
    pub descriptor: EmbeddingProviderDescriptor,
    pub vectors: Vec<EmbeddingVector>,
}

impl EmbeddingBatchResponse {
    pub fn new(descriptor: EmbeddingProviderDescriptor, vectors: Vec<EmbeddingVector>) -> Self {
        Self {
            descriptor,
            vectors,
        }
    }
}

impl fmt::Debug for EmbeddingBatchResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmbeddingBatchResponse")
            .field("descriptor", &self.descriptor)
            .field("vector_count", &self.vectors.len())
            .finish()
    }
}

/// Fully validated output. Vectors preserve caller input order.
#[derive(Clone, Debug)]
pub struct EmbeddingExecution {
    pub descriptor: EmbeddingProviderDescriptor,
    pub vectors: Vec<EmbeddingVector>,
    pub batch_count: usize,
    pub provider_attempts: usize,
}

/// Stable category used for retry and degraded-state decisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingFailureKind {
    Cancelled,
    Timeout,
    RateLimited,
    Unavailable,
    Authentication,
    InvalidRequest,
    Other,
}

/// Typed failure returned by a host embedding adapter.
///
/// Provider response bodies are deliberately absent so source text echoed by a
/// remote endpoint cannot enter Code errors or logs through this contract.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum EmbeddingProviderError {
    #[error("embedding request was cancelled")]
    Cancelled,
    #[error("embedding request timed out")]
    Timeout,
    #[error("embedding provider rate limited the request")]
    RateLimited { retry_after: Option<Duration> },
    #[error("embedding provider is temporarily unavailable")]
    Unavailable { retry_after: Option<Duration> },
    #[error("embedding provider rejected authentication")]
    Authentication,
    #[error("embedding provider rejected the request")]
    InvalidRequest,
    #[error("embedding provider failed")]
    Other,
}

impl EmbeddingProviderError {
    pub fn kind(&self) -> EmbeddingFailureKind {
        match self {
            Self::Cancelled => EmbeddingFailureKind::Cancelled,
            Self::Timeout => EmbeddingFailureKind::Timeout,
            Self::RateLimited { .. } => EmbeddingFailureKind::RateLimited,
            Self::Unavailable { .. } => EmbeddingFailureKind::Unavailable,
            Self::Authentication => EmbeddingFailureKind::Authentication,
            Self::InvalidRequest => EmbeddingFailureKind::InvalidRequest,
            Self::Other => EmbeddingFailureKind::Other,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::RateLimited { .. } | Self::Unavailable { .. }
        )
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after } | Self::Unavailable { retry_after } => *retry_after,
            _ => None,
        }
    }
}

/// Validation and execution failures produced by [`super::EmbeddingExecutor`].
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum EmbeddingError {
    #[error("embedding request must contain at least one input")]
    EmptyRequest,
    #[error("invalid embedding configuration for {field}: {reason}")]
    InvalidConfiguration {
        field: &'static str,
        reason: &'static str,
    },
    #[error("invalid embedding provider descriptor field: {field}")]
    InvalidDescriptor { field: &'static str },
    #[error("invalid embedding input at index {index}: {reason}")]
    InvalidInput { index: usize, reason: &'static str },
    #[error("embedding {resource} budget exceeded: requested {requested}, limit {limit}")]
    BudgetExceeded {
        resource: &'static str,
        requested: usize,
        limit: usize,
    },
    #[error("embedding request was cancelled")]
    Cancelled,
    #[error("embedding provider panicked during {operation}")]
    ProviderPanicked { operation: &'static str },
    #[error("embedding provider failed with {kind:?} after {attempts} attempt(s)")]
    ProviderFailure {
        kind: EmbeddingFailureKind,
        attempts: usize,
    },
    #[error("embedding retries exhausted with {kind:?} after {attempts} attempt(s)")]
    RetriesExhausted {
        kind: EmbeddingFailureKind,
        attempts: usize,
    },
    #[error("embedding provider descriptor changed within one generation")]
    DescriptorChanged,
    #[error("embedding response returned {actual} vectors; expected {expected}")]
    OutputCountMismatch { expected: usize, actual: usize },
    #[error("embedding response repeated the vector for input index {input_index}")]
    DuplicateOutput { input_index: usize },
    #[error("embedding response contained an unknown input identifier")]
    UnexpectedOutput,
    #[error(
        "embedding vector at input index {input_index} has dimension {actual}; expected {expected}"
    )]
    DimensionMismatch {
        input_index: usize,
        expected: usize,
        actual: usize,
    },
    #[error(
        "embedding vector at input index {input_index} contains a non-finite value at position {position}"
    )]
    NonFiniteValue { input_index: usize, position: usize },
    #[error("embedding vector at input index {input_index} is not unit normalized")]
    NormalizationMismatch { input_index: usize },
}

pub type EmbeddingResult<T> = Result<T, EmbeddingError>;
