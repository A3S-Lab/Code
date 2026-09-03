use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::{
    Agent, AgentEvent, CodeError, InterruptRequest, PlanningMode, RunControlError,
    RunControlOperation, RunControlReceiptState, RunStatus, SessionOptions, SteerRequest,
};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const STEER_INPUT: &str = "Replace the final answer with exactly: STEER_APPLIED";

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("fixture/controlled".into()),
        providers: vec![ProviderConfig {
            name: "fixture".into(),
            api_key: Some("offline".into()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "controlled".into(),
                name: "Controlled".into(),
                family: "fixture".into(),
                api_key: None,
                base_url: None,
                headers: HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

fn response(content: Vec<ContentBlock>, stop_reason: &str) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content,
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some(stop_reason.into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn tool_response() -> LlmResponse {
    response(
        vec![ContentBlock::ToolUse {
            id: "run-control-ls".into(),
            name: "ls".into(),
            input: serde_json::json!({}),
        }],
        "tool_use",
    )
}

fn text_response(text: &str) -> LlmResponse {
    response(vec![ContentBlock::Text { text: text.into() }], "end_turn")
}

#[derive(Default)]
struct CoordinatedSteerClient {
    calls: AtomicUsize,
    first_started: Arc<Notify>,
    release_first: Arc<Notify>,
    observed_messages: Mutex<Vec<Vec<Message>>>,
}

impl CoordinatedSteerClient {
    fn saw_steer(&self) -> bool {
        self.observed_messages
            .lock()
            .unwrap()
            .iter()
            .skip(1)
            .flatten()
            .any(|message| message.role == "user" && message.text().contains(STEER_INPUT))
    }
}

#[async_trait]
impl LlmClient for CoordinatedSteerClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        anyhow::bail!("run-control fixture requires streaming")
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.observed_messages
            .lock()
            .unwrap()
            .push(messages.to_vec());
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(2);
        if call == 0 {
            let release = Arc::clone(&self.release_first);
            self.first_started.notify_one();
            tokio::spawn(async move {
                tokio::select! {
                    _ = release.notified() => {
                        let _ = tx.send(StreamEvent::Done(tool_response())).await;
                    }
                    _ = cancel_token.cancelled() => {}
                }
            });
        } else {
            let saw_steer = messages
                .iter()
                .any(|message| message.role == "user" && message.text().contains(STEER_INPUT));
            tokio::spawn(async move {
                let text = if saw_steer {
                    "STEER_APPLIED"
                } else {
                    "STEER_MISSING"
                };
                let _ = tx.send(StreamEvent::TextDelta(text.into())).await;
                let _ = tx.send(StreamEvent::Done(text_response(text))).await;
            });
        }
        Ok(rx)
    }
}

#[derive(Default)]
struct PendingInterruptClient {
    started: Arc<Notify>,
}

#[async_trait]
impl LlmClient for PendingInterruptClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        anyhow::bail!("run-control fixture requires streaming")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let (tx, rx) = mpsc::channel(2);
        self.started.notify_one();
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::TextDelta("working".into())).await;
            cancel_token.cancelled().await;
        });
        Ok(rx)
    }
}

async fn session_with_client(
    workspace: &std::path::Path,
    client: Arc<dyn LlmClient>,
    session_id: &str,
) -> a3s_code_core::AgentSession {
    let agent = Agent::from_config(offline_config()).await.unwrap();
    agent
        .session_async(
            workspace.display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id(session_id)
                    .with_llm_client(client)
                    .with_permission_policy(PermissionPolicy::new().allow("ls(*)"))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_continuation(false)
                    .with_max_tool_rounds(3),
            ),
        )
        .await
        .unwrap()
}

async fn active_snapshot(
    session: &a3s_code_core::AgentSession,
) -> a3s_code_core::RunControlSnapshot {
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if let Some(snapshot) = session.run_control_snapshot().await {
                if snapshot.turn_id.is_some() {
                    return snapshot;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("active run-control snapshot timed out")
}

#[tokio::test(flavor = "multi_thread")]
async fn steer_crosses_the_public_session_boundary_at_the_next_safe_point() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(CoordinatedSteerClient::default());
    let session = session_with_client(workspace.path(), client.clone(), "steer-runtime").await;
    let (mut events, worker) = session
        .stream("List this workspace, then answer.", None)
        .await
        .unwrap();

    tokio::time::timeout(TEST_TIMEOUT, client.first_started.notified())
        .await
        .expect("first model turn did not start");
    let snapshot = active_snapshot(&session).await;
    let mut request = SteerRequest::new(STEER_INPUT)
        .with_run_id(snapshot.run_id.clone())
        .with_expected_turn(
            snapshot.turn_id.clone().expect("turn id"),
            snapshot.turn_revision,
        );
    request.request_id = Some("steer-runtime-request".into());
    let accepted = session.steer(request.clone()).await.unwrap();
    assert_eq!(accepted.state, RunControlReceiptState::Accepted);
    assert_eq!(session.steer(request).await.unwrap(), accepted);
    client.release_first.notify_one();

    let mut saw_applied = false;
    let mut final_text = String::new();
    tokio::time::timeout(TEST_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::RunControlApplied {
                    request_id,
                    operation,
                    input,
                    ..
                } => {
                    assert_eq!(request_id, "steer-runtime-request");
                    assert_eq!(operation, RunControlOperation::Steer);
                    assert_eq!(input.as_deref(), Some(STEER_INPUT));
                    saw_applied = true;
                }
                AgentEvent::End { text, .. } => {
                    final_text = text;
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("steered run did not finish");
    worker.await.unwrap();

    assert!(saw_applied, "stream omitted RunControlApplied");
    assert_eq!(final_text, "STEER_APPLIED");
    assert!(
        client.saw_steer(),
        "second model turn omitted the steer input"
    );
    let run = session.run_snapshot(&snapshot.run_id).await.unwrap();
    assert_eq!(run.status, RunStatus::Completed);
    assert!(session
        .run_events(&snapshot.run_id)
        .await
        .iter()
        .any(|record| {
            matches!(
                &record.event,
                AgentEvent::RunControlApplied { request_id, .. }
                    if request_id == "steer-runtime-request"
            )
        }));
    assert!(session.run_control_snapshot().await.is_none());
    assert!(matches!(
        session.steer(SteerRequest::new("too late")).await,
        Err(CodeError::RunControl(RunControlError::NoActiveRun))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupt_crosses_the_public_session_boundary_and_settles_the_run() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(PendingInterruptClient::default());
    let session = session_with_client(workspace.path(), client.clone(), "interrupt-runtime").await;
    let (mut events, worker) = session
        .stream("Keep working until stopped.", None)
        .await
        .unwrap();

    tokio::time::timeout(TEST_TIMEOUT, client.started.notified())
        .await
        .expect("pending model turn did not start");
    let snapshot = active_snapshot(&session).await;
    let receipt = session
        .interrupt(
            InterruptRequest::new()
                .with_reason("host requested stop")
                .with_run_id(snapshot.run_id.clone())
                .with_expected_turn(
                    snapshot.turn_id.clone().expect("turn id"),
                    snapshot.turn_revision,
                ),
        )
        .await
        .unwrap();
    assert_eq!(receipt.state, RunControlReceiptState::Accepted);

    let mut saw_applied = false;
    tokio::time::timeout(TEST_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            if matches!(
                event,
                AgentEvent::RunControlApplied {
                    operation: RunControlOperation::Interrupt,
                    ..
                }
            ) {
                saw_applied = true;
            }
        }
    })
    .await
    .expect("interrupted stream did not settle");
    worker.await.unwrap();

    assert!(saw_applied, "stream omitted interrupt application evidence");
    let run = session.run_snapshot(&snapshot.run_id).await.unwrap();
    assert_eq!(run.status, RunStatus::Cancelled);
    assert!(session.run_control_snapshot().await.is_none());
}
