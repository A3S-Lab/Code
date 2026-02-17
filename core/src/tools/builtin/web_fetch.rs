//! Web fetch tool - Fetch content from URLs

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;

/// Maximum response size (5MB)
const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL and convert to text or markdown. Supports HTML to Markdown conversion. 5MB response size limit. Configurable timeout (max 120 seconds)."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch content from (must start with http:// or https://)"
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "text", "html"],
                    "description": "Output format (default: markdown)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 30, max: 120)"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let url = match args.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return Ok(ToolOutput::error("url parameter is required")),
        };

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(ToolOutput::error("URL must start with http:// or https://"));
        }

        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("markdown");

        let timeout_secs = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(30)
            .min(120);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .user_agent("a3s-code/0.7")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let response = match client.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Failed to fetch URL {}: {}",
                    url, e
                )))
            }
        };

        let status = response.status();
        if !status.is_success() {
            return Ok(ToolOutput::error(format!(
                "HTTP {} for URL: {}",
                status, url
            )));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Failed to read response body: {}",
                    e
                )))
            }
        };

        if bytes.len() > MAX_RESPONSE_SIZE {
            return Ok(ToolOutput::error(format!(
                "Response too large: {} bytes (max: {} bytes)",
                bytes.len(),
                MAX_RESPONSE_SIZE
            )));
        }

        let body = String::from_utf8_lossy(&bytes).to_string();

        let output = match format {
            "html" => body,
            "text" => {
                if content_type.contains("text/html") {
                    html_to_text(&body)
                } else {
                    body
                }
            }
            _ => {
                // markdown (default)
                if content_type.contains("text/html") {
                    html_to_markdown(&body)
                } else {
                    body
                }
            }
        };

        Ok(ToolOutput::success(output))
    }
}

/// Simple HTML to plain text conversion
fn html_to_text(html: &str) -> String {
    // Strip tags
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if !in_tag && i + 7 < len && &lower[i..i + 7] == "<script" {
            in_script = true;
            in_tag = true;
        } else if in_script && i + 9 <= len && &lower[i..i + 9] == "</script>" {
            in_script = false;
            i += 9;
            continue;
        } else if !in_tag && i + 6 < len && &lower[i..i + 6] == "<style" {
            in_style = true;
            in_tag = true;
        } else if in_style && i + 8 <= len && &lower[i..i + 8] == "</style>" {
            in_style = false;
            i += 8;
            continue;
        }

        if chars[i] == '<' {
            in_tag = true;

            // Add newlines for block elements
            if i + 3 < len {
                let tag_start = &lower[i..];
                if tag_start.starts_with("<br")
                    || tag_start.starts_with("<p")
                    || tag_start.starts_with("</p")
                    || tag_start.starts_with("<div")
                    || tag_start.starts_with("</div")
                    || tag_start.starts_with("<h")
                    || tag_start.starts_with("</h")
                    || tag_start.starts_with("<li")
                    || tag_start.starts_with("<tr")
                {
                    result.push('\n');
                }
            }
        } else if chars[i] == '>' {
            in_tag = false;
        } else if !in_tag && !in_script && !in_style {
            result.push(chars[i]);
        }

        i += 1;
    }

    // Decode basic HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Simple HTML to markdown conversion
fn html_to_markdown(html: &str) -> String {
    // First get plain text
    let text = html_to_text(html);

    // Clean up excessive whitespace
    let lines: Vec<&str> = text.lines().collect();
    let mut result = String::new();
    let mut prev_blank = false;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank {
                result.push('\n');
                prev_blank = true;
            }
        } else {
            result.push_str(trimmed);
            result.push('\n');
            prev_blank = false;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_text_basic() {
        let html = "<p>Hello <b>world</b></p>";
        let text = html_to_text(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("world"));
        assert!(!text.contains("<p>"));
        assert!(!text.contains("<b>"));
    }

    #[test]
    fn test_html_to_text_entities() {
        let html = "foo &amp; bar &lt; baz &gt; qux";
        let text = html_to_text(html);
        assert_eq!(text, "foo & bar < baz > qux");
    }

    #[test]
    fn test_html_to_text_strips_script() {
        let html = "<p>before</p><script>alert('xss')</script><p>after</p>";
        let text = html_to_text(html);
        assert!(text.contains("before"));
        assert!(text.contains("after"));
        assert!(!text.contains("alert"));
    }

    #[tokio::test]
    async fn test_web_fetch_invalid_url() {
        let tool = WebFetchTool;
        let ctx = ToolContext::new(std::path::PathBuf::from("/tmp"));

        let result = tool
            .execute(&serde_json::json!({"url": "not-a-url"}), &ctx)
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("must start with"));
    }

    #[tokio::test]
    async fn test_web_fetch_missing_url() {
        let tool = WebFetchTool;
        let ctx = ToolContext::new(std::path::PathBuf::from("/tmp"));

        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
    }

    #[test]
    fn test_html_to_markdown() {
        let html = "<h1>Title</h1><p>Content here</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("Title"));
        assert!(md.contains("Content here"));
    }
}
