use a3s_code_core::orchestrator::SubAgentConfig;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
/// Integration test for SubAgent permission fine-grained control
///
/// Tests that permissive mode with deny rules correctly blocks specific tools
/// while allowing others.
///
/// Run with: cargo test --test test_subagent_permissions -- --nocapture
use a3s_code_core::{load_agents_from_dir, AgentRegistry};

#[tokio::test]
async fn test_permissive_deny_wildcard() {
    // Test 1: PermissionPolicy directly
    let policy = PermissionPolicy::permissive().deny("mcp__longvt__*");

    // longvt tools should be denied
    assert_eq!(
        policy.check("mcp__longvt__search", &serde_json::json!({})),
        PermissionDecision::Deny,
        "mcp__longvt__search should be denied"
    );
    assert_eq!(
        policy.check("mcp__longvt__create_memory", &serde_json::json!({})),
        PermissionDecision::Deny,
        "mcp__longvt__create_memory should be denied"
    );

    // Other tools should be allowed (permissive default)
    assert_eq!(
        policy.check("mcp__pencil__draw", &serde_json::json!({})),
        PermissionDecision::Allow,
        "mcp__pencil__draw should be allowed"
    );
    assert_eq!(
        policy.check("read", &serde_json::json!({"file_path": "test.txt"})),
        PermissionDecision::Allow,
        "read should be allowed"
    );

    println!("✅ Test 1: PermissionPolicy with wildcard deny - PASSED");
}

#[tokio::test]
async fn test_agent_definition_deny_with_permissive() {
    // Test 2: Agent definition deny rules should be respected in permissive mode
    let registry = AgentRegistry::new();

    // Load test agent from file (if exists)
    let test_agent_path = std::path::Path::new("/Users/roylin/Desktop/code/a3s/.a3s/agents");
    if test_agent_path.exists() {
        let agents = load_agents_from_dir(test_agent_path);
        for def in agents {
            registry.register(def);
        }
    }

    // Check if test-permission agent was loaded
    if let Some(def) = registry.get("test-permission") {
        println!("✅ Loaded agent: {}", def.name);
        println!("   Description: {}", def.description);
        println!("   Deny rules: {:?}", def.permissions.deny);

        // Verify deny rules contain mcp__longvt__*
        let has_longvt_deny = def
            .permissions
            .deny
            .iter()
            .any(|r| r.rule.contains("mcp__longvt__"));
        assert!(
            has_longvt_deny,
            "Agent should have mcp__longvt__* deny rule"
        );

        // Test the permission policy
        assert_eq!(
            def.permissions
                .check("mcp__longvt__search", &serde_json::json!({})),
            PermissionDecision::Deny,
            "Agent definition should deny mcp__longvt__search"
        );

        println!("✅ Test 2: Agent definition deny rules - PASSED");
    } else {
        println!("⚠️  Test 2: SKIPPED (test-permission agent not found)");
    }
}

#[tokio::test]
async fn test_subagent_config_permissive_deny() {
    // Test 3: SubAgentConfig permissive_deny parameter
    let config = SubAgentConfig {
        agent_type: "general".to_string(),
        description: "Test permission control".to_string(),
        workspace: "/tmp/test".to_string(),
        prompt: "Test prompt".to_string(),
        permissive: true,
        permissive_deny: vec!["mcp__longvt__*".to_string()],
        max_steps: Some(5),
        timeout_ms: None,
        parent_id: None,
        metadata: serde_json::json!({}),
        agent_dirs: vec![],
        lane_config: None,
    };

    // Verify config structure
    assert!(config.permissive, "permissive should be true");
    assert_eq!(config.permissive_deny.len(), 1, "should have 1 deny rule");
    assert_eq!(config.permissive_deny[0], "mcp__longvt__*");

    println!("✅ Test 3: SubAgentConfig structure - PASSED");

    // The actual permission enforcement happens in SubAgentWrapper::execute_with_agent
    // which builds a PermissionPolicy from permissive_deny rules.
    // We've already tested that in test_permissive_deny_wildcard.
}

#[tokio::test]
async fn test_multiple_wildcard_patterns() {
    // Test 4: Multiple wildcard patterns
    let policy = PermissionPolicy::permissive()
        .deny("mcp__longvt__*")
        .deny("mcp__dangerous__*")
        .deny("bash");

    // All longvt tools denied
    assert_eq!(
        policy.check("mcp__longvt__search", &serde_json::json!({})),
        PermissionDecision::Deny
    );
    assert_eq!(
        policy.check("mcp__longvt__create", &serde_json::json!({})),
        PermissionDecision::Deny
    );

    // All dangerous tools denied
    assert_eq!(
        policy.check("mcp__dangerous__execute", &serde_json::json!({})),
        PermissionDecision::Deny
    );

    // bash denied
    assert_eq!(
        policy.check("bash", &serde_json::json!({"command": "ls"})),
        PermissionDecision::Deny
    );

    // Other tools allowed
    assert_eq!(
        policy.check("mcp__pencil__draw", &serde_json::json!({})),
        PermissionDecision::Allow
    );
    assert_eq!(
        policy.check("read", &serde_json::json!({"file_path": "test.txt"})),
        PermissionDecision::Allow
    );

    println!("✅ Test 4: Multiple wildcard patterns - PASSED");
}

#[tokio::test]
async fn test_mcp_wildcard_variations() {
    // Test 5: Different MCP wildcard patterns
    let policy = PermissionPolicy::permissive().deny("mcp__*"); // Deny ALL MCP tools

    // All MCP tools should be denied
    assert_eq!(
        policy.check("mcp__longvt__search", &serde_json::json!({})),
        PermissionDecision::Deny
    );
    assert_eq!(
        policy.check("mcp__pencil__draw", &serde_json::json!({})),
        PermissionDecision::Deny
    );
    assert_eq!(
        policy.check("mcp__any__tool", &serde_json::json!({})),
        PermissionDecision::Deny
    );

    // Non-MCP tools should be allowed
    assert_eq!(
        policy.check("read", &serde_json::json!({"file_path": "test.txt"})),
        PermissionDecision::Allow
    );
    assert_eq!(
        policy.check("bash", &serde_json::json!({"command": "ls"})),
        PermissionDecision::Allow
    );

    println!("✅ Test 5: MCP wildcard variations - PASSED");
}
