use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::store::{MemorySessionStore, SessionStore};
use a3s_code_core::{
    Agent, AgentProtocolChangeSetRequestV1, AgentProtocolChangeSetV1, AgentProtocolCommandV1,
    AgentProtocolEventPageRequestV1, AgentProtocolHarness, AgentProtocolHarnessError,
    AgentProtocolRunIdentityV1, AgentProtocolRunStartV1, AgentProtocolRunStateV1, PlanningMode,
    SessionOptions, AGENT_PROTOCOL_V1,
};
use base64::Engine as _;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct StaticStreamingClient;

#[derive(Clone)]
struct ScriptedStreamingClient {
    responses: Arc<std::sync::Mutex<Vec<LlmResponse>>>,
}

impl ScriptedStreamingClient {
    fn new(mut responses: Vec<LlmResponse>) -> Self {
        responses.reverse();
        Self {
            responses: Arc::new(std::sync::Mutex::new(responses)),
        }
    }

    fn next(&self) -> anyhow::Result<LlmResponse> {
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow::anyhow!("scripted Harness client exhausted"))
    }
}

#[async_trait::async_trait]
impl LlmClient for StaticStreamingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(response())
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (sender, receiver) = mpsc::channel(4);
        tokio::spawn(async move {
            let _ = sender.send(StreamEvent::TextDelta("done".into())).await;
            let _ = sender.send(StreamEvent::Done(response())).await;
        });
        Ok(receiver)
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedStreamingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.next()
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let response = self.next()?;
        let (sender, receiver) = mpsc::channel(4);
        tokio::spawn(async move {
            let text = response.text();
            if !text.is_empty() {
                let _ = sender.send(StreamEvent::TextDelta(text)).await;
            }
            let _ = sender.send(StreamEvent::Done(response)).await;
        });
        Ok(receiver)
    }
}

fn response() -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text {
                text: "done".into(),
            }],
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("end_turn".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn tool_response(name: &str, input: serde_json::Value) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::ToolUse {
                id: "tool-write-1".into(),
                name: name.into(),
                input,
            }],
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("tool_use".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn git(workspace: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .expect("run Git fixture command");
    assert!(
        output.status.success(),
        "Git fixture command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn initialize_git_workspace(workspace: &std::path::Path) {
    git(workspace, &["init"]);
    git(workspace, &["config", "user.name", "A3S Test"]);
    git(workspace, &["config", "user.email", "test@a3s.invalid"]);
    std::fs::write(workspace.join("seed.txt"), "seed\n").expect("write seed file");
    git(workspace, &["add", "seed.txt"]);
    git(workspace, &["commit", "-m", "seed"]);
}

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("fixture/static".into()),
        providers: vec![ProviderConfig {
            name: "fixture".into(),
            api_key: Some("offline".into()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "static".into(),
                name: "Static".into(),
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

fn manifest() -> a3s_code_core::release::AgentReleaseManifest {
    a3s_code_core::release::AgentReleaseManifest::parse(include_str!(
        "../../fixtures/agent-release-contract/.a3s/asset.acl"
    ))
    .unwrap()
}

fn start(release_identity: &str, session_id: &str, run_id: &str) -> AgentProtocolCommandV1 {
    AgentProtocolCommandV1::Start {
        request: AgentProtocolRunStartV1 {
            schema: AgentProtocolRunStartV1::SCHEMA.into(),
            request_id: format!("{run_id}:start"),
            identity: AgentProtocolRunIdentityV1 {
                schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
                protocol: AGENT_PROTOCOL_V1.into(),
                agent_release_identity: release_identity.into(),
                session_id: session_id.into(),
                run_id: run_id.into(),
            },
            prompt: format!("execute {run_id}"),
        },
    }
}

async fn wait_for_terminal(
    harness: &AgentProtocolHarness,
    command: &AgentProtocolCommandV1,
) -> a3s_code_core::AgentProtocolEventPageV1 {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let page = harness
                .event_page(&AgentProtocolEventPageRequestV1 {
                    schema: AgentProtocolEventPageRequestV1::SCHEMA.into(),
                    identity: command.identity().clone(),
                    after_event_sequence: None,
                    limit: 64,
                })
                .await
                .unwrap();
            if page.state.is_terminal() {
                break page;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached Harness run must terminate")
}

async fn wait_for_change_set(
    harness: &AgentProtocolHarness,
    command: &AgentProtocolCommandV1,
) -> AgentProtocolChangeSetV1 {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match harness
                .change_set(&AgentProtocolChangeSetRequestV1 {
                    schema: AgentProtocolChangeSetRequestV1::SCHEMA.into(),
                    identity: command.identity().clone(),
                })
                .await
            {
                Ok(change_set) => break change_set,
                Err(AgentProtocolHarnessError::Host(
                    a3s_code_core::AgentProtocolHostError::ChangeSetPending,
                )) => tokio::task::yield_now().await,
                Err(error) => panic!("unexpected change-set error: {error}"),
            }
        }
    })
    .await
    .expect("change set must be captured after terminal state")
}

#[tokio::test]
async fn harness_multiplexes_sessions_through_code_owned_hosts() {
    let workspace = tempfile::tempdir().unwrap();
    let manifest = manifest();
    let identity = manifest.artifact().digest().to_string();
    let agent = Arc::new(Agent::from_config(offline_config()).await.unwrap());
    let harness = AgentProtocolHarness::new(
        manifest,
        Arc::clone(&agent),
        workspace.path().display().to_string(),
    )
    .unwrap()
    .with_session_options(SessionOptions::new().with_llm_client(Arc::new(StaticStreamingClient)));
    let first = start(&identity, "conversation-one", "execution-one");
    let second = start(&identity, "conversation-two", "execution-two");

    harness.execute(&first).await.unwrap();
    harness.execute(&second).await.unwrap();
    assert_eq!(
        wait_for_terminal(&harness, &first)
            .await
            .identity
            .session_id,
        "conversation-one"
    );
    assert_eq!(
        wait_for_terminal(&harness, &second)
            .await
            .identity
            .session_id,
        "conversation-two"
    );
    assert_eq!(harness.session_count().await, 2);
    assert_eq!(agent.list_sessions().await.len(), 2);

    harness.close().await;
    assert!(agent.is_closed());
}

#[tokio::test]
async fn harness_isolates_sessions_and_exports_one_digest_bound_run_patch() {
    let workspace = tempfile::tempdir().unwrap();
    initialize_git_workspace(workspace.path());
    let manifest = manifest();
    let release_identity = manifest.artifact().digest().to_string();
    let client = ScriptedStreamingClient::new(vec![
        tool_response(
            "write",
            serde_json::json!({
                "file_path": "remote.txt",
                "content": "remote change\n"
            }),
        ),
        response(),
    ]);
    let harness = AgentProtocolHarness::new(
        manifest,
        Arc::new(Agent::from_config(offline_config()).await.unwrap()),
        workspace.path().display().to_string(),
    )
    .unwrap()
    .with_session_options(
        SessionOptions::new()
            .with_planning_mode(PlanningMode::Disabled)
            .with_confirmation_manager(Arc::new(a3s_code_core::hitl::AutoApproveConfirmation))
            .with_llm_client(Arc::new(client)),
    );
    let command = start(
        &release_identity,
        "changes-conversation",
        "changes-execution",
    );

    harness.execute(&command).await.unwrap();
    assert_eq!(
        wait_for_terminal(&harness, &command).await.state,
        AgentProtocolRunStateV1::Completed
    );
    let change_set = wait_for_change_set(&harness, &command).await;
    change_set.validate().unwrap();
    let patch = base64::engine::general_purpose::STANDARD
        .decode(&change_set.patch_base64)
        .unwrap();
    let patch = String::from_utf8(patch).unwrap();
    assert!(
        patch.contains("diff --git a/remote.txt b/remote.txt"),
        "{patch}"
    );
    assert!(patch.contains("+remote change"), "{patch}");
    assert_eq!(change_set.patch_bytes as usize, patch.len());
    assert!(!workspace.path().join("remote.txt").exists());

    harness.close().await;
    git(workspace.path(), &["status", "--porcelain"]);
}

#[tokio::test]
async fn harness_resumes_the_code_store_before_replaying_a_start_after_restart() {
    let workspace = tempfile::tempdir().unwrap();
    initialize_git_workspace(workspace.path());
    let store = Arc::new(MemorySessionStore::new());
    let release = manifest();
    let release_identity = release.artifact().digest().to_string();
    let command = start(
        &release_identity,
        "durable-conversation",
        "durable-execution",
    );
    let first_client = ScriptedStreamingClient::new(vec![
        tool_response(
            "write",
            serde_json::json!({
                "file_path": "remote.txt",
                "content": "survives restart\n"
            }),
        ),
        response(),
    ]);

    let first_agent = Arc::new(Agent::from_config(offline_config()).await.unwrap());
    let first = AgentProtocolHarness::new(
        release.clone(),
        first_agent,
        workspace.path().display().to_string(),
    )
    .unwrap()
    .with_session_options(
        SessionOptions::new()
            .with_session_store(store.clone() as Arc<dyn SessionStore>)
            .with_planning_mode(PlanningMode::Disabled)
            .with_confirmation_manager(Arc::new(a3s_code_core::hitl::AutoApproveConfirmation))
            .with_llm_client(Arc::new(first_client)),
    );
    let receipt = first.execute(&command).await.unwrap();
    assert!(!receipt.replayed);
    assert_eq!(
        wait_for_terminal(&first, &command).await.state,
        AgentProtocolRunStateV1::Completed
    );
    wait_for_change_set(&first, &command).await;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if store
                .load_snapshot("durable-conversation")
                .await
                .unwrap()
                .is_some_and(|snapshot| {
                    snapshot.run_records.iter().any(|record| {
                        record.snapshot.id == "durable-execution"
                            && record.snapshot.status == a3s_code_core::RunStatus::Completed
                            && record.snapshot.workspace_change_set.is_some()
                    })
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal run must be persisted before restart");
    first.close().await;
    git(workspace.path(), &["gc", "--prune=now"]);

    let second_agent = Arc::new(Agent::from_config(offline_config()).await.unwrap());
    let second_client = ScriptedStreamingClient::new(vec![
        tool_response(
            "write",
            serde_json::json!({
                "file_path": "follow-up.txt",
                "content": "second run\n"
            }),
        ),
        response(),
    ]);
    let second = AgentProtocolHarness::new(
        release,
        second_agent,
        workspace.path().display().to_string(),
    )
    .unwrap()
    .with_session_options(
        SessionOptions::new()
            .with_session_store(store as Arc<dyn SessionStore>)
            .with_planning_mode(PlanningMode::Disabled)
            .with_confirmation_manager(Arc::new(a3s_code_core::hitl::AutoApproveConfirmation))
            .with_llm_client(Arc::new(second_client)),
    );
    let replay = second.execute(&command).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.state, AgentProtocolRunStateV1::Completed);
    assert_eq!(second.session_count().await, 1);

    let follow_up = start(
        &release_identity,
        "durable-conversation",
        "durable-execution-follow-up",
    );
    second.execute(&follow_up).await.unwrap();
    assert_eq!(
        wait_for_terminal(&second, &follow_up).await.state,
        AgentProtocolRunStateV1::Completed
    );
    let change_set = wait_for_change_set(&second, &follow_up).await;
    let patch = base64::engine::general_purpose::STANDARD
        .decode(change_set.patch_base64)
        .unwrap();
    let patch = String::from_utf8(patch).unwrap();
    assert!(patch.contains("diff --git a/follow-up.txt b/follow-up.txt"));
    assert!(!patch.contains("remote.txt"));
    assert!(!workspace.path().join("remote.txt").exists());
    assert!(!workspace.path().join("follow-up.txt").exists());
    second.close().await;
}

#[tokio::test]
async fn harness_does_not_create_a_session_for_an_unknown_observation() {
    let workspace = tempfile::tempdir().unwrap();
    let manifest = manifest();
    let command = start(
        manifest.artifact().digest(),
        "missing-conversation",
        "missing-execution",
    );
    let harness = AgentProtocolHarness::new(
        manifest,
        Arc::new(Agent::from_config(offline_config()).await.unwrap()),
        workspace.path().display().to_string(),
    )
    .unwrap();
    let error = harness
        .event_page(&AgentProtocolEventPageRequestV1 {
            schema: AgentProtocolEventPageRequestV1::SCHEMA.into(),
            identity: command.identity().clone(),
            after_event_sequence: None,
            limit: 1,
        })
        .await
        .expect_err("an unknown observation must not allocate a session");

    assert!(matches!(error, AgentProtocolHarnessError::SessionNotFound));
    assert_eq!(harness.session_count().await, 0);
    harness.close().await;
}

#[tokio::test]
async fn harness_fails_closed_at_its_retained_session_limit() {
    let workspace = tempfile::tempdir().unwrap();
    let manifest = manifest();
    let release_identity = manifest.artifact().digest().to_string();
    let harness = AgentProtocolHarness::new(
        manifest,
        Arc::new(Agent::from_config(offline_config()).await.unwrap()),
        workspace.path().display().to_string(),
    )
    .unwrap()
    .with_session_options(SessionOptions::new().with_llm_client(Arc::new(StaticStreamingClient)))
    .with_max_sessions(1)
    .unwrap();
    harness
        .execute(&start(
            &release_identity,
            "first-conversation",
            "first-execution",
        ))
        .await
        .unwrap();

    let error = harness
        .execute(&start(
            &release_identity,
            "second-conversation",
            "second-execution",
        ))
        .await
        .expect_err("a second retained conversation must exceed the exact limit");
    assert!(matches!(error, AgentProtocolHarnessError::SessionCapacity));
    assert_eq!(harness.session_count().await, 1);
    harness.close().await;
}

#[test]
fn harness_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AgentProtocolHarness>();
}
