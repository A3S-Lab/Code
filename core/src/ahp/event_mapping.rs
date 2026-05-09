//! Mapping from local hook events to AHP wire events.

use crate::hooks::HookEvent;
use a3s_ahp::{AhpEvent, EventContext, EventType};
use chrono::Utc;

pub(super) fn map_hook_event(
    event: &HookEvent,
    agent_id: &str,
    depth: u32,
    workspace: Option<String>,
    context: Option<EventContext>,
) -> Option<AhpEvent> {
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
        HookEvent::PreContextPerception(e) => (
            EventType::ContextPerception,
            serde_json::json!({
                "intent": e.intent,
                "target_type": e.target_type,
                "target_name": e.target_name,
                "domain": e.domain,
                "query": e.query,
                "working_directory": workspace.unwrap_or_default(),
                "urgency": e.urgency,
            }),
        ),
        HookEvent::PostContextPerception(e) => (
            EventType::ContextPerception,
            serde_json::json!({
                "intent": e.intent,
                "target_type": e.target_type,
                "success": e.success,
                "facts_retrieved": e.facts_retrieved,
                "files_retrieved": e.files_retrieved,
                "error": e.error,
            }),
        ),
        HookEvent::OnSuccess(e) => (
            EventType::Success,
            serde_json::json!({
                "action_type": e.action_type,
                "action_summary": e.action_summary,
                "duration_ms": e.duration_ms,
            }),
        ),
        HookEvent::PreMemoryRecall(e) => (
            EventType::MemoryRecall,
            serde_json::json!({
                "query": e.query,
                "memory_type": e.memory_type,
                "max_results": e.max_results,
                "working_directory": e.working_directory,
            }),
        ),
        HookEvent::PostMemoryRecall(e) => (
            EventType::MemoryRecall,
            serde_json::json!({
                "query": e.query,
                "memory_type": e.memory_type,
                "facts_retrieved": e.facts_retrieved,
                "success": e.success,
                "error": e.error,
            }),
        ),
        HookEvent::PrePlanning(e) => (
            EventType::Planning,
            serde_json::json!({
                "task_description": e.task_description,
                "available_strategies": e.available_strategies,
                "constraints": e.constraints,
            }),
        ),
        HookEvent::PostPlanning(e) => (
            EventType::Planning,
            serde_json::json!({
                "task_description": e.task_description,
                "strategy_used": e.strategy_used,
                "subtasks": e.subtasks,
                "success": e.success,
                "error": e.error,
            }),
        ),
        HookEvent::PreReasoning(e) => (
            EventType::Reasoning,
            serde_json::json!({
                "reasoning_type": format!("{:?}", e.reasoning_type),
                "problem_statement": e.problem_statement,
                "hints": e.hints,
            }),
        ),
        HookEvent::PostReasoning(e) => (
            EventType::Reasoning,
            serde_json::json!({
                "reasoning_type": format!("{:?}", e.reasoning_type),
                "conclusion": e.conclusion,
                "steps_count": e.steps_count,
                "success": e.success,
                "error": e.error,
            }),
        ),
        HookEvent::OnRateLimit(e) => (
            EventType::RateLimit,
            serde_json::json!({
                "limit_type": format!("{:?}", e.limit_type),
                "retry_after_ms": e.retry_after_ms,
                "current_usage": e.current_usage,
            }),
        ),
        HookEvent::OnConfirmation(e) => (
            EventType::Confirmation,
            serde_json::json!({
                "confirmation_type": format!("{:?}", e.confirmation_type),
                "message": e.message,
                "options": e.options,
            }),
        ),
        HookEvent::IntentDetection(e) => (
            EventType::IntentDetection,
            serde_json::json!({
                "prompt": e.prompt,
                "workspace": e.workspace,
                "language_hint": e.language_hint,
            }),
        ),
        HookEvent::GenerateEnd(e) => (
            EventType::PostAction,
            serde_json::json!({
                "response_text": e.response_text,
                "tool_calls": e.tool_calls,
                "usage": e.usage,
                "duration_ms": e.duration_ms,
            }),
        ),
        HookEvent::SkillLoad(_) | HookEvent::SkillUnload(_) => {
            return None;
        }
    };

    Some(AhpEvent {
        event_type,
        session_id: extract_session_id(event, agent_id),
        agent_id: agent_id.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        depth,
        payload,
        context,
        metadata: None,
    })
}

fn extract_session_id(event: &HookEvent, agent_id: &str) -> String {
    match event {
        HookEvent::PreToolUse(e) => e.session_id.clone(),
        HookEvent::PostToolUse(e) => e.session_id.clone(),
        HookEvent::GenerateStart(e) => e.session_id.clone(),
        HookEvent::SessionStart(e) => e.session_id.clone(),
        HookEvent::SessionEnd(e) => e.session_id.clone(),
        HookEvent::PrePrompt(e) => e.session_id.clone(),
        HookEvent::PreContextPerception(e) => e.session_id.clone(),
        HookEvent::PostContextPerception(e) => e.session_id.clone(),
        HookEvent::OnSuccess(e) => e.session_id.clone(),
        HookEvent::PreMemoryRecall(e) => e.session_id.clone(),
        HookEvent::PostMemoryRecall(e) => e.session_id.clone(),
        HookEvent::PrePlanning(e) => e.session_id.clone(),
        HookEvent::PostPlanning(e) => e.session_id.clone(),
        HookEvent::PreReasoning(e) => e.session_id.clone(),
        HookEvent::PostReasoning(e) => e.session_id.clone(),
        HookEvent::OnRateLimit(e) => e.session_id.clone(),
        HookEvent::OnConfirmation(e) => e.session_id.clone(),
        HookEvent::IntentDetection(e) => e.session_id.clone(),
        HookEvent::SkillLoad(_) | HookEvent::SkillUnload(_) => String::new(),
        HookEvent::GenerateEnd(_) | HookEvent::PostResponse(_) | HookEvent::OnError(_) => {
            agent_id.to_string()
        }
    }
}
