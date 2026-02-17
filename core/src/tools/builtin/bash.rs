//! Bash tool - Execute shell commands

use crate::tools::dynamic::read_process_output;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

/// Default timeout in seconds (2 minutes)
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub struct BashTool;

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

        let timeout_ms = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_MS);

        let timeout_secs = timeout_ms / 1000;

        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn bash: {}", e))?;

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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
