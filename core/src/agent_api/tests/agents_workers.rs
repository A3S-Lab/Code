use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn test_register_agent_dir_loads_agents_into_live_session() {
    let temp_dir = tempfile::tempdir().unwrap();

    // Write a valid agent file
    std::fs::write(
        temp_dir.path().join("my-agent.yaml"),
        "name: my-dynamic-agent\ndescription: Dynamically registered agent\n",
    )
    .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();

    // The agent must not be known before registration
    assert!(!session.agent_registry.exists("my-dynamic-agent"));

    let count = session.register_agent_dir(temp_dir.path()).unwrap();
    assert_eq!(count, 1);
    assert!(session.agent_registry.exists("my-dynamic-agent"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_agent_dir_empty_dir_returns_zero() {
    let temp_dir = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();
    let count = session.register_agent_dir(temp_dir.path()).unwrap();
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_agent_dir_nonexistent_returns_zero() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();
    let count = session
        .register_agent_dir(std::path::Path::new("/nonexistent/path/abc"))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_worker_agent_loads_worker_into_live_session() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();

    assert!(!session.agent_registry.exists("frontend-cow"));
    let definition = session
        .register_worker_agent(
            crate::subagent::WorkerAgentSpec::implementer(
                "frontend-cow",
                "Disposable frontend implementer",
            )
            .with_max_steps(9),
        )
        .unwrap();

    assert_eq!(definition.max_steps, Some(9));
    assert!(session.agent_registry.exists("frontend-cow"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_worker_agents_loads_batch_into_live_session() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();

    let definitions = session
        .register_worker_agents([
            crate::subagent::WorkerAgentSpec::planner("planner-cow", "Plan work"),
            crate::subagent::WorkerAgentSpec::verifier("verify-cow", "Verify work"),
        ])
        .unwrap();

    assert_eq!(definitions.len(), 2);
    assert!(session.agent_registry.exists("planner-cow"));
    assert!(session.agent_registry.exists("verify-cow"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_live_worker_catalog_is_model_visible_and_tracks_registry_changes() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();

    session
        .register_worker_agent(crate::subagent::WorkerAgentSpec::custom(
            "use",
            "Operate Browser and Office through A3S Use",
        ))
        .unwrap();
    session
        .register_worker_agent(
            crate::subagent::WorkerAgentSpec::custom(
                "hidden-use-helper",
                "Internal application helper",
            )
            .hidden(true),
        )
        .unwrap();

    let definitions = session.tool_definitions();
    let definition = definitions
        .iter()
        .find(|tool| tool.name == "task")
        .expect("unified delegation tool definition");
    assert!(!definitions.iter().any(|tool| tool.name == "parallel_task"));
    let agent_schema =
        &definition.parameters["properties"]["tasks"]["items"]["properties"]["agent"];
    let examples = agent_schema["examples"]
        .as_array()
        .expect("live canonical agent examples");

    assert!(examples.contains(&serde_json::json!("use")));
    assert!(!examples.contains(&serde_json::json!("hidden-use-helper")));
    assert!(agent_schema["description"]
        .as_str()
        .unwrap()
        .contains("use: Operate Browser and Office through A3S Use"));
    assert!(definition
        .description
        .contains("use: Operate Browser and Office through A3S Use"));
    assert!(!definition.description.contains("hidden-use-helper"));

    assert!(session.agent_registry.unregister("use"));
    for definition in session
        .tool_definitions()
        .into_iter()
        .filter(|tool| tool.name == "task")
    {
        assert!(!definition
            .description
            .contains("Operate Browser and Office through A3S Use"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_options_worker_agents_register_for_task_delegation() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_worker_agent(crate::subagent::WorkerAgentSpec::planner(
        "release-planner",
        "Plan releases",
    ));
    let session = agent.session_async(".", Some(opts)).await.unwrap();

    assert!(session.agent_registry.exists("release-planner"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_loads_workspace_a3s_agents() {
    let workspace = tempfile::tempdir().unwrap();
    let agents_dir = workspace.path().join(".a3s").join("agents").join("quality");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(
        agents_dir.join("code-reviewer.md"),
        r#"---
name: code-reviewer
description: Use proactively after code changes to review quality
tools: Read, Grep
---
Review the changed code and return prioritized findings.
"#,
    )
    .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .unwrap();

    let loaded = session
        .agent_registry
        .get("code-reviewer")
        .expect("workspace .a3s/agents agent should load");
    assert!(loaded
        .permissions
        .allow
        .iter()
        .any(|rule| { rule.matches("read", &serde_json::json!({"file_path": "README.md"})) }));
    assert!(loaded.permissions.allow.iter().any(|rule| {
        rule.matches(
            "search",
            &serde_json::json!({"mode": "grep", "query": "TODO"}),
        )
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_keeps_claude_agents_as_compatibility_source() {
    let workspace = tempfile::tempdir().unwrap();
    let agents_dir = workspace.path().join(".claude").join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(
        agents_dir.join("compat-reviewer.md"),
        r#"---
name: compat-reviewer
description: Compatibility agent
tools: Read
---
Review in compatibility mode.
"#,
    )
    .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .unwrap();

    assert!(session.agent_registry.exists("compat-reviewer"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_workspace_a3s_agents_override_claude_compat_agents() {
    let workspace = tempfile::tempdir().unwrap();
    let claude_dir = workspace.path().join(".claude").join("agents");
    let a3s_dir = workspace.path().join(".a3s").join("agents");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::create_dir_all(&a3s_dir).unwrap();
    std::fs::write(
        claude_dir.join("same-agent.md"),
        r#"---
name: same-agent
description: Claude compatibility version
tools: Read
---
Compat prompt.
"#,
    )
    .unwrap();
    std::fs::write(
        a3s_dir.join("same-agent.md"),
        r#"---
name: same-agent
description: A3S native version
tools: Read
---
A3S prompt.
"#,
    )
    .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .unwrap();

    let loaded = session.agent_registry.get("same-agent").unwrap();
    assert_eq!(loaded.description, "A3S native version");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_for_worker_maps_worker_spec_to_session_options() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_for_worker_async(
            ".",
            crate::subagent::WorkerAgentSpec::reviewer("review-cow", "Review changes")
                .with_max_steps(11),
            None,
        )
        .await
        .unwrap();

    assert_eq!(session.config.max_tool_rounds, 11);
    assert!(session.config.prompt_slots.extra.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_with_mcp_manager_builds_ok() {
    use crate::mcp::manager::McpManager;
    let mcp = Arc::new(McpManager::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_mcp(mcp);
    // No servers connected — should build fine with zero MCP tools registered
    let session = agent
        .session_async("/tmp/test-ws-mcp", Some(opts))
        .await
        .unwrap();
    assert!(!session.id().is_empty());
}

#[test]
fn test_session_command_is_available_from_queue_module() {
    // Compile-time check: SessionCommand remains available from its owning module.
    use crate::queue::SessionCommand;
    let _ = std::marker::PhantomData::<Box<dyn SessionCommand>>;
}
