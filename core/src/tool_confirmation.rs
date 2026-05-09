//! HITL confirmation runtime for tool execution.
//!
//! The safety gate decides whether a tool call needs confirmation; this module
//! owns the confirmation protocol details so the agent loop stays an action
//! pipeline rather than a HITL state machine.

use crate::agent::AgentEvent;
use crate::hitl::{ConfirmationProvider, TimeoutAction};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolConfirmationResolution {
    Approved,
    Rejected { output: String },
}

pub(crate) struct ToolConfirmationRequest<'a> {
    pub(crate) tool_id: &'a str,
    pub(crate) tool_name: &'a str,
    pub(crate) args: &'a Value,
    pub(crate) timeout_ms: u64,
    pub(crate) timeout_action: TimeoutAction,
}

pub(crate) struct ToolConfirmationRuntime<'a> {
    manager: &'a dyn ConfirmationProvider,
    event_tx: Option<&'a mpsc::Sender<AgentEvent>>,
}

impl<'a> ToolConfirmationRuntime<'a> {
    pub(crate) fn new(
        manager: &'a dyn ConfirmationProvider,
        event_tx: Option<&'a mpsc::Sender<AgentEvent>>,
    ) -> Self {
        Self { manager, event_tx }
    }

    pub(crate) async fn resolve(
        &self,
        request: ToolConfirmationRequest<'_>,
    ) -> ToolConfirmationResolution {
        let rx = self
            .manager
            .request_confirmation(request.tool_id, request.tool_name, request.args)
            .await;

        self.forward_event(AgentEvent::ConfirmationRequired {
            tool_id: request.tool_id.to_string(),
            tool_name: request.tool_name.to_string(),
            args: request.args.clone(),
            timeout_ms: request.timeout_ms,
        })
        .await;

        match tokio::time::timeout(Duration::from_millis(request.timeout_ms), rx).await {
            Ok(Ok(response)) => {
                self.forward_event(AgentEvent::ConfirmationReceived {
                    tool_id: request.tool_id.to_string(),
                    approved: response.approved,
                    reason: response.reason.clone(),
                })
                .await;

                if response.approved {
                    ToolConfirmationResolution::Approved
                } else {
                    ToolConfirmationResolution::Rejected {
                        output: format!(
                            "Tool '{}' execution was REJECTED by the user. Reason: {}. DO NOT retry this tool call unless the user explicitly asks you to.",
                            request.tool_name,
                            response
                                .reason
                                .unwrap_or_else(|| "No reason provided".to_string())
                        ),
                    }
                }
            }
            Ok(Err(_)) => {
                self.forward_timeout(request.tool_id, "rejected").await;
                ToolConfirmationResolution::Rejected {
                    output: format!(
                        "Tool '{}' confirmation failed: confirmation channel closed",
                        request.tool_name
                    ),
                }
            }
            Err(_) => {
                self.manager.check_timeouts().await;
                self.forward_timeout(request.tool_id, action_taken(request.timeout_action))
                    .await;

                match request.timeout_action {
                    TimeoutAction::Reject => ToolConfirmationResolution::Rejected {
                        output: format!(
                            "Tool '{}' execution was REJECTED: user confirmation timed out after {}ms. DO NOT retry this tool call - the user did not approve it. Inform the user that the operation requires their approval and ask them to try again.",
                            request.tool_name, request.timeout_ms
                        ),
                    },
                    TimeoutAction::AutoApprove => ToolConfirmationResolution::Approved,
                }
            }
        }
    }

    async fn forward_timeout(&self, tool_id: &str, action_taken: &str) {
        self.forward_event(AgentEvent::ConfirmationTimeout {
            tool_id: tool_id.to_string(),
            action_taken: action_taken.to_string(),
        })
        .await;
    }

    async fn forward_event(&self, event: AgentEvent) {
        if let Some(tx) = self.event_tx {
            tx.send(event).await.ok();
        }
    }
}

fn action_taken(action: TimeoutAction) -> &'static str {
    match action {
        TimeoutAction::Reject => "rejected",
        TimeoutAction::AutoApprove => "auto_approved",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hitl::{
        ConfirmationManager, ConfirmationPolicy, ConfirmationResponse, PendingConfirmationInfo,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::{broadcast, oneshot};

    fn enabled_policy(timeout_ms: u64, timeout_action: TimeoutAction) -> ConfirmationPolicy {
        ConfirmationPolicy::enabled().with_timeout(timeout_ms, timeout_action)
    }

    async fn approve_when_pending(
        manager: Arc<ConfirmationManager>,
        tool_id: &'static str,
        approved: bool,
        reason: Option<String>,
    ) {
        for _ in 0..20 {
            if !manager.pending_confirmations().await.is_empty() {
                manager.confirm(tool_id, approved, reason).await.unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("confirmation request was not created");
    }

    async fn collect_events(rx: &mut mpsc::Receiver<AgentEvent>, count: usize) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        for _ in 0..count {
            events.push(rx.recv().await.expect("missing forwarded event"));
        }
        events
    }

    #[tokio::test]
    async fn approved_confirmation_returns_approved_and_forwards_events() {
        let (broadcast_tx, _) = broadcast::channel(8);
        let manager = Arc::new(ConfirmationManager::new(
            enabled_policy(1_000, TimeoutAction::Reject),
            broadcast_tx,
        ));
        let (event_tx, mut event_rx) = mpsc::channel(8);

        let approver = tokio::spawn(approve_when_pending(
            manager.clone(),
            "tool-1",
            true,
            Some("ok".to_string()),
        ));

        let runtime = ToolConfirmationRuntime::new(manager.as_ref(), Some(&event_tx));
        let resolution = runtime
            .resolve(ToolConfirmationRequest {
                tool_id: "tool-1",
                tool_name: "bash",
                args: &json!({"command": "pwd"}),
                timeout_ms: 1_000,
                timeout_action: TimeoutAction::Reject,
            })
            .await;

        approver.await.unwrap();
        assert_eq!(resolution, ToolConfirmationResolution::Approved);

        let events = collect_events(&mut event_rx, 2).await;
        assert!(matches!(events[0], AgentEvent::ConfirmationRequired { .. }));
        assert!(matches!(
            events[1],
            AgentEvent::ConfirmationReceived { approved: true, .. }
        ));
    }

    #[tokio::test]
    async fn rejected_confirmation_returns_llm_safe_rejection() {
        let (broadcast_tx, _) = broadcast::channel(8);
        let manager = Arc::new(ConfirmationManager::new(
            enabled_policy(1_000, TimeoutAction::Reject),
            broadcast_tx,
        ));
        let (event_tx, mut event_rx) = mpsc::channel(8);

        let approver = tokio::spawn(approve_when_pending(
            manager.clone(),
            "tool-1",
            false,
            Some("not allowed".to_string()),
        ));

        let runtime = ToolConfirmationRuntime::new(manager.as_ref(), Some(&event_tx));
        let resolution = runtime
            .resolve(ToolConfirmationRequest {
                tool_id: "tool-1",
                tool_name: "write",
                args: &json!({"file_path": "secret.txt"}),
                timeout_ms: 1_000,
                timeout_action: TimeoutAction::Reject,
            })
            .await;

        approver.await.unwrap();
        let ToolConfirmationResolution::Rejected { output } = resolution else {
            panic!("expected rejection");
        };
        assert!(output.contains("REJECTED by the user"));
        assert!(output.contains("not allowed"));

        let events = collect_events(&mut event_rx, 2).await;
        assert!(matches!(events[0], AgentEvent::ConfirmationRequired { .. }));
        assert!(matches!(
            events[1],
            AgentEvent::ConfirmationReceived {
                approved: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn timeout_rejects_and_forwards_timeout_event() {
        let (broadcast_tx, _) = broadcast::channel(8);
        let manager = Arc::new(ConfirmationManager::new(
            enabled_policy(5, TimeoutAction::Reject),
            broadcast_tx,
        ));
        let (event_tx, mut event_rx) = mpsc::channel(8);

        let runtime = ToolConfirmationRuntime::new(manager.as_ref(), Some(&event_tx));
        let resolution = runtime
            .resolve(ToolConfirmationRequest {
                tool_id: "tool-1",
                tool_name: "bash",
                args: &json!({"command": "rm -rf target"}),
                timeout_ms: 5,
                timeout_action: TimeoutAction::Reject,
            })
            .await;

        let ToolConfirmationResolution::Rejected { output } = resolution else {
            panic!("expected rejection");
        };
        assert!(output.contains("timed out after 5ms"));

        let events = collect_events(&mut event_rx, 2).await;
        assert!(matches!(events[0], AgentEvent::ConfirmationRequired { .. }));
        assert!(matches!(
            events[1],
            AgentEvent::ConfirmationTimeout { ref action_taken, .. }
                if action_taken == "rejected"
        ));
    }

    #[tokio::test]
    async fn timeout_auto_approve_returns_approved() {
        let (broadcast_tx, _) = broadcast::channel(8);
        let manager = Arc::new(ConfirmationManager::new(
            enabled_policy(5, TimeoutAction::AutoApprove),
            broadcast_tx,
        ));
        let (event_tx, mut event_rx) = mpsc::channel(8);

        let runtime = ToolConfirmationRuntime::new(manager.as_ref(), Some(&event_tx));
        let resolution = runtime
            .resolve(ToolConfirmationRequest {
                tool_id: "tool-1",
                tool_name: "read",
                args: &json!({"file_path": "README.md"}),
                timeout_ms: 5,
                timeout_action: TimeoutAction::AutoApprove,
            })
            .await;

        assert_eq!(resolution, ToolConfirmationResolution::Approved);

        let events = collect_events(&mut event_rx, 2).await;
        assert!(matches!(events[0], AgentEvent::ConfirmationRequired { .. }));
        assert!(matches!(
            events[1],
            AgentEvent::ConfirmationTimeout { ref action_taken, .. }
                if action_taken == "auto_approved"
        ));
    }

    struct ClosedConfirmationProvider;

    #[async_trait]
    impl ConfirmationProvider for ClosedConfirmationProvider {
        async fn requires_confirmation(&self, _tool_name: &str) -> bool {
            true
        }

        async fn request_confirmation(
            &self,
            _tool_id: &str,
            _tool_name: &str,
            _args: &Value,
        ) -> oneshot::Receiver<ConfirmationResponse> {
            let (tx, rx) = oneshot::channel();
            drop(tx);
            rx
        }

        async fn confirm(
            &self,
            _tool_id: &str,
            _approved: bool,
            _reason: Option<String>,
        ) -> Result<bool, String> {
            Ok(false)
        }

        async fn policy(&self) -> ConfirmationPolicy {
            enabled_policy(1_000, TimeoutAction::Reject)
        }

        async fn set_policy(&self, _policy: ConfirmationPolicy) {}

        async fn check_timeouts(&self) -> usize {
            0
        }

        async fn cancel_all(&self) -> usize {
            0
        }

        async fn pending_confirmations(&self) -> Vec<PendingConfirmationInfo> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn closed_confirmation_channel_is_a_rejection() {
        let provider = ClosedConfirmationProvider;
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let runtime = ToolConfirmationRuntime::new(&provider, Some(&event_tx));

        let resolution = runtime
            .resolve(ToolConfirmationRequest {
                tool_id: "tool-1",
                tool_name: "bash",
                args: &json!({"command": "pwd"}),
                timeout_ms: 1_000,
                timeout_action: TimeoutAction::Reject,
            })
            .await;

        let ToolConfirmationResolution::Rejected { output } = resolution else {
            panic!("expected rejection");
        };
        assert!(output.contains("confirmation channel closed"));

        let events = collect_events(&mut event_rx, 2).await;
        assert!(matches!(events[0], AgentEvent::ConfirmationRequired { .. }));
        assert!(matches!(
            events[1],
            AgentEvent::ConfirmationTimeout { ref action_taken, .. }
                if action_taken == "rejected"
        ));
    }
}
