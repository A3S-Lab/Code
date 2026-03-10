/// Example: SubAgent permission fine-grained control
///
/// Demonstrates how to use permissive mode with deny rules to control
/// which tools a subagent can access.
///
/// Run with: cargo run --example permission_control
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use serde_json::json;

fn main() {
    println!("\n{}", "=".repeat(60));
    println!("SubAgent Permission Fine-Grained Control Demo");
    println!("{}\n", "=".repeat(60));

    // Example 1: Permissive mode with wildcard deny
    println!("Example 1: Permissive + Deny mcp__longvt__*");
    println!("{}", "-".repeat(60));

    let policy = PermissionPolicy::permissive().deny("mcp__longvt__*");

    test_tool(&policy, "mcp__longvt__search", json!({}));
    test_tool(&policy, "mcp__longvt__create_memory", json!({}));
    test_tool(&policy, "mcp__pencil__draw", json!({}));
    test_tool(&policy, "read", json!({"file_path": "test.txt"}));
    test_tool(&policy, "bash", json!({"command": "ls"}));

    // Example 2: Multiple deny patterns
    println!("\n\nExample 2: Multiple Deny Patterns");
    println!("{}", "-".repeat(60));

    let policy = PermissionPolicy::permissive()
        .deny("mcp__longvt__*")
        .deny("mcp__dangerous__*")
        .deny("bash");

    test_tool(&policy, "mcp__longvt__search", json!({}));
    test_tool(&policy, "mcp__dangerous__execute", json!({}));
    test_tool(&policy, "bash", json!({"command": "rm -rf /"}));
    test_tool(&policy, "read", json!({"file_path": "test.txt"}));
    test_tool(&policy, "mcp__pencil__draw", json!({}));

    // Example 3: Deny all MCP tools
    println!("\n\nExample 3: Deny All MCP Tools");
    println!("{}", "-".repeat(60));

    let policy = PermissionPolicy::permissive().deny("mcp__*");

    test_tool(&policy, "mcp__longvt__search", json!({}));
    test_tool(&policy, "mcp__pencil__draw", json!({}));
    test_tool(&policy, "mcp__any__tool", json!({}));
    test_tool(&policy, "read", json!({"file_path": "test.txt"}));
    test_tool(&policy, "bash", json!({"command": "ls"}));

    // Example 4: Agent definition permissions
    println!("\n\nExample 4: Agent Definition Permissions");
    println!("{}", "-".repeat(60));
    println!("In agent .md file:");
    println!("---");
    println!("permissions:");
    println!("  allow:");
    println!("    - read");
    println!("    - grep");
    println!("  deny:");
    println!("    - mcp__longvt__*");
    println!("---");
    println!("\nWhen spawned with permissive=true:");
    println!("  - Agent deny rules are STILL enforced");
    println!("  - Other tools are allowed (permissive default)");

    println!("\n{}", "=".repeat(60));
    println!("✅ Demo completed");
    println!("{}\n", "=".repeat(60));
}

fn test_tool(policy: &PermissionPolicy, tool_name: &str, args: serde_json::Value) {
    let decision = policy.check(tool_name, &args);
    let icon = match decision {
        PermissionDecision::Allow => "✅",
        PermissionDecision::Deny => "❌",
        PermissionDecision::Ask => "❓",
    };
    println!("  {} {:30} → {:?}", icon, tool_name, decision);
}
