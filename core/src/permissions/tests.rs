use super::*;
use serde_json::json;

// ========================================================================
// PermissionRule Tests
// ========================================================================

#[test]
fn test_rule_parse_simple() {
    let rule = PermissionRule::new("Bash");
    assert_eq!(rule.tool_name, Some("Bash".to_string()));
    assert_eq!(rule.arg_pattern, None);
}

#[test]
fn test_rule_parse_with_pattern() {
    let rule = PermissionRule::new("Bash(cargo:*)");
    assert_eq!(rule.tool_name, Some("Bash".to_string()));
    assert_eq!(rule.arg_pattern, Some("cargo:*".to_string()));
}

#[test]
fn test_rule_parse_wildcard() {
    let rule = PermissionRule::new("Grep(*)");
    assert_eq!(rule.tool_name, Some("Grep".to_string()));
    assert_eq!(rule.arg_pattern, Some("*".to_string()));
}

#[test]
fn test_rule_match_tool_only() {
    let rule = PermissionRule::new("Bash");
    assert!(rule.matches("Bash", &json!({"command": "ls -la"})));
    assert!(rule.matches("bash", &json!({"command": "echo hello"})));
    assert!(!rule.matches("Read", &json!({})));
}

#[test]
fn test_rule_match_wildcard() {
    let rule = PermissionRule::new("Grep(*)");
    assert!(rule.matches("Grep", &json!({"pattern": "foo", "path": "/tmp"})));
    assert!(rule.matches("grep", &json!({"pattern": "bar"})));
}

#[test]
fn test_rule_match_prefix_wildcard() {
    let rule = PermissionRule::new("Bash(cargo:*)");
    assert!(rule.matches("Bash", &json!({"command": "cargo build"})));
    assert!(rule.matches("Bash", &json!({"command": "cargo test --lib"})));
    assert!(rule.matches("Bash", &json!({"command": "cargo"})));
    assert!(!rule.matches("Bash", &json!({"command": "npm install"})));
}

#[test]
fn test_rule_match_npm_commands() {
    let rule = PermissionRule::new("Bash(npm run:*)");
    assert!(rule.matches("Bash", &json!({"command": "npm run test"})));
    assert!(rule.matches("Bash", &json!({"command": "npm run build"})));
    assert!(!rule.matches("Bash", &json!({"command": "npm install"})));
}

#[test]
fn test_rule_match_file_path() {
    let rule = PermissionRule::new("Read(src/*.rs)");
    assert!(rule.matches("Read", &json!({"file_path": "src/main.rs"})));
    assert!(rule.matches("Read", &json!({"file_path": "src/lib.rs"})));
    assert!(!rule.matches("Read", &json!({"file_path": "src/foo/bar.rs"})));
}

#[test]
fn test_rule_match_recursive_glob() {
    let rule = PermissionRule::new("Read(src/**/*.rs)");
    assert!(rule.matches("Read", &json!({"file_path": "src/main.rs"})));
    assert!(rule.matches("Read", &json!({"file_path": "src/foo/bar.rs"})));
    assert!(rule.matches("Read", &json!({"file_path": "src/a/b/c.rs"})));
}

#[test]
fn test_rule_match_mcp_tool() {
    let rule = PermissionRule::new("mcp__pencil");
    assert!(rule.matches("mcp__pencil", &json!({})));
    assert!(rule.matches("mcp__pencil__batch_design", &json!({})));
    assert!(rule.matches("mcp__pencil__batch_get", &json!({})));
    assert!(!rule.matches("mcp__other", &json!({})));
}

#[test]
fn test_rule_match_mcp_tool_wildcard() {
    // "mcp__longvt__*" must match all tools from the longvt server.
    // Previously this failed because matches_tool_name treated '*' as a
    // literal character and starts_with("mcp__longvt__*") always returned false.
    let rule = PermissionRule::new("mcp__longvt__*");
    assert!(rule.matches("mcp__longvt__search", &json!({})));
    assert!(rule.matches("mcp__longvt__create_memory", &json!({})));
    assert!(rule.matches("mcp__longvt__delete", &json!({})));
    assert!(!rule.matches("mcp__pencil__batch_design", &json!({})));
    assert!(!rule.matches("mcp__other__tool", &json!({})));

    // "mcp__*" must match any MCP tool from any server.
    let rule_all = PermissionRule::new("mcp__*");
    assert!(rule_all.matches("mcp__longvt__search", &json!({})));
    assert!(rule_all.matches("mcp__pencil__draw", &json!({})));
    assert!(!rule_all.matches("bash", &json!({})));
}

#[test]
fn test_rule_case_insensitive() {
    let rule = PermissionRule::new("BASH(cargo:*)");
    assert!(rule.matches("Bash", &json!({"command": "cargo build"})));
    assert!(rule.matches("bash", &json!({"command": "cargo test"})));
    assert!(rule.matches("BASH", &json!({"command": "cargo check"})));
}

// ========================================================================
// PermissionPolicy Tests
// ========================================================================

#[test]
fn test_policy_default() {
    let policy = PermissionPolicy::default();
    assert!(policy.enabled);
    assert_eq!(policy.default_decision, PermissionDecision::Ask);
    assert!(policy.allow.is_empty());
    assert!(policy.deny.is_empty());
    assert!(policy.ask.is_empty());
}

#[test]
fn test_policy_explicit_allow_default() {
    let policy = PermissionPolicy {
        default_decision: PermissionDecision::Allow,
        ..PermissionPolicy::default()
    };
    assert_eq!(policy.default_decision, PermissionDecision::Allow);
}

#[test]
fn test_policy_strict() {
    let policy = PermissionPolicy::strict();
    assert_eq!(policy.default_decision, PermissionDecision::Ask);
}

#[test]
fn test_policy_builder() {
    let policy = PermissionPolicy::new()
        .allow("Bash(cargo:*)")
        .allow("Grep(*)")
        .deny("Bash(rm -rf:*)")
        .ask("Write(*)");

    assert_eq!(policy.allow.len(), 2);
    assert_eq!(policy.deny.len(), 1);
    assert_eq!(policy.ask.len(), 1);
}

#[test]
fn test_policy_check_allow() {
    let policy = PermissionPolicy::new().allow("Bash(cargo:*)");

    let decision = policy.check("Bash", &json!({"command": "cargo build"}));
    assert_eq!(decision, PermissionDecision::Allow);
}

#[test]
fn test_policy_check_deny() {
    let policy = PermissionPolicy::new().deny("Bash(rm -rf:*)");

    let decision = policy.check("Bash", &json!({"command": "rm -rf /"}));
    assert_eq!(decision, PermissionDecision::Deny);
}

#[test]
fn test_policy_check_ask() {
    let policy = PermissionPolicy::new().ask("Write(*)");

    let decision = policy.check("Write", &json!({"file_path": "/tmp/test.txt"}));
    assert_eq!(decision, PermissionDecision::Ask);
}

#[test]
fn test_policy_check_default() {
    let policy = PermissionPolicy::new();

    let decision = policy.check("Unknown", &json!({}));
    assert_eq!(decision, PermissionDecision::Ask);
}

#[test]
fn test_policy_deny_wins_over_allow() {
    let policy = PermissionPolicy::new().allow("Bash(*)").deny("Bash(rm:*)");

    // Allow matches, but deny also matches - deny wins
    let decision = policy.check("Bash", &json!({"command": "rm -rf /tmp"}));
    assert_eq!(decision, PermissionDecision::Deny);

    // Only allow matches
    let decision = policy.check("Bash", &json!({"command": "ls -la"}));
    assert_eq!(decision, PermissionDecision::Allow);
}

#[test]
fn test_policy_allow_wins_over_ask() {
    let policy = PermissionPolicy::new()
        .allow("Bash(cargo:*)")
        .ask("Bash(*)");

    // Both match, but allow is checked before ask
    let decision = policy.check("Bash", &json!({"command": "cargo build"}));
    assert_eq!(decision, PermissionDecision::Allow);

    // Only ask matches
    let decision = policy.check("Bash", &json!({"command": "npm install"}));
    assert_eq!(decision, PermissionDecision::Ask);
}

#[test]
fn test_policy_disabled() {
    let mut policy = PermissionPolicy::new().deny("Bash(rm:*)").ask("Bash(*)");
    policy.enabled = false;

    // When disabled, everything is allowed
    let decision = policy.check("Bash", &json!({"command": "rm -rf /"}));
    assert_eq!(decision, PermissionDecision::Allow);
}

#[test]
fn test_policy_is_allowed() {
    let policy = PermissionPolicy::new().allow("Bash(cargo:*)");

    assert!(policy.is_allowed("Bash", &json!({"command": "cargo build"})));
    assert!(!policy.is_allowed("Bash", &json!({"command": "npm install"})));
}

#[test]
fn test_policy_is_denied() {
    let policy = PermissionPolicy::new().deny("Bash(rm:*)");

    assert!(policy.is_denied("Bash", &json!({"command": "rm -rf /"})));
    assert!(!policy.is_denied("Bash", &json!({"command": "ls -la"})));
}

#[test]
fn test_policy_requires_confirmation() {
    // Create a policy that explicitly allows Read but asks for Write
    let mut policy = PermissionPolicy::new().allow("Read(*)").ask("Write(*)");
    policy.default_decision = PermissionDecision::Deny; // Make default deny to test ask rule

    assert!(policy.requires_confirmation("Write", &json!({"file_path": "/tmp/test"})));
    assert!(!policy.requires_confirmation("Read", &json!({"file_path": "/tmp/test"})));
}

#[test]
fn test_policy_matching_rules() {
    let policy = PermissionPolicy::new()
        .allow("Bash(cargo:*)")
        .deny("Bash(cargo fmt:*)")
        .ask("Bash(*)");

    let matching = policy.get_matching_rules("Bash", &json!({"command": "cargo fmt"}));
    assert_eq!(matching.deny.len(), 1);
    assert_eq!(matching.allow.len(), 1);
    assert_eq!(matching.ask.len(), 1);
}

#[test]
fn test_policy_allow_all() {
    let policy = PermissionPolicy::new().allow_all(&["Bash(cargo:*)", "Bash(npm:*)", "Grep(*)"]);

    assert_eq!(policy.allow.len(), 3);
    assert!(policy.is_allowed("Bash", &json!({"command": "cargo build"})));
    assert!(policy.is_allowed("Bash", &json!({"command": "npm run test"})));
    assert!(policy.is_allowed("Grep", &json!({"pattern": "foo"})));
}

// ========================================================================
// PermissionRule Deserialization Tests
// ========================================================================

#[test]
fn test_rule_deserialize_plain_string() {
    // YAML: `- read`
    let rule: PermissionRule = serde_yaml::from_str("read").unwrap();
    assert_eq!(rule.rule, "read");
    assert!(rule.matches("read", &json!({})));
    assert!(!rule.matches("write", &json!({})));
}

#[test]
fn test_rule_deserialize_plain_string_with_pattern() {
    // YAML: `- "Bash(cargo:*)"`
    let rule: PermissionRule = serde_yaml::from_str("\"Bash(cargo:*)\"").unwrap();
    assert_eq!(rule.rule, "Bash(cargo:*)");
    assert!(rule.matches("Bash", &json!({"command": "cargo build"})));
}

#[test]
fn test_rule_deserialize_struct_form() {
    // YAML: `- rule: read`
    let rule: PermissionRule = serde_yaml::from_str("rule: read").unwrap();
    assert_eq!(rule.rule, "read");
    assert!(rule.matches("read", &json!({})));
}

#[test]
fn test_rule_deserialize_in_policy() {
    // Full policy YAML with mixed formats
    let yaml = r#"
allow:
  - read
  - "Bash(cargo:*)"
  - rule: grep
deny:
  - write
"#;
    let policy: PermissionPolicy = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(policy.allow.len(), 3);
    assert_eq!(policy.deny.len(), 1);
    assert!(policy.is_allowed("read", &json!({})));
    assert!(policy.is_allowed("Bash", &json!({"command": "cargo build"})));
    assert!(policy.is_allowed("grep", &json!({})));
    assert!(policy.is_denied("write", &json!({})));
}

// ========================================================================
// PermissionManager Tests
// ========================================================================

#[test]
fn test_manager_default() {
    let manager = PermissionManager::new();
    assert_eq!(
        manager.global_policy().default_decision,
        PermissionDecision::Ask
    );
}

#[test]
fn test_manager_with_global_policy() {
    let policy = PermissionPolicy {
        default_decision: PermissionDecision::Allow,
        ..PermissionPolicy::default()
    };
    let manager = PermissionManager::with_global_policy(policy);
    assert_eq!(
        manager.global_policy().default_decision,
        PermissionDecision::Allow
    );
}

#[test]
fn test_manager_session_policy() {
    let mut manager = PermissionManager::new();

    let session_policy = PermissionPolicy::new().allow("Bash(cargo:*)");
    manager.set_session_policy("session-1", session_policy);

    // Session 1 has custom policy
    let decision = manager.check("session-1", "Bash", &json!({"command": "cargo build"}));
    assert_eq!(decision, PermissionDecision::Allow);

    // Session 2 uses global policy
    let decision = manager.check("session-2", "Bash", &json!({"command": "cargo build"}));
    assert_eq!(decision, PermissionDecision::Ask);
}

#[test]
fn test_manager_remove_session_policy() {
    let mut manager = PermissionManager::new();

    let session_policy = PermissionPolicy {
        default_decision: PermissionDecision::Allow,
        ..PermissionPolicy::default()
    };
    manager.set_session_policy("session-1", session_policy);

    // Before removal
    let decision = manager.check("session-1", "Bash", &json!({"command": "anything"}));
    assert_eq!(decision, PermissionDecision::Allow);

    manager.remove_session_policy("session-1");

    // After removal - falls back to global
    let decision = manager.check("session-1", "Bash", &json!({"command": "anything"}));
    assert_eq!(decision, PermissionDecision::Ask);
}

#[test]
fn test_manager_global_deny_overrides_session_allow() {
    let mut manager =
        PermissionManager::with_global_policy(PermissionPolicy::new().deny("Bash(rm:*)"));

    let session_policy = PermissionPolicy::new().allow("Bash(*)");
    manager.set_session_policy("session-1", session_policy);

    // Session allows all Bash, but global denies rm
    let decision = manager.check("session-1", "Bash", &json!({"command": "rm -rf /"}));
    assert_eq!(decision, PermissionDecision::Deny);

    // Other commands are allowed
    let decision = manager.check("session-1", "Bash", &json!({"command": "ls -la"}));
    assert_eq!(decision, PermissionDecision::Allow);
}

// ========================================================================
// Integration Tests
// ========================================================================

#[test]
fn test_realistic_dev_policy() {
    let policy = PermissionPolicy::new()
        // Allow common dev commands
        .allow_all(&[
            "Bash(cargo:*)",
            "Bash(npm:*)",
            "Bash(pnpm:*)",
            "Bash(just:*)",
            "Bash(git status:*)",
            "Bash(git diff:*)",
            "Bash(echo:*)",
            "Grep(*)",
            "Glob(*)",
            "Ls(*)",
        ])
        // Deny dangerous commands
        .deny_all(&["Bash(rm -rf:*)", "Bash(sudo:*)", "Bash(curl | sh:*)"])
        // Always ask for writes
        .ask_all(&["Write(*)", "Edit(*)"]);

    // Allowed
    assert!(policy.is_allowed("Bash", &json!({"command": "cargo build"})));
    assert!(policy.is_allowed("Bash", &json!({"command": "npm run test"})));
    assert!(policy.is_allowed("Grep", &json!({"pattern": "TODO"})));

    // Denied
    assert!(policy.is_denied("Bash", &json!({"command": "rm -rf /"})));
    assert!(policy.is_denied("Bash", &json!({"command": "sudo apt install"})));

    // Ask
    assert!(policy.requires_confirmation("Write", &json!({"file_path": "/tmp/test.rs"})));
    assert!(policy.requires_confirmation("Edit", &json!({"file_path": "src/main.rs"})));
}

#[test]
fn test_mcp_tool_permissions() {
    let policy = PermissionPolicy::new()
        .allow("mcp__pencil")
        .deny("mcp__dangerous");

    assert!(policy.is_allowed("mcp__pencil__batch_design", &json!({})));
    assert!(policy.is_allowed("mcp__pencil__batch_get", &json!({})));
    assert!(policy.is_denied("mcp__dangerous__execute", &json!({})));
}

#[test]
fn test_allow_by_default_with_mcp_wildcard_deny() {
    // Allow-by-default policies can still carry explicit deny rules.
    let policy = PermissionPolicy {
        default_decision: PermissionDecision::Allow,
        ..PermissionPolicy::default()
    }
    .deny("mcp__longvt__*");

    // longvt tools are denied
    assert_eq!(
        policy.check("mcp__longvt__search", &json!({})),
        PermissionDecision::Deny
    );
    assert_eq!(
        policy.check("mcp__longvt__create_memory", &json!({})),
        PermissionDecision::Deny
    );
    // other MCP tools are still allowed by the explicit default decision
    assert_eq!(
        policy.check("mcp__pencil__draw", &json!({})),
        PermissionDecision::Allow
    );
    assert_eq!(
        policy.check("bash", &json!({"command": "ls"})),
        PermissionDecision::Allow
    );
}

#[test]
fn test_serialization() {
    let policy = PermissionPolicy::new()
        .allow("Bash(cargo:*)")
        .deny("Bash(rm:*)");

    let json = serde_json::to_string(&policy).unwrap();
    let deserialized: PermissionPolicy = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.allow.len(), 1);
    assert_eq!(deserialized.deny.len(), 1);
}

#[test]
fn test_matching_rules_is_empty() {
    let rules = MatchingRules {
        deny: vec![],
        allow: vec![],
        ask: vec![],
    };
    assert!(rules.is_empty());

    let rules = MatchingRules {
        deny: vec!["Bash".to_string()],
        allow: vec![],
        ask: vec![],
    };
    assert!(!rules.is_empty());

    let rules = MatchingRules {
        deny: vec![],
        allow: vec!["Read".to_string()],
        ask: vec![],
    };
    assert!(!rules.is_empty());

    let rules = MatchingRules {
        deny: vec![],
        allow: vec![],
        ask: vec!["Write".to_string()],
    };
    assert!(!rules.is_empty());
}

#[test]
fn test_permission_manager_default() {
    let pm = PermissionManager::default();
    let policy = pm.global_policy();
    assert!(policy.allow.is_empty());
    assert!(policy.deny.is_empty());
    assert!(policy.ask.is_empty());
}

#[test]
fn test_permission_manager_set_global_policy() {
    let mut pm = PermissionManager::new();
    let policy = PermissionPolicy::new().allow("Bash(*)");
    pm.set_global_policy(policy);
    assert_eq!(pm.global_policy().allow.len(), 1);
}

#[test]
fn test_permission_manager_session_policy() {
    let mut pm = PermissionManager::new();
    let policy = PermissionPolicy::new().deny("Bash(rm:*)");
    pm.set_session_policy("s1", policy);

    let effective = pm.get_effective_policy("s1");
    assert_eq!(effective.deny.len(), 1);

    // Non-existent session falls back to global
    let global = pm.get_effective_policy("s2");
    assert!(global.deny.is_empty());
}

#[test]
fn test_permission_manager_remove_session_policy() {
    let mut pm = PermissionManager::new();
    pm.set_session_policy("s1", PermissionPolicy::new().deny("Bash(*)"));
    assert_eq!(pm.get_effective_policy("s1").deny.len(), 1);

    pm.remove_session_policy("s1");
    assert!(pm.get_effective_policy("s1").deny.is_empty());
}

#[test]
fn test_permission_manager_check_deny() {
    let mut pm = PermissionManager::new();
    pm.set_global_policy(PermissionPolicy::new().deny("Bash(rm:*)"));

    let decision = pm.check("s1", "Bash", &json!({"command": "rm -rf /"}));
    assert_eq!(decision, PermissionDecision::Deny);
}

#[test]
fn test_permission_manager_check_allow() {
    let mut pm = PermissionManager::new();
    pm.set_global_policy(PermissionPolicy::new().allow("Bash(cargo:*)"));

    let decision = pm.check("s1", "Bash", &json!({"command": "cargo build"}));
    assert_eq!(decision, PermissionDecision::Allow);
}

#[test]
fn test_permission_manager_check_session_override() {
    let mut pm = PermissionManager::new();
    pm.set_global_policy(PermissionPolicy::new().allow("Bash(*)"));
    pm.set_session_policy("s1", PermissionPolicy::new().deny("Bash(rm:*)"));

    // Session policy denies rm
    let decision = pm.check("s1", "Bash", &json!({"command": "rm -rf /"}));
    assert_eq!(decision, PermissionDecision::Deny);

    // Other session uses global (allow all)
    let decision = pm.check("s2", "Bash", &json!({"command": "rm -rf /"}));
    assert_eq!(decision, PermissionDecision::Allow);
}

#[test]
fn test_permission_manager_with_global_policy() {
    let policy = PermissionPolicy::new().allow("Read(*)").deny("Write(*)");
    let pm = PermissionManager::with_global_policy(policy);
    assert_eq!(pm.global_policy().allow.len(), 1);
    assert_eq!(pm.global_policy().deny.len(), 1);
}
