//! Read tool - Read file contents

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::{MAX_LINE_LENGTH, MAX_READ_LINES};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;

/// File reading tool
pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Returns line-numbered output. Supports text files and images."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to read (absolute or relative to workspace)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-indexed, default: 0)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read (default: 2000)"
                }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let file_path = args["file_path"]
            .as_str()
            .context("Missing 'file_path' parameter")?;

        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = args["limit"].as_u64().unwrap_or(MAX_READ_LINES as u64) as usize;

        tracing::info!(
            "Reading file: {} (workspace: {})",
            file_path,
            ctx.workspace.display()
        );

        let resolved_path = ctx.resolve_path(file_path)?;

        tracing::debug!("Resolved absolute path: {}", resolved_path.display());

        // Check if file exists
        if !resolved_path.exists() {
            return Ok(ToolOutput::error(format!("File not found: {}", file_path)));
        }

        // Check if it's a directory
        if resolved_path.is_dir() {
            return Ok(ToolOutput::error(format!(
                "{} is a directory, not a file",
                file_path
            )));
        }

        // Check if it's an image
        if is_image_file(&resolved_path) {
            return read_image(&resolved_path).await;
        }

        // Read text file
        let content = tokio::fs::read_to_string(&resolved_path)
            .await
            .with_context(|| format!("Failed to read file: {}", file_path))?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Apply offset and limit
        let start = offset.min(total_lines);
        let end = (start + limit).min(total_lines);
        let selected_lines = &lines[start..end];

        // Format with line numbers (1-indexed like cat -n)
        let mut output = String::new();
        for (i, line) in selected_lines.iter().enumerate() {
            let line_num = start + i + 1;
            let truncated_line = if line.len() > MAX_LINE_LENGTH {
                format!("{}...", &line[..MAX_LINE_LENGTH])
            } else {
                line.to_string()
            };
            output.push_str(&format!("{:>6}\t{}\n", line_num, truncated_line));
        }

        // Add metadata if truncated
        if end < total_lines {
            output.push_str(&format!(
                "\n[Showing lines {}-{} of {}. Use offset/limit for more.]",
                start + 1,
                end,
                total_lines
            ));
        }

        Ok(ToolOutput::success(output))
    }
}

/// Check if file is an image based on extension
fn is_image_file(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(
            ext.to_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico"
        ),
        None => false,
    }
}

/// Read image file and return base64
async fn read_image(path: &Path) -> Result<ToolOutput> {
    let content = tokio::fs::read(path)
        .await
        .with_context(|| format!("Failed to read image: {}", path.display()))?;

    let base64_content =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &content);

    let mime_type = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    };

    Ok(ToolOutput::success(format!(
        "data:{};base64,{}",
        mime_type, base64_content
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_image_file() {
        assert!(is_image_file(Path::new("test.png")));
        assert!(is_image_file(Path::new("test.jpg")));
        assert!(is_image_file(Path::new("test.JPEG")));
        assert!(!is_image_file(Path::new("test.txt")));
        assert!(!is_image_file(Path::new("test.rs")));
    }

    #[tokio::test]
    async fn test_read_nonexistent() {
        let tool = ReadTool;
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp_dir.path().to_path_buf());

        let result = tool
            .execute(
                &serde_json::json!({"file_path": "nonexistent_file_12345.txt"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("not found"));
    }

    #[test]
    fn test_read_parameters() {
        let tool = ReadTool;
        let params = tool.parameters();

        assert!(params["properties"]["file_path"].is_object());
        assert!(params["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("file_path")));
    }
}
