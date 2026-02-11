//! Integration layer between a3s-code and a3s-lane
//!
//! This module provides a bridge between the session-level command queue
//! and the a3s-lane priority queue system, enabling advanced features like
//! metrics, DLQ, retry policies, and persistent storage.
//!
//! ## Features
//!
//! ### Phase 1: Core Integration ✅
//! - Priority-based scheduling
//! - Per-lane concurrency control
//! - Dead letter queue (DLQ)
//! - Queue statistics
//! - Graceful shutdown
//!
//! ### Phase 2: Observability ✅
//! - Metrics collection (local + pluggable backends)
//! - Latency histograms (p50, p90, p95, p99)
//! - Queue depth alerts
//! - Latency alerts
//!
//! ### Phase 3: Reliability ✅
//! - Command timeout per lane
//! - Retry policies (exponential backoff, fixed delay)
//! - Persistent storage (LocalStorage + pluggable)
//! - Rate limiting per lane
//! - Priority boosting

use a3s_lane::{
    AlertManager, Command as LaneCommand, EventEmitter, LaneConfig, LaneError, LocalStorage,
    PriorityBoostConfig, QueueManager, QueueManagerBuilder, QueueMetrics, QueueStats,
    RateLimitConfig, Result as LaneResult, RetryPolicy,
};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::hitl::SessionLane;

// Re-export useful types from a3s-lane
pub use a3s_lane::{
    AlertManager as LaneAlertManager, DeadLetterQueue, LaneError as QueueError,
    LocalStorage as LaneLocalStorage, QueueMetrics as LaneMetrics, QueueStats as LaneQueueStats,
    RetryPolicy as LaneRetryPolicy,
};

/// Map SessionLane to a3s-lane lane IDs
impl SessionLane {
    /// Convert to a3s-lane lane ID string
    pub fn to_lane_id(&self) -> &'static str {
        match self {
            SessionLane::Control => "control",
            SessionLane::Query => "query",
            SessionLane::Execute => "skill",
            SessionLane::Generate => "prompt",
        }
    }

    /// Get default lane configuration for a3s-lane
    pub fn to_lane_config(&self) -> LaneConfig {
        match self {
            SessionLane::Control => LaneConfig::new(1, 2),
            SessionLane::Query => LaneConfig::new(1, 4),
            SessionLane::Execute => LaneConfig::new(1, 2),
            SessionLane::Generate => LaneConfig::new(1, 1),
        }
    }

    /// Get priority value (lower = higher priority)
    pub fn to_priority(&self) -> u8 {
        match self {
            SessionLane::Control => 1,
            SessionLane::Query => 2,
            SessionLane::Execute => 4,
            SessionLane::Generate => 5,
        }
    }
}

/// Wrapper for async commands to be executed through a3s-lane
pub struct AsyncSessionCommand<F, Fut>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = anyhow::Result<Value>> + Send,
{
    command_type: String,
    execute_fn: F,
}

impl<F, Fut> AsyncSessionCommand<F, Fut>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = anyhow::Result<Value>> + Send,
{
    pub fn new(command_type: impl Into<String>, execute_fn: F) -> Self {
        Self {
            command_type: command_type.into(),
            execute_fn,
        }
    }
}

#[async_trait]
impl<F, Fut> LaneCommand for AsyncSessionCommand<F, Fut>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = anyhow::Result<Value>> + Send,
{
    async fn execute(&self) -> LaneResult<Value> {
        (self.execute_fn)()
            .await
            .map_err(|e| LaneError::CommandError(e.to_string()))
    }

    fn command_type(&self) -> &str {
        &self.command_type
    }
}

/// Simple command wrapper for synchronous operations
pub struct SimpleCommand {
    command_type: String,
    result: Value,
}

impl SimpleCommand {
    pub fn new(command_type: impl Into<String>, result: Value) -> Self {
        Self {
            command_type: command_type.into(),
            result,
        }
    }
}

#[async_trait]
impl LaneCommand for SimpleCommand {
    async fn execute(&self) -> LaneResult<Value> {
        Ok(self.result.clone())
    }

    fn command_type(&self) -> &str {
        &self.command_type
    }
}

/// Enhanced queue manager using a3s-lane with full Phase 1-3 features
pub struct EnhancedQueueManager {
    manager: Arc<QueueManager>,
    metrics: Option<QueueMetrics>,
    alerts: Option<Arc<AlertManager>>,
}

impl EnhancedQueueManager {
    /// Create a new enhanced queue manager with default configuration
    pub async fn new() -> anyhow::Result<Self> {
        Self::with_config(EnhancedQueueConfig::default()).await
    }

    /// Create with custom configuration including all Phase 1-3 features
    pub async fn with_config(config: EnhancedQueueConfig) -> anyhow::Result<Self> {
        let emitter = EventEmitter::new(config.event_buffer_size);
        let mut builder = QueueManagerBuilder::new(emitter);

        // Build lane configs with reliability features
        let control_config = config.control_config.unwrap_or_else(|| {
            let mut cfg = SessionLane::Control.to_lane_config();
            if let Some(timeout) = config.default_timeout {
                cfg = cfg.with_timeout(timeout);
            }
            if let Some(ref retry) = config.default_retry_policy {
                cfg = cfg.with_retry_policy(retry.clone());
            }
            cfg
        });

        let query_config = config.query_config.unwrap_or_else(|| {
            let mut cfg = SessionLane::Query.to_lane_config();
            if let Some(timeout) = config.default_timeout {
                cfg = cfg.with_timeout(timeout);
            }
            if let Some(ref retry) = config.default_retry_policy {
                cfg = cfg.with_retry_policy(retry.clone());
            }
            if let Some(ref rate_limit) = config.query_rate_limit {
                cfg = cfg.with_rate_limit(rate_limit.clone());
            }
            cfg
        });

        let execute_config = config.execute_config.unwrap_or_else(|| {
            let mut cfg = SessionLane::Execute.to_lane_config();
            if let Some(timeout) = config.execute_timeout {
                cfg = cfg.with_timeout(timeout);
            } else if let Some(timeout) = config.default_timeout {
                cfg = cfg.with_timeout(timeout);
            }
            if let Some(ref retry) = config.default_retry_policy {
                cfg = cfg.with_retry_policy(retry.clone());
            }
            cfg
        });

        let generate_config = config.generate_config.unwrap_or_else(|| {
            let mut cfg = SessionLane::Generate.to_lane_config();
            // LLM calls need longer timeout
            if let Some(timeout) = config.generate_timeout {
                cfg = cfg.with_timeout(timeout);
            }
            // Rate limit for LLM API calls
            if let Some(ref rate_limit) = config.generate_rate_limit {
                cfg = cfg.with_rate_limit(rate_limit.clone());
            }
            // Priority boost for deadline-sensitive prompts
            if let Some(ref boost) = config.generate_priority_boost {
                cfg = cfg.with_priority_boost(boost.clone());
            }
            cfg
        });

        // Add lanes
        builder = builder
            .with_lane(
                "control",
                control_config,
                SessionLane::Control.to_priority(),
            )
            .with_lane("query", query_config, SessionLane::Query.to_priority())
            .with_lane("skill", execute_config, SessionLane::Execute.to_priority())
            .with_lane(
                "prompt",
                generate_config,
                SessionLane::Generate.to_priority(),
            );

        // Phase 1: DLQ
        if let Some(dlq_size) = config.dlq_max_size {
            builder = builder.with_dlq(dlq_size);
        }

        // Phase 2: Metrics
        let metrics = if config.enable_metrics {
            let m = QueueMetrics::local();
            builder = builder.with_metrics(m.clone());
            Some(m)
        } else {
            None
        };

        // Phase 2: Alerts
        let alerts = if config.enable_alerts {
            let alert_manager = Arc::new(AlertManager::with_queue_depth_alerts(
                config.alert_warning_threshold,
                config.alert_critical_threshold,
            ));
            builder = builder.with_alerts(alert_manager.clone());
            Some(alert_manager)
        } else {
            None
        };

        // Phase 3: Persistent Storage
        if let Some(storage_path) = config.storage_path {
            let storage = Arc::new(LocalStorage::new(storage_path).await?);
            builder = builder.with_storage(storage);
        }

        let manager = builder.build().await?;

        Ok(Self {
            manager: Arc::new(manager),
            metrics,
            alerts,
        })
    }

    /// Submit a command to a specific lane
    pub async fn submit(
        &self,
        lane: SessionLane,
        command: Box<dyn LaneCommand>,
    ) -> anyhow::Result<tokio::sync::oneshot::Receiver<LaneResult<Value>>> {
        Ok(self.manager.submit(lane.to_lane_id(), command).await?)
    }

    /// Submit a command by lane ID string
    pub async fn submit_to_lane(
        &self,
        lane_id: &str,
        command: Box<dyn LaneCommand>,
    ) -> anyhow::Result<tokio::sync::oneshot::Receiver<LaneResult<Value>>> {
        Ok(self.manager.submit(lane_id, command).await?)
    }

    /// Start the queue scheduler
    pub async fn start(&self) -> anyhow::Result<()> {
        self.manager.start().await
    }

    /// Get queue statistics
    pub async fn stats(&self) -> anyhow::Result<QueueStats> {
        self.manager.stats().await
    }

    /// Initiate graceful shutdown
    pub async fn shutdown(&self) {
        self.manager.shutdown().await;
    }

    /// Drain pending commands with timeout
    pub async fn drain(&self, timeout: Duration) -> anyhow::Result<()> {
        Ok(self.manager.drain(timeout).await?)
    }

    /// Check if shutdown is in progress
    pub fn is_shutting_down(&self) -> bool {
        self.manager.is_shutting_down()
    }

    /// Get the underlying queue manager
    pub fn inner(&self) -> &Arc<QueueManager> {
        &self.manager
    }

    /// Get the underlying CommandQueue for monitoring
    pub fn queue(&self) -> Arc<a3s_lane::CommandQueue> {
        self.manager.queue()
    }

    /// Get metrics collector (Phase 2)
    pub fn metrics(&self) -> Option<&QueueMetrics> {
        self.metrics.as_ref()
    }

    /// Get alert manager (Phase 2)
    pub fn alerts(&self) -> Option<&Arc<AlertManager>> {
        self.alerts.as_ref()
    }

    /// Get metrics snapshot (Phase 2)
    pub async fn metrics_snapshot(&self) -> Option<a3s_lane::MetricsSnapshot> {
        if let Some(ref metrics) = self.metrics {
            Some(metrics.snapshot().await)
        } else {
            None
        }
    }
}

/// Configuration for EnhancedQueueManager with Phase 1-3 features
#[derive(Debug, Clone)]
pub struct EnhancedQueueConfig {
    // Phase 1: Core
    /// Event buffer size for the event emitter
    pub event_buffer_size: usize,
    /// Custom config for control lane
    pub control_config: Option<LaneConfig>,
    /// Custom config for query lane
    pub query_config: Option<LaneConfig>,
    /// Custom config for execute/skill lane
    pub execute_config: Option<LaneConfig>,
    /// Custom config for generate/prompt lane
    pub generate_config: Option<LaneConfig>,
    /// Dead letter queue max size (None = disabled)
    pub dlq_max_size: Option<usize>,

    // Phase 2: Observability
    /// Enable metrics collection
    pub enable_metrics: bool,
    /// Enable alerts
    pub enable_alerts: bool,
    /// Alert warning threshold for queue depth
    pub alert_warning_threshold: usize,
    /// Alert critical threshold for queue depth
    pub alert_critical_threshold: usize,

    // Phase 3: Reliability
    /// Default timeout for all lanes
    pub default_timeout: Option<Duration>,
    /// Timeout for execute/skill lane (tool execution)
    pub execute_timeout: Option<Duration>,
    /// Timeout for generate/prompt lane (LLM calls)
    pub generate_timeout: Option<Duration>,
    /// Default retry policy for all lanes
    pub default_retry_policy: Option<RetryPolicy>,
    /// Rate limit for query lane
    pub query_rate_limit: Option<RateLimitConfig>,
    /// Rate limit for generate/prompt lane (LLM API rate limiting)
    pub generate_rate_limit: Option<RateLimitConfig>,
    /// Priority boost for generate/prompt lane
    pub generate_priority_boost: Option<PriorityBoostConfig>,
    /// Persistent storage path (None = in-memory only)
    pub storage_path: Option<PathBuf>,
}

impl Default for EnhancedQueueConfig {
    fn default() -> Self {
        Self {
            // Phase 1
            event_buffer_size: 100,
            control_config: None,
            query_config: None,
            execute_config: None,
            generate_config: None,
            dlq_max_size: Some(1000),

            // Phase 2
            enable_metrics: true,
            enable_alerts: true,
            alert_warning_threshold: 50,
            alert_critical_threshold: 100,

            // Phase 3
            default_timeout: Some(Duration::from_secs(60)),
            execute_timeout: Some(Duration::from_secs(120)),
            generate_timeout: Some(Duration::from_secs(300)),
            default_retry_policy: Some(RetryPolicy::exponential(3)),
            query_rate_limit: None,
            generate_rate_limit: Some(RateLimitConfig::per_minute(60)),
            generate_priority_boost: Some(PriorityBoostConfig::standard(Duration::from_secs(300))),
            storage_path: None,
        }
    }
}

impl EnhancedQueueConfig {
    /// Create a minimal config without Phase 2-3 features
    pub fn minimal() -> Self {
        Self {
            event_buffer_size: 100,
            control_config: None,
            query_config: None,
            execute_config: None,
            generate_config: None,
            dlq_max_size: None,
            enable_metrics: false,
            enable_alerts: false,
            alert_warning_threshold: 50,
            alert_critical_threshold: 100,
            default_timeout: None,
            execute_timeout: None,
            generate_timeout: None,
            default_retry_policy: None,
            query_rate_limit: None,
            generate_rate_limit: None,
            generate_priority_boost: None,
            storage_path: None,
        }
    }

    /// Create config with persistent storage
    pub fn with_storage(mut self, path: impl Into<PathBuf>) -> Self {
        self.storage_path = Some(path.into());
        self
    }

    /// Set LLM rate limit (requests per minute)
    pub fn with_llm_rate_limit(mut self, requests_per_minute: u64) -> Self {
        self.generate_rate_limit = Some(RateLimitConfig::per_minute(requests_per_minute));
        self
    }

    /// Disable metrics and alerts
    pub fn without_observability(mut self) -> Self {
        self.enable_metrics = false;
        self.enable_alerts = false;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_lane_mapping() {
        assert_eq!(SessionLane::Control.to_lane_id(), "control");
        assert_eq!(SessionLane::Query.to_lane_id(), "query");
        assert_eq!(SessionLane::Execute.to_lane_id(), "skill");
        assert_eq!(SessionLane::Generate.to_lane_id(), "prompt");
    }

    #[test]
    fn test_lane_priority() {
        assert!(SessionLane::Control.to_priority() < SessionLane::Query.to_priority());
        assert!(SessionLane::Query.to_priority() < SessionLane::Execute.to_priority());
        assert!(SessionLane::Execute.to_priority() < SessionLane::Generate.to_priority());
    }

    #[test]
    fn test_lane_config() {
        let control_config = SessionLane::Control.to_lane_config();
        assert_eq!(control_config.min_concurrency, 1);
        assert_eq!(control_config.max_concurrency, 2);

        let query_config = SessionLane::Query.to_lane_config();
        assert_eq!(query_config.max_concurrency, 4);

        let generate_config = SessionLane::Generate.to_lane_config();
        assert_eq!(generate_config.max_concurrency, 1);
    }

    #[test]
    fn test_default_config() {
        let config = EnhancedQueueConfig::default();
        assert!(config.enable_metrics);
        assert!(config.enable_alerts);
        assert_eq!(config.dlq_max_size, Some(1000));
        assert!(config.default_timeout.is_some());
        assert!(config.default_retry_policy.is_some());
        assert!(config.generate_rate_limit.is_some());
    }

    #[test]
    fn test_minimal_config() {
        let config = EnhancedQueueConfig::minimal();
        assert!(!config.enable_metrics);
        assert!(!config.enable_alerts);
        assert!(config.dlq_max_size.is_none());
        assert!(config.default_timeout.is_none());
    }

    #[tokio::test]
    async fn test_enhanced_queue_manager_creation() {
        let manager = EnhancedQueueManager::new().await;
        assert!(manager.is_ok());
        let manager = manager.unwrap();
        assert!(manager.metrics().is_some());
        assert!(manager.alerts().is_some());
    }

    #[tokio::test]
    async fn test_enhanced_queue_manager_minimal() {
        let config = EnhancedQueueConfig::minimal();
        let manager = EnhancedQueueManager::with_config(config).await;
        assert!(manager.is_ok());
        let manager = manager.unwrap();
        assert!(manager.metrics().is_none());
        assert!(manager.alerts().is_none());
    }

    #[tokio::test]
    async fn test_simple_command_execution() {
        let manager = EnhancedQueueManager::new().await.unwrap();
        manager.start().await.unwrap();

        let command = SimpleCommand::new("test", serde_json::json!({"result": "success"}));

        let rx = manager
            .submit(SessionLane::Query, Box::new(command))
            .await
            .unwrap();

        let result = rx.await.unwrap().unwrap();
        assert_eq!(result["result"], "success");

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_queue_stats() {
        let manager = EnhancedQueueManager::new().await.unwrap();
        manager.start().await.unwrap();

        let stats = manager.stats().await.unwrap();
        assert!(stats.lanes.contains_key("control"));
        assert!(stats.lanes.contains_key("query"));
        assert!(stats.lanes.contains_key("skill"));
        assert!(stats.lanes.contains_key("prompt"));

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_metrics_snapshot() {
        let manager = EnhancedQueueManager::new().await.unwrap();
        manager.start().await.unwrap();

        let snapshot = manager.metrics_snapshot().await;
        assert!(snapshot.is_some());

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_with_storage_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config =
            EnhancedQueueConfig::default().with_storage(temp_dir.path().join("queue_data"));

        let manager = EnhancedQueueManager::with_config(config).await;
        assert!(manager.is_ok());
    }

    #[tokio::test]
    async fn test_llm_rate_limit_config() {
        let config = EnhancedQueueConfig::default().with_llm_rate_limit(30); // 30 requests per minute

        assert!(config.generate_rate_limit.is_some());
    }

    #[test]
    fn test_without_observability() {
        let config = EnhancedQueueConfig::default().without_observability();
        assert!(!config.enable_metrics);
        assert!(!config.enable_alerts);
    }

    #[tokio::test]
    async fn test_async_session_command() {
        let cmd = AsyncSessionCommand::new("test-cmd", || async {
            Ok(serde_json::json!({"result": "ok"}))
        });
        assert_eq!(cmd.command_type(), "test-cmd");
        let result = cmd.execute().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!({"result": "ok"}));
    }

    #[tokio::test]
    async fn test_async_session_command_error() {
        let cmd = AsyncSessionCommand::new("fail-cmd", || async {
            Err(anyhow::anyhow!("something went wrong"))
        });
        let result = cmd.execute().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_simple_command() {
        let cmd = SimpleCommand::new("simple", serde_json::json!(42));
        assert_eq!(cmd.command_type(), "simple");
        let result = cmd.execute().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!(42));
    }

    #[tokio::test]
    async fn test_enhanced_queue_manager_drain() {
        let manager = EnhancedQueueManager::new().await.unwrap();
        manager.start().await.unwrap();

        let result = manager.drain(Duration::from_millis(100)).await;
        assert!(result.is_ok());

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_enhanced_queue_manager_inner() {
        let manager = EnhancedQueueManager::new().await.unwrap();
        let _inner = manager.inner();
        let _queue = manager.queue();
        assert!(!manager.is_shutting_down());
    }

    #[tokio::test]
    async fn test_enhanced_queue_manager_submit_to_lane() {
        let manager = EnhancedQueueManager::new().await.unwrap();
        manager.start().await.unwrap();

        let cmd = Box::new(SimpleCommand::new("test", serde_json::json!("hello")));
        let rx = manager.submit_to_lane("query", cmd).await;
        assert!(rx.is_ok());

        manager.shutdown().await;
    }

    #[test]
    fn test_enhanced_queue_config_minimal() {
        let config = EnhancedQueueConfig::minimal();
        assert!(!config.enable_metrics);
        assert!(!config.enable_alerts);
        assert!(config.dlq_max_size.is_none());
        assert!(config.default_timeout.is_none());
        assert!(config.execute_timeout.is_none());
        assert!(config.generate_timeout.is_none());
        assert!(config.default_retry_policy.is_none());
        assert!(config.generate_rate_limit.is_none());
        assert!(config.storage_path.is_none());
    }

    #[test]
    fn test_enhanced_queue_config_default() {
        let config = EnhancedQueueConfig::default();
        assert!(config.enable_metrics);
        assert!(config.enable_alerts);
        assert_eq!(config.dlq_max_size, Some(1000));
        assert!(config.default_timeout.is_some());
        assert!(config.execute_timeout.is_some());
        assert!(config.generate_timeout.is_some());
        assert!(config.default_retry_policy.is_some());
        assert!(config.generate_rate_limit.is_some());
    }
}
