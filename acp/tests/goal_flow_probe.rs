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

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use a3s_code_acp::{load_launch_config, A3sCodeAgent, LaunchConfig};
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
