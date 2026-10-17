//! Every core shell permission surface refuses the shared bypass corpus.

#[path = "support/shell_bypass_corpus.rs"]
mod corpus;

use a3s_code_core::permissions::shell::{analyze, ShellRisk};
use a3s_code_core::permissions::{
    policy_for_posture, InteractiveToolGuardrail, PermissionChecker, PermissionDecision,
    PermissionEnforcement, PermissionPolicy,
};
use a3s_code_core::read_only_verifier;
use serde_json::json;

const MODES: &[&str] = &["default", "plan", "auto", "force"];

fn bash(command: &str) -> serde_json::Value {
    json!({ "command": command })
}

fn prefix_allow_policy() -> PermissionPolicy {
    corpus::PREFIX_ALLOW_RULES
        .iter()
        .fold(PermissionPolicy::new(), |policy, rule| policy.allow(rule))
}

fn unrestricted_policy() -> PermissionPolicy {
    PermissionPolicy {
        enforcement: PermissionEnforcement::Unrestricted,
        ..PermissionPolicy::new()
    }
}

fn wide_open_policies() -> Vec<(&'static str, PermissionPolicy)> {
    vec![
        ("prefix allow rules", prefix_allow_policy()),
        ("yolo posture", policy_for_posture("yolo")),
        ("allow bash", PermissionPolicy::new().allow("bash")),
        ("allow Bash(*)", PermissionPolicy::new().allow("Bash(*)")),
        ("unrestricted", unrestricted_policy()),
    ]
}

#[test]
fn catastrophic_commands_are_denied_under_every_allow() {
    for command in corpus::CATASTROPHIC {
        assert_eq!(
            analyze(command).risk(),
            ShellRisk::Catastrophic,
            "classifier missed: {command:?}"
        );
        for (name, policy) in wide_open_policies() {
            assert_eq!(
                policy.check("bash", &bash(command)),
                PermissionDecision::Deny,
                "{name} let a catastrophic command through: {command:?}"
            );
        }
        for mode in MODES {
            let guardrail = InteractiveToolGuardrail::for_mode(mode).unwrap();
            assert_eq!(
                guardrail.check("bash", &bash(command)),
                PermissionDecision::Deny,
                "{mode} guardrail let a catastrophic command through: {command:?}"
            );
        }
        assert!(
            !read_only_verifier::tool_allowed("bash", &bash(command)),
            "verifier ran: {command:?}"
        );
    }
}

#[test]
fn hard_denials_are_denied_by_the_guardrail_and_never_ride_an_allow_prefix() {
    let policy = prefix_allow_policy();
    for command in corpus::HARD_DENIAL {
        assert!(
            analyze(command).risk().is_hard_denial(),
            "classifier missed: {command:?} -> {:?}",
            analyze(command).risk()
        );
        assert!(InteractiveToolGuardrail::is_catastrophic_bash_command(
            command
        ));
        for mode in MODES {
            let guardrail = InteractiveToolGuardrail::for_mode(mode).unwrap();
            assert_eq!(
                guardrail.check("bash", &bash(command)),
                PermissionDecision::Deny,
                "{mode} guardrail: {command:?}"
            );
        }
        assert_ne!(
            policy.check("bash", &bash(command)),
            PermissionDecision::Allow,
            "prefix rule allowed: {command:?}"
        );
        assert!(!read_only_verifier::tool_allowed("bash", &bash(command)));
    }
}

#[test]
fn smuggled_commands_never_ride_an_allowed_prefix() {
    let policy = prefix_allow_policy();
    let guardrail = InteractiveToolGuardrail::default();
    for command in corpus::SMUGGLED {
        assert_ne!(
            policy.check("bash", &bash(command)),
            PermissionDecision::Allow,
            "prefix rule allowed: {command:?}"
        );
        assert_ne!(
            guardrail.check("bash", &bash(command)),
            PermissionDecision::Allow,
            "guardrail read-only: {command:?}"
        );
    }
}

#[test]
fn a_deny_rule_reaches_every_simple_command() {
    let policy = PermissionPolicy::new()
        .allow_all(&["bash"])
        .deny("Bash(rm:*)");
    let asking = PermissionPolicy::new()
        .ask("Bash(rm:*)")
        .allow("Bash(ls:*)");
    for command in corpus::RM_DENY_EVASIONS {
        assert_eq!(
            policy.check("bash", &bash(command)),
            PermissionDecision::Deny,
            "deny rule missed: {command:?}"
        );
        assert_ne!(
            asking.check("bash", &bash(command)),
            PermissionDecision::Allow,
            "ask rule missed: {command:?}"
        );
    }
}

#[test]
fn the_verifier_refuses_every_write() {
    for command in corpus::VERIFIER_WRITES {
        assert!(
            !read_only_verifier::tool_allowed("bash", &bash(command)),
            "verifier ran a write: {command:?}"
        );
    }
}

#[test]
fn ordinary_work_still_passes_every_surface() {
    let policy = prefix_allow_policy();
    for command in corpus::ALLOWED_BY_PREFIX_RULES {
        assert_eq!(
            policy.check("bash", &bash(command)),
            PermissionDecision::Allow,
            "prefix rule over-narrowed: {command:?}"
        );
    }
    let yolo = policy_for_posture("yolo");
    for command in corpus::ALLOWED_BY_YOLO {
        assert_eq!(
            yolo.check("bash", &bash(command)),
            PermissionDecision::Allow,
            "yolo over-narrowed: {command:?}"
        );
    }
    let guardrail = InteractiveToolGuardrail::default();
    for command in corpus::GUARDRAIL_READ_ONLY {
        assert_eq!(
            guardrail.check("bash", &bash(command)),
            PermissionDecision::Allow,
            "guardrail over-narrowed: {command:?}"
        );
    }
    for command in corpus::VERIFIER_RUNS {
        assert!(
            read_only_verifier::tool_allowed("bash", &bash(command)),
            "verifier over-narrowed: {command:?}"
        );
    }
}

#[test]
fn the_classifier_never_reports_an_unparsed_command_as_read_only() {
    for command in corpus::SMUGGLED
        .iter()
        .chain(corpus::HARD_DENIAL)
        .chain(corpus::CATASTROPHIC)
    {
        let analysis = analyze(command);
        assert!(
            analysis.risk() != ShellRisk::ReadOnly,
            "{command:?} classified read-only"
        );
    }
}
