//! ACP agent over a3s-code-core 9.0.0.
//!
//! Speaks Agent Client Protocol on stdio so the forked a3s-build pager can
//! drive real A3S sessions instead of the in-process A3S Code `MvpAgent`.

mod agent;
mod cc_switch;
mod config;
mod goal;
mod grok_account;

pub use agent::A3sCodeAgent;
pub use config::{load_launch_config, LaunchConfig};

/// Project one tool through the posture the next admission freezes.
///
/// `deny_rule` is part of that policy. Deny and ask do not call `execute`.
/// Always-approve adds execute-lane allows; a deny rule still wins.
pub fn check_admitted_tool<T>(
    posture: &str,
    deny_rule: &str,
    tool: &str,
    args: &serde_json::Value,
    execute: impl FnOnce() -> T,
) -> Result<T, &'static str> {
    let policy = a3s_code_core::permissions::policy_for_posture(posture).deny(deny_rule);
    match a3s_code_core::fact_control::admit_tool_call(&policy, tool, args, execute) {
        Ok(value) => Ok(value),
        Err(a3s_code_core::permissions::PermissionDecision::Deny) => Err("deny"),
        Err(a3s_code_core::permissions::PermissionDecision::Ask) => Err("ask"),
        Err(a3s_code_core::permissions::PermissionDecision::Allow) => Err("allow"),
    }
}
