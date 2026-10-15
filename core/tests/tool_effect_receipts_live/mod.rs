#![cfg(test)]

//! Additional real-provider child-process windows over the original fact log.
//! The current production chain commits the terminal receipt, then exports
//! its checkpoint from the same log prefix, then calls the provider again.
//! Each asserted boundary is independently read from the original log/store.

mod decorators;

use super::{
    append_json, execution_count, options, preparations, real_config, receipts,
    retain_fact_snapshot, retain_usage, retained_attempt, sync_write, CHILD_ROOT_ENV, FILE_BYTES,
    FILE_NAME, RESUME_TIMEOUT, SCENARIO_TIMEOUT,
};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::fact_control::{read_workspace_facts, thread_for_session};
use a3s_code_core::loop_checkpoint::LoopCheckpoint;
use a3s_code_core::store::{FileSessionStore, SessionStore};
use a3s_code_core::tools::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use a3s_code_core::{Agent, SessionOptions};
use a3s_effect::{Fact, ToolExecutionOutcome, ToolObservationCompleteness, ToolPreparedV1};
use serde_json::{json, Value};

const CHILD_WINDOW_ENV: &str = "A3S_TEST_TOOL_EFFECT_CHILD_WINDOW";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Window {
    PreparedBeforeExecution,
    TerminalCommitted,
    CheckpointBeforeCommit,
}

impl Window {
    fn name(self) -> &'static str {
        match self {
            Self::PreparedBeforeExecution => "prepared-before-execution",
            Self::TerminalCommitted => "terminal-committed",
            Self::CheckpointBeforeCommit => "checkpoint-before-commit",
        }
    }

    fn test_name(self) -> &'static str {
        match self {
            Self::PreparedBeforeExecution => "process_windows::real_llm_prepared_before_execution_crash_keeps_zero_effects_and_unknown",
            Self::TerminalCommitted => "process_windows::real_llm_terminal_committed_crash_reuses_exact_result_without_reexecution",
            Self::CheckpointBeforeCommit => "process_windows::real_llm_checkpoint_before_commit_crash_recovers_from_original_log",
        }
    }

    fn session(self) -> &'static str {
        match self {
            Self::PreparedBeforeExecution => "tool-effect-prepared-before-execution-live",
            Self::TerminalCommitted => "tool-effect-terminal-committed-live",
            Self::CheckpointBeforeCommit => "tool-effect-checkpoint-before-commit-live",
        }
    }

    fn tool(self) -> &'static str {
        match self {
            Self::PreparedBeforeExecution => "receipt_prepared_barrier",
            Self::TerminalCommitted => "receipt_terminal_write",
            Self::CheckpointBeforeCommit => "receipt_checkpoint_write",
        }
    }
}

fn facts(root: &Path, window: Window) -> Vec<Fact> {
    read_workspace_facts(
        &root.join("workspace"),
        &thread_for_session(window.session()),
    )
    .expect("read the original child FileLog")
}

fn announce_barrier(root: &Path, window: Window, phase: &str) {
    assert_eq!(phase, window.name());
    let facts = facts(root, window);
    let prepared = preparations(&facts);
    assert_eq!(
        prepared.len(),
        1,
        "the real model admitted exactly one call"
    );
    assert_eq!(prepared[0].tool_name, window.tool());
    let value = json!({
        "schema":"a3s.test.tool-effects-process-barrier.v1", "phase":phase,
        "process_id":std::process::id(), "session_id":window.session(),
        "identity":prepared[0].identity, "prepared_digest":prepared[0].digest().unwrap(),
        "last_fact_seq":facts.last().map(|fact| fact.seq),
        "authority":"notification only; parent independently reopens the original log/store",
    });
    let pending = root.join("process-barrier.pending");
    sync_write(&pending, &serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    std::fs::rename(pending, root.join("process-barrier.ready")).unwrap();
}

struct WindowWriteTool {
    root: PathBuf,
    window: Window,
    pause_prepared: bool,
}

#[async_trait::async_trait]
impl Tool for WindowWriteTool {
    fn name(&self) -> &str {
        self.window.tool()
    }

    fn description(&self) -> &str {
        "Qualification tool: invoke once with {} to write the requested local evidence file. Never retry."
    }

    fn parameters(&self) -> Value {
        json!({"type":"object", "properties":{}, "additionalProperties":false})
    }

    fn capabilities(&self, _args: &Value) -> ToolCapabilities {
        ToolCapabilities::conservative()
    }

    fn requires_confirmation(&self, _args: &Value) -> bool {
        if self.pause_prepared {
            // The fact-session adapter queries tool-owned escalation after
            // the actor committed preparation, before invoking Tool::execute.
            // This test-only metadata query keeps its ordinary false answer;
            // it grants no permission and cannot execute the tool body.
            announce_barrier(&self.root, self.window, "prepared-before-execution");
            loop {
                std::thread::park();
            }
        }
        false
    }

    async fn execute(&self, args: &Value, context: &ToolContext) -> anyhow::Result<ToolOutput> {
        sync_write(
            &self.root.join("tool-body-entered.ready"),
            b"actual Tool::execute entered\n",
        )?;
        anyhow::ensure!(
            args.as_object().is_some_and(|args| args.is_empty()),
            "qualification requires actual empty tool arguments"
        );
        let mut counter = std::fs::OpenOptions::new()
            .append(true)
            .open(self.root.join("executions.counter"))?;
        use std::io::Write;
        counter.write_all(b"executed\n")?;
        counter.sync_all()?;
        sync_write(&context.workspace.join(FILE_NAME), FILE_BYTES)?;
        Ok(ToolOutput::success("qualification file written and synced"))
    }
}

async fn child(root: &Path, window: Window) {
    let (config, model, cost) = real_config(root, "child");
    let provider = decorators::observed_provider(
        &config,
        &model,
        root,
        "child",
        window,
        window == Window::TerminalCommitted,
    );
    let agent = Agent::from_config(config)
        .await
        .expect("real-provider child agent");
    let mut opts = options(root, &model, window.session(), window.tool()).with_llm_client(provider);
    if window == Window::CheckpointBeforeCommit {
        opts = opts.with_session_store(Arc::new(decorators::CheckpointBarrierStore {
            inner: Arc::new(FileSessionStore::new(root.join("sessions")).await.unwrap()),
            root: root.to_path_buf(),
            window,
        }));
    }
    let session = agent
        .session_async(root.join("workspace").display().to_string(), Some(opts))
        .await
        .expect("original file-store Session");
    session
        .register_dynamic_tool(Arc::new(WindowWriteTool {
            root: root.to_path_buf(),
            window,
            pause_prepared: window == Window::PreparedBeforeExecution,
        }))
        .expect("register actual qualification tool");
    session
        .save()
        .await
        .expect("persist Session before Run admission");
    let prompt = format!(
        "Call {} exactly once with {{}}. Call no other tools. After its result never retry; return a short final answer.",
        window.tool(),
    );
    let mut stream = session
        .stream_with_run(&prompt, None)
        .await
        .expect("admit real model Run");
    let run = stream.run.as_ref().expect("stream carries the source Run");
    sync_write(&root.join("source-run-id"), run.id().as_bytes()).unwrap();
    while let Some(event) = stream.events.recv().await {
        retain_usage(root, "child", &event, &cost);
    }
    panic!("child ended before its named process barrier was killed");
}

fn assert_prepared(root: &Path, window: Window, observed: &[Fact]) -> ToolPreparedV1 {
    let preparations = preparations(observed);
    assert_eq!(preparations.len(), 1, "one real model-selected preparation");
    let prepared = preparations.into_iter().next().unwrap();
    assert_eq!(prepared.tool_name, window.tool());
    let admission = prepared
        .identity
        .admission
        .as_ref()
        .expect("actual frozen Run admission");
    assert_eq!(
        admission.run_id,
        std::fs::read_to_string(root.join("source-run-id")).unwrap()
    );
    assert_eq!(admission.thread_id, thread_for_session(window.session()));
    assert_eq!(
        prepared.original_args_digest,
        a3s_effect::tool_args_digest(&json!({})).unwrap()
    );
    assert_eq!(
        prepared.effective_args_digest,
        prepared.original_args_digest
    );
    prepared
}

fn assert_real_provider_observed(root: &Path, model: &str, prepared: &ToolPreparedV1) {
    let raw = std::fs::read_to_string(root.join("provider-child.jsonl"))
        .expect("actual delegated provider observations, never a synthesized response");
    let observations: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(observations
        .iter()
        .any(|value| value["phase"] == "started" && value["model"] == model));
    assert!(
        observations.iter().any(|value| {
            value["phase"] == "returned"
                && value["model"] == model
                && value["tool_calls"].as_array().is_some_and(|calls| {
                    calls.iter().any(|call| {
                        call["id"] == prepared.identity.tool_call_id
                            && call["name"] == prepared.tool_name
                            && call["args"] == json!({})
                    })
                })
        }),
        "a real provider response must contain the exact admitted tool call"
    );
}

async fn wait_for_barrier(root: &Path, window: Window, child: &mut tokio::process::Child) -> Value {
    let reached = tokio::time::timeout(SCENARIO_TIMEOUT, async {
        loop {
            match std::fs::read(root.join("process-barrier.ready")) {
                Ok(bytes) => {
                    let marker: Value =
                        serde_json::from_slice(&bytes).expect("atomic barrier marker");
                    assert_eq!(marker["phase"], window.name());
                    assert_eq!(marker["process_id"].as_u64(), child.id().map(u64::from));
                    if root.join("source-run-id").exists() {
                        return marker;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("read barrier notification: {error}"),
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited before the named barrier; inspect retained stderr"
            );
            // Polling only observes an explicit synced marker; elapsed time
            // never establishes that an execution boundary has been reached.
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    match reached {
        Ok(marker) => marker,
        Err(_) => {
            child.kill().await.expect("contain timed-out child");
            let status = child.wait().await.expect("reap timed-out child");
            append_json(
                &root.join("attempt-events.jsonl"),
                &json!({
                    "phase":"barrier-timeout", "window":window.name(), "child_exit":status.to_string(),
                    "usage":null, "billing_cost":null,
                }),
            );
            panic!(
                "bounded real model child did not reach {}; artifacts={}",
                window.name(),
                root.display()
            );
        }
    }
}

async fn run_window(window: Window) {
    if let Some(root) = std::env::var_os(CHILD_ROOT_ENV) {
        assert_eq!(
            std::env::var(CHILD_WINDOW_ENV).unwrap(),
            window.name(),
            "exact parent-owned child role"
        );
        child(&PathBuf::from(root), window).await;
        return;
    }
    let root = retained_attempt(window.name());
    // This is a real empty counter, not a default for missing evidence. Only
    // Tool::execute appends execution records after the child is launched.
    sync_write(&root.join("executions.counter"), b"").unwrap();
    sync_write(&root.join("attempt.json"), &serde_json::to_vec_pretty(&json!({
        "schema":"a3s.test.tool-effects-process-attempt.v1", "test":window.test_name(),
        "window":window.name(), "session_id":window.session(), "status":"started",
        "child_timeout_seconds":SCENARIO_TIMEOUT.as_secs(), "resume_timeout_seconds":RESUME_TIMEOUT.as_secs(),
        "tool_round_cap":1, "temperature":0.0, "provider_api_timeout_ms":180_000,
        "tool_timeout_ms":600_000, "billing_cost":null, "total_usage":null,
        "usage_note":"only observed provider returns/events have reported usage; missing usage is unknown",
        "retry_note":"every invocation creates a separate retained attempt directory; no failed attempt is overwritten",
    })).unwrap()).unwrap();
    let stdout = std::fs::File::create(root.join("child.stdout.log")).unwrap();
    let stderr = std::fs::File::create(root.join("child.stderr.log")).unwrap();
    let mut process = tokio::process::Command::new(std::env::current_exe().unwrap());
    process
        .args([
            "--exact",
            window.test_name(),
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(CHILD_ROOT_ENV, &root)
        .env(CHILD_WINDOW_ENV, window.name())
        .env(
            "A3S_CONFIG_FILE",
            super::repo_config_path().canonicalize().unwrap(),
        )
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true);
    let mut process = process
        .spawn()
        .expect("start the exact real-provider child role");
    append_json(
        &root.join("attempt-events.jsonl"),
        &json!({"phase":"spawned", "child_pid":process.id()}),
    );
    let marker = wait_for_barrier(&root, window, &mut process).await;
    let before = read_workspace_facts(
        &root.join("workspace"),
        &thread_for_session(window.session()),
    );
    // Reap before asserting window oracles so a failed oracle cannot leave
    // its intentionally blocked qualification process behind.
    process
        .kill()
        .await
        .expect("kill the child at its explicit process barrier");
    let status = process
        .wait()
        .await
        .expect("reap killed qualification child");
    append_json(
        &root.join("attempt-events.jsonl"),
        &json!({"phase":"killed-and-reaped", "child_exit":status.to_string()}),
    );
    assert!(!status.success());
    let before = before.expect("original log captured while child was blocked at its barrier");
    retain_fact_snapshot(&root, "barrier-before-kill", window.session(), &before);
    let prepared = assert_prepared(&root, window, &before);
    assert_eq!(
        marker["identity"],
        serde_json::to_value(&prepared.identity).unwrap()
    );
    assert_eq!(marker["prepared_digest"], prepared.digest().unwrap());
    let child_model: Value =
        serde_json::from_slice(&std::fs::read(root.join("model-child.json")).unwrap()).unwrap();
    let model = child_model["model"].as_str().unwrap();
    assert_real_provider_observed(&root, model, &prepared);
    assert!(before.iter().all(|fact| fact.kind != "turn.complete"));
    let expected_executions = usize::from(window != Window::PreparedBeforeExecution);
    assert_eq!(execution_count(&root), expected_executions);
    if expected_executions == 0 {
        assert!(
            !root.join("tool-body-entered.ready").exists(),
            "Tool::execute has not entered"
        );
        assert!(!root.join("workspace").join(FILE_NAME).exists());
        assert!(receipts(&before).is_empty());
    } else {
        assert!(root.join("tool-body-entered.ready").exists());
        assert_eq!(
            std::fs::read(root.join("workspace").join(FILE_NAME)).unwrap(),
            FILE_BYTES
        );
    }
    if window != Window::PreparedBeforeExecution {
        assert_known_terminal(&before, &prepared);
    }
    let killed = facts(&root, window);
    assert_eq!(
        serde_json::to_value(&killed).unwrap(),
        serde_json::to_value(&before).unwrap(),
        "blocked child cannot append after its barrier"
    );
    retain_fact_snapshot(&root, "killed-reopened-log", window.session(), &killed);
    let store = FileSessionStore::new(root.join("sessions")).await.unwrap();
    let run_id = prepared.identity.admission.as_ref().unwrap().run_id.clone();
    let checkpoint = store.load_loop_checkpoint(&run_id).await.unwrap();
    sync_write(
        &root.join("checkpoint-observed.json"),
        &serde_json::to_vec_pretty(&json!({
            "run_id":run_id, "checkpoint":checkpoint,
            "terminal_committed_at_barrier":!receipts(&before).is_empty(),
        }))
        .unwrap(),
    )
    .unwrap();
    match window {
        Window::PreparedBeforeExecution => assert!(checkpoint.is_none()),
        Window::CheckpointBeforeCommit => {
            assert!(
                checkpoint.is_none(),
                "real store has not committed the requested cache"
            );
            let requested: LoopCheckpoint = serde_json::from_slice(
                &std::fs::read(root.join("checkpoint-request.json")).unwrap(),
            )
            .unwrap();
            assert_checkpoint(&requested, &prepared, window);
        }
        Window::TerminalCommitted => {
            assert_checkpoint(
                checkpoint
                    .as_ref()
                    .expect("next provider call follows committed receipt and checkpoint"),
                &prepared,
                window,
            );
        }
    }
    let (config, recovery_model, cost) = real_config(&root, "recovery");
    assert_eq!(model, recovery_model);
    let provider =
        decorators::observed_provider(&config, &recovery_model, &root, "recovery", window, false);
    let agent = Agent::from_config(config).await.unwrap();
    let opts: SessionOptions =
        options(&root, &recovery_model, window.session(), window.tool()).with_llm_client(provider);
    let session = agent
        .resume_session_async(window.session(), opts)
        .await
        .expect("reopen original persisted Session");
    session
        .register_dynamic_tool(Arc::new(WindowWriteTool {
            root: root.clone(),
            window,
            pause_prepared: false,
        }))
        .expect("same public tool body makes blind replay observable");
    let resumed = tokio::time::timeout(RESUME_TIMEOUT, session.resume_run(&run_id))
        .await
        .expect("bounded original-log recovery must not repeat a completed/uncertain tool");
    let after = facts(&root, window);
    retain_fact_snapshot(&root, "resume-original-log", window.session(), &after);
    for run in session.runs().await {
        for record in session.run_events(&run.id).await {
            retain_usage(&root, "recovery", &record.event, &cost);
        }
    }
    assert!(
        resumed.is_err(),
        "unverified known effects or Unknown cannot settle as narrative success"
    );
    let error = resumed.err().unwrap().to_string();
    let terminals = receipts(&after)
        .into_iter()
        .filter(|receipt| receipt.identity == prepared.identity)
        .collect::<Vec<_>>();
    assert_eq!(
        terminals.len(),
        1,
        "the original invocation has exactly one terminal result"
    );
    terminals[0].ensure_prepared(&prepared).unwrap();
    if window == Window::PreparedBeforeExecution {
        assert_eq!(terminals[0].outcome, ToolExecutionOutcome::Unknown);
        assert!(
            error.contains("tool_effect_unresolved"),
            "Unknown fails for its durable invocation debt: {error}"
        );
    } else {
        let prior = assert_known_terminal(&before, &prepared);
        assert_eq!(
            serde_json::to_value(&terminals[0]).unwrap(),
            serde_json::to_value(&prior).unwrap(),
            "recovery reuses the exact committed result"
        );
        assert!(
            error.contains("completion gate: workspace mutation") && error.contains(FILE_NAME),
            "actual completion gate must restore and name the known file debt: {error}"
        );
        assert!(
            !error.contains("tool_effect_unresolved"),
            "a committed terminal cannot degrade to Unknown"
        );
    }
    assert_eq!(
        preparations(&after),
        vec![prepared.clone()],
        "preparation identity cannot be reset or replaced"
    );
    assert!(after.iter().all(|fact| fact.kind != "turn.complete"));
    assert_eq!(
        execution_count(&root),
        expected_executions,
        "no blind replay in the actual tool body"
    );
    if expected_executions == 0 {
        assert!(!root.join("tool-body-entered.ready").exists());
        assert!(!root.join("workspace").join(FILE_NAME).exists());
    } else {
        assert_eq!(
            std::fs::read(root.join("workspace").join(FILE_NAME)).unwrap(),
            FILE_BYTES
        );
    }
    sync_write(&root.join("oracle.json"), &serde_json::to_vec_pretty(&json!({
        "schema":"a3s.test.tool-effects-process-oracle.v1", "window":window.name(),
        "model":recovery_model, "original_preparation":prepared, "original_terminal":terminals[0],
        "independent_executions":execution_count(&root), "successful_settlement_markers":0,
        "terminal_committed_at_barrier":!receipts(&before).is_empty(), "recovery_error":error,
        "billing_cost":null, "total_usage":null,
        "usage_note":"observed provider calls and host usage events are retained separately; unobserved usage/cost unknown",
    })).unwrap()).unwrap();
    append_json(
        &root.join("attempt-events.jsonl"),
        &json!({"phase":"all-independent-oracles-passed"}),
    );
    session.close().await;
}

fn assert_known_terminal(
    observed: &[Fact],
    prepared: &ToolPreparedV1,
) -> a3s_effect::ToolExecutionResultV1 {
    let receipts = receipts(observed);
    assert_eq!(receipts.len(), 1);
    let receipt = receipts.into_iter().next().unwrap();
    receipt
        .ensure_prepared(prepared)
        .expect("terminal binds actual frozen preparation");
    assert_eq!(receipt.outcome, ToolExecutionOutcome::Succeeded);
    assert_eq!(receipt.exit_code, Some(0));
    assert_eq!(
        receipt.effects.completeness,
        ToolObservationCompleteness::Complete
    );
    assert!(receipt.effects.unresolved_external_operations.is_empty());
    let expected_digest = format!("sha256:{}", sha256::digest(FILE_BYTES));
    assert!(
        receipt.effects.changes.iter().any(|change| {
            change.path == FILE_NAME
                && change.after.kind == a3s_effect::ToolFileKind::File
                && change.after.content_digest.as_deref() == Some(expected_digest.as_str())
        }),
        "committed terminal binds the independently read file bytes"
    );
    receipt
}

fn assert_checkpoint(checkpoint: &LoopCheckpoint, prepared: &ToolPreparedV1, window: Window) {
    let admission = prepared.identity.admission.as_ref().unwrap();
    checkpoint
        .ensure_loadable()
        .expect("valid actual portable checkpoint");
    checkpoint
        .ensure_owned_by(&admission.run_id, window.session())
        .unwrap();
    let binding = checkpoint
        .capability_binding
        .as_ref()
        .expect("actual frozen capability cache binding");
    assert_eq!(
        a3s_code_core::content_digest::digest_json("a3s.code.tool-admission-binding.v1", binding)
            .unwrap(),
        admission.capability_binding_digest,
    );
    assert!(checkpoint
        .convergence
        .mutations
        .paths()
        .any(|path| path == FILE_NAME));
    let expected_digest = sha256::digest(FILE_BYTES);
    assert_eq!(
        checkpoint
            .convergence
            .mutations
            .content_digest_for_path(FILE_NAME),
        Some(expected_digest.as_str())
    );
    assert!(
        checkpoint
            .messages
            .iter()
            .any(|message| message.content.iter().any(|block| {
                matches!(block, a3s_code_core::llm::ContentBlock::ToolResult { tool_use_id, .. }
            if tool_use_id == &prepared.identity.tool_call_id)
            })),
        "checkpoint contains the original committed tool result"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live pinned provider and kills its prepared-before-execution child"]
async fn real_llm_prepared_before_execution_crash_keeps_zero_effects_and_unknown() {
    run_window(Window::PreparedBeforeExecution).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live pinned provider and kills its terminal-committed child"]
async fn real_llm_terminal_committed_crash_reuses_exact_result_without_reexecution() {
    run_window(Window::TerminalCommitted).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live pinned provider and kills its checkpoint-before-commit child"]
async fn real_llm_checkpoint_before_commit_crash_recovers_from_original_log() {
    run_window(Window::CheckpointBeforeCommit).await;
}
