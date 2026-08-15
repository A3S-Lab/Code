use crate::embedding::{
    EmbeddingError, EmbeddingExecutorConfig, EmbeddingProvider, EmbeddingProviderDescriptor,
};
use crate::workspace::{
    ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkingStrategy, WorkspaceRerankOptions,
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
const MAX_SEMANTIC_READINESS_TIMEOUT: Duration = Duration::from_secs(30);

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
    pub(crate) chunking_strategy: Option<WorkspaceChunkingStrategy>,
    pub(crate) chunking: Option<ChunkingConfig>,
    pub(crate) catalog_limits: Option<ChunkCatalogLimits>,
    pub(crate) rerank: WorkspaceRerankOptions,
    pub(crate) semantic_readiness_timeout: Duration,
}

impl WorkspaceRetrievalOptions {
    /// Enable semantic indexing with a host-supplied embedding provider.
    pub fn new(provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            provider,
            embedding: EmbeddingExecutorConfig::default(),
            index_limits: WorkspaceSemanticIndexLimits::default(),
            chunking_strategy: None,
            chunking: None,
            catalog_limits: None,
            rerank: WorkspaceRerankOptions::default(),
            semantic_readiness_timeout: Duration::ZERO,
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

    /// Select the deterministic text splitter used by the session-owned
    /// workspace catalog.
    ///
    /// This option configures the local catalog A3S Code creates for the
    /// session. A host that supplies [`crate::workspace::WorkspaceServices`]
    /// owns that catalog and must configure its strategy when constructing it.
    pub fn with_chunking_strategy(mut self, strategy: WorkspaceChunkingStrategy) -> Self {
        self.chunking_strategy = Some(strategy);
        self
    }

    /// Override hard per-file chunk size and count limits.
    pub fn with_chunking_config(mut self, config: ChunkingConfig) -> Self {
        self.chunking = Some(config);
        self
    }

    /// Override hard memory bounds for the session-owned text catalog.
    pub fn with_catalog_limits(mut self, limits: ChunkCatalogLimits) -> Self {
        self.catalog_limits = Some(limits);
        self
    }

    /// Select bounded second-stage behavior for hybrid workspace search.
    pub fn with_rerank_options(mut self, options: WorkspaceRerankOptions) -> Self {
        self.rerank = options;
        self
    }

    /// Wait up to this bound when a semantic query arrives while the current
    /// catalog generation is still building.
    ///
    /// The default is zero, which preserves immediate partial fallback. A
    /// host with a slower local provider can opt into a bounded readiness
    /// barrier without making session construction synchronous.
    pub fn with_semantic_readiness_timeout(mut self, timeout: Duration) -> Self {
        self.semantic_readiness_timeout = timeout;
        self
    }

    pub(crate) fn validate_semantic_readiness_timeout(&self) -> WorkspaceRetrievalResult<()> {
        if self.semantic_readiness_timeout > MAX_SEMANTIC_READINESS_TIMEOUT {
            return Err(WorkspaceRetrievalError::InvalidConfiguration {
                field: "semantic_readiness_timeout",
                reason: "must not exceed thirty seconds",
            });
        }
        Ok(())
    }

    pub(crate) fn has_catalog_configuration(&self) -> bool {
        self.chunking_strategy.is_some() || self.chunking.is_some() || self.catalog_limits.is_some()
    }
}

impl fmt::Debug for WorkspaceRetrievalOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceRetrievalOptions")
            .field("provider", &"<host-injected>")
            .field("embedding", &self.embedding)
            .field("index_limits", &self.index_limits)
            .field("chunking_strategy", &self.chunking_strategy)
            .field("chunking", &self.chunking)
            .field("catalog_limits", &self.catalog_limits)
            .field("rerank", &self.rerank)
            .field(
                "semantic_readiness_timeout",
                &self.semantic_readiness_timeout,
            )
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

/// Machine-readable batching evidence for the current catalog generation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEmbeddingBatchMetrics {
    /// Text chunks admitted to this semantic projection generation.
    pub document_inputs: usize,
    /// UTF-8 bytes submitted by those document inputs, including overlap.
    pub document_text_bytes: usize,
    /// Logical batches submitted by the session-local coordinator.
    pub document_batches: usize,
    /// Physical provider calls, including retries, observed by the executor.
    pub document_provider_requests: usize,
    /// The theoretical request lower bound imposed by count, text, and vector limits.
    pub batch_limit_lower_bound: usize,
    pub input_limit_flushes: usize,
    pub text_byte_limit_flushes: usize,
    pub vector_byte_limit_flushes: usize,
    /// Underfilled batches flushed immediately because the catalog generation was complete.
    pub generation_complete_flushes: usize,
    /// Elapsed time from observing the generation to its first file-atomic publication.
    pub time_to_first_ready_ms: Option<u64>,
    /// Must remain zero: non-text assets never enter the text chunk catalog.
    pub non_text_inputs: usize,
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
    #[serde(default)]
    pub batching: WorkspaceEmbeddingBatchMetrics,
    pub model: Option<EmbeddingProviderDescriptor>,
}

/// Bounded request for semantic workspace retrieval.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkspaceSemanticSearchRequest {
    pub query: String,
    pub path: Option<String>,
    pub include: Option<String>,
    pub limit: usize,
}

impl fmt::Debug for WorkspaceSemanticSearchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceSemanticSearchRequest")
            .field("query_bytes", &self.query.len())
            .field("has_path_filter", &self.path.is_some())
            .field("has_include_filter", &self.include.is_some())
            .field("limit", &self.limit)
            .finish()
    }
}

impl WorkspaceSemanticSearchRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            path: None,
            include: None,
            limit: 10,
        }
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_include(mut self, include: impl Into<String>) -> Self {
        self.include = Some(include.into());
        self
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// One digest-verified semantic match from an immutable catalog revision.
#[derive(Clone, PartialEq)]
pub struct WorkspaceSemanticSearchHit {
    pub chunk: Arc<super::WorkspaceChunk>,
    pub score: f32,
}

impl fmt::Debug for WorkspaceSemanticSearchHit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceSemanticSearchHit")
            .field("chunk_id", &self.chunk.id)
            .field("start_line", &self.chunk.start_line)
            .field("end_line", &self.chunk.end_line)
            .field("source_revision", &self.chunk.source_revision)
            .field("score", &self.score)
            .finish_non_exhaustive()
    }
}

/// Explicit reason a semantic query could not run or returned partial data.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSemanticFallbackReason {
    Building,
    Degraded,
    Closed,
    QueryEmbeddingFailed,
    VectorSearchFailed,
    RevisionChanged,
    FilteredStaleHits,
}

/// Structured semantic results and the exact status that produced them.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceSemanticSearchResult {
    pub hits: Vec<WorkspaceSemanticSearchHit>,
    pub status: WorkspaceRetrievalStatus,
    pub searched_records: usize,
    pub truncated: bool,
    pub fallback: Option<WorkspaceSemanticFallbackReason>,
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
            batching: WorkspaceEmbeddingBatchMetrics::default(),
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
    #[error("invalid workspace semantic query: {0}")]
    InvalidQuery(String),
    #[error("workspace semantic retrieval is not enabled for this session")]
    Unavailable,
    #[error("workspace semantic query was cancelled")]
    Cancelled,
    #[error(transparent)]
    Embedding(#[from] EmbeddingError),
    #[error(transparent)]
    VectorIndex(#[from] VectorIndexError),
    #[error(transparent)]
    WorkspaceIndex(#[from] super::WorkspaceIndexError),
}

pub type WorkspaceRetrievalResult<T> = Result<T, WorkspaceRetrievalError>;
