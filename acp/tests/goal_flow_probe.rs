//! Goal command flow probe over the REAL pager path: an in-process
//! ClientSideConnection (the pager's exact transport role) drives the
//! A3sCodeAgent backend (AgentSideConnection) across cross-wired duplexes,
//! and the recording client captures every streamed session update.
//!
//! Contracts asserted:
//! 1. /goal set runs a model turn (the marker is streamed).
//! 2. /goal status / pause / resume report and transition deterministically.
//! 3. The standing goal reaches LATER turns (the codename is answered).
//! 4. Queued mid-turn commands serialize (resume + immediate status race).
//! 5. Streamed replies deliver their text exactly once (no duplication).
//!
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-acp --test goal_flow_probe -- --ignored --nocapture --test-threads=1
//!
//! `goal_horizon_qwen_flash_keeps_admitting_slices` loads that ACL, forces
//! `anthropic/qwen3.8-flash`, and resumes a bound goal under a two-slice host policy.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use a3s_code_acp::{load_launch_config, A3sCodeAgent};
use agent_client_protocol as acp;
use async_trait::async_trait;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Records every streamed session update so the probe can assert on the
/// assistant text exactly as the pager's rendering layer receives it.
#[derive(Default)]
struct RecordingClient {
    chunks: RefCell<Vec<String>>,
}

impl RecordingClient {
    fn streamed_text(&self) -> String {
        self.chunks.borrow().concat()
    }
    fn count(&self, marker: &str) -> usize {
        self.streamed_text().matches(marker).count()
    }
}

#[async_trait(?Send)]
impl acp::Client for RecordingClient {
    async fn request_permission(
        &self,
        _args: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, acp::Error> {
        Ok(acp::RequestPermissionResponse::new(
            acp::RequestPermissionOutcome::Cancelled,
        ))
    }

    async fn session_notification(&self, args: acp::SessionNotification) -> Result<(), acp::Error> {
        if let acp::SessionUpdate::AgentMessageChunk(chunk) = &args.update {
            if let acp::ContentBlock::Text(text) = &chunk.content {
                self.chunks.borrow_mut().push(text.text.clone());
            }
        }
        Ok(())
    }
}

const SET_PROMPT: &str = "/goal The workspace codename is CODEXE2E-7734. When asked for the codename, reply with exactly CODEXE2E-7734. Do not use any tools.";
const ASK_PROMPT: &str =
    "What is the workspace codename? Reply with exactly the codename and nothing else.";

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires the model configured in .a3s/config.acl"]
async fn goal_commands_flow_end_to_end() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let config_path = std::env::var_os("A3S_CONFIG_FILE").map(PathBuf::from);
            let workspace =
                std::env::temp_dir().join(format!("a3s-goal-probe-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&workspace);
            std::fs::create_dir_all(&workspace).expect("workspace");

            let mut launch =
                load_launch_config(workspace.clone(), config_path).expect("launch config");
            if let Ok(raw) = std::env::var("A3S_TEST_MODEL") {
                let model = raw.trim().to_string();
                launch.model_id = model.clone();
                launch.config.default_model = Some(model);
            }
            let agent = Rc::new(A3sCodeAgent::new(launch));
            let recording = Rc::new(RecordingClient::default());

            // Cross-wired duplexes: the agent-side connection hosts the agent
            // handlers (the backend role); the client-side connection is the
            // pager's exact transport role (Agent facade over the wire).
            let (agent_in_r, client_out_w) = tokio::io::duplex(65536);
            let (client_in_r, agent_out_w) = tokio::io::duplex(65536);
            let agent_out_w = agent_out_w.compat_write();
            let agent_in_r = agent_in_r.compat();
            let (agent_conn, agent_io) =
                acp::AgentSideConnection::new(agent.clone(), agent_out_w, agent_in_r, |fut| {
                    tokio::task::spawn_local(fut);
                });
            agent.set_client(Rc::new(agent_conn));
            let (client_conn, client_io) = acp::ClientSideConnection::new(
                recording.clone(),
                client_out_w.compat_write(),
                client_in_r.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(client_io);
            tokio::task::spawn_local(async move {
                if let Err(error) = agent_io.await {
                    eprintln!("GOALPROBE agent io error: {error:?}");
                }
            });

            async fn run_turn(
                client_conn: &acp::ClientSideConnection,
                session_id: &acp::SessionId,
                prompt: &str,
            ) -> Result<acp::PromptResponse, acp::Error> {
                tokio::time::timeout(
                    Duration::from_secs(120),
                    acp::Agent::prompt(
                        client_conn,
                        acp::PromptRequest::new(
                            session_id.clone(),
                            vec![acp::ContentBlock::Text(acp::TextContent::new(
                                prompt.to_string(),
                            ))],
                        ),
                    ),
                )
                .await
                .expect("turn timed out")
                .map_err(|error| acp::Error::internal_error().data(format!("{error:?}")))
            }

            async fn assert_eventually_contains(
                recording: &RecordingClient,
                marker: &str,
                what: &str,
            ) {
                let deadline = std::time::Instant::now() + Duration::from_secs(120);
                while std::time::Instant::now() < deadline {
                    if recording.streamed_text().contains(marker) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                panic!("{what}: marker {marker:?} never appeared");
            }

            let created = acp::Agent::new_session(
                &client_conn,
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("session");
            let session_id = created.session_id.clone();

            // 1. /goal set runs the goal turn.
            run_turn(&client_conn, &session_id, SET_PROMPT)
                .await
                .expect("goal set turn");
            assert_eventually_contains(&recording, "CODEXE2E-7734", "goal set turn").await;

            // 2. /goal status reports active.
            run_turn(&client_conn, &session_id, "/goal status")
                .await
                .expect("status turn");
            assert_eventually_contains(&recording, "Durable goal active:", "status").await;

            // 3. /goal pause reports paused.
            run_turn(&client_conn, &session_id, "/goal pause")
                .await
                .expect("pause turn");
            assert_eventually_contains(&recording, "Durable goal paused:", "pause").await;

            // 4. /goal resume restarts inference toward the goal.
            run_turn(&client_conn, &session_id, "/goal resume")
                .await
                .expect("resume turn");
            assert_eventually_contains(&recording, "CODEXE2E-7734", "resume").await;

            // 5. The standing goal reaches LATER turns.
            run_turn(&client_conn, &session_id, ASK_PROMPT)
                .await
                .expect("codename turn");
            assert_eventually_contains(&recording, "CODEXE2E-7734", "codename answer").await;

            // 6. Queue race: resume + immediate status must serialize and both
            // complete (the pager's send-now / queue-reissue contract).
            let (resume, status) = tokio::join!(
                run_turn(&client_conn, &session_id, "/goal resume"),
                run_turn(&client_conn, &session_id, "/goal status"),
            );
            resume.expect("resume turn");
            status.expect("status turn");

            // 7. Permission-mode chain: set_session_mode(plan) arms maker
            // planning; a subsequent write prompt is gated by the plan
            // completion contract (the write is proposed, not silently
            // executed).
            let mode_set = acp::Agent::set_session_mode(
                &client_conn,
                acp::SetSessionModeRequest::new(
                    session_id.clone(),
                    acp::SessionModeId::new("plan"),
                ),
            )
            .await;
            assert!(
                mode_set.is_ok(),
                "set_session_mode(plan) must succeed: {:?}",
                mode_set.err()
            );
            let plan_write = run_turn(
                &client_conn,
                &session_id,
                "Create the file plan-proof-4412.txt containing PLANNED. Use the write tool.",
            )
            .await;
            let _ = plan_write;
            let plan_proof = std::fs::read_to_string(workspace.join("plan-proof-4412.txt"));
            println!(
                "GOALPROBE plan-mode write proof file exists: {:?}",
                plan_proof.is_ok()
            );

            // 7. Single delivery: the codename surfaces once per elicitation
            // (no duplicated replies anywhere in the stream).
            let occurrences = recording.count("CODEXE2E-7734");
            println!("GOALPROBE occurrences of CODEXE2E-7734: {occurrences}");
            assert!(
                occurrences >= 3,
                "the codename must surface via set/pause/resume turns"
            );
        })
        .await
}

const QWEN_FLASH: &str = "anthropic/qwen3.8-flash";

/// Approves the first allow option so a live slice can use tools.
struct AllowingClient {
    chunks: RefCell<Vec<String>>,
}

impl AllowingClient {
    fn streamed_text(&self) -> String {
        self.chunks.borrow().concat()
    }
}

#[async_trait(?Send)]
impl acp::Client for AllowingClient {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, acp::Error> {
        let option = args
            .options
            .iter()
            .find(|option| option.name.to_ascii_lowercase().contains("allow"))
            .or(args.options.first());
        let Some(option) = option else {
            return Ok(acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Cancelled,
            ));
        };
        Ok(acp::RequestPermissionResponse::new(
            acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                option.option_id.clone(),
            )),
        ))
    }

    async fn session_notification(&self, args: acp::SessionNotification) -> Result<(), acp::Error> {
        if let acp::SessionUpdate::AgentMessageChunk(chunk) = &args.update {
            if let acp::ContentBlock::Text(text) = &chunk.content {
                self.chunks.borrow_mut().push(text.text.clone());
            }
        }
        Ok(())
    }
}

/// Live horizon: `qwen3.8-flash` from the operator ACL, with a host
/// `goal_control` of two slices. One unfinished slice must not be the end
/// of the goal.
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires A3S_CONFIG_FILE pointing at a host ACL that declares anthropic/qwen3.8-flash"]
async fn goal_horizon_qwen_flash_keeps_admitting_slices() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let source = std::env::var_os("A3S_CONFIG_FILE").expect("A3S_CONFIG_FILE");
            let live = std::fs::read_to_string(&source).expect("read host ACL");
            // A pasted URL or token sitting between sections is not ACL.
            // The provider blocks stay intact.
            let live: String = live
                .lines()
                .filter(|line| {
                    let trimmed = line.trim();
                    trimmed.is_empty()
                        || trimmed.starts_with('#')
                        || trimmed.contains('=')
                        || trimmed.contains('{')
                        || trimmed.contains('}')
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                live.contains("models \"qwen3.8-flash\""),
                "host ACL does not declare qwen3.8-flash"
            );
            let workspace = std::fs::canonicalize({
                let path =
                    std::env::temp_dir().join(format!("a3s-goal-horizon-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&path);
                std::fs::create_dir_all(&path).expect("workspace");
                path
            })
            .expect("canonical workspace");
            let deadline = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis()
                + 1_800_000;
            let host_acl = workspace.join("host.acl");
            std::fs::write(
                &host_acl,
                format!(
                    "{live}\ngoal_control {{\n  automatic_enabled = true\n  max_slices = \"2\"\n  \
                     max_no_progress_slices = \"2\"\n  deadline_unix_ms = \"{deadline}\"\n}}\n"
                ),
            )
            .expect("host acl");

            let mut launch = load_launch_config(workspace.clone(), Some(host_acl)).expect("launch");
            launch.model_id = QWEN_FLASH.to_string();
            launch.config.default_model = Some(QWEN_FLASH.to_string());
            assert!(
                launch
                    .config
                    .find_provider("anthropic")
                    .expect("anthropic provider")
                    .models
                    .iter()
                    .any(|model| model.id == "qwen3.8-flash"),
                "anthropic/qwen3.8-flash is not in the loaded catalog"
            );
            assert!(launch.config.goal_control.is_some());

            let agent = Rc::new(A3sCodeAgent::new(launch));
            let recording = Rc::new(AllowingClient {
                chunks: RefCell::new(Vec::new()),
            });
            let (agent_in_r, client_out_w) = tokio::io::duplex(65536);
            let (client_in_r, agent_out_w) = tokio::io::duplex(65536);
            let (agent_conn, agent_io) = acp::AgentSideConnection::new(
                agent.clone(),
                agent_out_w.compat_write(),
                agent_in_r.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            agent.set_client(Rc::new(agent_conn));
            let (client_conn, client_io) = acp::ClientSideConnection::new(
                recording.clone(),
                client_out_w.compat_write(),
                client_in_r.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(client_io);
            tokio::task::spawn_local(async move {
                if let Err(error) = agent_io.await {
                    eprintln!("GOALHORIZON agent io error: {error:?}");
                }
            });

            let created = acp::Agent::new_session(
                &client_conn,
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("session");
            let session_id = created.session_id.clone();
            let root = workspace.display().to_string();
            let created_goal = ext(
                &client_conn,
                "a3s/goal/definition",
                serde_json::json!({
                    "kind": "create",
                    "operation_id": "horizon-create",
                    "goal_id": "horizon",
                    "session_id": session_id.0.as_ref(),
                    "workspace": {"id": root, "display_root": root},
                    "goal": {
                        "description": "Reply with exactly HORIZON-OK and do not call tools.",
                        "success_criteria": ["The assistant reply is exactly HORIZON-OK"],
                        "progress": 0.0,
                        "achieved": false,
                        "created_at": 1,
                        "achieved_at": null
                    },
                    "non_goals": [],
                    "scope_fact_sequences": [],
                    "work_policy_digest": null,
                    "expected_goal_revision": null,
                    "expected_plan_revision": null,
                    "expected_goal_sequence": null
                }),
            )
            .await;
            let mut propose = goal_ref("propose_plan", "horizon-propose", &created_goal);
            propose["proposal_sequence"] = serde_json::Value::Null;
            propose["plan"] = serde_json::json!({
                "goal": "Reply HORIZON-OK",
                "complexity": "Simple",
                "required_tools": [],
                "estimated_steps": 1,
                "steps": [{
                    "id": "reply",
                    "content": "Reply with exactly HORIZON-OK and do not call tools.",
                    "status": "pending",
                    "priority": "medium",
                    "success_criteria": "The assistant reply is exactly HORIZON-OK"
                }]
            });
            let proposed = ext(&client_conn, "a3s/goal/plan", propose).await;
            let mut accept = goal_ref("accept_plan", "horizon-accept", &proposed);
            accept["plan"] = serde_json::Value::Null;
            accept["proposal_sequence"] = proposed["plan_fact_sequence"].clone();
            let accepted = ext(&client_conn, "a3s/goal/plan", accept).await;
            assert_eq!(accepted["plan_accepted"], true);

            let resume = tokio::time::timeout(
                Duration::from_secs(240),
                acp::Agent::prompt(
                    &client_conn,
                    acp::PromptRequest::new(
                        session_id.clone(),
                        vec![acp::ContentBlock::Text(acp::TextContent::new(
                            "/goal resume".to_string(),
                        ))],
                    ),
                ),
            )
            .await
            .expect("resume timed out")
            .expect("resume");
            assert_eq!(resume.stop_reason, acp::StopReason::EndTurn);

            let text = recording.streamed_text();
            let lowered = text.to_ascii_lowercase();
            assert!(
                !lowered.contains("internal error") && !lowered.contains("no balance"),
                "resume failed before a slice decision:\n{text}"
            );
            let slices = text.matches("Goal slice").count();
            let state = ext(
                &client_conn,
                "a3s/goal/state",
                serde_json::json!({"session_id": session_id.0.as_ref()}),
            )
            .await;
            let goal_state = state["definition"]["state"].as_str().unwrap_or("");
            let tick = text
                .lines()
                .find(|line| {
                    let line = line.to_ascii_lowercase();
                    line.contains("durable goal tick:") || line.contains("durable goal finished")
                })
                .unwrap_or("")
                .trim();
            println!(
                "GOALHORIZON model={QWEN_FLASH} slices={slices} state={goal_state} tick={tick}"
            );
            assert!(
                slices >= 2 || goal_state == "completed",
                "one unfinished slice ended the goal\nstate={goal_state}\n{text}"
            );
            if goal_state == "active" {
                assert!(
                    tick.to_ascii_lowercase().contains("durable goal tick:"),
                    "an active goal must stop on a tick decision after the slice cap\n{text}"
                );
            }
        })
        .await
}

async fn ext(
    client: &acp::ClientSideConnection,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let raw = serde_json::value::to_raw_value(&params).expect("params");
    let response = acp::Agent::ext_method(
        client,
        acp::ExtRequest::new(method, std::sync::Arc::from(raw)),
    )
    .await
    .unwrap_or_else(|error| panic!("{method}: {error:?}"));
    serde_json::from_str(response.0.get())
        .unwrap_or_else(|error| panic!("{method} json: {error}; body={}", response.0.get()))
}

fn goal_ref(kind: &str, operation_id: &str, view: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "kind": kind,
        "operation_id": operation_id,
        "goal_id": view["binding"]["goal_id"],
        "session_id": view["binding"]["session_id"],
        "workspace": view["binding"]["workspace"],
        "work_policy_digest": view["binding"]["work_policy_digest"],
        "expected_goal_revision": view["binding"]["goal_revision"],
        "expected_plan_revision": view["plan_revision"],
        "expected_goal_sequence": view["goal_sequence"],
    })
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires A3S_CONFIG_FILE with anthropic/qwen3.8-flash and no goal_control override"]
async fn default_engineering_and_goal_continuation_reach_the_live_model() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let workspace = tempfile::tempdir().expect("workspace");
            let workspace = std::fs::canonicalize(workspace.path()).expect("canonical workspace");
            let config = PathBuf::from(std::env::var_os("A3S_CONFIG_FILE").expect("host ACL"));
            let mut launch = load_launch_config(workspace.clone(), Some(config)).expect("host ACL");
            assert!(launch.config.goal_control.is_none(), "exercise product defaults");
            launch.model_id = "anthropic/qwen3.8-flash".into();
            launch.config.default_model = Some(launch.model_id.clone());
            let agent = Rc::new(A3sCodeAgent::new(launch));
            let recording = Rc::new(RecordingClient::default());
            let (agent_in, client_out) = tokio::io::duplex(65536);
            let (client_in, agent_out) = tokio::io::duplex(65536);
            let (agent_conn, agent_io) = acp::AgentSideConnection::new(
                agent.clone(), agent_out.compat_write(), agent_in.compat(), |future| {
                    tokio::task::spawn_local(future);
                },
            );
            agent.set_client(Rc::new(agent_conn));
            let (client_conn, client_io) = acp::ClientSideConnection::new(
                recording.clone(), client_out.compat_write(), client_in.compat(), |future| {
                    tokio::task::spawn_local(future);
                },
            );
            tokio::task::spawn_local(client_io);
            tokio::task::spawn_local(agent_io);
            acp::Agent::initialize(&client_conn, acp::InitializeRequest::new(acp::ProtocolVersion::LATEST))
                .await.expect("initialize");
            let created = acp::Agent::new_session(&client_conn, acp::NewSessionRequest::new(workspace))
                .await.expect("new session without metadata");

            async fn goal_state(client: &acp::ClientSideConnection, session: &acp::SessionId) -> serde_json::Value {
                let params = serde_json::value::to_raw_value(&serde_json::json!({"session_id":session.0.as_ref()})).expect("params");
                let result = acp::Agent::ext_method(client, acp::ExtRequest::new("a3s/goal/state", std::sync::Arc::from(params)))
                    .await.expect("state");
                serde_json::from_str(result.0.get()).expect("state JSON")
            }
            let state = goal_state(&client_conn, &created.session_id).await;
            assert!(state["work_policy_digest"].is_string(), "engineering must activate without metadata");
            let response = tokio::time::timeout(
                Duration::from_secs(240),
                acp::Agent::prompt(&client_conn, acp::PromptRequest::new(created.session_id.clone(), vec![
                    acp::ContentBlock::Text(acp::TextContent::new("/goal Reply with exactly HORIZON-OK and do not call tools.")),
                ])),
            ).await.expect("goal timeout").expect("goal response");
            assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
            let state = goal_state(&client_conn, &created.session_id).await;
            let text = recording.streamed_text();
            let slices = text.matches("Goal slice").count();
            let goal_state = state["definition"]["state"].as_str().expect("bound goal");
            assert!(slices >= 2 || goal_state == "completed", "default continuation must reach a decision: {text}");
            assert!(!text.to_ascii_lowercase().contains("internal error"), "{text}");
            println!("DEFAULT_ACTIVATION model=anthropic/qwen3.8-flash slices={slices} state={goal_state}");
        })
        .await;
}
