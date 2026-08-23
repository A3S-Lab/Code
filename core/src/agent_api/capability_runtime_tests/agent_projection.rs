use super::*;

struct AgentCutoverClient {
    parent_calls: AtomicUsize,
    observed_task_versions: Mutex<Vec<String>>,
    observed_child_versions: Mutex<Vec<String>>,
    old_surface_observed: Semaphore,
    release_old_call: Semaphore,
}

impl AgentCutoverClient {
    fn new() -> Self {
        Self {
            parent_calls: AtomicUsize::new(0),
            observed_task_versions: Mutex::new(Vec::new()),
            observed_child_versions: Mutex::new(Vec::new()),
            old_surface_observed: Semaphore::new(0),
            release_old_call: Semaphore::new(0),
        }
    }

    fn task_call(generation: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: format!("task-{generation}"),
                    name: "task".to_string(),
                    input: serde_json::json!({
                        "tasks": [{
                            "agent": "projected-agent",
                            "description": format!("Run {generation}"),
                            "prompt": format!("Return the {generation} marker.")
                        }]
                    }),
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

    fn observe_task_definition(
        &self,
        tools: &[ToolDefinition],
        expected: &str,
    ) -> anyhow::Result<()> {
        let definition = tools
            .iter()
            .find(|definition| definition.name == "task")
            .ok_or_else(|| anyhow::anyhow!("task Tool definition is missing"))?;
        anyhow::ensure!(
            definition.description.contains(expected),
            "task Tool definition does not contain {expected}: {}",
            definition.description
        );
        self.observed_task_versions
            .lock()
            .unwrap()
            .push(expected.to_string());
        Ok(())
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
            .ok_or_else(|| anyhow::anyhow!("task result is missing"))
    }
}

#[async_trait]
impl LlmClient for AgentCutoverClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if tools.iter().any(|definition| definition.name == "task") {
            return match self.parent_calls.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    self.observe_task_definition(tools, "generation-one")?;
                    self.old_surface_observed.add_permits(1);
                    self.release_old_call.acquire().await?.forget();
                    Ok(Self::task_call("generation-one"))
                }
                1 => {
                    let result = Self::latest_tool_result(messages)?;
                    anyhow::ensure!(
                        result.contains("child generation-one"),
                        "first Run delegated through another Agent generation: {result}"
                    );
                    Ok(CutoverClient::final_text("agent generation one complete"))
                }
                2 => {
                    self.observe_task_definition(tools, "generation-two")?;
                    Ok(Self::task_call("generation-two"))
                }
                3 => {
                    let result = Self::latest_tool_result(messages)?;
                    anyhow::ensure!(
                        result.contains("child generation-two"),
                        "second Run delegated through another Agent generation: {result}"
                    );
                    Ok(CutoverClient::final_text("agent generation two complete"))
                }
                call => anyhow::bail!("unexpected AgentCutoverClient parent call {call}"),
            };
        }

        let system = system.unwrap_or_default();
        for version in ["generation-one", "generation-two"] {
            if system.contains(&format!("PROJECTED_AGENT_{version}")) {
                self.observed_child_versions
                    .lock()
                    .unwrap()
                    .push(version.to_string());
                return Ok(CutoverClient::final_text(&format!("child {version}")));
            }
        }

        Ok(CutoverClient::final_text("auxiliary completion"))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by the Agent cutover test")
    }
}

struct AutomaticAgentClient {
    observed_child_versions: Mutex<Vec<String>>,
}

#[async_trait]
impl LlmClient for AutomaticAgentClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if system
            .unwrap_or_default()
            .contains("PROJECTED_AGENT_automatic-generation")
        {
            self.observed_child_versions
                .lock()
                .unwrap()
                .push("automatic-generation".to_string());
            return Ok(CutoverClient::final_text("automatic child complete"));
        }
        if tools.iter().any(|definition| definition.name == "task") {
            return Ok(CutoverClient::final_text("automatic parent complete"));
        }
        Ok(CutoverClient::final_text("auxiliary completion"))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by the automatic Agent test")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn active_run_keeps_n_agent_registry_task_binding_and_use_lease_across_cutover() {
    let client = Arc::new(AgentCutoverClient::new());
    let session = Arc::new(
        test_session_with_client(
            "capability-agent-cutover",
            Arc::clone(&client) as Arc<dyn LlmClient>,
        )
        .await,
    );
    let first_acquired = Arc::new(AtomicUsize::new(0));
    let first_dropped = Arc::new(AtomicUsize::new(0));
    let second_acquired = Arc::new(AtomicUsize::new(0));
    let second_dropped = Arc::new(AtomicUsize::new(0));

    let first_upstream = use_generation(1, 'a');
    let (first_set, first_ids) =
        use_agent_set(1, first_upstream.clone(), &[("projected-agent", 'b')]);
    let mut first = SessionCapabilityBatch::from_use_projection(
        first_set,
        provider(first_upstream, &first_acquired, &first_dropped),
    )
    .unwrap();
    first
        .stage_value(
            first_ids["projected-agent"].clone(),
            CapabilityValue::Agent(projected_agent("projected-agent", "generation-one")),
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
                .send("Delegate one task to projected-agent.", None)
                .await
        }
    });
    client
        .old_surface_observed
        .acquire()
        .await
        .unwrap()
        .forget();
    assert_eq!(first_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);

    let second_upstream = use_generation(2, 'c');
    let (second_set, second_ids) =
        use_agent_set(2, second_upstream.clone(), &[("projected-agent", 'd')]);
    let mut second = SessionCapabilityBatch::from_use_projection(
        second_set,
        provider(second_upstream, &second_acquired, &second_dropped),
    )
    .unwrap();
    second
        .stage_value(
            second_ids["projected-agent"].clone(),
            CapabilityValue::Agent(projected_agent("projected-agent", "generation-two")),
        )
        .unwrap();
    session
        .apply_capability_batch(second, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 0);
    client.release_old_call.add_permits(1);
    let old_result = old_run.await.unwrap().unwrap();
    assert_eq!(old_result.text, "agent generation one complete");
    assert_eq!(old_result.tool_calls_count, 1);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 1);

    let new_result = session
        .send("Delegate one task to projected-agent.", None)
        .await
        .unwrap();
    assert_eq!(new_result.text, "agent generation two complete");
    assert_eq!(new_result.tool_calls_count, 1);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(second_dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        &*client.observed_task_versions.lock().unwrap(),
        &["generation-one", "generation-two"]
    );
    assert_eq!(
        &*client.observed_child_versions.lock().unwrap(),
        &["generation-one", "generation-two"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_delegation_uses_the_run_frozen_projected_agent_registry() {
    let client = Arc::new(AutomaticAgentClient {
        observed_child_versions: Mutex::new(Vec::new()),
    });
    let mut config = super::super::tests::test_config();
    config.auto_delegation.enabled = true;
    config.auto_delegation.auto_parallel = false;
    config.auto_delegation.max_tasks = 1;
    let agent = Agent::from_config(config).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/capability-automatic-agent".to_string(),
            Arc::clone(&client) as Arc<dyn LlmClient>,
            &SessionOptions::new()
                .with_session_id("capability-automatic-agent")
                .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"))
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_agent_set(1, upstream.clone(), &[("projected-agent", 'b')]);
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-agent"].clone(),
            CapabilityValue::Agent(projected_agent("projected-agent", "automatic-generation")),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let result = session
        .send("Ask @projected-agent to return the requested marker.", None)
        .await
        .unwrap();
    assert_eq!(result.text, "automatic parent complete");
    assert_eq!(result.tool_calls_count, 1);
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(
        &*client.observed_child_versions.lock().unwrap(),
        &["automatic-generation"]
    );
}
