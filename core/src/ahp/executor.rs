// AHP Hook Executor Implementation
//
// Bridges A3S Code's hook system with AHP protocol

use super::runtime_state::{handshake_capabilities, AhpRuntimeSnapshot, AhpRuntimeState};
use crate::agent::AgentEvent;
use crate::error::{read_or_recover, write_or_recover};
use crate::hooks::{HookEvent, HookEventType, HookExecutor, HookResult};
use a3s_ahp::{AhpClient, AhpEvent, EventType, HeartbeatEvent, IdleEvent, SessionStats, Transport};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// AHP Hook Executor
///
/// Implements `HookExecutor` trait to forward A3S Code hook events
/// to an external AHP harness server for supervision.
#[derive(Clone)]
pub struct AhpHookExecutor {
    client: Arc<AhpClient>,
    agent_id: String,
    depth: u32,
    /// Last activity timestamp for idle detection
    last_activity: Arc<AtomicU64>,
    /// Idle threshold in milliseconds - fire Idle event after this duration of inactivity
    idle_threshold_ms: u64,
    /// Start time of the executor
    start_time: Instant,
    /// Total events processed
    total_events: Arc<AtomicU64>,
    /// Error count for session stats
    error_count: Arc<AtomicU64>,
    /// Runtime counters observed from durable AgentEvents.
    runtime_state: Arc<AhpRuntimeState>,
    /// Client-exposed capabilities for the server to use.
    capabilities: HashMap<String, serde_json::Value>,
    /// Shutdown signal for background tasks
    shutdown: Arc<AtomicBool>,
    /// Memory summary for context (set via set_memory_summary)
    memory_summary: Arc<RwLock<Option<a3s_ahp::MemorySummary>>>,
    /// Current task description for context (set via set_current_task)
    current_task: Arc<RwLock<Option<String>>>,
    /// Recent facts for context (set via add_recent_fact)
    recent_facts: Arc<RwLock<Vec<a3s_ahp::Fact>>>,
    /// Current workspace path (set via set_workspace)
    workspace: Arc<RwLock<Option<String>>>,
    /// Batch accumulator for non-blocking events
    batch_buffer: Arc<RwLock<Vec<a3s_ahp::AhpEvent>>>,
    /// Batch size threshold (default 10)
    batch_size: usize,
    /// Batch flush timeout in milliseconds (default 5000)
    batch_timeout_ms: u64,
    /// Last batch flush timestamp
    last_batch_flush: Arc<AtomicU64>,
    /// Enable batch processing
    batch_enabled: bool,
}

impl std::fmt::Debug for AhpHookExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AhpHookExecutor")
            .field("agent_id", &self.agent_id)
            .field("depth", &self.depth)
            .field("idle_threshold_ms", &self.idle_threshold_ms)
            .finish()
    }
}

impl AhpHookExecutor {
    /// Create a new AHP hook executor
    ///
    /// # Arguments
    ///
    /// * `transport` - AHP transport (stdio, HTTP, WebSocket)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use a3s_code_core::ahp::{AhpHookExecutor, AhpTransport};
    ///
    /// # async fn example() -> anyhow::Result<()> {
    /// let executor = AhpHookExecutor::new(
    ///     AhpTransport::Http {
    ///         url: "http://localhost:8080/ahp".to_string(),
    ///         auth: None,
    ///     }
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(transport: Transport) -> Result<Self, a3s_ahp::AhpError> {
        Self::new_with_config(transport, 10_000).await // Default 10s idle threshold
    }

    /// Create with custom idle threshold
    pub async fn new_with_config(
        transport: Transport,
        idle_threshold_ms: u64,
    ) -> Result<Self, a3s_ahp::AhpError> {
        let client = AhpClient::new(transport).await?;

        // Perform handshake with capabilities
        client.handshake(handshake_capabilities()).await?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Ok(Self {
            client: Arc::new(client),
            agent_id: uuid::Uuid::new_v4().to_string(),
            depth: 0,
            last_activity: Arc::new(AtomicU64::new(now)),
            idle_threshold_ms,
            start_time: Instant::now(),
            total_events: Arc::new(AtomicU64::new(0)),
            error_count: Arc::new(AtomicU64::new(0)),
            runtime_state: Arc::new(AhpRuntimeState::default()),
            capabilities: HashMap::new(),
            shutdown: Arc::new(AtomicBool::new(false)),
            memory_summary: Arc::new(RwLock::new(None)),
            current_task: Arc::new(RwLock::new(None)),
            recent_facts: Arc::new(RwLock::new(Vec::new())),
            workspace: Arc::new(RwLock::new(None)),
            batch_buffer: Arc::new(RwLock::new(Vec::new())),
            batch_size: 10,
            batch_timeout_ms: 5000,
            last_batch_flush: Arc::new(AtomicU64::new(now)),
            batch_enabled: false,
        })
    }

    /// Create a new executor for testing with a pre-configured client.
    ///
    /// This bypasses the handshake step, allowing integration tests to use
    /// a mock transport without requiring a running AHP server.
    ///
    /// # Arguments
    ///
    /// * `client` - Pre-configured AhpClient (typically with a mock transport)
    /// * `idle_threshold_ms` - Idle threshold in milliseconds
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use a3s_ahp::transport::TransportLayer;
    ///
    /// let mock_transport = MockTransport::new();
    /// let client = AhpClient::new_for_testing(Arc::new(mock_transport));
    /// let executor = AhpHookExecutor::new_for_testing(client, 10_000);
    /// ```
    pub fn new_for_testing(client: Arc<AhpClient>, idle_threshold_ms: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Self {
            client,
            agent_id: uuid::Uuid::new_v4().to_string(),
            depth: 0,
            last_activity: Arc::new(AtomicU64::new(now)),
            idle_threshold_ms,
            start_time: Instant::now(),
            total_events: Arc::new(AtomicU64::new(0)),
            error_count: Arc::new(AtomicU64::new(0)),
            runtime_state: Arc::new(AhpRuntimeState::default()),
            capabilities: HashMap::new(),
            shutdown: Arc::new(AtomicBool::new(false)),
            memory_summary: Arc::new(RwLock::new(None)),
            current_task: Arc::new(RwLock::new(None)),
            recent_facts: Arc::new(RwLock::new(Vec::new())),
            workspace: Arc::new(RwLock::new(None)),
            batch_buffer: Arc::new(RwLock::new(Vec::new())),
            batch_size: 10,
            batch_timeout_ms: 5000,
            last_batch_flush: Arc::new(AtomicU64::new(now)),
            batch_enabled: false,
        }
    }

    /// Create with specific agent ID and depth
    pub async fn with_context(
        transport: Transport,
        agent_id: String,
        depth: u32,
    ) -> Result<Self, a3s_ahp::AhpError> {
        Self::with_context_and_config(transport, agent_id, depth, 10_000).await
    }

    /// Create with specific agent ID, depth, and custom idle threshold
    pub async fn with_context_and_config(
        transport: Transport,
        agent_id: String,
        depth: u32,
        idle_threshold_ms: u64,
    ) -> Result<Self, a3s_ahp::AhpError> {
        let client = AhpClient::new(transport).await?;

        client.handshake(handshake_capabilities()).await?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        Ok(Self {
            client: Arc::new(client),
            agent_id,
            depth,
            last_activity: Arc::new(AtomicU64::new(now)),
            idle_threshold_ms,
            start_time: Instant::now(),
            total_events: Arc::new(AtomicU64::new(0)),
            error_count: Arc::new(AtomicU64::new(0)),
            runtime_state: Arc::new(AhpRuntimeState::default()),
            capabilities: HashMap::new(),
            shutdown: Arc::new(AtomicBool::new(false)),
            memory_summary: Arc::new(RwLock::new(None)),
            current_task: Arc::new(RwLock::new(None)),
            recent_facts: Arc::new(RwLock::new(Vec::new())),
            workspace: Arc::new(RwLock::new(None)),
            batch_buffer: Arc::new(RwLock::new(Vec::new())),
            batch_size: 10,
            batch_timeout_ms: 5000,
            last_batch_flush: Arc::new(AtomicU64::new(now)),
            batch_enabled: false,
        })
    }

    /// Builder method to add client-exposed capabilities.
    ///
    /// Capabilities allow the server to interact with the agent by calling
    /// exposed functions/URLs. Common capabilities:
    /// - `memory_search`: Search across memories
    /// - `session_info`: Get current session information
    /// - `cross_session`: Query cross-session data
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use a3s_code_core::ahp::{AhpHookExecutor, AhpTransport};
    ///
    /// # async fn example() -> anyhow::Result<()> {
    /// let executor = AhpHookExecutor::new(
    ///     AhpTransport::Http {
    ///         url: "http://localhost:8080/ahp".to_string(),
    ///         auth: None,
    ///     }
    /// )
    /// .await?
    /// .with_capabilities(vec![
    ///     ("memory_search".into(), serde_json::json!({
    ///         "type": "http",
    ///         "url": "http://localhost:8080/memory/search"
    ///     })),
    ///     ("session_info".into(), serde_json::json!({
    ///         "type": "query",
    ///         "handler": "get_session_info"
    ///     })),
    /// ]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = (String, serde_json::Value)>,
    ) -> Self {
        for (key, value) in capabilities {
            self.capabilities.insert(key, value);
        }
        self
    }

    /// Add a single capability
    pub fn add_capability(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.capabilities.insert(key.into(), value);
        self
    }

    /// Record an error for session stats.
    pub fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total events processed.
    pub fn total_events_count(&self) -> u64 {
        self.total_events.load(Ordering::Relaxed)
    }

    /// Get error count.
    pub fn error_count_value(&self) -> u64 {
        self.error_count.load(Ordering::Relaxed)
    }

    /// Return the current runtime counters observed by AHP.
    pub fn runtime_snapshot(&self) -> AhpRuntimeSnapshot {
        let active_tools = self.runtime_state.active_tools();
        let pending_actions = self.runtime_state.pending_actions();
        let queue_depth = self.runtime_state.queue_depth();
        let tokens_used = self.runtime_state.tokens_used();
        let total_events_processed = self.total_events.load(Ordering::Relaxed);
        let error_count = self.error_count.load(Ordering::Relaxed) as usize;
        let uptime_ms = self.start_time.elapsed().as_millis() as u64;
        let current_state = if pending_actions > 0 {
            "waiting"
        } else if active_tools > 0 {
            "running_tools"
        } else if self.get_idle_duration_ms() >= self.idle_threshold_ms {
            "idle"
        } else {
            "active"
        }
        .to_string();

        AhpRuntimeSnapshot {
            active_tools,
            pending_actions,
            queue_depth,
            tokens_used,
            total_events_processed,
            error_count,
            uptime_ms,
            current_state,
        }
    }

    /// Get idle duration in milliseconds.
    pub fn get_idle_duration_ms(&self) -> u64 {
        let last = self.last_activity.load(Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        now.saturating_sub(last)
    }

    /// Check if agent is idle and create idle event if threshold exceeded.
    pub fn check_idle(&self) -> Option<IdleEvent> {
        let elapsed = self.get_idle_duration_ms();
        if elapsed >= self.idle_threshold_ms {
            Some(IdleEvent {
                idle_duration_ms: elapsed,
                idle_reason: "no_activity".to_string(),
                last_event_type: None,
                suggested_action: Some("dream".to_string()),
            })
        } else {
            None
        }
    }

    /// Set memory summary for context population.
    ///
    /// This allows the executor to include memory statistics in the EventContext.
    pub fn set_memory_summary(self: Arc<Self>, summary: a3s_ahp::MemorySummary) {
        let mut lock = write_or_recover(&self.memory_summary);
        *lock = Some(summary);
    }

    /// Set current task description for context population.
    ///
    /// This allows the executor to include the current task in the EventContext.
    pub fn set_current_task(self: Arc<Self>, task: String) {
        let mut lock = write_or_recover(&self.current_task);
        *lock = Some(task);
    }

    /// Add a recent fact for context population.
    ///
    /// Facts are used for Retrieve intent in ContextPerception events.
    pub fn add_recent_fact(self: Arc<Self>, fact: a3s_ahp::Fact) {
        let mut lock = write_or_recover(&self.recent_facts);
        lock.push(fact);
    }

    /// Set recent facts for context population (replaces existing facts).
    ///
    /// Facts are used for Retrieve intent in ContextPerception events.
    pub fn set_recent_facts(self: Arc<Self>, facts: Vec<a3s_ahp::Fact>) {
        let mut lock = write_or_recover(&self.recent_facts);
        *lock = facts;
    }

    /// Get a clone of recent facts.
    pub fn get_recent_facts(&self) -> Vec<a3s_ahp::Fact> {
        read_or_recover(&self.recent_facts).clone()
    }

    /// Set workspace path for context population.
    ///
    /// This allows the executor to include the current workspace in the EventContext.
    pub fn set_workspace(self: Arc<Self>, workspace: String) {
        let mut lock = write_or_recover(&self.workspace);
        *lock = Some(workspace);
    }

    /// Get workspace path.
    pub fn get_workspace(&self) -> Option<String> {
        read_or_recover(&self.workspace).clone()
    }

    /// Send a query to the harness and wait for response.
    ///
    /// This allows the agent to request guidance or information from the harness.
    /// Used for clarify actions, request approvals, or query harness knowledge.
    pub async fn query(
        &self,
        query_type: impl Into<String>,
        payload: serde_json::Value,
    ) -> Result<a3s_ahp::QueryResponse, a3s_ahp::AhpError> {
        self.client.query(query_type, payload).await
    }

    /// Send a batch of events to the harness.
    ///
    /// This allows non-blocking events to be batched for efficiency.
    /// The harness processes them and returns a batch response.
    pub async fn send_batch(
        &self,
        events: Vec<a3s_ahp::AhpEvent>,
    ) -> Result<a3s_ahp::BatchResponse, a3s_ahp::AhpError> {
        self.client.send_batch(events).await
    }

    /// Enable batch processing for non-blocking events.
    ///
    /// When enabled, non-blocking events are accumulated and sent in batches
    /// either when batch_size is reached or batch_timeout_ms expires.
    pub fn with_batch_config(mut self, batch_size: usize, batch_timeout_ms: u64) -> Self {
        self.batch_size = batch_size;
        self.batch_timeout_ms = batch_timeout_ms;
        self.batch_enabled = true;
        self
    }

    /// Add an event to the batch buffer.
    ///
    /// Returns true if the batch should be flushed (size threshold reached).
    pub async fn add_to_batch(&self, event: a3s_ahp::AhpEvent) -> bool {
        let should_flush = {
            let mut buffer = write_or_recover(&self.batch_buffer);
            buffer.push(event);
            buffer.len() >= self.batch_size
        };

        if should_flush {
            self.flush_batch().await;
        }

        should_flush
    }

    /// Flush the batch buffer and send all events.
    pub async fn flush_batch(&self) {
        let events = {
            let mut buffer = write_or_recover(&self.batch_buffer);
            if buffer.is_empty() {
                return;
            }
            std::mem::take(&mut *buffer)
        };

        if !events.is_empty() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            self.last_batch_flush.store(now, Ordering::Relaxed);

            match self.client.send_batch(events).await {
                Ok(_) => {
                    debug!("Batch sent successfully");
                }
                Err(e) => {
                    warn!("Batch send failed: {}", e);
                }
            }
        }
    }

    /// Check if batch timeout has expired and flush if needed.
    pub async fn check_batch_timeout(&self) {
        let elapsed = {
            let last = self.last_batch_flush.load(Ordering::Relaxed);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            now.saturating_sub(last)
        };

        if elapsed >= self.batch_timeout_ms {
            self.flush_batch().await;
        }
    }

    /// Publish durable AHP contract events derived from runtime AgentEvents.
    pub async fn publish_agent_event(&self, event: &AgentEvent, run_id: &str, session_id: &str) {
        self.record_event();
        self.record_runtime_agent_event(event, run_id);

        let mut events = crate::ahp::agent_event_to_ahp_events(
            event,
            run_id,
            session_id,
            &self.agent_id,
            self.depth,
        );
        if events.is_empty() {
            return;
        }
        let context = self.build_context();
        for event in &mut events {
            event.context = context.clone();
        }

        let should_flush = matches!(event, AgentEvent::End { .. } | AgentEvent::Error { .. });

        if self.batch_enabled {
            for event in events {
                self.add_to_batch(event).await;
            }
            if should_flush {
                self.flush_batch().await;
            }
            return;
        }

        for event in events {
            if let Err(e) = self.client.send_event_full(&event).await {
                warn!("AHP runtime contract event failed: {}", e);
            }
        }
    }

    /// Publish an explicit run cancellation lifecycle event.
    pub async fn publish_run_cancelled(
        &self,
        run_id: &str,
        session_id: &str,
        reason: Option<&str>,
    ) {
        self.record_event();
        self.runtime_state.clear_runtime_actions();
        let mut event =
            crate::ahp::cancelled_run_event(run_id, session_id, &self.agent_id, self.depth, reason);
        event.context = self.build_context();

        if self.batch_enabled {
            self.add_to_batch(event).await;
            self.flush_batch().await;
            return;
        }

        if let Err(e) = self.client.send_event_full(&event).await {
            warn!("AHP run cancellation event failed: {}", e);
        }
    }

    /// Start background tasks for heartbeat and idle detection.
    ///
    /// This method spawns two background Tokio tasks:
    /// - Heartbeat: sends HeartbeatEvent every 60 seconds
    /// - Idle detection: checks idle state every 5 seconds, fires IdleEvent if threshold exceeded
    ///
    /// The tasks run until the shutdown signal is set or the executor is dropped.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use a3s_code_core::ahp::{AhpHookExecutor, AhpTransport};
    ///
    /// # async fn example() -> anyhow::Result<()> {
    /// let executor = std::sync::Arc::new(AhpHookExecutor::new(
    ///     AhpTransport::Http {
    ///         url: "http://localhost:8080/ahp".to_string(),
    ///         auth: None,
    ///     }
    /// ).await?);
    ///
    /// // Start background heartbeat and idle detection
    /// executor.execute_background();
    ///
    /// // Executor is now supervised in the background
    /// # Ok(())
    /// # }
    /// ```
    pub fn execute_background(self: Arc<Self>) {
        let shutdown = Arc::clone(&self.shutdown);
        shutdown.store(false, Ordering::Relaxed);

        // Spawn heartbeat task
        let heartbeat_executor = Arc::clone(&self);
        let heartbeat_shutdown = Arc::clone(&shutdown);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if heartbeat_shutdown.load(Ordering::Relaxed) {
                            debug!("Heartbeat task shutting down");
                            break;
                        }
                        let event = AhpEvent {
                            event_type: EventType::Heartbeat,
                            session_id: heartbeat_executor.agent_id.clone(),
                            agent_id: heartbeat_executor.agent_id.clone(),
                            timestamp: Utc::now().to_rfc3339(),
                            depth: heartbeat_executor.depth,
                            payload: serde_json::to_value(heartbeat_executor.heartbeat_payload())
                                .unwrap_or_default(),
                            context: heartbeat_executor.build_context(),
                            metadata: None,
                        };
                        if let Err(e) = heartbeat_executor.client.send_event_full(&event).await {
                            warn!("Heartbeat failed: {}", e);
                        }
                    }
                }
            }
        });

        // Spawn idle detection task
        let idle_executor = Arc::clone(&self);
        let idle_shutdown = Arc::clone(&shutdown);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if idle_shutdown.load(Ordering::Relaxed) {
                            debug!("Idle detection task shutting down");
                            break;
                        }
                        if let Some(idle_event) = idle_executor.check_idle() {
                            debug!("Idle detected, sending IdleEvent");
                            let event = AhpEvent {
                                event_type: EventType::Idle,
                                session_id: idle_executor.agent_id.clone(),
                                agent_id: idle_executor.agent_id.clone(),
                                timestamp: Utc::now().to_rfc3339(),
                                depth: idle_executor.depth,
                                payload: serde_json::to_value(idle_event).unwrap_or_default(),
                                context: idle_executor.build_context(),
                                metadata: None,
                            };
                            // Wait for idle decision (blocking)
                            match idle_executor.client.send_event_full(&event).await {
                                Ok(decision) => {
                                    debug!("Idle decision: {:?}", decision);
                                    match decision {
                                        a3s_ahp::Decision::Defer { .. } => {
                                            // Increase threshold temporarily
                                        }
                                        _ => {
                                            // Reset idle detection
                                            idle_executor.update_activity();
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Idle decision failed: {}", e);
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    /// Stop background tasks (heartbeat and idle detection).
    ///
    /// This signals the background tasks to shut down gracefully.
    pub fn stop_background(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// Get the agent ID
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Get the depth
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// Get idle threshold in milliseconds
    pub fn idle_threshold(&self) -> u64 {
        self.idle_threshold_ms
    }

    /// Update last activity timestamp
    fn update_activity(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.last_activity.store(now, Ordering::Relaxed);
    }

    /// Increment event counter and update activity
    fn record_event(&self) {
        self.total_events.fetch_add(1, Ordering::Relaxed);
        self.update_activity();
    }

    fn record_runtime_agent_event(&self, event: &AgentEvent, run_id: &str) {
        if matches!(event, AgentEvent::Error { .. }) {
            self.record_error();
        }
        self.runtime_state.observe_agent_event(event, run_id);
    }

    fn record_hook_event_state(&self, event: &HookEvent) {
        if matches!(event, HookEvent::OnError(_)) {
            self.record_error();
        }
        self.runtime_state.observe_hook_event(event);
    }

    fn heartbeat_payload(&self) -> HeartbeatEvent {
        let snapshot = self.runtime_snapshot();
        HeartbeatEvent {
            uptime_ms: snapshot.uptime_ms,
            total_events_processed: snapshot.total_events_processed,
            current_state: snapshot.current_state,
            cpu_percent: None,
            memory_bytes: None,
            active_tools: Some(snapshot.active_tools),
            pending_actions: Some(snapshot.pending_actions),
            queue_depth: Some(snapshot.queue_depth),
            tokens_used: Some(snapshot.tokens_used),
        }
    }

    /// Map A3S Code hook event to AHP event
    fn map_event(&self, event: &HookEvent) -> Option<AhpEvent> {
        super::event_mapping::map_hook_event(
            event,
            &self.agent_id,
            self.depth,
            read_or_recover(&self.workspace).clone(),
            self.build_context(),
        )
    }

    /// Build EventContext with runtime state and client-exposed capabilities.
    ///
    /// Session stats are always populated from AHP's observed runtime state so
    /// harnesses can make advisory decisions without a separate in-core advisor.
    /// Memory summary and current task are populated if set via setter methods.
    fn build_context(&self) -> Option<a3s_ahp::EventContext> {
        let snapshot = self.runtime_snapshot();

        // Build session stats from tracked data
        let session_stats = SessionStats {
            total_actions: snapshot.total_events_processed as usize,
            total_tokens: snapshot.tokens_used,
            duration_ms: snapshot.uptime_ms,
            error_count: snapshot.error_count,
        };

        // Get optional memory summary
        let memory_summary = read_or_recover(&self.memory_summary).clone();

        // Get optional current task
        let current_task = read_or_recover(&self.current_task).clone();

        // Get recent facts
        let recent_facts = read_or_recover(&self.recent_facts).clone();

        let capabilities = if self.capabilities.is_empty() {
            None
        } else {
            Some(self.capabilities.clone())
        };

        Some(a3s_ahp::EventContext {
            recent_facts: (!recent_facts.is_empty()).then_some(recent_facts),
            memory_summary,
            session_stats: Some(session_stats),
            current_task,
            capabilities,
        })
    }

    /// Map AHP decision to hook result based on event type.
    fn map_decision(
        &self,
        event_type: EventType,
        decision_payload: serde_json::Value,
    ) -> HookResult {
        super::decision::map_decision(event_type, decision_payload)
    }

    /// Check if event type requires blocking (synchronous) response
    fn is_blocking_event(&self, event_type: HookEventType) -> bool {
        matches!(
            event_type,
            HookEventType::PreToolUse
                | HookEventType::PrePrompt
                | HookEventType::GenerateStart
                | HookEventType::PreContextPerception
                | HookEventType::PreMemoryRecall
                | HookEventType::PrePlanning
                | HookEventType::PreReasoning
                | HookEventType::OnConfirmation
                | HookEventType::IntentDetection
        )
    }
}

#[async_trait]
impl HookExecutor for AhpHookExecutor {
    async fn fire(&self, event: &HookEvent) -> HookResult {
        // Record this event (updates activity timestamp and counter)
        self.record_event();
        self.record_hook_event_state(event);

        // Map to AHP event
        let ahp_event = match self.map_event(event) {
            Some(e) => e,
            None => {
                // Event not mapped to AHP, allow by default
                debug!("Event {:?} not mapped to AHP, allowing", event.event_type());
                return HookResult::Continue(None);
            }
        };

        // Check if this is a blocking event
        let is_blocking = self.is_blocking_event(event.event_type());

        if is_blocking {
            // Flush any pending batch before sending blocking event
            if self.batch_enabled {
                self.flush_batch().await;
            }

            // Send event and wait for decision
            match self.client.send_event_full(&ahp_event).await {
                Ok(decision) => {
                    let decision_payload = serde_json::to_value(decision).unwrap_or_default();
                    debug!(
                        "AHP decision for {:?}: {:?}",
                        ahp_event.event_type, decision_payload
                    );
                    self.map_decision(ahp_event.event_type, decision_payload)
                }
                Err(e) => {
                    warn!("AHP error: {}, allowing by default", e);
                    HookResult::Continue(None)
                }
            }
        } else if self.batch_enabled {
            // Batch mode: accumulate non-blocking events
            self.add_to_batch(ahp_event).await;
            HookResult::Continue(None)
        } else {
            // Fire-and-forget for non-blocking events.
            let client = self.client.clone();
            let event = ahp_event;
            tokio::spawn(async move {
                if let Err(e) = client.send_event_full(&event).await {
                    warn!("AHP fire-and-forget error: {}", e);
                }
            });
            HookResult::Continue(None)
        }
    }

    async fn record_agent_event(&self, event: &AgentEvent, run_id: &str, session_id: &str) {
        self.publish_agent_event(event, run_id, session_id).await;
    }

    async fn record_run_cancelled(&self, run_id: &str, session_id: &str, reason: Option<&str>) {
        self.publish_run_cancelled(run_id, session_id, reason).await;
    }
}

#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;
