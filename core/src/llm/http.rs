//! HTTP utilities and abstraction for LLM API calls

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use std::pin::Pin;
use std::sync::Arc;

/// HTTP response from a non-streaming POST request
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// HTTP response from a streaming POST request
pub struct StreamingHttpResponse {
    pub status: u16,
    /// Retry-After header value (if present)
    pub retry_after: Option<String>,
    /// Byte stream (valid when status is 2xx)
    pub byte_stream: Pin<Box<dyn futures::Stream<Item = Result<bytes::Bytes>> + Send>>,
    /// Error body (populated when status is not 2xx)
    pub error_body: String,
}

/// Abstraction over HTTP POST requests for LLM API calls.
///
/// Enables dependency injection for testing without hitting real HTTP endpoints.
#[async_trait]
pub trait HttpClient: Send + Sync {
    /// Make a POST request and return status + body
    async fn post(
        &self,
        url: &str,
        headers: Vec<(&str, &str)>,
        body: &serde_json::Value,
    ) -> Result<HttpResponse>;

    /// Make a POST request and return a streaming response
    async fn post_streaming(
        &self,
        url: &str,
        headers: Vec<(&str, &str)>,
        body: &serde_json::Value,
    ) -> Result<StreamingHttpResponse>;
}

/// Default HTTP client backed by reqwest
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for ReqwestHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn post(
        &self,
        url: &str,
        headers: Vec<(&str, &str)>,
        body: &serde_json::Value,
    ) -> Result<HttpResponse> {
        tracing::debug!(
            "HTTP POST to {}: {}",
            url,
            serde_json::to_string_pretty(body)?
        );

        let mut request = self.client.post(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        request = request.json(body);

        let response = request
            .send()
            .await
            .context(format!("Failed to send request to {}", url))?;

        let status = response.status().as_u16();
        let body = response.text().await?;

        Ok(HttpResponse { status, body })
    }

    async fn post_streaming(
        &self,
        url: &str,
        headers: Vec<(&str, &str)>,
        body: &serde_json::Value,
    ) -> Result<StreamingHttpResponse> {
        tracing::debug!(
            "HTTP POST streaming to {}: {}",
            url,
            serde_json::to_string_pretty(body)?
        );

        let mut request = self.client.post(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        request = request.json(body);

        let response = request
            .send()
            .await
            .context(format!("Failed to send streaming request to {}", url))?;

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        if (200..300).contains(&status) {
            let byte_stream = response
                .bytes_stream()
                .map(|r| r.map_err(|e| anyhow::anyhow!("Stream error: {}", e)));
            Ok(StreamingHttpResponse {
                status,
                retry_after,
                byte_stream: Box::pin(byte_stream),
                error_body: String::new(),
            })
        } else {
            let error_body = response.text().await.unwrap_or_default();
            // Return an empty stream for error responses
            let empty: futures::stream::Empty<Result<bytes::Bytes>> = futures::stream::empty();
            Ok(StreamingHttpResponse {
                status,
                retry_after,
                byte_stream: Box::pin(empty),
                error_body,
            })
        }
    }
}

/// Create a default HTTP client
pub fn default_http_client() -> Arc<dyn HttpClient> {
    Arc::new(ReqwestHttpClient::new())
}

/// Normalize base URL by stripping trailing /v1
pub(crate) fn normalize_base_url(base_url: &str) -> String {
    base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_base_url() {
        assert_eq!(
            normalize_base_url("https://api.example.com"),
            "https://api.example.com"
        );
        assert_eq!(
            normalize_base_url("https://api.example.com/"),
            "https://api.example.com"
        );
        assert_eq!(
            normalize_base_url("https://api.example.com/v1"),
            "https://api.example.com"
        );
        assert_eq!(
            normalize_base_url("https://api.example.com/v1/"),
            "https://api.example.com"
        );
    }

    #[test]
    fn test_normalize_base_url_edge_cases() {
        assert_eq!(
            normalize_base_url("http://localhost:8080/v1"),
            "http://localhost:8080"
        );
        assert_eq!(
            normalize_base_url("http://localhost:8080"),
            "http://localhost:8080"
        );
        assert_eq!(
            normalize_base_url("https://api.example.com/v1/"),
            "https://api.example.com"
        );
    }

    #[test]
    fn test_normalize_base_url_multiple_trailing_slashes() {
        assert_eq!(
            normalize_base_url("https://api.example.com//"),
            "https://api.example.com"
        );
    }

    #[test]
    fn test_normalize_base_url_with_port() {
        assert_eq!(
            normalize_base_url("http://localhost:11434/v1/"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn test_normalize_base_url_already_normalized() {
        assert_eq!(
            normalize_base_url("https://api.openai.com"),
            "https://api.openai.com"
        );
    }

    #[test]
    fn test_normalize_base_url_empty_string() {
        assert_eq!(normalize_base_url(""), "");
    }

    #[test]
    fn test_default_http_client_creation() {
        let _client = default_http_client();
    }
}
