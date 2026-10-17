//! End-to-end coverage of the shipped pager and the shipped ACP agent.
//!
//! The model is a local OpenAI-compatible server, except the ignored
//! `shipped_bound_goal_real_llm`, which runs the operator's live ACL. The agent process is
//! `a3s-code-acp`. Palette and launch cases spawn `a3s-code-tui` on a PTY.
//! Nothing here substitutes a scripted `LlmClient` or the pager's content mock.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_client_protocol as acp;
use async_trait::async_trait;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

const MODEL_ID: &str = "mock/gpt";
const COMPACT_SUMMARY: &str = "COMPACT-SUMMARY-7f3a";
const DURABLE_TOKEN: &str = "DURABLE-TOKEN-42";
const GOAL_OBJECTIVE: &str = "pty-goal-7b2e";
const WIRE_TEST: &str = "shipped_agent_session_wire";
/// Mirrors a3s-code-core's READ_ONLY_EVIDENCE_LIMIT: eight read-only tool
/// results with an empty mutation ledger end evidence gathering, independent
/// of the advertised tool-round budget.
const READ_ONLY_EVIDENCE_STOP: usize = 8;
const PTY_TEST: &str = "shipped_tui_launch_resume_and_visible_commands";

fn oracle(test: &str, name: &str, detail: &str) {
    let detail = detail.replace(['\n', '\r'], " ");
    eprintln!("ORACLE test={test} name={name} result=pass detail={detail}");
}

/// Hidden names and a description fragment that only appears on their palette row.
const UNAVAILABLE: &[(&str, &str)] = &[
    ("usage", "View usage"),
    ("view-plan", "View the current plan"),
    ("plan", "Enter plan mode"),
    ("tutorial", "Quick tips to get the most out of A3S Code"),
    ("docs", "Open How-to Guides"),
    ("privacy", "coding data, retention"),
    ("login", "Log in or re-authenticate"),
    ("logout", "Log out and return to the login screen"),
    ("btw", "side question without interrupting"),
    ("feedback", "Send feedback about the current session"),
    ("queue", "prompts queued behind"),
    ("fork", "Branch the current session"),
    ("resume", "Resume a previous session"),
    ("rename", "Rename the current session"),
    ("history", "Search prompt history"),
    ("transcript", "conversation transcript"),
    ("rewind", "Rewind to a previous turn"),
    ("workflows", "Browse installed workflows"),
    ("workflow", "Launch a saved workflow"),
    ("personas", "Manage personas"),
    ("config-agents", "Manage agent definitions"),
    ("plugins", "View plugins"),
    ("skills", "View skills"),
    ("mcps", "MCP server status"),
    ("hooks", "View hooks"),
    ("marketplace", "View marketplace"),
    ("import-claude", "Claude settings import"),
    ("remember", "Save a memory note"),
    ("tasks", "background tasks"),
    ("imagine", "imagine"),
    ("imagine-video", "imagine-video"),
    ("voice", "voice"),
    ("dashboard", "Agent Dashboard"),
    ("recap", "Summarize the session so far"),
    ("share", "Share this session via URL"),
    ("release-notes", "release notes"),
    ("announcements", "announcements"),
    ("dream", "Consolidate memory"),
    ("flush", "memory to disk"),
    ("memory", "manage your memories"),
    ("loop", "recurring interval"),
    ("auto", "classifier approves safe tools"),
];

/// Offered in the fullscreen agent session, with the palette description.
const FULLSCREEN_PALETTE: &[(&str, &str)] = &[
    ("settings", "Open the settings modal"),
    ("config", "Add and configure providers and models"),
    ("new", "Start a new session"),
    ("effort", "Set reasoning effort"),
    ("goal", "durable goal"),
    ("compact", "Compact conversation history"),
    ("model", "Switch the active model"),
    ("context", "View context usage"),
    ("jump", "Jump to a turn"),
    ("edit-prompt", "external editor"),
    ("session-info", "Show session info"),
    ("export", "Export the current conversation"),
    ("copy", "Copy last response"),
    ("find", "Search the conversation scrollback"),
    ("theme", "Switch the color theme"),
    ("yolo", "skip all permission prompts"),
    ("vim-mode", "vim-style scrollback"),
    ("multiline", "multiline input mode"),
    ("compact-mode", "less padding, more content"),
    ("timestamps", "message timestamps"),
    ("toggle-mouse-reporting", "terminal mouse reporting"),
    ("minimal", "scrollback-native"),
    ("timeline", "timeline sidebar"),
    ("doctor", "show available fixes"),
    ("home", "welcome screen"),
    ("delete", "Delete this session"),
    ("help", "keyboard shortcuts"),
    ("quit", "Quit the application"),
];

struct MockState {
    bodies: Vec<String>,
    smallest_rounds: Option<u64>,
    larger_tool_replies: u64,
    tool_seq: u64,
    write_tool_replies: u64,
    expand_replies: u64,
    ask_tool_replies: u64,
    write_path: String,
    late_path: String,
}

fn spawn_mock(write_path: &Path, late_path: &Path) -> (SocketAddr, Arc<Mutex<MockState>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let state = Arc::new(Mutex::new(MockState {
        bodies: Vec::new(),
        smallest_rounds: None,
        larger_tool_replies: 0,
        tool_seq: 0,
        write_tool_replies: 0,
        expand_replies: 0,
        ask_tool_replies: 0,
        write_path: write_path.display().to_string(),
        late_path: late_path.display().to_string(),
    }));
    let shared = Arc::clone(&state);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
            if let Some(body) = read_http_body(&mut stream) {
                let response = {
                    let mut guard = shared.lock().expect("mock");
                    guard.bodies.push(body.clone());
                    decide(&body, &mut guard)
                };
                let _ = write_http(&mut stream, &response);
            }
        }
    });
    (addr, state)
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
        if buf.len() > 16_000_000 {
            return None;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]);
    let mut length = 0_usize;
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
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

fn decide(raw: &str, state: &mut MockState) -> String {
    let parsed: serde_json::Value = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
    let streaming = parsed
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if raw.contains("context-compaction engine") {
        return reply_text(COMPACT_SUMMARY, streaming);
    }
    // The classifier call carries the user text and a tool named
    // emit_pre_analysis. Answer it before the oracle arms, or that tool
    // call consumes the write/bash reply the product turn still needs.
    if raw.contains("emit_pre_analysis") || raw.contains("You are a pre-analysis assistant") {
        return reply_text(&pre_analysis_object(), streaming);
    }
    let tools = has_tools(&parsed);
    if raw.contains("WRITE-ORACLE") && tools {
        if state.write_tool_replies >= 1 {
            return reply_text("ok", streaming);
        }
        state.write_tool_replies = state.write_tool_replies.saturating_add(1);
        let args = serde_json::json!({
            "file_path": state.write_path,
            "content": "wrote-marker"
        })
        .to_string();
        return reply_tool("write", &args, state, streaming);
    }
    if raw.contains("ASK-WRITE") && tools {
        if raw.contains("wrote-marker") {
            return reply_text("ok", streaming);
        }
        let args = serde_json::json!({
            "file_path": state.write_path,
            "content": "wrote-marker"
        })
        .to_string();
        return reply_tool("write", &args, state, streaming);
    }
    if raw.contains("EXPAND-ORACLE") && tools {
        if state.expand_replies >= 1 {
            return reply_text("ok", streaming);
        }
        state.expand_replies = state.expand_replies.saturating_add(1);
        // Default sandbox bash never starts on macOS, so the host does not
        // ask. Escalation is the path that parks for approval and then runs.
        let args = serde_json::json!({
            "command": "printf 'EXPAND-MARKER\\n'",
            "sandbox_permissions": "require_escalated",
            "justification": "workspace sandbox cannot start this command"
        })
        .to_string();
        return reply_tool("bash", &args, state, streaming);
    }
    if raw.contains("ASK-ORACLE") && tools {
        if state.ask_tool_replies >= 1 {
            return reply_text("ok", streaming);
        }
        state.ask_tool_replies = state.ask_tool_replies.saturating_add(1);
        let args = serde_json::json!({
            "question": "Pick one",
            "options": ["alpha", "beta"]
        })
        .to_string();
        return reply_tool("ask_user", &args, state, streaming);
    }
    if raw.contains("CANCEL-ORACLE") && tools {
        let command = format!(
            "sleep 4; printf LATE > '{}'",
            state.late_path.replace('\'', "")
        );
        let args = serde_json::json!({ "command": command }).to_string();
        return reply_tool("bash", &args, state, streaming);
    }
    if raw.contains("LIST-FILES") {
        if !tools {
            return reply_text("listed", streaming);
        }
        if let Some(rounds) = budget_number(raw, "tool_rounds=") {
            match state.smallest_rounds {
                Some(smallest) if rounds > smallest => {
                    if state.larger_tool_replies >= smallest.saturating_add(1) {
                        return reply_text("high-stopped", streaming);
                    }
                    state.larger_tool_replies = state.larger_tool_replies.saturating_add(1);
                }
                Some(smallest) => {
                    state.smallest_rounds = Some(smallest.min(rounds));
                }
                None => state.smallest_rounds = Some(rounds),
            }
        }
        let args = serde_json::json!({ "path": "." }).to_string();
        return reply_tool("ls", &args, state, streaming);
    }
    reply_text("ok", streaming)
}

fn has_tools(body: &serde_json::Value) -> bool {
    body.get("tools")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
}

fn budget_number(body: &str, marker: &str) -> Option<u64> {
    let rest = body.split_once(marker)?.1;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn pre_analysis_object() -> String {
    // Empty optimized_input keeps the original user sentence on the product turn.
    serde_json::json!({
        "intent": "GeneralPurpose",
        "requires_planning": false,
        "goal": {
            "description": "ok",
            "success_criteria": []
        },
        "execution_plan": {
            "complexity": "Simple",
            "steps": [{
                "id": "step-1",
                "description": "ok",
                "dependencies": [],
                "success_criteria": "Complete the request"
            }],
            "required_tools": []
        },
        "optimized_input": ""
    })
    .to_string()
}

fn reply_text(text: &str, streaming: bool) -> String {
    if !streaming {
        return serde_json::json!({
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
    let chunk = serde_json::json!({
        "choices": [{ "delta": { "content": text } }]
    });
    let done = serde_json::json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }]
    });
    format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

fn reply_tool(name: &str, args: &str, state: &mut MockState, streaming: bool) -> String {
    state.tool_seq = state.tool_seq.saturating_add(1);
    let id = format!("call_{name}_{}", state.tool_seq);
    if !streaming {
        return serde_json::json!({
            "id": "cmpl",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": id,
                        "type": "function",
                        "function": { "name": name, "arguments": args }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })
        .to_string();
    }
    let chunk = serde_json::json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": id,
                    "function": { "name": name, "arguments": args }
                }]
            }
        }]
    });
    let done = serde_json::json!({
        "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
    });
    format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

struct Gate {
    option_ids: Vec<String>,
    reply: tokio::sync::oneshot::Sender<String>,
}

struct TestClient {
    gates: tokio::sync::mpsc::UnboundedSender<Gate>,
    tool_names: Rc<std::cell::RefCell<Vec<String>>>,
    /// `(session id, prompt id)` stamped on session updates.
    stamps: Rc<std::cell::RefCell<Vec<(String, String)>>>,
    /// `(session id, is_replay, text)` from user and agent message chunks.
    replayed: Rc<std::cell::RefCell<Vec<(String, bool, String)>>>,
}

#[async_trait(?Send)]
impl acp::Client for TestClient {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, acp::Error> {
        let option_ids = args
            .options
            .iter()
            .map(|option| option.option_id.0.to_string())
            .collect();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.gates.send(Gate {
            option_ids,
            reply: tx,
        });
        let chosen = rx.await.unwrap_or_else(|_| "cancel".to_string());
        if chosen == "cancel" {
            return Ok(acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Cancelled,
            ));
        }
        Ok(acp::RequestPermissionResponse::new(
            acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                acp::PermissionOptionId::new(chosen),
            )),
        ))
    }

    async fn session_notification(&self, args: acp::SessionNotification) -> Result<(), acp::Error> {
        if let Some(prompt_id) = args
            .meta
            .as_ref()
            .and_then(|meta| meta.get("promptId"))
            .and_then(serde_json::Value::as_str)
        {
            self.stamps
                .borrow_mut()
                .push((args.session_id.0.to_string(), prompt_id.to_string()));
        }
        let is_replay = args
            .meta
            .as_ref()
            .and_then(|meta| meta.get("isReplay"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if let Some(text) = chunk_text(&args.update) {
            self.replayed
                .borrow_mut()
                .push((args.session_id.0.to_string(), is_replay, text));
        }
        if let acp::SessionUpdate::ToolCall(call) = &args.update {
            if !call.title.is_empty() {
                self.tool_names.borrow_mut().push(call.title.clone());
            }
        }
        if let acp::SessionUpdate::ToolCallUpdate(update) = &args.update {
            if let Some(title) = update.fields.title.as_ref() {
                if !title.is_empty() {
                    self.tool_names.borrow_mut().push(title.clone());
                }
            }
        }
        Ok(())
    }
}

struct AgentProc {
    child: tokio::process::Child,
    conn: Rc<acp::ClientSideConnection>,
    gates: tokio::sync::mpsc::UnboundedReceiver<Gate>,
    tool_names: Rc<std::cell::RefCell<Vec<String>>>,
    stamps: Rc<std::cell::RefCell<Vec<(String, String)>>>,
    replayed: Rc<std::cell::RefCell<Vec<(String, bool, String)>>>,
}

fn isolate_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "a3s-shipped-e2e-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn write_acl(home: &Path, workspace: &Path, base_url: &str) -> PathBuf {
    let config_dir = workspace.join(".a3s");
    std::fs::create_dir_all(&config_dir).expect("acl dir");
    let path = config_dir.join("config.acl");
    std::fs::write(
        &path,
        format!(
            r#"
providers "mock" {{
  base_url = "{base_url}"
  api_key = "test"
  models "gpt" {{
  }}
  models "alt" {{
  }}
}}
default_model = "{MODEL_ID}"
"#
        ),
    )
    .expect("acl");
    let _ = home;
    path
}

async fn spawn_agent(workspace: &Path, config: &Path, home: &Path) -> AgentProc {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_a3s-code-acp"));
    let stderr_path = workspace.join("acp.stderr");
    let stderr = std::fs::File::create(&stderr_path).expect("acp stderr");
    let mut child = tokio::process::Command::new(bin)
        .arg("--cwd")
        .arg(workspace)
        .env("A3S_CONFIG", config)
        .env("A3S_DISABLE_CC_SWITCH", "1")
        .env("A3S_DISABLE_GROK_ACCOUNT", "1")
        .env("A3S_DISABLE_CURSOR_ACCOUNT", "1")
        .env("XAI_API_KEY", "sk-dummy-not-a-model")
        .env("HOME", home)
        .env("GROK_HOME", home.join("grok"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn a3s-code-acp");
    let stdout = child.stdout.take().expect("stdout").compat();
    let stdin = child.stdin.take().expect("stdin").compat_write();
    let (gate_tx, gate_rx) = tokio::sync::mpsc::unbounded_channel();
    let tool_names = Rc::new(std::cell::RefCell::new(Vec::new()));
    let stamps = Rc::new(std::cell::RefCell::new(Vec::new()));
    let replayed = Rc::new(std::cell::RefCell::new(Vec::new()));
    let client = TestClient {
        gates: gate_tx,
        tool_names: Rc::clone(&tool_names),
        stamps: Rc::clone(&stamps),
        replayed: Rc::clone(&replayed),
    };
    let (conn, io) = acp::ClientSideConnection::new(client, stdin, stdout, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(async move {
        let _ = io.await;
    });
    AgentProc {
        child,
        conn: Rc::new(conn),
        gates: gate_rx,
        tool_names,
        stamps,
        replayed,
    }
}

fn chunk_text(update: &acp::SessionUpdate) -> Option<String> {
    let chunk = match update {
        acp::SessionUpdate::UserMessageChunk(chunk)
        | acp::SessionUpdate::AgentMessageChunk(chunk) => chunk,
        _ => return None,
    };
    match &chunk.content {
        acp::ContentBlock::Text(text) => Some(text.text.clone()),
        _ => None,
    }
}

fn user_prompt(session: &acp::SessionId, text: &str) -> acp::PromptRequest {
    acp::PromptRequest::new(
        session.clone(),
        vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
    )
}

fn user_prompt_id(session: &acp::SessionId, text: &str, prompt_id: &str) -> acp::PromptRequest {
    let mut meta = acp::Meta::new();
    meta.insert(
        "promptId".to_string(),
        serde_json::Value::String(prompt_id.to_string()),
    );
    user_prompt(session, text).meta(Some(meta))
}

async fn session_info_body(
    conn: &acp::ClientSideConnection,
    session: &acp::SessionId,
) -> serde_json::Value {
    let params = serde_json::value::to_raw_value(&serde_json::json!({
        "sessionId": session.0.as_ref(),
    }))
    .expect("session info params");
    let response = acp::Agent::ext_method(
        conn,
        acp::ExtRequest::new("x.ai/session/info", std::sync::Arc::from(params)),
    )
    .await
    .expect("session info");
    serde_json::from_str(response.0.get()).expect("session info json")
}

fn effort_meta(level: &str) -> acp::Meta {
    let mut meta = acp::Meta::new();
    meta.insert(
        "reasoningEffort".to_string(),
        serde_json::Value::String(level.to_string()),
    );
    meta
}

fn append_jsonl_dir(dir: &Path, out: &mut String) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path()) {
            out.push_str(&text);
            out.push('\n');
        }
    }
}

fn log_text(workspace: &Path) -> String {
    let mut out = String::new();
    append_jsonl_dir(&workspace.join(".a3s").join("effect-log"), &mut out);
    let Ok(dates) = std::fs::read_dir(workspace.join(".a3s").join("sessions")) else {
        return out;
    };
    for date in dates.flatten() {
        let Ok(homes) = std::fs::read_dir(date.path()) else {
            continue;
        };
        for home in homes.flatten() {
            append_jsonl_dir(&home.path(), &mut out);
        }
    }
    out
}

fn bodies_since(state: &MockState, start: usize) -> Vec<String> {
    state.bodies.iter().skip(start).cloned().collect()
}

async fn answer_gate(agent: &mut AgentProc, option_id: &str) {
    let gate = tokio::time::timeout(Duration::from_secs(30), agent.gates.recv())
        .await
        .expect("permission timed out")
        .expect("agent closed permission channel");
    assert!(
        gate.option_ids.iter().any(|id| id == option_id),
        "missing {option_id} in {:?}",
        gate.option_ids
    );
    gate.reply.send(option_id.to_string()).expect("reply");
}

#[tokio::test(flavor = "current_thread")]
async fn shipped_agent_session_wire() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate_dir("wire");
            let home = isolate_dir("wire-home");
            let write_path = workspace.join("side-effect.txt");
            let late_path = workspace.join("late.txt");
            let (addr, state) = spawn_mock(&write_path, &late_path);
            let config = write_acl(&home, &workspace, &format!("http://{addr}/v1"));
            let mut agent = spawn_agent(&workspace, &config, &home).await;

            let init = acp::Agent::initialize(
                agent.conn.as_ref(),
                acp::InitializeRequest::new(acp::ProtocolVersion::V1),
            )
            .await
            .expect("initialize");
            let meta = init.meta.expect("initialize meta");
            assert_eq!(
                meta.get("a3sCode").and_then(serde_json::Value::as_bool),
                Some(true)
            );
            let current = meta
                .get("modelState")
                .and_then(|value| value.get("currentModelId"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            assert_eq!(current, MODEL_ID, "XAI_API_KEY must not select the model");
            oracle(WIRE_TEST, "initialize-acl-model", current);

            let created = acp::Agent::new_session(
                agent.conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("new session");
            let session_model = created
                .models
                .as_ref()
                .map(|models| models.current_model_id.0.to_string())
                .unwrap_or_default();
            assert_eq!(session_model, MODEL_ID);
            oracle(WIRE_TEST, "new-session-model", &session_model);
            let session_id = created.session_id.clone();

            acp::Agent::set_session_model(
                agent.conn.as_ref(),
                acp::SetSessionModelRequest::new(session_id.clone(), acp::ModelId::new(MODEL_ID))
                    .meta(Some(effort_meta("low"))),
            )
            .await
            .expect("effort low");

            let start = state.lock().expect("mock").bodies.len();
            let mut product_prompts = 0u64;
            let listed = tokio::task::spawn_local({
                let conn = agent.conn.clone();
                let session_id = session_id.clone();
                async move {
                    acp::Agent::prompt(
                        conn.as_ref(),
                        user_prompt(&session_id, "LIST-FILES the workspace"),
                    )
                    .await
                }
            });
            let listed = listed.await.expect("join").expect("low prompt");
            assert_eq!(listed.stop_reason, acp::StopReason::EndTurn);
            product_prompts += 1;
            let info = session_info_body(agent.conn.as_ref(), &session_id).await;
            let turn_index = info["result"]["turnIndex"].as_u64().unwrap_or(0);
            let turns = info["result"]["turns"].as_u64().unwrap_or(0);
            let turn_count = info["result"]["context"]["turnCount"].as_u64().unwrap_or(0);
            let used = info["result"]["context"]["used"].as_u64().unwrap_or(0);
            assert_eq!(
                (turn_index, turns, turn_count),
                (product_prompts, product_prompts, product_prompts),
                "session-info turn does not match the prompts submitted on this session: {info}"
            );
            assert!(
                used >= 1,
                "completed turn still reports no token usage: {info}"
            );
            let low_bodies = bodies_since(&state.lock().expect("mock"), start);
            assert!(
                !low_bodies.is_empty(),
                "low effort produced no model request"
            );
            let low_thinking = budget_number(&low_bodies[0], "thinking_budget=")
                .expect("thinking budget in the request the model receives");
            let low_rounds = budget_number(&low_bodies[0], "tool_rounds=")
                .expect("tool rounds in the request the model receives");
            assert!(
                turn_index < low_rounds,
                "turn index {turn_index} counts tool rounds whose budget is {low_rounds}"
            );
            oracle(
                WIRE_TEST,
                "session-info-turn-usage",
                &format!(
                    "turn={turn_index} used={used} prompts={product_prompts} rounds={low_rounds}"
                ),
            );
            let tool_requests = low_bodies.iter().filter(|body| {
                serde_json::from_str::<serde_json::Value>(body)
                    .ok()
                    .is_some_and(|parsed| has_tools(&parsed))
            });
            let tool_request_count = tool_requests.count();
            // The advertised budget is a ceiling, and a second safety stop
            // binds first on this fixture: eight read-only tool results with
            // an empty mutation ledger end evidence gathering (the core's
            // READ_ONLY_EVIDENCE_LIMIT), so an `ls`-only loop stops below the
            // advertised 16. The none-effort phase below stops exactly at its
            // advertised 4 — the pair proves the enforced stop tracks the
            // request's budget instead of one hardcoded cap.
            assert_eq!(
                tool_request_count, READ_ONLY_EVIDENCE_STOP,
                "a read-only loop must stop at the evidence limit before the advertised {low_rounds}"
            );
            assert!(
                low_bodies.iter().any(|body| {
                    serde_json::from_str::<serde_json::Value>(body)
                        .ok()
                        .is_some_and(|parsed| !has_tools(&parsed))
                }),
                "the round after the stop must be asked to finish with no tools"
            );
            assert!(
                agent
                    .tool_names
                    .borrow()
                    .iter()
                    .any(|name| !name.is_empty() && name.contains("ls")),
                "tool updates need a non-empty tool name, saw {:?}",
                agent.tool_names.borrow()
            );
            oracle(
                WIRE_TEST,
                "tool-call-name",
                &format!("low_thinking={low_thinking} low_rounds={low_rounds}"),
            );

            acp::Agent::set_session_model(
                agent.conn.as_ref(),
                acp::SetSessionModelRequest::new(session_id.clone(), acp::ModelId::new(MODEL_ID))
                    .meta(Some(effort_meta("high"))),
            )
            .await
            .expect("effort high");
            let start = state.lock().expect("mock").bodies.len();
            let high = tokio::task::spawn_local({
                let conn = agent.conn.clone();
                let session_id = session_id.clone();
                async move {
                    acp::Agent::prompt(conn.as_ref(), user_prompt(&session_id, "LIST-FILES again"))
                        .await
                }
            });
            let high = high.await.expect("join").expect("high prompt");
            assert_eq!(high.stop_reason, acp::StopReason::EndTurn);
            let high_bodies = bodies_since(&state.lock().expect("mock"), start);
            let high_thinking =
                budget_number(&high_bodies[0], "thinking_budget=").expect("high thinking");
            let high_rounds = budget_number(&high_bodies[0], "tool_rounds=").expect("high rounds");
            assert!(high_thinking > low_thinking);
            assert!(high_rounds > low_rounds);
            let high_tool_requests = high_bodies
                .iter()
                .filter(|body| {
                    serde_json::from_str::<serde_json::Value>(body)
                        .ok()
                        .is_some_and(|parsed| has_tools(&parsed))
                })
                .count();
            // The evidence stop is conversation-scoped, not per turn: the
            // eight read-only results from the low turn are still in the
            // window, so the follow-up read-only prompt is asked to finish
            // immediately even though the advertised ceiling rose to
            // {high_rounds}. A fresh session (the none phase below) re-arms
            // the counter.
            assert_eq!(
                high_tool_requests, 0,
                "a read-only follow-up on the same session must not resume tool looping"
            );
            oracle(
                WIRE_TEST,
                "effort-budget",
                &format!("low={low_thinking}/{low_rounds} high={high_thinking}/{high_rounds}"),
            );

            // A budget below the evidence limit binds first: effort none
            // advertises four tool rounds, and the enforced stop lands exactly
            // there — with the eight-stop above this rules out a hardcoded cap.
            let none_session = acp::Agent::new_session(
                agent.conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("none session")
            .session_id;
            acp::Agent::set_session_model(
                agent.conn.as_ref(),
                acp::SetSessionModelRequest::new(none_session.clone(), acp::ModelId::new(MODEL_ID))
                    .meta(Some(effort_meta("none"))),
            )
            .await
            .expect("effort none");
            let start = state.lock().expect("mock").bodies.len();
            let budgeted = tokio::task::spawn_local({
                let conn = agent.conn.clone();
                let none_session = none_session.clone();
                async move {
                    acp::Agent::prompt(
                        conn.as_ref(),
                        user_prompt(&none_session, "LIST-FILES small budget"),
                    )
                    .await
                }
            });
            let budgeted = budgeted.await.expect("join").expect("none prompt");
            assert_eq!(budgeted.stop_reason, acp::StopReason::EndTurn);
            let none_bodies = bodies_since(&state.lock().expect("mock"), start);
            let none_rounds = budget_number(&none_bodies[0], "tool_rounds=").expect("none rounds");
            assert_eq!(none_rounds, 4, "effort none advertises four tool rounds");
            let none_tool_requests = none_bodies
                .iter()
                .filter(|body| {
                    serde_json::from_str::<serde_json::Value>(body)
                        .ok()
                        .is_some_and(|parsed| has_tools(&parsed))
                })
                .count();
            assert_eq!(
                none_tool_requests, none_rounds as usize,
                "tool calls must stop at the budget parsed from the request, not a hardcoded cap"
            );
            oracle(
                WIRE_TEST,
                "effort-budget-stop",
                &format!("advertised={none_rounds} enforced={none_tool_requests}"),
            );

            // Permission blocks until AllowOnce. The side-effect file stays absent.
            let perm_session = acp::Agent::new_session(
                agent.conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("perm session")
            .session_id;
            let writing = tokio::task::spawn_local({
                let conn = agent.conn.clone();
                let perm_session = perm_session.clone();
                async move {
                    acp::Agent::prompt(
                        conn.as_ref(),
                        user_prompt(&perm_session, "WRITE-ORACLE side-effect.txt"),
                    )
                    .await
                }
            });
            let gate = tokio::time::timeout(Duration::from_secs(30), agent.gates.recv())
                .await
                .expect("permission")
                .expect("gate");
            assert!(
                gate.option_ids.iter().any(|id| id == "allow-once"),
                "AllowOnce id missing: {:?}",
                gate.option_ids
            );
            assert!(
                !write_path.exists(),
                "write ran before the permission was answered"
            );
            gate.reply.send("allow-once".to_string()).expect("allow");
            let wrote = writing.await.expect("join");
            // The file is the side effect. An unverified mutation must not
            // EndTurn; the pager shows this completion gate as TurnFailed.
            let detail = match &wrote {
                Ok(response) => panic!(
                    "unverified write must stay behind the completion gate, got {response:?}"
                ),
                Err(error) => error
                    .data
                    .as_ref()
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string(),
            };
            assert!(
                detail.contains("completion gate:")
                    && detail.contains("no bound Passed verification"),
                "write turn ended without the completion gate: {detail}"
            );
            let written = std::fs::read_to_string(&write_path).expect("side effect file");
            assert_eq!(written, "wrote-marker");
            assert!(
                agent
                    .tool_names
                    .borrow()
                    .iter()
                    .any(|name| name.contains("write")),
                "write tool name missing: {:?}",
                agent.tool_names.borrow()
            );
            oracle(
                WIRE_TEST,
                "permission-blocks-until-allow-once",
                "completion-gate file=wrote-marker",
            );

            // Question stays parked until question.answered for the selected option.
            let ask_session = acp::Agent::new_session(
                agent.conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("ask session")
            .session_id;
            let asking = tokio::task::spawn_local({
                let conn = agent.conn.clone();
                let ask_session = ask_session.clone();
                async move {
                    acp::Agent::prompt(
                        conn.as_ref(),
                        user_prompt(&ask_session, "ASK-ORACLE a choice"),
                    )
                    .await
                }
            });
            let gate = tokio::time::timeout(Duration::from_secs(30), agent.gates.recv())
                .await
                .expect("question")
                .expect("gate");
            assert!(
                gate.option_ids.iter().any(|id| id == "question-0"),
                "question options: {:?}",
                gate.option_ids
            );
            let parked = log_text(&workspace);
            assert!(
                !parked.contains("\"question.answered\""),
                "question settled before the answer"
            );
            gate.reply.send("question-0".to_string()).expect("answer");
            let asked = asking.await.expect("join").expect("ask prompt");
            assert_eq!(asked.stop_reason, acp::StopReason::EndTurn);
            let answered = log_text(&workspace);
            assert!(
                answered.contains("\"kind\":\"question.answered\"")
                    || answered.contains("\"question.answered\""),
                "missing question.answered fact: {answered}"
            );
            assert!(
                answered.contains("alpha"),
                "the answer fact must record the selected option"
            );
            oracle(WIRE_TEST, "question-answered", "option=alpha");

            // Cancel ends Cancelled and does not leave the late file.
            let cancel_session = acp::Agent::new_session(
                agent.conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("cancel session")
            .session_id;
            let cancelling = tokio::task::spawn_local({
                let conn = agent.conn.clone();
                let cancel_session = cancel_session.clone();
                async move {
                    acp::Agent::prompt(
                        conn.as_ref(),
                        user_prompt(&cancel_session, "CANCEL-ORACLE run the slow command"),
                    )
                    .await
                }
            });
            answer_gate(&mut agent, "allow-once").await;
            let saw_bash = tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    if agent
                        .tool_names
                        .borrow()
                        .iter()
                        .any(|name| name.contains("bash"))
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await;
            assert!(saw_bash.is_ok(), "bash tool name was not reported");
            acp::Agent::cancel(
                agent.conn.as_ref(),
                acp::CancelNotification::new(cancel_session.clone()),
            )
            .await
            .expect("cancel");
            let cancelled = tokio::time::timeout(Duration::from_secs(20), cancelling)
                .await
                .expect("cancel prompt hung")
                .expect("join")
                .expect("cancel result");
            assert_eq!(cancelled.stop_reason, acp::StopReason::Cancelled);
            tokio::time::sleep(Duration::from_secs(5)).await;
            assert!(
                !late_path.exists(),
                "cancelled turn wrote the late side-effect file"
            );
            oracle(WIRE_TEST, "cancel-no-late-file", "stop=Cancelled");

            // Compact appends compaction.done and the next turn keeps the summary.
            let compact_session = acp::Agent::new_session(
                agent.conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("compact session")
            .session_id;
            for text in ["first note", "second note"] {
                let response =
                    acp::Agent::prompt(agent.conn.as_ref(), user_prompt(&compact_session, text))
                        .await
                        .expect("seed");
                assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
            }
            let params = serde_json::value::to_raw_value(&serde_json::json!({
                "sessionId": compact_session.0.as_ref(),
            }))
            .expect("params");
            acp::Agent::ext_method(
                agent.conn.as_ref(),
                acp::ExtRequest::new("x.ai/compact_conversation", std::sync::Arc::from(params)),
            )
            .await
            .expect("compact");
            let facts = log_text(&workspace);
            assert!(
                facts.contains("compaction.done"),
                "missing compaction.done: {facts}"
            );
            assert!(facts.contains(COMPACT_SUMMARY));
            let start = state.lock().expect("mock").bodies.len();
            acp::Agent::prompt(
                agent.conn.as_ref(),
                user_prompt(&compact_session, "continue after compact"),
            )
            .await
            .expect("post compact");
            let after = bodies_since(&state.lock().expect("mock"), start);
            assert!(
                after.iter().any(|body| body.contains(COMPACT_SUMMARY)),
                "next turn dropped the compaction summary"
            );
            oracle(WIRE_TEST, "compact-summary-kept", COMPACT_SUMMARY);

            // status / pause / clear do not start a model turn. set stores the objective.
            let goal_session = acp::Agent::new_session(
                agent.conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("goal session")
            .session_id;
            for command in ["/goal status", "/goal pause", "/goal clear"] {
                let start = state.lock().expect("mock").bodies.len();
                acp::Agent::prompt(agent.conn.as_ref(), user_prompt(&goal_session, command))
                    .await
                    .expect(command);
                let added = state.lock().expect("mock").bodies.len() - start;
                assert_eq!(added, 0, "{command} started a model turn");
            }
            acp::Agent::prompt(
                agent.conn.as_ref(),
                user_prompt(&goal_session, "/goal ship the durable objective"),
            )
            .await
            .expect("goal set");
            let goal_file =
                std::fs::read_to_string(workspace.join(".a3s").join("durable-goal.json"))
                    .expect("durable goal");
            assert!(goal_file.contains("ship the durable objective"));
            oracle(
                WIRE_TEST,
                "goal-file-and-quiet-verbs",
                "status/pause/clear added 0 bodies; set stored the objective",
            );

            // Two live sessions must stamp their own prompt ids. A shared
            // first-session id would land on the other session's updates.
            agent.stamps.borrow_mut().clear();
            let stamp_left = acp::Agent::new_session(
                agent.conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("stamp left")
            .session_id;
            let stamp_right = acp::Agent::new_session(
                agent.conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("stamp right")
            .session_id;
            acp::Agent::prompt(
                agent.conn.as_ref(),
                user_prompt_id(&stamp_left, "STAMP-LEFT", "stamp-left"),
            )
            .await
            .expect("stamp left prompt");
            acp::Agent::prompt(
                agent.conn.as_ref(),
                user_prompt_id(&stamp_right, "STAMP-RIGHT", "stamp-right"),
            )
            .await
            .expect("stamp right prompt");
            let stamps = agent.stamps.borrow().clone();
            let left_key = stamp_left.0.to_string();
            let right_key = stamp_right.0.to_string();
            let left_ids: Vec<_> = stamps
                .iter()
                .filter(|(session, _)| session == &left_key)
                .map(|(_, prompt_id)| prompt_id.as_str())
                .collect();
            let right_ids: Vec<_> = stamps
                .iter()
                .filter(|(session, _)| session == &right_key)
                .map(|(_, prompt_id)| prompt_id.as_str())
                .collect();
            assert!(
                !left_ids.is_empty() && left_ids.iter().all(|id| *id == "stamp-left"),
                "left session updates were stamped with another prompt id: {stamps:?}"
            );
            assert!(
                !right_ids.is_empty() && right_ids.iter().all(|id| *id == "stamp-right"),
                "right session updates were stamped with another prompt id: {stamps:?}"
            );
            oracle(
                WIRE_TEST,
                "prompt-id-per-session",
                "left=stamp-left right=stamp-right",
            );

            // A second process reloads the same session id and still sees the token.
            acp::Agent::prompt(agent.conn.as_ref(), user_prompt(&session_id, DURABLE_TOKEN))
                .await
                .expect("seed token");
            drop(agent.conn);
            let _ = agent.child.start_kill();
            let _ = agent.child.wait().await;

            let mut resumed = spawn_agent(&workspace, &config, &home).await;
            acp::Agent::initialize(
                resumed.conn.as_ref(),
                acp::InitializeRequest::new(acp::ProtocolVersion::V1),
            )
            .await
            .expect("reinitialize");
            let loaded = acp::Agent::load_session(
                resumed.conn.as_ref(),
                acp::LoadSessionRequest::new(session_id.clone(), workspace.clone()),
            )
            .await
            .expect("load session");
            let replayed = resumed.replayed.borrow().clone();
            let loaded_key = session_id.0.to_string();
            assert!(
                replayed.iter().any(|(sid, is_replay, text)| {
                    sid == &loaded_key && *is_replay && text.contains(DURABLE_TOKEN)
                }),
                "load_session did not replay the durable transcript: {replayed:?}"
            );
            let loaded_model = loaded
                .models
                .as_ref()
                .map(|models| models.current_model_id.0.to_string())
                .unwrap_or_default();
            assert_eq!(loaded_model, MODEL_ID);
            let start = state.lock().expect("mock").bodies.len();
            acp::Agent::prompt(
                resumed.conn.as_ref(),
                user_prompt(&session_id, "what was stored"),
            )
            .await
            .expect("resumed prompt");
            let resumed_bodies = bodies_since(&state.lock().expect("mock"), start);
            assert!(
                resumed_bodies
                    .iter()
                    .any(|body| body.contains(DURABLE_TOKEN)),
                "resume opened a blank session"
            );
            oracle(WIRE_TEST, "load-session-replay", DURABLE_TOKEN);

            drop(resumed.conn);
            let _ = resumed.child.start_kill();
            let _ = resumed.child.wait().await;
            let _ = std::fs::remove_dir_all(&workspace);
            let _ = std::fs::remove_dir_all(&home);
        })
        .await;
}

struct PtySession {
    parser: vt100::Parser,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    raw: Vec<u8>,
    /// Raw offset already answered with a cursor-position report.
    cpr_replied_through: usize,
}

fn tui_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("A3S_CODE_TUI_BIN") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vendor/a3s-code-ui/target/debug/a3s-code-tui")
}

fn open_pty(binary: &Path, args: &[&str], env: &[(&str, &str)], cwd: &Path) -> PtySession {
    open_pty_inner(binary, args, env, cwd, false)
}

/// Same as [`open_pty`], after dropping the parent environment.
/// portable-pty still injects `SHELL` from the password database.
fn open_pty_cleared(binary: &Path, args: &[&str], env: &[(&str, &str)], cwd: &Path) -> PtySession {
    open_pty_inner(binary, args, env, cwd, true)
}

fn open_pty_inner(
    binary: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    cwd: &Path,
    clear_env: bool,
) -> PtySession {
    let rows = 40;
    let cols = 120;
    let system = native_pty_system();
    let pair = system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("pty");
    let mut command = CommandBuilder::new(binary);
    if clear_env {
        command.env_clear();
    }
    for arg in args {
        command.arg(arg);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    command.cwd(cwd);
    let child = pair.slave.spawn_command(command).expect("spawn tui");
    let mut reader = pair.master.try_clone_reader().expect("reader");
    let writer = pair.master.take_writer().expect("writer");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0_u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    PtySession {
        parser: vt100::Parser::new(rows, cols, 1000),
        rx,
        writer,
        child,
        raw: Vec::new(),
        cpr_replied_through: 0,
    }
}

impl PtySession {
    fn drain(&mut self) {
        while let Ok(bytes) = self.rx.try_recv() {
            self.raw.extend_from_slice(&bytes);
            self.parser.process(&bytes);
        }
        // Inline mode asks the terminal where the cursor is (`CSI 6 n`).
        // A real terminal answers; this PTY has to do the same or the switch aborts.
        let query = b"\x1b[6n";
        if let Some(at) = self
            .raw
            .windows(query.len())
            .rposition(|window| window == query)
        {
            if at >= self.cpr_replied_through {
                self.cpr_replied_through = self.raw.len();
                let _ = self.writer.write_all(b"\x1b[12;1R");
                let _ = self.writer.flush();
            }
        }
    }

    fn pump(&mut self, budget: Duration) -> String {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            self.drain();
            std::thread::sleep(Duration::from_millis(30));
        }
        self.drain();
        self.screen()
    }

    fn screen(&mut self) -> String {
        self.drain();
        self.parser.screen().contents()
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        if let Err(error) = self.writer.write_all(bytes) {
            let screen = self.screen();
            let status = self.child.try_wait();
            panic!("pty write {bytes:?}: {error}; child={status:?}\n{screen}");
        }
        self.writer.flush().ok();
    }

    fn type_text(&mut self, text: &str) {
        self.write_bytes(text.as_bytes());
    }

    fn enter(&mut self) {
        self.write_bytes(b"\r");
    }

    fn escape(&mut self) {
        self.write_bytes(b"\x1b");
    }

    /// Close a modal or scrollback search.
    /// A lone Escape is held until the sequence timeout, so the next byte
    /// has to wait or it is swallowed as part of that sequence.
    fn dismiss(&mut self) {
        for _ in 0..3 {
            let screen = self.screen();
            let open = screen.contains("[✗]")
                || screen.contains("search:")
                || screen.contains("Esc close")
                || screen.contains("Esc:cancel");
            if !open {
                return;
            }
            self.escape();
            std::thread::sleep(Duration::from_millis(400));
            self.pump(Duration::from_millis(80));
        }
    }

    /// Vim scrollback keeps `/` as search. `i` returns to the prompt; on the
    /// prompt it is just a letter, which Ctrl-U clears.
    fn focus_composer(&mut self) {
        self.dismiss();
        self.write_bytes(b"i");
        std::thread::sleep(Duration::from_millis(80));
        self.write_bytes(b"\x15");
        self.pump(Duration::from_millis(80));
    }

    fn wait_for_frame(&mut self, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        let mut stable_since: Option<Instant> = None;
        while Instant::now() < deadline {
            self.drain();
            let screen = self.parser.screen().contents();
            let visible: String = screen.chars().filter(|ch| !ch.is_whitespace()).collect();
            if visible.len() > 12 {
                if screen == last {
                    if stable_since
                        .is_some_and(|start| start.elapsed() > Duration::from_millis(500))
                    {
                        return screen;
                    }
                    stable_since.get_or_insert_with(Instant::now);
                } else {
                    stable_since = Some(Instant::now());
                    last = screen;
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        panic!("timed out waiting for a product frame\n{last}")
    }

    fn wait_for(&mut self, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        let mut last = String::new();
        while Instant::now() < deadline {
            self.drain();
            last = self.parser.screen().contents();
            if last.contains(needle) {
                return last;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        panic!("timed out waiting for {needle:?}\n{last}")
    }
}

/// `default_model` as written into the ACL file, not a hard-coded id.
fn acl_default_model(config: &Path) -> String {
    let text = std::fs::read_to_string(config).expect("read acl");
    text.lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("default_model")?;
            let rest = rest.trim().strip_prefix('=')?.trim();
            let rest = rest.trim_matches('"').trim();
            (!rest.is_empty()).then(|| rest.to_string())
        })
        .expect("acl default_model")
}

/// Model id on the session-info row. `Model Hash` is a different label.
fn model_field(screen: &str) -> Option<String> {
    screen.lines().find_map(|line| {
        let rest = line.split_once("Model:")?.1;
        rest.split_whitespace()
            .map(|word| {
                word.trim_matches(|ch: char| {
                    !ch.is_ascii_alphanumeric() && ch != '/' && ch != '-' && ch != '_' && ch != '.'
                })
            })
            .find(|word| word.contains('/'))
            .map(str::to_string)
    })
}

/// Launch necessities plus one credential. No model selector and no importer switch.
fn key_only_env(acp: &str, config: &Path, home: &Path) -> Vec<(String, String)> {
    vec![
        ("A3S_ACP_AGENT_BIN".into(), acp.into()),
        ("A3S_CONFIG".into(), config.display().to_string()),
        ("XAI_API_KEY".into(), "sk-dummy-not-a-model".into()),
        ("HOME".into(), home.display().to_string()),
        ("GROK_HOME".into(), home.join("grok").display().to_string()),
        ("TERM".into(), "xterm-256color".into()),
    ]
}

const KEY_ONLY_FORBIDDEN: &[&str] = &[
    "A3S_DEFAULT_MODEL",
    "A3S_PREFER_GROK",
    "A3S_PREFER_CC_SWITCH",
    "A3S_DISABLE_GROK_ACCOUNT",
    "A3S_DISABLE_CC_SWITCH",
    "A3S_GROK_HOME",
    "A3S_CC_SWITCH_DB",
    "CC_SWITCH_HOME",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_MODEL",
    "OPENAI_API_KEY",
    "OPENAI_MODEL",
];

fn child_env(acp: &str, config: &Path, home: &Path) -> Vec<(String, String)> {
    vec![
        ("A3S_ACP_AGENT_BIN".into(), acp.into()),
        ("A3S_CONFIG".into(), config.display().to_string()),
        ("A3S_DISABLE_CC_SWITCH".into(), "1".into()),
        ("A3S_DISABLE_GROK_ACCOUNT".into(), "1".into()),
        ("A3S_DISABLE_CURSOR_ACCOUNT".into(), "1".into()),
        ("XAI_API_KEY".into(), "sk-dummy-not-a-model".into()),
        ("HOME".into(), home.display().to_string()),
        ("GROK_HOME".into(), home.join("grok").display().to_string()),
        ("TERM".into(), "xterm-256color".into()),
    ]
}

fn line_session_id(line: &str) -> Option<String> {
    line.split_whitespace().find_map(|word| {
        let token = word.trim_matches(|ch: char| !ch.is_ascii_hexdigit() && ch != '-');
        looks_like_session_id(token).then(|| token.to_string())
    })
}

fn session_id_from_screen(screen: &str) -> String {
    let lines: Vec<&str> = screen.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("Session ID") {
            continue;
        }
        if let Some(id) = line_session_id(line) {
            return id;
        }
        // The modal puts the label and the id on separate rows, with a box
        // glyph as the first word of the value row.
        for next in lines.iter().skip(index + 1).take(4) {
            if next.contains("Working directory") || next.contains("Model:") {
                break;
            }
            if let Some(id) = line_session_id(next) {
                return id;
            }
        }
    }
    String::new()
}

fn palette_offers(screen: &str, name: &str, description: &str) -> bool {
    screen.lines().any(|line| {
        let trimmed = line.trim();
        let description_hit = description != name
            && !description.eq_ignore_ascii_case(name)
            && trimmed.contains(description);
        if description_hit {
            return true;
        }
        // Palette rows are `❯ /name description`. The composer echoes the
        // typed command inside a box (`│ ❯ /name │`), so that line's only
        // alphanumeric token is the name. A row still counts when the name
        // shares the line with any other word.
        let tokens: Vec<&str> = trimmed
            .split_whitespace()
            .map(|word| word.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-'))
            .filter(|word| !word.is_empty())
            .collect();
        let name_hits = tokens.iter().filter(|word| **word == name).count();
        name_hits > 0 && tokens.len() > name_hits
    })
}

fn config_texts(home: &Path) -> String {
    [
        home.join("grok").join("config.toml"),
        home.join(".grok").join("config.toml"),
    ]
    .iter()
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .collect::<Vec<_>>()
    .join("\n")
}

fn wait_config(home: &Path, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if config_texts(home).contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    false
}

fn summary_for(home: &Path, session_id: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, session_id: &str, depth: usize, out: &mut Option<PathBuf>) {
        if depth > 6 || out.is_some() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path.file_name().and_then(|name| name.to_str()) == Some(session_id) {
                let summary = path.join("summary.json");
                if summary.is_file() {
                    *out = Some(summary);
                    return;
                }
            }
            walk(&path, session_id, depth + 1, out);
        }
    }
    let mut out = None;
    walk(home, session_id, 0, &mut out);
    out
}

fn session_goal_file(workspace: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, found: &mut Option<PathBuf>) {
        if found.is_some() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.file_name().and_then(|name| name.to_str()) == Some("durable-goal.json") {
                *found = Some(path);
                return;
            }
        }
    }
    let mut found = None;
    walk(&workspace.join(".a3s").join("sessions"), &mut found);
    found
}

fn read_session_goal(workspace: &Path) -> (Option<PathBuf>, String) {
    let Some(path) = session_goal_file(workspace) else {
        return (None, String::new());
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    (Some(path), text)
}

fn wait_file(path: &Path, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(path) {
            if !text.is_empty() {
                return text;
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    std::fs::read_to_string(path).unwrap_or_default()
}

fn wait_body(state: &Arc<Mutex<MockState>>, needle: &str, timeout: Duration) -> String {
    wait_body_all(state, &[needle], timeout)
}

fn wait_body_all(state: &Arc<Mutex<MockState>>, needles: &[&str], timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(body) = state
            .lock()
            .expect("mock")
            .bodies
            .iter()
            .rev()
            .find(|body| needles.iter().all(|needle| body.contains(needle)))
        {
            return body.clone();
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    let last = state
        .lock()
        .expect("mock")
        .bodies
        .last()
        .cloned()
        .unwrap_or_default();
    let excerpt: String = last.chars().take(400).collect();
    panic!("no model request contained {needles:?}\nlast body: {excerpt}");
}

fn looks_like_session_id(token: &str) -> bool {
    token.len() >= 8 && token.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
}

fn launch_cleared_session(env: &[(&str, &str)], cwd: &Path) -> (String, String, String) {
    let binary = tui_binary();
    assert!(
        binary.is_file(),
        "shipped a3s-code-tui is missing at {}",
        binary.display()
    );
    let args = ["--cwd", cwd.to_str().expect("cwd")];
    let mut pty = open_pty_cleared(&binary, &args, env, cwd);
    let ready = pty.wait_for_frame(Duration::from_secs(45));
    assert!(
        ready.chars().any(|ch| !ch.is_whitespace()),
        "cleared launch produced an empty product frame"
    );
    pty.type_text("/session-info");
    pty.enter();
    let info = pty.wait_for("Session ID", Duration::from_secs(15));
    let session_id = session_id_from_screen(&info);
    let model = model_field(&info).unwrap_or_default();
    pty.escape();
    pty.pump(Duration::from_millis(200));
    pty.type_text("/quit");
    pty.enter();
    let _ = pty.child.wait();
    (session_id, model, info)
}

fn launch_once(continue_last: bool, env: &[(&str, &str)], cwd: &Path) -> (String, String) {
    let binary = tui_binary();
    assert!(
        binary.is_file(),
        "shipped a3s-code-tui is missing at {}",
        binary.display()
    );
    let mut args = vec!["--cwd", cwd.to_str().expect("cwd")];
    if continue_last {
        args.push("--continue");
    }
    let mut pty = open_pty(&binary, &args, env, cwd);
    let ready = pty.wait_for_frame(Duration::from_secs(45));
    assert!(
        ready.chars().any(|ch| !ch.is_whitespace()),
        "launch produced an empty product frame"
    );
    pty.type_text("/session-info");
    pty.enter();
    let info = pty.wait_for("Session ID", Duration::from_secs(15));
    assert!(
        info.contains(MODEL_ID),
        "session info did not show the ACL model\n{info}"
    );
    let session_id = session_id_from_screen(&info);
    assert!(
        !session_id.is_empty(),
        "session info had no session id\n{info}"
    );
    pty.escape();
    pty.pump(Duration::from_millis(200));
    pty.type_text("/quit");
    pty.enter();
    let _ = pty.child.wait();
    (session_id, info)
}

fn session_info_turn_and_used(screen: &str) -> Option<(u64, u64)> {
    let mut turn = None;
    let mut used = None;
    for line in screen.lines() {
        // The session-info modal paints a box glyph before the label.
        if let Some(rest) = line.split_once("Turn:").map(|(_, rest)| rest) {
            if let Some(parsed) = rest
                .split_whitespace()
                .next()
                .and_then(|token| token.parse().ok())
            {
                turn = Some(parsed);
            }
        }
        if let Some(rest) = line.split_once("Context:").map(|(_, rest)| rest) {
            if let Some(parsed) = rest
                .split(|ch: char| !ch.is_ascii_digit())
                .find(|token| !token.is_empty())
                .and_then(|token| token.parse().ok())
            {
                used = Some(parsed);
            }
        }
    }
    Some((turn?, used?))
}

/// One real turn, then quit. Returns the session id the pager persisted.
fn seed_marked_session(
    env: &[(&str, &str)],
    cwd: &Path,
    state: &Arc<Mutex<MockState>>,
    marker: &str,
) -> String {
    let binary = tui_binary();
    let cwd_arg = cwd.to_str().expect("cwd");
    let args = ["--cwd", cwd_arg];
    let mut pty = open_pty(&binary, &args, env, cwd);
    pty.wait_for_frame(Duration::from_secs(45));
    pty.type_text(marker);
    pty.enter();
    let _ = wait_body(state, marker, Duration::from_secs(25));
    let seeded = pty.wait_for(marker, Duration::from_secs(15));
    assert!(
        seeded.contains(marker),
        "seeded session did not keep the transcript token\n{seeded}"
    );
    pty.write_bytes(b"\x15");
    pty.type_text("/session-info");
    pty.enter();
    let info = pty.wait_for("Session ID", Duration::from_secs(15));
    assert!(
        info.contains(MODEL_ID),
        "seeded session did not show the ACL model\n{info}"
    );
    let session_id = session_id_from_screen(&info);
    assert!(
        !session_id.is_empty(),
        "seeded session info had no session id\n{info}"
    );
    pty.escape();
    pty.pump(Duration::from_millis(200));
    pty.type_text("/quit");
    pty.enter();
    let _ = pty.child.wait();
    session_id
}

fn launch_resumed(
    session_id: &str,
    env: &[(&str, &str)],
    cwd: &Path,
    marker: &str,
) -> (String, String) {
    let binary = tui_binary();
    let cwd_arg = cwd.to_str().expect("cwd");
    let args = ["--cwd", cwd_arg, "--resume", session_id];
    let mut pty = open_pty(&binary, &args, env, cwd);
    let resumed = pty.wait_for(marker, Duration::from_secs(30));
    assert!(
        resumed.contains(marker),
        "--resume did not reopen the prior transcript\n{resumed}"
    );
    assert!(
        resumed.contains(MODEL_ID),
        "--resume did not show the ACL model\n{resumed}"
    );
    pty.type_text("/session-info");
    pty.enter();
    let info = pty.wait_for("Session ID", Duration::from_secs(15));
    let resumed_id = session_id_from_screen(&info);
    assert_eq!(
        resumed_id, session_id,
        "--resume opened a different session\n{info}"
    );
    pty.escape();
    pty.pump(Duration::from_millis(200));
    pty.type_text("/quit");
    pty.enter();
    let _ = pty.child.wait();
    (resumed_id, resumed)
}

fn exercise_marker_navigation(
    env: &[(&str, &str)],
    cwd: &Path,
    session_id: &str,
    marker: &str,
    state: &Arc<Mutex<MockState>>,
) {
    let binary = tui_binary();
    let cwd_arg = cwd.to_str().expect("cwd");
    let args = ["--cwd", cwd_arg, "--resume", session_id];
    let mut pty = open_pty(&binary, &args, env, cwd);
    let opened = pty.wait_for(marker, Duration::from_secs(30));
    assert!(
        opened.contains(marker),
        "marked session did not show the transcript\n{opened}"
    );

    pty.write_bytes(b"\x15");
    pty.type_text(&format!("/find {marker}"));
    pty.enter();
    let found = pty.wait_for("search:", Duration::from_secs(6));
    assert!(
        found.contains("search:") && found.contains(marker),
        "find did not show the marker\n{found}"
    );
    oracle(PTY_TEST, "find-marker", marker);

    // The picker opens only when the timeline has two turns. One turn toasts
    // "Nothing to jump to yet" and never paints the title.
    pty.focus_composer();
    pty.type_text("JUMP-SECOND-7c1a");
    pty.enter();
    let _ = wait_body(state, "JUMP-SECOND-7c1a", Duration::from_secs(25));
    let _second = pty.wait_for("JUMP-SECOND-7c1a", Duration::from_secs(15));

    pty.focus_composer();
    pty.type_text("/jump");
    pty.enter();
    let jump = pty.wait_for("Jump to which turn?", Duration::from_secs(6));
    assert!(
        jump.contains(marker),
        "jump preview omitted the marker\n{jump}"
    );
    // Oldest row is the marked turn. Up is a no-op when the picker already opened there.
    pty.write_bytes(b"k");
    pty.pump(Duration::from_millis(200));
    let selected = pty.screen();
    assert!(
        selected.contains("Jump to which turn?") && selected.contains(marker),
        "jump lost the marker row\n{selected}"
    );
    pty.enter();
    pty.pump(Duration::from_millis(300));
    pty.focus_composer();
    pty.type_text("/session-info");
    pty.enter();
    let info = pty.wait_for("Session ID", Duration::from_secs(15));
    let (turn, _) = session_info_turn_and_used(&info).unwrap_or((0, 0));
    // Turn counts user.message facts. The marker plus this second prompt is 2.
    // It is not the viewport row the picker selected.
    assert_eq!(
        turn, 2,
        "session-info turn is the product prompt count\n{info}"
    );
    oracle(PTY_TEST, "jump-turn", "preview=marker prompts=2");
    pty.escape();
    pty.pump(Duration::from_millis(200));
    pty.focus_composer();

    // Bash asks while the posture is still Ask. /yolo would persist into the
    // shared grok home and the later tour would skip Allow once.
    let before_expand = state.lock().expect("mock").bodies.len();
    pty.type_text("EXPAND-ORACLE");
    pty.enter();
    let allow_expand = pty.wait_for("Yes, proceed", Duration::from_secs(20));
    assert!(
        allow_expand.contains("Yes, proceed"),
        "expand bash did not ask\n{allow_expand}"
    );
    pty.write_bytes(b"1");
    let expand_deadline = Instant::now() + Duration::from_secs(25);
    while Instant::now() < expand_deadline {
        if state.lock().expect("mock").bodies.len() > before_expand.saturating_add(1) {
            break;
        }
        pty.pump(Duration::from_millis(100));
    }
    assert!(
        state.lock().expect("mock").bodies.len() > before_expand.saturating_add(1),
        "expand bash did not finish"
    );
    pty.write_bytes(b"\x15");
    pty.type_text("/minimal");
    pty.enter();
    let _minimal = pty.wait_for("Switched to minimal mode", Duration::from_secs(6));
    pty.pump(Duration::from_millis(800));
    pty.write_bytes(b"\x15");
    pty.type_text("/expand");
    pty.enter();
    let expanded = pty.wait_for("EXPAND-MARKER", Duration::from_secs(8));
    assert!(
        expanded.contains("EXPAND-MARKER") && !expanded.contains("Nothing to expand"),
        "expand did not reprint the folded tool output\n{expanded}"
    );
    oracle(PTY_TEST, "expand-marker", "EXPAND-MARKER");

    // Minimal mode quits on a second Ctrl-C while the prompt is idle and empty.
    // A slash `/quit` clears the composer and leaves the process up.
    pty.write_bytes(b"\x15");
    pty.pump(Duration::from_millis(80));
    pty.write_bytes(b"\x03");
    pty.pump(Duration::from_millis(200));
    pty.write_bytes(b"\x03");
    let quit_deadline = Instant::now() + Duration::from_secs(8);
    loop {
        pty.pump(Duration::from_millis(80));
        match pty.child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= quit_deadline => {
                pty.child.kill().ok();
                let _ = pty.child.wait();
                break;
            }
            Ok(None) => {}
            Err(error) => panic!("quit wait failed: {error}"),
        }
    }
}

fn launch_continued(
    expected_id: &str,
    env: &[(&str, &str)],
    cwd: &Path,
    marker: &str,
) -> (String, String) {
    let binary = tui_binary();
    let cwd_arg = cwd.to_str().expect("cwd");
    let args = ["--cwd", cwd_arg, "--continue"];
    let mut pty = open_pty(&binary, &args, env, cwd);
    let continued = pty.wait_for(marker, Duration::from_secs(30));
    assert!(
        continued.contains(marker),
        "--continue did not reopen the prior transcript\n{continued}"
    );
    assert!(
        continued.contains(MODEL_ID),
        "--continue did not show the ACL model\n{continued}"
    );
    pty.type_text("/session-info");
    pty.enter();
    let info = pty.wait_for("Session ID", Duration::from_secs(15));
    let continued_id = session_id_from_screen(&info);
    assert_eq!(
        continued_id, expected_id,
        "--continue opened a different session\n{info}"
    );
    pty.escape();
    pty.pump(Duration::from_millis(200));
    pty.type_text("/quit");
    pty.enter();
    let _ = pty.child.wait();
    (continued_id, continued)
}

#[test]
fn shipped_tui_xai_key_keeps_acl_model() {
    let binary = tui_binary();
    assert!(
        binary.is_file(),
        "build a3s-code-tui before this test: {}",
        binary.display()
    );
    let workspace = isolate_dir("xai-key");
    let home = isolate_dir("xai-key-home");
    let grok = home.join("grok");
    std::fs::create_dir_all(&grok).expect("grok home");
    assert!(
        !grok.join("auth.json").is_file(),
        "hermetic home must not contain grok auth"
    );
    assert!(!home.join(".cc-switch").join("cc-switch.db").is_file());
    let (addr, _state) = spawn_mock(
        &workspace.join("side-effect.txt"),
        &workspace.join("late.txt"),
    );
    let config = write_acl(&home, &workspace, &format!("http://{addr}/v1"));
    let acl_model = acl_default_model(&config);
    assert!(
        acl_model.contains('/'),
        "ACL default_model must be provider/model, got {acl_model}"
    );
    let owned = key_only_env(env!("CARGO_BIN_EXE_a3s-code-acp"), &config, &home);
    assert!(
        owned
            .iter()
            .any(|(key, value)| key == "XAI_API_KEY" && !value.is_empty()),
        "XAI_API_KEY missing from the cleared launch"
    );
    for (key, _) in &owned {
        assert!(
            !KEY_ONLY_FORBIDDEN.contains(&key.as_str()),
            "cleared launch set a model selector: {key}"
        );
    }
    let credential_keys = owned
        .iter()
        .filter(|(key, _)| key.ends_with("API_KEY") || key.contains("API_KEY"))
        .count();
    assert_eq!(
        credential_keys, 1,
        "only XAI_API_KEY may be set among credentials: {owned:?}"
    );
    let env: Vec<(&str, &str)> = owned
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (session_id, shown, info) = launch_cleared_session(&env, &workspace);
    assert!(
        !session_id.is_empty(),
        "session info had no session id\n{info}"
    );
    assert_eq!(
        shown, acl_model,
        "XAI_API_KEY replaced the merged ACL default\n{info}"
    );
    oracle(
        "shipped_tui_xai_key_keeps_acl_model",
        "xai-key-keeps-acl-model",
        &format!("model={shown} session={session_id}"),
    );
    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn shipped_tui_launch_resume_and_visible_commands() {
    let binary = tui_binary();
    if !binary.is_file() {
        panic!("build a3s-code-tui before this test: {}", binary.display());
    }
    let workspace = isolate_dir("pty");
    let home = isolate_dir("pty-home");
    let write_path = workspace.join("side-effect.txt");
    let late_path = workspace.join("late.txt");
    let (addr, state) = spawn_mock(&write_path, &late_path);
    let config = write_acl(&home, &workspace, &format!("http://{addr}/v1"));
    let grok_home = home.join("grok");
    std::fs::create_dir_all(&grok_home).expect("grok home");
    std::fs::write(
        grok_home.join("config.toml"),
        "[ui]\nmouse_reporting_toggle = true\n",
    )
    .expect("mouse flag");
    let editor = home.join("editor.sh");
    std::fs::write(
        &editor,
        format!(
            "#!/bin/sh\nprintf edited > '{}'\nexit 0\n",
            workspace.join("editor-ran.txt").display()
        ),
    )
    .expect("editor script");
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).expect("editor mode");
    let acp_bin = env!("CARGO_BIN_EXE_a3s-code-acp");
    let mut owned = child_env(acp_bin, &config, &home);
    owned.push(("EDITOR".into(), editor.display().to_string()));
    owned.push(("VISUAL".into(), editor.display().to_string()));
    let env: Vec<(&str, &str)> = owned
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    let (first_id, first_frame) = launch_once(false, &env, &workspace);
    assert!(first_frame.contains(MODEL_ID));
    let (second_id, second_frame) = launch_once(true, &env, &workspace);
    assert_eq!(first_id, second_id, "continue opened a different session");
    assert!(second_frame.contains(MODEL_ID));
    assert_ne!(first_frame.trim(), "", "first frame blank");
    assert_ne!(second_frame.trim(), "", "second frame blank");
    oracle(
        PTY_TEST,
        "continue-same-session",
        &format!("session={first_id} model={MODEL_ID}"),
    );
    eprintln!("---LAUNCH-1---\n{first_frame}\n---END-LAUNCH-1---");
    eprintln!("---LAUNCH-2---\n{second_frame}\n---END-LAUNCH-2---");

    // The same continue path with the agent found beside the pager, not via
    // A3S_ACP_AGENT_BIN. A separate directory keeps this summary out of the
    // launch pair above.
    let discover_ws = isolate_dir("pty-discover");
    let discover_config = write_acl(&home, &discover_ws, &format!("http://{addr}/v1"));
    let mut discover_owned = child_env(acp_bin, &discover_config, &home);
    discover_owned.retain(|(key, _)| key != "A3S_ACP_AGENT_BIN");
    let discover_env: Vec<(&str, &str)> = discover_owned
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let (discovered_id, discovered_frame) = launch_once(false, &discover_env, &discover_ws);
    assert!(
        discovered_frame.contains(MODEL_ID),
        "discovered agent did not report the ACL model\n{discovered_frame}"
    );
    let (continued_id, continued_frame) = launch_once(true, &discover_env, &discover_ws);
    assert_eq!(
        discovered_id, continued_id,
        "continue without A3S_ACP_AGENT_BIN opened a different session"
    );
    assert!(continued_frame.contains(MODEL_ID));
    oracle(
        PTY_TEST,
        "continue-without-agent-env",
        &format!("session={discovered_id} model={MODEL_ID}"),
    );

    let resume_marker = "RESUME-MARKER-9e4b";
    let resume_ws = isolate_dir("pty-resume");
    let seeded_id = seed_marked_session(&env, &resume_ws, &state, resume_marker);
    let (resumed_id, resumed_frame) = launch_resumed(&seeded_id, &env, &resume_ws, resume_marker);
    assert_eq!(
        resumed_id, seeded_id,
        "--resume reopened a different session"
    );
    assert!(
        resumed_frame.contains(MODEL_ID) && resumed_frame.contains(resume_marker),
        "--resume frame lost the model or the prior transcript\n{resumed_frame}"
    );
    oracle(
        PTY_TEST,
        "resume-durable-transcript",
        &format!("session={resumed_id} marker={resume_marker}"),
    );
    let (continued_marked_id, continued_marked) =
        launch_continued(&seeded_id, &env, &resume_ws, resume_marker);
    assert_eq!(
        continued_marked_id, seeded_id,
        "--continue reopened a different session"
    );
    assert!(
        continued_marked.contains(MODEL_ID) && continued_marked.contains(resume_marker),
        "--continue frame lost the model or the prior transcript\n{continued_marked}"
    );
    oracle(
        PTY_TEST,
        "continue-durable-transcript",
        &format!("session={continued_marked_id} marker={resume_marker}"),
    );
    exercise_marker_navigation(&env, &resume_ws, &seeded_id, resume_marker, &state);

    let args = vec!["--cwd", workspace.to_str().expect("cwd")];
    let mut pty = open_pty(&binary, &args, &env, &workspace);
    pty.wait_for_frame(Duration::from_secs(45));

    for (name, description) in UNAVAILABLE {
        pty.escape();
        pty.pump(Duration::from_millis(80));
        pty.write_bytes(b"\x15");
        pty.type_text(&format!("/{name}"));
        let screen = pty.pump(Duration::from_millis(350));
        assert!(
            !palette_offers(&screen, name, description),
            "/{name} is still offered\n{screen}"
        );
        oracle(PTY_TEST, &format!("hidden-{name}"), "absent from palette");
    }
    // `cd` stays in the effective list and is dashboard-only, so the agent session does not offer it.
    pty.escape();
    pty.pump(Duration::from_millis(80));
    pty.write_bytes(b"\x15");
    pty.type_text("/cd");
    let cd_screen = pty.pump(Duration::from_millis(350));
    assert!(
        !palette_offers(&cd_screen, "cd", "working directory for new agents"),
        "/cd is offered outside the dashboard\n{cd_screen}"
    );
    oracle(PTY_TEST, "cd-agent-palette", "absent outside the dashboard");

    for (name, description) in FULLSCREEN_PALETTE {
        pty.escape();
        pty.pump(Duration::from_millis(80));
        pty.write_bytes(b"\x15");
        pty.type_text(&format!("/{name}"));
        let screen = pty.pump(Duration::from_millis(400));
        assert!(
            palette_offers(&screen, name, description),
            "/{name} missing from the palette\n{screen}"
        );
        oracle(PTY_TEST, &format!("palette-{name}"), description);
    }

    // Each still-visible command's own effect. Escape dismisses the palette
    // left open by the last probe; the line clear has to land after that.
    pty.escape();
    pty.pump(Duration::from_millis(80));
    pty.write_bytes(b"\x15");
    pty.type_text("/doctor");
    pty.enter();
    let doctor = pty.wait_for("terminal", Duration::from_secs(8));
    assert!(
        doctor.to_ascii_lowercase().contains("terminal")
            || doctor.to_ascii_lowercase().contains("color")
            || doctor.to_ascii_lowercase().contains("clipboard"),
        "doctor did not report\n{doctor}"
    );
    oracle(PTY_TEST, "doctor", "terminal report");

    // Goal and compact before any user turn: status must not call the model,
    // and an empty session reports that there is nothing to compact.
    let bodies_before_goal = state.lock().expect("mock").bodies.len();
    pty.write_bytes(b"\x15");
    pty.type_text("/goal status");
    pty.enter();
    let goal = pty.wait_for("No durable goal is set.", Duration::from_secs(8));
    assert!(
        goal.contains("No durable goal is set."),
        "goal status did not report the empty objective\n{goal}"
    );
    assert_eq!(
        state.lock().expect("mock").bodies.len(),
        bodies_before_goal,
        "/goal status started a model turn"
    );
    oracle(
        PTY_TEST,
        "goal-status",
        "empty objective, zero model requests",
    );
    pty.write_bytes(b"\x15");
    pty.type_text("/compact");
    pty.enter();
    let compact = pty.wait_for("Nothing to compact yet.", Duration::from_secs(8));
    assert!(
        compact.contains("Nothing to compact yet."),
        "compact on an empty session did not report its own result\n{compact}"
    );
    oracle(PTY_TEST, "compact-empty", "Nothing to compact yet");

    // Set stores the objective and starts one model turn. Pause and clear
    // report their own result and must not start another.
    let mut product_prompts = 0u64;
    let mut goal_path = workspace.join(".a3s").join("sessions");
    pty.write_bytes(b"\x15");
    pty.type_text(&format!("/goal {GOAL_OBJECTIVE}"));
    pty.enter();
    let goal_deadline = Instant::now() + Duration::from_secs(25);
    let mut stored = String::new();
    let mut saw_goal_turn = false;
    while Instant::now() < goal_deadline {
        pty.pump(Duration::from_millis(100));
        saw_goal_turn = state
            .lock()
            .expect("mock")
            .bodies
            .iter()
            .any(|body| body.contains(GOAL_OBJECTIVE));
        let (path, text) = read_session_goal(&workspace);
        if let Some(path) = path {
            goal_path = path;
        }
        stored = text;
        if saw_goal_turn && stored.contains(GOAL_OBJECTIVE) {
            break;
        }
    }
    assert!(
        saw_goal_turn && stored.contains(GOAL_OBJECTIVE) && stored.contains("\"paused\": false"),
        "goal set did not store the objective or start its turn: {stored:?}"
    );
    pty.pump(Duration::from_millis(800));
    product_prompts += 1;
    oracle(PTY_TEST, "goal-set", GOAL_OBJECTIVE);
    let bodies_before_pause = state.lock().expect("mock").bodies.len();
    pty.write_bytes(b"\x15");
    pty.type_text("/goal pause");
    pty.enter();
    let paused = pty.wait_for("Durable goal paused:", Duration::from_secs(8));
    assert!(
        paused.contains("Durable goal paused:") && paused.contains(GOAL_OBJECTIVE),
        "goal pause did not report the stored objective\n{paused}"
    );
    let (paused_path, paused_file) = read_session_goal(&workspace);
    if let Some(path) = paused_path {
        goal_path = path;
    }
    assert!(
        paused_file.contains(GOAL_OBJECTIVE) && paused_file.contains("\"paused\": true"),
        "goal pause did not persist the paused objective: {paused_file}"
    );
    assert_eq!(
        state.lock().expect("mock").bodies.len(),
        bodies_before_pause,
        "/goal pause started a model turn"
    );
    oracle(PTY_TEST, "goal-pause", "paused=true, zero model requests");
    let bodies_before_resume = state.lock().expect("mock").bodies.len();
    pty.write_bytes(b"\x15");
    pty.type_text("/goal resume");
    pty.enter();
    let resume_deadline = Instant::now() + Duration::from_secs(25);
    let mut resumed_file = String::new();
    let mut saw_resume_turn = false;
    while Instant::now() < resume_deadline {
        pty.pump(Duration::from_millis(100));
        saw_resume_turn = state.lock().expect("mock").bodies.iter().any(|body| {
            body.contains("Continue this durable goal") && body.contains(GOAL_OBJECTIVE)
        });
        let (path, text) = read_session_goal(&workspace);
        if let Some(path) = path {
            goal_path = path;
        }
        resumed_file = text;
        if saw_resume_turn
            && resumed_file.contains(GOAL_OBJECTIVE)
            && resumed_file.contains("\"paused\": false")
        {
            break;
        }
    }
    assert!(
        saw_resume_turn
            && resumed_file.contains(GOAL_OBJECTIVE)
            && resumed_file.contains("\"paused\": false"),
        "goal resume did not clear paused or continue the objective: {resumed_file:?}"
    );
    assert!(
        state.lock().expect("mock").bodies.len() > bodies_before_resume,
        "/goal resume did not send the continuation prompt"
    );
    product_prompts += 1;
    oracle(PTY_TEST, "goal-resume", "paused=false continuation-sent");
    let bodies_before_clear = state.lock().expect("mock").bodies.len();
    pty.write_bytes(b"\x15");
    pty.type_text("/goal clear");
    pty.enter();
    let cleared = pty.wait_for("Cleared the durable goal.", Duration::from_secs(8));
    assert!(
        cleared.contains("Cleared the durable goal."),
        "goal clear did not report\n{cleared}"
    );
    let clear_deadline = Instant::now() + Duration::from_secs(4);
    while goal_path.exists() && Instant::now() < clear_deadline {
        pty.pump(Duration::from_millis(80));
    }
    assert!(
        !goal_path.exists(),
        "goal clear left {}",
        goal_path.display()
    );
    assert_eq!(
        state.lock().expect("mock").bodies.len(),
        bodies_before_clear,
        "/goal clear started a model turn"
    );
    oracle(PTY_TEST, "goal-clear", "file removed, zero model requests");

    // Export skips system chrome. A user turn has to exist or the command
    // reports an empty transcript and writes nothing. This stays before
    // vim-mode so the composer is still in insert mode.
    pty.write_bytes(b"\x15");
    pty.pump(Duration::from_millis(80));
    pty.type_text("EXPORT-SOURCE");
    let typed = pty.pump(Duration::from_millis(300));
    assert!(
        typed.contains("EXPORT-SOURCE"),
        "composer did not accept the export prompt\n{typed}"
    );
    pty.enter();
    let _export_turn = wait_body(&state, "EXPORT-SOURCE", Duration::from_secs(25));
    product_prompts += 1;
    pty.pump(Duration::from_secs(1));
    let export_path = workspace.join("export-out").join("exported-session.md");
    pty.write_bytes(b"\x15");
    pty.type_text(&format!("/export {}", export_path.display()));
    pty.pump(Duration::from_millis(200));
    pty.escape();
    pty.pump(Duration::from_millis(80));
    pty.enter();
    let export_screen = pty.pump(Duration::from_millis(400));
    let exported = wait_file(&export_path, Duration::from_secs(4));
    assert!(
        exported.contains("EXPORT-SOURCE"),
        "export did not write the conversation: {exported:?}\n{export_screen}"
    );
    oracle(PTY_TEST, "export", "file contains EXPORT-SOURCE");

    pty.write_bytes(b"\x15");
    pty.type_text("/session-info");
    pty.enter();
    let after_turn = pty.wait_for("Session ID", Duration::from_secs(15));
    let (turn, used) = session_info_turn_and_used(&after_turn).unwrap_or((0, 0));
    assert_eq!(
        turn, product_prompts,
        "session-info turn {turn} is not the {product_prompts} prompts submitted in this session\n{after_turn}"
    );
    assert!(
        used >= 1,
        "session-info after a completed turn still shows {used} tokens\n{after_turn}"
    );
    let tour_id = session_id_from_screen(&after_turn);
    assert!(
        !tour_id.is_empty(),
        "session-info had no session id\n{after_turn}"
    );
    oracle(
        PTY_TEST,
        "session-info",
        &format!("session={tour_id} turn={turn} prompts={product_prompts} used={used}"),
    );
    pty.escape();
    pty.pump(Duration::from_millis(200));
    pty.dismiss();

    pty.escape();
    pty.write_bytes(b"\x15");
    let before_theme = config_texts(&home);
    assert!(
        !before_theme.contains("grokday"),
        "theme was already grokday before /theme\n{before_theme}"
    );
    pty.type_text("/theme grokday");
    pty.enter();
    assert!(
        wait_config(&home, "grokday", Duration::from_secs(3)),
        "theme did not persist grokday\n{}",
        config_texts(&home)
    );
    oracle(PTY_TEST, "theme", "grokday");

    pty.write_bytes(b"\x15");
    let before_vim = config_texts(&home);
    assert!(
        !before_vim.contains("vim_mode = true"),
        "vim mode was already on\n{before_vim}"
    );
    pty.type_text("/vim-mode");
    pty.enter();
    assert!(
        wait_config(&home, "vim_mode = true", Duration::from_secs(3)),
        "vim-mode did not persist vim_mode = true\n{}",
        config_texts(&home)
    );
    oracle(PTY_TEST, "vim-mode", "vim_mode = true");

    pty.write_bytes(b"\x15");
    let before_timestamps = config_texts(&home);
    assert!(
        !before_timestamps.contains("show_timestamps = false"),
        "timestamps were already off\n{before_timestamps}"
    );
    pty.type_text("/timestamps");
    pty.enter();
    assert!(
        wait_config(&home, "show_timestamps = false", Duration::from_secs(3)),
        "timestamps did not persist show_timestamps = false\n{}",
        config_texts(&home)
    );
    oracle(PTY_TEST, "timestamps", "show_timestamps = false");
    pty.write_bytes(b"\x15");
    pty.type_text("/multiline");
    pty.enter();
    let multiline = pty.wait_for("Multiline:", Duration::from_secs(4));
    assert!(
        multiline.contains("Multiline:"),
        "multiline toggle did not report its session flag\n{multiline}"
    );
    oracle(PTY_TEST, "multiline-on", "Multiline:");
    pty.write_bytes(b"\x15");
    let before_compact_mode = config_texts(&home);
    assert!(
        !before_compact_mode.contains("compact_mode = true"),
        "compact mode was already on\n{before_compact_mode}"
    );
    pty.type_text("/compact-mode");
    pty.enter();
    assert!(
        wait_config(&home, "compact_mode = true", Duration::from_secs(3)),
        "compact-mode did not persist compact_mode = true\n{}",
        config_texts(&home)
    );
    oracle(PTY_TEST, "compact-mode", "compact_mode = true");
    pty.write_bytes(b"\x15");
    let before_timeline = config_texts(&home);
    assert!(
        !before_timeline.contains("show_timeline = true"),
        "timeline was already on\n{before_timeline}"
    );
    pty.type_text("/timeline");
    pty.enter();
    assert!(
        wait_config(&home, "show_timeline = true", Duration::from_secs(3)),
        "timeline did not persist show_timeline = true\n{}",
        config_texts(&home)
    );
    oracle(PTY_TEST, "timeline", "show_timeline = true");

    pty.write_bytes(b"\x15");
    pty.type_text("/help");
    pty.enter();
    let help = pty.wait_for("Commands", Duration::from_secs(5));
    assert!(
        help.contains("Settings") && (help.contains("Quit") || help.contains("Keyboard Shortcuts")),
        "help did not open the command palette\n{help}"
    );
    oracle(PTY_TEST, "help", "Commands palette");
    pty.escape();
    pty.pump(Duration::from_millis(120));
    // One Escape can land while the palette is still opening. Close it
    // before the next slash command, or Enter selects a palette row.
    if pty.screen().contains("Esc close") {
        pty.escape();
        pty.pump(Duration::from_millis(120));
    }

    pty.write_bytes(b"\x15");
    pty.type_text("/settings");
    pty.enter();
    let settings = pty.wait_for("Space toggle", Duration::from_secs(5));
    assert!(
        settings.contains("Settings") && settings.contains("Space toggle"),
        "settings modal did not open\n{settings}"
    );
    oracle(PTY_TEST, "settings", "Settings modal");
    pty.dismiss();

    pty.write_bytes(b"\x15");
    pty.type_text("/config");
    pty.enter();
    let config_modal = pty.wait_for("Add provider", Duration::from_secs(6));
    assert!(
        config_modal.contains("Providers")
            && config_modal.contains("Models")
            && config_modal.contains("Add provider"),
        "config modal did not open\n{config_modal}"
    );
    oracle(PTY_TEST, "config", "Providers and Models");
    pty.dismiss();

    pty.write_bytes(b"\x15");
    pty.type_text("/context");
    pty.enter();
    let context = pty.wait_for("System prompt", Duration::from_secs(8));
    assert!(
        context.contains("System prompt") && context.contains("Auto-compact"),
        "context report missing\n{context}"
    );
    let context_turns = context.lines().find_map(|line| {
        // The context modal paints a box glyph before the footer label.
        let rest = line.split_once("Turns:")?.1;
        let digits: String = rest
            .trim()
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        digits.parse::<u64>().ok()
    });
    assert_eq!(
        context_turns,
        Some(product_prompts),
        "context Turns is not the {product_prompts} prompts submitted in this session\n{context}"
    );
    let context_used = context
        .lines()
        .any(|line| line.contains("tokens") && line.contains('/') && !line.contains("0 /"));
    assert!(
        context_used,
        "context after a completed turn still shows no tokens\n{context}"
    );
    oracle(PTY_TEST, "context", "turns and tokens");
    pty.dismiss();

    let before_compact = log_text(&workspace);
    assert!(
        !before_compact.contains("compaction.done"),
        "compaction.done was already in the log before /compact"
    );
    pty.write_bytes(b"\x15");
    pty.type_text("/compact");
    pty.enter();
    let compact_deadline = Instant::now() + Duration::from_secs(25);
    let mut compact_facts = String::new();
    let mut compact_screen = String::new();
    while Instant::now() < compact_deadline {
        compact_screen = pty.pump(Duration::from_millis(150));
        compact_facts = log_text(&workspace);
        if compact_facts.contains("compaction.done") && compact_facts.contains(COMPACT_SUMMARY) {
            break;
        }
    }
    assert!(
        compact_facts.contains("compaction.done") && compact_facts.contains(COMPACT_SUMMARY),
        "compact did not append compaction.done\n{compact_facts}\n{compact_screen}"
    );
    oracle(PTY_TEST, "compact-done", COMPACT_SUMMARY);
    pty.dismiss();

    pty.write_bytes(b"\x15");
    pty.type_text("/find marker");
    pty.enter();
    let find = pty.wait_for("search:", Duration::from_secs(6));
    assert!(
        find.contains("search:"),
        "find did not open the scrollback search row\n{find}"
    );
    oracle(PTY_TEST, "find", "search:");
    // Find leaves vim scrollback focused, where `/` opens search again.
    pty.focus_composer();
    pty.type_text("/jump");
    pty.enter();
    let jump_deadline = Instant::now() + Duration::from_secs(6);
    let mut jump = String::new();
    while Instant::now() < jump_deadline {
        jump = pty.pump(Duration::from_millis(150));
        if jump.contains("Jump to which turn?") || jump.contains("Nothing to jump to yet") {
            break;
        }
    }
    assert!(
        jump.contains("Jump to which turn?") || jump.contains("Nothing to jump to yet"),
        "jump picker missing\n{jump}"
    );
    oracle(PTY_TEST, "jump", "picker");
    pty.escape();
    pty.pump(Duration::from_millis(150));
    pty.dismiss();

    // Multiline swaps Enter and Shift+Enter. The on-toast already landed.
    // Find left scrollback focused; return to the prompt before the toggle.
    pty.focus_composer();
    pty.type_text("/multiline");
    pty.enter();
    let multiline_deadline = Instant::now() + Duration::from_secs(4);
    let mut multiline_screen = String::new();
    let mut multiline_off = false;
    while Instant::now() < multiline_deadline {
        multiline_screen = pty.pump(Duration::from_millis(120));
        let footer_on = multiline_screen
            .lines()
            .any(|line| line.contains("mock/gpt") && line.contains("multiline"));
        if multiline_screen.contains("mock/gpt") && !footer_on {
            multiline_off = true;
            break;
        }
    }
    assert!(multiline_off, "multiline stayed on\n{multiline_screen}");
    oracle(PTY_TEST, "multiline-off", "footer cleared");

    pty.write_bytes(b"\x15");
    pty.type_text("/effort low");
    pty.enter();
    pty.pump(Duration::from_millis(400));
    pty.write_bytes(b"\x15");
    pty.type_text("EFFORT-LOW-PROBE");
    let typed_effort = pty.pump(Duration::from_millis(200));
    assert!(
        typed_effort.contains("EFFORT-LOW-PROBE"),
        "composer did not accept the effort probe\n{typed_effort}"
    );
    pty.enter();
    let low_body = wait_body_all(
        &state,
        &["EFFORT-LOW-PROBE", "thinking_budget=", COMPACT_SUMMARY],
        Duration::from_secs(20),
    );
    let low_thinking = budget_number(&low_body, "thinking_budget=").expect("tui low thinking");
    let low_rounds = budget_number(&low_body, "tool_rounds=").expect("tui low rounds");
    pty.write_bytes(b"\x15");
    pty.type_text("/effort high");
    pty.enter();
    pty.pump(Duration::from_millis(400));
    pty.write_bytes(b"\x15");
    pty.type_text("EFFORT-HIGH-PROBE");
    pty.enter();
    let high_body = wait_body_all(
        &state,
        &["EFFORT-HIGH-PROBE", "thinking_budget="],
        Duration::from_secs(20),
    );
    let high_thinking = budget_number(&high_body, "thinking_budget=").expect("tui high thinking");
    let high_rounds = budget_number(&high_body, "tool_rounds=").expect("tui high rounds");
    assert!(
        high_thinking > low_thinking,
        "tui effort did not change thinking budget"
    );
    assert!(
        high_rounds > low_rounds,
        "tui effort did not change tool rounds"
    );
    oracle(
        PTY_TEST,
        "effort",
        &format!("low={low_thinking}/{low_rounds} high={high_thinking}/{high_rounds}"),
    );

    // The session already uses mock/gpt, so repeating that id is a no-op.
    // mock/alt is the other catalog entry. A reasoning model row ends with a
    // space, so Enter would open the effort menu instead of submitting.
    // Escape closes that menu and leaves the typed id; Enter then persists it.
    pty.write_bytes(b"\x15");
    pty.type_text("/model mock/alt");
    let typed_model = pty.pump(Duration::from_millis(250));
    assert!(
        typed_model.contains("mock/alt"),
        "composer did not show the model id\n{typed_model}"
    );
    pty.escape();
    std::thread::sleep(Duration::from_millis(450));
    let armed = pty.pump(Duration::from_millis(80));
    assert!(
        armed.contains("mock/alt"),
        "escape cleared the model command\n{armed}"
    );
    pty.enter();
    let config_path = grok_home.join("config.toml");
    let model_deadline = Instant::now() + Duration::from_secs(6);
    let mut persisted = String::new();
    while Instant::now() < model_deadline {
        persisted = std::fs::read_to_string(&config_path).unwrap_or_default();
        if persisted.contains("mock/alt") {
            break;
        }
        pty.pump(Duration::from_millis(150));
    }
    assert!(
        persisted.contains("mock/alt"),
        "model did not persist the selected catalog id\n{persisted}"
    );
    oracle(PTY_TEST, "model", "mock/alt");

    // Ask keeps the write parked until Allow once. The file and the gate
    // sentence are the observations; the assistant sentence is not.
    pty.write_bytes(b"\x15");
    pty.type_text("ASK-WRITE please");
    pty.enter();
    let ask_prompt = pty.wait_for("Yes, proceed", Duration::from_secs(20));
    assert!(
        !write_path.exists(),
        "write ran before Allow once\n{ask_prompt}"
    );
    pty.write_bytes(b"1");
    let ask_gate = pty.wait_for("completion gate:", Duration::from_secs(20));
    let asked = std::fs::read_to_string(&write_path).unwrap_or_default();
    assert_eq!(asked, "wrote-marker", "Allow once did not write the marker");
    assert!(
        ask_gate.contains("Assistant text does not count") && ask_gate.contains("side-effect.txt"),
        "Ask turn hid the completion gate\n{ask_gate}"
    );
    oracle(PTY_TEST, "ask-allow-once-gate", "side-effect.txt");

    // yolo lets a write through without a permission key.
    pty.write_bytes(b"\x15");
    pty.type_text("/yolo");
    pty.enter();
    pty.write_bytes(b"\x15");
    let yolo_frame = pty.pump(Duration::from_millis(400));
    assert!(
        yolo_frame.contains("yolo"),
        "info line did not show the yolo flag\n{yolo_frame}"
    );
    assert!(
        !yolo_frame.contains("/yolo"),
        "cleared composer still shows the slash\n{yolo_frame}"
    );
    assert!(
        !yolo_frame.contains("always-approve"),
        "info line still shows always-approve\n{yolo_frame}"
    );
    oracle(PTY_TEST, "yolo-flag", "yolo");
    pty.write_bytes(b"\x15");
    pty.type_text("WRITE-ORACLE please");
    pty.enter();
    let yolo_gate = pty.wait_for("completion gate:", Duration::from_secs(20));
    let wrote = std::fs::read_to_string(&write_path).unwrap_or_default();
    assert_eq!(wrote, "wrote-marker", "yolo did not select AllowOnce");
    assert!(
        yolo_gate.contains("Assistant text does not count")
            && yolo_gate.contains("side-effect.txt"),
        "yolo turn hid the completion gate\n{yolo_gate}"
    );
    oracle(PTY_TEST, "yolo", "wrote-marker completion-gate");

    pty.write_bytes(b"\x15");
    let copy_path = workspace.join("copied-reply.txt");
    pty.type_text(&format!("/copy {}", copy_path.display()));
    pty.enter();
    let copied = wait_file(&copy_path, Duration::from_secs(4));
    assert!(
        copied.contains("ok") || copied.contains("wrote-marker"),
        "copy did not write the assistant reply: {copied:?}"
    );
    oracle(PTY_TEST, "copy", "assistant reply file");

    let editor_marker = workspace.join("editor-ran.txt");
    pty.write_bytes(b"\x15");
    pty.type_text("/edit-prompt");
    pty.enter();
    // The editor starts only after the frame writer drains. Leave the PTY
    // unread and the handoff times out before the script runs.
    let editor_deadline = Instant::now() + Duration::from_secs(8);
    let mut edited = String::new();
    while Instant::now() < editor_deadline {
        pty.pump(Duration::from_millis(50));
        if let Ok(text) = std::fs::read_to_string(&editor_marker) {
            if !text.is_empty() {
                edited = text;
                break;
            }
        }
    }
    assert!(
        edited.contains("edited"),
        "edit-prompt did not launch EDITOR: {edited:?}"
    );
    oracle(PTY_TEST, "edit-prompt", "editor-ran");

    let mouse_at = pty.raw.len();
    pty.write_bytes(b"\x15");
    pty.type_text("/toggle-mouse-reporting");
    pty.enter();
    let mouse_deadline = Instant::now() + Duration::from_secs(3);
    let mut mouse_changed = false;
    while Instant::now() < mouse_deadline {
        pty.pump(Duration::from_millis(100));
        let tail = String::from_utf8_lossy(&pty.raw[mouse_at.min(pty.raw.len())..]);
        if tail.contains("1000l")
            || tail.contains("1002l")
            || tail.contains("1003l")
            || tail.contains("1006l")
            || tail.contains("1000h")
            || tail.contains("1002h")
        {
            mouse_changed = true;
            break;
        }
    }
    assert!(
        mouse_changed,
        "toggle-mouse-reporting did not change mouse capture"
    );
    oracle(PTY_TEST, "toggle-mouse-reporting", "mouse capture changed");

    pty.write_bytes(b"\x15");
    pty.type_text("/home");
    pty.enter();
    let welcome = pty.wait_for("Resume session", Duration::from_secs(6));
    assert!(
        welcome.contains("Resume session"),
        "home did not return to the welcome screen\n{welcome}"
    );
    pty.write_bytes(b"\x15");
    pty.type_text("/session-info");
    pty.enter();
    let home_info = pty.wait_for("No active session", Duration::from_secs(6));
    assert!(
        home_info.contains("No active session") && session_id_from_screen(&home_info) != tour_id,
        "home left session {tour_id} active\n{home_info}"
    );
    oracle(PTY_TEST, "home", "welcome, no active session");

    pty.escape();
    pty.write_bytes(b"\x15");
    pty.type_text("/new");
    pty.enter();
    pty.pump(Duration::from_secs(2));
    pty.write_bytes(b"\x15");
    pty.type_text("/session-info");
    pty.enter();
    let created = pty.wait_for("Session ID", Duration::from_secs(15));
    let created_id = session_id_from_screen(&created);
    assert!(
        !created_id.is_empty(),
        "new did not open a session\n{created}"
    );
    assert_ne!(created_id, tour_id, "new reused the session home just left");
    oracle(PTY_TEST, "new", &format!("session={created_id}"));
    pty.focus_composer();
    pty.type_text("/minimal");
    pty.enter();
    let minimal = pty.wait_for("Switched to minimal mode", Duration::from_secs(6));
    assert!(minimal.contains("Switched to minimal mode"));
    oracle(PTY_TEST, "minimal", "Switched to minimal mode");
    pty.write_bytes(b"\x15");
    pty.type_text("/expand");
    let expand = pty.pump(Duration::from_millis(400));
    assert!(
        palette_offers(&expand, "expand", "fully expanded"),
        "/expand is not offered in minimal mode\n{expand}"
    );
    pty.enter();
    let expanded = pty.wait_for("Nothing to expand", Duration::from_secs(6));
    assert!(
        expanded.contains("Nothing to expand"),
        "/expand did not report its empty state\n{expanded}"
    );
    oracle(PTY_TEST, "expand", "Nothing to expand");
    pty.write_bytes(b"\x15");
    pty.type_text("/fullscreen");
    pty.enter();
    let fullscreen = pty.wait_for("Switched to fullscreen mode", Duration::from_secs(6));
    assert!(fullscreen.contains("Switched to fullscreen mode"));
    oracle(PTY_TEST, "fullscreen", "Switched to fullscreen mode");
    let created_summary = {
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(path) = summary_for(&home, &created_id) {
                break path;
            }
            if Instant::now() >= deadline {
                panic!(
                    "new session {created_id} was not persisted under {}",
                    home.display()
                );
            }
            pty.pump(Duration::from_millis(100));
        }
    };
    pty.write_bytes(b"\x15");
    pty.type_text("/delete");
    pty.enter();
    let question = pty.wait_for("Delete this session permanently?", Duration::from_secs(6));
    assert!(
        question.contains("Delete this session permanently?"),
        "delete did not ask\n{question}"
    );
    pty.pump(Duration::from_millis(200));
    pty.write_bytes(b"1");
    let delete_deadline = Instant::now() + Duration::from_secs(8);
    while created_summary.exists() && Instant::now() < delete_deadline {
        pty.pump(Duration::from_millis(100));
    }
    let delete_screen = pty.screen();
    assert!(
        !created_summary.exists(),
        "delete left {}\n{delete_screen}",
        created_summary.display()
    );
    pty.write_bytes(b"\x15");
    pty.type_text("/session-info");
    pty.enter();
    let deleted = pty.wait_for("No active session", Duration::from_secs(6));
    assert!(
        deleted.contains("No active session") && !deleted.contains(&created_id),
        "delete left session {created_id} active\n{deleted}"
    );
    oracle(PTY_TEST, "delete", &format!("removed {created_id}"));

    pty.write_bytes(b"\x15");
    pty.type_text("/quit");
    pty.enter();
    let status = pty.child.wait().expect("quit");
    assert!(status.success(), "/quit exited {status:?}");
    oracle(PTY_TEST, "quit", "exit 0");
    let _ = args;
    let _ = late_path;
    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_dir_all(&home);
}

const REAL_GOAL_TEST: &str = "shipped_bound_goal_real_llm";
const REAL_GOAL_MODEL: &str = "boyue/deepseek-v4-flash";

/// Explicit host ACL: the operator's live file plus trusted goal bounds.
fn write_real_goal_acl(home: &Path) -> PathBuf {
    let source = std::env::var_os("A3S_CONFIG_FILE").expect("A3S_CONFIG_FILE names the live ACL");
    let mut acl = std::fs::read_to_string(source).expect("live ACL");
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis()
        + 3_600_000;
    acl.push_str(&format!(
        "\ngoal_control {{\n  automatic_enabled = true\n  max_slices = \"4\"\n  \
         max_no_progress_slices = \"2\"\n  deadline_unix_ms = \"{deadline}\"\n}}\n",
    ));
    let path = home.join("goal-host.acl");
    std::fs::write(&path, acl).expect("goal host acl");
    path
}

async fn goal_ext(
    agent: &AgentProc,
    method: &'static str,
    params: serde_json::Value,
) -> serde_json::Value {
    let raw = serde_json::value::to_raw_value(&params).expect("goal params");
    let response = acp::Agent::ext_method(
        agent.conn.as_ref(),
        acp::ExtRequest::new(method, std::sync::Arc::from(raw)),
    )
    .await
    .unwrap_or_else(|error| panic!("{method}: {error:?}"));
    serde_json::from_str(response.0.get()).expect("goal json")
}

async fn goal_state(agent: &AgentProc, id: &acp::SessionId) -> serde_json::Value {
    goal_ext(
        agent,
        "a3s/goal/state",
        serde_json::json!({"session_id": id.0.as_ref()}),
    )
    .await
}

fn goal_command(kind: &str, operation_id: &str, view: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "kind": kind, "operation_id": operation_id,
        "goal_id": view["binding"]["goal_id"], "session_id": view["binding"]["session_id"],
        "workspace": view["binding"]["workspace"],
        "work_policy_digest": view["binding"]["work_policy_digest"],
        "expected_goal_revision": view["binding"]["goal_revision"],
        "expected_plan_revision": view["plan_revision"],
        "expected_goal_sequence": view["goal_sequence"],
    })
}

/// Agent text since `from`, for one session, excluding replay.
fn agent_text(agent: &AgentProc, session: &acp::SessionId, from: usize) -> String {
    agent.replayed.borrow()[from..]
        .iter()
        .filter(|(sid, replay, _)| sid == session.0.as_ref() && !replay)
        .map(|(_, _, text)| text.as_str())
        .collect()
}

async fn slash(agent: &AgentProc, session: &acp::SessionId, text: &str) -> String {
    let from = agent.replayed.borrow().len();
    acp::Agent::prompt(agent.conn.as_ref(), user_prompt(session, text))
        .await
        .unwrap_or_else(|error| panic!("{text}: {error:?}"));
    agent_text(agent, session, from)
}

async fn connect_agent(workspace: &Path, config: &Path, home: &Path) -> AgentProc {
    let agent = spawn_agent(workspace, config, home).await;
    acp::Agent::initialize(
        agent.conn.as_ref(),
        acp::InitializeRequest::new(acp::ProtocolVersion::V1),
    )
    .await
    .expect("initialize");
    agent
}

async fn stop_agent(mut agent: AgentProc) {
    drop(agent.conn);
    let _ = agent.child.start_kill();
    let _ = agent.child.wait().await;
}

fn create_goal_command(
    session: &acp::SessionId,
    workspace: &Path,
    goal_id: &str,
    description: &str,
    criterion: &str,
) -> serde_json::Value {
    let root = workspace.display().to_string();
    serde_json::json!({
        "kind": "create", "operation_id": format!("{goal_id}-create"), "goal_id": goal_id,
        "session_id": session.0.as_ref(), "workspace": {"id": root, "display_root": root},
        "goal": {"description": description, "success_criteria": [criterion],
            "progress": 0.0, "achieved": false, "created_at": 1, "achieved_at": null},
        "non_goals": ["Do not modify release.txt"], "scope_fact_sequences": [],
        "work_policy_digest": null,
        "expected_goal_revision": null, "expected_plan_revision": null,
        "expected_goal_sequence": null,
    })
}

/// A policy session's real Run admits the policy; a goal binds only to that
/// digest, and review admission works on the bound session.
async fn policy_goal_review(agent: &AgentProc, workspace: &Path) {
    let meta = serde_json::json!({"a3sEngineering": {"policy": "engineering-v1"}});
    let id = acp::Agent::new_session(
        agent.conn.as_ref(),
        acp::NewSessionRequest::new(workspace.to_path_buf()).meta(meta.as_object().cloned()),
    )
    .await
    .expect("policy session")
    .session_id;
    let unbound = goal_state(agent, &id).await;
    assert_eq!(unbound["definition"], serde_json::Value::Null);
    assert_eq!(
        unbound["work_policy_digest"],
        serde_json::Value::Null,
        "no Run yet"
    );
    let answer = slash(
        agent,
        &id,
        "Reply with the word ADMITTED and nothing else. Do not use tools.",
    )
    .await;
    assert!(answer.contains("ADMITTED"), "policy turn: {answer}");
    let digest = goal_state(agent, &id).await["work_policy_digest"].clone();
    assert!(
        digest
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:")),
        "the Run admitted the policy: {digest}"
    );
    let mut create = create_goal_command(
        &id,
        workspace,
        "policy-goal",
        "Review release.txt",
        "Findings name release.txt",
    );
    let raw = serde_json::value::to_raw_value(&create).expect("create");
    assert!(
        acp::Agent::ext_method(
            agent.conn.as_ref(),
            acp::ExtRequest::new("a3s/goal/definition", std::sync::Arc::from(raw)),
        )
        .await
        .is_err(),
        "a goal must bind the admitted policy"
    );
    create["work_policy_digest"] = digest.clone();
    let bound = goal_ext(agent, "a3s/goal/definition", create).await;
    assert_eq!(bound["binding"]["work_policy_digest"], digest);
    let target = goal_ext(
        agent,
        "a3s/review/target",
        serde_json::json!({"sessionId": id.0.as_ref(), "paths": ["release.txt"]}),
    )
    .await["digest"]
        .clone();
    let admitted = goal_ext(
        agent,
        "a3s/review/admit",
        serde_json::json!({
            "sessionId": id.0.as_ref(), "scenarioId": "engineering.change",
            "sourceReviewId": "pager-review-1", "workPolicyDigest": digest,
            "target": {"paths": ["release.txt"], "digest": target},
            "scope": {"type": "sessionTarget"},
            "findings": [{"findingId": "f-1", "severity": "warning", "category": "correctness",
                "claim": "The release code has no checksum", "evidence": "release.txt:1",
                "subject": {"type": "opaque", "kind": "engineering.target", "uri": target}}],
        }),
    )
    .await;
    assert_eq!(admitted["admitted"], 1);
    oracle(
        REAL_GOAL_TEST,
        "stdio-policy-goal-review",
        &format!("policy={digest} findings=1"),
    );
}

/// Bind a goal and plan to the pager's session, then status and pause.
async fn bind_pager_session(agent: &AgentProc, workspace: &Path, id: &acp::SessionId) {
    acp::Agent::load_session(
        agent.conn.as_ref(),
        acp::LoadSessionRequest::new(id.clone(), workspace.to_path_buf()),
    )
    .await
    .expect("load the pager session");
    assert_eq!(
        goal_state(agent, id).await["definition"],
        serde_json::Value::Null
    );
    let created = goal_ext(
        agent,
        "a3s/goal/definition",
        create_goal_command(
            id,
            workspace,
            "release-report",
            "Record the release code stored in release.txt in report.txt",
            "report.txt contains exactly the release code read from release.txt",
        ),
    )
    .await;
    // The shipped host verifies a write plus a passing shell check of that
    // path; a read-only step has no host report and is never accepted.
    let mut propose = goal_command("propose_plan", "pager-propose", &created);
    propose["proposal_sequence"] = serde_json::Value::Null;
    propose["plan"] = serde_json::json!({
        "goal": "Record the release code", "complexity": "Simple", "required_tools": [],
        "estimated_steps": 1,
        "steps": [{"id": "report",
                   "content": "Write the exact release code from release.txt (the code only) to a new file report.txt, then run the shell command `test -f report.txt`",
                   "status": "pending", "priority": "medium",
                   "success_criteria": "report.txt holds exactly the release code from release.txt and `test -f report.txt` passed after the write"}],
    });
    let proposed = goal_ext(agent, "a3s/goal/plan", propose).await;
    let mut accept = goal_command("accept_plan", "pager-accept", &proposed);
    accept["plan"] = serde_json::Value::Null;
    accept["proposal_sequence"] = proposed["plan_fact_sequence"].clone();
    let accepted = goal_ext(agent, "a3s/goal/plan", accept).await;
    assert_eq!(accepted["plan_accepted"], true);
    let status = slash(agent, id, "/goal status").await;
    assert!(
        status.contains("Durable goal active")
            && status.contains("Plan r1 accepted: 0/1 steps done")
            && status.contains("Next: advance"),
        "{status}"
    );
    let paused = slash(agent, id, "/goal pause").await;
    assert!(
        paused.contains("Durable goal paused") && paused.contains("Next: paused"),
        "{paused}"
    );
    assert_eq!(goal_state(agent, id).await["definition"]["state"], "paused");
    oracle(
        REAL_GOAL_TEST,
        "stdio-bind-status-pause",
        "create+plan, active 0/1 next advance, paused",
    );
}

#[test]
#[ignore = "requires A3S_CONFIG_FILE with boyue/deepseek-v4-flash and a built a3s-code-tui"]
fn shipped_bound_goal_real_llm() {
    let binary = tui_binary();
    assert!(binary.is_file(), "build a3s-code-tui: {}", binary.display());
    let started = Instant::now();
    let workspace = std::fs::canonicalize(isolate_dir("bound-goal")).expect("workspace");
    let home = isolate_dir("bound-goal-home");
    let code = format!("RC-{}", uuid::Uuid::new_v4().simple());
    std::fs::write(
        workspace.join("release.txt"),
        format!("release code: {code}\n"),
    )
    .expect("release file");
    let config = write_real_goal_acl(&home);
    // A policy session refuses a model switch before its first Run admits the
    // policy, so every agent starts on the model. This ignored test runs alone.
    std::env::set_var("A3S_DEFAULT_MODEL", REAL_GOAL_MODEL);
    let acp_bin = env!("CARGO_BIN_EXE_a3s-code-acp");
    let owned = child_env(acp_bin, &config, &home);
    let env: Vec<(&str, &str)> = owned
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let cwd = workspace.to_str().expect("cwd");

    // The shipped pager creates and registers the session with one real turn.
    let number = std::process::id() % 9000 + 1000;
    let reply = format!("SEED-{number}");
    let mut pty = open_pty(&binary, &["--cwd", cwd], &env, &workspace);
    pty.wait_for_frame(Duration::from_secs(45));
    pty.type_text(&format!(
        "Reply with the word SEED, a hyphen, then the number {number}, and nothing else. Do not use tools."
    ));
    pty.enter();
    pty.wait_for(&reply, Duration::from_secs(180));
    pty.write_bytes(b"\x15");
    pty.type_text("/session-info");
    pty.enter();
    let info = pty.wait_for("Session ID", Duration::from_secs(15));
    let session_id = session_id_from_screen(&info);
    assert!(!session_id.is_empty(), "{info}");
    pty.escape();
    pty.pump(Duration::from_millis(200));
    pty.type_text("/quit");
    pty.enter();
    let _ = pty.child.wait();
    oracle(
        REAL_GOAL_TEST,
        "pager-seed",
        &format!("session={session_id}"),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let local = tokio::task::LocalSet::new();
    let id = acp::SessionId::new(session_id.clone());
    local.block_on(&runtime, async {
        let agent = connect_agent(&workspace, &config, &home).await;
        policy_goal_review(&agent, &workspace).await;
        bind_pager_session(&agent, &workspace, &id).await;
        stop_agent(agent).await;
    });

    // The pager resumes the bound session; `/goal resume` runs one real slice.
    let mut pty = open_pty(
        &binary,
        &["--cwd", cwd, "--resume", &session_id],
        &env,
        &workspace,
    );
    pty.wait_for(&reply, Duration::from_secs(60));
    pty.write_bytes(b"\x15");
    pty.type_text("/goal status");
    pty.enter();
    let status = pty.wait_for("Durable goal paused", Duration::from_secs(30));
    assert!(status.contains("0/1 steps done"), "{status}");
    oracle(REAL_GOAL_TEST, "pager-reopen-status", "paused 0/1");
    let slice_started = Instant::now();
    pty.write_bytes(b"\x15");
    pty.type_text("/goal resume");
    pty.enter();
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut screen = String::new();
    while Instant::now() < deadline {
        screen = pty.pump(Duration::from_millis(500));
        if screen.contains("Durable goal completed") {
            break;
        }
        if screen.contains("Yes, proceed") {
            pty.write_bytes(b"1");
        }
    }
    assert!(screen.contains("Durable goal completed"), "{screen}");
    assert!(screen.contains("1/1 steps done"), "{screen}");
    let report = std::fs::read_to_string(workspace.join("report.txt")).expect("report.txt");
    assert_eq!(report.trim(), code, "{screen}");
    let slice_ms = slice_started.elapsed().as_millis();
    oracle(
        REAL_GOAL_TEST,
        "pager-resume-completes",
        &format!("slice_ms={slice_ms}"),
    );
    pty.write_bytes(b"\x15");
    pty.type_text("/quit");
    pty.enter();
    let _ = pty.child.wait();

    // A new agent process reopens the completed goal: nothing resumes or runs.
    local.block_on(&runtime, async {
        let agent = connect_agent(&workspace, &config, &home).await;
        acp::Agent::load_session(
            agent.conn.as_ref(),
            acp::LoadSessionRequest::new(id.clone(), workspace.clone()),
        )
        .await
        .expect("reopen");
        let before = goal_state(&agent, &id).await;
        assert_eq!(before["definition"]["state"], "completed");
        assert_eq!(before["definition"]["plan"]["steps"][0]["status"], "completed");
        let resumed = slash(&agent, &id, "/goal resume").await;
        assert!(
            resumed.contains("Durable goal completed") && !resumed.contains("Goal slice"),
            "{resumed}"
        );
        assert_eq!(
            goal_state(&agent, &id).await,
            before,
            "a completed goal records nothing"
        );
        let info = session_info_body(agent.conn.as_ref(), &id).await;
        oracle(REAL_GOAL_TEST, "reopen-completed", "no slice, no fact");

        // Clear on the shipped binary cancels an unstarted goal.
        let other = acp::Agent::new_session(
            agent.conn.as_ref(),
            acp::NewSessionRequest::new(workspace.clone()),
        )
        .await
        .expect("second session")
        .session_id;
        let created = goal_ext(
            &agent,
            "a3s/goal/definition",
            create_goal_command(&other, &workspace, "clear-me", "Never started", "Cancelled"),
        )
        .await;
        assert_eq!(created["state"], "active");
        let cleared = slash(&agent, &other, "/goal clear").await;
        assert!(cleared.contains("Durable goal cancelled"), "{cleared}");
        oracle(REAL_GOAL_TEST, "stdio-clear", "active→cancelled");
        eprintln!(
            "BOUND-GOAL-REAL model={REAL_GOAL_MODEL} total_ms={} slice_ms={slice_ms} session_info={info}",
            started.elapsed().as_millis()
        );
        stop_agent(agent).await;
    });
    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_dir_all(&home);
}
