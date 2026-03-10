use a3s_code_core::orchestrator::SubAgentConfig;
/// Scenario 1: Test SubAgentConfig permissive_deny parameter
///
/// This test verifies that the permissive_deny parameter correctly blocks
/// specific tools (like mcp__longvt__*) while allowing others in permissive mode.
///
/// Run with: cargo run --example test_scenario1
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use serde_json::json;

fn main() {
    println!("\n{}", "=".repeat(70));
    println!("Scenario 1: SubAgentConfig permissive_deny Parameter Test");
    println!("{}\n", "=".repeat(70));

    // Step 1: Create SubAgentConfig with permissive_deny
    println!("Step 1: Create SubAgentConfig");
    println!("{}", "-".repeat(70));

    let config = SubAgentConfig {
        agent_type: "general".to_string(),
        description: "Test permission control".to_string(),
        workspace: ".".to_string(),
        prompt: "Test prompt".to_string(),
        permissive: true,
        permissive_deny: vec!["mcp__longvt__*".to_string(), "bash".to_string()],
        max_steps: Some(10),
        timeout_ms: None,
        parent_id: None,
        metadata: json!({}),
        agent_dirs: vec![],
        lane_config: None,
    };

    println!("  Config created:");
    println!("    - agent_type: {}", config.agent_type);
    println!("    - permissive: {}", config.permissive);
    println!("    - permissive_deny: {:?}", config.permissive_deny);
    println!();

    // Step 2: Build PermissionPolicy from config (simulating SubAgentWrapper logic)
    println!("Step 2: Build PermissionPolicy from config");
    println!("{}", "-".repeat(70));

    let mut policy = PermissionPolicy::permissive();
    for rule in &config.permissive_deny {
        policy = policy.deny(rule);
    }

    println!("  Policy built:");
    println!("    - default_decision: Allow (permissive mode)");
    println!("    - deny rules: {:?}", config.permissive_deny);
    println!();

    // Step 3: Test tool permissions
    println!("Step 3: Test Tool Permissions");
    println!("{}", "-".repeat(70));

    let test_cases = vec![
        // MCP longvt tools - should be DENIED
        (
            "mcp__longvt__search",
            json!({}),
            PermissionDecision::Deny,
            "❌",
        ),
        (
            "mcp__longvt__create_memory",
            json!({}),
            PermissionDecision::Deny,
            "❌",
        ),
        (
            "mcp__longvt__delete",
            json!({}),
            PermissionDecision::Deny,
            "❌",
        ),
        // bash - should be DENIED
        (
            "bash",
            json!({"command": "ls"}),
            PermissionDecision::Deny,
            "❌",
        ),
        (
            "bash",
            json!({"command": "rm -rf /"}),
            PermissionDecision::Deny,
            "❌",
        ),
        // Other MCP tools - should be ALLOWED
        (
            "mcp__pencil__draw",
            json!({}),
            PermissionDecision::Allow,
            "✅",
        ),
        (
            "mcp__other__tool",
            json!({}),
            PermissionDecision::Allow,
            "✅",
        ),
        // Built-in tools - should be ALLOWED
        (
            "read",
            json!({"file_path": "test.txt"}),
            PermissionDecision::Allow,
            "✅",
        ),
        (
            "write",
            json!({"file_path": "test.txt"}),
            PermissionDecision::Allow,
            "✅",
        ),
        (
            "grep",
            json!({"pattern": "test"}),
            PermissionDecision::Allow,
            "✅",
        ),
    ];

    let mut passed = 0;
    let mut failed = 0;

    for (tool_name, args, expected, icon) in test_cases {
        let decision = policy.check(tool_name, &args);
        let status = if decision == expected {
            passed += 1;
            "PASS"
        } else {
            failed += 1;
            "FAIL"
        };

        println!("  {} {:35} → {:?} [{}]", icon, tool_name, decision, status);
    }

    println!();

    // Step 4: Summary
    println!("Step 4: Test Summary");
    println!("{}", "-".repeat(70));
    println!("  Total tests: {}", passed + failed);
    println!("  Passed: {} ✅", passed);
    println!("  Failed: {} ❌", failed);
    println!();

    if failed == 0 {
        println!("{}", "=".repeat(70));
        println!("✅ Scenario 1 Test: PASSED");
        println!("{}\n", "=".repeat(70));
        std::process::exit(0);
    } else {
        println!("{}", "=".repeat(70));
        println!("❌ Scenario 1 Test: FAILED");
        println!("{}\n", "=".repeat(70));
        std::process::exit(1);
    }
}
