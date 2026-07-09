//! Streamable HTTP Transport for MCP
//!
//! Implements the MCP 2025-03-26 Streamable HTTP transport spec.
//!
//! ## Protocol
//!
//! All communication goes through a single HTTP endpoint:
//!
//! - **Requests**: POST JSON-RPC with `Accept: application/json, text/event-stream`
//!   - Server may respond with `Content-Type: application/json` (single response)
//!   - Server may respond with `Content-Type: text/event-stream` (streaming response)
//! - **Notifications (outbound)**: POST JSON-RPC notification (no response body expected)
//! - **Notifications (inbound)**: GET `{url}` with `Accept: text/event-stream` for server-push
//! - **Session**: Server may return `Mcp-Session-Id` header; client echoes it on subsequent requests
//!
//! ## Compatibility
//!
//! Falls back gracefully: if the server returns plain JSON, it is used directly.
//! If the server returns SSE, the first `data:` event containing a JSON-RPC response is used.

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

const DEFAULT_TIMEOUT_SECS: u64 = 60;
const SSE_RECONNECT_BASE_MS: u64 = 500;
const SSE_RECONNECT_MAX_MS: u64 = 30_000;

/// Streamable HTTP transport (MCP 2025-03-26)
pub struct StreamableHttpTransport {
    /// MCP endpoint URL
    url: String,
    client: Client,
    /// Optional session ID returned by server
    session_id: RwLock<Option<String>>,
    connected: Arc<AtomicBool>,
    notification_rx: RwLock<Option<mpsc::Receiver<McpNotification>>>,
    sse_abort: RwLock<Option<tokio::task::AbortHandle>>,
}

impl StreamableHttpTransport {
    /// Connect to a Streamable HTTP MCP server.
    pub async fn connect(url: impl Into<String>, headers: HashMap<String, String>) -> Result<Self> {
        Self::connect_with_timeout(url, headers, DEFAULT_TIMEOUT_SECS).await
    }

    pub async fn connect_with_timeout(
        url: impl Into<String>,
        headers: HashMap<String, String>,
        timeout_secs: u64,
    ) -> Result<Self> {
        let url = url.into().trim_end_matches('/').to_string();

        let mut header_map = reqwest::header::HeaderMap::new();
        header_map.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        for (k, v) in &headers {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(v),
            ) {
                header_map.insert(name, val);
            }
        }

        let client =
            build_reqwest_client(Some(Duration::from_secs(timeout_secs)), Some(header_map))
                .context("Failed to build HTTP client")?;

        let (notification_tx, notification_rx) = mpsc::channel::<McpNotification>(256);
        let connected = Arc::new(AtomicBool::new(true));

        // Spawn SSE listener for server-initiated notifications (GET endpoint)
        let sse_client = client.clone();
        let sse_url = url.clone();
        let sse_connected = Arc::clone(&connected);
        let sse_handle = tokio::spawn(async move {
            Self::sse_listener(sse_client, sse_url, notification_tx, sse_connected).await;
        });

        Ok(Self {
            url,
            client,
            session_id: RwLock::new(None),
            connected,
            notification_rx: RwLock::new(Some(notification_rx)),
            sse_abort: RwLock::new(Some(sse_handle.abort_handle())),
        })
    }

    /// Build request headers including session ID if present.
    async fn request_headers(&self) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        map.insert(
            "Accept",
            "application/json, text/event-stream".parse().unwrap(),
        );
        if let Some(ref sid) = *self.session_id.read().await {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(sid) {
                map.insert("Mcp-Session-Id", val);
            }
        }
        map
    }

    /// Extract and store session ID from response headers.
    async fn capture_session_id(&self, headers: &reqwest::header::HeaderMap) {
        if let Some(val) = headers.get("Mcp-Session-Id") {
            if let Ok(s) = val.to_str() {
                let mut sid = self.session_id.write().await;
                *sid = Some(s.to_string());
            }
        }
    }

    /// Parse a JSON-RPC response from either plain JSON or SSE stream body.
    async fn parse_response(response: reqwest::Response) -> Result<JsonRpcResponse> {
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if content_type.contains("text/event-stream") {
            // Read SSE stream and return first JSON-RPC response event
            Self::parse_sse_response(response).await
        } else {
            // Plain JSON response
            response
                .json::<JsonRpcResponse>()
                .await
                .context("Failed to parse JSON-RPC response")
        }
    }

    /// Read an SSE stream and return the first JSON-RPC response found.
    async fn parse_sse_response(response: reqwest::Response) -> Result<JsonRpcResponse> {
        use futures::StreamExt;

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.context("SSE stream error")?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            // Process complete SSE events (double newline separated)
            while let Some(pos) = buffer.find("\n\n") {
                let event_text = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                if let Some(resp) = Self::extract_json_rpc_response(&event_text) {
                    return Ok(resp);
                }
            }
        }

        Err(anyhow!("SSE stream ended without a JSON-RPC response"))
    }

    /// Extract a JSON-RPC response from an SSE event block.
    fn extract_json_rpc_response(event_text: &str) -> Option<JsonRpcResponse> {
        let mut data = String::new();
        for line in event_text.lines() {
            if let Some(value) = crate::sse::data_field_value(line) {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
        }
        if data.is_empty() {
            return None;
        }
        serde_json::from_str::<JsonRpcResponse>(&data).ok()
    }

    /// Extract an MCP notification from an SSE event block.
    fn extract_notification(event_text: &str) -> Option<McpNotification> {
        let mut data = String::new();
        for line in event_text.lines() {
            if let Some(value) = crate::sse::data_field_value(line) {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
        }
        if data.is_empty() {
            return None;
        }
        serde_json::from_str::<crate::mcp::protocol::JsonRpcNotification>(&data)
            .ok()
            .map(|n| McpNotification::from_json_rpc(&n))
    }

    /// SSE listener for server-initiated notifications (GET endpoint).
    async fn sse_listener(
        client: Client,
        url: String,
        tx: mpsc::Sender<McpNotification>,
        connected: Arc<AtomicBool>,
    ) {
        let mut delay = SSE_RECONNECT_BASE_MS;

        loop {
            if !connected.load(Ordering::SeqCst) {
                break;
            }

            let result = client
                .get(&url)
                .header("Accept", "text/event-stream")
                .send()
                .await;

            match result {
                Ok(resp) if resp.status().is_success() => {
                    delay = SSE_RECONNECT_BASE_MS;
                    use futures::StreamExt;
                    let mut stream = resp.bytes_stream();
                    let mut buffer = String::new();

                    while let Some(chunk) = stream.next().await {
                        if !connected.load(Ordering::SeqCst) {
                            return;
                        }
                        match chunk {
                            Ok(bytes) => {
                                buffer.push_str(&String::from_utf8_lossy(&bytes));
                                while let Some(pos) = buffer.find("\n\n") {
                                    let event = buffer[..pos].to_string();
                                    buffer = buffer[pos + 2..].to_string();
                                    if let Some(n) = Self::extract_notification(&event) {
                                        if tx.send(n).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Streamable HTTP SSE stream error");
                                break;
                            }
                        }
                    }
                }
                Ok(resp) => {
                    // Server doesn't support GET SSE — that's fine, notifications come inline
                    tracing::debug!(
                        status = %resp.status(),
                        "Streamable HTTP GET SSE not supported by server (status {}), skipping",
                        resp.status()
                    );
                    return;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "Streamable HTTP SSE GET failed");
                }
            }

            if !connected.load(Ordering::SeqCst) {
                break;
            }

            tokio::time::sleep(Duration::from_millis(delay)).await;
            delay = (delay * 2).min(SSE_RECONNECT_MAX_MS);
        }
    }
}

#[async_trait]
impl McpTransport for StreamableHttpTransport {
    async fn request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(anyhow!("Transport not connected"));
        }

        let extra_headers = self.request_headers().await;
        let body = serde_json::to_string(&request)?;

        let response = self
            .client
            .post(&self.url)
            .headers(extra_headers)
            .body(body)
            .send()
            .await
            .with_context(|| format!("Streamable HTTP POST to {} failed", self.url))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("MCP server returned HTTP {}: {}", status, body));
        }

        self.capture_session_id(response.headers()).await;
        Self::parse_response(response).await
    }

    async fn notify(&self, notification: JsonRpcNotification) -> Result<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(anyhow!("Transport not connected"));
        }

        let extra_headers = self.request_headers().await;
        let body = serde_json::to_string(&notification)?;

        let response = self
            .client
            .post(&self.url)
            .headers(extra_headers)
            .body(body)
            .send()
            .await
            .with_context(|| format!("Streamable HTTP notification to {} failed", self.url))?;

        if !response.status().is_success() {
            tracing::warn!(
                status = %response.status(),
                "Streamable HTTP notification returned non-success status"
            );
        }

        Ok(())
    }

    fn notifications(&self) -> mpsc::Receiver<McpNotification> {
        let mut rx = self.notification_rx.blocking_write();
        rx.take().unwrap_or_else(|| {
            let (_, rx) = mpsc::channel(1);
            rx
        })
    }

    async fn close(&self) -> Result<()> {
        self.connected.store(false, Ordering::SeqCst);

        // Send DELETE to terminate session if we have a session ID
        if let Some(ref sid) = *self.session_id.read().await {
            let mut headers = reqwest::header::HeaderMap::new();
            if let Ok(val) = reqwest::header::HeaderValue::from_str(sid) {
                headers.insert("Mcp-Session-Id", val);
            }
            let _ = self.client.delete(&self.url).headers(headers).send().await;
        }

        let mut abort = self.sse_abort.write().await;
        if let Some(handle) = abort.take() {
            handle.abort();
        }

        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for StreamableHttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamableHttpTransport")
            .field("url", &self.url)
            .field("connected", &self.connected.load(Ordering::Relaxed))
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
    fn test_extract_json_rpc_response_valid() {
        let event = r#"data: {"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let result = StreamableHttpTransport::extract_json_rpc_response(event);
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, Some(1));
    }

    #[test]
    fn test_extract_json_rpc_response_data_no_space() {
        let event = r#"data:{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let result = StreamableHttpTransport::extract_json_rpc_response(event);
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, Some(1));
    }

    #[test]
    fn test_extract_json_rpc_response_empty() {
        let result = StreamableHttpTransport::extract_json_rpc_response("");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_json_rpc_response_no_data_prefix() {
        let event = r#"event: message"#;
        let result = StreamableHttpTransport::extract_json_rpc_response(event);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_json_rpc_response_invalid_json() {
        let event = "data: not-json";
        let result = StreamableHttpTransport::extract_json_rpc_response(event);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_notification_valid() {
        let event = r#"data: {"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#;
        let result = StreamableHttpTransport::extract_notification(event);
        assert!(result.is_some());
        match result.unwrap() {
            McpNotification::ToolsListChanged => {}
            _ => panic!("Expected ToolsListChanged"),
        }
    }

    #[test]
    fn test_extract_notification_data_no_space() {
        let event = r#"data:{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#;
        let result = StreamableHttpTransport::extract_notification(event);
        assert!(result.is_some());
        match result.unwrap() {
            McpNotification::ToolsListChanged => {}
            _ => panic!("Expected ToolsListChanged"),
        }
    }

    #[test]
    fn test_extract_notification_empty() {
        let result = StreamableHttpTransport::extract_notification("");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_connect_and_close() {
        let transport = StreamableHttpTransport::connect("http://127.0.0.1:1", HashMap::new())
            .await
            .unwrap();
        assert!(transport.is_connected());
        transport.close().await.unwrap();
        assert!(!transport.is_connected());
    }

    #[tokio::test]
    async fn test_request_after_close_returns_error() {
        let transport = StreamableHttpTransport::connect("http://127.0.0.1:1", HashMap::new())
            .await
            .unwrap();
        transport.close().await.unwrap();
        let req = JsonRpcRequest::new(1, "test", None);
        let result = transport.request(req).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn test_notify_after_close_returns_error() {
        let transport = StreamableHttpTransport::connect("http://127.0.0.1:1", HashMap::new())
            .await
            .unwrap();
        transport.close().await.unwrap();
        let notif = JsonRpcNotification::new("test", None);
        let result = transport.notify(notif).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn test_double_close() {
        let transport = StreamableHttpTransport::connect("http://127.0.0.1:1", HashMap::new())
            .await
            .unwrap();
        transport.close().await.unwrap();
        transport.close().await.unwrap(); // must not panic
    }

    #[tokio::test]
    async fn test_custom_timeout() {
        let transport =
            StreamableHttpTransport::connect_with_timeout("http://127.0.0.1:1", HashMap::new(), 5)
                .await
                .unwrap();
        assert!(transport.is_connected());
        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_with_auth_header() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token".to_string());
        let transport = StreamableHttpTransport::connect("http://127.0.0.1:1", headers)
            .await
            .unwrap();
        assert!(transport.is_connected());
        transport.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_debug_format() {
        let transport = StreamableHttpTransport::connect("http://127.0.0.1:1", HashMap::new())
            .await
            .unwrap();
        let s = format!("{:?}", transport);
        assert!(s.contains("StreamableHttpTransport"));
        transport.close().await.unwrap();
    }
}
