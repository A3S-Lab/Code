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
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingNormalization, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentEvent, AgentSession, CodeConfig, SessionOptions, SystemPromptSlots,
    WorkspaceRerankOptions, WorkspaceRetrievalOptions, WorkspaceRetrievalPhase,
    WorkspaceRetrievalStatus,
};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

#[path = "workspace_retrieval_real_llm/report.rs"]
mod report;
use report::{summarize, summarize_rerank, EvaluationReport, RerankEvaluationReport};
#[path = "workspace_retrieval_real_llm/strategy_matrix.rs"]
mod strategy_matrix;
use strategy_matrix::EvaluationChunking;

const DIMENSION: usize = 8;
const TEXT_FILE_COUNT: usize = 30;
const NON_TEXT_FILE_COUNT: usize = 3;
const EXPECTED_CHUNK_COUNT: usize = 31;
const QUERY_ID: &str = "workspace-query";
const TURN_TIMEOUT: Duration = Duration::from_secs(240);
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const COLLISION_COPIES_PER_TASK: usize = 8;
const TEST_GUIDELINES: &str = "This is a deterministic repository retrieval evaluation. Follow the requested one-tool protocol exactly. Never guess an identifier that is absent from the tool evidence.";

#[derive(Clone, Copy)]
struct EvaluationTask {
    name: &'static str,
    query: &'static str,
    expected_path: &'static str,
    expected_identifier: &'static str,
    collision_marker: &'static str,
}

const TASKS: [EvaluationTask; 3] = [
    EvaluationTask {
        name: "reconnect_replay_guard",
        query: "what routine prevents duplicate delivery after a transport reconnect",
        expected_path: "src/replay_fence.rs",
        expected_identifier: "suppress_replayed_envelopes",
        collision_marker: "SEMANTIC_COLLISION_REPLAY_GUARD",
    },
    EvaluationTask {
        name: "session_projection_cleanup",
        query: "会话结束后，哪个函数负责销毁只存在于内存中的检索投影",
        expected_path: "src/session_projection.rs",
        expected_identifier: "release_ephemeral_projection",
        collision_marker: "SEMANTIC_COLLISION_SESSION_PROJECTION",
    },
    EvaluationTask {
        name: "embedding_backpressure_limit",
        query: "where is the backpressure ceiling for queued embedding work defined",
        expected_path: "src/embedding_admission.rs",
        expected_identifier: "MAX_PENDING_EMBED_BATCHES",
        collision_marker: "SEMANTIC_COLLISION_EMBEDDING_BACKPRESSURE",
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EvaluationVariant {
    Disabled,
    Semantic,
    HybridRrf,
    HybridDeterministic,
}

impl EvaluationVariant {
    fn label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Semantic => "semantic",
            Self::HybridRrf => "hybrid_rrf",
            Self::HybridDeterministic => "hybrid_deterministic",
        }
    }

    fn retrieval_enabled(self) -> bool {
        self != Self::Disabled
    }

    fn requested_mode(self) -> &'static str {
        match self {
            Self::Disabled => "bm25",
            Self::Semantic => "semantic",
            Self::HybridRrf | Self::HybridDeterministic => "hybrid",
        }
    }
}

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
            .position(|task| text.contains(task.collision_marker))
        {
            task
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
    variant: &'static str,
    chunking_strategy: &'static str,
    retrieval_enabled: bool,
    requested_mode: String,
    tool_protocol_ok: bool,
    completion_correct: bool,
    expected_path_rank: Option<usize>,
    result_count: usize,
    collision_result_count: usize,
    algorithm: Option<String>,
    rerank_requested_mode: Option<String>,
    rerank_applied_mode: Option<String>,
    rerank_input_candidates: Option<usize>,
    rerank_evaluated_candidates: Option<usize>,
    rerank_selected_candidates: Option<usize>,
    near_duplicate_candidates: Option<usize>,
    selected_near_duplicates: Option<usize>,
    rerank_feature_bytes: Option<usize>,
    rerank_scratch_bytes: Option<usize>,
    rerank_candidate_truncated: Option<bool>,
    rerank_fallback: Option<String>,
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

fn write_rerank_adversarial_fixture(root: &Path) {
    write_fixture(root);
    let source = root.join("src");
    for task in TASKS {
        for copy in 0..COLLISION_COPIES_PER_TASK {
            let mut body = String::new();
            for _ in 0..12 {
                writeln!(body, "// {} {}", task.query, task.collision_marker)
                    .expect("write collision evidence");
            }
            writeln!(
                body,
                "pub fn documented_{}_collision_{copy:02}() {{}}",
                task.name
            )
            .expect("write collision identifier");
            std::fs::write(
                source.join(format!("collision_{}_{copy:02}.rs", task.name)),
                body,
            )
            .expect("write adversarial collision source");
        }
    }
}

fn session_options(
    session_id: String,
    provider: Arc<dyn EmbeddingProvider>,
    variant: EvaluationVariant,
    chunking: EvaluationChunking,
) -> SessionOptions {
    let mut policy = PermissionPolicy::new().allow_all(&["search(*)"]);
    policy.default_decision = PermissionDecision::Deny;
    let retrieval =
        WorkspaceRetrievalOptions::new(provider).with_chunking_strategy(chunking.core_strategy());
    let retrieval = if variant == EvaluationVariant::HybridDeterministic {
        retrieval.with_rerank_options(WorkspaceRerankOptions::deterministic())
    } else {
        retrieval
    };
    let options = SessionOptions::new()
        .with_session_id(session_id)
        .with_permission_policy(policy)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(2)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(TEST_GUIDELINES))
        .with_workspace_retrieval(retrieval);
    if variant.retrieval_enabled() {
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

async fn run_turn(
    session: &AgentSession,
    task: EvaluationTask,
    variant: EvaluationVariant,
) -> TurnTrace {
    let prompt = format!(
        "Inspect the search tool schema. Make exactly one search call and no other tool call. Use query exactly: {query}. Set path to '.', include to '*.rs', limit to 5, and mode to '{mode}'. After the result, return exactly one Rust identifier that directly answers the query and is supported by the evidence, or NOT_FOUND when no relevant identifier is present.",
        query = task.query,
        mode = variant.requested_mode(),
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
    variant: EvaluationVariant,
    chunking: EvaluationChunking,
    ordinal: usize,
    adversarial_rerank_fixture: bool,
) -> RunMetric {
    let workspace = tempfile::tempdir().expect("create evaluation workspace");
    if adversarial_rerank_fixture {
        write_rerank_adversarial_fixture(workspace.path());
    } else {
        write_fixture(workspace.path());
    }
    let counters = Arc::new(ProviderCounters::default());
    let provider: Arc<dyn EmbeddingProvider> =
        Arc::new(EvaluationEmbeddingProvider::new(Arc::clone(&counters)));
    let construction_started = Instant::now();
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(session_options(
                format!(
                    "wsr-deepseek-{ordinal}-{}-{}",
                    variant.label(),
                    chunking.label()
                ),
                provider,
                variant,
                chunking,
            )),
        )
        .await
        .expect("construct evaluation session");
    let session_construction_ms = elapsed_ms(construction_started);
    let index_started = Instant::now();
    let status = if variant.retrieval_enabled() {
        wait_until_ready(&session).await
    } else {
        session.workspace_retrieval_status()
    };
    let index_ready_ms = if variant.retrieval_enabled() {
        elapsed_ms(index_started)
    } else {
        0
    };
    let trace = run_turn(&session, task, variant).await;
    let call = trace.calls.first();
    let requested_mode = call
        .and_then(|call| call.args.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("<missing>")
        .to_owned();
    let expected_mode = variant.requested_mode();
    let tool_protocol_ok = trace.calls.len() == 1
        && call.is_some_and(|call| {
            call.name == "search"
                && call.exit_code == 0
                && call.args.get("query").and_then(Value::as_str) == Some(task.query)
                && call.args.get("mode").and_then(Value::as_str) == Some(expected_mode)
        });
    let metadata = call.and_then(|call| call.metadata.as_ref());
    let results = metadata
        .and_then(|metadata| metadata.get("results"))
        .and_then(Value::as_array);
    let expected_path_rank = results.and_then(|results| {
        results.iter().position(|result| {
            result.get("path").and_then(Value::as_str) == Some(task.expected_path)
        })
    });
    let expected_path_rank = expected_path_rank.map(|rank| rank + 1);
    let result_count = results.map_or(0, Vec::len);
    let collision_result_count = results.map_or(0, |results| {
        results
            .iter()
            .filter(|result| {
                result
                    .get("path")
                    .and_then(Value::as_str)
                    .is_some_and(|path| path.starts_with("src/collision_"))
            })
            .count()
    });
    let rerank = metadata.and_then(|metadata| metadata.get("rerank"));
    let algorithm = json_string(metadata, "algorithm");
    let rerank_requested_mode = json_string(rerank, "requestedMode");
    let rerank_applied_mode = json_string(rerank, "appliedMode");
    let rerank_input_candidates = json_usize(rerank, "inputCandidates");
    let rerank_evaluated_candidates = json_usize(rerank, "evaluatedCandidates");
    let rerank_selected_candidates = json_usize(rerank, "selectedCandidates");
    let near_duplicate_candidates = json_usize(rerank, "nearDuplicateCandidates");
    let selected_near_duplicates = json_usize(rerank, "selectedNearDuplicates");
    let rerank_feature_bytes = json_usize(rerank, "featureBytes");
    let rerank_scratch_bytes = json_usize(rerank, "accountedScratchBytes");
    let rerank_candidate_truncated = rerank
        .and_then(|rerank| rerank.get("candidateTruncated"))
        .and_then(Value::as_bool);
    let rerank_fallback = json_string(rerank, "fallback");
    let normalized_answer = trace.final_text.trim().trim_matches('`').trim().to_owned();
    let completion_correct = normalized_answer == task.expected_identifier;
    let close_started = Instant::now();
    session.close().await;
    let close_ms = elapsed_ms(close_started);
    let closed = session.workspace_retrieval_status();
    assert_eq!(
        closed.phase,
        if variant.retrieval_enabled() {
            WorkspaceRetrievalPhase::Closed
        } else {
            WorkspaceRetrievalPhase::Disabled
        }
    );
    let released_after_close = closed.vector_records == 0 && closed.vector_bytes == 0;

    RunMetric {
        task: task.name,
        variant: variant.label(),
        chunking_strategy: chunking.label(),
        retrieval_enabled: variant.retrieval_enabled(),
        requested_mode,
        tool_protocol_ok,
        completion_correct,
        expected_path_rank,
        result_count,
        collision_result_count,
        algorithm,
        rerank_requested_mode,
        rerank_applied_mode,
        rerank_input_candidates,
        rerank_evaluated_candidates,
        rerank_selected_candidates,
        near_duplicate_candidates,
        selected_near_duplicates,
        rerank_feature_bytes,
        rerank_scratch_bytes,
        rerank_candidate_truncated,
        rerank_fallback,
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

fn json_string(value: Option<&Value>, field: &str) -> Option<String> {
    value
        .and_then(|value| value.get(field))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn json_usize(value: Option<&Value>, field: &str) -> Option<usize> {
    value
        .and_then(|value| value.get(field))
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn stable_bucket(text: &str, buckets: usize) -> usize {
    text.bytes().fold(0usize, |hash, byte| {
        hash.wrapping_mul(16777619).wrapping_add(byte as usize)
    }) % buckets
}

async fn deepseek_agent() -> (Agent, String) {
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
    (agent, model)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the repository DeepSeek credentials and network access"]
async fn real_deepseek_completes_semantic_tasks_and_beats_disabled_ablation() {
    let (agent, model) = deepseek_agent().await;
    let mut runs = Vec::with_capacity(TASKS.len() * 2);
    for (ordinal, task) in TASKS.iter().copied().enumerate() {
        runs.push(
            run_task(
                &agent,
                task,
                EvaluationVariant::Disabled,
                EvaluationChunking::Line,
                ordinal,
                false,
            )
            .await,
        );
        runs.push(
            run_task(
                &agent,
                task,
                EvaluationVariant::Semantic,
                EvaluationChunking::Line,
                ordinal,
                false,
            )
            .await,
        );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the repository DeepSeek credentials and network access"]
async fn real_deepseek_deterministic_rerank_defeats_duplicate_channel_collisions() {
    let (agent, model) = deepseek_agent().await;
    let mut runs = Vec::with_capacity(TASKS.len() * 2);
    for (ordinal, task) in TASKS.iter().copied().enumerate() {
        let variants = if ordinal % 2 == 0 {
            [
                EvaluationVariant::HybridRrf,
                EvaluationVariant::HybridDeterministic,
            ]
        } else {
            [
                EvaluationVariant::HybridDeterministic,
                EvaluationVariant::HybridRrf,
            ]
        };
        for variant in variants {
            runs.push(
                run_task(
                    &agent,
                    task,
                    variant,
                    EvaluationChunking::Line,
                    ordinal,
                    true,
                )
                .await,
            );
        }
    }

    let summary = summarize_rerank(&runs);
    println!(
        "WSR_DEEPSEEK_RERANK_SUMMARY={}",
        serde_json::to_string(&summary).expect("serialize rerank evaluation summary")
    );
    assert_eq!(summary.rrf_tool_protocol_rate, 1.0, "{runs:#?}");
    assert_eq!(summary.deterministic_tool_protocol_rate, 1.0, "{runs:#?}");
    assert_eq!(summary.deterministic_task_accuracy, 1.0, "{runs:#?}");
    assert!(
        summary.deterministic_task_accuracy > summary.rrf_task_accuracy,
        "rerank did not improve task completion: {runs:#?}"
    );
    assert_eq!(summary.deterministic_recall_at_5, 1.0, "{runs:#?}");
    assert!(
        summary.deterministic_recall_at_5 > summary.rrf_recall_at_5,
        "rerank did not improve retrieval recall: {runs:#?}"
    );
    assert!(
        summary.deterministic_mrr > summary.rrf_mrr,
        "rerank did not improve reciprocal rank: {runs:#?}"
    );
    assert!(
        summary.deterministic_collision_result_rate < summary.rrf_collision_result_rate,
        "rerank did not reduce collision evidence: {runs:#?}"
    );
    assert!(summary.deterministic_near_duplicate_candidates > 0);
    assert_eq!(
        summary.deterministic_input_candidates,
        summary.deterministic_evaluated_candidates
    );
    assert_eq!(summary.deterministic_selected_candidates, TASKS.len() * 10);
    assert!(
        summary.deterministic_selected_near_duplicates < summary.deterministic_selected_candidates
    );
    assert!(summary.deterministic_selected_near_duplicate_rate < 1.0);
    assert!(summary.deterministic_max_feature_bytes <= 100 * 4 * 1024);
    assert!(summary.deterministic_max_scratch_bytes <= 4 * 1024 * 1024);
    assert_eq!(summary.non_text_provider_inputs, 0, "{runs:#?}");

    let expected_text_files = TEXT_FILE_COUNT + TASKS.len() * COLLISION_COPIES_PER_TASK;
    let expected_chunks = EXPECTED_CHUNK_COUNT + TASKS.len() * COLLISION_COPIES_PER_TASK;
    for run in &runs {
        assert!(run.released_after_close, "{run:#?}");
        assert_eq!(run.phase, WorkspaceRetrievalPhase::Ready, "{run:#?}");
        assert_eq!(run.coverage_bps, 10_000, "{run:#?}");
        assert_eq!(run.eligible_files, expected_text_files, "{run:#?}");
        assert_eq!(run.indexed_files, expected_text_files, "{run:#?}");
        assert_eq!(run.indexed_chunks, expected_chunks, "{run:#?}");
        assert_eq!(run.failed_files, 0, "{run:#?}");
        assert_eq!(run.embedded_documents, expected_chunks, "{run:#?}");
        assert_eq!(run.non_text_provider_inputs, 0, "{run:#?}");
        assert_eq!(run.rerank_selected_candidates, Some(10), "{run:#?}");
        assert_eq!(run.rerank_candidate_truncated, Some(false), "{run:#?}");
        assert_eq!(run.rerank_fallback, None, "{run:#?}");
        match run.variant {
            "hybrid_rrf" => {
                assert_eq!(run.algorithm.as_deref(), Some("rrf_k60"), "{run:#?}");
                assert_eq!(run.rerank_requested_mode.as_deref(), Some("rrf_only"));
                assert_eq!(run.rerank_applied_mode.as_deref(), Some("rrf_only"));
            }
            "hybrid_deterministic" => {
                assert_eq!(
                    run.algorithm.as_deref(),
                    Some("rrf_k60+deterministic_mmr_v1"),
                    "{run:#?}"
                );
                assert_eq!(run.rerank_requested_mode.as_deref(), Some("deterministic"));
                assert_eq!(run.rerank_applied_mode.as_deref(), Some("deterministic"));
            }
            variant => panic!("unexpected rerank evaluation variant: {variant}"),
        }
    }

    let report = RerankEvaluationReport {
        schema_version: 1,
        chat_model: model,
        embedding_provider: "process-local deterministic semantic collision oracle",
        task_count: TASKS.len(),
        collision_copies_per_task: COLLISION_COPIES_PER_TASK,
        summary,
        runs,
    };
    println!(
        "WSR_DEEPSEEK_RERANK_EVAL={}",
        serde_json::to_string(&report).expect("serialize rerank evaluation report")
    );
}
