use super::execution_state::ExecutionLoopState;
use super::tool_result_runtime::NormalizedToolResult;
use super::{AgentEvent, AgentLoop};
use crate::llm::ToolCall;
use crate::safety_gate::{ToolGateDecision, ToolGateInput, ToolSafetyGate};
use crate::tool_confirmation::{
    ToolConfirmationRequest, ToolConfirmationResolution, ToolConfirmationRuntime,
};
use tokio::sync::mpsc;

impl AgentLoop {
    pub(super) async fn decide_tool_gate(
        &self,
        tool_call: &ToolCall,
        state: &ExecutionLoopState,
        session_id: Option<&str>,
    ) -> ToolGateDecision {
        let pre_tool_block = match self
            .fire_pre_tool_use(
                session_id.unwrap_or(""),
                &tool_call.name,
                &tool_call.args,
                state.recent_tool_signatures(),
            )
            .await
        {
            Some(crate::hooks::HookResult::Block(reason)) => Some(reason),
            _ => None,
        };

        ToolSafetyGate::new(&self.config)
            .decide(ToolGateInput {
                tool_name: &tool_call.name,
                args: &tool_call.args,
                pre_tool_block,
            })
            .await
    }

    pub(super) async fn resolve_tool_gate_decision(
        &self,
        gate_decision: ToolGateDecision,
        tool_call: &ToolCall,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> NormalizedToolResult {
        match gate_decision {
            ToolGateDecision::Deny {
                output,
                event_reason,
                reason,
            } => {
                tracing::info!(
                    tool_name = tool_call.name.as_str(),
                    gate_reason = reason.as_str(),
                    "Tool denied by safety gate"
                );
                if let Some(tx) = event_tx {
                    tx.send(AgentEvent::PermissionDenied {
                        tool_id: tool_call.id.clone(),
                        tool_name: tool_call.name.clone(),
                        args: tool_call.args.clone(),
                        reason: event_reason,
                    })
                    .await
                    .ok();
                }
                NormalizedToolResult::denied(output)
            }
            ToolGateDecision::Execute { reason } => {
                tracing::info!(
                    tool_name = tool_call.name.as_str(),
                    gate_reason = reason.as_str(),
                    "Tool approved by safety gate"
                );
                self.execute_approved_tool_call(
                    event_tx,
                    &tool_call.id,
                    &tool_call.name,
                    &tool_call.args,
                )
                .await
            }
            ToolGateDecision::Confirm {
                timeout_ms,
                timeout_action,
            } => {
                tracing::info!(
                    tool_name = tool_call.name.as_str(),
                    permission = "ask",
                    "Tool requires HITL confirmation"
                );
                let confirmation = if let Some(cm) = self.config.confirmation_manager.as_ref() {
                    ToolConfirmationRuntime::new(cm.as_ref(), event_tx.as_ref())
                        .resolve(ToolConfirmationRequest {
                            tool_id: &tool_call.id,
                            tool_name: &tool_call.name,
                            args: &tool_call.args,
                            timeout_ms,
                            timeout_action,
                        })
                        .await
                } else {
                    ToolConfirmationResolution::Rejected {
                        output: format!(
                            "Tool '{}' requires confirmation but no HITL confirmation manager is configured.",
                            tool_call.name
                        ),
                    }
                };

                match confirmation {
                    ToolConfirmationResolution::Approved => {
                        self.execute_approved_tool_call(
                            event_tx,
                            &tool_call.id,
                            &tool_call.name,
                            &tool_call.args,
                        )
                        .await
                    }
                    ToolConfirmationResolution::Rejected { output } => {
                        NormalizedToolResult::denied(output)
                    }
                }
            }
        }
    }
}
