//! `a3s-code-acp --unattended` through the shipped binary against a local
//! OpenAI-compatible model. The same task in an interactive launch is the pin:
//! no host bind, and harness state stays where it always was.

use std::cell::Cell;
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

#[derive(Default)]
struct Script {
    bodies: Vec<String>,
    asked: bool,
    searches: usize,
    written: bool,
    verified: bool,
    bashed: bool,
    skilled: bool,
    tool_seq: u64,
}

/// Searches the unattended model makes before writing: the bm25 query rides
/// the persistent `a3s-vec` index, the grep one the trigram candidates.
const SEARCHES: [(&str, &str); 2] = [("bm25", "answer notes"), ("grep", "UNIVERSE")];

/// A SLOW-TASK parent request answers with text after this long, so every
/// attempt ends without tools and the host keeps continuing.
const SLOW_REPLY: Duration = Duration::from_millis(3500);

struct AgentProc {
    child: tokio::process::Child,
    conn: Rc<acp::ClientSideConnection>,
    permission_asks: Rc<Cell<u32>>,
}

impl Drop for AgentProc {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn isolate(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "a3s-unattended-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

fn write_acl(dir: &Path, base_url: &str) -> PathBuf {
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

fn spawn_mock() -> (SocketAddr, Arc<Mutex<Script>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let script = Arc::new(Mutex::new(Script::default()));
    let shared = Arc::clone(&script);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
                if let Some(body) = read_http_body(&mut stream) {
                    let response = {
                        let mut guard = shared.lock().expect("mock");
                        guard.bodies.push(body.clone());
                        decide(&body, &mut guard)
                    };
                    if body.contains("SLOW-TASK") && body.contains("\"write\"") {
                        std::thread::sleep(SLOW_REPLY);
                    }
                    let _ = write_http(&mut stream, &response);
                }
            });
        }
    });
    (addr, script)
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

fn reply_tool(name: &str, args: serde_json::Value, seq: u64, streaming: bool) -> String {
    let id = format!("call_{name}_{seq}");
    let args = args.to_string();
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

/// A model that asks once, writes once, then checks with a read-only tool
/// and stops whenever the last message is a tool result.
fn decide(raw: &str, script: &mut Script) -> String {
    let parsed: serde_json::Value = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
    let streaming = parsed["stream"].as_bool().unwrap_or(false);
    if raw.contains("context-compaction engine") {
        return reply_text(
            "Goal: write answer.txt for UNATTENDED-TASK. The file exists. No check proved the task.",
            streaming,
        );
    }
    if raw.contains("You extract durable") {
        return reply_text("{\"items\":[]}", false);
    }
    let tool_names: Vec<&str> = parsed["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect();
    let judgment = serde_json::json!({"achieved": script.written && script.verified,
        "progress": if script.verified { 1.0 } else { 0.0 }, "remaining_criteria": []});
    if tool_names.contains(&"emit_goal_achievement") {
        script.tool_seq += 1;
        return reply_tool(
            "emit_goal_achievement",
            judgment,
            script.tool_seq,
            streaming,
        );
    }
    if parsed["response_format"]["json_schema"]["name"] == "goal_achievement" {
        return reply_text(&judgment.to_string(), streaming);
    }
    if parsed["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["role"] == "system"
                && message["content"].as_str().is_some_and(|text| {
                    text.starts_with("You are an evaluation assistant. Given a goal with success criteria and the current state, evaluate progress.")
                })
        })
    }) {
        return reply_text(&judgment.to_string(), streaming);
    }
    if !raw.contains("UNATTENDED-TASK") || tool_names.is_empty() {
        return reply_text("ok", streaming);
    }
    // The read-only explorer automatic delegation starts before the parent.
    if !tool_names.contains(&"write") {
        return reply_text("explored: notes.md holds the answer", streaming);
    }
    let last_role = parsed["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["role"].as_str())
        .unwrap_or("");
    if last_role == "user" {
        script.verified = false;
    }
    if raw.contains("SLOW-TASK") {
        return reply_text("still thinking", streaming);
    }
    script.tool_seq += 1;
    let seq = script.tool_seq;
    if !script.asked && raw.contains("ASK-FIRST") {
        script.asked = true;
        return reply_tool(
            "ask_user",
            serde_json::json!({ "question": "Which file name should I use?" }),
            seq,
            streaming,
        );
    }
    if raw.contains("SEARCH-FIRST") && script.searches < SEARCHES.len() {
        let (mode, query) = SEARCHES[script.searches];
        script.searches += 1;
        return reply_tool(
            "search",
            serde_json::json!({ "mode": mode, "query": query }),
            seq,
            streaming,
        );
    }
    if raw.contains("MISSING-SKILL-FIRST") && !script.skilled {
        script.skilled = true;
        return reply_tool(
            "Skill",
            serde_json::json!({ "skill_name": "not-installed" }),
            seq,
            streaming,
        );
    }
    if raw.contains("BASH-FIRST") && !script.bashed {
        script.bashed = true;
        return reply_tool(
            "bash",
            serde_json::json!({ "command": "echo BASH-RAN" }),
            seq,
            streaming,
        );
    }
    if !script.written {
        script.written = true;
        return reply_tool(
            "write",
            serde_json::json!({ "file_path": "answer.txt", "content": "42\n" }),
            seq,
            streaming,
        );
    }
    if !script.verified && raw.contains("Execute only the current admitted step `task`") {
        script.verified = true;
        return reply_tool(
            "bash",
            serde_json::json!({
                "command": "test -f answer.txt && test \"$(cat answer.txt)\" = 42",
                "sandbox_permissions": "require_escalated",
                "justification": "Verify the private fixture through approved host execution"
            }),
            seq,
            streaming,
        );
    }
    if last_role == "tool" {
        return reply_text("verified", streaming);
    }
    reply_tool(
        "read",
        serde_json::json!({ "file_path": "answer.txt" }),
        seq,
        streaming,
    )
}

struct TestClient {
    permission_asks: Rc<Cell<u32>>,
}

#[async_trait(?Send)]
impl acp::Client for TestClient {
    async fn request_permission(
        &self,
        _args: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, acp::Error> {
        self.permission_asks.set(self.permission_asks.get() + 1);
        Ok(acp::RequestPermissionResponse::new(
            acp::RequestPermissionOutcome::Cancelled,
        ))
    }

    async fn session_notification(
        &self,
        _args: acp::SessionNotification,
    ) -> Result<(), acp::Error> {
        Ok(())
    }
}

async fn spawn_agent(
    workspace: &Path,
    config: &Path,
    home: &Path,
    tmp: &Path,
    extra: &[&str],
) -> AgentProc {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_a3s-code-acp"));
    let mut child = tokio::process::Command::new(bin)
        .args(extra)
        .arg("--cwd")
        .arg(workspace)
        .env("A3S_CONFIG", config)
        .env("A3S_DISABLE_CC_SWITCH", "1")
        .env("A3S_DISABLE_GROK_ACCOUNT", "1")
        .env("A3S_DISABLE_CURSOR_ACCOUNT", "1")
        .env("XAI_API_KEY", "sk-dummy-not-a-model")
        .env("HOME", home)
        .env("GROK_HOME", home.join("grok"))
        .env("TMPDIR", tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn a3s-code-acp");
    let stdout = child.stdout.take().expect("stdout").compat();
    let stdin = child.stdin.take().expect("stdin").compat_write();
    let permission_asks = Rc::new(Cell::new(0));
    let client = TestClient {
        permission_asks: Rc::clone(&permission_asks),
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
        permission_asks,
    }
}

async fn open(agent: &AgentProc, workspace: &Path) -> acp::SessionId {
    acp::Agent::initialize(
        agent.conn.as_ref(),
        acp::InitializeRequest::new(acp::ProtocolVersion::V1),
    )
    .await
    .expect("initialize");
    acp::Agent::new_session(
        agent.conn.as_ref(),
        acp::NewSessionRequest::new(workspace.to_path_buf()),
    )
    .await
    .expect("new session")
    .session_id
}

async fn say(
    agent: &AgentProc,
    session: &acp::SessionId,
    text: &str,
) -> acp::Result<acp::PromptResponse> {
    let request = acp::PromptRequest::new(
        session.clone(),
        vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
    );
    tokio::time::timeout(
        Duration::from_secs(180),
        acp::Agent::prompt(agent.conn.as_ref(), request),
    )
    .await
    .expect("prompt timed out")
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[tokio::test(flavor = "current_thread")]
async fn unattended_prompt_binds_proves_and_records_without_touching_the_workspace() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("ws");
            std::fs::write(
                workspace.join("notes.md"),
                "The answer to life, the UNIVERSE and everything.\n",
            )
            .expect("seed notes");
            let outside = isolate("outside");
            let home = isolate("home");
            let tmp = isolate("tmp");
            let (addr, script) = spawn_mock();
            let config = write_acl(&outside, &format!("http://{addr}/v1"));
            let result_file = outside.join("logs").join("a3s-code.result.json");
            let result_arg = result_file.display().to_string();
            let agent = spawn_agent(
                &workspace,
                &config,
                &home,
                &tmp,
                &[
                    "--unattended",
                    "--no-goal-continuation",
                    "--max-execution-ms",
                    "170000",
                    "--result-file",
                    &result_arg,
                ],
            )
            .await;
            let session = open(&agent, &workspace).await;

            let response = say(
                &agent,
                &session,
                "ASK-FIRST SEARCH-FIRST UNATTENDED-TASK write answer.txt",
            )
            .await
            .expect("unattended prompt");
            assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
            assert_eq!(agent.permission_asks.get(), 0, "nobody may be asked");

            let bodies = script.lock().expect("mock").bodies.clone();
            let task_bodies: Vec<&String> = bodies
                .iter()
                .filter(|body| body.contains("UNATTENDED-TASK"))
                .collect();
            let mut tools = std::collections::BTreeSet::new();
            for body in &task_bodies {
                let request: serde_json::Value = serde_json::from_str(body).expect("request json");
                for tool in request["tools"].as_array().into_iter().flatten() {
                    if let Some(name) = tool["function"]["name"].as_str() {
                        tools.insert(name.to_string());
                    }
                }
            }
            for name in [
                "search",
                "web_search",
                "task",
                "code_symbols",
                "code_navigation",
                "code_diagnostics",
            ] {
                assert!(tools.contains(name), "{name} missing from {tools:?}");
            }
            assert!(
                bodies
                    .iter()
                    .any(|body| body.contains("BM25 results for: answer notes")),
                "the bm25 search never answered"
            );
            assert!(
                bodies.iter().any(|body| body.contains(">notes.md:1:")),
                "the grep search did not find the seeded line"
            );
            assert!(
                bodies.iter().any(|body| body.contains("Agent: explore")),
                "automatic delegation did not run an explorer first"
            );
            assert!(
                task_bodies[0].contains("Host note: A host admission bind only reopens"),
                "task proof note missing from the first request"
            );
            assert!(
                bodies
                    .iter()
                    .any(|body| body.contains("Nobody is available to answer")),
                "the unattended question answer never reached the model"
            );
            assert!(
                bodies
                    .iter()
                    .any(|body| body.contains("The host bound admission digest")),
                "the completion gate was not answered with a host bind"
            );
            assert!(
                bodies
                    .iter()
                    .any(|body| body.contains("You used tools and then stopped")),
                "a tool-using stop after the bind was taken as a grade"
            );
            assert!(
                bodies
                    .iter()
                    .any(|body| body.contains("context-compaction engine")
                        && body.contains("Drop claims")),
                "the capped stop did not get one fresh reading"
            );

            let record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&result_file).expect("result record"))
                    .expect("result json");
            assert_eq!(record["schema"], "a3s.code.terminal-bench-result.v1");
            assert_eq!(record["outcome"], "succeeded", "{record}");
            assert_eq!(record["reason"], "agent_end");
            assert_eq!(record["terminalEvent"], true);
            assert!(
                record["successfulToolCalls"].as_u64() >= Some(2),
                "{record}"
            );

            assert_eq!(entries(&workspace), ["answer.txt", "notes.md"]);
            assert_eq!(
                std::fs::read_to_string(workspace.join("answer.txt")).expect("answer"),
                "42\n"
            );
            let roots: Vec<String> = entries(&tmp)
                .into_iter()
                .filter(|name| name.starts_with("a3s-code-acp-"))
                .collect();
            assert_eq!(roots.len(), 1, "one private state root: {roots:?}");
            let state = tmp.join(&roots[0]);
            assert!(
                state.join("sessions").is_dir(),
                "session state did not move to the state root"
            );
            let indexes = entries(&state.join("indexes"));
            assert_eq!(indexes.len(), 1, "one workspace index: {indexes:?}");
            assert!(
                state
                    .join("indexes")
                    .join(&indexes[0])
                    .join("index")
                    .is_dir(),
                "the a3s-vec index did not move to the state root"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_continuation_attempt_never_outlives_the_run_deadline() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("ws-deadline");
            let outside = isolate("outside-deadline");
            let home = isolate("home-deadline");
            let tmp = isolate("tmp-deadline");
            let (addr, _script) = spawn_mock();
            let config = write_acl(&outside, &format!("http://{addr}/v1"));
            let result_file = outside.join("a3s-code.result.json");
            let result_arg = result_file.display().to_string();
            let agent = spawn_agent(
                &workspace,
                &config,
                &home,
                &tmp,
                &[
                    "--unattended",
                    "--max-execution-ms",
                    "4000",
                    "--result-file",
                    &result_arg,
                ],
            )
            .await;
            let session = open(&agent, &workspace).await;

            // Attempt one ends at ~3.5 s; a continuation with a fresh core
            // budget would run to ~7 s.
            let started = std::time::Instant::now();
            let response = say(&agent, &session, "SLOW-TASK UNATTENDED-TASK think").await;
            let elapsed = started.elapsed();
            assert_eq!(
                response
                    .expect("recorded timeout has a normal protocol response")
                    .stop_reason,
                acp::StopReason::EndTurn
            );
            assert!(
                elapsed < Duration::from_millis(6000),
                "the run outlived its 4 s deadline: {elapsed:?}"
            );
            let record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&result_file).expect("result record"))
                    .expect("result json");
            assert_eq!(record["outcome"], "timed_out", "{record}");
            assert_eq!(record["reason"], "deadline_exceeded", "{record}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn bash_in_a_large_non_git_workspace_does_not_end_the_run() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("ws-large");
            let deps = workspace.join("node_modules");
            std::fs::create_dir_all(&deps).expect("deps dir");
            for index in 0..4200 {
                std::fs::write(deps.join(format!("m{index}.js")), "x\n").expect("seed dep");
            }
            let outside = isolate("outside-large");
            let home = isolate("home-large");
            let tmp = isolate("tmp-large");
            let (addr, script) = spawn_mock();
            let config = write_acl(&outside, &format!("http://{addr}/v1"));
            let result_file = outside.join("a3s-code.result.json");
            let result_arg = result_file.display().to_string();
            let agent = spawn_agent(
                &workspace,
                &config,
                &home,
                &tmp,
                &[
                    "--unattended",
                    "--max-execution-ms",
                    "170000",
                    "--result-file",
                    &result_arg,
                ],
            )
            .await;
            let session = open(&agent, &workspace).await;

            let response = say(
                &agent,
                &session,
                "BASH-FIRST UNATTENDED-TASK write answer.txt",
            )
            .await;
            let bodies = script.lock().expect("mock").bodies.clone();
            let record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&result_file).expect("result record"))
                    .expect("result json");
            assert!(
                !bodies
                    .iter()
                    .any(|body| body.contains("tool_effect_unresolved")),
                "a mutation in a large tree left an obligation nothing can discharge"
            );
            assert!(response.is_ok(), "{record}");
            assert_eq!(record["outcome"], "succeeded", "{record}");
            assert_eq!(
                std::fs::read_to_string(workspace.join("answer.txt")).expect("answer"),
                "42\n"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_missing_skill_call_does_not_block_later_writes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("ws-skill");
            let outside = isolate("outside-skill");
            let home = isolate("home-skill");
            let tmp = isolate("tmp-skill");
            let (addr, script) = spawn_mock();
            let config = write_acl(&outside, &format!("http://{addr}/v1"));
            let result_file = outside.join("a3s-code.result.json");
            let result_arg = result_file.display().to_string();
            let agent = spawn_agent(
                &workspace,
                &config,
                &home,
                &tmp,
                &[
                    "--unattended",
                    "--max-execution-ms",
                    "170000",
                    "--result-file",
                    &result_arg,
                ],
            )
            .await;
            let session = open(&agent, &workspace).await;

            let response = say(
                &agent,
                &session,
                "MISSING-SKILL-FIRST UNATTENDED-TASK write answer.txt",
            )
            .await;
            let bodies = script.lock().expect("mock").bodies.clone();
            let record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&result_file).expect("result record"))
                    .expect("result json");
            assert!(
                !bodies
                    .iter()
                    .any(|body| body.contains("tool_effect_unresolved")),
                "a rejected skill call left an obligation nothing can discharge"
            );
            assert!(response.is_ok(), "{record}");
            assert_eq!(record["outcome"], "succeeded", "{record}");
            assert_eq!(
                std::fs::read_to_string(workspace.join("answer.txt")).expect("answer"),
                "42\n"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_goal_launch_runs_the_first_attempt_as_the_bound_task_slice() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("ws-goal");
            let outside = isolate("outside-goal");
            let home = isolate("home-goal");
            let tmp = isolate("tmp-goal");
            let (addr, script) = spawn_mock();
            let config = write_acl(&outside, &format!("http://{addr}/v1"));
            let result_file = outside.join("a3s-code.result.json");
            let result_arg = result_file.display().to_string();
            let agent = spawn_agent(
                &workspace,
                &config,
                &home,
                &tmp,
                &[
                    "--unattended",
                    "--max-execution-ms",
                    "170000",
                    "--result-file",
                    &result_arg,
                ],
            )
            .await;
            let session = open(&agent, &workspace).await;

            let response = say(&agent, &session, "UNATTENDED-TASK write answer.txt").await;
            let bodies = script.lock().expect("mock").bodies.clone();
            let record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&result_file).expect("result record"))
                    .expect("result json");
            assert!(
                bodies.iter().any(|body| body
                    .contains("Execute only the current admitted step `task`")
                    && body.contains("UNATTENDED-TASK")
                    && body.matches("Engineering discipline").count() == 1),
                "the default first attempt must carry the bound task and engineering policy"
            );
            assert!(response.is_ok(), "{record}");
            assert_eq!(record["outcome"], "succeeded", "{record}");
            assert_eq!(
                std::fs::read_to_string(workspace.join("answer.txt")).expect("answer"),
                "42\n"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_interactive_launch_never_binds_the_gate_and_keeps_state_in_the_workspace() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = isolate("ws-interactive");
            let outside = isolate("outside-interactive");
            let home = isolate("home-interactive");
            let tmp = isolate("tmp-interactive");
            let (addr, script) = spawn_mock();
            let config = write_acl(&outside, &format!("http://{addr}/v1"));
            let agent = spawn_agent(&workspace, &config, &home, &tmp, &[]).await;
            let session = open(&agent, &workspace).await;
            acp::Agent::set_session_mode(
                agent.conn.as_ref(),
                acp::SetSessionModeRequest::new(session.clone(), acp::SessionModeId::new("yolo")),
            )
            .await
            .expect("set mode");

            let detail = match say(&agent, &session, "UNATTENDED-TASK write answer.txt").await {
                Ok(response) => panic!("an unverified write completed: {response:?}"),
                Err(error) => error.data.map(|data| data.to_string()).unwrap_or_default(),
            };
            assert!(detail.contains("completion gate:"), "{detail}");
            let bodies = script.lock().expect("mock").bodies.clone();
            assert!(
                !bodies
                    .iter()
                    .any(|body| body.contains("The host bound admission digest")
                        || body.contains("Host note: A host admission bind")),
                "an interactive turn received unattended host text"
            );
            assert!(workspace.join(".a3s").is_dir(), "default state root moved");
            assert!(
                !entries(&tmp)
                    .iter()
                    .any(|name| name.starts_with("a3s-code-acp-")),
                "an interactive launch created an unattended state root"
            );
        })
        .await;
}
