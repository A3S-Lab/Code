use super::AgentLoop;
use crate::hooks::{
    ErrorType, HookEvent, HookResult, OnErrorEvent, PostResponseEvent, PostToolUseEvent,
    PreToolUseEvent, TokenUsageInfo, ToolResultData,
};
use crate::llm::TokenUsage;
use std::sync::Arc;

impl AgentLoop {
    /// Fire PreToolUse hook event before tool execution.
    /// Returns the HookResult which may block the tool call.
    pub(super) async fn fire_pre_tool_use(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        recent_tools: Vec<String>,
    ) -> Option<HookResult> {
        if let Some(he) = &self.config.hook_engine {
            // Convert null args to empty object so JS callbacks don't get null.input errors
            let safe_args = if args.is_null() {
                serde_json::Value::Object(Default::default())
            } else {
                args.clone()
            };
            let event = HookEvent::PreToolUse(PreToolUseEvent {
                session_id: session_id.to_string(),
                tool: tool_name.to_string(),
                args: safe_args,
                working_directory: self.tool_context.workspace.to_string_lossy().to_string(),
                recent_tools,
            });
            let result = he.fire(&event).await;
            if result.is_block() {
                return Some(result);
            }
        }
        None
    }

    /// Fire PostToolUse hook event after tool execution (fire-and-forget).
    pub(super) async fn fire_post_tool_use(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        output: &str,
        success: bool,
        duration_ms: u64,
    ) {
        if let Some(he) = &self.config.hook_engine {
            // Convert null args to empty object so JS callbacks don't get null.input errors
            let safe_args = if args.is_null() {
                serde_json::Value::Object(Default::default())
            } else {
                args.clone()
            };
            let event = HookEvent::PostToolUse(PostToolUseEvent {
                session_id: session_id.to_string(),
                tool: tool_name.to_string(),
                args: safe_args,
                result: ToolResultData {
                    success,
                    output: output.to_string(),
                    exit_code: if success { Some(0) } else { Some(1) },
                    duration_ms,
                },
            });
            let he = Arc::clone(he);
            tokio::spawn(async move {
                let _ = he.fire(&event).await;
            });
        }
    }

    /// Fire PostResponse hook event after the agent loop completes.
    pub(super) async fn fire_post_response(
        &self,
        session_id: &str,
        response_text: &str,
        tool_calls_count: usize,
        usage: &TokenUsage,
        duration_ms: u64,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::PostResponse(PostResponseEvent {
                session_id: session_id.to_string(),
                response_text: response_text.to_string(),
                tool_calls_count,
                usage: TokenUsageInfo {
                    prompt_tokens: usage.prompt_tokens as i32,
                    completion_tokens: usage.completion_tokens as i32,
                    total_tokens: usage.total_tokens as i32,
                },
                duration_ms,
            });
            let he = Arc::clone(he);
            tokio::spawn(async move {
                let _ = he.fire(&event).await;
            });
        }
    }

    /// Fire OnError hook event when an error occurs.
    pub(super) async fn fire_on_error(
        &self,
        session_id: &str,
        error_type: ErrorType,
        error_message: &str,
        context: serde_json::Value,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::OnError(OnErrorEvent {
                session_id: session_id.to_string(),
                error_type,
                error_message: error_message.to_string(),
                context,
            });
            let he = Arc::clone(he);
            tokio::spawn(async move {
                let _ = he.fire(&event).await;
            });
        }
    }
}
