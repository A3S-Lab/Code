//! Grep tool - Search file contents with regex

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::MAX_OUTPUT_SIZE;
use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::process::Command;

/// Grep/ripgrep search tool
pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a pattern in files using ripgrep. Returns matching lines with file paths and line numbers."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in (default: workspace root)"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., '*.rs', '*.{ts,tsx}')"
                },
                "context": {
                    "type": "integer",
                    "description": "Number of context lines to show before and after matches"
                },
                "-i": {
                    "type": "boolean",
                    "description": "Case insensitive search"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let pattern = args["pattern"]
            .as_str()
            .context("Missing 'pattern' parameter")?;

        let path = args["path"].as_str().unwrap_or(".");
        let glob_filter = args["glob"].as_str();
        let context_lines = args["context"].as_u64().unwrap_or(0) as usize;
        let case_insensitive = args["-i"].as_bool().unwrap_or(false);

        let resolved_path = ctx.resolve_path(path)?;

        // Build ripgrep command
        let mut cmd = Command::new("rg");
        cmd.arg("--color=never")
            .arg("--line-number")
            .arg("--no-heading");

        if case_insensitive {
            cmd.arg("-i");
        }

        if context_lines > 0 {
            cmd.arg("-C").arg(context_lines.to_string());
        }

        if let Some(glob) = glob_filter {
            cmd.arg("--glob").arg(glob);
        }

        cmd.arg(pattern).arg(&resolved_path);

        let output = cmd.output().await.context("Failed to run ripgrep")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = stdout.to_string();
        if !stderr.is_empty() {
            result.push_str(&format!("\n{}", stderr));
        }

        // Truncate if too long
        if result.len() > MAX_OUTPUT_SIZE {
            result.truncate(MAX_OUTPUT_SIZE);
            result.push_str(&format!(
                "\n\n[Output truncated at {} bytes]",
                MAX_OUTPUT_SIZE
            ));
        }

        // ripgrep returns exit code 1 when no matches found, which is not an error
        let success = output.status.success() || output.status.code() == Some(1);

        Ok(ToolOutput {
            content: result,
            success,
            metadata: Some(serde_json::json!({
                "exit_code": output.status.code().unwrap_or(0)
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_grep_pattern() {
        let tool = GrepTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create a test file
        std::fs::write(
            temp_dir.path().join("test.txt"),
            "hello world\nfoo bar\nhello again",
        )
        .unwrap();

        let result = tool
            .execute(
                &serde_json::json!({
                    "pattern": "hello",
                    "path": "."
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn test_grep_no_match() {
        let tool = GrepTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create a test file
        std::fs::write(temp_dir.path().join("test.txt"), "hello world").unwrap();

        let result = tool
            .execute(
                &serde_json::json!({
                    "pattern": "nonexistent",
                    "path": "."
                }),
                &ctx,
            )
            .await
            .unwrap();

        // No match is still success (exit code 1 from rg)
        assert!(result.success);
    }

    #[test]
    fn test_grep_parameters() {
        let tool = GrepTool;
        let params = tool.parameters();

        assert!(params["properties"]["pattern"].is_object());
        assert!(params["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("pattern")));
    }
}
