//! Script tool - Execute scripts with interpreters

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::MAX_OUTPUT_SIZE;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Tool that executes scripts with an interpreter
pub struct ScriptTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    /// Interpreter command (bash, python, node, etc.)
    interpreter: String,
    /// Script content
    script: String,
    /// Additional interpreter arguments
    interpreter_args: Vec<String>,
}

impl ScriptTool {
    pub fn new(
        name: String,
        description: String,
        parameters: serde_json::Value,
        interpreter: String,
        script: String,
        interpreter_args: Vec<String>,
    ) -> Self {
        Self {
            name,
            description,
            parameters,
            interpreter,
            script,
            interpreter_args,
        }
    }
}

#[async_trait]
impl Tool for ScriptTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        tracing::debug!("Executing script with interpreter: {}", self.interpreter);

        let mut cmd = Command::new(&self.interpreter);

        // Add interpreter arguments
        for arg in &self.interpreter_args {
            cmd.arg(arg);
        }

        // For most interpreters, we can pass script via stdin
        // Add "-" to indicate reading from stdin (works for bash, python, etc.)
        match self.interpreter.as_str() {
            "bash" | "sh" | "zsh" => {
                cmd.arg("-s"); // Read from stdin
            }
            "python" | "python3" => {
                cmd.arg("-"); // Read from stdin
            }
            "node" => {
                cmd.arg("-e").arg(&self.script); // Execute inline
            }
            _ => {
                // For unknown interpreters, try stdin
                cmd.arg("-");
            }
        }

        cmd.current_dir(&ctx.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Pass arguments as environment variables
        if let Some(obj) = args.as_object() {
            for (key, value) in obj {
                let env_key = format!("TOOL_ARG_{}", key.to_uppercase());
                let env_value = match value {
                    serde_json::Value::String(s) => s.clone(),
                    _ => value.to_string(),
                };
                cmd.env(env_key, env_value);
            }
        }
        // Also pass full args as JSON
        cmd.env("TOOL_ARGS", args.to_string());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn interpreter: {}", self.interpreter))?;

        // Write script to stdin (except for node which uses -e)
        if self.interpreter != "node" {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(self.script.as_bytes()).await?;
                stdin.shutdown().await?;
            }
        }

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut output = String::new();
        let mut total_size = 0usize;

        // Read output with timeout (60 seconds for scripts)
        let timeout = tokio::time::Duration::from_secs(60);
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
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                    line = stderr_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if total_size < MAX_OUTPUT_SIZE {
                                    output.push_str(&line);
                                    output.push('\n');
                                    total_size += line.len() + 1;
                                }
                            }
                            Ok(None) => {}
                            Err(_) => {}
                        }
                    }
                }
            }
        })
        .await;

        if result.is_err() {
            child.kill().await.ok();
            return Ok(ToolOutput::error(format!(
                "{}\n\n[Script execution timed out after 60s]",
                output
            )));
        }

        let status = child.wait().await?;
        let exit_code = status.code().unwrap_or(-1);

        Ok(ToolOutput {
            content: output,
            success: exit_code == 0,
            metadata: Some(serde_json::json!({
                "exit_code": exit_code,
                "interpreter": self.interpreter
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_script_tool_bash() {
        let tool = ScriptTool::new(
            "test".to_string(),
            "A test script".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"}
                }
            }),
            "bash".to_string(),
            "echo \"Hello, $TOOL_ARG_NAME!\"".to_string(),
            vec![],
        );

        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool
            .execute(&serde_json::json!({"name": "World"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Hello, World!"));
    }

    #[tokio::test]
    async fn test_script_tool_python() {
        let tool = ScriptTool::new(
            "test".to_string(),
            "A Python script".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            "python3".to_string(),
            "import os\nprint('Python works!')".to_string(),
            vec![],
        );

        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Python works!"));
    }

    #[tokio::test]
    async fn test_script_tool_with_args() {
        let tool = ScriptTool::new(
            "test".to_string(),
            "Script with args".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer"},
                    "y": {"type": "integer"}
                }
            }),
            "bash".to_string(),
            "echo $(($TOOL_ARG_X + $TOOL_ARG_Y))".to_string(),
            vec![],
        );

        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool
            .execute(&serde_json::json!({"x": 10, "y": 20}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("30"));
    }

    #[tokio::test]
    async fn test_script_tool_failure() {
        let tool = ScriptTool::new(
            "test".to_string(),
            "Failing script".to_string(),
            serde_json::json!({}),
            "bash".to_string(),
            "exit 1".to_string(),
            vec![],
        );

        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();

        assert!(!result.success);
    }

    #[test]
    fn test_script_tool_parameters() {
        let tool = ScriptTool::new(
            "test".to_string(),
            "test".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string"}
                },
                "required": ["input"]
            }),
            "bash".to_string(),
            "echo $TOOL_ARG_INPUT".to_string(),
            vec![],
        );

        let params = tool.parameters();
        assert!(params["properties"]["input"].is_object());
    }
}
