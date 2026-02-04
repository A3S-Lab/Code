//! Write tool - Write content to files

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;

/// File writing tool
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
        let file_path = args["file_path"]
            .as_str()
            .context("Missing 'file_path' parameter")?;

        let content = args["content"]
            .as_str()
            .context("Missing 'content' parameter")?;

        tracing::info!(
            "Writing file: {} (workspace: {})",
            file_path,
            ctx.workspace.display()
        );

        let resolved_path = ctx.resolve_path_for_write(file_path)?;

        tracing::info!("Resolved absolute path: {}", resolved_path.display());

        // Create parent directories if needed
        if let Some(parent) = resolved_path.parent() {
            if !parent.exists() {
                tracing::info!("Creating parent directories: {}", parent.display());
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
            }
        }

        // Write file
        tokio::fs::write(&resolved_path, content)
            .await
            .with_context(|| format!("Failed to write file: {}", file_path))?;

        tracing::info!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            resolved_path.display()
        );

        Ok(ToolOutput::success(format!(
            "Successfully wrote {} bytes to {}\nAbsolute path: {}",
            content.len(),
            file_path,
            resolved_path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_write_file() {
        let tool = WriteTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "content": "Hello, World!"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("13 bytes"));

        // Verify file was written
        let written_content = std::fs::read_to_string(temp_dir.path().join("test.txt")).unwrap();
        assert_eq!(written_content, "Hello, World!");
    }

    #[tokio::test]
    async fn test_write_creates_directories() {
        let tool = WriteTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "subdir/nested/test.txt",
                    "content": "Nested content"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);

        // Verify file was written in nested directory
        let written_content =
            std::fs::read_to_string(temp_dir.path().join("subdir/nested/test.txt")).unwrap();
        assert_eq!(written_content, "Nested content");
    }

    #[test]
    fn test_write_parameters() {
        let tool = WriteTool;
        let params = tool.parameters();

        assert!(params["properties"]["file_path"].is_object());
        assert!(params["properties"]["content"].is_object());

        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("file_path")));
        assert!(required.contains(&serde_json::json!("content")));
    }
}
