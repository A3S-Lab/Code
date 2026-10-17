#![cfg(test)]
#![cfg(unix)]

//! ACP-CONF1: ACP v1 conformance of the shipped `a3s-code-acp` binary.
//!
//! Each case drives a fresh agent over stdio with raw JSON-RPC lines, not
//! through the Rust SDK's types, against a scripted local model. The runner
//! records every case against the ACP v1 requirement it checks, prints a
//! Markdown report, writes it to `A3S_ACP_CONFORMANCE_REPORT` when that is
//! set, and fails if any case fails.
//!
//! On macOS the guest backend points at a stub shim (as in
//! `approval_rules_stdio.rs`), so a bash call reaches the permission ask.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

const MODEL_ID: &str = "mock/gpt";
const WAIT: Duration = Duration::from_secs(30);

type Outcome = Result<(), String>;

macro_rules! ensure {
    ($cond:expr, $($fmt:tt)+) => {
        if !$cond {
            return Err(format!($($fmt)+));
        }
    };
}

// ---- scripted model -------------------------------------------------------

struct Model {
    addr: SocketAddr,
    slow_seen: Arc<AtomicBool>,
}

fn spawn_model() -> Model {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind model");
    let addr = listener.local_addr().expect("model addr");
    let slow_seen = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&slow_seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let seen = Arc::clone(&seen);
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
                if let Some(body) = read_http_body(&mut stream) {
                    let reply = decide(&body, &seen);
                    let _ = write_http(&mut stream, &reply);
                }
            });
        }
    });
    Model { addr, slow_seen }
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

fn reply_text(text: &str, streaming: bool) -> String {
    if !streaming {
        return json!({
            "id": "cmpl",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": text },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })
        .to_string();
    }
    let chunk = json!({ "choices": [{ "delta": { "content": text } }] });
    let done = json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] });
    format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

fn reply_bash(command: &str) -> String {
    let args = json!({ "command": command }).to_string();
    let chunk = json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_bash",
                    "function": { "name": "bash", "arguments": args }
                }]
            }
        }]
    });
    let done = json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] });
    format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

fn message_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// `SAY: <text>` answers the text, `SLOW` stalls the turn, and
/// `RUN: <command>` calls bash once. Auxiliary calls without tools get text.
fn decide(raw: &str, slow_seen: &AtomicBool) -> String {
    let parsed: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let streaming = parsed["stream"].as_bool().unwrap_or(false);
    let has_bash = parsed["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|tool| tool["function"]["name"] == "bash");
    let messages = parsed["messages"].as_array().cloned().unwrap_or_default();
    if !has_bash || messages.last().is_some_and(|m| m["role"] == "tool") {
        return reply_text("done", streaming);
    }
    let prompt = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .map(message_text)
        .unwrap_or_default();
    if let Some((_, said)) = prompt.split_once("SAY: ") {
        return reply_text(said.lines().next().unwrap_or(""), streaming);
    }
    if prompt.contains("SLOW") {
        slow_seen.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_secs(40));
        return reply_text("too late", streaming);
    }
    if let Some((_, command)) = prompt.split_once("RUN: ") {
        return reply_bash(command.lines().next().unwrap_or(""));
    }
    reply_text("done", streaming)
}

// ---- agent over raw JSON-RPC ----------------------------------------------

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    home: PathBuf,
    state: PathBuf,
    tmp: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new(model: &Model) -> Self {
        let root = tempfile::tempdir().expect("root");
        let dir = |name: &str| {
            let path = root.path().join(name);
            std::fs::create_dir_all(&path).expect("fixture dir");
            path
        };
        let workspace = std::fs::canonicalize(dir("workspace")).expect("workspace");
        let (home, state, tmp, outside) = (dir("home"), dir("state"), dir("tmp"), dir("outside"));
        let config = outside.join("config.acl");
        std::fs::write(
            &config,
            format!(
                "providers \"mock\" {{\n  base_url = \"http://{}/v1\"\n  api_key = \"test\"\n  models \"gpt\" {{\n  }}\n}}\ndefault_model = \"{MODEL_ID}\"\n",
                model.addr
            ),
        )
        .expect("config");
        Self {
            _root: root,
            workspace,
            home,
            state,
            tmp,
            config,
        }
    }
}

struct Wire {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    seen: Vec<Value>,
    next_id: u64,
}

impl Wire {
    fn launch(fixture: &Fixture) -> Self {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_a3s-code-acp"));
        if cfg!(target_os = "macos") {
            let image = fixture.tmp.join("guest-image.json");
            let shim = fixture.tmp.join("guest-shim");
            std::fs::write(&image, "{}").expect("stub image");
            std::fs::write(&shim, "#!/bin/sh\nexit 1\n").expect("stub shim");
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))
                .expect("shim mode");
            command
                .env("A3S_CODE_GUEST_SYSTEM_IMAGE", image)
                .env("A3S_CODE_KRUN_SHIM", shim);
        }
        let mut child = command
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
        let input = child.stdin.take().expect("stdin");
        let output = BufReader::new(child.stdout.take().expect("stdout")).lines();
        Self {
            child,
            input,
            output,
            seen: Vec::new(),
            next_id: 1,
        }
    }

    async fn send(&mut self, message: Value) -> Outcome {
        self.input
            .write_all(format!("{message}\n").as_bytes())
            .await
            .map_err(|error| format!("write to agent: {error}"))?;
        self.input
            .flush()
            .await
            .map_err(|error| format!("flush to agent: {error}"))
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        Ok(id)
    }

    async fn notify(&mut self, method: &str, params: Value) -> Outcome {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    /// Reads until a line satisfies `done`; returns its index in `seen`.
    async fn read_until(
        &mut self,
        what: &str,
        done: impl Fn(&Value) -> bool,
    ) -> Result<usize, String> {
        if let Some(index) = self.seen.iter().position(&done) {
            return Ok(index);
        }
        let read = tokio::time::timeout(WAIT, async {
            loop {
                let line = match self.output.next_line().await {
                    Ok(Some(line)) => line,
                    Ok(None) => return Err("agent stdout ended".to_string()),
                    Err(error) => return Err(format!("read agent stdout: {error}")),
                };
                let value: Value = serde_json::from_str(&line)
                    .map_err(|error| format!("agent wrote a non-JSON line {line:?}: {error}"))?;
                let matched = done(&value);
                self.seen.push(value);
                if matched {
                    return Ok(self.seen.len() - 1);
                }
            }
        })
        .await;
        match read {
            Ok(result) => result,
            Err(_) => Err(format!("no {what} within {WAIT:?}")),
        }
    }

    async fn response(&mut self, id: u64) -> Result<Value, String> {
        let index = self
            .read_until(&format!("response {id}"), |value| {
                value["id"] == id && value.get("method").is_none()
            })
            .await?;
        Ok(self.seen[index].clone())
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.request(method, params).await?;
        self.response(id).await
    }

    async fn initialize(&mut self) -> Result<Value, String> {
        let initialized = self
            .call(
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": {}}),
            )
            .await?;
        result_of(&initialized).cloned()
    }

    async fn new_session(&mut self, workspace: &Path) -> Result<String, String> {
        let opened = self
            .call("session/new", json!({"cwd": workspace, "mcpServers": []}))
            .await?;
        result_of(&opened)?["sessionId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("session/new without a sessionId: {opened}"))
    }

    async fn prompt(&mut self, session_id: &str, text: &str) -> Result<u64, String> {
        self.request(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
        )
        .await
    }

    async fn stop(mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

fn result_of(response: &Value) -> Result<&Value, String> {
    match (response.get("result"), response.get("error")) {
        (Some(result), None) => Ok(result),
        _ => Err(format!("expected a result: {response}")),
    }
}

fn error_code(response: &Value) -> Option<i64> {
    response
        .get("result")
        .is_none()
        .then(|| response["error"]["code"].as_i64())?
}

fn stop_reason(response: &Value) -> Result<String, String> {
    result_of(response)?["stopReason"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("prompt response without stopReason: {response}"))
}

fn update_kind<'a>(value: &'a Value, session_id: &str) -> Option<&'a str> {
    (value["method"] == "session/update" && value["params"]["sessionId"] == session_id)
        .then(|| value["params"]["update"]["sessionUpdate"].as_str())
        .flatten()
}

fn chunk_text(value: &Value) -> &str {
    value["params"]["update"]["content"]["text"]
        .as_str()
        .unwrap_or("")
}

fn is_permission_request(value: &Value) -> bool {
    value["method"] == "session/request_permission" && value.get("id").is_some()
}

// ---- cases ----------------------------------------------------------------

const CASES: &[(&str, &str)] = &[
    (
        "initialize-version",
        "initialize answers protocolVersion 1 for a v1 client, and its latest supported version (1) for a client asking a newer one",
    ),
    (
        "initialize-capabilities",
        "initialize returns agentCapabilities (loadSession, promptCapabilities) and an authMethods list",
    ),
    (
        "prompt-end-turn",
        "session/prompt streams agent_message_chunk updates for the session, then answers stopReason end_turn",
    ),
    (
        "prompt-unknown-session",
        "session/prompt for an unknown sessionId is a JSON-RPC error, not a result",
    ),
    (
        "prompt-invalid-params",
        "session/prompt without sessionId is invalid params (-32602)",
    ),
    (
        "unknown-method",
        "an unknown method is method not found (-32601)",
    ),
    (
        "cancel-in-flight",
        "session/cancel during a model call answers the pending session/prompt with stopReason cancelled",
    ),
    (
        "permission-request-shape",
        "session/request_permission names the session and toolCallId and offers options with an optionId, a name and a kind from the closed set; a reject ends the turn normally",
    ),
    (
        "permission-cancelled",
        "a client that cancels the turn and answers the pending permission request with outcome cancelled gets stopReason cancelled",
    ),
    (
        "load-replays-history",
        "with loadSession, session/load replays the conversation as user_message_chunk and agent_message_chunk updates before its response",
    ),
];

async fn run_case(id: &str, model: &Model) -> Outcome {
    let fixture = Fixture::new(model);
    let mut wire = Wire::launch(&fixture);
    let outcome = match id {
        "initialize-version" => initialize_version(&mut wire, &fixture).await,
        "initialize-capabilities" => initialize_capabilities(&mut wire).await,
        "prompt-end-turn" => prompt_end_turn(&mut wire, &fixture).await,
        "prompt-unknown-session" => prompt_unknown_session(&mut wire).await,
        "prompt-invalid-params" => prompt_invalid_params(&mut wire).await,
        "unknown-method" => unknown_method(&mut wire).await,
        "cancel-in-flight" => cancel_in_flight(&mut wire, &fixture, model).await,
        "permission-request-shape" => permission_request_shape(&mut wire, &fixture).await,
        "permission-cancelled" => permission_cancelled(&mut wire, &fixture).await,
        "load-replays-history" => load_replays_history(wire, &fixture).await,
        other => Err(format!("no case {other}")),
    };
    outcome
}

async fn initialize_version(wire: &mut Wire, fixture: &Fixture) -> Outcome {
    let result = wire.initialize().await?;
    ensure!(result["protocolVersion"] == 1, "v1 client got {result}");
    let mut newer = Wire::launch(fixture);
    let response = newer
        .call(
            "initialize",
            json!({"protocolVersion": 99, "clientCapabilities": {}}),
        )
        .await;
    newer.stop().await;
    let response = response?;
    ensure!(
        result_of(&response)?["protocolVersion"] == 1,
        "a client asking v99 got {response}"
    );
    Ok(())
}

async fn initialize_capabilities(wire: &mut Wire) -> Outcome {
    let result = wire.initialize().await?;
    let caps = &result["agentCapabilities"];
    ensure!(caps.is_object(), "no agentCapabilities: {result}");
    ensure!(
        caps["loadSession"] == true,
        "loadSession not advertised: {caps}"
    );
    ensure!(
        caps["promptCapabilities"].is_object(),
        "no promptCapabilities: {caps}"
    );
    ensure!(
        result["authMethods"].is_array(),
        "no authMethods list: {result}"
    );
    Ok(())
}

async fn prompt_end_turn(wire: &mut Wire, fixture: &Fixture) -> Outcome {
    wire.initialize().await?;
    let session = wire.new_session(&fixture.workspace).await?;
    let start = wire.seen.len();
    let id = wire.prompt(&session, "SAY: conformance hello").await?;
    let response = wire.response(id).await?;
    ensure!(stop_reason(&response)? == "end_turn", "{response}");
    let response_at = wire.seen.len() - 1;
    let said: String = wire.seen[start..response_at]
        .iter()
        .filter(|value| update_kind(value, &session) == Some("agent_message_chunk"))
        .map(chunk_text)
        .collect();
    ensure!(
        said.contains("conformance hello"),
        "no agent_message_chunk with the reply before the response; got {said:?}"
    );
    let foreign = wire.seen[start..]
        .iter()
        .filter(|value| value["method"] == "session/update")
        .find(|value| value["params"]["sessionId"] != session.as_str());
    ensure!(
        foreign.is_none(),
        "an update for another session: {foreign:?}"
    );
    Ok(())
}

async fn prompt_unknown_session(wire: &mut Wire) -> Outcome {
    wire.initialize().await?;
    let id = wire.prompt("no-such-session", "SAY: hi").await?;
    let response = wire.response(id).await?;
    ensure!(error_code(&response).is_some(), "{response}");
    Ok(())
}

async fn prompt_invalid_params(wire: &mut Wire) -> Outcome {
    wire.initialize().await?;
    let response = wire
        .call(
            "session/prompt",
            json!({"prompt": [{"type": "text", "text": "hi"}]}),
        )
        .await?;
    ensure!(error_code(&response) == Some(-32602), "{response}");
    Ok(())
}

async fn unknown_method(wire: &mut Wire) -> Outcome {
    wire.initialize().await?;
    let response = wire.call("session/no_such_method", json!({})).await?;
    ensure!(error_code(&response) == Some(-32601), "{response}");
    let response = wire.call("_vendor/no_such_method", json!({})).await?;
    ensure!(error_code(&response) == Some(-32601), "{response}");
    Ok(())
}

async fn cancel_in_flight(wire: &mut Wire, fixture: &Fixture, model: &Model) -> Outcome {
    wire.initialize().await?;
    let session = wire.new_session(&fixture.workspace).await?;
    model.slow_seen.store(false, Ordering::SeqCst);
    let id = wire.prompt(&session, "SLOW").await?;
    let deadline = tokio::time::Instant::now() + WAIT;
    while !model.slow_seen.load(Ordering::SeqCst) {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "the model call never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let cancelled_at = tokio::time::Instant::now();
    wire.notify("session/cancel", json!({"sessionId": session}))
        .await?;
    let response = wire.response(id).await?;
    ensure!(stop_reason(&response)? == "cancelled", "{response}");
    let took = cancelled_at.elapsed();
    ensure!(
        took < Duration::from_secs(10),
        "cancelled after {took:?}, the model would have answered in 40 s"
    );
    Ok(())
}

async fn permission_request(
    wire: &mut Wire,
    fixture: &Fixture,
) -> Result<(String, u64, Value), String> {
    wire.initialize().await?;
    let session = wire.new_session(&fixture.workspace).await?;
    let prompt = wire.prompt(&session, "RUN: make check").await?;
    let at = wire
        .read_until("session/request_permission", is_permission_request)
        .await?;
    Ok((session, prompt, wire.seen[at].clone()))
}

async fn permission_request_shape(wire: &mut Wire, fixture: &Fixture) -> Outcome {
    let (session, prompt, request) = permission_request(wire, fixture).await?;
    let params = &request["params"];
    ensure!(params["sessionId"] == session.as_str(), "{request}");
    ensure!(
        params["toolCall"]["toolCallId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "no toolCallId: {request}"
    );
    let options = params["options"].as_array().cloned().unwrap_or_default();
    ensure!(!options.is_empty(), "no options: {request}");
    for option in &options {
        ensure!(
            option["optionId"].as_str().is_some_and(|id| !id.is_empty())
                && option["name"].as_str().is_some_and(|name| !name.is_empty()),
            "option without optionId or name: {option}"
        );
        let kind = option["kind"].as_str().unwrap_or("");
        ensure!(
            matches!(
                kind,
                "allow_once" | "allow_always" | "reject_once" | "reject_always"
            ),
            "option kind outside the closed set: {option}"
        );
    }
    let reject = options
        .iter()
        .find(|option| option["kind"] == "reject_once")
        .and_then(|option| option["optionId"].as_str())
        .ok_or_else(|| format!("no reject_once option: {request}"))?
        .to_owned();
    wire.send(json!({"jsonrpc": "2.0", "id": request["id"],
                     "result": {"outcome": {"outcome": "selected", "optionId": reject}}}))
        .await?;
    let response = wire.response(prompt).await?;
    ensure!(stop_reason(&response)? == "end_turn", "{response}");
    Ok(())
}

async fn permission_cancelled(wire: &mut Wire, fixture: &Fixture) -> Outcome {
    let (session, prompt, request) = permission_request(wire, fixture).await?;
    wire.notify("session/cancel", json!({"sessionId": session}))
        .await?;
    wire.send(json!({"jsonrpc": "2.0", "id": request["id"],
                     "result": {"outcome": {"outcome": "cancelled"}}}))
        .await?;
    let response = wire.response(prompt).await?;
    ensure!(stop_reason(&response)? == "cancelled", "{response}");
    Ok(())
}

async fn load_replays_history(mut wire: Wire, fixture: &Fixture) -> Outcome {
    wire.initialize().await?;
    let session = wire.new_session(&fixture.workspace).await?;
    let id = wire.prompt(&session, "SAY: remembered reply").await?;
    let response = wire.response(id).await?;
    ensure!(stop_reason(&response)? == "end_turn", "{response}");
    wire.stop().await;

    let mut wire = Wire::launch(fixture);
    let outcome = async {
        let result = wire.initialize().await?;
        ensure!(
            result["agentCapabilities"]["loadSession"] == true,
            "loadSession not advertised: {result}"
        );
        let id = wire
            .request(
                "session/load",
                json!({"sessionId": session, "cwd": fixture.workspace, "mcpServers": []}),
            )
            .await?;
        let response = wire.response(id).await?;
        result_of(&response)?;
        let response_at = wire.seen.len() - 1;
        let replayed = &wire.seen[..response_at];
        let user: String = replayed
            .iter()
            .filter(|value| update_kind(value, &session) == Some("user_message_chunk"))
            .map(chunk_text)
            .collect();
        let agent: String = replayed
            .iter()
            .filter(|value| update_kind(value, &session) == Some("agent_message_chunk"))
            .map(chunk_text)
            .collect();
        ensure!(
            user.contains("SAY: remembered reply"),
            "no user_message_chunk replay before the response; got {user:?}"
        );
        ensure!(
            agent.contains("remembered reply"),
            "no agent_message_chunk replay before the response; got {agent:?}"
        );
        Ok(())
    }
    .await;
    wire.stop().await;
    outcome
}

// ---- runner ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acp_v1_conformance() {
    let model = spawn_model();
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for (id, requirement) in CASES {
        let outcome = run_case(id, &model).await;
        let cell = match &outcome {
            Ok(()) => "pass".to_string(),
            Err(error) => {
                failed.push(format!("{id}: {error}"));
                format!("FAIL: {}", error.replace('|', "\\|").replace('\n', " "))
            }
        };
        rows.push(format!("| `{id}` | {requirement} | {cell} |"));
    }
    let report = format!(
        "# a3s-code-acp {} ACP v1 conformance\n\n{} of {} cases pass. Client: raw JSON-RPC over stdio (`acp/tests/acp_conformance_stdio.rs`), scripted local model.\n\n| Case | ACP v1 requirement | Result |\n|---|---|---|\n{}\n",
        env!("CARGO_PKG_VERSION"),
        CASES.len() - failed.len(),
        CASES.len(),
        rows.join("\n")
    );
    println!("{report}");
    if let Some(path) = std::env::var_os("A3S_ACP_CONFORMANCE_REPORT") {
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("report dir");
        }
        std::fs::write(&path, &report).expect("write conformance report");
    }
    assert!(
        failed.is_empty(),
        "ACP conformance failures:\n{}",
        failed.join("\n")
    );
}
