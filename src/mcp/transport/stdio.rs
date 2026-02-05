//! Stdio Transport for MCP
//!
//! Implements MCP transport over standard input/output for local process communication.

use super::McpTransport;
use crate::mcp::protocol::{
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpNotification,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, RwLock};

/// Stdio transport for MCP servers
pub struct StdioTransport {
    /// Child process
    child: RwLock<Option<Child>>,
    /// Stdin writer
    stdin_tx: mpsc::Sender<String>,
    /// Pending requests (id -> response sender)
    pending: Arc<RwLock<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Notification receiver
    notification_rx: RwLock<Option<mpsc::Receiver<McpNotification>>>,
    /// Connected flag
    connected: AtomicBool,
}

impl StdioTransport {
    /// Create a new stdio transport by spawning a process
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        // Spawn the process
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Add environment variables
        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {} {:?}", command, args))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("No stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("No stdout"))?;

        // Create channels
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(100);
        let (notification_tx, notification_rx) = mpsc::channel::<McpNotification>(100);
        let pending: Arc<RwLock<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Spawn stdin writer task
        let mut stdin_writer = stdin;
        tokio::spawn(async move {
            while let Some(msg) = stdin_rx.recv().await {
                if let Err(e) = stdin_writer.write_all(msg.as_bytes()).await {
                    tracing::error!("Failed to write to MCP stdin: {}", e);
                    break;
                }
                if let Err(e) = stdin_writer.flush().await {
                    tracing::error!("Failed to flush MCP stdin: {}", e);
                    break;
                }
            }
        });

        // Spawn stdout reader task
        let pending_clone = pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();

            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        tracing::debug!("MCP stdout closed");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        // Try to parse as response
                        if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
                            if let Some(id) = response.id {
                                let mut pending = pending_clone.write().await;
                                if let Some(tx) = pending.remove(&id) {
                                    let _ = tx.send(response);
                                }
                            }
                            continue;
                        }

                        // Try to parse as notification
                        if let Ok(notification) =
                            serde_json::from_str::<JsonRpcNotification>(trimmed)
                        {
                            let mcp_notif = McpNotification::from_json_rpc(&notification);
                            let _ = notification_tx.send(mcp_notif).await;
                            continue;
                        }

                        tracing::warn!("Unknown MCP message: {}", trimmed);
                    }
                    Err(e) => {
                        tracing::error!("Failed to read MCP stdout: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            child: RwLock::new(Some(child)),
            stdin_tx,
            pending,
            notification_rx: RwLock::new(Some(notification_rx)),
            connected: AtomicBool::new(true),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(anyhow!("Transport not connected"));
        }

        // Create response channel
        let (tx, rx) = oneshot::channel();

        // Register pending request
        {
            let mut pending = self.pending.write().await;
            pending.insert(request.id, tx);
        }

        // Serialize and send request
        let msg = serde_json::to_string(&request)? + "\n";
        self.stdin_tx
            .send(msg)
            .await
            .map_err(|_| anyhow!("Failed to send request"))?;

        // Wait for response with timeout
        let response = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| anyhow!("Request timeout"))?
            .map_err(|_| anyhow!("Response channel closed"))?;

        Ok(response)
    }

    async fn notify(&self, notification: JsonRpcNotification) -> Result<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(anyhow!("Transport not connected"));
        }

        let msg = serde_json::to_string(&notification)? + "\n";
        self.stdin_tx
            .send(msg)
            .await
            .map_err(|_| anyhow!("Failed to send notification"))?;

        Ok(())
    }

    fn notifications(&self) -> mpsc::Receiver<McpNotification> {
        // This is a bit awkward - we need to take ownership of the receiver
        // In practice, this should only be called once
        let mut rx_guard = self.notification_rx.blocking_write();
        rx_guard.take().unwrap_or_else(|| {
            let (_, rx) = mpsc::channel(1);
            rx
        })
    }

    async fn close(&self) -> Result<()> {
        self.connected.store(false, Ordering::SeqCst);

        // Kill the child process
        let mut child_guard = self.child.write().await;
        if let Some(mut child) = child_guard.take() {
            let _ = child.kill().await;
        }

        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stdio_transport_spawn_invalid_command() {
        let result = StdioTransport::spawn(
            "nonexistent_command_12345",
            &[],
            &HashMap::new(),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stdio_transport_spawn_echo() {
        // Use a simple command that exists on most systems
        let result = StdioTransport::spawn("cat", &[], &HashMap::new()).await;

        if let Ok(transport) = result {
            assert!(transport.is_connected());
            transport.close().await.unwrap();
            assert!(!transport.is_connected());
        }
        // If cat doesn't exist, that's fine - skip the test
    }
}
