//! Glob tool - Find files matching a glob pattern

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;

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
                    "description": "Base directory for the search (default workspace root)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("pattern parameter is required")),
        };

        let base_dir = match args.get("path").and_then(|v| v.as_str()) {
            Some(p) => {
                if std::path::Path::new(p).is_absolute() {
                    PathBuf::from(p)
                } else {
                    ctx.workspace.join(p)
                }
            }
            None => ctx.workspace.clone(),
        };

        // Build the full glob pattern
        let full_pattern = base_dir.join(pattern);
        // Normalize to forward slashes — glob crate requires '/' on all platforms
        let full_pattern_str = full_pattern.to_string_lossy().replace('\\', "/");

        let entries = match glob::glob(&full_pattern_str) {
            Ok(paths) => paths,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Invalid glob pattern '{}': {}",
                    pattern, e
                )))
            }
        };

        let mut matches: Vec<String> = Vec::new();
        for entry in entries {
            match entry {
                Ok(path) => {
                    // Show path relative to workspace if possible, always use forward slashes
                    let display = path
                        .strip_prefix(&ctx.workspace)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    matches.push(display);
                }
                Err(e) => {
                    tracing::warn!("Glob entry error: {}", e);
                }
            }
        }

        matches.sort();

        if matches.is_empty() {
            Ok(ToolOutput::success(format!(
                "No files found matching pattern: {}",
                pattern
            )))
        } else {
            let count = matches.len();
            let mut output = matches.join("\n");
            output.push_str(&format!("\n\n{} file(s) found", count));
            Ok(ToolOutput::success(output))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_glob_find_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a.txt"), "").unwrap();
        std::fs::write(temp.path().join("b.txt"), "").unwrap();
        std::fs::write(temp.path().join("c.rs"), "").unwrap();

        let tool = GlobTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(&serde_json::json!({"pattern": "*.txt"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("a.txt"));
        assert!(result.content.contains("b.txt"));
        assert!(!result.content.contains("c.rs"));
        assert!(result.content.contains("2 file(s)"));
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let temp = tempfile::tempdir().unwrap();
        let tool = GlobTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(&serde_json::json!({"pattern": "*.xyz"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("No files found"));
    }

    #[tokio::test]
    async fn test_glob_recursive() {
        let temp = tempfile::tempdir().unwrap();
        let sub = temp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(temp.path().join("root.rs"), "").unwrap();
        std::fs::write(sub.join("nested.rs"), "").unwrap();

        let tool = GlobTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(&serde_json::json!({"pattern": "**/*.rs"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("root.rs"));
        assert!(result.content.contains("nested.rs"));
    }

    #[tokio::test]
    async fn test_glob_missing_pattern() {
        let tool = GlobTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
    }
}
