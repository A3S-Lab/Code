//! Task Types and Definitions

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Unique task identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(Uuid);

impl TaskId {
    /// Create a new random TaskId
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create a TaskId from a UUID string
    pub fn from_str(s: &str) -> Option<Self> {
        Uuid::parse_str(s).ok().map(Self)
    }

    /// Get the underlying UUID as a string
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Task type variants
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TaskType {
    /// Direct tool execution
    Tool {
        /// Tool name
        name: String,
        /// Tool arguments
        args: serde_json::Value,
    },
    /// Local agent execution (sub-agent)
    Agent {
        /// Agent configuration name or type
        agent_type: String,
        /// Workspace path
        workspace: String,
        /// Initial prompt
        prompt: String,
    },
    /// Remote agent execution
    RemoteAgent {
        /// Remote endpoint
        endpoint: String,
        /// Agent config
        config: serde_json::Value,
    },
    /// In-process teammate (worker within same process)
    InProcessTeammate {
        /// Teammate ID
        teammate_id: String,
        /// Task to execute
        task: Box<Task>,
    },
    /// DAG-based workflow execution
    Workflow {
        /// Workflow DAG as JSON
        dag: serde_json::Value,
    },
    /// Coordinator task - manages sub-tasks and aggregates results
    Coordinator {
        /// Strategy: "sequential", "parallel", or "hierarchical"
        strategy: String,
    },
    /// MCP monitor task
    MonitorMcp {
        /// MCP server config
        server_config: serde_json::Value,
    },
    /// Idle (memory consolidation) task
    Idle {
        /// Reason for idle
        reason: String,
    },
}

/// Task status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is queued, not yet started
    Pending,
    /// Task is currently executing
    Running,
    /// Task completed successfully
    Completed,
    /// Task failed with an error
    Failed,
    /// Task was killed by user/system
    Killed,
}

impl TaskStatus {
    /// Returns true if the status is terminal (will not transition further)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Killed
        )
    }
}

/// Base fields shared by all task states
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task identifier
    pub id: TaskId,
    /// Task type and configuration
    pub kind: TaskType,
    /// Current status
    pub status: TaskStatus,
    /// Human-readable description
    pub description: String,
    /// Associated tool use ID (if any)
    pub tool_use_id: Option<String>,
    /// When the task started
    pub start_time: std::time::SystemTime,
    /// When the task ended (if applicable)
    pub end_time: Option<std::time::SystemTime>,
    /// Total time paused in milliseconds
    pub total_paused_ms: u64,
    /// Output file path (for streaming output)
    pub output_file: Option<std::path::PathBuf>,
    /// Output offset for reading
    pub output_offset: u64,
    /// Whether completion notification was sent
    pub notified: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Parent task ID (if this task was spawned by another)
    pub parent_id: Option<TaskId>,
    /// Child task IDs (tasks spawned by this task)
    pub child_ids: Vec<TaskId>,
}

impl Task {
    /// Create a new Task with the given type
    pub fn new(kind: TaskType, description: impl Into<String>) -> Self {
        Self {
            id: TaskId::new(),
            kind,
            status: TaskStatus::Pending,
            description: description.into(),
            tool_use_id: None,
            start_time: std::time::SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: None,
            output_offset: 0,
            notified: false,
            error: None,
            parent_id: None,
            child_ids: Vec::new(),
        }
    }

    /// Create a tool task
    pub fn tool(name: impl Into<String>, args: serde_json::Value) -> Self {
        let name = name.into();
        Self::new(
            TaskType::Tool {
                name: name.clone(),
                args,
            },
            format!("Tool: {}", name),
        )
    }

    /// Create an agent task
    pub fn agent(
        agent_type: impl Into<String>,
        workspace: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        let agent_type = agent_type.into();
        Self::new(
            TaskType::Agent {
                agent_type: agent_type.clone(),
                workspace: workspace.into(),
                prompt: prompt.into(),
            },
            format!("Agent: {}", agent_type),
        )
    }

    /// Create an idle task
    pub fn idle(reason: impl Into<String>) -> Self {
        Self::new(
            TaskType::Idle {
                reason: reason.into(),
            },
            "Idle: Memory consolidation",
        )
    }

    /// Mark task as running
    pub fn start(&mut self) {
        self.status = TaskStatus::Running;
        self.start_time = std::time::SystemTime::now();
    }

    /// Mark task as completed
    pub fn complete(&mut self) {
        self.status = TaskStatus::Completed;
        self.end_time = Some(std::time::SystemTime::now());
    }

    /// Mark task as failed with error message
    pub fn fail(&mut self, error: impl Into<String>) {
        self.status = TaskStatus::Failed;
        self.error = Some(error.into());
        self.end_time = Some(std::time::SystemTime::now());
    }

    /// Mark task as killed
    pub fn kill(&mut self) {
        self.status = TaskStatus::Killed;
        self.end_time = Some(std::time::SystemTime::now());
    }

    /// Add a child task
    pub fn add_child(&mut self, child_id: TaskId) {
        self.child_ids.push(child_id);
    }

    /// Get task duration in milliseconds
    pub fn duration_ms(&self) -> Option<u64> {
        self.end_time.map(|end| {
            end.duration_since(self.start_time)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        })
    }

    /// Check if task is in a terminal state
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }
}

impl Default for TaskStatus {
    fn default() -> Self {
        TaskStatus::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_task_id() {
        let id1 = TaskId::new();
        let id2 = TaskId::new();
        assert_ne!(id1, id2);
        assert_eq!(id1.as_str(), id1.0.to_string());
    }

    #[test]
    fn test_task_lifecycle() {
        let mut task = Task::tool("read", json!({"file_path": "test.txt"}));

        assert_eq!(task.status, TaskStatus::Pending);

        task.start();
        assert_eq!(task.status, TaskStatus::Running);
        assert!(task.duration_ms().is_some());

        task.complete();
        assert!(task.is_terminal());
        assert!(task.error.is_none());

        let mut failed_task = Task::agent("general", "/workspace", "test");
        failed_task.start();
        failed_task.fail("Test error");
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(failed_task.error, Some("Test error".to_string()));
    }

    #[test]
    fn test_task_status_is_terminal() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Killed.is_terminal());
    }
}
