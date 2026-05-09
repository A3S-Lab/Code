use std::collections::HashMap;

use super::{PermissionDecision, PermissionPolicy};

/// Matching rules for debugging
#[derive(Debug, Default, Clone)]
pub struct MatchingRules {
    pub deny: Vec<String>,
    pub allow: Vec<String>,
    pub ask: Vec<String>,
}

impl MatchingRules {
    pub fn is_empty(&self) -> bool {
        self.deny.is_empty() && self.allow.is_empty() && self.ask.is_empty()
    }
}

/// Permission manager that handles per-session permissions
#[derive(Debug)]
pub struct PermissionManager {
    /// Global policy applied to all sessions
    global_policy: PermissionPolicy,
    /// Per-session policy overrides
    session_policies: HashMap<String, PermissionPolicy>,
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionManager {
    /// Create a new permission manager with default global policy
    pub fn new() -> Self {
        Self {
            global_policy: PermissionPolicy::default(),
            session_policies: HashMap::new(),
        }
    }

    /// Create with a custom global policy
    pub fn with_global_policy(policy: PermissionPolicy) -> Self {
        Self {
            global_policy: policy,
            session_policies: HashMap::new(),
        }
    }

    /// Set the global policy
    pub fn set_global_policy(&mut self, policy: PermissionPolicy) {
        self.global_policy = policy;
    }

    /// Get the global policy
    pub fn global_policy(&self) -> &PermissionPolicy {
        &self.global_policy
    }

    /// Set a session-specific policy
    pub fn set_session_policy(&mut self, session_id: &str, policy: PermissionPolicy) {
        self.session_policies.insert(session_id.to_string(), policy);
    }

    /// Remove a session-specific policy
    pub fn remove_session_policy(&mut self, session_id: &str) {
        self.session_policies.remove(session_id);
    }

    /// Get the effective policy for a session
    ///
    /// Session policy takes precedence over global policy for matching rules.
    /// If no session policy exists, uses global policy.
    pub fn get_effective_policy(&self, session_id: &str) -> &PermissionPolicy {
        self.session_policies
            .get(session_id)
            .unwrap_or(&self.global_policy)
    }

    /// Check permission for a tool invocation in a session
    pub fn check(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> PermissionDecision {
        // Get session policy or fall back to global
        let policy = self.get_effective_policy(session_id);

        // Session deny rules
        for rule in &policy.deny {
            if rule.matches(tool_name, args) {
                return PermissionDecision::Deny;
            }
        }

        // Global deny rules (if different policy)
        if !self.session_policies.contains_key(session_id) {
            // Already checked global
        } else {
            for rule in &self.global_policy.deny {
                if rule.matches(tool_name, args) {
                    return PermissionDecision::Deny;
                }
            }
        }

        // Session allow rules
        for rule in &policy.allow {
            if rule.matches(tool_name, args) {
                return PermissionDecision::Allow;
            }
        }

        // Session ask rules
        for rule in &policy.ask {
            if rule.matches(tool_name, args) {
                return PermissionDecision::Ask;
            }
        }

        // Fall back to policy default
        policy.default_decision
    }
}
