//! Hooks System for A3S Code Agent
//!
//! Provides a mechanism to intercept and customize agent behavior at various
//! lifecycle points. Hooks can validate, transform, or block operations.
//!
//! ## Hook Events
//!
//! - `PreToolUse`: Before tool execution (can block/modify)
//! - `PostToolUse`: After tool execution (fire-and-forget)
//! - `GenerateStart`: Before LLM generation
//! - `GenerateEnd`: After LLM generation
//! - `SessionStart`: When session is created
//! - `SessionEnd`: When session is destroyed
//! - `PrePlanning`: Before task planning/decomposition (can block)
//! - `PostPlanning`: After a plan is generated or planning fails
//!
//! ## Example
//!
//! ```ignore
//! let engine = HookEngine::new();
//!
//! // Register a hook
//! engine.register(Hook {
//!     id: "security-check".to_string(),
//!     event_type: HookEventType::PreToolUse,
//!     matcher: Some(HookMatcher::tool("Bash")),
//!     config: HookConfig::default(),
//! });
//!
//! // Fire hook and get result
//! let result = engine.fire(HookEvent::PreToolUse { ... }).await;
//! match result {
//!     HookResult::Continue(None) => { /* proceed */ }
//!     HookResult::Continue(Some(modified)) => { /* proceed with modified data */ }
//!     HookResult::Block(reason) => { /* stop execution */ }
//! }
//! ```

mod binding;
mod contract;
mod engine;
mod events;
mod matcher;

pub use binding::HookBinding;
pub use contract::{Hook, HookConfig, HookExecutor, HookHandler, HookOutcome, HookResult};
pub use engine::HookEngine;
pub(crate) use engine::{HookEngineSnapshotError, HookTaskDispatcher, HookTaskFuture};
pub use events::{
    ConfirmationType, ErrorType, GenerateEndEvent, GenerateStartEvent, HookEvent, HookEventType,
    IntentDetectionEvent, OnConfirmationEvent, OnErrorEvent, OnRateLimitEvent, OnSuccessEvent,
    PermissionRequestEvent, PlanningStrategy, PostCompactEvent, PostContextPerceptionEvent,
    PostMemoryRecallEvent, PostPlanningEvent, PostReasoningEvent, PostResponseEvent,
    PostToolUseEvent, PreCompactEvent, PreContextPerceptionEvent, PreMemoryRecallEvent,
    PrePlanningEvent, PrePromptEvent, PreReasoningEvent, PreToolUseEvent, RateLimitType,
    ReasoningType, SessionEndEvent, SessionStartEvent, SkillLoadEvent, SkillUnloadEvent,
    TokenUsageInfo, ToolCallInfo, ToolResultData,
};
pub use matcher::HookMatcher;

/// Hook response action from SDK
#[derive(Debug, Clone, PartialEq)]
pub enum HookAction {
    /// Proceed with execution (optionally with modifications)
    Continue,
    /// Block the operation
    Block,
    /// Retry after a delay
    Retry,
    /// Skip remaining hooks but continue execution
    Skip,
}

/// Response from a hook handler
#[derive(Debug, Clone)]
pub struct HookResponse {
    /// The hook ID this response is for
    pub hook_id: String,
    /// Action to take
    pub action: HookAction,
    /// Reason for blocking or retrying (if action is Block or Retry)
    pub reason: Option<String>,
    /// Modified data (if action is Continue with modifications)
    pub modified: Option<serde_json::Value>,
    /// Retry delay in milliseconds (if action is Retry)
    pub retry_delay_ms: Option<u64>,
}

impl HookResponse {
    /// Create a continue response
    pub fn continue_() -> Self {
        Self {
            hook_id: String::new(),
            action: HookAction::Continue,
            reason: None,
            modified: None,
            retry_delay_ms: None,
        }
    }

    /// Create a continue response with modifications
    pub fn continue_with(modified: serde_json::Value) -> Self {
        Self {
            hook_id: String::new(),
            action: HookAction::Continue,
            reason: None,
            modified: Some(modified),
            retry_delay_ms: None,
        }
    }

    /// Create a block response
    pub fn block(reason: impl Into<String>) -> Self {
        Self {
            hook_id: String::new(),
            action: HookAction::Block,
            reason: Some(reason.into()),
            modified: None,
            retry_delay_ms: None,
        }
    }

    /// Create a retry response
    pub fn retry(delay_ms: u64) -> Self {
        Self {
            hook_id: String::new(),
            action: HookAction::Retry,
            reason: None,
            modified: None,
            retry_delay_ms: Some(delay_ms),
        }
    }

    /// Create a retry response with actionable context.
    pub fn retry_with_reason(reason: impl Into<String>, delay_ms: u64) -> Self {
        Self {
            hook_id: String::new(),
            action: HookAction::Retry,
            reason: Some(reason.into()),
            modified: None,
            retry_delay_ms: Some(delay_ms),
        }
    }

    /// Create a skip response
    pub fn skip() -> Self {
        Self {
            hook_id: String::new(),
            action: HookAction::Skip,
            reason: None,
            modified: None,
            retry_delay_ms: None,
        }
    }

    /// Set the hook ID
    pub fn with_hook_id(mut self, id: impl Into<String>) -> Self {
        self.hook_id = id.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_response_continue() {
        let response = HookResponse::continue_();
        assert_eq!(response.action, HookAction::Continue);
        assert!(response.reason.is_none());
        assert!(response.modified.is_none());
    }

    #[test]
    fn test_hook_response_continue_with_modified() {
        let modified = serde_json::json!({"timeout": 5000});
        let response = HookResponse::continue_with(modified.clone());
        assert_eq!(response.action, HookAction::Continue);
        assert_eq!(response.modified, Some(modified));
    }

    #[test]
    fn test_hook_response_block() {
        let response = HookResponse::block("Dangerous command");
        assert_eq!(response.action, HookAction::Block);
        assert_eq!(response.reason, Some("Dangerous command".to_string()));
    }

    #[test]
    fn test_hook_response_retry() {
        let response = HookResponse::retry(1000);
        assert_eq!(response.action, HookAction::Retry);
        assert_eq!(response.retry_delay_ms, Some(1000));
    }

    #[test]
    fn test_hook_response_retry_with_reason() {
        let response = HookResponse::retry_with_reason("temporary outage", 750);
        assert_eq!(response.action, HookAction::Retry);
        assert_eq!(response.reason.as_deref(), Some("temporary outage"));
        assert_eq!(response.retry_delay_ms, Some(750));
    }

    #[test]
    fn test_hook_response_skip() {
        let response = HookResponse::skip();
        assert_eq!(response.action, HookAction::Skip);
    }

    #[test]
    fn test_hook_response_with_hook_id() {
        let response = HookResponse::continue_().with_hook_id("hook-123");
        assert_eq!(response.hook_id, "hook-123");
    }
}
