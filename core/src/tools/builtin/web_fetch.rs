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
            "additionalProperties": false,
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Required. The URL to fetch content from. Must start with http:// or https://. Always provide this exact field name: 'url'."
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "text", "html"],
                    "description": "Optional. Output format. Default: markdown."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional. Timeout in seconds. Default: 30. Maximum: 120."
                }
            },
            "required": ["url"],
            "examples": [
                {
                    "url": "https://example.com"
                },
                {
                    "url": "https://example.com",
                    "format": "text",
                    "timeout": 15
                }
            ]
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

/// Convert HTML to plain text using html2text (handles encoding, tags, scripts, etc.)
fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 120)
        .unwrap_or_else(|_| String::from("[failed to parse HTML]"))
}

/// Convert HTML to markdown using htmd
fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html_to_text(html))
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
        assert!(text.contains("foo & bar < baz > qux"));
    }

    #[test]
    fn test_html_to_text_strips_script() {
        let html = "<p>before</p><script>alert('xss')</script><p>after</p>";
        let text = html_to_text(html);
        assert!(text.contains("before"));
        assert!(text.contains("after"));
        assert!(!text.contains("alert"));
    }

    #[test]
    fn test_html_to_text_multibyte_utf8() {
        let html = "<p>Diseño español — café résumé naïve</p>";
        let text = html_to_text(html);
        assert!(text.contains("Diseño"));
        assert!(text.contains("café"));
        assert!(text.contains("résumé"));
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
    fn test_web_fetch_schema_is_canonical() {
        let tool = WebFetchTool;
        let params = tool.parameters();
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["required"], serde_json::json!(["url"]));
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["url"], "https://example.com");
        assert!(examples[0].get("link").is_none());
    }

    #[test]
    fn test_html_to_markdown() {
        let html = "<h1>Title</h1><p>Content here</p>";
        let md = html_to_markdown(html);
        assert!(md.contains("Title"));
        assert!(md.contains("Content here"));
        // htmd produces proper markdown headers
        assert!(md.contains("# Title"));
    }
}
