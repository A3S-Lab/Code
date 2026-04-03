//! Coordinator - Multi-agent Task Coordination
//!
//! Provides a coordinator pattern for managing sub-agents and aggregating results.
//! Built on top of TaskManager for hierarchical task lifecycle management.
//!
//! ## Coordinator Pattern
//!
//! ```text
//! Coordinator
//!   +-- spawns sub-tasks via TaskManager
//!   +-- waits for children completion
//!   +-- aggregates results
//! ```
//!
//! ## Example
//!
//! ```rust,ignore
//! use a3s_code_core::task::{coordinator::Coordinator, Task, TaskType};
//!
//! let task_manager = Arc::new(TaskManager::new());
//! let coordinator = Coordinator::new(Arc::clone(&task_manager));
//!
//! // Spawn multiple sub-tasks
//! coordinator.spawn_subtask(Task::agent("worker", "/workspace", "Task 1")).await;
//! coordinator.spawn_subtask(Task::agent("worker", "/workspace", "Task 2")).await;
//!
//! // Wait for all to complete
//! let results = coordinator.wait_all().await;
//! ```

use crate::task::{Task, TaskId, TaskManager, TaskResult, TaskType};
use std::sync::Arc;

/// Coordinator for multi-agent task orchestration.
///
/// A coordinator manages the lifecycle of sub-tasks, waits for their completion,
/// and aggregates results. It uses TaskManager for task lifecycle management.
#[derive(Clone)]
pub struct Coordinator {
    /// Task manager for task lifecycle
    task_manager: Arc<TaskManager>,
    /// Coordinator task ID (parent of all sub-tasks)
    coordinator_id: TaskId,
}

impl Coordinator {
    /// Create a new coordinator with its own coordinator task.
    pub fn new(task_manager: Arc<TaskManager>) -> Self {
        let task = Task::new(
            TaskType::Coordinator {
                strategy: "parallel".to_string(),
            },
            "Coordinator",
        );
        let coordinator_id = task_manager.spawn(task);
        let _ = task_manager.start(coordinator_id);

        Self {
            task_manager,
            coordinator_id,
        }
    }

    /// Create a new coordinator with a specific ID.
    pub fn with_id(task_manager: Arc<TaskManager>, coordinator_id: TaskId) -> Self {
        Self {
            task_manager,
            coordinator_id,
        }
    }

    /// Get the coordinator's task ID.
    pub fn id(&self) -> TaskId {
        self.coordinator_id
    }

    /// Spawn a sub-task as a child of this coordinator.
    ///
    /// The sub-task is automatically added as a child of the coordinator task.
    pub fn spawn_subtask(&self, task: Task) -> Result<TaskId, crate::error::CodeError> {
        self.task_manager
            .spawn_child(self.coordinator_id, task)
            .map_err(|e| crate::error::CodeError::Session(e.to_string()))
    }

    /// Spawn a sub-task with a specific parent ID (for nested coordinators).
    pub fn spawn_child_task(
        &self,
        parent_id: TaskId,
        task: Task,
    ) -> Result<TaskId, crate::error::CodeError> {
        self.task_manager
            .spawn_child(parent_id, task)
            .map_err(|e| crate::error::CodeError::Session(e.to_string()))
    }

    /// Start a task (mark as running).
    pub fn start(&self, task_id: TaskId) -> Result<(), crate::error::CodeError> {
        self.task_manager
            .start(task_id)
            .map_err(|e| crate::error::CodeError::Session(e.to_string()))
    }

    /// Complete a task with a result.
    pub fn complete(
        &self,
        task_id: TaskId,
        output: Option<serde_json::Value>,
    ) -> Result<(), crate::error::CodeError> {
        self.task_manager
            .complete(task_id, output)
            .map_err(|e| crate::error::CodeError::Session(e.to_string()))
    }

    /// Wait for all sub-tasks (direct children) to complete.
    pub async fn wait_subtasks(&self) -> Result<Vec<TaskResult>, crate::error::CodeError> {
        self.task_manager
            .wait_children(self.coordinator_id)
            .await
            .map_err(|e| crate::error::CodeError::Session(e.to_string()))
    }

    /// Wait for all children of a specific task to complete.
    pub async fn wait_children(
        &self,
        parent_id: TaskId,
    ) -> Result<Vec<TaskResult>, crate::error::CodeError> {
        self.task_manager
            .wait_children(parent_id)
            .await
            .map_err(|e| crate::error::CodeError::Session(e.to_string()))
    }

    /// Wait for a specific task to complete.
    pub async fn wait(&self, task_id: TaskId) -> Result<TaskResult, crate::error::CodeError> {
        self.task_manager
            .wait(task_id)
            .await
            .map_err(|e| crate::error::CodeError::Session(e.to_string()))
    }

    /// Get all sub-task IDs.
    pub fn get_subtask_ids(&self) -> Result<Vec<TaskId>, crate::error::CodeError> {
        self.task_manager
            .get_children(self.coordinator_id)
            .map_err(|e| crate::error::CodeError::Session(e.to_string()))
    }

    /// Check if all sub-tasks are complete.
    pub fn all_subtasks_complete(&self) -> bool {
        self.task_manager.all_children_complete(self.coordinator_id)
    }

    /// Get the task manager reference.
    pub fn task_manager(&self) -> &Arc<TaskManager> {
        &self.task_manager
    }

    /// Complete the coordinator task with aggregated results.
    pub fn finish(&self, results: Vec<TaskResult>) -> Result<(), crate::error::CodeError> {
        let aggregated = serde_json::json!({
            "coordinator_id": self.coordinator_id.as_str(),
            "subtask_count": results.len(),
            "results": results,
        });
        self.task_manager
            .complete(self.coordinator_id, Some(aggregated))
            .map_err(|e| crate::error::CodeError::Session(e.to_string()))
    }

    /// Fail the coordinator task with an error.
    pub fn fail(&self, error: impl Into<String>) -> Result<(), crate::error::CodeError> {
        self.task_manager
            .fail(self.coordinator_id, error)
            .map_err(|e| crate::error::CodeError::Session(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_coordinator_spawn_and_wait() {
        let tm = Arc::new(TaskManager::new());
        let coord = Coordinator::new(Arc::clone(&tm));

        // Spawn two sub-tasks
        let task1 = Task::tool("read", json!({"file_path": "test1.txt"}));
        let task2 = Task::tool("read", json!({"file_path": "test2.txt"}));

        let id1 = coord.spawn_subtask(task1).unwrap();
        let id2 = coord.spawn_subtask(task2).unwrap();

        // Start and complete them
        coord.start(id1).unwrap();
        coord
            .complete(id1, Some(json!({"output": "content1"})))
            .unwrap();

        coord.start(id2).unwrap();
        coord
            .complete(id2, Some(json!({"output": "content2"})))
            .unwrap();

        // Wait for all
        let results = coord.wait_subtasks().await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(coord.all_subtasks_complete());
    }

    #[tokio::test]
    async fn test_coordinator_finish() {
        let tm = Arc::new(TaskManager::new());
        let coord = Coordinator::new(Arc::clone(&tm));

        let task = Task::tool("read", json!({"file_path": "test.txt"}));
        let id = coord.spawn_subtask(task).unwrap();
        coord.start(id).unwrap();
        coord.complete(id, Some(json!({"output": "test"}))).unwrap();

        let results = coord.wait_subtasks().await.unwrap();
        coord.finish(results).unwrap();

        // Coordinator itself should be complete now
        assert!(tm.is_terminal(coord.id()));
    }
}
