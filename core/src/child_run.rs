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
//! | sandbox_handle          | Yes       | Child shell commands must keep the parent boundary |
//! | budget_guard            | Yes       | One shared cost ledger spans the whole fan-out |
//! | memory                  | No        | Child has isolated context                     |
//! | queue_config            | No        | Child runs are synchronous within parent       |
//! | planning_mode           | No        | Child tasks are pre-planned by parent          |
//! | context_providers       | No        | Child has its own prompt context                |

use crate::agent::AgentConfig;
use crate::hitl::{
    ConfirmationPolicy, ConfirmationProvider, ConfirmationResponse, PendingConfirmationInfo,
    TimeoutAction,
};
use crate::hooks::HookExecutor;
use crate::permissions::{PermissionChecker, PermissionDecision, PermissionPolicy};
use crate::security::SecurityProvider;
use crate::skills::SkillRegistry;
use crate::subagent::ConfirmationInheritance;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Capabilities inherited from a parent session into a child run.
///
/// Fields that are `Some` are inherited from the parent. Fields that are `None`
/// use the child's own defaults (set by `AgentDefinition::apply_to`).
#[derive(Clone)]
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
    pub sandbox_handle: Option<Arc<dyn crate::sandbox::BashSandbox>>,
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

pub(crate) fn compose_permission_checker(
    child: Arc<dyn PermissionChecker>,
    child_policy: Option<PermissionPolicy>,
    parent: Option<Arc<dyn PermissionChecker>>,
    parent_policy: Option<PermissionPolicy>,
) -> Arc<dyn PermissionChecker> {
    match parent {
        Some(parent) => Arc::new(DelegatedPermissionChecker {
            child,
            child_policy,
            parent,
            parent_policy,
        }),
        None => child,
    }
}

impl PermissionChecker for DelegatedPermissionChecker {
    fn snapshot_for_run(&self) -> Option<Arc<dyn PermissionChecker>> {
        let child = self
            .child
            .snapshot_for_run()
            .unwrap_or_else(|| Arc::clone(&self.child));
        let parent = self
            .parent
            .snapshot_for_run()
            .unwrap_or_else(|| Arc::clone(&self.parent));
        Some(Arc::new(Self {
            child,
            child_policy: self.child_policy.clone(),
            parent,
            parent_policy: self.parent_policy.clone(),
        }))
    }

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

struct DelegatedConfirmationProvider {
    child_checker: Option<Arc<dyn PermissionChecker>>,
    child_confirmation: Option<Arc<dyn ConfirmationProvider>>,
    parent_checker: Option<Arc<dyn PermissionChecker>>,
    parent_confirmation: Option<Arc<dyn ConfirmationProvider>>,
}

#[derive(Default)]
struct ConfirmationTargets {
    providers: Vec<Arc<dyn ConfirmationProvider>>,
    missing_scopes: Vec<&'static str>,
    permission_denied: bool,
}

impl ConfirmationTargets {
    fn add(&mut self, scope: &'static str, provider: Option<&Arc<dyn ConfirmationProvider>>) {
        let Some(provider) = provider else {
            if !self.missing_scopes.contains(&scope) {
                self.missing_scopes.push(scope);
            }
            return;
        };
        if !self
            .providers
            .iter()
            .any(|existing| Arc::ptr_eq(existing, provider))
        {
            self.providers.push(Arc::clone(provider));
        }
    }

    fn is_available(&self) -> bool {
        !self.permission_denied && self.missing_scopes.is_empty()
    }
}

impl DelegatedConfirmationProvider {
    fn targets(&self, tool_name: &str, args: &serde_json::Value) -> ConfirmationTargets {
        let child_decision = self
            .child_checker
            .as_ref()
            .map(|checker| checker.check(tool_name, args));
        let parent_decision = self
            .parent_checker
            .as_ref()
            .map(|checker| checker.check(tool_name, args));

        let mut targets = ConfirmationTargets::default();
        if matches!(child_decision, Some(PermissionDecision::Deny))
            || matches!(parent_decision, Some(PermissionDecision::Deny))
        {
            targets.permission_denied = true;
            return targets;
        }

        let child_asks = matches!(child_decision, Some(PermissionDecision::Ask));
        let parent_asks = matches!(parent_decision, Some(PermissionDecision::Ask));
        if child_asks {
            targets.add("child", self.child_confirmation.as_ref());
        }
        if parent_asks {
            targets.add("parent", self.parent_confirmation.as_ref());
        }

        if !child_asks && !parent_asks {
            if child_decision.is_none() && parent_decision.is_none() {
                // With no checker at either scope, ToolSafetyGate's fail-closed
                // default is a child-local Ask.
                targets.add("child", self.child_confirmation.as_ref());
            } else {
                // Both explicit permission scopes allowed the invocation, so
                // the gate can only have reached confirmation because the tool
                // itself declared an escalation. The parent host owns that
                // boundary; a child-local auto-approver cannot waive it.
                targets.add("parent", self.parent_confirmation.as_ref());
            }
        }

        targets
    }

    fn all_providers(&self) -> Vec<Arc<dyn ConfirmationProvider>> {
        let mut providers = Vec::new();
        for provider in [
            self.child_confirmation.as_ref(),
            self.parent_confirmation.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !providers
                .iter()
                .any(|existing| Arc::ptr_eq(existing, provider))
            {
                providers.push(Arc::clone(provider));
            }
        }
        providers
    }

    async fn active_providers(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> ConfirmationTargets {
        let mut targets = self.targets(tool_name, args);
        if !targets.is_available() {
            return targets;
        }

        let mut active = Vec::new();
        for provider in targets.providers {
            if provider.requires_confirmation_for(tool_name, args).await {
                active.push(provider);
            }
        }
        targets.providers = active;
        targets
    }

    fn immediate_response(
        approved: bool,
        reason: Option<String>,
    ) -> oneshot::Receiver<ConfirmationResponse> {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(ConfirmationResponse { approved, reason });
        rx
    }
}

#[async_trait::async_trait]
impl ConfirmationProvider for DelegatedConfirmationProvider {
    fn snapshot_for_run(&self) -> Option<Arc<dyn ConfirmationProvider>> {
        let snapshot_checker = |checker: &Arc<dyn PermissionChecker>| {
            checker
                .snapshot_for_run()
                .unwrap_or_else(|| Arc::clone(checker))
        };
        let snapshot_provider = |provider: &Arc<dyn ConfirmationProvider>| {
            provider
                .snapshot_for_run()
                .unwrap_or_else(|| Arc::clone(provider))
        };
        Some(Arc::new(Self {
            child_checker: self.child_checker.as_ref().map(snapshot_checker),
            child_confirmation: self.child_confirmation.as_ref().map(snapshot_provider),
            parent_checker: self.parent_checker.as_ref().map(snapshot_checker),
            parent_confirmation: self.parent_confirmation.as_ref().map(snapshot_provider),
        }))
    }

    async fn requires_confirmation(&self, tool_name: &str) -> bool {
        for provider in self.all_providers() {
            if provider.requires_confirmation(tool_name).await {
                return true;
            }
        }
        self.all_providers().is_empty()
    }

    async fn requires_confirmation_for(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        let targets = self.active_providers(tool_name, args).await;
        !targets.is_available() || !targets.providers.is_empty()
    }

    async fn confirmation_available_for(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        self.targets(tool_name, args).is_available()
    }

    async fn policy_for(&self, tool_name: &str, args: &serde_json::Value) -> ConfirmationPolicy {
        let targets = self.active_providers(tool_name, args).await;
        if !targets.is_available() {
            return ConfirmationPolicy::enabled();
        }
        combine_confirmation_policies(targets.providers).await
    }

    async fn request_confirmation(
        &self,
        tool_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> oneshot::Receiver<ConfirmationResponse> {
        let targets = self.active_providers(tool_name, args).await;
        if !targets.is_available() {
            let scopes = if targets.missing_scopes.is_empty() {
                "delegated permission boundary".to_string()
            } else {
                targets.missing_scopes.join(" and ")
            };
            return Self::immediate_response(
                false,
                Some(format!(
                    "Confirmation is unavailable for the {scopes} scope; execution was denied."
                )),
            );
        }
        if targets.providers.is_empty() {
            return Self::immediate_response(true, None);
        }

        let mut receivers = Vec::with_capacity(targets.providers.len());
        for provider in targets.providers {
            receivers.push(
                provider
                    .request_confirmation(tool_id, tool_name, args)
                    .await,
            );
        }
        if receivers.len() == 1 {
            return receivers.pop().expect("one confirmation receiver");
        }

        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let mut rejection_reasons = Vec::new();
            for receiver in receivers {
                match receiver.await {
                    Ok(response) if response.approved => {}
                    Ok(response) => rejection_reasons.push(
                        response
                            .reason
                            .unwrap_or_else(|| "Confirmation was rejected".to_string()),
                    ),
                    Err(_) => rejection_reasons.push("Confirmation channel closed".to_string()),
                }
            }

            let response = if rejection_reasons.is_empty() {
                ConfirmationResponse {
                    approved: true,
                    reason: None,
                }
            } else {
                ConfirmationResponse {
                    approved: false,
                    reason: Some(rejection_reasons.join("; ")),
                }
            };
            let _ = tx.send(response);
        });
        rx
    }

    async fn confirm(
        &self,
        tool_id: &str,
        approved: bool,
        reason: Option<String>,
    ) -> Result<bool, String> {
        let mut found = false;
        let mut errors = Vec::new();
        for provider in self.all_providers() {
            match provider.confirm(tool_id, approved, reason.clone()).await {
                Ok(provider_found) => found |= provider_found,
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Ok(found)
        } else {
            Err(errors.join("; "))
        }
    }

    async fn policy(&self) -> ConfirmationPolicy {
        combine_confirmation_policies(self.all_providers()).await
    }

    async fn set_policy(&self, policy: ConfirmationPolicy) {
        for provider in self.all_providers() {
            provider.set_policy(policy.clone()).await;
        }
    }

    async fn check_timeouts(&self) -> usize {
        let mut timed_out = 0usize;
        for provider in self.all_providers() {
            timed_out = timed_out.saturating_add(provider.check_timeouts().await);
        }
        timed_out
    }

    async fn cancel(&self, tool_id: &str) -> bool {
        let mut cancelled = false;
        for provider in self.all_providers() {
            cancelled |= provider.cancel(tool_id).await;
        }
        cancelled
    }

    async fn expire(&self, tool_id: &str, action: TimeoutAction) -> bool {
        let mut expired = false;
        for provider in self.all_providers() {
            expired |= provider.expire(tool_id, action).await;
        }
        expired
    }

    async fn cancel_all(&self) -> usize {
        let mut cancelled = 0usize;
        for provider in self.all_providers() {
            cancelled = cancelled.saturating_add(provider.cancel_all().await);
        }
        cancelled
    }

    async fn pending_confirmations(&self) -> Vec<PendingConfirmationInfo> {
        let mut pending = HashMap::<String, PendingConfirmationInfo>::new();
        for provider in self.all_providers() {
            for info in provider.pending_confirmations().await {
                pending
                    .entry(info.tool_id.clone())
                    .and_modify(|existing| {
                        existing.remaining_ms = existing.remaining_ms.min(info.remaining_ms);
                    })
                    .or_insert(info);
            }
        }
        let mut pending: Vec<_> = pending.into_values().collect();
        pending.sort_by(|left, right| left.tool_id.cmp(&right.tool_id));
        pending
    }
}

async fn combine_confirmation_policies(
    providers: Vec<Arc<dyn ConfirmationProvider>>,
) -> ConfirmationPolicy {
    let mut policies = Vec::with_capacity(providers.len());
    for provider in providers {
        policies.push(provider.policy().await);
    }
    let Some(mut combined) = policies.first().cloned() else {
        return ConfirmationPolicy::enabled();
    };
    for policy in &policies[1..] {
        combined.enabled |= policy.enabled;
        combined.default_timeout_ms = combined.default_timeout_ms.min(policy.default_timeout_ms);
        if policy.timeout_action == TimeoutAction::Reject {
            combined.timeout_action = TimeoutAction::Reject;
        }
        combined
            .yolo_lanes
            .retain(|lane| policy.yolo_lanes.contains(lane));
    }
    combined
}

impl ChildRunContext {
    /// Replace session-captured interactive governance with the exact snapshot
    /// owned by the invoking run.
    pub(crate) fn with_run_governance(
        mut self,
        permission_checker: Option<Arc<dyn PermissionChecker>>,
        confirmation_manager: Option<Arc<dyn ConfirmationProvider>>,
    ) -> Self {
        self.permission_checker = permission_checker;
        self.confirmation_manager = confirmation_manager;
        self
    }

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
        let child_permission_checker = config.permission_checker.clone();
        let parent_permission_checker = self.permission_checker.clone();
        let child_confirmation = config.confirmation_manager.clone();
        let parent_confirmation = self.confirmation_manager.clone();

        match (
            child_permission_checker.clone(),
            parent_permission_checker.clone(),
        ) {
            (Some(child), Some(parent)) => {
                config.permission_checker = Some(compose_permission_checker(
                    child,
                    config.permission_policy.clone(),
                    Some(parent),
                    self.permission_policy.clone(),
                ));
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
        let child_confirmation = if child_confirmation.is_none()
            && matches!(
                config.confirmation_inheritance,
                Some(ConfirmationInheritance::InheritParent)
            ) {
            parent_confirmation.clone()
        } else {
            child_confirmation
        };
        config.confirmation_manager = Some(Arc::new(DelegatedConfirmationProvider {
            child_checker: child_permission_checker,
            child_confirmation,
            parent_checker: parent_permission_checker,
            parent_confirmation,
        }));
        if let Some(enforce) = self.enforce_active_skill_tool_restrictions {
            config.enforce_active_skill_tool_restrictions = enforce;
        }
        if config.budget_guard.is_none() {
            config.budget_guard = self.budget_guard.clone();
        }
    }
}

#[cfg(test)]
mod tests;
