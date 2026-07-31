//! HTTP utilities and abstraction for LLM API calls

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use std::env;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Typed failures emitted before an HTTP response exists.
///
/// Retry and tool policies inspect this enum rather than rendered diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum HttpClientError {
    #[error("{operation} was cancelled")]
    Cancelled { operation: String },
    #[error("{operation} transport failed: {message}")]
    Transport { operation: String, message: String },
    #[error("{operation} request was invalid: {message}")]
    InvalidRequest { operation: String, message: String },
}

impl HttpClientError {
    pub fn cancelled(operation: impl Into<String>) -> Self {
        Self::Cancelled {
            operation: operation.into(),
        }
    }

    pub fn transport(operation: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Transport {
            operation: operation.into(),
            message: message.into(),
        }
    }

    fn from_reqwest(operation: &str, error: reqwest::Error) -> Self {
        if error.is_builder() {
            Self::InvalidRequest {
                operation: operation.to_string(),
                message: error.to_string(),
            }
        } else if error.is_timeout() {
            // reqwest 0.12 no longer guarantees that the rendered transport
            // chain contains the word "timeout" on every platform. Preserve
            // the stable public diagnostic while retaining the retryable
            // Transport classification.
            Self::transport(operation, format!("timed out: {error}"))
        } else {
            Self::transport(operation, error.to_string())
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transport { .. })
    }
}

pub(crate) fn is_retryable_http_failure(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<HttpClientError>()
        .is_some_and(HttpClientError::is_retryable)
}

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

/// Information about an HTTP request for metrics collection.
#[derive(Debug, Clone)]
pub struct HttpMetricsRecord {
    /// The target URL
    pub url: String,
    /// HTTP method (currently only POST is used for LLM calls)
    pub method: String,
    /// Response status code
    pub status: u16,
    /// Request duration in milliseconds
    pub duration_ms: f64,
    /// Number of bytes sent (request body size)
    pub request_bytes: u64,
    /// Number of bytes received (response body size)
    pub response_bytes: u64,
    /// Whether this was a streaming request
    pub streaming: bool,
}

/// Callback function type for HTTP metrics collection.
/// The callback is called after each HTTP request completes.
pub type HttpMetricsCallback = Arc<dyn Fn(HttpMetricsRecord) + Send + Sync>;

/// Global HTTP metrics callback registry.
///
/// Set this to enable HTTP metrics collection for LLM API calls.
/// The callback will be invoked after each HTTP request completes.
static HTTP_METRICS_CALLBACK: std::sync::RwLock<Option<HttpMetricsCallback>> =
    std::sync::RwLock::new(None);

/// Register a global HTTP metrics callback.
/// The callback will be invoked after each HTTP request completes.
pub fn set_http_metrics_callback(callback: HttpMetricsCallback) {
    *HTTP_METRICS_CALLBACK.write().unwrap() = Some(callback);
}

/// Clear the global HTTP metrics callback.
pub fn clear_http_metrics_callback() {
    *HTTP_METRICS_CALLBACK.write().unwrap() = None;
}

fn maybe_record_metrics(record: HttpMetricsRecord) {
    if let Some(callback) = HTTP_METRICS_CALLBACK.read().unwrap().as_ref() {
        callback(record);
    }
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
        cancel_token: CancellationToken,
    ) -> Result<HttpResponse>;

    /// Make a POST request and return a streaming response.
    /// If cancel_token is cancelled during the request, the HTTP connection is aborted.
    async fn post_streaming(
        &self,
        url: &str,
        headers: Vec<(&str, &str)>,
        body: &serde_json::Value,
        cancel_token: CancellationToken,
    ) -> Result<StreamingHttpResponse>;
}

/// Default HTTP client backed by reqwest
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    pub fn new() -> Self {
        Self {
            client: build_reqwest_client(None, None).expect("failed to build default HTTP client"),
        }
    }

    pub fn with_timeout(timeout: Duration) -> Result<Self> {
        Ok(Self {
            client: build_reqwest_client(Some(timeout), None)?,
        })
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
        cancel_token: CancellationToken,
    ) -> Result<HttpResponse> {
        let start = std::time::Instant::now();
        let request_body = serde_json::to_string(body).unwrap_or_default();
        let request_bytes = request_body.len() as u64;

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

        let response = tokio::select! {
            _ = cancel_token.cancelled() => {
                return Err(anyhow::Error::new(HttpClientError::cancelled("HTTP request")));
            }
            result = request.send() => {
                result.map_err(|error| {
                    anyhow::Error::new(HttpClientError::from_reqwest("HTTP request", error))
                })?
            }
        };

        let status = response.status().as_u16();
        let response_body = response.text().await.map_err(|error| {
            anyhow::Error::new(HttpClientError::from_reqwest("HTTP response body", error))
        })?;
        let response_bytes = response_body.len() as u64;
        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

        maybe_record_metrics(HttpMetricsRecord {
            url: url.to_string(),
            method: "POST".to_string(),
            status,
            duration_ms,
            request_bytes,
            response_bytes,
            streaming: false,
        });

        Ok(HttpResponse {
            status,
            body: response_body,
        })
    }

    async fn post_streaming(
        &self,
        url: &str,
        headers: Vec<(&str, &str)>,
        body: &serde_json::Value,
        cancel_token: CancellationToken,
    ) -> Result<StreamingHttpResponse> {
        let start = std::time::Instant::now();
        let request_body = serde_json::to_string(body).unwrap_or_default();
        let request_bytes = request_body.len() as u64;

        let mut request = self.client.post(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        request = request.json(body);

        let response = tokio::select! {
            _ = cancel_token.cancelled() => {
                return Err(anyhow::Error::new(HttpClientError::cancelled(
                    "HTTP streaming request",
                )));
            }
            result = request.send() => {
                result.map_err(|error| {
                    anyhow::Error::new(HttpClientError::from_reqwest(
                        "HTTP streaming request",
                        error,
                    ))
                })?
            }
        };

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        // For streaming, we record metrics after sending but before consuming the stream
        // Note: response_bytes is estimated as we can't know the full stream size upfront
        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
        maybe_record_metrics(HttpMetricsRecord {
            url: url.to_string(),
            method: "POST".to_string(),
            status,
            duration_ms,
            request_bytes,
            response_bytes: 0, // Unknown for streaming
            streaming: true,
        });

        if (200..300).contains(&status) {
            let byte_stream = response.bytes_stream().map(|result| {
                result.map_err(|error| {
                    anyhow::Error::new(HttpClientError::from_reqwest("HTTP response stream", error))
                })
            });
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ExplicitProxyConfig {
    http: Option<String>,
    https: Option<String>,
}

/// Build a reqwest client without consulting system proxy settings.
///
/// On macOS test runners, the system proxy lookup path can panic inside the
/// `system-configuration` crate when no dynamic store is available. Disabling
/// implicit proxy discovery keeps client construction deterministic while still
/// honoring standard proxy environment variables explicitly.
pub(crate) fn build_reqwest_client(
    timeout: Option<Duration>,
    default_headers: Option<reqwest::header::HeaderMap>,
) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().no_proxy();

    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }

    if let Some(default_headers) = default_headers {
        builder = builder.default_headers(default_headers);
    }

    let proxy_config = explicit_proxy_config_from_env();
    if let Some(http_proxy) = proxy_config.http.as_deref() {
        builder = builder.proxy(
            reqwest::Proxy::http(http_proxy)
                .with_context(|| format!("Invalid HTTP proxy URL: {http_proxy}"))?,
        );
    }
    if let Some(https_proxy) = proxy_config.https.as_deref() {
        builder = builder.proxy(
            reqwest::Proxy::https(https_proxy)
                .with_context(|| format!("Invalid HTTPS proxy URL: {https_proxy}"))?,
        );
    }

    builder.build().context("Failed to build reqwest client")
}

fn explicit_proxy_config_from_env() -> ExplicitProxyConfig {
    let http = first_non_empty_env(&["http_proxy", "HTTP_PROXY"]);
    let https = first_non_empty_env(&["https_proxy", "HTTPS_PROXY"]).or_else(|| http.clone());

    ExplicitProxyConfig { http, https }
}

fn first_non_empty_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
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
    use std::sync::{Mutex, OnceLock};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn retryable_http_failure_requires_a_typed_transport_error() {
        let prose = anyhow::anyhow!(
            "Human-readable text says timeout, connection reset, and TLS handshake."
        );
        assert!(!is_retryable_http_failure(&prose));

        let transport = anyhow::Error::new(HttpClientError::transport(
            "stream request",
            "opaque diagnostic",
        ));
        assert!(is_retryable_http_failure(&transport));

        let cancelled = anyhow::Error::new(HttpClientError::cancelled("stream request"));
        assert!(!is_retryable_http_failure(&cancelled));
    }

    fn proxy_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn clear_proxy_env() {
        for key in ["http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY"] {
            unsafe { env::remove_var(key) };
        }
    }

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

    #[tokio::test]
    async fn test_reqwest_http_client_timeout_applies_to_api_call() {
        let mut last_refused = None;
        for _ in 0..3 {
            let (elapsed, err) = post_to_slow_local_server().await;
            assert!(
                elapsed < Duration::from_secs(1),
                "API timeout should fail quickly, elapsed={elapsed:?}"
            );

            let msg = format!("{err:?}").to_ascii_lowercase();
            if msg.contains("connection refused") {
                last_refused = Some(err);
                continue;
            }

            assert!(
                msg.contains("timed out") || msg.contains("timeout"),
                "expected timeout error, got: {err:?}"
            );
            return;
        }

        panic!(
            "local timeout server was not reachable after retries; last error: {:?}",
            last_refused.expect("at least one connection-refused error")
        );
    }

    async fn post_to_slow_local_server() -> (Duration, anyhow::Error) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf).await;
            tokio::time::sleep(Duration::from_millis(250)).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await;
        });

        let client = ReqwestHttpClient::with_timeout(Duration::from_millis(50)).unwrap();
        let started = std::time::Instant::now();
        let err = match client
            .post(
                &format!("http://{addr}/v1/chat/completions"),
                Vec::new(),
                &serde_json::json!({"model": "test"}),
                CancellationToken::new(),
            )
            .await
        {
            Ok(_) => panic!("expected API timeout error"),
            Err(err) => err,
        };

        server.abort();
        (started.elapsed(), err)
    }

    #[test]
    #[cfg(not(windows))]
    fn test_explicit_proxy_config_from_env_prefers_lowercase_vars() {
        let _guard = proxy_env_lock().lock().unwrap();
        clear_proxy_env();
        unsafe {
            env::set_var("http_proxy", "http://lower-http:3128");
            env::set_var("HTTP_PROXY", "http://upper-http:3128");
            env::set_var("https_proxy", "http://lower-https:3128");
            env::set_var("HTTPS_PROXY", "http://upper-https:3128");
        }

        let proxy_config = explicit_proxy_config_from_env();

        assert_eq!(
            proxy_config,
            ExplicitProxyConfig {
                http: Some("http://lower-http:3128".to_string()),
                https: Some("http://lower-https:3128".to_string()),
            }
        );
        clear_proxy_env();
    }

    #[test]
    fn test_explicit_proxy_config_from_env_falls_back_to_http_for_https() {
        let _guard = proxy_env_lock().lock().unwrap();
        clear_proxy_env();
        unsafe {
            env::set_var("HTTP_PROXY", "http://proxy.example:3128");
        }

        let proxy_config = explicit_proxy_config_from_env();

        assert_eq!(
            proxy_config,
            ExplicitProxyConfig {
                http: Some("http://proxy.example:3128".to_string()),
                https: Some("http://proxy.example:3128".to_string()),
            }
        );
        clear_proxy_env();
    }

    #[test]
    fn test_build_reqwest_client_accepts_proxy_env_urls() {
        let _guard = proxy_env_lock().lock().unwrap();
        clear_proxy_env();
        unsafe {
            env::set_var("http_proxy", "http://127.0.0.1:3128");
            env::set_var("https_proxy", "http://127.0.0.1:3128");
        }

        let client = build_reqwest_client(None, None);
        assert!(client.is_ok());
        clear_proxy_env();
    }
}
