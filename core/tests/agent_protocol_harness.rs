use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::store::{MemorySessionStore, SessionStore};
use a3s_code_core::{
    Agent, AgentProtocolCommandV1, AgentProtocolEventPageRequestV1, AgentProtocolHarness,
    AgentProtocolHarnessError, AgentProtocolRunIdentityV1, AgentProtocolRunStartV1,
    AgentProtocolRunStateV1, SessionOptions, AGENT_PROTOCOL_V1,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct StaticStreamingClient;

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

#[tokio::test]
async fn harness_multiplexes_sessions_through_code_owned_hosts() {
    let workspace = tempfile::tempdir().unwrap();
    let manifest = manifest();
    let identity = manifest.identity().to_string();
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
async fn harness_resumes_the_code_store_before_replaying_a_start_after_restart() {
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let release = manifest();
    let release_identity = release.identity().to_string();
    let command = start(
        &release_identity,
        "durable-conversation",
        "durable-execution",
    );

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
            .with_llm_client(Arc::new(StaticStreamingClient)),
    );
    let receipt = first.execute(&command).await.unwrap();
    assert!(!receipt.replayed);
    assert_eq!(
        wait_for_terminal(&first, &command).await.state,
        AgentProtocolRunStateV1::Completed
    );
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

    let second_agent = Arc::new(Agent::from_config(offline_config()).await.unwrap());
    let second = AgentProtocolHarness::new(
        release,
        second_agent,
        workspace.path().display().to_string(),
    )
    .unwrap()
    .with_session_options(
        SessionOptions::new()
            .with_session_store(store as Arc<dyn SessionStore>)
            .with_llm_client(Arc::new(StaticStreamingClient)),
    );
    let replay = second.execute(&command).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.state, AgentProtocolRunStateV1::Completed);
    assert_eq!(second.session_count().await, 1);
    second.close().await;
}

#[tokio::test]
async fn harness_does_not_create_a_session_for_an_unknown_observation() {
    let workspace = tempfile::tempdir().unwrap();
    let manifest = manifest();
    let command = start(
        manifest.identity(),
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
    let release_identity = manifest.identity().to_string();
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
