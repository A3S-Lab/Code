//! Glob tool - Find files by pattern

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;

/// File glob pattern matching tool
pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Returns a list of file paths."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match (e.g., '**/*.rs', 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "Base directory for the search (default: workspace root)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let pattern = args["pattern"]
            .as_str()
            .context("Missing 'pattern' parameter")?;

        let path = args["path"].as_str();
        let base_path = if let Some(p) = path {
            ctx.resolve_path(p)?
        } else {
            ctx.workspace.clone()
        };

        // Use glob crate for pattern matching
        let full_pattern = base_path.join(pattern);
        let pattern_str = full_pattern.to_string_lossy();

        let mut files = Vec::new();
        for entry in glob::glob(&pattern_str).context("Invalid glob pattern")? {
            match entry {
                Ok(path) => {
                    // Get relative path from workspace
                    let relative = path
                        .strip_prefix(&ctx.workspace)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    files.push(relative);
                }
                Err(e) => {
                    tracing::warn!("Glob error: {}", e);
                }
            }
        }

        // Sort alphabetically
        files.sort();

        let output = if files.is_empty() {
            format!("No files matching pattern: {}", pattern)
        } else {
            files.join("\n")
        };

        Ok(ToolOutput::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_glob_pattern() {
        let tool = GlobTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create test files
        std::fs::write(temp_dir.path().join("test1.txt"), "").unwrap();
        std::fs::write(temp_dir.path().join("test2.txt"), "").unwrap();
        std::fs::write(temp_dir.path().join("other.rs"), "").unwrap();

        let result = tool
            .execute(
                &serde_json::json!({
                    "pattern": "*.txt"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("test1.txt"));
        assert!(result.content.contains("test2.txt"));
        assert!(!result.content.contains("other.rs"));
    }

    #[tokio::test]
    async fn test_glob_no_match() {
        let tool = GlobTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "pattern": "*.nonexistent"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("No files matching"));
    }

    #[tokio::test]
    async fn test_glob_recursive() {
        let tool = GlobTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create nested structure
        std::fs::create_dir_all(temp_dir.path().join("subdir")).unwrap();
        std::fs::write(temp_dir.path().join("root.txt"), "").unwrap();
        std::fs::write(temp_dir.path().join("subdir/nested.txt"), "").unwrap();

        let result = tool
            .execute(
                &serde_json::json!({
                    "pattern": "**/*.txt"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("root.txt"));
        assert!(result.content.contains("nested.txt"));
    }

    #[test]
    fn test_glob_parameters() {
        let tool = GlobTool;
        let params = tool.parameters();

        assert!(params["properties"]["pattern"].is_object());
        assert!(params["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("pattern")));
    }
}
