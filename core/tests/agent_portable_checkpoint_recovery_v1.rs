use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};
use a3s_code_core::run::{RunRecord, RunSnapshot, RunStatus};
use a3s_code_core::store::{
    ContextUsage, FileSessionStore, MemorySessionStore, SessionConfig, SessionData,
    SessionSnapshotV1, SessionState, SessionStore,
};
use a3s_code_core::tools::{ArtifactStore, ImmutableContentAdapterBindingV1};
use a3s_code_core::{
    Agent, AgentProtocolCheckpointRecoveryError, AgentProtocolCommandV1,
    AgentProtocolEventPageRequestV1, AgentProtocolHarness, AgentProtocolHarnessError,
    AgentProtocolRunCancelV1, AgentProtocolRunIdentityV1, AgentProtocolRunRecoverExactV1,
    AgentProtocolRunStartV1, AgentProtocolRunStateV1, CodeError, SessionCheckpointError,
    SessionCheckpointExportV1, SessionOptions, SessionSnapshotEvidenceV1, AGENT_PROTOCOL_V1,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct StaticStreamingClient;

fn response() -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text {
                text: "portable recovery complete".into(),
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
        let (sender, receiver) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = sender
                .send(StreamEvent::TextDelta("portable recovery complete".into()))
                .await;
            let _ = sender.send(StreamEvent::Done(response())).await;
        });
        Ok(receiver)
    }
}

#[derive(Clone)]
struct CancelAwareStreamingClient {
    started: Arc<Semaphore>,
}

#[async_trait::async_trait]
impl LlmClient for CancelAwareStreamingClient {
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
        cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (sender, receiver) = mpsc::channel(2);
        let started = Arc::clone(&self.started);
        tokio::spawn(async move {
            if sender
                .send(StreamEvent::TextDelta("portable recovery started".into()))
                .await
                .is_err()
            {
                return;
            }
            started.add_permits(1);
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

fn manifest() -> a3s_code_core::release::AgentReleaseManifest {
    a3s_code_core::release::AgentReleaseManifest::parse(include_str!(
        "../../fixtures/agent-release-contract/.a3s/asset.acl"
    ))
    .unwrap()
}

fn logical_resume(session_id: &str, turn: usize) -> LoopCheckpoint {
    LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: "portable-source-run".into(),
        session_id: session_id.into(),
        capability_binding: None,
        turn,
        messages: vec![Message::user(&format!("continue portable round {turn}"))],
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
        checkpoint_ms: 1_724_000_000_000 + turn as u64,
    }
}

fn portable_export(session_id: &str, turn: usize) -> SessionCheckpointExportV1 {
    let logical = logical_resume(session_id, turn);
    let session = SessionData {
        id: session_id.into(),
        config: SessionConfig {
            name: "portable Harness fixture".into(),
            workspace: "/source/workspace".into(),
            max_context_length: 128_000,
            ..SessionConfig::default()
        },
        state: SessionState::Active,
        messages: logical.messages.clone(),
        context_usage: ContextUsage::default(),
        total_usage: logical.total_usage.clone(),
        total_cost: 0.0,
        model_name: Some("fixture/static".into()),
        cost_records: Vec::new(),
        tool_names: Vec::new(),
        thinking_enabled: false,
        thinking_budget: None,
        created_at: 1_724_000_000,
        updated_at: 1_724_000_001,
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
            id: logical.run_id.clone(),
            session_id: session_id.into(),
            status: RunStatus::Executing,
            prompt: "portable source".into(),
            cognitive_package_binding: None,
            capability_binding: logical.capability_binding.clone(),
            created_at_ms: 1_724_000_000_000,
            updated_at_ms: logical.checkpoint_ms,
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
    SessionCheckpointExportV1::new(snapshot, Some(logical)).unwrap()
}

fn request(
    release: &str,
    session_id: &str,
    target_run_id: &str,
    export: &SessionCheckpointExportV1,
) -> AgentProtocolRunRecoverExactV1 {
    AgentProtocolRunRecoverExactV1 {
        schema: AgentProtocolRunRecoverExactV1::SCHEMA.into(),
        request_id: format!("{target_run_id}:portable-recover"),
        identity: AgentProtocolRunIdentityV1 {
            schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
            protocol: AGENT_PROTOCOL_V1.into(),
            agent_release_identity: release.into(),
            session_id: session_id.into(),
            run_id: target_run_id.into(),
        },
        checkpoint: export.descriptor().clone(),
    }
}

fn start_request(release: &str, session_id: &str, run_id: &str) -> AgentProtocolCommandV1 {
    AgentProtocolCommandV1::Start {
        request: AgentProtocolRunStartV1 {
            schema: AgentProtocolRunStartV1::SCHEMA.into(),
            request_id: format!("{run_id}:start"),
            identity: AgentProtocolRunIdentityV1 {
                schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
                protocol: AGENT_PROTOCOL_V1.into(),
                agent_release_identity: release.into(),
                session_id: session_id.into(),
                run_id: run_id.into(),
            },
            prompt: "create an ordinary live Session".into(),
        },
    }
}

async fn harness_fixture(
    workspace: &tempfile::TempDir,
    store: Arc<MemorySessionStore>,
) -> AgentProtocolHarness {
    harness_fixture_with(
        workspace,
        store as Arc<dyn SessionStore>,
        Arc::new(StaticStreamingClient),
    )
    .await
}

async fn harness_fixture_with(
    workspace: &tempfile::TempDir,
    store: Arc<dyn SessionStore>,
    llm_client: Arc<dyn LlmClient>,
) -> AgentProtocolHarness {
    AgentProtocolHarness::new(
        manifest(),
        Arc::new(Agent::from_config(offline_config()).await.unwrap()),
        workspace.path().display().to_string(),
    )
    .unwrap()
    .with_session_options(
        SessionOptions::new()
            .with_session_store(store)
            .with_llm_client(llm_client),
    )
}

async fn wait_for_run_state(
    harness: &AgentProtocolHarness,
    identity: &AgentProtocolRunIdentityV1,
    expected: AgentProtocolRunStateV1,
) {
    let request = AgentProtocolEventPageRequestV1 {
        schema: AgentProtocolEventPageRequestV1::SCHEMA.into(),
        identity: identity.clone(),
        after_event_sequence: None,
        limit: 64,
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let page = harness.event_page(&request).await.unwrap();
            if page.state == expected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Run did not reach {expected:?}"));
}

#[tokio::test]
async fn portable_checkpoint_is_one_harness_visible_admission() {
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let harness = harness_fixture(&workspace, Arc::clone(&store)).await;
    let export = portable_export("portable-session", 2);
    let request = request(
        harness.agent_release_identity(),
        "portable-session",
        "portable-target-run",
        &export,
    );

    assert!(!store.exists("portable-session").await.unwrap());
    assert!(store
        .load_loop_checkpoint("portable-source-run")
        .await
        .unwrap()
        .is_none());

    let receipt = harness
        .execute_checkpoint_recovery(&request, export.clone())
        .await
        .unwrap();
    assert!(!receipt.replayed);
    receipt.validate_for_exact_recovery(&request).unwrap();
    assert_eq!(harness.session_count().await, 1);

    let page_request = AgentProtocolEventPageRequestV1 {
        schema: AgentProtocolEventPageRequestV1::SCHEMA.into(),
        identity: request.identity.clone(),
        after_event_sequence: None,
        limit: 64,
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let page = harness.event_page(&page_request).await.unwrap();
            if page.state == AgentProtocolRunStateV1::Completed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("portable recovery must complete");

    assert!(store
        .load_loop_checkpoint("portable-source-run")
        .await
        .unwrap()
        .is_none());
    let restart_export = export.clone();
    let replay = harness
        .execute_checkpoint_recovery(&request, export)
        .await
        .unwrap();
    assert!(replay.replayed);
    replay.validate_for_exact_recovery(&request).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if store
                .load_snapshot("portable-session")
                .await
                .unwrap()
                .is_some_and(|snapshot| {
                    snapshot.run_records.iter().any(|record| {
                        record.snapshot.id == "portable-target-run"
                            && record.snapshot.status == RunStatus::Completed
                    })
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal target Run must be persisted before restart");
    harness.close().await;

    let restarted = harness_fixture(&workspace, Arc::clone(&store)).await;
    let replay = restarted
        .execute_checkpoint_recovery(&request, restart_export)
        .await
        .unwrap();
    assert!(replay.replayed);
    replay.validate_for_exact_recovery(&request).unwrap();
    restarted.close().await;
}

#[tokio::test]
async fn descriptor_payload_drift_publishes_no_session_or_store_state() {
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let harness = harness_fixture(&workspace, Arc::clone(&store)).await;
    let expected = portable_export("drift-session", 1);
    let changed = portable_export("drift-session", 2);
    let request = request(
        harness.agent_release_identity(),
        "drift-session",
        "drift-target-run",
        &expected,
    );

    let error = harness
        .execute_checkpoint_recovery(&request, changed)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AgentProtocolCheckpointRecoveryError::Checkpoint(SessionCheckpointError::ContentDrift(_))
    ));
    assert_eq!(harness.session_count().await, 0);
    assert!(!store.exists("drift-session").await.unwrap());
    assert!(store
        .load_loop_checkpoint("portable-source-run")
        .await
        .unwrap()
        .is_none());
    harness.close().await;
}

#[tokio::test]
async fn failed_runtime_rebinding_publishes_no_partial_session() {
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let harness = harness_fixture(&workspace, Arc::clone(&store)).await;
    let export = portable_export("binding-session", 1);
    let (mut snapshot, logical_resume) = export.open().unwrap().into_parts();
    snapshot.session.immutable_content_adapter_binding = Some(
        ImmutableContentAdapterBindingV1::new(format!("sha256:{}", "e".repeat(64)), 1024).unwrap(),
    );
    let export = SessionCheckpointExportV1::new(snapshot, logical_resume).unwrap();
    let request = request(
        harness.agent_release_identity(),
        "binding-session",
        "binding-target-run",
        &export,
    );

    let error = harness
        .execute_checkpoint_recovery(&request, export)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AgentProtocolCheckpointRecoveryError::Harness(AgentProtocolHarnessError::Code(
            CodeError::SessionConfiguration {
                field: "immutable_content_adapter",
                ..
            }
        ))
    ));
    assert_eq!(harness.session_count().await, 0);
    assert!(!store.exists("binding-session").await.unwrap());
    assert!(store
        .load_loop_checkpoint("portable-source-run")
        .await
        .unwrap()
        .is_none());
    harness.close().await;
}

#[tokio::test]
async fn portable_restore_rejects_a_different_persisted_semantic_generation() {
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let expected = portable_export("persisted-drift-session", 1);
    let persisted = portable_export("persisted-drift-session", 2);
    let persisted_snapshot = persisted.open().unwrap().snapshot;
    let persisted_evidence = SessionSnapshotEvidenceV1::from_snapshot(&persisted_snapshot).unwrap();
    store.save_snapshot(&persisted_snapshot).await.unwrap();
    let harness = harness_fixture(&workspace, Arc::clone(&store)).await;
    let request = request(
        harness.agent_release_identity(),
        "persisted-drift-session",
        "persisted-target-run",
        &expected,
    );

    let error = harness
        .execute_checkpoint_recovery(&request, expected)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AgentProtocolCheckpointRecoveryError::Checkpoint(SessionCheckpointError::ContentDrift(_))
    ));
    assert_eq!(harness.session_count().await, 0);
    let after = store
        .load_snapshot("persisted-drift-session")
        .await
        .unwrap()
        .unwrap();
    persisted_evidence.validate_for(&after).unwrap();
    assert!(store
        .load_loop_checkpoint("portable-source-run")
        .await
        .unwrap()
        .is_none());
    harness.close().await;
}

#[tokio::test]
async fn portable_restore_never_replaces_an_unrelated_live_session() {
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let harness = harness_fixture(&workspace, Arc::clone(&store)).await;
    let session_id = "already-live-session";
    harness
        .execute(&start_request(
            harness.agent_release_identity(),
            session_id,
            "ordinary-run",
        ))
        .await
        .unwrap();
    let export = portable_export(session_id, 1);
    let request = request(
        harness.agent_release_identity(),
        session_id,
        "portable-target-run",
        &export,
    );

    let error = harness
        .execute_checkpoint_recovery(&request, export)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AgentProtocolCheckpointRecoveryError::SessionAlreadyActive
    ));
    assert_eq!(
        error.code(),
        "a3s.code.agent_protocol.checkpoint_session_already_active"
    );
    assert_eq!(harness.session_count().await, 1);
    assert!(store
        .load_loop_checkpoint("portable-source-run")
        .await
        .unwrap()
        .is_none());
    harness.close().await;
}

#[tokio::test]
async fn competing_portable_checkpoints_linearize_to_one_session_and_run_identity() {
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let harness = harness_fixture(&workspace, Arc::clone(&store)).await;
    let first = portable_export("competing-session", 1);
    let second = portable_export("competing-session", 2);
    let first_request = request(
        harness.agent_release_identity(),
        "competing-session",
        "competing-target-run",
        &first,
    );
    let second_request = request(
        harness.agent_release_identity(),
        "competing-session",
        "competing-target-run",
        &second,
    );

    let (first_result, second_result) = tokio::join!(
        harness.execute_checkpoint_recovery(&first_request, first),
        harness.execute_checkpoint_recovery(&second_request, second),
    );
    let winner = match (first_result, second_result) {
        (Ok(receipt), Err(error)) => {
            receipt.validate_for_exact_recovery(&first_request).unwrap();
            assert_eq!(error.code(), "RUN_IDENTITY_CONFLICT");
            first_request
        }
        (Err(error), Ok(receipt)) => {
            receipt
                .validate_for_exact_recovery(&second_request)
                .unwrap();
            assert_eq!(error.code(), "RUN_IDENTITY_CONFLICT");
            second_request
        }
        (first, second) => {
            panic!("exactly one competing checkpoint must win: first={first:?}, second={second:?}")
        }
    };

    assert_eq!(harness.session_count().await, 1);
    wait_for_run_state(
        &harness,
        &winner.identity,
        AgentProtocolRunStateV1::Completed,
    )
    .await;
    assert!(store
        .load_loop_checkpoint("portable-source-run")
        .await
        .unwrap()
        .is_none());
    harness.close().await;
}

#[tokio::test]
async fn portable_recovery_cancellation_settles_and_replays_after_restart() {
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let started = Arc::new(Semaphore::new(0));
    let harness = harness_fixture_with(
        &workspace,
        store.clone() as Arc<dyn SessionStore>,
        Arc::new(CancelAwareStreamingClient {
            started: Arc::clone(&started),
        }),
    )
    .await;
    let export = portable_export("cancelled-portable-session", 2);
    let recovery = request(
        harness.agent_release_identity(),
        "cancelled-portable-session",
        "cancelled-portable-target",
        &export,
    );

    harness
        .execute_checkpoint_recovery(&recovery, export.clone())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), started.acquire())
        .await
        .expect("portable recovery provider did not start")
        .unwrap()
        .forget();
    let cancel = AgentProtocolCommandV1::Cancel {
        request: AgentProtocolRunCancelV1 {
            schema: AgentProtocolRunCancelV1::SCHEMA.into(),
            request_id: "cancel-portable-recovery".into(),
            identity: recovery.identity.clone(),
            reason: "test exact portable cancellation".into(),
        },
    };
    let cancelled = harness.execute(&cancel).await.unwrap();
    assert_eq!(cancelled.state, AgentProtocolRunStateV1::Cancelled);
    assert!(!cancelled.replayed);
    cancelled.validate_for(&cancel).unwrap();
    wait_for_run_state(
        &harness,
        &recovery.identity,
        AgentProtocolRunStateV1::Cancelled,
    )
    .await;

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if store
                .load_snapshot("cancelled-portable-session")
                .await
                .unwrap()
                .is_some_and(|snapshot| {
                    snapshot.run_records.iter().any(|record| {
                        record.snapshot.id == "cancelled-portable-target"
                            && record.snapshot.status == RunStatus::Cancelled
                    })
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled portable target must be persisted");
    harness.close().await;

    let restarted = harness_fixture(&workspace, Arc::clone(&store)).await;
    let replay = restarted
        .execute_checkpoint_recovery(&recovery, export)
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.state, AgentProtocolRunStateV1::Cancelled);
    replay.validate_for_exact_recovery(&recovery).unwrap();
    restarted.close().await;
}

#[tokio::test]
async fn file_store_restart_replays_the_exact_portable_checkpoint() {
    let workspace = tempfile::tempdir().unwrap();
    let store_root = tempfile::tempdir().unwrap();
    let store = Arc::new(FileSessionStore::new(store_root.path()).await.unwrap());
    let harness = harness_fixture_with(
        &workspace,
        store.clone() as Arc<dyn SessionStore>,
        Arc::new(StaticStreamingClient),
    )
    .await;
    let export = portable_export("file-portable-session", 3);
    let recovery = request(
        harness.agent_release_identity(),
        "file-portable-session",
        "file-portable-target",
        &export,
    );

    let receipt = harness
        .execute_checkpoint_recovery(&recovery, export.clone())
        .await
        .unwrap();
    assert!(!receipt.replayed);
    wait_for_run_state(
        &harness,
        &recovery.identity,
        AgentProtocolRunStateV1::Completed,
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if store
                .load_snapshot("file-portable-session")
                .await
                .unwrap()
                .is_some_and(|snapshot| {
                    snapshot.run_records.iter().any(|record| {
                        record.snapshot.id == "file-portable-target"
                            && record.snapshot.status == RunStatus::Completed
                    })
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("file-backed target Run must persist before restart");
    assert!(store
        .load_loop_checkpoint("portable-source-run")
        .await
        .unwrap()
        .is_none());
    harness.close().await;
    drop(store);

    let reopened = Arc::new(FileSessionStore::new(store_root.path()).await.unwrap());
    let restarted = harness_fixture_with(
        &workspace,
        reopened.clone() as Arc<dyn SessionStore>,
        Arc::new(StaticStreamingClient),
    )
    .await;
    let replay = restarted
        .execute_checkpoint_recovery(&recovery, export)
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.state, AgentProtocolRunStateV1::Completed);
    replay.validate_for_exact_recovery(&recovery).unwrap();
    assert!(reopened
        .load_loop_checkpoint("portable-source-run")
        .await
        .unwrap()
        .is_none());
    restarted.close().await;
}
