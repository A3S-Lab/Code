//! Hermetic twin of `tests/test_long_horizon_capabilities_real_llm.rs`:
//! the same choreography (plan → 3-call parallel batch → failing verify →
//! fix → passing verify → sentinel answer) with a scripted client, pinning
//! the HARNESS side of every oracle — the update_plan call is admitted,
//! one decision folds into call + also (the batch shape), a failing tool
//! result does not end the run, and the success cycle completes.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const TWIN_SENTINEL: &str = "LH4_TWIN_OK_51e9";

struct LongHorizonTwinClient {
    calls: AtomicUsize,
}

fn tool_use(id: &str, name: &str, input: serde_json::Value) -> crate::llm::ContentBlock {
    crate::llm::ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
    }
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for LongHorizonTwinClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let message = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => Message {
                role: "assistant".into(),
                content: vec![tool_use(
                    "lh-1",
                    "update_plan",
                    serde_json::json!({ "plan": [
                        { "step": "inspect", "status": "in_progress" },
                        { "step": "fix", "status": "pending" },
                        { "step": "verify", "status": "pending" },
                    ]}),
                )],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            },
            // One decision, three disjoint reads: the batch shape.
            1 => Message {
                role: "assistant".into(),
                content: vec![
                    tool_use("lh-2", "read", serde_json::json!({ "path": "data/a.mjs" })),
                    tool_use("lh-3", "read", serde_json::json!({ "path": "data/b.mjs" })),
                    tool_use("lh-4", "read", serde_json::json!({ "path": "data/c.mjs" })),
                ],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            },
            2 => Message {
                role: "assistant".into(),
                content: vec![tool_use(
                    "lh-5",
                    "bash",
                    serde_json::json!({ "command": "node test.mjs" }),
                )],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            },
            3 => Message {
                role: "assistant".into(),
                content: vec![tool_use(
                    "lh-6",
                    "edit",
                    serde_json::json!({
                        "file_path": "src/impl.mjs",
                        "old_string": "return base * a * b;",
                        "new_string": "return base + a + b + c;",
                    }),
                )],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            },
            4 => Message {
                role: "assistant".into(),
                content: vec![tool_use(
                    "lh-7",
                    "bash",
                    serde_json::json!({ "command": "node test.mjs" }),
                )],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            },
            _ => Message::assistant(TWIN_SENTINEL),
        };
        Ok(LlmResponse {
            message,
            usage: TokenUsage::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("streaming not used by this twin")
    }
}

#[tokio::test]
async fn plan_batch_and_recovery_choreography_survives_the_fact_spine() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, value) in [("a", 2), ("b", 3), ("c", 5)] {
        std::fs::write(
            root.join(format!("data/{name}.mjs")),
            format!("export const {} = {value};\n", name.to_ascii_uppercase()),
        )
        .unwrap();
    }
    std::fs::write(
        root.join("src/impl.mjs"),
        "export function shipCost(base, a, b, c) { return base * a * b; }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("test.mjs"),
        "import assert from 'node:assert/strict';\n\
         import { shipCost } from './src/impl.mjs';\n\
         import { A } from './data/a.mjs';\n\
         import { B } from './data/b.mjs';\n\
         import { C } from './data/c.mjs';\n\
         assert.equal(shipCost(10, A, B, C), 20, 'LH4_TWIN_WRONG_SUM');\n\
         assert.equal(shipCost(0, 0, 0, 0), 0);\n\
         assert.equal(shipCost(-1, 2, -3, 4), 2);\n\
         console.log('LH4_TWIN_VERIFIED');\n",
    )
    .unwrap();

    let client = Arc::new(LongHorizonTwinClient {
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("lh4-capabilities-twin")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false)
                .with_verification_contract(
                    crate::verification::VerificationContractV1::new(
                        "node-suite",
                        "node test.mjs",
                        ["src/impl.mjs"],
                    )
                    .unwrap(),
                ),
        )
        .unwrap();

    let result = session
        .send("Plan, read in parallel, verify, fix, verify again.", None)
        .await
        .expect("fact turn");
    assert!(matches!(
        result.completion,
        crate::harness_loop::CompletionTerminal::Verified { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(root.join("src/impl.mjs")).unwrap(),
        "export function shipCost(base, a, b, c) { return base + a + b + c; }\n"
    );

    let facts = crate::fact_control::read_workspace_facts(
        &root,
        &crate::fact_control::thread_for_session("lh4-capabilities-twin"),
    )
    .unwrap();

    // PLAN: the update_plan call folded as a model.turn.
    let turns: Vec<&a3s_effect::Fact> = facts
        .iter()
        .filter(|fact| fact.kind == "model.turn")
        .collect();
    assert!(
        turns
            .iter()
            .any(|fact| fact.payload.to_string().contains("update_plan")),
        "update_plan call recorded"
    );

    // PAR: one decision carried call + also (three reads).
    let batch_turn = turns
        .iter()
        .find(|fact| fact.payload.to_string().contains("\"also\""))
        .expect("batch decision recorded");
    let rendered = batch_turn.payload.to_string();
    assert!(
        rendered.matches("\"id\"").count() >= 3,
        "the batch carries three calls; got: {rendered}"
    );

    // The batch produced three tool.result facts — every call settled.
    let results = facts
        .iter()
        .filter(|fact| fact.kind == "tool.result")
        .count();
    assert!(
        results >= 6,
        "plan + 3 batch + 2 bash results; got {results}"
    );

    let outputs: Vec<&str> = facts
        .iter()
        .filter(|fact| fact.kind == "tool.result")
        .filter_map(|fact| {
            fact.payload
                .get("output")
                .and_then(serde_json::Value::as_str)
        })
        .collect();
    let failed = outputs
        .iter()
        .position(|output| output.contains("LH4_TWIN_WRONG_SUM"))
        .expect("the original suite fails on the actual broken implementation");
    let passed = outputs
        .iter()
        .position(|output| output.contains("LH4_TWIN_VERIFIED"))
        .expect("the original suite passes after the actual edit");
    assert!(
        failed < passed,
        "failed verification must precede the repair proof"
    );
}

mod plan_heavy;

#[tokio::test]
async fn plan_management_does_not_exhaust_the_read_only_ceiling() {
    plan_heavy::run_plan_heavy_choreography().await;
}

// --- Completion-gate twins: the declared-suite contract ---------------------
//
// "Run the project's test suite" is the most natural verification a coding
// agent performs, but a suite run names no path, so the mutation gate cannot
// bind it by itself. The host closes that gap by declaring a
// `VerificationContractV1` (command + covered paths). These twins pin the
// gate side on the fact spine with a REAL bash execution — the suite must
// actually run and pass for the contract to bind.

const GATE_CONTRACT_SENTINEL: &str = "LH4_GATE_OK_e41b";

/// Seed a workspace whose suite passes exactly when `src/impl.mjs` says
/// `1 + 2`. Both gate twins edit the broken file with a REAL edit-tool call
/// (`old_string`/`new_string` — the args the tool schema demands).
fn seed_gate_workspace(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("src/impl.mjs"), "export const V = 1 * 2;\n").unwrap();
    std::fs::write(root.join("data/a.mjs"), "export const A = 5;\n").unwrap();
    std::fs::write(
        root.join("test.mjs"),
        format!(
            "import assert from \"node:assert/strict\";\n\
             import {{ V }} from \"./src/impl.mjs\";\n\
             assert.equal(V, 3);\n\
             console.log(\"{GATE_CONTRACT_SENTINEL}\");\n"
        ),
    )
    .unwrap();
}

/// Step `extra` inserts the uncovered `data/a.mjs` mutation BEFORE the
/// suite run — a mutation made after the passing run would change the
/// digest and invalidate the binding for a different reason. Here the
/// digest the gate sees is exactly the one the passing suite covered.
fn gate_client_step(step: usize, extra: bool) -> Option<Message> {
    let content = match (step, extra) {
        (0, _) => vec![tool_use(
            "gate-edit-impl",
            "edit",
            serde_json::json!({
                "file_path": "src/impl.mjs",
                "old_string": "1 * 2",
                "new_string": "1 + 2",
            }),
        )],
        (1, true) => vec![tool_use(
            "gate-edit-data",
            "edit",
            serde_json::json!({
                "file_path": "data/a.mjs",
                "old_string": "5",
                "new_string": "6",
            }),
        )],
        (1, false) | (2, true) => vec![tool_use(
            "gate-run-suite",
            "bash",
            serde_json::json!({ "command": "node test.mjs" }),
        )],
        _ => return None,
    };
    Some(Message {
        role: "assistant".into(),
        content,
        reasoning_content: None,
        transcript_text: None,
        transcript_visibility: crate::llm::TranscriptVisibility::Wire,
    })
}

/// Fixes `src/impl.mjs`, then runs the declared suite once (exit 0). The
/// verified tool result settles the fact completion without another call.
struct ContractGateClient {
    calls: AtomicUsize,
    extra_data_edit: bool,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for ContractGateClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let step = self.calls.fetch_add(1, Ordering::SeqCst);
        let message = match gate_client_step(step, self.extra_data_edit) {
            Some(message) => message,
            None => Message::assistant(GATE_CONTRACT_SENTINEL),
        };
        Ok(LlmResponse {
            message,
            usage: TokenUsage::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("streaming not used by this twin")
    }
}

#[tokio::test]
async fn declared_suite_contract_binds_a_passing_run_and_completes_verified() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    seed_gate_workspace(&root);

    let client = Arc::new(ContractGateClient {
        calls: AtomicUsize::new(0),
        extra_data_edit: false,
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("lh4-gate-contract")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false)
                .with_verification_contract(
                    crate::verification::VerificationContractV1::new(
                        "node-suite",
                        "node test.mjs",
                        ["src/impl.mjs"],
                    )
                    .unwrap(),
                ),
        )
        .unwrap();

    let result = session
        .send("Fix the file, run the suite, stop.", None)
        .await
        .expect("a covered, passing suite run completes the gate");

    // The verified bash result settles the completion through the pinned
    // fact-path short-circuit: the answer is the literal `completed` and
    // the fact log's LAST model turn is that text — the engine's stripped
    // tools answer phase may still run, so the provider-call count is not
    // the oracle here; the settled fact shape is.
    assert_eq!(result.text, "completed");
    let facts = crate::fact_control::read_workspace_facts(
        &root,
        &crate::fact_control::thread_for_session("lh4-gate-contract"),
    )
    .unwrap();
    let last_model_turn = facts
        .iter()
        .rev()
        .find(|fact| fact.kind == "model.turn")
        .map(|fact| fact.payload.to_string())
        .unwrap_or_default();
    assert!(
        last_model_turn.contains("\"kind\":\"text\"") && last_model_turn.contains("completed"),
        "the run must settle on the verified-text turn; got: {last_model_turn}"
    );
    // The suite really ran on the record: the sentinel it prints exists
    // nowhere in the prompt's data until the passing test emits it.
    assert!(
        facts.iter().any(|fact| fact.kind == "tool.result"
            && fact
                .payload
                .get("output")
                .and_then(|value| value.as_str())
                .is_some_and(|output| output.contains(GATE_CONTRACT_SENTINEL))),
        "the passing suite run must be on the fact log"
    );
    assert!(
        matches!(
            result.completion,
            crate::harness_loop::CompletionTerminal::Verified { .. }
        ),
        "the contract run must complete Verified; got {:?}",
        result.completion
    );
    assert!(
        result
            .verification_reports
            .iter()
            .any(|report| report.subject == "shell:contract:node-suite"
                && report.effect_digest.is_some()
                && report.checks.iter().any(|check| check.required
                    && matches!(
                        check.status,
                        crate::verification::VerificationStatus::Passed
                    ))),
        "the bound contract report must be on the result; got {:?}",
        result.verification_reports
    );
    assert!(
        std::fs::read_to_string(root.join("src/impl.mjs"))
            .unwrap()
            .contains("1 + 2"),
        "the fix landed on disk"
    );
}

#[tokio::test]
async fn contract_does_not_bind_a_suite_that_covers_only_part_of_the_mutation() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    seed_gate_workspace(&root);

    // The suite still passes (the impl fix is real), but the client ALSO
    // mutates data/a.mjs, which the contract does not cover. A partial
    // suite is not verification of the rest: the gate must stay shut.
    let client = Arc::new(ContractGateClient {
        calls: AtomicUsize::new(0),
        extra_data_edit: true,
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("lh4-gate-partial")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false)
                .with_verification_contract(
                    crate::verification::VerificationContractV1::new(
                        "node-suite",
                        "node test.mjs",
                        ["src/impl.mjs"],
                    )
                    .unwrap(),
                ),
        )
        .unwrap();

    let error = session
        .send("Fix both files, run the suite, stop.", None)
        .await
        .expect_err("an uncovered mutation must not complete as success");
    let message = format!("{error:#}");
    assert!(
        message.contains("no bound Passed verification"),
        "the refusal must name the missing bound verification; got: {message}"
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 4);
}

/// The default posture is unchanged: with NO contract declared, a bare
/// `node test.mjs` still does not bind. This is the hermetic pin of the
/// live defect the contract feature exists to close.
#[tokio::test]
async fn without_a_contract_a_bare_suite_run_still_does_not_bind() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    seed_gate_workspace(&root);

    let client = Arc::new(ContractGateClient {
        calls: AtomicUsize::new(0),
        extra_data_edit: false,
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("lh4-gate-bare")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false),
        )
        .unwrap();

    let error = session
        .send("Fix the file, run the suite, stop.", None)
        .await
        .expect_err("a bare suite run must not bind without a host contract");
    let message = format!("{error:#}");
    assert!(
        message.contains("no bound Passed verification"),
        "the refusal must name the missing bound verification; got: {message}"
    );
}

// --- Recovery twin: crash-mid-long-task → checkpoint resume → Verified -----
//
// A long task interrupted between tool rounds must not lose its mutation
// accountability: the fact spine rebuilds the conversation from the workspace
// log, but the mutation ledger is process-local state. The checkpoint now
// carries the ledger, and the recovery spawn seeds it onto the resumed
// surface — so the resumed run's completion gate still sees the whole task's
// effect and the declared-suite contract binds the FULL effect, not just the
// post-resume fragment.
//
// The crash is simulated the only way a hard crash can be in-process: the
// scripted client parks INSIDE the second model call, so the source run
// never settles (no graceful-interrupt cleanup, checkpoint intact), and the
// recovery spawn races nothing.

struct RecoveryClient {
    calls: AtomicUsize,
    /// Parks the second model call until released, holding the source run
    /// mid-task with its checkpoint committed. Taken on first use.
    park_gate: std::sync::Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// Written the instant the park is entered, so a test can interrupt
    /// deterministically instead of racing the source's second call.
    parked_marker: Option<std::path::PathBuf>,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for RecoveryClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        // Index 1 is the source run's next decision: park inside the model
        // call — the in-process shape of a hard crash between tool rounds.
        // The RECOVERY run's first decision is index 2: the suite run.
        if index == 1 {
            let gate = self
                .park_gate
                .lock()
                .unwrap()
                .take()
                .expect("park gate present exactly once");
            if let Some(marker) = &self.parked_marker {
                let _ = std::fs::write(marker, b"parked");
            }
            gate.notified().await;
        }
        let message = match index {
            2 => gate_client_step(1, false).expect("suite decision"),
            0 => gate_client_step(0, false).expect("edit decision"),
            _ => Message::assistant(GATE_CONTRACT_SENTINEL),
        };
        Ok(LlmResponse {
            message,
            usage: TokenUsage::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("streaming not used by this twin")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_mid_long_task_resumes_its_mutation_ledger_and_completes_verified() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    seed_gate_workspace(&root);
    let store_dir = tempfile::tempdir().unwrap();

    let marker_dir = tempfile::tempdir().unwrap();
    let parked_marker = marker_dir.path().join("parked");
    let gate = Arc::new(tokio::sync::Notify::new());
    let client = Arc::new(RecoveryClient {
        calls: AtomicUsize::new(0),
        park_gate: std::sync::Mutex::new(Some(Arc::clone(&gate))),
        parked_marker: Some(parked_marker.clone()),
    });
    let source_run = "lh4-rec-src";
    let resumed_run = "lh4-rec-dst";
    // The source "process" runs on its own runtime. Dropping that runtime
    // drops every task it owns — the parked worker and the guardian holding
    // its Run lease — without any settlement: the in-process shape of a hard
    // crash, which releases the run-owner lock the way process exit does.
    let (src_root, src_store, src_client, src_marker) = (
        root.clone(),
        store_dir.path().to_path_buf(),
        Arc::clone(&client),
        parked_marker,
    );
    tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let agent = Agent::from_config(test_config()).await.unwrap();
                // The file session store needs the async build path; the scripted
                // client rides in through the same session option `build_session` uses.
                let session = agent
                    .session_async(
                        src_root.to_string_lossy().into_owned(),
                        Some(
                            SessionOptions::new()
                                .with_session_id("lh4-recovery")
                                .with_llm_client(
                                    Arc::clone(&src_client) as Arc<dyn crate::llm::LlmClient>
                                )
                                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                                .with_auto_delegation_enabled(false)
                                .with_manual_delegation_enabled(false)
                                .with_file_session_store(src_store.clone())
                                .with_verification_contract(
                                    crate::verification::VerificationContractV1::new(
                                        "node-suite",
                                        "node test.mjs",
                                        ["src/impl.mjs"],
                                    )
                                    .unwrap(),
                                ),
                        ),
                    )
                    .await
                    .unwrap();

                let spawned = session
                    .spawn_run_with_id(source_run, "Fix the file, then run the suite.")
                    .await
                    .expect("spawn source run");
                let _source_worker = match spawned {
                    crate::AgentRunSpawn::Started { worker, .. } => worker,
                    crate::AgentRunSpawn::Replayed { .. } => panic!("fresh run id must not replay"),
                };

                // The edit settles: the fix is on disk and the per-tool-round checkpoint
                // carries the durable mutation ledger. The source run is now parked
                // inside its next model call — the in-process shape of a hard crash.
                tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    while !std::fs::read_to_string(src_root.join("src/impl.mjs"))
                        .unwrap_or_default()
                        .contains("1 + 2")
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("the edit never landed");
                // Poll the SAME store API the recovery will use — no file-layout
                // guessing, and the checkpoint is loaded (not merely present).
                let poll_store = crate::store::FileSessionStore::new(&src_store)
                    .await
                    .expect("open store for polling");
                tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    loop {
                        let checkpoint = poll_store
                            .load_loop_checkpoint(source_run)
                            .await
                            .expect("checkpoint load");
                        if checkpoint.is_some_and(|checkpoint| {
                            checkpoint.convergence.mutations
                                != crate::harness_loop::MutationLedger::default()
                        }) {
                            return;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("the checkpoint with the mutation ledger never committed");
                drop(poll_store);
                // Crash only once the source is parked inside its next model
                // call, so the recovery run's first decision is the suite run.
                tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    while !src_marker.is_file() {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("the source run never parked");
            });
            drop(runtime);
        })
        .join()
        .expect("source process thread")
    })
    .await
    .unwrap();

    // Crash recovery: admit one exact fresh run id from the checkpoint. A
    // fresh AGENT (its own session registry — the registry state a real crash
    // loses) opens a session over the SAME workspace, session id, and store —
    // exactly what the next process opens.
    let recovered_agent = Agent::from_config(test_config()).await.unwrap();
    let recovered_session = recovered_agent
        .session_async(
            root.to_string_lossy().into_owned(),
            Some(
                SessionOptions::new()
                    .with_session_id("lh4-recovery")
                    .with_llm_client(Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>)
                    .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_file_session_store(store_dir.path().to_path_buf())
                    .with_verification_contract(
                        crate::verification::VerificationContractV1::new(
                            "node-suite",
                            "node test.mjs",
                            ["src/impl.mjs"],
                        )
                        .unwrap(),
                    ),
            ),
        )
        .await
        .unwrap();
    let recovered = recovered_session
        .spawn_recovery_with_run_id(source_run, resumed_run)
        .await
        .expect("spawn recovery run");
    let resumed_worker = match recovered {
        crate::AgentRunSpawn::Started { worker, .. } => worker,
        crate::AgentRunSpawn::Replayed { .. } => panic!("fresh recovery id must not replay"),
    };

    // The resumed run finishes the task: the suite runs, the contract binds
    // the FULL effect (seeded pre-crash edit + resumed verification), and
    // the verified short-circuit answers `completed` without a text turn.
    let status = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            if let Some(snapshot) = recovered_session.run_snapshot(resumed_run).await {
                if matches!(
                    snapshot.status,
                    crate::run::RunStatus::Completed
                        | crate::run::RunStatus::Failed
                        | crate::run::RunStatus::Cancelled
                ) {
                    return snapshot.status;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    let Ok(status) = status else {
        panic!(
            "resumed run never settled; calls={} snapshot={:?} events={:?}",
            client.calls.load(Ordering::SeqCst),
            recovered_session
                .run_snapshot(resumed_run)
                .await
                .map(|s| s.status),
            recovered_session.run_events(resumed_run).await
        );
    };
    assert_eq!(
        status,
        crate::run::RunStatus::Completed,
        "the resumed long task must complete; events: {:?}",
        recovered_session.run_events(resumed_run).await
    );

    // The verified short-circuit: no post-suite text call happened (the
    // counter stops at the suite decision), and the run store carries the
    // `completed` answer. Without the seeded ledger the resumed gate would
    // be blind to the pre-crash edit and the run would drift to Narrative
    // with a third model call.
    // The provider-call count is engine scheduling (the stripped-tools
    // answer phase is optional) and is not an oracle here. The
    // discriminators are the End text above, the Completed status, and the
    // fact log below: without the seeded ledger this run is REFUSED at the
    // gate ("no bound Passed verification") instead of completing.
    let end = recovered_session
        .run_events(resumed_run)
        .await
        .into_iter()
        .find_map(|record| match record.event {
            crate::agent::AgentEvent::End { text, .. } => Some(text),
            _ => None,
        });
    assert_eq!(
        end.as_deref(),
        Some("completed"),
        "the resumed run must settle through the verified completion"
    );

    // The suite really ran on the record, post-resume.
    let facts = crate::fact_control::read_workspace_facts(
        &root,
        &crate::fact_control::thread_for_session("lh4-recovery"),
    )
    .unwrap();
    assert!(
        facts.iter().any(|fact| fact.kind == "tool.result"
            && fact
                .payload
                .get("output")
                .and_then(|value| value.as_str())
                .is_some_and(|output| output.contains(GATE_CONTRACT_SENTINEL))),
        "the resumed suite run must be on the fact log"
    );

    // Drain the resumed worker and close the session so no background task
    // outlives the test.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), resumed_worker).await;
    recovered_session.close().await;
}

// --- Continuation twin: graceful interrupt → next send inherits the debt ---
//
// An interrupt settles the run as Cancelled and clears its checkpoint;
// continuation is the NEXT SEND on the same session. That send's fresh
// surface replays the unsettled mutations from the fact log (everything
// after the last `turn.complete` marker), so its completion gate still
// sees the interrupted run's effect and the declared-suite contract binds
// the whole task — not just the post-interrupt fragment.

#[tokio::test(flavor = "multi_thread")]
async fn interrupted_run_mutations_carry_into_the_next_send_gate() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    seed_gate_workspace(&root);

    let gate = Arc::new(tokio::sync::Notify::new());
    let parked_marker = root.join("parked.marker");
    let client = Arc::new(RecoveryClient {
        calls: AtomicUsize::new(0),
        park_gate: std::sync::Mutex::new(Some(Arc::clone(&gate))),
        parked_marker: Some(parked_marker.clone()),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("lh4-continuation")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false)
                .with_verification_contract(
                    crate::verification::VerificationContractV1::new(
                        "node-suite",
                        "node test.mjs",
                        ["src/impl.mjs"],
                    )
                    .unwrap(),
                ),
        )
        .unwrap();

    // A foreground stream is what makes the session's run-control surface
    // active — the interrupt boundary the continuation seam hangs off.
    let (mut events, source_worker) = session
        .stream("Fix the file, then run the suite.", None)
        .await
        .expect("start interrupted stream");
    tokio::spawn(async move { while events.recv().await.is_some() {} });

    // Hold the run mid-task: the edit landed, the model is parked inside
    // its next decision.
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !std::fs::read_to_string(root.join("src/impl.mjs"))
            .unwrap_or_default()
            .contains("1 + 2")
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the edit never landed");

    // Graceful interrupt: the run settles Cancelled (no checkpoint left
    // behind, no turn.complete marker — the debt stays open).
    // Deterministic hold: the source run is parked INSIDE its next model
    // call. Interrupting before this marker would race the source's second
    // call and shift every later client index.
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !parked_marker.is_file() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the source run never parked");
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Some(snapshot) = session.run_control_snapshot().await {
                if snapshot.turn_id.is_some() {
                    return snapshot;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("no active run-control snapshot");
    let receipt = session
        .interrupt(
            crate::run_control::InterruptRequest::new()
                .with_reason("continuation twin")
                .with_run_id(snapshot.run_id.clone()),
        )
        .await
        .expect("interrupt rejected");
    assert_eq!(
        receipt.state,
        crate::run_control::RunControlReceiptState::Accepted
    );
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while session.run_control_snapshot().await.is_some() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("interrupted stream never settled");

    // The continuation send: the gate must inherit the interrupted run's
    // edit, so the passing suite binds the FULL effect and the run settles
    // through the verified short-circuit.
    let result = session
        .send(
            "Now run exactly `node test.mjs` to verify, then stop.",
            None,
        )
        .await
        .expect("the continuation send must complete");
    assert_eq!(result.text, "completed");
    assert!(
        matches!(
            result.completion,
            crate::harness_loop::CompletionTerminal::Verified { .. }
        ),
        "the continuation must complete Verified over the inherited effect; got {:?}",
        result.completion
    );

    let facts = crate::fact_control::read_workspace_facts(
        &root,
        &crate::fact_control::thread_for_session("lh4-continuation"),
    )
    .unwrap();
    assert!(
        facts
            .iter()
            .any(|fact| fact.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND),
        "a settled send must append the turn.complete marker"
    );
    assert!(
        facts.iter().any(|fact| fact.kind == "tool.result"
            && fact
                .payload
                .get("output")
                .and_then(|value| value.as_str())
                .is_some_and(|output| output.contains(GATE_CONTRACT_SENTINEL))),
        "the continuation's suite run must be on the fact log"
    );

    session.close().await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), source_worker).await;
    let _ = gate; // the parked call was cancelled with the run
}

// --- Debt mechanics: compaction, persistence, accumulation -----------------
//
// The `turn.complete` marker bounds mutation accountability. These twins
// pin the debt's behavior around it: compaction must not forgive it, a
// narrative answer must not wash it, and several unsettled turns
// accumulate into one inherited effect the suite contract settles.

/// The full scripted decision sequence across ALL sends, consumed in
/// order — per-send indices are engine scheduling, so debt twins script
/// the whole conversation up front.
fn debt_script(total: usize, bash_at: Option<usize>, edits: &[serde_json::Value]) -> Vec<Message> {
    let mut steps: Vec<Message> = Vec::new();
    let mut edit_iter = edits.iter();
    for step in 0..total {
        if Some(&step) == bash_at.as_ref() {
            steps.push(gate_client_step(1, false).expect("suite decision"));
        } else if let Some(edit) = edit_iter.next() {
            steps.push(Message {
                role: "assistant".into(),
                content: vec![tool_use(&format!("debt-edit-{step}"), "edit", edit.clone())],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            });
        } else {
            steps.push(Message::assistant(GATE_CONTRACT_SENTINEL));
        }
    }
    steps
}

struct ScriptedDebtClient {
    steps: Vec<Message>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for ScriptedDebtClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        let message = self
            .steps
            .get(index)
            .cloned()
            .unwrap_or_else(|| Message::assistant(GATE_CONTRACT_SENTINEL));
        Ok(LlmResponse {
            message,
            usage: TokenUsage::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("streaming not used by this twin")
    }
}

fn impl_edit() -> serde_json::Value {
    serde_json::json!({
        "file_path": "src/impl.mjs",
        "old_string": "1 * 2",
        "new_string": "1 + 2",
    })
}

fn debt_session(
    agent: &Agent,
    root: &std::path::Path,
    client: Arc<dyn crate::llm::LlmClient>,
    contract: bool,
) -> AgentSession {
    let mut options = SessionOptions::new()
        .with_session_id("lh4-debt")
        .with_planning_mode(crate::prompts::PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false);
    if contract {
        options = options.with_verification_contract(
            crate::verification::VerificationContractV1::new(
                "node-suite",
                "node test.mjs",
                ["src/impl.mjs"],
            )
            .unwrap(),
        );
    }
    agent
        .build_session(root.to_string_lossy().into_owned(), client, &options)
        .unwrap()
}

/// The edit mutates, the model answers in text, the gate refuses: the debt
/// is on the log with no marker. A compaction lands BETWEEN the debt and
/// the settlement send. The append-only log keeps the facts readable, so
/// the next send must still inherit the mutation and complete Verified —
/// compaction is a summary, not an amnesty.
#[tokio::test]
async fn compaction_between_debt_and_settlement_does_not_forgive_it() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    seed_gate_workspace(&root);

    let client = Arc::new(ScriptedDebtClient {
        steps: debt_script(3, Some(2), &[impl_edit()]),
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = debt_session(
        &agent,
        &root,
        Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
        true,
    );

    let first = session
        .send("Fix the file, then stop.", None)
        .await
        .expect_err("an unsettled mutation must refuse the first send");
    assert!(
        format!("{first:#}").contains("no bound Passed verification"),
        "the refusal must name the missing verification: {first:#}"
    );

    // Mid-conversation compaction: a summary lands after the debt.
    {
        let log = session
            .run_admission
            .fact_log_slot()
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        let writer = log.try_writer_fence().unwrap();
        crate::fact_control::append_compaction_done(&log, &writer, "lh4-debt", "summary", None)
            .unwrap();
    }

    let second = session
        .send(
            "Now run exactly `node test.mjs` to verify, then stop.",
            None,
        )
        .await;
    let second = second.expect("compaction must not forgive the debt");
    assert_eq!(second.text, "completed");
    assert!(matches!(
        second.completion,
        crate::harness_loop::CompletionTerminal::Verified { .. }
    ));

    let facts = crate::fact_control::read_workspace_facts(
        &root,
        &crate::fact_control::thread_for_session("lh4-debt"),
    )
    .unwrap();
    let edit = facts
        .iter()
        .position(|fact| fact.kind == "tool.result")
        .expect("the edit result is on the log");
    let compaction = facts
        .iter()
        .position(|fact| fact.kind == "compaction.done")
        .expect("the compaction marker is on the log");
    let complete = facts
        .iter()
        .rposition(|fact| fact.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND)
        .expect("the settled send appended the marker");
    assert!(
        edit < compaction && compaction < complete,
        "the debt precedes the compaction, which precedes the settlement"
    );
    // The scripted floor; the stripped-tools answer phase may add one.
    assert!(client.calls.load(Ordering::SeqCst) >= 3);
}

/// A refused run leaves the debt open, and a text-only answer on the next
/// send cannot wash it: the gate refuses again, no marker is appended, and
/// the debt persists until something actually verifies the effect.
#[tokio::test]
async fn a_narrative_answer_does_not_wash_inherited_debt() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    seed_gate_workspace(&root);

    let client = Arc::new(ScriptedDebtClient {
        steps: debt_script(4, None, &[impl_edit()]),
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    // No contract: nothing can verify the mutation during this twin.
    let session = debt_session(
        &agent,
        &root,
        Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
        false,
    );

    let first = session
        .send("Fix the file, then stop.", None)
        .await
        .expect_err("the first send must refuse");
    assert!(format!("{first:#}").contains("no bound Passed verification"));

    let second = session
        .send("Just tell me what you changed.", None)
        .await
        .expect_err("a text-only answer must not settle inherited debt");
    assert!(
        format!("{second:#}").contains("no bound Passed verification"),
        "the inherited debt must refuse again: {second:#}"
    );

    let facts = crate::fact_control::read_workspace_facts(
        &root,
        &crate::fact_control::thread_for_session("lh4-debt"),
    )
    .unwrap();
    assert!(
        !facts
            .iter()
            .any(|fact| fact.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND),
        "no refused send may close the book"
    );
}

fn open_mutation_digest(error: &impl std::fmt::Display) -> String {
    let text = format!("{error:#}");
    let marker = "workspace mutation ";
    let Some(marker_at) = text.find(marker) else {
        panic!("the gate names the open mutation, got: {text}");
    };
    let start = marker_at + marker.len();
    let digest: String = text[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_hexdigit())
        .collect();
    assert_eq!(
        digest.len(),
        64,
        "effect digest is 64 hex chars, got {digest}"
    );
    digest
}

fn host_passed_report(digest: &str) -> crate::verification::VerificationReport {
    crate::verification::VerificationReport::new(
        "host:recorded",
        vec![crate::verification::VerificationCheck::required(
            "check:host",
            "host",
            "Host bound this effect digest before the next send.",
        )
        .with_status(crate::verification::VerificationStatus::Passed)],
    )
    .with_effect_digest(digest.to_string())
}

/// A refused mutation stays open, and a Passed report for some other digest
/// does not close it. Recording a Passed report bound to the digest the gate
/// named makes the next text-only send complete as Verified.
#[tokio::test]
async fn a_recorded_host_report_closes_the_next_sends_open_digest() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    seed_gate_workspace(&root);

    let client = Arc::new(ScriptedDebtClient {
        steps: debt_script(4, None, &[impl_edit()]),
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = debt_session(
        &agent,
        &root,
        Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
        false,
    );

    let first = session
        .send("Fix the file, then stop.", None)
        .await
        .expect_err("the first send must refuse");
    let digest = open_mutation_digest(&first);
    let foreign = "ab".repeat(32);

    session.record_verification_reports([host_passed_report(&foreign)]);
    let second = session
        .send("Just tell me what you changed.", None)
        .await
        .expect_err("a report for another digest must not close this mutation");
    assert_eq!(
        open_mutation_digest(&second),
        digest,
        "the open effect stays the refused mutation"
    );

    session.record_verification_reports([host_passed_report(&digest)]);
    let third = session
        .send("The host bound that digest. Answer and stop.", None)
        .await
        .expect("a Passed report bound to the open digest must close the next send");
    assert_eq!(third.text, GATE_CONTRACT_SENTINEL);
    assert!(matches!(
        &third.completion,
        crate::harness_loop::CompletionTerminal::Verified { effect_digest }
            if effect_digest == &digest
    ));

    let facts = crate::fact_control::read_workspace_facts(
        &root,
        &crate::fact_control::thread_for_session("lh4-debt"),
    )
    .unwrap();
    assert!(
        facts
            .iter()
            .any(|fact| fact.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND),
        "the closed send must append the turn.complete marker"
    );
    let stored = session.verification_reports();
    assert_eq!(
        stored
            .iter()
            .filter(|report| report.effect_digest.as_deref() == Some(digest.as_str()))
            .count(),
        1,
        "a successful gate must not append the seeded host report again"
    );
    assert_eq!(stored.len(), 2);

    session.close().await;
}

/// Debts accumulate across several unsettled turns; one passing suite run
/// on a contract that covers them all settles the whole accumulated effect.
#[tokio::test]
async fn debts_from_multiple_unsettled_turns_settle_together() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    seed_gate_workspace(&root);

    std::fs::write(
        root.join("src/notes.mjs"),
        "export const note = 1;
",
    )
    .unwrap();
    let client = Arc::new(ScriptedDebtClient {
        steps: debt_script(
            6,
            Some(4),
            &[
                impl_edit(),
                serde_json::json!({
                    "file_path": "src/notes.mjs",
                    "old_string": "1",
                    "new_string": "2",
                }),
            ],
        ),
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    // The contract covers the whole src tree: both edits land under it.
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("lh4-debt")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false)
                .with_verification_contract(
                    crate::verification::VerificationContractV1::new(
                        "node-suite",
                        "node test.mjs",
                        ["src/**"],
                    )
                    .unwrap(),
                ),
        )
        .unwrap();

    // Turn 1: edit impl.mjs, answer in text → refused.
    session
        .send("Fix the file, then stop.", None)
        .await
        .expect_err("turn 1 must refuse");
    // Turn 2: another src mutation (the client's index-2 step is the suite
    // run, so turn 2 gets a text answer) → refused again.
    session
        .send("Also update src/notes.mjs with a comment.", None)
        .await
        .expect_err("turn 2 must refuse");

    // Turn 3: the suite runs and covers the accumulated effect.
    let third = session
        .send(
            "Now run exactly `node test.mjs` to verify, then stop.",
            None,
        )
        .await
        .expect("the accumulated debt settles with one verified suite run");
    assert!(matches!(
        third.completion,
        crate::harness_loop::CompletionTerminal::Verified { .. }
    ));

    let facts = crate::fact_control::read_workspace_facts(
        &root,
        &crate::fact_control::thread_for_session("lh4-debt"),
    )
    .unwrap();
    let edit_results = facts
        .iter()
        .filter(|fact| fact.kind == "tool.result")
        .count();
    assert!(
        edit_results >= 3,
        "two edits plus the suite run must be on the log; got {edit_results}"
    );
}

// ---------------------------------------------------------------------------
// Turn-finality streaming pins: the settled End marker must carry the real
// verification summary, and a final response that produced no stream deltas
// must be re-sent instead of being lost to both de-dup guards at once.
// ---------------------------------------------------------------------------

/// Streams on the first response only: response 0 emits a text delta plus a
/// bash tool call; response 1 (the final answer) emits NO deltas — the
/// non-streaming-fallback shape that used to lose the final text on both
/// de-dup rails (core suppressed the re-send, the host dropped the End text).
struct UnstreamedFinalTextClient {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for UnstreamedFinalTextClient {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("streaming-only twin")
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[crate::llm::ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let step = self.calls.fetch_add(1, Ordering::SeqCst);
        let response = match step {
            0 => {
                let _ = tx
                    .send(crate::llm::StreamEvent::TextDelta(
                        "first streamed message".into(),
                    ))
                    .await;
                LlmResponse {
                    message: Message {
                        role: "assistant".into(),
                        content: vec![tool_use(
                            "unstreamed-bash",
                            "bash",
                            serde_json::json!({ "command": "echo twin-unstreamed-ok" }),
                        )],
                        reasoning_content: None,
                        transcript_text: None,
                        transcript_visibility: crate::llm::TranscriptVisibility::Wire,
                    },
                    usage: TokenUsage::default(),
                    stop_reason: None,
                    token_logprobs: Vec::new(),
                    meta: None,
                }
            }
            _ => LlmResponse {
                message: Message::assistant("the unstreamed final answer"),
                usage: TokenUsage::default(),
                stop_reason: None,
                token_logprobs: Vec::new(),
                meta: None,
            },
        };
        tokio::spawn(async move {
            let _ = tx.send(crate::llm::StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

/// The settle replay re-sends the final response's text when that response
/// streamed nothing — even though an EARLIER response in the same turn did.
#[tokio::test]
async fn unstreamed_final_response_text_is_resent_at_settle() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    std::fs::create_dir_all(&root).unwrap();
    // A process-free tool round isolates the stream/settle replay protocol.
    let sandbox = Arc::new(
        crate::tools::builtin::background::test_backend::DeterministicBackgroundSandbox::new(
            root.clone(),
            "protocol-unstreamed",
            0,
            false,
        ),
    );

    let client = Arc::new(UnstreamedFinalTextClient {
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("lh4-unstreamed-final")
                .with_sandbox_handle(Arc::clone(&sandbox) as Arc<dyn crate::sandbox::BashSandbox>)
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false),
        )
        .unwrap();

    let (mut events, _worker) = session
        .stream("Do the work, then answer.", None)
        .await
        .unwrap();
    let mut deltas: Vec<String> = Vec::new();
    let mut end: Option<crate::agent::AgentEvent> = None;
    while let Some(event) = events.recv().await {
        match &event {
            crate::agent::AgentEvent::TextDelta { text } => deltas.push(text.clone()),
            crate::agent::AgentEvent::End { .. } => end = Some(event),
            _ => {}
        }
    }

    assert_eq!(
        deltas.last().map(String::as_str),
        Some("the unstreamed final answer"),
        "the settle replay must re-send the unstreamed final text; deltas: {deltas:?}"
    );
    assert_eq!(
        deltas.len(),
        2,
        "exactly the streamed message plus the re-sent final text: {deltas:?}"
    );
    let end = end.expect("the turn must end with an End event");
    assert_eq!(sandbox.calls(), 1);
    assert!(sandbox.was_released());
    match end {
        crate::agent::AgentEvent::End { text, .. } => {
            assert_eq!(text, "the unstreamed final answer");
        }
        other => panic!("expected End, got {other:?}"),
    }
}

/// Always calls a tool: with a one-round budget the second call must be
/// denied by the machine, and the settled End must NAME the stop.
struct BudgetExhaustingClient;

#[async_trait::async_trait]
impl crate::llm::LlmClient for BudgetExhaustingClient {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("streaming-only twin")
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[crate::llm::ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            let _ = tx
                .send(crate::llm::StreamEvent::Done(LlmResponse {
                    message: Message {
                        role: "assistant".into(),
                        content: vec![tool_use(
                            "budget-bash",
                            "bash",
                            serde_json::json!({ "command": "echo round" }),
                        )],
                        reasoning_content: None,
                        transcript_text: None,
                        transcript_visibility: crate::llm::TranscriptVisibility::Wire,
                    },
                    usage: TokenUsage::default(),
                    stop_reason: None,
                    token_logprobs: Vec::new(),
                    meta: None,
                }))
                .await;
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn budget_denial_names_the_forced_stop() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    // The budget oracle is admission of one tool round and denial of the next;
    // fixed process-free output is not native execution evidence.
    let sandbox = Arc::new(
        crate::tools::builtin::background::test_backend::DeterministicBackgroundSandbox::new(
            root.clone(),
            "protocol-budget-round",
            0,
            false,
        ),
    );

    let client = Arc::new(BudgetExhaustingClient);
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("lh4-budget-forced-stop")
                .with_sandbox_handle(Arc::clone(&sandbox) as Arc<dyn crate::sandbox::BashSandbox>)
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false)
                .with_max_tool_rounds(1),
        )
        .unwrap();

    let (mut events, _worker) = session.stream("Keep working.", None).await.unwrap();
    let mut forced = None;
    while let Some(event) = events.recv().await {
        if let crate::agent::AgentEvent::End { forced_stop, .. } = event {
            forced = forced_stop;
        }
    }
    assert_eq!(
        forced,
        Some(crate::harness_loop::ForcedStop::ToolBudget),
        "a budget-denied turn must name the stop on its End event"
    );
    assert_eq!(sandbox.calls(), 1, "the second tool round must not execute");
    assert!(sandbox.was_released());
}

/// Final response arrives length-cut (stop_reason `length`, no tool call):
/// the settled End must name the truncation.
struct LengthTruncatedClient;

#[async_trait::async_trait]
impl crate::llm::LlmClient for LengthTruncatedClient {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("streaming-only twin")
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[crate::llm::ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            let _ = tx
                .send(crate::llm::StreamEvent::Done(LlmResponse {
                    message: Message::assistant("the answer is cut mid sent"),
                    usage: TokenUsage::default(),
                    stop_reason: Some("length".into()),
                    token_logprobs: Vec::new(),
                    meta: None,
                }))
                .await;
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn length_limited_answer_names_the_truncation() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();

    let client = Arc::new(LengthTruncatedClient);
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("lh4-length-truncated")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false),
        )
        .unwrap();

    let (mut events, _worker) = session.stream("Answer fully.", None).await.unwrap();
    let mut forced = None;
    while let Some(event) = events.recv().await {
        if let crate::agent::AgentEvent::End { forced_stop, .. } = event {
            forced = forced_stop;
        }
    }
    assert_eq!(
        forced,
        Some(crate::harness_loop::ForcedStop::ResponseTruncated),
        "a length-cut answer must name the truncation on its End event"
    );
}

/// The contract-gate choreography over the streaming path; the settled End
/// marker must carry the gate's real verification summary (a bound Passed
/// contract report), not an empty one.
struct ContractGateStreamingClient {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for ContractGateStreamingClient {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("streaming-only twin")
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[crate::llm::ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        let step = self.calls.fetch_add(1, Ordering::SeqCst);
        let message = match gate_client_step(step, false) {
            Some(message) => message,
            None => Message::assistant(GATE_CONTRACT_SENTINEL),
        };
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            // No text deltas: the settled reply rides the End marker (plus
            // the settle re-send), which is exactly what this twin pins.
            let _ = tx
                .send(crate::llm::StreamEvent::Done(LlmResponse {
                    message,
                    usage: TokenUsage::default(),
                    stop_reason: None,
                    token_logprobs: Vec::new(),
                    meta: None,
                }))
                .await;
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn settled_end_carries_the_gates_verification_summary() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    seed_gate_workspace(&root);

    let client = Arc::new(ContractGateStreamingClient {
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("lh4-gate-stream-summary")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false)
                .with_verification_contract(
                    crate::verification::VerificationContractV1::new(
                        "node-suite",
                        "node test.mjs",
                        ["src/impl.mjs"],
                    )
                    .unwrap(),
                ),
        )
        .unwrap();

    let (mut events, _worker) = session
        .stream("Fix the file, run the suite, stop.", None)
        .await
        .unwrap();
    let mut end_summary: Option<crate::verification::VerificationSummary> = None;
    let mut end_terminal: Option<crate::harness_loop::CompletionTerminal> = None;
    let mut refused = Vec::new();
    while let Some(event) = events.recv().await {
        let diagnostic = match &event {
            crate::agent::AgentEvent::Error { message } => Some(message.as_str()),
            crate::agent::AgentEvent::ToolEnd {
                output, exit_code, ..
            } if *exit_code != 0 => Some(output.as_str()),
            _ => None,
        };
        if let Some(diagnostic) = diagnostic.filter(|_| refused.len() < 16) {
            refused.push(diagnostic.chars().take(2048).collect::<String>());
        }
        if let crate::agent::AgentEvent::End {
            verification_summary,
            completion_terminal,
            ..
        } = event
        {
            end_summary = Some(*verification_summary);
            end_terminal = Some(completion_terminal);
        }
    }

    if end_summary.is_none() {
        let facts =
            crate::fact_control::read_workspace_facts(&root, "lh4-gate-stream-summary").unwrap();
        for fact in facts
            .iter()
            .filter(|fact| fact.kind == "tool.result")
            .take(8)
        {
            refused.push(format!(
                "tool.result: {}",
                fact.payload
                    .to_string()
                    .chars()
                    .take(2048)
                    .collect::<String>()
            ));
        }
    }
    let summary = end_summary.unwrap_or_else(|| {
        panic!("the verified turn must settle with an End event; observed refusals: {refused:?}")
    });
    assert!(
        matches!(
            end_terminal,
            Some(crate::harness_loop::CompletionTerminal::Verified { .. })
        ),
        "the End event must carry the gate's Verified terminal; got {end_terminal:?}"
    );
    assert_eq!(
        summary.status,
        crate::verification::VerificationStatus::Passed,
        "the End marker must carry the gate's Passed outcome: {summary:?}"
    );
    assert!(
        summary.required_check_count >= 1,
        "the bound contract's required check must be counted: {summary:?}"
    );
}

/// Manual compaction must preserve an original unsettled mutation even when
/// summary prose and the next answer both claim success; no native command is needed.
#[tokio::test]
async fn manual_compaction_cannot_accept_unverified_original_mutation() {
    let workspace = tempfile::tempdir().unwrap();
    seed_gate_workspace(workspace.path());
    let client = Arc::new(ScriptedDebtClient {
        steps: debt_script(2, None, &[impl_edit()]),
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = debt_session(&agent, workspace.path(), client, true);
    let first = session
        .send("Fix the original file, then stop.", None)
        .await
        .unwrap_err();
    assert!(first.to_string().contains("no bound Passed verification"));
    let before = crate::fact_control::read_workspace_facts(workspace.path(), "lh4-debt").unwrap();
    assert!(before
        .iter()
        .any(|fact| fact.kind == "tool.result" && fact.payload["ok"] == true));
    session.compact_conversation(None).await.unwrap();
    let compacted =
        crate::fact_control::read_workspace_facts(workspace.path(), "lh4-debt").unwrap();
    assert_eq!(&compacted[..before.len()], before.as_slice());
    assert_eq!(compacted.len(), before.len() + 1);
    assert_eq!(compacted.last().unwrap().kind, "compaction.done");
    let second = session
        .send("Declare the work complete without running checks.", None)
        .await
        .unwrap_err();
    assert!(second.to_string().contains("no bound Passed verification"));
    let after = crate::fact_control::read_workspace_facts(workspace.path(), "lh4-debt").unwrap();
    assert!(!after
        .iter()
        .any(|fact| fact.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND));
    session.close().await;
    agent.close().await;
}
