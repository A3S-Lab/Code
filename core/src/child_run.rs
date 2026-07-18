//! Parent-to-child capability inheritance for delegated task runs.
//!
//! When a parent session delegates work via `task`/`parallel_task`, certain
//! capabilities should propagate to the child run. This module makes that
//! contract explicit and documented.
//!
//! ## Inheritance Rules
//!
//! | Capability              | Inherits? | Rationale                                    |
//! |-------------------------|-----------|----------------------------------------------|
//! | security_provider       | Yes       | Taint tracking must be consistent             |
//! | hook_engine             | Yes       | Parent hooks observe child tool calls          |
//! | skill_registry          | Yes       | Skills are workspace-scoped                    |
//! | permission_checker      | Yes       | Host/session policy must bound inherited runs  |
//! | tool_timeout_ms         | Yes       | Safety limits should propagate                 |
//! | llm_api_timeout_ms      | Yes       | Provider/network deadlines should propagate    |
//! | max_parallel_tasks      | Yes       | Parent fan-out limits should constrain children |
//! | max_execution_time_ms   | Yes       | Prevents runaway child runs                    |
//! | circuit_breaker_threshold | Yes     | LLM failure handling should be consistent      |
//! | duplicate_tool_call_threshold | Yes | Repeated-search guard should be consistent     |
//! | confirmation_manager    | Depends   | Governed by ConfirmationInheritance            |
//! | active skill restrictions | Yes     | Skill allow-lists must remain effective        |
//! | workspace_services      | Yes       | Child tools must operate on the same workspace |
//! | budget_guard            | Yes       | One shared cost ledger spans the whole fan-out |
//! | memory                  | No        | Child has isolated context                     |
//! | queue_config            | No        | Child runs are synchronous within parent       |
//! | planning_mode           | No        | Child tasks are pre-planned by parent          |
//! | context_providers       | No        | Child has its own prompt context                |

use crate::agent::AgentConfig;
use crate::hitl::ConfirmationProvider;
use crate::hooks::HookExecutor;
use crate::permissions::{PermissionChecker, PermissionDecision, PermissionPolicy};
use crate::security::SecurityProvider;
use crate::skills::SkillRegistry;
use std::sync::Arc;

/// Capabilities inherited from a parent session into a child run.
///
/// Fields that are `Some` are inherited from the parent. Fields that are `None`
/// use the child's own defaults (set by `AgentDefinition::apply_to`).
pub struct ChildRunContext {
    pub security_provider: Option<Arc<dyn SecurityProvider>>,
    pub hook_engine: Option<Arc<dyn HookExecutor>>,
    pub skill_registry: Option<Arc<SkillRegistry>>,
    pub permission_checker: Option<Arc<dyn PermissionChecker>>,
    pub permission_policy: Option<PermissionPolicy>,
    pub tool_timeout_ms: Option<u64>,
    pub llm_api_timeout_ms: Option<u64>,
    pub max_parallel_tasks: Option<usize>,
    pub max_execution_time_ms: Option<u64>,
    pub circuit_breaker_threshold: Option<u32>,
    pub duplicate_tool_call_threshold: Option<u32>,
    pub confirmation_manager: Option<Arc<dyn ConfirmationProvider>>,
    pub enforce_active_skill_tool_restrictions: Option<bool>,
    pub workspace_services: Option<Arc<crate::workspace::WorkspaceServices>>,
    /// Shared budget/quota guard. When inherited, every child run feeds the same
    /// guard, so a single ledger can cap an entire delegated fan-out / workflow
    /// rather than each child counting independently.
    pub budget_guard: Option<Arc<dyn crate::budget::BudgetGuard>>,
}

struct DelegatedPermissionChecker {
    child: Arc<dyn PermissionChecker>,
    child_policy: Option<PermissionPolicy>,
    parent: Arc<dyn PermissionChecker>,
    parent_policy: Option<PermissionPolicy>,
}

impl PermissionChecker for DelegatedPermissionChecker {
    fn expose_to_model(&self, tool_name: &str) -> bool {
        if !self.child.expose_to_model(tool_name) {
            return false;
        }

        if self
            .child_policy
            .as_ref()
            .is_some_and(|policy| policy.declares_tool_access(tool_name))
        {
            // A worker's explicitly declared capability may cross a host's
            // ordinary parent-only visibility filter. A serializable parent
            // deny remains authoritative and keeps the tool hidden.
            return self
                .parent_policy
                .as_ref()
                .map(|policy| policy.expose_to_model(tool_name))
                .unwrap_or_else(|| self.parent.expose_to_model(tool_name));
        }

        self.parent.expose_to_model(tool_name)
    }

    fn check(&self, tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        stricter_decision(
            self.child.check(tool_name, args),
            self.parent.check(tool_name, args),
        )
    }
}

const fn stricter_decision(
    left: PermissionDecision,
    right: PermissionDecision,
) -> PermissionDecision {
    match (left, right) {
        (PermissionDecision::Deny, _) | (_, PermissionDecision::Deny) => PermissionDecision::Deny,
        (PermissionDecision::Ask, _) | (_, PermissionDecision::Ask) => PermissionDecision::Ask,
        (PermissionDecision::Allow, PermissionDecision::Allow) => PermissionDecision::Allow,
    }
}

impl ChildRunContext {
    /// Apply inherited capabilities to a child AgentConfig.
    ///
    /// Called after `AgentDefinition::apply_to()` so that parent capabilities
    /// fill remaining gaps without overriding agent-specific settings.
    pub(crate) fn apply_to(&self, config: &mut AgentConfig) {
        if config.security_provider.is_none() {
            config.security_provider = self.security_provider.clone();
        }
        if config.hook_engine.is_none() {
            config.hook_engine = self.hook_engine.clone();
        }
        if config.skill_registry.is_none() {
            config.skill_registry = self.skill_registry.clone();
        }
        match (
            config.permission_checker.take(),
            self.permission_checker.clone(),
        ) {
            (Some(child), Some(parent)) => {
                config.permission_checker = Some(Arc::new(DelegatedPermissionChecker {
                    child,
                    child_policy: config.permission_policy.clone(),
                    parent,
                    parent_policy: self.permission_policy.clone(),
                }));
            }
            (Some(child), None) => config.permission_checker = Some(child),
            (None, Some(parent)) => {
                config.permission_checker = Some(parent);
                config.permission_policy = self.permission_policy.clone();
            }
            (None, None) => {}
        }
        if config.permission_policy.is_none() {
            config.permission_policy = self.permission_policy.clone();
        }
        if config.tool_timeout_ms.is_none() {
            config.tool_timeout_ms = self.tool_timeout_ms;
        }
        if config.llm_api_timeout_ms.is_none() {
            config.llm_api_timeout_ms = self.llm_api_timeout_ms;
        }
        if let Some(max_parallel_tasks) = self.max_parallel_tasks {
            config.max_parallel_tasks = max_parallel_tasks.max(1);
        }
        if config.max_execution_time_ms.is_none() {
            config.max_execution_time_ms = self.max_execution_time_ms;
        }
        if let Some(threshold) = self.circuit_breaker_threshold {
            config.circuit_breaker_threshold = threshold;
        }
        if let Some(threshold) = self.duplicate_tool_call_threshold {
            config.duplicate_tool_call_threshold = threshold.max(1);
        }
        if config.confirmation_manager.is_none() {
            config.confirmation_manager = self.confirmation_manager.clone();
        }
        if let Some(enforce) = self.enforce_active_skill_tool_restrictions {
            config.enforce_active_skill_tool_restrictions = enforce;
        }
        if config.budget_guard.is_none() {
            config.budget_guard = self.budget_guard.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct ParentVisibility {
        policy: PermissionPolicy,
        hide_use_from_primary: bool,
    }

    impl PermissionChecker for ParentVisibility {
        fn expose_to_model(&self, tool_name: &str) -> bool {
            !(self.hide_use_from_primary && tool_name.starts_with("mcp__use_"))
                && self.policy.expose_to_model(tool_name)
        }

        fn check(&self, tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
            self.policy.check(tool_name, args)
        }
    }

    fn delegated(
        child_policy: PermissionPolicy,
        parent_policy: PermissionPolicy,
    ) -> DelegatedPermissionChecker {
        DelegatedPermissionChecker {
            child: Arc::new(child_policy.clone()),
            child_policy: Some(child_policy),
            parent: Arc::new(ParentVisibility {
                policy: parent_policy.clone(),
                hide_use_from_primary: true,
            }),
            parent_policy: Some(parent_policy),
        }
    }

    #[test]
    fn explicitly_scoped_worker_can_see_parent_hidden_tool() {
        let mut child = PermissionPolicy::new().allow("mcp__use_*");
        child.default_decision = PermissionDecision::Deny;
        let parent = PermissionPolicy::new().allow("mcp__use_*");
        let checker = delegated(child, parent);

        assert!(checker.expose_to_model("mcp__use_browser__browser_snapshot"));
        assert_eq!(
            checker.check("mcp__use_browser__browser_snapshot", &serde_json::json!({})),
            PermissionDecision::Allow
        );
    }

    #[test]
    fn unrelated_worker_does_not_inherit_parent_hidden_use_tools() {
        let child = PermissionPolicy::new().allow("read(*)");
        let parent = PermissionPolicy::new().allow("mcp__use_*");
        let checker = delegated(child, parent);

        assert!(!checker.expose_to_model("mcp__use_browser__browser_snapshot"));
    }

    #[test]
    fn parent_deny_remains_authoritative_for_explicit_worker_capability() {
        let mut child = PermissionPolicy::new().allow("mcp__use_*");
        child.default_decision = PermissionDecision::Deny;
        let parent = PermissionPolicy::new().deny("mcp__use_ocr__ocr_extract");
        let checker = delegated(child, parent);

        assert!(!checker.expose_to_model("mcp__use_ocr__ocr_extract"));
        assert_eq!(
            checker.check("mcp__use_ocr__ocr_extract", &serde_json::json!({})),
            PermissionDecision::Deny
        );
    }
}
