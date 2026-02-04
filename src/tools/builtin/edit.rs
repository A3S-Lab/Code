//! Edit tool - Edit files with string replacement

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;

/// File editing tool with string replacement
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
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact string to replace (must be unique unless replace_all=true)"
                },
                "new_string": {
                    "type": "string",
                    "description": "The string to replace it with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file_path = args["file_path"]
            .as_str()
            .context("Missing 'file_path' parameter")?;

        let old_string = args["old_string"]
            .as_str()
            .context("Missing 'old_string' parameter")?;

        let new_string = args["new_string"]
            .as_str()
            .context("Missing 'new_string' parameter")?;

        let replace_all = args["replace_all"].as_bool().unwrap_or(false);

        tracing::info!(
            "Editing file: {} (workspace: {})",
            file_path,
            ctx.workspace.display()
        );

        let resolved_path = ctx.resolve_path(file_path)?;

        tracing::debug!("Resolved absolute path: {}", resolved_path.display());

        // Read current content
        let content = tokio::fs::read_to_string(&resolved_path)
            .await
            .with_context(|| format!("Failed to read file: {}", file_path))?;

        // Check if old_string exists
        let count = content.matches(old_string).count();
        if count == 0 {
            return Ok(ToolOutput::error(format!(
                "String not found in {}: {:?}",
                file_path, old_string
            )));
        }

        // Check for ambiguity
        if count > 1 && !replace_all {
            return Ok(ToolOutput::error(format!(
                "Found {} occurrences of the string in {}. Use replace_all=true to replace all, or provide more context to make it unique.",
                count, file_path
            )));
        }

        // Perform replacement
        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        // Write back
        tokio::fs::write(&resolved_path, &new_content)
            .await
            .with_context(|| format!("Failed to write file: {}", file_path))?;

        // Generate diff for output
        let diff = generate_diff(&content, &new_content, file_path);

        Ok(ToolOutput::success(diff))
    }
}

/// Generate unified diff between old and new content
fn generate_diff(old: &str, new: &str, file_path: &str) -> String {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();

    output.push_str(&format!("--- a/{}\n", file_path));
    output.push_str(&format!("+++ b/{}\n", file_path));

    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        output.push_str(sign);
        output.push_str(change.value());
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_edit_file() {
        let tool = EditTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "Hello, World!").unwrap();

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "World",
                    "new_string": "Rust"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);

        // Verify file was edited
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "Hello, Rust!");
    }

    #[tokio::test]
    async fn test_edit_ambiguous() {
        let tool = EditTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create a test file with duplicate strings
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "foo bar foo").unwrap();

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "foo",
                    "new_string": "baz"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("2 occurrences"));
    }

    #[tokio::test]
    async fn test_edit_replace_all() {
        let tool = EditTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        // Create a test file with duplicate strings
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "foo bar foo").unwrap();

        let result = tool
            .execute(
                &serde_json::json!({
                    "file_path": "test.txt",
                    "old_string": "foo",
                    "new_string": "baz",
                    "replace_all": true
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);

        // Verify all occurrences were replaced
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "baz bar baz");
    }

    #[test]
    fn test_edit_parameters() {
        let tool = EditTool;
        let params = tool.parameters();

        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("file_path")));
        assert!(required.contains(&serde_json::json!("old_string")));
        assert!(required.contains(&serde_json::json!("new_string")));
    }

    #[test]
    fn test_generate_diff() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nmodified\nline3\n";
        let diff = generate_diff(old, new, "test.txt");

        assert!(diff.contains("--- a/test.txt"));
        assert!(diff.contains("+++ b/test.txt"));
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+modified"));
    }
}
