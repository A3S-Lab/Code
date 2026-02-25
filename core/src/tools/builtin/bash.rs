//! Bash tool - Execute shell commands

use crate::tools::process::read_process_output;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

/// Default timeout in seconds (2 minutes)
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub struct BashTool;

/// Spawn a shell command cross-platform.
///
/// - Unix: `bash -c <command>`
/// - Windows: `cmd /C <command>` (falls back to PowerShell if cmd is unavailable)
fn spawn_shell(
    command: &str,
    workspace: &std::path::Path,
) -> std::io::Result<tokio::process::Child> {
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", command])
            .current_dir(workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    }
    #[cfg(not(windows))]
    {
        Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command in the workspace directory. Use for running commands, installing packages, running tests, etc."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default 120000)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("command parameter is required")),
        };

        // If a sandbox is configured, route execution through it.
        if let Some(ref sandbox) = ctx.sandbox {
            let result = sandbox
                .exec_command(command, "/workspace")
                .await
                .map_err(|e| anyhow::anyhow!("Sandbox bash execution failed: {}", e))?;

            // Combine stdout and stderr the same way the local path does.
            let mut output = result.stdout;
            if !result.stderr.is_empty() {
                output.push_str(&result.stderr);
            }

            return Ok(ToolOutput {
                content: output,
                success: result.exit_code == 0,
                metadata: Some(serde_json::json!({ "exit_code": result.exit_code })),
                images: vec![],
            });
        }

        // Local execution path (default when no sandbox is configured).
        let timeout_ms = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_MS);

        let timeout_secs = timeout_ms / 1000;

        let mut child = spawn_shell(command, std::path::Path::new(&ctx.workspace))
            .map_err(|e| anyhow::anyhow!("Failed to spawn shell: {}", e))?;

        let (output, timed_out) =
            read_process_output(&mut child, timeout_secs, ctx.event_tx.as_ref()).await;

        if timed_out {
            return Ok(ToolOutput::error(format!(
                "{}\n\n[Command timed out after {}ms]",
                output, timeout_ms
            )));
        }

        let status = child.wait().await?;
        let exit_code = status.code().unwrap_or(-1);

        Ok(ToolOutput {
            content: output,
            success: exit_code == 0,
            metadata: Some(serde_json::json!({ "exit_code": exit_code })),
            images: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
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
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool
            .execute(&serde_json::json!({"command": "echo hello"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn test_bash_exit_code() {
        let tool = BashTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

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
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
        assert!(result.content.contains("command"));
    }

    #[tokio::test]
    async fn test_bash_workspace_dir() {
        let temp = tempfile::tempdir().unwrap();
        let tool = BashTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(&serde_json::json!({"command": "pwd"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        // The output should contain the temp dir path
        let canonical = temp.path().canonicalize().unwrap();
        assert!(result
            .content
            .contains(&canonical.to_string_lossy().to_string()));
    }
}
