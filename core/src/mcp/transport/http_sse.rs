//! HTTP+SSE Transport for MCP
//!
//! Implements MCP transport over HTTP (requests) and Server-Sent Events (notifications)
//! for remote MCP server communication.
//!
//! ## Protocol
//!
//! - **Requests**: POST JSON-RPC to the server's HTTP endpoint
//! - **Notifications (outbound)**: POST to the server's HTTP endpoint
//! - **Notifications (inbound)**: Received via SSE stream from the server
//! - **Reconnection**: Automatic reconnect with exponential backoff on SSE disconnect

use super::McpTransport;
use crate::llm::http::build_reqwest_client;
use crate::mcp::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpNotification};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

/// Default request timeout
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 60;

/// Default SSE reconnect base delay
const DEFAULT_RECONNECT_BASE_MS: u64 = 500;

/// Maximum SSE reconnect delay
const MAX_RECONNECT_DELAY_MS: u64 = 30_000;

/// HTTP+SSE transport for remote MCP servers
pub struct HttpSseTransport {
    /// Base URL of the MCP server (e.g., "http://localhost:3000/mcp")
    base_url: String,
    /// HTTP client
    client: Client,
    /// Custom headers (e.g., auth tokens) — retained for reconnection
    #[allow(dead_code)]
    headers: HashMap<String, String>,
    /// Notification receiver (from SSE stream)
    notification_rx: RwLock<Option<mpsc::Receiver<McpNotification>>>,
    /// Connected flag (shared with SSE listener)
    connected: Arc<AtomicBool>,
    /// Request timeout
    request_timeout: Duration,
    /// Abort handle for SSE listener task
    sse_abort: RwLock<Option<tokio::task::AbortHandle>>,
}

impl HttpSseTransport {
    /// Connect to a remote MCP server via HTTP+SSE.
    ///
    /// - `base_url`: The server's HTTP endpoint (e.g., "http://localhost:3000/mcp")
    /// - `headers`: Optional custom headers (e.g., `Authorization: Bearer ...`)
    pub async fn connect(
        base_url: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Result<Self> {
        Self::connect_with_timeout(base_url, headers, DEFAULT_REQUEST_TIMEOUT_SECS).await
    }

    /// Connect with a custom request timeout.
    pub async fn connect_with_timeout(
        base_url: impl Into<String>,
        headers: HashMap<String, String>,
        request_timeout_secs: u64,
    ) -> Result<Self> {
        let base_url = base_url.into().trim_end_matches('/').to_string();

        // Build default headers
        let mut header_map = reqwest::header::HeaderMap::new();
        header_map.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        for (key, value) in &headers {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                reqwest::header::HeaderValue::from_str(value),
            ) {
                header_map.insert(name, val);
            }
        }
        let client = build_reqwest_client(
            Some(Duration::from_secs(request_timeout_secs)),
            Some(header_map),
        )
        .context("Failed to build HTTP client")?;

        // Verify connectivity with a simple GET (optional, non-fatal)
        let ping_url = format!("{}/health", base_url);
        let reachable = client.get(&ping_url).send().await.is_ok();
        if !reachable {
            tracing::debug!(
                url = %base_url,
                "MCP server health check failed (non-fatal, will attempt SSE connection)"
            );
        }

        // Create notification channel
        let (notification_tx, notification_rx) = mpsc::channel::<McpNotification>(256);

        // Spawn SSE listener
        let sse_url = format!("{}/sse", base_url);
        let sse_client = client.clone();
        let connected = Arc::new(AtomicBool::new(true));
        let sse_connected = Arc::clone(&connected);

        let sse_handle = tokio::spawn(async move {
            Self::sse_listener(sse_client, sse_url, notification_tx, sse_connected).await;
        });

        Ok(Self {
            base_url,
            client,
            headers,
            notification_rx: RwLock::new(Some(notification_rx)),
            connected,
            request_timeout: Duration::from_secs(request_timeout_secs),
            sse_abort: RwLock::new(Some(sse_handle.abort_handle())),
        })
    }

    /// SSE listener loop with automatic reconnection
    async fn sse_listener(
        client: Client,
        url: String,
        tx: mpsc::Sender<McpNotification>,
        connected: Arc<AtomicBool>,
    ) {
        let mut reconnect_delay = DEFAULT_RECONNECT_BASE_MS;

        loop {
            if !connected.load(Ordering::SeqCst) {
                break;
            }

            tracing::debug!(url = %url, "Connecting to MCP SSE endpoint");

            match client
                .get(&url)
                .header("Accept", "text/event-stream")
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    tracing::info!(url = %url, "Connected to MCP SSE endpoint");
                    reconnect_delay = DEFAULT_RECONNECT_BASE_MS; // Reset on success

                    // Read SSE stream line by line
                    use futures::StreamExt;
                    let mut stream = response.bytes_stream();
                    let mut buffer = String::new();

                    while let Some(chunk) = stream.next().await {
                        if !connected.load(Ordering::SeqCst) {
                            return;
                        }

                        match chunk {
                            Ok(bytes) => {
                                buffer.push_str(&String::from_utf8_lossy(&bytes));

                                // Process complete SSE events (double newline separated)
                                while let Some(pos) = buffer.find("\n\n") {
                                    let event_text = buffer[..pos].to_string();
                                    buffer = buffer[pos + 2..].to_string();

                                    if let Some(notification) = Self::parse_sse_event(&event_text) {
                                        if tx.send(notification).await.is_err() {
                                            tracing::debug!("SSE notification channel closed");
                                            return;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "SSE stream error");
                                break;
                            }
                        }
                    }

                    tracing::debug!("SSE stream ended, will reconnect");
                }
                Ok(response) => {
                    tracing::warn!(
                        status = %response.status(),
                        "SSE connection failed with status"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "SSE connection failed");
                }
            }

            if !connected.load(Ordering::SeqCst) {
                break;
            }

            // Exponential backoff
            tracing::debug!(delay_ms = reconnect_delay, "SSE reconnecting after delay");
            tokio::time::sleep(Duration::from_millis(reconnect_delay)).await;
            reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY_MS);
        }
    }

    /// Parse an SSE event into an MCP notification
    fn parse_sse_event(event_text: &str) -> Option<McpNotification> {
        let mut data = String::new();

        for line in event_text.lines() {
            if let Some(value) = crate::sse::data_field_value(line) {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
            // Ignore "event:", "id:", "retry:" fields for now
        }

        if data.is_empty() {
            return None;
        }

        match serde_json::from_str::<JsonRpcNotification>(&data) {
            Ok(notification) => Some(McpNotification::from_json_rpc(&notification)),
            Err(e) => {
                tracing::debug!(error = %e, data = %data, "Failed to parse SSE data as JSON-RPC notification");
                None
            }
        }
    }
}

#[async_trait]
impl McpTransport for HttpSseTransport {
    async fn request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(anyhow!("Transport not connected"));
        }

        let url = format!("{}/message", self.base_url);
        let body = serde_json::to_string(&request)?;

        let response = self
            .client
            .post(&url)
            .body(body)
            .send()
            .await
            .with_context(|| format!("HTTP request to {} failed", url))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("MCP server returned HTTP {}: {}", status, body));
        }

        let json_response: JsonRpcResponse = response
            .json()
            .await
            .context("Failed to parse JSON-RPC response")?;

        Ok(json_response)
    }

    async fn notify(&self, notification: JsonRpcNotification) -> Result<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(anyhow!("Transport not connected"));
        }

        let url = format!("{}/message", self.base_url);
        let body = serde_json::to_string(&notification)?;

        let response = self
            .client
            .post(&url)
            .body(body)
            .send()
            .await
            .with_context(|| format!("HTTP notification to {} failed", url))?;

        if !response.status().is_success() {
            let status = response.status();
            tracing::warn!(
                status = %status,
                "MCP notification returned non-success status"
            );
        }

        Ok(())
    }

    fn notifications(&self) -> mpsc::Receiver<McpNotification> {
        let mut rx_guard = self.notification_rx.blocking_write();
        rx_guard.take().unwrap_or_else(|| {
            let (_, rx) = mpsc::channel(1);
            rx
        })
    }

    async fn close(&self) -> Result<()> {
        self.connected.store(false, Ordering::SeqCst);

        // Abort SSE listener
        let mut abort_guard = self.sse_abort.write().await;
        if let Some(handle) = abort_guard.take() {
            handle.abort();
        }

        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for HttpSseTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpSseTransport")
            .field("base_url", &self.base_url)
            .field("connected", &self.connected.load(Ordering::Relaxed))
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sse_event_valid() {
        let event = "data: {\"jsonrpc\":\"2.0\",\"method\":\"test\",\"params\":null}";
        let result = HttpSseTransport::parse_sse_event(event);
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_sse_event_multiline_data() {
        // Multi-line data fields get concatenated with newlines
        let event = "data: {\"jsonrpc\":\"2.0\",\ndata: \"method\":\"test\",\"params\":null}";
        let result = HttpSseTransport::parse_sse_event(event);
        // The concatenated string is: {"jsonrpc":"2.0",\n"method":"test","params":null}
        // which is valid JSON (JSON allows whitespace including newlines)
        // so this should parse successfully
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_sse_event_empty() {
        let result = HttpSseTransport::parse_sse_event("");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_event_no_data() {
        let event = "event: ping\nid: 123";
        let result = HttpSseTransport::parse_sse_event(event);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_event_invalid_json() {
        let event = "data: not-json";
        let result = HttpSseTransport::parse_sse_event(event);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_event_with_event_field() {
        let event = "event: notification\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"update\",\"params\":null}";
        let result = HttpSseTransport::parse_sse_event(event);
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_sse_event_data_no_space() {
        // "data:" with no space after colon
        let event = "data:{\"jsonrpc\":\"2.0\",\"method\":\"test\",\"params\":null}";
        let result = HttpSseTransport::parse_sse_event(event);
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_http_sse_transport_connect_invalid_url() {
        // Should succeed in creating transport (SSE connects lazily)
        let result = HttpSseTransport::connect("http://127.0.0.1:1", HashMap::new()).await;
        // Connection creation should succeed (SSE reconnects in background)
        assert!(result.is_ok());
        if let Ok(transport) = result {
            assert!(transport.is_connected());
            transport.close().await.unwrap();
            assert!(!transport.is_connected());
        }
    }

    #[tokio::test]
    async fn test_http_sse_transport_close() {
        let transport = HttpSseTransport::connect("http://127.0.0.1:1", HashMap::new())
            .await
            .unwrap();

        assert!(transport.is_connected());
        transport.close().await.unwrap();
        assert!(!transport.is_connected());
    }

    #[tokio::test]
    async fn test_http_sse_transport_double_close() {
        let transport = HttpSseTransport::connect("http://127.0.0.1:1", HashMap::new())
            .await
            .unwrap();

        transport.close().await.unwrap();
        // Second close should not panic
        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_http_sse_transport_request_after_close() {
        let transport = HttpSseTransport::connect("http://127.0.0.1:1", HashMap::new())
            .await
            .unwrap();

        transport.close().await.unwrap();

        let request = JsonRpcRequest::new(1, "test", None);
        let result = transport.request(request).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn test_http_sse_transport_notify_after_close() {
        let transport = HttpSseTransport::connect("http://127.0.0.1:1", HashMap::new())
            .await
            .unwrap();

        transport.close().await.unwrap();

        let notification = JsonRpcNotification::new("test", None);
        let result = transport.notify(notification).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn test_http_sse_transport_custom_timeout() {
        let transport =
            HttpSseTransport::connect_with_timeout("http://127.0.0.1:1", HashMap::new(), 5)
                .await
                .unwrap();

        assert_eq!(transport.request_timeout, Duration::from_secs(5));
        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_http_sse_transport_with_headers() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer test-token".to_string());
        headers.insert("X-Custom".to_string(), "value".to_string());

        let transport = HttpSseTransport::connect("http://127.0.0.1:1", headers)
            .await
            .unwrap();

        assert_eq!(transport.headers.len(), 2);
        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_http_sse_transport_debug() {
        let transport = HttpSseTransport::connect("http://127.0.0.1:1", HashMap::new())
            .await
            .unwrap();

        let debug = format!("{:?}", transport);
        assert!(debug.contains("HttpSseTransport"));
        assert!(debug.contains("127.0.0.1"));
        transport.close().await.unwrap();
    }

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_REQUEST_TIMEOUT_SECS, 60);
        assert_eq!(DEFAULT_RECONNECT_BASE_MS, 500);
        assert_eq!(MAX_RECONNECT_DELAY_MS, 30_000);
    }
}
