//! MCP Transport Layer
//!
//! Provides transport abstraction for MCP communication.
//! - `stdio`: Local process communication via stdin/stdout
//! - `http_sse`: Remote server communication via HTTP + Server-Sent Events

use std::sync::Arc;

pub mod http_sse;
mod notifications;
pub mod stdio;

pub use stdio::SamplingHandler;
pub mod streamable_http;

use crate::mcp::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpNotification};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

/// MCP transport trait
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Install a server-sampling handler (the host's opt-in decision
    /// point). Transports without server-request support ignore it.
    fn set_sampling_handler(&self, _handler: Arc<dyn SamplingHandler>) {}

    /// Revoke a locally owned transport synchronously when a caller is dropped.
    /// Asynchronous `close` remains the settlement acknowledgement boundary.
    fn start_close(&self) {}

    /// Send request and wait for response
    async fn request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// Send notification (no response expected)
    async fn notify(&self, notification: JsonRpcNotification) -> Result<()>;

    /// Get the host notification receiver. Built-in transports hand off one
    /// FIFO, retain a bounded prefix before subscription, and apply bounded
    /// backpressure after the host claims it. Later calls return a closed FIFO.
    fn notifications(&self) -> mpsc::Receiver<McpNotification>;

    /// Independent, coalesced tools-list changes for automatic cache refresh.
    /// Built-in transports provide this without claiming the host's FIFO.
    /// Custom transports returning None retain the legacy single-consumer path.
    fn tool_list_changes(&self) -> Option<tokio::sync::watch::Receiver<()>> {
        None
    }

    /// Close the transport
    async fn close(&self) -> Result<()>;

    /// Check if transport is connected
    fn is_connected(&self) -> bool;
}
