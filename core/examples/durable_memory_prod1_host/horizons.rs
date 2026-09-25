//! Long-horizon consolidation, drift, and cache-reuse measurement.
//!
//! Each horizon mutates the source namespace, lets the owned schedule publish a
//! new generation, then proves three things: the receipt's Active count equals
//! the repository projection, every Active node is present in the published
//! partition, and no record outside this namespace's partition was written.

use super::corpus;
use super::session::{wait_for_published_run, wait_for_unchanged_run, CANDIDATE_LIMIT};
use a3s_code_core::memory::{
    ScheduledSemanticRefresh, SemanticRefreshRunMetrics, SemanticRefreshRunOutcome,
};
use a3s_code_core::{DurableMemoryRecallChannel, DurableMemorySession};
use a3s_memory::repository::MemoryRepository;
use a3s_memory::vector::{VectorIndex, VectorSearchRequest};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::Arc;

const LABEL_NODE_ID: &str = "a3s.memory.semantic.node_id";
const LABEL_SCHEMA: &str = "a3s.memory.semantic.schema";
const RECORD_SCHEMA_V1: &str = "a3s.code.memory.semantic-record.v1";

/// One published (or unchanged) horizon plus its invariant checks.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HorizonRecord {
    pub label: &'static str,
    pub sequence: u64,
    pub outcome: SemanticRefreshRunOutcome,
    pub elapsed_ms: u64,
    pub repository_active_nodes: usize,
    pub receipt_active_nodes: usize,
    pub index_record_count: usize,
    pub index_revision: u64,
    pub published_node_count: usize,
    /// Every repository Active node appears exactly once in the partition.
    pub active_projection_exact: bool,
    /// No published record belongs to a partition outside this binding.
    pub namespace_scoped: bool,
    pub embedding_cache_hits: u64,
    pub provider_requests: u64,
    pub provider_inputs: u64,
    pub publication_attempts: u64,
    pub publication_records: u64,
    pub target_recall_rank: Option<usize>,
}

impl HorizonRecord {
    pub fn passed(&self) -> bool {
        self.active_projection_exact
            && self.namespace_scoped
            && self.repository_active_nodes == self.receipt_active_nodes
            && self.index_record_count == self.repository_active_nodes
            // The paraphrase must still reach the target inside the bounded
            // candidate set. Demanding rank 0 would grade the embedding model
            // rather than the durable-memory machinery, so the observed rank is
            // recorded as evidence instead of gated on.
            && self.target_recall_rank.is_some()
    }
}

/// Inputs that stay constant across every horizon.
pub struct HorizonContext<'a> {
    pub session: &'a DurableMemorySession,
    pub schedule: &'a ScheduledSemanticRefresh,
    pub repository: Arc<dyn MemoryRepository>,
    pub index: Arc<dyn VectorIndex>,
    pub query_vector: Vec<f32>,
    pub target_node_id: String,
    pub node_limit: usize,
}

/// Observe the horizon that published the mutation applied after `baseline`.
pub async fn observe_publication(
    context: &HorizonContext<'_>,
    label: &'static str,
    baseline: u64,
) -> Result<HorizonRecord> {
    let run = wait_for_published_run(context.schedule, baseline).await?;
    settle(context, label, run).await
}

/// Observe a horizon in which nothing changed, proving the no-op fast path.
pub async fn observe_quiescent(
    context: &HorizonContext<'_>,
    label: &'static str,
    baseline: u64,
) -> Result<HorizonRecord> {
    let run = wait_for_unchanged_run(context.schedule, baseline).await?;
    settle(context, label, run).await
}

/// Observe a horizon whose run is already settled (for example the first one).
pub async fn settle(
    context: &HorizonContext<'_>,
    label: &'static str,
    run: SemanticRefreshRunMetrics,
) -> Result<HorizonRecord> {
    let receipt = context
        .schedule
        .last_receipt()
        .context("horizon produced no retained refresh receipt")?;
    let observation = context.index.observe().await?;
    let active_ids = corpus::active_node_ids(
        context.repository.as_ref(),
        context.session.namespace(),
        context.node_limit,
    )
    .await?;
    let expected: BTreeSet<String> = active_ids.iter().cloned().collect();

    // Read the whole published partition back through the index contract so the
    // check depends on stored records, not on in-process bookkeeping.
    let request = VectorSearchRequest::new(
        context.query_vector.clone(),
        observation.status.record_count.max(1),
    )
    .with_label(LABEL_SCHEMA, RECORD_SCHEMA_V1);
    let published = context.index.search(request).await?;
    let mut published_nodes = BTreeSet::new();
    let mut partitions = BTreeSet::new();
    for hit in &published.hits {
        partitions.insert(hit.partition.clone());
        if let Some(node_id) = hit.labels.get(LABEL_NODE_ID) {
            published_nodes.insert(node_id.clone());
        }
    }

    let preview = context.session.preview_recall(corpus::TARGET_QUERY).await?;
    let target_recall_rank = preview
        .hits
        .iter()
        .position(|hit| {
            hit.node_id == context.target_node_id
                && hit.channel == DurableMemoryRecallChannel::Semantic
        })
        .filter(|_| preview.hits.len() <= CANDIDATE_LIMIT);

    Ok(HorizonRecord {
        label,
        sequence: run.sequence(),
        outcome: run.outcome(),
        elapsed_ms: run.elapsed_ms(),
        repository_active_nodes: active_ids.len(),
        receipt_active_nodes: receipt.active_node_count(),
        index_record_count: observation.status.record_count,
        index_revision: observation.status.revision.value(),
        published_node_count: published_nodes.len(),
        active_projection_exact: published_nodes == expected
            && published.hits.len() == active_ids.len(),
        namespace_scoped: observation.status.partition_count == 1 && partitions.len() <= 1,
        embedding_cache_hits: run.embedding_cache_hits(),
        provider_requests: run.provider_requests(),
        provider_inputs: run.provider_inputs(),
        publication_attempts: run.publication_attempts(),
        publication_records: run.publication_records(),
        target_recall_rank,
    })
}

/// Activate `count` candidate nodes with independent decision evidence.
pub async fn activate_candidates(
    session: &DurableMemorySession,
    candidate_ids: &[String],
    count: usize,
    horizon: i64,
) -> Result<Vec<String>> {
    let mut activated = Vec::new();
    for node_id in candidate_ids.iter().take(count) {
        let node = session
            .repository()
            .get(session.namespace(), node_id)
            .await?
            .with_context(|| format!("candidate {node_id} is absent"))?;
        session
            .activate_candidate(a3s_code_core::DurableMemoryActivation::try_new(
                format!("dm-prod1-activate-{node_id}"),
                node_id.clone(),
                node.revision,
                corpus::activation_evidence(node_id, horizon)?,
                corpus::activation_timestamp(horizon),
            )?)
            .await?;
        activated.push(node_id.clone());
    }
    Ok(activated)
}
