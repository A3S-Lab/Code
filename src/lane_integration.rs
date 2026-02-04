//! Integration layer between a3s-code and a3s-lane
//!
//! This module provides a bridge between the session-level command queue
//! and the a3s-lane priority queue system, enabling advanced features like
//! metrics, DLQ, retry policies, and persistent storage.

use a3s_lane::{
    Command as LaneCommand, EventEmitter, LaneConfig, LaneError, QueueManager, QueueManagerBuilder,
    QueueStats, Result as LaneResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::hitl::SessionLane;

/// Map SessionLane to a3s-lane lane IDs
impl SessionLane {
    /// Convert to a3s-lane lane ID string
    pub fn to_lane_id(&self) -> &'static str {
        match self {
            SessionLane::Control => "control",
            SessionLane::Query => "query",
            SessionLane::Execute => "skill", // Map to skill lane (tool execution)
            SessionLane::Generate => "prompt", // Map to prompt lane (LLM calls)
        }
    }

    /// Get lane configuration for a3s-lane
    pub fn to_lane_config(&self) -> LaneConfig {
        match self {
            // Control: High priority, low concurrency
            SessionLane::Control => LaneConfig::new(1, 2),
            // Query: Medium-high priority, higher concurrency (read-only)
            SessionLane::Query => LaneConfig::new(1, 4),
            // Execute: Medium priority, moderate concurrency (mutating)
            SessionLane::Execute => LaneConfig::new(1, 2),
            // Generate: Low priority, single concurrency (LLM calls)
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

/// Enhanced queue manager using a3s-lane
///
/// Provides a unified interface for submitting commands with priority-based
/// scheduling, metrics collection, and reliability features.
pub struct EnhancedQueueManager {
    manager: Arc<QueueManager>,
}

impl EnhancedQueueManager {
    /// Create a new enhanced queue manager with default session lanes
    pub async fn new() -> anyhow::Result<Self> {
        Self::with_config(EnhancedQueueConfig::default()).await
    }

    /// Create with custom configuration
    pub async fn with_config(config: EnhancedQueueConfig) -> anyhow::Result<Self> {
        let emitter = EventEmitter::new(config.event_buffer_size);

        let mut builder = QueueManagerBuilder::new(emitter);

        // Add session lanes with custom or default configs
        builder = builder
            .with_lane(
                "control",
                config
                    .control_config
                    .unwrap_or_else(|| SessionLane::Control.to_lane_config()),
                SessionLane::Control.to_priority(),
            )
            .with_lane(
                "query",
                config
                    .query_config
                    .unwrap_or_else(|| SessionLane::Query.to_lane_config()),
                SessionLane::Query.to_priority(),
            )
            .with_lane(
                "skill",
                config
                    .execute_config
                    .unwrap_or_else(|| SessionLane::Execute.to_lane_config()),
                SessionLane::Execute.to_priority(),
            )
            .with_lane(
                "prompt",
                config
                    .generate_config
                    .unwrap_or_else(|| SessionLane::Generate.to_lane_config()),
                SessionLane::Generate.to_priority(),
            );

        // Add DLQ if configured
        if let Some(dlq_size) = config.dlq_max_size {
            builder = builder.with_dlq(dlq_size);
        }

        let manager = builder.build().await?;

        Ok(Self {
            manager: Arc::new(manager),
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
        Ok(self.manager.start().await?)
    }

    /// Get queue statistics
    pub async fn stats(&self) -> anyhow::Result<QueueStats> {
        Ok(self.manager.stats().await?)
    }

    /// Initiate graceful shutdown
    pub async fn shutdown(&self) {
        self.manager.shutdown().await;
    }

    /// Drain pending commands with timeout
    pub async fn drain(&self, timeout: std::time::Duration) -> anyhow::Result<()> {
        Ok(self.manager.drain(timeout).await?)
    }

    /// Check if shutdown is in progress
    pub fn is_shutting_down(&self) -> bool {
        self.manager.is_shutting_down()
    }

    /// Get the underlying queue manager for advanced operations
    pub fn inner(&self) -> &Arc<QueueManager> {
        &self.manager
    }

    /// Get the underlying CommandQueue for monitoring
    pub fn queue(&self) -> Arc<a3s_lane::CommandQueue> {
        self.manager.queue()
    }
}

/// Configuration for EnhancedQueueManager
#[derive(Debug, Clone)]
pub struct EnhancedQueueConfig {
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
}

impl Default for EnhancedQueueConfig {
    fn default() -> Self {
        Self {
            event_buffer_size: 100,
            control_config: None,
            query_config: None,
            execute_config: None,
            generate_config: None,
            dlq_max_size: Some(1000), // Enable DLQ by default
        }
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

    #[tokio::test]
    async fn test_enhanced_queue_manager_creation() {
        let manager = EnhancedQueueManager::new().await;
        assert!(manager.is_ok());
    }

    #[tokio::test]
    async fn test_enhanced_queue_manager_with_config() {
        let config = EnhancedQueueConfig {
            event_buffer_size: 50,
            control_config: Some(LaneConfig::new(1, 4)),
            dlq_max_size: Some(500),
            ..Default::default()
        };

        let manager = EnhancedQueueManager::with_config(config).await;
        assert!(manager.is_ok());
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
}

