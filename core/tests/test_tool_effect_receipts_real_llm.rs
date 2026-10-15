#![cfg(test)]

//! Real-provider process-crash and failed-write receipt qualification.
//!
//! These tests require a pinned live provider; each crash test launches its own
//! child role through parent-owned environment values:
//!
//! ```text
//! A3S_CONFIG_FILE=/absolute/path/config.acl \
//!   cargo test -p a3s-code-core --test test_tool_effect_receipts_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Optional A3S_TEST_TOOL_EFFECT_EVIDENCE_DIR places retained attempt directories
//! under an existing evidence directory. Every attempt retains stdout/stderr,
//! the workspace/log/store, independent counter, and observed usage. No provider
//! configuration or key is copied into these artifacts.

#[path = "tool_effect_receipts_live/mod.rs"]
mod process_windows;
mod support;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::config::{CodeConfig, ModelCost};
use a3s_code_core::fact_control::{read_workspace_facts, thread_for_session};
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::store::{FileSessionStore, SessionStore};
use a3s_code_core::tools::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use a3s_code_core::{Agent, AgentEvent, SessionOptions, SystemPromptSlots};
use a3s_effect::{
    ToolExecutionOutcome, ToolExecutionResultV1, ToolObservationCompleteness, ToolPreparedV1,
};
use serde_json::{json, Value};
use support::layer_c_model::{load_pinned_layer_c_config, pinned_layer_c_model, repo_config_path};

const SESSION: &str = "tool-effect-process-crash-live";
const CHILD_TEST: &str = "real_llm_write_before_result_crash_recovers_unknown_without_reexecution";
const CHILD_ROOT_ENV: &str = "A3S_TEST_TOOL_EFFECT_CHILD_ROOT";
const EVIDENCE_ENV: &str = "A3S_TEST_TOOL_EFFECT_EVIDENCE_DIR";
const WRITE_TOOL: &str = "receipt_write_barrier";
const FAIL_TOOL: &str = "receipt_fail_after_write";
const FILE_NAME: &str = "effect.txt";
const FILE_BYTES: &[u8] = b"actual durable tool effect bytes\n";
const MARKER_BYTES: &[u8] = b"workspace-written-and-synced; tool-has-not-returned\n";
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(420);
const RESUME_TIMEOUT: Duration = Duration::from_secs(180);
const GUIDELINES: &str = "This is an execution qualification. Invoke the one requested tool exactly once with {}, invoke no other tools, never retry it even after an error or Unknown result, and then return a short final answer. A narrative answer never replaces the required invocation.";

#[derive(Clone, Copy)]
enum ToolMode {
    BlockAfterWrite,
    FailAfterWrite,
}

struct ReceiptWriteTool {
    root: PathBuf,
    mode: ToolMode,
}

fn sync_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Capture facts at the named boundary, independently of ignored state dirs.
/// This qualification uses only its fixed prompts, public {} tool arguments,
/// file bytes and frozen binding digests; provider configuration is not copied.
fn retain_fact_snapshot(root: &Path, phase: &str, session: &str, facts: &[a3s_effect::Fact]) {
    let artifacts = root.join("artifacts");
    std::fs::create_dir_all(&artifacts).expect("create retained fact snapshot directory");
    let counter = std::fs::read_to_string(root.join("executions.counter"))
        .ok()
        .map(|value| value.lines().count());
    let file_digest = std::fs::read(root.join("workspace").join(FILE_NAME))
        .ok()
        .map(|bytes| format!("sha256:{}", sha256::digest(bytes)));
    let prepared_status = preparations(facts)
        .into_iter()
        .map(|prepared| {
            json!({
                "schema":prepared.schema, "identity":prepared.identity,
                "schema_and_identity_valid":prepared.validate().is_ok(),
            })
        })
        .collect::<Vec<_>>();
    let terminal_status = receipts(facts)
        .into_iter()
        .map(|receipt| {
            json!({
                "schema":receipt.schema, "identity":receipt.identity,
                "outcome":receipt.outcome, "schema_and_identity_valid":receipt.validate().is_ok(),
            })
        })
        .collect::<Vec<_>>();
    sync_write(
        &artifacts.join(format!("{phase}.json")),
        &serde_json::to_vec_pretty(&json!({
            "schema":"a3s.test.tool-effects-fact-snapshot.v1", "capture_phase":phase,
            "session_id":session, "thread_id":thread_for_session(session),
            "source_run_id":std::fs::read_to_string(root.join("source-run-id")).ok(),
            "executions_observed":counter, "independent_file_digest":file_digest,
            "preparations":prepared_status, "terminals":terminal_status,
            "successful_settlement_markers_observed":facts.iter().filter(|fact| fact.kind == "turn.complete").count(),
            "facts":facts,
        }))
        .unwrap(),
    )
    .expect("retain authoritative fact snapshot at observed boundary");
}

fn append_json(path: &Path, value: &Value) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open retained observation");
    serde_json::to_writer(&mut file, value).expect("encode retained observation");
    file.write_all(b"\n")
        .expect("write retained observation separator");
    file.sync_all().expect("sync retained observation");
}

#[async_trait::async_trait]
impl Tool for ReceiptWriteTool {
    fn name(&self) -> &str {
        match self.mode {
            ToolMode::BlockAfterWrite => WRITE_TOOL,
            ToolMode::FailAfterWrite => FAIL_TOOL,
        }
    }

    fn description(&self) -> &str {
        "Qualification tool: call exactly once with {} to write the required local evidence file. Never retry it."
    }

    fn parameters(&self) -> Value {
        json!({"type":"object", "properties":{}, "additionalProperties":false})
    }

    fn capabilities(&self, _args: &Value) -> ToolCapabilities {
        ToolCapabilities::conservative()
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> anyhow::Result<ToolOutput> {
        anyhow::ensure!(
            args.as_object().is_some_and(|args| args.is_empty()),
            "qualification expects empty tool arguments"
        );
        // The counter is outside the observed workspace and is written by the
        // actual tool body, independently of runner output and log projections.
        let mut counter = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("executions.counter"))?;
        counter.write_all(b"executed\n")?;
        counter.sync_all()?;
        sync_write(&context.workspace.join(FILE_NAME), FILE_BYTES)?;
        match self.mode {
            ToolMode::BlockAfterWrite => {
                let pending_marker = self.root.join("write-marker.pending");
                sync_write(&pending_marker, MARKER_BYTES)?;
                std::fs::rename(pending_marker, self.root.join("write-marker.ready"))?;
                std::future::pending::<anyhow::Result<ToolOutput>>().await
            }
            ToolMode::FailAfterWrite => Ok(ToolOutput::error(
                "qualification failure after the file write",
            )),
        }
    }
}

fn retained_attempt(name: &str) -> PathBuf {
    let parent = std::env::var_os(EVIDENCE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&parent).expect("create retained evidence parent");
    let root = tempfile::Builder::new()
        .prefix(&format!("a3s-tool-effects-{name}-"))
        .tempdir_in(parent)
        .expect("allocate isolated retained attempt")
        .keep();
    std::fs::create_dir(root.join("workspace")).expect("create attempt workspace");
    eprintln!("tool-effects attempt={name} artifacts={}", root.display());
    root
}

fn real_config(root: &Path, stage: &str) -> (CodeConfig, String, ModelCost) {
    let config = load_pinned_layer_c_config();
    let model = pinned_layer_c_model(&config);
    let (provider, model_id) = model.split_once('/').expect("pinned provider/model route");
    let cost = config
        .find_provider(provider)
        .expect("declared pinned provider")
        .models
        .iter()
        .find(|entry| entry.id == model_id)
        .expect("declared pinned model")
        .cost
        .clone();
    sync_write(
        &root.join(format!("model-{stage}.json")),
        &serde_json::to_vec_pretty(&json!({
            "model":model, "configured_cost_per_million_tokens":cost,
            "billing_cost":null, "billing_note":"configured rates are not a provider invoice",
        }))
        .unwrap(),
    )
    .expect("retain non-secret model/rate identity");
    (config, model, cost)
}

fn options(root: &Path, model: &str, session: &str, tool: &str) -> SessionOptions {
    let mut permission = PermissionPolicy::new().allow_all(&[&format!("{tool}(*)")]);
    permission.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id(session)
        .with_model(model)
        .with_file_session_store(root.join("sessions"))
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(permission)
        .with_default_security()
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_continuation(false)
        .with_max_tool_rounds(1)
        .with_temperature(0.0)
        .with_llm_api_timeout(180_000)
        .with_tool_timeout(600_000)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(GUIDELINES))
}

fn retain_usage(root: &Path, stage: &str, event: &AgentEvent, cost: &ModelCost) {
    if let AgentEvent::ModelUsageBound { snapshot } = event {
        // ModelCost defaults missing rates to zero. Zero therefore cannot
        // establish a free call or a qualified cost estimate in this campaign.
        let rates_qualified = cost.input.is_finite()
            && cost.output.is_finite()
            && cost.input > 0.0
            && cost.output > 0.0;
        let nominal = rates_qualified.then(|| {
            (snapshot.reported_prompt_tokens as f64 * cost.input
                + snapshot.reported_completion_tokens as f64 * cost.output)
                / 1_000_000.0
        });
        append_json(
            &root.join(format!("usage-{stage}.jsonl")),
            &json!({
                "snapshot":snapshot,
                "configured_nominal_cost":nominal,
                "configured_rates_qualified":rates_qualified,
                "configured_rates_note":"zero or missing rates do not establish free usage; positive configured rates are estimates, not invoices",
                "cache_adjustment_applied":false,
                "billing_cost":null,
                "scope":"only completed provider calls whose usage event was observed; unobserved calls remain unknown",
            }),
        );
    }
}

fn execution_count(root: &Path) -> usize {
    std::fs::read_to_string(root.join("executions.counter"))
        .expect("independent tool counter")
        .lines()
        .filter(|line| *line == "executed")
        .count()
}

fn preparations(facts: &[a3s_effect::Fact]) -> Vec<ToolPreparedV1> {
    facts
        .iter()
        .filter(|fact| fact.kind == "tool.prepared")
        .map(|fact| {
            let prepared: ToolPreparedV1 = serde_json::from_value(fact.payload["prepared"].clone())
                .expect("typed committed preparation");
            prepared.validate().expect("valid committed preparation");
            prepared
        })
        .collect()
}

fn receipts(facts: &[a3s_effect::Fact]) -> Vec<ToolExecutionResultV1> {
    facts
        .iter()
        .filter(|fact| fact.kind == "tool.result")
        .map(|fact| {
            let receipt: ToolExecutionResultV1 =
                serde_json::from_value(fact.payload["receipt"].clone())
                    .expect("typed committed terminal receipt, never legacy fallback");
            receipt
                .validate()
                .expect("valid committed terminal receipt");
            receipt
        })
        .collect()
}

async fn tool_effect_write_before_result_child() {
    let root = PathBuf::from(
        std::env::var_os(CHILD_ROOT_ENV)
            .expect("this helper must be explicitly launched by its qualification parent"),
    );
    let (config, model, cost) = real_config(&root, "child");
    let agent = Agent::from_config(config)
        .await
        .expect("create real-provider child agent");
    let session = agent
        .session_async(
            root.join("workspace").display().to_string(),
            Some(options(&root, &model, SESSION, WRITE_TOOL)),
        )
        .await
        .expect("create file-store child session");
    session
        .register_dynamic_tool(Arc::new(ReceiptWriteTool {
            root: root.clone(),
            mode: ToolMode::BlockAfterWrite,
        }))
        .expect("register the public dynamic barrier tool");
    session
        .save()
        .await
        .expect("persist session before admitting its run");
    let mut stream = session.stream_with_run(
        "Call receipt_write_barrier exactly once with {}. Call no other tools. After its result, never retry; finish with a short final answer.", None)
        .await.expect("admit real-provider child stream");
    let run = stream.run.as_ref().expect("model stream has an exact Run");
    sync_write(&root.join("source-run-id"), run.id().as_bytes()).expect("retain exact source Run");
    while let Some(event) = stream.events.recv().await {
        retain_usage(&root, "child", &event, &cost);
    }
    panic!("child stream ended before its parent killed the write-before-result barrier");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live pinned provider and intentionally kills a qualification child process"]
async fn real_llm_write_before_result_crash_recovers_unknown_without_reexecution() {
    if std::env::var_os(CHILD_ROOT_ENV).is_some() {
        tool_effect_write_before_result_child().await;
        return;
    }
    let root = retained_attempt("crash");
    let stdout = std::fs::File::create(root.join("child.stdout.log")).unwrap();
    let stderr = std::fs::File::create(root.join("child.stderr.log")).unwrap();
    let mut child =
        tokio::process::Command::new(std::env::current_exe().expect("integration executable"));
    child
        .args([
            "--exact",
            CHILD_TEST,
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(CHILD_ROOT_ENV, &root)
        .env(
            "A3S_CONFIG_FILE",
            repo_config_path()
                .canonicalize()
                .expect("explicit ACL configuration exists"),
        )
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true);
    let mut child = child
        .spawn()
        .expect("start real-provider qualification child");
    let reached = tokio::time::timeout(SCENARIO_TIMEOUT, async {
        loop {
            if std::fs::read(root.join("write-marker.ready"))
                .ok()
                .as_deref()
                == Some(MARKER_BYTES)
            {
                break;
            }
            assert!(
                child.try_wait().expect("poll child status").is_none(),
                "child exited before the write barrier; inspect retained child stderr"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    if reached.is_err() {
        child.kill().await.expect("contain timed-out child");
        panic!("real-provider child did not reach the bounded write barrier; inspect retained child stderr");
    }
    let workspace = root.join("workspace");
    let before = read_workspace_facts(&workspace, &thread_for_session(SESSION))
        .expect("read child committed log");
    retain_fact_snapshot(&root, "crash-before-kill", SESSION, &before);
    let prepared = preparations(&before);
    assert_eq!(
        prepared.len(),
        1,
        "the model must admit exactly one barrier intent"
    );
    assert_eq!(prepared[0].tool_name, WRITE_TOOL);
    let admission = prepared[0]
        .identity
        .admission
        .as_ref()
        .expect("actual frozen Run admission");
    let run_id = std::fs::read_to_string(root.join("source-run-id")).expect("source Run identity");
    assert_eq!(admission.run_id, run_id);
    assert_eq!(admission.thread_id, thread_for_session(SESSION));
    assert!(before
        .iter()
        .all(|fact| fact.kind != "tool.result" && fact.kind != "turn.complete"));
    assert_eq!(execution_count(&root), 1);
    assert_eq!(
        std::fs::read(workspace.join(FILE_NAME)).unwrap(),
        FILE_BYTES
    );
    child
        .kill()
        .await
        .expect("kill and wait for the write-before-result child");
    assert!(!child.wait().await.expect("reap killed child").success());

    let store = FileSessionStore::new(root.join("sessions"))
        .await
        .expect("reopen real file session store");
    assert!(
        store
            .load_loop_checkpoint(&run_id)
            .await
            .expect("inspect checkpoint boundary")
            .is_none(),
        "this barrier must precede result/checkpoint, so recovery uses the original immutable log"
    );
    let (config, model, cost) = real_config(&root, "recovery");
    let child_model: Value =
        serde_json::from_slice(&std::fs::read(root.join("model-child.json")).unwrap()).unwrap();
    assert_eq!(child_model["model"], model);
    let agent = Agent::from_config(config)
        .await
        .expect("create fresh real-provider recovery agent");
    let session = agent
        .resume_session_async(SESSION, options(&root, &model, SESSION, WRITE_TOOL))
        .await
        .expect("reopen the same persisted session and workspace");
    session
        .register_dynamic_tool(Arc::new(ReceiptWriteTool {
            root: root.clone(),
            mode: ToolMode::BlockAfterWrite,
        }))
        .expect(
            "restore the same public tool so blind replay would increment the independent counter",
        );
    let resumed = tokio::time::timeout(RESUME_TIMEOUT, session.resume_run(&run_id))
        .await
        .expect("bounded original-log recovery must not enter the barrier tool again");
    let after = read_workspace_facts(&workspace, &thread_for_session(SESSION))
        .expect("reopened recovery log");
    retain_fact_snapshot(&root, "resume-recovered-unknown", SESSION, &after);
    assert!(
        resumed.is_err(),
        "an uncertain invocation cannot close as narrative success"
    );
    assert!(
        resumed
            .err()
            .unwrap()
            .to_string()
            .contains("tool_effect_unresolved"),
        "recovery must refuse for durable invocation uncertainty"
    );
    for run in session.runs().await {
        for record in session.run_events(&run.id).await {
            retain_usage(&root, "recovery", &record.event, &cost);
        }
    }
    let original = receipts(&after)
        .into_iter()
        .filter(|result| result.identity == prepared[0].identity)
        .collect::<Vec<_>>();
    assert_eq!(original.len(), 1);
    original[0]
        .ensure_prepared(&prepared[0])
        .expect("terminal binds the original frozen preparation");
    assert_eq!(original[0].outcome, ToolExecutionOutcome::Unknown);
    assert!(
        preparations(&after).contains(&prepared[0]),
        "the immutable preparation cannot be reset or replaced"
    );
    assert!(after.iter().all(|fact| fact.kind != "turn.complete"));
    assert_eq!(
        execution_count(&root),
        1,
        "the physical side effect must execute exactly once"
    );
    assert_eq!(
        std::fs::read(workspace.join(FILE_NAME)).unwrap(),
        FILE_BYTES
    );
    sync_write(&root.join("oracle-crash.json"), &serde_json::to_vec_pretty(&json!({
        "model":model, "executions":1, "file_digest":sha256::digest(FILE_BYTES),
        "original_preparation":prepared[0], "terminal":original[0], "successful_settlement_markers":0,
        "usage_scope":"observed child and recovery usage events retained separately; unobserved usage/cost unknown",
    })).unwrap()).unwrap();
    session.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live pinned provider for a real failed-write invocation"]
async fn real_llm_failed_write_retains_complete_byte_receipt_and_refuses_narrative() {
    let root = retained_attempt("failed-write");
    let (config, model, cost) = real_config(&root, "failed-write");
    let agent = Agent::from_config(config)
        .await
        .expect("create real-provider failed-write agent");
    let workspace = root.join("workspace");
    let session_id = "tool-effect-failed-write-live";
    let session = agent
        .session_async(
            workspace.display().to_string(),
            Some(options(&root, &model, session_id, FAIL_TOOL)),
        )
        .await
        .expect("create failed-write session");
    session
        .register_dynamic_tool(Arc::new(ReceiptWriteTool {
            root: root.clone(),
            mode: ToolMode::FailAfterWrite,
        }))
        .expect("register public failed-write tool");
    let mut stream = session.stream_with_run(
        "Call receipt_fail_after_write exactly once with {}. It deliberately fails after writing a file. Do not retry it, run verification, or call any other tool. After the failure reply with a short final answer.", None)
        .await.expect("admit real failed-write stream");
    let mut saw_error = false;
    tokio::time::timeout(SCENARIO_TIMEOUT, async {
        while let Some(event) = stream.events.recv().await {
            retain_usage(&root, "failed-write", &event, &cost);
            if matches!(event, AgentEvent::Error { .. }) {
                saw_error = true;
            }
        }
        stream.worker.await.expect("failed-write worker joins");
    })
    .await
    .expect("bounded failed-write scenario");
    let facts = read_workspace_facts(&workspace, &thread_for_session(session_id))
        .expect("failed-write authoritative facts");
    retain_fact_snapshot(&root, "failed-write", session_id, &facts);
    let prepared = preparations(&facts);
    let results = receipts(&facts);
    let known = results
        .iter()
        .find(|result| {
            prepared.iter().any(|prepared| {
                prepared.tool_name == FAIL_TOOL && prepared.identity == result.identity
            })
        })
        .expect("failed-write receipt");
    assert_eq!(known.outcome, ToolExecutionOutcome::Failed);
    assert!(known.exit_code.is_some_and(|code| code != 0));
    assert_eq!(
        known.effects.completeness,
        ToolObservationCompleteness::Complete
    );
    assert!(known.effects.unresolved_external_operations.is_empty());
    let actual_bytes = std::fs::read(workspace.join(FILE_NAME)).unwrap();
    assert_eq!(actual_bytes, FILE_BYTES);
    let expected_digest = format!("sha256:{}", sha256::digest(&actual_bytes));
    sync_write(
        &root.join("oracle-failed-write-observation.json"),
        &serde_json::to_vec_pretty(&json!({
            "model":model, "executions":execution_count(&root),
            "independent_file_digest":expected_digest, "known_failed_terminal":known,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        known
            .effects
            .changes
            .iter()
            .any(|change| change.path == FILE_NAME
                && change.after.kind == a3s_effect::ToolFileKind::File
                && change.after.content_digest.as_deref() == Some(expected_digest.as_str())),
        "known failed receipt must bind the independently read actual bytes: expected {expected_digest}, observed {:?}",
        known.effects.changes,
    );
    assert_eq!(execution_count(&root), 1);
    assert!(
        saw_error,
        "unverified known changes must fail the actual session completion gate"
    );
    assert!(facts.iter().all(|fact| fact.kind != "turn.complete"));
    sync_write(
        &root.join("oracle-failed-write.json"),
        &serde_json::to_vec_pretty(&json!({
            "model":model, "executions":1, "file_digest":sha256::digest(FILE_BYTES),
            "known_failed_terminal":known, "successful_settlement_markers":0,
        }))
        .unwrap(),
    )
    .unwrap();
    session.close().await;
}
