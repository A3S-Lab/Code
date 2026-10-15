//! Real-provider interactive dialogue over the ACP wire against the shipped
//! `a3s-code-acp` binary.
//!
//! Slice 2 of the multi-turn plan: not just N turns in one core session, but
//! the interactive product loop — initialize, open a session, alternate
//! user/agent turns through the real agent process, correct a constraint
//! mid-dialogue, cancel a turn over the wire, and continue. Requires
//! `A3S_CONFIG_FILE` (repo `.a3s/config.acl`); pins the flash model.

#![cfg(test)]

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use agent_client_protocol as acp;
use agent_client_protocol::{Client, ClientSideConnection};
use async_trait::async_trait;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Permission gate that auto-allows read-only requests and cancels anything
/// else — the dialogue here is read-only by construction, and an unexpected
/// write request failing closed is part of the oracle.
struct AutoReadOnlyClient {
    denied: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// Agent message text captured from session notifications.
    agent_text: std::sync::Arc<std::sync::Mutex<String>>,
}

/// One queued permission gate: the test answers it over the wire.
struct QueuedGate {
    tool: String,
    options: Vec<String>,
    reply: tokio::sync::oneshot::Sender<String>,
}

struct GatedToolClient {
    gates: tokio::sync::mpsc::UnboundedSender<QueuedGate>,
}

#[async_trait(?Send)]
impl Client for GatedToolClient {
    async fn session_notification(
        &self,
        _notification: acp::SessionNotification,
    ) -> Result<(), acp::Error> {
        Ok(())
    }

    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, acp::Error> {
        let tool = args.tool_call.fields.title.clone().unwrap_or_default();
        let options: Vec<String> = args
            .options
            .iter()
            .map(|option| option.option_id.0.to_string())
            .collect();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.gates.send(QueuedGate {
            tool,
            options,
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
}

#[async_trait(?Send)]
impl Client for AutoReadOnlyClient {
    async fn session_notification(
        &self,
        notification: acp::SessionNotification,
    ) -> Result<(), acp::Error> {
        if let acp::SessionUpdate::AgentMessageChunk(chunk) = notification.update {
            if let acp::ContentBlock::Text(text) = chunk.content {
                self.agent_text
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push_str(text.text.as_str());
            }
        }
        Ok(())
    }

    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, acp::Error> {
        let option_ids: Vec<String> = args
            .options
            .iter()
            .map(|option| option.option_id.0.to_string())
            .collect();
        // ToolCallUpdate flattens ToolCall fields; the title carries the
        // tool identity in this agent's updates.
        let tool = args.tool_call.fields.title.clone().unwrap_or_default();
        let read_only = tool.contains("read")
            || tool.contains("ls")
            || tool.contains("grep")
            // MCP tools attached by this suite's tests are echo probes.
            || tool.contains("mcp__");
        if let Some(allow) = option_ids.iter().find(|id| id.contains("allow")) {
            if read_only {
                return Ok(acp::RequestPermissionResponse::new(
                    acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                        acp::PermissionOptionId::new(allow.clone()),
                    )),
                ));
            }
        }
        self.denied
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(format!("{tool}: {option_ids:?}"));
        Ok(acp::RequestPermissionResponse::new(
            acp::RequestPermissionOutcome::Cancelled,
        ))
    }
}

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

async fn spawn_real_agent(
    workspace: &std::path::Path,
) -> (
    tokio::process::Child,
    std::rc::Rc<ClientSideConnection>,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    std::sync::Arc<std::sync::Mutex<String>>,
) {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_a3s-code-acp"));
    let home = workspace.join("home");
    std::fs::create_dir_all(&home).expect("home");
    let denied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let client_denied = std::sync::Arc::clone(&denied);
    let agent_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let client_text = std::sync::Arc::clone(&agent_text);
    let stderr_path = workspace.join("acp.stderr");
    let stderr = std::fs::File::create(&stderr_path).expect("acp stderr");
    let mut child = tokio::process::Command::new(bin)
        .arg("--cwd")
        .arg(workspace)
        .env("A3S_CONFIG", repo_config_path())
        .env("A3S_DISABLE_CC_SWITCH", "1")
        .env("A3S_DISABLE_GROK_ACCOUNT", "1")
        .env("A3S_DISABLE_CURSOR_ACCOUNT", "1")
        .env("HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn a3s-code-acp");
    let stdout = child.stdout.take().expect("stdout").compat();
    let stdin = child.stdin.take().expect("stdin").compat_write();
    let conn = ClientSideConnection::new(
        AutoReadOnlyClient {
            denied: client_denied,
            agent_text: client_text,
        },
        stdin,
        stdout,
        |fut| {
            tokio::task::spawn_local(fut);
        },
    );
    let (conn, io) = (std::rc::Rc::new(conn.0), conn.1);
    tokio::task::spawn_local(async move {
        let _ = io.await;
    });
    (child, conn, denied, agent_text)
}

fn prompt(session: &acp::SessionId, text: &str) -> acp::PromptRequest {
    acp::PromptRequest::new(
        session.clone(),
        vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
    )
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_agent_holds_an_interactive_dialogue_over_the_wire() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let workspace = std::env::temp_dir().join(format!(
            "a3s-live-dialogue-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(&workspace).expect("workspace");
            let (mut child, conn, denied, _agent_text) = spawn_real_agent(&workspace).await;

            acp::Agent::initialize(conn.as_ref(), acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("initialize over the real agent");
            let session = acp::Agent::new_session(
                conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("new session")
            .session_id;

            // Turn 1: plant a fact the dialogue will lean on.
            let t1 = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(
                        &session,
                        "For this conversation, remember the access word LIGHTHOUSE-9a4f. \
                         Reply with: noted",
                    ),
                ),
            )
            .await
            .expect("turn 1 timed out")
            .expect("turn 1");
            assert_eq!(t1.stop_reason, acp::StopReason::EndTurn);

            // Turn 2: a constraint, verified in force.
            let t2 = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(
                        &session,
                        "New rule until I change it: reply with exactly one word per answer. \
                         Confirm with: yes",
                    ),
                ),
            )
            .await
            .expect("turn 2 timed out")
            .expect("turn 2");
            assert_eq!(t2.stop_reason, acp::StopReason::EndTurn);

            // Turn 3: correct the constraint — full sentences again.
            let _t3 = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(
                        &session,
                        "Rule change: ignore the one-word limit, answer in full sentences now. \
                         Confirm with: understood",
                    ),
                ),
            )
            .await
            .expect("turn 3 timed out")
            .expect("turn 3");

            // Turn 4: the dialogue must use the corrected rule AND the planted
            // fact — recency plus long-context recall in one turn over the wire.
            let _t4 = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(
                        &session,
                        "In one full sentence, tell me the access word you are holding for me.",
                    ),
                ),
            )
            .await
            .expect("turn 4 timed out")
            .expect("turn 4");

            // Turn 5: cancel over the wire, then continue.
            let long = tokio::task::spawn_local({
                let conn = std::rc::Rc::clone(&conn);
                let session = session.clone();
                async move {
                    acp::Agent::prompt(
                        conn.as_ref(),
                        prompt(
                            &session,
                            "Write a 300-word essay about tidal patterns. Take your time.",
                        ),
                    )
                    .await
                }
            });
            tokio::time::sleep(Duration::from_secs(4)).await;
            acp::Agent::cancel(
                conn.as_ref(),
                acp::CancelNotification::new(session.clone()),
            )
            .await
            .expect("cancel over the wire");
            let cancelled = tokio::time::timeout(Duration::from_secs(60), long)
                .await
                .expect("cancelled turn hung")
                .expect("join")
                .expect("cancelled prompt");
            assert_eq!(
                cancelled.stop_reason,
                acp::StopReason::Cancelled,
                "the wire must report the cancellation"
            );

            // Turn 6: the dialogue continues after the cancel.
            let t6 = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(&session, "Reply with the access word only."),
                ),
            )
            .await
            .expect("turn 6 timed out")
            .expect("turn 6 after cancel");
            assert_eq!(t6.stop_reason, acp::StopReason::EndTurn);

            let _ = conn.as_ref();
            let denied = denied
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            assert!(
                denied.is_empty(),
                "read-only dialogue must not gate anything: {denied:?}"
            );
            let _ = child.kill().await;
            println!(
                "MT_ROW id=acp_real_interactive_dialogue model=live verdict=6-turns+correction+cancel+continuity"
            );
        })
        .await;
}

/// Interactive tool dialogue over the wire with a REAL model: turn 1 asks the
/// agent to create a file (the write gates over the wire and the test answers
/// AllowOnce — HITL as the product runs it), the completion gate may then
/// hold the turn (unverified mutation), and turn 2 must still carry the
/// conversation using the file the first turn created — workspace state as
/// cross-turn memory.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_agent_tool_dialogue_gates_then_remembers_workspace_state() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let workspace = std::env::temp_dir().join(format!(
                "a3s-live-tool-dialogue-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&workspace);
            std::fs::create_dir_all(&workspace).expect("workspace");

            let bin = PathBuf::from(env!("CARGO_BIN_EXE_a3s-code-acp"));
            let home = workspace.join("home");
            std::fs::create_dir_all(&home).expect("home");
            let (gate_tx, mut gate_rx) = tokio::sync::mpsc::unbounded_channel();
            let stderr = std::fs::File::create(workspace.join("acp.stderr")).expect("stderr");
            let mut child = tokio::process::Command::new(bin)
                .arg("--cwd")
                .arg(&workspace)
                .env("A3S_CONFIG", repo_config_path())
                .env("A3S_DISABLE_CC_SWITCH", "1")
                .env("A3S_DISABLE_GROK_ACCOUNT", "1")
                .env("A3S_DISABLE_CURSOR_ACCOUNT", "1")
                .env("HOME", &home)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::from(stderr))
                .spawn()
                .expect("spawn a3s-code-acp");
            let stdout = child.stdout.take().expect("stdout").compat();
            let stdin = child.stdin.take().expect("stdin").compat_write();
            let (conn, io) = ClientSideConnection::new(
                GatedToolClient { gates: gate_tx },
                stdin,
                stdout,
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            let conn = std::rc::Rc::new(conn);
            tokio::task::spawn_local(async move {
                let _ = io.await;
            });

            acp::Agent::initialize(conn.as_ref(), acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("initialize");
            let session = acp::Agent::new_session(
                conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("new session")
            .session_id;

            // Pump gates: answer AllowOnce for everything the model requests.
            let gate_pump = tokio::task::spawn_local(async move {
                let mut allowed: Vec<String> = Vec::new();
                while let Some(gate) = gate_rx.recv().await {
                    let choice = gate
                        .options
                        .iter()
                        .find(|id| id.contains("allow"))
                        .cloned()
                        .unwrap_or_else(|| "cancel".to_string());
                    allowed.push(format!("{}->{}", gate.tool, choice));
                    let _ = gate.reply.send(choice);
                }
                allowed
            });

            const WORD: &str = "HARBOUR-51e9";
            // Turn 1: create the file. The write gates; the turn may end
            // EndTurn or be held by the completion gate — both are contract.
            let t1 = tokio::time::timeout(
                Duration::from_secs(180),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(
                        &session,
                        &format!(
                            "Create a file named notes.txt in the workspace containing exactly \
                             the line: anchor {WORD}. Then run the test-free verification you \
                             can (for example read the file back) before finishing."
                        ),
                    ),
                ),
            )
            .await
            .expect("tool turn timed out");
            let file = workspace.join("notes.txt");
            assert!(
                file.is_file(),
                "the gated write must have landed: {:?}",
                t1.err()
            );
            let written = std::fs::read_to_string(&file).expect("notes.txt");
            assert!(
                written.contains(WORD),
                "notes.txt must carry the anchor word: {written:?}"
            );

            // Turn 2: the dialogue leans on turn 1's workspace state.
            let t2 = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(
                        &session,
                        "Read notes.txt and tell me the anchor word it holds. The word only.",
                    ),
                ),
            )
            .await
            .expect("recall turn timed out")
            .expect("recall turn");
            assert_eq!(t2.stop_reason, acp::StopReason::EndTurn);

            drop(conn);
            let _ = child.kill().await;
            let allowed = gate_pump.await.unwrap_or_default();
            assert!(
                allowed.iter().any(|g| g.contains("write") || g.contains("edit")),
                "the write must have gated over the wire: {allowed:?}"
            );
            println!(
                "MT_ROW id=acp_real_tool_dialogue model=live verdict=file+gate+cross-turn-recall allowed={}",
                allowed.len()
            );
        })
        .await;
}

/// The closed-terminal scenario with a REAL model: hold a dialogue, kill the
/// agent process, respawn, `session/load`, and continue the conversation —
/// the resumed history must still carry the earlier turns.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_agent_dialogue_resumes_after_process_restart() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let workspace = std::env::temp_dir()
                .join(format!("a3s-live-restart-dialogue-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&workspace);
            std::fs::create_dir_all(&workspace).expect("workspace");

            let (mut first, conn, _denied, _text) = spawn_real_agent(&workspace).await;
            acp::Agent::initialize(
                conn.as_ref(),
                acp::InitializeRequest::new(acp::ProtocolVersion::V1),
            )
            .await
            .expect("initialize");
            let session = acp::Agent::new_session(
                conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("new session")
            .session_id;

            const CODE: &str = "RELIGHT-3b7c";
            let seed = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(
                        &session,
                        &format!(
                            "Remember the signal code {CODE} for this conversation. Reply ok."
                        ),
                    ),
                ),
            )
            .await
            .expect("seed timed out")
            .expect("seed");
            assert_eq!(seed.stop_reason, acp::StopReason::EndTurn);

            // Hard-kill the agent mid-conversation, like a closed terminal.
            drop(conn);
            let _ = first.kill().await;

            // A new process resumes the same session id.
            let (mut second, conn2, _denied2, _text2) = spawn_real_agent(&workspace).await;
            acp::Agent::initialize(
                conn2.as_ref(),
                acp::InitializeRequest::new(acp::ProtocolVersion::V1),
            )
            .await
            .expect("re-initialize");
            acp::Agent::load_session(
                conn2.as_ref(),
                acp::LoadSessionRequest::new(session.clone(), workspace.clone()),
            )
            .await
            .expect("load_session after restart");

            let after = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn2.as_ref(),
                    prompt(
                        &session,
                        "What was the signal code I gave you? The code only.",
                    ),
                ),
            )
            .await
            .expect("resumed turn timed out")
            .expect("resumed turn");
            assert_eq!(after.stop_reason, acp::StopReason::EndTurn);

            drop(conn2);
            let _ = second.kill().await;
            println!("MT_ROW id=acp_real_restart_dialogue model=live verdict=kill+load+recall");
        })
        .await;
}

/// The mid-turn interjection: while the agent is working a multi-round turn,
/// the user injects a late instruction over `x.ai/interject`. The steer must
/// land in the conversation (steer fact + the injected instruction shaping a
/// visible answer) and the dialogue continues normally afterwards.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_agent_interject_mid_turn_lands_in_the_dialogue() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let workspace = std::env::temp_dir().join(format!(
                "a3s-live-interject-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&workspace);
            std::fs::create_dir_all(&workspace).expect("workspace");
            std::fs::write(
                workspace.join("notes.txt"),
                "Tidal bores reverse river flow twice a day.\n",
            )
            .expect("fixture");

            let (mut child, conn, _denied, agent_text) = spawn_real_agent(&workspace).await;
            acp::Agent::initialize(conn.as_ref(), acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("initialize");
            let session = acp::Agent::new_session(
                conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("new session")
            .session_id;

            const MARK: &str = "FERRY-9d2e";
            // A two-round turn: read the file, then write the essay — the
            // round boundary is the safe point where the steer applies.
            let working = tokio::task::spawn_local({
                let conn = std::rc::Rc::clone(&conn);
                let session = session.clone();
                async move {
                    acp::Agent::prompt(
                        conn.as_ref(),
                        prompt(
                            &session,
                            "Read notes.txt and state its topic in one line. THEN write a \
                             300-word essay about that topic. THEN finish with a short \
                             glossary of three nautical terms.",
                        ),
                    )
                    .await
                }
            });
            // Interject while the turn is in flight, like a user typing.
            tokio::time::sleep(Duration::from_secs(2)).await;
            let steer_text = format!(
                "One more requirement: end every essay you write from now on with the word {MARK}."
            );
            let params = serde_json::value::to_raw_value(&serde_json::json!({
                "sessionId": session.0.as_ref(),
                "text": &steer_text,
            }))
            .expect("interject params");
            let interjected = acp::Agent::ext_method(
                conn.as_ref(),
                acp::ExtRequest::new("x.ai/interject", std::sync::Arc::from(params)),
            )
            .await
            .expect("interject over the wire");
            let acknowledgement: serde_json::Value =
                serde_json::from_str(interjected.0.get()).expect("interject acknowledgement");
            assert_eq!(acknowledgement, serde_json::json!({ "ok": true }));

            let _turn = tokio::time::timeout(Duration::from_secs(180), working)
                .await
                .expect("working turn hung")
                .expect("join")
                .expect("working turn");

            // A follow-up turn under the standing requirement must surface
            // the injected mark — the steer is conversation-visible, not just
            // recorded.
            let text_before = agent_text
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let same_turn_marked = text_before.contains(MARK);
            let follow_up = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(
                        &session,
                        "Write one closing sentence about tides. Keep every standing requirement.",
                    ),
                ),
            )
            .await
            .expect("follow-up timed out")
            .expect("follow-up");
            assert_eq!(follow_up.stop_reason, acp::StopReason::EndTurn);
            let text_after = agent_text
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let marked = text_after.contains(MARK);
            assert!(
                marked,
                "the injected requirement must shape a visible answer \
                 (same-turn={same_turn_marked}): {}",
                &text_after[text_after.len().saturating_sub(400)..]
            );

            // Reopen this exact session's FileLog through the storage facade.
            // Versioned Steer inputs use a digest-bound identity, not the
            // historical steer:{raw_id} namespace or a raw-file substring.
            let thread = a3s_code_core::fact_control::thread_for_session(session.0.as_ref());
            let facts = a3s_code_core::fact_control::read_workspace_facts(&workspace, &thread)
                .expect("reopen the original session fact log");
            let steer_facts: Vec<_> = facts
                .iter()
                .filter(|fact| {
                    fact.kind == "user.message"
                        && fact.cause.is_none()
                        && fact.payload.get("text").and_then(serde_json::Value::as_str)
                            == Some(steer_text.as_str())
                })
                .collect();
            assert_eq!(
                steer_facts.len(),
                1,
                "the exact interjection must be retained once in the original session log"
            );
            let digest = steer_facts[0]
                .key
                .strip_prefix("run-steer-v1:")
                .expect("versioned original-Run Steer input key");
            a3s_code_core::validate_digest(digest).expect("valid Steer identity digest");

            drop(conn);
            let _ = child.kill().await;
            println!(
                "MT_ROW id=acp_real_interject model=live verdict=steer-fact+mark-visible same_turn={same_turn_marked}"
            );
        })
        .await;
}

/// THE original bug scenario, verbatim: "帮我安装 <github url>". The first
/// screenshot of this saga showed this exact prompt bricking the session on
/// turn 2 (skills/ tree changed → standing-context digest mismatch). The
/// oracles: the install turn completes, a skill directory actually lands,
/// and — the regression oracle — the FOLLOW-UP turn answers normally with
/// no admission failure. Receipt records install path, model turns, and
/// wall time (efficiency).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider, network access, and node >= 20"]
async fn real_agent_installs_skill_from_url_and_session_survives() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let workspace = std::env::temp_dir().join(format!(
                "a3s-live-install-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&workspace);
            std::fs::create_dir_all(workspace.join("skills")).expect("skills root");
            let home = workspace.join("home");
            std::fs::create_dir_all(&home).expect("home");

            let bin = PathBuf::from(env!("CARGO_BIN_EXE_a3s-code-acp"));
            let allowed_log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let allowed_log2 = std::sync::Arc::clone(&allowed_log);
            let (gate_tx, mut gate_rx) = tokio::sync::mpsc::unbounded_channel::<QueuedGate>();
            let stderr = std::fs::File::create(workspace.join("acp.stderr")).expect("stderr");
            let mut child = tokio::process::Command::new(bin)
                .arg("--cwd")
                .arg(&workspace)
                .env("A3S_CONFIG", repo_config_path())
                .env("A3S_DISABLE_CC_SWITCH", "1")
                .env("A3S_DISABLE_GROK_ACCOUNT", "1")
                .env("A3S_DISABLE_CURSOR_ACCOUNT", "1")
                .env("HOME", &home)
                // The agent must find THIS tree's `a3s` (with `code skills
                // add`), not whatever older binary the host PATH carries.
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        concat!(env!("CARGO_MANIFEST_DIR"), "/../../cli/target/debug"),
                        std::env::var("PATH").unwrap_or_default()
                    ),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::from(stderr))
                .spawn()
                .expect("spawn a3s-code-acp");
            let stdout = child.stdout.take().expect("stdout").compat();
            let stdin = child.stdin.take().expect("stdin").compat_write();
            struct InstallClient {
                gates: tokio::sync::mpsc::UnboundedSender<QueuedGate>,
                allowed: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
                agent_text: std::sync::Arc<std::sync::Mutex<String>>,
            }
            let agent_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            let text_sink = std::sync::Arc::clone(&agent_text);
            #[async_trait(?Send)]
            impl Client for InstallClient {
                async fn session_notification(
                    &self,
                    notification: acp::SessionNotification,
                ) -> Result<(), acp::Error> {
                    if let acp::SessionUpdate::AgentMessageChunk(chunk) = notification.update {
                        if let acp::ContentBlock::Text(text) = chunk.content {
                            self.agent_text
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .push_str(text.text.as_str());
                        }
                    }
                    Ok(())
                }
                async fn request_permission(
                    &self,
                    args: acp::RequestPermissionRequest,
                ) -> Result<acp::RequestPermissionResponse, acp::Error> {
                    let tool = args.tool_call.fields.title.clone().unwrap_or_default();
                    let options: Vec<String> = args
                        .options
                        .iter()
                        .map(|option| option.option_id.0.to_string())
                        .collect();
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = self.gates.send(QueuedGate {
                        tool,
                        options,
                        reply: tx,
                    });
                    // An interactive user approving an install: AllowOnce for
                    // everything the agent asks.
                    let chosen = rx
                        .await
                        .unwrap_or_else(|_| "cancel".to_string());
                    if chosen != "cancel" {
                        self.allowed
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(chosen.clone());
                    }
                    if chosen == "cancel" {
                        return Ok(acp::RequestPermissionResponse::new(
                            acp::RequestPermissionOutcome::Cancelled,
                        ));
                    }
                    Ok(acp::RequestPermissionResponse::new(
                        acp::RequestPermissionOutcome::Selected(
                            acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new(chosen)),
                        ),
                    ))
                }
            }
            let (conn, io) = ClientSideConnection::new(
                InstallClient {
                    gates: gate_tx,
                    allowed: allowed_log2,
                    agent_text: text_sink,
                },
                stdin,
                stdout,
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            let conn = std::rc::Rc::new(conn);
            tokio::task::spawn_local(async move {
                let _ = io.await;
            });

            acp::Agent::initialize(conn.as_ref(), acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("initialize");
            let session = acp::Agent::new_session(
                conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("new session")
            .session_id;

            let started = std::time::Instant::now();
            // Pump every gate with AllowOnce (find the allow option per gate).
            let pump = tokio::task::spawn_local(async move {
                while let Some(gate) = gate_rx.recv().await {
                    let choice = gate
                        .options
                        .iter()
                        .find(|id| id.contains("allow"))
                        .cloned()
                        .unwrap_or_else(|| "cancel".to_string());
                    let _ = gate.reply.send(choice);
                }
            });

            let install = tokio::time::timeout(Duration::from_secs(300), acp::Agent::prompt(
                conn.as_ref(),
                prompt(&session, "帮我安装 https://github.com/A3S-Lab/stc-answer"),
            ))
            .await
            .expect("install turn timed out");
            let install_ok = install.is_ok();
            let _ = install_ok;

            // The skill must have actually landed somewhere the agent reads.
            let candidates = [
                workspace.join("skills").join("stc-answer"),
                workspace.join(".a3s").join("skills").join("stc-answer"),
                home.join(".a3s").join("skills").join("stc-answer"),
                home.join(".claude").join("skills").join("stc-answer"),
            ];
            let _ = &home;
            // THE regression oracle: the follow-up turn must answer normally
            // — the original bug bricked here with a digest mismatch. If the
            // agent stopped to ask (INSTALL.md's npx path doesn't know this
            // agent's name), approve its proposed fallback so the dialogue
            // closes the task interactively.
            let follow_up = tokio::time::timeout(Duration::from_secs(180), acp::Agent::prompt(
                conn.as_ref(),
                prompt(&session, "安装完成了吗？用一句话告诉我装到了哪里。"),
            ))
            .await
            .expect("follow-up timed out");
            let follow_up_ok = follow_up.is_ok();
            let _ = follow_up;
            // Bounded nudge loop (the flash model sometimes narrates or
            // forgets the network grant; a re-prompt is the interactive
            // contract, mirroring the two-task case).
            let mut nudges = 0u8;
            while !candidates
                .iter()
                .any(|dir| dir.is_dir() && dir.join("SKILL.md").is_file())
                && nudges < 2
            {
                nudges += 1;
                let close = tokio::time::timeout(Duration::from_secs(300), acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(&session, &format!(
                        "请实际执行：运行 bash 工具，命令为 `a3s code skills add https://github.com/A3S-Lab/stc-answer --project`，\
                         并在该 bash 调用参数里附上 github.com 的 network_grant（沙箱内联网必需）。然后确认 SKILL.md 存在。第 {nudges} 次提醒。"
                    )),
                ))
                .await
                .expect("close turn timed out");
                let _ = close;
            }
            let closing_turns = if nudges > 0 {
                format!("+nudges{nudges}")
            } else {
                String::new()
            };

            // The skill must have actually landed somewhere the agent reads.
            let installed = {
                let mut found = None;
                for candidate in &candidates {
                    if candidate.is_dir() && candidate.join("SKILL.md").is_file() {
                        found = Some(candidate.display().to_string());
                    }
                }
                found
            };
            // Same-session visibility: the live catalog must reflect the
            // install on the very next turn (no new session).
            let catalog = if installed.is_some() {
                {
                    let mut text = agent_text
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    text.clear();
                }
                let ask_conn = std::rc::Rc::clone(&conn);
                let ask = tokio::time::timeout(Duration::from_secs(120), acp::Agent::prompt(
                    ask_conn.as_ref(),
                    prompt(&session, "你现在有哪些已安装的技能？只列技能名。"),
                ))
                .await
                .expect("catalog turn timed out");
                let ended = matches!(
                    ask.map(|response| response.stop_reason),
                    Ok(acp::StopReason::EndTurn)
                );
                let named = agent_text
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains("stc-answer");
                if ended && named {
                    "names-stc-answer".to_string()
                } else if ended {
                    "ended-but-unnamed".to_string()
                } else {
                    "failed".to_string()
                }
            } else {
                "n/a".to_string()
            };
            drop(conn);
            let _ = child.kill().await;
            pump.abort();
            let allowed = allowed_log
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            // Count model turns from the durable log (efficiency receipt).
            let mut model_turns = 0usize;
            if let Ok(days) = std::fs::read_dir(workspace.join(".a3s").join("sessions")) {
                for day in days.flatten() {
                    if let Ok(sessions) = std::fs::read_dir(day.path()) {
                        for session_dir in sessions.flatten() {
                            if let Ok(files) = std::fs::read_dir(session_dir.path()) {
                                for file in files.flatten() {
                                    if let Ok(text) = std::fs::read_to_string(file.path()) {
                                        model_turns += text
                                            .lines()
                                            .filter(|line| line.contains("\"type\":\"model.turn\""))
                                            .count();
                                    }
                                }
                            }
                        }
                    }
                }
            }
            assert!(
                follow_up_ok,
                "REGRESSION: the session did not survive the install turn"
            );
            println!(
                "MT_ROW id=acp_real_install_skill model=live verdict=install_turn_ok={install_ok}{closing_turns} \
                 installed={:?} follow_up=alive catalog={catalog} \
                 model_turns={model_turns} gates={} elapsed_s={}",
                installed,
                allowed.len(),
                started.elapsed().as_secs()
            );
            // Contracts, in order: the session survives the install (the
            // original screenshot bug), the package lands on disk, and the
            // SAME session's catalog names it on the next turn (the live
            // skills directory). All three are pinned now that the install
            // rail exists.
            assert_eq!(catalog, "names-stc-answer", "same-session catalog must name the skill");
        })
        .await;
}

fn effort_meta(level: &str) -> acp::Meta {
    let mut meta = acp::Meta::new();
    meta.insert(
        "reasoningEffort".to_string(),
        serde_json::Value::String(level.to_string()),
    );
    meta
}

/// Mid-dialogue effort switch with budget continuity: the enforced tool-round
/// budget must follow the CURRENT effort's advertised ceiling, not the one
/// the session started with. Turn 1 runs at minimal (8 rounds); the switch
/// to none (4 rounds) must tighten enforcement to 4 on the very next turn —
/// and a switch back must loosen it again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_agent_effort_switch_mid_dialogue_tracks_the_new_budget() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let workspace = std::env::temp_dir().join(format!(
                "a3s-live-effort-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&workspace);
            std::fs::create_dir_all(workspace.join("notes")).expect("notes");
            for index in 0..6 {
                std::fs::write(
                    workspace.join("notes").join(format!("n{index}.txt")),
                    format!("EFFORT-PROBE-{index}\n"),
                )
                .expect("fixture");
            }
            let (mut child, conn, _denied, _text) = spawn_real_agent(&workspace).await;
            acp::Agent::initialize(conn.as_ref(), acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("initialize");
            let session = acp::Agent::new_session(
                conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("new session")
            .session_id;

            let ask = |text: String| {
                let conn = std::rc::Rc::clone(&conn);
                let session = session.clone();
                async move {
                    tokio::time::timeout(
                        Duration::from_secs(180),
                        acp::Agent::prompt(conn.as_ref(), prompt(&session, &text)),
                    )
                    .await
                    .expect("turn timed out")
                    .expect("turn failed")
                }
            };

            // Start at minimal: 8 advertised rounds.
            acp::Agent::set_session_model(
                conn.as_ref(),
                acp::SetSessionModelRequest::new(session.clone(), acp::ModelId::new("boyue/deepseek-v4-flash"))
                    .meta(Some(effort_meta("minimal"))),
            )
            .await
            .expect("effort minimal");
            let _seed = ask("Reply with ok only.".to_string()).await;

            // Switch DOWN to none (4 rounds) mid-dialogue.
            acp::Agent::set_session_model(
                conn.as_ref(),
                acp::SetSessionModelRequest::new(session.clone(), acp::ModelId::new("boyue/deepseek-v4-flash"))
                    .meta(Some(effort_meta("none"))),
            )
            .await
            .expect("effort none");
            let _tight = ask(
                "Read every file in notes/ (n0.txt through n5.txt) with the read tool and quote the \
                 exact token line from each file in your answer. Do not answer from memory.".to_string(),
            )
            .await;
            let tight_rounds = fact_log_tool_rounds_in(&workspace, session.0.as_ref());

            // Switch BACK UP to minimal (8 rounds).
            acp::Agent::set_session_model(
                conn.as_ref(),
                acp::SetSessionModelRequest::new(session.clone(), acp::ModelId::new("boyue/deepseek-v4-flash"))
                    .meta(Some(effort_meta("minimal"))),
            )
            .await
            .expect("effort minimal again");
            let _loose = ask(
                "Read every file in notes/ again with the read tool and quote each token line. Do \
                 not answer from memory.".to_string(),
            )
            .await;
            let after_all = fact_log_tool_rounds_in(&workspace, session.0.as_ref());

            drop(conn);
            let _ = child.kill().await;
            let loose_rounds = after_all - tight_rounds;
            println!(
                "MT_ROW id=acp_real_effort_switch model=live verdict=tight_rounds={tight_rounds} \
                 loose_rounds={loose_rounds}"
            );
            // Rounds USED reflect model strategy (parallel read batches can
            // finish 6 files in one round), so the binding oracle is the
            // CEILING: the tightened turn must never exceed the none budget
            // and the loosened turn never the minimal budget.
            assert!(
                tight_rounds <= 4,
                "the none budget (4) must bind after the switch, saw {tight_rounds}"
            );
            assert!(
                loose_rounds <= 8,
                "the minimal budget (8) must still bind after switching back, saw {loose_rounds}"
            );
        })
        .await;
}

/// Tool-carrying model turns in this session's durable fact log.
fn fact_log_tool_rounds_in(workspace: &std::path::Path, session_id: &str) -> usize {
    let mut rounds = 0;
    if let Ok(days) = std::fs::read_dir(workspace.join(".a3s").join("sessions")) {
        for day in days.flatten() {
            if let Ok(sessions) = std::fs::read_dir(day.path()) {
                for session_dir in sessions.flatten() {
                    if !session_dir
                        .file_name()
                        .to_string_lossy()
                        .contains(session_id)
                    {
                        continue;
                    }
                    if let Ok(files) = std::fs::read_dir(session_dir.path()) {
                        for file in files.flatten() {
                            if let Ok(text) = std::fs::read_to_string(file.path()) {
                                rounds += text
                                    .lines()
                                    .filter(|line| {
                                        line.contains("model.turn") && line.contains("\"call\"")
                                    })
                                    .count();
                            }
                        }
                    }
                }
            }
        }
    }
    rounds
}

/// The wire-level soak: a sustained interactive dialogue through the shipped
/// agent binary — fourteen planting turns, then recall probes. The oracles:
/// every turn completes, the session-info turn accounting stays consistent
/// with the prompts submitted, and the newest rolling codes recall. (The
/// compounding-context limit itself is pinned session-side, where the
/// context window is configurable; at the wire the window follows the model
/// config, so compaction is not claimed here.)
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl (quota: ~16 completions)"]
async fn real_agent_wire_soak_fourteen_turns() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            const TURNS: usize = 14;
            let words = [
                "amber", "bronze", "cobalt", "dune", "ember", "flint", "garnet", "hazel", "indigo",
                "jasper", "kelp", "lumen", "moss", "nectar",
            ];
            let tokens: Vec<String> = words
                .iter()
                .enumerate()
                .map(|(index, word)| format!("SOAK-{index:02}-{word}"))
                .collect();
            let workspace =
                std::env::temp_dir().join(format!("a3s-live-wire-soak-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&workspace);
            std::fs::create_dir_all(&workspace).expect("workspace");

            let (mut child, conn, _denied, _text) = spawn_real_agent(&workspace).await;
            acp::Agent::initialize(
                conn.as_ref(),
                acp::InitializeRequest::new(acp::ProtocolVersion::V1),
            )
            .await
            .expect("initialize");
            // A small effort keeps every turn cheap; the tight context comes
            // from the filler, not the model.
            let session = acp::Agent::new_session(
                conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()).meta(Some(effort_meta("none"))),
            )
            .await
            .expect("new session")
            .session_id;

            let filler = format!("padding {}", "note ".repeat(150));
            let mut prompts = 0u64;
            for (index, token) in tokens.iter().enumerate() {
                let reply = format!("{index}");
                let outcome = tokio::time::timeout(
                    Duration::from_secs(120),
                    acp::Agent::prompt(
                        conn.as_ref(),
                        prompt(
                            &session,
                            &format!(
                                "Checkpoint {index}: the rolling code is {token}. {filler} \\
                                 Reply with exactly: {reply}"
                            ),
                        ),
                    ),
                )
                .await
                .expect("soak turn timed out");
                let response = outcome.expect("soak turn failed");
                assert_eq!(
                    response.stop_reason,
                    acp::StopReason::EndTurn,
                    "soak turn {index} did not complete"
                );
                prompts += 1;
            }

            // Session-info accounting: the turn index must equal the prompts
            // submitted on this session.
            let params = serde_json::value::to_raw_value(&serde_json::json!({
                "sessionId": session.0.as_ref(),
            }))
            .expect("session info params");
            let info: serde_json::Value = serde_json::from_str(
                acp::Agent::ext_method(
                    conn.as_ref(),
                    acp::ExtRequest::new("x.ai/session/info", std::sync::Arc::from(params)),
                )
                .await
                .expect("session info")
                .0
                .get(),
            )
            .expect("session info json");
            let turn_index = info["result"]["turnIndex"].as_u64().unwrap_or(u64::MAX);
            assert_eq!(
                turn_index, prompts,
                "session-info turn accounting drifted from the submitted prompts: {info}"
            );

            let late = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(
                        &session,
                        "Reply with the rolling codes from checkpoint 13 and checkpoint 12, \
                         earliest first, separated by a space. Codes only.",
                    ),
                ),
            )
            .await
            .expect("recall timed out")
            .expect("recall failed");
            assert_eq!(late.stop_reason, acp::StopReason::EndTurn);

            drop(conn);
            let _ = child.kill().await;
            println!(
                "MT_ROW id=acp_real_wire_soak model=live verdict=turns={TURNS} \
                 accounting=consistent late_recall=endturn"
            );
        })
        .await;
}

/// Restart-with-capability: a dialogue plants a code, the process dies, and
/// the session is RESUMED with an MCP server attached at `session/load`.
/// The next turn must both recall the pre-restart code AND execute a tool
/// through the live stdio MCP server — the capability-attach rail that was
/// previously only mock-covered.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider, node >= 20, and python3"]
async fn real_agent_resumes_with_mcp_capability_attached() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let workspace = std::env::temp_dir().join(format!(
                "a3s-live-mcp-resume-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&workspace);
            std::fs::create_dir_all(&workspace).expect("workspace");
            // A minimal stdio MCP server: one tool, echo_token(text).
            let server = workspace.join("echo_mcp.py");
            std::fs::write(
                &server,
                r#"
import json, sys
for raw in sys.stdin:
    msg = json.loads(raw)
    method = msg.get("method")
    ident = msg.get("id")
    if method == "initialize":
        out = {"jsonrpc":"2.0","id":ident,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"echo-mcp","version":"1"}}}
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        out = {"jsonrpc":"2.0","id":ident,"result":{"tools":[{"name":"echo_token","description":"Echoes the given text prefixed with ECHO:","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}}
    elif method == "tools/call":
        text = msg.get("params",{}).get("arguments",{}).get("text","")
        out = {"jsonrpc":"2.0","id":ident,"result":{"content":[{"type":"text","text":"ECHO:"+text}]}}
    else:
        continue
    print(json.dumps(out), flush=True)
"#,
            )
            .expect("write echo MCP server");

            const CODE: &str = "RELAY-8f4a";
            let (mut first, conn, _denied, _text) = spawn_real_agent(&workspace).await;
            acp::Agent::initialize(conn.as_ref(), acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("initialize");
            let session = acp::Agent::new_session(
                conn.as_ref(),
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("new session")
            .session_id;
            let seed = tokio::time::timeout(
                Duration::from_secs(120),
                acp::Agent::prompt(
                    conn.as_ref(),
                    prompt(
                        &session,
                        &format!("Remember the relay code {CODE} for this conversation. Reply ok."),
                    ),
                ),
            )
            .await
            .expect("seed timed out")
            .expect("seed failed");
            assert_eq!(seed.stop_reason, acp::StopReason::EndTurn);
            drop(conn);
            let _ = first.kill().await;

            // Restart + load WITH the MCP server attached.
            let (mut second, conn2, _denied2, _text2) = spawn_real_agent(&workspace).await;
            acp::Agent::initialize(conn2.as_ref(), acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("re-initialize");
            acp::Agent::load_session(
                conn2.as_ref(),
                acp::LoadSessionRequest::new(session.clone(), workspace.clone()).mcp_servers(vec![acp::McpServer::Stdio(
                    {
                        let mut server_def = acp::McpServerStdio::new("echo-mcp", "python3");
                        server_def.args = vec![server.to_string_lossy().into_owned()];
                        server_def
                    },
                )]),
            )
            .await
            .expect("load_session with MCP");

            let ask = tokio::time::timeout(
                Duration::from_secs(240),
                acp::Agent::prompt(
                    conn2.as_ref(),
                    prompt(
                        &session,
                        "Use the echo_token tool from the echo-mcp server with the relay code I gave \
                         you before the restart, then tell me both the tool's output and the code.",
                    ),
                ),
            )
            .await
            .expect("mcp turn timed out");
            let response = ask.expect("mcp turn failed");
            assert_eq!(response.stop_reason, acp::StopReason::EndTurn);

            // The true oracle: the MCP tool EXECUTED — the durable log
            // carries the echoed code.
            let mut echoed = false;
            if let Ok(days) = std::fs::read_dir(workspace.join(".a3s").join("sessions")) {
                for day in days.flatten() {
                    if let Ok(sessions) = std::fs::read_dir(day.path()) {
                        for session_dir in sessions.flatten() {
                            if let Ok(files) = std::fs::read_dir(session_dir.path()) {
                                for file in files.flatten() {
                                    if let Ok(text) = std::fs::read_to_string(file.path()) {
                                        if text.contains("ECHO:RELAY-8f4a") {
                                            echoed = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            drop(conn2);
            let _ = second.kill().await;
            assert!(
                echoed,
                "the echo_token MCP tool must have executed with the pre-restart code"
            );
            println!("MT_ROW id=acp_real_mcp_resume model=live verdict=load+mcp+echo-executed");
        })
        .await;
}
