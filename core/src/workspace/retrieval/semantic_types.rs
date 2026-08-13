use crate::embedding::{
    EmbeddingError, EmbeddingExecutorConfig, EmbeddingProvider, EmbeddingProviderDescriptor,
};
use a3s_memory::vector::VectorIndexError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_VECTOR_MAX_RECORDS: usize = 100_000;
const DEFAULT_VECTOR_MAX_BYTES: usize = 128 * 1024 * 1024;
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Resource limits for one session-owned semantic workspace index.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceSemanticIndexLimits {
    /// Maximum vectors retained across all ready file partitions.
    pub max_records: usize,
    /// Maximum bytes accounted by A3S Memory for vectors and record metadata.
    pub max_bytes: usize,
    /// Maximum cooperative wait when a session closes the indexing task.
    pub shutdown_timeout: Duration,
}

impl Default for WorkspaceSemanticIndexLimits {
    fn default() -> Self {
        Self {
            max_records: DEFAULT_VECTOR_MAX_RECORDS,
            max_bytes: DEFAULT_VECTOR_MAX_BYTES,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

impl WorkspaceSemanticIndexLimits {
    pub(crate) fn validate(self) -> WorkspaceRetrievalResult<Self> {
        if self.max_records == 0 {
            return Err(WorkspaceRetrievalError::InvalidConfiguration {
                field: "max_records",
                reason: "must be greater than zero",
            });
        }
        if self.max_bytes == 0 {
            return Err(WorkspaceRetrievalError::InvalidConfiguration {
                field: "max_bytes",
                reason: "must be greater than zero",
            });
        }
        if self.shutdown_timeout.is_zero() || self.shutdown_timeout > MAX_SHUTDOWN_TIMEOUT {
            return Err(WorkspaceRetrievalError::InvalidConfiguration {
                field: "shutdown_timeout",
                reason: "must be between one nanosecond and thirty seconds",
            });
        }
        Ok(self)
    }
}

/// Typed, host-owned semantic retrieval configuration for one session.
///
/// A provider object is injected directly. The vector implementation remains
/// an internal detail, so callers cannot select a backend by string.
#[derive(Clone)]
pub struct WorkspaceRetrievalOptions {
    pub(crate) provider: Arc<dyn EmbeddingProvider>,
    pub(crate) embedding: EmbeddingExecutorConfig,
    pub(crate) index_limits: WorkspaceSemanticIndexLimits,
}

impl WorkspaceRetrievalOptions {
    /// Enable semantic indexing with a host-supplied embedding provider.
    pub fn new(provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            provider,
            embedding: EmbeddingExecutorConfig::default(),
            index_limits: WorkspaceSemanticIndexLimits::default(),
        }
    }

    /// Override bounded provider execution and retry behavior.
    pub fn with_embedding_config(mut self, config: EmbeddingExecutorConfig) -> Self {
        self.embedding = config;
        self
    }

    /// Override vector memory, record, and close-time limits.
    pub fn with_index_limits(mut self, limits: WorkspaceSemanticIndexLimits) -> Self {
        self.index_limits = limits;
        self
    }
}

impl fmt::Debug for WorkspaceRetrievalOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceRetrievalOptions")
            .field("provider", &"<host-injected>")
            .field("embedding", &self.embedding)
            .field("index_limits", &self.index_limits)
            .finish()
    }
}

/// Lifecycle state of a session's semantic workspace index.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRetrievalPhase {
    Disabled,
    Building,
    Ready,
    Degraded,
    Closed,
}

/// Non-sensitive, immutable observation of one semantic index revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRetrievalStatus {
    pub phase: WorkspaceRetrievalPhase,
    pub catalog_revision: u64,
    pub source_revision: u64,
    pub vector_revision: u64,
    pub eligible_files: usize,
    pub catalog_files: usize,
    pub catalog_chunks: usize,
    pub indexed_files: usize,
    pub indexed_chunks: usize,
    /// Integer coverage in basis points (`10_000 == 100%`).
    pub coverage_bps: u16,
    pub queue_depth: usize,
    pub failed_files: usize,
    pub total_failures: u64,
    pub vector_records: usize,
    pub vector_bytes: usize,
    pub model: Option<EmbeddingProviderDescriptor>,
}

impl WorkspaceRetrievalStatus {
    pub fn disabled() -> Self {
        Self {
            phase: WorkspaceRetrievalPhase::Disabled,
            catalog_revision: 0,
            source_revision: 0,
            vector_revision: 0,
            eligible_files: 0,
            catalog_files: 0,
            catalog_chunks: 0,
            indexed_files: 0,
            indexed_chunks: 0,
            coverage_bps: 0,
            queue_depth: 0,
            failed_files: 0,
            total_failures: 0,
            vector_records: 0,
            vector_bytes: 0,
            model: None,
        }
    }

    pub(crate) fn building(model: EmbeddingProviderDescriptor) -> Self {
        Self {
            phase: WorkspaceRetrievalPhase::Building,
            model: Some(model),
            ..Self::disabled()
        }
    }
}

/// Construction failures for a session semantic retrieval runtime.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WorkspaceRetrievalError {
    #[error("invalid workspace retrieval configuration for {field}: {reason}")]
    InvalidConfiguration {
        field: &'static str,
        reason: &'static str,
    },
    #[error(transparent)]
    Embedding(#[from] EmbeddingError),
    #[error(transparent)]
    VectorIndex(#[from] VectorIndexError),
}

pub type WorkspaceRetrievalResult<T> = Result<T, WorkspaceRetrievalError>;
