//! Permission system for tool execution control
//!
//! Implements a declarative permission system similar to Claude Code's permissions.
//! Supports pattern matching with wildcards and three-tier evaluation:
//! 1. Deny rules - checked first, any match = immediate denial
//! 2. Allow rules - checked second, any match = auto-approval
//! 3. Ask rules - checked third, forces confirmation prompt
//! 4. Default behavior - falls back to HITL policy

mod interactive;
mod manager;
mod policy;
mod risk;
mod rule;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub use interactive::{InteractiveApprovalMode, InteractiveToolGuardrail};
pub use manager::{MatchingRules, PermissionManager};
pub use policy::PermissionPolicy;
pub use risk::{
    EnvironmentSensitivity, ImpactScope, OperationTarget, Reversibility, ToolRiskAction,
    ToolRiskAssessment, ToolRiskDimensions, ToolRiskLevel, ToolRiskReason, ToolRiskType,
};
pub use rule::PermissionRule;

/// Trait for checking tool execution permissions.
///
/// Implement this trait to provide custom permission logic.
/// The built-in `PermissionPolicy` implements this trait using
/// declarative allow/deny/ask rules with pattern matching.
pub trait PermissionChecker: Send + Sync {
    /// Freeze any mutable host policy for one agent run.
    ///
    /// Stateless checkers can keep the default and will be shared as-is.
    /// Interactive hosts whose policy changes between turns should return an
    /// immutable checker here so an in-flight or background child cannot gain
    /// or lose authority when the next turn selects a different mode.
    fn snapshot_for_run(&self) -> Option<Arc<dyn PermissionChecker>> {
        None
    }

    /// Whether a tool definition should be exposed to the model.
    ///
    /// This controls model-visible capabilities only. [`Self::check`] remains
    /// the authoritative execution-time decision for any tool invocation.
    /// Existing checkers expose every tool unless they explicitly override
    /// this method.
    fn expose_to_model(&self, _tool_name: &str) -> bool {
        true
    }

    /// Check whether a tool invocation is allowed, denied, or requires confirmation.
    fn check(&self, tool_name: &str, args: &serde_json::Value) -> PermissionDecision;
}

/// Permission decision result
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    /// Automatically allow without user confirmation
    Allow,
    /// Deny execution
    Deny,
    /// Ask user for confirmation
    Ask,
}
