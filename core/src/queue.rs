//! Per-session command queue with lane-based priority scheduling
//!
//! Provides session-isolated command queues where each session has its own
//! set of lanes with configurable concurrency limits and priorities.
//!
//! ## External Task Handling
//!
//! Supports pluggable task handlers allowing SDK users to implement custom
//! processing logic for different lanes:
//!
//! - **Internal**: Default, tasks executed within the runtime
//! - **External**: Tasks sent to SDK, wait for callback completion
//! - **Hybrid**: Internal execution with external notification
//!
//! ## Implementation
//!
//! The actual queue implementation is in `SessionLaneQueue` which is backed
//! by a3s-lane with features like DLQ, metrics, retry policies, and rate limiting.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ============================================================================
// Session Lane
// ============================================================================

/// Session lane for queue priority scheduling and HITL auto-approval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionLane {
    /// Control operations (P0) - pause, resume, cancel
    Control,
    /// Query operations (P1) - read, glob, ls, grep
    Query,
    /// Execute operations (P2) - bash, write, edit
    Execute,
    /// Generate operations (P3) - LLM calls
    Generate,
}

impl SessionLane {
    /// Get the priority level (lower = higher priority)
    pub fn priority(&self) -> u8 {
        match self {
            SessionLane::Control => 0,
            SessionLane::Query => 1,
            SessionLane::Execute => 2,
            SessionLane::Generate => 3,
        }
    }

    /// Map a tool name to its lane
    pub fn from_tool_name(tool_name: &str) -> Self {
        match tool_name {
            "read" | "glob" | "ls" | "grep" | "list_files" | "search" => SessionLane::Query,
            "bash" | "write" | "edit" | "delete" | "move" | "copy" | "execute" => {
                SessionLane::Execute
            }
            _ => SessionLane::Execute,
        }
    }
}

// ============================================================================
// Task Handler Configuration
// ============================================================================

/// Task handler mode determines how tasks in a lane are processed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TaskHandlerMode {
    /// Tasks are executed internally within the runtime (default)
    #[default]
    Internal,
    /// Tasks are sent to external handler (SDK), wait for callback
    External,
    /// Tasks are executed internally but also notify external handler
    Hybrid,
}

/// Configuration for a lane's task handler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaneHandlerConfig {
    /// Processing mode
    pub mode: TaskHandlerMode,
    /// Timeout for external processing (ms), default 60000 (60s)
    pub timeout_ms: u64,
}

impl Default for LaneHandlerConfig {
    fn default() -> Self {
        Self {
            mode: TaskHandlerMode::Internal,
            timeout_ms: 60_000,
        }
    }
}

// ============================================================================
// External Task Types
// ============================================================================

/// An external task that needs to be processed by SDK
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTask {
    /// Unique task identifier
    pub task_id: String,
    /// Session this task belongs to
    pub session_id: String,
    /// Lane the task is in
    pub lane: SessionLane,
    /// Type of command (e.g., "bash", "read", "write")
    pub command_type: String,
    /// Task payload as JSON
    pub payload: serde_json::Value,
    /// Timeout in milliseconds
    pub timeout_ms: u64,
    /// When the task was created
    #[serde(skip)]
    pub created_at: Option<Instant>,
}

impl ExternalTask {
    /// Check if this task has timed out
    pub fn is_timed_out(&self) -> bool {
        self.created_at
            .map(|t| t.elapsed() > Duration::from_millis(self.timeout_ms))
            .unwrap_or(false)
    }

    /// Get remaining time until timeout in milliseconds
    pub fn remaining_ms(&self) -> u64 {
        self.created_at
            .map(|t| {
                let elapsed = t.elapsed().as_millis() as u64;
                self.timeout_ms.saturating_sub(elapsed)
            })
            .unwrap_or(self.timeout_ms)
    }
}

/// Result of external task processing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTaskResult {
    /// Whether the task succeeded
    pub success: bool,
    /// Result data (JSON)
    pub result: serde_json::Value,
    /// Error message if failed
    pub error: Option<String>,
}

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for a session command queue
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueueConfig {
    /// Max concurrency for Control lane (P0)
    #[serde(default = "default_control_concurrency")]
    pub control_max_concurrency: usize,
    /// Max concurrency for Query lane (P1)
    #[serde(default = "default_query_concurrency")]
    pub query_max_concurrency: usize,
    /// Max concurrency for Execute lane (P2)
    #[serde(default = "default_execute_concurrency")]
    pub execute_max_concurrency: usize,
    /// Max concurrency for Generate lane (P3)
    #[serde(default = "default_generate_concurrency")]
    pub generate_max_concurrency: usize,
    /// Handler configurations per lane
    #[serde(default)]
    pub lane_handlers: HashMap<SessionLane, LaneHandlerConfig>,

    // ========================================================================
    // a3s-lane v0.4.0 integration features
    // ========================================================================
    /// Enable dead letter queue for failed commands
    #[serde(default)]
    pub enable_dlq: bool,
    /// Max size of dead letter queue (None = use default 1000)
    #[serde(default)]
    pub dlq_max_size: Option<usize>,
    /// Enable metrics collection
    #[serde(default)]
    pub enable_metrics: bool,
    /// Enable queue alerts
    #[serde(default)]
    pub enable_alerts: bool,
    /// Default timeout for commands in milliseconds
    #[serde(default)]
    pub default_timeout_ms: Option<u64>,
    /// Persistent storage path (None = in-memory only)
    #[serde(default)]
    pub storage_path: Option<std::path::PathBuf>,

    // ========================================================================
    // a3s-lane v0.4.0 advanced features
    // ========================================================================
    /// Retry policy configuration
    #[serde(default)]
    pub retry_policy: Option<RetryPolicyConfig>,
    /// Rate limit configuration
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
    /// Priority boost configuration
    #[serde(default)]
    pub priority_boost: Option<PriorityBoostConfig>,
    /// Pressure threshold for emitting pressure/idle events
    #[serde(default)]
    pub pressure_threshold: Option<usize>,
    /// Per-lane timeout overrides in milliseconds
    #[serde(default)]
    pub lane_timeouts: HashMap<SessionLane, u64>,
}

/// Retry policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicyConfig {
    /// Retry strategy: "exponential", "fixed", or "none"
    pub strategy: String,
    /// Maximum number of retries
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Initial delay in milliseconds (for exponential)
    #[serde(default = "default_initial_delay_ms")]
    pub initial_delay_ms: u64,
    /// Fixed delay in milliseconds (for fixed strategy)
    #[serde(default)]
    pub fixed_delay_ms: Option<u64>,
}

fn default_max_retries() -> u32 {
    3
}

fn default_initial_delay_ms() -> u64 {
    100
}

/// Rate limit configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitConfig {
    /// Rate limit type: "per_second", "per_minute", "per_hour", or "unlimited"
    pub limit_type: String,
    /// Maximum number of operations per time period
    #[serde(default)]
    pub max_operations: Option<u64>,
}

/// Priority boost configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriorityBoostConfig {
    /// Boost strategy: "standard", "aggressive", or "disabled"
    pub strategy: String,
    /// Deadline in milliseconds
    #[serde(default)]
    pub deadline_ms: Option<u64>,
}

fn default_control_concurrency() -> usize {
    4
}

fn default_query_concurrency() -> usize {
    12 // Balanced: better stability than 8, good performance (between 8 and 16)
}

fn default_execute_concurrency() -> usize {
    4
}

fn default_generate_concurrency() -> usize {
    2
}

impl Default for SessionQueueConfig {
    fn default() -> Self {
        Self {
            control_max_concurrency: 2,
            query_max_concurrency: 4,
            execute_max_concurrency: 2,
            generate_max_concurrency: 1,
            lane_handlers: HashMap::new(),
            enable_dlq: false,
            dlq_max_size: None,
            enable_metrics: false,
            enable_alerts: false,
            default_timeout_ms: None,
            storage_path: None,
            retry_policy: None,
            rate_limit: None,
            priority_boost: None,
            pressure_threshold: None,
            lane_timeouts: HashMap::new(),
        }
    }
}

impl SessionQueueConfig {
    /// Get max concurrency for a lane
    pub fn max_concurrency(&self, lane: SessionLane) -> usize {
        match lane {
            SessionLane::Control => self.control_max_concurrency,
            SessionLane::Query => self.query_max_concurrency,
            SessionLane::Execute => self.execute_max_concurrency,
            SessionLane::Generate => self.generate_max_concurrency,
        }
    }

    /// Get handler config for a lane (returns default if not configured)
    pub fn handler_config(&self, lane: SessionLane) -> LaneHandlerConfig {
        self.lane_handlers.get(&lane).cloned().unwrap_or_default()
    }

    /// Enable dead letter queue with optional max size
    pub fn with_dlq(mut self, max_size: Option<usize>) -> Self {
        self.enable_dlq = true;
        self.dlq_max_size = max_size;
        self
    }

    /// Enable metrics collection
    pub fn with_metrics(mut self) -> Self {
        self.enable_metrics = true;
        self
    }

    /// Enable queue alerts
    pub fn with_alerts(mut self) -> Self {
        self.enable_alerts = true;
        self
    }

    /// Set default timeout for commands
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.default_timeout_ms = Some(timeout_ms);
        self
    }

    /// Set persistent storage path
    pub fn with_storage(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.storage_path = Some(path.into());
        self
    }

    /// Enable all a3s-lane features with sensible defaults
    pub fn with_lane_features(mut self) -> Self {
        self.enable_dlq = true;
        self.dlq_max_size = Some(1000);
        self.enable_metrics = true;
        self.enable_alerts = true;
        self.default_timeout_ms = Some(60_000);
        self
    }
}

// ============================================================================
// Session Command Trait
// ============================================================================

/// Command to be executed in a session queue
#[async_trait]
pub trait SessionCommand: Send + Sync {
    /// Execute the command
    async fn execute(&self) -> Result<serde_json::Value>;

    /// Get command type (for logging/debugging)
    fn command_type(&self) -> &str;

    /// Get command payload as JSON (for external handling)
    fn payload(&self) -> serde_json::Value {
        serde_json::json!({})
    }
}

// ============================================================================
// Queue Status Types
// ============================================================================

/// Status of a single lane
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaneStatus {
    pub lane: SessionLane,
    pub pending: usize,
    pub active: usize,
    pub max_concurrency: usize,
    pub handler_mode: TaskHandlerMode,
}

/// Statistics for a session queue
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionQueueStats {
    pub total_pending: usize,
    pub total_active: usize,
    pub external_pending: usize,
    pub lanes: HashMap<String, LaneStatus>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_handler_mode_default() {
        let mode = TaskHandlerMode::default();
        assert_eq!(mode, TaskHandlerMode::Internal);
    }

    #[test]
    fn test_lane_handler_config_default() {
        let config = LaneHandlerConfig::default();
        assert_eq!(config.mode, TaskHandlerMode::Internal);
        assert_eq!(config.timeout_ms, 60_000);
    }

    #[test]
    fn test_external_task_timeout() {
        let task = ExternalTask {
            task_id: "test".to_string(),
            session_id: "session".to_string(),
            lane: SessionLane::Query,
            command_type: "read".to_string(),
            payload: serde_json::json!({}),
            timeout_ms: 100,
            created_at: Some(Instant::now()),
        };

        assert!(!task.is_timed_out());
        assert!(task.remaining_ms() <= 100);
    }

    #[test]
    fn test_session_queue_config_default() {
        let config = SessionQueueConfig::default();
        assert_eq!(config.control_max_concurrency, 2);
        assert_eq!(config.query_max_concurrency, 4);
        assert_eq!(config.execute_max_concurrency, 2);
        assert_eq!(config.generate_max_concurrency, 1);
        assert!(!config.enable_dlq);
        assert!(!config.enable_metrics);
        assert!(!config.enable_alerts);
    }

    #[test]
    fn test_session_queue_config_max_concurrency() {
        let config = SessionQueueConfig::default();
        assert_eq!(config.max_concurrency(SessionLane::Control), 2);
        assert_eq!(config.max_concurrency(SessionLane::Query), 4);
        assert_eq!(config.max_concurrency(SessionLane::Execute), 2);
        assert_eq!(config.max_concurrency(SessionLane::Generate), 1);
    }

    #[test]
    fn test_session_queue_config_handler_config() {
        let config = SessionQueueConfig::default();
        let handler = config.handler_config(SessionLane::Execute);
        assert_eq!(handler.mode, TaskHandlerMode::Internal);
        assert_eq!(handler.timeout_ms, 60_000);
    }

    #[test]
    fn test_session_queue_config_builders() {
        let config = SessionQueueConfig::default()
            .with_dlq(Some(500))
            .with_metrics()
            .with_alerts()
            .with_timeout(30_000);

        assert!(config.enable_dlq);
        assert_eq!(config.dlq_max_size, Some(500));
        assert!(config.enable_metrics);
        assert!(config.enable_alerts);
        assert_eq!(config.default_timeout_ms, Some(30_000));
    }

    #[test]
    fn test_session_queue_config_with_lane_features() {
        let config = SessionQueueConfig::default().with_lane_features();

        assert!(config.enable_dlq);
        assert_eq!(config.dlq_max_size, Some(1000));
        assert!(config.enable_metrics);
        assert!(config.enable_alerts);
        assert_eq!(config.default_timeout_ms, Some(60_000));
    }

    #[test]
    fn test_external_task_result() {
        let result = ExternalTaskResult {
            success: true,
            result: serde_json::json!({"output": "hello"}),
            error: None,
        };
        assert!(result.success);
        assert!(result.error.is_none());
    }
}
