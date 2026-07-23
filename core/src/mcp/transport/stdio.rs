//! Stdio Transport for MCP
//!
//! Implements MCP transport over standard input/output for local process communication.

use super::McpTransport;
use crate::mcp::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpNotification};
use crate::tools::process::{configure_process_group, ProcessGroupGuard};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, Command};
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::task::JoinHandle;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::sync::CancellationToken;

/// Default request timeout for MCP tool calls
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 60;
const PROCESS_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_MCP_STDIO_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Stdio transport for MCP servers
pub struct StdioTransport {
    /// Process group owned by the child. The synchronous guard is also the
    /// drop-time backstop when an async close cannot run.
    process_group: Arc<StdMutex<ProcessGroupGuard>>,
    /// Monitor that owns and reaps the direct child.
    process_task: StdMutex<Option<JoinHandle<std::io::Result<()>>>>,
    /// Stdin/stdout/stderr workers. Async close awaits them so notification
    /// EOF and pending-request settlement happen before close returns.
    io_tasks: StdMutex<Vec<JoinHandle<()>>>,
    /// Stdin writer
    stdin_tx: mpsc::Sender<String>,
    /// Pending requests (id -> response sender)
    pending: Arc<RwLock<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Notification receiver
    notification_rx: RwLock<Option<mpsc::Receiver<McpNotification>>>,
    /// Connected flag
    connected: Arc<AtomicBool>,
    /// Stops the stdin/stdout/stderr tasks during close and drop.
    shutdown: CancellationToken,
    /// Per-request timeout in seconds
    request_timeout_secs: u64,
}

impl StdioTransport {
    /// Create a new stdio transport by spawning a process
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        Self::spawn_with_timeout(command, args, env, DEFAULT_REQUEST_TIMEOUT_SECS).await
    }

    /// Create a new stdio transport with a custom request timeout
    pub async fn spawn_with_timeout(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        request_timeout_secs: u64,
    ) -> Result<Self> {
        // Spawn the process
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut cmd);

        // Add environment variables
        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {} {:?}", command, args))?;
        let process_group = ProcessGroupGuard::for_child(&child);

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("No stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("No stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("No stderr"))?;

        // Create channels
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(100);
        let (notification_tx, notification_rx) = mpsc::channel::<McpNotification>(100);
        let pending: Arc<RwLock<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let connected = Arc::new(AtomicBool::new(true));
        let shutdown = CancellationToken::new();
        let process_group = Arc::new(StdMutex::new(process_group));
        let process_task = tokio::spawn(monitor_child(
            child,
            Arc::clone(&process_group),
            shutdown.clone(),
        ));

        // Spawn stdin writer task
        let mut stdin_writer = stdin;
        let writer_connected = Arc::clone(&connected);
        let writer_pending = Arc::clone(&pending);
        let writer_shutdown = shutdown.clone();
        let writer_task = tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    _ = writer_shutdown.cancelled() => break,
                    message = stdin_rx.recv() => message,
                };
                let Some(message) = message else {
                    break;
                };
                let write = async {
                    stdin_writer.write_all(message.as_bytes()).await?;
                    stdin_writer.flush().await
                };
                let result = tokio::select! {
                    _ = writer_shutdown.cancelled() => break,
                    result = write => result,
                };
                if let Err(error) = result {
                    tracing::error!("Failed to write to MCP stdin: {}", error);
                    break;
                }
            }
            writer_connected.store(false, Ordering::SeqCst);
            writer_pending.write().await.clear();
            writer_shutdown.cancel();
        });

        // Spawn stdout reader task
        let pending_clone = pending.clone();
        let reader_connected = Arc::clone(&connected);
        let reader_shutdown = shutdown.clone();
        let reader_task = tokio::spawn(async move {
            let mut reader = FramedRead::new(
                stdout,
                LinesCodec::new_with_max_length(MAX_MCP_STDIO_LINE_BYTES),
            );
            loop {
                let read = tokio::select! {
                    _ = reader_shutdown.cancelled() => break,
                    read = reader.next() => read,
                };
                match read {
                    None => {
                        tracing::debug!("MCP stdout closed");
                        break;
                    }
                    Some(Ok(line)) => {
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
                            tokio::select! {
                                _ = reader_shutdown.cancelled() => break,
                                _ = notification_tx.send(mcp_notif) => {}
                            }
                            continue;
                        }

                        tracing::warn!("Unknown MCP message: {}", trimmed);
                    }
                    Some(Err(e)) => {
                        tracing::error!("Failed to read MCP stdout: {}", e);
                        break;
                    }
                }
            }
            reader_connected.store(false, Ordering::SeqCst);
            pending_clone.write().await.clear();
            reader_shutdown.cancel();
        });
        let stderr_task = tokio::spawn(drain_stderr(stderr, shutdown.clone()));

        Ok(Self {
            process_group,
            process_task: StdMutex::new(Some(process_task)),
            io_tasks: StdMutex::new(vec![writer_task, reader_task, stderr_task]),
            stdin_tx,
            pending,
            notification_rx: RwLock::new(Some(notification_rx)),
            connected,
            shutdown,
            request_timeout_secs,
        })
    }

    fn kill_process_group(&self) {
        self.process_group
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .kill();
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.connected.store(false, Ordering::SeqCst);
        self.shutdown.cancel();
        self.process_group
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .kill();
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
        let request_id = request.id;

        // Register pending request
        {
            let mut pending = self.pending.write().await;
            pending.insert(request_id, tx);
        }
        if !self.connected.load(Ordering::SeqCst) {
            self.pending.write().await.remove(&request_id);
            return Err(anyhow!("Transport not connected"));
        }

        // Serialize and send request
        let msg = serde_json::to_string(&request)? + "\n";
        self.stdin_tx
            .send(msg)
            .await
            .map_err(|_| anyhow!("Failed to send request"))?;

        // Wait for response with timeout
        let response = match tokio::time::timeout(
            std::time::Duration::from_secs(self.request_timeout_secs),
            rx,
        )
        .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                // Channel closed — clean up pending entry
                self.pending.write().await.remove(&request_id);
                return Err(anyhow!("Response channel closed"));
            }
            Err(_) => {
                // Timeout — clean up pending entry to prevent memory leak
                self.pending.write().await.remove(&request_id);
                return Err(anyhow!(
                    "MCP request timed out after {}s",
                    self.request_timeout_secs
                ));
            }
        };

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
        self.shutdown.cancel();
        self.pending.write().await.clear();
        self.kill_process_group();

        let process_task = self
            .process_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let process_result = if let Some(process_task) = process_task {
            match tokio::time::timeout(PROCESS_SETTLEMENT_TIMEOUT * 2, process_task).await {
                Ok(Ok(Ok(()))) => Ok(()),
                Ok(Ok(Err(error))) => {
                    Err(error).context("Failed to reap MCP server after termination")
                }
                Ok(Err(error)) => Err(anyhow!("MCP server monitor task failed: {error}")),
                Err(_) => Err(anyhow!(
                    "MCP server monitor did not settle after termination"
                )),
            }
        } else {
            Ok(())
        };
        let io_tasks = self
            .io_tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect();
        let io_result = settle_io_tasks(io_tasks).await;

        process_result?;
        io_result
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

async fn settle_io_tasks(tasks: Vec<JoinHandle<()>>) -> Result<()> {
    let mut first_error = None;
    for mut task in tasks {
        match tokio::time::timeout(PROCESS_SETTLEMENT_TIMEOUT, &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                first_error.get_or_insert_with(|| anyhow!("MCP stdio task failed: {error}"));
            }
            Err(_) => {
                task.abort();
                let _ = task.await;
                first_error
                    .get_or_insert_with(|| anyhow!("MCP stdio task did not settle during close"));
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

async fn monitor_child(
    mut child: Child,
    process_group: Arc<StdMutex<ProcessGroupGuard>>,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let result = tokio::select! {
        result = child.wait() => result,
        _ = shutdown.cancelled() => {
            process_group
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .kill();
            let _ = child.start_kill();
            match tokio::time::timeout(PROCESS_SETTLEMENT_TIMEOUT, child.wait()).await {
                Ok(result) => result,
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "MCP server did not exit after process-group termination",
                    ));
                }
            }
        }
    };
    // A server may leave helpers alive even after its direct process exits.
    process_group
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .kill();
    result.map(|_| ())
}

async fn drain_stderr(mut stderr: ChildStderr, shutdown: CancellationToken) {
    let mut chunk = [0_u8; 4096];
    loop {
        let read = tokio::select! {
            _ = shutdown.cancelled() => break,
            read = stderr.read(&mut chunk) => read,
        };
        match read {
            Ok(0) => break,
            Ok(count) => {
                tracing::debug!(
                    "MCP server stderr: {}",
                    String::from_utf8_lossy(&chunk[..count]).trim_end()
                );
            }
            Err(error) => {
                tracing::debug!("Failed to read MCP stderr: {}", error);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    async fn wait_for_path(path: &std::path::Path) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("MCP test process did not start");
    }

    #[cfg(unix)]
    async fn spawn_descendant_writer(
        started: &std::path::Path,
        leaked: &std::path::Path,
    ) -> StdioTransport {
        let args = vec![
            "-c".to_string(),
            "touch \"$1\"; (sleep 0.30; touch \"$2\") & wait".to_string(),
            "mcp-process-tree-test".to_string(),
            started.to_string_lossy().into_owned(),
            leaked.to_string_lossy().into_owned(),
        ];
        StdioTransport::spawn("/bin/sh", &args, &HashMap::new())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_stdio_transport_spawn_invalid_command() {
        let result = StdioTransport::spawn("nonexistent_command_12345", &[], &HashMap::new()).await;
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

    #[tokio::test]
    async fn test_stdio_transport_is_connected_initial() {
        let result = StdioTransport::spawn("cat", &[], &HashMap::new()).await;
        if let Ok(transport) = result {
            assert!(transport.is_connected());
            let _ = transport.close().await;
        }
    }

    #[tokio::test]
    async fn test_stdio_transport_close_disconnects() {
        let result = StdioTransport::spawn("cat", &[], &HashMap::new()).await;
        if let Ok(transport) = result {
            assert!(transport.is_connected());
            transport.close().await.unwrap();
            assert!(!transport.is_connected());
        }
    }

    #[tokio::test]
    async fn test_stdio_transport_spawn_with_args() {
        let args = vec!["--version".to_string()];
        let result = StdioTransport::spawn("cat", &args, &HashMap::new()).await;
        // May fail depending on system, but should not panic
        let _ = result;
    }

    #[tokio::test]
    async fn test_stdio_transport_spawn_with_env() {
        let mut env = HashMap::new();
        env.insert("TEST_VAR".to_string(), "test_value".to_string());
        let result = StdioTransport::spawn("cat", &[], &env).await;
        if let Ok(transport) = result {
            let _ = transport.close().await;
        }
    }

    #[tokio::test]
    async fn test_stdio_transport_double_close() {
        let result = StdioTransport::spawn("cat", &[], &HashMap::new()).await;
        if let Ok(transport) = result {
            transport.close().await.unwrap();
            // Second close should not panic
            let result = transport.close().await;
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_stdio_transport_request_after_close() {
        let result = StdioTransport::spawn("cat", &[], &HashMap::new()).await;
        if let Ok(transport) = result {
            transport.close().await.unwrap();

            let request = JsonRpcRequest::new(1, "test", None);
            let result = transport.request(request).await;
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("not connected"));
        }
    }

    #[tokio::test]
    async fn test_stdio_transport_notify_after_close() {
        let result = StdioTransport::spawn("cat", &[], &HashMap::new()).await;
        if let Ok(transport) = result {
            transport.close().await.unwrap();

            let notification = JsonRpcNotification::new("test", None);
            let result = transport.notify(notification).await;
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("not connected"));
        }
    }

    #[test]
    fn test_json_rpc_request_creation() {
        let request =
            JsonRpcRequest::new(1, "test_method", Some(serde_json::json!({"key": "value"})));
        assert_eq!(request.id, 1);
        assert_eq!(request.method, "test_method");
        assert!(request.params.is_some());
    }

    #[test]
    fn test_json_rpc_notification_creation() {
        let notification = JsonRpcNotification::new("test_notification", None);
        assert_eq!(notification.method, "test_notification");
        assert!(notification.params.is_none());
    }

    #[tokio::test]
    async fn test_stdio_transport_custom_timeout() {
        // Spawn with a very short timeout (1 second)
        let result = StdioTransport::spawn_with_timeout("cat", &[], &HashMap::new(), 1).await;
        if let Ok(transport) = result {
            assert_eq!(transport.request_timeout_secs, 1);
            let _ = transport.close().await;
        }
    }

    #[tokio::test]
    async fn test_stdio_transport_default_timeout() {
        let result = StdioTransport::spawn("cat", &[], &HashMap::new()).await;
        if let Ok(transport) = result {
            assert_eq!(transport.request_timeout_secs, DEFAULT_REQUEST_TIMEOUT_SECS);
            let _ = transport.close().await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_kills_the_entire_mcp_process_group() {
        let directory = tempfile::tempdir().unwrap();
        let started = directory.path().join("started");
        let leaked = directory.path().join("close-leak");
        let transport = spawn_descendant_writer(&started, &leaked).await;
        wait_for_path(&started).await;

        transport.close().await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;

        assert!(
            !leaked.exists(),
            "closing an MCP transport must kill server descendants"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn drop_kills_the_entire_mcp_process_group() {
        let directory = tempfile::tempdir().unwrap();
        let started = directory.path().join("started");
        let leaked = directory.path().join("drop-leak");
        let transport = spawn_descendant_writer(&started, &leaked).await;
        wait_for_path(&started).await;

        drop(transport);
        tokio::time::sleep(Duration::from_millis(400)).await;

        assert!(
            !leaked.exists(),
            "dropping an MCP transport must kill server descendants"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn protocol_eof_reaps_a_still_running_server_tree() {
        let directory = tempfile::tempdir().unwrap();
        let descendant_started = directory.path().join("descendant-started");
        let leaked = directory.path().join("protocol-eof-leak");
        let args = vec![
            "-c".to_string(),
            "(: > \"$1\"; sleep 0.30; : > \"$2\") >/dev/null 2>&1 & \
             while [ ! -e \"$1\" ]; do :; done; exec 1>&- 2>&-; wait"
                .to_string(),
            "mcp-protocol-eof-test".to_string(),
            descendant_started.to_string_lossy().into_owned(),
            leaked.to_string_lossy().into_owned(),
        ];
        let transport = StdioTransport::spawn("/bin/sh", &args, &HashMap::new())
            .await
            .unwrap();
        wait_for_path(&descendant_started).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while transport.is_connected() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("protocol EOF did not disconnect the MCP transport");

        transport.close().await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;

        assert!(
            !leaked.exists(),
            "protocol EOF must reap the MCP server and every descendant"
        );
    }
}
