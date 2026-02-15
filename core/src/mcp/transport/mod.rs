//! MCP Transport Layer
//!
//! Provides transport abstraction for MCP communication.

pub mod stdio;

use crate::mcp::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpNotification};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

/// MCP transport trait
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send request and wait for response
    async fn request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// Send notification (no response expected)
    async fn notify(&self, notification: JsonRpcNotification) -> Result<()>;

    /// Get notification receiver
    fn notifications(&self) -> mpsc::Receiver<McpNotification>;

    /// Close the transport
    async fn close(&self) -> Result<()>;

    /// Check if transport is connected
    fn is_connected(&self) -> bool;
}
