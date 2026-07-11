use super::*;
use crate::sandbox::{BashSandbox, SandboxOutput};
use async_trait::async_trait;
use std::path::PathBuf;
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
    assert_eq!(result.metadata.unwrap()["exit_code"], 0);
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
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());
    let started = temp.path().join("started");
    let leaked = temp.path().join("leaked");

    let execution = tokio::spawn(async move {
        tool.execute(
            &serde_json::json!({
                "command": "printf started > started; sleep 1; printf leaked > leaked"
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
#[cfg(windows)]
async fn test_bash_head_compat_shim() {
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
async fn test_bash_json_normalizer_repairs_unquoted_object_literal() {
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
    use std::sync::{Arc, Mutex};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = Arc::new(Mutex::new(String::new()));
    let body_clone = Arc::clone(&body);

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).unwrap();
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let split = request.find("\r\n\r\n").unwrap();
        let payload = request[(split + 4)..].to_string();
        *body_clone.lock().unwrap() = payload;

        let response =
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}";
        stream.write_all(response).unwrap();
    });

    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());
    let command = format!(
            "curl.exe -sS -X POST \"http://127.0.0.1:{port}/capture\" -H \"Content-Type: application/json\" --data-raw {{image:nginx:alpine,name:mock-nginx,port_map:[18080:80],start:true}}"
        );

    let result = tool
        .execute(&serde_json::json!({ "command": command }), &ctx)
        .await
        .unwrap();

    server.join().unwrap();

    assert!(result.success, "{}", result.content);
    assert_eq!(
        body.lock().unwrap().as_str(),
        r#"{"image":"nginx:alpine","name":"mock-nginx","port_map":["18080:80"],"start":true}"#
    );
}

#[tokio::test]
async fn test_bash_inherits_command_env() {
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
    let tool = BashTool;
    let temp = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(temp.path().to_path_buf());

    let result = tool
        .execute(&serde_json::json!({"command": "exit /b 1"}), &ctx)
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
}

#[test]
#[cfg(windows)]
fn test_build_powershell_command_wraps_with_compat_shim() {
    let wrapped = build_powershell_command("curl -s http://127.0.0.1:18790/health | head -5");
    assert!(wrapped.contains("function curl"));
    assert!(wrapped.contains("function GET"));
    assert!(wrapped.contains("function head"));
    assert!(wrapped.contains("curl -s http://127.0.0.1:18790/health | head -5"));
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
