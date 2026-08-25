use a3s_code_core::capability::{
    CapabilityContribution, CapabilityDescriptor, CapabilityKind, CapabilitySet, CapabilitySource,
    CapabilityValue, CodeCatalogGeneration, SessionCapabilityBatch, Sha256Digest,
};
use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::tools::{Tool, ToolContext, ToolOutput};
use a3s_code_core::{
    Agent, AgentProtocolEventPageRequestV1, AgentProtocolExactRecoveryError, AgentProtocolHarness,
    AgentProtocolHost, AgentProtocolRunIdentityV1, AgentProtocolRunRecoverExactV1,
    AgentProtocolRunStateV1, PlanningMode, SessionCheckpointError, SessionCheckpointExportSink,
    SessionCheckpointExportV1, SessionOptions, AGENT_PROTOCOL_V1,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct ScriptedClient {
    responses: Arc<Mutex<Vec<LlmResponse>>>,
}

impl ScriptedClient {
    fn new(mut responses: Vec<LlmResponse>) -> Self {
        responses.reverse();
        Self {
            responses: Arc::new(Mutex::new(responses)),
        }
    }

    fn next(&self) -> anyhow::Result<LlmResponse> {
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow::anyhow!("scripted capability recovery client exhausted"))
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedClient {
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
        let (sender, receiver) = mpsc::channel(2);
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

#[derive(Default)]
struct RecordingExportSink {
    exports: Mutex<Vec<SessionCheckpointExportV1>>,
}

#[async_trait::async_trait]
impl SessionCheckpointExportSink for RecordingExportSink {
    async fn export_checkpoint(&self, checkpoint: SessionCheckpointExportV1) -> anyhow::Result<()> {
        self.exports.lock().unwrap().push(checkpoint);
        Ok(())
    }
}

#[derive(Clone)]
struct GenerationTool(&'static str);

#[async_trait::async_trait]
impl Tool for GenerationTool {
    fn name(&self) -> &str {
        "generation_probe"
    }

    fn description(&self) -> &str {
        "Return the exact projected generation"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _context: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::success(self.0))
    }
}

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
}

fn capability_batch(
    generation: u64,
    surface: char,
    output: &'static str,
) -> SessionCapabilityBatch {
    let source = CapabilitySource::host("checkpoint-recovery", digest('a')).unwrap();
    let descriptor = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Tool,
        "generation-probe",
        "generation_probe",
        digest(surface),
        [],
    )
    .unwrap();
    let id = descriptor.id().clone();
    let set = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(generation),
        [CapabilityContribution::new(source, [descriptor]).unwrap()],
    )
    .unwrap();
    let mut batch = SessionCapabilityBatch::new(set).unwrap();
    batch
        .stage_value(id, CapabilityValue::Tool(Arc::new(GenerationTool(output))))
        .unwrap();
    batch
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

fn tool_response() -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::ToolUse {
                id: "generation-boundary".into(),
                name: "generation_probe".into(),
                input: serde_json::json!({}),
            }],
            reasoning_content: None,
        },
        usage: TokenUsage::default(),
        stop_reason: Some("tool_use".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn final_response(text: &str) -> LlmResponse {
    LlmResponse {
        message: Message::assistant(text),
        usage: TokenUsage::default(),
        stop_reason: Some("end_turn".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn release_manifest() -> a3s_code_core::release::AgentReleaseManifest {
    a3s_code_core::release::AgentReleaseManifest::parse(include_str!(
        "../../fixtures/agent-release-contract/.a3s/asset.acl"
    ))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn exact_recovery_rejects_checkpoint_after_capability_generation_cutover() {
    let workspace = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingExportSink::default());
    let client = Arc::new(ScriptedClient::new(vec![
        tool_response(),
        final_response("source complete"),
        final_response("unsafe recovery used the latest generation"),
    ]));
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_async(
                workspace.path().display().to_string(),
                Some(
                    SessionOptions::new()
                        .with_session_id("capability-recovery-session")
                        .with_llm_client(client)
                        .with_session_checkpoint_export_sink(sink.clone())
                        .with_permission_policy(
                            PermissionPolicy::new().allow("generation_probe(*)"),
                        )
                        .with_planning_mode(PlanningMode::Disabled)
                        .with_continuation(false),
                ),
            )
            .await
            .unwrap(),
    );
    session
        .apply_capability_batch(
            capability_batch(1, 'b', "generation-one"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let result = session.send("capture generation one", None).await.unwrap();
    assert_eq!(result.text, "source complete");

    let export = sink.exports.lock().unwrap().pop().unwrap();
    let descriptor = export.descriptor().clone();
    let logical = export.open().unwrap().logical_resume.unwrap();
    let source_run_id = logical.run_id.clone();

    session
        .apply_capability_batch(
            capability_batch(2, 'c', "generation-two"),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let release = format!("sha256:{}", "d".repeat(64));
    let host = AgentProtocolHost::new(release.clone(), Arc::clone(&session)).unwrap();
    let request = AgentProtocolRunRecoverExactV1 {
        schema: AgentProtocolRunRecoverExactV1::SCHEMA.into(),
        request_id: "recover-generation-one".into(),
        identity: AgentProtocolRunIdentityV1 {
            schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
            protocol: AGENT_PROTOCOL_V1.into(),
            agent_release_identity: release,
            session_id: session.session_id().into(),
            run_id: "recovered-generation-one".into(),
        },
        checkpoint: descriptor,
    };

    let error = host
        .execute_exact_recovery_from_checkpoint(&request, logical)
        .await
        .expect_err("a generation-one checkpoint must not run over generation two");
    assert!(matches!(
        error,
        AgentProtocolExactRecoveryError::Checkpoint(SessionCheckpointError::ContentDrift(_))
    ));
    assert!(session.run_snapshot(&source_run_id).await.is_some());
    assert!(session
        .run_snapshot("recovered-generation-one")
        .await
        .is_none());
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fresh_session_bootstraps_the_exact_historical_generation_before_recovery() {
    let source_workspace = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingExportSink::default());
    let source_agent = Agent::from_config(offline_config()).await.unwrap();
    let source = Arc::new(
        source_agent
            .session_async(
                source_workspace.path().display().to_string(),
                Some(
                    SessionOptions::new()
                        .with_session_id("capability-bootstrap-session")
                        .with_llm_client(Arc::new(ScriptedClient::new(vec![
                            tool_response(),
                            final_response("generation three captured"),
                        ])))
                        .with_session_checkpoint_export_sink(sink.clone())
                        .with_permission_policy(
                            PermissionPolicy::new().allow("generation_probe(*)"),
                        )
                        .with_planning_mode(PlanningMode::Disabled)
                        .with_continuation(false),
                ),
            )
            .await
            .unwrap(),
    );
    for (generation, surface) in [(1, 'b'), (2, 'c'), (3, 'd')] {
        source
            .apply_capability_batch(
                capability_batch(generation, surface, "generation-three"),
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    source.send("capture generation three", None).await.unwrap();
    let export = sink.exports.lock().unwrap().pop().unwrap();
    let descriptor = export.descriptor().clone();
    let logical = export.open().unwrap().logical_resume.unwrap();
    let expected = logical
        .capability_binding
        .clone()
        .expect("new checkpoints bind their complete capability generation");
    assert_eq!(expected.code_catalog_generation(), 3);
    source.close().await;

    let recovery_workspace = tempfile::tempdir().unwrap();
    let recovery_agent = Agent::from_config(offline_config()).await.unwrap();
    let recovery = Arc::new(
        recovery_agent
            .session_async(
                recovery_workspace.path().display().to_string(),
                Some(
                    SessionOptions::new()
                        .with_session_id("capability-bootstrap-session")
                        .with_llm_client(Arc::new(ScriptedClient::new(vec![final_response(
                            "recovered generation three",
                        )])))
                        .with_permission_policy(
                            PermissionPolicy::new().allow("generation_probe(*)"),
                        )
                        .with_planning_mode(PlanningMode::Disabled)
                        .with_continuation(false),
                ),
            )
            .await
            .unwrap(),
    );
    let mismatch = recovery
        .bootstrap_recovery_capability_batch(
            &expected,
            capability_batch(4, 'e', "wrong-generation"),
            CancellationToken::new(),
        )
        .await
        .expect_err("a recovery batch must match the checkpoint identity");
    assert!(matches!(
        mismatch,
        a3s_code_core::capability::CapabilityRuntimeError::RecoveryBinding { .. }
    ));
    assert_eq!(recovery.capability_catalog_stamp().generation().get(), 0);

    let receipt = recovery
        .bootstrap_recovery_capability_batch(
            &expected,
            capability_batch(3, 'd', "generation-three"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(receipt.previous().generation().get(), 0);
    assert_eq!(receipt.committed().generation().get(), 3);

    let release = format!("sha256:{}", "e".repeat(64));
    let host = AgentProtocolHost::new(release.clone(), Arc::clone(&recovery)).unwrap();
    let request = AgentProtocolRunRecoverExactV1 {
        schema: AgentProtocolRunRecoverExactV1::SCHEMA.into(),
        request_id: "recover-bootstrapped-generation".into(),
        identity: AgentProtocolRunIdentityV1 {
            schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
            protocol: AGENT_PROTOCOL_V1.into(),
            agent_release_identity: release,
            session_id: recovery.session_id().into(),
            run_id: "bootstrapped-recovery-run".into(),
        },
        checkpoint: descriptor,
    };
    let receipt = host
        .execute_exact_recovery_from_checkpoint(&request, logical)
        .await
        .unwrap();
    assert!(!receipt.replayed);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if recovery
                .run_snapshot("bootstrapped-recovery-run")
                .await
                .is_some_and(|snapshot| snapshot.status.is_terminal())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("bootstrapped exact recovery must finish");
    let snapshot = recovery
        .run_snapshot("bootstrapped-recovery-run")
        .await
        .unwrap();
    assert_eq!(snapshot.capability_binding.as_ref(), Some(&expected));
    recovery.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn harness_requires_and_consumes_an_exact_capability_recovery_batch() {
    let source_workspace = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingExportSink::default());
    let source_agent = Agent::from_config(offline_config()).await.unwrap();
    let source = source_agent
        .session_async(
            source_workspace.path().display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("capability-harness-session")
                    .with_llm_client(Arc::new(ScriptedClient::new(vec![
                        tool_response(),
                        final_response("harness source captured"),
                    ])))
                    .with_session_checkpoint_export_sink(sink.clone())
                    .with_permission_policy(PermissionPolicy::new().allow("generation_probe(*)"))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_continuation(false),
            ),
        )
        .await
        .unwrap();
    for (generation, surface) in [(1, 'b'), (2, 'c'), (3, 'd')] {
        source
            .apply_capability_batch(
                capability_batch(generation, surface, "harness-generation-three"),
                CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    source.send("capture for harness", None).await.unwrap();
    let export = sink.exports.lock().unwrap().pop().unwrap();
    source.close().await;

    let harness_workspace = tempfile::tempdir().unwrap();
    let harness = AgentProtocolHarness::new(
        release_manifest(),
        Arc::new(Agent::from_config(offline_config()).await.unwrap()),
        harness_workspace.path().display().to_string(),
    )
    .unwrap()
    .with_session_options(
        SessionOptions::new()
            .with_llm_client(Arc::new(ScriptedClient::new(vec![final_response(
                "harness recovery complete",
            )])))
            .with_permission_policy(PermissionPolicy::new().allow("generation_probe(*)"))
            .with_planning_mode(PlanningMode::Disabled)
            .with_continuation(false),
    );
    let request = AgentProtocolRunRecoverExactV1 {
        schema: AgentProtocolRunRecoverExactV1::SCHEMA.into(),
        request_id: "harness-capability-recovery".into(),
        identity: AgentProtocolRunIdentityV1 {
            schema: AgentProtocolRunIdentityV1::SCHEMA.into(),
            protocol: AGENT_PROTOCOL_V1.into(),
            agent_release_identity: harness.agent_release_identity().into(),
            session_id: "capability-harness-session".into(),
            run_id: "capability-harness-target".into(),
        },
        checkpoint: export.descriptor().clone(),
    };

    let missing = harness
        .execute_checkpoint_recovery(&request, export.clone())
        .await
        .expect_err("the Harness cannot invent historical runtime values");
    assert!(matches!(
        missing,
        a3s_code_core::AgentProtocolCheckpointRecoveryError::Checkpoint(
            SessionCheckpointError::ContentDrift(_)
        )
    ));
    assert_eq!(harness.session_count().await, 0);

    let receipt = harness
        .execute_checkpoint_recovery_with_capability_batch(
            &request,
            export,
            capability_batch(3, 'd', "harness-generation-three"),
        )
        .await
        .unwrap();
    assert!(!receipt.replayed);
    let page_request = AgentProtocolEventPageRequestV1 {
        schema: AgentProtocolEventPageRequestV1::SCHEMA.into(),
        identity: request.identity.clone(),
        after_event_sequence: None,
        limit: 64,
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if harness.event_page(&page_request).await.unwrap().state
                == AgentProtocolRunStateV1::Completed
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Harness capability recovery must finish");
    harness.close().await;
}
