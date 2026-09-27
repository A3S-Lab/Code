use super::broad_allow_floor_requires_prompt;
use crate::permission::gate_preflight::GatePreflight;
use crate::permission::policy::CompiledPolicy;
use crate::permission::rules::parse_permission_rule;
use crate::permission::types::{AccessKind, Decision, PermissionConfig, RuleAction};

fn allow_policy(rule: &str) -> CompiledPolicy {
    CompiledPolicy::new(PermissionConfig::new(vec![
        parse_permission_rule(rule, RuleAction::Allow).expect("rule must parse"),
    ]))
}

fn standalone_floor(rule: &str, cmd: &str, cwd: &std::path::Path) -> bool {
    let policy = allow_policy(rule);
    let access = AccessKind::Bash(cmd.to_owned());
    let preflight = GatePreflight::evaluate(Some(&policy), &access, cwd, false);
    assert!(
        matches!(preflight.policy_decision(), Some(Decision::Allow)),
        "{rule} + {cmd} must be a policy Allow"
    );
    broad_allow_floor_requires_prompt(&access, Some(&policy), cwd, /*configured_mode*/ true)
}

#[test]
fn broad_allow_floor_without_a_manager_matches_the_manager() {
    let cwd = tempfile::tempdir().expect("tempdir must be created");

    assert!(standalone_floor(
        "Bash(git:*)",
        "git status > out",
        cwd.path()
    ));
    assert!(standalone_floor("Bash(*)", "cp src dst", cwd.path()));
    assert!(standalone_floor(
        "Bash(touch:*)",
        "LD_PRELOAD=/x/e.so touch CANARY",
        cwd.path()
    ));
    assert!(!standalone_floor("Bash(git:*)", "git status", cwd.path()));
    assert!(!standalone_floor("Bash(cp:*)", "cp src dst", cwd.path()));
    assert!(!broad_allow_floor_requires_prompt(
        &AccessKind::Edit("out".to_owned()),
        Some(&allow_policy("Bash(*)")),
        cwd.path(),
        /*configured_mode*/ true,
    ));
    // A recovered filename script clears the floor only where configured rules decide.
    const SCRIPT: &str = r#"LOG=/tmp/log; ls -lh "$LOG"; rg -n ERROR "$LOG" | head -40"#;
    assert!(!standalone_floor("Bash", SCRIPT, cwd.path()));
    let access = AccessKind::Bash(SCRIPT.to_owned());
    let policy = allow_policy("Bash");
    let floor = broad_allow_floor_requires_prompt(&access, Some(&policy), cwd.path(), false);
    assert!(floor, "dontAsk callers pass configured_mode = false");
}
