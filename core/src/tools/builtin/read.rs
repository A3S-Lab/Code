//! Read tool - Read file contents with line numbering

use crate::text::truncate_utf8;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::{MAX_LINE_LENGTH, MAX_READ_LINES};
use crate::workspace::WorkspaceTextRange;
use anyhow::Result;
use async_trait::async_trait;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns line-numbered output. Supports text files and images. Large outputs are capped; use offset/limit for long files."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Required. Path to the file to read, absolute or relative to the workspace. Always provide this exact field name: 'file_path'."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Optional. Line number to start reading from. 0-indexed. Default: 0."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_READ_LINES,
                    "description": "Optional. Maximum number of lines to read. Default and maximum: 2000."
                }
            },
            "required": ["file_path"],
            "examples": [
                {
                    "file_path": "src/main.rs"
                },
                {
                    "file_path": "src/main.rs",
                    "offset": 40,
                    "limit": 80
                }
            ]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        crate::tools::ToolCapabilities::read_only_paginated(16)
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("file_path parameter is required")),
        };

        let offset = match args.get("offset") {
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(value) => value,
                None => return Ok(ToolOutput::error("offset must be a non-negative integer")),
            },
            None => 0,
        };

        let requested_limit = match args.get("limit") {
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(0) | None => return Ok(ToolOutput::error("limit must be a positive integer")),
                Some(value) => value,
            },
            None => MAX_READ_LINES,
        };
        let limit = requested_limit.min(MAX_READ_LINES);

        let workspace_path = match ctx.resolve_workspace_path(file_path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to resolve path: {}", e))),
        };
        let range = match read_range(ctx, &workspace_path, offset, limit).await {
            Ok(range) => range,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Failed to read file {}: {}",
                    ctx.workspace_services.display_path(&workspace_path),
                    e
                )))
            }
        };
        if range.lines.is_empty()
            && range.eof
            && range.total_lines.is_some_and(|total| offset > total)
        {
            let total_lines = range.total_lines.unwrap_or_default();
            return Ok(ToolOutput::error(format!(
                "Offset {} exceeds file length ({} lines)",
                offset, total_lines
            )));
        }
        let mut metadata = serde_json::json!({
            "source_anchors": [workspace_path.as_str()],
            "range": {
                "offset": offset,
                "requested_limit": requested_limit,
                "applied_limit": limit,
                "returned_lines": range.lines.len(),
                "next_offset": range.next_offset,
                "eof": range.eof,
                "total_lines": range.total_lines,
                "limit_clamped": requested_limit != limit,
            }
        });
        if range.lines.is_empty() && range.eof {
            return Ok(ToolOutput::success(format!(
                "(end of file: offset {} equals file length)\n",
                offset
            ))
            .with_metadata(metadata));
        }

        let mut output = String::new();
        for (i, line) in range.lines.iter().enumerate() {
            let line_num = offset + i + 1; // 1-indexed
            let truncated = truncate_utf8(line, MAX_LINE_LENGTH);
            output.push_str(&format!("{:>6}\t{}\n", line_num, truncated));
        }

        if let Some(next_offset) = range.next_offset {
            output.push_str(&format!(
                "\n... (more lines available; continue with offset={next_offset})\n"
            ));
        }
        metadata["range"]["output_bytes"] = serde_json::json!(output.len());
        Ok(ToolOutput::success(output).with_metadata(metadata))
    }
}

async fn read_range(
    ctx: &ToolContext,
    path: &crate::workspace::WorkspacePath,
    offset: usize,
    limit: usize,
) -> crate::workspace::WorkspaceResult<WorkspaceTextRange> {
    if let Some(reader) = ctx.workspace_services.text_reader() {
        let path = path.clone();
        return ctx
            .workspace_services
            .run_with_timeout("read_text_range", async move {
                reader.read_text_range(&path, offset, limit).await
            })
            .await;
    }

    let fs = ctx.workspace_services.fs();
    let path = path.clone();
    let content = ctx
        .workspace_services
        .run_with_timeout("read_text", async move { fs.read_text(&path).await })
        .await?;
    let lines = content.lines().collect::<Vec<_>>();
    if offset >= lines.len() {
        return Ok(WorkspaceTextRange {
            lines: Vec::new(),
            next_offset: None,
            eof: true,
            total_lines: Some(lines.len()),
        });
    }
    let end = offset.saturating_add(limit).min(lines.len());
    Ok(WorkspaceTextRange {
        lines: lines[offset..end]
            .iter()
            .map(|line| (*line).to_string())
            .collect(),
        next_offset: (end < lines.len()).then_some(end),
        eof: end == lines.len(),
        total_lines: Some(lines.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{
        WorkspaceDirEntry, WorkspaceError, WorkspaceFileSystem, WorkspacePath, WorkspaceRef,
        WorkspaceResult, WorkspaceServices, WorkspaceTextReader, WorkspaceWriteOutcome,
    };
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::Arc;

    struct RangeOnlyBackend;

    #[async_trait]
    impl WorkspaceFileSystem for RangeOnlyBackend {
        async fn read_text(&self, _path: &WorkspacePath) -> WorkspaceResult<String> {
            panic!("range-capable read must not fall back to whole-file read_text")
        }

        async fn write_text(
            &self,
            _path: &WorkspacePath,
            _content: &str,
        ) -> WorkspaceResult<WorkspaceWriteOutcome> {
            Err(WorkspaceError::InvalidArgument {
                message: "write_text is unsupported".to_string(),
            })
        }

        async fn list_dir(&self, _path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
            Err(WorkspaceError::InvalidArgument {
                message: "list_dir is unsupported".to_string(),
            })
        }
    }

    #[async_trait]
    impl WorkspaceTextReader for RangeOnlyBackend {
        async fn read_text_range(
            &self,
            _path: &WorkspacePath,
            offset: usize,
            limit: usize,
        ) -> WorkspaceResult<WorkspaceTextRange> {
            assert_eq!(offset, 7);
            assert_eq!(limit, 2);
            Ok(WorkspaceTextRange {
                lines: vec!["eight".to_string(), "nine".to_string()],
                next_offset: Some(9),
                eof: false,
                total_lines: None,
            })
        }
    }

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
        assert_eq!(
            result.metadata.unwrap()["source_anchors"],
            serde_json::json!(["test.txt"])
        );
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
    async fn test_read_uses_streaming_range_capability_without_whole_file_read() {
        let backend = Arc::new(RangeOnlyBackend);
        let fs: Arc<dyn WorkspaceFileSystem> = backend.clone();
        let reader: Arc<dyn WorkspaceTextReader> = backend;
        let services = WorkspaceServices::builder(WorkspaceRef::new("range", "range://ws"), fs)
            .text_reader(reader)
            .build();
        let ctx = ToolContext::new(std::env::temp_dir()).with_workspace_services(services);

        let result = ReadTool
            .execute(
                &serde_json::json!({"file_path": "large.txt", "offset": 7, "limit": 2}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("eight"));
        assert!(result.content.contains("offset=9"));
    }

    #[tokio::test]
    async fn test_read_clamps_oversized_limit_and_reports_metadata() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "one\ntwo\n").unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = ReadTool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "limit": MAX_READ_LINES + 1,
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success, "{}", result.content);
        let range = &result.metadata.unwrap()["range"];
        assert_eq!(range["applied_limit"], MAX_READ_LINES);
        assert_eq!(range["limit_clamped"], true);
    }

    #[tokio::test]
    async fn test_read_at_eof_is_successful_empty_tail() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("test.txt");
        std::fs::write(&file, "a\nb\nc\n").unwrap();

        let tool = ReadTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(
                &serde_json::json!({"file_path": "test.txt", "offset": 3, "limit": 20}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("end of file"));
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

    #[test]
    fn test_read_schema_is_canonical() {
        let tool = ReadTool;
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["required"], serde_json::json!(["file_path"]));
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["file_path"], "src/main.rs");
        assert!(examples[0].get("path").is_none());
    }

    #[tokio::test]
    async fn test_read_truncation_at_utf8_boundary() {
        // Regression test: truncation at byte 2000 should not panic
        // when byte 2000 falls inside a multibyte UTF-8 character.
        // "频" is 3 bytes (bytes 1999..2002). When byte 2000 is
        // inside '频', truncation must find a valid char boundary.
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("boundary.txt");
        // 1999 ASCII bytes + one 3-byte UTF-8 char + trailing ASCII.
        // Byte 2000 is inside the '频' character (bytes 1999..2002).
        let content = "a".repeat(1999) + "频" + &"z".repeat(20);
        std::fs::write(&file, &content).unwrap();

        let tool = ReadTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        // Should not panic
        let result = tool
            .execute(&serde_json::json!({"file_path": "boundary.txt"}), &ctx)
            .await
            .unwrap();

        assert!(
            result.success,
            "read should succeed, got error: {}",
            result.content
        );
        // Verify the truncated content is valid UTF-8
        assert!(!result.content.contains("byte index"));
    }
}
