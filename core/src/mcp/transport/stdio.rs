//! Stdio Transport for MCP
//!
//! Implements MCP transport over standard input/output for local process communication.

use super::notifications::NotificationReceiver;
use super::McpTransport;
use crate::mcp::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpNotification};
use crate::tools::process::{configure_process_group, spawn_tokio_child, ProcessGroupGuard};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, Command};
use tokio::sync::{mpsc, oneshot, watch, RwLock};
use tokio::task::JoinHandle;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::sync::CancellationToken;

/// Default request timeout for MCP tool calls
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 60;
const PROCESS_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_MCP_STDIO_LINE_BYTES: usize = 8 * 1024 * 1024;

type ProcessSettlement = Option<std::result::Result<(), String>>;

/// Stdio transport for MCP servers
/// Server-initiated sampling (MCP spec `sampling/createMessage`). The
/// handler is the host's decision point: installing one declares the
/// capability and answers sampling with the host's model; not installing
/// keeps the honest -32601 refusal.
#[async_trait]
pub trait SamplingHandler: Send + Sync {
    async fn create_message(&self, params: serde_json::Value) -> Result<serde_json::Value>;
}

pub struct StdioTransport {
    /// Process group owned by the child. The synchronous guard is also the
    /// drop-time backstop when an async close cannot run.
    process_group: Arc<StdMutex<ProcessGroupGuard>>,
    /// Monitor that owns and reaps the direct child.
    process_task: StdMutex<Option<JoinHandle<std::io::Result<()>>>>,
    process_settlement: watch::Receiver<ProcessSettlement>,
    /// Stdin/stdout/stderr workers. Async close awaits them so notification
    /// EOF and pending-request settlement happen before close returns.
    io_tasks: StdMutex<Vec<JoinHandle<()>>>,
    /// Stdin writer
    stdin_tx: mpsc::Sender<String>,
    sampling_handler: Arc<StdRwLock<Option<Arc<dyn SamplingHandler>>>>,
    /// Pending requests (id -> response sender)
    pending: Arc<RwLock<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Notification receiver
    notification_rx: NotificationReceiver,
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
        Self::spawn_with_timeout_and_retention(command, args, env, request_timeout_secs, None, None)
            .await
    }

    pub(crate) async fn spawn_with_timeout_and_retention(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        request_timeout_secs: u64,
        retention: Option<Arc<dyn crate::capability::RetainedUseGeneration>>,
        working_directory: Option<&std::path::Path>,
    ) -> Result<Self> {
        // Spawn the process
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        configure_process_group(&mut cmd);
        if let Some(directory) = working_directory {
            cmd.current_dir(directory);
        }

        // Add environment variables
        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = spawn_tokio_child(&mut cmd)
            .with_context(|| format!("Failed to spawn MCP server: {} {:?}", command, args))?;
        let process_group = ProcessGroupGuard::for_child(&child);

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("No stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("No stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("No stderr"))?;

        // Create channels
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(100);
        let (notification_tx, notification_rx) = NotificationReceiver::channel(100);
        let pending: Arc<RwLock<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let connected = Arc::new(AtomicBool::new(true));
        let shutdown = CancellationToken::new();
        let process_group = Arc::new(StdMutex::new(process_group));
        let monitored_group = Arc::clone(&process_group);
        let monitored_shutdown = shutdown.clone();
        let retained = retention.is_some();
        let (settlement_tx, process_settlement) = watch::channel(None);
        let process_task = tokio::spawn(async move {
            let result = monitor_child(child, monitored_group, monitored_shutdown, retained).await;
            drop(retention);
            let _ = settlement_tx.send(Some(
                result.as_ref().map(|_| ()).map_err(ToString::to_string),
            ));
            result
        });

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
        let sampling_handler: Arc<StdRwLock<Option<Arc<dyn SamplingHandler>>>> =
            Arc::new(StdRwLock::new(None));
        let handler_clone = Arc::clone(&sampling_handler);
        let pending_clone = pending.clone();
        let reader_connected = Arc::clone(&connected);
        let reader_shutdown = shutdown.clone();
        let reply_tx = stdin_tx.clone();
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
                        // LinesCodec already dropped a trailing CR. Classification
                        // trims and parses that same line; an empty line is ignored.
                        let Some(inbound) = classify_stdio_line(&line) else {
                            continue;
                        };
                        match inbound {
                            StdioInbound::ServerRequest(value) => {
                                // Server-initiated REQUESTS (id + method) must be
                                // answered, never silently dropped: a ping with no
                                // reply makes spec-conformant servers treat the client
                                // as dead, and an unanswered sampling/roots request
                                // hangs the server. Ping gets an empty result; an
                                // undeclared-capability request gets -32601 so the
                                // server fails fast instead of waiting.
                                let id =
                                    value.get("id").cloned().unwrap_or(serde_json::Value::Null);
                                let method = value
                                    .get("method")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or_default()
                                    .to_string();
                                if method == "sampling/createMessage" {
                                    let handler = handler_clone
                                        .read()
                                        .unwrap_or_else(|p| p.into_inner())
                                        .clone();
                                    match handler {
                                        Some(handler) => {
                                            let params = value
                                                .get("params")
                                                .cloned()
                                                .unwrap_or(serde_json::Value::Null);
                                            let reply = match handler.create_message(params).await {
                                                Ok(result) => serde_json::json!({
                                                    "jsonrpc": "2.0",
                                                    "id": id,
                                                    "result": result,
                                                }),
                                                Err(error) => serde_json::json!({
                                                    "jsonrpc": "2.0",
                                                    "id": id,
                                                    "error": {
                                                        "code": -32603,
                                                        "message": format!("sampling failed: {error}"),
                                                    },
                                                }),
                                            };
                                            let frame = format!(
                                                "{}\n",
                                                serde_json::to_string(&reply).unwrap_or_default()
                                            );
                                            tokio::select! {
                                                _ = reader_shutdown.cancelled() => break,
                                                _ = reply_tx.send(frame) => {}
                                            }
                                            continue;
                                        }
                                        None => {
                                            // No handler installed: fall through to the
                                            // -32601 refusal below.
                                        }
                                    }
                                }
                                let reply = if method == "ping" {
                                    serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {},
                                    })
                                } else {
                                    serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "error": {
                                            "code": -32601,
                                            "message": format!(
                                                "a3s-code MCP client does not implement \
                                                 {method} (capability not declared)"
                                            ),
                                        },
                                    })
                                };
                                let frame = format!(
                                    "{}\n",
                                    serde_json::to_string(&reply).unwrap_or_default()
                                );
                                tokio::select! {
                                    _ = reader_shutdown.cancelled() => break,
                                    _ = reply_tx.send(frame) => {}
                                }
                            }
                            StdioInbound::Response(response) => {
                                if let Some(id) = response.id {
                                    let mut pending = pending_clone.write().await;
                                    if let Some(tx) = pending.remove(&id) {
                                        let _ = tx.send(response);
                                    }
                                }
                            }
                            StdioInbound::Notification(notification) => {
                                let mcp_notif = McpNotification::from_json_rpc(&notification);
                                tokio::select! {
                                    _ = reader_shutdown.cancelled() => break,
                                    _ = notification_tx.send(mcp_notif) => {}
                                }
                            }
                            StdioInbound::Unknown(trimmed) => {
                                tracing::warn!("Unknown MCP message: {}", trimmed);
                            }
                        }
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
            sampling_handler,
            process_task: StdMutex::new(Some(process_task)),
            process_settlement,
            io_tasks: StdMutex::new(vec![writer_task, reader_task, stderr_task]),
            stdin_tx,
            pending,
            notification_rx,
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

impl StdioTransport {
    /// Install the sampling handler: declares the capability and answers
    /// `sampling/createMessage` with the host's model. Not installing keeps
    /// the -32601 refusal.
    pub fn set_sampling_handler_concrete(&self, handler: Arc<dyn SamplingHandler>) {
        *self
            .sampling_handler
            .write()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(handler);
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    fn set_sampling_handler(&self, handler: Arc<dyn SamplingHandler>) {
        StdioTransport::set_sampling_handler_concrete(self, handler)
    }

    fn start_close(&self) {
        self.connected.store(false, Ordering::SeqCst);
        self.shutdown.cancel();
        self.kill_process_group();
    }
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
        self.notification_rx.take()
    }

    fn tool_list_changes(&self) -> Option<watch::Receiver<()>> {
        Some(self.notification_rx.tool_list_changes())
    }

    async fn close(&self) -> Result<()> {
        self.start_close();
        self.pending.write().await.clear();

        let mut settlement = self.process_settlement.clone();
        let observe = async {
            loop {
                if let Some(result) = settlement.borrow().clone() {
                    return result.map_err(anyhow::Error::msg);
                }
                settlement.changed().await.map_err(|_| {
                    anyhow!("MCP server monitor ended without a settlement acknowledgement")
                })?;
            }
        };
        let process_result =
            match tokio::time::timeout(PROCESS_SETTLEMENT_TIMEOUT * 2, observe).await {
                Ok(result) => {
                    self.process_task
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    result
                }
                Err(_) => Err(anyhow!(
                    "MCP server monitor did not settle after termination"
                )),
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
    retained: bool,
) -> std::io::Result<()> {
    let mut result = tokio::select! {
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
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "MCP server did not exit after process-group termination",
                    ))
                }
            }
        }
    };
    while retained && result.is_err() {
        tracing::error!(
            "Retained MCP process has not acknowledged reaping; package retention stays active"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        result = child.wait().await;
    }
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

enum StdioInbound {
    ServerRequest(serde_json::Value),
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
    Unknown(String),
}

/// Trim and classify one stdout line the way the reader task does.
///
/// `None` is an empty line. A `tools/list` result is [`StdioInbound::Response`].
fn classify_stdio_line(line: &str) -> Option<StdioInbound> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if value.get("method").is_some() && value.get("id").is_some() {
            return Some(StdioInbound::ServerRequest(value));
        }
    }
    // Notifications have no `id` but still deserialize as JsonRpcResponse
    // (unknown `method`/`params` ignored). Only frames with an id are responses.
    if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
        if response.id.is_some() {
            return Some(StdioInbound::Response(response));
        }
    }
    if let Ok(notification) = serde_json::from_str::<JsonRpcNotification>(trimmed) {
        return Some(StdioInbound::Notification(notification));
    }
    Some(StdioInbound::Unknown(trimmed.to_string()))
}

#[cfg(test)]
#[path = "stdio_retention_tests.rs"]
mod retention_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::process::ProcessGroupGuard;

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
            "(sleep 0.30; : > \"$2\") & : > \"$1\"; wait".to_string(),
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
    fn notification_wire_must_not_be_consumed_as_id_less_response() {
        // Pin the serde ambiguity that previously made reader_task drop
        // notifications: JsonRpcResponse accepts frames with a `method` field
        // because unknown fields are ignored and `id` is Optional.
        let wire = r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"t","progress":1.0}}"#;
        let as_response =
            serde_json::from_str::<JsonRpcResponse>(wire).expect("parses as response");
        assert!(as_response.id.is_none());
        let as_notification =
            serde_json::from_str::<JsonRpcNotification>(wire).expect("parses as notification");
        assert_eq!(as_notification.method, "notifications/progress");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stdio_reader_delivers_server_progress_notifications() {
        let directory = tempfile::tempdir().unwrap();
        let server = directory.path().join("progress_server.py");
        std::fs::write(
            &server,
            r#"
import json, sys, time
for raw in sys.stdin:
    msg = json.loads(raw)
    method = msg.get("method")
    ident = msg.get("id")
    if method == "initialize":
        out = {"jsonrpc":"2.0","id":ident,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"probe","version":"1"}}}
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        out = {"jsonrpc":"2.0","id":ident,"result":{"tools":[{"name":"progress_tool","description":"p","inputSchema":{"type":"object"}}]}}
    elif method == "tools/call":
        print(json.dumps({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"t","progress":1.0,"total":2.0}}), flush=True)
        time.sleep(0.05)
        print(json.dumps({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"t","progress":2.0,"total":2.0}}), flush=True)
        out = {"jsonrpc":"2.0","id":ident,"result":{"content":[{"type":"text","text":"ok"}]}}
    else:
        continue
    print(json.dumps(out), flush=True)
"#,
        )
        .unwrap();

        let transport = Arc::new(
            StdioTransport::spawn_with_timeout(
                "python3",
                &[server.to_string_lossy().into_owned()],
                &HashMap::new(),
                5,
            )
            .await
            .expect("spawn progress MCP server"),
        );
        let mut rx = {
            let transport = Arc::clone(&transport);
            tokio::task::spawn_blocking(move || transport.notifications())
                .await
                .expect("take notification receiver")
        };
        let client = crate::mcp::client::McpClient::new("probe".into(), transport);
        client.initialize().await.expect("initialize");
        client.list_tools().await.expect("list tools");
        let call = client.call_tool("progress_tool", Some(serde_json::json!({})));
        let received = tokio::time::timeout(Duration::from_secs(2), async {
            let mut n = 0;
            while let Some(notification) = rx.recv().await {
                if matches!(notification, McpNotification::Progress { .. }) {
                    n += 1;
                    if n == 2 {
                        break;
                    }
                }
            }
            n
        });
        let (result, received) = tokio::join!(call, received);
        assert!(result.is_ok(), "tool call failed: {result:?}");
        assert_eq!(
            received.expect("timed out waiting for progress notifications"),
            2,
            "stdio reader must deliver notifications/progress frames"
        );
        client.close().await.ok();
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

    #[cfg(unix)]
    #[tokio::test]
    async fn stdout_reader_skips_blank_lines_and_unknown_frames() {
        let args = vec![
            "-c".to_string(),
            r#"
printf '\n'
printf '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info"}}\n'
printf 'not-json-at-all\n'
printf '{"jsonrpc":"2.0","id":1,"result":{"ok":true}}\n'
"#
            .to_string(),
        ];
        let transport = StdioTransport::spawn("/bin/sh", &args, &HashMap::new())
            .await
            .unwrap();
        // Give the reader time to consume frames (including unknown + blank).
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = transport.close().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn request_after_close_fails_to_send() {
        let transport = StdioTransport::spawn("cat", &[], &HashMap::new())
            .await
            .unwrap();
        transport.close().await.unwrap();
        let error = transport
            .request(JsonRpcRequest::new(1, "tools/list", None))
            .await
            .expect_err("closed transport must refuse requests");
        assert!(
            error.to_string().contains("Failed to send request")
                || error.to_string().contains("not connected")
                || error.to_string().contains("Transport"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn notification_after_close_fails_to_send() {
        let transport = StdioTransport::spawn("cat", &[], &HashMap::new())
            .await
            .unwrap();
        transport.close().await.unwrap();
        let error = transport
            .notify(JsonRpcNotification::new("notifications/initialized", None))
            .await
            .expect_err("closed transport must refuse notifications");
        assert!(
            error.to_string().contains("Failed to send notification")
                || error.to_string().contains("not connected")
                || error.to_string().contains("Transport"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn notifications_receiver_is_empty_after_second_take() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let transport = runtime.block_on(async {
            let transport = StdioTransport::spawn("cat", &[], &HashMap::new())
                .await
                .unwrap();
            transport
        });
        let _first = transport.notifications();
        let mut second = transport.notifications();
        assert!(second.try_recv().is_err());
        runtime.block_on(async {
            let _ = transport.close().await;
        });
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdout_reader_breaks_on_oversized_line() {
        // LinesCodec max length is 8 MiB; one longer line forces a decode Err.
        let args = vec![
            "-c".to_string(),
            format!(
                "python3 -c \"import sys; sys.stdout.write('x'*{} + chr(10)); sys.stdout.flush()\"",
                MAX_MCP_STDIO_LINE_BYTES + 16
            ),
        ];
        let transport = StdioTransport::spawn("/bin/sh", &args, &HashMap::new())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = transport.close().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stderr_drain_captures_server_noise() {
        let args = vec![
            "-c".to_string(),
            "printf 'mcp-stderr-noise\\n' 1>&2; cat".to_string(),
        ];
        let transport = StdioTransport::spawn("/bin/sh", &args, &HashMap::new())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        transport.close().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn settle_io_tasks_reports_panic_as_task_failure() {
        let handle = tokio::spawn(async {
            panic!("stdio settle panic");
        });
        // Yield so the task panics before we await it through settle_io_tasks.
        tokio::task::yield_now().await;
        let error = settle_io_tasks(vec![handle])
            .await
            .expect_err("panic must surface");
        assert!(
            error.to_string().contains("MCP stdio task failed"),
            "{error}"
        );
    }

    #[test]
    fn crlf_stdio_tools_list_line_decodes_tool_name() {
        use tokio_util::codec::Decoder;

        let payload = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"lookup","inputSchema":{"type":"object"}}]}}"#;
        let frame = format!("{payload}\r\n");
        let mut codec = LinesCodec::new_with_max_length(MAX_MCP_STDIO_LINE_BYTES);
        let mut buf = bytes::BytesMut::from(frame.as_bytes());
        let line = codec
            .decode(&mut buf)
            .expect("lines codec accepts the frame")
            .expect("one tools/list line");
        assert!(buf.is_empty(), "the CRLF terminator is one line");
        assert!(!line.contains('\r'));

        let inbound = classify_stdio_line(&line).expect("the tools/list line is not empty");
        let StdioInbound::Response(response) = inbound else {
            panic!("tools/list result must parse as a JSON-RPC response");
        };
        let listed: crate::mcp::protocol::ListToolsResult =
            serde_json::from_value(response.result.expect("result")).expect("tools");
        assert_eq!(listed.tools.len(), 1);
        assert_eq!(listed.tools[0].name, "lookup");
        assert!(!listed.tools[0].name.contains('\r'));
    }

    fn disconnected_stdin_transport(stdin_tx: mpsc::Sender<String>) -> StdioTransport {
        StdioTransport {
            process_group: Arc::new(StdMutex::new(ProcessGroupGuard::for_process_id(None))),
            process_task: StdMutex::new(None),
            process_settlement: watch::channel(Some(Ok(()))).1,
            io_tasks: StdMutex::new(Vec::new()),
            stdin_tx,
            sampling_handler: Arc::new(StdRwLock::new(None)),
            pending: Arc::new(RwLock::new(HashMap::new())),
            notification_rx: NotificationReceiver::channel(1).1,
            connected: Arc::new(AtomicBool::new(true)),
            shutdown: CancellationToken::new(),
            request_timeout_secs: 1,
        }
    }

    #[tokio::test]
    async fn request_maps_closed_stdin_channel_to_send_failure() {
        let (stdin_tx, stdin_rx) = mpsc::channel(1);
        drop(stdin_rx);
        let transport = disconnected_stdin_transport(stdin_tx);
        let error = transport
            .request(JsonRpcRequest::new(7, "tools/list", None))
            .await
            .expect_err("closed stdin channel must fail send");
        assert!(
            error.to_string().contains("Failed to send request"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn notify_maps_closed_stdin_channel_to_send_failure() {
        let (stdin_tx, stdin_rx) = mpsc::channel(1);
        drop(stdin_rx);
        let transport = disconnected_stdin_transport(stdin_tx);
        let error = transport
            .notify(JsonRpcNotification::new("notifications/initialized", None))
            .await
            .expect_err("closed stdin channel must fail notify");
        assert!(
            error.to_string().contains("Failed to send notification"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn settle_io_tasks_aborts_hanging_task() {
        let handle = tokio::spawn(async {
            // Longer than PROCESS_SETTLEMENT_TIMEOUT; aborted by settle_io_tasks.
            futures::future::pending::<()>().await;
        });
        let error = settle_io_tasks(vec![handle])
            .await
            .expect_err("hanging task must time out");
        assert!(
            error
                .to_string()
                .contains("MCP stdio task did not settle during close"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdin_writer_breaks_when_child_closes_its_stdin() {
        // Close the child's stdin fd immediately so subsequent writes get EPIPE.
        let args = vec!["-c".to_string(), "exec 0<&-; sleep 1.5".to_string()];
        let transport = StdioTransport::spawn("/bin/sh", &args, &HashMap::new())
            .await
            .unwrap();
        for i in 0..64 {
            let _ = transport
                .notify(JsonRpcNotification::new(
                    "notifications/message",
                    Some(serde_json::json!({ "i": i, "pad": "x".repeat(256) })),
                ))
                .await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = transport.close().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stderr_drain_reads_flushed_server_noise() {
        // Force unbuffered stderr so drain_stderr's Ok(count) arm runs before close.
        let args = vec![
            "-c".to_string(),
            "python3 -c \"import sys,time; sys.stderr.write('mcp-stderr-noise\\\\n'); sys.stderr.flush(); time.sleep(0.4)\"".to_string(),
        ];
        let transport = StdioTransport::spawn("/bin/sh", &args, &HashMap::new())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        transport.close().await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stdio_answers_server_requests_instead_of_dropping_them() {
        let directory = tempfile::tempdir().unwrap();
        let server = directory.path().join("request_server.py");
        // The server sends a ping and a sampling request, then reports on
        // stdout (as a tool result) whether BOTH got replies and what the
        // sampling reply's error code was.
        std::fs::write(
            &server,
            r#"
import json, sys
ping_replied = False
sampling_error_code = None
def send(obj):
    print(json.dumps(obj), flush=True)
for raw in sys.stdin:
    msg = json.loads(raw)
    method = msg.get("method")
    ident = msg.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":ident,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"probe","version":"1"}}})
    elif method == "notifications/initialized":
        send({"jsonrpc":"2.0","id":9001,"method":"ping"})
        send({"jsonrpc":"2.0","id":9002,"method":"sampling/createMessage","params":{}})
    elif method == "ping" and ident == 9001:
        # Impossible echo path; ping replies come to our stdin, handled below.
        continue
    elif ident == 9001 or ident == 9002:
        if ident == 9001 and "result" in msg:
            globals()["ping_replied"] = True
        if ident == 9002 and "error" in msg:
            globals()["sampling_error_code"] = msg["error"].get("code")
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":ident,"result":{"tools":[{"name":"t","description":"p","inputSchema":{"type":"object"}}]}})
    elif method == "tools/call":
        text = json.dumps({"ping_replied": ping_replied, "sampling_error_code": sampling_error_code})
        send({"jsonrpc":"2.0","id":ident,"result":{"content":[{"type":"text","text":text}]}})
"#,
        )
        .unwrap();

        let transport = Arc::new(
            StdioTransport::spawn_with_timeout(
                "python3",
                &[server.to_string_lossy().into_owned()],
                &HashMap::new(),
                5,
            )
            .await
            .expect("spawn request-probe MCP server"),
        );
        let client = crate::mcp::client::McpClient::new("probe".into(), transport);
        client.initialize().await.expect("initialize");
        // Give the server time to receive our replies before the tool call.
        tokio::time::sleep(Duration::from_millis(200)).await;
        client.list_tools().await.expect("list tools");
        let call = client
            .call_tool("t", Some(serde_json::json!({})))
            .await
            .expect("call tool");
        let text = match call.content.first() {
            Some(crate::mcp::protocol::ToolContent::Text { text }) => text.clone(),
            _ => String::new(),
        };
        let observed: serde_json::Value = serde_json::from_str(&text).expect("probe report json");
        assert_eq!(
            observed["ping_replied"], true,
            "a server ping must receive a reply: {observed}"
        );
        assert_eq!(
            observed["sampling_error_code"], -32601,
            "an undeclared-capability request must fail fast with method-not-found: {observed}"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn installed_sampling_handler_declares_and_serves() {
        struct CannedSampling;
        #[async_trait]
        impl SamplingHandler for CannedSampling {
            async fn create_message(&self, params: serde_json::Value) -> Result<serde_json::Value> {
                let asked = params
                    .get("messages")
                    .and_then(|m| m.as_array())
                    .and_then(|m| m.first())
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string();
                Ok(serde_json::json!({
                    "role": "assistant",
                    "content": {"type": "text", "text": format!("sampled:{asked}")},
                    "model": "canned",
                }))
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let server = directory.path().join("sampling_server.py");
        std::fs::write(
            &server,
            r#"
import json, sys
saw_capability = False
sampling_text = None
def send(obj):
    print(json.dumps(obj), flush=True)
for raw in sys.stdin:
    msg = json.loads(raw)
    method = msg.get("method")
    ident = msg.get("id")
    if method == "initialize":
        caps = msg.get("params", {}).get("capabilities", {})
        globals()["saw_capability"] = "sampling" in caps
        send({"jsonrpc":"2.0","id":ident,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"probe","version":"1"}}})
    elif method == "notifications/initialized":
        send({"jsonrpc":"2.0","id":7001,"method":"sampling/createMessage","params":{"messages":[{"role":"user","content":{"type":"text","text":"hello bridge"}}],"maxTokens":64}})
    elif ident == 7001:
        globals()["sampling_text"] = msg.get("result", {}).get("content", {}).get("text")
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":ident,"result":{"tools":[{"name":"t","description":"p","inputSchema":{"type":"object"}}]}})
    elif method == "tools/call":
        text = json.dumps({"saw_capability": saw_capability, "sampling_text": sampling_text})
        send({"jsonrpc":"2.0","id":ident,"result":{"content":[{"type":"text","text":text}]}})
"#,
        )
        .unwrap();

        let transport = Arc::new(
            StdioTransport::spawn_with_timeout(
                "python3",
                &[server.to_string_lossy().into_owned()],
                &HashMap::new(),
                5,
            )
            .await
            .expect("spawn sampling probe"),
        );
        let client = crate::mcp::client::McpClient::new("probe".into(), transport);
        // The client-level install is the declaration point: it routes to
        // the transport and turns on the handshake capability.
        client.install_sampling_handler(Arc::new(CannedSampling));
        client.initialize().await.expect("initialize");
        tokio::time::sleep(Duration::from_millis(200)).await;
        client.list_tools().await.expect("list tools");
        let call = client.call_tool("t", None).await.expect("call tool");
        let text = match call.content.first() {
            Some(crate::mcp::protocol::ToolContent::Text { text }) => text.clone(),
            _ => String::new(),
        };
        let observed: serde_json::Value = serde_json::from_str(&text).expect("probe report json");
        assert_eq!(
            observed["saw_capability"], true,
            "installing the handler declares sampling in the handshake: {observed}"
        );
        assert_eq!(
            observed["sampling_text"], "sampled:hello bridge",
            "the server-initiated sampling round-trips through the handler: {observed}"
        );
    }
}
