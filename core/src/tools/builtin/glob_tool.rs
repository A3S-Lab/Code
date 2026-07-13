//! Glob tool - Find files matching a glob pattern

use crate::tools::pagination::{PageRequest, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::workspace::WorkspaceGlobRequest;
use anyhow::Result;
use async_trait::async_trait;

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
            "additionalProperties": false,
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Required. Glob pattern to match, for example '**/*.rs' or 'src/**/*.ts'. Always provide this exact field name: 'pattern'."
                },
                "path": {
                    "type": "string",
                    "description": "Optional. Base directory for the search. Default: workspace root."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_PAGE_LIMIT,
                    "description": "Optional. Maximum paths to return. Default: 200; maximum: 1000."
                },
                "cursor": {
                    "type": "string",
                    "description": "Optional. Omit on the first call. On a continuation, copy the exact opaque cursor returned by the previous glob call; never invent a placeholder."
                }
            },
            "required": ["pattern"],
            "examples": [
                {
                    "pattern": "**/*.rs"
                },
                {
                    "pattern": "*.md",
                    "path": "docs"
                }
            ]
        })
    }

    fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        crate::tools::ToolCapabilities::read_only_paginated(16)
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("pattern parameter is required")),
        };
        let page_request = match PageRequest::parse(args, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT) {
            Ok(request) => request,
            Err(error) => return Ok(ToolOutput::error(error)),
        };

        let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let base = match ctx.resolve_workspace_path(path_str) {
            Ok(path) => path,
            Err(e) => return Ok(ToolOutput::error(format!("Failed to resolve path: {}", e))),
        };

        let Some(search) = ctx.workspace_services.search() else {
            return Ok(ToolOutput::error(
                "glob is not available: this workspace backend did not provide search",
            ));
        };

        let request = WorkspaceGlobRequest {
            base,
            pattern: pattern.to_string(),
        };
        let result = match ctx
            .workspace_services
            .run_with_timeout("glob", async move { search.glob(request).await })
            .await
        {
            Ok(result) => result,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Invalid glob pattern '{}': {}",
                    pattern, e
                )))
            }
        };
        let matches: Vec<String> = result
            .matches
            .into_iter()
            .map(|path| path.as_str().to_string())
            .collect();
        let page = match page_request.page(matches) {
            Ok(page) => page,
            Err(error) => return Ok(ToolOutput::error(error)),
        };

        if page.items.is_empty() && page.total_items == 0 {
            Ok(
                ToolOutput::success(format!("No files found matching pattern: {}", pattern))
                    .with_metadata(serde_json::json!({ "page": page.metadata() })),
            )
        } else {
            let mut output = page.items.join("\n");
            output.push_str(&format!(
                "\n\n{} of {} file(s) shown",
                page.items.len(),
                page.total_items
            ));
            if let Some(cursor) = page.next_cursor.as_deref() {
                output.push_str(&format!(
                    "\nMore files available; continue with cursor={cursor}"
                ));
            }
            Ok(ToolOutput::success(output)
                .with_metadata(serde_json::json!({ "page": page.metadata() })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
    async fn test_glob_paginates_with_resume_cursor() {
        let temp = tempfile::tempdir().unwrap();
        for name in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(temp.path().join(name), "").unwrap();
        }
        let tool = GlobTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let first = tool
            .execute(&serde_json::json!({"pattern": "*.rs", "limit": 2}), &ctx)
            .await
            .unwrap();
        assert!(first.success);
        assert!(first.content.contains("a.rs"));
        assert!(first.content.contains("b.rs"));
        assert!(!first.content.contains("c.rs"));
        assert_eq!(first.metadata.as_ref().unwrap()["page"]["next_cursor"], "2");

        let second = tool
            .execute(
                &serde_json::json!({"pattern": "*.rs", "limit": 2, "cursor": "2"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(second.success);
        assert!(second.content.contains("c.rs"));
        assert_eq!(second.metadata.unwrap()["page"]["truncated"], false);
    }

    #[tokio::test]
    async fn test_glob_missing_pattern() {
        let tool = GlobTool;
        let ctx = ToolContext::new(PathBuf::from("/tmp"));

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_glob_schema_is_canonical() {
        let tool = GlobTool;
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["required"], serde_json::json!(["pattern"]));
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["pattern"], "**/*.rs");
        assert!(examples[0].get("glob").is_none());
    }
}
