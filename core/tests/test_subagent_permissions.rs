use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
/// Integration test for fine-grained permission control.
///
/// Tests that explicit allow-by-default policies with deny rules correctly
/// block specific tools while allowing others.
///
/// Run with: cargo test --test test_subagent_permissions -- --nocapture
use a3s_code_core::subagent::{load_agents_from_dir, AgentRegistry};

fn allow_by_default_policy() -> PermissionPolicy {
    PermissionPolicy {
        default_decision: PermissionDecision::Allow,
        ..PermissionPolicy::default()
    }
}

#[tokio::test]
async fn test_allow_by_default_policy_deny_wildcard() {
    // Test 1: PermissionPolicy directly
    let policy = allow_by_default_policy().deny("mcp__longvt__*");

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
async fn test_agent_definition_deny_rules() {
    // Test 2: Agent definition deny rules should be parsed and respected.
    let registry = AgentRegistry::new();

    // Create a temporary agent file for testing (YAML format)
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("test-permission.yaml"),
        r#"
name: test-permission
description: Test agent for permission control
permissions:
  allow:
    - read
    - write
  deny:
    - mcp__longvt__*
    - bash
"#,
    )
    .unwrap();

    // Load test agent from temp file
    let agents = load_agents_from_dir(temp_dir.path());
    for def in agents {
        registry.register(def);
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
        panic!("Failed to load test-permission agent from temp file");
    }
}

#[tokio::test]
async fn test_multiple_wildcard_patterns() {
    // Test 4: Multiple wildcard patterns
    let policy = allow_by_default_policy()
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
    let policy = allow_by_default_policy().deny("mcp__*"); // Deny ALL MCP tools

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
