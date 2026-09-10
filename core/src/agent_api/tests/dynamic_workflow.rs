use super::*;

#[cfg(feature = "dynamic-workflow")]
#[tokio::test]
async fn test_dynamic_workflow_parallel_explore_can_use_readonly_web_tools() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "fetch-1",
            "web_fetch",
            serde_json::json!({"url": "not-a-url"}),
        ),
        scripted_text_response("explore web fetch completed"),
        scripted_text_response("independent web review completed"),
    ]));
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_max_parallel_tasks(1)
        .with_manual_delegation_enabled(true);
    let session = agent
        .session_async(dir.path().to_string_lossy().to_string(), Some(opts))
        .await
        .unwrap();
    session.register_dynamic_workflow_runtime().unwrap();

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const fanout = inputs.step_outputs.web_research;
    if (fanout) {
      return { type: "complete", output: { fanout } };
    }
    return {
      type: "schedule_step",
      step_id: "web_research",
      step_name: "task",
      input: {
        tasks: [
          {
            agent: "explore",
            description: "Fetch web evidence",
            prompt: "Use web_fetch on the requested URL and summarize the result.",
            max_steps: 3,
          },
          {
            agent: "explore",
            description: "Review web evidence",
            prompt: "Return a short independent review.",
            max_steps: 1,
          },
        ],
      },
      retry: { max_attempts: 1, delay_ms: 0 },
    };
  }

  return { error: "task should run as a host step" };
}
"#;

    let (_rx, join) = session.tool_with_events(
        "dynamic_workflow",
        serde_json::json!({
            "source": source,
            "run_id": "test-dynamic-workflow-explore-web",
            "allowed_tools": [],
        }),
    );

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(
        result.exit_code, 0,
        "dynamic workflow output: {}",
        result.output
    );
    assert!(
        result.output.contains("explore web fetch completed"),
        "{}",
        result.output
    );
    assert!(
        !result.output.contains("Permission denied"),
        "web_fetch must be permitted for read-only explore research: {}",
        result.output
    );
}

#[cfg(feature = "dynamic-workflow")]
#[tokio::test]
async fn test_dynamic_workflow_parallel_deep_research_inherits_parent_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let workspace_fs: Arc<dyn crate::workspace::WorkspaceFileSystem> =
        Arc::new(TestWorkspaceFs::default());
    let runner = Arc::new(TestWorkspaceRunner::default());
    let runner_backend: Arc<dyn crate::workspace::WorkspaceCommandRunner> = runner.clone();
    let services = crate::workspace::WorkspaceServices::builder(
        crate::workspace::WorkspaceRef::new(
            "deep-research-permission-inheritance",
            dir.path().to_string_lossy(),
        ),
        workspace_fs,
    )
    .command_runner(runner_backend)
    .build();
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "bash-1",
            "bash",
            serde_json::json!({"command": "echo inherited-dynamic-workflow-deep-research"}),
        ),
        scripted_text_response("deep-research child bash completed"),
        scripted_text_response("deep-research control child completed"),
    ]));
    let policy = crate::permissions::PermissionPolicy::new().allow("bash(*)");
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_permission_policy(policy)
        .with_workspace_backend(services)
        .with_max_parallel_tasks(1)
        .with_manual_delegation_enabled(true);
    let session = agent
        .session_async(dir.path().to_string_lossy().to_string(), Some(opts))
        .await
        .unwrap();
    session.register_dynamic_workflow_runtime().unwrap();

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const fanout = inputs.step_outputs.deep_research;
    if (fanout) {
      return { type: "complete", output: { fanout } };
    }
    return {
      type: "schedule_step",
      step_id: "deep_research",
      step_name: "task",
      input: {
        tasks: [
          {
            agent: "deep-research",
            description: "Use inherited parent tool policy",
            prompt: "Run the harmless bash command requested by this regression test.",
            max_steps: 3,
          },
          {
            agent: "deep-research",
            description: "Verify inherited policy independently",
            prompt: "Return a short control result.",
            max_steps: 1,
          },
        ],
      },
      retry: { max_attempts: 1, delay_ms: 0 },
    };
  }

  return { error: "task should run as a host step" };
}
"#;

    let (_rx, join) = session.tool_with_events(
        "dynamic_workflow",
        serde_json::json!({
            "source": source,
            "run_id": "test-dynamic-workflow-deep-research-inherits-permissions",
            "allowed_tools": [],
        }),
    );

    let result = tokio::time::timeout(std::time::Duration::from_secs(15), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(
        result.exit_code, 0,
        "dynamic workflow output: {}",
        result.output
    );
    assert!(
        result.output.contains("deep-research child bash completed"),
        "{}",
        result.output
    );
    assert!(
        !result.output.contains("Permission denied"),
        "deep-research must inherit the parent permission checker: {}",
        result.output
    );
    assert!(
        !result.output.contains("requires confirmation but no HITL"),
        "deep-research must inherit the parent confirmation context: {}",
        result.output
    );
    assert_eq!(
        runner.commands.read().unwrap().as_slice(),
        ["echo inherited-dynamic-workflow-deep-research"],
        "the child must execute through the parent workspace runner"
    );
}

#[cfg(feature = "dynamic-workflow")]
#[tokio::test]
async fn test_dynamic_workflow_parallel_deep_research_inherits_parent_write_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("child-evidence.txt");
    let agent = Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "write-1",
            "write",
            serde_json::json!({
                "file_path": "child-evidence.txt",
                "content": "deep-research child inherited write permission\n"
            }),
        ),
        scripted_text_response("deep-research child write completed"),
        scripted_text_response("deep-research write control completed"),
    ]));
    let policy = crate::permissions::PermissionPolicy::new().allow("write(*)");
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_permission_policy(policy)
        .with_max_parallel_tasks(1)
        .with_manual_delegation_enabled(true);
    let session = agent
        .session_async(dir.path().to_string_lossy().to_string(), Some(opts))
        .await
        .unwrap();
    session.register_dynamic_workflow_runtime().unwrap();

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const fanout = inputs.step_outputs.deep_research;
    if (fanout) {
      return { type: "complete", output: { fanout } };
    }
    return {
      type: "schedule_step",
      step_id: "deep_research",
      step_name: "task",
      input: {
        tasks: [
          {
            agent: "deep-research",
            description: "Use inherited parent write policy",
            prompt: "Write the requested child evidence file.",
            max_steps: 3,
          },
          {
            agent: "deep-research",
            description: "Verify inherited write policy independently",
            prompt: "Return a short control result.",
            max_steps: 1,
          },
        ],
      },
      retry: { max_attempts: 1, delay_ms: 0 },
    };
  }

  return { error: "task should run as a host step" };
}
"#;

    let (_rx, join) = session.tool_with_events(
        "dynamic_workflow",
        serde_json::json!({
            "source": source,
            "run_id": "test-dynamic-workflow-deep-research-inherits-write-permissions",
            "allowed_tools": [],
        }),
    );

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(
        result.exit_code, 0,
        "dynamic workflow output: {}",
        result.output
    );
    assert!(
        result
            .output
            .contains("deep-research child write completed"),
        "{}",
        result.output
    );
    assert!(
        !result.output.contains("Permission denied"),
        "deep-research must inherit parent write permissions: {}",
        result.output
    );
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "deep-research child inherited write permission\n"
    );
}

/// `AgentSession::workflow()` must pre-wire a shared budget ledger and a stable,
/// session-derived root id (so phase checkpoints resume across runs).
#[tokio::test]
async fn test_session_workflow_is_prewired_with_budget_and_stable_root_id() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace", None)
        .await
        .unwrap();

    let wf = session.workflow();
    // An uncapped workflow still owns a ledger (for snapshots / aggregation).
    let snap = wf
        .budget_snapshot()
        .expect("workflow is pre-wired with a shared budget ledger");
    assert_eq!(snap.limit_tokens, None);
    assert_eq!(snap.consumed_tokens, 0);
    assert!(
        wf.root_id().contains(session.id()),
        "root id is session-derived so phase checkpoints are stable across runs"
    );

    // A capped workflow records its hard ceiling.
    let capped = session.workflow_with_token_budget(Some(50_000));
    assert_eq!(capped.budget_snapshot().unwrap().limit_tokens, Some(50_000));
}

/// End-to-end: `session.workflow().agent(spec)` actually spawns a real child
/// agent loop through the wired executor and returns its output. Uses a static
/// mock LLM so the built-in `explore` agent finishes with that text.
#[tokio::test]
async fn test_session_workflow_runs_a_real_child_agent_step() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new();
    let session = agent
        .build_session(
            "/tmp/test-workflow-e2e".into(),
            Arc::new(StaticStreamingClient::new("explored answer")),
            &opts,
        )
        .unwrap();

    let wf = session.workflow();
    let outcome = wf
        .agent(crate::orchestration::AgentStepSpec::new(
            "t1",
            "explore",
            "explore",
            "find the auth code",
        ))
        .await;

    assert!(outcome.success, "child step failed: {}", outcome.output);
    assert_eq!(outcome.agent, "explore");
    assert!(
        outcome.output.contains("explored answer"),
        "child agent returned the mock LLM output; got: {}",
        outcome.output
    );

    // The shared ledger recorded the child's token usage (proves the workflow
    // budget was installed as the child's budget guard).
    assert!(
        wf.budget_snapshot().unwrap().consumed_tokens > 0,
        "child LLM usage fed the shared workflow budget"
    );
}

#[tokio::test]
async fn test_session_workflow_scopes_explicit_host_tools_to_its_children() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tool: Arc<dyn crate::tools::Tool> = Arc::new(CountingScopedWorkflowTool {
        calls: Arc::clone(&calls),
    });
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "scoped-evidence-1",
            "scoped_workflow_evidence",
            serde_json::json!({}),
        ),
        scripted_text_response("scoped workflow evidence reviewed"),
    ]));
    let mut parent_permissions =
        crate::permissions::PermissionPolicy::new().allow("scoped_workflow_evidence(*)");
    parent_permissions.default_decision = crate::permissions::PermissionDecision::Deny;
    let mut worker_permissions =
        crate::permissions::PermissionPolicy::new().allow("scoped_workflow_evidence(*)");
    worker_permissions.default_decision = crate::permissions::PermissionDecision::Deny;
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_permission_policy(parent_permissions)
        .with_worker_agent(
            crate::subagent::WorkerAgentSpec::custom(
                "scoped-evidence-reviewer",
                "Review only explicitly scoped host evidence.",
            )
            .with_permissions(worker_permissions)
            .with_max_steps(2),
        );
    let session = agent
        .session_async("/tmp/test-workflow-scoped-host-tools", Some(opts))
        .await
        .unwrap();

    let parent_before = session
        .tool("scoped_workflow_evidence", serde_json::json!({}))
        .await
        .unwrap();
    assert_ne!(
        parent_before.exit_code, 0,
        "a workflow-scoped tool must not be registered on its parent session"
    );

    let workflow = session.workflow_with_token_budget_and_tools(Some(1_024), vec![tool]);
    let outcome = workflow
        .agent(
            crate::orchestration::AgentStepSpec::new(
                "scoped-evidence-step",
                "scoped-evidence-reviewer",
                "Review scoped evidence",
                "Call the admitted evidence tool exactly once, then report completion.",
            )
            .with_max_steps(2),
        )
        .await;

    assert!(outcome.success, "child step failed: {}", outcome.output);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the explicit workflow child must execute the scoped tool once"
    );
    let parent_after = session
        .tool("scoped_workflow_evidence", serde_json::json!({}))
        .await
        .unwrap();
    assert_ne!(
        parent_after.exit_code, 0,
        "a workflow-scoped tool must not escape after child execution"
    );
}

#[tokio::test]
async fn test_session_workflow_parent_permission_can_deny_scoped_host_tool() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tool: Arc<dyn crate::tools::Tool> = Arc::new(CountingScopedWorkflowTool {
        calls: Arc::clone(&calls),
    });
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "scoped-evidence-denied-1",
            "scoped_workflow_evidence",
            serde_json::json!({}),
        ),
        scripted_text_response("permission boundary observed"),
    ]));
    let mut parent_permissions = crate::permissions::PermissionPolicy::new();
    parent_permissions.default_decision = crate::permissions::PermissionDecision::Deny;
    let mut worker_permissions =
        crate::permissions::PermissionPolicy::new().allow("scoped_workflow_evidence(*)");
    worker_permissions.default_decision = crate::permissions::PermissionDecision::Deny;
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_permission_policy(parent_permissions)
        .with_worker_agent(
            crate::subagent::WorkerAgentSpec::custom(
                "scoped-evidence-reviewer-denied",
                "Review only explicitly scoped host evidence.",
            )
            .with_permissions(worker_permissions)
            .with_max_steps(2),
        );
    let session = agent
        .session_async(
            "/tmp/test-workflow-scoped-host-tool-parent-deny",
            Some(opts),
        )
        .await
        .unwrap();

    let workflow = session.workflow_with_token_budget_and_tools(Some(1_024), vec![tool]);
    let outcome = workflow
        .agent(
            crate::orchestration::AgentStepSpec::new(
                "scoped-evidence-denied-step",
                "scoped-evidence-reviewer-denied",
                "Review scoped evidence",
                "Attempt the admitted evidence tool exactly once, then report completion.",
            )
            .with_max_steps(2),
        )
        .await;

    assert!(
        outcome.success,
        "the child should observe the governed denial and finish: {}",
        outcome.output
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a child allow rule must not override its parent deny boundary"
    );
}

#[tokio::test]
async fn test_session_workflow_inherits_runtime_budget_guard() {
    let runtime_budget = Arc::new(DenyingBudgetGuard::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-workflow-runtime-budget".into(),
            Arc::new(StaticStreamingClient::new("must not reach the provider")),
            &SessionOptions::new(),
        )
        .unwrap();
    session
        .set_budget_guard(Some(
            Arc::clone(&runtime_budget) as Arc<dyn crate::budget::BudgetGuard>
        ))
        .unwrap();

    let outcome = session
        .workflow()
        .agent(crate::orchestration::AgentStepSpec::new(
            "runtime-budget-step",
            "explore",
            "budget check",
            "Return a short answer.",
        ))
        .await;

    assert!(!outcome.success, "runtime budget must deny workflow child");
    assert_eq!(
        runtime_budget
            .checks
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        runtime_budget
            .llm_records
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}
