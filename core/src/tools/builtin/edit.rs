//! Edit tool - Edit files by string replacement

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing a specific string with another. The old_string must be unique in the file unless replace_all is true."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Required. Path to the file to edit. Always provide this exact field name: 'file_path'."
                },
                "old_string": {
                    "type": "string",
                    "description": "Required. The exact string to replace. It must be unique unless replace_all=true."
                },
                "new_string": {
                    "type": "string",
                    "description": "Required. The replacement string."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Optional. Replace all occurrences. Default: false."
                }
            },
            "required": ["file_path", "old_string", "new_string"],
            "examples": [
                {
                    "file_path": "src/lib.rs",
                    "old_string": "old_value",
                    "new_string": "new_value"
                },
                {
                    "file_path": "src/lib.rs",
                    "old_string": "foo",
                    "new_string": "bar",
                    "replace_all": true
                }
            ]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file_path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(ToolOutput::error("file_path parameter is required")),
        };

        let old_string = match args.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolOutput::error("old_string parameter is required")),
        };

        let new_string = match args.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return Ok(ToolOutput::error("new_string parameter is required")),
        };

        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

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

        let count = content.matches(old_string).count();

        if count == 0 {
            return Ok(ToolOutput::error(format!(
                "old_string not found in {}",
                resolved.display()
            )));
        }

        if count > 1 && !replace_all {
            return Ok(ToolOutput::error(format!(
                "old_string found {} times in {}. Use replace_all=true to replace all occurrences, or provide a more specific string.",
                count,
                resolved.display()
            )));
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        match tokio::fs::write(&resolved, &new_content).await {
            Ok(()) => Ok(ToolOutput::success(format!(
                "Replaced {} occurrence(s) in {}",
                if replace_all { count } else { 1 },
                resolved.display()
            ))),
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
    async fn test_edit_single_replace() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "hello world").unwrap();

        let tool = EditTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "hello",
                    "new_string": "goodbye"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        let content = std::fs::read_to_string(temp.path().join("test.txt")).unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[tokio::test]
    async fn test_edit_replace_all() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "aaa bbb aaa").unwrap();

        let tool = EditTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "aaa",
                    "new_string": "ccc",
                    "replace_all": true
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        let content = std::fs::read_to_string(temp.path().join("test.txt")).unwrap();
        assert_eq!(content, "ccc bbb ccc");
    }

    #[tokio::test]
    async fn test_edit_not_unique() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "aaa bbb aaa").unwrap();

        let tool = EditTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "aaa",
                    "new_string": "ccc"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("2 times"));
    }

    #[tokio::test]
    async fn test_edit_not_found() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("test.txt"), "hello world").unwrap();

        let tool = EditTool;
        let ctx = ToolContext::new(temp.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "xyz",
                    "new_string": "abc"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("not found"));
    }

    #[test]
    fn test_edit_schema_is_canonical() {
        let tool = EditTool;
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(
            params["required"],
            serde_json::json!(["file_path", "old_string", "new_string"])
        );
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["file_path"], "src/lib.rs");
        assert!(examples[0].get("path").is_none());
    }
}
