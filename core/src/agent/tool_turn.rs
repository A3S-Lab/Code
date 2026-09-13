use super::execution_state::ExecutionLoopState;
use super::tool_completion_runtime::ToolCompletionInput;
use super::{AgentEvent, AgentLoop};
use crate::llm::{ContentBlock, Message, ToolCall};
use crate::tools::ToolContext;
use crate::tools::ToolInvocation;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

impl AgentLoop {
    pub(super) async fn execute_tool_turn(
        &self,
        tool_calls: Vec<ToolCall>,
        state: &mut ExecutionLoopState,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        session_id: Option<&str>,
        cancel_token: &CancellationToken,
        tool_context: &ToolContext,
    ) -> anyhow::Result<()> {
        let (tool_calls, refused) = partition_tool_calls(tool_calls);
        if !refused.is_empty() {
            let ids = refused.iter().map(|call| call.id.clone()).collect();
            collapse_duplicate_tool_uses(state, &ids);
            for call in refused {
                refuse_ambiguous_tool_call(call, state, event_tx).await;
            }
        }
        if tool_calls.is_empty() {
            return Ok(());
        }

        if self.can_run_parallel_write_batch(&tool_calls) {
            self.execute_parallel_write_batch(
                &tool_calls,
                state,
                event_tx,
                session_id,
                cancel_token,
                tool_context,
            )
            .await;
            return Ok(());
        }

        for tool_call in tool_calls {
            self.execute_sequential_tool_call(
                tool_call,
                state,
                event_tx,
                session_id,
                cancel_token,
                tool_context,
            )
            .await?;
        }

        Ok(())
    }

    async fn execute_sequential_tool_call(
        &self,
        tool_call: ToolCall,
        state: &mut ExecutionLoopState,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        session_id: Option<&str>,
        cancel_token: &CancellationToken,
        tool_context: &ToolContext,
    ) -> anyhow::Result<()> {
        state.record_tool_call();
        let tool_start = std::time::Instant::now();
        let turn = state.current_turn();
        self.config.rl_trajectory_recorder.record_tool_call(
            session_id.unwrap_or(""),
            turn,
            &tool_call,
        );

        tracing::info!(
            tool_name = tool_call.name.as_str(),
            tool_id = tool_call.id.as_str(),
            "Tool execution started"
        );

        if self
            .handle_tool_preflight_guard(&tool_call, state, event_tx, session_id)
            .await?
        {
            return Ok(());
        }

        let normalized = self
            .invoke_model_tool(
                ToolInvocation::agent(
                    tool_call.id.clone(),
                    tool_call.name.clone(),
                    tool_call.args.clone(),
                    state.recent_tool_signatures(),
                ),
                session_id,
                event_tx,
                cancel_token,
                tool_context,
            )
            .await;

        self.complete_tool_call(
            state,
            ToolCompletionInput {
                tool_call: &tool_call,
                event_tx,
                session_id,
                tool_start,
                normalized,
            },
        )
        .await;
        Ok(())
    }
}

fn partition_tool_calls(calls: Vec<ToolCall>) -> (Vec<ToolCall>, Vec<ToolCall>) {
    let mut counts = HashMap::<String, usize>::new();
    for call in &calls {
        if call.id.trim().is_empty() {
            continue;
        }
        *counts.entry(call.id.clone()).or_insert(0) += 1;
    }
    let mut runnable = Vec::new();
    let mut refused = Vec::new();
    let mut kept = HashSet::new();
    for call in calls {
        let ambiguous = call.id.trim().is_empty() || counts.get(&call.id).copied().unwrap_or(0) > 1;
        if !ambiguous {
            runnable.push(call);
            continue;
        }
        if kept.insert(call.id.clone()) {
            refused.push(call);
        }
    }
    (runnable, refused)
}

fn collapse_duplicate_tool_uses(state: &mut ExecutionLoopState, ambiguous_ids: &HashSet<String>) {
    let Some(message) = state.messages.iter_mut().rev().find(|message| {
        message.role == "assistant"
            && message.content.iter().any(|block| {
                matches!(block, ContentBlock::ToolUse { id, .. } if ambiguous_ids.contains(id))
            })
    }) else {
        return;
    };
    let mut seen = HashSet::new();
    message.content.retain(|block| {
        let ContentBlock::ToolUse { id, .. } = block else {
            return true;
        };
        if !ambiguous_ids.contains(id) {
            return true;
        }
        seen.insert(id.clone())
    });
}

async fn refuse_ambiguous_tool_call(
    tool_call: ToolCall,
    state: &mut ExecutionLoopState,
    event_tx: &Option<mpsc::Sender<AgentEvent>>,
) {
    let message = if tool_call.id.trim().is_empty() {
        "tool call id must not be empty".to_string()
    } else {
        format!(
            "tool call id '{}' is duplicated; refusing all calls with this id",
            tool_call.id
        )
    };
    if let Some(tx) = event_tx {
        tx.send(AgentEvent::ToolEnd {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            args: Some(tool_call.args.clone()),
            output: message.clone(),
            exit_code: 1,
            metadata: None,
            error_kind: Some(crate::tools::ToolErrorKind::InvalidArgument {
                message: message.clone(),
            }),
        })
        .await
        .ok();
    }
    state
        .messages
        .push(Message::tool_result_trusted(&tool_call.id, &message, true));
}
