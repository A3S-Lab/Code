use super::*;

#[test]
fn test_agent_definition_builder() {
    let agent = AgentDefinition::new("test", "Test agent")
        .native()
        .hidden()
        .with_max_steps(10);

    assert_eq!(agent.name, "test");
    assert_eq!(agent.description, "Test agent");
    assert!(agent.native);
    assert!(agent.hidden);
    assert_eq!(agent.max_steps, Some(10));
}

#[test]
fn test_agent_registry_new() {
    let registry = AgentRegistry::new();

    // Should have built-in agents
    assert!(registry.exists("explore"));
    assert!(registry.exists("general"));
    assert!(registry.exists("plan"));
    assert!(registry.exists("verification"));
    assert!(registry.exists("review"));
    assert!(registry.exists("general-purpose"));
    assert!(registry.exists("deepresearch"));
    assert!(registry.exists("loop-planner"));
    assert!(registry.exists("loop-checker"));
    assert_eq!(registry.len(), 8);
}

#[test]
fn test_agent_registry_get() {
    let registry = AgentRegistry::new();

    let explore = registry.get("explore").unwrap();
    assert_eq!(explore.name, "explore");
    assert!(explore.native);
    assert!(!explore.hidden);

    let general = registry.get("general-purpose").unwrap();
    assert_eq!(general.name, "general");

    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn test_agent_registry_register_unregister() {
    let registry = AgentRegistry::new();
    let initial_count = registry.len();

    // Register custom agent
    let custom = AgentDefinition::new("custom", "Custom agent");
    registry.register(custom);
    assert_eq!(registry.len(), initial_count + 1);
    assert!(registry.exists("custom"));

    // Unregister
    assert!(registry.unregister("custom"));
    assert_eq!(registry.len(), initial_count);
    assert!(!registry.exists("custom"));

    // Unregister non-existent
    assert!(!registry.unregister("nonexistent"));
}

#[test]
fn test_agent_registry_list_visible() {
    let registry = AgentRegistry::new();

    let visible = registry.list_visible();
    let all = registry.list();

    assert!(visible.len() < all.len());
    assert!(visible.iter().all(|a| !a.hidden));
    assert!(!visible.iter().any(|a| a.name == "deep-research"));
    assert!(!visible.iter().any(|a| a.name == "loop-planner"));
    assert!(!visible.iter().any(|a| a.name == "loop-checker"));
}

#[test]
fn test_builtin_agents() {
    let agents = builtin_agents();

    // Check we have expected agents
    let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"explore"));
    assert!(names.contains(&"general"));
    assert!(names.contains(&"deep-research"));
    assert!(names.contains(&"loop-planner"));
    assert!(names.contains(&"loop-checker"));
    assert!(names.contains(&"plan"));
    assert!(names.contains(&"verification"));
    assert!(names.contains(&"review"));

    // Check explore is read-only (has deny rules for write)
    let explore = agents.iter().find(|a| a.name == "explore").unwrap();
    assert!(!explore.permissions.deny.is_empty());
    let deep_research = agents.iter().find(|a| a.name == "deep-research").unwrap();
    assert!(deep_research.hidden);
    assert!(deep_research.permissions.allow.is_empty());
    assert!(deep_research.permissions.deny.is_empty());
    assert_eq!(
        deep_research.confirmation_inheritance,
        Some(ConfirmationInheritance::InheritParent)
    );
    assert!(agents
        .iter()
        .filter(|agent| matches!(agent.name.as_str(), "loop-planner" | "loop-checker"))
        .all(|agent| agent.hidden && agent.tool_free));
}

// ========================================================================
// Agent File Loading Tests
// ========================================================================

#[test]
fn test_parse_agent_yaml() {
    let yaml = r#"
name: test-agent
description: A test agent
hidden: false
max_steps: 20
"#;
    let agent = parse_agent_yaml(yaml).unwrap();
    assert_eq!(agent.name, "test-agent");
    assert_eq!(agent.description, "A test agent");
    assert!(!agent.hidden);
    assert_eq!(agent.max_steps, Some(20));
}

#[test]
fn test_parse_agent_yaml_with_permissions() {
    let yaml = r#"
name: restricted-agent
description: Agent with permissions
permissions:
  allow:
    - rule: read
    - rule: grep
  deny:
    - rule: write
"#;
    let agent = parse_agent_yaml(yaml).unwrap();
    assert_eq!(agent.name, "restricted-agent");
    assert_eq!(agent.permissions.allow.len(), 2);
    assert_eq!(agent.permissions.deny.len(), 1);
    // Verify that deserialized rules actually match (tool_name populated)
    assert!(agent.permissions.allow[0].matches("read", &serde_json::json!({})));
    assert!(agent.permissions.allow[1].matches("grep", &serde_json::json!({})));
    assert!(agent.permissions.deny[0].matches("write", &serde_json::json!({})));
}

#[test]
fn test_parse_agent_yaml_with_plain_string_permissions() {
    // Users naturally write plain strings in allow/deny lists
    let yaml = r#"
name: plain-agent
description: Agent with plain string permissions
permissions:
  allow:
    - read
    - grep
    - "Bash(cargo:*)"
  deny:
    - write
"#;
    let agent = parse_agent_yaml(yaml).unwrap();
    assert_eq!(agent.name, "plain-agent");
    assert_eq!(agent.permissions.allow.len(), 3);
    assert_eq!(agent.permissions.deny.len(), 1);
    // Verify rules are functional
    assert!(agent.permissions.allow[0].matches("read", &serde_json::json!({})));
    assert!(agent.permissions.allow[1].matches("grep", &serde_json::json!({})));
    assert!(
        agent.permissions.allow[2].matches("Bash", &serde_json::json!({"command": "cargo build"}))
    );
    assert!(agent.permissions.deny[0].matches("write", &serde_json::json!({})));
}

#[test]
fn test_parse_claude_style_agent_md_tools_field() {
    let md = r#"---
name: code-reviewer
description: Use proactively after code changes to review quality
tools: Read, Grep, Glob, Bash
---
Review the changed code and return prioritized findings.
"#;
    let agent = parse_agent_md(md).unwrap();

    assert_eq!(agent.name, "code-reviewer");
    assert_eq!(
        agent.confirmation_inheritance,
        Some(ConfirmationInheritance::AutoApprove)
    );
    assert!(agent
        .permissions
        .allow
        .iter()
        .any(|r| r.matches("read", &serde_json::json!({}))));
    assert!(agent
        .permissions
        .allow
        .iter()
        .any(|r| r.matches("grep", &serde_json::json!({}))));
    assert!(agent
        .permissions
        .allow
        .iter()
        .any(|r| r.matches("bash", &serde_json::json!({}))));
    assert_eq!(
        agent
            .permissions
            .check("write", &serde_json::json!({"file_path": "src/lib.rs"})),
        PermissionDecision::Deny
    );
    assert!(agent
        .prompt
        .as_deref()
        .unwrap_or_default()
        .contains("prioritized findings"));
}

#[test]
fn test_parse_claude_style_agent_md_disallowed_tools_field() {
    let md = r#"---
name: shell-checker
description: Use proactively to run safe shell checks
tools:
  - Read
  - Bash
disallowedTools:
  - Bash(rm:*)
  - Write
---
Run safe checks only.
"#;
    let agent = parse_agent_md(md).unwrap();

    assert_eq!(agent.name, "shell-checker");
    assert_eq!(
        agent
            .permissions
            .check("bash", &serde_json::json!({"command": "rm -rf target"})),
        PermissionDecision::Deny
    );
    assert_eq!(
        agent
            .permissions
            .check("bash", &serde_json::json!({"command": "cargo test"})),
        PermissionDecision::Allow
    );
    assert_eq!(
        agent
            .permissions
            .check("write", &serde_json::json!({"file_path": "x"})),
        PermissionDecision::Deny
    );
}

#[test]
fn test_parse_worker_agent_md_supports_claude_tools_fields() {
    let md = r#"---
name: planner-worker
description: Plan work
kind: planner
tools: Read, Grep
disallowedTools: Grep(secret:*)
---
Plan without editing.
"#;
    let agent = parse_agent_md(md).unwrap();

    assert_eq!(agent.name, "planner-worker");
    assert_eq!(
        agent
            .permissions
            .check("read", &serde_json::json!({"file_path": "src/lib.rs"})),
        PermissionDecision::Allow
    );
    assert_eq!(
        agent.permissions.check(
            "grep",
            &serde_json::json!({"pattern": "secret", "path": "src"})
        ),
        PermissionDecision::Deny
    );
    assert_eq!(
        agent
            .permissions
            .check("bash", &serde_json::json!({"command": "echo no"})),
        PermissionDecision::Deny
    );
}

#[test]
fn test_builtin_agent_permissions_are_bounded() {
    let registry = AgentRegistry::new();
    let explore = registry.get("explore").unwrap();
    let general = registry.get("general-purpose").unwrap();
    let deep_research = registry.get("deepresearch").unwrap();
    let verification = registry.get("verification").unwrap();
    let review = registry.get("review").unwrap();

    assert_eq!(
        explore
            .permissions
            .check("bash", &serde_json::json!({"command": "cargo test"})),
        PermissionDecision::Deny
    );
    assert_eq!(
        explore
            .permissions
            .check("bash", &serde_json::json!({"command": "ls src"})),
        PermissionDecision::Allow
    );
    assert!(
        !deep_research.has_defined_permissions(),
        "DeepResearch child agent should inherit parent session permissions"
    );
    assert_eq!(
        explore
            .permissions
            .check("web_search", &serde_json::json!({"query": "a3s"})),
        PermissionDecision::Allow
    );
    assert_eq!(
        explore.permissions.check(
            "web_fetch",
            &serde_json::json!({"url": "https://example.com"})
        ),
        PermissionDecision::Allow
    );
    assert_eq!(
        explore
            .permissions
            .check("write", &serde_json::json!({"file_path": "x"})),
        PermissionDecision::Deny
    );
    assert_eq!(
        general
            .permissions
            .check("parallel_task", &serde_json::json!({})),
        PermissionDecision::Deny
    );
    for agent in [verification, review] {
        assert_eq!(
            agent
                .permissions
                .check("bash", &serde_json::json!({"command": "cargo test"})),
            PermissionDecision::Allow,
            "{} should allow runtime checks",
            agent.name
        );
        assert_eq!(
            agent
                .permissions
                .check("web_search", &serde_json::json!({"query": "a3s"})),
            PermissionDecision::Allow,
            "{} should allow evidence search",
            agent.name
        );
        assert_eq!(
            agent.permissions.check(
                "web_fetch",
                &serde_json::json!({"url": "https://example.com"})
            ),
            PermissionDecision::Allow,
            "{} should allow source fetches",
            agent.name
        );
        assert_eq!(
            agent
                .permissions
                .check("write", &serde_json::json!({"file_path": "x"})),
            PermissionDecision::Deny,
            "{} should stay read-only for workspace writes",
            agent.name
        );
        assert_eq!(
            agent
                .permissions
                .check("parallel_task", &serde_json::json!({})),
            PermissionDecision::Deny,
            "{} should not recurse into more subagents",
            agent.name
        );
    }
}

#[test]
fn test_parse_worker_agent_yaml_uses_cattle_defaults() {
    let yaml = r#"
name: frontend-fixer
description: Disposable frontend implementer
kind: implementer
max_steps: 7
"#;
    let agent = parse_agent_yaml(yaml).unwrap();

    assert_eq!(agent.name, "frontend-fixer");
    assert_eq!(agent.max_steps, Some(7));
    assert!(agent
        .permissions
        .allow
        .iter()
        .any(|r| r.matches("write", &serde_json::json!({}))));
    assert!(agent
        .permissions
        .deny
        .iter()
        .any(|r| r.matches("task", &serde_json::json!({}))));
}

#[test]
fn test_parse_agent_yaml_missing_name() {
    let yaml = r#"
description: Agent without name
"#;
    let result = parse_agent_yaml(yaml);
    assert!(result.is_err());
}

#[test]
fn test_parse_agent_md() {
    let md = r#"---
name: md-agent
description: Agent from markdown
max_steps: 15
---
# System Prompt

You are a helpful agent.
Do your best work.
"#;
    let agent = parse_agent_md(md).unwrap();
    assert_eq!(agent.name, "md-agent");
    assert_eq!(agent.description, "Agent from markdown");
    assert_eq!(agent.max_steps, Some(15));
    assert!(agent.prompt.is_some());
    assert!(agent.prompt.unwrap().contains("helpful agent"));
}

#[test]
fn test_parse_agent_md_with_prompt_in_frontmatter() {
    let md = r#"---
name: prompt-agent
description: Agent with prompt in frontmatter
prompt: "Frontmatter prompt"
---
Body content that should be ignored
"#;
    let agent = parse_agent_md(md).unwrap();
    assert_eq!(agent.prompt.unwrap(), "Frontmatter prompt");
}

#[test]
fn test_parse_worker_agent_md_uses_body_prompt() {
    let md = r#"---
name: review-cow
description: Disposable review worker
kind: reviewer
---
Review only the staged diff and return prioritized findings.
"#;
    let agent = parse_agent_md(md).unwrap();

    assert_eq!(agent.name, "review-cow");
    assert_eq!(
        agent.prompt.as_deref(),
        Some("Review only the staged diff and return prioritized findings.")
    );
    assert!(agent
        .permissions
        .deny
        .iter()
        .any(|r| r.matches("write", &serde_json::json!({}))));
}

#[test]
fn test_parse_agent_md_missing_frontmatter() {
    let md = "Just markdown without frontmatter";
    let result = parse_agent_md(md);
    assert!(result.is_err());
}

#[test]
fn test_load_agents_from_dir() {
    let temp_dir = tempfile::tempdir().unwrap();

    // Create a YAML agent file
    std::fs::write(
        temp_dir.path().join("agent1.yaml"),
        r#"
name: yaml-agent
description: Agent from YAML file
"#,
    )
    .unwrap();

    // Create a Markdown agent file
    std::fs::write(
        temp_dir.path().join("agent2.md"),
        r#"---
name: md-agent
description: Agent from Markdown file
---
System prompt here
"#,
    )
    .unwrap();

    // Create an invalid file (should be skipped)
    std::fs::write(temp_dir.path().join("invalid.yaml"), "not: valid: yaml: [").unwrap();

    // Create a nested agent file (Claude-style directories are recursive)
    std::fs::create_dir_all(temp_dir.path().join("nested")).unwrap();
    std::fs::write(
        temp_dir.path().join("nested").join("agent3.md"),
        r#"---
name: nested-agent
description: Agent from nested Markdown file
---
Nested prompt
"#,
    )
    .unwrap();

    // Create a non-agent file (should be skipped)
    std::fs::write(temp_dir.path().join("readme.txt"), "Just a text file").unwrap();

    let agents = load_agents_from_dir(temp_dir.path());
    assert_eq!(agents.len(), 3);

    let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"yaml-agent"));
    assert!(names.contains(&"md-agent"));
    assert!(names.contains(&"nested-agent"));
}

#[test]
fn test_load_agents_from_nonexistent_dir() {
    let agents = load_agents_from_dir(std::path::Path::new("/nonexistent/dir"));
    assert!(agents.is_empty());
}

#[test]
fn test_registry_with_config() {
    let temp_dir = tempfile::tempdir().unwrap();

    // Create an agent file
    std::fs::write(
        temp_dir.path().join("custom.yaml"),
        r#"
name: custom-agent
description: Custom agent from config
"#,
    )
    .unwrap();

    let config = CodeConfig::new().add_agent_dir(temp_dir.path());
    let registry = AgentRegistry::with_config(&config);

    // Should have built-in agents plus custom agent
    assert!(registry.exists("explore"));
    assert!(registry.exists("custom-agent"));
    assert_eq!(registry.len(), 9); // 8 built-in + 1 custom
}

#[test]
fn test_agent_definition_with_model() {
    let model = ModelConfig {
        model: "claude-3-5-sonnet".to_string(),
        provider: Some("anthropic".to_string()),
    };
    let agent = AgentDefinition::new("test", "Test").with_model(model);
    assert!(agent.model.is_some());
    assert_eq!(agent.model.unwrap().provider, Some("anthropic".to_string()));
}

#[test]
fn test_model_config_from_model_ref() {
    let model = ModelConfig::from_model_ref("openai/gpt-4o");
    assert_eq!(model.provider.as_deref(), Some("openai"));
    assert_eq!(model.model, "gpt-4o");
    assert_eq!(model.model_ref(), "openai/gpt-4o");

    let inherited = ModelConfig::from_model_ref("claude-sonnet");
    assert_eq!(inherited.provider, None);
    assert_eq!(inherited.model_ref(), "claude-sonnet");
}

#[test]
fn test_worker_agent_kind_from_str_accepts_aliases() {
    assert_eq!(
        "explore".parse::<WorkerAgentKind>().unwrap(),
        WorkerAgentKind::ReadOnly
    );
    assert_eq!(
        "general".parse::<WorkerAgentKind>().unwrap(),
        WorkerAgentKind::Implementer
    );
    assert!("unknown".parse::<WorkerAgentKind>().is_err());
}

#[test]
fn worker_spec_implementer_creates_cattle_agent_definition() {
    let agent = WorkerAgentSpec::implementer("frontend-fixer", "Fix frontend issues")
        .with_prompt("Focus on small, verified patches.")
        .with_provider_model("anthropic", "claude-sonnet")
        .with_max_steps(12)
        .into_agent_definition();

    assert_eq!(agent.name, "frontend-fixer");
    assert_eq!(agent.max_steps, Some(12));
    assert_eq!(
        agent.prompt.as_deref(),
        Some("Focus on small, verified patches.")
    );
    assert_eq!(agent.model.unwrap().provider.as_deref(), Some("anthropic"));
    assert!(agent
        .permissions
        .allow
        .iter()
        .any(|r| r.matches("write", &serde_json::json!({}))));
    assert!(agent
        .permissions
        .deny
        .iter()
        .any(|r| r.matches("task", &serde_json::json!({}))));
}

#[test]
fn worker_spec_read_only_uses_safe_defaults() {
    let agent = WorkerAgentSpec::read_only("scanner", "Scan repository")
        .hidden(true)
        .into_agent_definition();

    assert!(agent.hidden);
    assert_eq!(agent.max_steps, Some(20));
    assert!(agent.prompt.is_some());
    assert!(agent
        .permissions
        .allow
        .iter()
        .any(|r| r.matches("read", &serde_json::json!({}))));
    assert!(agent
        .permissions
        .deny
        .iter()
        .any(|r| r.matches("write", &serde_json::json!({}))));
}

#[test]
fn registry_register_worker_returns_and_stores_definition() {
    let registry = AgentRegistry::new();
    let agent = registry.register_worker(WorkerAgentSpec::custom("strict-worker", "Strict worker"));

    assert_eq!(agent.name, "strict-worker");
    assert!(registry.exists("strict-worker"));
    assert_eq!(
        agent
            .permissions
            .check("bash", &serde_json::json!({"command":"echo hi"})),
        crate::permissions::PermissionDecision::Ask
    );
}

#[test]
fn registry_register_workers_batches_cattle_specs() {
    let registry = AgentRegistry::new();
    let agents = registry.register_workers([
        WorkerAgentSpec::planner("planner-cow", "Plan work"),
        WorkerAgentSpec::verifier("verify-cow", "Verify work"),
    ]);

    assert_eq!(agents.len(), 2);
    assert!(registry.exists("planner-cow"));
    assert!(registry.exists("verify-cow"));
}

#[test]
fn test_agent_registry_default() {
    let registry = AgentRegistry::default();
    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 8);
}

#[test]
fn test_agent_registry_is_empty() {
    let registry = AgentRegistry {
        agents: RwLock::new(HashMap::new()),
    };
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

#[test]
fn test_apply_to_sets_permissions() {
    use crate::agent::AgentConfig;
    use crate::permissions::PermissionDecision;

    let def = AgentDefinition::new("writer", "Write files")
        .with_permissions(PermissionPolicy::new().allow("write(*)"));

    let mut config = AgentConfig::default();
    assert!(config.permission_checker.is_none());

    def.apply_to(&mut config);

    assert!(config.permission_checker.is_some());
    assert!(config.permission_policy.is_some());
    let checker = config.permission_checker.unwrap();
    assert_eq!(
        checker.check(
            "write",
            &serde_json::json!({"file_path": "x.txt", "content": "hi"})
        ),
        PermissionDecision::Allow
    );
}

#[test]
fn test_apply_to_sets_prompt() {
    use crate::agent::AgentConfig;

    let def = AgentDefinition::new("helper", "Help").with_prompt("Be helpful.");
    let mut config = AgentConfig::default();

    def.apply_to(&mut config);

    assert_eq!(config.prompt_slots.extra.as_deref(), Some("Be helpful."));
}

#[test]
fn test_apply_to_sets_max_steps() {
    use crate::agent::AgentConfig;

    let def = AgentDefinition::new("fast", "Fast agent").with_max_steps(7);
    let mut config = AgentConfig::default();

    def.apply_to(&mut config);

    assert_eq!(config.max_tool_rounds, 7);
}

#[test]
fn test_apply_to_tool_free_role_removes_model_tools_and_parent_permissions() {
    use crate::agent::AgentConfig;
    use crate::llm::ToolDefinition;

    let def = AgentDefinition::new("decision", "Pure decision role").tool_free();
    let mut config = AgentConfig::default();
    config.tools.push(ToolDefinition {
        name: "web_search".to_string(),
        description: "Search the web".to_string(),
        parameters: serde_json::json!({ "type": "object" }),
    });

    def.apply_to(&mut config);

    assert!(config.tools.is_empty());
    assert!(config.permission_checker.is_some());
    assert!(config.permission_policy.is_some());
}

#[test]
fn test_apply_to_respects_host_overrides() {
    use crate::agent::AgentConfig;

    let def = AgentDefinition::new("agent", "Agent")
        .with_permissions(PermissionPolicy::new().allow("bash(*)"))
        .with_prompt("Agent prompt.")
        .with_max_steps(10);

    let mut config = AgentConfig::default();
    config.prompt_slots.extra = Some("Host prompt.".to_string());
    config.max_tool_rounds = 25;
    config.permission_checker = Some(std::sync::Arc::new(PermissionPolicy::new().allow("*")));

    def.apply_to(&mut config);

    // Host overrides should be preserved
    assert_eq!(config.prompt_slots.extra.as_deref(), Some("Host prompt."));
    assert_eq!(config.max_tool_rounds, 25);
}

#[test]
fn test_apply_to_skips_empty_permissions() {
    use crate::agent::AgentConfig;

    let def = AgentDefinition::new("empty", "No permissions");
    let mut config = AgentConfig::default();

    def.apply_to(&mut config);

    assert!(config.permission_checker.is_none());
    assert!(config.permission_policy.is_none());
}
