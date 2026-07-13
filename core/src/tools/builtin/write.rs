//! Write tool - Write content to files

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::workspace::WorkspaceError;
use anyhow::Result;
use async_trait::async_trait;

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Overwrite is the default. Large generated files can be written in bounded chunks by overwriting the first chunk, then appending later chunks with an expected byte offset for safe retries."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Required. Path to the file to write. Always provide this exact field name: 'file_path'."
                },
                "content": {
                    "type": "string",
                    "description": "Required. Content for this write. In overwrite mode this is the full replacement or first chunk; in append mode this is the next chunk. Always provide this exact field name: 'content'."
                },
                "mode": {
                    "type": "string",
                    "enum": ["overwrite", "append"],
                    "default": "overwrite",
                    "description": "Optional. 'overwrite' replaces the file and remains the default. 'append' adds one chunk and requires expected_offset."
                },
                "expected_offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Required in append mode. Expected current file size in UTF-8 bytes. A matching already-applied chunk is treated as a successful idempotent retry; any other mismatch is rejected. Providers that materialize optional integer fields may send 0 in overwrite mode; that compatibility value is ignored."
                }
            },
            "required": ["file_path", "content"],
            "examples": [
                {
                    "file_path": "notes.txt",
                    "content": "hello world"
                },
                {
                    "file_path": "report.html",
                    "content": "<section>next chunk</section>",
                    "mode": "append",
                    "expected_offset": 8192
                }
            ]
        })
    }

    fn capabilities(&self, args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        let mut capabilities = crate::tools::ToolCapabilities::conservative();
        capabilities.idempotent = true;
        capabilities.resumable =
            args.get("mode").and_then(|value| value.as_str()) == Some("append");
        capabilities.output_kind = crate::tools::ToolOutputKind::Diff;
        capabilities
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("file_path parameter is required")),
        };

        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("content parameter is required")),
        };

        let mode = match args.get("mode").and_then(|value| value.as_str()) {
            None | Some("overwrite") => "overwrite",
            Some("append") => "append",
            Some(other) => {
                return Ok(ToolOutput::error(format!(
                    "mode must be 'overwrite' or 'append', got '{other}'"
                )))
            }
        };

        let expected_offset = match (mode, args.get("expected_offset")) {
            ("append", Some(value)) => match value
                .as_u64()
                .and_then(|offset| usize::try_from(offset).ok())
            {
                Some(offset) => Some(offset),
                None => return Ok(ToolOutput::error(
                    "expected_offset must be a non-negative byte offset that fits this platform",
                )),
            },
            ("append", None) => {
                return Ok(ToolOutput::error(
                    "expected_offset parameter is required in append mode",
                ))
            }
            // Some model providers materialize every optional integer field in
            // a tool schema with its minimum value. Accepting the neutral value
            // keeps overwrite interoperable without weakening append's
            // compare-and-append contract.
            ("overwrite", Some(value)) if value.as_u64() == Some(0) => None,
            ("overwrite", Some(_)) => {
                return Ok(ToolOutput::error(
                    "expected_offset must be omitted or 0 in overwrite mode",
                ))
            }
            _ => None,
        };

        let workspace_path = match ctx.resolve_workspace_path(file_path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to resolve path: {}", e))),
        };

        if let Some(expected_offset) = expected_offset {
            let display_path = ctx.workspace_services.display_path(&workspace_path);
            let (mut current, version) =
                match ctx.workspace_services.read_for_edit(&workspace_path).await {
                    Ok(pair) => pair,
                    Err(error) => {
                        return Ok(ToolOutput::error(format!(
                            "Failed to append to file {}: {}. Start a segmented write with mode='overwrite'.",
                            display_path, error
                        )))
                    }
                };

            let resulting_size = match expected_offset.checked_add(content.len()) {
                Some(size) => size,
                None => {
                    return Ok(ToolOutput::error(
                        "append size exceeds the supported byte range",
                    ))
                }
            };
            if current.len() != expected_offset {
                let already_applied = current.len() == resulting_size
                    && current
                        .as_bytes()
                        .get(expected_offset..)
                        .is_some_and(|suffix| suffix == content.as_bytes());
                if already_applied {
                    return Ok(append_success_output(
                        file_path,
                        &display_path,
                        expected_offset,
                        content.len(),
                        resulting_size,
                        true,
                    ));
                }
                return Ok(ToolOutput::error(format!(
                    "Append offset mismatch for {}: expected {} bytes, found {} bytes. Read the current file tail or resume from the reported size; the file was not changed.",
                    display_path,
                    expected_offset,
                    current.len()
                )));
            }

            current.push_str(content);
            return match ctx
                .workspace_services
                .write_for_edit(&workspace_path, &current, version.as_deref())
                .await
            {
                Ok(_) => Ok(append_success_output(
                    file_path,
                    &display_path,
                    expected_offset,
                    content.len(),
                    resulting_size,
                    false,
                )),
                Err(error) => {
                    let typed = crate::tools::ToolErrorKind::from_workspace_error(&error);
                    let output = if matches!(error, WorkspaceError::VersionConflict(_)) {
                        ToolOutput::error(format!(
                            "Concurrent modification detected on {} while appending at byte offset {}. Re-read the file and retry from its current size.",
                            display_path, expected_offset
                        ))
                    } else {
                        ToolOutput::error(format!(
                            "Failed to append to file {}: {}",
                            display_path, error
                        ))
                    };
                    Ok(match typed {
                        Some(kind) => output.with_error_kind(kind),
                        None => output,
                    })
                }
            };
        }

        // Read existing content for diff metadata (if file exists)
        let fs = ctx.workspace_services.fs();
        let path_for_before = workspace_path.clone();
        let fs_for_before = fs.clone();
        let before_content = ctx
            .workspace_services
            .run_with_timeout("read_text", async move {
                fs_for_before.read_text(&path_for_before).await
            })
            .await
            .ok();

        let path_for_write = workspace_path.clone();
        let content_for_write = content.to_string();
        match ctx
            .workspace_services
            .run_with_timeout("write_text", async move {
                fs.write_text(&path_for_write, &content_for_write).await
            })
            .await
        {
            Ok(outcome) => {
                // Attach diff metadata
                let mut metadata = serde_json::Map::new();
                metadata.insert("file_path".to_string(), serde_json::json!(file_path));
                metadata.insert("after".to_string(), serde_json::json!(content));
                if let Some(before) = before_content {
                    metadata.insert("before".to_string(), serde_json::json!(before));
                }

                Ok(ToolOutput::success(format!(
                    "Wrote {} bytes ({} lines) to {}",
                    outcome.bytes,
                    outcome.lines,
                    ctx.workspace_services.display_path(&workspace_path)
                ))
                .with_metadata(serde_json::Value::Object(metadata)))
            }
            Err(e) => Ok(ToolOutput::error(format!(
                "Failed to write file {}: {}",
                ctx.workspace_services.display_path(&workspace_path),
                e
            ))),
        }
    }
}

fn append_success_output(
    file_path: &str,
    display_path: &str,
    offset: usize,
    appended_bytes: usize,
    resulting_size: usize,
    already_applied: bool,
) -> ToolOutput {
    let mut metadata = serde_json::Map::new();
    metadata.insert("file_path".to_string(), serde_json::json!(file_path));
    metadata.insert("mode".to_string(), serde_json::json!("append"));
    metadata.insert("offset".to_string(), serde_json::json!(offset));
    metadata.insert(
        "appended_bytes".to_string(),
        serde_json::json!(appended_bytes),
    );
    metadata.insert(
        "resulting_size".to_string(),
        serde_json::json!(resulting_size),
    );
    metadata.insert(
        "already_applied".to_string(),
        serde_json::json!(already_applied),
    );

    let content = if already_applied {
        format!(
            "Append chunk already applied at byte offset {} in {}; no duplicate content was written (file size: {} bytes)",
            offset, display_path, resulting_size
        )
    } else {
        format!(
            "Appended {} bytes at byte offset {} to {} (file size: {} bytes)",
            appended_bytes, offset, display_path, resulting_size
        )
    };
    ToolOutput::success(content).with_metadata(serde_json::Value::Object(metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_write_new_file() {
        let temp = tempfile::tempdir().unwrap();
        let tool = WriteTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({"file_path": "new.txt", "content": "hello world"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        let content = std::fs::read_to_string(temp.path().join("new.txt")).unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let tool = WriteTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({"file_path": "sub/dir/file.txt", "content": "nested"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        let content = std::fs::read_to_string(temp.path().join("sub/dir/file.txt")).unwrap();
        assert_eq!(content, "nested");
    }

    #[tokio::test]
    async fn test_write_overwrite_accepts_provider_materialized_zero_offset() {
        let temp = tempfile::tempdir().unwrap();
        let tool = WriteTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "provider-compatible.txt",
                    "content": "hello",
                    "mode": "overwrite",
                    "expected_offset": 0
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success, "{}", result.content);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("provider-compatible.txt")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn test_write_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("existing.txt"), "old").unwrap();

        let tool = WriteTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({"file_path": "existing.txt", "content": "new"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        let content = std::fs::read_to_string(temp.path().join("existing.txt")).unwrap();
        assert_eq!(content, "new");
    }

    #[tokio::test]
    async fn test_write_append_reconstructs_large_content() {
        let temp = tempfile::tempdir().unwrap();
        let tool = WriteTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let chunk = "0123456789abcdef".repeat(1024);
        let expected = chunk.repeat(16);

        tool.execute(
            &serde_json::json!({
                "file_path": "large.txt",
                "content": chunk,
                "mode": "overwrite"
            }),
            &ctx,
        )
        .await
        .unwrap();

        for index in 1..16 {
            let result = tool
                .execute(
                    &serde_json::json!({
                        "file_path": "large.txt",
                        "content": chunk,
                        "mode": "append",
                        "expected_offset": index * chunk.len()
                    }),
                    &ctx,
                )
                .await
                .unwrap();
            assert!(result.success, "{}", result.content);
        }

        let content = std::fs::read_to_string(temp.path().join("large.txt")).unwrap();
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn test_write_append_rejects_offset_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("existing.txt"), "prefix").unwrap();
        let tool = WriteTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "existing.txt",
                    "content": "suffix",
                    "mode": "append",
                    "expected_offset": 3
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(
            result.content.contains("offset mismatch"),
            "{}",
            result.content
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("existing.txt")).unwrap(),
            "prefix"
        );
    }

    #[tokio::test]
    async fn test_write_append_retry_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("existing.txt"), "prefixsuffix").unwrap();
        let tool = WriteTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let args = serde_json::json!({
            "file_path": "existing.txt",
            "content": "suffix",
            "mode": "append",
            "expected_offset": 6
        });

        let result = tool.execute(&args, &ctx).await.unwrap();

        assert!(result.success, "{}", result.content);
        assert!(
            result.content.contains("already applied"),
            "{}",
            result.content
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("existing.txt")).unwrap(),
            "prefixsuffix"
        );
    }

    #[tokio::test]
    async fn test_write_append_metadata_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("existing.txt"), "prefix").unwrap();
        let tool = WriteTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "existing.txt",
                    "content": "suffix",
                    "mode": "append",
                    "expected_offset": 6
                }),
                &ctx,
            )
            .await
            .unwrap();

        let metadata = result.metadata.expect("append metadata");
        assert_eq!(metadata["mode"], "append");
        assert_eq!(metadata["offset"], 6);
        assert_eq!(metadata["appended_bytes"], 6);
        assert_eq!(metadata["resulting_size"], 12);
        assert!(metadata.get("before").is_none());
        assert!(metadata.get("after").is_none());
    }

    #[tokio::test]
    async fn test_write_missing_params() {
        let tool = WriteTool;
        let ctx = ToolContext::new(std::path::PathBuf::from("/tmp"));

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);

        let result = tool
            .execute(&serde_json::json!({"file_path": "x"}), &ctx)
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_write_schema_is_canonical() {
        let tool = WriteTool;
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(
            params["required"],
            serde_json::json!(["file_path", "content"])
        );
        assert_eq!(
            params["properties"]["mode"]["enum"],
            serde_json::json!(["overwrite", "append"])
        );
        assert!(params["properties"].get("expected_offset").is_some());
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["file_path"], "notes.txt");
        assert!(examples[0].get("path").is_none());
    }
}
