use std::sync::Arc;

/// Stable identifier for one deterministic workspace chunk.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceChunkId(Arc<str>);

impl WorkspaceChunkId {
    pub(crate) fn new(value: String) -> Self {
        Self(Arc::from(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One bounded source chunk admitted to workspace retrieval.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkspaceChunk {
    pub id: WorkspaceChunkId,
    pub path: Arc<str>,
    pub language: Option<Arc<str>>,
    /// One-based inclusive line number.
    pub start_line: usize,
    /// One-based inclusive line number.
    pub end_line: usize,
    /// Zero-based inclusive byte offset in the source file.
    pub start_byte: usize,
    /// Zero-based exclusive byte offset in the source file.
    pub end_byte: usize,
    pub content_digest: Arc<str>,
    pub source_revision: u64,
    pub text: Arc<str>,
}

impl std::fmt::Debug for WorkspaceChunk {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceChunk")
            .field("id", &self.id)
            .field("language", &self.language)
            .field("start_line", &self.start_line)
            .field("end_line", &self.end_line)
            .field("start_byte", &self.start_byte)
            .field("end_byte", &self.end_byte)
            .field("source_revision", &self.source_revision)
            .field("text_bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// Deterministic chunking limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkingConfig {
    pub max_lines: usize,
    pub max_bytes: usize,
    pub max_chunks_per_file: usize,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            max_lines: 80,
            max_bytes: 64 * 1024,
            max_chunks_per_file: 128,
        }
    }
}

impl ChunkingConfig {
    pub(crate) fn validate(self) -> Result<Self, WorkspaceIndexError> {
        if self.max_lines == 0 {
            return Err(WorkspaceIndexError::InvalidConfig(
                "max_lines must be greater than zero".to_owned(),
            ));
        }
        if self.max_bytes < 4 {
            return Err(WorkspaceIndexError::InvalidConfig(
                "max_bytes must be at least four to admit any UTF-8 scalar".to_owned(),
            ));
        }
        if self.max_chunks_per_file == 0 {
            return Err(WorkspaceIndexError::InvalidConfig(
                "max_chunks_per_file must be greater than zero".to_owned(),
            ));
        }
        Ok(self)
    }
}

/// Hard catalog bounds checked before publishing a replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkCatalogLimits {
    pub max_files: usize,
    pub max_chunks: usize,
    /// Maximum retained source-text bytes.
    pub max_text_bytes: usize,
    /// Maximum conservatively estimated bytes for chunks, document statistics,
    /// term maps, and postings. Allocator bookkeeping may add a small overhead.
    pub max_index_bytes: usize,
}

impl Default for ChunkCatalogLimits {
    fn default() -> Self {
        Self {
            max_files: 50_000,
            max_chunks: 100_000,
            max_text_bytes: 64 * 1024 * 1024,
            max_index_bytes: 64 * 1024 * 1024,
        }
    }
}

impl ChunkCatalogLimits {
    pub(crate) fn validate(self) -> Result<Self, WorkspaceIndexError> {
        if self.max_files == 0
            || self.max_chunks == 0
            || self.max_text_bytes == 0
            || self.max_index_bytes == 0
        {
            return Err(WorkspaceIndexError::InvalidConfig(
                "catalog limits must all be greater than zero".to_owned(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WorkspaceIndexError {
    #[error("invalid workspace retrieval configuration: {0}")]
    InvalidConfig(String),
    #[error("workspace file '{path}' produced more than {limit} chunks")]
    TooManyChunks { path: String, limit: usize },
    #[error("custom chunking strategy failed for workspace file '{path}'")]
    ChunkingStrategyFailed { path: String },
    #[error("chunking strategy returned invalid ranges for workspace file '{path}': {reason}")]
    InvalidChunkRanges { path: String, reason: &'static str },
    #[error(
        "workspace retrieval {resource} budget exceeded: requested {requested}, limit {limit}"
    )]
    BudgetExceeded {
        resource: &'static str,
        requested: usize,
        limit: usize,
    },
    #[error("workspace retrieval lock was poisoned")]
    LockPoisoned,
    #[error("stale workspace revision {requested}; current revision is {current}")]
    StaleRevision { requested: u64, current: u64 },
    #[error(
        "workspace catalog changed concurrently: expected revision {expected}, found {actual}"
    )]
    ConcurrentUpdate { expected: u64, actual: u64 },
    #[error("workspace retrieval query is invalid: {0}")]
    InvalidQuery(String),
    #[error("failed to read workspace path '{path}': {message}")]
    ReadFailed { path: String, message: String },
}

pub(crate) type WorkspaceIndexResult<T> = Result<T, WorkspaceIndexError>;
