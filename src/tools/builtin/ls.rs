//! Ls tool - List directory contents

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;

/// Directory listing tool
pub struct LsTool;

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        "List contents of a directory with file types and sizes."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to list (default: workspace root)"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let path = args["path"].as_str().unwrap_or(".");
        let resolved_path = ctx.resolve_path(path)?;

        if !resolved_path.is_dir() {
            return Ok(ToolOutput::error(format!("{} is not a directory", path)));
        }

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&resolved_path)
            .await
            .with_context(|| format!("Failed to read directory: {}", path))?;

        while let Some(entry) = dir.next_entry().await? {
            let metadata = entry.metadata().await?;
            let name = entry.file_name().to_string_lossy().to_string();

            let type_char = if metadata.is_dir() {
                "d"
            } else if metadata.is_symlink() {
                "l"
            } else {
                "-"
            };

            let size = metadata.len();
            entries.push(format!("{} {:>10} {}", type_char, size, name));
        }

        entries.sort();
        let output = entries.join("\n");

        Ok(ToolOutput::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ls_directory() {
        let tool = LsTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create test files and directories
        std::fs::write(temp_dir.path().join("file.txt"), "content").unwrap();
        std::fs::create_dir(temp_dir.path().join("subdir")).unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": "."}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("file.txt"));
        assert!(result.content.contains("subdir"));
        assert!(result.content.contains("d")); // directory marker
        assert!(result.content.contains("-")); // file marker
    }

    #[tokio::test]
    async fn test_ls_not_directory() {
        let tool = LsTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create a file
        std::fs::write(temp_dir.path().join("file.txt"), "content").unwrap();

        let result = tool
            .execute(&serde_json::json!({"path": "file.txt"}), &ctx)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("not a directory"));
    }

    #[tokio::test]
    async fn test_ls_default_path() {
        let tool = LsTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create a test file
        std::fs::write(temp_dir.path().join("test.txt"), "").unwrap();

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("test.txt"));
    }

    #[test]
    fn test_ls_parameters() {
        let tool = LsTool;
        let params = tool.parameters();

        assert!(params["properties"]["path"].is_object());
        // path is optional, so required should be empty
        assert!(params["required"].as_array().unwrap().is_empty());
    }
}
