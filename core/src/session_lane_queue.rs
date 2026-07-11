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
use crate::queue::SessionLane;
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
    is_shutting_down: Arc<std::sync::atomic::AtomicBool>,
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
        is_shutting_down: Arc<std::sync::atomic::AtomicBool>,
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
            is_shutting_down,
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
            if self
                .is_shutting_down
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(LaneError::CommandError(
                    "Session queue is shutting down".to_string(),
                ));
            }
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

/// Per-session command queue backed by a3s-lane with external task handling
pub struct SessionLaneQueue {
    session_id: String,
    manager: Arc<QueueManager>,
    metrics: Option<QueueMetrics>,
    external_tasks: Arc<RwLock<HashMap<String, PendingExternalTask>>>,
    lane_handlers: Arc<RwLock<HashMap<SessionLane, LaneHandlerConfig>>>,
    event_tx: broadcast::Sender<AgentEvent>,
    task_id_counter: Arc<std::sync::atomic::AtomicU64>, // Fast task ID generation
    is_shutting_down: Arc<std::sync::atomic::AtomicBool>,
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
        Ok(Self {
            session_id: session_id.to_string(),
            manager: Arc::new(manager),
            metrics,
            external_tasks: Arc::new(RwLock::new(HashMap::new())),
            lane_handlers: Arc::new(RwLock::new(lane_handlers)),
            event_tx,
            task_id_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            is_shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            // Apply user-configured concurrency limits
            let max_concurrency = match lane {
                SessionLane::Control => config.control_max_concurrency,
                SessionLane::Query => config.query_max_concurrency,
                SessionLane::Execute => config.execute_max_concurrency,
                SessionLane::Generate => config.generate_max_concurrency,
            };

            // Create LaneConfig with user-specified max_concurrency
            let mut cfg = LaneConfig::new(1, max_concurrency);

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
        self.manager
            .start()
            .await
            .map_err(|e| anyhow::anyhow!("Lane manager start failed: {}", e))
    }

    /// Stop accepting commands and resolve every external task that is still
    /// waiting on an SDK callback. This is the first phase of session close.
    pub async fn shutdown(&self) {
        self.is_shutting_down
            .store(true, std::sync::atomic::Ordering::Release);
        self.manager.shutdown().await;
        let pending = {
            let mut external_tasks = self.external_tasks.write().await;
            std::mem::take(&mut *external_tasks)
        };
        for (task_id, pending) in pending {
            let _ = self.event_tx.send(AgentEvent::ExternalTaskCompleted {
                task_id,
                session_id: self.session_id.clone(),
                success: false,
            });
            let _ = pending
                .result_tx
                .send(Err(anyhow::anyhow!("Session queue is shutting down")));
        }
    }

    /// Wait until commands admitted before [`Self::shutdown`] have finished.
    pub async fn drain(&self, timeout: Duration) -> Result<()> {
        self.manager
            .drain(timeout)
            .await
            .map_err(|error| anyhow::anyhow!("Lane manager drain failed: {error}"))
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

        // Fast task ID generation using atomic counter instead of UUID
        let task_id = format!(
            "{}-{}",
            self.session_id,
            self.task_id_counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );

        let adapter = SessionCommandAdapter::new(
            command,
            task_id,
            handler_config.mode,
            self.session_id.clone(),
            lane,
            handler_config.timeout_ms,
            Arc::clone(&self.external_tasks),
            Arc::clone(&self.is_shutting_down),
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

    /// Subscribe to queue events (CommandDeadLettered, CommandRetry, QueueAlert, etc.)
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
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
        assert_eq!(q.session_id, "test-session");
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

    #[tokio::test]
    async fn shutdown_rejects_new_commands_and_resolves_pending_external_tasks() {
        let (tx, _) = broadcast::channel(100);
        let q = SessionLaneQueue::new("s", SessionQueueConfig::default(), tx)
            .await
            .unwrap();
        q.set_lane_handler(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 30_000,
            },
        )
        .await;
        q.start().await.unwrap();

        let pending_result = q
            .submit(
                SessionLane::Execute,
                Box::new(TestCommand {
                    value: serde_json::json!({"side_effect": true}),
                }),
            )
            .await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while q.pending_external_tasks().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("external task should become pending");

        q.shutdown().await;
        assert!(q.manager.is_shutting_down());
        assert!(q.pending_external_tasks().await.is_empty());
        let result = tokio::time::timeout(Duration::from_secs(1), pending_result)
            .await
            .expect("pending task should resolve during shutdown")
            .expect("queue result sender should remain connected");
        assert!(result.is_err());
        q.drain(Duration::from_secs(2)).await.unwrap();

        let rejected = q
            .submit(
                SessionLane::Execute,
                Box::new(TestCommand {
                    value: serde_json::json!({"late": true}),
                }),
            )
            .await;
        assert!(rejected.await.unwrap().is_err());
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

    // Queue configuration is handled by SessionQueueConfig.

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
