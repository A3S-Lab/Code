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
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                 SessionCommandQueue                          │
//! │  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐               │
//! │  │Control │ │ Query  │ │Execute │ │Generate│               │
//! │  │ P0     │ │ P1     │ │ P2     │ │ P3     │               │
//! │  └───┬────┘ └───┬────┘ └───┬────┘ └───┬────┘               │
//! │      └──────────┴──────────┴──────────┘                     │
//! │                        │                                     │
//! │                  ┌─────▼─────┐                               │
//! │                  │  Router   │                               │
//! │                  └─────┬─────┘                               │
//! │         ┌──────────────┼──────────────┐                     │
//! │   ┌─────▼─────┐  ┌─────▼─────┐  ┌─────▼─────┐              │
//! │   │ Internal  │  │ External  │  │  Hybrid   │              │
//! │   │ Handler   │  │ Handler   │  │  Handler  │              │
//! │   └───────────┘  └─────┬─────┘  └───────────┘              │
//! │                        │                                     │
//! │                  ┌─────▼─────┐                               │
//! │                  │    SDK    │  (via gRPC event stream)     │
//! │                  └───────────┘                               │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use crate::agent::AgentEvent;
use crate::hitl::SessionLane;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, oneshot, Mutex, RwLock, Semaphore};

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

impl TaskHandlerMode {
    /// Convert to proto i32 value
    pub fn to_proto_i32(self) -> i32 {
        match self {
            TaskHandlerMode::Internal => 1,
            TaskHandlerMode::External => 2,
            TaskHandlerMode::Hybrid => 3,
        }
    }

    /// Create from proto i32 value
    pub fn from_proto_i32(value: i32) -> Self {
        match value {
            2 => TaskHandlerMode::External,
            3 => TaskHandlerMode::Hybrid,
            _ => TaskHandlerMode::Internal,
        }
    }
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
// Pending External Task
// ============================================================================

/// A pending external task waiting for completion
struct PendingExternalTask {
    task: ExternalTask,
    result_tx: oneshot::Sender<Result<serde_json::Value>>,
}

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for a session command queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionQueueConfig {
    /// Max concurrency for Control lane (P0)
    pub control_max_concurrency: usize,
    /// Max concurrency for Query lane (P1)
    pub query_max_concurrency: usize,
    /// Max concurrency for Execute lane (P2)
    pub execute_max_concurrency: usize,
    /// Max concurrency for Generate lane (P3)
    pub generate_max_concurrency: usize,
    /// Handler configurations per lane
    #[serde(default)]
    pub lane_handlers: HashMap<SessionLane, LaneHandlerConfig>,
}

impl Default for SessionQueueConfig {
    fn default() -> Self {
        Self {
            control_max_concurrency: 2,
            query_max_concurrency: 4,
            execute_max_concurrency: 2,
            generate_max_concurrency: 1,
            lane_handlers: HashMap::new(),
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
// Internal Types
// ============================================================================

/// Command wrapper with result channel
#[allow(dead_code)]
struct CommandWrapper {
    id: String,
    command: Box<dyn SessionCommand>,
    result_tx: oneshot::Sender<Result<serde_json::Value>>,
}

/// Internal lane state
#[allow(dead_code)]
struct LaneState {
    /// Lane identifier
    lane: SessionLane,
    /// Pending commands (FIFO queue)
    pending: VecDeque<CommandWrapper>,
    /// Active command count
    active: usize,
    /// Max concurrency
    max_concurrency: usize,
    /// Semaphore for concurrency control
    semaphore: Arc<Semaphore>,
    /// Handler configuration
    handler_config: LaneHandlerConfig,
}

impl LaneState {
    fn new(lane: SessionLane, max_concurrency: usize, handler_config: LaneHandlerConfig) -> Self {
        Self {
            lane,
            pending: VecDeque::new(),
            active: 0,
            max_concurrency,
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            handler_config,
        }
    }

    fn has_capacity(&self) -> bool {
        self.active < self.max_concurrency
    }

    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

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
// Session Command Queue
// ============================================================================

/// Per-session command queue with lane-based priority and external handler support
pub struct SessionCommandQueue {
    session_id: String,
    lanes: Arc<Mutex<HashMap<SessionLane, LaneState>>>,
    scheduler_running: Arc<Mutex<bool>>,
    /// Pending external tasks waiting for completion
    external_tasks: Arc<RwLock<HashMap<String, PendingExternalTask>>>,
    /// Event broadcaster for external task events
    event_tx: broadcast::Sender<AgentEvent>,
}

impl SessionCommandQueue {
    /// Create a new session command queue
    pub fn new(
        session_id: &str,
        config: SessionQueueConfig,
        event_tx: broadcast::Sender<AgentEvent>,
    ) -> Self {
        let mut lanes = HashMap::new();

        // Create all lanes with their handler configs
        for lane in [
            SessionLane::Control,
            SessionLane::Query,
            SessionLane::Execute,
            SessionLane::Generate,
        ] {
            let handler_config = config.handler_config(lane);
            lanes.insert(
                lane,
                LaneState::new(lane, config.max_concurrency(lane), handler_config),
            );
        }

        Self {
            session_id: session_id.to_string(),
            lanes: Arc::new(Mutex::new(lanes)),
            scheduler_running: Arc::new(Mutex::new(false)),
            external_tasks: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }
    }

    /// Start the queue scheduler
    pub async fn start(&self) {
        let mut running = self.scheduler_running.lock().await;
        if *running {
            return;
        }
        *running = true;
        drop(running);

        let lanes = Arc::clone(&self.lanes);
        let scheduler_running = Arc::clone(&self.scheduler_running);
        let external_tasks = Arc::clone(&self.external_tasks);
        let event_tx = self.event_tx.clone();
        let session_id = self.session_id.clone();

        tokio::spawn(async move {
            loop {
                // Check if scheduler should stop
                {
                    let running = scheduler_running.lock().await;
                    if !*running {
                        break;
                    }
                }

                // Schedule next command
                Self::schedule_next(&lanes, &external_tasks, &event_tx, &session_id).await;

                // Check for timed out external tasks
                Self::check_external_timeouts(&external_tasks).await;

                // Small delay to prevent busy-waiting
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        });
    }

    /// Stop the queue scheduler
    pub async fn stop(&self) {
        let mut running = self.scheduler_running.lock().await;
        *running = false;
    }

    /// Set handler configuration for a lane
    pub async fn set_lane_handler(&self, lane: SessionLane, config: LaneHandlerConfig) {
        let mut lanes = self.lanes.lock().await;
        if let Some(state) = lanes.get_mut(&lane) {
            state.handler_config = config;
        }
    }

    /// Get handler configuration for a lane
    pub async fn get_lane_handler(&self, lane: SessionLane) -> LaneHandlerConfig {
        let lanes = self.lanes.lock().await;
        lanes
            .get(&lane)
            .map(|s| s.handler_config.clone())
            .unwrap_or_default()
    }

    /// Submit a command to a specific lane
    pub async fn submit(
        &self,
        lane: SessionLane,
        command: Box<dyn SessionCommand>,
    ) -> oneshot::Receiver<Result<serde_json::Value>> {
        let (tx, rx) = oneshot::channel();
        let wrapper = CommandWrapper {
            id: uuid::Uuid::new_v4().to_string(),
            command,
            result_tx: tx,
        };

        let mut lanes = self.lanes.lock().await;
        if let Some(lane_state) = lanes.get_mut(&lane) {
            lane_state.pending.push_back(wrapper);
        }

        rx
    }

    /// Submit a command by tool name (auto-determines lane)
    pub async fn submit_by_tool(
        &self,
        tool_name: &str,
        command: Box<dyn SessionCommand>,
    ) -> oneshot::Receiver<Result<serde_json::Value>> {
        let lane = SessionLane::from_tool_name(tool_name);
        self.submit(lane, command).await
    }

    /// Complete an external task with result
    ///
    /// Called by SDK when external processing is done.
    /// Returns true if task was found and completed.
    pub async fn complete_external_task(&self, task_id: &str, result: ExternalTaskResult) -> bool {
        let pending = {
            let mut tasks = self.external_tasks.write().await;
            tasks.remove(task_id)
        };

        if let Some(pending) = pending {
            // Emit completion event
            let _ = self.event_tx.send(AgentEvent::ExternalTaskCompleted {
                task_id: task_id.to_string(),
                session_id: self.session_id.clone(),
                success: result.success,
            });

            // Send result to original caller
            let final_result = if result.success {
                Ok(result.result)
            } else {
                Err(anyhow::anyhow!(result
                    .error
                    .unwrap_or_else(|| "External task failed".to_string())))
            };

            let _ = pending.result_tx.send(final_result);

            // Decrement active count for the lane
            let mut lanes = self.lanes.lock().await;
            if let Some(state) = lanes.get_mut(&pending.task.lane) {
                state.active = state.active.saturating_sub(1);
            }

            true
        } else {
            false
        }
    }

    /// Get queue statistics
    pub async fn stats(&self) -> SessionQueueStats {
        let lanes = self.lanes.lock().await;
        let external_tasks = self.external_tasks.read().await;

        let mut total_pending = 0;
        let mut total_active = 0;
        let mut lane_stats = HashMap::new();

        for (lane, state) in lanes.iter() {
            total_pending += state.pending.len();
            total_active += state.active;

            lane_stats.insert(
                format!("{:?}", lane),
                LaneStatus {
                    lane: *lane,
                    pending: state.pending.len(),
                    active: state.active,
                    max_concurrency: state.max_concurrency,
                    handler_mode: state.handler_config.mode,
                },
            );
        }

        SessionQueueStats {
            total_pending,
            total_active,
            external_pending: external_tasks.len(),
            lanes: lane_stats,
        }
    }

    /// Get pending external tasks
    pub async fn pending_external_tasks(&self) -> Vec<ExternalTask> {
        let tasks = self.external_tasks.read().await;
        tasks.values().map(|p| p.task.clone()).collect()
    }

    /// Get session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Schedule the next command from highest priority lane with capacity
    async fn schedule_next(
        lanes: &Arc<Mutex<HashMap<SessionLane, LaneState>>>,
        external_tasks: &Arc<RwLock<HashMap<String, PendingExternalTask>>>,
        event_tx: &broadcast::Sender<AgentEvent>,
        session_id: &str,
    ) {
        let mut lanes_guard = lanes.lock().await;

        // Collect lanes sorted by priority (lower number = higher priority)
        let mut lane_order: Vec<_> = lanes_guard.keys().copied().collect();
        lane_order.sort_by_key(|l| l.priority());

        for lane in lane_order {
            let lane_state = lanes_guard.get_mut(&lane).unwrap();

            if lane_state.has_capacity() && lane_state.has_pending() {
                if let Some(wrapper) = lane_state.pending.pop_front() {
                    lane_state.active += 1;

                    let handler_mode = lane_state.handler_config.mode;
                    let timeout_ms = lane_state.handler_config.timeout_ms;

                    match handler_mode {
                        TaskHandlerMode::Internal => {
                            // Execute internally
                            let lanes_clone = Arc::clone(lanes);
                            let lane_copy = lane;

                            tokio::spawn(async move {
                                let result = wrapper.command.execute().await;
                                let _ = wrapper.result_tx.send(result);

                                // Mark as completed
                                let mut lanes = lanes_clone.lock().await;
                                if let Some(state) = lanes.get_mut(&lane_copy) {
                                    state.active = state.active.saturating_sub(1);
                                }
                            });
                        }
                        TaskHandlerMode::External => {
                            // Send to external handler
                            let task = ExternalTask {
                                task_id: wrapper.id.clone(),
                                session_id: session_id.to_string(),
                                lane,
                                command_type: wrapper.command.command_type().to_string(),
                                payload: wrapper.command.payload(),
                                timeout_ms,
                                created_at: Some(Instant::now()),
                            };

                            // Store pending task
                            {
                                let mut tasks = external_tasks.write().await;
                                tasks.insert(
                                    wrapper.id.clone(),
                                    PendingExternalTask {
                                        task: task.clone(),
                                        result_tx: wrapper.result_tx,
                                    },
                                );
                            }

                            // Emit external task event
                            let _ = event_tx.send(AgentEvent::ExternalTaskPending {
                                task_id: task.task_id.clone(),
                                session_id: task.session_id.clone(),
                                lane: task.lane,
                                command_type: task.command_type.clone(),
                                payload: task.payload.clone(),
                                timeout_ms: task.timeout_ms,
                            });
                        }
                        TaskHandlerMode::Hybrid => {
                            // Execute internally but also notify external
                            let task = ExternalTask {
                                task_id: wrapper.id.clone(),
                                session_id: session_id.to_string(),
                                lane,
                                command_type: wrapper.command.command_type().to_string(),
                                payload: wrapper.command.payload(),
                                timeout_ms,
                                created_at: Some(Instant::now()),
                            };

                            // Emit notification event (for monitoring/logging)
                            let _ = event_tx.send(AgentEvent::ExternalTaskPending {
                                task_id: task.task_id.clone(),
                                session_id: task.session_id.clone(),
                                lane: task.lane,
                                command_type: task.command_type.clone(),
                                payload: task.payload.clone(),
                                timeout_ms: task.timeout_ms,
                            });

                            // Execute internally
                            let lanes_clone = Arc::clone(lanes);
                            let lane_copy = lane;
                            let event_tx_clone = event_tx.clone();
                            let task_id = task.task_id.clone();
                            let session_id_clone = session_id.to_string();

                            tokio::spawn(async move {
                                let result = wrapper.command.execute().await;
                                let success = result.is_ok();

                                // Notify completion
                                let _ = event_tx_clone.send(AgentEvent::ExternalTaskCompleted {
                                    task_id,
                                    session_id: session_id_clone,
                                    success,
                                });

                                let _ = wrapper.result_tx.send(result);

                                // Mark as completed
                                let mut lanes = lanes_clone.lock().await;
                                if let Some(state) = lanes.get_mut(&lane_copy) {
                                    state.active = state.active.saturating_sub(1);
                                }
                            });
                        }
                    }

                    // Only schedule one command per iteration
                    break;
                }
            }
        }
    }

    /// Check for and handle timed out external tasks
    async fn check_external_timeouts(
        external_tasks: &Arc<RwLock<HashMap<String, PendingExternalTask>>>,
    ) {
        let mut timed_out = Vec::new();

        // Find timed out tasks
        {
            let tasks = external_tasks.read().await;
            for (task_id, pending) in tasks.iter() {
                if pending.task.is_timed_out() {
                    timed_out.push(task_id.clone());
                }
            }
        }

        // Handle timed out tasks
        for task_id in timed_out {
            let pending = {
                let mut tasks = external_tasks.write().await;
                tasks.remove(&task_id)
            };

            if let Some(pending) = pending {
                let _ = pending.result_tx.send(Err(anyhow::anyhow!(
                    "External task timed out after {}ms",
                    pending.task.timeout_ms
                )));
            }
        }
    }
}

/// Map tool name to session lane
pub fn tool_to_lane(tool_name: &str) -> SessionLane {
    SessionLane::from_tool_name(tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Test Commands
    // ========================================================================

    struct TestCommand {
        value: serde_json::Value,
    }

    #[async_trait]
    impl SessionCommand for TestCommand {
        async fn execute(&self) -> Result<serde_json::Value> {
            Ok(self.value.clone())
        }

        fn command_type(&self) -> &str {
            "test"
        }

        fn payload(&self) -> serde_json::Value {
            self.value.clone()
        }
    }

    #[allow(dead_code)]
    struct SlowCommand {
        delay_ms: u64,
        value: serde_json::Value,
    }

    #[async_trait]
    impl SessionCommand for SlowCommand {
        async fn execute(&self) -> Result<serde_json::Value> {
            tokio::time::sleep(tokio::time::Duration::from_millis(self.delay_ms)).await;
            Ok(self.value.clone())
        }

        fn command_type(&self) -> &str {
            "slow"
        }

        fn payload(&self) -> serde_json::Value {
            self.value.clone()
        }
    }

    struct FailingCommand {
        error_msg: String,
    }

    #[async_trait]
    impl SessionCommand for FailingCommand {
        async fn execute(&self) -> Result<serde_json::Value> {
            Err(anyhow::anyhow!("{}", self.error_msg))
        }

        fn command_type(&self) -> &str {
            "failing"
        }
    }

    // ========================================================================
    // Configuration Tests
    // ========================================================================

    #[test]
    fn test_session_queue_config_default() {
        let config = SessionQueueConfig::default();
        assert_eq!(config.control_max_concurrency, 2);
        assert_eq!(config.query_max_concurrency, 4);
        assert_eq!(config.execute_max_concurrency, 2);
        assert_eq!(config.generate_max_concurrency, 1);
        assert!(config.lane_handlers.is_empty());
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
        let mut config = SessionQueueConfig::default();

        // Default handler config
        let handler = config.handler_config(SessionLane::Execute);
        assert_eq!(handler.mode, TaskHandlerMode::Internal);
        assert_eq!(handler.timeout_ms, 60_000);

        // Custom handler config
        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 30_000,
            },
        );

        let handler = config.handler_config(SessionLane::Execute);
        assert_eq!(handler.mode, TaskHandlerMode::External);
        assert_eq!(handler.timeout_ms, 30_000);
    }

    #[test]
    fn test_lane_handler_config_default() {
        let config = LaneHandlerConfig::default();
        assert_eq!(config.mode, TaskHandlerMode::Internal);
        assert_eq!(config.timeout_ms, 60_000);
    }

    #[test]
    fn test_tool_to_lane() {
        assert_eq!(tool_to_lane("read"), SessionLane::Query);
        assert_eq!(tool_to_lane("glob"), SessionLane::Query);
        assert_eq!(tool_to_lane("bash"), SessionLane::Execute);
        assert_eq!(tool_to_lane("write"), SessionLane::Execute);
    }

    #[test]
    fn test_task_handler_mode_conversion() {
        assert_eq!(TaskHandlerMode::Internal.to_proto_i32(), 1);
        assert_eq!(TaskHandlerMode::External.to_proto_i32(), 2);
        assert_eq!(TaskHandlerMode::Hybrid.to_proto_i32(), 3);

        assert_eq!(
            TaskHandlerMode::from_proto_i32(1),
            TaskHandlerMode::Internal
        );
        assert_eq!(
            TaskHandlerMode::from_proto_i32(2),
            TaskHandlerMode::External
        );
        assert_eq!(TaskHandlerMode::from_proto_i32(3), TaskHandlerMode::Hybrid);
        assert_eq!(
            TaskHandlerMode::from_proto_i32(0),
            TaskHandlerMode::Internal
        ); // Unknown defaults to Internal
        assert_eq!(
            TaskHandlerMode::from_proto_i32(99),
            TaskHandlerMode::Internal
        );
    }

    // ========================================================================
    // ExternalTask Tests
    // ========================================================================

    #[test]
    fn test_external_task_timeout_check() {
        let task = ExternalTask {
            task_id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            lane: SessionLane::Execute,
            command_type: "test".to_string(),
            payload: serde_json::json!({}),
            timeout_ms: 100,
            created_at: Some(std::time::Instant::now()),
        };

        // Should not be timed out immediately
        assert!(!task.is_timed_out());
        assert!(task.remaining_ms() > 0);
    }

    #[test]
    fn test_external_task_no_created_at() {
        let task = ExternalTask {
            task_id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            lane: SessionLane::Execute,
            command_type: "test".to_string(),
            payload: serde_json::json!({}),
            timeout_ms: 100,
            created_at: None,
        };

        // Without created_at, should not be timed out
        assert!(!task.is_timed_out());
        assert_eq!(task.remaining_ms(), 100);
    }

    // ========================================================================
    // Basic Queue Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_command_queue() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        assert_eq!(queue.session_id(), "test-session");

        let stats = queue.stats().await;
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
        assert_eq!(stats.external_pending, 0);
        assert_eq!(stats.lanes.len(), 4);
    }

    #[tokio::test]
    async fn test_submit_and_execute_internal() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        queue.start().await;

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"result": "success"}),
        });
        let rx = queue.submit(SessionLane::Query, cmd).await;

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["result"], "success");

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_submit_by_tool() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        queue.start().await;

        // Submit using tool name
        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"tool": "read"}),
        });
        let rx = queue.submit_by_tool("read", cmd).await; // Should go to Query lane

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["tool"], "read");

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_multiple_commands_same_lane() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        queue.start().await;

        // Submit multiple commands to same lane
        let mut receivers = Vec::new();
        for i in 0..5 {
            let cmd = Box::new(TestCommand {
                value: serde_json::json!({"index": i}),
            });
            receivers.push(queue.submit(SessionLane::Query, cmd).await);
        }

        // All should complete
        for (i, rx) in receivers.into_iter().enumerate() {
            let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx)
                .await
                .expect("Timeout")
                .expect("Channel closed");

            assert!(result.is_ok());
            let value = result.unwrap();
            assert_eq!(value["index"], i);
        }

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_commands_across_lanes() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        queue.start().await;

        // Submit to different lanes
        let cmd1 = Box::new(TestCommand {
            value: serde_json::json!({"lane": "query"}),
        });
        let cmd2 = Box::new(TestCommand {
            value: serde_json::json!({"lane": "execute"}),
        });
        let cmd3 = Box::new(TestCommand {
            value: serde_json::json!({"lane": "generate"}),
        });

        let rx1 = queue.submit(SessionLane::Query, cmd1).await;
        let rx2 = queue.submit(SessionLane::Execute, cmd2).await;
        let rx3 = queue.submit(SessionLane::Generate, cmd3).await;

        // All should complete
        for (rx, expected_lane) in [(rx1, "query"), (rx2, "execute"), (rx3, "generate")] {
            let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
                .await
                .expect("Timeout")
                .expect("Channel closed");

            assert!(result.is_ok());
            let value = result.unwrap();
            assert_eq!(value["lane"], expected_lane);
        }

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_failing_command() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        queue.start().await;

        let cmd = Box::new(FailingCommand {
            error_msg: "Test error".to_string(),
        });
        let rx = queue.submit(SessionLane::Execute, cmd).await;

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Test error"));

        queue.stop().await;
    }

    // ========================================================================
    // Lane Handler Tests
    // ========================================================================

    #[tokio::test]
    async fn test_set_lane_handler() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        // Default should be Internal
        let handler = queue.get_lane_handler(SessionLane::Execute).await;
        assert_eq!(handler.mode, TaskHandlerMode::Internal);

        // Set to External
        queue
            .set_lane_handler(
                SessionLane::Execute,
                LaneHandlerConfig {
                    mode: TaskHandlerMode::External,
                    timeout_ms: 30000,
                },
            )
            .await;

        let handler = queue.get_lane_handler(SessionLane::Execute).await;
        assert_eq!(handler.mode, TaskHandlerMode::External);
        assert_eq!(handler.timeout_ms, 30000);

        // Other lanes should still be Internal
        let handler = queue.get_lane_handler(SessionLane::Query).await;
        assert_eq!(handler.mode, TaskHandlerMode::Internal);
    }

    #[tokio::test]
    async fn test_set_lane_handler_all_lanes() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        // Set all lanes to different modes
        for (lane, mode) in [
            (SessionLane::Control, TaskHandlerMode::Internal),
            (SessionLane::Query, TaskHandlerMode::Hybrid),
            (SessionLane::Execute, TaskHandlerMode::External),
            (SessionLane::Generate, TaskHandlerMode::Internal),
        ] {
            queue
                .set_lane_handler(
                    lane,
                    LaneHandlerConfig {
                        mode,
                        timeout_ms: 10000,
                    },
                )
                .await;
        }

        // Verify
        assert_eq!(
            queue.get_lane_handler(SessionLane::Control).await.mode,
            TaskHandlerMode::Internal
        );
        assert_eq!(
            queue.get_lane_handler(SessionLane::Query).await.mode,
            TaskHandlerMode::Hybrid
        );
        assert_eq!(
            queue.get_lane_handler(SessionLane::Execute).await.mode,
            TaskHandlerMode::External
        );
        assert_eq!(
            queue.get_lane_handler(SessionLane::Generate).await.mode,
            TaskHandlerMode::Internal
        );
    }

    // ========================================================================
    // External Handler Mode Tests
    // ========================================================================

    #[tokio::test]
    async fn test_external_handler_mode() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();

        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 5000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"command": "test"}),
        });
        let rx = queue.submit(SessionLane::Execute, cmd).await;

        // Should receive external task event
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event received");

        let task_id = match event {
            AgentEvent::ExternalTaskPending {
                task_id,
                session_id,
                lane,
                command_type,
                payload,
                timeout_ms,
            } => {
                assert_eq!(session_id, "test-session");
                assert_eq!(lane, SessionLane::Execute);
                assert_eq!(command_type, "test");
                assert_eq!(payload["command"], "test");
                assert_eq!(timeout_ms, 5000);
                task_id
            }
            _ => panic!("Expected ExternalTaskPending event"),
        };

        // Complete the external task
        let completed = queue
            .complete_external_task(
                &task_id,
                ExternalTaskResult {
                    success: true,
                    result: serde_json::json!({"external": "result"}),
                    error: None,
                },
            )
            .await;
        assert!(completed);

        // Should receive completion event
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event received");

        match event {
            AgentEvent::ExternalTaskCompleted {
                task_id: completed_id,
                session_id,
                success,
            } => {
                assert_eq!(completed_id, task_id);
                assert_eq!(session_id, "test-session");
                assert!(success);
            }
            _ => panic!("Expected ExternalTaskCompleted event"),
        }

        // Should receive the result
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["external"], "result");

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_external_handler_failure() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();

        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 5000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({}),
        });
        let rx = queue.submit(SessionLane::Execute, cmd).await;

        // Get task_id from event
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event received");

        let task_id = match event {
            AgentEvent::ExternalTaskPending { task_id, .. } => task_id,
            _ => panic!("Expected ExternalTaskPending event"),
        };

        // Complete with failure
        let completed = queue
            .complete_external_task(
                &task_id,
                ExternalTaskResult {
                    success: false,
                    result: serde_json::json!({}),
                    error: Some("External processing failed".to_string()),
                },
            )
            .await;
        assert!(completed);

        // Should receive error result
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("External processing failed"));

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_complete_nonexistent_task() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        let completed = queue
            .complete_external_task(
                "nonexistent-task",
                ExternalTaskResult {
                    success: true,
                    result: serde_json::json!({}),
                    error: None,
                },
            )
            .await;

        assert!(!completed);
    }

    #[tokio::test]
    async fn test_pending_external_tasks() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig {
            // Increase concurrency to allow all 3 tasks to be active simultaneously
            execute_max_concurrency: 3,
            ..Default::default()
        };

        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 60000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        // Submit multiple external tasks
        for i in 0..3 {
            let cmd = Box::new(TestCommand {
                value: serde_json::json!({"index": i}),
            });
            drop(queue.submit(SessionLane::Execute, cmd).await);
        }

        // Wait for events to be processed
        for _ in 0..3 {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv()).await;
        }

        // Check pending external tasks
        let pending = queue.pending_external_tasks().await;
        assert_eq!(pending.len(), 3);

        // Check stats
        let stats = queue.stats().await;
        assert_eq!(stats.external_pending, 3);

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_multiple_external_tasks_completion() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig {
            // Increase concurrency to allow all 3 tasks to be active simultaneously
            execute_max_concurrency: 3,
            ..Default::default()
        };

        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 60000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        // Submit multiple tasks
        let mut task_ids = Vec::new();
        let mut receivers = Vec::new();

        for i in 0..3 {
            let cmd = Box::new(TestCommand {
                value: serde_json::json!({"index": i}),
            });
            receivers.push(queue.submit(SessionLane::Execute, cmd).await);
        }

        // Collect task IDs from events
        for _ in 0..3 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
                .await
                .expect("Timeout")
                .expect("No event received");

            if let AgentEvent::ExternalTaskPending { task_id, .. } = event {
                task_ids.push(task_id);
            }
        }

        // Complete tasks in reverse order
        for (i, task_id) in task_ids.iter().rev().enumerate() {
            queue
                .complete_external_task(
                    task_id,
                    ExternalTaskResult {
                        success: true,
                        result: serde_json::json!({"completed_index": i}),
                        error: None,
                    },
                )
                .await;
        }

        // All receivers should get results
        for rx in receivers {
            let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
                .await
                .expect("Timeout")
                .expect("Channel closed");
            assert!(result.is_ok());
        }

        // No more pending external tasks
        assert_eq!(queue.pending_external_tasks().await.len(), 0);

        queue.stop().await;
    }

    // ========================================================================
    // Hybrid Mode Tests
    // ========================================================================

    #[tokio::test]
    async fn test_hybrid_handler_mode() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();

        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::Hybrid,
                timeout_ms: 5000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"result": "internal"}),
        });
        let rx = queue.submit(SessionLane::Execute, cmd).await;

        // Should receive ExternalTaskPending event (notification)
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event received");

        match event {
            AgentEvent::ExternalTaskPending { command_type, .. } => {
                assert_eq!(command_type, "test");
            }
            _ => panic!("Expected ExternalTaskPending event"),
        }

        // Should receive result from internal execution (no need to call complete_external_task)
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["result"], "internal");

        // Should receive ExternalTaskCompleted event
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event received");

        match event {
            AgentEvent::ExternalTaskCompleted { success, .. } => {
                assert!(success);
            }
            _ => panic!("Expected ExternalTaskCompleted event"),
        }

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_hybrid_mode_failing_command() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();

        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::Hybrid,
                timeout_ms: 5000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        let cmd = Box::new(FailingCommand {
            error_msg: "Hybrid fail".to_string(),
        });
        let rx = queue.submit(SessionLane::Execute, cmd).await;

        // Skip ExternalTaskPending event
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv()).await;

        // Should receive error result from internal execution
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_err());

        // Should receive ExternalTaskCompleted event with success=false
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event received");

        match event {
            AgentEvent::ExternalTaskCompleted { success, .. } => {
                assert!(!success);
            }
            _ => panic!("Expected ExternalTaskCompleted event"),
        }

        queue.stop().await;
    }

    // ========================================================================
    // External Task Timeout Tests
    // ========================================================================

    #[tokio::test]
    async fn test_external_task_timeout() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();

        // Very short timeout for testing
        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 50,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({}),
        });
        let rx = queue.submit(SessionLane::Execute, cmd).await;

        // Skip ExternalTaskPending event
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv()).await;

        // Wait for timeout and let scheduler handle it
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Should receive timeout error
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));

        queue.stop().await;
    }

    // ========================================================================
    // Mixed Mode Tests
    // ========================================================================

    #[tokio::test]
    async fn test_mixed_handler_modes() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();

        // Query: Internal, Execute: External
        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 60000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        // Submit internal command
        let internal_cmd = Box::new(TestCommand {
            value: serde_json::json!({"type": "internal"}),
        });
        let internal_rx = queue.submit(SessionLane::Query, internal_cmd).await;

        // Submit external command
        let external_cmd = Box::new(TestCommand {
            value: serde_json::json!({"type": "external"}),
        });
        let external_rx = queue.submit(SessionLane::Execute, external_cmd).await;

        // Internal should complete immediately
        let internal_result = tokio::time::timeout(std::time::Duration::from_secs(1), internal_rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(internal_result.is_ok());
        assert_eq!(internal_result.unwrap()["type"], "internal");

        // External should wait for completion
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event received");

        let task_id = match event {
            AgentEvent::ExternalTaskPending { task_id, .. } => task_id,
            _ => panic!("Expected ExternalTaskPending event"),
        };

        queue
            .complete_external_task(
                &task_id,
                ExternalTaskResult {
                    success: true,
                    result: serde_json::json!({"type": "external_completed"}),
                    error: None,
                },
            )
            .await;

        let external_result = tokio::time::timeout(std::time::Duration::from_secs(1), external_rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(external_result.is_ok());
        assert_eq!(external_result.unwrap()["type"], "external_completed");

        queue.stop().await;
    }

    // ========================================================================
    // Stats Tests
    // ========================================================================

    #[tokio::test]
    async fn test_stats_with_handler_modes() {
        let (event_tx, _) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();

        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 60000,
            },
        );
        config.lane_handlers.insert(
            SessionLane::Query,
            LaneHandlerConfig {
                mode: TaskHandlerMode::Hybrid,
                timeout_ms: 30000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        let stats = queue.stats().await;

        // Check handler modes in stats
        assert_eq!(
            stats.lanes["Execute"].handler_mode,
            TaskHandlerMode::External
        );
        assert_eq!(stats.lanes["Query"].handler_mode, TaskHandlerMode::Hybrid);
        assert_eq!(
            stats.lanes["Control"].handler_mode,
            TaskHandlerMode::Internal
        );
        assert_eq!(
            stats.lanes["Generate"].handler_mode,
            TaskHandlerMode::Internal
        );
    }

    // ========================================================================
    // Start/Stop Tests
    // ========================================================================

    #[tokio::test]
    async fn test_start_stop_scheduler() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        // Start twice should be idempotent
        queue.start().await;
        queue.start().await;

        // Submit and execute
        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"test": true}),
        });
        let rx = queue.submit(SessionLane::Query, cmd).await;

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(result.is_ok());

        // Stop
        queue.stop().await;

        // Commands submitted after stop may not execute (scheduler is stopped)
        // This is expected behavior
    }

    // ========================================================================
    // Serialization Tests
    // ========================================================================

    #[test]
    fn test_task_handler_mode_serialization() {
        let modes = [
            (TaskHandlerMode::Internal, "\"Internal\""),
            (TaskHandlerMode::External, "\"External\""),
            (TaskHandlerMode::Hybrid, "\"Hybrid\""),
        ];

        for (mode, expected_json) in modes {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, expected_json);
            let parsed: TaskHandlerMode = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, mode);
        }
    }

    #[test]
    fn test_task_handler_mode_default() {
        let mode = TaskHandlerMode::default();
        assert_eq!(mode, TaskHandlerMode::Internal);
    }

    #[test]
    fn test_lane_handler_config_serialization() {
        let config = LaneHandlerConfig {
            mode: TaskHandlerMode::External,
            timeout_ms: 30000,
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: LaneHandlerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.mode, TaskHandlerMode::External);
        assert_eq!(parsed.timeout_ms, 30000);
    }

    #[test]
    fn test_session_queue_config_serialization() {
        let mut config = SessionQueueConfig::default();
        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 5000,
            },
        );

        let json = serde_json::to_string(&config).unwrap();
        let parsed: SessionQueueConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.control_max_concurrency, 2);
        assert_eq!(parsed.query_max_concurrency, 4);
        assert_eq!(parsed.execute_max_concurrency, 2);
        assert_eq!(parsed.generate_max_concurrency, 1);
        assert_eq!(
            parsed
                .lane_handlers
                .get(&SessionLane::Execute)
                .unwrap()
                .mode,
            TaskHandlerMode::External
        );
    }

    #[test]
    fn test_external_task_serialization() {
        let task = ExternalTask {
            task_id: "task-123".to_string(),
            session_id: "session-1".to_string(),
            lane: SessionLane::Execute,
            command_type: "bash".to_string(),
            payload: serde_json::json!({"command": "ls"}),
            timeout_ms: 5000,
            created_at: Some(Instant::now()),
        };

        let json = serde_json::to_string(&task).unwrap();
        let parsed: ExternalTask = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_id, "task-123");
        assert_eq!(parsed.session_id, "session-1");
        assert_eq!(parsed.lane, SessionLane::Execute);
        assert_eq!(parsed.command_type, "bash");
        assert_eq!(parsed.payload["command"], "ls");
        assert_eq!(parsed.timeout_ms, 5000);
        // created_at is skipped in serialization
        assert!(parsed.created_at.is_none());
    }

    #[test]
    fn test_external_task_result_serialization() {
        let result = ExternalTaskResult {
            success: true,
            result: serde_json::json!({"output": "file.txt"}),
            error: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ExternalTaskResult = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.result["output"], "file.txt");
        assert!(parsed.error.is_none());
    }

    #[test]
    fn test_external_task_result_failure_serialization() {
        let result = ExternalTaskResult {
            success: false,
            result: serde_json::json!({}),
            error: Some("Something went wrong".to_string()),
        };

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ExternalTaskResult = serde_json::from_str(&json).unwrap();
        assert!(!parsed.success);
        assert_eq!(parsed.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_lane_status_serialization() {
        let status = LaneStatus {
            lane: SessionLane::Query,
            pending: 3,
            active: 2,
            max_concurrency: 4,
            handler_mode: TaskHandlerMode::Hybrid,
        };

        let json = serde_json::to_string(&status).unwrap();
        let parsed: LaneStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.lane, SessionLane::Query);
        assert_eq!(parsed.pending, 3);
        assert_eq!(parsed.active, 2);
        assert_eq!(parsed.max_concurrency, 4);
        assert_eq!(parsed.handler_mode, TaskHandlerMode::Hybrid);
    }

    #[test]
    fn test_session_queue_stats_default() {
        let stats = SessionQueueStats::default();
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
        assert_eq!(stats.external_pending, 0);
        assert!(stats.lanes.is_empty());
    }

    #[test]
    fn test_session_queue_stats_serialization() {
        let mut lanes = HashMap::new();
        lanes.insert(
            "Execute".to_string(),
            LaneStatus {
                lane: SessionLane::Execute,
                pending: 1,
                active: 2,
                max_concurrency: 4,
                handler_mode: TaskHandlerMode::Internal,
            },
        );

        let stats = SessionQueueStats {
            total_pending: 1,
            total_active: 2,
            external_pending: 0,
            lanes,
        };

        let json = serde_json::to_string(&stats).unwrap();
        let parsed: SessionQueueStats = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.total_pending, 1);
        assert_eq!(parsed.total_active, 2);
        assert_eq!(parsed.external_pending, 0);
        assert_eq!(parsed.lanes["Execute"].pending, 1);
    }

    // ========================================================================
    // ExternalTask Method Tests
    // ========================================================================

    #[tokio::test]
    async fn test_external_task_actually_times_out() {
        let task = ExternalTask {
            task_id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            lane: SessionLane::Execute,
            command_type: "test".to_string(),
            payload: serde_json::json!({}),
            timeout_ms: 30, // 30ms timeout
            created_at: Some(Instant::now()),
        };

        assert!(!task.is_timed_out());
        assert!(task.remaining_ms() > 0);

        // Wait for timeout
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(task.is_timed_out());
        assert_eq!(task.remaining_ms(), 0);
    }

    #[test]
    fn test_external_task_remaining_ms_decreases() {
        let task = ExternalTask {
            task_id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            lane: SessionLane::Execute,
            command_type: "test".to_string(),
            payload: serde_json::json!({}),
            timeout_ms: 10000,
            created_at: Some(Instant::now()),
        };

        let remaining = task.remaining_ms();
        // Should be close to timeout_ms (within a few ms of creation)
        assert!(remaining <= 10000);
        assert!(remaining > 9900);
    }

    // ========================================================================
    // SessionQueueConfig Edge Cases
    // ========================================================================

    #[test]
    fn test_session_queue_config_custom_concurrency() {
        let config = SessionQueueConfig {
            control_max_concurrency: 1,
            query_max_concurrency: 8,
            execute_max_concurrency: 4,
            generate_max_concurrency: 2,
            lane_handlers: HashMap::new(),
        };

        assert_eq!(config.max_concurrency(SessionLane::Control), 1);
        assert_eq!(config.max_concurrency(SessionLane::Query), 8);
        assert_eq!(config.max_concurrency(SessionLane::Execute), 4);
        assert_eq!(config.max_concurrency(SessionLane::Generate), 2);
    }

    #[test]
    fn test_session_queue_config_handler_config_default_fallback() {
        let config = SessionQueueConfig::default();

        // All lanes should return default handler config when not explicitly set
        for lane in [
            SessionLane::Control,
            SessionLane::Query,
            SessionLane::Execute,
            SessionLane::Generate,
        ] {
            let handler = config.handler_config(lane);
            assert_eq!(handler.mode, TaskHandlerMode::Internal);
            assert_eq!(handler.timeout_ms, 60_000);
        }
    }

    #[test]
    fn test_session_queue_config_mixed_handlers() {
        let mut config = SessionQueueConfig::default();
        config.lane_handlers.insert(
            SessionLane::Control,
            LaneHandlerConfig {
                mode: TaskHandlerMode::Internal,
                timeout_ms: 1000,
            },
        );
        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 30000,
            },
        );
        config.lane_handlers.insert(
            SessionLane::Generate,
            LaneHandlerConfig {
                mode: TaskHandlerMode::Hybrid,
                timeout_ms: 120000,
            },
        );

        assert_eq!(
            config.handler_config(SessionLane::Control).mode,
            TaskHandlerMode::Internal
        );
        assert_eq!(config.handler_config(SessionLane::Control).timeout_ms, 1000);

        // Query has no explicit config, should be default
        assert_eq!(
            config.handler_config(SessionLane::Query).mode,
            TaskHandlerMode::Internal
        );
        assert_eq!(config.handler_config(SessionLane::Query).timeout_ms, 60_000);

        assert_eq!(
            config.handler_config(SessionLane::Execute).mode,
            TaskHandlerMode::External
        );
        assert_eq!(
            config.handler_config(SessionLane::Execute).timeout_ms,
            30000
        );

        assert_eq!(
            config.handler_config(SessionLane::Generate).mode,
            TaskHandlerMode::Hybrid
        );
        assert_eq!(
            config.handler_config(SessionLane::Generate).timeout_ms,
            120000
        );
    }

    // ========================================================================
    // tool_to_lane Tests
    // ========================================================================

    #[test]
    fn test_tool_to_lane_query_tools() {
        for tool in ["read", "glob", "ls", "grep", "list_files", "search"] {
            assert_eq!(
                tool_to_lane(tool),
                SessionLane::Query,
                "Tool '{}' should map to Query lane",
                tool
            );
        }
    }

    #[test]
    fn test_tool_to_lane_execute_tools() {
        for tool in ["bash", "write", "edit", "delete", "move", "copy", "execute"] {
            assert_eq!(
                tool_to_lane(tool),
                SessionLane::Execute,
                "Tool '{}' should map to Execute lane",
                tool
            );
        }
    }

    #[test]
    fn test_tool_to_lane_unknown_defaults_to_execute() {
        for tool in ["unknown_tool", "custom_tool", "mcp_tool", ""] {
            assert_eq!(
                tool_to_lane(tool),
                SessionLane::Execute,
                "Unknown tool '{}' should default to Execute lane",
                tool
            );
        }
    }

    // ========================================================================
    // TaskHandlerMode Proto Conversion Edge Cases
    // ========================================================================

    #[test]
    fn test_task_handler_mode_from_proto_negative() {
        // Negative values should default to Internal
        assert_eq!(
            TaskHandlerMode::from_proto_i32(-1),
            TaskHandlerMode::Internal
        );
        assert_eq!(
            TaskHandlerMode::from_proto_i32(-100),
            TaskHandlerMode::Internal
        );
    }

    #[test]
    fn test_task_handler_mode_roundtrip() {
        for mode in [
            TaskHandlerMode::Internal,
            TaskHandlerMode::External,
            TaskHandlerMode::Hybrid,
        ] {
            let proto = mode.to_proto_i32();
            let back = TaskHandlerMode::from_proto_i32(proto);
            assert_eq!(back, mode);
        }
    }

    // ========================================================================
    // Queue Stats Tests
    // ========================================================================

    #[tokio::test]
    async fn test_stats_initial_all_lanes() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        let stats = queue.stats().await;
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
        assert_eq!(stats.external_pending, 0);
        assert_eq!(stats.lanes.len(), 4);

        // Verify each lane has correct defaults
        for (lane_name, expected_concurrency) in [
            ("Control", 2),
            ("Query", 4),
            ("Execute", 2),
            ("Generate", 1),
        ] {
            let lane_stat = &stats.lanes[lane_name];
            assert_eq!(lane_stat.pending, 0);
            assert_eq!(lane_stat.active, 0);
            assert_eq!(lane_stat.max_concurrency, expected_concurrency);
            assert_eq!(lane_stat.handler_mode, TaskHandlerMode::Internal);
        }
    }

    #[tokio::test]
    async fn test_stats_with_pending_commands() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        // Don't start scheduler - commands stay pending
        for _ in 0..3 {
            let cmd = Box::new(TestCommand {
                value: serde_json::json!({}),
            });
            let _rx = queue.submit(SessionLane::Query, cmd).await;
        }
        for _ in 0..2 {
            let cmd = Box::new(TestCommand {
                value: serde_json::json!({}),
            });
            let _rx = queue.submit(SessionLane::Execute, cmd).await;
        }

        let stats = queue.stats().await;
        assert_eq!(stats.total_pending, 5);
        assert_eq!(stats.total_active, 0);
        assert_eq!(stats.lanes["Query"].pending, 3);
        assert_eq!(stats.lanes["Execute"].pending, 2);
        assert_eq!(stats.lanes["Control"].pending, 0);
        assert_eq!(stats.lanes["Generate"].pending, 0);
    }

    #[tokio::test]
    async fn test_stats_with_custom_concurrency() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig {
            control_max_concurrency: 10,
            query_max_concurrency: 20,
            execute_max_concurrency: 5,
            generate_max_concurrency: 3,
            lane_handlers: HashMap::new(),
        };
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        let stats = queue.stats().await;
        assert_eq!(stats.lanes["Control"].max_concurrency, 10);
        assert_eq!(stats.lanes["Query"].max_concurrency, 20);
        assert_eq!(stats.lanes["Execute"].max_concurrency, 5);
        assert_eq!(stats.lanes["Generate"].max_concurrency, 3);
    }

    // ========================================================================
    // Queue Initialization with Config Tests
    // ========================================================================

    #[tokio::test]
    async fn test_queue_with_preconfigured_handlers() {
        let (event_tx, _) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();

        // Pre-configure handlers in config
        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 10000,
            },
        );
        config.lane_handlers.insert(
            SessionLane::Query,
            LaneHandlerConfig {
                mode: TaskHandlerMode::Hybrid,
                timeout_ms: 20000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        // Handlers should be set from config
        let execute_handler = queue.get_lane_handler(SessionLane::Execute).await;
        assert_eq!(execute_handler.mode, TaskHandlerMode::External);
        assert_eq!(execute_handler.timeout_ms, 10000);

        let query_handler = queue.get_lane_handler(SessionLane::Query).await;
        assert_eq!(query_handler.mode, TaskHandlerMode::Hybrid);
        assert_eq!(query_handler.timeout_ms, 20000);

        // Unconfigured lanes should have default handler
        let control_handler = queue.get_lane_handler(SessionLane::Control).await;
        assert_eq!(control_handler.mode, TaskHandlerMode::Internal);
        assert_eq!(control_handler.timeout_ms, 60_000);
    }

    // ========================================================================
    // Session ID Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_id_preserved() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();

        let queue1 = SessionCommandQueue::new("session-abc", config.clone(), event_tx.clone());
        let queue2 = SessionCommandQueue::new("session-xyz", config, event_tx);

        assert_eq!(queue1.session_id(), "session-abc");
        assert_eq!(queue2.session_id(), "session-xyz");
    }

    // ========================================================================
    // External Task Failure Without Error Message
    // ========================================================================

    #[tokio::test]
    async fn test_external_handler_failure_no_error_message() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();
        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 5000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({}),
        });
        let rx = queue.submit(SessionLane::Execute, cmd).await;

        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event");

        let task_id = match event {
            AgentEvent::ExternalTaskPending { task_id, .. } => task_id,
            _ => panic!("Expected ExternalTaskPending"),
        };

        // Complete with failure but no explicit error message
        queue
            .complete_external_task(
                &task_id,
                ExternalTaskResult {
                    success: false,
                    result: serde_json::json!({}),
                    error: None,
                },
            )
            .await;

        let result = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_err());
        // Should use default error message
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("External task failed"));

        queue.stop().await;
    }

    // ========================================================================
    // Concurrency Limiting Tests
    // ========================================================================

    #[tokio::test]
    async fn test_concurrency_limit_respected() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig {
            // Only allow 1 concurrent task in Generate lane
            generate_max_concurrency: 1,
            ..Default::default()
        };

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        // Submit 3 slow commands to Generate lane (concurrency = 1)
        let mut receivers = Vec::new();
        for i in 0..3 {
            let cmd = Box::new(SlowCommand {
                delay_ms: 30,
                value: serde_json::json!({"index": i}),
            });
            receivers.push(queue.submit(SessionLane::Generate, cmd).await);
        }

        // All should eventually complete (sequentially due to concurrency limit)
        for rx in receivers {
            let result = tokio::time::timeout(Duration::from_secs(3), rx)
                .await
                .expect("Timeout")
                .expect("Channel closed");
            assert!(result.is_ok());
        }

        queue.stop().await;
    }

    // ========================================================================
    // Priority Scheduling Tests
    // ========================================================================

    #[tokio::test]
    async fn test_control_lane_priority_over_generate() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig {
            // Only 1 concurrency per lane to force ordering
            control_max_concurrency: 1,
            query_max_concurrency: 1,
            execute_max_concurrency: 1,
            generate_max_concurrency: 1,
            lane_handlers: HashMap::new(),
        };

        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        // Submit commands to both lanes BEFORE starting scheduler
        // Control (P0) should be scheduled before Generate (P3)
        let gen_cmd = Box::new(TestCommand {
            value: serde_json::json!({"lane": "generate"}),
        });
        let gen_rx = queue.submit(SessionLane::Generate, gen_cmd).await;

        let ctrl_cmd = Box::new(TestCommand {
            value: serde_json::json!({"lane": "control"}),
        });
        let ctrl_rx = queue.submit(SessionLane::Control, ctrl_cmd).await;

        // Now start scheduler
        queue.start().await;

        // Both should complete successfully
        let ctrl_result = tokio::time::timeout(Duration::from_secs(1), ctrl_rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(ctrl_result.is_ok());
        assert_eq!(ctrl_result.unwrap()["lane"], "control");

        let gen_result = tokio::time::timeout(Duration::from_secs(1), gen_rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(gen_result.is_ok());
        assert_eq!(gen_result.unwrap()["lane"], "generate");

        queue.stop().await;
    }

    // ========================================================================
    // Submit After Stop Tests
    // ========================================================================

    #[tokio::test]
    async fn test_submit_before_start() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        // Submit without starting - command should remain pending
        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"test": true}),
        });
        let _rx = queue.submit(SessionLane::Query, cmd).await;

        let stats = queue.stats().await;
        assert_eq!(stats.total_pending, 1);
        assert_eq!(stats.total_active, 0);

        // Now start and it should drain
        queue.start().await;

        // Wait for processing
        tokio::time::sleep(Duration::from_millis(100)).await;

        let stats = queue.stats().await;
        assert_eq!(stats.total_pending, 0);

        queue.stop().await;
    }

    // ========================================================================
    // External Task Active Count Tests
    // ========================================================================

    #[tokio::test]
    async fn test_external_task_active_count_decrements_on_complete() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();
        config.lane_handlers.insert(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 60000,
            },
        );

        let queue = SessionCommandQueue::new("test-session", config, event_tx);
        queue.start().await;

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({}),
        });
        let _rx = queue.submit(SessionLane::Execute, cmd).await;

        // Wait for external task event
        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event");

        let task_id = match event {
            AgentEvent::ExternalTaskPending { task_id, .. } => task_id,
            _ => panic!("Expected ExternalTaskPending"),
        };

        // Active count should be 1
        let stats = queue.stats().await;
        assert_eq!(stats.lanes["Execute"].active, 1);

        // Complete the task
        queue
            .complete_external_task(
                &task_id,
                ExternalTaskResult {
                    success: true,
                    result: serde_json::json!({}),
                    error: None,
                },
            )
            .await;

        // Active count should be back to 0
        let stats = queue.stats().await;
        assert_eq!(stats.lanes["Execute"].active, 0);
        assert_eq!(stats.external_pending, 0);

        queue.stop().await;
    }

    // ========================================================================
    // Test Command Trait Implementations
    // ========================================================================

    #[tokio::test]
    async fn test_test_command_type_and_payload() {
        let cmd = TestCommand {
            value: serde_json::json!({"key": "val"}),
        };

        assert_eq!(cmd.command_type(), "test");
        assert_eq!(cmd.payload(), serde_json::json!({"key": "val"}));

        let result = cmd.execute().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!({"key": "val"}));
    }

    #[tokio::test]
    async fn test_failing_command_type() {
        let cmd = FailingCommand {
            error_msg: "boom".to_string(),
        };

        assert_eq!(cmd.command_type(), "failing");
        assert_eq!(cmd.payload(), serde_json::json!({})); // Default payload

        let result = cmd.execute().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("boom"));
    }

    #[tokio::test]
    async fn test_slow_command_delay() {
        let cmd = SlowCommand {
            delay_ms: 30,
            value: serde_json::json!({"slow": true}),
        };

        assert_eq!(cmd.command_type(), "slow");
        assert_eq!(cmd.payload(), serde_json::json!({"slow": true}));

        let start = Instant::now();
        let result = cmd.execute().await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!({"slow": true}));
        assert!(elapsed >= Duration::from_millis(25)); // Allow small jitter
    }

    // ========================================================================
    // Multiple Stop / Restart Tests
    // ========================================================================

    #[tokio::test]
    async fn test_stop_idempotent() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        queue.start().await;

        // Stop multiple times should not panic
        queue.stop().await;
        queue.stop().await;
        queue.stop().await;
    }

    #[tokio::test]
    async fn test_restart_scheduler() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        // Start → Stop → Start → execute command
        queue.start().await;
        queue.stop().await;
        // Small delay to let scheduler loop exit
        tokio::time::sleep(Duration::from_millis(50)).await;
        queue.start().await;

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"restarted": true}),
        });
        let rx = queue.submit(SessionLane::Query, cmd).await;

        let result = tokio::time::timeout(Duration::from_secs(1), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["restarted"], true);

        queue.stop().await;
    }

    // ========================================================================
    // External Task Events Test
    // ========================================================================

    #[tokio::test]
    async fn test_external_task_event_payload() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut config = SessionQueueConfig::default();
        config.lane_handlers.insert(
            SessionLane::Query,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 15000,
            },
        );

        let queue = SessionCommandQueue::new("my-session", config, event_tx);
        queue.start().await;

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"data": "hello"}),
        });
        let _rx = queue.submit(SessionLane::Query, cmd).await;

        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event");

        match event {
            AgentEvent::ExternalTaskPending {
                task_id,
                session_id,
                lane,
                command_type,
                payload,
                timeout_ms,
            } => {
                assert!(!task_id.is_empty());
                assert_eq!(session_id, "my-session");
                assert_eq!(lane, SessionLane::Query);
                assert_eq!(command_type, "test");
                assert_eq!(payload["data"], "hello");
                assert_eq!(timeout_ms, 15000);
            }
            _ => panic!("Expected ExternalTaskPending"),
        }

        queue.stop().await;
    }

    // ========================================================================
    // Handler Mode Change While Commands Pending
    // ========================================================================

    #[tokio::test]
    async fn test_handler_mode_change_affects_new_commands() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        queue.start().await;

        // First command in Internal mode → executes internally
        let cmd1 = Box::new(TestCommand {
            value: serde_json::json!({"phase": 1}),
        });
        let rx1 = queue.submit(SessionLane::Execute, cmd1).await;
        let result1 = tokio::time::timeout(Duration::from_secs(1), rx1)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(result1.is_ok());

        // Switch to External mode
        queue
            .set_lane_handler(
                SessionLane::Execute,
                LaneHandlerConfig {
                    mode: TaskHandlerMode::External,
                    timeout_ms: 5000,
                },
            )
            .await;

        // Second command should go through External path
        let cmd2 = Box::new(TestCommand {
            value: serde_json::json!({"phase": 2}),
        });
        let rx2 = queue.submit(SessionLane::Execute, cmd2).await;

        // Should get ExternalTaskPending event
        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event");

        let task_id = match event {
            AgentEvent::ExternalTaskPending { task_id, .. } => task_id,
            _ => panic!("Expected ExternalTaskPending"),
        };

        // Complete externally
        queue
            .complete_external_task(
                &task_id,
                ExternalTaskResult {
                    success: true,
                    result: serde_json::json!({"phase": "2_done"}),
                    error: None,
                },
            )
            .await;

        let result2 = tokio::time::timeout(Duration::from_secs(1), rx2)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap()["phase"], "2_done");

        // Switch back to Internal
        queue
            .set_lane_handler(
                SessionLane::Execute,
                LaneHandlerConfig {
                    mode: TaskHandlerMode::Internal,
                    timeout_ms: 60000,
                },
            )
            .await;

        // Third command should execute internally again
        let cmd3 = Box::new(TestCommand {
            value: serde_json::json!({"phase": 3}),
        });
        let rx3 = queue.submit(SessionLane::Execute, cmd3).await;
        let result3 = tokio::time::timeout(Duration::from_secs(1), rx3)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(result3.is_ok());
        assert_eq!(result3.unwrap()["phase"], 3);

        queue.stop().await;
    }

    // ========================================================================
    // Dynamic Handler Mode Change Tests (existing test kept, adding this section header)
    // ========================================================================

    #[tokio::test]
    async fn test_dynamic_handler_mode_change() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionCommandQueue::new("test-session", config, event_tx);

        queue.start().await;

        // First command with Internal mode
        let cmd1 = Box::new(TestCommand {
            value: serde_json::json!({"mode": "internal"}),
        });
        let rx1 = queue.submit(SessionLane::Execute, cmd1).await;

        let result1 = tokio::time::timeout(std::time::Duration::from_secs(1), rx1)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap()["mode"], "internal");

        // Change to External mode
        queue
            .set_lane_handler(
                SessionLane::Execute,
                LaneHandlerConfig {
                    mode: TaskHandlerMode::External,
                    timeout_ms: 60000,
                },
            )
            .await;

        // Second command with External mode
        let cmd2 = Box::new(TestCommand {
            value: serde_json::json!({"mode": "external"}),
        });
        let rx2 = queue.submit(SessionLane::Execute, cmd2).await;

        // Should receive external task event
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("Timeout")
            .expect("No event received");

        let task_id = match event {
            AgentEvent::ExternalTaskPending { task_id, .. } => task_id,
            _ => panic!("Expected ExternalTaskPending event"),
        };

        // Complete externally
        queue
            .complete_external_task(
                &task_id,
                ExternalTaskResult {
                    success: true,
                    result: serde_json::json!({"mode": "external_completed"}),
                    error: None,
                },
            )
            .await;

        let result2 = tokio::time::timeout(std::time::Duration::from_secs(1), rx2)
            .await
            .expect("Timeout")
            .expect("Channel closed");
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap()["mode"], "external_completed");

        queue.stop().await;
    }
}
