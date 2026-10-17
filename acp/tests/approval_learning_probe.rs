//! Approval-learning probe over the REAL pager path: an in-process
//! ClientSideConnection (the pager's exact transport role) drives the
//! A3sCodeAgent backend across cross-wired duplexes, with a client that
//! counts every permission round-trip and answers `allow-always`.
//!
//! Contracts asserted:
//! 1. A turn whose model call runs `make check` asks the client exactly
//!    once; the `allow-always` answer persists the `make check` rule to
//!    `<A3S_STATE_HOME>/code/approvals.acl` (parseable ACL, entry present)
//!    and writes nothing into the workspace `.a3s`.
//! 2. A NEW session in the SAME workspace runs the same rule with ZERO
//!    further permission round-trips.
//! 3. The second session's stream carries the target's real output
//!    (`make-check-ran`), so "no round-trip" cannot pass vacuously — the
//!    tool provably ran.
//!
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-acp --test approval_learning_probe -- --ignored --nocapture --test-threads=1

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use a3s_code_acp::{load_launch_config, A3sCodeAgent};
use agent_client_protocol as acp;
use async_trait::async_trait;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Records streamed text and every permission round-trip, answering each
/// one as an interactive user approving the call (the allow option).
#[derive(Default)]
struct ApprovingClient {
    chunks: RefCell<Vec<String>>,
    permission_requests: std::cell::Cell<usize>,
}

impl ApprovingClient {
    fn streamed_text(&self) -> String {
        self.chunks.borrow().concat()
    }
}

#[async_trait(?Send)]
impl acp::Client for ApprovingClient {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, acp::Error> {
        self.permission_requests
            .set(self.permission_requests.get() + 1);
        let ids: Vec<String> = args
            .options
            .iter()
            .map(|option| option.option_id.0.to_string())
            .collect();
        let allow = ["allow-always", "allow-once"]
            .into_iter()
            .find(|id| ids.iter().any(|offered| offered == id))
            .unwrap_or("cancel")
            .to_string();
        if allow == "cancel" {
            return Ok(acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Cancelled,
            ));
        }
        Ok(acp::RequestPermissionResponse::new(
            acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                acp::PermissionOptionId::new(allow),
            )),
        ))
    }

    async fn session_notification(&self, args: acp::SessionNotification) -> Result<(), acp::Error> {
        if let acp::SessionUpdate::AgentMessageChunk(chunk) = &args.update {
            if let acp::ContentBlock::Text(text) = &chunk.content {
                self.chunks.borrow_mut().push(text.text.clone());
            }
        }
        // Tool output may also stream; capture it so the probe can see the
        // real `make-check-ran` line even when the model paraphrases.
        if let acp::SessionUpdate::ToolCallUpdate(update) = &args.update {
            for block in update.fields.content.iter().flatten() {
                if let acp::ToolCallContent::Content(content) = block {
                    if let acp::ContentBlock::Text(text) = &content.content {
                        self.chunks.borrow_mut().push(text.text.clone());
                    }
                }
            }
        }
        Ok(())
    }
}

async fn assert_eventually_contains(client: &ApprovingClient, marker: &str, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    while std::time::Instant::now() < deadline {
        if client.streamed_text().contains(marker) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("{what}: marker {marker:?} never appeared");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires the model configured in .a3s/config.acl"]
async fn allow_always_rule_skips_confirmation_in_later_sessions() {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let config_path = std::env::var_os("A3S_CONFIG_FILE").map(PathBuf::from);
            let workspace =
                std::env::temp_dir().join(format!("a3s-approval-probe-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&workspace);
            std::fs::create_dir_all(&workspace).expect("workspace");
            std::fs::write(
                workspace.join("Makefile"),
                "check:\n\t@echo make-check-ran\n",
            )
            .expect("Makefile");
            let state = workspace.with_extension("state");
            let _ = std::fs::remove_dir_all(&state);
            std::env::set_var("A3S_STATE_HOME", &state);

            let mut launch =
                load_launch_config(workspace.clone(), config_path).expect("launch config");
            if let Ok(raw) = std::env::var("A3S_TEST_MODEL") {
                let model = raw.trim().to_string();
                launch.model_id = model.clone();
                launch.config.default_model = Some(model);
            }
            let agent = Rc::new(A3sCodeAgent::new(launch));
            let client = Rc::new(ApprovingClient::default());

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
                client.clone(),
                client_out_w.compat_write(),
                client_in_r.compat(),
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );
            tokio::task::spawn_local(client_io);
            tokio::task::spawn_local(async move {
                if let Err(error) = agent_io.await {
                    eprintln!("APPROVALPROBE agent io error: {error:?}");
                }
            });

            async fn run_turn(
                client_conn: &acp::ClientSideConnection,
                session_id: &acp::SessionId,
                prompt: &str,
            ) -> Result<acp::PromptResponse, acp::Error> {
                tokio::time::timeout(
                    Duration::from_secs(180),
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

            // Session 1: the allow-always answer that teaches the rule.
            let created = acp::Agent::new_session(
                &client_conn,
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("session one");
            let first = created.session_id.clone();
            run_turn(
                &client_conn,
                &first,
                "Use the bash tool to run exactly this command: make check. \
                 Then report the command's output verbatim.",
            )
            .await
            .expect("first turn");
            assert_eventually_contains(&client, "make-check-ran", "first turn output").await;
            assert_eq!(
                client.permission_requests.get(),
                1,
                "the first session's bash call must ask the client exactly once"
            );

            // The allow-always answer persisted the rule in the user state root.
            let approvals = state.join("code").join("approvals.acl");
            let raw = std::fs::read_to_string(&approvals)
                .unwrap_or_else(|e| panic!("approvals.acl must exist after allow-always: {e}"));
            assert!(a3s_acl::parse_acl(&raw).is_ok(), "raw: {raw}");
            assert!(raw.contains("\"make check\""), "raw: {raw}");
            assert!(
                !workspace.join(".a3s").join("approvals.acl").exists(),
                "approvals must not be written into the workspace"
            );

            // Session 2 (same workspace, same agent process): the learned
            // rule admits the call before the client is consulted.
            let created2 = acp::Agent::new_session(
                &client_conn,
                acp::NewSessionRequest::new(workspace.clone()),
            )
            .await
            .expect("session two");
            let second = created2.session_id.clone();
            run_turn(
                &client_conn,
                &second,
                "Use the bash tool to run exactly this command: make check. \
                 Then report the command's output verbatim.",
            )
            .await
            .expect("second turn");
            assert_eventually_contains(&client, "make-check-ran", "second turn output").await;
            assert_eq!(
                client.permission_requests.get(),
                1,
                "the second session's make call must skip the round-trip via the learned rule"
            );

            println!(
                "APPROVALPROBE permission round-trips: {} (1 teaching, 0 learned)",
                client.permission_requests.get()
            );
        })
        .await
}
