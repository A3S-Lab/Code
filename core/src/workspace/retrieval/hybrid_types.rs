use super::{WorkspaceChunk, WorkspaceRetrievalStatus};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

const DEFAULT_RERANK_MAX_CANDIDATES: usize = 100;
const DEFAULT_RERANK_MAX_FEATURE_BYTES: usize = 4 * 1024;
const DEFAULT_RERANK_MAX_FINGERPRINTS: usize = 128;
const DEFAULT_RERANK_MAX_SCRATCH_BYTES: usize = 4 * 1024 * 1024;

/// Independent retrieval evidence channels fused by hybrid search.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRetrievalChannel {
    Exact,
    Lexical,
    Structural,
    Semantic,
}

/// Why one hybrid channel or the final fused result is partial.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceHybridFallbackReason {
    Unavailable,
    Building,
    Degraded,
    QueryEmbeddingFailed,
    VectorSearchFailed,
    StructuralQueryFailed,
    RevisionChanged,
    FilteredStaleHits,
}

/// Per-channel evidence included with every hybrid result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceHybridChannelStatus {
    pub channel: WorkspaceRetrievalChannel,
    pub candidate_count: usize,
    pub truncated: bool,
    pub fallback: Option<WorkspaceHybridFallbackReason>,
}

/// One channel rank that contributed to a fused hit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceHybridChannelRank {
    pub channel: WorkspaceRetrievalChannel,
    /// One-based rank within this channel.
    pub rank: usize,
}

/// Requested or applied second-stage ranking behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRerankMode {
    /// Preserve deterministic RRF ordering and per-file diversity.
    #[default]
    RrfOnly,
    /// Apply the bounded deterministic MMR v1 stage after RRF.
    Deterministic,
}

/// Versioned ranking pipeline actually applied to one hybrid result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WorkspaceRerankAlgorithm {
    #[serde(rename = "rrf_k60")]
    RrfK60,
    #[serde(rename = "rrf_k60+deterministic_mmr_v1")]
    RrfK60DeterministicMmrV1,
}

impl WorkspaceRerankAlgorithm {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RrfK60 => "rrf_k60",
            Self::RrfK60DeterministicMmrV1 => "rrf_k60+deterministic_mmr_v1",
        }
    }
}

/// Explicit bounded configuration for hybrid second-stage ranking.
///
/// RRF-only remains the compatibility default until the locked `WSR-EVAL2`
/// strategy matrix proves that deterministic reranking should be promoted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct WorkspaceRerankOptions {
    pub mode: WorkspaceRerankMode,
    pub max_candidates: usize,
    pub max_feature_bytes_per_candidate: usize,
    pub max_fingerprints_per_candidate: usize,
    pub max_scratch_bytes: usize,
}

impl WorkspaceRerankOptions {
    /// Enable deterministic in-memory MMR v1 with the locked resource bounds.
    pub fn deterministic() -> Self {
        Self {
            mode: WorkspaceRerankMode::Deterministic,
            ..Self::default()
        }
    }

    /// Override the maximum fused candidates evaluated by the second stage.
    pub fn with_max_candidates(mut self, maximum: usize) -> Self {
        self.max_candidates = maximum;
        self
    }

    /// Override sampled source bytes retained per candidate for similarity.
    pub fn with_max_feature_bytes_per_candidate(mut self, maximum: usize) -> Self {
        self.max_feature_bytes_per_candidate = maximum;
        self
    }

    /// Override the maximum bottom-k lexical fingerprints per candidate.
    pub fn with_max_fingerprints_per_candidate(mut self, maximum: usize) -> Self {
        self.max_fingerprints_per_candidate = maximum;
        self
    }

    /// Override the checked transient-memory budget for one rerank operation.
    pub fn with_max_scratch_bytes(mut self, maximum: usize) -> Self {
        self.max_scratch_bytes = maximum;
        self
    }
}

impl Default for WorkspaceRerankOptions {
    fn default() -> Self {
        Self {
            mode: WorkspaceRerankMode::RrfOnly,
            max_candidates: DEFAULT_RERANK_MAX_CANDIDATES,
            max_feature_bytes_per_candidate: DEFAULT_RERANK_MAX_FEATURE_BYTES,
            max_fingerprints_per_candidate: DEFAULT_RERANK_MAX_FINGERPRINTS,
            max_scratch_bytes: DEFAULT_RERANK_MAX_SCRATCH_BYTES,
        }
    }
}

/// Why an explicitly requested reranker preserved the original RRF order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRerankFallbackReason {
    ScratchBudgetExceeded,
    InvalidConfiguration,
}

/// Non-sensitive accounting for one bounded hybrid rerank operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRerankStatus {
    pub requested_mode: WorkspaceRerankMode,
    pub applied_mode: WorkspaceRerankMode,
    pub algorithm: WorkspaceRerankAlgorithm,
    pub input_candidates: usize,
    pub evaluated_candidates: usize,
    pub selected_candidates: usize,
    pub near_duplicate_candidates: usize,
    pub selected_near_duplicates: usize,
    pub feature_bytes: usize,
    pub accounted_scratch_bytes: usize,
    pub candidate_truncated: bool,
    pub fallback: Option<WorkspaceRerankFallbackReason>,
}

/// Bounded request for deterministic hybrid workspace retrieval.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkspaceHybridSearchRequest {
    pub query: String,
    pub path: Option<String>,
    pub include: Option<String>,
    pub limit: usize,
}

impl WorkspaceHybridSearchRequest {
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

impl fmt::Debug for WorkspaceHybridSearchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceHybridSearchRequest")
            .field("query_bytes", &self.query.len())
            .field("has_path_filter", &self.path.is_some())
            .field("has_include_filter", &self.include.is_some())
            .field("limit", &self.limit)
            .finish()
    }
}

/// One current-source-verified fused match.
#[derive(Clone, PartialEq)]
pub struct WorkspaceHybridSearchHit {
    pub chunk: Arc<WorkspaceChunk>,
    pub fused_score: f64,
    /// Greedy selection score assigned by the applied rerank stage.
    pub rerank_score: f64,
    /// Maximum interval or lexical similarity to an earlier selected hit.
    pub redundancy_score: f64,
    pub exact_identifier: bool,
    pub channels: Vec<WorkspaceHybridChannelRank>,
}

impl fmt::Debug for WorkspaceHybridSearchHit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceHybridSearchHit")
            .field("chunk_id", &self.chunk.id)
            .field("start_line", &self.chunk.start_line)
            .field("end_line", &self.chunk.end_line)
            .field("fused_score", &self.fused_score)
            .field("rerank_score", &self.rerank_score)
            .field("redundancy_score", &self.redundancy_score)
            .field("exact_identifier", &self.exact_identifier)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

/// Fused results plus the exact semantic and catalog state used to build them.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceHybridSearchResult {
    pub hits: Vec<WorkspaceHybridSearchHit>,
    pub semantic_status: WorkspaceRetrievalStatus,
    pub catalog_revision: u64,
    pub source_revision: u64,
    pub channels: Vec<WorkspaceHybridChannelStatus>,
    pub rerank: WorkspaceRerankStatus,
    pub truncated: bool,
    pub fallback: Option<WorkspaceHybridFallbackReason>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_debug_redacts_query_and_filters() {
        let request = WorkspaceHybridSearchRequest::new("private-query-sentinel")
            .with_path("private/path")
            .with_include("*.secret");

        let debug = format!("{request:?}");
        assert!(!debug.contains("private-query-sentinel"));
        assert!(!debug.contains("private/path"));
        assert!(!debug.contains("*.secret"));
        assert!(debug.contains("query_bytes"));
    }

    #[test]
    fn public_hybrid_value_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<WorkspaceRetrievalChannel>();
        assert_send_sync::<WorkspaceHybridChannelStatus>();
        assert_send_sync::<WorkspaceHybridSearchRequest>();
        assert_send_sync::<WorkspaceHybridSearchHit>();
        assert_send_sync::<WorkspaceHybridSearchResult>();
        assert_send_sync::<WorkspaceRerankMode>();
        assert_send_sync::<WorkspaceRerankAlgorithm>();
        assert_send_sync::<WorkspaceRerankOptions>();
        assert_send_sync::<WorkspaceRerankStatus>();
    }
}
