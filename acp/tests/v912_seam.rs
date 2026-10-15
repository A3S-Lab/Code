//! v9.1.2 mechanisms through the shipped `a3s-code-acp` binary and core log.
//!
//! The agent process is `CARGO_BIN_EXE_a3s-code-acp`. The model is a local
//! OpenAI-compatible server. Nothing here reimplements the agent.

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
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x05, 0xFE, 0x02, 0xFE, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];
const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVQI12P4z8AAAAADAAEABf4C/gAAAABJRU5ErkJggg==";
const STANDING_LINE: &str = "STANDING-LINE-9f3a";
const STANDING: &str = "# Instructions (personal + project AGENTS.md chain)\n\nPersonal: never print the token named CARRY-PERSONAL-7f3a.\n\n--- instruction-doc ---\n\nProject: format Rust with cargo fmt before finishing CARRY-PROJECT-7f3a.";
const CONSTRAINTS: &str = "Must: prefer the example-csv crate.\nRevoke: prefer the example-csv crate.\nMust not: add a dependency to parse CSV.";
const REVISION: &str = "Revision: the active task is a JSON exporter in crates/export.";

struct MockState {
    bodies: Vec<String>,
    writes: u64,
    reads: u64,
    plans: u64,
    denies: u64,
    gits: u64,
    write_path: String,
    deny_command: String,
}

struct AgentProc {
    child: tokio::process::Child,
    conn: Rc<acp::ClientSideConnection>,
}

impl Drop for AgentProc {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn isolate(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "a3s-v912-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn write_acl(workspace: &Path, base_url: &str) -> PathBuf {
    let dir = workspace.join(".a3s");
    std::fs::create_dir_all(&dir).expect("acl dir");
    let path = dir.join("config.acl");
    std::fs::write(
        &path,
        format!(
            r#"
providers "mock" {{
  base_url = "{base_url}"
  api_key = "test"
  models "gpt" {{
  }}
}}
default_model = "{MODEL_ID}"
"#
        ),
    )
    .expect("acl");
    path
}

fn spawn_mock(write_path: &Path, deny_command: &str) -> (SocketAddr, Arc<Mutex<MockState>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let state = Arc::new(Mutex::new(MockState {
        bodies: Vec::new(),
        writes: 0,
        reads: 0,
        plans: 0,
        denies: 0,
        gits: 0,
        write_path: write_path.display().to_string(),
        deny_command: deny_command.to_string(),
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

fn has_tools(body: &serde_json::Value) -> bool {
    body.get("tools")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
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
    let chunk = serde_json::json!({ "choices": [{ "delta": { "content": text } }] });
    let done = serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] });
    format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

fn reply_tool(name: &str, args: &str, seq: u64, streaming: bool) -> String {
    let id = format!("call_{name}_{seq}");
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
    let done = serde_json::json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] });
    format!("data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n")
}

fn decide(raw: &str, state: &mut MockState) -> String {
    let parsed: serde_json::Value = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
    let streaming = parsed
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if raw.contains("context-compaction engine") {
        return reply_text(
            "The task is finished. All required steps are done. Goal: Add a CSV parser in crates/import. No constraints remain.",
            streaming,
        );
    }
    if raw.contains("You extract durable") {
        return reply_text("{\"items\":[]}", false);
    }
    let tools = has_tools(&parsed);
    if raw.contains("WRITE-GATE") && tools {
        if state.writes >= 1 {
            return reply_text("ok", streaming);
        }
        state.writes += 1;
        let args = serde_json::json!({
            "file_path": state.write_path,
            "content": "hello"
        })
        .to_string();
        return reply_tool("write", &args, state.writes, streaming);
    }
    if raw.contains("DENY-ORACLE") && tools {
        if state.denies >= 1 {
            return reply_text("blocked", streaming);
        }
        state.denies += 1;
        let args = serde_json::json!({ "command": state.deny_command }).to_string();
        return reply_tool("bash", &args, state.denies, streaming);
    }
    if raw.contains("READ-ORACLE") && tools {
        if state.reads >= 1 {
            return reply_text("saw the image", streaming);
        }
        state.reads += 1;
        let args = serde_json::json!({ "file_path": "shot.png" }).to_string();
        return reply_tool("read", &args, state.reads, streaming);
    }
    if raw.contains("READONLY-GIT") && tools {
        if state.gits >= 1 {
            return reply_text("noted", streaming);
        }
        state.gits += 1;
        let command = r#"sleep 0.05
git -c core.quotepath=false status --porcelain >/dev/null
git ls-files >/dev/null
i=0
seen=0
while [ "$i" -lt 40 ]; do
  if git cat-file -e HEAD:guest.txt 2>/dev/null; then
    seen=1
    break
  fi
  i=$((i + 1))
  sleep 0.1
done
git log -1 --oneline >/dev/null
git check-ignore -q -- guest.txt || true
if [ "$seen" = 1 ]; then
  printf 'READONLY-SEEN\n'
  exit 0
fi
exit 1
"#;
        let args = serde_json::json!({ "command": command }).to_string();
        return reply_tool("bash", &args, state.gits, streaming);
    }
    if raw.contains("PLAN-ORACLE") && tools {
        if state.plans >= 1 {
            return reply_text("plan stored", streaming);
        }
        state.plans += 1;
        let args = serde_json::json!({
            "plan": [
                {"id": "schema", "step": "read the export schema", "status": "completed"},
                {"id": "write", "step": "write the JSON exporter", "status": "pending"}
            ]
        })
        .to_string();
        return reply_tool("update_plan", &args, state.plans, streaming);
    }
    reply_text("noted", streaming)
}

struct TestClient;

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

    async fn session_notification(
        &self,
        _args: acp::SessionNotification,
    ) -> Result<(), acp::Error> {
        Ok(())
    }
}

async fn spawn_agent(workspace: &Path, config: &Path, home: &Path) -> AgentProc {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_a3s-code-acp"));
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
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn a3s-code-acp");
    let stdout = child.stdout.take().expect("stdout").compat();
    let stdin = child.stdin.take().expect("stdin").compat_write();
    let (conn, io) = acp::ClientSideConnection::new(TestClient, stdin, stdout, |fut| {
        tokio::task::spawn_local(fut);
    });
    tokio::task::spawn_local(async move {
        let _ = io.await;
    });
    AgentProc {
        child,
        conn: Rc::new(conn),
    }
}

async fn open_session(
    workspace: &Path,
    deny_command: &str,
) -> (AgentProc, acp::SessionId, Arc<Mutex<MockState>>) {
    let home = isolate("home");
    let write_path = workspace.join("gate-target.ts");
    let (addr, state) = spawn_mock(&write_path, deny_command);
    let config = write_acl(workspace, &format!("http://{addr}/v1"));
    let agent = spawn_agent(workspace, &config, &home).await;
    acp::Agent::initialize(
        agent.conn.as_ref(),
        acp::InitializeRequest::new(acp::ProtocolVersion::V1),
    )
    .await
    .expect("initialize");
    let created = acp::Agent::new_session(
        agent.conn.as_ref(),
        acp::NewSessionRequest::new(workspace.to_path_buf()),
    )
    .await
    .expect("new session");
    (agent, created.session_id, state)
}

fn prompt(session: &acp::SessionId, text: &str) -> acp::PromptRequest {
    acp::PromptRequest::new(
        session.clone(),
        vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
    )
}

async fn say(
    agent: &AgentProc,
    session: &acp::SessionId,
    text: &str,
) -> acp::Result<acp::PromptResponse> {
    tokio::time::timeout(
        Duration::from_secs(40),
        acp::Agent::prompt(agent.conn.as_ref(), prompt(session, text)),
    )
    .await
    .expect("prompt timed out")
}

async fn ext(agent: &AgentProc, method: &str, body: serde_json::Value) -> serde_json::Value {
    let params = serde_json::value::to_raw_value(&body).expect("ext params");
    let response = tokio::time::timeout(
        Duration::from_secs(60),
        acp::Agent::ext_method(
            agent.conn.as_ref(),
            acp::ExtRequest::new(method, std::sync::Arc::from(params)),
        ),
    )
    .await
    .expect("ext timed out")
    .expect(method);
    serde_json::from_str(response.0.get()).expect("ext json")
}

async fn ext_err(agent: &AgentProc, method: &str, body: serde_json::Value) -> String {
    let params = serde_json::value::to_raw_value(&body).expect("ext params");
    let response = tokio::time::timeout(
        Duration::from_secs(60),
        acp::Agent::ext_method(
            agent.conn.as_ref(),
            acp::ExtRequest::new(method, std::sync::Arc::from(params)),
        ),
    )
    .await
    .expect("ext timed out");
    match response {
        Ok(value) => panic!("expected {method} to fail, got {value:?}"),
        Err(error) => error
            .data
            .as_ref()
            .map(|value| value.to_string())
            .unwrap_or_default(),
    }
}

fn effect_log(workspace: &Path) -> String {
    let mut out = String::new();
    let mut append_dir = |dir: &Path| {
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
    };
    append_dir(&workspace.join(".a3s").join("effect-log"));
    if let Ok(dates) = std::fs::read_dir(workspace.join(".a3s").join("sessions")) {
        for date in dates.flatten() {
            let Ok(homes) = std::fs::read_dir(date.path()) else {
                continue;
            };
            for home in homes.flatten() {
                append_dir(&home.path());
            }
        }
    }
    out
}

async fn set_mode(agent: &AgentProc, session: &acp::SessionId, mode: &str) {
    acp::Agent::set_session_mode(
        agent.conn.as_ref(),
        acp::SetSessionModeRequest::new(session.clone(), acp::SessionModeId::new(mode.to_string())),
    )
    .await
    .expect("set mode");
}

#[tokio::test(flavor = "current_thread")]
async fn v912_session_posture_and_deny() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("posture");
            let marker = workspace.join("denied-marker.txt");
            let command = format!("printf executed > '{}'", marker.display());
            let (agent, session, _state) = open_session(&workspace, &command).await;
            assert_ne!(session.0.as_ref(), "tui-session");
            assert!(!session.0.as_ref().is_empty());

            let fresh = ext(
                &agent,
                "x.ai/session/info",
                serde_json::json!({ "sessionId": session.0.as_ref() }),
            )
            .await;
            assert_eq!(
                fresh["result"]["permissionPosture"].as_str(),
                Some("default"),
                "fresh session did not start on default: {fresh}"
            );
            set_mode(&agent, &session, "always-approve").await;
            set_mode(&agent, &session, "force").await;
            let still_default = ext(
                &agent,
                "x.ai/session/info",
                serde_json::json!({ "sessionId": session.0.as_ref() }),
            )
            .await;
            assert_eq!(
                still_default["result"]["permissionPosture"].as_str(),
                Some("default"),
                "retired posture names changed the session: {still_default}"
            );
            for retired in ["always-approve", "force"] {
                let detail = ext_err(
                    &agent,
                    "x.ai/permission_posture",
                    serde_json::json!({
                        "sessionId": session.0.as_ref(),
                        "posture": retired
                    }),
                )
                .await;
                assert!(
                    detail.contains("unknown posture"),
                    "{retired} was accepted: {detail}"
                );
                assert!(
                    detail.contains(retired),
                    "{retired} missing from the error: {detail}"
                );
                eprintln!("ORACLE rejected-posture={retired} error={detail}");
            }
            let after_reject = ext(
                &agent,
                "x.ai/session/info",
                serde_json::json!({ "sessionId": session.0.as_ref() }),
            )
            .await;
            assert_eq!(
                after_reject["result"]["permissionPosture"].as_str(),
                Some("default"),
                "rejected posture changed the session: {after_reject}"
            );
            eprintln!(
                "ORACLE permissionPosture={} unknown-posture-rejected=always-approve,force",
                after_reject["result"]["permissionPosture"]
            );

            for mode in ["default", "plan", "auto", "yolo"] {
                set_mode(&agent, &session, mode).await;
                let info = ext(
                    &agent,
                    "x.ai/session/info",
                    serde_json::json!({ "sessionId": session.0.as_ref() }),
                )
                .await;
                assert_eq!(
                    info["result"]["permissionPosture"].as_str(),
                    Some(mode),
                    "session info did not admit {mode}: {info}"
                );
                if mode == "plan" || mode == "auto" {
                    eprintln!("ORACLE permissionPosture={mode}");
                }
            }

            let admitted = ext(
                &agent,
                "x.ai/permission_posture",
                serde_json::json!({
                    "sessionId": session.0.as_ref(),
                    "posture": "yolo",
                    "deny": "bash(*)"
                }),
            )
            .await;
            assert_eq!(
                admitted["result"]["permissionPosture"].as_str(),
                Some("yolo")
            );

            let _ = say(&agent, &session, "DENY-ORACLE do not run the shell").await;
            assert!(
                !marker.exists(),
                "yolo deny still executed {}",
                marker.display()
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn v912_standing_context_before_model() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("standing");
            std::fs::write(
                workspace.join("instructions.md"),
                format!("# Instructions\n\n{STANDING_LINE}\n"),
            )
            .expect("instructions");
            let (agent, session, state) = open_session(&workspace, "true").await;
            let _ = say(&agent, &session, "hello from the standing check").await;
            let bodies = state.lock().expect("mock").bodies.clone();
            assert!(!bodies.is_empty(), "the model was never called");
            let log = effect_log(&workspace);
            assert!(
                log.contains("a3s.standing-context"),
                "standing context was not admitted before the model call: {log}"
            );
            let mut standing_seq: Option<u64> = None;
            let mut user_seq: Option<u64> = None;
            for line in log.lines() {
                let Ok(fact) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                let kind = fact
                    .get("type")
                    .or_else(|| fact.get("kind"))
                    .and_then(|value| value.as_str());
                let seq = fact.get("seq").and_then(|value| value.as_u64()).unwrap_or(u64::MAX);
                if kind == Some("user.message") {
                    user_seq = Some(match user_seq {
                        Some(prior) => prior.min(seq),
                        None => seq,
                    });
                }
                if kind != Some("a3s.standing-context") {
                    continue;
                }
                let payload = &fact["payload"];
                let digest = payload.get("digest").and_then(|value| value.as_str()).unwrap_or("");
                let hex = digest
                    .strip_prefix("sha256:")
                    .unwrap_or_else(|| panic!("standing digest is not sha-256: {digest}"));
                assert_eq!(hex.len(), 64, "standing digest is not sha-256: {digest}");
                assert!(
                    hex.bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')),
                    "standing digest is not lowercase sha-256: {digest}"
                );
                assert!(
                    !payload.to_string().contains(STANDING_LINE),
                    "standing fact stored the instruction text"
                );
                standing_seq = Some(match standing_seq {
                    Some(prior) => prior.min(seq),
                    None => seq,
                });
            }
            let standing_seq = standing_seq.expect("no standing-context fact");
            let user_seq = user_seq.expect("no user.message fact");
            assert!(
                standing_seq < user_seq,
                "standing context seq {standing_seq} was not admitted before the user turn {user_seq}"
            );
            eprintln!(
                "ORACLE standing-context seq={standing_seq} before user.message seq={user_seq}"
            );
            assert!(
                bodies.iter().any(|body| body.contains(STANDING_LINE)),
                "the model request did not receive the instruction text"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn v912_compact_keeps_obligations() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("compact");
            let (agent, session, _state) = open_session(&workspace, "true").await;
            for text in [
                STANDING,
                "Add a CSV parser in crates/import.",
                CONSTRAINTS,
                REVISION,
                "PLAN-ORACLE record the export plan",
                "status note 0",
                "status note 1",
                "status note 2",
                "status note 3",
            ] {
                say(&agent, &session, text)
                    .await
                    .unwrap_or_else(|error| panic!("prompt failed: {error:?}"));
            }
            let compacted = ext(
                &agent,
                "x.ai/compact_conversation",
                serde_json::json!({ "sessionId": session.0.as_ref() }),
            )
            .await;
            assert!(
                compacted.get("error").is_none(),
                "compact failed: {compacted}"
            );
            let log = effect_log(&workspace);
            let summary = log
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .filter(|fact| {
                    fact.get("type")
                        .or_else(|| fact.get("kind"))
                        .and_then(|value| value.as_str())
                        == Some("compaction.done")
                })
                .filter_map(|fact| {
                    fact.get("payload")
                        .and_then(|payload| payload.get("summary"))
                        .and_then(|summary| summary.as_str())
                        .map(str::to_string)
                })
                .next_back()
                .expect("compaction.done summary");
            assert!(summary.contains("# Instructions (personal + project AGENTS.md chain)"));
            assert!(summary.contains("CARRY-PERSONAL-7f3a"));
            assert!(summary.contains("CARRY-PROJECT-7f3a"));
            assert!(summary.contains("the active task is a JSON exporter in crates/export."));
            assert!(summary.contains("Must not: add a dependency to parse CSV."));
            assert!(summary.contains("## Open steps"));
            assert!(summary.contains("write the JSON exporter"));
            let goal = summary
                .split("## Goal\n")
                .nth(1)
                .unwrap_or("")
                .split("\n## ")
                .next()
                .unwrap_or("");
            assert!(
                goal.contains("JSON exporter"),
                "active goal was replaced: {goal}"
            );
            assert!(
                !goal.contains("CSV parser"),
                "the revised goal was dropped for the earlier task: {goal}"
            );
            eprintln!("ORACLE compaction-summary={summary}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn v912_image_bytes_reach_the_model() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("image");
            std::fs::write(workspace.join("shot.png"), PNG).expect("png");
            let (agent, session, state) = open_session(&workspace, "true").await;
            say(&agent, &session, "READ-ORACLE shot.png")
                .await
                .expect("read prompt");
            let bodies = state.lock().expect("mock").bodies.clone();
            assert!(
                bodies.iter().any(|body| body.contains(PNG_BASE64)),
                "model request did not contain the image bytes"
            );
            eprintln!("ORACLE image-bytes model-body-contains=PNG_BASE64");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn v912_remember_before_stream_close() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("remember");
            let (agent, session, _state) = open_session(&workspace, "true").await;
            const TOKEN: &str = "REMEMBER-TOKEN-4411";
            say(
                &agent,
                &session,
                &format!(
                    "Please remember this durable workspace preference and codename {TOKEN} for the export workflow."
                ),
            )
            .await
            .expect("remember prompt");
            let memory_dir = workspace.join(".a3s").join("memory");
            assert!(
                explicit_preference_stored(&memory_dir, TOKEN),
                "explicit preference was not durable under {} at End; effect log has the token: {}",
                memory_dir.display(),
                effect_log(&workspace).contains(TOKEN)
            );
            eprintln!("ORACLE remember token={TOKEN} kind=explicit-preference stored-before-return");
        })
        .await;
}

fn explicit_preference_stored(memory_dir: &Path, token: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(memory_dir.join("items")) else {
        return false;
    };
    entries.flatten().any(|entry| {
        std::fs::read_to_string(entry.path())
            .is_ok_and(|text| text.contains(token) && text.contains("explicit-preference"))
    })
}

#[test]
fn v912_cursor_replays_before_new_chain_digest() {
    use a3s_code_core::{
        AgentEvent, CapabilityStamp, CoreEventLog, OperationId, RunEventRecord, SourceRevision,
    };

    let log = CoreEventLog::new();
    let operation = OperationId::new("session-1/run-1").unwrap();
    let revision = SourceRevision::new(7);
    let stamp = CapabilityStamp::new(3, format!("sha256:{}", "a".repeat(64))).unwrap();
    let record = RunEventRecord {
        sequence: 0,
        timestamp_ms: 10,
        event: AgentEvent::TurnStart { turn: 1 },
    };
    let appended = log
        .append_run_event(operation.clone(), revision, Some(stamp.clone()), &record)
        .unwrap();
    assert!(appended.appended && !appended.replayed);
    let page = log.page(&operation, None, usize::MAX).unwrap().unwrap();
    assert_eq!(page.entries.len(), 1);
    let digest = page.entries[0].entry_digest.clone();

    let replayed = log
        .append_run_event(operation.clone(), revision, Some(stamp.clone()), &record)
        .unwrap();
    assert!(!replayed.appended && replayed.replayed);
    let page = log.page(&operation, None, usize::MAX).unwrap().unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].entry_digest, digest);

    let next = RunEventRecord {
        sequence: 1,
        timestamp_ms: 11,
        event: AgentEvent::TurnStart { turn: 2 },
    };
    let chained = log
        .append_run_event(operation.clone(), revision, Some(stamp), &next)
        .unwrap();
    assert!(chained.appended && !chained.replayed);
    let page = log.page(&operation, None, usize::MAX).unwrap().unwrap();
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[1].previous_digest, digest);
    assert_ne!(page.entries[1].entry_digest, digest);
    eprintln!(
        "ORACLE cursor replayed={} appended={} previous_digest={} entry_digest={}",
        replayed.replayed,
        chained.appended,
        page.entries[1].previous_digest,
        page.entries[1].entry_digest
    );
}

#[tokio::test(flavor = "current_thread")]
async fn v912_completion_gate_keeps_path() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("gate");
            let (agent, session, _state) = open_session(&workspace, "true").await;
            set_mode(&agent, &session, "yolo").await;
            let info = ext(
                &agent,
                "x.ai/session/info",
                serde_json::json!({ "sessionId": session.0.as_ref() }),
            )
            .await;
            assert_eq!(
                info["result"]["permissionPosture"].as_str(),
                Some("yolo"),
                "yolo was not admitted before the write: {info}"
            );
            let wrote = say(&agent, &session, "WRITE-GATE gate-target.ts").await;
            let detail = match wrote {
                Ok(response) => panic!("unverified write completed: {response:?}"),
                Err(error) => error
                    .data
                    .as_ref()
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_string(),
            };
            assert!(
                detail.contains("completion gate:"),
                "gate sentence missing: {detail}"
            );
            assert!(
                detail.contains("gate-target.ts"),
                "mutated path missing: {detail}"
            );
            assert!(
                detail.contains("Assistant text does not count"),
                "kernel sentence was replaced: {detail}"
            );
            assert!(
                !detail.contains("Try sending again"),
                "pager retry replaced the kernel sentence: {detail}"
            );
            assert!(
                detail.contains("completion gate: workspace mutation"),
                "kernel sentence missing: {detail}"
            );
            assert!(
                detail.contains("Assistant text does not count"),
                "assistant text was allowed to close the gate: {detail}"
            );
            eprintln!("ORACLE yolo-gate-error={detail}");
        })
        .await;
}

fn git_at(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?} failed to spawn: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_init(root: &Path) {
    git_at(root, &["init", "-b", "main"]);
    git_at(root, &["config", "user.email", "test@example.com"]);
    git_at(root, &["config", "user.name", "test"]);
    git_at(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("README.md"), "base\n").expect("readme");
    git_at(root, &["add", "README.md"]);
    git_at(root, &["commit", "-m", "base"]);
}

fn changed_paths_in_log(log: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in log.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        collect_changed_paths(&value, &mut paths);
    }
    paths
}

fn collect_changed_paths(value: &serde_json::Value, paths: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(entries) = map.get("changed_paths").and_then(|entry| entry.as_array()) {
                for entry in entries {
                    if let Some(path) = entry.as_str() {
                        paths.push(path.to_string());
                    }
                }
            }
            for child in map.values() {
                collect_changed_paths(child, paths);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_changed_paths(item, paths);
            }
        }
        _ => {}
    }
}

#[tokio::test(flavor = "current_thread")]
async fn v912_readonly_git_parallel_commit_stays_ok() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("readonly-git");
            git_init(&workspace);
            let (agent, session, _state) = open_session(&workspace, "true").await;
            let commit_root = workspace.clone();
            let committer = tokio::task::spawn_blocking(move || {
                std::thread::sleep(Duration::from_millis(300));
                std::fs::write(commit_root.join("guest.txt"), "foreign\n").expect("guest");
                git_at(&commit_root, &["add", "guest.txt"]);
                git_at(&commit_root, &["commit", "-m", "foreign"]);
            });
            let response = say(&agent, &session, "READONLY-GIT inspect the tree").await;
            committer.await.expect("parallel commit task");
            response.expect("read-only git turn failed on the parallel commit");
            let log = effect_log(&workspace);
            assert!(
                log.contains("READONLY-SEEN"),
                "bash did not observe the parallel commit: {log}"
            );
            assert!(
                !log.contains("completion gate:"),
                "read-only git failed the turn: {log}"
            );
            let paths = changed_paths_in_log(&log);
            assert!(
                !paths.iter().any(|path| path == "guest.txt"),
                "foreign commit entered the mutation observations: {paths:?}"
            );
            git_at(&workspace, &["cat-file", "-e", "HEAD:guest.txt"]);
            eprintln!("ORACLE readonly-git prompt=ok foreign-paths={paths:?} seen=READONLY-SEEN");
        })
        .await;
}
