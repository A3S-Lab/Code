// AHP Hook Executor Implementation
//
// Bridges A3S Code's hook system with AHP protocol

use crate::hooks::{HookEvent, HookEventType, HookExecutor, HookResult};
use a3s_ahp::{
    AhpClient, AhpEvent, Decision, EventType, HeartbeatEvent, IdleEvent, MemorySummary,
    SessionStats, Transport,
};
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
    /// Client自主 exposes capabilities for the server to use
    capabilities: HashMap<String, serde_json::Value>,
    /// Shutdown signal for background tasks
    shutdown: Arc<AtomicBool>,
    /// Memory summary for context (set via set_memory_summary)
    memory_summary: Arc<RwLock<Option<a3s_ahp::MemorySummary>>>,
    /// Current task description for context (set via set_current_task)
    current_task: Arc<RwLock<Option<String>>>,
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
    ///     AhpTransport::http("http://localhost:8080/ahp", None)
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

        // Perform handshake
        client.handshake().await?;

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
            capabilities: HashMap::new(),
            shutdown: Arc::new(AtomicBool::new(false)),
            memory_summary: Arc::new(RwLock::new(None)),
            current_task: Arc::new(RwLock::new(None)),
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
            capabilities: HashMap::new(),
            shutdown: Arc::new(AtomicBool::new(false)),
            memory_summary: Arc::new(RwLock::new(None)),
            current_task: Arc::new(RwLock::new(None)),
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
        client.handshake().await?;

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
            capabilities: HashMap::new(),
            shutdown: Arc::new(AtomicBool::new(false)),
            memory_summary: Arc::new(RwLock::new(None)),
            current_task: Arc::new(RwLock::new(None)),
            batch_buffer: Arc::new(RwLock::new(Vec::new())),
            batch_size: 10,
            batch_timeout_ms: 5000,
            last_batch_flush: Arc::new(AtomicU64::new(now)),
            batch_enabled: false,
        })
    }

    /// Builder method to add client自主 exposes capabilities.
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
    ///     AhpTransport::http("http://localhost:8080/ahp", None)?
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
        let mut lock = self.memory_summary.write().unwrap();
        *lock = Some(summary);
    }

    /// Set current task description for context population.
    ///
    /// This allows the executor to include the current task in the EventContext.
    pub fn set_current_task(self: Arc<Self>, task: String) {
        let mut lock = self.current_task.write().unwrap();
        *lock = Some(task);
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
            let mut buffer = self.batch_buffer.write().unwrap();
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
            let mut buffer = self.batch_buffer.write().unwrap();
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
    /// let executor = AhpHookExecutor::new(
    ///     AhpTransport::http("http://localhost:8080/ahp", None)
    /// ).await?;
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
                            payload: serde_json::to_value(HeartbeatEvent {
                                uptime_ms: heartbeat_executor.start_time.elapsed().as_millis() as u64,
                                total_events_processed: heartbeat_executor.total_events.load(Ordering::Relaxed),
                                current_state: "active".to_string(),
                            }).unwrap_or_default(),
                            context: heartbeat_executor.build_context(),
                            metadata: None,
                        };
                        if let Err(e) = heartbeat_executor.client.send_event(event.event_type.clone(), event.payload.clone()).await {
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
                            match idle_executor.client.send_event(event.event_type.clone(), event.payload.clone()).await {
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

    /// Map A3S Code hook event to AHP event
    fn map_event(&self, event: &HookEvent) -> Option<AhpEvent> {
        let (event_type, payload) = match event {
            HookEvent::PreToolUse(e) => (
                EventType::PreAction,
                serde_json::json!({
                    "tool": e.tool,
                    "arguments": e.args,
                    "working_directory": e.working_directory,
                    "recent_tools": e.recent_tools,
                }),
            ),
            HookEvent::PostToolUse(e) => (
                EventType::PostAction,
                serde_json::json!({
                    "tool": e.tool,
                    "arguments": e.args,
                    "result": {
                        "success": e.result.success,
                        "output": e.result.output,
                        "exit_code": e.result.exit_code,
                        "duration_ms": e.result.duration_ms,
                    }
                }),
            ),
            HookEvent::PrePrompt(e) => (
                EventType::PrePrompt,
                serde_json::json!({
                    "prompt": e.prompt,
                    "system_prompt": e.system_prompt,
                    "message_count": e.message_count,
                }),
            ),
            HookEvent::GenerateStart(e) => (
                EventType::PrePrompt,
                serde_json::json!({
                    "prompt": e.prompt,
                    "session_id": e.session_id,
                }),
            ),
            HookEvent::PostResponse(e) => (
                EventType::PostAction,
                serde_json::json!({
                    "response_text": e.response_text,
                    "tool_calls_count": e.tool_calls_count,
                    "usage": e.usage,
                    "duration_ms": e.duration_ms,
                }),
            ),
            HookEvent::SessionStart(e) => (
                EventType::SessionStart,
                serde_json::json!({
                    "session_id": e.session_id,
                    "system_prompt": e.system_prompt,
                    "model_provider": e.model_provider,
                    "model_name": e.model_name,
                }),
            ),
            HookEvent::SessionEnd(e) => (
                EventType::SessionEnd,
                serde_json::json!({
                    "session_id": e.session_id,
                    "duration_ms": e.duration_ms,
                }),
            ),
            HookEvent::OnError(e) => (
                EventType::Error,
                serde_json::json!({
                    "error_type": format!("{:?}", e.error_type),
                    "error_message": e.error_message,
                    "context": e.context,
                }),
            ),
            // Events not mapped to AHP
            HookEvent::GenerateEnd(_) | HookEvent::SkillLoad(_) | HookEvent::SkillUnload(_) => {
                return None;
            }
        };

        Some(AhpEvent {
            event_type,
            session_id: self.extract_session_id(event),
            agent_id: self.agent_id.clone(),
            timestamp: Utc::now().to_rfc3339(),
            depth: self.depth,
            payload,
            context: self.build_context(),
            metadata: None,
        })
    }

    /// Build EventContext with client自主 exposes capabilities.
    ///
    /// The capabilities field is always populated if any capabilities were set.
    /// Session stats are populated from the executor's tracked data.
    /// Memory summary and current task are populated if set via setter methods.
    fn build_context(&self) -> Option<a3s_ahp::EventContext> {
        // Always include capabilities if any were set
        if self.capabilities.is_empty() {
            return None;
        }

        // Build session stats from tracked data
        let session_stats = SessionStats {
            total_actions: self.total_events.load(Ordering::Relaxed) as usize,
            total_tokens: 0, // Requires LLM client access
            duration_ms: self.start_time.elapsed().as_millis() as u64,
            error_count: self.error_count.load(Ordering::Relaxed) as usize,
        };

        // Get optional memory summary
        let memory_summary = self.memory_summary.read().unwrap().clone();

        // Get optional current task
        let current_task = self.current_task.read().unwrap().clone();

        Some(a3s_ahp::EventContext {
            recent_facts: None,
            memory_summary,
            session_stats: Some(session_stats),
            current_task,
            capabilities: Some(self.capabilities.clone()),
        })
    }

    /// Extract session ID from hook event
    fn extract_session_id(&self, event: &HookEvent) -> String {
        match event {
            HookEvent::PreToolUse(e) => e.session_id.clone(),
            HookEvent::PostToolUse(e) => e.session_id.clone(),
            HookEvent::GenerateStart(e) => e.session_id.clone(),
            HookEvent::SessionStart(e) => e.session_id.clone(),
            HookEvent::SessionEnd(e) => e.session_id.clone(),
            _ => self.agent_id.clone(),
        }
    }

    /// Map AHP decision to hook result
    fn map_decision(&self, decision: Decision) -> HookResult {
        match decision {
            Decision::Allow {
                modified_payload, ..
            } => {
                if let Some(modified) = modified_payload {
                    HookResult::Continue(Some(modified))
                } else {
                    HookResult::Continue(None)
                }
            }
            Decision::Block { reason, .. } => HookResult::Block(reason),
            Decision::Defer {
                retry_after_ms,
                reason,
            } => {
                if let Some(r) = reason {
                    debug!("AHP defer: {}", r);
                }
                HookResult::Retry(retry_after_ms)
            }
            Decision::Modify {
                modified_payload, ..
            } => HookResult::Continue(Some(modified_payload)),
            Decision::Escalate { reason, .. } => {
                // Escalate is treated as block for now
                // TODO: Implement human-in-the-loop escalation
                HookResult::Block(reason)
            }
        }
    }

    /// Check if event type requires blocking (synchronous) response
    fn is_blocking_event(&self, event_type: HookEventType) -> bool {
        matches!(
            event_type,
            HookEventType::PreToolUse | HookEventType::PrePrompt | HookEventType::GenerateStart
        )
    }
}

#[async_trait]
impl HookExecutor for AhpHookExecutor {
    async fn fire(&self, event: &HookEvent) -> HookResult {
        // Record this event (updates activity timestamp and counter)
        self.record_event();

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
            match self
                .client
                .send_event(ahp_event.event_type.clone(), ahp_event.payload.clone())
                .await
            {
                Ok(decision) => {
                    debug!("AHP decision: {:?}", decision);
                    self.map_decision(decision)
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
            // Fire-and-forget for non-blocking events (legacy behavior)
            let client = self.client.clone();
            let event = ahp_event;
            tokio::spawn(async move {
                if let Err(e) = client
                    .send_event(event.event_type.clone(), event.payload.clone())
                    .await
                {
                    warn!("AHP fire-and-forget error: {}", e);
                }
            });
            HookResult::Continue(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::PreToolUseEvent;

    fn make_test_executor() -> AhpHookExecutor {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        AhpHookExecutor {
            client: Arc::new(unsafe { std::mem::zeroed() }),
            agent_id: "test-agent".to_string(),
            depth: 0,
            last_activity: Arc::new(AtomicU64::new(now)),
            idle_threshold_ms: 10_000,
            start_time: Instant::now(),
            total_events: Arc::new(AtomicU64::new(0)),
            error_count: Arc::new(AtomicU64::new(0)),
            capabilities: HashMap::new(),
            shutdown: Arc::new(AtomicBool::new(false)),
            memory_summary: Arc::new(RwLock::new(None)),
            current_task: Arc::new(RwLock::new(None)),
            batch_buffer: Arc::new(RwLock::new(Vec::new())),
            batch_size: 10,
            batch_timeout_ms: 5000,
            last_batch_flush: Arc::new(AtomicU64::new(now)),
            batch_enabled: false,
        }
    }

    #[test]
    #[ignore] // Requires mock AhpClient - zeroed Arc causes UB
    fn test_map_pre_tool_use() {
        let executor = make_test_executor();

        let event = HookEvent::PreToolUse(PreToolUseEvent {
            session_id: "session-123".to_string(),
            tool: "Bash".to_string(),
            args: serde_json::json!({"command": "ls"}),
            working_directory: "/workspace".to_string(),
            recent_tools: vec![],
        });

        let ahp_event = executor.map_event(&event).unwrap();
        assert_eq!(ahp_event.event_type, EventType::PreAction);
        assert_eq!(ahp_event.session_id, "session-123");
        assert_eq!(ahp_event.depth, 0);
    }

    #[test]
    #[ignore] // Requires mock AhpClient - zeroed Arc causes UB
    fn test_map_decision_allow() {
        let executor = make_test_executor();

        let decision = Decision::Allow {
            modified_payload: None,
            metadata: None,
        };

        let result = executor.map_decision(decision);
        assert!(matches!(result, HookResult::Continue(None)));
    }

    #[test]
    #[ignore] // Requires mock AhpClient - zeroed Arc causes UB
    fn test_map_decision_block() {
        let executor = make_test_executor();

        let decision = Decision::Block {
            reason: "Dangerous command".to_string(),
            metadata: None,
        };

        let result = executor.map_decision(decision);
        assert!(matches!(result, HookResult::Block(_)));
    }

    #[test]
    #[ignore] // Requires mock AhpClient - zeroed Arc causes UB
    fn test_idle_detection_not_idle() {
        let executor = make_test_executor();
        // Should not be idle since we just created it
        let idle_event = executor.check_idle();
        assert!(idle_event.is_none());
    }

    #[test]
    #[ignore] // Requires mock AhpClient - zeroed Arc causes UB
    fn test_idle_detection_after_threshold() {
        let executor = make_test_executor();
        // Simulate old last activity (11 seconds ago)
        let old_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 11_000;
        executor.last_activity.store(old_time, Ordering::Relaxed);

        let idle_event = executor.check_idle();
        assert!(idle_event.is_some());
        let idle = idle_event.unwrap();
        assert!(idle.idle_duration_ms >= 10_000);
        assert_eq!(idle.idle_reason, "no_activity");
        assert_eq!(idle.suggested_action, Some("dream".to_string()));
    }

    #[test]
    #[ignore] // Requires mock AhpClient - zeroed Arc causes UB
    fn test_record_event_updates_activity() {
        let executor = make_test_executor();
        let before = executor.get_idle_duration_ms();

        // Small delay then record
        std::thread::sleep(Duration::from_millis(10));
        executor.record_event();

        let after = executor.get_idle_duration_ms();
        // After recording, idle duration should be small (near zero)
        assert!(after < before);
    }
}
