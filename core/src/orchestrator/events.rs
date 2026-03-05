//! Orchestrator 事件定义

use crate::agent::AgentEvent;
use crate::llm::TokenUsage;
use crate::orchestrator::{ControlSignal, SubAgentConfig, SubAgentState};
use crate::planning::ExecutionPlan;
use serde::{Deserialize, Serialize};

/// Orchestrator 事件 - 统一的事件类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum OrchestratorEvent {
    /// SubAgent 启动
    SubAgentStarted {
        id: String,
        agent_type: String,
        description: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        config: SubAgentConfig,
    },

    /// SubAgent 完成
    SubAgentCompleted {
        id: String,
        success: bool,
        output: String,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        token_usage: Option<TokenUsage>,
    },

    /// SubAgent 状态变更
    SubAgentStateChanged {
        id: String,
        old_state: SubAgentState,
        new_state: SubAgentState,
    },

    /// SubAgent 进度更新
    SubAgentProgress {
        id: String,
        step: usize,
        total_steps: usize,
        message: String,
    },

    /// SubAgent 内部事件（来自 AgentLoop）
    SubAgentInternalEvent {
        id: String,
        #[serde(flatten)]
        event: AgentEvent,
    },

    /// 规划开始
    PlanningStarted { id: String, goal: String },

    /// 规划完成
    PlanningCompleted { id: String, plan: ExecutionPlan },

    /// 工具执行开始
    ToolExecutionStarted {
        id: String,
        tool_id: String,
        tool_name: String,
        args: serde_json::Value,
    },

    /// 工具执行完成
    ToolExecutionCompleted {
        id: String,
        tool_id: String,
        tool_name: String,
        result: String,
        exit_code: i32,
        duration_ms: u64,
    },

    /// 控制信号接收
    ControlSignalReceived { id: String, signal: ControlSignal },

    /// 控制信号应用
    ControlSignalApplied {
        id: String,
        signal: ControlSignal,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

impl OrchestratorEvent {
    /// 获取 SubAgent ID
    pub fn subagent_id(&self) -> Option<&str> {
        match self {
            OrchestratorEvent::SubAgentStarted { id, .. }
            | OrchestratorEvent::SubAgentCompleted { id, .. }
            | OrchestratorEvent::SubAgentStateChanged { id, .. }
            | OrchestratorEvent::SubAgentProgress { id, .. }
            | OrchestratorEvent::SubAgentInternalEvent { id, .. }
            | OrchestratorEvent::PlanningStarted { id, .. }
            | OrchestratorEvent::PlanningCompleted { id, .. }
            | OrchestratorEvent::ToolExecutionStarted { id, .. }
            | OrchestratorEvent::ToolExecutionCompleted { id, .. }
            | OrchestratorEvent::ControlSignalReceived { id, .. }
            | OrchestratorEvent::ControlSignalApplied { id, .. } => Some(id),
        }
    }

    /// 获取事件类型名称
    pub fn event_name(&self) -> &'static str {
        match self {
            OrchestratorEvent::SubAgentStarted { .. } => "subagent_started",
            OrchestratorEvent::SubAgentCompleted { .. } => "subagent_completed",
            OrchestratorEvent::SubAgentStateChanged { .. } => "subagent_state_changed",
            OrchestratorEvent::SubAgentProgress { .. } => "subagent_progress",
            OrchestratorEvent::SubAgentInternalEvent { .. } => "subagent_internal_event",
            OrchestratorEvent::PlanningStarted { .. } => "planning_started",
            OrchestratorEvent::PlanningCompleted { .. } => "planning_completed",
            OrchestratorEvent::ToolExecutionStarted { .. } => "tool_execution_started",
            OrchestratorEvent::ToolExecutionCompleted { .. } => "tool_execution_completed",
            OrchestratorEvent::ControlSignalReceived { .. } => "control_signal_received",
            OrchestratorEvent::ControlSignalApplied { .. } => "control_signal_applied",
        }
    }
}

/// SubAgent 事件 Payload（用于 a3s-event）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentEventPayload {
    pub subagent_id: String,
    pub event: OrchestratorEvent,
    pub timestamp: i64,
}
