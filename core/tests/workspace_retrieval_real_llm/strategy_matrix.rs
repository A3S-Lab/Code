//! Orthogonal chunk-strategy slice for the opt-in real-model evaluation.

use std::sync::Arc;

use a3s_code_core::{
    ChunkCatalogLimits, ChunkingConfig, CustomWorkspaceChunkingStrategy,
    FixedWindowChunkingOptions, RecursiveChunkingOptions, WorkspaceChunkCatalog,
    WorkspaceChunkRange, WorkspaceChunkingError, WorkspaceChunkingInput, WorkspaceChunkingStrategy,
    WorkspacePath, WorkspaceRetrievalPhase,
};
use serde::Serialize;

use super::{
    deepseek_agent, run_task, write_fixture, EvaluationVariant, RunMetric, EXPECTED_CHUNK_COUNT,
    TASKS, TEXT_FILE_COUNT,
};

const WINDOW_TARGET_BYTES: usize = 512;
const WINDOW_OVERLAP_BYTES: usize = 64;
const RECURSIVE_SEPARATORS: [&str; 4] = ["\n\n", "\n", ". ", " "];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EvaluationChunking {
    Line,
    FixedWindow,
    Recursive,
    CustomWholeFile,
}

impl EvaluationChunking {
    const ALL: [Self; 4] = [
        Self::Line,
        Self::FixedWindow,
        Self::Recursive,
        Self::CustomWholeFile,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::FixedWindow => "fixed_window_512_64",
            Self::Recursive => "recursive_512_64_explicit",
            Self::CustomWholeFile => "rust_custom_whole_file_negative_control",
        }
    }

    const fn quality_gate_required(self) -> bool {
        !matches!(self, Self::CustomWholeFile)
    }

    const fn expected_chunk_count(self) -> usize {
        match self {
            Self::Line => EXPECTED_CHUNK_COUNT,
            Self::FixedWindow => 38,
            Self::Recursive => 39,
            Self::CustomWholeFile => TEXT_FILE_COUNT,
        }
    }

    pub(super) fn core_strategy(self) -> WorkspaceChunkingStrategy {
        match self {
            Self::Line => WorkspaceChunkingStrategy::Lines,
            Self::FixedWindow => WorkspaceChunkingStrategy::FixedWindow(
                FixedWindowChunkingOptions::new(WINDOW_TARGET_BYTES, WINDOW_OVERLAP_BYTES)
                    .expect("locked fixed-window strategy"),
            ),
            Self::Recursive => WorkspaceChunkingStrategy::Recursive(
                RecursiveChunkingOptions::new(WINDOW_TARGET_BYTES, WINDOW_OVERLAP_BYTES)
                    .expect("locked recursive strategy")
                    .with_separators(RECURSIVE_SEPARATORS)
                    .expect("locked recursive separators"),
            ),
            Self::CustomWholeFile => {
                WorkspaceChunkingStrategy::custom(Arc::new(WholeFileChunkingStrategy))
            }
        }
    }
}

struct WholeFileChunkingStrategy;

impl CustomWorkspaceChunkingStrategy for WholeFileChunkingStrategy {
    fn split(
        &self,
        input: WorkspaceChunkingInput<'_>,
    ) -> Result<Vec<WorkspaceChunkRange>, WorkspaceChunkingError> {
        if input.content.is_empty() {
            Ok(Vec::new())
        } else {
            Ok(vec![WorkspaceChunkRange::new(0, input.content.len())])
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StrategySummary {
    strategy: &'static str,
    quality_gate_required: bool,
    quality_gate_passed: bool,
    task_count: usize,
    task_accuracy: f64,
    tool_protocol_rate: f64,
    recall_at_5: f64,
    mean_reciprocal_rank: f64,
    indexed_chunks: usize,
    chunks_per_text_file: f64,
    max_vector_bytes: usize,
    total_provider_input_bytes: usize,
    document_requests: usize,
    document_request_amplification: f64,
    session_construction_p50_ms: u64,
    session_construction_p95_ms: u64,
    index_ready_p50_ms: u64,
    index_ready_p95_ms: u64,
    turn_p50_ms: u64,
    turn_p95_ms: u64,
    close_p50_ms: u64,
    close_p95_ms: u64,
    total_model_tokens: usize,
    non_text_provider_inputs: usize,
    released_session_rate: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StrategyEvaluationReport {
    schema_version: u32,
    chat_model: String,
    embedding_provider: &'static str,
    task_count: usize,
    strategy_count: usize,
    fixed_target_bytes: usize,
    overlap_bytes: usize,
    recursive_separators: [&'static str; 4],
    summaries: Vec<StrategySummary>,
    runs: Vec<RunMetric>,
}

fn summarize_strategy(runs: &[RunMetric], strategy: EvaluationChunking) -> StrategySummary {
    let selected = runs
        .iter()
        .filter(|run| run.chunking_strategy == strategy.label())
        .collect::<Vec<_>>();
    assert!(!selected.is_empty(), "missing {} runs", strategy.label());
    let indexed_chunks = selected[0].indexed_chunks;
    assert!(
        selected
            .iter()
            .all(|run| run.indexed_chunks == indexed_chunks),
        "{} produced unstable chunk counts",
        strategy.label()
    );
    let request_lower_bound = selected
        .iter()
        .map(|run| run.embedding_batching.batch_limit_lower_bound)
        .sum::<usize>();
    let task_accuracy = ratio(
        selected.iter().filter(|run| run.completion_correct).count(),
        selected.len(),
    );
    let tool_protocol_rate = ratio(
        selected.iter().filter(|run| run.tool_protocol_ok).count(),
        selected.len(),
    );
    let recall_at_5 = ratio(
        selected
            .iter()
            .filter(|run| run.expected_path_rank.is_some_and(|rank| rank <= 5))
            .count(),
        selected.len(),
    );
    let mean_reciprocal_rank = selected
        .iter()
        .filter_map(|run| run.expected_path_rank)
        .map(|rank| 1.0 / rank as f64)
        .sum::<f64>()
        / selected.len() as f64;
    StrategySummary {
        strategy: strategy.label(),
        quality_gate_required: strategy.quality_gate_required(),
        quality_gate_passed: task_accuracy == 1.0
            && tool_protocol_rate == 1.0
            && recall_at_5 == 1.0
            && mean_reciprocal_rank >= 0.5,
        task_count: selected.len(),
        task_accuracy,
        tool_protocol_rate,
        recall_at_5,
        mean_reciprocal_rank,
        indexed_chunks,
        chunks_per_text_file: ratio(indexed_chunks, TEXT_FILE_COUNT),
        max_vector_bytes: selected
            .iter()
            .map(|run| run.vector_bytes)
            .max()
            .unwrap_or(0),
        total_provider_input_bytes: selected.iter().map(|run| run.embedded_input_bytes).sum(),
        document_requests: selected
            .iter()
            .map(|run| run.document_embedding_requests)
            .sum(),
        document_request_amplification: ratio(
            selected
                .iter()
                .map(|run| run.document_embedding_requests)
                .sum(),
            request_lower_bound,
        ),
        session_construction_p50_ms: percentile(
            selected
                .iter()
                .map(|run| run.session_construction_ms)
                .collect(),
            0.50,
        ),
        session_construction_p95_ms: percentile(
            selected
                .iter()
                .map(|run| run.session_construction_ms)
                .collect(),
            0.95,
        ),
        index_ready_p50_ms: percentile(
            selected.iter().map(|run| run.index_ready_ms).collect(),
            0.50,
        ),
        index_ready_p95_ms: percentile(
            selected.iter().map(|run| run.index_ready_ms).collect(),
            0.95,
        ),
        turn_p50_ms: percentile(
            selected.iter().map(|run| run.turn_elapsed_ms).collect(),
            0.50,
        ),
        turn_p95_ms: percentile(
            selected.iter().map(|run| run.turn_elapsed_ms).collect(),
            0.95,
        ),
        close_p50_ms: percentile(selected.iter().map(|run| run.close_ms).collect(), 0.50),
        close_p95_ms: percentile(selected.iter().map(|run| run.close_ms).collect(), 0.95),
        total_model_tokens: selected.iter().map(|run| run.total_tokens).sum(),
        non_text_provider_inputs: selected
            .iter()
            .map(|run| run.non_text_provider_inputs)
            .sum(),
        released_session_rate: ratio(
            selected
                .iter()
                .filter(|run| run.released_after_close)
                .count(),
            selected.len(),
        ),
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn percentile(mut values: Vec<u64>, quantile: f64) -> u64 {
    assert!(!values.is_empty(), "percentile requires observations");
    values.sort_unstable();
    let index = ((values.len() - 1) as f64 * quantile).ceil() as usize;
    values[index]
}

#[test]
fn locked_strategy_fixture_has_stable_chunks_and_complete_answer_tokens() {
    let workspace = tempfile::tempdir().expect("create strategy fixture workspace");
    write_fixture(workspace.path());
    let mut paths = std::fs::read_dir(workspace.path().join("src"))
        .expect("read strategy fixture source")
        .map(|entry| entry.expect("strategy fixture entry").path())
        .collect::<Vec<_>>();
    paths.sort();
    assert_eq!(paths.len(), TEXT_FILE_COUNT);

    for strategy in EvaluationChunking::ALL {
        let catalog = WorkspaceChunkCatalog::new_with_strategy(
            strategy.core_strategy(),
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
        )
        .expect("construct strategy fixture catalog");
        let mut snapshot = catalog.snapshot().expect("empty strategy catalog");
        for (index, path) in paths.iter().enumerate() {
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("UTF-8 fixture filename");
            let relative = format!("src/{filename}");
            let content = std::fs::read_to_string(path).expect("read strategy fixture source");
            snapshot = catalog
                .replace_file(
                    &WorkspacePath::from_normalized(relative),
                    Some("rust"),
                    index as u64 + 1,
                    &content,
                )
                .expect("chunk strategy fixture source");
        }
        assert_eq!(snapshot.file_count(), TEXT_FILE_COUNT, "{strategy:?}");
        assert_eq!(
            snapshot.chunk_count(),
            strategy.expected_chunk_count(),
            "{strategy:?}"
        );
        let chunks = snapshot.chunks();
        for task in TASKS {
            assert!(
                chunks.iter().any(|chunk| {
                    chunk.path.as_ref() == task.expected_path
                        && chunk.text.contains(task.expected_identifier)
                }),
                "{} split the answer token for {}",
                strategy.label(),
                task.name
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the repository DeepSeek credentials and network access"]
async fn real_deepseek_chunking_strategy_matrix_qualifies_builtins_and_audits_custom_control() {
    let (agent, model) = deepseek_agent().await;
    let mut runs = Vec::with_capacity(TASKS.len() * EvaluationChunking::ALL.len());
    for (task_index, task) in TASKS.iter().copied().enumerate() {
        // Rotate arm order per task so a fixed remote-service time trend does
        // not always favor the same strategy.
        for offset in 0..EvaluationChunking::ALL.len() {
            let strategy =
                EvaluationChunking::ALL[(task_index + offset) % EvaluationChunking::ALL.len()];
            runs.push(
                run_task(
                    &agent,
                    task,
                    EvaluationVariant::HybridDeterministic,
                    strategy,
                    task_index * EvaluationChunking::ALL.len() + offset,
                    false,
                )
                .await,
            );
        }
    }

    let summaries = EvaluationChunking::ALL
        .into_iter()
        .map(|strategy| summarize_strategy(&runs, strategy))
        .collect::<Vec<_>>();
    println!(
        "WSR_DEEPSEEK_CHUNKING_SUMMARY={}",
        serde_json::to_string(&summaries).expect("serialize chunking summaries")
    );

    for summary in &summaries {
        assert_eq!(summary.task_count, TASKS.len(), "{summary:#?}");
        assert_eq!(summary.tool_protocol_rate, 1.0, "{runs:#?}");
        assert_eq!(summary.recall_at_5, 1.0, "{runs:#?}");
        assert!(summary.mean_reciprocal_rank >= 0.5, "{runs:#?}");
        assert_eq!(summary.non_text_provider_inputs, 0, "{runs:#?}");
        assert_eq!(summary.released_session_rate, 1.0, "{runs:#?}");
        if summary.quality_gate_required {
            assert_eq!(summary.task_accuracy, 1.0, "{runs:#?}");
            assert!(summary.quality_gate_passed, "{runs:#?}");
        }
    }
    for run in &runs {
        let strategy = EvaluationChunking::ALL
            .into_iter()
            .find(|strategy| strategy.label() == run.chunking_strategy)
            .expect("known evaluation strategy");
        assert_eq!(run.phase, WorkspaceRetrievalPhase::Ready, "{run:#?}");
        assert_eq!(run.coverage_bps, 10_000, "{run:#?}");
        assert_eq!(run.eligible_files, TEXT_FILE_COUNT, "{run:#?}");
        assert_eq!(run.indexed_files, TEXT_FILE_COUNT, "{run:#?}");
        assert_eq!(
            run.indexed_chunks,
            strategy.expected_chunk_count(),
            "{run:#?}"
        );
        assert_eq!(run.failed_files, 0, "{run:#?}");
        assert_eq!(run.embedded_documents, run.indexed_chunks, "{run:#?}");
        assert_eq!(run.document_embedding_requests, 1, "{run:#?}");
        assert_eq!(
            run.embedding_batching.document_inputs, run.indexed_chunks,
            "{run:#?}"
        );
        assert_eq!(run.embedding_batching.document_provider_requests, 1);
        assert_eq!(run.embedding_batching.batch_limit_lower_bound, 1);
        assert_eq!(run.embedded_queries, 1, "{run:#?}");
        assert_eq!(run.query_embedding_requests, 1, "{run:#?}");
        assert_eq!(run.vector_records, run.indexed_chunks, "{run:#?}");
        assert_eq!(run.non_text_provider_inputs, 0, "{run:#?}");
        assert_eq!(
            run.algorithm.as_deref(),
            Some("rrf_k60+deterministic_mmr_v1"),
            "{run:#?}"
        );
        assert_eq!(run.rerank_requested_mode.as_deref(), Some("deterministic"));
        assert_eq!(run.rerank_applied_mode.as_deref(), Some("deterministic"));
        assert_eq!(run.rerank_fallback, None, "{run:#?}");
        assert!(run.released_after_close, "{run:#?}");
    }

    let report = StrategyEvaluationReport {
        schema_version: 2,
        chat_model: model,
        embedding_provider: "process-local deterministic semantic oracle",
        task_count: TASKS.len(),
        strategy_count: EvaluationChunking::ALL.len(),
        fixed_target_bytes: WINDOW_TARGET_BYTES,
        overlap_bytes: WINDOW_OVERLAP_BYTES,
        recursive_separators: RECURSIVE_SEPARATORS,
        summaries,
        runs,
    };
    println!(
        "WSR_DEEPSEEK_CHUNKING_EVAL={}",
        serde_json::to_string(&report).expect("serialize chunking evaluation report")
    );
}
