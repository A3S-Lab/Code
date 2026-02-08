//! Permission Policy Example
//!
//! Demonstrates the declarative permission system for controlling tool execution.
//! Shows how to create policies with allow/deny/ask rules and pattern matching.
//!
//! Run with:
//!   cargo run --example permission_policy

use a3s_box_code::permissions::{PermissionDecision, PermissionPolicy, PermissionRule};
use serde_json::json;

fn main() {
    println!("╔══════════════════════════════════════════════════╗");
    println!("║       A3S Code - Permission Policy Demo          ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    // =========================================================================
    // 1. Default policy (permissive)
    // =========================================================================
    println!("━━━ 1. Default Policy ━━━");
    let default_policy = PermissionPolicy::default();
    println!("  Enabled: {}", default_policy.enabled);
    println!("  Default: {:?}", default_policy.default_decision);
    println!();

    // =========================================================================
    // 2. Development policy - allow common dev tools, deny dangerous commands
    // =========================================================================
    println!("━━━ 2. Development Policy ━━━");
    let dev_policy = PermissionPolicy::new()
        // Allow safe read operations
        .allow("Read(*)")
        .allow("Grep(*)")
        .allow("Glob(*)")
        .allow("ls(*)")
        // Allow common dev commands
        .allow("Bash(cargo:*)")
        .allow("Bash(npm:*)")
        .allow("Bash(git:*)")
        .allow("Bash(rustc:*)")
        // Deny dangerous operations
        .deny("Bash(rm -rf:*)")
        .deny("Bash(sudo:*)")
        .deny("Bash(chmod 777:*)")
        // Ask for file modifications
        .ask("Write(*)")
        .ask("Edit(*)");

    println!("  Rules:");
    println!("    Allow: {} rules", dev_policy.allow.len());
    println!("    Deny:  {} rules", dev_policy.deny.len());
    println!("    Ask:   {} rules", dev_policy.ask.len());
    println!();

    // Test various tool invocations against the policy
    let test_cases = vec![
        // (tool_name, args, expected_description)
        ("Read", json!({"file_path": "src/main.rs"}), "Read any file"),
        (
            "Grep",
            json!({"pattern": "TODO", "path": "src/"}),
            "Grep search",
        ),
        ("Bash", json!({"command": "cargo build"}), "cargo build"),
        ("Bash", json!({"command": "cargo test --lib"}), "cargo test"),
        ("Bash", json!({"command": "git status"}), "git status"),
        ("Bash", json!({"command": "npm install"}), "npm install"),
        (
            "Bash",
            json!({"command": "rm -rf /"}),
            "rm -rf (dangerous!)",
        ),
        (
            "Bash",
            json!({"command": "sudo apt install"}),
            "sudo (elevated)",
        ),
        (
            "Write",
            json!({"file_path": "src/lib.rs", "content": "..."}),
            "Write file",
        ),
        (
            "Edit",
            json!({"file_path": "src/lib.rs", "old_string": "a", "new_string": "b"}),
            "Edit file",
        ),
        (
            "Bash",
            json!({"command": "python3 script.py"}),
            "python3 (no rule)",
        ),
    ];

    println!("  Permission Checks:");
    for (tool, args, desc) in &test_cases {
        let decision = dev_policy.check(tool, args);
        let icon = match decision {
            PermissionDecision::Allow => "✅",
            PermissionDecision::Deny => "🚫",
            PermissionDecision::Ask => "❓",
        };
        println!("    {} {:?} ← {} ({})", icon, decision, desc, tool);
    }
    println!();

    // =========================================================================
    // 3. Strict policy - deny by default, whitelist only
    // =========================================================================
    println!("━━━ 3. Strict Policy (Whitelist Only) ━━━");
    let strict_policy = PermissionPolicy::strict()
        .allow("Read(src/**/*.rs)")
        .allow("Grep(src/**)")
        .allow("Bash(cargo test:*)");

    let strict_tests = vec![
        (
            "Read",
            json!({"file_path": "src/main.rs"}),
            "Read Rust source",
        ),
        (
            "Read",
            json!({"file_path": "/etc/passwd"}),
            "Read system file",
        ),
        ("Bash", json!({"command": "cargo test"}), "cargo test"),
        ("Bash", json!({"command": "cargo build"}), "cargo build"),
        (
            "Write",
            json!({"file_path": "src/lib.rs", "content": "..."}),
            "Write file",
        ),
    ];

    println!("  Permission Checks:");
    for (tool, args, desc) in &strict_tests {
        let decision = strict_policy.check(tool, args);
        let icon = match decision {
            PermissionDecision::Allow => "✅",
            PermissionDecision::Deny => "🚫",
            PermissionDecision::Ask => "❓",
        };
        println!("    {} {:?} ← {}", icon, decision, desc);
    }
    println!();

    // =========================================================================
    // 4. MCP tool permissions
    // =========================================================================
    println!("━━━ 4. MCP Tool Permissions ━━━");
    let mcp_policy = PermissionPolicy::new()
        .allow("mcp__github") // Allow all GitHub MCP tools
        .deny("mcp__filesystem") // Deny filesystem MCP tools
        .ask("mcp__database"); // Ask for database MCP tools

    let mcp_tests = vec![
        ("mcp__github__list_repos", json!({}), "GitHub list repos"),
        (
            "mcp__github__create_issue",
            json!({}),
            "GitHub create issue",
        ),
        ("mcp__filesystem__read", json!({}), "Filesystem read"),
        ("mcp__database__query", json!({}), "Database query"),
    ];

    println!("  Permission Checks:");
    for (tool, args, desc) in &mcp_tests {
        let decision = mcp_policy.check(tool, args);
        let icon = match decision {
            PermissionDecision::Allow => "✅",
            PermissionDecision::Deny => "🚫",
            PermissionDecision::Ask => "❓",
        };
        println!("    {} {:?} ← {}", icon, decision, desc);
    }
    println!();

    // =========================================================================
    // 5. Rule pattern matching details
    // =========================================================================
    println!("━━━ 5. Rule Pattern Matching ━━━");
    let patterns = vec![
        ("Bash(cargo:*)", "Matches all cargo commands"),
        ("Bash(npm run test:*)", "Matches npm run test with any args"),
        ("Read(src/**/*.rs)", "Matches Rust files in src/"),
        ("Grep(*)", "Matches all grep invocations"),
        ("Write(*.md)", "Matches writing Markdown files"),
        ("mcp__pencil", "Matches all pencil MCP tools"),
    ];

    for (pattern, desc) in &patterns {
        let rule = PermissionRule::new(pattern);
        println!("  📐 {} → {}", pattern, desc);

        // Test some matches
        let test_args = match *pattern {
            "Bash(cargo:*)" => vec![
                ("Bash", json!({"command": "cargo build"}), true),
                ("Bash", json!({"command": "cargo test --lib"}), true),
                ("Bash", json!({"command": "npm install"}), false),
            ],
            "Read(src/**/*.rs)" => vec![
                ("Read", json!({"file_path": "src/main.rs"}), true),
                ("Read", json!({"file_path": "src/lib/mod.rs"}), true),
                ("Read", json!({"file_path": "tests/test.rs"}), false),
            ],
            _ => vec![],
        };

        for (tool, args, expected) in test_args {
            let matched = rule.matches(tool, &args);
            let icon = if matched == expected {
                "  ✓"
            } else {
                "  ✗"
            };
            println!(
                "    {} matches({}, {}) = {} {}",
                icon,
                tool,
                args,
                matched,
                if matched == expected {
                    ""
                } else {
                    "UNEXPECTED!"
                }
            );
        }
    }
    println!();

    // =========================================================================
    // 6. Evaluation order demonstration
    // =========================================================================
    println!("━━━ 6. Evaluation Order: Deny → Allow → Ask → Default ━━━");
    let order_policy = PermissionPolicy::new()
        .allow("Bash(cargo:*)") // Allow cargo commands
        .deny("Bash(cargo publish:*)") // But deny cargo publish specifically
        .ask("Bash(cargo release:*)"); // Ask for cargo release

    let order_tests = vec![
        (
            "Bash",
            json!({"command": "cargo build"}),
            "cargo build → Allow (matches allow rule)",
        ),
        (
            "Bash",
            json!({"command": "cargo publish"}),
            "cargo publish → Deny (deny checked first!)",
        ),
        (
            "Bash",
            json!({"command": "cargo release"}),
            "cargo release → Deny wins over Ask",
        ),
    ];

    println!("  Note: Deny rules are always evaluated FIRST\n");
    for (tool, args, desc) in &order_tests {
        let decision = order_policy.check(tool, args);
        let icon = match decision {
            PermissionDecision::Allow => "✅",
            PermissionDecision::Deny => "🚫",
            PermissionDecision::Ask => "❓",
        };
        println!("    {} {:?} ← {}", icon, decision, desc);
    }
    println!();

    println!("✅ Permission policy demo complete!");
}
