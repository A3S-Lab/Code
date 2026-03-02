//! Write tool - Write content to files

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file and parent directories if they don't exist."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
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

        // Build the target path, creating parent dirs if needed
        let target = if std::path::Path::new(file_path).is_absolute() {
            std::path::PathBuf::from(file_path)
        } else {
            ctx.workspace.join(file_path)
        };

        // Create parent directories first (before resolve_path_for_write which checks parent exists)
        if let Some(parent) = target.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return Ok(ToolOutput::error(format!(
                    "Failed to create parent directories for {}: {}",
                    target.display(),
                    e
                )));
            }
        }

        let resolved = match ctx.resolve_path_for_write(file_path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to resolve path: {}", e))),
        };

        // Read existing content for diff metadata (if file exists)
        let before_content = if resolved.exists() {
            tokio::fs::read_to_string(&resolved).await.ok()
        } else {
            None
        };

        match tokio::fs::write(&resolved, content).await {
            Ok(()) => {
                let lines = content.lines().count();
                let bytes = content.len();

                // Attach diff metadata
                let mut metadata = serde_json::Map::new();
                metadata.insert("file_path".to_string(), serde_json::json!(file_path));
                metadata.insert("after".to_string(), serde_json::json!(content));
                if let Some(before) = before_content {
                    metadata.insert("before".to_string(), serde_json::json!(before));
                }

                Ok(ToolOutput::success(format!(
                    "Wrote {} bytes ({} lines) to {}",
                    bytes,
                    lines,
                    resolved.display()
                ))
                .with_metadata(serde_json::Value::Object(metadata)))
            }
            Err(e) => Ok(ToolOutput::error(format!(
                "Failed to write file {}: {}",
                resolved.display(),
                e
            ))),
        }
    }
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
}
