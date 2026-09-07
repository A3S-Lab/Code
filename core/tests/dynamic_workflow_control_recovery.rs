//! Cross-process qualification for the dynamic-workflow control boundary.
//!
//! The helper test is intentionally launched through the test binary itself.
//! This keeps the fixture provider-free while still exercising independent
//! process address spaces, the shared Flow journal, and the shared lease
//! sidecar.

use a3s_code_core::tools::{Tool, ToolContext, ToolExecutor, ToolOutput};
use a3s_code_core::{
    CrossProcessFlowEventStore, DynamicWorkflowControlSnapshot, DynamicWorkflowRuntime,
    DynamicWorkflowTool, FlowDecisionClaimState, ToolCapabilities,
};
use a3s_flow::{FlowEngine, FlowEvent, FlowEventStore, WorkflowProgress};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};

const ROLE_ENV: &str = "A3S_DYNAMIC_WORKFLOW_CONTROL_ROLE";
const WORKSPACE_ENV: &str = "A3S_DYNAMIC_WORKFLOW_CONTROL_WORKSPACE";
const MARKER_ENV: &str = "A3S_DYNAMIC_WORKFLOW_CONTROL_MARKER";
const GO_ENV: &str = "A3S_DYNAMIC_WORKFLOW_CONTROL_GO";
const PROGRESS_ID_ENV: &str = "A3S_DYNAMIC_WORKFLOW_CONTROL_PROGRESS_ID";
const RUN_ID: &str = "cross-process-control-run";
const PROGRESS_RUN_ID: &str = "cross-process-progress-run";

const WORKFLOW_SOURCE: &str = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const cancellationRequested = inputs.history.some((entry) =>
      entry.event && entry.event.type === "run_cancellation_requested"
    );
    if (cancellationRequested) return { type: "cancel" };
    if (inputs.step_outputs.work) {
      return { type: "complete", output: { recovered: true } };
    }
    return {
      type: "schedule_step",
      step_id: "work",
      step_name: "task",
      input: {},
      retry: { max_attempts: 1, delay_ms: 0 },
    };
  }
  return await ctx.tool("task", inputs.input);
}
"#;

struct CrashBlockingTask {
    started_marker: PathBuf,
}

#[async_trait]
impl Tool for CrashBlockingTask {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "A qualification-only task that keeps a worker process alive."
    }

    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }

    fn capabilities(&self, _args: &Value) -> ToolCapabilities {
        ToolCapabilities::conservative()
    }

    async fn execute(&self, _args: &Value, _ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        tokio::fs::write(&self.started_marker, b"started").await?;
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(ToolOutput::success("unexpectedly released"))
    }
}

/// A fault-injecting event-store adapter used only by the process helper.
/// It forces one optimistic conflict before delegating the real append, so
/// Flow's bounded conflict retry is exercised deterministically.
struct ConflictOnceStore {
    inner: Arc<CrossProcessFlowEventStore>,
    injected: AtomicBool,
    progress_id: String,
}

#[async_trait]
impl FlowEventStore for ConflictOnceStore {
    async fn append(
        &self,
        run_id: &str,
        event: FlowEvent,
    ) -> a3s_flow::Result<a3s_flow::FlowEventEnvelope> {
        self.inner.append(run_id, event).await
    }

    async fn append_if_sequence(
        &self,
        run_id: &str,
        expected_sequence: u64,
        event: FlowEvent,
    ) -> a3s_flow::Result<a3s_flow::FlowEventEnvelope> {
        if self.injected.swap(true, Ordering::SeqCst) {
            return self
                .inner
                .append_if_sequence(run_id, expected_sequence, event)
                .await;
        }
        let injected = FlowEvent::RunProgressRecorded {
            progress: WorkflowProgress::new(format!("injected-{}", self.progress_id), 1)
                .with_total(2),
        };
        match self
            .inner
            .append_if_sequence(run_id, expected_sequence, injected)
            .await
        {
            Ok(_) | Err(a3s_flow::FlowError::EventConflict { .. }) => {}
            Err(error) => return Err(error),
        }
        self.inner
            .append_if_sequence(run_id, expected_sequence, event)
            .await
    }

    async fn list(&self, run_id: &str) -> a3s_flow::Result<Vec<a3s_flow::FlowEventEnvelope>> {
        self.inner.list(run_id).await
    }

    async fn list_run_ids(&self) -> a3s_flow::Result<Vec<String>> {
        self.inner.list_run_ids().await
    }
}

async fn wait_for_path(path: &Path) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if tokio::fs::try_exists(path).await? {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for {}", path.display()))??;
    Ok(())
}

fn helper_command(
    role: &str,
    workspace: &Path,
    marker: &Path,
    go: Option<&Path>,
) -> anyhow::Result<Command> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("--exact")
        .arg("dynamic_workflow_process_helper")
        .arg("--nocapture")
        .env(ROLE_ENV, role)
        .env(WORKSPACE_ENV, workspace)
        .env(MARKER_ENV, marker)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(go) = go {
        command.env(GO_ENV, go);
    }
    Ok(command)
}

async fn finish_child(child: Child, role: &str) -> anyhow::Result<()> {
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        anyhow::bail!(
            "{role} helper failed: status={}; stdout={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dynamic_workflow_control_recovers_after_process_crash_and_retries_conflicts(
) -> anyhow::Result<()> {
    let workspace = tempfile::tempdir()?;
    let owner_ready = workspace.path().join("owner-ready");
    let owner_started = workspace.path().join("owner-started");
    let recovered = workspace.path().join("recovered.json");

    let owner = helper_command("owner", workspace.path(), &owner_ready, None)?.spawn()?;
    wait_for_path(&owner_ready).await?;
    wait_for_path(&owner_started).await?;

    let executor = ToolExecutor::new(workspace.path().display().to_string());
    let observer_control = DynamicWorkflowTool::new(Arc::clone(executor.registry())).control(
        RUN_ID,
        WORKFLOW_SOURCE,
        json!({}),
        &executor.registry().context(),
    )?;
    let live = observer_control.inspect().await?;
    assert!(matches!(
        live.worker_lease,
        FlowDecisionClaimState::Pending { .. }
    ));
    assert!(live.worker_lease.is_live_at(current_time_ms()));
    let busy = observer_control
        .request_cancellation(Some("operator probe".to_string()))
        .await
        .expect_err("a live worker must retain the lease");
    assert!(
        busy.to_string().contains("worker lease is busy"),
        "{busy:#}"
    );

    let mut owner = owner;
    owner.kill().await?;
    let _ = owner.wait().await?;
    // The killed process no longer renews. Wait past the explicit owner lease
    // used by the helper before allowing takeover.
    tokio::time::sleep(Duration::from_millis(450)).await;

    let recovery = helper_command("recover", workspace.path(), &recovered, None)?.spawn()?;
    finish_child(recovery, "recover").await?;
    let recovered_snapshot: DynamicWorkflowControlSnapshot =
        serde_json::from_slice(&tokio::fs::read(&recovered).await?)?;
    assert_eq!(
        recovered_snapshot.status,
        a3s_flow::WorkflowRunStatus::Cancelled
    );
    assert!(recovered_snapshot.cancellation_requested);
    assert!(matches!(
        recovered_snapshot.worker_lease,
        FlowDecisionClaimState::Completed { .. }
    ));

    // Build a second suspended run, then ask two independent helper
    // processes to append progress through fault-injecting stores. Each
    // helper forces an EventConflict once; Flow must reload and retry.
    let progress_control = DynamicWorkflowTool::new(Arc::clone(executor.registry())).control(
        PROGRESS_RUN_ID,
        r#"async function run(ctx, inputs) {
          return { type: "wait_until", wait_id: "external", resume_at: "2099-01-01T00:00:00Z" };
        }"#,
        json!({}),
        &executor.registry().context(),
    )?;
    let progress = progress_control.drive().await?;
    assert_eq!(progress.status, a3s_flow::WorkflowRunStatus::Suspended);
    let progress_a_marker = workspace.path().join("progress-a-ready");
    let progress_b_marker = workspace.path().join("progress-b-ready");
    let progress_a = spawn_progress_helper(
        workspace.path(),
        "progress-a",
        &progress_a_marker,
        &workspace.path().join("progress-a-go"),
    )
    .await?;
    let progress_b = spawn_progress_helper(
        workspace.path(),
        "progress-b",
        &progress_b_marker,
        &workspace.path().join("progress-b-go"),
    )
    .await?;
    wait_for_path(&progress_a_marker).await?;
    wait_for_path(&progress_b_marker).await?;
    tokio::fs::write(workspace.path().join("progress-a-go"), b"go").await?;
    tokio::fs::write(workspace.path().join("progress-b-go"), b"go").await?;
    finish_child(progress_a, "progress-a").await?;
    finish_child(progress_b, "progress-b").await?;
    let history = progress_control.history().await?;
    let progress_ids = history
        .iter()
        .filter_map(|event| match &event.event {
            FlowEvent::RunProgressRecorded { progress } => Some(progress.progress_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(progress_ids.contains(&"progress-a"));
    assert!(progress_ids.contains(&"progress-b"));
    Ok(())
}

async fn spawn_progress_helper(
    workspace: &Path,
    progress_id: &str,
    marker: &Path,
    go: &Path,
) -> anyhow::Result<Child> {
    let mut command = helper_command("progress", workspace, marker, Some(go))?;
    command.env(PROGRESS_ID_ENV, progress_id);
    Ok(command.spawn()?)
}

fn current_time_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_workflow_process_helper() -> anyhow::Result<()> {
    let Some(role) = std::env::var_os(ROLE_ENV) else {
        return Ok(());
    };
    let role = role.to_string_lossy();
    let workspace = PathBuf::from(std::env::var(WORKSPACE_ENV)?);
    let marker = PathBuf::from(std::env::var(MARKER_ENV)?);
    match role.as_ref() {
        "owner" => {
            let executor = ToolExecutor::new(workspace.display().to_string());
            executor.register_dynamic_tool(Arc::new(CrashBlockingTask {
                started_marker: workspace.join("owner-started"),
            }));
            let control = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
                .with_continuation_lease_ms(250)
                .control(
                    RUN_ID,
                    WORKFLOW_SOURCE,
                    json!({}),
                    &executor.registry().context(),
                )?;
            tokio::fs::write(&marker, b"ready").await?;
            let _ = control.drive().await?;
        }
        "recover" => {
            let executor = ToolExecutor::new(workspace.display().to_string());
            let control = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
                .with_continuation_lease_ms(1_000)
                .control(
                    RUN_ID,
                    WORKFLOW_SOURCE,
                    json!({}),
                    &executor.registry().context(),
                )?;
            let snapshot = control
                .request_cancellation(Some("recovered after owner crash".to_string()))
                .await?;
            tokio::fs::write(&marker, serde_json::to_vec(&snapshot)?).await?;
        }
        "progress" => {
            let progress_id = std::env::var(PROGRESS_ID_ENV)?;
            let go = PathBuf::from(std::env::var(GO_ENV)?);
            let executor = ToolExecutor::new(workspace.display().to_string());
            let inner = Arc::new(CrossProcessFlowEventStore::new(
                a3s_code_core::dynamic_workflow_store_path(&workspace),
            ));
            let store = Arc::new(ConflictOnceStore {
                inner,
                injected: AtomicBool::new(false),
                progress_id: progress_id.clone(),
            });
            let runtime = Arc::new(DynamicWorkflowRuntime::new(
                Arc::clone(executor.registry()),
                executor.registry().context(),
                "async function run(ctx, inputs) { return { type: 'fail', error: 'unused' }; }",
            ));
            let engine = FlowEngine::new(store, runtime);
            tokio::fs::write(&marker, b"ready").await?;
            wait_for_path(&go).await?;
            engine
                .record_progress(
                    PROGRESS_RUN_ID,
                    WorkflowProgress::new(progress_id, 1).with_total(2),
                )
                .await?;
        }
        other => anyhow::bail!("unknown process-helper role {other}"),
    }
    Ok(())
}
