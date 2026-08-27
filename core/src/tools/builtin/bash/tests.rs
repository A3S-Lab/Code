use super::*;
use crate::sandbox::{BashSandbox, SandboxCommandRequest, SandboxExecutionOutput, SandboxOutput};
use crate::workspace::CommandOutputSummary;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ------------------------------------------------------------------
// Mock sandbox for testing the delegation path
// ------------------------------------------------------------------

struct MockSandbox {
    stdout: String,
    stderr: String,
    exit_code: i32,
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
    let ctx = ToolContext::new(PathBuf::from("/tmp")).with_sandbox(sandbox);

    let result = tool
        .execute(&serde_json::json!({"command": "echo ignored"}), &ctx)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.content.contains("sandbox output"));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["exit_code"], 0);
    assert_eq!(metadata["sandboxed"], true);
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
#[cfg(not(windows))]
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
    assert_eq!(result.content, "host");
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

#[tokio::test]
async fn test_bash_sandbox_combines_stderr() {
    let tool = BashTool;
    let sandbox = Arc::new(MockSandbox {
        stdout: "out\n".into(),
        stderr: "err\n".into(),
        exit_code: 0,
    });
    let ctx = ToolContext::new(PathBuf::from("/tmp")).with_sandbox(sandbox);

    let result = tool
        .execute(&serde_json::json!({"command": "ls"}), &ctx)
        .await
        .unwrap();

    assert!(result.content.contains("out"));
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
    let ctx = ToolContext::new(PathBuf::from("/tmp")).with_sandbox(sandbox);

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
        .execute(&serde_json::json!({"command": "echo hello"}), &ctx)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.content.contains("hello"));
}

#[tokio::test]
#[cfg(not(windows))]
async fn test_bash_tiny_timeout_is_clamped() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
        .execute(
            &serde_json::json!({
                "command": "sleep 0.05; printf done",
                "timeout": 1
            }),
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
            &serde_json::json!({
                "command": "printf started > started; (sleep 1; printf leaked > leaked) & wait"
            }),
            &ctx,
        )
        .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !started.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("shell should start before cancellation");

    execution.abort();
    let _ = execution.await;
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;

    assert!(
        !leaked.exists(),
        "a cancelled bash execution must not continue later shell side effects"
    );
}

#[tokio::test]
#[cfg(not(windows))]
async fn test_bash_bounds_long_single_line_and_reports_exact_capture_metadata() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
        .execute(
            &serde_json::json!({
                "command": "printf '%*s' 120000 '' | tr ' ' x"
            }),
            &ctx,
        )
        .await
        .unwrap();

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
        .execute(&serde_json::json!({"command": "1..5 | head -2"}), &ctx)
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
async fn test_bash_json_normalizer_repairs_unquoted_object_literal() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
            .execute(
                &serde_json::json!({
                    "command": "Write-Output (__a3s_normalize_json_like '{image:nginx:alpine,name:mock-nginx,port_map:[18080:80],start:true}')"
                }),
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
                &serde_json::json!({
                    "command": r#"Write-Output (__a3s_normalize_json_like '{"image":"nginx:alpine","name":"mock-nginx","port_map":["18080:80"],"start":true}')"#
                }),
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
        .execute(
            &serde_json::json!({ "command": command, "timeout": 15_000 }),
            &ctx,
        )
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

    let result = tool
        .execute(&serde_json::json!({ "command": command }), &ctx)
        .await
        .unwrap();

    assert!(result.success);
    assert_eq!(result.content.trim_end(), "visible");
}

#[tokio::test]
#[cfg(not(windows))]
async fn test_bash_exit_code() {
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
        .execute(&serde_json::json!({"command": "exit 1"}), &ctx)
        .await
        .unwrap();

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

    let result = tool
        .execute(&serde_json::json!({"command": "exit 1"}), &ctx)
        .await
        .unwrap();

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

    let result = tool
        .execute(&serde_json::json!({"command": "pwd"}), &ctx)
        .await
        .unwrap();

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
            &serde_json::json!({"command": "Get-Location | Select-Object -ExpandProperty Path"}),
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
