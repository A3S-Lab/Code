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
use crate::todo::Todo;
use std::collections::HashSet;

// ============================================================================
// Message Conversions
// ============================================================================

/// Convert proto Message to internal Message
/// Proto now uses OpenAI-compatible string roles directly
pub fn proto_message_to_internal(msg: &proto::Message) -> InternalMessage {
    // Normalize role string to lowercase
    let role = match msg.role.to_lowercase().as_str() {
        "user" => "user",
        "assistant" => "assistant",
        "system" => "system",
        "tool" => "user", // Tool results are sent as user messages
        _ => "user",
    };

    let content = vec![ContentBlock::Text {
        text: msg.content.clone(),
    }];

    InternalMessage {
        role: role.to_string(),
        content,
        reasoning_content: None,
    }
}

/// Convert internal Message to proto Message
/// Proto now uses OpenAI-compatible string roles directly
pub fn internal_message_to_proto(msg: &InternalMessage) -> proto::Message {
    proto::Message {
        role: msg.role.clone(),
        content: msg.text(),
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
            r#type: "content".to_string(),
            session_id: session_id.to_string(),
            content: text,
            tool_call: None,
            tool_result: None,
            metadata: std::collections::HashMap::new(),
            finish_reason: String::new(),
        }),
        InternalAgentEvent::ToolStart { id, name } => Some(proto::GenerateChunk {
            r#type: "tool_call".to_string(),
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
            finish_reason: String::new(),
        }),
        InternalAgentEvent::ToolEnd {
            id: _,
            name: _,
            output,
            exit_code,
        } => Some(proto::GenerateChunk {
            r#type: "tool_result".to_string(),
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
            finish_reason: String::new(),
        }),
        InternalAgentEvent::ToolOutputDelta { id: _, name: _, delta } => {
            Some(proto::GenerateChunk {
                r#type: "tool_output_delta".to_string(),
                session_id: session_id.to_string(),
                content: delta,
                tool_call: None,
                tool_result: None,
                metadata: std::collections::HashMap::new(),
                finish_reason: String::new(),
            })
        }
        InternalAgentEvent::End { text, usage } => Some(proto::GenerateChunk {
            r#type: "done".to_string(),
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
            finish_reason: "stop".to_string(),
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
        InternalAgentEvent::ToolOutputDelta { id, name, delta } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::ToolCalled as i32,
            session_id: session_id.map(String::from),
            timestamp,
            message: format!("Tool output delta: {}", name),
            data: [
                ("tool_id".to_string(), id),
                ("tool_name".to_string(), name),
                ("delta".to_string(), delta),
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
        InternalAgentEvent::ContextCompacted {
            session_id: compacted_session_id,
            before_messages,
            after_messages,
            percent_before,
        } => Some(proto::AgentEvent {
            r#type: proto::agent_event::EventType::ContextCompacted as i32,
            session_id: Some(compacted_session_id),
            timestamp,
            message: format!(
                "Context auto-compacted: {} → {} messages (was {:.1}% full)",
                before_messages,
                after_messages,
                percent_before * 100.0
            ),
            data: [
                ("before_messages".to_string(), before_messages.to_string()),
                ("after_messages".to_string(), after_messages.to_string()),
                (
                    "percent_before".to_string(),
                    format!("{:.4}", percent_before),
                ),
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

/// Convert LLM stop reason to OpenAI-compatible finish_reason string
pub fn stop_reason_to_finish_reason(stop_reason: Option<&str>) -> String {
    match stop_reason {
        Some("end_turn") | Some("stop") => "stop".to_string(),
        Some("max_tokens") | Some("length") => "length".to_string(),
        Some("tool_use") | Some("tool_calls") => "tool_calls".to_string(),
        Some("content_filter") => "content_filter".to_string(),
        Some("error") => "error".to_string(),
        _ => "stop".to_string(), // Default to "stop" for unknown reasons
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

// ============================================================================
// Todo Conversions
// ============================================================================

/// Convert internal Todo to proto Todo
pub fn internal_todo_to_proto(todo: &Todo) -> proto::Todo {
    proto::Todo {
        id: todo.id.clone(),
        content: todo.content.clone(),
        status: todo.status.to_string(),
        priority: todo.priority.to_string(),
    }
}

/// Convert proto Todo to internal Todo
pub fn proto_todo_to_internal(todo: &proto::Todo) -> Todo {
    Todo {
        id: todo.id.clone(),
        content: todo.content.clone(),
        status: todo.status.parse().unwrap_or_default(),
        priority: todo.priority.parse().unwrap_or_default(),
    }
}

// ============================================================================
// Conversation Message Conversions
// ============================================================================

/// Convert internal Message to proto ConversationMessage
pub fn internal_message_to_conversation_message(
    msg: &InternalMessage,
    index: usize,
    timestamp: i64,
) -> proto::ConversationMessage {
    let content_blocks: Vec<proto::ContentBlock> = msg
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => proto::ContentBlock {
                block: Some(proto::content_block::Block::Text(proto::TextContent {
                    text: text.clone(),
                })),
            },
            ContentBlock::ToolUse { id, name, input } => proto::ContentBlock {
                block: Some(proto::content_block::Block::ToolUse(
                    proto::ToolUseContent {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: input.to_string(),
                    },
                )),
            },
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => proto::ContentBlock {
                block: Some(proto::content_block::Block::ToolResult(
                    proto::ToolResultContent {
                        tool_use_id: tool_use_id.clone(),
                        content: content.clone(),
                        is_error: is_error.unwrap_or(false),
                    },
                )),
            },
        })
        .collect();

    proto::ConversationMessage {
        id: format!("msg_{}", index),
        role: msg.role.clone(),
        content: content_blocks,
        timestamp,
        metadata: std::collections::HashMap::new(),
    }
}

// ============================================================================
// Provider Configuration Conversions
// ============================================================================

use crate::config::{ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};

/// Convert internal ModelCost to proto ModelCostInfo
pub fn internal_model_cost_to_proto(cost: &ModelCost) -> proto::ModelCostInfo {
    proto::ModelCostInfo {
        input: cost.input,
        output: cost.output,
        cache_read: cost.cache_read,
        cache_write: cost.cache_write,
    }
}

/// Convert proto ModelCostInfo to internal ModelCost
pub fn proto_model_cost_to_internal(cost: &proto::ModelCostInfo) -> ModelCost {
    ModelCost {
        input: cost.input,
        output: cost.output,
        cache_read: cost.cache_read,
        cache_write: cost.cache_write,
    }
}

/// Convert internal ModelLimit to proto ModelLimitInfo
pub fn internal_model_limit_to_proto(limit: &ModelLimit) -> proto::ModelLimitInfo {
    proto::ModelLimitInfo {
        context: limit.context,
        output: limit.output,
    }
}

/// Convert proto ModelLimitInfo to internal ModelLimit
pub fn proto_model_limit_to_internal(limit: &proto::ModelLimitInfo) -> ModelLimit {
    ModelLimit {
        context: limit.context,
        output: limit.output,
    }
}

/// Convert internal ModelModalities to proto ModelModalitiesInfo
pub fn internal_model_modalities_to_proto(
    modalities: &ModelModalities,
) -> proto::ModelModalitiesInfo {
    proto::ModelModalitiesInfo {
        input: modalities.input.clone(),
        output: modalities.output.clone(),
    }
}

/// Convert proto ModelModalitiesInfo to internal ModelModalities
pub fn proto_model_modalities_to_internal(
    modalities: &proto::ModelModalitiesInfo,
) -> ModelModalities {
    ModelModalities {
        input: modalities.input.clone(),
        output: modalities.output.clone(),
    }
}

/// Convert internal ModelConfig to proto ModelInfo
pub fn internal_model_config_to_proto(model: &ModelConfig) -> proto::ModelInfo {
    proto::ModelInfo {
        id: model.id.clone(),
        name: model.name.clone(),
        family: model.family.clone(),
        api_key: model.api_key.clone(),
        base_url: model.base_url.clone(),
        attachment: model.attachment,
        reasoning: model.reasoning,
        tool_call: model.tool_call,
        temperature: model.temperature,
        release_date: model.release_date.clone(),
        modalities: Some(internal_model_modalities_to_proto(&model.modalities)),
        cost: Some(internal_model_cost_to_proto(&model.cost)),
        limit: Some(internal_model_limit_to_proto(&model.limit)),
    }
}

/// Convert proto ModelInfo to internal ModelConfig
pub fn proto_model_info_to_internal(model: &proto::ModelInfo) -> ModelConfig {
    ModelConfig {
        id: model.id.clone(),
        name: model.name.clone(),
        family: model.family.clone(),
        api_key: model.api_key.clone(),
        base_url: model.base_url.clone(),
        attachment: model.attachment,
        reasoning: model.reasoning,
        tool_call: model.tool_call,
        temperature: model.temperature,
        release_date: model.release_date.clone(),
        modalities: model
            .modalities
            .as_ref()
            .map(proto_model_modalities_to_internal)
            .unwrap_or_default(),
        cost: model
            .cost
            .as_ref()
            .map(proto_model_cost_to_internal)
            .unwrap_or_default(),
        limit: model
            .limit
            .as_ref()
            .map(proto_model_limit_to_internal)
            .unwrap_or_default(),
    }
}

/// Convert internal ProviderConfig to proto ProviderInfo
pub fn internal_provider_config_to_proto(provider: &ProviderConfig) -> proto::ProviderInfo {
    proto::ProviderInfo {
        name: provider.name.clone(),
        api_key: provider.api_key.clone(),
        base_url: provider.base_url.clone(),
        models: provider
            .models
            .iter()
            .map(internal_model_config_to_proto)
            .collect(),
    }
}

/// Convert proto ProviderInfo to internal ProviderConfig
pub fn proto_provider_info_to_internal(provider: &proto::ProviderInfo) -> ProviderConfig {
    ProviderConfig {
        name: provider.name.clone(),
        api_key: provider.api_key.clone(),
        base_url: provider.base_url.clone(),
        models: provider
            .models
            .iter()
            .map(proto_model_info_to_internal)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proto_message_to_internal() {
        // Test with OpenAI-compatible string role
        let proto_msg = proto::Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            metadata: std::collections::HashMap::new(),
        };

        let internal = proto_message_to_internal(&proto_msg);
        assert_eq!(internal.role, "user");
        assert_eq!(internal.text(), "Hello");

        // Test case-insensitive role handling
        let proto_msg_upper = proto::Message {
            role: "USER".to_string(),
            content: "Hello".to_string(),
            metadata: std::collections::HashMap::new(),
        };
        let internal_upper = proto_message_to_internal(&proto_msg_upper);
        assert_eq!(internal_upper.role, "user");
    }

    #[test]
    fn test_internal_message_to_proto() {
        let internal = InternalMessage::user("Hello");
        let proto_msg = internal_message_to_proto(&internal);

        assert_eq!(proto_msg.role, "user");
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

    #[test]
    fn test_internal_message_to_conversation_message() {
        // Test text message
        let msg = InternalMessage::user("Hello, world!");
        let conv_msg = internal_message_to_conversation_message(&msg, 0, 1234567890);

        assert_eq!(conv_msg.id, "msg_0");
        assert_eq!(conv_msg.role, "user");
        assert_eq!(conv_msg.timestamp, 1234567890);
        assert_eq!(conv_msg.content.len(), 1);

        // Check text content
        if let Some(proto::content_block::Block::Text(text)) = &conv_msg.content[0].block {
            assert_eq!(text.text, "Hello, world!");
        } else {
            panic!("Expected text content block");
        }

        // Test assistant message with tool use
        let msg_with_tool = InternalMessage {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "Let me help you.".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "tool_123".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"path": "/tmp/test.txt"}),
                },
            ],
            reasoning_content: None,
        };
        let conv_msg2 = internal_message_to_conversation_message(&msg_with_tool, 1, 1234567891);

        assert_eq!(conv_msg2.id, "msg_1");
        assert_eq!(conv_msg2.role, "assistant");
        assert_eq!(conv_msg2.content.len(), 2);

        // Check tool use content
        if let Some(proto::content_block::Block::ToolUse(tool_use)) = &conv_msg2.content[1].block {
            assert_eq!(tool_use.id, "tool_123");
            assert_eq!(tool_use.name, "read");
        } else {
            panic!("Expected tool use content block");
        }
    }
}

#[cfg(test)]
mod extra_convert_tests {
    use super::*;
    use crate::todo::{TodoPriority, TodoStatus};

    // ========================================================================
    // Stop Reason / Finish Reason
    // ========================================================================

    #[test]
    fn test_stop_reason_end_turn() {
        assert_eq!(stop_reason_to_finish_reason(Some("end_turn")), "stop");
    }

    #[test]
    fn test_stop_reason_stop() {
        assert_eq!(stop_reason_to_finish_reason(Some("stop")), "stop");
    }

    #[test]
    fn test_stop_reason_max_tokens() {
        assert_eq!(stop_reason_to_finish_reason(Some("max_tokens")), "length");
    }

    #[test]
    fn test_stop_reason_length() {
        assert_eq!(stop_reason_to_finish_reason(Some("length")), "length");
    }

    #[test]
    fn test_stop_reason_tool_use() {
        assert_eq!(stop_reason_to_finish_reason(Some("tool_use")), "tool_calls");
    }

    #[test]
    fn test_stop_reason_tool_calls() {
        assert_eq!(
            stop_reason_to_finish_reason(Some("tool_calls")),
            "tool_calls"
        );
    }

    #[test]
    fn test_stop_reason_content_filter() {
        assert_eq!(
            stop_reason_to_finish_reason(Some("content_filter")),
            "content_filter"
        );
    }

    #[test]
    fn test_stop_reason_error() {
        assert_eq!(stop_reason_to_finish_reason(Some("error")), "error");
    }

    #[test]
    fn test_stop_reason_none() {
        assert_eq!(stop_reason_to_finish_reason(None), "stop");
    }

    #[test]
    fn test_stop_reason_unknown() {
        assert_eq!(stop_reason_to_finish_reason(Some("something_else")), "stop");
    }

    // ========================================================================
    // Permission Decision Conversions
    // ========================================================================

    #[test]
    fn test_permission_decision_allow_roundtrip() {
        let proto_val = internal_permission_decision_to_proto(PermissionDecision::Allow);
        let back = proto_permission_decision_to_internal(proto_val);
        assert_eq!(back, PermissionDecision::Allow);
    }

    #[test]
    fn test_permission_decision_deny_roundtrip() {
        let proto_val = internal_permission_decision_to_proto(PermissionDecision::Deny);
        let back = proto_permission_decision_to_internal(proto_val);
        assert_eq!(back, PermissionDecision::Deny);
    }

    #[test]
    fn test_permission_decision_ask_roundtrip() {
        let proto_val = internal_permission_decision_to_proto(PermissionDecision::Ask);
        let back = proto_permission_decision_to_internal(proto_val);
        assert_eq!(back, PermissionDecision::Ask);
    }

    #[test]
    fn test_permission_decision_unknown_defaults_to_ask() {
        let back = proto_permission_decision_to_internal(999);
        assert_eq!(back, PermissionDecision::Ask);
    }

    // ========================================================================
    // Permission Rule Conversions
    // ========================================================================

    #[test]
    fn test_permission_rule_roundtrip() {
        let rule = PermissionRule::new("Bash(cargo:*)");
        let proto_rule = internal_permission_rule_to_proto(&rule);
        assert_eq!(proto_rule.rule, "Bash(cargo:*)");
        let back = proto_permission_rule_to_internal(&proto_rule);
        assert_eq!(back.rule, "Bash(cargo:*)");
    }

    #[test]
    fn test_permission_rule_simple() {
        let rule = PermissionRule::new("Read");
        let proto_rule = internal_permission_rule_to_proto(&rule);
        let back = proto_permission_rule_to_internal(&proto_rule);
        assert_eq!(back.rule, "Read");
    }

    // ========================================================================
    // Permission Policy Conversions
    // ========================================================================

    #[test]
    fn test_permission_policy_roundtrip() {
        let policy = PermissionPolicy {
            deny: vec![PermissionRule::new("Bash(rm -rf *)")],
            allow: vec![PermissionRule::new("Read"), PermissionRule::new("Grep(*)")],
            ask: vec![PermissionRule::new("Bash(*)")],
            default_decision: PermissionDecision::Ask,
            enabled: true,
        };
        let proto_policy = internal_permission_policy_to_proto(&policy);
        assert_eq!(proto_policy.deny.len(), 1);
        assert_eq!(proto_policy.allow.len(), 2);
        assert_eq!(proto_policy.ask.len(), 1);
        assert!(proto_policy.enabled);

        let back = proto_permission_policy_to_internal(&proto_policy);
        assert_eq!(back.deny.len(), 1);
        assert_eq!(back.allow.len(), 2);
        assert_eq!(back.ask.len(), 1);
        assert_eq!(back.default_decision, PermissionDecision::Ask);
        assert!(back.enabled);
    }

    #[test]
    fn test_permission_policy_empty() {
        let policy = PermissionPolicy {
            deny: vec![],
            allow: vec![],
            ask: vec![],
            default_decision: PermissionDecision::Allow,
            enabled: false,
        };
        let proto_policy = internal_permission_policy_to_proto(&policy);
        assert!(proto_policy.deny.is_empty());
        assert!(proto_policy.allow.is_empty());
        assert!(proto_policy.ask.is_empty());
        assert!(!proto_policy.enabled);

        let back = proto_permission_policy_to_internal(&proto_policy);
        assert_eq!(back.default_decision, PermissionDecision::Allow);
        assert!(!back.enabled);
    }
}

#[cfg(test)]
mod extra_convert_tests2 {
    use super::*;
    use crate::todo::{TodoPriority, TodoStatus};

    // ========================================================================
    // Todo Conversions
    // ========================================================================

    #[test]
    fn test_todo_roundtrip() {
        let todo = Todo {
            id: "todo-1".to_string(),
            content: "Fix the bug".to_string(),
            status: TodoStatus::Pending,
            priority: TodoPriority::High,
        };
        let proto_todo = internal_todo_to_proto(&todo);
        assert_eq!(proto_todo.id, "todo-1");
        assert_eq!(proto_todo.content, "Fix the bug");
        assert_eq!(proto_todo.status, "pending");
        assert_eq!(proto_todo.priority, "high");

        let back = proto_todo_to_internal(&proto_todo);
        assert_eq!(back.id, "todo-1");
        assert_eq!(back.content, "Fix the bug");
        assert_eq!(back.status, TodoStatus::Pending);
        assert_eq!(back.priority, TodoPriority::High);
    }

    #[test]
    fn test_todo_completed_medium() {
        let todo = Todo {
            id: "todo-2".to_string(),
            content: "Write tests".to_string(),
            status: TodoStatus::Completed,
            priority: TodoPriority::Medium,
        };
        let proto_todo = internal_todo_to_proto(&todo);
        assert_eq!(proto_todo.status, "completed");
        assert_eq!(proto_todo.priority, "medium");

        let back = proto_todo_to_internal(&proto_todo);
        assert_eq!(back.status, TodoStatus::Completed);
        assert_eq!(back.priority, TodoPriority::Medium);
    }

    #[test]
    fn test_todo_in_progress_low() {
        let todo = Todo {
            id: "todo-3".to_string(),
            content: "Refactor".to_string(),
            status: TodoStatus::InProgress,
            priority: TodoPriority::Low,
        };
        let proto_todo = internal_todo_to_proto(&todo);
        assert_eq!(proto_todo.status, "in_progress");
        assert_eq!(proto_todo.priority, "low");
    }

    #[test]
    fn test_todo_cancelled() {
        let todo = Todo {
            id: "todo-4".to_string(),
            content: "Old task".to_string(),
            status: TodoStatus::Cancelled,
            priority: TodoPriority::Medium,
        };
        let proto_todo = internal_todo_to_proto(&todo);
        assert_eq!(proto_todo.status, "cancelled");
    }

    #[test]
    fn test_todo_unknown_status_defaults() {
        let proto_todo = proto::Todo {
            id: "t1".to_string(),
            content: "test".to_string(),
            status: "unknown_status".to_string(),
            priority: "unknown_priority".to_string(),
        };
        let back = proto_todo_to_internal(&proto_todo);
        assert_eq!(back.status, TodoStatus::Pending);
        assert_eq!(back.priority, TodoPriority::Medium);
    }

    // ========================================================================
    // SessionLane Conversions
    // ========================================================================

    #[test]
    fn test_session_lane_control_roundtrip() {
        let proto_val = internal_session_lane_to_proto(SessionLane::Control);
        let back = proto_session_lane_to_internal(proto_val);
        assert_eq!(back, Some(SessionLane::Control));
    }

    #[test]
    fn test_session_lane_query_roundtrip() {
        let proto_val = internal_session_lane_to_proto(SessionLane::Query);
        let back = proto_session_lane_to_internal(proto_val);
        assert_eq!(back, Some(SessionLane::Query));
    }

    #[test]
    fn test_session_lane_execute_roundtrip() {
        let proto_val = internal_session_lane_to_proto(SessionLane::Execute);
        let back = proto_session_lane_to_internal(proto_val);
        assert_eq!(back, Some(SessionLane::Execute));
    }

    #[test]
    fn test_session_lane_generate_roundtrip() {
        let proto_val = internal_session_lane_to_proto(SessionLane::Generate);
        let back = proto_session_lane_to_internal(proto_val);
        assert_eq!(back, Some(SessionLane::Generate));
    }

    #[test]
    fn test_session_lane_unknown_returns_none() {
        let back = proto_session_lane_to_internal(999);
        assert_eq!(back, None);
    }

    // ========================================================================
    // TaskHandlerMode Conversions
    // ========================================================================

    #[test]
    fn test_task_handler_mode_internal_roundtrip() {
        let proto_val = internal_task_handler_mode_to_proto(TaskHandlerMode::Internal);
        let back = proto_task_handler_mode_to_internal(proto_val);
        assert!(matches!(back, TaskHandlerMode::Internal));
    }

    #[test]
    fn test_task_handler_mode_external_roundtrip() {
        let proto_val = internal_task_handler_mode_to_proto(TaskHandlerMode::External);
        let back = proto_task_handler_mode_to_internal(proto_val);
        assert!(matches!(back, TaskHandlerMode::External));
    }

    #[test]
    fn test_task_handler_mode_hybrid_roundtrip() {
        let proto_val = internal_task_handler_mode_to_proto(TaskHandlerMode::Hybrid);
        let back = proto_task_handler_mode_to_internal(proto_val);
        assert!(matches!(back, TaskHandlerMode::Hybrid));
    }

    #[test]
    fn test_task_handler_mode_unknown_defaults_internal() {
        let back = proto_task_handler_mode_to_internal(999);
        assert!(matches!(back, TaskHandlerMode::Internal));
    }

    // ========================================================================
    // LaneHandlerConfig Conversions
    // ========================================================================

    #[test]
    fn test_lane_handler_config_roundtrip() {
        let config = LaneHandlerConfig {
            mode: TaskHandlerMode::External,
            timeout_ms: 30000,
        };
        let proto_config = internal_lane_handler_config_to_proto(&config);
        assert_eq!(proto_config.timeout_ms, 30000);

        let back = proto_lane_handler_config_to_internal(&proto_config);
        assert!(matches!(back.mode, TaskHandlerMode::External));
        assert_eq!(back.timeout_ms, 30000);
    }

    #[test]
    fn test_lane_handler_config_hybrid() {
        let config = LaneHandlerConfig {
            mode: TaskHandlerMode::Hybrid,
            timeout_ms: 120000,
        };
        let proto_config = internal_lane_handler_config_to_proto(&config);
        let back = proto_lane_handler_config_to_internal(&proto_config);
        assert!(matches!(back.mode, TaskHandlerMode::Hybrid));
        assert_eq!(back.timeout_ms, 120000);
    }

    // ========================================================================
    // ConfirmationPolicy Conversions
    // ========================================================================

    #[test]
    fn test_confirmation_policy_roundtrip() {
        let mut auto_approve = HashSet::new();
        auto_approve.insert("read".to_string());
        auto_approve.insert("grep".to_string());

        let mut require_confirm = HashSet::new();
        require_confirm.insert("bash".to_string());

        let mut yolo_lanes = HashSet::new();
        yolo_lanes.insert(SessionLane::Query);

        let policy = ConfirmationPolicy {
            enabled: true,
            auto_approve_tools: auto_approve,
            require_confirm_tools: require_confirm,
            default_timeout_ms: 60000,
            timeout_action: TimeoutAction::AutoApprove,
            yolo_lanes,
        };

        let proto_policy = internal_confirmation_policy_to_proto(&policy);
        assert!(proto_policy.enabled);
        assert_eq!(proto_policy.auto_approve_tools.len(), 2);
        assert_eq!(proto_policy.require_confirm_tools.len(), 1);
        assert_eq!(proto_policy.default_timeout_ms, 60000);
        assert_eq!(proto_policy.yolo_lanes.len(), 1);

        let back = proto_confirmation_policy_to_internal(&proto_policy);
        assert!(back.enabled);
        assert!(back.auto_approve_tools.contains("read"));
        assert!(back.auto_approve_tools.contains("grep"));
        assert!(back.require_confirm_tools.contains("bash"));
        assert_eq!(back.default_timeout_ms, 60000);
        assert_eq!(back.timeout_action, TimeoutAction::AutoApprove);
        assert!(back.yolo_lanes.contains(&SessionLane::Query));
    }

    #[test]
    fn test_confirmation_policy_default_reject() {
        let policy = ConfirmationPolicy {
            enabled: false,
            auto_approve_tools: HashSet::new(),
            require_confirm_tools: HashSet::new(),
            default_timeout_ms: 30000,
            timeout_action: TimeoutAction::Reject,
            yolo_lanes: HashSet::new(),
        };

        let proto_policy = internal_confirmation_policy_to_proto(&policy);
        let back = proto_confirmation_policy_to_internal(&proto_policy);
        assert!(!back.enabled);
        assert_eq!(back.timeout_action, TimeoutAction::Reject);
        assert!(back.yolo_lanes.is_empty());
    }

    #[test]
    fn test_confirmation_policy_multiple_yolo_lanes() {
        let mut yolo_lanes = HashSet::new();
        yolo_lanes.insert(SessionLane::Query);
        yolo_lanes.insert(SessionLane::Control);
        yolo_lanes.insert(SessionLane::Execute);

        let policy = ConfirmationPolicy {
            enabled: true,
            auto_approve_tools: HashSet::new(),
            require_confirm_tools: HashSet::new(),
            default_timeout_ms: 5000,
            timeout_action: TimeoutAction::Reject,
            yolo_lanes,
        };

        let proto_policy = internal_confirmation_policy_to_proto(&policy);
        assert_eq!(proto_policy.yolo_lanes.len(), 3);

        let back = proto_confirmation_policy_to_internal(&proto_policy);
        assert_eq!(back.yolo_lanes.len(), 3);
        assert!(back.yolo_lanes.contains(&SessionLane::Query));
        assert!(back.yolo_lanes.contains(&SessionLane::Control));
        assert!(back.yolo_lanes.contains(&SessionLane::Execute));
    }
}

#[cfg(test)]
mod extra_convert_tests3 {
    use super::*;
    use crate::config::{ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};

    // ========================================================================
    // ExternalTask Conversions
    // ========================================================================

    #[test]
    fn test_external_task_to_proto() {
        let task = ExternalTask {
            task_id: "task-123".to_string(),
            session_id: "sess-456".to_string(),
            lane: SessionLane::Execute,
            command_type: "bash".to_string(),
            payload: serde_json::json!({"command": "ls -la"}),
            timeout_ms: 30000,
            created_at: None,
        };
        let proto_task = internal_external_task_to_proto(&task);
        assert_eq!(proto_task.task_id, "task-123");
        assert_eq!(proto_task.session_id, "sess-456");
        assert_eq!(proto_task.command_type, "bash");
        assert_eq!(proto_task.timeout_ms, 30000);
        assert!(proto_task.payload.contains("ls -la"));
    }

    #[test]
    fn test_external_task_to_proto_query_lane() {
        let task = ExternalTask {
            task_id: "task-789".to_string(),
            session_id: "sess-abc".to_string(),
            lane: SessionLane::Query,
            command_type: "read".to_string(),
            payload: serde_json::json!({"path": "/tmp/test.txt"}),
            timeout_ms: 5000,
            created_at: None,
        };
        let proto_task = internal_external_task_to_proto(&task);
        assert_eq!(proto_task.command_type, "read");
        assert_eq!(proto_task.timeout_ms, 5000);
    }

    // ========================================================================
    // proto_complete_request_to_result
    // ========================================================================

    #[test]
    fn test_complete_request_to_result_success() {
        let result = proto_complete_request_to_result(true, r#"{"output": "hello"}"#, "");
        assert!(result.success);
        assert_eq!(result.result["output"], "hello");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_complete_request_to_result_failure() {
        let result = proto_complete_request_to_result(false, "{}", "Something went wrong");
        assert!(!result.success);
        assert_eq!(result.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_complete_request_to_result_invalid_json() {
        let result = proto_complete_request_to_result(true, "not valid json", "");
        assert!(result.success);
        // Should default to empty object on invalid JSON
        assert_eq!(result.result, serde_json::json!({}));
    }

    #[test]
    fn test_complete_request_to_result_empty_error() {
        let result = proto_complete_request_to_result(false, "{}", "");
        assert!(!result.success);
        assert!(result.error.is_none()); // empty string becomes None
    }

    // ========================================================================
    // Tool Call Conversions
    // ========================================================================

    #[test]
    fn test_proto_tool_call_to_args() {
        let tc = proto::ToolCall {
            id: "call-1".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command": "echo hello"}"#.to_string(),
            result: None,
        };
        let (name, args) = proto_tool_call_to_args(&tc);
        assert_eq!(name, "bash");
        assert_eq!(args["command"], "echo hello");
    }

    #[test]
    fn test_proto_tool_call_to_args_invalid_json() {
        let tc = proto::ToolCall {
            id: "call-2".to_string(),
            name: "read".to_string(),
            arguments: "not json".to_string(),
            result: None,
        };
        let (name, args) = proto_tool_call_to_args(&tc);
        assert_eq!(name, "read");
        assert!(args.is_null()); // unwrap_or_default gives null
    }

    #[test]
    fn test_proto_tool_call_to_args_empty() {
        let tc = proto::ToolCall {
            id: "call-3".to_string(),
            name: "glob".to_string(),
            arguments: "{}".to_string(),
            result: None,
        };
        let (name, args) = proto_tool_call_to_args(&tc);
        assert_eq!(name, "glob");
        assert!(args.is_object());
    }

    // ========================================================================
    // Tool Result Conversions
    // ========================================================================

    #[test]
    fn test_internal_tool_result_to_proto_success() {
        let result = internal_tool_result_to_proto(true, "output data".to_string(), None);
        assert!(result.success);
        assert_eq!(result.output, "output data");
        assert_eq!(result.error, "");
    }

    #[test]
    fn test_internal_tool_result_to_proto_failure() {
        let result =
            internal_tool_result_to_proto(false, "".to_string(), Some("error msg".to_string()));
        assert!(!result.success);
        assert_eq!(result.error, "error msg");
    }

    // ========================================================================
    // Context Usage Conversions
    // ========================================================================

    #[test]
    fn test_context_usage_to_proto() {
        let usage = InternalContextUsage {
            used_tokens: 5000,
            max_tokens: 200000,
            percent: 0.025,
            turns: 10,
        };
        let proto_usage = internal_context_usage_to_proto(&usage);
        assert_eq!(proto_usage.total_tokens, 5000);
        assert_eq!(proto_usage.message_count, 10);
        assert_eq!(proto_usage.prompt_tokens, 0);
        assert_eq!(proto_usage.completion_tokens, 0);
    }

    #[test]
    fn test_context_usage_to_proto_zero() {
        let usage = InternalContextUsage::default();
        let proto_usage = internal_context_usage_to_proto(&usage);
        assert_eq!(proto_usage.total_tokens, 0);
        assert_eq!(proto_usage.message_count, 0);
    }

    // ========================================================================
    // LLM Config Conversions
    // ========================================================================

    #[test]
    fn test_llm_config_to_internal() {
        let proto_config = proto::LlmConfig {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            api_key: "sk-test-key".to_string(),
            base_url: "".to_string(),
            max_tokens: 0,
            temperature: 0.0,
        };
        let config = proto_llm_config_to_internal(&proto_config);
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_llm_config_to_internal_with_base_url() {
        let proto_config = proto::LlmConfig {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            api_key: "sk-key".to_string(),
            base_url: "https://custom.api.com".to_string(),
            max_tokens: 0,
            temperature: 0.0,
        };
        let config = proto_llm_config_to_internal(&proto_config);
        assert_eq!(config.provider, "openai");
        assert_eq!(config.model, "gpt-4");
    }

    // ========================================================================
    // Model Cost Conversions
    // ========================================================================

    #[test]
    fn test_model_cost_roundtrip() {
        let cost = ModelCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        };
        let proto_cost = internal_model_cost_to_proto(&cost);
        assert_eq!(proto_cost.input, 3.0);
        assert_eq!(proto_cost.output, 15.0);
        assert_eq!(proto_cost.cache_read, 0.3);
        assert_eq!(proto_cost.cache_write, 3.75);

        let back = proto_model_cost_to_internal(&proto_cost);
        assert_eq!(back.input, 3.0);
        assert_eq!(back.output, 15.0);
        assert_eq!(back.cache_read, 0.3);
        assert_eq!(back.cache_write, 3.75);
    }

    // ========================================================================
    // Model Limit Conversions
    // ========================================================================

    #[test]
    fn test_model_limit_roundtrip() {
        let limit = ModelLimit {
            context: 200000,
            output: 8192,
        };
        let proto_limit = internal_model_limit_to_proto(&limit);
        assert_eq!(proto_limit.context, 200000);
        assert_eq!(proto_limit.output, 8192);

        let back = proto_model_limit_to_internal(&proto_limit);
        assert_eq!(back.context, 200000);
        assert_eq!(back.output, 8192);
    }

    // ========================================================================
    // Model Modalities Conversions
    // ========================================================================

    #[test]
    fn test_model_modalities_roundtrip() {
        let modalities = ModelModalities {
            input: vec!["text".to_string(), "image".to_string()],
            output: vec!["text".to_string()],
        };
        let proto_mod = internal_model_modalities_to_proto(&modalities);
        assert_eq!(proto_mod.input, vec!["text", "image"]);
        assert_eq!(proto_mod.output, vec!["text"]);

        let back = proto_model_modalities_to_internal(&proto_mod);
        assert_eq!(back.input, vec!["text", "image"]);
        assert_eq!(back.output, vec!["text"]);
    }

    #[test]
    fn test_model_modalities_empty() {
        let modalities = ModelModalities {
            input: vec![],
            output: vec![],
        };
        let proto_mod = internal_model_modalities_to_proto(&modalities);
        let back = proto_model_modalities_to_internal(&proto_mod);
        assert!(back.input.is_empty());
        assert!(back.output.is_empty());
    }
}

#[cfg(test)]
mod extra_convert_tests4 {
    use super::*;
    use crate::config::{ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};

    // ========================================================================
    // ModelConfig Conversions
    // ========================================================================

    fn make_test_model_config() -> ModelConfig {
        ModelConfig {
            id: "claude-sonnet-4-20250514".to_string(),
            name: "Claude Sonnet".to_string(),
            family: "claude-sonnet".to_string(),
            api_key: Some("sk-test".to_string()),
            base_url: Some("https://api.anthropic.com".to_string()),
            attachment: true,
            reasoning: true,
            tool_call: true,
            temperature: true,
            release_date: Some("2025-05-14".to_string()),
            modalities: ModelModalities {
                input: vec!["text".to_string(), "image".to_string()],
                output: vec!["text".to_string()],
            },
            cost: ModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
            },
            limit: ModelLimit {
                context: 200000,
                output: 8192,
            },
        }
    }

    #[test]
    fn test_model_config_roundtrip() {
        let model = make_test_model_config();
        let proto_model = internal_model_config_to_proto(&model);
        assert_eq!(proto_model.id, "claude-sonnet-4-20250514");
        assert_eq!(proto_model.name, "Claude Sonnet");
        assert_eq!(proto_model.family, "claude-sonnet");
        assert_eq!(proto_model.api_key, Some("sk-test".to_string()));
        assert!(proto_model.attachment);
        assert!(proto_model.reasoning);
        assert!(proto_model.tool_call);
        assert!(proto_model.temperature);
        assert!(proto_model.cost.is_some());
        assert!(proto_model.limit.is_some());
        assert!(proto_model.modalities.is_some());

        let back = proto_model_info_to_internal(&proto_model);
        assert_eq!(back.id, "claude-sonnet-4-20250514");
        assert_eq!(back.name, "Claude Sonnet");
        assert_eq!(back.family, "claude-sonnet");
        assert_eq!(back.api_key, Some("sk-test".to_string()));
        assert!(back.attachment);
        assert!(back.reasoning);
        assert!(back.tool_call);
        assert_eq!(back.cost.input, 3.0);
        assert_eq!(back.limit.context, 200000);
    }

    #[test]
    fn test_model_config_no_optional_fields() {
        let proto_model = proto::ModelInfo {
            id: "gpt-4".to_string(),
            name: "GPT-4".to_string(),
            family: "gpt".to_string(),
            api_key: None,
            base_url: None,
            attachment: false,
            reasoning: false,
            tool_call: true,
            temperature: true,
            release_date: None,
            modalities: None,
            cost: None,
            limit: None,
        };
        let back = proto_model_info_to_internal(&proto_model);
        assert_eq!(back.id, "gpt-4");
        assert!(back.api_key.is_none());
        assert!(back.base_url.is_none());
        assert!(back.release_date.is_none());
        // Defaults when None
        assert!(back.modalities.input.is_empty());
        assert_eq!(back.cost.input, 0.0);
        assert_eq!(back.limit.context, 0);
    }

    // ========================================================================
    // ProviderConfig Conversions
    // ========================================================================

    #[test]
    fn test_provider_config_roundtrip() {
        let provider = ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("sk-provider-key".to_string()),
            base_url: Some("https://api.anthropic.com".to_string()),
            models: vec![make_test_model_config()],
        };
        let proto_provider = internal_provider_config_to_proto(&provider);
        assert_eq!(proto_provider.name, "anthropic");
        assert_eq!(proto_provider.api_key, Some("sk-provider-key".to_string()));
        assert_eq!(proto_provider.models.len(), 1);

        let back = proto_provider_info_to_internal(&proto_provider);
        assert_eq!(back.name, "anthropic");
        assert_eq!(back.api_key, Some("sk-provider-key".to_string()));
        assert_eq!(back.models.len(), 1);
        assert_eq!(back.models[0].id, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_provider_config_empty_models() {
        let provider = ProviderConfig {
            name: "openai".to_string(),
            api_key: None,
            base_url: None,
            models: vec![],
        };
        let proto_provider = internal_provider_config_to_proto(&provider);
        assert!(proto_provider.models.is_empty());
        assert!(proto_provider.api_key.is_none());

        let back = proto_provider_info_to_internal(&proto_provider);
        assert!(back.models.is_empty());
        assert!(back.api_key.is_none());
    }

    // ========================================================================
    // Event to GenerateChunk Conversions
    // ========================================================================

    #[test]
    fn test_event_text_delta_to_chunk() {
        let event = InternalAgentEvent::TextDelta {
            text: "Hello world".to_string(),
        };
        let chunk = internal_event_to_generate_chunk(event, "sess-1");
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.r#type, "content");
        assert_eq!(chunk.content, "Hello world");
        assert_eq!(chunk.session_id, "sess-1");
    }

    #[test]
    fn test_event_tool_start_to_chunk() {
        let event = InternalAgentEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
        };
        let chunk = internal_event_to_generate_chunk(event, "sess-1");
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.r#type, "tool_call");
        assert!(chunk.tool_call.is_some());
        let tc = chunk.tool_call.unwrap();
        assert_eq!(tc.id, "tool-1");
        assert_eq!(tc.name, "bash");
    }

    #[test]
    fn test_event_tool_end_to_chunk() {
        let event = InternalAgentEvent::ToolEnd {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            output: "file1.txt\nfile2.txt".to_string(),
            exit_code: 0,
        };
        let chunk = internal_event_to_generate_chunk(event, "sess-1");
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.r#type, "tool_result");
        assert!(chunk.tool_result.is_some());
        let tr = chunk.tool_result.unwrap();
        assert!(tr.success);
        assert_eq!(tr.output, "file1.txt\nfile2.txt");
    }

    #[test]
    fn test_event_tool_end_failure_to_chunk() {
        let event = InternalAgentEvent::ToolEnd {
            id: "tool-2".to_string(),
            name: "bash".to_string(),
            output: "command not found".to_string(),
            exit_code: 127,
        };
        let chunk = internal_event_to_generate_chunk(event, "sess-1");
        let chunk = chunk.unwrap();
        let tr = chunk.tool_result.unwrap();
        assert!(!tr.success); // exit_code != 0
    }

    #[test]
    fn test_event_end_to_chunk() {
        let event = InternalAgentEvent::End {
            text: "Done!".to_string(),
            usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
        };
        let chunk = internal_event_to_generate_chunk(event, "sess-1");
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.r#type, "done");
        assert_eq!(chunk.content, "Done!");
        assert_eq!(chunk.finish_reason, "stop");
    }

    #[test]
    fn test_event_turn_start_to_chunk_none() {
        let event = InternalAgentEvent::TurnStart { turn: 1 };
        let chunk = internal_event_to_generate_chunk(event, "sess-1");
        assert!(chunk.is_none());
    }

    #[test]
    fn test_event_turn_end_to_chunk_none() {
        let event = InternalAgentEvent::TurnEnd {
            turn: 1,
            usage: TokenUsage::default(),
        };
        let chunk = internal_event_to_generate_chunk(event, "sess-1");
        assert!(chunk.is_none());
    }
}

#[cfg(test)]
mod extra_convert_tests5 {
    use super::*;

    // ========================================================================
    // Event to Proto AgentEvent Conversions
    // ========================================================================

    #[test]
    fn test_event_start_to_proto() {
        let event = InternalAgentEvent::Start {
            prompt: "Hello".to_string(),
        };
        let proto_event = internal_event_to_proto_event(event, Some("sess-1"));
        assert!(proto_event.is_some());
        let pe = proto_event.unwrap();
        assert_eq!(
            pe.r#type,
            proto::agent_event::EventType::GenerationStarted as i32
        );
        assert_eq!(pe.session_id, Some("sess-1".to_string()));
        assert!(pe.message.contains("Hello"));
    }

    #[test]
    fn test_event_tool_start_to_proto() {
        let event = InternalAgentEvent::ToolStart {
            id: "t1".to_string(),
            name: "bash".to_string(),
        };
        let pe = internal_event_to_proto_event(event, Some("s1")).unwrap();
        assert_eq!(pe.r#type, proto::agent_event::EventType::ToolCalled as i32);
        assert!(pe.message.contains("bash"));
        assert_eq!(pe.data["tool_id"], "t1");
        assert_eq!(pe.data["tool_name"], "bash");
    }

    #[test]
    fn test_event_tool_end_to_proto() {
        let event = InternalAgentEvent::ToolEnd {
            id: "t1".to_string(),
            name: "bash".to_string(),
            output: "hello world".to_string(),
            exit_code: 0,
        };
        let pe = internal_event_to_proto_event(event, Some("s1")).unwrap();
        assert_eq!(
            pe.r#type,
            proto::agent_event::EventType::ToolCompleted as i32
        );
        assert!(pe.message.contains("bash"));
        assert_eq!(pe.data["exit_code"], "0");
        assert_eq!(pe.data["output_length"], "11");
    }

    #[test]
    fn test_event_end_to_proto() {
        let event = InternalAgentEvent::End {
            text: "Done".to_string(),
            usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
        };
        let pe = internal_event_to_proto_event(event, None).unwrap();
        assert_eq!(
            pe.r#type,
            proto::agent_event::EventType::GenerationCompleted as i32
        );
        assert!(pe.session_id.is_none());
        assert_eq!(pe.data["prompt_tokens"], "100");
        assert_eq!(pe.data["completion_tokens"], "50");
        assert_eq!(pe.data["total_tokens"], "150");
    }

    #[test]
    fn test_event_error_to_proto() {
        let event = InternalAgentEvent::Error {
            message: "Something broke".to_string(),
        };
        let pe = internal_event_to_proto_event(event, Some("s1")).unwrap();
        assert_eq!(pe.r#type, proto::agent_event::EventType::Error as i32);
        assert_eq!(pe.message, "Something broke");
    }

    #[test]
    fn test_event_confirmation_required_to_proto() {
        let event = InternalAgentEvent::ConfirmationRequired {
            tool_id: "t1".to_string(),
            tool_name: "bash".to_string(),
            args: serde_json::json!({"command": "rm -rf /"}),
            timeout_ms: 30000,
        };
        let pe = internal_event_to_proto_event(event, Some("s1")).unwrap();
        assert_eq!(
            pe.r#type,
            proto::agent_event::EventType::ConfirmationRequired as i32
        );
        assert!(pe.message.contains("bash"));
        assert_eq!(pe.data["tool_id"], "t1");
        assert_eq!(pe.data["timeout_ms"], "30000");
    }

    #[test]
    fn test_event_confirmation_received_to_proto() {
        let event = InternalAgentEvent::ConfirmationReceived {
            tool_id: "t1".to_string(),
            approved: true,
            reason: Some("Looks safe".to_string()),
        };
        let pe = internal_event_to_proto_event(event, Some("s1")).unwrap();
        assert_eq!(
            pe.r#type,
            proto::agent_event::EventType::ConfirmationReceived as i32
        );
        assert!(pe.message.contains("approved"));
        assert_eq!(pe.data["approved"], "true");
        assert_eq!(pe.data["reason"], "Looks safe");
    }

    #[test]
    fn test_event_confirmation_received_rejected() {
        let event = InternalAgentEvent::ConfirmationReceived {
            tool_id: "t1".to_string(),
            approved: false,
            reason: None,
        };
        let pe = internal_event_to_proto_event(event, Some("s1")).unwrap();
        assert!(pe.message.contains("rejected"));
        assert_eq!(pe.data["approved"], "false");
        assert_eq!(pe.data["reason"], "");
    }

    #[test]
    fn test_event_confirmation_timeout_to_proto() {
        let event = InternalAgentEvent::ConfirmationTimeout {
            tool_id: "t1".to_string(),
            action_taken: "rejected".to_string(),
        };
        let pe = internal_event_to_proto_event(event, Some("s1")).unwrap();
        assert_eq!(
            pe.r#type,
            proto::agent_event::EventType::ConfirmationTimeout as i32
        );
        assert!(pe.message.contains("rejected"));
        assert_eq!(pe.data["action_taken"], "rejected");
    }

    #[test]
    fn test_event_external_task_pending_to_proto() {
        let event = InternalAgentEvent::ExternalTaskPending {
            task_id: "task-1".to_string(),
            session_id: "sess-1".to_string(),
            lane: SessionLane::Execute,
            command_type: "bash".to_string(),
            payload: serde_json::json!({"cmd": "ls"}),
            timeout_ms: 60000,
        };
        let pe = internal_event_to_proto_event(event, None).unwrap();
        assert_eq!(
            pe.r#type,
            proto::agent_event::EventType::ExternalTaskPending as i32
        );
        assert_eq!(pe.session_id, Some("sess-1".to_string()));
        assert_eq!(pe.data["task_id"], "task-1");
        assert_eq!(pe.data["command_type"], "bash");
    }

    #[test]
    fn test_event_external_task_completed_to_proto() {
        let event = InternalAgentEvent::ExternalTaskCompleted {
            task_id: "task-1".to_string(),
            session_id: "sess-1".to_string(),
            success: true,
        };
        let pe = internal_event_to_proto_event(event, None).unwrap();
        assert_eq!(
            pe.r#type,
            proto::agent_event::EventType::ExternalTaskCompleted as i32
        );
        assert_eq!(pe.session_id, Some("sess-1".to_string()));
        assert_eq!(pe.data["success"], "true");
    }

    #[test]
    fn test_event_external_task_completed_failure() {
        let event = InternalAgentEvent::ExternalTaskCompleted {
            task_id: "task-2".to_string(),
            session_id: "sess-2".to_string(),
            success: false,
        };
        let pe = internal_event_to_proto_event(event, None).unwrap();
        assert!(pe.message.contains("failed"));
        assert_eq!(pe.data["success"], "false");
    }

    #[test]
    fn test_event_permission_denied_to_proto() {
        let event = InternalAgentEvent::PermissionDenied {
            tool_id: "t1".to_string(),
            tool_name: "bash".to_string(),
            args: serde_json::json!({"command": "rm -rf /"}),
            reason: "Dangerous command".to_string(),
        };
        let pe = internal_event_to_proto_event(event, Some("s1")).unwrap();
        assert_eq!(
            pe.r#type,
            proto::agent_event::EventType::PermissionDenied as i32
        );
        assert!(pe.message.contains("bash"));
        assert!(pe.message.contains("Dangerous command"));
        assert_eq!(pe.data["reason"], "Dangerous command");
    }

    #[test]
    fn test_event_context_compacted_to_proto() {
        let event = InternalAgentEvent::ContextCompacted {
            session_id: "sess-1".to_string(),
            before_messages: 100,
            after_messages: 20,
            percent_before: 0.85,
        };
        let pe = internal_event_to_proto_event(event, None).unwrap();
        assert_eq!(
            pe.r#type,
            proto::agent_event::EventType::ContextCompacted as i32
        );
        assert_eq!(pe.session_id, Some("sess-1".to_string()));
        assert!(pe.message.contains("100"));
        assert!(pe.message.contains("20"));
        assert_eq!(pe.data["before_messages"], "100");
        assert_eq!(pe.data["after_messages"], "20");
    }

    // ========================================================================
    // Message Conversions (additional edge cases)
    // ========================================================================

    #[test]
    fn test_proto_message_tool_role_maps_to_user() {
        let msg = proto::Message {
            role: "tool".to_string(),
            content: "result".to_string(),
            metadata: std::collections::HashMap::new(),
        };
        let internal = proto_message_to_internal(&msg);
        assert_eq!(internal.role, "user");
    }

    #[test]
    fn test_proto_message_system_role() {
        let msg = proto::Message {
            role: "system".to_string(),
            content: "You are helpful".to_string(),
            metadata: std::collections::HashMap::new(),
        };
        let internal = proto_message_to_internal(&msg);
        assert_eq!(internal.role, "system");
    }

    #[test]
    fn test_proto_message_assistant_role() {
        let msg = proto::Message {
            role: "ASSISTANT".to_string(),
            content: "Hi".to_string(),
            metadata: std::collections::HashMap::new(),
        };
        let internal = proto_message_to_internal(&msg);
        assert_eq!(internal.role, "assistant");
    }

    #[test]
    fn test_proto_message_unknown_role_defaults_user() {
        let msg = proto::Message {
            role: "unknown_role".to_string(),
            content: "test".to_string(),
            metadata: std::collections::HashMap::new(),
        };
        let internal = proto_message_to_internal(&msg);
        assert_eq!(internal.role, "user");
    }

    // ========================================================================
    // Conversation Message with ToolResult
    // ========================================================================

    #[test]
    fn test_conversation_message_with_tool_result() {
        let msg = InternalMessage {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tu-1".to_string(),
                content: "file contents here".to_string(),
                is_error: Some(false),
            }],
            reasoning_content: None,
        };
        let conv = internal_message_to_conversation_message(&msg, 5, 999);
        assert_eq!(conv.id, "msg_5");
        assert_eq!(conv.content.len(), 1);
        if let Some(proto::content_block::Block::ToolResult(tr)) = &conv.content[0].block {
            assert_eq!(tr.tool_use_id, "tu-1");
            assert_eq!(tr.content, "file contents here");
            assert!(!tr.is_error);
        } else {
            panic!("Expected ToolResult block");
        }
    }

    #[test]
    fn test_conversation_message_with_error_tool_result() {
        let msg = InternalMessage {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tu-2".to_string(),
                content: "error occurred".to_string(),
                is_error: Some(true),
            }],
            reasoning_content: None,
        };
        let conv = internal_message_to_conversation_message(&msg, 0, 0);
        if let Some(proto::content_block::Block::ToolResult(tr)) = &conv.content[0].block {
            assert!(tr.is_error);
        } else {
            panic!("Expected ToolResult block");
        }
    }

    #[test]
    fn test_conversation_message_tool_result_none_is_error() {
        let msg = InternalMessage {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tu-3".to_string(),
                content: "result".to_string(),
                is_error: None,
            }],
            reasoning_content: None,
        };
        let conv = internal_message_to_conversation_message(&msg, 0, 0);
        if let Some(proto::content_block::Block::ToolResult(tr)) = &conv.content[0].block {
            assert!(!tr.is_error); // None defaults to false
        } else {
            panic!("Expected ToolResult block");
        }
    }
}
