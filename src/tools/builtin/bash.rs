//! Bash tool - Execute shell commands

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::MAX_OUTPUT_SIZE;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Bash command execution tool
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
                    "description": "Timeout in milliseconds (default: 120000)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let command = args["command"]
            .as_str()
            .context("Missing 'command' parameter")?;

        let timeout_ms = args["timeout"].as_u64().unwrap_or(120_000);

        tracing::debug!("Bash command: {}", command);

        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn bash process")?;

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut output = String::new();
        let mut total_size = 0usize;
        let mut truncated = false;

        // Read output with timeout
        let timeout = tokio::time::Duration::from_millis(timeout_ms);
        let result = tokio::time::timeout(timeout, async {
            loop {
                tokio::select! {
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if total_size < MAX_OUTPUT_SIZE {
                                    output.push_str(&line);
                                    output.push('\n');
                                    total_size += line.len() + 1;
                                } else {
                                    truncated = true;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!("Error reading stdout: {}", e);
                                break;
                            }
                        }
                    }
                    line = stderr_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if total_size < MAX_OUTPUT_SIZE {
                                    output.push_str(&line);
                                    output.push('\n');
                                    total_size += line.len() + 1;
                                } else {
                                    truncated = true;
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tracing::warn!("Error reading stderr: {}", e);
                            }
                        }
                    }
                }
            }
        })
        .await;

        // Handle timeout
        if result.is_err() {
            child.kill().await.ok();
            return Ok(ToolOutput {
                content: format!("{}\n\n[Command timed out after {}ms]", output, timeout_ms),
                success: false,
                metadata: Some(serde_json::json!({ "exit_code": 124 })),
            });
        }

        let status = child.wait().await.context("Failed to wait for process")?;
        let exit_code = status.code().unwrap_or(-1);

        if truncated {
            output.push_str(&format!(
                "\n\n[Output truncated at {} bytes]",
                MAX_OUTPUT_SIZE
            ));
        }

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
    }

    #[test]
    fn test_bash_parameters() {
        let tool = BashTool;
        let params = tool.parameters();

        assert!(params["properties"]["command"].is_object());
        assert!(params["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("command")));
    }
}
