use super::invalid;
use a3s_memory::repository::{DurableMemoryKind, MemoryRepositoryError, MAX_QUERY_LIMIT};
use serde::{Deserialize, Serialize};

/// Runtime behavior enabled for one durable-memory binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DurableMemoryMode {
    /// Mirror successful V1 extractions as evidence-backed V2 candidates.
    ShadowCandidates,
    /// Mirror candidates and recall only explicitly activated V2 nodes.
    ActiveRecall,
}

/// Bounded policy for opt-in active V2 recall.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DurableMemoryRecallPolicy {
    max_results: usize,
    min_lexical_score: f32,
    max_related_lookups: usize,
}

impl DurableMemoryRecallPolicy {
    pub fn try_new(
        max_results: usize,
        min_lexical_score: f32,
    ) -> Result<Self, MemoryRepositoryError> {
        if !(1..=MAX_QUERY_LIMIT).contains(&max_results) {
            return Err(invalid(
                "recallPolicy.maxResults",
                format!("must be between 1 and {MAX_QUERY_LIMIT}"),
            ));
        }
        if !min_lexical_score.is_finite() || !(0.0..=1.0).contains(&min_lexical_score) {
            return Err(invalid(
                "recallPolicy.minLexicalScore",
                "must be finite and between 0 and 1",
            ));
        }
        Ok(Self {
            max_results,
            min_lexical_score,
            max_related_lookups: 0,
        })
    }

    /// Enable a bounded number of exact `RelatedTo` target reads after lexical
    /// seeding. Final results remain capped by `max_results`.
    pub fn try_with_related_lookups(
        mut self,
        max_related_lookups: usize,
    ) -> Result<Self, MemoryRepositoryError> {
        if max_related_lookups > MAX_QUERY_LIMIT {
            return Err(invalid(
                "recallPolicy.maxRelatedLookups",
                format!("must not exceed {MAX_QUERY_LIMIT}"),
            ));
        }
        self.max_related_lookups = max_related_lookups;
        Ok(self)
    }

    pub fn max_results(self) -> usize {
        self.max_results
    }

    pub fn min_lexical_score(self) -> f32 {
        self.min_lexical_score
    }

    pub fn max_related_lookups(self) -> usize {
        self.max_related_lookups
    }
}

/// Retrieval branch that produced one pure recall preview hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DurableMemoryRecallChannel {
    Lexical,
    Semantic,
    Hybrid,
    Related,
}

/// One active V2 hit returned by a pure diagnostic recall preview.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DurableMemoryRecallHit {
    pub node_id: String,
    pub node_revision: u64,
    pub kind: DurableMemoryKind,
    pub content: String,
    pub score: f32,
    pub channel: DurableMemoryRecallChannel,
    pub related_from: Option<String>,
}

/// Pure, bounded active-memory recall result. Previewing does not record an
/// admission or use event and therefore cannot authorize prompt injection.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DurableMemoryRecallPreview {
    pub hits: Vec<DurableMemoryRecallHit>,
}
