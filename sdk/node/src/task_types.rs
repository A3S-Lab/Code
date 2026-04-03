//! Task type bindings for Node.js SDK
//!
//! Exposes a3s-code-core task lifecycle types to Node.js via napi-rs.

use a3s_code_core::task::{
    Task as RustTask, TaskId as RustTaskId, TaskResult as RustTaskResult,
    TaskStatus as RustTaskStatus, TaskType as RustTaskType,
};

// ============================================================================
// TaskId
// ============================================================================

/// Unique task identifier (UUID string in JavaScript).
#[napi(object)]
#[derive(Clone)]
pub struct TaskId {
    /// The task ID as a string
    pub id: String,
}

impl From<RustTaskId> for TaskId {
    fn from(id: RustTaskId) -> Self {
        Self { id: id.as_str() }
    }
}

// ============================================================================
// TaskStatus
// ============================================================================

/// Task execution status.
#[napi(object)]
#[derive(Clone)]
pub struct TaskStatus {
    /// Status string: "pending", "running", "completed", "failed", "killed"
    pub status: String,
}

impl From<RustTaskStatus> for TaskStatus {
    fn from(status: RustTaskStatus) -> Self {
        Self {
            status: match status {
                RustTaskStatus::Pending => "pending".to_string(),
                RustTaskStatus::Running => "running".to_string(),
                RustTaskStatus::Completed => "completed".to_string(),
                RustTaskStatus::Failed => "failed".to_string(),
                RustTaskStatus::Killed => "killed".to_string(),
            },
        }
    }
}

// ============================================================================
// TaskType
// ============================================================================

/// Task type variants.
#[napi(object)]
#[derive(Clone)]
pub struct TaskType {
    /// Type string: "tool", "agent", "remote_agent", "in_process_teammate", "workflow", "coordinator", "monitor_mcp", "idle"
    pub type_: String,
    /// JSON-encoded data for the variant
    pub data: Option<String>,
}

impl From<RustTaskType> for TaskType {
    fn from(kind: RustTaskType) -> Self {
        match kind {
            RustTaskType::Tool { name, args } => Self {
                type_: "tool".to_string(),
                data: serde_json::to_string(&serde_json::json!({ "name": name, "args": args })).ok(),
            },
            RustTaskType::Agent {
                agent_type,
                workspace,
                prompt,
            } => Self {
                type_: "agent".to_string(),
                data: serde_json::to_string(
                    &serde_json::json!({ "agent_type": agent_type, "workspace": workspace, "prompt": prompt }),
                ).ok(),
            },
            RustTaskType::RemoteAgent { endpoint, config } => Self {
                type_: "remote_agent".to_string(),
                data: serde_json::to_string(&serde_json::json!({ "endpoint": endpoint, "config": config })).ok(),
            },
            RustTaskType::InProcessTeammate { teammate_id, task } => Self {
                type_: "in_process_teammate".to_string(),
                data: serde_json::to_string(&serde_json::json!({ "teammate_id": teammate_id, "task": *task })).ok(),
            },
            RustTaskType::Workflow { dag } => Self {
                type_: "workflow".to_string(),
                data: serde_json::to_string(&serde_json::json!({ "dag": dag })).ok(),
            },
            RustTaskType::Coordinator { strategy } => Self {
                type_: "coordinator".to_string(),
                data: serde_json::to_string(&serde_json::json!({ "strategy": strategy })).ok(),
            },
            RustTaskType::MonitorMcp { server_config } => Self {
                type_: "monitor_mcp".to_string(),
                data: serde_json::to_string(&serde_json::json!({ "server_config": server_config })).ok(),
            },
            RustTaskType::Idle { reason } => Self {
                type_: "idle".to_string(),
                data: serde_json::to_string(&serde_json::json!({ "reason": reason })).ok(),
            },
        }
    }
}

// ============================================================================
// Task
// ============================================================================

/// Base task with lifecycle management.
#[napi(object)]
#[derive(Clone)]
pub struct Task {
    pub id: String,
    pub kind: TaskType,
    pub status: TaskStatus,
    pub description: String,
    pub tool_use_id: Option<String>,
    pub parent_id: Option<String>,
    pub child_ids: Vec<String>,
    pub error: Option<String>,
}

impl From<RustTask> for Task {
    fn from(task: RustTask) -> Self {
        Self {
            id: task.id.as_str(),
            kind: TaskType::from(task.kind),
            status: TaskStatus::from(task.status),
            description: task.description,
            tool_use_id: task.tool_use_id,
            parent_id: task.parent_id.map(|id| id.as_str()),
            child_ids: task.child_ids.into_iter().map(|id| id.as_str()).collect(),
            error: task.error,
        }
    }
}

// ============================================================================
// TaskResult
// ============================================================================

/// Result of a completed task.
#[napi(object)]
#[derive(Clone)]
pub struct TaskResult {
    pub task_id: String,
    pub output: Option<String>,
    pub duration_ms: u32,
}

impl From<RustTaskResult> for TaskResult {
    fn from(result: RustTaskResult) -> Self {
        Self {
            task_id: result.task_id.as_str(),
            output: result.output.map(|v| serde_json::to_string(&v).unwrap_or_default()),
            duration_ms: result.duration_ms as u32,
        }
    }
}
