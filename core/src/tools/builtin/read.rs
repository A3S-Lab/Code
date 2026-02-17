//! Read tool - Read file contents with line numbering

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::{MAX_LINE_LENGTH, MAX_READ_LINES};
use anyhow::Result;
use async_trait::async_trait;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns line-numbered output. Supports text files and images."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to read (absolute or relative to workspace)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-indexed, default 0)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (default 2000)"
                }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("file_path parameter is required")),
        };

        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(MAX_READ_LINES);

        let resolved = match ctx.resolve_path(file_path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to resolve path: {}", e))),
        };

        let content = match tokio::fs::read_to_string(&resolved).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Failed to read file {}: {}",
                    resolved.display(),
                    e
                )))
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        if offset >= total_lines && total_lines > 0 {
            return Ok(ToolOutput::error(format!(
                "Offset {} exceeds file length ({} lines)",
                offset, total_lines
            )));
        }

        let end = (offset + limit).min(total_lines);
        let selected = &lines[offset..end];

        let mut output = String::new();
        for (i, line) in selected.iter().enumerate() {
            let line_num = offset + i + 1; // 1-indexed
            let truncated = if line.len() > MAX_LINE_LENGTH {
                &line[..MAX_LINE_LENGTH]
            } else {
                line
            };
            output.push_str(&format!("{:>6}\t{}\n", line_num, truncated));
        }

        if end < total_lines {
            output.push_str(&format!(
                "\n... ({} more lines not shown, use offset/limit to read more)\n",
                total_lines - end
            ));
        }

        Ok(ToolOutput::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_read_file() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("test.txt");
        std::fs::write(&file, "line1\nline2\nline3\n").unwrap();

        let tool = ReadTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(&serde_json::json!({"file_path": "test.txt"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("line1"));
        assert!(result.content.contains("line2"));
        assert!(result.content.contains("line3"));
    }

    #[tokio::test]
    async fn test_read_with_offset_and_limit() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("test.txt");
        std::fs::write(&file, "a\nb\nc\nd\ne\n").unwrap();

        let tool = ReadTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(
                &serde_json::json!({"file_path": "test.txt", "offset": 1, "limit": 2}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("b"));
        assert!(result.content.contains("c"));
        assert!(!result.content.contains("\ta\n"));
    }

    #[tokio::test]
    async fn test_read_missing_file() {
        let temp = tempfile::tempdir().unwrap();
        let tool = ReadTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(&serde_json::json!({"file_path": "nonexistent.txt"}), &ctx)
            .await
            .unwrap();

        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_read_missing_param() {
        let tool = ReadTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();

        assert!(!result.success);
        assert!(result.content.contains("file_path"));
    }
}
