use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::Duration,
};

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
#[cfg(test)]
use tokio::io;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot, Mutex},
    task::JoinHandle,
    time::{self, Instant},
};
use tokio_util::{
    codec::{FramedRead, FramedWrite},
    sync::CancellationToken,
};

use super::{
    codec::{LspCodec, LspCodecError},
    message::{
        IncomingMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponsePayload, RequestId,
    },
    router::ServerRequestRouter,
};

const DEFAULT_WRITER_CAPACITY: usize = 64;
const DEFAULT_NOTIFICATION_CAPACITY: usize = 256;

type PendingResult = Result<Value, LspClientError>;
type PendingMap = HashMap<RequestId, oneshot::Sender<PendingResult>>;

/// Bounded channel sizes used by a protocol client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LspClientConfig {
    writer_capacity: usize,
    notification_capacity: usize,
}

impl LspClientConfig {
    #[cfg(test)]
    pub(crate) fn new(writer_capacity: usize, notification_capacity: usize) -> Self {
        Self {
            writer_capacity: writer_capacity.max(1),
            notification_capacity: notification_capacity.max(1),
        }
    }
}

impl Default for LspClientConfig {
    fn default() -> Self {
        Self {
            writer_capacity: DEFAULT_WRITER_CAPACITY,
            notification_capacity: DEFAULT_NOTIFICATION_CAPACITY,
        }
    }
}

/// Failure returned by the protocol actor.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub(crate) enum LspClientError {
    #[error("language server returned error {code}: {message}")]
    RemoteError {
        code: i64,
        message: String,
        data: Option<Value>,
    },

    #[error("language server request was cancelled")]
    Cancelled,

    #[error("language server request '{method}' timed out after {duration:?}")]
    Timeout { method: String, duration: Duration },

    #[error("language server connection closed: {message}")]
    Closed { message: String },

    #[error("language server transport failed: {message}")]
    Transport { message: String },

    #[error("language server protocol failed: {message}")]
    Protocol { message: String },
}

#[derive(Debug)]
struct SharedState {
    writer: mpsc::Sender<Value>,
    pending: Mutex<PendingMap>,
    next_request_id: AtomicU64,
    closed: AtomicBool,
    close_reason: StdMutex<Option<LspClientError>>,
    shutdown: CancellationToken,
}

#[derive(Debug)]
struct ClientInner {
    shared: Arc<SharedState>,
    tasks: Mutex<Option<Vec<JoinHandle<()>>>>,
    notifications: StdMutex<Option<mpsc::Receiver<JsonRpcNotification>>>,
}

/// Cloneable handle to one bidirectional language-server connection.
#[derive(Debug, Clone)]
pub(crate) struct LspClient {
    inner: Arc<ClientInner>,
}

impl LspClient {
    /// Start a client over one bidirectional asynchronous stream.
    #[cfg(test)]
    pub(crate) fn start<T>(io: T, router: ServerRequestRouter) -> Self
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::start_with_config(io, router, LspClientConfig::default())
    }

    #[cfg(test)]
    pub(crate) fn start_with_config<T>(
        io: T,
        router: ServerRequestRouter,
        config: LspClientConfig,
    ) -> Self
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, writer) = io::split(io);
        Self::start_split_with_config(reader, writer, router, config)
    }

    /// Start a client when the process exposes separate stdout and stdin
    /// handles instead of one bidirectional stream.
    pub(crate) fn start_split<R, W>(reader: R, writer: W, router: ServerRequestRouter) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        Self::start_split_with_config(reader, writer, router, LspClientConfig::default())
    }

    pub(crate) fn start_split_with_config<R, W>(
        reader: R,
        writer: W,
        router: ServerRequestRouter,
        config: LspClientConfig,
    ) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (writer_tx, writer_rx) = mpsc::channel(config.writer_capacity);
        let (notification_tx, notification_rx) = mpsc::channel(config.notification_capacity);
        let shared = Arc::new(SharedState {
            writer: writer_tx,
            pending: Mutex::new(HashMap::new()),
            next_request_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            close_reason: StdMutex::new(None),
            shutdown: CancellationToken::new(),
        });

        let writer_task = tokio::spawn(run_writer(writer, writer_rx, shared.clone()));
        let reader_task = tokio::spawn(run_reader(reader, router, notification_tx, shared.clone()));
        Self {
            inner: Arc::new(ClientInner {
                shared,
                tasks: Mutex::new(Some(vec![writer_task, reader_task])),
                notifications: StdMutex::new(Some(notification_rx)),
            }),
        }
    }

    /// Send a request and wait for its result for no longer than `timeout`.
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Result<Value, LspClientError> {
        if cancellation.is_cancelled() {
            return Err(LspClientError::Cancelled);
        }
        self.ensure_open()?;

        let id = RequestId::from(
            self.inner
                .shared
                .next_request_id
                .fetch_add(1, Ordering::Relaxed),
        );
        let request = JsonRpcRequest::new(id.clone(), method, params).to_value();
        let (response_tx, response_rx) = oneshot::channel();
        {
            let mut pending = self.inner.shared.pending.lock().await;
            if self.inner.shared.closed.load(Ordering::Acquire) {
                return Err(self.close_error());
            }
            pending.insert(id.clone(), response_tx);
        }

        let deadline = Instant::now() + timeout;
        let enqueue = self.inner.shared.writer.send(request);
        tokio::pin!(enqueue);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                self.abort_request(&id).await;
                return Err(LspClientError::Cancelled);
            }
            _ = time::sleep_until(deadline) => {
                self.abort_request(&id).await;
                return Err(LspClientError::Timeout {
                    method: method.to_owned(),
                    duration: timeout,
                });
            }
            result = &mut enqueue => {
                if result.is_err() {
                    self.inner.shared.pending.lock().await.remove(&id);
                    return Err(self.close_error());
                }
            }
        }

        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                self.abort_request(&id).await;
                Err(LspClientError::Cancelled)
            }
            _ = time::sleep_until(deadline) => {
                self.abort_request(&id).await;
                Err(LspClientError::Timeout {
                    method: method.to_owned(),
                    duration: timeout,
                })
            }
            response = response_rx => match response {
                Ok(result) => result,
                Err(_) => Err(self.close_error()),
            }
        }
    }

    /// Send a notification to the server, applying bounded backpressure.
    pub(crate) async fn notify(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), LspClientError> {
        self.ensure_open()?;
        let notification = JsonRpcNotification::new(method, params).to_value();
        self.inner
            .shared
            .writer
            .send(notification)
            .await
            .map_err(|_| self.close_error())
    }

    /// Take the typed server-notification stream. Only one consumer may own it.
    pub(crate) fn take_notifications(&self) -> Option<mpsc::Receiver<JsonRpcNotification>> {
        self.inner
            .notifications
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    pub(crate) fn shutdown_token(&self) -> CancellationToken {
        self.inner.shared.shutdown.clone()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.inner.shared.closed.load(Ordering::Acquire)
    }

    /// Stop the reader and writer tasks and fail any outstanding requests.
    pub(crate) async fn close(&self) {
        terminate(
            &self.inner.shared,
            LspClientError::Closed {
                message: "client shutdown requested".to_owned(),
            },
        )
        .await;

        let tasks = self.inner.tasks.lock().await.take().unwrap_or_default();
        for task in tasks {
            let _ = task.await;
        }
    }

    async fn abort_request(&self, id: &RequestId) {
        let removed = self.inner.shared.pending.lock().await.remove(id).is_some();
        if !removed || self.inner.shared.closed.load(Ordering::Acquire) {
            return;
        }

        let cancellation =
            JsonRpcNotification::new("$/cancelRequest", Some(json!({"id": id.to_value()})))
                .to_value();
        // Cancellation must remain prompt even when the transport is already
        // backpressured. The local pending entry has been removed, so a late
        // response remains harmless if this best-effort notification cannot
        // enter a full queue.
        let _ = self.inner.shared.writer.try_send(cancellation);
    }

    fn ensure_open(&self) -> Result<(), LspClientError> {
        if self.inner.shared.closed.load(Ordering::Acquire) {
            Err(self.close_error())
        } else {
            Ok(())
        }
    }

    fn close_error(&self) -> LspClientError {
        self.inner
            .shared
            .close_reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| LspClientError::Closed {
                message: "connection is no longer available".to_owned(),
            })
    }
}

async fn run_writer<W>(writer: W, mut messages: mpsc::Receiver<Value>, shared: Arc<SharedState>)
where
    W: AsyncWrite + Unpin,
{
    let mut framed = FramedWrite::new(writer, LspCodec::default());
    loop {
        let message = tokio::select! {
            _ = shared.shutdown.cancelled() => {
                terminate_if_needed(&shared).await;
                break;
            }
            message = messages.recv() => match message {
                Some(message) => message,
                None => {
                    terminate(
                        &shared,
                        LspClientError::Closed {
                            message: "protocol writer channel closed".to_owned(),
                        },
                    ).await;
                    break;
                }
            }
        };

        let send = framed.send(message);
        tokio::pin!(send);
        tokio::select! {
            _ = shared.shutdown.cancelled() => {
                terminate_if_needed(&shared).await;
                break;
            }
            result = &mut send => {
                if let Err(error) = result {
                    terminate(&shared, codec_client_error(error)).await;
                    break;
                }
            }
        }
    }
}

async fn run_reader<R>(
    reader: R,
    router: ServerRequestRouter,
    notifications: mpsc::Sender<JsonRpcNotification>,
    shared: Arc<SharedState>,
) where
    R: AsyncRead + Unpin,
{
    let mut framed = FramedRead::new(reader, LspCodec::default());
    loop {
        let frame = tokio::select! {
            _ = shared.shutdown.cancelled() => {
                terminate_if_needed(&shared).await;
                break;
            }
            frame = framed.next() => frame,
        };

        let value = match frame {
            Some(Ok(value)) => value,
            Some(Err(error)) => {
                terminate(&shared, codec_client_error(error)).await;
                break;
            }
            None => {
                terminate(
                    &shared,
                    LspClientError::Closed {
                        message: "language server reached end of stream".to_owned(),
                    },
                )
                .await;
                break;
            }
        };

        let message = match IncomingMessage::try_from(value) {
            Ok(message) => message,
            Err(error) => {
                terminate(
                    &shared,
                    LspClientError::Protocol {
                        message: error.to_string(),
                    },
                )
                .await;
                break;
            }
        };

        match message {
            IncomingMessage::Response(response) => {
                let sender = shared.pending.lock().await.remove(&response.id);
                let Some(sender) = sender else {
                    // Unknown and late responses are expected after cancellation.
                    continue;
                };
                let result = match response.payload {
                    JsonRpcResponsePayload::Result(value) => Ok(value),
                    JsonRpcResponsePayload::Error(error) => Err(LspClientError::RemoteError {
                        code: error.code,
                        message: error.message,
                        data: error.data,
                    }),
                };
                let _ = sender.send(result);
            }
            IncomingMessage::Request(request) => {
                let response = router.route(&request).to_value();
                let enqueue = shared.writer.send(response);
                tokio::pin!(enqueue);
                let sent = tokio::select! {
                    _ = shared.shutdown.cancelled() => false,
                    result = &mut enqueue => result.is_ok(),
                };
                if !sent {
                    terminate(
                        &shared,
                        LspClientError::Transport {
                            message: "protocol writer is unavailable".to_owned(),
                        },
                    )
                    .await;
                    break;
                }
            }
            IncomingMessage::Notification(notification) => {
                // Apply bounded backpressure instead of dropping diagnostics:
                // losing a later empty diagnostics notification would retain
                // stale errors in callers.
                let deliver = notifications.send(notification);
                tokio::pin!(deliver);
                tokio::select! {
                    _ = shared.shutdown.cancelled() => {
                        terminate_if_needed(&shared).await;
                        break;
                    }
                    // A caller may intentionally decline the notification
                    // stream. Request/response handling remains usable.
                    _ = &mut deliver => {}
                }
            }
        }
    }
}

async fn terminate_if_needed(shared: &Arc<SharedState>) {
    if !shared.closed.load(Ordering::Acquire) {
        terminate(
            shared,
            LspClientError::Closed {
                message: "protocol shutdown requested".to_owned(),
            },
        )
        .await;
    }
}

async fn terminate(shared: &Arc<SharedState>, error: LspClientError) {
    if shared
        .closed
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    *shared
        .close_reason
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.clone());
    shared.shutdown.cancel();
    let pending = {
        let mut pending = shared.pending.lock().await;
        std::mem::take(&mut *pending)
    };
    for sender in pending.into_values() {
        let _ = sender.send(Err(error.clone()));
    }
}

fn codec_client_error(error: LspCodecError) -> LspClientError {
    match error {
        LspCodecError::Io(error) => LspClientError::Transport {
            message: error.to_string(),
        },
        error => LspClientError::Protocol {
            message: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio::io::DuplexStream;
    use tokio_util::codec::Framed;

    use super::*;
    use crate::code_intelligence::lsp::{
        message::{IncomingMessage, JsonRpcResponse},
        router::ServerRequestRouterConfig,
    };

    fn client_and_server() -> (LspClient, Framed<DuplexStream, LspCodec>) {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let client = LspClient::start(
            client_io,
            ServerRequestRouter::new(ServerRequestRouterConfig::default()),
        );
        let server = Framed::new(server_io, LspCodec::default());
        (client, server)
    }

    async fn next_server_message(server: &mut Framed<DuplexStream, LspCodec>) -> IncomingMessage {
        let value = time::timeout(Duration::from_secs(1), server.next())
            .await
            .expect("server message timed out")
            .expect("client stream closed")
            .expect("client frame failed");
        IncomingMessage::try_from(value).expect("client sent invalid message")
    }

    #[tokio::test]
    async fn completes_client_request_and_delivers_typed_notification() {
        let (client, mut server) = client_and_server();
        let mut notifications = client.take_notifications().unwrap();
        let request_task = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .request(
                        "workspace/symbol",
                        Some(json!({"query": "Client"})),
                        CancellationToken::new(),
                        Duration::from_secs(1),
                    )
                    .await
            }
        });

        let IncomingMessage::Request(request) = next_server_message(&mut server).await else {
            panic!("expected request");
        };
        assert_eq!(request.method, "workspace/symbol");
        server
            .send(JsonRpcResponse::success(request.id, json!([{"name": "Client"}])).to_value())
            .await
            .unwrap();
        assert_eq!(request_task.await.unwrap().unwrap()[0]["name"], "Client");

        server
            .send(json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {"diagnostics": []}
            }))
            .await
            .unwrap();
        let notification = time::timeout(Duration::from_secs(1), notifications.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(notification.method, "textDocument/publishDiagnostics");
        assert_eq!(notification.params.unwrap()["diagnostics"], json!([]));
        client.close().await;
    }

    #[tokio::test]
    async fn routes_server_request_with_string_id_and_unknown_method() {
        let (client, mut server) = client_and_server();
        server
            .send(json!({
                "jsonrpc": "2.0",
                "id": "server-42",
                "method": "unknown/method",
                "params": {}
            }))
            .await
            .unwrap();

        let IncomingMessage::Response(response) = next_server_message(&mut server).await else {
            panic!("expected response");
        };
        assert_eq!(response.id, RequestId::from("server-42"));
        let JsonRpcResponsePayload::Error(error) = response.payload else {
            panic!("expected method-not-found error");
        };
        assert_eq!(error.code, -32601);
        client.close().await;
    }

    #[tokio::test]
    async fn cancellation_removes_pending_and_notifies_server() {
        let (client, mut server) = client_and_server();
        let cancellation = CancellationToken::new();
        let request_task = tokio::spawn({
            let client = client.clone();
            let cancellation = cancellation.clone();
            async move {
                client
                    .request(
                        "textDocument/references",
                        Some(json!({})),
                        cancellation,
                        Duration::from_secs(1),
                    )
                    .await
            }
        });

        let IncomingMessage::Request(request) = next_server_message(&mut server).await else {
            panic!("expected request");
        };
        cancellation.cancel();
        let IncomingMessage::Notification(cancel) = next_server_message(&mut server).await else {
            panic!("expected cancellation notification");
        };
        assert_eq!(cancel.method, "$/cancelRequest");
        assert_eq!(cancel.params.unwrap()["id"], request.id.to_value());
        assert_eq!(request_task.await.unwrap(), Err(LspClientError::Cancelled));
        client.close().await;
    }

    #[tokio::test]
    async fn timeout_removes_pending_and_notifies_server() {
        let (client, mut server) = client_and_server();
        let request_task = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .request(
                        "textDocument/definition",
                        Some(json!({})),
                        CancellationToken::new(),
                        Duration::from_millis(20),
                    )
                    .await
            }
        });

        let IncomingMessage::Request(request) = next_server_message(&mut server).await else {
            panic!("expected request");
        };
        let IncomingMessage::Notification(cancel) = next_server_message(&mut server).await else {
            panic!("expected cancellation notification");
        };
        assert_eq!(cancel.params.unwrap()["id"], request.id.to_value());
        assert!(matches!(
            request_task.await.unwrap(),
            Err(LspClientError::Timeout { .. })
        ));
        client.close().await;
    }

    #[tokio::test]
    async fn cancellation_returns_promptly_when_writer_queue_is_saturated() {
        let (client_io, _server_io) = tokio::io::duplex(64);
        let client = LspClient::start_with_config(
            client_io,
            ServerRequestRouter::new(ServerRequestRouterConfig::default()),
            LspClientConfig::new(1, 1),
        );
        let large_params = Some(json!({"payload": "x".repeat(8 * 1024)}));

        // The first frame blocks in the tiny duplex buffer. The second fills
        // the sole writer queue slot.
        client
            .notify("test/first", large_params.clone())
            .await
            .unwrap();
        client.notify("test/second", large_params).await.unwrap();
        time::timeout(Duration::from_secs(1), async {
            while client.inner.shared.writer.capacity() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let cancellation = CancellationToken::new();
        let pending = tokio::spawn({
            let client = client.clone();
            let cancellation = cancellation.clone();
            async move {
                client
                    .request(
                        "workspace/symbol",
                        Some(json!({"query": "blocked"})),
                        cancellation,
                        Duration::from_secs(5),
                    )
                    .await
            }
        });
        time::timeout(Duration::from_secs(1), async {
            while client.inner.shared.pending.lock().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        cancellation.cancel();
        assert_eq!(
            time::timeout(Duration::from_millis(100), pending)
                .await
                .expect("cancellation was blocked by the full writer queue")
                .unwrap(),
            Err(LspClientError::Cancelled)
        );
        client.close().await;
    }

    #[tokio::test]
    async fn notification_backpressure_preserves_every_notification() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let client = LspClient::start_with_config(
            client_io,
            ServerRequestRouter::new(ServerRequestRouterConfig::default()),
            LspClientConfig::new(8, 1),
        );
        let mut server = Framed::new(server_io, LspCodec::default());

        server
            .send(json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {"version": 1, "diagnostics": [{"message": "old"}]}
            }))
            .await
            .unwrap();
        server
            .send(json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {"version": 2, "diagnostics": []}
            }))
            .await
            .unwrap();

        // Delay taking the single-slot receiver so the second notification
        // must wait rather than being dropped.
        time::sleep(Duration::from_millis(20)).await;
        let mut notifications = client.take_notifications().unwrap();
        let first = time::timeout(Duration::from_secs(1), notifications.recv())
            .await
            .unwrap()
            .unwrap();
        let second = time::timeout(Duration::from_secs(1), notifications.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.params.unwrap()["version"], 1);
        assert_eq!(second.params.unwrap()["version"], 2);
        client.close().await;
    }

    #[tokio::test]
    async fn eof_settles_all_pending_requests_once() {
        let (client, mut server) = client_and_server();
        let request_task = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .request(
                        "workspace/symbol",
                        Some(json!({"query": "x"})),
                        CancellationToken::new(),
                        Duration::from_secs(5),
                    )
                    .await
            }
        });
        let _ = next_server_message(&mut server).await;
        drop(server);

        let result = time::timeout(Duration::from_secs(1), request_task)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            result,
            Err(LspClientError::Closed { .. } | LspClientError::Transport { .. })
        ));
        assert!(client.is_closed());
        client.close().await;
    }

    #[tokio::test]
    async fn safely_ignores_late_response_after_timeout() {
        let (client, mut server) = client_and_server();
        let timed_out = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .request(
                        "textDocument/definition",
                        Some(json!({})),
                        CancellationToken::new(),
                        Duration::from_millis(20),
                    )
                    .await
            }
        });
        let IncomingMessage::Request(first) = next_server_message(&mut server).await else {
            panic!("expected request");
        };
        let _ = next_server_message(&mut server).await;
        assert!(matches!(
            timed_out.await.unwrap(),
            Err(LspClientError::Timeout { .. })
        ));
        server
            .send(JsonRpcResponse::success(first.id, json!({"late": true})).to_value())
            .await
            .unwrap();

        let next = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .request(
                        "workspace/symbol",
                        Some(json!({"query": "next"})),
                        CancellationToken::new(),
                        Duration::from_secs(1),
                    )
                    .await
            }
        });
        let IncomingMessage::Request(second) = next_server_message(&mut server).await else {
            panic!("expected second request");
        };
        server
            .send(JsonRpcResponse::success(second.id, json!({"ok": true})).to_value())
            .await
            .unwrap();
        assert_eq!(next.await.unwrap().unwrap(), json!({"ok": true}));
        client.close().await;
    }

    #[tokio::test]
    async fn external_shutdown_token_stops_tasks_and_settles_pending() {
        let (client, mut server) = client_and_server();
        let pending = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .request(
                        "workspace/symbol",
                        Some(json!({"query": "pending"})),
                        CancellationToken::new(),
                        Duration::from_secs(5),
                    )
                    .await
            }
        });
        let _ = next_server_message(&mut server).await;
        client.shutdown_token().cancel();
        assert!(matches!(
            time::timeout(Duration::from_secs(1), pending)
                .await
                .unwrap()
                .unwrap(),
            Err(LspClientError::Closed { .. })
        ));
        client.close().await;
    }
}
