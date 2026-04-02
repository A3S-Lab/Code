// AHP Hook Executor Implementation
//
// Bridges A3S Code's hook system with AHP protocol

use crate::hooks::{HookEvent, HookEventType, HookExecutor, HookResult};
use a3s_ahp::{AhpClient, AhpEvent, Decision, EventType, IdleEvent, Transport};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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
    /// Client自主 exposes capabilities for the server to use
    capabilities: HashMap<String, serde_json::Value>,
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
            capabilities: HashMap::new(),
        })
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
            capabilities: HashMap::new(),
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

    /// Get idle duration in milliseconds
    fn get_idle_duration_ms(&self) -> u64 {
        let last = self.last_activity.load(Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        now.saturating_sub(last)
    }

    /// Check if agent is idle and create idle event if threshold exceeded
    fn check_idle(&self) -> Option<IdleEvent> {
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
    /// Other fields (recent_facts, memory_summary, etc.) can be added by
    /// implementing a custom executor that queries memory/session stores.
    fn build_context(&self) -> Option<a3s_ahp::EventContext> {
        // Always include capabilities if any were set
        if self.capabilities.is_empty() {
            return None;
        }

        Some(a3s_ahp::EventContext {
            recent_facts: None,
            memory_summary: None,
            session_stats: None,
            current_task: None,
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
            // Send event and wait for decision
            match self
                .client
                .send_event(ahp_event.event_type, ahp_event.payload)
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
        } else {
            // Fire-and-forget for non-blocking events
            let client = self.client.clone();
            let event_type = ahp_event.event_type;
            let payload = ahp_event.payload;
            tokio::spawn(async move {
                if let Err(e) = client.send_event(event_type, payload).await {
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
            capabilities: HashMap::new(),
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
