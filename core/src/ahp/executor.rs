// AHP Hook Executor Implementation
//
// Bridges A3S Code's hook system with AHP protocol

use crate::hooks::{HookEvent, HookEventType, HookExecutor, HookResult};
use a3s_ahp::{AhpClient, AhpEvent, Decision, EventType, Transport};
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;
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
}

impl std::fmt::Debug for AhpHookExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AhpHookExecutor")
            .field("agent_id", &self.agent_id)
            .field("depth", &self.depth)
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
        let client = AhpClient::new(transport).await?;

        // Perform handshake
        client.handshake().await?;

        Ok(Self {
            client: Arc::new(client),
            agent_id: uuid::Uuid::new_v4().to_string(),
            depth: 0,
        })
    }

    /// Create with specific agent ID and depth
    pub async fn with_context(
        transport: Transport,
        agent_id: String,
        depth: u32,
    ) -> Result<Self, a3s_ahp::AhpError> {
        let client = AhpClient::new(transport).await?;
        client.handshake().await?;

        Ok(Self {
            client: Arc::new(client),
            agent_id,
            depth,
        })
    }

    /// Get the agent ID
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Get the depth
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// Map A3S Code hook event to AHP event
    fn map_event(&self, event: &HookEvent) -> Option<AhpEvent> {
        let (event_type, payload) = match event {
            HookEvent::PreToolUse(e) => {
                (EventType::PreAction, serde_json::json!({
                    "tool": e.tool,
                    "arguments": e.args,
                    "working_directory": e.working_directory,
                    "recent_tools": e.recent_tools,
                }))
            }
            HookEvent::PostToolUse(e) => {
                (EventType::PostAction, serde_json::json!({
                    "tool": e.tool,
                    "arguments": e.args,
                    "result": {
                        "success": e.result.success,
                        "output": e.result.output,
                        "exit_code": e.result.exit_code,
                        "duration_ms": e.result.duration_ms,
                    }
                }))
            }
            HookEvent::PrePrompt(e) => {
                (EventType::PrePrompt, serde_json::json!({
                    "prompt": e.prompt,
                    "system_prompt": e.system_prompt,
                    "message_count": e.message_count,
                }))
            }
            HookEvent::GenerateStart(e) => {
                (EventType::PrePrompt, serde_json::json!({
                    "prompt": e.prompt,
                    "session_id": e.session_id,
                }))
            }
            HookEvent::PostResponse(e) => {
                (EventType::PostAction, serde_json::json!({
                    "response_text": e.response_text,
                    "tool_calls_count": e.tool_calls_count,
                    "usage": e.usage,
                    "duration_ms": e.duration_ms,
                }))
            }
            HookEvent::SessionStart(e) => {
                (EventType::SessionStart, serde_json::json!({
                    "session_id": e.session_id,
                    "system_prompt": e.system_prompt,
                    "model_provider": e.model_provider,
                    "model_name": e.model_name,
                }))
            }
            HookEvent::SessionEnd(e) => {
                (EventType::SessionEnd, serde_json::json!({
                    "session_id": e.session_id,
                    "duration_ms": e.duration_ms,
                }))
            }
            HookEvent::OnError(e) => {
                (EventType::Error, serde_json::json!({
                    "error_type": format!("{:?}", e.error_type),
                    "error_message": e.error_message,
                    "context": e.context,
                }))
            }
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
            metadata: None,
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
            Decision::Allow { modified_payload, .. } => {
                if let Some(modified) = modified_payload {
                    HookResult::Continue(Some(modified))
                } else {
                    HookResult::Continue(None)
                }
            }
            Decision::Block { reason, .. } => {
                HookResult::Block(reason)
            }
            Decision::Defer { retry_after_ms, reason } => {
                if let Some(r) = reason {
                    debug!("AHP defer: {}", r);
                }
                HookResult::Retry(retry_after_ms)
            }
            Decision::Modify { modified_payload, .. } => {
                HookResult::Continue(Some(modified_payload))
            }
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
            match self.client.send_event(ahp_event.event_type, ahp_event.payload).await {
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
    use crate::hooks::{PreToolUseEvent, ToolResultData, PostToolUseEvent};

    #[test]
    fn test_map_pre_tool_use() {
        let executor = AhpHookExecutor {
            client: Arc::new(unsafe { std::mem::zeroed() }), // Mock for test
            agent_id: "test-agent".to_string(),
            depth: 0,
        };

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
    fn test_map_decision_allow() {
        let executor = AhpHookExecutor {
            client: Arc::new(unsafe { std::mem::zeroed() }),
            agent_id: "test-agent".to_string(),
            depth: 0,
        };

        let decision = Decision {
            decision: "allow".to_string(),
            reason: None,
            modified_payload: None,
            retry_after_ms: None,
            metadata: None,
        };

        let result = executor.map_decision(decision);
        assert!(matches!(result, HookResult::Continue(None)));
    }

    #[test]
    fn test_map_decision_block() {
        let executor = AhpHookExecutor {
            client: Arc::new(unsafe { std::mem::zeroed() }),
            agent_id: "test-agent".to_string(),
            depth: 0,
        };

        let decision = Decision {
            decision: "block".to_string(),
            reason: Some("Dangerous command".to_string()),
            modified_payload: None,
            retry_after_ms: None,
            metadata: None,
        };

        let result = executor.map_decision(decision);
        assert!(matches!(result, HookResult::Block(_)));
    }
}
