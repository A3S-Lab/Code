use a3s_memory::repository::{DurableMemoryKind, MemoryStatus};
use serde::Deserialize;

pub(super) const FIXTURE: &str =
    include_str!("../fixtures/durable-memory-semantic-v1/evaluation.json");

#[derive(Debug, Deserialize)]
pub(super) struct Fixture {
    pub(super) schema_version: u32,
    pub(super) binding_schema_version: u32,
    pub(super) semantic_binding_schema: String,
    pub(super) fusion_profile: String,
    pub(super) embedding: EmbeddingFixture,
    pub(super) recall_policy: RecallPolicy,
    pub(super) semantic_policy: SemanticPolicy,
    pub(super) nodes: Vec<Node>,
    pub(super) candidate_node: Node,
    pub(super) foreign_node: Node,
    pub(super) stale_node: StaleNode,
    pub(super) queries: Vec<Query>,
    pub(super) negative_queries: Vec<NegativeQuery>,
    pub(super) thresholds: Thresholds,
}

#[derive(Debug, Deserialize)]
pub(super) struct EmbeddingFixture {
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) revision: String,
    pub(super) dimension: usize,
    pub(super) authority_digest: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct RecallPolicy {
    pub(super) max_results: usize,
    pub(super) min_lexical_score: f32,
    pub(super) max_related_lookups: usize,
}

#[derive(Debug, Deserialize)]
pub(super) struct SemanticPolicy {
    pub(super) candidate_limit: usize,
    pub(super) min_score: f32,
}

#[derive(Debug, Deserialize)]
pub(super) struct Node {
    pub(super) id: String,
    pub(super) kind: DurableMemoryKind,
    pub(super) status: MemoryStatus,
    pub(super) content: String,
    pub(super) embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct StaleNode {
    pub(super) id: String,
    pub(super) kind: DurableMemoryKind,
    pub(super) indexed_content: String,
    pub(super) current_content: String,
    pub(super) embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Query {
    pub(super) id: String,
    #[serde(rename = "language")]
    pub(super) _language: String,
    pub(super) query: String,
    pub(super) relevant_node_id: String,
    pub(super) embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct NegativeQuery {
    pub(super) id: String,
    pub(super) query: String,
    pub(super) embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Thresholds {
    pub(super) minimum_semantic_recall_at_1: f64,
    pub(super) maximum_lexical_positive_hits: usize,
    pub(super) maximum_negative_hits: usize,
    pub(super) maximum_context_nodes_per_query: usize,
    pub(super) maximum_model_calls_per_query: usize,
    pub(super) expected_admissions: u64,
}
