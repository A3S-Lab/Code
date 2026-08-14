//! Report types and aggregate metrics for the opt-in real-model evaluation.

use super::{EvaluationVariant, RunMetric, TASKS};
use a3s_code_core::embedding::EmbeddingExecutorConfig;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EvaluationSummary {
    pub(super) enabled_task_accuracy: f64,
    pub(super) disabled_task_accuracy: f64,
    pub(super) enabled_tool_protocol_rate: f64,
    pub(super) disabled_tool_protocol_rate: f64,
    pub(super) semantic_recall_at_5: f64,
    pub(super) semantic_mrr: f64,
    pub(super) chunks_per_text_file: f64,
    pub(super) document_requests_per_chunk: f64,
    pub(super) document_request_amplification_vs_input_limit: f64,
    pub(super) non_text_provider_inputs: usize,
    pub(super) enabled_session_construction_p50_ms: u64,
    pub(super) enabled_session_construction_p95_ms: u64,
    pub(super) disabled_session_construction_p50_ms: u64,
    pub(super) disabled_session_construction_p95_ms: u64,
    pub(super) enabled_turn_p50_ms: u64,
    pub(super) enabled_turn_p95_ms: u64,
    pub(super) disabled_turn_p50_ms: u64,
    pub(super) disabled_turn_p95_ms: u64,
    pub(super) index_ready_p50_ms: u64,
    pub(super) index_ready_p95_ms: u64,
    pub(super) enabled_close_p50_ms: u64,
    pub(super) enabled_close_p95_ms: u64,
    pub(super) disabled_close_p50_ms: u64,
    pub(super) disabled_close_p95_ms: u64,
    pub(super) enabled_total_tokens: usize,
    pub(super) disabled_total_tokens: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EvaluationReport {
    pub(super) schema_version: u32,
    pub(super) chat_model: String,
    pub(super) embedding_provider: &'static str,
    pub(super) task_count: usize,
    pub(super) text_file_count: usize,
    pub(super) non_text_file_count: usize,
    pub(super) expected_chunk_count: usize,
    pub(super) summary: EvaluationSummary,
    pub(super) runs: Vec<RunMetric>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RerankEvaluationSummary {
    pub(super) rrf_task_accuracy: f64,
    pub(super) deterministic_task_accuracy: f64,
    pub(super) rrf_tool_protocol_rate: f64,
    pub(super) deterministic_tool_protocol_rate: f64,
    pub(super) rrf_recall_at_5: f64,
    pub(super) deterministic_recall_at_5: f64,
    pub(super) rrf_mrr: f64,
    pub(super) deterministic_mrr: f64,
    pub(super) rrf_collision_result_rate: f64,
    pub(super) deterministic_collision_result_rate: f64,
    pub(super) rrf_turn_p95_ms: u64,
    pub(super) deterministic_turn_p95_ms: u64,
    pub(super) rrf_total_tokens: usize,
    pub(super) deterministic_total_tokens: usize,
    pub(super) rrf_max_vector_bytes: usize,
    pub(super) deterministic_max_vector_bytes: usize,
    pub(super) rrf_document_request_amplification: f64,
    pub(super) deterministic_document_request_amplification: f64,
    pub(super) deterministic_input_candidates: usize,
    pub(super) deterministic_evaluated_candidates: usize,
    pub(super) deterministic_selected_candidates: usize,
    pub(super) deterministic_near_duplicate_candidates: usize,
    pub(super) deterministic_selected_near_duplicates: usize,
    pub(super) deterministic_candidate_near_duplicate_rate: f64,
    pub(super) deterministic_selected_near_duplicate_rate: f64,
    pub(super) deterministic_max_feature_bytes: usize,
    pub(super) deterministic_max_scratch_bytes: usize,
    pub(super) non_text_provider_inputs: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RerankEvaluationReport {
    pub(super) schema_version: u32,
    pub(super) chat_model: String,
    pub(super) embedding_provider: &'static str,
    pub(super) task_count: usize,
    pub(super) collision_copies_per_task: usize,
    pub(super) summary: RerankEvaluationSummary,
    pub(super) runs: Vec<RunMetric>,
}

pub(super) fn summarize(runs: &[RunMetric]) -> EvaluationSummary {
    let enabled = runs
        .iter()
        .filter(|run| run.retrieval_enabled)
        .collect::<Vec<_>>();
    let disabled = runs
        .iter()
        .filter(|run| !run.retrieval_enabled)
        .collect::<Vec<_>>();
    let enabled_ranks = enabled
        .iter()
        .filter_map(|run| run.expected_path_rank)
        .collect::<Vec<_>>();
    let indexed_files = enabled.iter().map(|run| run.indexed_files).sum::<usize>();
    let indexed_chunks = enabled.iter().map(|run| run.indexed_chunks).sum::<usize>();
    let document_requests = enabled
        .iter()
        .map(|run| run.document_embedding_requests)
        .sum::<usize>();
    let document_inputs = enabled
        .iter()
        .map(|run| run.embedded_documents)
        .sum::<usize>();
    let max_batch_inputs = EmbeddingExecutorConfig::default().max_batch_inputs;
    let input_limit_request_lower_bound = enabled
        .iter()
        .map(|run| run.embedded_documents.div_ceil(max_batch_inputs))
        .sum::<usize>();
    EvaluationSummary {
        enabled_task_accuracy: rate(enabled.iter().filter(|run| run.completion_correct).count()),
        disabled_task_accuracy: rate(disabled.iter().filter(|run| run.completion_correct).count()),
        enabled_tool_protocol_rate: rate(enabled.iter().filter(|run| run.tool_protocol_ok).count()),
        disabled_tool_protocol_rate: rate(
            disabled.iter().filter(|run| run.tool_protocol_ok).count(),
        ),
        semantic_recall_at_5: rate(enabled_ranks.iter().filter(|rank| **rank <= 5).count()),
        semantic_mrr: enabled_ranks
            .iter()
            .map(|rank| 1.0 / *rank as f64)
            .sum::<f64>()
            / TASKS.len() as f64,
        chunks_per_text_file: ratio(indexed_chunks, indexed_files),
        document_requests_per_chunk: ratio(document_requests, document_inputs),
        document_request_amplification_vs_input_limit: ratio(
            document_requests,
            input_limit_request_lower_bound,
        ),
        non_text_provider_inputs: enabled.iter().map(|run| run.non_text_provider_inputs).sum(),
        enabled_session_construction_p50_ms: percentile(
            enabled
                .iter()
                .map(|run| run.session_construction_ms)
                .collect(),
            0.50,
        ),
        enabled_session_construction_p95_ms: percentile(
            enabled
                .iter()
                .map(|run| run.session_construction_ms)
                .collect(),
            0.95,
        ),
        disabled_session_construction_p50_ms: percentile(
            disabled
                .iter()
                .map(|run| run.session_construction_ms)
                .collect(),
            0.50,
        ),
        disabled_session_construction_p95_ms: percentile(
            disabled
                .iter()
                .map(|run| run.session_construction_ms)
                .collect(),
            0.95,
        ),
        enabled_turn_p50_ms: percentile(
            enabled.iter().map(|run| run.turn_elapsed_ms).collect(),
            0.50,
        ),
        enabled_turn_p95_ms: percentile(
            enabled.iter().map(|run| run.turn_elapsed_ms).collect(),
            0.95,
        ),
        disabled_turn_p50_ms: percentile(
            disabled.iter().map(|run| run.turn_elapsed_ms).collect(),
            0.50,
        ),
        disabled_turn_p95_ms: percentile(
            disabled.iter().map(|run| run.turn_elapsed_ms).collect(),
            0.95,
        ),
        index_ready_p50_ms: percentile(
            enabled.iter().map(|run| run.index_ready_ms).collect(),
            0.50,
        ),
        index_ready_p95_ms: percentile(
            enabled.iter().map(|run| run.index_ready_ms).collect(),
            0.95,
        ),
        enabled_close_p50_ms: percentile(enabled.iter().map(|run| run.close_ms).collect(), 0.50),
        enabled_close_p95_ms: percentile(enabled.iter().map(|run| run.close_ms).collect(), 0.95),
        disabled_close_p50_ms: percentile(disabled.iter().map(|run| run.close_ms).collect(), 0.50),
        disabled_close_p95_ms: percentile(disabled.iter().map(|run| run.close_ms).collect(), 0.95),
        enabled_total_tokens: enabled.iter().map(|run| run.total_tokens).sum(),
        disabled_total_tokens: disabled.iter().map(|run| run.total_tokens).sum(),
    }
}

pub(super) fn summarize_rerank(runs: &[RunMetric]) -> RerankEvaluationSummary {
    let rrf = variant_runs(runs, EvaluationVariant::HybridRrf);
    let deterministic = variant_runs(runs, EvaluationVariant::HybridDeterministic);
    let deterministic_input_candidates =
        optional_sum(&deterministic, |run| run.rerank_input_candidates);
    let deterministic_evaluated_candidates =
        optional_sum(&deterministic, |run| run.rerank_evaluated_candidates);
    let deterministic_selected_candidates =
        optional_sum(&deterministic, |run| run.rerank_selected_candidates);
    let deterministic_near_duplicate_candidates =
        optional_sum(&deterministic, |run| run.near_duplicate_candidates);
    let deterministic_selected_near_duplicates =
        optional_sum(&deterministic, |run| run.selected_near_duplicates);
    RerankEvaluationSummary {
        rrf_task_accuracy: task_rate(&rrf, |run| run.completion_correct),
        deterministic_task_accuracy: task_rate(&deterministic, |run| run.completion_correct),
        rrf_tool_protocol_rate: task_rate(&rrf, |run| run.tool_protocol_ok),
        deterministic_tool_protocol_rate: task_rate(&deterministic, |run| run.tool_protocol_ok),
        rrf_recall_at_5: recall_at_5(&rrf),
        deterministic_recall_at_5: recall_at_5(&deterministic),
        rrf_mrr: mean_reciprocal_rank(&rrf),
        deterministic_mrr: mean_reciprocal_rank(&deterministic),
        rrf_collision_result_rate: collision_result_rate(&rrf),
        deterministic_collision_result_rate: collision_result_rate(&deterministic),
        rrf_turn_p95_ms: percentile(rrf.iter().map(|run| run.turn_elapsed_ms).collect(), 0.95),
        deterministic_turn_p95_ms: percentile(
            deterministic
                .iter()
                .map(|run| run.turn_elapsed_ms)
                .collect(),
            0.95,
        ),
        rrf_total_tokens: rrf.iter().map(|run| run.total_tokens).sum(),
        deterministic_total_tokens: deterministic.iter().map(|run| run.total_tokens).sum(),
        rrf_max_vector_bytes: rrf.iter().map(|run| run.vector_bytes).max().unwrap_or(0),
        deterministic_max_vector_bytes: deterministic
            .iter()
            .map(|run| run.vector_bytes)
            .max()
            .unwrap_or(0),
        rrf_document_request_amplification: request_amplification(&rrf),
        deterministic_document_request_amplification: request_amplification(&deterministic),
        deterministic_input_candidates,
        deterministic_evaluated_candidates,
        deterministic_selected_candidates,
        deterministic_near_duplicate_candidates,
        deterministic_selected_near_duplicates,
        deterministic_candidate_near_duplicate_rate: ratio(
            deterministic_near_duplicate_candidates,
            deterministic_evaluated_candidates,
        ),
        deterministic_selected_near_duplicate_rate: ratio(
            deterministic_selected_near_duplicates,
            deterministic_selected_candidates,
        ),
        deterministic_max_feature_bytes: deterministic
            .iter()
            .filter_map(|run| run.rerank_feature_bytes)
            .max()
            .unwrap_or(0),
        deterministic_max_scratch_bytes: deterministic
            .iter()
            .filter_map(|run| run.rerank_scratch_bytes)
            .max()
            .unwrap_or(0),
        non_text_provider_inputs: runs.iter().map(|run| run.non_text_provider_inputs).sum(),
    }
}

fn optional_sum(runs: &[&RunMetric], value: impl Fn(&RunMetric) -> Option<usize>) -> usize {
    runs.iter().filter_map(|run| value(run)).sum()
}

fn variant_runs(runs: &[RunMetric], variant: EvaluationVariant) -> Vec<&RunMetric> {
    runs.iter()
        .filter(|run| run.variant == variant.label())
        .collect()
}

fn task_rate(runs: &[&RunMetric], predicate: impl Fn(&RunMetric) -> bool) -> f64 {
    ratio(runs.iter().filter(|run| predicate(run)).count(), runs.len())
}

fn recall_at_5(runs: &[&RunMetric]) -> f64 {
    task_rate(runs, |run| {
        run.expected_path_rank.is_some_and(|rank| rank <= 5)
    })
}

fn mean_reciprocal_rank(runs: &[&RunMetric]) -> f64 {
    runs.iter()
        .filter_map(|run| run.expected_path_rank)
        .map(|rank| 1.0 / rank as f64)
        .sum::<f64>()
        / runs.len().max(1) as f64
}

fn collision_result_rate(runs: &[&RunMetric]) -> f64 {
    ratio(
        runs.iter().map(|run| run.collision_result_count).sum(),
        runs.iter().map(|run| run.result_count).sum(),
    )
}

fn request_amplification(runs: &[&RunMetric]) -> f64 {
    let max_batch_inputs = EmbeddingExecutorConfig::default().max_batch_inputs;
    let lower_bound = runs
        .iter()
        .map(|run| run.embedded_documents.div_ceil(max_batch_inputs))
        .sum();
    ratio(
        runs.iter().map(|run| run.document_embedding_requests).sum(),
        lower_bound,
    )
}

fn rate(count: usize) -> f64 {
    count as f64 / TASKS.len() as f64
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn percentile(mut values: Vec<u64>, quantile: f64) -> u64 {
    values.sort_unstable();
    let index = ((values.len() - 1) as f64 * quantile).ceil() as usize;
    values[index]
}
