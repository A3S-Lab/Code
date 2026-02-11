//! Session Lane Queue - a3s-lane backed command queue
//!
//! Provides per-session command queues with lane-based priority scheduling,
//! backed by a3s-lane for advanced features.
//!
//! ## Features
//!
//! - Per-session QueueManager with independent queue, metrics, DLQ
//! - Preserves Internal/External/Hybrid task handler modes
//! - Dead letter queue for failed commands
//! - Metrics collection and alerts
//! - Retry policies and rate limiting

use crate::agent::AgentEvent;
use crate::hitl::SessionLane;
use crate::lane_integration::{EnhancedQueueConfig, EnhancedQueueManager};
use crate::queue::{
    ExternalTask, ExternalTaskResult, LaneHandlerConfig, SessionCommand, SessionQueueConfig,
    TaskHandlerMode,
};
use a3s_lane::{
    Command as LaneCommand, DeadLetter, LaneError, MetricsSnapshot, Result as LaneResult,
};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, oneshot, RwLock};

// ============================================================================
// Pending External Task
// ============================================================================

/// A pending external task waiting for completion
struct PendingExternalTask {
    task: ExternalTask,
    result_tx: oneshot::Sender<Result<Value>>,
}

// ============================================================================
// Session Command Adapter
// ============================================================================

/// Adapter that wraps SessionCommand for a3s-lane execution
pub struct SessionCommandAdapter {
    inner: Box<dyn SessionCommand>,
    task_id: String,
    handler_mode: TaskHandlerMode,
    session_id: String,
    lane: SessionLane,
    timeout_ms: u64,
    external_tasks: Arc<RwLock<HashMap<String, PendingExternalTask>>>,
    event_tx: broadcast::Sender<AgentEvent>,
}

impl SessionCommandAdapter {
    #[allow(clippy::too_many_arguments)]
    fn new(
        inner: Box<dyn SessionCommand>,
        task_id: String,
        handler_mode: TaskHandlerMode,
        session_id: String,
        lane: SessionLane,
        timeout_ms: u64,
        external_tasks: Arc<RwLock<HashMap<String, PendingExternalTask>>>,
        event_tx: broadcast::Sender<AgentEvent>,
    ) -> Self {
        Self {
            inner,
            task_id,
            handler_mode,
            session_id,
            lane,
            timeout_ms,
            external_tasks,
            event_tx,
        }
    }

    /// Register as external task and wait for completion
    async fn register_and_wait(&self) -> LaneResult<Value> {
        let (tx, rx) = oneshot::channel();

        let task = ExternalTask {
            task_id: self.task_id.clone(),
            session_id: self.session_id.clone(),
            lane: self.lane,
            command_type: self.inner.command_type().to_string(),
            payload: self.inner.payload(),
            timeout_ms: self.timeout_ms,
            created_at: Some(Instant::now()),
        };

        // Store pending task
        {
            let mut tasks = self.external_tasks.write().await;
            tasks.insert(
                self.task_id.clone(),
                PendingExternalTask {
                    task: task.clone(),
                    result_tx: tx,
                },
            );
        }

        // Emit external task event
        let _ = self.event_tx.send(AgentEvent::ExternalTaskPending {
            task_id: task.task_id.clone(),
            session_id: task.session_id.clone(),
            lane: task.lane,
            command_type: task.command_type.clone(),
            payload: task.payload.clone(),
            timeout_ms: task.timeout_ms,
        });

        // Wait for completion or timeout
        match tokio::time::timeout(Duration::from_millis(self.timeout_ms), rx).await {
            Ok(Ok(result)) => result.map_err(|e| LaneError::CommandError(e.to_string())),
            Ok(Err(_)) => Err(LaneError::CommandError("Channel closed".to_string())),
            Err(_) => {
                // Remove from pending on timeout
                let mut tasks = self.external_tasks.write().await;
                tasks.remove(&self.task_id);
                Err(LaneError::Timeout(Duration::from_millis(self.timeout_ms)))
            }
        }
    }

    /// Execute internally with external notification (Hybrid mode)
    async fn execute_with_notification(&self) -> LaneResult<Value> {
        let task = ExternalTask {
            task_id: self.task_id.clone(),
            session_id: self.session_id.clone(),
            lane: self.lane,
            command_type: self.inner.command_type().to_string(),
            payload: self.inner.payload(),
            timeout_ms: self.timeout_ms,
            created_at: Some(Instant::now()),
        };

        // Emit notification event (for monitoring/logging)
        let _ = self.event_tx.send(AgentEvent::ExternalTaskPending {
            task_id: task.task_id.clone(),
            session_id: task.session_id.clone(),
            lane: task.lane,
            command_type: task.command_type.clone(),
            payload: task.payload.clone(),
            timeout_ms: task.timeout_ms,
        });

        // Execute internally
        let result = self
            .inner
            .execute()
            .await
            .map_err(|e| LaneError::CommandError(e.to_string()));

        // Notify completion
        let _ = self.event_tx.send(AgentEvent::ExternalTaskCompleted {
            task_id: self.task_id.clone(),
            session_id: self.session_id.clone(),
            success: result.is_ok(),
        });

        result
    }
}

#[async_trait]
impl LaneCommand for SessionCommandAdapter {
    async fn execute(&self) -> LaneResult<Value> {
        match self.handler_mode {
            TaskHandlerMode::Internal => self
                .inner
                .execute()
                .await
                .map_err(|e| LaneError::CommandError(e.to_string())),
            TaskHandlerMode::External => self.register_and_wait().await,
            TaskHandlerMode::Hybrid => self.execute_with_notification().await,
        }
    }

    fn command_type(&self) -> &str {
        self.inner.command_type()
    }
}

// ============================================================================
// Event Bridge
// ============================================================================

/// Bridge that translates a3s-lane events to AgentEvent
pub struct EventBridge {
    #[allow(dead_code)]
    session_id: String,
    event_tx: broadcast::Sender<AgentEvent>,
}

impl EventBridge {
    pub fn new(session_id: String, event_tx: broadcast::Sender<AgentEvent>) -> Self {
        Self {
            session_id,
            event_tx,
        }
    }

    /// Translate a3s-lane event to AgentEvent and emit
    pub fn emit_dead_letter(&self, dead_letter: &DeadLetter) {
        let _ = self.event_tx.send(AgentEvent::CommandDeadLettered {
            command_id: dead_letter.command_id.clone(),
            command_type: dead_letter.command_type.clone(),
            lane: dead_letter.lane_id.clone(),
            error: dead_letter.error.clone(),
            attempts: dead_letter.attempts,
        });
    }

    /// Emit retry event
    pub fn emit_retry(
        &self,
        command_id: &str,
        command_type: &str,
        lane: &str,
        attempt: u32,
        delay_ms: u64,
    ) {
        let _ = self.event_tx.send(AgentEvent::CommandRetry {
            command_id: command_id.to_string(),
            command_type: command_type.to_string(),
            lane: lane.to_string(),
            attempt,
            delay_ms,
        });
    }

    /// Emit queue alert
    pub fn emit_alert(&self, level: &str, alert_type: &str, message: &str) {
        let _ = self.event_tx.send(AgentEvent::QueueAlert {
            level: level.to_string(),
            alert_type: alert_type.to_string(),
            message: message.to_string(),
        });
    }
}

// ============================================================================
// Session Lane Queue
// ============================================================================

/// Per-session command queue backed by a3s-lane with external task handling
pub struct SessionLaneQueue {
    session_id: String,
    manager: EnhancedQueueManager,
    external_tasks: Arc<RwLock<HashMap<String, PendingExternalTask>>>,
    lane_handlers: Arc<RwLock<HashMap<SessionLane, LaneHandlerConfig>>>,
    event_tx: broadcast::Sender<AgentEvent>,
    #[allow(dead_code)]
    event_bridge: Arc<EventBridge>,
}

impl SessionLaneQueue {
    /// Create a new session lane queue
    pub async fn new(
        session_id: &str,
        config: SessionQueueConfig,
        event_tx: broadcast::Sender<AgentEvent>,
    ) -> Result<Self> {
        // Convert SessionQueueConfig to EnhancedQueueConfig
        let enhanced_config = Self::build_enhanced_config(&config);

        let manager = EnhancedQueueManager::with_config(enhanced_config).await?;

        // Initialize lane handlers from config
        let mut lane_handlers = HashMap::new();
        for lane in [
            SessionLane::Control,
            SessionLane::Query,
            SessionLane::Execute,
            SessionLane::Generate,
        ] {
            lane_handlers.insert(lane, config.handler_config(lane));
        }

        let event_bridge = Arc::new(EventBridge::new(session_id.to_string(), event_tx.clone()));

        Ok(Self {
            session_id: session_id.to_string(),
            manager,
            external_tasks: Arc::new(RwLock::new(HashMap::new())),
            lane_handlers: Arc::new(RwLock::new(lane_handlers)),
            event_tx,
            event_bridge,
        })
    }

    /// Build EnhancedQueueConfig from SessionQueueConfig
    fn build_enhanced_config(config: &SessionQueueConfig) -> EnhancedQueueConfig {
        let mut enhanced = if config.enable_metrics || config.enable_alerts || config.enable_dlq {
            EnhancedQueueConfig::default()
        } else {
            EnhancedQueueConfig::minimal()
        };

        // Apply DLQ settings
        enhanced.dlq_max_size = if config.enable_dlq {
            config.dlq_max_size.or(Some(1000))
        } else {
            None
        };

        // Apply observability settings
        enhanced.enable_metrics = config.enable_metrics;
        enhanced.enable_alerts = config.enable_alerts;

        // Apply timeout settings
        if let Some(timeout_ms) = config.default_timeout_ms {
            enhanced.default_timeout = Some(Duration::from_millis(timeout_ms));
        }

        // Apply storage path
        enhanced.storage_path = config.storage_path.clone();

        enhanced
    }

    /// Start the queue scheduler
    pub async fn start(&self) -> Result<()> {
        self.manager.start().await
    }

    /// Stop the queue scheduler
    pub async fn stop(&self) {
        self.manager.shutdown().await;
    }

    /// Set handler configuration for a lane
    pub async fn set_lane_handler(&self, lane: SessionLane, config: LaneHandlerConfig) {
        let mut handlers = self.lane_handlers.write().await;
        handlers.insert(lane, config);
    }

    /// Get handler configuration for a lane
    pub async fn get_lane_handler(&self, lane: SessionLane) -> LaneHandlerConfig {
        let handlers = self.lane_handlers.read().await;
        handlers.get(&lane).cloned().unwrap_or_default()
    }

    /// Submit a command to a specific lane
    pub async fn submit(
        &self,
        lane: SessionLane,
        command: Box<dyn SessionCommand>,
    ) -> oneshot::Receiver<Result<Value>> {
        let (result_tx, result_rx) = oneshot::channel();

        let handler_config = self.get_lane_handler(lane).await;
        let task_id = uuid::Uuid::new_v4().to_string();

        // Create adapter that handles Internal/External/Hybrid modes
        let adapter = SessionCommandAdapter::new(
            command,
            task_id,
            handler_config.mode,
            self.session_id.clone(),
            lane,
            handler_config.timeout_ms,
            Arc::clone(&self.external_tasks),
            self.event_tx.clone(),
        );

        // Submit to a3s-lane
        match self.manager.submit(lane, Box::new(adapter)).await {
            Ok(lane_rx) => {
                // Bridge the lane result to our result channel
                tokio::spawn(async move {
                    match lane_rx.await {
                        Ok(Ok(value)) => {
                            let _ = result_tx.send(Ok(value));
                        }
                        Ok(Err(e)) => {
                            let _ = result_tx.send(Err(anyhow::anyhow!("{}", e)));
                        }
                        Err(_) => {
                            let _ = result_tx.send(Err(anyhow::anyhow!("Channel closed")));
                        }
                    }
                });
            }
            Err(e) => {
                let _ = result_tx.send(Err(e));
            }
        }

        result_rx
    }

    /// Submit a command by tool name (auto-determines lane)
    pub async fn submit_by_tool(
        &self,
        tool_name: &str,
        command: Box<dyn SessionCommand>,
    ) -> oneshot::Receiver<Result<Value>> {
        let lane = SessionLane::from_tool_name(tool_name);
        self.submit(lane, command).await
    }

    /// Complete an external task with result
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
            true
        } else {
            false
        }
    }

    /// Get queue statistics (unified format)
    pub async fn stats(&self) -> crate::queue::SessionQueueStats {
        let lane_stats = self.manager.stats().await.ok();
        let external_tasks = self.external_tasks.read().await;

        let mut total_pending = 0;
        let mut total_active = 0;
        let mut lanes = HashMap::new();

        if let Some(stats) = lane_stats {
            for (lane_id, lane_stat) in stats.lanes {
                total_pending += lane_stat.pending;
                total_active += lane_stat.active;

                // Map lane_id back to SessionLane
                let session_lane = match lane_id.as_str() {
                    "control" => SessionLane::Control,
                    "query" => SessionLane::Query,
                    "skill" => SessionLane::Execute,
                    "prompt" => SessionLane::Generate,
                    _ => continue,
                };

                let handler_mode = self.get_lane_handler(session_lane).await.mode;

                lanes.insert(
                    format!("{:?}", session_lane),
                    crate::queue::LaneStatus {
                        lane: session_lane,
                        pending: lane_stat.pending,
                        active: lane_stat.active,
                        max_concurrency: lane_stat.max,
                        handler_mode,
                    },
                );
            }
        }

        crate::queue::SessionQueueStats {
            total_pending,
            total_active,
            external_pending: external_tasks.len(),
            lanes,
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

    /// Get dead letters from DLQ
    pub async fn dead_letters(&self) -> Vec<DeadLetter> {
        if let Some(dlq) = self.manager.queue().dlq() {
            dlq.list().await
        } else {
            Vec::new()
        }
    }

    /// Get metrics snapshot
    pub async fn metrics_snapshot(&self) -> Option<MetricsSnapshot> {
        self.manager.metrics_snapshot().await
    }

    /// Drain pending commands with timeout
    pub async fn drain(&self, timeout: Duration) -> Result<()> {
        self.manager.drain(timeout).await
    }

    /// Check if shutdown is in progress
    pub fn is_shutting_down(&self) -> bool {
        self.manager.is_shutting_down()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::SessionCommand;

    struct TestCommand {
        value: Value,
    }

    #[async_trait]
    impl SessionCommand for TestCommand {
        async fn execute(&self) -> Result<Value> {
            Ok(self.value.clone())
        }

        fn command_type(&self) -> &str {
            "test"
        }

        fn payload(&self) -> Value {
            self.value.clone()
        }
    }

    #[tokio::test]
    async fn test_session_lane_queue_creation() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("test-session", config, event_tx).await;
        assert!(queue.is_ok());
        let queue = queue.unwrap();
        assert_eq!(queue.session_id(), "test-session");
    }

    #[tokio::test]
    async fn test_submit_and_execute() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("test-session", config, event_tx)
            .await
            .unwrap();

        queue.start().await.unwrap();

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"result": "success"}),
        });
        let rx = queue.submit(SessionLane::Query, cmd).await;

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["result"], "success");

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_stats() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("test-session", config, event_tx)
            .await
            .unwrap();

        queue.start().await.unwrap();

        let stats = queue.stats().await;
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
        assert_eq!(stats.external_pending, 0);

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_lane_handler_config() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("test-session", config, event_tx)
            .await
            .unwrap();

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
    }

    #[tokio::test]
    async fn test_submit_by_tool() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("test-session", config, event_tx)
            .await
            .unwrap();

        queue.start().await.unwrap();

        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"tool": "read"}),
        });
        let rx = queue.submit_by_tool("read", cmd).await;

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx)
            .await
            .expect("Timeout")
            .expect("Channel closed");

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value["tool"], "read");

        queue.stop().await;
    }

    #[tokio::test]
    async fn test_dead_letters_empty() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("test-session", config, event_tx)
            .await
            .unwrap();

        let dead_letters = queue.dead_letters().await;
        assert!(dead_letters.is_empty());
    }

    #[tokio::test]
    async fn test_metrics_snapshot() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig {
            enable_metrics: true,
            ..Default::default()
        };
        let queue = SessionLaneQueue::new("test-session", config, event_tx)
            .await
            .unwrap();

        queue.start().await.unwrap();

        let snapshot = queue.metrics_snapshot().await;
        assert!(snapshot.is_some());

        queue.stop().await;
    }
    #[test]
    fn test_build_enhanced_config_all_disabled() {
        let config = SessionQueueConfig {
            control_max_concurrency: 1,
            query_max_concurrency: 2,
            execute_max_concurrency: 2,
            generate_max_concurrency: 1,
            lane_handlers: HashMap::new(),
            enable_metrics: false,
            enable_alerts: false,
            enable_dlq: false,
            dlq_max_size: None,
            default_timeout_ms: None,
            storage_path: None,
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert!(!enhanced.enable_metrics);
        assert!(!enhanced.enable_alerts);
        assert_eq!(enhanced.dlq_max_size, None);
    }

    #[test]
    fn test_build_enhanced_config_dlq_enabled_default_size() {
        let config = SessionQueueConfig {
            enable_dlq: true,
            dlq_max_size: None,
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert_eq!(enhanced.dlq_max_size, Some(1000));
    }

    #[test]
    fn test_build_enhanced_config_dlq_enabled_custom_size() {
        let config = SessionQueueConfig {
            enable_dlq: true,
            dlq_max_size: Some(500),
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert_eq!(enhanced.dlq_max_size, Some(500));
    }

    #[test]
    fn test_build_enhanced_config_dlq_disabled() {
        let config = SessionQueueConfig {
            enable_dlq: false,
            dlq_max_size: Some(500),
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert_eq!(enhanced.dlq_max_size, None);
    }

    #[test]
    fn test_build_enhanced_config_metrics_enabled() {
        let config = SessionQueueConfig {
            enable_metrics: true,
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert!(enhanced.enable_metrics);
    }

    #[test]
    fn test_build_enhanced_config_alerts_enabled() {
        let config = SessionQueueConfig {
            enable_alerts: true,
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert!(enhanced.enable_alerts);
    }

    #[test]
    fn test_build_enhanced_config_custom_timeout() {
        let config = SessionQueueConfig {
            default_timeout_ms: Some(5000),
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert_eq!(enhanced.default_timeout, Some(Duration::from_millis(5000)));
    }

    #[test]
    fn test_build_enhanced_config_no_timeout() {
        let config = SessionQueueConfig {
            default_timeout_ms: None,
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert_eq!(enhanced.default_timeout, None);
    }

    #[test]
    fn test_build_enhanced_config_storage_path() {
        let config = SessionQueueConfig {
            storage_path: Some(std::path::PathBuf::from("/tmp/test")),
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert_eq!(enhanced.storage_path, Some(std::path::PathBuf::from("/tmp/test")));
    }

    #[test]
    fn test_build_enhanced_config_all_enabled() {
        let config = SessionQueueConfig {
            control_max_concurrency: 1,
            query_max_concurrency: 2,
            execute_max_concurrency: 2,
            generate_max_concurrency: 1,
            lane_handlers: HashMap::new(),
            enable_metrics: true,
            enable_alerts: true,
            enable_dlq: true,
            dlq_max_size: Some(2000),
            default_timeout_ms: Some(10000),
            storage_path: Some(std::path::PathBuf::from("/tmp/queue")),
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert!(enhanced.enable_metrics);
        assert!(enhanced.enable_alerts);
        assert_eq!(enhanced.dlq_max_size, Some(2000));
        assert_eq!(enhanced.default_timeout, Some(Duration::from_millis(10000)));
        assert_eq!(enhanced.storage_path, Some(std::path::PathBuf::from("/tmp/queue")));
    }

    #[test]
    fn test_build_enhanced_config_zero_timeout() {
        let config = SessionQueueConfig {
            default_timeout_ms: Some(0),
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert_eq!(enhanced.default_timeout, Some(Duration::from_millis(0)));
    }

    #[test]
    fn test_build_enhanced_config_large_dlq() {
        let config = SessionQueueConfig {
            enable_dlq: true,
            dlq_max_size: Some(100000),
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert_eq!(enhanced.dlq_max_size, Some(100000));
    }

    #[test]
    fn test_build_enhanced_config_metrics_only() {
        let config = SessionQueueConfig {
            enable_metrics: true,
            enable_alerts: false,
            enable_dlq: false,
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert!(enhanced.enable_metrics);
        assert!(!enhanced.enable_alerts);
        assert_eq!(enhanced.dlq_max_size, None);
    }

    #[test]
    fn test_build_enhanced_config_alerts_only() {
        let config = SessionQueueConfig {
            enable_metrics: false,
            enable_alerts: true,
            enable_dlq: false,
            ..Default::default()
        };
        let enhanced = SessionLaneQueue::build_enhanced_config(&config);
        assert!(!enhanced.enable_metrics);
        assert!(enhanced.enable_alerts);
        assert_eq!(enhanced.dlq_max_size, None);
    }

    #[test]
    fn test_event_bridge_new() {
        let (event_tx, _) = broadcast::channel(100);
        let bridge = EventBridge::new("test-session".to_string(), event_tx);
        assert_eq!(bridge.session_id, "test-session");
    }

    #[test]
    fn test_event_bridge_session_id_check() {
        let (event_tx, _) = broadcast::channel(100);
        let bridge = EventBridge::new("my-session-123".to_string(), event_tx);
        assert_eq!(bridge.session_id, "my-session-123");
    }

    #[test]
    fn test_event_bridge_emit_dead_letter() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let bridge = EventBridge::new("test-session".to_string(), event_tx);

        let dead_letter = DeadLetter {
            command_id: "cmd-123".to_string(),
            command_type: "test_command".to_string(),
            lane_id: "control".to_string(),
            error: "Test error".to_string(),
            attempts: 3,
            failed_at: chrono::Utc::now(),
        };

        bridge.emit_dead_letter(&dead_letter);

        let event = event_rx.try_recv().unwrap();
        match event {
            AgentEvent::CommandDeadLettered {
                command_id,
                command_type,
                lane,
                error,
                attempts,
            } => {
                assert_eq!(command_id, "cmd-123");
                assert_eq!(command_type, "test_command");
                assert_eq!(lane, "control");
                assert_eq!(error, "Test error");
                assert_eq!(attempts, 3);
            }
            _ => panic!("Expected CommandDeadLettered event"),
        }
    }

    #[test]
    fn test_event_bridge_emit_retry() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let bridge = EventBridge::new("test-session".to_string(), event_tx);

        bridge.emit_retry("cmd-456", "retry_command", "query", 2, 1000);

        let event = event_rx.try_recv().unwrap();
        match event {
            AgentEvent::CommandRetry {
                command_id,
                command_type,
                lane,
                attempt,
                delay_ms,
            } => {
                assert_eq!(command_id, "cmd-456");
                assert_eq!(command_type, "retry_command");
                assert_eq!(lane, "query");
                assert_eq!(attempt, 2);
                assert_eq!(delay_ms, 1000);
            }
            _ => panic!("Expected CommandRetry event"),
        }
    }

    #[test]
    fn test_event_bridge_emit_alert() {
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let bridge = EventBridge::new("test-session".to_string(), event_tx);

        bridge.emit_alert("warning", "queue_full", "Queue is at capacity");

        let event = event_rx.try_recv().unwrap();
        match event {
            AgentEvent::QueueAlert {
                level,
                alert_type,
                message,
            } => {
                assert_eq!(level, "warning");
                assert_eq!(alert_type, "queue_full");
                assert_eq!(message, "Queue is at capacity");
            }
            _ => panic!("Expected QueueAlert event"),
        }
    }

    #[tokio::test]
    async fn test_session_lane_queue_is_shutting_down() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("test-session", config, event_tx)
            .await
            .unwrap();

        assert!(!queue.is_shutting_down());
        queue.stop().await;
        assert!(queue.is_shutting_down());
    }

    #[tokio::test]
    async fn test_session_lane_queue_session_id() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("my-test-session", config, event_tx)
            .await
            .unwrap();

        assert_eq!(queue.session_id(), "my-test-session");
    }

    #[tokio::test]
    async fn test_session_lane_queue_set_get_lane_handler() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("test-session", config, event_tx)
            .await
            .unwrap();

        let new_config = LaneHandlerConfig {
            mode: TaskHandlerMode::External,
            timeout_ms: 15000,
        };

        queue.set_lane_handler(SessionLane::Query, new_config.clone()).await;
        let retrieved = queue.get_lane_handler(SessionLane::Query).await;

        assert_eq!(retrieved.mode, TaskHandlerMode::External);
        assert_eq!(retrieved.timeout_ms, 15000);
    }

    #[tokio::test]
    async fn test_session_lane_queue_pending_external_tasks_empty() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("test-session", config, event_tx)
            .await
            .unwrap();

        let tasks = queue.pending_external_tasks().await;
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn test_session_lane_queue_complete_external_task_nonexistent() {
        let (event_tx, _) = broadcast::channel(100);
        let config = SessionQueueConfig::default();
        let queue = SessionLaneQueue::new("test-session", config, event_tx)
            .await
            .unwrap();

        let result = ExternalTaskResult {
            success: true,
            result: serde_json::json!({"status": "ok"}),
            error: None,
        };

        let completed = queue.complete_external_task("nonexistent-task", result).await;
        assert!(!completed);
    }

    #[test]
    fn test_test_command_payload() {
        let cmd = TestCommand {
            value: serde_json::json!({"key": "value"}),
        };
        assert_eq!(cmd.payload(), serde_json::json!({"key": "value"}));
        assert_eq!(cmd.command_type(), "test");
    }
}
