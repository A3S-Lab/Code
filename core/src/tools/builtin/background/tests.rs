#![cfg(test)]

use super::test_backend::DeterministicBackgroundSandbox;
use super::*;
use crate::capability::{
    AgentCapabilityRuntime, AgentCapabilityTurn, CapabilityCeiling, CapabilityExecutionCeiling,
    CapabilityScope, CapabilitySet, GovernanceCapabilityCeiling, Run, Session,
    WorkspaceCapabilityCeiling,
};
use crate::sandbox::{BashSandbox, SandboxCommandRequest, SandboxExecutionOutput, SandboxOutput};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_os = "macos")]
mod escape;

struct TestRun {
    workspace: tempfile::TempDir,
    session: CapabilityScope<Session>,
    run: CapabilityScope<Run>,
    turn: AgentCapabilityTurn,
    ctx: ToolContext,
}

impl TestRun {
    fn new(sandbox: Option<Arc<dyn BashSandbox>>) -> Self {
        let workspace = tempfile::tempdir().unwrap();
        Self::in_workspace(workspace, sandbox)
    }

    fn in_workspace(workspace: tempfile::TempDir, sandbox: Option<Arc<dyn BashSandbox>>) -> Self {
        let set = CapabilitySet::empty().unwrap();
        let ceiling = CapabilityCeiling::all(
            &set,
            WorkspaceCapabilityCeiling::all(),
            GovernanceCapabilityCeiling::none_required(),
            CapabilityExecutionCeiling::new(8, 2, None, None, None).unwrap(),
        )
        .unwrap();
        let identity = format!("bg-proof-{}", uuid::Uuid::new_v4());
        let session = CapabilityScope::new_session(&identity, set, ceiling.clone()).unwrap();
        let run = session.admit_run("run", ceiling).unwrap();
        let turn = AgentCapabilityRuntime::from_run(&run)
            .begin_turn(0)
            .unwrap();
        let log = Arc::new(
            FileLog::open(crate::session_folder::effect_log_dir(
                workspace.path(),
                &identity,
            ))
            .unwrap(),
        );
        let mut ctx = ToolContext::new(workspace.path().to_path_buf())
            .with_session_id(&identity)
            .with_run_id(format!("admitted-{identity}"))
            .with_fact_log(log)
            .with_capability_context(turn.tool_context());
        ctx.sandbox = sandbox;
        Self {
            workspace,
            session,
            run,
            turn,
            ctx,
        }
    }

    #[cfg(unix)]
    fn native() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let sandbox =
            Arc::new(crate::sandbox::native::NativeBashSandbox::new(workspace.path()).unwrap());
        Self::in_workspace(workspace, Some(sandbox))
    }

    async fn close(&self) {
        assert!(self.turn.close().await.unwrap().is_clean());
        assert!(self.run.close().await.unwrap().is_clean());
        assert!(self.session.close().await.unwrap().is_clean());
    }

    fn facts(&self) -> Vec<a3s_effect::Fact> {
        self.ctx
            .fact_log
            .as_ref()
            .unwrap()
            .read(&crate::fact_control::thread_for_session(
                self.ctx.session_id.as_deref().unwrap(),
            ))
            .unwrap()
    }
}

async fn start(tool: &BackgroundTool, ctx: &ToolContext, name: &str, command: &str) -> ToolOutput {
    tool.execute(
        &serde_json::json!({ "action": "start", "name": name, "command": command }),
        ctx,
    )
    .await
    .unwrap()
}

fn task(tool: &BackgroundTool, ctx: &ToolContext, name: &str) -> Arc<BackgroundTask> {
    tool.runs
        .get(ctx.capability_run_instance_id().unwrap(), name)
        .unwrap()
}

async fn wait_settled(task: &BackgroundTask) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let notified = task.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if task.status().is_some() {
                break;
            }
            notified.await;
        }
    })
    .await
    .expect("background task settles");
}

fn bare_task(workspace: &std::path::Path, name: &str) -> BackgroundTask {
    BackgroundTask::new(
        name.to_string(),
        format!("background-{}", uuid::Uuid::new_v4()),
        "admitted-bare-task".to_string(),
        uuid::Uuid::new_v4(),
        Arc::new(FileLog::open(workspace.join("facts")).unwrap()),
        "bg-evidence".to_string(),
    )
}

#[cfg(unix)]
async fn assert_native_start_is_unsupported(
    owned: &TestRun,
    tool: &BackgroundTool,
    output: &ToolOutput,
) {
    assert!(!output.success);
    assert!(matches!(
        output.error_kind,
        Some(ToolErrorKind::Unsupported { .. })
    ));
    assert!(!owned
        .ctx
        .sandbox
        .as_ref()
        .unwrap()
        .supports_background_execution());
    assert!(output
        .metadata
        .as_ref()
        .is_none_or(|metadata| metadata.get("workspace_child").is_none()));
    assert!(tool
        .runs
        .names_for(owned.ctx.capability_run_instance_id().unwrap())
        .is_empty());
    assert!(
        owned.facts().is_empty(),
        "unsupported admission must emit no background facts"
    );
    assert!(owned.turn.close().await.unwrap().is_clean());
    let report = owned.run.close().await.unwrap();
    assert!(report.is_clean());
    assert_eq!(report.tasks_completed + report.tasks_cancelled, 0);
    assert_eq!(report.effects_closed, 0);
    assert!(owned.session.close().await.unwrap().is_clean());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_macos_refuses_background_before_task_fact_or_process_admission() {
    let owned = TestRun::native();
    let tool = BackgroundTool::default();
    let output = start(
        &tool,
        &owned.ctx,
        "uncontained",
        "printf '%s' $$ > native-launch.pid; printf leaked > sentinel; sleep 30",
    )
    .await;
    assert_native_start_is_unsupported(&owned, &tool, &output).await;
    assert!(!owned.workspace.path().join("native-launch.pid").exists());
    assert!(!owned.workspace.path().join("sentinel").exists());
    assert!(tool.runs.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn start_without_a_run_owner_is_refused() {
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf()).with_session_id("background-plain");
    let output = start(&BackgroundTool::default(), &ctx, "watch", "echo hi").await;
    assert!(!output.success);
    assert!(output
        .content
        .contains("requires a Run-owned capability context"));
}

#[tokio::test]
async fn unknown_action_names_the_closed_set() {
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf());
    let output = BackgroundTool::default()
        .execute(
            &serde_json::json!({ "action": "launch", "name": "watch" }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(output
        .content
        .contains("the closed set is start, tail, stop"));
}

#[test]
fn terminal_disposition_and_settlement_basis_parse_name_their_closed_sets() {
    for value in ["completed", "cancelled", "failed", "timed_out", "unknown"] {
        assert_eq!(BackgroundDisposition::parse(value).unwrap().as_str(), value);
    }
    for value in [
        "execution_returned",
        "backend_drop_contract",
        "unproven",
        "admission_refused",
    ] {
        assert_eq!(
            ProcessSettlementBasis::parse(value).unwrap().as_str(),
            value
        );
    }
    assert!(BackgroundDisposition::parse("done")
        .unwrap_err()
        .to_string()
        .contains("background disposition"));
    assert!(ProcessSettlementBasis::parse("terminated")
        .unwrap_err()
        .to_string()
        .contains("background process settlement basis"));
}

#[tokio::test]
async fn run_context_without_sandbox_never_spawns_on_host() {
    let owned = TestRun::new(None);
    let output = start(
        &BackgroundTool::default(),
        &owned.ctx,
        "host-leak",
        "echo leaked > sentinel",
    )
    .await;
    assert!(!output.success);
    assert!(matches!(
        output.error_kind,
        Some(ToolErrorKind::Unsupported { .. })
    ));
    assert!(!owned.workspace.path().join("sentinel").exists());
    owned.close().await;
}

#[tokio::test]
async fn missing_source_run_is_refused_before_backend_execution() {
    let calls = Arc::new(AtomicUsize::new(0));
    let owned = TestRun::new(Some(Arc::new(LegacySandbox(Arc::clone(&calls)))));
    let ctx = ToolContext::new(owned.workspace.path().to_path_buf())
        .with_session_id(owned.ctx.session_id.as_deref().unwrap())
        .with_fact_log(Arc::clone(owned.ctx.fact_log.as_ref().unwrap()))
        .with_capability_context(owned.turn.tool_context());
    let output = start(&BackgroundTool::default(), &ctx, "unbound", "anything").await;
    assert!(!output.success);
    assert!(output.content.contains("admitted source Run identity"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(owned.facts().is_empty());
    owned.close().await;
}

struct LegacySandbox(Arc<AtomicUsize>);

#[async_trait]
impl BashSandbox for LegacySandbox {
    async fn exec_command(&self, _: &str, _: &str) -> anyhow::Result<SandboxOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("legacy adapter must never execute")
    }
    async fn shutdown(&self) {}
}

#[tokio::test]
async fn legacy_sandbox_is_refused_before_execution() {
    let calls = Arc::new(AtomicUsize::new(0));
    let owned = TestRun::new(Some(Arc::new(LegacySandbox(Arc::clone(&calls)))));
    let output = start(&BackgroundTool::default(), &owned.ctx, "legacy", "anything").await;
    assert!(!output.success);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    owned.close().await;
}

struct MissingAccountingSandbox;

#[async_trait]
impl BashSandbox for MissingAccountingSandbox {
    fn supports_background_execution(&self) -> bool {
        true
    }
    async fn exec_command(&self, _: &str, _: &str) -> anyhow::Result<SandboxOutput> {
        unreachable!("extended contract is required")
    }
    async fn exec(&self, request: SandboxCommandRequest) -> anyhow::Result<SandboxExecutionOutput> {
        request
            .output_observer
            .unwrap()
            .on_output_delta("unaccounted\n")
            .await;
        Ok(SandboxExecutionOutput {
            stdout: "unaccounted\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        })
    }
    async fn shutdown(&self) {}
}

#[tokio::test]
async fn missing_capture_accounting_refuses_successful_run_close() {
    let owned = TestRun::new(Some(Arc::new(MissingAccountingSandbox)));
    let tool = BackgroundTool::default();
    assert!(
        start(&tool, &owned.ctx, "unaccounted", "anything")
            .await
            .success
    );
    let task = task(&tool, &owned.ctx, "unaccounted");
    wait_settled(&task).await;
    assert!(task.failure().unwrap().contains("capture accounting"));
    let terminal = owned
        .facts()
        .into_iter()
        .find(|fact| fact.payload["event"] == "exited")
        .unwrap();
    assert_eq!(terminal.payload["disposition"], "unknown");
    assert_eq!(terminal.payload["process_settled"], false);
    assert!(owned.turn.close().await.unwrap().is_clean());
    let report = owned.run.close().await.unwrap();
    assert_eq!(report.tasks_failed, 1);
    assert_eq!(report.effects_failed, 1);
    assert!(!report.is_clean());
    owned.session.close().await.unwrap();
}

struct PostCaptureOutcomeSandbox {
    fail_after_capture: bool,
    exit_code: i32,
    held_after_capture: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[async_trait]
impl BashSandbox for PostCaptureOutcomeSandbox {
    fn supports_background_execution(&self) -> bool {
        true
    }
    async fn exec_command(&self, _: &str, _: &str) -> anyhow::Result<SandboxOutput> {
        unreachable!("extended contract is required")
    }
    async fn exec(&self, request: SandboxCommandRequest) -> anyhow::Result<SandboxExecutionOutput> {
        let timed_out = !self.fail_after_capture && self.exit_code == 124;
        request
            .output_observer
            .unwrap()
            .on_output_complete(&CommandOutputSummary {
                total_bytes: 0,
                captured_bytes: 0,
                truncated: false,
                timed_out,
            })
            .await;
        if let Some(dropped) = &self.held_after_capture {
            struct ExecutionDropProof(Arc<std::sync::atomic::AtomicBool>);
            impl Drop for ExecutionDropProof {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _drop_proof = ExecutionDropProof(Arc::clone(dropped));
            std::future::pending::<()>().await;
        }
        if self.fail_after_capture {
            anyhow::bail!("backend postcondition failed after output capture");
        }
        Ok(SandboxExecutionOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: self.exit_code,
            timed_out,
        })
    }
    async fn shutdown(&self) {}
}

struct PreSpawnRefusalSandbox;

#[async_trait]
impl BashSandbox for PreSpawnRefusalSandbox {
    fn supports_background_execution(&self) -> bool {
        true
    }

    async fn exec_command(&self, _: &str, _: &str) -> anyhow::Result<SandboxOutput> {
        Err(crate::sandbox::native::native_process_lifetime_unqualified())
    }

    async fn exec(&self, request: SandboxCommandRequest) -> anyhow::Result<SandboxExecutionOutput> {
        let _ = request;
        Err(crate::sandbox::native::native_process_lifetime_unqualified())
    }

    async fn shutdown(&self) {}
}

#[tokio::test]
async fn typed_pre_spawn_refusal_settles_without_a_process_or_exit_code() {
    let owned = TestRun::new(Some(Arc::new(PreSpawnRefusalSandbox)));
    let sentinel = owned.workspace.path().join("must-not-exist");
    let tool = BackgroundTool::default();
    assert!(
        start(
            &tool,
            &owned.ctx,
            "refused",
            &format!("printf started > {}", sentinel.display()),
        )
        .await
        .success
    );
    let task = task(&tool, &owned.ctx, "refused");
    wait_settled(&task).await;
    assert!(task.failure().is_none(), "{:?}", task.failure());
    let terminal = owned
        .facts()
        .into_iter()
        .find(|fact| fact.payload["event"] == "exited")
        .unwrap();
    assert_eq!(
        terminal.payload["disposition"].as_str(),
        Some(BackgroundDisposition::Failed.as_str())
    );
    assert_eq!(
        terminal.payload["process_settlement_basis"].as_str(),
        Some(ProcessSettlementBasis::AdmissionRefused.as_str())
    );
    assert_eq!(terminal.payload["process_settled"], true);
    assert_eq!(terminal.payload["exit_code"], serde_json::Value::Null);
    assert_eq!(terminal.payload["cancelled"], false);
    assert_eq!(terminal.payload["timed_out"], false);
    assert_eq!(terminal.payload["output_complete"], false);
    assert_eq!(terminal.payload["observation_incomplete"], false);
    assert_eq!(
        terminal.payload["line"].as_str(),
        Some(crate::sandbox::native::NATIVE_PROCESS_LIFETIME_UNQUALIFIED)
    );
    assert!(!sentinel.exists());
    owned.close().await;
}

#[tokio::test]
async fn execution_error_after_complete_capture_never_proves_process_settlement() {
    let owned = TestRun::new(Some(Arc::new(PostCaptureOutcomeSandbox {
        fail_after_capture: true,
        exit_code: 0,
        held_after_capture: None,
    })));
    let tool = BackgroundTool::default();
    assert!(
        start(&tool, &owned.ctx, "postcondition", "anything")
            .await
            .success
    );
    let task = task(&tool, &owned.ctx, "postcondition");
    wait_settled(&task).await;
    let terminal = owned
        .facts()
        .into_iter()
        .find(|fact| fact.payload["event"] == "exited")
        .unwrap();
    assert_eq!(terminal.payload["output_complete"], true);
    assert_eq!(terminal.payload["exit_code"], serde_json::Value::Null);
    assert_eq!(terminal.payload["disposition"], "failed");
    assert_eq!(terminal.payload["process_settled"], false);
    assert_eq!(terminal.payload["process_settlement_basis"], "unproven");
    assert!(owned.turn.close().await.unwrap().is_clean());
    assert!(!owned.run.close().await.unwrap().is_clean());
    owned.session.close().await.unwrap();
}

#[tokio::test]
async fn timeout_after_complete_capture_never_proves_process_settlement() {
    let owned = TestRun::new(Some(Arc::new(PostCaptureOutcomeSandbox {
        fail_after_capture: false,
        exit_code: 124,
        held_after_capture: None,
    })));
    let tool = BackgroundTool::default();
    assert!(
        start(&tool, &owned.ctx, "timeout", "anything")
            .await
            .success
    );
    let task = task(&tool, &owned.ctx, "timeout");
    wait_settled(&task).await;
    let terminal = owned
        .facts()
        .into_iter()
        .find(|fact| fact.payload["event"] == "exited")
        .unwrap();
    assert_eq!(terminal.payload["output_complete"], true);
    assert_eq!(terminal.payload["disposition"], "timed_out");
    assert_eq!(terminal.payload["process_settled"], false);
    assert_eq!(terminal.payload["process_settlement_basis"], "unproven");
    owned.close().await;
}

#[tokio::test]
async fn unavailable_exit_code_never_claims_a_completed_process() {
    let owned = TestRun::new(Some(Arc::new(PostCaptureOutcomeSandbox {
        fail_after_capture: false,
        exit_code: -1,
        held_after_capture: None,
    })));
    let tool = BackgroundTool::default();
    assert!(
        start(&tool, &owned.ctx, "unknown-exit", "anything")
            .await
            .success
    );
    let task = task(&tool, &owned.ctx, "unknown-exit");
    wait_settled(&task).await;
    let terminal = owned
        .facts()
        .into_iter()
        .find(|fact| fact.payload["event"] == "exited")
        .unwrap();
    assert_eq!(terminal.payload["exit_code"], serde_json::Value::Null);
    assert_eq!(terminal.payload["disposition"], "unknown");
    assert_eq!(terminal.payload["process_settled"], false);
    assert!(owned.turn.close().await.unwrap().is_clean());
    assert!(!owned.run.close().await.unwrap().is_clean());
    owned.session.close().await.unwrap();
}

#[tokio::test]
async fn cancelled_future_is_dropped_before_terminal_and_never_claims_complete_output() {
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let owned = TestRun::new(Some(Arc::new(PostCaptureOutcomeSandbox {
        fail_after_capture: false,
        exit_code: 0,
        held_after_capture: Some(Arc::clone(&dropped)),
    })));
    let tool = BackgroundTool::default();
    assert!(
        start(&tool, &owned.ctx, "captured-then-held", "anything")
            .await
            .success
    );
    let task = task(&tool, &owned.ctx, "captured-then-held");
    tokio::time::timeout(Duration::from_secs(5), async {
        while task.output_summary.lock().unwrap().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let stopped = tool.stop("captured-then-held", &owned.ctx).await.unwrap();
    assert!(stopped.success, "{}", stopped.content);
    assert!(dropped.load(Ordering::SeqCst));
    let terminal = owned
        .facts()
        .into_iter()
        .find(|fact| fact.payload["event"] == "exited")
        .unwrap();
    assert_eq!(terminal.payload["disposition"], "cancelled");
    assert_eq!(
        terminal.payload["process_settlement_basis"],
        "backend_drop_contract"
    );
    assert_eq!(terminal.payload["process_settled"], true);
    assert_eq!(terminal.payload["execution_future_released"], true);
    assert_eq!(terminal.payload["output_complete"], false);
    assert_ne!(terminal.payload["output"], serde_json::Value::Null);
    let effects: a3s_effect::ToolEffectObservationV1 =
        serde_json::from_value(terminal.payload["effects"].clone()).unwrap();
    effects.validate().unwrap();
    assert_eq!(
        effects.completeness,
        a3s_effect::ToolObservationCompleteness::Complete
    );
    owned.close().await;
}

#[test]
fn line_truncation_respects_char_boundaries() {
    let cut = truncate_utf8(&"标记".repeat(4096), BG_MAX_LINE_BYTES);
    assert!(cut.len() <= BG_MAX_LINE_BYTES + 3);
    assert!(cut.ends_with('…'));
}

#[test]
fn window_render_drops_from_the_front_under_the_cap() {
    let lines: Vec<String> = (0..50)
        .map(|index| format!("line-{index:04}-{}", "x".repeat(100)))
        .collect();
    let body = render_window(&lines, 1024);
    assert!(body.len() <= 1024 + 64);
    assert!(body.contains("line-0049"));
    assert!(!body.contains("line-0000"));
}

#[tokio::test]
async fn streaming_unterminated_lines_stay_bounded_and_flush_before_exit() {
    let workspace = tempfile::tempdir().unwrap();
    let task = bare_task(workspace.path(), "partial");
    task.on_output_delta(&"标记".repeat(100_000)).await;
    assert!(task.output_lines.lock().unwrap().pending.len() <= BG_MAX_LINE_BYTES);
    task.finish("exit status 0".to_string(), serde_json::json!({}));
    let facts = task.log.read(&task.thread).unwrap();
    assert_eq!(facts.len(), 2);
    assert!(facts[0].payload["line"].as_str().unwrap().len() <= BG_MAX_LINE_BYTES + 3);
    assert_eq!(facts.last().unwrap().payload["event"], "exited");
}

#[tokio::test]
async fn stream_cap_records_one_truncation_fact_and_a_terminal_fact() {
    let workspace = tempfile::tempdir().unwrap();
    let task = bare_task(workspace.path(), "spew");
    for index in 0..(BG_FACT_LINES_PER_TASK + 20) {
        task.record_line(&format!("line-{index}"), None);
    }
    task.finish("exit status 0".to_string(), serde_json::json!({}));
    let facts = task.log.read(&task.thread).unwrap();
    assert_eq!(facts.len() as u64, BG_FACT_LINES_PER_TASK + 2);
    assert_eq!(
        facts
            .iter()
            .filter(|fact| fact.payload["event"] == "truncated")
            .count(),
        1
    );
    assert_eq!(task.ring.lock().unwrap().len(), BG_RING_LINES);
    assert_eq!(facts.last().unwrap().payload["event"], "exited");
}

#[tokio::test]
async fn output_or_terminal_append_failure_refuses_settlement() {
    use crate::capability::CapabilityEffect;
    let workspace = tempfile::tempdir().unwrap();
    let task = Arc::new(bare_task(workspace.path(), "broken-log"));
    // A directory at the thread-file path deterministically breaks durable append.
    std::fs::create_dir(workspace.path().join("facts/bg-evidence.jsonl")).unwrap();
    task.record_line("output before exit", None);
    task.finish("exit status 0".to_string(), serde_json::json!({}));
    assert!(task.failure().unwrap().contains("evidence append failed"));
    let runs = Arc::new(BackgroundRuns::default());
    runs.insert(task.run, Arc::clone(&task));
    let effect = BackgroundSettlementEffect {
        runs,
        run: task.run,
        task,
    };
    assert!(Box::new(effect).close().await.is_err());
}

#[tokio::test]
async fn recovered_terminal_append_retains_an_earlier_output_evidence_failure() {
    let workspace = tempfile::tempdir().unwrap();
    let task = bare_task(workspace.path(), "transient-log-failure");
    let thread_path = workspace.path().join("facts/bg-evidence.jsonl");
    std::fs::create_dir(&thread_path).unwrap();
    task.record_line("cannot retain this output", None);
    std::fs::remove_dir(&thread_path).unwrap();
    task.finish(
        "exit status 0".to_string(),
        serde_json::json!({
            "disposition": "completed", "process_settled": true,
            "process_settlement_basis": "execution_returned", "output_complete": true,
        }),
    );
    let facts = task.log.read(&task.thread).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].payload["event"], "exited");
    assert_eq!(facts[0].payload["disposition"], "unknown");
    assert_eq!(facts[0].payload["process_settled"], false);
    assert_eq!(facts[0].payload["output_facts_retained"], false);
}

#[tokio::test]
async fn failed_command_still_records_and_folds_workspace_changes() {
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = Arc::new(
        DeterministicBackgroundSandbox::new(
            workspace.path().to_path_buf(),
            "writer-ready",
            7,
            false,
        )
        .with_write("changed.txt", b"changed", 0o600),
    );
    let owned = TestRun::in_workspace(workspace, Some(sandbox));
    let tool = BackgroundTool::default();
    let output = start(
        &tool,
        &owned.ctx,
        "writer",
        "printf changed > changed.txt; chmod 600 changed.txt; printf 'writer-ready\n'; exit 7",
    )
    .await;
    assert!(output.success, "{}", output.content);
    assert_eq!(
        output.metadata.as_ref().unwrap()["source_run"],
        owned.ctx.run_id().unwrap()
    );
    let effect_run = owned.ctx.capability_run_instance_id().unwrap().to_string();
    assert_eq!(output.metadata.as_ref().unwrap()["run"], effect_run);
    let mut ledger = crate::harness_loop::MutationLedger::default();
    ledger.observe_tool("background", 0, output.metadata.as_ref());
    let task = task(&tool, &owned.ctx, "writer");
    wait_settled(&task).await;
    assert!(task.failure().is_none(), "{:?}", task.failure());
    let facts = owned.facts();
    let terminal = facts
        .iter()
        .find(|fact| fact.payload["event"] == "exited")
        .unwrap();
    assert_eq!(terminal.payload["exit_code"], 7);
    assert_eq!(terminal.payload["disposition"], "completed");
    assert_eq!(terminal.payload["process_settled"], true);
    assert_eq!(
        terminal.payload["process_settlement_basis"],
        "execution_returned"
    );
    assert_eq!(terminal.payload["execution_future_released"], true);
    assert_eq!(terminal.payload["output_facts_retained"], true);
    assert!(facts.iter().all(|fact| {
        fact.payload["source_run"] == owned.ctx.run_id().unwrap()
            && fact.payload["run"] == effect_run
    }));
    let effects: a3s_effect::ToolEffectObservationV1 =
        serde_json::from_value(terminal.payload["effects"].clone()).unwrap();
    effects.validate().unwrap();
    assert_eq!(
        effects.completeness,
        a3s_effect::ToolObservationCompleteness::Complete
    );
    let changed = effects
        .changes
        .iter()
        .find(|change| change.path == "changed.txt")
        .unwrap();
    assert_eq!(changed.after.kind, a3s_effect::ToolFileKind::File);
    assert_eq!(
        changed.after.content_digest.as_deref(),
        Some("sha256:d67e2e944994496c8d8ec76eed0cf9f09679448d584b532bebf941852a37f5ed")
    );
    #[cfg(unix)]
    assert_eq!(changed.after.mode.unwrap() & 0o777, 0o600);
    assert!(terminal.payload["changed_paths"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == "changed.txt"));
    assert!(terminal.payload["output"]["total_bytes"].as_u64().unwrap() > 0);
    assert!(
        !crate::harness_loop::absorb_open_workspace_children(
            &mut ledger,
            owned.workspace.path(),
            &tokio_util::sync::CancellationToken::new()
        )
        .await
    );
    assert!(ledger.paths().any(|path| path == "changed.txt"));
    assert!(!ledger.has_open_children());
    owned.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn native_background_denies_outside_and_control_metadata_writes() {
    let owned = TestRun::native();
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("host-leak");
    let tool = BackgroundTool::default();
    let command = format!(
        "printf 'sandbox-ready\n'; printf leaked > {}; printf protected > .a3s/host-leak",
        shell_words::quote(external.to_str().unwrap())
    );
    let output = start(&tool, &owned.ctx, "denied", &command).await;
    if cfg!(target_os = "macos") {
        assert_native_start_is_unsupported(&owned, &tool, &output).await;
        assert!(!external.exists());
        assert!(!owned.workspace.path().join(".a3s/host-leak").exists());
        return;
    }
    assert!(output.success, "{}", output.content);
    let task = task(&tool, &owned.ctx, "denied");
    wait_settled(&task).await;
    assert!(task.failure().is_none(), "{:?}", task.failure());
    assert!(!external.exists());
    assert!(
        owned
            .facts()
            .iter()
            .any(|fact| fact.payload["line"] == "sandbox-ready"),
        "the sandboxed shell must run before its denial counts as evidence"
    );
    assert!(!owned.workspace.path().join(".a3s/host-leak").exists());
    assert!(owned
        .facts()
        .iter()
        .any(|fact| fact.payload["event"] == "exited" && fact.payload["exit_code"] != 0));
    owned.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn native_background_denies_ungranted_network_egress() {
    let owned = TestRun::native();
    assert!(
        std::path::Path::new("/usr/bin/curl").is_file(),
        "network proof requires the real curl executable"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let tool = BackgroundTool::default();
    let command =
        format!("printf 'sandbox-ready\n'; /usr/bin/curl --noproxy '*' --connect-timeout 1 --max-time 2 http://127.0.0.1:{port}/");
    let output = start(&tool, &owned.ctx, "network-denied", &command).await;
    if cfg!(target_os = "macos") {
        assert_native_start_is_unsupported(&owned, &tool, &output).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
        return;
    }
    assert!(output.success, "{}", output.content);
    let task = task(&tool, &owned.ctx, "network-denied");
    wait_settled(&task).await;
    assert!(task.failure().is_none(), "{:?}", task.failure());
    assert!(owned
        .facts()
        .iter()
        .any(|fact| fact.payload["line"] == "sandbox-ready"));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    let terminal = owned
        .facts()
        .into_iter()
        .find(|fact| fact.payload["event"] == "exited")
        .unwrap();
    assert_eq!(terminal.payload["exit_code"], 7);
    owned.close().await;
}

#[tokio::test]
async fn run_settlement_releases_background_future_after_the_turn_closes() {
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = Arc::new(DeterministicBackgroundSandbox::new(
        workspace.path().to_path_buf(),
        "ready",
        0,
        true,
    ));
    let owned = TestRun::in_workspace(
        workspace,
        Some(Arc::clone(&sandbox) as Arc<dyn BashSandbox>),
    );
    let tool = BackgroundTool::default();
    assert!(
        start(
            &tool,
            &owned.ctx,
            "sleeper",
            "printf 'ready\n'; (sleep 0.8; printf leaked > descendant-leak) & wait"
        )
        .await
        .success
    );
    let task = task(&tool, &owned.ctx, "sleeper");
    let tail = tool.execute(&serde_json::json!({ "action": "tail", "name": "sleeper", "wait_for": "ready", "timeout_ms": 3000 }), &owned.ctx).await.unwrap();
    assert!(tail.metadata.unwrap()["found"].as_bool().unwrap());
    assert!(owned.turn.close().await.unwrap().is_clean());
    assert!(
        task.status().is_none(),
        "Turn close must not terminate Run-owned background work"
    );
    let report = owned.run.close().await.unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(task.status().as_deref(), Some("terminated by request"));
    let terminal = owned
        .facts()
        .into_iter()
        .find(|fact| fact.payload["event"] == "exited")
        .unwrap();
    assert_eq!(terminal.payload["cancelled"], true);
    assert_eq!(terminal.payload["disposition"], "cancelled");
    assert_eq!(terminal.payload["process_settled"], true);
    assert_eq!(
        terminal.payload["process_settlement_basis"],
        "backend_drop_contract"
    );
    assert_eq!(terminal.payload["execution_future_released"], true);
    assert_eq!(terminal.payload["output_facts_retained"], true);
    assert_eq!(terminal.payload["output_complete"], false);
    assert!(terminal.payload["output_observed_bytes"].as_u64().unwrap() > 0);
    assert_eq!(sandbox.calls(), 1);
    assert!(sandbox.was_released());
    assert!(owned.session.close().await.unwrap().is_clean());
}

#[tokio::test]
async fn completion_gate_settles_live_background_before_absorbing_its_changes() {
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = Arc::new(
        DeterministicBackgroundSandbox::new(workspace.path().to_path_buf(), "ready", 0, true)
            .with_write("gate.txt", b"changed", 0o600),
    );
    let owned = TestRun::in_workspace(workspace, Some(sandbox));
    let tool = BackgroundTool::default();
    let output = start(
        &tool,
        &owned.ctx,
        "gate-writer",
        "printf changed > gate.txt; printf 'ready\n'; sleep 30",
    )
    .await;
    assert!(output.success, "{}", output.content);
    let task = task(&tool, &owned.ctx, "gate-writer");
    let tail = tool.execute(&serde_json::json!({ "action": "tail", "name": "gate-writer", "wait_for": "ready", "timeout_ms": 3000 }), &owned.ctx).await.unwrap();
    assert_eq!(tail.metadata.unwrap()["found"], true);
    let mut ledger = crate::harness_loop::MutationLedger::default();
    ledger.observe_tool("background", 0, output.metadata.as_ref());
    let incomplete = tokio::time::timeout(
        Duration::from_secs(3),
        crate::harness_loop::absorb_open_workspace_children(
            &mut ledger,
            owned.workspace.path(),
            &tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .expect("completion must not deadlock on a live background process");
    assert!(!incomplete);
    assert!(!ledger.has_open_children());
    assert!(ledger.paths().any(|path| path == "gate.txt"));
    assert_eq!(task.status().as_deref(), Some("terminated by request"));
    owned.close().await;
}

#[tokio::test]
async fn stop_releases_admission_without_reusing_fact_identity() {
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = Arc::new(DeterministicBackgroundSandbox::new(
        workspace.path().to_path_buf(),
        "ready",
        0,
        false,
    ));
    let owned = TestRun::in_workspace(workspace, Some(sandbox));
    let tool = BackgroundTool::default();
    assert!(
        start(&tool, &owned.ctx, "reusable", "printf first")
            .await
            .success
    );
    let first = task(&tool, &owned.ctx, "reusable");
    wait_settled(&first).await;
    assert!(tool.stop("reusable", &owned.ctx).await.unwrap().success);
    assert!(
        start(&tool, &owned.ctx, "reusable", "printf second")
            .await
            .success
    );
    let second = task(&tool, &owned.ctx, "reusable");
    wait_settled(&second).await;
    assert_ne!(first.task_id, second.task_id);
    let facts = owned.facts();
    assert_eq!(
        facts
            .iter()
            .filter(|fact| fact.payload["event"] == "exited")
            .count(),
        2
    );
    assert!(facts.iter().any(|fact| fact.payload["line"] == "ready-1"));
    assert!(facts.iter().any(|fact| fact.payload["line"] == "ready-2"));
    owned.close().await;
}
