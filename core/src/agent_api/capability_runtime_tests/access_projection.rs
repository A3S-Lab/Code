#![cfg(test)]

use super::*;

async fn install_tool_access(
    session: &AgentSession,
    generation: u64,
    allowed: bool,
    executions: &Arc<Mutex<Vec<&'static str>>>,
) -> crate::capability::CapabilityId {
    let upstream = use_generation(generation, if generation == 1 { 'a' } else { 'c' });
    let (set, ids) = use_tool_set(generation, upstream.clone(), &[("projected", 'b')]);
    let id = ids["projected"].clone();
    let counter = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &counter, &counter))
            .unwrap()
            .with_allowed_capabilities(if allowed { vec![id.clone()] } else { vec![] })
            .unwrap();
    batch
        .stage_value(
            id.clone(),
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "projected".to_owned(),
                version: "accepted",
                executions: Arc::clone(executions),
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn projected_tool_refuses_unscoped_stale_denied_and_other_catalog_calls() {
    let session = test_session("capability-access-call-boundary").await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    install_tool_access(&session, 1, true, &executions).await;
    let (old_runtime, old_run) =
        super::super::agent_loop_runtime::pin_and_admit_runtime_projection(&session)
            .await
            .unwrap();
    let tool = old_runtime
        .tool_executor()
        .registry()
        .get("projected")
        .unwrap();
    let runtime = crate::capability::AgentCapabilityRuntime::from_run(old_run.run_scope());
    let turn = runtime.begin_turn(1).unwrap();
    let ctx =
        ToolContext::new(session.workspace.clone()).with_capability_context(turn.tool_context());
    assert!(tool
        .execute(
            &serde_json::json!({}),
            &ToolContext::new(session.workspace.clone())
        )
        .await
        .is_err());
    assert!(executions.lock().unwrap().is_empty());
    assert!(tool.execute(&serde_json::json!({}), &ctx).await.is_ok());

    install_tool_access(&session, 2, false, &executions).await;
    let (new_runtime, new_run) =
        super::super::agent_loop_runtime::pin_and_admit_runtime_projection(&session)
            .await
            .unwrap();
    assert!(new_runtime
        .tool_executor()
        .registry()
        .get("projected")
        .is_none());
    let denied_turn = crate::capability::AgentCapabilityRuntime::from_run(new_run.run_scope())
        .begin_turn(1)
        .unwrap();
    let denied_ctx = ToolContext::new(session.workspace.clone())
        .with_capability_context(denied_turn.tool_context());
    assert!(tool
        .execute(&serde_json::json!({}), &denied_ctx)
        .await
        .is_err());
    assert!(
        tool.execute(&serde_json::json!({}), &ctx).await.is_ok(),
        "old admitted Run retains its authority"
    );
    install_tool_access(&session, 3, true, &executions).await;
    let (third_runtime, third_run) =
        super::super::agent_loop_runtime::pin_and_admit_runtime_projection(&session)
            .await
            .unwrap();
    let third_turn = crate::capability::AgentCapabilityRuntime::from_run(third_run.run_scope())
        .begin_turn(1)
        .unwrap();
    let third_ctx = ToolContext::new(session.workspace.clone())
        .with_capability_context(third_turn.tool_context());
    let third_tool = third_runtime
        .tool_executor()
        .registry()
        .get("projected")
        .unwrap();
    assert!(
        tool.execute(&serde_json::json!({}), &third_ctx)
            .await
            .is_err(),
        "an allowed name in another catalog cannot authorize old code"
    );
    assert!(third_tool
        .execute(&serde_json::json!({}), &ctx)
        .await
        .is_err());
    let background = turn
        .tool_context()
        .admit_subtask("promoted-proof", true)
        .unwrap();
    turn.close().await.unwrap();
    assert!(tool.execute(&serde_json::json!({}), &ctx).await.is_err());
    let child_runtime = background.runtime();
    assert!(
        child_runtime
            .projected_identity(CapabilityKind::Tool, "projected")
            .unwrap()
            .is_some(),
        "promoted work resolves through its live Subtask, not the closed invoking Turn"
    );
    let child_turn = child_runtime.begin_turn(1).unwrap();
    let child_ctx = ToolContext::new(session.workspace.clone())
        .with_capability_context(child_turn.tool_context());
    assert!(tool
        .execute(&serde_json::json!({}), &child_ctx)
        .await
        .is_ok());
    assert_eq!(
        &*executions.lock().unwrap(),
        &["accepted", "accepted", "accepted"]
    );
    child_turn.close().await.unwrap();
    background.close().await.unwrap();
    denied_turn.close().await.unwrap();
    old_run.close().await.unwrap();
    new_run.close().await.unwrap();
    third_turn.close().await.unwrap();
    third_run.close().await.unwrap();
}

struct DeniedCallClient(AtomicUsize);

#[async_trait]
impl LlmClient for DeniedCallClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::ensure!(
            !tools.iter().any(|tool| tool.name == "projected"),
            "denied Tool leaked to model"
        );
        match self.0.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(CutoverClient::tool_call("malicious-denied-call")),
            1 => {
                let failed = messages
                    .iter()
                    .flat_map(|message| &message.content)
                    .any(|block| {
                        matches!(
                            block,
                            ContentBlock::ToolResult {
                                is_error: Some(true),
                                ..
                            }
                        )
                    });
                anyhow::ensure!(failed, "denied Tool call did not produce an error");
                Ok(CutoverClient::final_text("denied safely"))
            }
            _ => anyhow::bail!("unexpected infer"),
        }
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is outside this contract")
    }
}

#[tokio::test]
async fn denied_tool_is_absent_from_actual_model_and_cannot_be_requested_by_name() {
    let client = Arc::new(DeniedCallClient(AtomicUsize::new(0)));
    let session = test_session_with_client("capability-access-model", client.clone()).await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    install_tool_access(&session, 1, false, &executions).await;
    let result = session.send("Try the denied Tool.", None).await.unwrap();
    assert_eq!(result.text, "denied safely");

    assert!(executions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn raw_run_admission_cannot_widen_published_access() {
    let session = test_session("capability-access-no-bypass").await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    let id = install_tool_access(&session, 1, false, &executions).await;
    let projection = session.capability_catalog.pin();
    let ceiling = session
        .capability_run_ceiling(projection.projection().set(), None)
        .unwrap();
    assert!(ceiling.allows(&id));
    let error = crate::capability::SessionCapabilityRun::admit(
        projection,
        "forged",
        "forged",
        ceiling,
        CancellationToken::new(),
        &session.workspace,
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(
        error,
        CapabilityRuntimeError::Scope(crate::capability::CapabilityScopeError::CeilingExpansion {
            dimension: "projected_capabilities"
        })
    ));
}

struct TurnEffect(Arc<AtomicUsize>);

#[async_trait]
impl CapabilityEffect for TurnEffect {
    fn name(&self) -> &str {
        "projected.turn-proof"
    }
    async fn close(self: Box<Self>) -> std::result::Result<(), CapabilityEffectError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct TurnProofTool {
    closed: Arc<AtomicUsize>,
    context: Arc<Mutex<Option<ToolContext>>>,
}

#[async_trait]
impl Tool for TurnProofTool {
    fn name(&self) -> &str {
        "projected"
    }
    fn description(&self) -> &str {
        "Register a Turn-owned resource"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    async fn execute(
        &self,
        _: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        ctx.register_capability_effect(TurnEffect(Arc::clone(&self.closed)))?;
        *self.context.lock().unwrap() = Some(ctx.clone());
        Ok(ToolOutput::success("resource accepted"))
    }
}

struct TurnProofClient {
    calls: AtomicUsize,
    closed: Arc<AtomicUsize>,
    context: Arc<Mutex<Option<ToolContext>>>,
}

#[async_trait]
impl LlmClient for TurnProofClient {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(CutoverClient::tool_call("scope-proof"));
        }
        anyhow::ensure!(
            self.closed.load(Ordering::SeqCst) == 1,
            "next model observation preceded resource cleanup"
        );
        let saved = self
            .context
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no actual Tool context"))?;
        anyhow::ensure!(
            saved
                .register_capability_effect(TurnEffect(Arc::clone(&self.closed)))
                .is_err(),
            "stale Tool context still accepts effects"
        );
        Ok(CutoverClient::final_text("resource settled"))
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is outside this contract")
    }
}

#[tokio::test]
async fn actual_fact_tool_call_owns_and_settles_a_turn_before_next_model_observation() {
    let closed = Arc::new(AtomicUsize::new(0));
    let context = Arc::new(Mutex::new(None));
    let client = Arc::new(TurnProofClient {
        calls: AtomicUsize::new(0),
        closed: closed.clone(),
        context: context.clone(),
    });
    let session = test_session_with_client("capability-access-real-turn", client).await;
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_tool_set(1, upstream.clone(), &[("projected", 'b')]);
    let counter = Arc::new(AtomicUsize::new(0));
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &counter, &counter))
            .unwrap();
    batch
        .stage_value(
            ids["projected"].clone(),
            CapabilityValue::Tool(Arc::new(TurnProofTool {
                closed: closed.clone(),
                context,
            })),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    let result = session
        .send("Call the projected resource Tool.", None)
        .await
        .unwrap();
    assert_eq!(result.text, "resource settled");
    assert_eq!(closed.load(Ordering::SeqCst), 1);
}
