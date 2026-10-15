#![cfg(test)]

use a3s_code_core::mcp::client::McpClient;
use a3s_code_core::mcp::protocol::McpNotification;
use a3s_code_core::mcp::transport::http_sse::HttpSseTransport;
use a3s_code_core::mcp::transport::streamable_http::StreamableHttpTransport;
use a3s_code_core::mcp::transport::McpTransport;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

const SERVER: &str = r#"import http.server, json, pathlib, queue, sys, threading, time
root = pathlib.Path(sys.argv[1])
events = queue.Queue(16)
changed = threading.Event()
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def body(self, value, code=200):
        raw = json.dumps(value).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)
        self.wfile.flush()
    def do_GET(self):
        if self.path.endswith("/health"):
            self.body({"ok":True})
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.wfile.flush()
        (root / "sse-ready").write_text("actual GET stream")
        while True:
            value = events.get()
            self.wfile.write(("data: " + json.dumps(value) + "\n\n").encode())
            self.wfile.flush()
    def do_POST(self):
        size = int(self.headers.get("Content-Length", "0"))
        if size > 16384:
            self.body({}, 413)
            return
        msg = json.loads(self.rfile.read(size))
        if "id" not in msg:
            self.body({}, 202)
            return
        method = msg["method"]
        if method == "initialize":
            result = {"protocolVersion":msg["params"]["protocolVersion"], "capabilities":{"tools":{"listChanged":True}}, "serverInfo":{"name":"http-progress-cache", "version":"1"}}
        elif method == "tools/list":
            names = ["lookup", "added"] if changed.is_set() else ["lookup"]
            result = {"tools":[{"name":name, "description":name, "inputSchema":{"type":"object", "properties":{}}} for name in names]}
        elif method == "tools/call":
            changed.set()
            events.put({"jsonrpc":"2.0", "method":"notifications/progress", "params":{"progressToken":"http-call", "progress":1.0, "total":2.0}})
            events.put({"jsonrpc":"2.0", "method":"notifications/tools/list_changed"})
            events.put({"jsonrpc":"2.0", "method":"notifications/progress", "params":{"progressToken":"http-call", "progress":2.0, "total":2.0}})
            end = time.monotonic() + 15
            while not (root / "release").exists() and time.monotonic() < end:
                time.sleep(0.005)
            result = {"content":[{"type":"text", "text":"http-progress-cache-913"}], "isError":False}
        else:
            result = {}
        self.body({"jsonrpc":"2.0", "id":msg["id"], "result":result})
server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_port, flush=True)
server.serve_forever()
"#;

struct HttpPeer {
    _directory: tempfile::TempDir,
    root: PathBuf,
    url: String,
    child: tokio::process::Child,
}

impl HttpPeer {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_owned();
        let script = root.join("server.py");
        std::fs::write(&script, SERVER).unwrap();
        let mut child =
            tokio::process::Command::new(if cfg!(windows) { "python" } else { "python3" })
                .arg("-u")
                .arg(script)
                .arg(&root)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::from(
                    std::fs::File::create(root.join("server.stderr")).unwrap(),
                ))
                .kill_on_drop(true)
                .spawn()
                .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let port: u16 = tokio::time::timeout(Duration::from_secs(3), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .parse()
            .unwrap();
        Self {
            _directory: directory,
            root,
            url: format!("http://127.0.0.1:{port}/mcp"),
            child,
        }
    }

    async fn transport(&self, streamable: bool) -> Arc<dyn McpTransport> {
        if streamable {
            Arc::new(
                StreamableHttpTransport::connect(&self.url, HashMap::new())
                    .await
                    .unwrap(),
            )
        } else {
            Arc::new(
                HttpSseTransport::connect(&self.url, HashMap::new())
                    .await
                    .unwrap(),
            )
        }
    }

    async fn close(&mut self) {
        self.child.start_kill().unwrap();
        self.child.wait().await.unwrap();
    }
}

async fn delivery(streamable: bool) {
    let mut peer = HttpPeer::start().await;
    let client = Arc::new(McpClient::new(
        "http-progress".into(),
        peer.transport(streamable).await,
    ));
    client.initialize().await.unwrap();
    assert_eq!(client.list_tools().await.unwrap().len(), 1);
    let refresher = client.spawn_list_changed_refresher();
    let duplicate = client.spawn_list_changed_refresher();
    tokio::time::timeout(Duration::from_secs(1), duplicate)
        .await
        .unwrap()
        .unwrap();
    // This must be safe on the async runtime thread for both HTTP transports.
    let mut notifications = client.notifications();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !peer.root.join("sse-ready").is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let caller = Arc::clone(&client);
    let call = tokio::spawn(async move { caller.call_tool("lookup", None).await });
    let progress = tokio::time::timeout(Duration::from_secs(3), async {
        let mut values = Vec::new();
        while let Some(notification) = notifications.recv().await {
            if let McpNotification::Progress {
                progress_token,
                progress,
                ..
            } = notification
            {
                assert_eq!(progress_token, "http-call");
                values.push(progress);
                if values.len() == 2 {
                    break;
                }
            }
        }
        values
    })
    .await
    .ok();
    let refreshed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if client
                .get_cached_tools()
                .await
                .iter()
                .any(|tool| tool.name == "added")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    let outstanding = !call.is_finished();
    std::fs::write(peer.root.join("release"), b"release original tools/call").unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), call).await;
    client.close().await.unwrap();
    let exited = tokio::time::timeout(Duration::from_secs(3), refresher).await;
    peer.close().await;
    let stderr = std::fs::read_to_string(peer.root.join("server.stderr")).unwrap();
    assert_eq!(progress, Some(vec![1.0, 2.0]), "{stderr}");
    assert!(
        refreshed && outstanding,
        "cache/progress must arrive before tools/call returns: {stderr}"
    );
    let result = result.unwrap().unwrap().unwrap();
    assert!(
        matches!(&result.content[..], [a3s_code_core::mcp::protocol::ToolContent::Text { text }] if text == "http-progress-cache-913")
    );
    exited.unwrap().unwrap();
}

#[tokio::test]
async fn http_sse_progress_and_cache_refresh_share_the_original_call_without_competing() {
    delivery(false).await;
}

#[tokio::test]
async fn streamable_http_progress_and_cache_refresh_share_the_original_call_without_competing() {
    delivery(true).await;
}

#[tokio::test]
async fn dropping_the_exact_client_stops_idle_refresh_while_a_host_retains_the_transport() {
    let mut peer = HttpPeer::start().await;
    let transport = peer.transport(true).await;
    let client = Arc::new(McpClient::new("drop-client".into(), Arc::clone(&transport)));
    client.initialize().await.unwrap();
    let refresher = client.spawn_list_changed_refresher();
    let weak = Arc::downgrade(&client);
    drop(client);
    let exited = tokio::time::timeout(Duration::from_secs(3), refresher).await;
    transport.close().await.unwrap();
    peer.close().await;
    assert!(
        weak.upgrade().is_none(),
        "idle cache worker must not retain the original client"
    );
    exited.unwrap().unwrap();
}
