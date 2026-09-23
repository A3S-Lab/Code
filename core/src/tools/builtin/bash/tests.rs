use super::*;
use crate::sandbox::{BashSandbox, SandboxCommandRequest, SandboxExecutionOutput, SandboxOutput};
use crate::workspace::CommandOutputSummary;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const TEST_ESCALATION_JUSTIFICATION: &str =
    "This test explicitly exercises the approved host command runner.";

fn escalated_args(command: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "command": command.into(),
        "sandbox_permissions": "require_escalated",
        "justification": TEST_ESCALATION_JUSTIFICATION,
    })
}

fn escalated_args_with_timeout(command: impl Into<String>, timeout: u64) -> serde_json::Value {
    let mut args = escalated_args(command);
    args["timeout"] = serde_json::json!(timeout);
    args
}

// ------------------------------------------------------------------
// Mock sandbox for testing the delegation path
// ------------------------------------------------------------------

struct MockSandbox {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

struct HangingSandbox;

#[async_trait]
impl BashSandbox for HangingSandbox {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        std::future::pending().await
    }

    async fn shutdown(&self) {}
}

#[async_trait]
impl BashSandbox for MockSandbox {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        Ok(SandboxOutput {
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            exit_code: self.exit_code,
        })
    }

    async fn shutdown(&self) {}
}

#[derive(Debug, PartialEq, Eq)]
struct RecordedSandboxRequest {
    command: String,
    guest_workspace: String,
    timeout_ms: u64,
    env: Option<HashMap<String, String>>,
    had_output_observer: bool,
}

struct RecordingExtendedSandbox {
    called: Arc<AtomicBool>,
    request: Arc<std::sync::Mutex<Option<RecordedSandboxRequest>>>,
    output: SandboxExecutionOutput,
    summary: CommandOutputSummary,
}

#[async_trait]
impl BashSandbox for RecordingExtendedSandbox {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        anyhow::bail!("the bash tool must use the extended sandbox contract")
    }

    async fn exec(&self, request: SandboxCommandRequest) -> anyhow::Result<SandboxExecutionOutput> {
        self.called.store(true, Ordering::SeqCst);
        *self.request.lock().unwrap() = Some(RecordedSandboxRequest {
            command: request.command,
            guest_workspace: request.guest_workspace,
            timeout_ms: request.timeout_ms,
            env: request.env.as_deref().cloned(),
            had_output_observer: request.output_observer.is_some(),
        });
        if let Some(observer) = request.output_observer {
            observer.on_output_delta("streamed sandbox output").await;
            observer.on_output_complete(&self.summary).await;
        }
        Ok(SandboxExecutionOutput {
            stdout: self.output.stdout.clone(),
            stderr: self.output.stderr.clone(),
            exit_code: self.output.exit_code,
            timed_out: self.output.timed_out,
        })
    }

    async fn shutdown(&self) {}
}

#[tokio::test]
async fn test_bash_delegates_to_sandbox() {
    let tool = BashTool;
    let sandbox = Arc::new(MockSandbox {
        stdout: "sandbox output\n".into(),
        stderr: String::new(),
        exit_code: 0,
    });
    // `/tmp` is not an isolated workspace: other tests claim dirty paths under
    // it, and a workspace of `/tmp` treats every one of those claims as foreign.
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf()).with_sandbox(sandbox);

    let result = tool
        .execute(&serde_json::json!({"command": "echo ignored"}), &ctx)
        .await
        .unwrap();

    assert!(
        result.success,
        "sandbox delegation should succeed: {}",
        result.content
    );
    assert!(result.content.contains("sandbox output"));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["exit_code"], 0);
    assert_eq!(metadata["sandboxed"], true);
}

struct WorkspaceWritingSandbox {
    root: PathBuf,
}

#[async_trait]
impl BashSandbox for WorkspaceWritingSandbox {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        anyhow::bail!("the bash tool must use the extended sandbox contract")
    }

    async fn exec(
        &self,
        _request: SandboxCommandRequest,
    ) -> anyhow::Result<SandboxExecutionOutput> {
        std::fs::write(self.root.join("guest.txt"), "from sandbox\n").unwrap();
        Ok(SandboxExecutionOutput {
            stdout: "wrote".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        })
    }

    async fn shutdown(&self) {}
}

#[tokio::test]
async fn sandbox_bash_records_changed_paths_from_the_workspace_not_the_command() {
    let root = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(status.success(), "{args:?}");
    };
    git(&["init"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "test"]);
    std::fs::write(root.path().join("README.md"), "base\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-m", "base"]);

    let tool = BashTool;
    let ctx = ToolContext::new(root.path().to_path_buf()).with_sandbox(Arc::new(
        WorkspaceWritingSandbox {
            root: root.path().to_path_buf(),
        },
    ));
    let result = tool
        .execute(&serde_json::json!({"command": "printf done"}), &ctx)
        .await
        .unwrap();
    assert!(result.success);
    let paths = result.metadata.unwrap()["changed_paths"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        paths.iter().any(|path| path == "guest.txt"),
        "sandbox mutation must be recorded without parsing the command"
    );
}

#[tokio::test]
async fn default_sandbox_execution_preserves_timeout_env_and_streaming_contract() {
    let tool = BashTool;
    let called = Arc::new(AtomicBool::new(false));
    let request = Arc::new(std::sync::Mutex::new(None));
    let sandbox = Arc::new(RecordingExtendedSandbox {
        called: Arc::clone(&called),
        request: Arc::clone(&request),
        output: SandboxExecutionOutput {
            stdout: "sandbox result".to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        },
        summary: CommandOutputSummary {
            total_bytes: 23,
            captured_bytes: 23,
            truncated: false,
            timed_out: false,
        },
    });
    let temp = tempfile::tempdir().unwrap();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
    let ctx = ToolContext::new(temp.path().to_path_buf())
        .with_sandbox(sandbox)
        .with_command_env(Arc::new(HashMap::from([(
            "A3S_TEST_ENV".to_string(),
            "visible".to_string(),
        )])))
        .with_event_tx(event_tx);

    let result = tool
        .execute(
            &serde_json::json!({
                "command": "printf result",
                "timeout": 42,
                "sandbox_permissions": "use_default"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    assert!(called.load(Ordering::SeqCst));
    assert_eq!(
        request.lock().unwrap().as_ref(),
        Some(&RecordedSandboxRequest {
            command: "printf result".to_string(),
            guest_workspace: "/workspace".to_string(),
            timeout_ms: MIN_TIMEOUT_MS,
            env: Some(HashMap::from([(
                "A3S_TEST_ENV".to_string(),
                "visible".to_string(),
            )])),
            had_output_observer: true,
        })
    );
    assert!(matches!(
        event_rx.recv().await,
        Some(ToolStreamEvent::OutputDelta(delta))
            if delta == "streamed sandbox output"
    ));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["sandboxed"], true);
    assert_eq!(metadata["output"]["total_bytes"], 23);
}

#[tokio::test]
async fn escalated_execution_requires_a_justification() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
        .execute(
            &serde_json::json!({
                "command": "printf host",
                "sandbox_permissions": "require_escalated"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.success);
    assert!(result.content.contains("justification is required"));
}

#[tokio::test]
async fn default_execution_fails_closed_without_a_sandbox() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("must-not-exist");
    let ctx = ToolContext::new(temp.path().to_path_buf()).with_run_governance(None, None);

    let result = tool
        .execute(
            &serde_json::json!({"command": "printf escaped > must-not-exist"}),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.success);
    assert!(result.content.contains("requires a configured sandbox"));
    assert!(!marker.exists());
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["sandbox_available"], false);
    assert_eq!(metadata["sandboxed"], false);
}

#[tokio::test]
async fn escalated_execution_skips_the_configured_sandbox() {
    let tool = BashTool;
    let called = Arc::new(AtomicBool::new(false));
    let request = Arc::new(std::sync::Mutex::new(None));
    let sandbox = Arc::new(RecordingExtendedSandbox {
        called: Arc::clone(&called),
        request,
        output: SandboxExecutionOutput {
            stdout: "wrong boundary".to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        },
        summary: CommandOutputSummary {
            total_bytes: 0,
            captured_bytes: 0,
            truncated: false,
            timed_out: false,
        },
    });
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf()).with_sandbox(sandbox);

    let result = tool
        .execute(
            &serde_json::json!({
                "command": "printf host",
                "sandbox_permissions": "require_escalated",
                "justification": "The exact command needs an approved host capability."
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    assert_eq!(result.content.trim(), "host");
    assert!(
        !result.content.contains("wrong boundary"),
        "escalated command ran inside the sandbox: {}",
        result.content
    );
    assert!(!called.load(Ordering::SeqCst));
    assert_eq!(result.metadata.unwrap()["sandboxed"], false);
}

#[tokio::test]
async fn sandbox_timeout_uses_the_sandbox_result_and_metadata() {
    let tool = BashTool;
    let sandbox = Arc::new(RecordingExtendedSandbox {
        called: Arc::new(AtomicBool::new(false)),
        request: Arc::new(std::sync::Mutex::new(None)),
        output: SandboxExecutionOutput {
            stdout: "partial".to_string(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: true,
        },
        summary: CommandOutputSummary {
            total_bytes: 7,
            captured_bytes: 7,
            truncated: false,
            timed_out: true,
        },
    });
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf()).with_sandbox(sandbox);

    let result = tool
        .execute(
            &serde_json::json!({"command": "slow", "timeout": 1_500}),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.success);
    assert!(result.content.contains("timed out after 1500ms"));
    assert!(matches!(
        result.error_kind,
        Some(ToolErrorKind::Timeout {
            ref op,
            duration_ms: 1_500
        }) if op == "bash"
    ));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["sandboxed"], true);
    assert_eq!(metadata["timeout_ms"], 1_500);
    assert_eq!(metadata["output"]["timed_out"], true);
}

#[tokio::test(start_paused = true)]
async fn sandbox_execution_is_bounded_when_the_backend_ignores_the_timeout() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf()).with_sandbox(Arc::new(HangingSandbox));

    // Paused time does not wake `tokio::process`, so each porcelain git
    // query burns its full 400ms budget. A non-repo snapshot is status plus
    // rev-parse, and both the pre-exec watch and the timeout re-read pay it.
    // The product deadline is still `timeout`; this bound only proves the
    // tool returns instead of following a hanging sandbox.
    const PORCELAIN_GIT_BUDGET_MS: u64 = 400 * 4;
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(MIN_TIMEOUT_MS + PORCELAIN_GIT_BUDGET_MS),
        tool.execute(
            &serde_json::json!({"command": "hang forever", "timeout": MIN_TIMEOUT_MS}),
            &ctx,
        ),
    )
    .await
    .expect("the bash tool must enforce its own sandbox deadline")
    .unwrap();

    assert!(!result.success);
    assert!(result.content.contains("timed out after 1000ms"));
    assert!(matches!(
        result.error_kind,
        Some(ToolErrorKind::Timeout {
            ref op,
            duration_ms: MIN_TIMEOUT_MS
        }) if op == "bash"
    ));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["sandboxed"], true);
    assert_eq!(metadata["timeout_ms"], MIN_TIMEOUT_MS);
}

#[tokio::test]
async fn test_bash_sandbox_combines_stderr() {
    let tool = BashTool;
    let sandbox = Arc::new(MockSandbox {
        stdout: "out\n".into(),
        stderr: "err\n".into(),
        exit_code: 0,
    });
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf()).with_sandbox(sandbox);

    let result = tool
        .execute(&serde_json::json!({"command": "ls"}), &ctx)
        .await
        .unwrap();

    assert!(
        result.content.contains("out"),
        "sandbox stdout should be returned: {}",
        result.content
    );
    assert!(result.content.contains("err"));
}

#[tokio::test]
async fn test_bash_sandbox_nonzero_exit() {
    let tool = BashTool;
    let sandbox = Arc::new(MockSandbox {
        stdout: String::new(),
        stderr: "not found\n".into(),
        exit_code: 127,
    });
    let workspace = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(workspace.path().to_path_buf()).with_sandbox(sandbox);

    let result = tool
        .execute(&serde_json::json!({"command": "nonexistent"}), &ctx)
        .await
        .unwrap();

    assert!(!result.success);
    assert_eq!(result.metadata.unwrap()["exit_code"], 127);
}

#[tokio::test]
async fn test_bash_echo() {
    #[cfg(windows)]
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
        .execute(&escalated_args("echo hello"), &ctx)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.content.contains("hello"));
}

#[tokio::test]
async fn test_bash_tiny_timeout_is_clamped() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
        .execute(
            &escalated_args_with_timeout("sleep 0.05; printf done", 1),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    assert_eq!(result.content.trim_end(), "done");
}

#[tokio::test]
#[cfg(not(windows))]
async fn test_dropping_bash_execution_kills_shell_before_later_side_effects() {
    // Starting a shell and its background process is resource-sensitive on
    // macOS (especially on Intel runners). Serialize it with the other
    // process-backed tests so cancellation is exercised rather than masked by
    // scheduler starvation.
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());
    let started = temp.path().join("started");
    let leaked = temp.path().join("leaked");

    let execution = tokio::spawn(async move {
        tool.execute(
            &escalated_args("printf started > started; (sleep 8; printf leaked > leaked) & wait"),
            &ctx,
        )
        .await
    });

    let started_at = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !started.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        std::time::Instant::now()
    })
    .await
    .expect("shell should start before cancellation");

    execution.abort();
    let _ = execution.await;
    let remain = std::time::Duration::from_secs(9).saturating_sub(started_at.elapsed());
    tokio::time::sleep(remain).await;

    assert!(
        !leaked.exists(),
        "a cancelled bash execution must not continue later shell side effects"
    );
}

#[tokio::test]
#[cfg(windows)]
async fn test_dropping_bash_execution_kills_shell_before_later_side_effects() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());
    let started = temp.path().join("started");
    let child_started = temp.path().join("child-started");
    let leaked = temp.path().join("leaked");
    let started_literal = started.to_string_lossy().replace('\'', "''");
    let child_started_literal = child_started.to_string_lossy().replace('\'', "''");
    let leaked_literal = leaked.to_string_lossy().replace('\'', "''");
    let powershell =
        crate::tools::builtin::bash::windows_host_powershell(temp.path()).expect("PowerShell 7");
    let powershell_literal = powershell.to_string_lossy().replace('\'', "''");
    let command = format!(
        "Set-Content -LiteralPath '{started_literal}' -Value started; \
         $child = Start-Process -FilePath '{powershell_literal}' -PassThru -WindowStyle Hidden \
         -ArgumentList '-NoLogo','-NoProfile','-NonInteractive','-Command','Set-Content -LiteralPath ''{child_started_literal}'' -Value started; Start-Sleep -Seconds 8; Set-Content -LiteralPath ''{leaked_literal}'' -Value leaked'; \
         Wait-Process -Id $child.Id"
    );

    let execution = tokio::spawn(async move { tool.execute(&escalated_args(command), &ctx).await });

    let started_at = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        while !child_started.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        std::time::Instant::now()
    })
    .await
    .expect("descendant should start before cancellation");

    execution.abort();
    let _ = execution.await;
    let remain = std::time::Duration::from_secs(9).saturating_sub(started_at.elapsed());
    tokio::time::sleep(remain).await;

    assert!(
        !leaked.exists(),
        "a cancelled bash execution must not continue later shell side effects"
    );
}

#[tokio::test]
async fn test_bash_bounds_long_single_line_and_reports_exact_capture_metadata() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());
    #[cfg(windows)]
    let command = "[Console]::Out.Write(('x' * 120000))";
    #[cfg(not(windows))]
    let command = "printf '%*s' 120000 '' | tr ' ' x";

    let result = tool.execute(&escalated_args(command), &ctx).await.unwrap();

    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("command output truncated"));
    assert!(result.content.len() < 110_000);
    let output = &result.metadata.unwrap()["output"];
    assert_eq!(output["total_bytes"], 120_000);
    assert_eq!(output["captured_bytes"], crate::tools::MAX_OUTPUT_SIZE);
    assert_eq!(output["truncated"], true);
    assert_eq!(output["timed_out"], false);
}

#[tokio::test]
#[cfg(windows)]
async fn test_bash_head_compat_shim() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
        .execute(&escalated_args("1..5 | head -2"), &ctx)
        .await
        .unwrap();

    assert!(result.success);
    assert_eq!(result.content.lines().collect::<Vec<_>>(), vec!["1", "2"]);
}

#[tokio::test]
#[cfg(windows)]
async fn missing_powershell_fails_closed_without_cmd_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("cmd-fallback-marker.txt");
    let missing = temp.path().join("missing-powershell.exe");

    let result = spawn_windows_shell(
        missing.as_os_str(),
        "echo fallback>cmd-fallback-marker.txt",
        temp.path(),
        None,
    );

    let error = match result {
        Ok(mut child) => {
            let _ = child.wait().await;
            panic!(
                "a missing PowerShell executable must fail closed; cmd fallback created marker: {}",
                marker.exists()
            );
        }
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(error.to_string().contains("refusing to reinterpret"));
    assert!(!marker.exists());
}

#[tokio::test]
#[cfg(windows)]
async fn host_shell_starts_after_native_sandbox_write() {
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = crate::sandbox::native::NativeBashSandbox::new(workspace.path()).unwrap();
    let output = sandbox
        .exec_command(
            "[IO.File]::WriteAllText((Join-Path (Get-Location) 'after-sandbox.txt'), 'ok')",
            "/workspace",
        )
        .await
        .expect("sandboxed write");
    assert_eq!(
        output.exit_code, 0,
        "stdout={} stderr={}",
        output.stdout, output.stderr
    );
    assert!(workspace.path().join("after-sandbox.txt").is_file());

    let mut child = super::spawn_shell("Write-Output host-ok", workspace.path(), None)
        .expect("host PowerShell 7 must start after a sandboxed write");
    let status = child.wait().await.expect("host powershell wait");
    assert!(status.success(), "host powershell exit={status}");
}

#[tokio::test]
#[cfg(windows)]
async fn windows_native_sandbox_runs_workspace_local_node() {
    let node = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|dir| {
            let candidate = dir.join("node.exe");
            candidate.is_file().then_some(candidate)
        })
    });
    let Some(node) = node else {
        return;
    };
    let workspace = tempfile::tempdir().unwrap();
    // AppContainer is fail-closed for host Program Files binaries. Copy node
    // into the workspace so PATH resolution stays inside the sandbox root.
    let local_node = workspace.path().join("node.exe");
    std::fs::copy(&node, &local_node).expect("copy node.exe into workspace");
    std::fs::write(workspace.path().join("probe.mjs"), "process.exit(0)\n").unwrap();
    let sandbox = crate::sandbox::native::NativeBashSandbox::new(workspace.path()).unwrap();
    let output = sandbox
        .exec_command(".\\node.exe probe.mjs", "/workspace")
        .await
        .expect("sandboxed node");
    assert_eq!(
        output.exit_code, 0,
        "stdout={} stderr={}",
        output.stdout, output.stderr
    );
}

#[tokio::test]
#[cfg(windows)]
async fn windows_host_shell_test_and_grep_use_posix_exit_codes() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("verify-me.txt"), "verified\n").unwrap();
    std::fs::create_dir(workspace.path().join("nested")).unwrap();

    async fn exit_code(workspace: &std::path::Path, command: &str) -> (i32, String) {
        let child = super::spawn_shell(command, workspace, None)
            .unwrap_or_else(|error| panic!("spawn {command}: {error}"));
        let output = child.wait_with_output().await.expect("wait");
        let detail = format!(
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (output.status.code().unwrap_or(1), detail)
    }

    let (code, detail) = exit_code(workspace.path(), "test -f verify-me.txt").await;
    assert_eq!(code, 0, "{detail}");
    let (code, detail) = exit_code(workspace.path(), "test -f missing.txt").await;
    assert_eq!(code, 1, "{detail}");
    let (code, detail) = exit_code(workspace.path(), "test -d nested").await;
    assert_eq!(code, 0, "{detail}");
    let (code, detail) = exit_code(workspace.path(), "grep -q '^verified$' verify-me.txt").await;
    assert_eq!(code, 0, "{detail}");
    let (code, detail) = exit_code(workspace.path(), "grep -q '^absent$' verify-me.txt").await;
    assert_eq!(code, 1, "{detail}");
}

#[tokio::test]
#[cfg(windows)]
async fn windows_host_shell_long_command_keeps_its_exit_code() {
    let mut padding = 8_000usize;
    let command = loop {
        let candidate = format!("{}\nexit 3", "#".repeat(padding));
        let wrapped = super::build_powershell_command(&candidate);
        let encoded = super::encode_powershell_command(&wrapped);
        if encoded.len() + 256 > 30_000 {
            break candidate;
        }
        padding += 4_000;
        assert!(
            padding <= 80_000,
            "command did not exceed the Windows limit"
        );
    };
    let workspace = tempfile::tempdir().unwrap();
    let child = super::spawn_shell(&command, workspace.path(), None)
        .unwrap_or_else(|error| panic!("long host command failed to spawn: {error}"));
    let output = child.wait_with_output().await.expect("wait");
    assert_eq!(
        output.status.code(),
        Some(3),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[cfg(windows)]
async fn test_bash_json_normalizer_repairs_unquoted_object_literal() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
            .execute(
                &escalated_args("Write-Output (__a3s_normalize_json_like '{image:nginx:alpine,name:mock-nginx,port_map:[18080:80],start:true}')"),
                &ctx,
            )
            .await
            .unwrap();

    assert!(result.success);
    assert_eq!(
        result.content.trim_end(),
        r#"{"image":"nginx:alpine","name":"mock-nginx","port_map":["18080:80"],"start":true}"#
    );
}

#[tokio::test]
#[cfg(windows)]
async fn test_bash_json_normalizer_preserves_valid_json() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
            .execute(
                &escalated_args(r#"Write-Output (__a3s_normalize_json_like '{"image":"nginx:alpine","name":"mock-nginx","port_map":["18080:80"],"start":true}')"#),
                &ctx,
            )
            .await
            .unwrap();

    assert!(result.success);
    assert_eq!(
        result.content.trim_end(),
        r#"{"image":"nginx:alpine","name":"mock-nginx","port_map":["18080:80"],"start":true}"#
    );
}

#[tokio::test]
#[cfg(windows)]
async fn test_bash_curl_json_literal_is_normalized_end_to_end() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();

    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "curl never connected to test server"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("test server accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let split = request.find("\r\n\r\n").unwrap();
        let payload = request[(split + 4)..].to_string();

        let response =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}";
        stream.write_all(response).unwrap();
        payload
    });

    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());
    let command = format!(
        "curl.exe -sS -X POST \"http://127.0.0.1:{port}/capture\" -H \"Content-Type: application/json\" --data-raw '{{image:nginx:alpine,name:mock-nginx,port_map:[18080:80],start:true}}'"
    );
    assert!(parse_simple_windows_http_command(&command).is_some());

    let result = tool
        .execute(&escalated_args_with_timeout(command, 15_000), &ctx)
        .await
        .unwrap();

    let body = server.join().unwrap();

    assert!(result.success, "{}", result.content);
    assert_eq!(
        body,
        r#"{"image":"nginx:alpine","name":"mock-nginx","port_map":["18080:80"],"start":true}"#
    );
}

#[tokio::test]
async fn test_bash_inherits_command_env() {
    #[cfg(windows)]
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf()).with_command_env(std::sync::Arc::new(
        HashMap::from([("A3S_TEST_ENV".to_string(), "visible".to_string())]),
    ));
    #[cfg(windows)]
    let command = "Write-Output $env:A3S_TEST_ENV";
    #[cfg(not(windows))]
    let command = "printf '%s' \"$A3S_TEST_ENV\"";

    let result = tool.execute(&escalated_args(command), &ctx).await.unwrap();

    assert!(result.success);
    assert_eq!(result.content.trim_end(), "visible");
}

#[tokio::test]
#[cfg(not(windows))]
async fn test_bash_exit_code() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool.execute(&escalated_args("exit 1"), &ctx).await.unwrap();

    assert!(!result.success);
    assert_eq!(
        result.metadata.as_ref().unwrap()["exit_code"]
            .as_i64()
            .unwrap(),
        1
    );
}

#[tokio::test]
#[cfg(windows)]
async fn test_bash_exit_code() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool.execute(&escalated_args("exit 1"), &ctx).await.unwrap();

    assert!(!result.success);
    assert_eq!(
        result.metadata.as_ref().unwrap()["exit_code"]
            .as_i64()
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn test_bash_missing_command() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
    assert!(!result.success);
    assert!(result.content.contains("command"));
}

#[test]
fn sandbox_command_keeps_posix_test_visible_to_the_host_shell() {
    let command = super::command_for_sandbox("test -f hello.txt");
    #[cfg(not(windows))]
    assert_eq!(command, "test -f hello.txt");
    #[cfg(windows)]
    {
        assert!(command.contains("function test"));
        assert!(command.contains("test -f hello.txt"));
    }
}

#[test]
fn test_bash_schema_is_canonical() {
    let tool = BashTool;
    let params = tool.parameters();
    assert_eq!(params["additionalProperties"], false);
    assert_eq!(params["required"], serde_json::json!(["command"]));
    let examples = params["examples"].as_array().unwrap();
    assert_eq!(
        examples[0]["command"],
        "cargo test -p a3s-code-core skill::"
    );
    assert!(examples[0].get("cmd").is_none());
    assert!(!tool.requires_confirmation(&serde_json::json!({
        "command": "cargo test",
        "sandbox_permissions": "use_default"
    })));
    assert!(tool.requires_confirmation(&serde_json::json!({
        "command": "cargo test",
        "sandbox_permissions": "require_escalated",
        "justification": "Needs an approved host capability."
    })));
}

#[test]
#[cfg(windows)]
fn test_build_powershell_command_wraps_with_compat_shim() {
    let wrapped = build_powershell_command("curl -s http://127.0.0.1:18790/health | head -5");
    assert!(wrapped.contains("function curl"));
    assert!(wrapped.contains("function GET"));
    assert!(wrapped.contains("function head"));
    assert!(wrapped.contains("curl --% -s http://127.0.0.1:18790/health | head -5"));
}

#[test]
#[cfg(windows)]
fn test_preprocess_windows_command_wraps_curl_json_literal() {
    let command = r#"curl.exe -sS -X POST "http://127.0.0.1:18790/api" -H "Content-Type: application/json" --data-raw {image:nginx:alpine,name:mock-nginx,port_map:[18080:80],start:true}"#;
    let processed = preprocess_windows_command(command);
    assert!(processed.contains(
            r#"curl.exe --% -sS -X POST "http://127.0.0.1:18790/api" -H "Content-Type: application/json" --data-raw {"image":"nginx:alpine","name":"mock-nginx","port_map":["18080:80"],"start":true}"#
        ));
}

#[test]
#[cfg(windows)]
fn test_normalize_json_like_literal_repairs_object_literal() {
    let normalized = normalize_json_like_literal(
        "{image:nginx:alpine,name:mock-nginx,port_map:[18080:80],start:true}",
    )
    .unwrap();
    assert_eq!(
        normalized,
        r#"{"image":"nginx:alpine","name":"mock-nginx","port_map":["18080:80"],"start":true}"#
    );
}

#[tokio::test]
#[cfg(not(windows))]
async fn test_bash_workspace_dir() {
    let temp = tempfile::tempdir().unwrap();
    let tool = BashTool;
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool.execute(&escalated_args("pwd"), &ctx).await.unwrap();

    assert!(result.success);
    let canonical = temp.path().canonicalize().unwrap();
    assert!(result
        .content
        .contains(&canonical.to_string_lossy().to_string()));
}

#[tokio::test]
#[cfg(windows)]
async fn test_bash_workspace_dir() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let temp = tempfile::tempdir().unwrap();
    let tool = BashTool;
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
        .execute(
            &escalated_args("Get-Location | Select-Object -ExpandProperty Path"),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.success);
    let canonical = temp.path().canonicalize().unwrap();
    // canonicalize() on Windows returns \\?\ extended path prefix; strip it for comparison
    let canonical_str = canonical
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_lowercase();
    assert!(result.content.to_lowercase().contains(&canonical_str));
}

#[test]
fn prefix_session_cwd_without_session_leaves_command_unmodified() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());
    assert_eq!(prefix_session_cwd("pwd", &ctx), "pwd");
}

#[test]
fn prefix_session_cwd_prefixes_non_cd_commands_with_admitted_cwd() {
    let root = tempfile::tempdir().unwrap();
    let session = format!("bash-prefix-{}", std::process::id());
    crate::shell_session::bind_session(&session, root.path());
    let ctx = ToolContext::new(root.path().to_path_buf()).with_session_id(&session);
    let prefixed = prefix_session_cwd("pwd", &ctx);
    assert!(
        prefixed.starts_with("cd ") && prefixed.contains(" && pwd"),
        "expected cwd prefix, got {prefixed}"
    );
    crate::shell_session::drop_session(&session);
}

#[test]
fn shell_admission_denies_when_run_checker_denies() {
    struct DenyBash;

    impl crate::permissions::PermissionChecker for DenyBash {
        fn check(&self, _: &str, _: &serde_json::Value) -> crate::permissions::PermissionDecision {
            crate::permissions::PermissionDecision::Deny
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf())
        .with_run_governance(Some(Arc::new(DenyBash)), None);
    assert!(matches!(
        shell_admission(&ctx, "pwd"),
        crate::shell_session::CommandAdmission::Deny
    ));
}

#[test]
fn porcelain_diff_records_only_paths_that_changed() {
    let before = vec![" M keep.rs".to_string()];
    let after = vec![" M keep.rs".to_string(), "?? src/new.rs".to_string()];
    assert_eq!(
        changed_paths_from_porcelain(&before, &after),
        vec!["src/new.rs".to_string()]
    );
    assert!(
        changed_paths_from_porcelain(&["?? README.md".to_string()], &[])
            .contains(&"README.md".to_string())
    );
}

#[cfg(test)]
struct DenyNamedCommand;

#[cfg(test)]
impl crate::permissions::PermissionChecker for DenyNamedCommand {
    fn check(&self, _: &str, args: &serde_json::Value) -> crate::permissions::PermissionDecision {
        let command = args
            .get("command")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if command.contains("leaked") {
            crate::permissions::PermissionDecision::Deny
        } else {
            crate::permissions::PermissionDecision::Allow
        }
    }
}

#[test]
fn denied_second_command_does_not_move_the_same_shell() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let session = format!("bash-deny-{}", std::process::id());
    crate::shell_session::bind_session(&session, root.path());
    let allow = ToolContext::new(root.path().to_path_buf()).with_session_id(&session);
    prefix_session_cwd("cd nested", &allow);
    assert_eq!(crate::shell_session::cwd(&session).unwrap(), nested);
    let deny = allow.with_run_governance(Some(Arc::new(DenyNamedCommand)), None);
    let rejected = prefix_session_cwd("cd leaked", &deny);
    assert_eq!(rejected, "cd leaked");
    assert_eq!(crate::shell_session::cwd(&session).unwrap(), nested);
    assert!(!nested.join("leaked").exists());
    crate::shell_session::drop_session(&session);
}

#[tokio::test]
async fn detached_job_write_opens_the_parent_gate() {
    let root = tempfile::tempdir().unwrap();
    let session = format!("bash-detach-{}", std::process::id());
    crate::shell_session::bind_session(&session, root.path());
    let tool = BashTool;
    let ctx = ToolContext::new(root.path().to_path_buf()).with_session_id(&session);
    let output = tool
        .execute(
            &serde_json::json!({
                "job_action": "detach",
                "command": "sleep 0.2; printf 'hello\\n' > guest.txt"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(output.success, "detach failed: {output:#?}");
    let metadata = output.metadata.expect("detach metadata");
    let job_id = metadata["workspace_child"]
        .as_str()
        .expect("workspace_child");
    let mut ledger = crate::harness_loop::MutationLedger::default();
    ledger.observe_tool("bash", 0, Some(&metadata));
    assert!(
        ledger.has_open_children() || !ledger.is_empty(),
        "detached job did not open a parent effect: {metadata}"
    );
    if ledger.has_open_children() {
        assert!(matches!(
            crate::harness_loop::decide_completion(&ledger, &[], &[], true),
            crate::harness_loop::CompletionGate::Incomplete { .. }
        ));
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::harness_loop::absorb_open_workspace_children(
            &mut ledger,
            root.path(),
            &tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .expect("detached job should settle");
    crate::shell_session::drop_session(&session);
    assert!(root.path().join("guest.txt").is_file(), "job did not write");
    assert!(
        ledger.paths().any(|path| path == "guest.txt"),
        "parent gate missed detached write {job_id}; ledger paths missing"
    );
    assert!(!ledger.has_open_children());
}

#[tokio::test]
async fn detached_job_write_stays_owned_by_the_parent_session() {
    let root = tempfile::tempdir().unwrap();
    let session = format!("bash-detach-own-{}", std::process::id());
    crate::shell_session::bind_session(&session, root.path());
    let ctx = ToolContext::new(root.path().to_path_buf()).with_session_id(&session);
    let output = BashTool
        .execute(
            &serde_json::json!({
                "job_action": "detach",
                "command": "printf 'owned-by-detach\\n' > guest.txt"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(output.success, "detach failed: {output:#?}");
    let appeared = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if root.path().join("guest.txt").is_file() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(appeared.is_ok(), "detached job did not write");
    let owned = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if crate::external_observation::session_owns_write(&session, root.path(), "guest.txt") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    let late = format!("bash-detach-late-{}", std::process::id());
    let blocked =
        crate::external_observation::claim_bound_write(Some(&late), root.path(), "guest.txt");
    crate::external_observation::release_session(&session);
    crate::external_observation::release_session(&late);
    crate::shell_session::drop_session(&session);
    assert!(
        owned.is_ok(),
        "a finished detached write must stay owned by the parent session"
    );
    assert!(
        blocked.is_err(),
        "another session must not claim a path a detached job already wrote"
    );
}

struct OverwriteIfCalled {
    root: PathBuf,
    called: Arc<AtomicBool>,
    bytes: &'static str,
}

#[async_trait]
impl BashSandbox for OverwriteIfCalled {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        anyhow::bail!("the bash tool must use the extended sandbox contract")
    }

    async fn exec(
        &self,
        _request: SandboxCommandRequest,
    ) -> anyhow::Result<SandboxExecutionOutput> {
        self.called.store(true, Ordering::SeqCst);
        std::fs::write(self.root.join("guest.txt"), self.bytes).unwrap();
        Ok(SandboxExecutionOutput {
            stdout: "wrote".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        })
    }

    async fn shutdown(&self) {}
}

#[tokio::test]
async fn bash_owns_a_path_it_dirtied_so_another_session_cannot_overwrite_it() {
    let root = tempfile::tempdir().unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let owner = ToolContext::new(root.path().to_path_buf())
        .with_session_id("bash-writer")
        .with_sandbox(Arc::new(OverwriteIfCalled {
            root: root.path().to_path_buf(),
            called: Arc::clone(&called),
            bytes: "bash-token-4c91",
        }));
    let written = BashTool
        .execute(&serde_json::json!({"command": "overwrite"}), &owner)
        .await
        .unwrap();
    assert!(written.success, "{}", written.content);
    assert!(called.load(Ordering::SeqCst));
    assert_eq!(
        std::fs::read_to_string(root.path().join("guest.txt")).unwrap(),
        "bash-token-4c91"
    );
    let blocked =
        crate::external_observation::claim_bound_write(Some("bash-late"), root.path(), "guest.txt");
    assert!(
        blocked.is_err(),
        "bash must own the path it changed, not leave it for another session"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("guest.txt")).unwrap(),
        "bash-token-4c91"
    );
    crate::external_observation::release_session("bash-writer");
    crate::external_observation::release_session("bash-late");
}

#[tokio::test]
async fn bash_does_not_hide_another_sessions_dirty_path() {
    let root = tempfile::tempdir().unwrap();
    let guest = root.path().join("guest.txt");
    std::fs::write(&guest, "owned-token-7e2c").unwrap();
    crate::external_observation::claim_bound_write(Some("bash-owner"), root.path(), "guest.txt")
        .unwrap();

    let hidden = Arc::new(AtomicBool::new(false));
    let other = ToolContext::new(root.path().to_path_buf())
        .with_session_id("bash-other")
        .with_sandbox(Arc::new(OverwriteIfCalled {
            root: root.path().to_path_buf(),
            called: Arc::clone(&hidden),
            bytes: "hidden-token-7e2c",
        }));
    let denied = BashTool
        .execute(&serde_json::json!({"command": "overwrite"}), &other)
        .await
        .unwrap();
    assert!(!denied.success);
    assert!(!hidden.load(Ordering::SeqCst), "foreign bash ran");
    assert_eq!(std::fs::read_to_string(&guest).unwrap(), "owned-token-7e2c");

    let unbound_called = Arc::new(AtomicBool::new(false));
    let unbound =
        ToolContext::new(root.path().to_path_buf()).with_sandbox(Arc::new(OverwriteIfCalled {
            root: root.path().to_path_buf(),
            called: Arc::clone(&unbound_called),
            bytes: "unbound-token-7e2c",
        }));
    let unbound_denied = BashTool
        .execute(&serde_json::json!({"command": "overwrite"}), &unbound)
        .await
        .unwrap();
    assert!(!unbound_denied.success);
    assert!(
        !unbound_called.load(Ordering::SeqCst),
        "unbound bash hid a claimed path"
    );
    assert_eq!(std::fs::read_to_string(&guest).unwrap(), "owned-token-7e2c");

    let owner_called = Arc::new(AtomicBool::new(false));
    let owner = ToolContext::new(root.path().to_path_buf())
        .with_session_id("bash-owner")
        .with_sandbox(Arc::new(OverwriteIfCalled {
            root: root.path().to_path_buf(),
            called: Arc::clone(&owner_called),
            bytes: "owner-applied-7e2c",
        }));
    let applied = BashTool
        .execute(&serde_json::json!({"command": "overwrite"}), &owner)
        .await
        .unwrap();
    assert!(
        applied.success,
        "owner bash should still apply: {applied:?}"
    );
    assert!(owner_called.load(Ordering::SeqCst));
    assert_eq!(
        std::fs::read_to_string(&guest).unwrap(),
        "owner-applied-7e2c"
    );
    crate::external_observation::release_session("bash-owner");
}

#[tokio::test]
async fn bash_existence_check_metadata_enables_verified_completion() {
    use crate::harness_loop::{
        decide_completion, CompletionGate, CompletionTerminal, MutationLedger,
    };
    use crate::verification::host_report_for_verified_mutation_path_with_content;

    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("guest.txt"), "hello\n").unwrap();
    let output = BashTool
        .execute(
            &serde_json::json!({ "command": "test -f guest.txt" }),
            &ToolContext::new(root.path().to_path_buf()),
        )
        .await
        .unwrap();
    assert!(output.success, "existence check failed: {output:#?}");
    let metadata = output.metadata.expect("bash metadata");
    assert_eq!(
        metadata
            .get("verification_shell_command")
            .and_then(|v| v.as_str()),
        Some("test -f guest.txt"),
        "bash must retain the exact host check command for gate binding: {metadata}"
    );

    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "write",
        0,
        Some(&serde_json::json!({"file_path": "guest.txt", "after": "hello\n"})),
    );
    let digest = ledger.digest().to_string();
    let expected = ledger
        .content_digest_for_path("guest.txt")
        .expect("write records content digest")
        .to_string();
    let report = host_report_for_verified_mutation_path_with_content(
        metadata["verification_shell_command"].as_str().unwrap(),
        0,
        &["guest.txt".to_string()],
        &digest,
        Some((expected.as_str(), expected.as_str())),
    )
    .expect("mutated-path existence check should synthesize a host report");
    match decide_completion(&ledger, &[report], &[], false) {
        CompletionGate::Allow(CompletionTerminal::Verified { effect_digest }) => {
            assert_eq!(effect_digest, digest);
        }
        other => panic!("expected Allow(Verified), got {other:?}"),
    }
}

#[test]
fn bash_tool_name_and_description_are_stable() {
    let tool = BashTool;
    assert_eq!(tool.name(), "bash");
    assert!(tool.description().contains("shell command"));
    let params = tool.parameters();
    assert_eq!(params["required"], serde_json::json!(["command"]));
    assert!(tool.requires_confirmation(&serde_json::json!({
        "sandbox_permissions": "require_escalated"
    })));
    assert!(!tool.requires_confirmation(&serde_json::json!({
        "sandbox_permissions": "use_default"
    })));
}

#[tokio::test]
async fn unsupported_sandbox_permissions_value_is_rejected() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
        .execute(
            &serde_json::json!({
                "command": "printf host",
                "sandbox_permissions": "totally_custom"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.success);
    assert!(result
        .content
        .contains("unsupported sandbox_permissions value"));
}

#[tokio::test]
async fn unsupported_job_action_requires_bound_shell_session() {
    let tool = BashTool;
    let root = tempfile::tempdir().unwrap();
    let session = format!("bash-unsupported-action-{}", std::process::id());
    crate::shell_session::bind_session(&session, root.path());
    let ctx = ToolContext::new(root.path().to_path_buf()).with_session_id(&session);

    let result = tool
        .execute(&serde_json::json!({"job_action": "restart"}), &ctx)
        .await
        .unwrap();

    assert!(!result.success);
    assert!(result.content.contains("unsupported job_action"));
    crate::shell_session::drop_session(&session);
}

#[tokio::test]
async fn poll_missing_job_returns_tool_error() {
    let root = tempfile::tempdir().unwrap();
    let session = format!("bash-poll-missing-{}", std::process::id());
    crate::shell_session::bind_session(&session, root.path());
    let tool = BashTool;
    let ctx = ToolContext::new(root.path().to_path_buf()).with_session_id(&session);
    let result = tool
        .execute(
            &serde_json::json!({"job_action": "poll", "job_id": "does-not-exist"}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!result.success);
    crate::shell_session::drop_session(&session);
}

#[tokio::test]
async fn detach_refuses_when_workspace_has_foreign_dirty_path() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("guest.txt"), "owned").unwrap();
    crate::external_observation::claim_bound_write(Some("bash-owner"), root.path(), "guest.txt")
        .unwrap();
    let session = format!("bash-detach-foreign-{}", std::process::id());
    crate::shell_session::bind_session(&session, root.path());
    let ctx = ToolContext::new(root.path().to_path_buf()).with_session_id(&session);
    let denied = BashTool
        .execute(
            &serde_json::json!({
                "job_action": "detach",
                "command": "printf x > other.txt"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!denied.success);
    crate::shell_session::drop_session(&session);
    crate::external_observation::release_session("bash-owner");
}

#[tokio::test]
async fn poll_and_kill_job_actions_work_for_detached_jobs() {
    let root = tempfile::tempdir().unwrap();
    let session = format!("bash-poll-kill-{}", std::process::id());
    crate::shell_session::bind_session(&session, root.path());
    let tool = BashTool;
    let ctx = ToolContext::new(root.path().to_path_buf()).with_session_id(&session);

    let detached = tool
        .execute(
            &serde_json::json!({
                "job_action": "detach",
                "command": "sleep 30"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(detached.success, "{detached:?}");
    let job_id = detached.metadata.unwrap()["job_id"]
        .as_str()
        .expect("job_id")
        .to_string();

    let polled = tool
        .execute(
            &serde_json::json!({"job_action": "poll", "job_id": job_id}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(polled.success, "{polled:?}");
    assert!(
        polled.content == "running" || polled.content == "done",
        "unexpected poll status: {}",
        polled.content
    );

    let killed = tool
        .execute(
            &serde_json::json!({"job_action": "kill", "job_id": job_id}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(killed.success, "{killed:?}");
    assert_eq!(killed.content.trim(), "killed");

    crate::shell_session::drop_session(&session);
}

#[tokio::test]
async fn require_escalated_without_justification_is_rejected() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());
    let result = tool
        .execute(
            &serde_json::json!({
                "command": "printf host",
                "sandbox_permissions": "require_escalated",
                "justification": "   "
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!result.success);
    assert!(result
        .content
        .contains("justification is required when sandbox_permissions is require_escalated"));
}

#[tokio::test]
async fn job_actions_without_bound_session_fail_closed() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());
    let result = tool
        .execute(
            &serde_json::json!({ "job_action": "poll", "job_id": "missing" }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!result.success);
}

#[tokio::test]
async fn missing_command_parameter_returns_tool_error() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());
    let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
    assert!(!result.success);
    assert!(result.content.contains("command parameter is required"));
}

struct FailingSandbox;

#[async_trait]
impl BashSandbox for FailingSandbox {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        anyhow::bail!("the bash tool must use the extended sandbox contract")
    }

    async fn exec(
        &self,
        _request: SandboxCommandRequest,
    ) -> anyhow::Result<SandboxExecutionOutput> {
        anyhow::bail!("sandbox backend refused the request")
    }

    async fn shutdown(&self) {}
}

struct HangingSandboxWithSummary {
    summary: CommandOutputSummary,
}

#[async_trait]
impl BashSandbox for HangingSandboxWithSummary {
    async fn exec_command(
        &self,
        _command: &str,
        _guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        std::future::pending().await
    }

    async fn exec(&self, request: SandboxCommandRequest) -> anyhow::Result<SandboxExecutionOutput> {
        if let Some(observer) = request.output_observer {
            observer.on_output_delta("partial before hang").await;
            observer.on_output_complete(&self.summary).await;
        }
        std::future::pending().await
    }

    async fn shutdown(&self) {}
}

struct FailingWorkspaceCommandRunner;

#[async_trait]
impl crate::workspace::WorkspaceCommandRunner for FailingWorkspaceCommandRunner {
    async fn exec(
        &self,
        _request: crate::workspace::CommandRequest,
    ) -> anyhow::Result<crate::workspace::CommandOutput> {
        anyhow::bail!("workspace command runner refused")
    }
}

struct EmptyWorkspaceFs;

#[async_trait]
impl crate::workspace::WorkspaceFileSystem for EmptyWorkspaceFs {
    async fn read_text(
        &self,
        path: &crate::workspace::WorkspacePath,
    ) -> crate::workspace::WorkspaceResult<String> {
        Err(crate::workspace::WorkspaceError::NotFound {
            path: path.as_str().to_string(),
        })
    }

    async fn write_text(
        &self,
        _path: &crate::workspace::WorkspacePath,
        content: &str,
    ) -> crate::workspace::WorkspaceResult<crate::workspace::WorkspaceWriteOutcome> {
        Ok(crate::workspace::WorkspaceWriteOutcome {
            bytes: content.len(),
            lines: content.lines().count(),
        })
    }

    async fn list_dir(
        &self,
        _path: &crate::workspace::WorkspacePath,
    ) -> crate::workspace::WorkspaceResult<Vec<crate::workspace::WorkspaceDirEntry>> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn sandbox_backend_failure_is_surfaced_as_tool_error() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf()).with_sandbox(Arc::new(FailingSandbox));
    let err = tool
        .execute(&serde_json::json!({"command": "true"}), &ctx)
        .await
        .expect_err("sandbox Err must propagate");
    assert!(
        err.to_string().contains("Sandbox bash execution failed"),
        "{err}"
    );
}

#[tokio::test(start_paused = true)]
async fn sandbox_outer_timeout_includes_capture_summary_metadata() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf()).with_sandbox(Arc::new(
        HangingSandboxWithSummary {
            summary: CommandOutputSummary {
                total_bytes: 21,
                captured_bytes: 18,
                truncated: true,
                timed_out: false,
            },
        },
    ));
    const PORCELAIN_GIT_BUDGET_MS: u64 = 400 * 4;
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(MIN_TIMEOUT_MS + PORCELAIN_GIT_BUDGET_MS),
        tool.execute(
            &serde_json::json!({"command": "hang forever", "timeout": MIN_TIMEOUT_MS}),
            &ctx,
        ),
    )
    .await
    .expect("outer sandbox deadline must fire")
    .unwrap();
    assert!(!result.success);
    let metadata = result.metadata.expect("timeout metadata");
    assert_eq!(metadata["sandboxed"], true);
    assert_eq!(metadata["output"]["total_bytes"], 21);
    assert_eq!(metadata["output"]["captured_bytes"], 18);
    assert_eq!(metadata["output"]["truncated"], true);
    assert_eq!(metadata["output"]["timed_out"], true);
}

#[tokio::test]
async fn workspace_command_runner_failure_is_surfaced() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let services = crate::workspace::WorkspaceServices::builder(
        crate::workspace::WorkspaceRef::new("fail-ws", "memory://fail-ws"),
        Arc::new(EmptyWorkspaceFs) as Arc<dyn crate::workspace::WorkspaceFileSystem>,
    )
    .command_runner(Arc::new(FailingWorkspaceCommandRunner))
    .build();
    let ctx = ToolContext::new(temp.path().to_path_buf()).with_workspace_services(services);
    let err = tool
        .execute(&escalated_args("printf host"), &ctx)
        .await
        .expect_err("workspace runner Err must propagate");
    assert!(
        err.to_string().contains("Workspace bash execution failed"),
        "{err}"
    );
}

#[tokio::test]
async fn detach_of_cd_command_returns_tool_error() {
    let root = tempfile::tempdir().unwrap();
    let session = format!("bash-detach-cd-{}", std::process::id());
    crate::shell_session::bind_session(&session, root.path());
    let tool = BashTool;
    let ctx = ToolContext::new(root.path().to_path_buf()).with_session_id(&session);
    let output = tool
        .execute(
            &serde_json::json!({
                "job_action": "detach",
                "command": "cd nested"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!output.success);
    assert!(
        output.content.contains("not a detachable job"),
        "{}",
        output.content
    );
    crate::shell_session::drop_session(&session);
}

#[path = "sandbox_soak.rs"]
mod sandbox_soak;
