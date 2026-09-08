//! Specialty AgentStyle permission policies shared by built-in delegated
//! agents and host-selected primary-session styles.

use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::prompts::AgentStyle;

/// Hard permission policy for an explicit specialty style.
///
/// `GeneralPurpose` returns `None` so the host/session default remains writable.
pub fn specialty_permission_policy(style: AgentStyle) -> Option<PermissionPolicy> {
    match style {
        AgentStyle::GeneralPurpose => None,
        AgentStyle::Explore => Some(explore_permissions()),
        AgentStyle::Plan => Some(plan_permissions()),
        AgentStyle::Verification | AgentStyle::CodeReview => Some(verification_permissions()),
    }
}

fn explore_permissions() -> PermissionPolicy {
    let mut policy = PermissionPolicy::new()
        .allow_all(&[
            "read",
            "search",
            "ls",
            "web_fetch",
            "web_search",
            "update_plan",
        ])
        .deny_all(&["write", "edit", "download", "task", "parallel_task"])
        .allow("Bash(ls:*)")
        .allow("Bash(cat:*)")
        .allow("Bash(head:*)")
        .allow("Bash(tail:*)")
        .allow("Bash(find:*)")
        .allow("Bash(wc:*)")
        .deny("Bash(rm:*)")
        .deny("Bash(mv:*)")
        .deny("Bash(cp:*)");
    policy.default_decision = PermissionDecision::Deny;
    policy
}

fn plan_permissions() -> PermissionPolicy {
    let mut policy = PermissionPolicy::new()
        .allow_all(&["read", "search", "ls", "update_plan"])
        .deny_all(&["write", "edit", "download", "bash", "task", "parallel_task"]);
    policy.default_decision = PermissionDecision::Deny;
    policy
}

fn verification_permissions() -> PermissionPolicy {
    let mut policy = PermissionPolicy::new()
        .allow_all(&[
            "read",
            "search",
            "ls",
            "bash",
            "web_fetch",
            "web_search",
            "update_plan",
        ])
        .deny_all(&["write", "edit", "download", "task", "parallel_task"]);
    policy.default_decision = PermissionDecision::Deny;
    policy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::PermissionChecker;
    use serde_json::json;

    #[test]
    fn explore_keeps_read_visible_and_denies_write() {
        let policy = specialty_permission_policy(AgentStyle::Explore).expect("explore policy");
        assert!(policy.expose_to_model("read"));
        assert!(policy.expose_to_model("update_plan"));
        assert!(!policy.expose_to_model("write"));
        assert_eq!(
            policy.check("write", &json!({"file_path": "x", "content": "y"})),
            PermissionDecision::Deny
        );
    }

    #[test]
    fn plan_style_exposes_update_plan() {
        let policy = specialty_permission_policy(AgentStyle::Plan).expect("plan policy");
        assert!(policy.expose_to_model("update_plan"));
        assert!(!policy.expose_to_model("write"));
    }

    #[test]
    fn general_purpose_has_no_specialty_overlay() {
        assert!(specialty_permission_policy(AgentStyle::GeneralPurpose).is_none());
    }
}
