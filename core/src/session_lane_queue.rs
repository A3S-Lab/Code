//! Session Lane Queue - a3s-lane backed command queue
//!
//! Provides per-session command queues with lane-based priority scheduling,
//! backed by a3s-lane directly (no intermediate wrapper).
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
use crate::queue::{
    ExternalTask, ExternalTaskResult, LaneHandlerConfig, SessionCommand, SessionQueueConfig,
    TaskHandlerMode,
};
use a3s_lane::{
    AlertManager, Command as LaneCommand, DeadLetter, EventEmitter, LaneConfig, LaneError,
    LocalStorage, MetricsSnapshot, PriorityBoostConfig, QueueManager, QueueManagerBuilder,
    QueueMetrics, RateLimitConfig, Result as LaneResult, RetryPolicy,
};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, oneshot, RwLock};

// ============================================================================
// SessionLane → a3s-lane mapping
// ============================================================================

impl SessionLane {
    /// Get a3s-lane lane ID string
    fn lane_id(self) -> &'static str {
        match self {
            SessionLane::Control => "control",
            SessionLane::Query => "query",
            SessionLane::Execute => "skill",
            SessionLane::Generate => "prompt",
        }
    }

    /// Get default lane configuration for a3s-lane
    fn lane_config(self) -> LaneConfig {
        match self {
            SessionLane::Control => LaneConfig::new(1, 2),
            SessionLane::Query => LaneConfig::new(1, 4),
            SessionLane::Execute => LaneConfig::new(1, 2),
            SessionLane::Generate => LaneConfig::new(1, 1),
        }
    }

    /// Get priority value (lower = higher priority)
    fn lane_priority(self) -> u8 {
        match self {
            SessionLane::Control => 1,
            SessionLane::Query => 2,
            SessionLane::Execute => 4,
            SessionLane::Generate => 5,
        }
    }
}

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
        let _ = self.event_tx.send(AgentEvent::ExternalTaskPending {
            task_id: task.task_id.clone(),
            session_id: task.session_id.clone(),
            lane: task.lane,
            command_type: task.command_type.clone(),
            payload: task.payload.clone(),
            timeout_ms: task.timeout_ms,
        });
        match tokio::time::timeout(Duration::from_millis(self.timeout_ms), rx).await {
            Ok(Ok(result)) => result.map_err(|e| LaneError::CommandError(e.to_string())),
            Ok(Err(_)) => Err(LaneError::CommandError("Channel closed".to_string())),
            Err(_) => {
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
        let _ = self.event_tx.send(AgentEvent::ExternalTaskPending {
            task_id: task.task_id.clone(),
            session_id: task.session_id.clone(),
            lane: task.lane,
            command_type: task.command_type.clone(),
            payload: task.payload.clone(),
            timeout_ms: task.timeout_ms,
        });
        let result = self
            .inner
            .execute()
            .await
            .map_err(|e| LaneError::CommandError(e.to_string()));
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

    pub fn emit_dead_letter(&self, dead_letter: &DeadLetter) {
        let _ = self.event_tx.send(AgentEvent::CommandDeadLettered {
            command_id: dead_letter.command_id.clone(),
            command_type: dead_letter.command_type.clone(),
            lane: dead_letter.lane_id.clone(),
            error: dead_letter.error.clone(),
            attempts: dead_letter.attempts,
        });
    }

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
    manager: Arc<QueueManager>,
    metrics: Option<QueueMetrics>,
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
        let (manager, metrics) = Self::build_queue_manager(&config).await?;
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
            manager: Arc::new(manager),
            metrics,
            external_tasks: Arc::new(RwLock::new(HashMap::new())),
            lane_handlers: Arc::new(RwLock::new(lane_handlers)),
            event_tx,
            event_bridge,
        })
    }

    /// Build a3s-lane QueueManager directly from SessionQueueConfig
    async fn build_queue_manager(
        config: &SessionQueueConfig,
    ) -> Result<(QueueManager, Option<QueueMetrics>)> {
        let emitter = EventEmitter::new(100);
        let mut builder = QueueManagerBuilder::new(emitter);
        let default_timeout = config.default_timeout_ms.map(Duration::from_millis);
        let default_retry = Some(RetryPolicy::exponential(3));

        for lane in [
            SessionLane::Control,
            SessionLane::Query,
            SessionLane::Execute,
            SessionLane::Generate,
        ] {
            let mut cfg = lane.lane_config();
            if let Some(timeout) = default_timeout {
                cfg = cfg.with_timeout(timeout);
            }
            if let Some(ref retry) = default_retry {
                cfg = cfg.with_retry_policy(retry.clone());
            }
            if lane == SessionLane::Generate {
                cfg = cfg.with_rate_limit(RateLimitConfig::per_minute(60));
                cfg = cfg
                    .with_priority_boost(PriorityBoostConfig::standard(Duration::from_secs(300)));
            }
            builder = builder.with_lane(lane.lane_id(), cfg, lane.lane_priority());
        }

        if config.enable_dlq {
            builder = builder.with_dlq(config.dlq_max_size.unwrap_or(1000));
        }

        let metrics = if config.enable_metrics {
            let m = QueueMetrics::local();
            builder = builder.with_metrics(m.clone());
            Some(m)
        } else {
            None
        };

        if config.enable_alerts {
            builder = builder.with_alerts(Arc::new(AlertManager::with_queue_depth_alerts(50, 100)));
        }

        if let Some(ref storage_path) = config.storage_path {
            builder = builder.with_storage(Arc::new(
                LocalStorage::new(storage_path.to_path_buf()).await?,
            ));
        }

        let manager = builder.build().await?;
        Ok((manager, metrics))
    }

    pub async fn start(&self) -> Result<()> {
        self.manager.start().await
    }
    pub async fn stop(&self) {
        self.manager.shutdown().await;
    }

    pub async fn set_lane_handler(&self, lane: SessionLane, config: LaneHandlerConfig) {
        self.lane_handlers.write().await.insert(lane, config);
    }

    pub async fn get_lane_handler(&self, lane: SessionLane) -> LaneHandlerConfig {
        self.lane_handlers
            .read()
            .await
            .get(&lane)
            .cloned()
            .unwrap_or_default()
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
        match self.manager.submit(lane.lane_id(), Box::new(adapter)).await {
            Ok(lane_rx) => {
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
                let _ = result_tx.send(Err(e.into()));
            }
        }
        result_rx
    }

    pub async fn submit_by_tool(
        &self,
        tool_name: &str,
        command: Box<dyn SessionCommand>,
    ) -> oneshot::Receiver<Result<Value>> {
        self.submit(SessionLane::from_tool_name(tool_name), command)
            .await
    }

    pub async fn complete_external_task(&self, task_id: &str, result: ExternalTaskResult) -> bool {
        let pending = { self.external_tasks.write().await.remove(task_id) };
        if let Some(pending) = pending {
            let _ = self.event_tx.send(AgentEvent::ExternalTaskCompleted {
                task_id: task_id.to_string(),
                session_id: self.session_id.clone(),
                success: result.success,
            });
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

    pub async fn pending_external_tasks(&self) -> Vec<ExternalTask> {
        self.external_tasks
            .read()
            .await
            .values()
            .map(|p| p.task.clone())
            .collect()
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub async fn dead_letters(&self) -> Vec<DeadLetter> {
        if let Some(dlq) = self.manager.queue().dlq() {
            dlq.list().await
        } else {
            Vec::new()
        }
    }

    pub async fn metrics_snapshot(&self) -> Option<MetricsSnapshot> {
        if let Some(ref m) = self.metrics {
            Some(m.snapshot().await)
        } else {
            None
        }
    }

    pub async fn drain(&self, timeout: Duration) -> Result<()> {
        Ok(self.manager.drain(timeout).await?)
    }
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
        let (tx, _) = broadcast::channel(100);
        let q = SessionLaneQueue::new("test-session", SessionQueueConfig::default(), tx)
            .await
            .unwrap();
        assert_eq!(q.session_id(), "test-session");
    }

    #[tokio::test]
    async fn test_submit_and_execute() {
        let (tx, _) = broadcast::channel(100);
        let q = SessionLaneQueue::new("s", SessionQueueConfig::default(), tx)
            .await
            .unwrap();
        q.start().await.unwrap();
        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"result": "success"}),
        });
        let rx = q.submit(SessionLane::Query, cmd).await;
        let result = tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap()["result"], "success");
        q.stop().await;
    }

    #[tokio::test]
    async fn test_stats() {
        let (tx, _) = broadcast::channel(100);
        let q = SessionLaneQueue::new("s", SessionQueueConfig::default(), tx)
            .await
            .unwrap();
        q.start().await.unwrap();
        let stats = q.stats().await;
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
        assert_eq!(stats.external_pending, 0);
        q.stop().await;
    }

    #[tokio::test]
    async fn test_lane_handler_config() {
        let (tx, _) = broadcast::channel(100);
        let q = SessionLaneQueue::new("s", SessionQueueConfig::default(), tx)
            .await
            .unwrap();
        assert_eq!(
            q.get_lane_handler(SessionLane::Execute).await.mode,
            TaskHandlerMode::Internal
        );
        q.set_lane_handler(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 30000,
            },
        )
        .await;
        let h = q.get_lane_handler(SessionLane::Execute).await;
        assert_eq!(h.mode, TaskHandlerMode::External);
        assert_eq!(h.timeout_ms, 30000);
    }

    #[tokio::test]
    async fn test_submit_by_tool() {
        let (tx, _) = broadcast::channel(100);
        let q = SessionLaneQueue::new("s", SessionQueueConfig::default(), tx)
            .await
            .unwrap();
        q.start().await.unwrap();
        let cmd = Box::new(TestCommand {
            value: serde_json::json!({"tool": "read"}),
        });
        let rx = q.submit_by_tool("read", cmd).await;
        let result = tokio::time::timeout(Duration::from_secs(2), rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap()["tool"], "read");
        q.stop().await;
    }

    #[tokio::test]
    async fn test_dead_letters_empty() {
        let (tx, _) = broadcast::channel(100);
        let q = SessionLaneQueue::new("s", SessionQueueConfig::default(), tx)
            .await
            .unwrap();
        assert!(q.dead_letters().await.is_empty());
    }

    #[tokio::test]
    async fn test_metrics_snapshot() {
        let (tx, _) = broadcast::channel(100);
        let cfg = SessionQueueConfig {
            enable_metrics: true,
            ..Default::default()
        };
        let q = SessionLaneQueue::new("s", cfg, tx).await.unwrap();
        q.start().await.unwrap();
        assert!(q.metrics_snapshot().await.is_some());
        q.stop().await;
    }

    #[tokio::test]
    async fn test_is_shutting_down() {
        let (tx, _) = broadcast::channel(100);
        let q = SessionLaneQueue::new("s", SessionQueueConfig::default(), tx)
            .await
            .unwrap();
        assert!(!q.is_shutting_down());
        q.stop().await;
        assert!(q.is_shutting_down());
    }

    #[tokio::test]
    async fn test_pending_external_tasks_empty() {
        let (tx, _) = broadcast::channel(100);
        let q = SessionLaneQueue::new("s", SessionQueueConfig::default(), tx)
            .await
            .unwrap();
        assert!(q.pending_external_tasks().await.is_empty());
    }

    #[tokio::test]
    async fn test_complete_external_task_nonexistent() {
        let (tx, _) = broadcast::channel(100);
        let q = SessionLaneQueue::new("s", SessionQueueConfig::default(), tx)
            .await
            .unwrap();
        let r = ExternalTaskResult {
            success: true,
            result: serde_json::json!("ok"),
            error: None,
        };
        assert!(!q.complete_external_task("nope", r).await);
    }

    #[test]
    fn test_command_payload() {
        let cmd = TestCommand {
            value: serde_json::json!({"k": "v"}),
        };
        assert_eq!(cmd.payload(), serde_json::json!({"k": "v"}));
        assert_eq!(cmd.command_type(), "test");
    }

    #[test]
    fn test_event_bridge_new() {
        let (tx, _) = broadcast::channel(100);
        let b = EventBridge::new("s".to_string(), tx);
        assert_eq!(b.session_id, "s");
    }

    #[test]
    fn test_event_bridge_emit_dead_letter() {
        let (tx, mut rx) = broadcast::channel(100);
        let b = EventBridge::new("s".to_string(), tx);
        b.emit_dead_letter(&DeadLetter {
            command_id: "c1".to_string(),
            command_type: "t".to_string(),
            lane_id: "control".to_string(),
            error: "err".to_string(),
            attempts: 3,
            failed_at: chrono::Utc::now(),
        });
        match rx.try_recv().unwrap() {
            AgentEvent::CommandDeadLettered {
                command_id,
                attempts,
                ..
            } => {
                assert_eq!(command_id, "c1");
                assert_eq!(attempts, 3);
            }
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn test_event_bridge_emit_retry() {
        let (tx, mut rx) = broadcast::channel(100);
        let b = EventBridge::new("s".to_string(), tx);
        b.emit_retry("c1", "t", "query", 2, 1000);
        match rx.try_recv().unwrap() {
            AgentEvent::CommandRetry {
                attempt, delay_ms, ..
            } => {
                assert_eq!(attempt, 2);
                assert_eq!(delay_ms, 1000);
            }
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn test_event_bridge_emit_alert() {
        let (tx, mut rx) = broadcast::channel(100);
        let b = EventBridge::new("s".to_string(), tx);
        b.emit_alert("warning", "queue_full", "at capacity");
        match rx.try_recv().unwrap() {
            AgentEvent::QueueAlert { level, .. } => assert_eq!(level, "warning"),
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn test_lane_mapping() {
        assert_eq!(SessionLane::Control.lane_id(), "control");
        assert_eq!(SessionLane::Query.lane_id(), "query");
        assert_eq!(SessionLane::Execute.lane_id(), "skill");
        assert_eq!(SessionLane::Generate.lane_id(), "prompt");
    }

    #[test]
    fn test_lane_priority() {
        assert!(SessionLane::Control.lane_priority() < SessionLane::Query.lane_priority());
        assert!(SessionLane::Query.lane_priority() < SessionLane::Execute.lane_priority());
        assert!(SessionLane::Execute.lane_priority() < SessionLane::Generate.lane_priority());
    }

    #[test]
    fn test_lane_config() {
        assert_eq!(SessionLane::Control.lane_config().max_concurrency, 2);
        assert_eq!(SessionLane::Query.lane_config().max_concurrency, 4);
        assert_eq!(SessionLane::Generate.lane_config().max_concurrency, 1);
    }

    #[tokio::test]
    async fn test_build_queue_manager_default() {
        let (_, metrics) = SessionLaneQueue::build_queue_manager(&SessionQueueConfig::default())
            .await
            .unwrap();
        assert!(metrics.is_none());
    }

    #[tokio::test]
    async fn test_build_queue_manager_with_metrics() {
        let cfg = SessionQueueConfig {
            enable_metrics: true,
            ..Default::default()
        };
        let (_, metrics) = SessionLaneQueue::build_queue_manager(&cfg).await.unwrap();
        assert!(metrics.is_some());
    }

    #[tokio::test]
    async fn test_build_queue_manager_with_dlq() {
        let cfg = SessionQueueConfig {
            enable_dlq: true,
            dlq_max_size: Some(500),
            ..Default::default()
        };
        assert!(SessionLaneQueue::build_queue_manager(&cfg).await.is_ok());
    }
}
