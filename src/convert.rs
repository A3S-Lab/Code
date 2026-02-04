//! Type conversions between proto and internal types
//!
//! Handles bidirectional conversion between:
//! - code_agent.proto types (gRPC interface)
//! - Internal types (llm, session, agent modules)

use crate::agent::AgentEvent as InternalAgentEvent;
use crate::hitl::{ConfirmationPolicy, SessionLane, TimeoutAction};
use crate::llm::{ContentBlock, LlmConfig, Message as InternalMessage, TokenUsage};
use crate::permissions::{PermissionDecision, PermissionPolicy, PermissionRule};
use crate::queue::{ExternalTask, ExternalTaskResult, LaneHandlerConfig, TaskHandlerMode};
use crate::service::proto;
use crate::session::ContextUsage as InternalContextUsage;
use std::collections::HashSet;

// ============================================================================
// Message Conversions
// ============================================================================

/// Convert proto Message to internal Message
pub fn proto_message_to_internal(msg: &proto::Message) -> InternalMessage {
    let role = match proto::message::Role::try_from(msg.role) {
        Ok(proto::message::Role::User) => "user",
        Ok(proto::message::Role::Assistant) => "assistant",
        Ok(proto::message::Role::System) => "system",
        Ok(proto::message::Role::Tool) => "user", // Tool results are sent as user messages
        _ => "user",
    };

    let content = vec![ContentBlock::Text {
        text: msg.content.clone(),
    }];

    InternalMessage {
        role: role.to_string(),
        content,
    }
}

/// Convert internal Message to proto Message
pub fn internal_message_to_proto(msg: &InternalMessage) -> proto::Message {
    let role = match msg.role.as_str() {
        "user" => proto::message::Role::User as i32,
        "assistant" => proto::message::Role::Assistant as i32,
        "system" => proto::message::Role::System as i32,
        _ => proto::message::Role::Unknown as i32,
    };

    proto::Message {
        role,
        content: msg.text(),
        attachments: vec![],
        metadata: std::collections::HashMap::new(),
    }
}

// ============================================================================
// Usage Conversions
// ============================================================================

/// Convert internal TokenUsage to proto Usage
pub fn internal_usage_to_proto(usage: &TokenUsage) -> proto::Usage {
    proto::Usage {
        prompt_tokens: usage.prompt_tokens as u32,
        completion_tokens: usage.completion_tokens as u32,
        total_tokens: usage.total_tokens as u32,
    }
}

/// Convert internal ContextUsage to proto ContextUsage
pub fn internal_context_usage_to_proto(usage: &InternalContextUsage) -> proto::ContextUsage {
    proto::ContextUsage {
        total_tokens: usage.used_tokens as u32,
        prompt_tokens: 0, // Not tracked separately in internal type
        completion_tokens: 0,
        message_count: usage.turns as u32,
    }
}

// ============================================================================
// Tool Conversions
// ============================================================================

/// Convert proto ToolCall to internal format (name, args)
pub fn proto_tool_call_to_args(tc: &proto::ToolCall) -> (String, serde_json::Value) {
    let args: serde_json::Value = serde_json::from_str(&tc.arguments).unwrap_or_default();
    (tc.name.clone(), args)
}

/// Convert internal tool result to proto ToolResult
pub fn internal_tool_result_to_proto(
    success: bool,
    output: String,
    error: Option<String>,
) -> proto::ToolResult {
    proto::ToolResult {
        success,
        output,
        error: error.unwrap_or_default(),
        metadata: std::collections::HashMap::new(),
    }
}

// ============================================================================
// LLM Config Conversions
// ============================================================================

/// Convert proto LLMConfig to internal LlmConfig
pub fn proto_llm_config_to_internal(config: &proto::LlmConfig) -> LlmConfig {
    let mut llm_config = LlmConfig::new(&config.provider, &config.model, &config.api_key);
    if !config.base_url.is_empty() {
        llm_config = llm_config.with_base_url(&config.base_url);
    }
    llm_config
}

// ============================================================================
// Event Conversions
// ============================================================================

/// Convert internal AgentEvent to proto GenerateChunk
pub fn internal_event_to_generate_chunk(
    event: InternalAgentEvent,
    session_id: &str,
) -> Option<proto::GenerateChunk> {
    match event {
        InternalAgentEvent::TextDelta { text } => Some(proto::GenerateChunk {
            r#type: proto::generate_chunk::ChunkType::Content as i32,
            session_id: session_id.to_string(),
            content: text,
            tool_call: None,
            tool_result: None,
            metadata: std::collections::HashMap::new(),
        }),
        InternalAgentEvent::ToolStart { id, name } => Some(proto::GenerateChunk {
            r#type: proto::generate_chunk::ChunkType::ToolCall as i32,
            session_id: session_id.to_string(),
            content: String::new(),
            tool_call: Some(proto::ToolCall {
                id,
                name,
                arguments: "{}".to_string(),
                result: None,
            }),
            tool_result: None,
            metadata: std::collections::HashMap::new(),
        }),
        InternalAgentEvent::ToolEnd {
            id: _,
            name: _,
            output,
            exit_code,
        } => Some(proto::GenerateChunk {
            r#type: proto::generate_chunk::ChunkType::ToolResult as i32,
            session_id: session_id.to_string(),
            content: String::new(),
            tool_call: None,
            tool_result: Some(proto::ToolResult {
                success: exit_code == 0,
                output,
                error: String::new(),
                metadata: std::collections::HashMap::new(),
            }),
            metadata: std::collections::HashMap::new(),
        }),
        InternalAgentEvent::End { text, usage } => Some(proto::GenerateChunk {
            r#type: proto::generate_chunk::ChunkType::Done as i32,
            session_id: session_id.to_string(),
            content: text,
            tool_call: None,
            tool_result: None,
            metadata: [
                ("prompt_tokens".to_string(), usage.prompt_tokens.to_string()),
                (
                    "completion_tokens".to_string(),
                    usage.completion_tokens.to_string(),
                ),
                ("total_tokens".to_string(), usage.total_tokens.to_string()),
            ]
            .into_iter()
            .collect(),
        }),
        _ => None,
    }
}

/// Convert internal AgentEvent to proto AgentEvent
pub fn internal_event_to_proto_event(
    event: InternalAgentEvent,
    session_id: Option<&str>,
) -> Option<proto::AgentEvent> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    match event {
        InternalAgentEvent::Start { prompt } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::GenerationStarted as i32,
            session_id: session_id.map(String::from),
            timestamp,
            message: format!("Generation started: {}", prompt),
            data: std::collections::HashMap::new(),
        }),
        InternalAgentEvent::ToolStart { id, name } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::ToolCalled as i32,
            session_id: session_id.map(String::from),
            timestamp,
            message: format!("Tool called: {}", name),
            data: [("tool_id".to_string(), id), ("tool_name".to_string(), name)]
                .into_iter()
                .collect(),
        }),
        InternalAgentEvent::ToolEnd {
            id,
            name,
            output,
            exit_code,
        } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::ToolCompleted as i32,
            session_id: session_id.map(String::from),
            timestamp,
            message: format!("Tool completed: {} (exit_code={})", name, exit_code),
            data: [
                ("tool_id".to_string(), id),
                ("tool_name".to_string(), name),
                ("exit_code".to_string(), exit_code.to_string()),
                ("output_length".to_string(), output.len().to_string()),
            ]
            .into_iter()
            .collect(),
        }),
        InternalAgentEvent::End { text: _, usage } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::GenerationCompleted as i32,
            session_id: session_id.map(String::from),
            timestamp,
            message: "Generation completed".to_string(),
            data: [
                ("prompt_tokens".to_string(), usage.prompt_tokens.to_string()),
                (
                    "completion_tokens".to_string(),
                    usage.completion_tokens.to_string(),
                ),
                ("total_tokens".to_string(), usage.total_tokens.to_string()),
            ]
            .into_iter()
            .collect(),
        }),
        InternalAgentEvent::Error { message } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::Error as i32,
            session_id: session_id.map(String::from),
            timestamp,
            message,
            data: std::collections::HashMap::new(),
        }),
        InternalAgentEvent::ConfirmationRequired {
            tool_id,
            tool_name,
            args,
            timeout_ms,
        } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::ConfirmationRequired as i32,
            session_id: session_id.map(String::from),
            timestamp,
            message: format!("Tool '{}' requires confirmation", tool_name),
            data: [
                ("tool_id".to_string(), tool_id),
                ("tool_name".to_string(), tool_name),
                ("args".to_string(), args.to_string()),
                ("timeout_ms".to_string(), timeout_ms.to_string()),
            ]
            .into_iter()
            .collect(),
        }),
        InternalAgentEvent::ConfirmationReceived {
            tool_id,
            approved,
            reason,
        } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::ConfirmationReceived as i32,
            session_id: session_id.map(String::from),
            timestamp,
            message: format!(
                "Confirmation received: {}",
                if approved { "approved" } else { "rejected" }
            ),
            data: [
                ("tool_id".to_string(), tool_id),
                ("approved".to_string(), approved.to_string()),
                (
                    "reason".to_string(),
                    reason.unwrap_or_else(|| "".to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        }),
        InternalAgentEvent::ConfirmationTimeout {
            tool_id,
            action_taken,
        } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::ConfirmationTimeout as i32,
            session_id: session_id.map(String::from),
            timestamp,
            message: format!("Confirmation timed out: {}", action_taken),
            data: [
                ("tool_id".to_string(), tool_id),
                ("action_taken".to_string(), action_taken),
            ]
            .into_iter()
            .collect(),
        }),
        InternalAgentEvent::ExternalTaskPending {
            task_id,
            session_id: task_session_id,
            lane,
            command_type,
            payload,
            timeout_ms,
        } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::ExternalTaskPending as i32,
            session_id: Some(task_session_id),
            timestamp,
            message: format!("External task pending: {} ({})", task_id, command_type),
            data: [
                ("task_id".to_string(), task_id),
                ("lane".to_string(), format!("{:?}", lane)),
                ("command_type".to_string(), command_type),
                ("payload".to_string(), payload.to_string()),
                ("timeout_ms".to_string(), timeout_ms.to_string()),
            ]
            .into_iter()
            .collect(),
        }),
        InternalAgentEvent::ExternalTaskCompleted {
            task_id,
            session_id: task_session_id,
            success,
        } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::ExternalTaskCompleted as i32,
            session_id: Some(task_session_id),
            timestamp,
            message: format!(
                "External task completed: {} ({})",
                task_id,
                if success { "success" } else { "failed" }
            ),
            data: [
                ("task_id".to_string(), task_id),
                ("success".to_string(), success.to_string()),
            ]
            .into_iter()
            .collect(),
        }),
        InternalAgentEvent::PermissionDenied {
            tool_id,
            tool_name,
            args,
            reason,
        } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::PermissionDenied as i32,
            session_id: session_id.map(String::from),
            timestamp,
            message: format!("Permission denied for tool '{}': {}", tool_name, reason),
            data: [
                ("tool_id".to_string(), tool_id),
                ("tool_name".to_string(), tool_name),
                ("args".to_string(), args.to_string()),
                ("reason".to_string(), reason),
            ]
            .into_iter()
            .collect(),
        }),
        _ => None,
    }
}

// ============================================================================
// HITL Policy Conversions
// ============================================================================

/// Convert proto SessionLane to internal SessionLane
pub fn proto_session_lane_to_internal(lane: i32) -> Option<SessionLane> {
    match proto::SessionLane::try_from(lane) {
        Ok(proto::SessionLane::Control) => Some(SessionLane::Control),
        Ok(proto::SessionLane::Query) => Some(SessionLane::Query),
        Ok(proto::SessionLane::Execute) => Some(SessionLane::Execute),
        Ok(proto::SessionLane::Generate) => Some(SessionLane::Generate),
        _ => None,
    }
}

/// Convert internal SessionLane to proto SessionLane
pub fn internal_session_lane_to_proto(lane: SessionLane) -> i32 {
    match lane {
        SessionLane::Control => proto::SessionLane::Control as i32,
        SessionLane::Query => proto::SessionLane::Query as i32,
        SessionLane::Execute => proto::SessionLane::Execute as i32,
        SessionLane::Generate => proto::SessionLane::Generate as i32,
    }
}

/// Convert proto ConfirmationPolicy to internal ConfirmationPolicy
pub fn proto_confirmation_policy_to_internal(
    policy: &proto::ConfirmationPolicy,
) -> ConfirmationPolicy {
    let timeout_action = match proto::TimeoutAction::try_from(policy.timeout_action) {
        Ok(proto::TimeoutAction::AutoApprove) => TimeoutAction::AutoApprove,
        _ => TimeoutAction::Reject,
    };

    let yolo_lanes: HashSet<SessionLane> = policy
        .yolo_lanes
        .iter()
        .filter_map(|l| proto_session_lane_to_internal(*l))
        .collect();

    ConfirmationPolicy {
        enabled: policy.enabled,
        auto_approve_tools: policy.auto_approve_tools.iter().cloned().collect(),
        require_confirm_tools: policy.require_confirm_tools.iter().cloned().collect(),
        default_timeout_ms: policy.default_timeout_ms,
        timeout_action,
        yolo_lanes,
    }
}

/// Convert internal ConfirmationPolicy to proto ConfirmationPolicy
pub fn internal_confirmation_policy_to_proto(
    policy: &ConfirmationPolicy,
) -> proto::ConfirmationPolicy {
    let timeout_action = match policy.timeout_action {
        TimeoutAction::Reject => proto::TimeoutAction::Reject as i32,
        TimeoutAction::AutoApprove => proto::TimeoutAction::AutoApprove as i32,
    };

    let yolo_lanes: Vec<i32> = policy
        .yolo_lanes
        .iter()
        .map(|l| internal_session_lane_to_proto(*l))
        .collect();

    proto::ConfirmationPolicy {
        enabled: policy.enabled,
        auto_approve_tools: policy.auto_approve_tools.iter().cloned().collect(),
        require_confirm_tools: policy.require_confirm_tools.iter().cloned().collect(),
        default_timeout_ms: policy.default_timeout_ms,
        timeout_action,
        yolo_lanes,
    }
}

// ============================================================================
// External Task Handler Conversions
// ============================================================================

/// Convert proto TaskHandlerMode to internal TaskHandlerMode
pub fn proto_task_handler_mode_to_internal(mode: i32) -> TaskHandlerMode {
    match proto::TaskHandlerMode::try_from(mode) {
        Ok(proto::TaskHandlerMode::External) => TaskHandlerMode::External,
        Ok(proto::TaskHandlerMode::Hybrid) => TaskHandlerMode::Hybrid,
        _ => TaskHandlerMode::Internal,
    }
}

/// Convert internal TaskHandlerMode to proto TaskHandlerMode
pub fn internal_task_handler_mode_to_proto(mode: TaskHandlerMode) -> i32 {
    match mode {
        TaskHandlerMode::Internal => proto::TaskHandlerMode::Internal as i32,
        TaskHandlerMode::External => proto::TaskHandlerMode::External as i32,
        TaskHandlerMode::Hybrid => proto::TaskHandlerMode::Hybrid as i32,
    }
}

/// Convert proto LaneHandlerConfig to internal LaneHandlerConfig
pub fn proto_lane_handler_config_to_internal(
    config: &proto::LaneHandlerConfig,
) -> LaneHandlerConfig {
    LaneHandlerConfig {
        mode: proto_task_handler_mode_to_internal(config.mode),
        timeout_ms: config.timeout_ms,
    }
}

/// Convert internal LaneHandlerConfig to proto LaneHandlerConfig
pub fn internal_lane_handler_config_to_proto(
    config: &LaneHandlerConfig,
) -> proto::LaneHandlerConfig {
    proto::LaneHandlerConfig {
        mode: internal_task_handler_mode_to_proto(config.mode),
        timeout_ms: config.timeout_ms,
    }
}

/// Convert internal ExternalTask to proto ExternalTask
pub fn internal_external_task_to_proto(task: &ExternalTask) -> proto::ExternalTask {
    proto::ExternalTask {
        task_id: task.task_id.clone(),
        session_id: task.session_id.clone(),
        lane: internal_session_lane_to_proto(task.lane),
        command_type: task.command_type.clone(),
        payload: task.payload.to_string(),
        timeout_ms: task.timeout_ms,
        remaining_ms: task.remaining_ms(),
    }
}

/// Convert proto CompleteExternalTaskRequest to internal ExternalTaskResult
pub fn proto_complete_request_to_result(
    success: bool,
    result: &str,
    error: &str,
) -> ExternalTaskResult {
    let result_value = serde_json::from_str(result).unwrap_or(serde_json::json!({}));
    ExternalTaskResult {
        success,
        result: result_value,
        error: if error.is_empty() {
            None
        } else {
            Some(error.to_string())
        },
    }
}

// ============================================================================
// FinishReason Conversions
// ============================================================================

/// Convert LLM stop reason to proto FinishReason
pub fn stop_reason_to_finish_reason(stop_reason: Option<&str>) -> i32 {
    match stop_reason {
        Some("end_turn") | Some("stop") => proto::FinishReason::Stop as i32,
        Some("max_tokens") | Some("length") => proto::FinishReason::Length as i32,
        Some("tool_use") | Some("tool_calls") => proto::FinishReason::ToolCalls as i32,
        Some("content_filter") => proto::FinishReason::ContentFilter as i32,
        _ => proto::FinishReason::Unknown as i32,
    }
}

// ============================================================================
// Permission System Conversions
// ============================================================================

/// Convert proto PermissionDecision to internal PermissionDecision
pub fn proto_permission_decision_to_internal(decision: i32) -> PermissionDecision {
    match proto::PermissionDecision::try_from(decision) {
        Ok(proto::PermissionDecision::Allow) => PermissionDecision::Allow,
        Ok(proto::PermissionDecision::Deny) => PermissionDecision::Deny,
        Ok(proto::PermissionDecision::Ask) => PermissionDecision::Ask,
        _ => PermissionDecision::Ask,
    }
}

/// Convert internal PermissionDecision to proto PermissionDecision
pub fn internal_permission_decision_to_proto(decision: PermissionDecision) -> i32 {
    match decision {
        PermissionDecision::Allow => proto::PermissionDecision::Allow as i32,
        PermissionDecision::Deny => proto::PermissionDecision::Deny as i32,
        PermissionDecision::Ask => proto::PermissionDecision::Ask as i32,
    }
}

/// Convert proto PermissionRule to internal PermissionRule
pub fn proto_permission_rule_to_internal(rule: &proto::PermissionRule) -> PermissionRule {
    PermissionRule::new(&rule.rule)
}

/// Convert internal PermissionRule to proto PermissionRule
pub fn internal_permission_rule_to_proto(rule: &PermissionRule) -> proto::PermissionRule {
    proto::PermissionRule {
        rule: rule.rule.clone(),
    }
}

/// Convert proto PermissionPolicy to internal PermissionPolicy
pub fn proto_permission_policy_to_internal(policy: &proto::PermissionPolicy) -> PermissionPolicy {
    PermissionPolicy {
        deny: policy
            .deny
            .iter()
            .map(proto_permission_rule_to_internal)
            .collect(),
        allow: policy
            .allow
            .iter()
            .map(proto_permission_rule_to_internal)
            .collect(),
        ask: policy
            .ask
            .iter()
            .map(proto_permission_rule_to_internal)
            .collect(),
        default_decision: proto_permission_decision_to_internal(policy.default_decision),
        enabled: policy.enabled,
    }
}

/// Convert internal PermissionPolicy to proto PermissionPolicy
pub fn internal_permission_policy_to_proto(policy: &PermissionPolicy) -> proto::PermissionPolicy {
    proto::PermissionPolicy {
        deny: policy
            .deny
            .iter()
            .map(internal_permission_rule_to_proto)
            .collect(),
        allow: policy
            .allow
            .iter()
            .map(internal_permission_rule_to_proto)
            .collect(),
        ask: policy
            .ask
            .iter()
            .map(internal_permission_rule_to_proto)
            .collect(),
        default_decision: internal_permission_decision_to_proto(policy.default_decision),
        enabled: policy.enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proto_message_to_internal() {
        let proto_msg = proto::Message {
            role: proto::message::Role::User as i32,
            content: "Hello".to_string(),
            attachments: vec![],
            metadata: std::collections::HashMap::new(),
        };

        let internal = proto_message_to_internal(&proto_msg);
        assert_eq!(internal.role, "user");
        assert_eq!(internal.text(), "Hello");
    }

    #[test]
    fn test_internal_message_to_proto() {
        let internal = InternalMessage::user("Hello");
        let proto_msg = internal_message_to_proto(&internal);

        assert_eq!(proto_msg.role, proto::message::Role::User as i32);
        assert_eq!(proto_msg.content, "Hello");
    }

    #[test]
    fn test_usage_conversion() {
        let internal = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };

        let proto_usage = internal_usage_to_proto(&internal);
        assert_eq!(proto_usage.prompt_tokens, 100);
        assert_eq!(proto_usage.completion_tokens, 50);
        assert_eq!(proto_usage.total_tokens, 150);
    }
}
