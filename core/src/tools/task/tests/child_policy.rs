#![cfg(test)]

use super::*;

struct PolicyClient(Mutex<Vec<String>>);

#[async_trait::async_trait]
impl LlmClient for PolicyClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.0
            .lock()
            .unwrap()
            .push(system.unwrap_or_default().to_owned());
        assert!(!tools.iter().any(|tool| matches!(
            tool.name.as_str(),
            "task" | "parallel_task" | "goal" | "goal_tick"
        )));
        if tools.iter().any(|tool| tool.name == "emit_step_output") {
            Ok(MockLlmClient::tool_call_response(
                "output",
                "emit_step_output",
                serde_json::json!({"verdict":"ok"}),
            ))
        } else {
            Ok(text_response("ready"))
        }
    }
    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("use complete")
    }
    fn native_structured_support(&self) -> crate::llm::structured::NativeStructuredSupport {
        crate::llm::structured::NativeStructuredSupport::ForcedTool
    }
}

#[tokio::test]
async fn child_policy_covers_normal_and_structured_task_with_original_budget() {
    for mode in 0..3 {
        let workspace = tempfile::tempdir().unwrap();
        let client = Arc::new(PolicyClient(Mutex::new(Vec::new())));
        let guard = Arc::new(CountingAllowBudgetGuard::default());
        let mut parent = child_context_with_budget(guard.clone());
        let policy = crate::WorkPolicyV1::new("parent", 1, "EXACT-CHILD-POLICY").unwrap();
        parent.work_policy = Some(policy.clone());
        let registry = Arc::new(AgentRegistry::new());
        let role = crate::subagent::AgentDefinition::new("policy-child", "Read-only child")
            .with_prompt("CHILD-ROLE-PRESERVED");
        registry.register(if mode == 1 { role.tool_free() } else { role });
        let executor = TaskExecutor::new(
            registry,
            client.clone(),
            workspace.path().display().to_string(),
        )
        .with_parent_context(parent);
        let mut spec = AgentStepSpec::new("policy-step", "policy-child", "policy", "Reply ready.");
        if mode != 0 {
            spec = spec.with_output_schema(verdict_schema());
        }
        let result = executor.execute_step(spec, None).await;
        assert!(result.success, "{}", result.output);
        let systems = client.0.lock().unwrap();
        let expected_calls = if mode == 2 { 2 } else { 1 };
        assert_eq!(
            systems.len(),
            expected_calls,
            "policy inheritance adds no model call"
        );
        for system in systems.iter() {
            assert_eq!(system.matches(policy.instructions()).count(), 1);
            assert_eq!(system.matches("CHILD-ROLE-PRESERVED").count(), 1);
        }
        assert_eq!(guard.records.load(Ordering::SeqCst), expected_calls);
    }
}

/// Exposed tool names per call; the first call also tries a shell write.
struct ReviewClient(Mutex<Vec<Vec<String>>>);

#[async_trait::async_trait]
impl LlmClient for ReviewClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let mut calls = self.0.lock().unwrap();
        calls.push(tools.iter().map(|tool| tool.name.clone()).collect());
        Ok(if calls.len() == 1 {
            MockLlmClient::tool_call_response(
                "shell",
                "bash",
                serde_json::json!({"command": "touch review-wrote.txt"}),
            )
        } else {
            text_response("no findings")
        })
    }
    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("use complete")
    }
}

#[tokio::test]
async fn review_under_a_work_policy_is_read_only_and_unchanged_without_one() {
    for policy in [
        None,
        Some(crate::WorkPolicyV1::new("engineering-v1", 1, "ENGINEERING").unwrap()),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let client = Arc::new(ReviewClient(Mutex::new(Vec::new())));
        let mut parent = child_context_with_budget(Arc::new(CountingAllowBudgetGuard::default()));
        parent.work_policy = policy.clone();
        let executor = TaskExecutor::new(
            Arc::new(AgentRegistry::new()),
            client.clone(),
            workspace.path().display().to_string(),
        )
        .with_parent_context(parent);
        let result = executor
            .execute_step(
                AgentStepSpec::new("review-step", "review", "review", "Review the workspace."),
                None,
            )
            .await;
        let exposed = client.0.lock().unwrap()[0].clone();
        if policy.is_none() {
            assert!(exposed.iter().any(|name| name == "bash"), "{exposed:?}");
            continue;
        }
        assert!(result.success, "{}", result.output);
        assert!(exposed.iter().any(|name| name == "read"), "{exposed:?}");
        for denied in ["bash", "write", "edit", "task", "parallel_task"] {
            assert!(!exposed.iter().any(|name| name == denied), "{denied}");
        }
        assert!(!workspace.path().join("review-wrote.txt").exists());
    }
}

#[tokio::test]
async fn child_policy_uses_invoking_run_instead_of_captured_registration() {
    for bound in [
        None,
        Some(crate::WorkPolicyV1::new("run", 1, "CURRENT-RUN-POLICY").unwrap()),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let client = Arc::new(PolicyClient(Mutex::new(Vec::new())));
        let mut parent = child_context_with_budget(Arc::new(CountingAllowBudgetGuard::default()));
        parent.work_policy =
            Some(crate::WorkPolicyV1::new("stale", 1, "STALE-REGISTRATION").unwrap());
        let executor = Arc::new(
            TaskExecutor::new(
                Arc::new(AgentRegistry::new()),
                client.clone(),
                workspace.path().display().to_string(),
            )
            .with_parent_context(parent),
        );
        let mut context =
            ToolContext::new(workspace.path().to_path_buf()).with_run_governance(None, None);
        context.run_work_policy = bound.clone();
        let scoped = executor.scoped_for_invocation(&context);
        let result = scoped
            .execute_step(
                AgentStepSpec::new("run-child", "general", "read", "Reply ready."),
                None,
            )
            .await;
        assert!(result.success, "{}", result.output);
        let systems = client.0.lock().unwrap();
        assert_eq!(systems.len(), 1);
        assert!(!systems[0].contains("STALE-REGISTRATION"));
        assert_eq!(
            systems[0].matches("CURRENT-RUN-POLICY").count(),
            usize::from(bound.is_some())
        );
    }
}
