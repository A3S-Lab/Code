use super::*;

#[cfg(unix)]
use crate::mcp::test_support::{compile_fake_server, fixture_started_pids, process_exists};
use crate::mcp::test_support::{mcp_tool, ready_binding};
#[cfg(unix)]
use crate::mcp::McpProjectionAdapter;
use crate::mcp::{McpClient, McpServerConfig, McpTransportConfig};

struct ClosingClientEffect {
    client: Arc<McpClient>,
}

#[async_trait]
impl CapabilityEffect for ClosingClientEffect {
    fn name(&self) -> &str {
        "test.projected-mcp-connection"
    }

    async fn close(self: Box<Self>) -> std::result::Result<(), CapabilityEffectError> {
        self.client
            .close()
            .await
            .map_err(|error| CapabilityEffectError::new(error.to_string()))
    }
}

struct McpCutoverClient {
    calls: AtomicUsize,
    observed_definitions: Mutex<Vec<String>>,
}

impl McpCutoverClient {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            observed_definitions: Mutex::new(Vec::new()),
        }
    }

    fn observe_definition(&self, tools: &[ToolDefinition]) -> anyhow::Result<()> {
        let definition = tools
            .iter()
            .find(|definition| definition.name == "mcp__catalog__lookup")
            .ok_or_else(|| anyhow::anyhow!("projected MCP definition is missing"))?;
        self.observed_definitions
            .lock()
            .unwrap()
            .push(definition.description.clone());
        Ok(())
    }

    fn tool_call(id: &str, generation: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: "mcp__catalog__lookup".to_string(),
                    input: serde_json::json!({"generation": generation}),
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
            stop_reason: Some("tool_use".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }

    fn latest_tool_result(messages: &[Message]) -> anyhow::Result<String> {
        messages
            .iter()
            .rev()
            .flat_map(|message| message.content.iter().rev())
            .find_map(|block| match block {
                ContentBlock::ToolResult { content, .. } => Some(content.as_text()),
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("projected MCP result is missing"))
    }
}

#[async_trait]
impl LlmClient for McpCutoverClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if !tools
            .iter()
            .any(|definition| definition.name == "mcp__catalog__lookup")
        {
            return Ok(CutoverClient::final_text("auxiliary completion"));
        }

        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => {
                self.observe_definition(tools)?;
                Ok(Self::tool_call("mcp-generation-one", "one"))
            }
            1 => {
                let result = Self::latest_tool_result(messages)?;
                anyhow::ensure!(
                    result.contains("generation-one"),
                    "first Run called another MCP generation: {result}"
                );
                Ok(CutoverClient::final_text("MCP generation one complete"))
            }
            2 => {
                self.observe_definition(tools)?;
                Ok(Self::tool_call("mcp-generation-two", "two"))
            }
            3 => {
                let result = Self::latest_tool_result(messages)?;
                anyhow::ensure!(
                    result.contains("generation-two"),
                    "second Run called another MCP generation: {result}"
                );
                Ok(CutoverClient::final_text("MCP generation two complete"))
            }
            call => anyhow::bail!("unexpected McpCutoverClient call {call}"),
        }
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by the MCP cutover test")
    }
}

fn closing_adapter(binding: Arc<crate::mcp::McpBinding>, client: Arc<McpClient>) -> ReadyAdapter {
    ReadyAdapter {
        value: CapabilityValue::Mcp(binding),
        effect: Some(Box::new(ClosingClientEffect { client })),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_run_keeps_exact_mcp_client_and_use_lease_across_cutover() {
    let llm = Arc::new(McpCutoverClient::new());
    let session = Arc::new(
        test_session_with_client(
            "capability-mcp-cutover",
            Arc::clone(&llm) as Arc<dyn LlmClient>,
        )
        .await,
    );
    let first_acquired = Arc::new(AtomicUsize::new(0));
    let first_dropped = Arc::new(AtomicUsize::new(0));
    let second_acquired = Arc::new(AtomicUsize::new(0));
    let second_dropped = Arc::new(AtomicUsize::new(0));

    let (first_binding, first_transport, first_client) = ready_binding(
        "catalog",
        "generation-one",
        vec![mcp_tool("lookup", "generation-one")],
    )
    .await;
    first_transport.block_calls();
    let first_upstream = use_generation(1, 'a');
    let (first_set, first_ids) = use_kind_set(
        1,
        first_upstream.clone(),
        CapabilityKind::Mcp,
        &[("catalog", 'b')],
    );
    let mut first = SessionCapabilityBatch::from_use_projection(
        first_set,
        provider(first_upstream, &first_acquired, &first_dropped),
    )
    .unwrap();
    first
        .stage(
            first_ids["catalog"].clone(),
            closing_adapter(first_binding, first_client),
        )
        .unwrap();
    session
        .apply_capability_batch(first, CancellationToken::new())
        .await
        .unwrap();

    let old_run = tokio::spawn({
        let session = Arc::clone(&session);
        async move {
            session
                .send("Call the projected MCP tool once.", None)
                .await
        }
    });
    first_transport
        .call_entered
        .acquire()
        .await
        .unwrap()
        .forget();
    assert_eq!(first_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);

    let (second_binding, second_transport, second_client) = ready_binding(
        "catalog",
        "generation-two",
        vec![mcp_tool("lookup", "generation-two")],
    )
    .await;
    let second_upstream = use_generation(2, 'c');
    let (second_set, second_ids) = use_kind_set(
        2,
        second_upstream.clone(),
        CapabilityKind::Mcp,
        &[("catalog", 'd')],
    );
    let mut second = SessionCapabilityBatch::from_use_projection(
        second_set,
        provider(second_upstream, &second_acquired, &second_dropped),
    )
    .unwrap();
    second
        .stage(
            second_ids["catalog"].clone(),
            closing_adapter(second_binding, second_client),
        )
        .unwrap();
    session
        .apply_capability_batch(second, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(first_transport.close_count(), 0);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 0);
    let premature_cleanup = session.drain_capability_cleanup().await;
    assert_eq!(premature_cleanup.retired_batches, 0);
    assert_eq!(first_transport.close_count(), 0);

    first_transport.release_call.add_permits(1);
    let old_result = old_run.await.unwrap().unwrap();
    assert_eq!(old_result.text, "MCP generation one complete");
    assert_eq!(first_dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        first_transport.calls()[0].name,
        "lookup",
        "the projected wrapper must call the raw exact-client identity"
    );

    let cleanup = session.drain_capability_cleanup().await;
    assert_eq!(cleanup.retired_batches, 1);
    assert_eq!(cleanup.effects_closed, 1);
    assert_eq!(first_transport.close_count(), 1);

    let new_result = session
        .send("Call the current projected MCP tool once.", None)
        .await
        .unwrap();
    assert_eq!(new_result.text, "MCP generation two complete");
    assert_eq!(second_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(second_dropped.load(Ordering::SeqCst), 1);
    assert_eq!(second_transport.calls()[0].name, "lookup");
    assert_eq!(
        &*llm.observed_definitions.lock().unwrap(),
        &["generation-one", "generation-two"]
    );

    session.close().await;
    assert_eq!(second_transport.close_count(), 1);
}

#[tokio::test]
async fn compatibility_mcp_names_conflict_before_and_after_projection_publication() {
    let before_session = test_session("capability-mcp-conflict-before").await;
    before_session
        .register_dynamic_tool(Arc::new(VersionedTool {
            name: "mcp__catalog__lookup".to_string(),
            version: "compatibility",
            executions: Arc::new(Mutex::new(Vec::new())),
        }))
        .unwrap();
    let before_stamp = before_session.capability_catalog_stamp();
    let (binding, transport, client) = ready_binding(
        "catalog",
        "projected",
        vec![mcp_tool("lookup", "projected")],
    )
    .await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Mcp,
        &[("catalog", 'b')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage(ids["catalog"].clone(), closing_adapter(binding, client))
        .unwrap();
    assert!(matches!(
        before_session
            .apply_capability_batch(batch, CancellationToken::new())
            .await,
        Err(CapabilityRuntimeError::RuntimeNameConflict {
            kind: CapabilityKind::Mcp,
            public_name,
        }) if public_name == "mcp__catalog__lookup"
    ));
    assert_eq!(before_session.capability_catalog_stamp(), before_stamp);
    let cleanup = before_session.drain_capability_cleanup().await;
    assert_eq!(cleanup.rollback_batches, 1);
    assert_eq!(transport.close_count(), 1);

    let after_session = test_session("capability-mcp-conflict-after").await;
    let (binding, _transport, client) = ready_binding(
        "catalog",
        "projected",
        vec![mcp_tool("lookup", "projected")],
    )
    .await;
    let upstream = use_generation(1, 'c');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Mcp,
        &[("catalog", 'd')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage(ids["catalog"].clone(), closing_adapter(binding, client))
        .unwrap();
    after_session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    let projected_status = after_session.mcp_status().await;
    assert!(projected_status["catalog"].connected);
    assert_eq!(projected_status["catalog"].tool_count, 1);

    assert!(matches!(
        after_session.register_dynamic_tool(Arc::new(VersionedTool {
            name: "mcp__catalog__lookup".to_string(),
            version: "compatibility",
            executions: Arc::new(Mutex::new(Vec::new())),
        })),
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Tool,
                public_name,
            }
        )) if public_name == "mcp__catalog__lookup"
    ));

    let incompatible_config = McpServerConfig {
        name: "catalog".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "must-not-be-spawned".to_string(),
            args: Vec::new(),
        },
        enabled: true,
        env: std::collections::HashMap::new(),
        oauth: None,
        tool_timeout_secs: 1,
    };
    assert!(matches!(
        after_session.add_mcp_server(incompatible_config).await,
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Mcp,
                public_name,
            }
        )) if public_name == "catalog"
    ));
    assert!(matches!(
        after_session.remove_mcp_server("catalog").await,
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::RuntimeNameConflict {
                kind: CapabilityKind::Mcp,
                public_name,
            }
        )) if public_name == "catalog"
    ));

    after_session.close().await;
}

#[tokio::test]
async fn configured_compatibility_server_name_blocks_projection_before_publication() {
    let session = test_session("capability-mcp-server-conflict-before").await;
    session
        .mcp_manager
        .register_server(McpServerConfig {
            name: "catalog".to_string(),
            transport: McpTransportConfig::Stdio {
                command: "unused".to_string(),
                args: Vec::new(),
            },
            enabled: false,
            env: std::collections::HashMap::new(),
            oauth: None,
            tool_timeout_secs: 1,
        })
        .await;
    let before = session.capability_catalog_stamp();
    let (binding, transport, client) = ready_binding(
        "catalog",
        "projected",
        vec![mcp_tool("lookup", "projected")],
    )
    .await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Mcp,
        &[("catalog", 'b')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage(ids["catalog"].clone(), closing_adapter(binding, client))
        .unwrap();

    assert!(matches!(
        session
            .apply_capability_batch(batch, CancellationToken::new())
            .await,
        Err(CapabilityRuntimeError::RuntimeNameConflict {
            kind: CapabilityKind::Mcp,
            public_name,
        }) if public_name == "catalog"
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    let cleanup = session.drain_capability_cleanup().await;
    assert_eq!(cleanup.rollback_batches, 1);
    assert_eq!(transport.close_count(), 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_rolls_back_a_prepared_stdio_connection_without_publication() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let fixture_dir = tempfile::tempdir().unwrap();
    let server = fixture_dir.path().join("mcp-fake-server");
    let log_path = fixture_dir.path().join("mcp-fake-server.log");
    compile_fake_server(&server);

    let session = Arc::new(test_session("capability-mcp-cancelled-prepare").await);
    let before = session.capability_catalog_stamp();
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'e');
    let (set, _ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Mcp,
        &[("alpha", 'f'), ("omega", 'a')],
    );
    let readiness = crate::capability::CapabilityReadinessPlan::from_set(&set).unwrap();
    let prepared_id = readiness.activation_order()[0].clone();
    let waiting_id = readiness.activation_order()[1].clone();
    let prepared_name = set.get(&prepared_id).unwrap().public_name().to_string();
    let entered = Arc::new(Semaphore::new(0));
    let mut environment = std::collections::HashMap::new();
    environment.insert(
        "A3S_TEST_MCP_LOG".to_string(),
        log_path.display().to_string(),
    );
    let config = McpServerConfig {
        name: prepared_name,
        transport: McpTransportConfig::Stdio {
            command: server.display().to_string(),
            args: Vec::new(),
        },
        enabled: true,
        env: environment,
        oauth: None,
        tool_timeout_secs: 5,
    };
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage(prepared_id, McpProjectionAdapter::new(config))
        .unwrap();
    batch
        .stage(
            waiting_id,
            CancellationWaitingAdapter {
                entered: Arc::clone(&entered),
            },
        )
        .unwrap();

    let cancellation = CancellationToken::new();
    let apply = tokio::spawn({
        let session = Arc::clone(&session);
        let cancellation = cancellation.clone();
        async move { session.apply_capability_batch(batch, cancellation).await }
    });
    entered.acquire().await.unwrap().forget();
    let log = std::fs::read_to_string(&log_path).unwrap();
    let pids = fixture_started_pids(&log);
    assert_eq!(pids.len(), 1, "unexpected fixture log: {log}");
    assert!(process_exists(pids[0]));

    cancellation.cancel();
    assert!(matches!(
        apply.await.unwrap(),
        Err(CapabilityRuntimeError::Cancelled)
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(acquired.load(Ordering::SeqCst), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);

    let cleanup = session.drain_capability_cleanup().await;
    assert_eq!(cleanup.rollback_batches, 1);
    assert_eq!(cleanup.effects_closed, 1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while process_exists(pids[0]) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("rolled-back MCP process must be reaped");
}
