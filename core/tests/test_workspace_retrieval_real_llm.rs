//! Real-DeepSeek completion and retrieval ablation for session-bound vectors.
//!
//! The chat model comes from the repository ACL. Embeddings are deterministic
//! and process-local so this test exercises the real Code catalog, vector index,
//! search tool, and model tool loop without sending fixture source to a second
//! remote model. Run serially with:
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_workspace_retrieval_real_llm \
//!   -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingNormalization,
    EmbeddingProvider, EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentEvent, AgentSession, CodeConfig, SessionOptions, SystemPromptSlots,
    WorkspaceRetrievalOptions, WorkspaceRetrievalPhase, WorkspaceRetrievalStatus,
};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

const DIMENSION: usize = 8;
const TEXT_FILE_COUNT: usize = 30;
const NON_TEXT_FILE_COUNT: usize = 3;
const EXPECTED_CHUNK_COUNT: usize = 31;
const QUERY_ID: &str = "workspace-query";
const TURN_TIMEOUT: Duration = Duration::from_secs(240);
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const TEST_GUIDELINES: &str = "This is a deterministic repository retrieval evaluation. Follow the requested one-tool protocol exactly. Never guess an identifier that is absent from the tool evidence.";

#[derive(Clone, Copy)]
struct EvaluationTask {
    name: &'static str,
    query: &'static str,
    expected_path: &'static str,
    expected_identifier: &'static str,
}

const TASKS: [EvaluationTask; 3] = [
    EvaluationTask {
        name: "reconnect_replay_guard",
        query: "what routine prevents duplicate delivery after a transport reconnect",
        expected_path: "src/replay_fence.rs",
        expected_identifier: "suppress_replayed_envelopes",
    },
    EvaluationTask {
        name: "session_projection_cleanup",
        query: "会话结束后，哪个函数负责销毁只存在于内存中的检索投影",
        expected_path: "src/session_projection.rs",
        expected_identifier: "release_ephemeral_projection",
    },
    EvaluationTask {
        name: "embedding_backpressure_limit",
        query: "where is the backpressure ceiling for queued embedding work defined",
        expected_path: "src/embedding_admission.rs",
        expected_identifier: "MAX_PENDING_EMBED_BATCHES",
    },
];

#[derive(Default)]
struct ProviderCounters {
    requests: AtomicUsize,
    document_requests: AtomicUsize,
    query_requests: AtomicUsize,
    document_inputs: AtomicUsize,
    query_inputs: AtomicUsize,
    input_bytes: AtomicUsize,
    non_text_inputs: AtomicUsize,
}

struct EvaluationEmbeddingProvider {
    counters: Arc<ProviderCounters>,
}

impl EvaluationEmbeddingProvider {
    fn new(counters: Arc<ProviderCounters>) -> Self {
        Self { counters }
    }

    fn vector(input_id: &str, text: &str) -> Vec<f32> {
        let axis = if input_id == QUERY_ID {
            TASKS
                .iter()
                .position(|task| text.trim() == task.query)
                .expect("evaluation query must be one of the locked tasks")
        } else if let Some(task) = TASKS
            .iter()
            .position(|task| text.contains(task.expected_identifier))
        {
            task
        } else {
            3 + stable_bucket(text, DIMENSION - 3)
        };
        let mut vector = vec![0.0; DIMENSION];
        vector[axis] = 1.0;
        vector
    }
}

#[async_trait]
impl EmbeddingProvider for EvaluationEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new("local-evaluation", "semantic-fixture-v1", DIMENSION)
            .with_revision("2026-08-14")
            .with_normalization(EmbeddingNormalization::Unit)
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if cancellation.is_cancelled() {
            return Err(EmbeddingProviderError::Cancelled);
        }
        self.counters.requests.fetch_add(1, Ordering::AcqRel);
        let is_query_request = request.inputs().iter().all(|input| input.id() == QUERY_ID);
        let is_document_request = request.inputs().iter().all(|input| input.id() != QUERY_ID);
        assert!(
            is_query_request || is_document_request,
            "evaluation provider requests must not mix documents and queries"
        );
        if is_query_request {
            self.counters.query_requests.fetch_add(1, Ordering::AcqRel);
        } else {
            self.counters
                .document_requests
                .fetch_add(1, Ordering::AcqRel);
        }
        let mut vectors = Vec::with_capacity(request.inputs().len());
        for input in request.inputs() {
            self.counters
                .input_bytes
                .fetch_add(input.text_bytes(), Ordering::AcqRel);
            if input.id() == QUERY_ID {
                self.counters.query_inputs.fetch_add(1, Ordering::AcqRel);
            } else {
                self.counters.document_inputs.fetch_add(1, Ordering::AcqRel);
            }
            if input.text().contains("NON_TEXT_ASSET_SENTINEL") {
                self.counters.non_text_inputs.fetch_add(1, Ordering::AcqRel);
            }
            vectors.push(EmbeddingVector::new(
                input.id(),
                Self::vector(input.id(), input.text()),
            ));
        }
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

#[derive(Debug)]
struct ToolCall {
    name: String,
    args: Value,
    exit_code: i32,
    metadata: Option<Value>,
}

#[derive(Debug)]
struct TurnTrace {
    calls: Vec<ToolCall>,
    final_text: String,
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
    elapsed_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunMetric {
    task: &'static str,
    retrieval_enabled: bool,
    requested_mode: String,
    tool_protocol_ok: bool,
    completion_correct: bool,
    expected_path_rank: Option<usize>,
    result_count: usize,
    session_construction_ms: u64,
    index_ready_ms: u64,
    turn_elapsed_ms: u64,
    close_ms: u64,
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
    phase: WorkspaceRetrievalPhase,
    coverage_bps: u16,
    eligible_files: usize,
    indexed_files: usize,
    indexed_chunks: usize,
    failed_files: usize,
    vector_records: usize,
    vector_bytes: usize,
    embedding_requests: usize,
    document_embedding_requests: usize,
    query_embedding_requests: usize,
    embedded_documents: usize,
    embedded_queries: usize,
    embedded_input_bytes: usize,
    non_text_provider_inputs: usize,
    released_after_close: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvaluationSummary {
    enabled_task_accuracy: f64,
    disabled_task_accuracy: f64,
    enabled_tool_protocol_rate: f64,
    disabled_tool_protocol_rate: f64,
    semantic_recall_at_5: f64,
    semantic_mrr: f64,
    chunks_per_text_file: f64,
    document_requests_per_chunk: f64,
    document_request_amplification_vs_input_limit: f64,
    non_text_provider_inputs: usize,
    enabled_session_construction_p50_ms: u64,
    enabled_session_construction_p95_ms: u64,
    disabled_session_construction_p50_ms: u64,
    disabled_session_construction_p95_ms: u64,
    enabled_turn_p50_ms: u64,
    enabled_turn_p95_ms: u64,
    disabled_turn_p50_ms: u64,
    disabled_turn_p95_ms: u64,
    index_ready_p50_ms: u64,
    index_ready_p95_ms: u64,
    enabled_close_p50_ms: u64,
    enabled_close_p95_ms: u64,
    disabled_close_p50_ms: u64,
    disabled_close_p95_ms: u64,
    enabled_total_tokens: usize,
    disabled_total_tokens: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvaluationReport {
    schema_version: u32,
    chat_model: String,
    embedding_provider: &'static str,
    task_count: usize,
    text_file_count: usize,
    non_text_file_count: usize,
    expected_chunk_count: usize,
    summary: EvaluationSummary,
    runs: Vec<RunMetric>,
}

fn config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

fn write_fixture(root: &Path) {
    let source = root.join("src");
    std::fs::create_dir_all(&source).expect("create source directory");
    for (path, body) in [
        (
            "replay_fence.rs",
            "pub fn suppress_replayed_envelopes(sequence: u64, committed: u64) -> bool {\n    sequence <= committed\n}\n",
        ),
        (
            "session_projection.rs",
            "pub fn release_ephemeral_projection(generation: &mut Option<u64>) {\n    generation.take();\n}\n",
        ),
        (
            "reconnect_notes.rs",
            "// what routine prevents duplicate delivery after a transport reconnect\npub fn describe_reconnect_incident() {}\n",
        ),
        (
            "cleanup_notes.rs",
            "// 会话结束后，哪个函数负责销毁只存在于内存中的检索投影\npub fn document_shutdown_checklist() {}\n",
        ),
        (
            "queue_notes.rs",
            "// where is the backpressure ceiling for queued embedding work defined\npub const DOCUMENTED_QUEUE_OBSERVATION: usize = 999;\n",
        ),
    ] {
        std::fs::write(source.join(path), body).expect("write locked fixture source");
    }
    for index in 0..24 {
        std::fs::write(
            source.join(format!("unrelated_{index:02}.rs")),
            format!(
                "pub fn unrelated_worker_{index:02}(value: usize) -> usize {{ value + {index} }}\n"
            ),
        )
        .expect("write distractor source");
    }

    // Put the third answer beyond the default 80-line boundary. This locks the
    // real task evaluation to the same line-aware chunker used in production.
    let mut chunked_source = String::new();
    for index in 0..90 {
        writeln!(
            chunked_source,
            "// deterministic chunk-boundary filler {index:02}"
        )
        .expect("write chunk fixture line");
    }
    chunked_source.push_str(
        "pub const MAX_PENDING_EMBED_BATCHES: usize = 8;\n\npub fn admits_batch(pending: usize) -> bool {\n    pending < MAX_PENDING_EMBED_BATCHES\n}\n",
    );
    std::fs::write(source.join("embedding_admission.rs"), chunked_source)
        .expect("write multi-chunk fixture source");

    let assets = root.join("assets");
    std::fs::create_dir_all(&assets).expect("create non-text asset directory");
    for (path, body) in [
        (
            "architecture.pdf",
            b"%PDF-1.7\nNON_TEXT_ASSET_SENTINEL\n".as_slice(),
        ),
        (
            "slides.pptx",
            b"PK OFFICE NON_TEXT_ASSET_SENTINEL\n".as_slice(),
        ),
        ("recording.mp3", b"ID3 NON_TEXT_ASSET_SENTINEL\n".as_slice()),
    ] {
        std::fs::write(assets.join(path), body).expect("write non-text fixture asset");
    }
}

fn session_options(
    session_id: String,
    provider: Arc<dyn EmbeddingProvider>,
    retrieval_enabled: bool,
) -> SessionOptions {
    let mut policy = PermissionPolicy::new().allow_all(&["search(*)"]);
    policy.default_decision = PermissionDecision::Deny;
    let options = SessionOptions::new()
        .with_session_id(session_id)
        .with_permission_policy(policy)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(2)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(TEST_GUIDELINES))
        .with_workspace_retrieval(WorkspaceRetrievalOptions::new(provider));
    if retrieval_enabled {
        options
    } else {
        options.without_workspace_retrieval()
    }
}

async fn wait_until_ready(session: &AgentSession) -> WorkspaceRetrievalStatus {
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            let status = session.workspace_retrieval_status();
            match status.phase {
                WorkspaceRetrievalPhase::Ready | WorkspaceRetrievalPhase::Degraded => {
                    break status;
                }
                WorkspaceRetrievalPhase::Building => tokio::task::yield_now().await,
                phase => panic!("unexpected retrieval phase while building: {phase:?}"),
            }
        }
    })
    .await
    .expect("workspace retrieval did not become ready")
}

async fn run_turn(session: &AgentSession, task: EvaluationTask) -> TurnTrace {
    let prompt = format!(
        "Inspect the search tool schema. Make exactly one search call and no other tool call. Use query exactly: {query}. Set path to '.', include to '*.rs', and limit to 5. If the mode 'semantic' is available, use semantic; otherwise use bm25. After the result, return exactly one Rust identifier supported by the evidence, or NOT_FOUND when no relevant identifier is present.",
        query = task.query
    );
    let started = Instant::now();
    let (mut events, worker) = session
        .stream(&prompt, None)
        .await
        .expect("start DeepSeek turn");
    let mut starts = HashMap::<String, (String, Value)>::new();
    let mut calls = Vec::new();
    let (final_text, usage) = tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            match events
                .recv()
                .await
                .expect("DeepSeek event stream ended early")
            {
                AgentEvent::ToolExecutionStart { id, name, args } => {
                    starts.insert(id, (name, args));
                }
                AgentEvent::ToolEnd {
                    id,
                    name,
                    args,
                    exit_code,
                    metadata,
                    ..
                } => {
                    let (started_name, started_args) = starts
                        .remove(&id)
                        .unwrap_or_else(|| (name.clone(), args.unwrap_or_default()));
                    assert_eq!(started_name, name, "tool start/end names diverged");
                    calls.push(ToolCall {
                        name,
                        args: started_args,
                        exit_code,
                        metadata,
                    });
                }
                AgentEvent::End { text, usage, .. } => break (text, usage),
                AgentEvent::Error { message } => panic!("DeepSeek turn failed: {message}"),
                AgentEvent::ConfirmationRequired { tool_name, .. } => {
                    panic!("unexpected confirmation for {tool_name}")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("DeepSeek retrieval turn timed out");
    worker.await.expect("DeepSeek stream worker joins");
    assert!(starts.is_empty(), "tool starts without terminal events");
    TurnTrace {
        calls,
        final_text,
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        elapsed_ms: elapsed_ms(started),
    }
}

async fn run_task(
    agent: &Agent,
    task: EvaluationTask,
    retrieval_enabled: bool,
    ordinal: usize,
) -> RunMetric {
    let workspace = tempfile::tempdir().expect("create evaluation workspace");
    write_fixture(workspace.path());
    let counters = Arc::new(ProviderCounters::default());
    let provider: Arc<dyn EmbeddingProvider> =
        Arc::new(EvaluationEmbeddingProvider::new(Arc::clone(&counters)));
    let construction_started = Instant::now();
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(
                format!(
                    "wsr-deepseek-{ordinal}-{}",
                    if retrieval_enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ),
                provider,
                retrieval_enabled,
            )),
        )
        .await
        .expect("construct evaluation session");
    let session_construction_ms = elapsed_ms(construction_started);
    let index_started = Instant::now();
    let status = if retrieval_enabled {
        wait_until_ready(&session).await
    } else {
        session.workspace_retrieval_status()
    };
    let index_ready_ms = if retrieval_enabled {
        elapsed_ms(index_started)
    } else {
        0
    };
    let trace = run_turn(&session, task).await;
    let call = trace.calls.first();
    let requested_mode = call
        .and_then(|call| call.args.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("<missing>")
        .to_owned();
    let expected_mode = if retrieval_enabled {
        "semantic"
    } else {
        "bm25"
    };
    let tool_protocol_ok = trace.calls.len() == 1
        && call.is_some_and(|call| {
            call.name == "search"
                && call.exit_code == 0
                && call.args.get("query").and_then(Value::as_str) == Some(task.query)
                && call.args.get("mode").and_then(Value::as_str) == Some(expected_mode)
        });
    let results = call
        .and_then(|call| call.metadata.as_ref())
        .and_then(|metadata| metadata.get("results"))
        .and_then(Value::as_array);
    let expected_path_rank = results.and_then(|results| {
        results.iter().position(|result| {
            result.get("path").and_then(Value::as_str) == Some(task.expected_path)
        })
    });
    let expected_path_rank = expected_path_rank.map(|rank| rank + 1);
    let result_count = results.map_or(0, Vec::len);
    let normalized_answer = trace.final_text.trim().trim_matches('`').trim().to_owned();
    let completion_correct = normalized_answer == task.expected_identifier;
    let close_started = Instant::now();
    session.close().await;
    let close_ms = elapsed_ms(close_started);
    let closed = session.workspace_retrieval_status();
    assert_eq!(
        closed.phase,
        if retrieval_enabled {
            WorkspaceRetrievalPhase::Closed
        } else {
            WorkspaceRetrievalPhase::Disabled
        }
    );
    let released_after_close = closed.vector_records == 0 && closed.vector_bytes == 0;

    RunMetric {
        task: task.name,
        retrieval_enabled,
        requested_mode,
        tool_protocol_ok,
        completion_correct,
        expected_path_rank,
        result_count,
        session_construction_ms,
        index_ready_ms,
        turn_elapsed_ms: trace.elapsed_ms,
        close_ms,
        prompt_tokens: trace.prompt_tokens,
        completion_tokens: trace.completion_tokens,
        total_tokens: trace.total_tokens,
        phase: status.phase,
        coverage_bps: status.coverage_bps,
        eligible_files: status.eligible_files,
        indexed_files: status.indexed_files,
        indexed_chunks: status.indexed_chunks,
        failed_files: status.failed_files,
        vector_records: status.vector_records,
        vector_bytes: status.vector_bytes,
        embedding_requests: counters.requests.load(Ordering::Acquire),
        document_embedding_requests: counters.document_requests.load(Ordering::Acquire),
        query_embedding_requests: counters.query_requests.load(Ordering::Acquire),
        embedded_documents: counters.document_inputs.load(Ordering::Acquire),
        embedded_queries: counters.query_inputs.load(Ordering::Acquire),
        embedded_input_bytes: counters.input_bytes.load(Ordering::Acquire),
        non_text_provider_inputs: counters.non_text_inputs.load(Ordering::Acquire),
        released_after_close,
    }
}

fn summarize(runs: &[RunMetric]) -> EvaluationSummary {
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

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn stable_bucket(text: &str, buckets: usize) -> usize {
    text.bytes().fold(0usize, |hash, byte| {
        hash.wrapping_mul(16777619).wrapping_add(byte as usize)
    }) % buckets
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the repository DeepSeek credentials and network access"]
async fn real_deepseek_completes_semantic_tasks_and_beats_disabled_ablation() {
    let path = config_path();
    let config = CodeConfig::from_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
    let model = config
        .default_model
        .clone()
        .expect("real evaluation requires a default model");
    assert!(
        model.starts_with("deepseek/"),
        "real evaluation requires a DeepSeek default model, got {model}"
    );
    let agent = Agent::from_config(config)
        .await
        .expect("create agent from DeepSeek config");
    let mut runs = Vec::with_capacity(TASKS.len() * 2);
    for (ordinal, task) in TASKS.iter().copied().enumerate() {
        runs.push(run_task(&agent, task, false, ordinal).await);
        runs.push(run_task(&agent, task, true, ordinal).await);
    }
    let summary = summarize(&runs);
    assert_eq!(summary.enabled_task_accuracy, 1.0, "{runs:#?}");
    assert!(
        summary.enabled_task_accuracy > summary.disabled_task_accuracy,
        "enabled retrieval did not beat the disabled ablation: {runs:#?}"
    );
    assert_eq!(summary.enabled_tool_protocol_rate, 1.0, "{runs:#?}");
    assert_eq!(summary.disabled_tool_protocol_rate, 1.0, "{runs:#?}");
    assert_eq!(summary.semantic_recall_at_5, 1.0, "{runs:#?}");
    assert_eq!(summary.semantic_mrr, 1.0, "{runs:#?}");
    assert_eq!(
        summary.document_request_amplification_vs_input_limit, 30.0,
        "{runs:#?}"
    );
    assert_eq!(summary.non_text_provider_inputs, 0, "{runs:#?}");
    for run in &runs {
        assert!(run.released_after_close, "{run:#?}");
        if run.retrieval_enabled {
            assert_eq!(run.phase, WorkspaceRetrievalPhase::Ready, "{run:#?}");
            assert_eq!(run.coverage_bps, 10_000, "{run:#?}");
            assert_eq!(run.eligible_files, TEXT_FILE_COUNT, "{run:#?}");
            assert_eq!(run.indexed_files, TEXT_FILE_COUNT, "{run:#?}");
            assert_eq!(run.indexed_chunks, EXPECTED_CHUNK_COUNT, "{run:#?}");
            assert_eq!(run.failed_files, 0, "{run:#?}");
            assert_eq!(run.embedded_queries, 1, "{run:#?}");
            assert_eq!(run.query_embedding_requests, 1, "{run:#?}");
            assert_eq!(run.embedded_documents, EXPECTED_CHUNK_COUNT, "{run:#?}");
            assert_eq!(run.document_embedding_requests, TEXT_FILE_COUNT, "{run:#?}");
            assert_eq!(run.vector_records, EXPECTED_CHUNK_COUNT, "{run:#?}");
            assert_eq!(run.non_text_provider_inputs, 0, "{run:#?}");
        } else {
            assert_eq!(run.phase, WorkspaceRetrievalPhase::Disabled, "{run:#?}");
            assert_eq!(run.embedding_requests, 0, "{run:#?}");
            assert_eq!(run.vector_records, 0, "{run:#?}");
            assert_eq!(run.vector_bytes, 0, "{run:#?}");
        }
    }
    let report = EvaluationReport {
        schema_version: 1,
        chat_model: model,
        embedding_provider: "process-local deterministic semantic oracle",
        task_count: TASKS.len(),
        text_file_count: TEXT_FILE_COUNT,
        non_text_file_count: NON_TEXT_FILE_COUNT,
        expected_chunk_count: EXPECTED_CHUNK_COUNT,
        summary,
        runs,
    };
    println!(
        "WSR_DEEPSEEK_EVAL={}",
        serde_json::to_string(&report).expect("serialize evaluation report")
    );
}
