#![cfg(test)]

use super::*;
use serde_json::json;

fn closed(rules: &[&str]) -> PermissionPolicy {
    let mut policy = PermissionPolicy::new().allow_all(rules);
    policy.default_decision = PermissionDecision::Deny;
    policy
}

fn batch(paths: &[&str]) -> serde_json::Value {
    json!({"files": paths.iter().map(|path| json!({"path": path})).collect::<Vec<_>>()})
}

#[test]
fn all_scoped_members_are_allowed_in_either_order() {
    let policy = closed(&["read(src/**)"]);
    for paths in [["src/a.rs", "src/b.rs"], ["src/b.rs", "src/a.rs"]] {
        assert_eq!(
            policy.check("read", &batch(&paths)),
            PermissionDecision::Allow
        );
    }
}

#[test]
fn different_allow_rules_cover_members_without_one_batch_wide_rule() {
    let policy = closed(&["read(src/a.rs)", "read(tests/b.rs)"]);
    assert_eq!(
        policy.check("read", &batch(&["src/a.rs", "tests/b.rs"])),
        PermissionDecision::Allow
    );
}

#[test]
fn any_denied_member_blocks_the_whole_batch_in_either_order() {
    let policy = closed(&["read"]).deny("read(private.txt)");
    for paths in [["src/a.rs", "private.txt"], ["private.txt", "src/a.rs"]] {
        assert_eq!(
            policy.check("read", &batch(&paths)),
            PermissionDecision::Deny
        );
    }
}

#[test]
fn wildcard_cannot_authorize_an_unscoped_member_across_a_joined_newline() {
    let policy = closed(&["read(src/*)"]);
    assert_eq!(
        policy.check("read", &batch(&["src/a.rs", "private.txt"])),
        PermissionDecision::Deny
    );
    assert_eq!(
        policy.check("read", &batch(&["src/a.rs\nprivate.txt"])),
        PermissionDecision::Allow,
        "one literal in-scope filename remains one argument"
    );
}

#[test]
fn every_member_contributes_its_default_ask_or_deny() {
    let policy = PermissionPolicy::new()
        .allow("read(src/a.rs)")
        .ask("read(reviews/b.rs)");
    assert_eq!(
        policy.check("read", &batch(&["src/a.rs", "reviews/b.rs"])),
        PermissionDecision::Ask
    );
    assert_eq!(
        policy.check("read", &batch(&["src/a.rs", "unknown.rs"])),
        PermissionDecision::Ask
    );
    let policy = policy.deny("read(private.txt)");
    assert_eq!(
        policy.check("read", &batch(&["reviews/b.rs", "private.txt"])),
        PermissionDecision::Deny
    );
    assert_eq!(
        closed(&["read(src/a.rs)"]).check("read", &batch(&["src/a.rs", "unknown.rs"])),
        PermissionDecision::Deny
    );
}

#[test]
fn invalid_ambiguous_or_oversized_batch_paths_fail_closed() {
    let policy = PermissionPolicy::new().allow("read");
    for args in [
        json!({"files":null}),
        json!({"files":[]}),
        json!({"files":"src/a.rs"}),
        json!({"files":[{}]}),
        json!({"files":[{"path":null}]}),
        json!({"files":[{"path":""}]}),
        json!({"files":[{"path":7}]}),
        json!({"files":[{"path":"src/a.rs"},{}]}),
        json!({"file_path":"src/a.rs","files":[{"path":"private.txt"}]}),
        json!({"file_path":null,"files":[{"path":"src/a.rs"}]}),
        json!({"files":vec![json!({"path":"src/a.rs"});33]}),
        json!({"files":[{"path":"x".repeat(4097)}]}),
    ] {
        assert_eq!(
            policy.check("read", &args),
            PermissionDecision::Deny,
            "{args}"
        );
    }
    assert_eq!(
        policy.check(
            "read",
            &json!({"files":vec![json!({"path":"src/a.rs"});32]})
        ),
        PermissionDecision::Allow
    );
    assert_eq!(
        policy.check("read", &json!({"files":[{"path":"x".repeat(4096)}]})),
        PermissionDecision::Allow
    );
}

#[test]
fn canonical_read_names_and_scalar_calls_keep_the_same_scope() {
    let policy = closed(&["Read(src/**)"]);
    for name in ["read", "Read", "functions.read", "Functions.Read"] {
        assert_eq!(
            policy.check(name, &batch(&["src/a.rs", "src/b.rs"])),
            PermissionDecision::Allow
        );
        assert_eq!(
            policy.check(name, &json!({"file_path":"src/a.rs"})),
            PermissionDecision::Allow
        );
        assert_eq!(
            policy.check(name, &json!({"file_path":"private.txt"})),
            PermissionDecision::Deny
        );
    }
    assert_eq!(
        closed(&["write"]).check("write", &json!({"files":[]})),
        PermissionDecision::Allow,
        "batch-read handling does not reinterpret another tool"
    );
}

#[test]
fn session_manager_keeps_global_deny_for_every_member() {
    let mut manager =
        PermissionManager::with_global_policy(closed(&["read"]).deny("read(private.txt)"));
    manager.set_session_policy("session", closed(&["read(src/**)", "read(private.txt)"]));
    assert_eq!(
        manager.check("session", "read", &batch(&["src/a.rs", "src/b.rs"])),
        PermissionDecision::Allow
    );
    for paths in [["src/a.rs", "private.txt"], ["private.txt", "src/a.rs"]] {
        assert_eq!(
            manager.check("session", "read", &batch(&paths)),
            PermissionDecision::Deny
        );
        assert_eq!(
            manager.check("other", "read", &batch(&paths)),
            PermissionDecision::Deny
        );
    }
    assert_eq!(
        manager.check("session", "read", &json!({"files":[]})),
        PermissionDecision::Deny
    );
}

#[test]
fn matching_rule_diagnostics_include_each_members_rules() {
    let policy = closed(&["read(src/**)"])
        .deny("read(private.txt)")
        .ask("read(reviews/**)");
    let rules =
        policy.get_matching_rules("read", &batch(&["src/a.rs", "private.txt", "reviews/b.rs"]));
    assert_eq!(rules.allow, ["read(src/**)"]);
    assert_eq!(rules.deny, ["read(private.txt)"]);
    assert_eq!(rules.ask, ["read(reviews/**)"]);
}

#[test]
fn unknown_unicode_tool_names_do_not_panic_or_gain_read_access() {
    for name in ["工具abλx", "functions.工具abλx", "read工具abλx"] {
        assert_eq!(
            PermissionPolicy::new().check(name, &batch(&["src/a.rs"])),
            PermissionDecision::Ask
        );
        assert_eq!(
            closed(&["read(src/**)"]).check(name, &batch(&["src/a.rs"])),
            PermissionDecision::Deny
        );
        assert_eq!(
            PermissionManager::new().check("session", name, &batch(&["src/a.rs"])),
            PermissionDecision::Ask
        );
    }
}
