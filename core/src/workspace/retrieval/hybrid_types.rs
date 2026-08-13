use super::{WorkspaceChunk, WorkspaceRetrievalStatus};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

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
    }
}
