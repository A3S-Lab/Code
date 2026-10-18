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
            Self::transport(
                operation,
                format!("timed out: {}", render_error_chain(&error)),
            )
        } else {
            // `Display` on reqwest 0.12 stops at "error sending request".
            // The "connection refused" text lives on the source chain, and
            // without it a closed port is retried as a 503.
            Self::transport(operation, render_error_chain(&error))
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            // Immediate connect/DNS failures rarely recover after a 30s sleep;
            // treating them as retryable 503s burns ~3 minutes of silent
            // Working… before the user sees the real transport error.
            Self::Transport { message, .. } => !is_hard_connect_failure(message),
            Self::Cancelled { .. } | Self::InvalidRequest { .. } => false,
        }
    }
}

/// True when the failure is a hard connectivity problem (reset, refused, DNS),
/// not a transient mid-stream blip or request timeout.
/// `Display` plus each `source`, capped so a cyclic chain cannot grow forever.
fn render_error_chain(error: &dyn std::error::Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    let mut depth = 0_u8;
    while let Some(cause) = source {
        if depth >= 8 {
            break;
        }
        depth = depth.saturating_add(1);
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}

pub(crate) fn is_hard_connect_failure(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("network is unreachable")
        || lower.contains("no route to host")
        || lower.contains("name or service not known")
        || lower.contains("nodename nor servname provided")
        || lower.contains("failed to lookup address information")
        || lower.contains("dns error")
        || lower.contains("could not resolve host")
        || lower.contains("temporary failure in name resolution")
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
    /// `Retry-After` header when the provider sent one.
    pub retry_after: Option<String>,
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
/// A fail-closed stand-in for an HTTP client that could not be built
/// (e.g. TLS backend initialization failed at runtime — an
/// environment-dependent failure, never a construction invariant). Every
/// request through it names the reason instead of panicking the process:
/// a session-start path must degrade to erroring calls, not die (a
/// panic in the exec path kills colocated workloads).
pub struct UnavailableHttpClient {
    reason: String,
}

impl UnavailableHttpClient {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    fn refusal(&self) -> anyhow::Error {
        anyhow::anyhow!("LLM HTTP client is unavailable: {}", self.reason)
    }
}

#[async_trait]
impl HttpClient for UnavailableHttpClient {
    async fn post(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel_token: CancellationToken,
    ) -> Result<HttpResponse> {
        Err(self.refusal())
    }

    async fn post_streaming(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel_token: CancellationToken,
    ) -> Result<StreamingHttpResponse> {
        Err(self.refusal())
    }
}

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

    /// POST a raw body and return the response stream.
    ///
    /// The default refuses. Clients that speak a framed protocol override it.
    /// JSON `post_streaming` stays the path for chat-completions providers.
    async fn post_bytes_streaming(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &[u8],
        _cancel_token: CancellationToken,
    ) -> Result<StreamingHttpResponse> {
        Err(anyhow::anyhow!(
            "HTTP client does not support a raw streaming body"
        ))
    }
    /// Keep a framed request open while the caller supplies protocol replies.
    async fn post_bytes_duplex(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &[u8],
        _replies: tokio::sync::mpsc::Receiver<Vec<u8>>,
        _cancel_token: CancellationToken,
    ) -> Result<StreamingHttpResponse> {
        Err(anyhow::anyhow!(
            "HTTP client does not support duplex streaming"
        ))
    }
}

/// Default HTTP client backed by reqwest
pub struct ReqwestHttpClient {
    client: reqwest::Client,
    /// Total deadline for a non-streaming response body.
    ///
    /// Streaming does not use this. A live generation may run longer than
    /// the non-streaming budget while bytes are still arriving. Silence on
    /// that stream is bounded by [`stream_idle_timeout`].
    request_timeout: Option<Duration>,
}

/// Maximum quiet time between chunks of a streaming response.
///
/// The idle bound is what turns a silently stalled provider connection into a
/// retryable failure: streaming responses have no total deadline (a legitimate
/// generation may run for minutes), but a connection that delivers no bytes at
/// all is dead. `A3S_CODE_LLM_STREAM_IDLE_TIMEOUT_MS` overrides the default;
/// `0` disables the bound entirely.
pub(crate) fn stream_idle_timeout() -> Duration {
    const DEFAULT_IDLE_TIMEOUT_MS: u64 = 300_000;
    let configured = env::var("A3S_CODE_LLM_STREAM_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok());
    let milliseconds = configured.unwrap_or(DEFAULT_IDLE_TIMEOUT_MS);
    Duration::from_millis(if milliseconds == 0 {
        u64::MAX
    } else {
        milliseconds
    })
}

/// Maximum wait for response HEADERS on a streaming request.
///
/// SSE headers arrive before generation starts, so unlike the body there is
/// no legitimate reason to wait minutes for them: a provider or proxy that
/// accepts the connection but never answers must surface as a retryable
/// failure instead of parking the model call (and the session) forever.
/// The non-streaming `post` deliberately has no such bound — its headers
/// only arrive after the full generation, which may legitimately take
/// minutes. `A3S_CODE_LLM_STREAM_HEADERS_TIMEOUT_MS` overrides the default;
/// `0` disables the bound entirely.
pub(crate) fn stream_headers_timeout() -> Duration {
    const DEFAULT_HEADERS_TIMEOUT_MS: u64 = 120_000;
    let configured = env::var("A3S_CODE_LLM_STREAM_HEADERS_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok());
    let milliseconds = configured.unwrap_or(DEFAULT_HEADERS_TIMEOUT_MS);
    Duration::from_millis(if milliseconds == 0 {
        u64::MAX
    } else {
        milliseconds
    })
}

impl ReqwestHttpClient {
    pub fn new() -> Self {
        // Construction-level fallback, not an expect: the default builder
        // fails on a malformed http_proxy/https_proxy env var or a TLS
        // backend init error, which must not panic the process at client
        // construction. The proxy env values are the usual cause and are
        // optional — a bare no-proxy reqwest client still serves direct
        // connections and surfaces any remaining failure per-request
        // instead of aborting startup.
        let client = build_reqwest_client(None, None).unwrap_or_else(|error| {
            tracing::warn!(
                error = %error,
                "default HTTP client build failed (malformed proxy env or TLS init); falling back to a no-proxy client"
            );
            reqwest::Client::builder().no_proxy().build().unwrap_or_default()
        });
        Self {
            client,
            request_timeout: None,
        }
    }

    pub fn with_timeout(timeout: Duration) -> Result<Self> {
        // Do not install reqwest's total timeout. It covers the response
        // body, so a streaming generation is cancelled at the same instant
        // as a non-streaming call. Non-streaming `post` applies this
        // duration itself. Streaming keeps the header and idle bounds.
        Ok(Self {
            client: build_reqwest_client(None, None)?,
            request_timeout: Some(timeout),
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

        // Verbose logs carry endpoint and size only. The request body holds
        // the whole conversation (system prompt, history, tool results) and
        // must never land in logs or tracing backends.
        tracing::debug!("HTTP POST to {} ({} bytes)", url, request_bytes);

        let mut request = self.client.post(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        request = request.json(body);

        let request_timeout = self.request_timeout;
        let send_and_read = async move {
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
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .map(String::from);
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
                retry_after,
            })
        };
        if let Some(timeout) = request_timeout {
            match tokio::time::timeout(timeout, send_and_read).await {
                Ok(result) => result,
                Err(_elapsed) => Err(anyhow::Error::new(HttpClientError::transport(
                    "HTTP request",
                    format!("timed out: no response for {}ms", timeout.as_millis()),
                ))),
            }
        } else {
            send_and_read.await
        }
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

        // SSE headers precede generation: bound how long a connected-but-
        // silent provider may hold the request before the call fails
        // retryable. Without this, a proxy that accepts TCP and never
        // answers hangs the model call until the user cancels.
        let headers_timeout = stream_headers_timeout();
        let headers_timeout_ms = headers_timeout.as_millis();
        let response = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                return Err(anyhow::Error::new(HttpClientError::cancelled(
                    "HTTP streaming request",
                )));
            }
            _ = tokio::time::sleep(headers_timeout) => {
                return Err(anyhow::Error::new(HttpClientError::transport(
                    "HTTP streaming request",
                    format!(
                        "timed out: no response headers for {headers_timeout_ms}ms"
                    ),
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
            let idle_timeout = stream_idle_timeout();
            let idle_timeout_ms = idle_timeout.as_millis();
            let raw = response.bytes_stream().map(|result| {
                result.map_err(|error| {
                    anyhow::Error::new(HttpClientError::from_reqwest("HTTP response stream", error))
                })
            });
            // Bound the inter-chunk idle time. A provider or proxy that stops
            // sending data without closing the connection must surface as a
            // retryable transport failure instead of hanging the caller's
            // stream loop (and the user's session) forever.
            let guarded = tokio_stream::StreamExt::timeout(Box::pin(raw), idle_timeout);
            let byte_stream = guarded.map(move |item| match item {
                Ok(Ok(chunk)) => Ok(chunk),
                Ok(Err(error)) => Err(error),
                Err(_elapsed) => Err(anyhow::Error::new(HttpClientError::transport(
                    "HTTP response stream",
                    format!("timed out: no stream data received for {idle_timeout_ms}ms"),
                ))),
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

    async fn post_bytes_streaming(
        &self,
        url: &str,
        headers: Vec<(&str, &str)>,
        body: &[u8],
        cancel_token: CancellationToken,
    ) -> Result<StreamingHttpResponse> {
        self.post_raw_streaming(
            url,
            headers,
            reqwest::Body::from(body.to_vec()),
            body.len() as u64,
            cancel_token,
        )
        .await
    }

    async fn post_bytes_duplex(
        &self,
        url: &str,
        headers: Vec<(&str, &str)>,
        body: &[u8],
        replies: tokio::sync::mpsc::Receiver<Vec<u8>>,
        cancel_token: CancellationToken,
    ) -> Result<StreamingHttpResponse> {
        let first = futures::stream::iter([body.to_vec()]);
        let frames = first
            .chain(tokio_stream::wrappers::ReceiverStream::new(replies))
            .map(Ok::<_, std::io::Error>);
        self.post_raw_streaming(
            url,
            headers,
            reqwest::Body::wrap_stream(frames),
            body.len() as u64,
            cancel_token,
        )
        .await
    }
}

impl ReqwestHttpClient {
    async fn post_raw_streaming(
        &self,
        url: &str,
        headers: Vec<(&str, &str)>,
        body: reqwest::Body,
        request_bytes: u64,
        cancel_token: CancellationToken,
    ) -> Result<StreamingHttpResponse> {
        let start = std::time::Instant::now();
        tracing::debug!("HTTP POST to {} ({} bytes)", url, request_bytes);

        let mut request = self.client.post(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        request = request.body(body);

        let headers_timeout = stream_headers_timeout();
        let headers_timeout_ms = headers_timeout.as_millis();
        let response = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                return Err(anyhow::Error::new(HttpClientError::cancelled(
                    "HTTP streaming request",
                )));
            }
            _ = tokio::time::sleep(headers_timeout) => {
                return Err(anyhow::Error::new(HttpClientError::transport(
                    "HTTP streaming request",
                    format!(
                        "timed out: no response headers for {headers_timeout_ms}ms"
                    ),
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
            .and_then(|value| value.to_str().ok())
            .map(String::from);
        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
        maybe_record_metrics(HttpMetricsRecord {
            url: url.to_string(),
            method: "POST".to_string(),
            status,
            duration_ms,
            request_bytes,
            response_bytes: 0,
            streaming: true,
        });

        if (200..300).contains(&status) {
            let idle_timeout = stream_idle_timeout();
            let idle_timeout_ms = idle_timeout.as_millis();
            let raw = response.bytes_stream().map(|result| {
                result.map_err(|error| {
                    anyhow::Error::new(HttpClientError::from_reqwest("HTTP response stream", error))
                })
            });
            let guarded = tokio_stream::StreamExt::timeout(Box::pin(raw), idle_timeout);
            let byte_stream = guarded.map(move |item| match item {
                Ok(Ok(chunk)) => Ok(chunk),
                Ok(Err(error)) => Err(error),
                Err(_elapsed) => Err(anyhow::Error::new(HttpClientError::transport(
                    "HTTP response stream",
                    format!("timed out: no stream data received for {idle_timeout_ms}ms"),
                ))),
            });
            Ok(StreamingHttpResponse {
                status,
                retry_after,
                byte_stream: Box::pin(byte_stream),
                error_body: String::new(),
            })
        } else {
            let error_body = response.text().await.unwrap_or_default();
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

    // TCP+TLS connect only — no generation is involved, so a host (or proxy)
    // that never completes the handshake must fail instead of hanging the
    // request phase, which otherwise has no deadline.
    builder = builder.connect_timeout(Duration::from_secs(30));

    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }

    if let Some(default_headers) = default_headers {
        builder = builder.default_headers(default_headers);
    }

    let proxy_config = explicit_proxy_config_from_env();
    // Explicit Proxy::{http,https} start with no_proxy=None. Attach the
    // process NO_PROXY / no_proxy list so localhost and named MCP hosts
    // bypass the proxy the same way curl and the system stack do.
    let no_proxy = reqwest::NoProxy::from_env();
    if let Some(http_proxy) = proxy_config.http.as_deref() {
        builder = builder.proxy(
            reqwest::Proxy::http(http_proxy)
                .with_context(|| format!("Invalid HTTP proxy URL: {http_proxy}"))?
                .no_proxy(no_proxy.clone()),
        );
    }
    if let Some(https_proxy) = proxy_config.https.as_deref() {
        builder = builder.proxy(
            reqwest::Proxy::https(https_proxy)
                .with_context(|| format!("Invalid HTTPS proxy URL: {https_proxy}"))?
                .no_proxy(no_proxy),
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

fn base_has_versioned_api_root(base: &str) -> bool {
    base.contains("/api/")
        || base.ends_with("/v4")
        || base.ends_with("/v3")
        || base.ends_with("/v2")
}

/// Join a normalized provider `base_url` with a chat-completions path.
///
/// Avoids duplicated path segments when the user supplies a base URL that
/// already includes a provider API root (for example Zhipu Coding Plan
/// `https://open.bigmodel.cn/api/coding/paas/v4`).
pub(crate) fn join_chat_completions_url(base_url: &str, chat_path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if chat_path.is_empty() {
        return base.to_string();
    }
    let mut path = if chat_path.starts_with('/') {
        chat_path.to_string()
    } else {
        format!("/{chat_path}")
    };

    if base.ends_with("/chat/completions") {
        return base.to_string();
    }

    // OpenAI-compatible default path is `/v1/chat/completions`. When the base
    // already ends at a versioned API root (…/paas/v4, …/api/…), append only
    // `/chat/completions` so Coding Plan URLs are not turned into …/v4/v1/….
    if path.starts_with("/v1/") && base_has_versioned_api_root(base) {
        path = path.replacen("/v1", "", 1);
    }

    // Built-in Zhipu uses `/api/paas/v4/chat/completions` against the host-only
    // default base. If the user overrides base_url to a Coding Plan root that
    // already contains `/paas/v4`, keep only the suffix after that marker.
    const PAAS_V4: &str = "/paas/v4";
    if base.contains(PAAS_V4) {
        if let Some(idx) = path.find(PAAS_V4) {
            let suffix = &path[idx + PAAS_V4.len()..];
            return format!("{base}{suffix}");
        }
    }

    format!("{base}{path}")
}

#[cfg(test)]
mod duplex_tests;

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

    #[tokio::test]
    async fn connection_refused_is_not_retryable() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("port").port();
        drop(listener);
        let client = {
            let _guard = proxy_env_lock().lock().expect("proxy env lock");
            clear_proxy_env();
            ReqwestHttpClient::new()
        };
        let error = match client
            .post(
                &format!("http://127.0.0.1:{port}/v1/chat/completions"),
                Vec::new(),
                &serde_json::json!({"model": "x"}),
                CancellationToken::new(),
            )
            .await
        {
            Ok(_) => panic!("closed port must fail"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(
            message.to_ascii_lowercase().contains("connection refused"),
            "{message}"
        );
        assert!(
            !is_retryable_http_failure(&error),
            "a refused connection must not be retried: {message}"
        );
    }

    #[test]
    fn hard_connect_failures_are_not_retryable() {
        let reset = anyhow::Error::new(HttpClientError::transport(
            "API request",
            "error sending request for url (https://api.deepseek.com/v1/chat/completions): connection reset",
        ));
        assert!(!is_retryable_http_failure(&reset));
        assert!(is_hard_connect_failure(
            "error sending request … connection reset by peer"
        ));

        let refused = anyhow::Error::new(HttpClientError::transport(
            "API request",
            "tcp connect error: Connection refused (os error 61)",
        ));
        assert!(!is_retryable_http_failure(&refused));

        let timeout = anyhow::Error::new(HttpClientError::transport(
            "API request",
            "timed out: operation timed out",
        ));
        assert!(is_retryable_http_failure(&timeout));
    }

    fn proxy_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn clear_proxy_env() {
        for key in [
            "http_proxy",
            "HTTP_PROXY",
            "https_proxy",
            "HTTPS_PROXY",
            "no_proxy",
            "NO_PROXY",
        ] {
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
    fn join_chat_completions_url_openai_default() {
        assert_eq!(
            join_chat_completions_url("https://api.openai.com", "/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn join_chat_completions_url_zhipu_default() {
        assert_eq!(
            join_chat_completions_url("https://open.bigmodel.cn", "/api/paas/v4/chat/completions"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
    }

    #[test]
    fn join_chat_completions_url_coding_plan_openai_compatible() {
        // Issue #136: custom OpenAI-compatible provider with Coding Plan base.
        assert_eq!(
            join_chat_completions_url(
                "https://open.bigmodel.cn/api/coding/paas/v4",
                "/v1/chat/completions"
            ),
            "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions"
        );
    }

    #[test]
    fn join_chat_completions_url_coding_plan_builtin_zhipu() {
        // Issue #136: built-in zhipu with Coding Plan base_url override.
        assert_eq!(
            join_chat_completions_url(
                "https://open.bigmodel.cn/api/coding/paas/v4",
                "/api/paas/v4/chat/completions"
            ),
            "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions"
        );
    }

    #[test]
    fn join_chat_completions_url_full_endpoint_as_base() {
        assert_eq!(
            join_chat_completions_url(
                "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions",
                "/v1/chat/completions"
            ),
            "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions"
        );
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

        // A localhost timeout must exercise reqwest's request deadline, not a
        // developer/CI HTTP proxy. Build the client while holding the same
        // environment lock as the proxy configuration tests, then release the
        // lock before awaiting the network operation.
        let client = {
            let _guard = proxy_env_lock().lock().unwrap();
            clear_proxy_env();
            ReqwestHttpClient::with_timeout(Duration::from_millis(50)).unwrap()
        };
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

    #[test]
    fn test_build_reqwest_client_accepts_no_proxy_with_proxies() {
        let _guard = proxy_env_lock().lock().unwrap();
        clear_proxy_env();
        unsafe {
            env::set_var("HTTP_PROXY", "http://proxy.example:3128");
            env::set_var("HTTPS_PROXY", "http://proxy.example:3128");
            env::set_var("NO_PROXY", "playwright-mcp,localhost,127.0.0.1");
        }

        assert!(
            reqwest::NoProxy::from_env().is_some(),
            "NO_PROXY must parse into a reqwest exclusion list"
        );
        let client = build_reqwest_client(None, None);
        assert!(
            client.is_ok(),
            "client must build when proxies and NO_PROXY are both set"
        );
        clear_proxy_env();
    }

    #[test]
    #[cfg(not(windows))]
    fn test_no_proxy_from_env_prefers_lowercase_var() {
        let _guard = proxy_env_lock().lock().unwrap();
        clear_proxy_env();
        unsafe {
            env::set_var("no_proxy", "lower.example");
            env::set_var("NO_PROXY", "upper.example");
        }

        // reqwest reads lowercase first; ensure the env we clear/set matches
        // what build_reqwest_client will attach via NoProxy::from_env().
        assert!(reqwest::NoProxy::from_env().is_some());
        clear_proxy_env();
    }
    #[allow(clippy::await_holding_lock)] // serialized critical section; blocking is the point
    #[tokio::test]
    #[cfg(not(windows))]
    async fn test_post_streaming_surfaces_stalled_stream_as_timeout_error() {
        let _guard = proxy_env_lock().lock().unwrap();
        clear_proxy_env();
        unsafe {
            env::set_var("A3S_CODE_LLM_STREAM_IDLE_TIMEOUT_MS", "150");
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf).await;
            // Deliver one chunk, then go silent without finishing the
            // chunked response and without closing the connection — the
            // stalled-proxy shape that previously hung callers forever.
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n")
                .await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let client = ReqwestHttpClient::new();
        let response = client
            .post_streaming(
                &format!("http://{addr}/v1/messages"),
                Vec::new(),
                &serde_json::json!({"model": "test"}),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 200);

        let mut stream = response.byte_stream;
        let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .expect("first chunk must arrive before the stall")
            .expect("first chunk must not be an error");
        assert_eq!(&first[..], &b"hello"[..]);

        let second = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .expect("the idle timeout must yield a terminal item");
        let error = second.expect_err("a stalled stream must surface as an error");
        let message = format!("{error:#}");
        assert!(
            message.contains("timed out"),
            "idle timeout must render as a retryable timeout, got: {message}"
        );
        assert!(message.contains("150ms"), "got: {message}");

        server.abort();
        unsafe {
            env::remove_var("A3S_CODE_LLM_STREAM_IDLE_TIMEOUT_MS");
        }
    }

    /// A provider that accepts the connection but never returns response
    /// headers must fail retryable within the headers deadline — the shape
    /// that previously parked the model call until the user cancelled.
    #[allow(clippy::await_holding_lock)] // serialized critical section; blocking is the point
    #[tokio::test]
    #[cfg(not(windows))]
    async fn test_post_streaming_surfaces_silent_headers_as_timeout_error() {
        let _guard = proxy_env_lock().lock().unwrap();
        clear_proxy_env();
        unsafe {
            env::set_var("A3S_CODE_LLM_STREAM_HEADERS_TIMEOUT_MS", "150");
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            // Read the request, then hold the connection open without ever
            // sending response headers — the silent-proxy shape.
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let client = ReqwestHttpClient::new();
        let error = match client
            .post_streaming(
                &format!("http://{addr}/v1/messages"),
                Vec::new(),
                &serde_json::json!({"model": "test"}),
                CancellationToken::new(),
            )
            .await
        {
            Ok(_) => panic!("a silent proxy must fail within the headers deadline"),
            Err(error) => error,
        };
        let message = format!("{error:#}");
        assert!(
            message.contains("timed out") && message.contains("150ms"),
            "headers timeout must render as a retryable timeout, got: {message}"
        );

        server.abort();
        unsafe {
            env::remove_var("A3S_CODE_LLM_STREAM_HEADERS_TIMEOUT_MS");
        }
    }

    /// A non-streaming request budget must not cancel a streaming body that
    /// keeps sending bytes past that budget. Silence is a separate bound.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    #[cfg(not(windows))]
    async fn streaming_body_survives_a_short_non_streaming_request_timeout() {
        let _guard = proxy_env_lock().lock().unwrap();
        clear_proxy_env();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 2048];
            let _ = stream.read(&mut buf).await;
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n")
                .await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = stream.write_all(b"5\r\nworld\r\n0\r\n\r\n").await;
        });

        let client = ReqwestHttpClient::with_timeout(Duration::from_millis(80)).unwrap();
        let response = client
            .post_streaming(
                &format!("http://{addr}/v1/chat/completions"),
                Vec::new(),
                &serde_json::json!({"model": "test"}),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 200);

        let mut stream = response.byte_stream;
        let mut body = Vec::new();
        while let Some(chunk) = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("a live stream must not inherit the non-streaming deadline")
            .transpose()
            .expect("chunks stay readable past the non-streaming budget")
        {
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("hello"), "{text}");
        assert!(text.contains("world"), "{text}");
        server.abort();
    }
}
