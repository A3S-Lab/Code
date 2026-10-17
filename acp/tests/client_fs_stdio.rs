//! Client file-system reads through the shipped `a3s-code-acp` binary
//! (ACP-CAPS1).
//!
//! A client that declares `fs.readTextFile` serves the model's `read` tool
//! from its unsaved buffer. A client that does not declare it gets no
//! `fs/read_text_file` request, and the model sees the saved file.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol as acp;
use async_trait::async_trait;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

const MODEL_ID: &str = "mock/gpt";
const SAVED: &str = "saved on disk\n";
const UNSAVED: &str = "UNSAVED BUFFER LINE\n";

/// Tool results the model received, in order.
type Seen = Arc<Mutex<Vec<String>>>;

fn spawn_mock(seen: Seen) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let seen = Arc::clone(&seen);
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
                if let Some(body) = read_http_body(&mut stream) {
                    let _ = write_http(&mut stream, &decide(&body, &seen));
                }
            });
        }
    });
    addr
}

fn read_http_body(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < header_end + length {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let end = (header_end + length).min(buf.len());
    Some(String::from_utf8_lossy(&buf[header_end..end]).into_owned())
}

fn write_http(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    let content_type = if body.starts_with("data:") {
        "text/event-stream"
    } else {
        "application/json"
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn reply(delta: serde_json::Value, finish: &str, streaming: bool) -> String {
    if !streaming {
        let mut message = delta;
        message["role"] = "assistant".into();
        return serde_json::json!({
            "id": "cmpl",
            "object": "chat.completion",
            "choices": [{ "index": 0, "message": message, "finish_reason": finish }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })
        .to_string();
    }
    let chunk = serde_json::json!({ "choices": [{ "delta": delta }] });
    let done = serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": finish }] });
    format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

fn message_text(message: &serde_json::Value) -> String {
    match &message["content"] {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Reads `notes.txt` once, records the tool result, then answers with text.
fn decide(raw: &str, seen: &Seen) -> String {
    let parsed: serde_json::Value = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
    let streaming = parsed["stream"].as_bool().unwrap_or(false);
    let has_read = parsed["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|tool| tool["function"]["name"] == "read");
    let messages = parsed["messages"].as_array().cloned().unwrap_or_default();
    if let Some(last) = messages.last().filter(|m| m["role"] == "tool") {
        seen.lock().expect("seen").push(message_text(last));
        return reply(serde_json::json!({ "content": "done" }), "stop", streaming);
    }
    if !has_read {
        return reply(serde_json::json!({ "content": "done" }), "stop", streaming);
    }
    let args = serde_json::json!({ "file_path": "notes.txt" }).to_string();
    let call = serde_json::json!({
        "content": null,
        "tool_calls": [{
            "index": 0,
            "id": "call_read",
            "type": "function",
            "function": { "name": "read", "arguments": args }
        }]
    });
    reply(call, "tool_calls", streaming)
}

struct TestClient {
    reads: Rc<RefCell<Vec<PathBuf>>>,
}

#[async_trait(?Send)]
impl acp::Client for TestClient {
    async fn request_permission(
        &self,
        _args: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, acp::Error> {
        Ok(acp::RequestPermissionResponse::new(
            acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                acp::PermissionOptionId::new("allow-once"),
            )),
        ))
    }

    async fn read_text_file(
        &self,
        args: acp::ReadTextFileRequest,
    ) -> Result<acp::ReadTextFileResponse, acp::Error> {
        self.reads.borrow_mut().push(args.path.clone());
        if args.path.ends_with("notes.txt") {
            Ok(acp::ReadTextFileResponse::new(UNSAVED))
        } else {
            Err(acp::Error::resource_not_found(None))
        }
    }

    async fn session_notification(
        &self,
        _args: acp::SessionNotification,
    ) -> Result<(), acp::Error> {
        Ok(())
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    home: PathBuf,
    state: PathBuf,
    tmp: PathBuf,
    config: PathBuf,
}

fn isolate(root: &Path, label: &str) -> PathBuf {
    let path = root.join(label);
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

impl Fixture {
    fn new(addr: SocketAddr) -> Self {
        let root = tempfile::tempdir().expect("root");
        let workspace = isolate(root.path(), "repo");
        std::fs::write(workspace.join("notes.txt"), SAVED).expect("notes");
        let outside = isolate(root.path(), "outside");
        let config = outside.join("config.acl");
        std::fs::write(
            &config,
            format!(
                r#"
providers "mock" {{
  base_url = "http://{addr}/v1"
  api_key = "test"
  models "gpt" {{
  }}
}}
default_model = "{MODEL_ID}"
"#
            ),
        )
        .expect("acl");
        Self {
            home: isolate(root.path(), "home"),
            state: isolate(root.path(), "state"),
            tmp: isolate(root.path(), "tmp"),
            _root: root,
            workspace,
            config,
        }
    }
}

/// Runs one `read notes.txt` turn and returns the client's
/// `fs/read_text_file` paths.
async fn read_turn(fixture: &Fixture, client_reads: bool) -> Vec<PathBuf> {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_a3s-code-acp"));
    let mut child = tokio::process::Command::new(bin)
        .arg("--cwd")
        .arg(&fixture.workspace)
        .env("A3S_CONFIG", &fixture.config)
        .env("A3S_STATE_HOME", &fixture.state)
        .env("A3S_DISABLE_CC_SWITCH", "1")
        .env("A3S_DISABLE_GROK_ACCOUNT", "1")
        .env("A3S_DISABLE_CURSOR_ACCOUNT", "1")
        .env("XAI_API_KEY", "sk-dummy-not-a-model")
        .env("HOME", &fixture.home)
        .env("GROK_HOME", fixture.home.join("grok"))
        .env("TMPDIR", &fixture.tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn a3s-code-acp");
    let stdout = child.stdout.take().expect("stdout").compat();
    let stdin = child.stdin.take().expect("stdin").compat_write();
    let reads = Rc::new(RefCell::new(Vec::new()));
    let client = TestClient {
        reads: Rc::clone(&reads),
    };
    let (conn, io) = acp::ClientSideConnection::new(client, stdin, stdout, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(async move {
        let _ = io.await;
    });
    let capabilities = acp::ClientCapabilities::new()
        .fs(acp::FileSystemCapabilities::new().read_text_file(client_reads));
    acp::Agent::initialize(
        &conn,
        acp::InitializeRequest::new(acp::ProtocolVersion::V1).client_capabilities(capabilities),
    )
    .await
    .expect("initialize");
    let session = acp::Agent::new_session(
        &conn,
        acp::NewSessionRequest::new(fixture.workspace.clone()),
    )
    .await
    .expect("new session")
    .session_id;
    let request = acp::PromptRequest::new(
        session,
        vec![acp::ContentBlock::Text(acp::TextContent::new(
            "read notes.txt",
        ))],
    );
    tokio::time::timeout(Duration::from_secs(120), acp::Agent::prompt(&conn, request))
        .await
        .expect("prompt timed out")
        .expect("prompt");
    let _ = child.start_kill();
    let paths = reads.borrow().clone();
    paths
}

#[tokio::test(flavor = "current_thread")]
async fn declared_client_reads_serve_the_unsaved_buffer() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let seen: Seen = Arc::default();
            let fixture = Fixture::new(spawn_mock(Arc::clone(&seen)));
            let reads = read_turn(&fixture, true).await;

            let expected = fixture.workspace.join("notes.txt");
            assert!(
                reads.iter().any(|path| path == &expected),
                "no fs/read_text_file for {expected:?}: {reads:?}"
            );
            let seen = seen.lock().expect("seen").clone();
            assert_eq!(seen.len(), 1, "{seen:?}");
            assert!(seen[0].contains("UNSAVED BUFFER LINE"), "{seen:?}");
            assert!(!seen[0].contains("saved on disk"), "{seen:?}");
            assert_eq!(
                std::fs::read_to_string(&expected).expect("notes"),
                SAVED,
                "a read changed the file"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn undeclared_client_reads_stay_on_the_workspace() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let seen: Seen = Arc::default();
            let fixture = Fixture::new(spawn_mock(Arc::clone(&seen)));
            let reads = read_turn(&fixture, false).await;

            assert!(
                reads.is_empty(),
                "undeclared capability was used: {reads:?}"
            );
            let seen = seen.lock().expect("seen").clone();
            assert_eq!(seen.len(), 1, "{seen:?}");
            assert!(seen[0].contains("saved on disk"), "{seen:?}");
        })
        .await;
}
