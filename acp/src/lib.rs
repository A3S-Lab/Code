//! ACP agent over a3s-code-core 9.0.0.
//!
//! Speaks Agent Client Protocol on stdio so the forked a3s-build pager can
//! drive real A3S sessions instead of the in-process A3S Code `MvpAgent`.

mod agent;
pub mod approval_learning;
mod cc_switch;
mod config;
mod cursor_account;
mod document_read;
mod engineering;
pub mod form_request;
mod goal;
mod grok_account;
mod harness_assemble;
mod host_context;
mod local_assets;
pub mod log_file;
mod plugin_mcp;
mod plugin_selection;
mod plugin_tools;
mod plugin_workspace;
mod run_observation;
mod sampling_consent;
mod turn_cancel;
pub mod unattended;
mod wiki_search;

pub use agent::A3sCodeAgent;
pub use config::{load_launch_config, LaunchConfig};
pub use plugin_selection::PluginSelectionHost;

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
