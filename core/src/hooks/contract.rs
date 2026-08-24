use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{HookEvent, HookEventType, HookMatcher, HookResponse};

/// Hook configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    /// Priority (lower values = higher priority).
    #[serde(default = "default_priority")]
    pub priority: i32,

    /// Timeout in milliseconds.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,

    /// Whether to execute observational hooks asynchronously (fire-and-forget).
    /// Gating hooks always wait for a decision before protected work starts.
    #[serde(default)]
    pub async_execution: bool,

    /// Maximum retry attempts.
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

/// Hook definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    /// Unique hook identifier.
    pub id: String,

    /// Event type that triggers this hook.
    pub event_type: HookEventType,

    /// Event matcher (optional, `None` matches all events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matcher: Option<HookMatcher>,

    /// Hook configuration.
    #[serde(default)]
    pub config: HookConfig,
}

impl Hook {
    pub fn new(id: impl Into<String>, event_type: HookEventType) -> Self {
        Self {
            id: id.into(),
            event_type,
            matcher: None,
            config: HookConfig::default(),
        }
    }

    pub fn with_matcher(mut self, matcher: HookMatcher) -> Self {
        self.matcher = Some(matcher);
        self
    }

    pub fn with_config(mut self, config: HookConfig) -> Self {
        self.config = config;
        self
    }

    pub fn matches(&self, event: &HookEvent) -> bool {
        if event.event_type() != self.event_type {
            return false;
        }
        self.matcher
            .as_ref()
            .is_none_or(|matcher| matcher.matches(event))
    }
}

/// Hook execution result.
#[derive(Debug, Clone)]
pub enum HookResult {
    Continue(Option<serde_json::Value>),
    Block(String),
    Retry(u64),
    Skip,
    Escalate {
        reason: String,
        target: Option<String>,
    },
}

impl HookResult {
    pub fn continue_() -> Self {
        Self::Continue(None)
    }

    pub fn continue_with(modified: serde_json::Value) -> Self {
        Self::Continue(Some(modified))
    }

    pub fn block(reason: impl Into<String>) -> Self {
        Self::Block(reason.into())
    }

    pub fn retry(delay_ms: u64) -> Self {
        Self::Retry(delay_ms)
    }

    pub fn skip() -> Self {
        Self::Skip
    }

    pub fn escalate(reason: impl Into<String>, target: Option<String>) -> Self {
        Self::Escalate {
            reason: reason.into(),
            target,
        }
    }

    pub fn is_continue(&self) -> bool {
        matches!(self, Self::Continue(_))
    }

    pub fn is_block(&self) -> bool {
        matches!(self, Self::Block(_))
    }
}

/// Rich Hook execution outcome used by governance-aware callers.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum HookOutcome {
    Continue(Option<serde_json::Value>),
    Block {
        reason: String,
    },
    Retry {
        reason: String,
        retry_after_ms: u64,
    },
    Skip,
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

pub trait HookHandler: Send + Sync {
    fn handle(&self, event: &HookEvent) -> HookResponse;

    /// SDK bridges override this method so callback infrastructure failures
    /// reach the engine instead of being converted to `Continue`.
    fn try_handle(&self, event: &HookEvent) -> Result<HookResponse, String> {
        Ok(self.handle(event))
    }
}

/// Execution seam for in-process, SDK, and host-provided Hook runtimes.
#[async_trait::async_trait]
pub trait HookExecutor: Send + Sync + std::fmt::Debug + 'static {
    async fn fire(&self, event: &HookEvent) -> HookResult;

    async fn fire_outcome(&self, event: &HookEvent) -> HookOutcome {
        self.fire(event).await.into()
    }

    /// Dispatch an observational event without delaying protected work.
    /// Run-scoped executors override this to register the work with their
    /// capability supervisor before returning.
    fn dispatch_observational(self: Arc<Self>, event: HookEvent) {
        tokio::spawn(async move {
            let _ = self.fire(&event).await;
        });
    }

    async fn record_agent_event(
        &self,
        _event: &crate::agent::AgentEvent,
        _run_id: &str,
        _session_id: &str,
    ) {
    }

    async fn record_run_cancelled(&self, _run_id: &str, _session_id: &str, _reason: Option<&str>) {}
}
