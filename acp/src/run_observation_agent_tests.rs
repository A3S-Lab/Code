#![cfg(test)]

use super::*;
use a3s_code_core::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, Semaphore};
use tokio::time::{timeout, Duration};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

struct Host(mpsc::UnboundedSender<Value>);

#[async_trait::async_trait(?Send)]
impl acp::Client for Host {
    async fn request_permission(
        &self,
        _: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        Err(acp::Error::internal_error())
    }

    async fn session_notification(&self, _: acp::SessionNotification) -> acp::Result<()> {
        Ok(())
    }

    async fn ext_notification(&self, args: acp::ExtNotification) -> acp::Result<()> {
        assert_eq!(args.method.as_ref(), crate::run_observation::METHOD);
        self.0
            .send(serde_json::from_str(args.params.get()).unwrap())
            .unwrap();
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Complete,
    ToolRound,
    Fail,
    Wait,
}

struct Model {
    mode: Mode,
    entered: Arc<Semaphore>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl LlmClient for Model {
    async fn complete(
        &self,
        messages: &[Message],
        _: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        match self.mode {
            Mode::Complete => {}
            Mode::ToolRound if tools.iter().any(|tool| tool.name == "read") => {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Ok(LlmResponse {
                        message: Message {
                            content: vec![ContentBlock::ToolUse {
                                id: "read-probe".into(),
                                name: "read".into(),
                                input: json!({"file_path":"note.txt"}),
                            }],
                            ..Message::assistant("")
                        },
                        usage: TokenUsage::default(),
                        stop_reason: None,
                        token_logprobs: vec![],
                        meta: None,
                    });
                }
                assert!(messages.iter().flat_map(|message| &message.content).any(
                    |block| matches!(block,
                    ContentBlock::ToolResult { content, is_error, .. }
                    if *is_error != Some(true) && content.as_text().contains("actual read marker"))
                ));
            }
            Mode::ToolRound => {}
            Mode::Fail => anyhow::bail!("model failure fixture"),
            Mode::Wait => {
                self.entered.add_permits(1);
                std::future::pending::<()>().await;
            }
        }
        Ok(LlmResponse {
            message: Message::assistant("private model result"),
            usage: TokenUsage::default(),
            stop_reason: None,
            token_logprobs: vec![],
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("non-streaming model fixture")
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    client: Rc<acp::ClientSideConnection>,
    session_id: acp::SessionId,
    session: Arc<AgentSession>,
    packets: mpsc::UnboundedReceiver<Value>,
    entered: Arc<Semaphore>,
    io: Vec<JoinHandle<()>>,
}

impl Fixture {
    async fn new(mode: Mode) -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("note.txt"), "actual read marker").unwrap();
        let mut launch = test_launch(Some(root.path().join("sessions")));
        launch.workspace = root.path().to_owned();
        let entered = Arc::new(Semaphore::new(0));
        let core = Agent::from_config(launch.config.clone()).await.unwrap();
        let session = Arc::new(
            core.session_async(
                root.path().display().to_string(),
                Some(
                    SessionOptions::new()
                        .with_llm_client(Arc::new(Model {
                            mode,
                            entered: entered.clone(),
                            calls: AtomicUsize::new(0),
                        }))
                        .with_permission_policy(
                            a3s_code_core::permissions::PermissionPolicy::new().allow("read"),
                        )
                        .with_planning_mode(a3s_code_core::PlanningMode::Disabled),
                ),
            )
            .await
            .unwrap(),
        );
        let agent = Rc::new(A3sCodeAgent::new(launch));
        let (left, right) = tokio::io::duplex(1024 * 1024);
        let (ar, aw) = tokio::io::split(left);
        let (cr, cw) = tokio::io::split(right);
        let (connection, agent_io) = acp::AgentSideConnection::new(
            agent.clone(),
            aw.compat_write(),
            ar.compat(),
            |future| {
                tokio::task::spawn_local(future);
            },
        );
        agent.set_client(Rc::new(connection));
        let (sender, packets) = mpsc::unbounded_channel();
        let (client, client_io) = acp::ClientSideConnection::new(
            Host(sender),
            cw.compat_write(),
            cr.compat(),
            |future| {
                tokio::task::spawn_local(future);
            },
        );
        let io = vec![
            tokio::task::spawn_local(async move {
                let _ = agent_io.await;
            }),
            tokio::task::spawn_local(async move {
                let _ = client_io.await;
            }),
        ];
        let client = Rc::new(client);
        let opened =
            acp::Agent::new_session(client.as_ref(), acp::NewSessionRequest::new(root.path()))
                .await
                .unwrap();
        let original = {
            let mut sessions = agent.sessions.lock().await;
            let live = sessions.get_mut(opened.session_id.0.as_ref()).unwrap();
            std::mem::replace(&mut live.session, session.clone())
        };
        original.close().await;
        Self {
            _root: root,
            client,
            session_id: opened.session_id,
            session,
            packets,
            entered,
            io,
        }
    }

    fn request(&self, prompt_id: &str, text: &str, enabled: bool) -> acp::PromptRequest {
        let mut request = acp::PromptRequest::new(
            self.session_id.clone(),
            vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
        );
        let mut meta = acp::Meta::new();
        meta.insert("promptId".into(), json!(prompt_id));
        if enabled {
            meta.insert("a3sExecutionObservation".into(), json!(1));
        }
        request.meta = Some(meta);
        request
    }

    async fn closed_run(&mut self) -> Vec<Value> {
        let mut packets = Vec::new();
        loop {
            let value = timeout(Duration::from_secs(5), self.packets.recv())
                .await
                .unwrap()
                .unwrap();
            let closed = value["kind"] == "run-closed";
            packets.push(value);
            assert!(packets.len() <= 32, "unbounded observation stream");
            if closed {
                return packets;
            }
        }
    }

    async fn stop(self) {
        self.session.close().await;
        for task in self.io {
            task.abort();
        }
    }
}

fn assert_run(
    packets: &[Value],
    run: &a3s_code_core::run::RunSnapshot,
    session_id: &acp::SessionId,
    prompt_id: &str,
) {
    let fields = [
        "protocolVersion",
        "sessionId",
        "promptId",
        "coreSessionId",
        "runId",
        "sequence",
        "kind",
        "turn",
        "status",
    ];
    for (index, packet) in packets.iter().enumerate() {
        assert_eq!(packet["protocolVersion"], 1);
        assert_eq!(packet["sessionId"], session_id.0.as_ref());
        assert_eq!(packet["promptId"], prompt_id);
        assert_eq!(packet["coreSessionId"], run.session_id);
        assert_eq!(packet["runId"], run.id);
        assert_eq!(packet["sequence"], index + 1);
        let object = packet.as_object().unwrap();
        assert_eq!(
            object.len(),
            fields.len(),
            "execution observations disclose no prompt, model output or resource grants"
        );
        assert!(fields.iter().all(|field| object.contains_key(*field)));
    }
    assert_eq!(packets.first().unwrap()["kind"], "run-opened");
    assert_eq!(packets.last().unwrap()["kind"], "run-closed");
    assert_eq!(
        packets.last().unwrap()["status"],
        serde_json::to_value(run.status).unwrap()
    );
}

// Catch identity guessed after settlement, cross-prompt reuse, or opt-in bypass on the actual ACP transport.
#[tokio::test]
async fn actual_acp_prompt_projects_core_runs_and_keeps_commands_and_legacy_clients_unobserved() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut fixture = Fixture::new(Mode::Complete).await;
            let mut retained = Vec::new();
            for prompt in ["first-prompt", "second-prompt"] {
                acp::Agent::prompt(
                    fixture.client.as_ref(),
                    fixture.request(prompt, "private prompt", true),
                )
                .await
                .unwrap();
                let packets = fixture.closed_run().await;
                let runs = fixture.session.runs().await;
                let run = runs.last().unwrap();
                assert_run(&packets, run, &fixture.session_id, prompt);
                assert_eq!(run.status, a3s_code_core::run::RunStatus::Completed);
                assert_eq!(
                    packets
                        .iter()
                        .map(|p| p["kind"].as_str().unwrap())
                        .collect::<Vec<_>>(),
                    [
                        "run-opened",
                        "model-turn-opened",
                        "model-turn-closed",
                        "run-closed"
                    ]
                );
                assert_eq!(packets[1]["turn"], 1);
                assert_eq!(packets[2]["turn"], 1);
                assert!(fixture.session.current_run().await.is_none());
                retained.extend(packets);
            }
            assert_ne!(retained[0]["runId"], retained[4]["runId"]);
            for (prompt, text, enabled) in [
                ("local-command", "/help", true),
                ("legacy-prompt", "legacy", false),
            ] {
                acp::Agent::prompt(
                    fixture.client.as_ref(),
                    fixture.request(prompt, text, enabled),
                )
                .await
                .unwrap();
                assert!(timeout(Duration::from_millis(50), fixture.packets.recv())
                    .await
                    .is_err());
            }
            assert_eq!(
                fixture.session.runs().await.len(),
                3,
                "help has no model Run; legacy execution still has its real Core Run"
            );
            fixture.stop().await;
            let mut fixture = Fixture::new(Mode::ToolRound).await;
            acp::Agent::prompt(
                fixture.client.as_ref(),
                fixture.request("tool-round-prompt", "Read note.txt", true),
            )
            .await
            .unwrap();
            let packets = fixture.closed_run().await;
            let runs = fixture.session.runs().await;
            assert_eq!(runs.len(), 1);
            assert_run(&packets, &runs[0], &fixture.session_id, "tool-round-prompt");
            assert_eq!(
                packets
                    .iter()
                    .map(|p| p["kind"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                [
                    "run-opened",
                    "model-turn-opened",
                    "model-turn-closed",
                    "model-turn-opened",
                    "model-turn-closed",
                    "run-closed"
                ]
            );
            assert_eq!(packets[1]["turn"], 1);
            assert_eq!(packets[3]["turn"], 2);
            retained.extend(packets);
            if let Some(path) = std::env::var_os("A3S_CLOUD_TEST_AIX_EXECUTION_OBSERVATIONS") {
                let lines = retained
                    .iter()
                    .map(|value| serde_json::to_string(value).unwrap())
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
                std::fs::write(path, lines).unwrap();
            }
            fixture.stop().await;
        })
        .await;
}

// Catch an error or ACP cancellation being reported as completed, or leaving an admitted observation open.
#[tokio::test]
async fn actual_acp_failure_and_cancel_close_only_with_the_core_terminal_outcome() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for (mode, expected) in [
                (Mode::Fail, a3s_code_core::run::RunStatus::Failed),
                (Mode::Wait, a3s_code_core::run::RunStatus::Cancelled),
            ] {
                let mut fixture = Fixture::new(mode).await;
                let request = fixture.request("terminal-prompt", "private terminal prompt", true);
                let client = fixture.client.clone();
                let prompt = tokio::task::spawn_local(async move {
                    acp::Agent::prompt(client.as_ref(), request).await
                });
                if matches!(mode, Mode::Wait) {
                    timeout(Duration::from_secs(5), fixture.entered.acquire())
                        .await
                        .unwrap()
                        .unwrap()
                        .forget();
                    acp::Agent::cancel(
                        fixture.client.as_ref(),
                        acp::CancelNotification::new(fixture.session_id.clone()),
                    )
                    .await
                    .unwrap();
                }
                let response = timeout(Duration::from_secs(5), prompt)
                    .await
                    .unwrap()
                    .unwrap();
                if matches!(mode, Mode::Fail) {
                    assert!(response.is_err());
                } else {
                    assert_eq!(response.unwrap().stop_reason, acp::StopReason::Cancelled);
                }
                let packets = fixture.closed_run().await;
                let runs = fixture.session.runs().await;
                let run = runs.last().unwrap();
                assert_eq!(run.status, expected);
                assert_run(&packets, run, &fixture.session_id, "terminal-prompt");
                fixture.stop().await;
            }
        })
        .await;
}
