use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::{
    Agent, AgentProtocolCommandV1, AgentProtocolEventPageRequestV1, AgentProtocolHost,
    AgentProtocolRunCancelV1, AgentProtocolRunIdentityV1, AgentProtocolRunRecoverV1,
    AgentProtocolRunStartV1, AgentRunSpawn, CodeError, SessionOptions, AGENT_PROTOCOL_V1,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct StaticStreamingClient {
    text: String,
}

#[derive(Clone)]
struct PendingStreamingClient;

impl StaticStreamingClient {
    fn response(&self) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".into(),
                content: vec![ContentBlock::Text {
                    text: self.text.clone(),
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
}

#[async_trait::async_trait]
impl LlmClient for StaticStreamingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(self.response())
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (sender, receiver) = mpsc::channel(4);
        let response = self.response();
        let text = self.text.clone();
        tokio::spawn(async move {
            let _ = sender.send(StreamEvent::TextDelta(text)).await;
            let _ = sender.send(StreamEvent::Done(response)).await;
        });
        Ok(receiver)
    }
}

#[async_trait::async_trait]
impl LlmClient for PendingStreamingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("pending fixture only supports streaming")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (sender, receiver) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = sender.send(StreamEvent::TextDelta("working".into())).await;
            cancel_token.cancelled().await;
        });
        Ok(receiver)
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

#[tokio::test]
async fn exact_run_replay_never_executes_the_same_code_run_twice() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = agent
        .session_builder(workspace.path().display().to_string())
        .options(
            SessionOptions::new()
                .with_session_id("cloud-conversation-1")
                .with_llm_client(Arc::new(StaticStreamingClient {
                    text: "completed once".into(),
                })),
        )
        .build()
        .await
        .unwrap();

    let started = session
        .spawn_run_with_id("cloud-execution-1-attempt-1", "fix the test")
        .await
        .unwrap();
    let AgentRunSpawn::Started { worker, .. } = started else {
        panic!("first exact run must start");
    };
    worker.await.unwrap();

    let completed = session
        .run_snapshot("cloud-execution-1-attempt-1")
        .await
        .unwrap();
    assert_eq!(completed.status, a3s_code_core::RunStatus::Completed);
    assert!(completed.event_count > 0);

    let replay = session
        .spawn_run_with_id("cloud-execution-1-attempt-1", "fix the test")
        .await
        .unwrap();
    assert!(replay.replayed());
    assert_eq!(session.runs().await.len(), 1);

    let conflict = match session
        .spawn_run_with_id("cloud-execution-1-attempt-1", "replace the immutable input")
        .await
    {
        Ok(_) => panic!("changed immutable input must fail"),
        Err(error) => error,
    };
    assert!(matches!(conflict, CodeError::RunIdentityConflict { .. }));
    assert_eq!(session.runs().await.len(), 1);
}

#[tokio::test]
async fn protocol_host_executes_and_observes_the_code_owned_run() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_builder(workspace.path().display().to_string())
            .options(
                SessionOptions::new()
                    .with_session_id("cloud-conversation-2")
                    .with_llm_client(Arc::new(StaticStreamingClient {
                        text: "protocol result".into(),
                    })),
            )
            .build()
            .await
            .unwrap(),
    );
    let manifest = a3s_code_core::release::AgentReleaseManifest::parse(include_str!(
        "../../fixtures/agent-release-contract/.a3s/asset.acl"
    ))
    .unwrap();
    let host = AgentProtocolHost::from_manifest(&manifest, Arc::clone(&session)).unwrap();
    let release_identity = manifest.artifact().digest().to_string();
    assert_eq!(host.agent_release_identity(), release_identity);
    assert_ne!(host.agent_release_identity(), manifest.identity());
    let identity = AgentProtocolRunIdentityV1 {
        schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
        protocol: AGENT_PROTOCOL_V1.into(),
        agent_release_identity: release_identity,
        session_id: session.session_id().into(),
        run_id: "cloud-execution-2-attempt-1".into(),
    };
    let command = AgentProtocolCommandV1::Start {
        request: AgentProtocolRunStartV1 {
            schema: AgentProtocolRunStartV1::SCHEMA.into(),
            request_id: "cloud-execution-2:start".into(),
            identity: identity.clone(),
            prompt: "explain the protocol".into(),
        },
    };

    let first = host.execute(&command).await.unwrap();
    assert!(!first.replayed);
    first.validate_for(&command).unwrap();

    let replay = host.execute(&command).await.unwrap();
    assert!(replay.replayed);
    replay.validate_for(&command).unwrap();

    let page = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let page = host
                .event_page_for(&AgentProtocolEventPageRequestV1 {
                    schema: AgentProtocolEventPageRequestV1::SCHEMA.into(),
                    identity: identity.clone(),
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
    .expect("detached Code run must terminate");
    assert!(!page.events.is_empty());
    assert_eq!(page.identity, identity);
    assert_eq!(session.runs().await.len(), 1);
}

#[tokio::test]
async fn protocol_host_rejects_a_release_for_another_code_protocol() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_builder(workspace.path().display().to_string())
            .options(SessionOptions::new().with_session_id("cloud-conversation-future"))
            .build()
            .await
            .unwrap(),
    );
    let manifest_source = include_str!("../../fixtures/agent-release-contract/.a3s/asset.acl")
        .replace(AGENT_PROTOCOL_V1, "a3s.code.agent.v2");
    let manifest = a3s_code_core::release::AgentReleaseManifest::parse(&manifest_source).unwrap();
    let error = match AgentProtocolHost::from_manifest(&manifest, session) {
        Ok(_) => panic!("a v1 host must reject a release for another protocol"),
        Err(error) => error,
    };
    assert_eq!(
        error.code(),
        "a3s.code.agent_protocol.release_protocol_mismatch"
    );
}

#[tokio::test]
async fn protocol_cancellation_targets_only_the_active_code_run() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_builder(workspace.path().display().to_string())
            .options(
                SessionOptions::new()
                    .with_session_id("cloud-conversation-3")
                    .with_llm_client(Arc::new(PendingStreamingClient)),
            )
            .build()
            .await
            .unwrap(),
    );
    let release_identity = format!("sha256:{}", "b".repeat(64));
    let host = AgentProtocolHost::new(release_identity.clone(), Arc::clone(&session)).unwrap();
    let identity = AgentProtocolRunIdentityV1 {
        schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
        protocol: AGENT_PROTOCOL_V1.into(),
        agent_release_identity: release_identity,
        session_id: session.session_id().into(),
        run_id: "cloud-execution-3-attempt-1".into(),
    };
    let start = AgentProtocolCommandV1::Start {
        request: AgentProtocolRunStartV1 {
            schema: AgentProtocolRunStartV1::SCHEMA.into(),
            request_id: "cloud-execution-3:start".into(),
            identity: identity.clone(),
            prompt: "wait for cancellation".into(),
        },
    };
    host.execute(&start).await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if session
                .run_snapshot(&identity.run_id)
                .await
                .is_some_and(|snapshot| snapshot.event_count > 0)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Code run must begin");

    let cancel = AgentProtocolCommandV1::Cancel {
        request: AgentProtocolRunCancelV1 {
            schema: AgentProtocolRunCancelV1::SCHEMA.into(),
            request_id: "cloud-execution-3:cancel".into(),
            identity,
            reason: "user_requested".into(),
        },
    };
    let cancelled = host.execute(&cancel).await.unwrap();
    assert!(!cancelled.replayed);
    assert_eq!(
        cancelled.state,
        a3s_code_core::AgentProtocolRunStateV1::Cancelled
    );
    cancelled.validate_for(&cancel).unwrap();

    let replay = host.execute(&cancel).await.unwrap();
    assert!(replay.replayed);
    replay.validate_for(&cancel).unwrap();
}

#[tokio::test]
async fn protocol_recovery_uses_code_checkpoint_semantics_and_a_fresh_exact_run() {
    use a3s_code_core::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};
    use a3s_code_core::store::{MemorySessionStore, SessionStore};

    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let checkpoint_run_id = "cloud-execution-4-attempt-1";
    let checkpoint = LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: checkpoint_run_id.into(),
        session_id: "cloud-conversation-4".into(),
        capability_binding: None,
        turn: 2,
        messages: vec![Message::user("continue from durable work")],
        total_usage: TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        tool_calls_count: 1,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1_723_000_000_000,
    };
    store
        .save_loop_checkpoint(checkpoint_run_id, &checkpoint)
        .await
        .unwrap();

    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_builder(workspace.path().display().to_string())
            .options(
                SessionOptions::new()
                    .with_session_id("cloud-conversation-4")
                    .with_session_store(store as Arc<dyn SessionStore>)
                    .with_llm_client(Arc::new(StaticStreamingClient {
                        text: "recovered result".into(),
                    })),
            )
            .build()
            .await
            .unwrap(),
    );
    let release_identity = format!("sha256:{}", "c".repeat(64));
    let host = AgentProtocolHost::new(release_identity.clone(), Arc::clone(&session)).unwrap();
    let recovered_identity = AgentProtocolRunIdentityV1 {
        schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
        protocol: AGENT_PROTOCOL_V1.into(),
        agent_release_identity: release_identity,
        session_id: session.session_id().into(),
        run_id: "cloud-execution-4-attempt-2".into(),
    };
    let recover = AgentProtocolCommandV1::Recover {
        request: AgentProtocolRunRecoverV1 {
            schema: AgentProtocolRunRecoverV1::SCHEMA.into(),
            request_id: "cloud-execution-4:recover".into(),
            identity: recovered_identity.clone(),
            checkpoint_run_id: checkpoint_run_id.into(),
        },
    };

    let receipt = host.execute(&recover).await.unwrap();
    assert!(!receipt.replayed);
    receipt.validate_for(&recover).unwrap();
    let page = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let page = host
                .event_page(&recovered_identity, None, 64)
                .await
                .unwrap();
            if page.state.is_terminal() {
                break page;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("recovered Code run must terminate");
    assert_eq!(
        page.state,
        a3s_code_core::AgentProtocolRunStateV1::Completed
    );
    assert_eq!(session.runs().await.len(), 1);
    assert_eq!(session.runs().await[0].id, recovered_identity.run_id);

    let replay = host.execute(&recover).await.unwrap();
    assert!(replay.replayed);
}
