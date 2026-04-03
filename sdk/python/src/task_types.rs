//! Task type bindings for Python SDK
//!
//! Exposes a3s-code-core task lifecycle types to Python.

use a3s_code_core::task::{
    Task as RustTask, TaskId as RustTaskId, TaskResult as RustTaskResult,
    TaskStatus as RustTaskStatus, TaskType as RustTaskType,
};
use pyo3::prelude::*;

// ============================================================================
// TaskId
// ============================================================================

/// Unique task identifier (8-char hex string in Python).
#[pyclass(name = "TaskId")]
#[derive(Clone)]
pub struct PyTaskId {
    id: String,
}

#[pymethods]
impl PyTaskId {
    #[new]
    fn new(id: String) -> Self {
        Self { id }
    }

    fn __repr__(&self) -> String {
        format!("TaskId('{}')", self.id)
    }

    fn __str__(&self) -> String {
        self.id.clone()
    }

    fn __eq__(&self, other: &PyTaskId) -> bool {
        self.id == other.id
    }

    fn __hash__(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut s = DefaultHasher::new();
        self.id.hash(&mut s);
        s.finish()
    }
}

impl From<RustTaskId> for PyTaskId {
    fn from(id: RustTaskId) -> Self {
        Self { id: id.as_str() }
    }
}

// ============================================================================
// TaskStatus
// ============================================================================

/// Task execution status.
#[pyclass(name = "TaskStatus")]
#[derive(Clone)]
pub struct PyTaskStatus {
    status: String,
}

#[pymethods]
impl PyTaskStatus {
    fn __repr__(&self) -> String {
        format!("TaskStatus('{}')", self.status)
    }

    fn __str__(&self) -> String {
        self.status.clone()
    }

    fn __eq__(&self, other: &PyTaskStatus) -> bool {
        self.status == other.status
    }
}

impl From<RustTaskStatus> for PyTaskStatus {
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
#[pyclass(name = "TaskType")]
#[derive(Clone)]
pub struct PyTaskType {
    type_: String,
    data: Option<String>,
}

#[pymethods]
impl PyTaskType {
    fn __repr__(&self) -> String {
        format!("TaskType('{}', data={:?})", self.type_, self.data)
    }

    fn __str__(&self) -> String {
        self.type_.clone()
    }
}

impl From<RustTaskType> for PyTaskType {
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
#[pyclass(name = "Task")]
#[derive(Clone)]
pub struct PyTask {
    #[pyo3(get)]
    id: String,
    #[pyo3(get)]
    kind: PyTaskType,
    #[pyo3(get)]
    status: PyTaskStatus,
    #[pyo3(get)]
    description: String,
    #[pyo3(get)]
    tool_use_id: Option<String>,
    #[pyo3(get)]
    parent_id: Option<String>,
    #[pyo3(get)]
    child_ids: Vec<String>,
    #[pyo3(get)]
    error: Option<String>,
}

#[pymethods]
impl PyTask {
    fn __repr__(&self) -> String {
        format!(
            "Task(id='{}', type={}, status={}, description={:?})",
            self.id,
            self.kind.type_,
            self.status.status,
            if self.description.len() > 40 {
                format!("{}...", &self.description[..40])
            } else {
                self.description.clone()
            }
        )
    }
}

impl From<RustTask> for PyTask {
    fn from(task: RustTask) -> Self {
        Self {
            id: task.id.as_str(),
            kind: PyTaskType::from(task.kind),
            status: PyTaskStatus::from(task.status),
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
#[pyclass(name = "TaskResult")]
#[derive(Clone)]
pub struct PyTaskResult {
    #[pyo3(get)]
    task_id: String,
    #[pyo3(get)]
    output: Option<String>,
    #[pyo3(get)]
    duration_ms: u64,
}

#[pymethods]
impl PyTaskResult {
    fn __repr__(&self) -> String {
        format!(
            "TaskResult(task_id='{}', duration_ms={})",
            self.task_id, self.duration_ms
        )
    }

    fn __str__(&self) -> String {
        format!("TaskResult(task_id='{}', output={:?})", self.task_id, self.output)
    }
}

impl From<RustTaskResult> for PyTaskResult {
    fn from(result: RustTaskResult) -> Self {
        Self {
            task_id: result.task_id.as_str(),
            output: result.output.map(|v| serde_json::to_string(&v).unwrap_or_default()),
            duration_ms: result.duration_ms,
        }
    }
}
