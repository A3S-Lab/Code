use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};
use a3s_code_core::run::{RunRecord, RunSnapshot, RunStatus};
use a3s_code_core::store::{
    ContextUsage, MemorySessionStore, SessionConfig, SessionData, SessionSnapshotV1, SessionState,
    SessionStore,
};
use a3s_code_core::tools::ArtifactStore;
use a3s_code_core::{
    Agent, AgentProtocolCommandActionV1, AgentProtocolCommandReceiptV1, AgentProtocolError,
    AgentProtocolExactRecoveryError, AgentProtocolHost, AgentProtocolHostError,
    AgentProtocolRunIdentityV1, AgentProtocolRunRecoverExactV1, AgentProtocolRunStateV1, CodeError,
    SessionCheckpointError, SessionCheckpointExportV1, SessionOptions, AGENT_PROTOCOL_V1,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct StaticStreamingClient;

impl StaticStreamingClient {
    fn response() -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".into(),
                content: vec![ContentBlock::Text {
                    text: "recovered exact boundary".into(),
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
        Ok(Self::response())
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (sender, receiver) = mpsc::channel(2);
        tokio::spawn(async move {
            let response = Self::response();
            let _ = sender
                .send(StreamEvent::TextDelta("recovered exact boundary".into()))
                .await;
            let _ = sender.send(StreamEvent::Done(response)).await;
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

fn checkpoint(source_run_id: &str, session_id: &str, turn: usize) -> LoopCheckpoint {
    LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: source_run_id.into(),
        session_id: session_id.into(),
        capability_binding: None,
        turn,
        messages: vec![Message::user(&format!("continue after tool round {turn}"))],
        total_usage: TokenUsage {
            prompt_tokens: turn,
            completion_tokens: turn,
            total_tokens: turn * 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        tool_calls_count: turn,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1_723_000_000_000 + turn as u64,
    }
}

fn portable_export(checkpoint: &LoopCheckpoint) -> SessionCheckpointExportV1 {
    let session = SessionData {
        id: checkpoint.session_id.clone(),
        config: SessionConfig {
            name: "exact recovery fixture".into(),
            workspace: "/portable/source".into(),
            max_context_length: 128_000,
            ..SessionConfig::default()
        },
        state: SessionState::Active,
        messages: checkpoint.messages.clone(),
        context_usage: ContextUsage::default(),
        total_usage: checkpoint.total_usage.clone(),
        total_cost: 0.0,
        model_name: Some("fixture/static".into()),
        cost_records: Vec::new(),
        tool_names: Vec::new(),
        thinking_enabled: false,
        thinking_budget: None,
        created_at: 1_723_000_000,
        updated_at: 1_723_000_001,
        llm_config: None,
        tasks: Vec::new(),
        parent_id: None,
        tenant_id: None,
        principal: None,
        agent_template_id: None,
        correlation_id: None,
        cognitive_package_binding: None,
        immutable_content_adapter_binding: None,
    };
    let source = RunRecord {
        snapshot: RunSnapshot {
            id: checkpoint.run_id.clone(),
            session_id: checkpoint.session_id.clone(),
            status: RunStatus::Executing,
            prompt: "portable source".into(),
            cognitive_package_binding: None,
            capability_binding: checkpoint.capability_binding.clone(),
            created_at_ms: 1_723_000_000_000,
            updated_at_ms: checkpoint.checkpoint_ms,
            result_text: None,
            error: None,
            event_count: 0,
            workspace_change_set: None,
        },
        events: Vec::new(),
    };
    let snapshot = SessionSnapshotV1::new(
        session,
        &ArtifactStore::new(),
        Vec::new(),
        vec![source],
        Vec::new(),
        Vec::new(),
    );
    SessionCheckpointExportV1::new(snapshot, Some(checkpoint.clone())).unwrap()
}

fn identity(release: &str, session_id: &str, run_id: &str) -> AgentProtocolRunIdentityV1 {
    AgentProtocolRunIdentityV1 {
        schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
        protocol: AGENT_PROTOCOL_V1.into(),
        agent_release_identity: release.into(),
        session_id: session_id.into(),
        run_id: run_id.into(),
    }
}

fn exact_request(
    release: &str,
    session_id: &str,
    target_run_id: &str,
    request_id: &str,
    checkpoint: &LoopCheckpoint,
) -> AgentProtocolRunRecoverExactV1 {
    AgentProtocolRunRecoverExactV1 {
        schema: AgentProtocolRunRecoverExactV1::SCHEMA.into(),
        request_id: request_id.into(),
        identity: identity(release, session_id, target_run_id),
        checkpoint: portable_export(checkpoint).descriptor().clone(),
    }
}

async fn host_fixture(
    store: Arc<MemorySessionStore>,
    session_id: &str,
) -> (
    tempfile::TempDir,
    Arc<a3s_code_core::AgentSession>,
    AgentProtocolHost,
    String,
) {
    let workspace = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_builder(workspace.path().display().to_string())
            .options(
                SessionOptions::new()
                    .with_session_id(session_id)
                    .with_session_store(store as Arc<dyn SessionStore>)
                    .with_llm_client(Arc::new(StaticStreamingClient)),
            )
            .build()
            .await
            .unwrap(),
    );
    let release = format!("sha256:{}", "d".repeat(64));
    let host = AgentProtocolHost::new(release.clone(), Arc::clone(&session)).unwrap();
    (workspace, session, host, release)
}

#[test]
fn exact_recovery_request_and_receipt_bind_the_complete_checkpoint() {
    let first = checkpoint("source-run", "session-1", 1);
    let second = checkpoint("source-run", "session-1", 2);
    let request = exact_request(
        &format!("sha256:{}", "a".repeat(64)),
        "session-1",
        "target-run",
        "recover-exact-1",
        &first,
    );
    let changed = exact_request(
        &format!("sha256:{}", "a".repeat(64)),
        "session-1",
        "target-run",
        "recover-exact-1",
        &second,
    );
    let (mut semantic_snapshot, logical_resume) =
        portable_export(&first).open().unwrap().into_parts();
    semantic_snapshot.session.principal = Some("different-principal".into());
    let semantic_export =
        SessionCheckpointExportV1::new(semantic_snapshot, logical_resume).unwrap();
    let semantic_changed = AgentProtocolRunRecoverExactV1 {
        schema: AgentProtocolRunRecoverExactV1::SCHEMA.into(),
        request_id: request.request_id.clone(),
        identity: request.identity.clone(),
        checkpoint: semantic_export.descriptor().clone(),
    };

    request.validate().unwrap();
    assert_ne!(request.digest().unwrap(), changed.digest().unwrap());
    assert_ne!(
        request.digest().unwrap(),
        semantic_changed.digest().unwrap(),
        "semantic snapshot drift must change exact recovery identity even when the logical boundary is unchanged"
    );

    let receipt = AgentProtocolCommandReceiptV1 {
        schema: AgentProtocolCommandReceiptV1::SCHEMA.into(),
        action: AgentProtocolCommandActionV1::Recover,
        request_id: request.request_id.clone(),
        identity: request.identity.clone(),
        command_digest: request.digest().unwrap(),
        state: AgentProtocolRunStateV1::Executing,
        latest_event_sequence_exclusive: 0,
        observed_at_ms: 1_723_000_000_000,
        replayed: false,
    };
    receipt.validate_for_exact_recovery(&request).unwrap();
    assert!(receipt.validate_for_exact_recovery(&changed).is_err());

    let encoded = serde_json::to_value(&request).unwrap();
    assert_eq!(encoded["schema"], AgentProtocolRunRecoverExactV1::SCHEMA);
    assert_eq!(
        encoded["checkpoint"]["descriptor_digest"],
        request.checkpoint.descriptor_digest
    );
    assert_eq!(
        serde_json::from_value::<AgentProtocolRunRecoverExactV1>(encoded).unwrap(),
        request
    );
}

#[test]
fn exact_recovery_requires_a_logical_component_in_the_complete_descriptor() {
    let boundary = checkpoint("source-run", "session-semantic-only", 1);
    let (snapshot, _) = portable_export(&boundary).open().unwrap().into_parts();
    let semantic_only = SessionCheckpointExportV1::new(snapshot, None).unwrap();
    let request = AgentProtocolRunRecoverExactV1 {
        schema: AgentProtocolRunRecoverExactV1::SCHEMA.into(),
        request_id: "recover-semantic-only".into(),
        identity: identity(
            &format!("sha256:{}", "a".repeat(64)),
            "session-semantic-only",
            "target-run",
        ),
        checkpoint: semantic_only.descriptor().clone(),
    };

    assert!(matches!(
        request.validate(),
        Err(AgentProtocolError::InvalidField("checkpoint"))
    ));
}

#[tokio::test]
async fn overwritten_checkpoint_is_rejected_before_target_run_admission() {
    let store = Arc::new(MemorySessionStore::new());
    let first = checkpoint("source-run", "session-drift", 1);
    let descriptor = portable_export(&first).descriptor().clone();
    let second = checkpoint("source-run", "session-drift", 2);
    store
        .save_loop_checkpoint(&second.run_id, &second)
        .await
        .unwrap();
    let (_workspace, session, host, release) =
        host_fixture(Arc::clone(&store), "session-drift").await;
    let request = AgentProtocolRunRecoverExactV1 {
        schema: AgentProtocolRunRecoverExactV1::SCHEMA.into(),
        request_id: "recover-drift".into(),
        identity: identity(&release, session.session_id(), "target-run"),
        checkpoint: descriptor,
    };

    let error = host.execute_exact_recovery(&request).await.unwrap_err();
    assert!(matches!(
        error,
        AgentProtocolExactRecoveryError::Checkpoint(SessionCheckpointError::ContentDrift(_))
    ));
    assert_eq!(error.code(), "a3s.code.session_checkpoint.content_drift");
    assert!(session.runs().await.is_empty());
}

#[tokio::test]
async fn exact_recovery_replays_only_the_same_boundary_identity() {
    let store = Arc::new(MemorySessionStore::new());
    let first = checkpoint("source-run", "session-replay", 1);
    store
        .save_loop_checkpoint(&first.run_id, &first)
        .await
        .unwrap();
    let (_workspace, session, host, release) =
        host_fixture(Arc::clone(&store), "session-replay").await;
    let request = exact_request(
        &release,
        session.session_id(),
        "target-run",
        "recover-exact",
        &first,
    );

    let started = host.execute_exact_recovery(&request).await.unwrap();
    assert!(!started.replayed);
    started.validate_for_exact_recovery(&request).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if session
                .run_snapshot("target-run")
                .await
                .is_some_and(|snapshot| snapshot.status.is_terminal())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("exactly recovered run must terminate");

    let second = checkpoint("source-run", "session-replay", 2);
    store
        .save_loop_checkpoint(&second.run_id, &second)
        .await
        .unwrap();
    let replayed = host.execute_exact_recovery(&request).await.unwrap();
    assert!(replayed.replayed);
    replayed.validate_for_exact_recovery(&request).unwrap();

    let changed = exact_request(
        &release,
        session.session_id(),
        "target-run",
        "recover-exact-changed",
        &second,
    );
    let error = host.execute_exact_recovery(&changed).await.unwrap_err();
    assert!(matches!(
        error,
        AgentProtocolExactRecoveryError::Host(AgentProtocolHostError::Code(
            CodeError::RunIdentityConflict { .. }
        ))
    ));
    assert_eq!(session.runs().await.len(), 1);
}
