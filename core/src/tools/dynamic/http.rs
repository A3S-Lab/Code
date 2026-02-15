//! HTTP tool - Make HTTP API calls

use super::substitute_template_args;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Cached regex for `${env:VAR_NAME}` substitution
fn env_var_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\$\{env:([^}]+)\}").unwrap())
}

/// Tool that makes HTTP API calls
pub struct HttpTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    /// API endpoint URL
    url: String,
    /// HTTP method
    method: String,
    /// Request headers
    headers: HashMap<String, String>,
    /// Request body template (JSON with ${arg_name} substitution)
    body_template: Option<String>,
    /// Reusable HTTP client (timeout configured at construction)
    client: reqwest::Client,
}

impl HttpTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        description: String,
        parameters: serde_json::Value,
        url: String,
        method: String,
        headers: HashMap<String, String>,
        body_template: Option<String>,
        timeout_ms: u64,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            name,
            description,
            parameters,
            url,
            method,
            headers,
            body_template,
            client,
        }
    }

    /// Substitute ${env:VAR_NAME} placeholders, then delegate ${arg_name} to shared helper
    fn substitute(&self, template: &str, args: &serde_json::Value) -> String {
        // Substitute environment variables first (${env:VAR_NAME})
        let after_env = env_var_regex()
            .replace_all(template, |caps: &regex::Captures| {
                let var_name = &caps[1];
                std::env::var(var_name).unwrap_or_default()
            })
            .to_string();

        // Delegate arg substitution to shared helper
        substitute_template_args(&after_env, args)
    }

    /// Build the request body
    fn build_body(&self, args: &serde_json::Value) -> Option<String> {
        if let Some(template) = &self.body_template {
            Some(self.substitute(template, args))
        } else {
            // Default: send args as JSON body for POST/PUT/PATCH
            match self.method.to_uppercase().as_str() {
                "POST" | "PUT" | "PATCH" => Some(args.to_string()),
                _ => None,
            }
        }
    }

    /// Build the request URL with query parameters for GET requests
    fn build_url(&self, args: &serde_json::Value) -> String {
        let base_url = self.substitute(&self.url, args);

        // For GET requests, add remaining args as query parameters
        if self.method.to_uppercase() == "GET" {
            if let Some(obj) = args.as_object() {
                let mut url = reqwest::Url::parse(&base_url)
                    .unwrap_or_else(|_| reqwest::Url::parse("http://invalid").unwrap());

                for (key, value) in obj {
                    // Skip if already substituted in URL
                    if base_url.contains(&format!("${{{}}}", key)) {
                        continue;
                    }
                    let value_str = match value {
                        serde_json::Value::String(s) => s.clone(),
                        _ => value.to_string(),
                    };
                    url.query_pairs_mut().append_pair(key, &value_str);
                }

                return url.to_string();
            }
        }

        base_url
    }
}

#[async_trait]
impl Tool for HttpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let url = self.build_url(args);
        tracing::debug!("HTTP {} {}", self.method, url);

        let mut request = match self.method.to_uppercase().as_str() {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            "PUT" => self.client.put(&url),
            "PATCH" => self.client.patch(&url),
            "DELETE" => self.client.delete(&url),
            "HEAD" => self.client.head(&url),
            _ => {
                return Ok(ToolOutput::error(format!(
                    "Unsupported HTTP method: {}",
                    self.method
                )));
            }
        };

        // Add headers
        for (key, value) in &self.headers {
            let substituted_value = self.substitute(value, args);
            request = request.header(key, substituted_value);
        }

        // Add body if applicable
        if let Some(body) = self.build_body(args) {
            request = request
                .header("Content-Type", "application/json")
                .body(body);
        }

        // Send request
        let response = request
            .send()
            .await
            .with_context(|| format!("HTTP request failed: {} {}", self.method, url))?;

        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();

        // Build output
        let mut output = String::new();
        output.push_str(&format!(
            "HTTP {} {}\n",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        ));
        output.push_str(&format!("URL: {}\n\n", url));

        // Include relevant headers
        if let Some(content_type) = headers.get("content-type") {
            output.push_str(&format!(
                "Content-Type: {}\n",
                content_type.to_str().unwrap_or("")
            ));
        }
        output.push('\n');

        // Try to pretty-print JSON response
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            output.push_str(&serde_json::to_string_pretty(&json).unwrap_or(body));
        } else {
            output.push_str(&body);
        }

        Ok(ToolOutput {
            content: output,
            success: status.is_success(),
            metadata: Some(serde_json::json!({
                "status_code": status.as_u16(),
                "url": url
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_substitute_args() {
        let tool = HttpTool::new(
            "test".to_string(),
            "test".to_string(),
            serde_json::json!({}),
            "https://api.example.com/${endpoint}".to_string(),
            "GET".to_string(),
            HashMap::new(),
            None,
            30_000,
        );

        let args = serde_json::json!({
            "endpoint": "users",
            "id": 123
        });

        let result = tool.substitute("https://api.example.com/${endpoint}/${id}", &args);
        assert_eq!(result, "https://api.example.com/users/123");
    }

    #[test]
    fn test_substitute_env() {
        std::env::set_var("TEST_API_KEY", "secret123");

        let tool = HttpTool::new(
            "test".to_string(),
            "test".to_string(),
            serde_json::json!({}),
            "https://api.example.com".to_string(),
            "GET".to_string(),
            HashMap::new(),
            None,
            30_000,
        );

        let result = tool.substitute("Bearer ${env:TEST_API_KEY}", &serde_json::json!({}));
        assert_eq!(result, "Bearer secret123");

        std::env::remove_var("TEST_API_KEY");
    }

    #[test]
    fn test_build_url_with_query_params() {
        let tool = HttpTool::new(
            "test".to_string(),
            "test".to_string(),
            serde_json::json!({}),
            "https://api.example.com/search".to_string(),
            "GET".to_string(),
            HashMap::new(),
            None,
            30_000,
        );

        let args = serde_json::json!({
            "q": "hello",
            "limit": 10
        });

        let url = tool.build_url(&args);
        assert!(url.contains("q=hello"));
        assert!(url.contains("limit=10"));
    }

    #[test]
    fn test_build_body() {
        let tool = HttpTool::new(
            "test".to_string(),
            "test".to_string(),
            serde_json::json!({}),
            "https://api.example.com".to_string(),
            "POST".to_string(),
            HashMap::new(),
            Some(r#"{"message": "${text}"}"#.to_string()),
            30_000,
        );

        let args = serde_json::json!({
            "text": "hello world"
        });

        let body = tool.build_body(&args).unwrap();
        assert_eq!(body, r#"{"message": "hello world"}"#);
    }

    #[tokio::test]
    async fn test_http_tool_invalid_url() {
        let tool = HttpTool::new(
            "test".to_string(),
            "test".to_string(),
            serde_json::json!({}),
            "not-a-valid-url".to_string(),
            "GET".to_string(),
            HashMap::new(),
            None,
            1000,
        );

        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool.execute(&serde_json::json!({}), &ctx).await;

        // Should fail with connection error
        assert!(result.is_err() || !result.unwrap().success);
    }
}
