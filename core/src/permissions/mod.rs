//! Permission system for tool execution control
//!
//! Implements a declarative permission system similar to Claude Code's permissions.
//! Supports pattern matching with wildcards and three-tier evaluation:
//! 1. Deny rules - checked first, any match = immediate denial
//! 2. Allow rules - checked second, any match = auto-approval
//! 3. Ask rules - checked third, forces confirmation prompt
//! 4. Default behavior - falls back to HITL policy

mod manager;
mod policy;
mod rule;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub use manager::{MatchingRules, PermissionManager};
pub use policy::PermissionPolicy;
pub use rule::PermissionRule;

/// Trait for checking tool execution permissions.
///
/// Implement this trait to provide custom permission logic.
/// The built-in `PermissionPolicy` implements this trait using
/// declarative allow/deny/ask rules with pattern matching.
pub trait PermissionChecker: Send + Sync {
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
