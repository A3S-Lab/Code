//! Hook Engine
//!
//! Core engine responsible for managing and executing hooks.

use super::events::{HookEvent, HookEventType};
use super::matcher::HookMatcher;
use super::{HookAction, HookResponse};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;

use crate::error::{read_or_recover, write_or_recover};

/// Hook configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Priority (lower values = higher priority)
    #[serde(default = "default_priority")]
    pub priority: i32,

    /// Timeout in milliseconds
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,

    /// Whether to execute observational hooks asynchronously (fire-and-forget).
    /// Gating hooks always wait for a decision before protected work starts.
    #[serde(default)]
    pub async_execution: bool,

    /// Maximum retry attempts
    #[serde(default)]
    pub max_retries: u32,
}

fn default_priority() -> i32 {
    100
}

fn default_timeout() -> u64 {
    30000
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            priority: default_priority(),
            timeout_ms: default_timeout(),
            async_execution: false,
            max_retries: 0,
        }
    }
}

/// Hook definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    /// Unique hook identifier
    pub id: String,

    /// Event type that triggers this hook
    pub event_type: HookEventType,

    /// Event matcher (optional, None matches all events)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<HookMatcher>,

    /// Hook configuration
    #[serde(default)]
    pub config: HookConfig,
}

impl Hook {
    /// Create a new hook
    pub fn new(id: impl Into<String>, event_type: HookEventType) -> Self {
        Self {
            id: id.into(),
            event_type,
            matcher: None,
            config: HookConfig::default(),
        }
    }

    /// Set the matcher
    pub fn with_matcher(mut self, matcher: HookMatcher) -> Self {
        self.matcher = Some(matcher);
        self
    }

    /// Set the configuration
    pub fn with_config(mut self, config: HookConfig) -> Self {
        self.config = config;
        self
    }

    /// Check if an event matches this hook
    pub fn matches(&self, event: &HookEvent) -> bool {
        // First check event type
        if event.event_type() != self.event_type {
            return false;
        }

        // If there's a matcher, check it
        if let Some(ref matcher) = self.matcher {
            matcher.matches(event)
        } else {
            true
        }
    }
}

/// Hook execution result
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Continue execution (with optional modified data)
    Continue(Option<serde_json::Value>),
    /// Block execution
    Block(String),
    /// Retry after delay (milliseconds)
    Retry(u64),
    /// Skip remaining hooks but continue execution
    Skip,
    /// Escalate to human review
    Escalate {
        reason: String,
        target: Option<String>,
    },
}

impl HookResult {
    /// Create a continue result
    pub fn continue_() -> Self {
        Self::Continue(None)
    }

    /// Create a continue result with modifications
    pub fn continue_with(modified: serde_json::Value) -> Self {
        Self::Continue(Some(modified))
    }

    /// Create a block result
    pub fn block(reason: impl Into<String>) -> Self {
        Self::Block(reason.into())
    }

    /// Create a retry result
    pub fn retry(delay_ms: u64) -> Self {
        Self::Retry(delay_ms)
    }

    /// Create a skip result
    pub fn skip() -> Self {
        Self::Skip
    }

    /// Create an escalate result
    pub fn escalate(reason: impl Into<String>, target: Option<String>) -> Self {
        Self::Escalate {
            reason: reason.into(),
            target,
        }
    }

    /// Check if this is a continue result
    pub fn is_continue(&self) -> bool {
        matches!(self, Self::Continue(_))
    }

    /// Check if this is a block result
    pub fn is_block(&self) -> bool {
        matches!(self, Self::Block(_))
    }
}

/// Rich hook execution outcome used by governance-aware callers.
///
/// [`HookResult`] remains the compatibility surface for existing executors.
/// This outcome additionally preserves the explanation attached to a retry so
/// callers can distinguish a temporary denial from a permanent block.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum HookOutcome {
    /// Continue execution (with optional modified data).
    Continue(Option<serde_json::Value>),
    /// Permanently block the current operation.
    Block { reason: String },
    /// Temporarily block the operation and suggest when it may be retried.
    Retry { reason: String, retry_after_ms: u64 },
    /// Skip remaining hooks but continue execution.
    Skip,
    /// Escalate to human review.
    Escalate {
        reason: String,
        target: Option<String>,
    },
}

impl From<HookResult> for HookOutcome {
    fn from(result: HookResult) -> Self {
        match result {
            HookResult::Continue(modified) => Self::Continue(modified),
            HookResult::Block(reason) => Self::Block { reason },
            HookResult::Retry(retry_after_ms) => Self::Retry {
                reason: "Hook requested a retry".to_string(),
                retry_after_ms,
            },
            HookResult::Skip => Self::Skip,
            HookResult::Escalate { reason, target } => Self::Escalate { reason, target },
        }
    }
}

impl From<HookOutcome> for HookResult {
    fn from(outcome: HookOutcome) -> Self {
        match outcome {
            HookOutcome::Continue(modified) => Self::Continue(modified),
            HookOutcome::Block { reason } => Self::Block(reason),
            HookOutcome::Retry { retry_after_ms, .. } => Self::Retry(retry_after_ms),
            HookOutcome::Skip => Self::Skip,
            HookOutcome::Escalate { reason, target } => Self::Escalate { reason, target },
        }
    }
}

/// Hook handler trait
pub trait HookHandler: Send + Sync {
    /// Handle a hook event
    fn handle(&self, event: &HookEvent) -> HookResponse;

    /// Handle a hook event while preserving callback infrastructure failures.
    ///
    /// Native handlers can rely on the default implementation. SDK bridges
    /// should override this method so language exceptions and callback channel
    /// failures reach the engine instead of being converted to `Continue`.
    fn try_handle(&self, event: &HookEvent) -> Result<HookResponse, String> {
        Ok(self.handle(event))
    }
}

/// Hook executor trait
///
/// Abstracts hook execution, allowing different implementations
/// (e.g., full engine, no-op, test mocks) while keeping agent logic clean.
#[async_trait::async_trait]
pub trait HookExecutor: Send + Sync + std::fmt::Debug {
    /// Fire a hook event and get the result
    async fn fire(&self, event: &HookEvent) -> HookResult;

    /// Fire a hook event while preserving denial context and retryability.
    ///
    /// Existing custom executors can rely on this compatibility projection.
    /// Executors with richer callback responses should override it.
    async fn fire_outcome(&self, event: &HookEvent) -> HookOutcome {
        self.fire(event).await.into()
    }

    /// Observe a product/runtime event emitted by the agent loop.
    ///
    /// Executors that only supervise lifecycle hooks can ignore this.
    async fn record_agent_event(
        &self,
        _event: &crate::agent::AgentEvent,
        _run_id: &str,
        _session_id: &str,
    ) {
    }

    /// Observe explicit run cancellation when cancellation happens outside the
    /// agent loop's normal event stream.
    async fn record_run_cancelled(&self, _run_id: &str, _session_id: &str, _reason: Option<&str>) {}
}

/// Hook engine
pub struct HookEngine {
    /// Registered hooks
    hooks: Arc<RwLock<HashMap<String, Hook>>>,

    /// Hook handlers (registered by SDK)
    handlers: Arc<RwLock<HashMap<String, Arc<dyn HookHandler>>>>,

    /// Event sender channel (for SDK listeners)
    event_tx: Option<mpsc::Sender<HookEvent>>,
}

impl std::fmt::Debug for HookEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookEngine")
            .field("hooks_count", &read_or_recover(&self.hooks).len())
            .field("handlers_count", &read_or_recover(&self.handlers).len())
            .field("has_event_channel", &self.event_tx.is_some())
            .finish()
    }
}

impl Default for HookEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HookEngine {
    /// Create a new hook engine
    pub fn new() -> Self {
        Self {
            hooks: Arc::new(RwLock::new(HashMap::new())),
            handlers: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// Set the event sender channel
    pub fn with_event_channel(mut self, tx: mpsc::Sender<HookEvent>) -> Self {
        self.event_tx = Some(tx);
        self
    }

    /// Register a hook
    pub fn register(&self, hook: Hook) {
        let mut hooks = write_or_recover(&self.hooks);
        hooks.insert(hook.id.clone(), hook);
    }

    /// Unregister a hook
    pub fn unregister(&self, hook_id: &str) -> Option<Hook> {
        let mut hooks = write_or_recover(&self.hooks);
        hooks.remove(hook_id)
    }

    /// Register a handler
    pub fn register_handler(&self, hook_id: &str, handler: Arc<dyn HookHandler>) {
        let mut handlers = write_or_recover(&self.handlers);
        handlers.insert(hook_id.to_string(), handler);
    }

    /// Unregister a handler
    pub fn unregister_handler(&self, hook_id: &str) {
        let mut handlers = write_or_recover(&self.handlers);
        handlers.remove(hook_id);
    }

    /// Get all hooks matching an event (sorted by priority)
    pub fn matching_hooks(&self, event: &HookEvent) -> Vec<Hook> {
        let hooks = read_or_recover(&self.hooks);
        let mut matching: Vec<Hook> = hooks
            .values()
            .filter(|h| h.matches(event))
            .cloned()
            .collect();

        // Sort by priority (lower values = higher priority)
        matching.sort_by_key(|h| h.config.priority);
        matching
    }

    /// Fire an event and get the result
    pub async fn fire(&self, event: &HookEvent) -> HookResult {
        self.fire_outcome(event).await.into()
    }

    /// Fire an event while preserving retry explanations.
    pub async fn fire_outcome(&self, event: &HookEvent) -> HookOutcome {
        // Send event to channel if available
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event.clone()).await;
        }

        // Get matching hooks
        let matching_hooks = self.matching_hooks(event);

        if matching_hooks.is_empty() {
            return HookOutcome::Continue(None);
        }

        // Execute each hook
        let mut last_modified: Option<serde_json::Value> = None;
        for hook in matching_hooks {
            let result = self.execute_hook(&hook, event).await;

            match result {
                HookOutcome::Continue(modified) => {
                    // Track the last modification — continue to subsequent hooks
                    if modified.is_some() {
                        last_modified = modified;
                    }
                }
                block @ HookOutcome::Block { .. } => return block,
                retry @ HookOutcome::Retry { .. } => return retry,
                HookOutcome::Skip => return HookOutcome::Continue(None),
                escalate @ HookOutcome::Escalate { .. } => return escalate,
            }
        }

        HookOutcome::Continue(last_modified)
    }

    /// Execute a single hook
    async fn execute_hook(&self, hook: &Hook, event: &HookEvent) -> HookOutcome {
        let is_gate = Self::is_gating_event(event);

        // Find handler
        let handler = {
            let handlers = read_or_recover(&self.handlers);
            handlers.get(&hook.id).cloned()
        };

        match handler {
            Some(h) => {
                // A gating hook must produce a decision before the protected
                // operation starts. Treat `async_execution` as best-effort only
                // for observational hooks; otherwise a configuration flag could
                // silently bypass a security policy.
                if hook.config.async_execution && !is_gate {
                    let hook_id = hook.id.clone();
                    let event = event.clone();
                    tokio::task::spawn_blocking(move || {
                        let response =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                h.try_handle(&event)
                            }));
                        match response {
                            Ok(Ok(_)) => {}
                            Ok(Err(error)) => tracing::warn!(
                                hook_id = %hook_id,
                                event_type = %event.event_type(),
                                failure = %error,
                                "Asynchronous observational hook handler failed"
                            ),
                            Err(_) => tracing::warn!(
                                hook_id = %hook_id,
                                event_type = %event.event_type(),
                                "Asynchronous observational hook handler panicked"
                            ),
                        }
                    });
                    return HookOutcome::Continue(None);
                }

                let timeout = std::time::Duration::from_millis(hook.config.timeout_ms);
                let event_for_handler = event.clone();
                let mut task =
                    tokio::task::spawn_blocking(move || h.try_handle(&event_for_handler));

                match tokio::time::timeout(timeout, &mut task).await {
                    Ok(Ok(Ok(response))) => self.response_to_outcome(response),
                    Ok(Ok(Err(error))) => self.handler_failure(hook, event, error),
                    Ok(Err(error)) => self.handler_failure(
                        hook,
                        event,
                        format!("handler terminated unexpectedly: {error}"),
                    ),
                    Err(_) => {
                        // `spawn_blocking` work cannot always be cancelled once
                        // running, but aborting prevents a queued callback from
                        // starting. The protected operation remains blocked.
                        task.abort();
                        self.handler_failure(
                            hook,
                            event,
                            format!("handler timed out after {} ms", hook.config.timeout_ms),
                        )
                    }
                }
            }
            // Hooks may be registered only to select events for an SDK listener.
            // Without an actual handler there is no gating policy to fail.
            None => HookOutcome::Continue(None),
        }
    }

    /// Events whose result gates a protected operation.
    ///
    /// These are the hook points whose callers explicitly consume a block
    /// decision before producing tool or planning side effects. Other hook
    /// points are observational or advisory and remain best-effort.
    fn is_gating_event(event: &HookEvent) -> bool {
        matches!(
            event,
            HookEvent::PreToolUse(_)
                | HookEvent::PermissionRequest(_)
                | HookEvent::PreCompact(_)
                | HookEvent::PrePrompt(_)
                | HookEvent::PrePlanning(_)
        )
    }

    /// Map handler infrastructure failures according to the hook point's role.
    fn handler_failure(&self, hook: &Hook, event: &HookEvent, failure: String) -> HookOutcome {
        tracing::warn!(
            hook_id = %hook.id,
            event_type = %event.event_type(),
            failure = %failure,
            gating = Self::is_gating_event(event),
            "Hook handler failed"
        );

        if Self::is_gating_event(event) {
            HookOutcome::Block {
                reason: format!("Required hook '{}' failed: {}", hook.id, failure),
            }
        } else {
            HookOutcome::Continue(None)
        }
    }

    /// Convert HookResponse to the lossless internal outcome.
    fn response_to_outcome(&self, response: HookResponse) -> HookOutcome {
        match response.action {
            HookAction::Continue => HookOutcome::Continue(response.modified),
            HookAction::Block => HookOutcome::Block {
                reason: response.reason.unwrap_or_else(|| "Blocked".to_string()),
            },
            HookAction::Retry => HookOutcome::Retry {
                reason: response
                    .reason
                    .unwrap_or_else(|| "Hook requested a retry".to_string()),
                retry_after_ms: response.retry_delay_ms.unwrap_or(1000),
            },
            HookAction::Skip => HookOutcome::Skip,
        }
    }

    /// Get the number of registered hooks
    pub fn hook_count(&self) -> usize {
        read_or_recover(&self.hooks).len()
    }

    /// Get a hook by ID
    pub fn get_hook(&self, id: &str) -> Option<Hook> {
        read_or_recover(&self.hooks).get(id).cloned()
    }

    /// Get all hooks
    pub fn all_hooks(&self) -> Vec<Hook> {
        read_or_recover(&self.hooks).values().cloned().collect()
    }
}

// Implement HookExecutor trait for HookEngine
#[async_trait]
impl HookExecutor for HookEngine {
    async fn fire(&self, event: &HookEvent) -> HookResult {
        HookEngine::fire(self, event).await
    }

    async fn fire_outcome(&self, event: &HookEvent) -> HookOutcome {
        HookEngine::fire_outcome(self, event).await
    }
}

#[cfg(test)]
#[path = "engine/tests.rs"]
mod tests;
