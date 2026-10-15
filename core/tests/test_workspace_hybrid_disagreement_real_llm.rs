//! Live hybrid-retrieval channel-disagreement gate against `.a3s/config.acl`.
//!
//! Hybrid retrieval earns its fourth channel only when single channels
//! genuinely disagree. This suite pins a two-task adversarial matrix:
//!
//! - **SYN** (semantic-only): the query shares zero lexical tokens with the
//!   target source, so `bm25` cannot surface it; the deterministic embedding
//!   provider places query and target on the same axis, so `semantic` can.
//! - **LEX** (lexical-only): the query names an exact identifier, but the
//!   deterministic provider deliberately mis-axes the target document (it
//!   carries the SYN marker), so `semantic` returns decoys; `bm25` and the
//!   hybrid exact channel find the identifier.
//!
//! Expected matrix (real DeepSeek model drives one `search` call per cell):
//!
//! | task | semantic | bm25 | hybrid |
//! |------|----------|------|--------|
//! | SYN  | HIT      | MISS | HIT    |
//! | LEX  | MISS     | HIT  | HIT    |
//!
//! A MISS cell still runs the model turn — the product contract under test is
//! evidence-grounded answering: without tool evidence the model must reply
//! `NOT_FOUND`, never an identifier echoed from the prompt.
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_workspace_hybrid_disagreement_real_llm \
//!   -- --ignored --nocapture --test-threads=1
//! ```

mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingNormalization, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentEvent, SessionOptions, SystemPromptSlots, WorkspaceRetrievalOptions,
    WorkspaceRetrievalPhase, WorkspaceRetrievalStatus,
};
use async_trait::async_trait;
use serde_json::Value;
use support::layer_c_model::load_pinned_layer_c_config;
use tokio_util::sync::CancellationToken;

const DIMENSION: usize = 8;
const QUERY_ID: &str = "workspace-query";
const TURN_TIMEOUT: Duration = Duration::from_secs(900);
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const TEST_GUIDELINES: &str = "This is a deterministic repository retrieval evaluation. \
     Follow the requested one-tool protocol exactly. Only reply with an identifier that \
     appears in the tool result evidence. If the tool result does not contain the answer, \
     reply NOT_FOUND. Never echo an identifier from the prompt without tool evidence.";

const SYN_MARKER: &str = "CHANNEL_MARKER_SYN_V1";
const LEX_MARKER: &str = "CHANNEL_MARKER_LEX_V1";

#[derive(Clone, Copy)]
struct DisagreementTask {
    name: &'static str,
    query: &'static str,
    expected_path: &'static str,
    expected_identifier: &'static str,
}

/// SYN: zero lexical overlap between query and target source (verified
/// token-by-token against `lexical::tokenize` behavior — lowercase words plus
/// snake/camel splits, no stopwords).
const TASK_SYN: DisagreementTask = DisagreementTask {
    name: "semantic_only_double_billing",
    // Zero token overlap with the target source (audited against
    // `lexical::tokenize`: lowercase words plus snake/camel splits — note
    // `is_empty()` in the target splits to `is` + `empty`, so the query must
    // avoid both).
    query: "how do we prevent charging twice when the same delivery gets replayed",
    expected_path: "src/billing_fence.rs",
    expected_identifier: "suppress_duplicate_debits",
};

/// LEX: exact identifier query whose target document is deliberately mis-axed
/// onto the SYN embedding axis, so the semantic channel returns decoys only.
const TASK_LEX: DisagreementTask = DisagreementTask {
    name: "lexical_only_backlog_ceiling",
    query: "find where EMBED_ADMISSION_BACKLOG_CEILING is defined",
    expected_path: "src/embed_admission.rs",
    expected_identifier: "EMBED_ADMISSION_BACKLOG_CEILING",
};

const TASKS: [DisagreementTask; 2] = [TASK_SYN, TASK_LEX];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Variant {
    Semantic,
    Bm25,
    Hybrid,
}

impl Variant {
    fn label(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Bm25 => "bm25",
            Self::Hybrid => "hybrid",
        }
    }

    fn mode(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Bm25 => "bm25",
            Self::Hybrid => "hybrid",
        }
    }
}

/// Deterministic axis-aligned embedding provider.
///
/// Queries land on the axis of the task whose locked query text they repeat.
/// Documents land on the axis of the first task marker they contain: the SYN
/// marker deliberately appears in BOTH the SYN target and the LEX target (the
/// mis-axis under test), decoys carry the LEX marker, and everything else
/// buckets deterministically off the semantic axes.
struct DisagreementEmbeddingProvider;

impl DisagreementEmbeddingProvider {
    fn vector(input_id: &str, text: &str) -> Vec<f32> {
        let axis = if input_id == QUERY_ID {
            TASKS
                .iter()
                .position(|task| text.trim() == task.query)
                .expect("query must be one of the locked disagreement tasks")
        } else if text.contains(SYN_MARKER) {
            0
        } else if text.contains(LEX_MARKER) {
            1
        } else {
            2 + (stable_bucket(text) % (DIMENSION - 2))
        };
        let mut vector = vec![0.0; DIMENSION];
        vector[axis.min(DIMENSION - 1)] = 1.0;
        vector
    }
}

fn stable_bucket(text: &str) -> usize {
    let mut hash = 0usize.wrapping_sub(1);
    for byte in text.bytes() {
        hash = (hash ^ byte as usize).wrapping_mul(16777619);
    }
    hash
}

#[async_trait]
impl EmbeddingProvider for DisagreementEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new(
            "local-disagreement",
            "hybrid-channel-fixture-v1",
            DIMENSION,
        )
        .with_revision("2026-10-07")
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
        let vectors = request
            .inputs()
            .iter()
            .map(|input| EmbeddingVector::new(input.id(), Self::vector(input.id(), input.text())))
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

fn write_fixture(root: &std::path::Path) {
    let write = |path: &str, body: &str| {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().expect("fixture parent")).expect("fixture dir");
        std::fs::write(full, body).expect("fixture source");
    };
    // SYN target: concept words only — no token overlap with the SYN query.
    write(
        "src/billing_fence.rs",
        &format!(
            r#"//! Idempotency fence for outbound payments. {SYN_MARKER}
pub fn suppress_duplicate_debits(idempotency_key: &str) -> bool {{
    // one debit per key even if upstream resends an envelope
    !idempotency_key.trim().is_empty()
}}
"#
        ),
    );
    // LEX target: carries the SYN marker so the deterministic provider
    // mis-axes it; the exact identifier keeps it findable lexically.
    write(
        "src/embed_admission.rs",
        &format!(
            r#"//! Admission control for queued embedding work. {SYN_MARKER}
pub const EMBED_ADMISSION_BACKLOG_CEILING: usize = 64;
"#
        ),
    );
    for index in 0..3 {
        write(
            &format!("src/lex_decoy_{index}.rs"),
            &format!(
                r#"//! Queued work accounting decoy {index}. {LEX_MARKER}
pub fn decoy_queue_window_{index}() -> u32 {{ {index} }}
"#
            ),
        );
    }
    for index in 0..4 {
        write(
            &format!("src/filler_{index:02}.rs"),
            &format!(
                "//! Unrelated fixture module {index:02}.\npub fn filler_{index:02}_helper() -> u32 {{ {index} }}\n"
            ),
        );
    }
}

async fn wait_until_ready(session: &a3s_code_core::AgentSession) -> WorkspaceRetrievalStatus {
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            let status = session.workspace_retrieval_status();
            match status.phase {
                WorkspaceRetrievalPhase::Ready | WorkspaceRetrievalPhase::Degraded => break status,
                WorkspaceRetrievalPhase::Building => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                phase => panic!("unexpected retrieval phase while building: {phase:?}"),
            }
        }
    })
    .await
    .expect("workspace retrieval did not become ready")
}

#[derive(Debug)]
struct TurnOutcome {
    final_text: String,
    result_paths: Vec<String>,
    total_tokens: usize,
    elapsed_ms: u128,
}

async fn run_turn(
    session: &a3s_code_core::AgentSession,
    task: DisagreementTask,
    variant: Variant,
) -> TurnOutcome {
    let prompt = format!(
        "Inspect the search tool schema. Make exactly one search call and no other tool call. \
         Use query exactly: {query}. Set path to '.', include to '*.rs', limit to 5, and mode \
         to '{mode}'. After the result, reply with only the answer as a single code identifier \
         (a function or constant name — never a file path) that appears in the tool result \
         evidence and directly answers the query (no prose, no punctuation), or NOT_FOUND \
         when the result does not contain the answer.",
        query = task.query,
        mode = variant.mode(),
    );
    let started = Instant::now();
    let (mut events, worker) = session
        .stream(&prompt, None)
        .await
        .expect("start model turn");
    let mut starts = HashMap::<String, String>::new();
    let mut tool_names = Vec::new();
    let mut search_metadata = Value::Null;
    let (final_text, usage) = match tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            match events
                .recv()
                .await
                .expect("model event stream ended before completion")
            {
                AgentEvent::ToolExecutionStart { id, name, .. } => {
                    starts.insert(id, name);
                }
                AgentEvent::ToolEnd {
                    id, name, metadata, ..
                } => {
                    assert_eq!(
                        starts.remove(&id).as_deref(),
                        Some(name.as_str()),
                        "tool start/end names diverged"
                    );
                    tool_names.push(name.clone());
                    if name == "search" {
                        search_metadata = metadata.unwrap_or(Value::Null);
                    }
                }
                AgentEvent::End { text, usage, .. } => break (text, usage),
                AgentEvent::Error { message } => panic!("model turn failed: {message}"),
                AgentEvent::ConfirmationRequired { tool_name, .. } => {
                    panic!("unexpected confirmation for {tool_name}")
                }
                _ => {}
            }
        }
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            let _ = session
                .cancel_and_settle(Duration::from_secs(5), Duration::from_secs(5))
                .await;
            let _ = tokio::time::timeout(Duration::from_secs(10), worker).await;
            panic!("model disagreement turn timed out");
        }
    };
    worker.await.expect("model stream worker joins");
    assert!(starts.is_empty(), "tool starts without terminal events");
    assert_eq!(tool_names, ["search"], "exactly one search call expected");
    let mode = search_metadata["mode"].as_str().unwrap_or_default();
    assert_eq!(mode, variant.mode(), "model must use the pinned mode");
    let result_paths = search_metadata["results"]
        .as_array()
        .map(|results| {
            results
                .iter()
                .filter_map(|result| result["path"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    TurnOutcome {
        final_text,
        result_paths,
        total_tokens: usage.total_tokens,
        elapsed_ms: started.elapsed().as_millis(),
    }
}

struct MatrixCell {
    task: DisagreementTask,
    variant: Variant,
    expect_hit: bool,
}

/// The disagreement oracle: which single channel can serve each task.
fn expected_hit(task: &DisagreementTask, variant: Variant) -> bool {
    let is_syn = task.name == TASK_SYN.name;
    match variant {
        // SYN: only the semantic axis connects query and target.
        Variant::Semantic => is_syn,
        // LEX: only lexical tokens connect query and target.
        Variant::Bm25 => !is_syn,
        // Hybrid fuses every channel, so both tasks resolve.
        Variant::Hybrid => true,
    }
}

fn matrix() -> Vec<MatrixCell> {
    let mut cells = Vec::new();
    for task in TASKS {
        for variant in [Variant::Semantic, Variant::Bm25, Variant::Hybrid] {
            cells.push(MatrixCell {
                task,
                variant,
                expect_hit: expected_hit(&task, variant),
            });
        }
    }
    cells
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_model_hybrid_resolves_channel_disagreement() {
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let model = config
        .default_model
        .clone()
        .expect("pinned Layer C config must declare default_model");
    config.default_model = Some(model.clone());
    let agent = Agent::from_config(config)
        .await
        .expect("create agent from configured model");

    let workspace = tempfile::tempdir().expect("create fixture workspace");
    write_fixture(workspace.path());

    let provider = Arc::new(DisagreementEmbeddingProvider);
    let query_requests = Arc::new(AtomicUsize::new(0));
    let mut summary = String::new();
    for cell in matrix() {
        let mut permissions = PermissionPolicy::new().allow_all(&["search(*)"]);
        permissions.default_decision = PermissionDecision::Deny;
        let retrieval =
            WorkspaceRetrievalOptions::new(Arc::clone(&provider) as Arc<dyn EmbeddingProvider>);
        let options = SessionOptions::new()
            .with_session_id(format!(
                "hybrid-disagreement-{}-{}",
                cell.task.name,
                cell.variant.label()
            ))
            .with_model(model.clone())
            .with_permission_policy(permissions)
            .with_auto_delegation_enabled(false)
            .with_manual_delegation_enabled(false)
            .with_temperature(0.0)
            .with_max_tool_rounds(2)
            .with_prompt_slots(SystemPromptSlots::default().with_guidelines(TEST_GUIDELINES))
            .with_workspace_retrieval(retrieval);
        let session = agent
            .session_async(workspace.path().display().to_string(), Some(options))
            .await
            .expect("create disagreement session");
        let readiness = wait_until_ready(&session).await;
        assert_eq!(
            readiness.phase,
            WorkspaceRetrievalPhase::Ready,
            "semantic projection must be ready: {readiness:?}"
        );
        let outcome = run_turn(&session, cell.task, cell.variant).await;
        let evidence_hit = outcome
            .result_paths
            .iter()
            .any(|path| path.ends_with(cell.task.expected_path));
        let answer_hit = outcome.final_text.contains(cell.task.expected_identifier);
        if cell.expect_hit {
            assert!(
                evidence_hit,
                "{}/{}: tool evidence must contain {}: {:?}",
                cell.task.name,
                cell.variant.label(),
                cell.task.expected_path,
                outcome.result_paths
            );
            assert!(
                answer_hit,
                "{}/{}: model answer must be grounded in evidence: {:?}",
                cell.task.name,
                cell.variant.label(),
                outcome.final_text
            );
        } else {
            assert!(
                !evidence_hit,
                "{}/{}: the losing channel must not surface {}: {:?}",
                cell.task.name,
                cell.variant.label(),
                cell.task.expected_path,
                outcome.result_paths
            );
            assert!(
                !answer_hit,
                "{}/{}: model must not echo an identifier without evidence: {:?}",
                cell.task.name,
                cell.variant.label(),
                outcome.final_text
            );
        }
        query_requests.fetch_add(1, Ordering::Relaxed);
        summary.push_str(&format!(
            "{}/{}={}({}ms,{}tok) ",
            cell.task.name,
            cell.variant.label(),
            if cell.expect_hit { "hit" } else { "miss" },
            outcome.elapsed_ms,
            outcome.total_tokens
        ));
        session.close().await;
    }

    println!(
        "workspace-hybrid-disagreement model={} matrix=[{summary}] turns={} elapsed_ms={} result=pass",
        model,
        query_requests.load(Ordering::Relaxed),
        started.elapsed().as_millis()
    );
}
