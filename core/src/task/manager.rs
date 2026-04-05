//! Task Manager - Centralized Task Lifecycle Management
//!
//! Provides centralized management for all tasks in a3s-code.
//! Handles spawning, tracking, and coordinating task execution.
//!
//! ## Example
//!
//! ```rust,ignore
//! use a3s_code_core::task::{TaskManager, Task, TaskType};
//!
//! let manager = TaskManager::new();
//!
//! // Spawn a task
//! let task = Task::agent("general", "/workspace", "Analyze this");
//! let task_id = manager.spawn(task);
//!
//! // Wait for completion
//! let result = manager.wait(task_id).await;
//! assert!(result.is_ok());
//!
//! // Subscribe to updates
//! let mut rx = manager.subscribe(task_id).unwrap();
//! while let Some(event) = rx.recv().await {
//!     println!("Task update: {:?}", event);
//! }
//! ```

use crate::task::{Task, TaskId};
use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;
use tokio::sync::broadcast;

/// Task lifecycle events
#[derive(Debug, Clone)]
pub enum TaskEvent {
    /// Task was spawned
    Spawned {
        task_id: TaskId,
        parent_id: Option<TaskId>,
    },
    /// Task started execution
    Started(TaskId),
    /// Task progress update
    Progress { task_id: TaskId, message: String },
    /// Task completed
    Completed { task_id: TaskId, result: TaskResult },
    /// Task failed
    Failed { task_id: TaskId, error: String },
    /// Task was killed
    Killed(TaskId),
    /// Child task spawned
    ChildSpawned { parent_id: TaskId, child_id: TaskId },
}

/// Task execution result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskResult {
    /// Task ID
    pub task_id: TaskId,
    /// Whether the task completed successfully
    pub success: bool,
    /// Output value (tool result, agent response, etc.)
    pub output: Option<serde_json::Value>,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// Task manager errors
#[derive(Debug, thiserror::Error)]
pub enum TaskManagerError {
    #[error("Task not found: {0}")]
    TaskNotFound(TaskId),

    #[error("Task already completed: {0}")]
    TaskAlreadyCompleted(TaskId),

    #[error("Task {0} is still running")]
    TaskStillRunning(TaskId),

    #[error("Manager is shutdown")]
    Shutdown,

    #[error("Send error: {0}")]
    SendError(String),
}

/// Centralized task lifecycle manager
pub struct TaskManager {
    /// All tasks indexed by ID
    tasks: RwLock<HashMap<TaskId, Task>>,
    /// Task results (kept after completion for retrieval)
    results: RwLock<HashMap<TaskId, TaskResult>>,
    /// Event subscribers (one per task)
    subscribers: RwLock<HashMap<TaskId, broadcast::Sender<TaskEvent>>>,
    /// Global event broadcast
    global_events: broadcast::Sender<TaskEvent>,
    /// Task queue for pending tasks
    pending_queue: RwLock<VecDeque<TaskId>>,
    /// Whether manager is shutdown
    shutdown: RwLock<bool>,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskManager {
    /// Create a new task manager
    pub fn new() -> Self {
        let (global_events, _) = broadcast::channel(1024);
        Self {
            tasks: RwLock::new(HashMap::new()),
            results: RwLock::new(HashMap::new()),
            subscribers: RwLock::new(HashMap::new()),
            global_events,
            pending_queue: RwLock::new(VecDeque::new()),
            shutdown: RwLock::new(false),
        }
    }

    /// Spawn a new task
    ///
    /// Returns the task ID.
    pub fn spawn(&self, task: Task) -> TaskId {
        if *self.shutdown.read().unwrap() {
            // Return a dummy ID, caller should check
            return task.id;
        }

        let task_id = task.id;
        let parent_id = task.parent_id;

        // Create subscription channel for this task
        let (tx, _) = broadcast::channel(64);
        self.subscribers.write().unwrap().insert(task_id, tx);

        // Add to tasks
        self.tasks.write().unwrap().insert(task_id, task);

        // Add to pending queue
        self.pending_queue.write().unwrap().push_back(task_id);

        // Emit spawn event
        let _ = self
            .global_events
            .send(TaskEvent::Spawned { task_id, parent_id });

        task_id
    }

    /// Start executing a task
    pub fn start(&self, task_id: TaskId) -> Result<(), TaskManagerError> {
        let mut tasks = self.tasks.write().unwrap();

        let task = tasks
            .get_mut(&task_id)
            .ok_or(TaskManagerError::TaskNotFound(task_id))?;

        if task.status.is_terminal() {
            return Err(TaskManagerError::TaskAlreadyCompleted(task_id));
        }

        task.start();

        // Notify subscribers
        if let Some(tx) = self.subscribers.read().unwrap().get(&task_id) {
            let _ = tx.send(TaskEvent::Started(task_id));
        }

        Ok(())
    }

    /// Update task progress
    pub fn progress(
        &self,
        task_id: TaskId,
        message: impl Into<String>,
    ) -> Result<(), TaskManagerError> {
        let tasks = self.tasks.read().unwrap();

        if !tasks.contains_key(&task_id) {
            return Err(TaskManagerError::TaskNotFound(task_id));
        }

        // Notify subscribers
        if let Some(tx) = self.subscribers.read().unwrap().get(&task_id) {
            let _ = tx.send(TaskEvent::Progress {
                task_id,
                message: message.into(),
            });
        }

        Ok(())
    }

    /// Complete a task with result
    pub fn complete(
        &self,
        task_id: TaskId,
        output: Option<serde_json::Value>,
    ) -> Result<(), TaskManagerError> {
        let mut tasks = self.tasks.write().unwrap();

        let task = tasks
            .get_mut(&task_id)
            .ok_or(TaskManagerError::TaskNotFound(task_id))?;

        if task.status.is_terminal() {
            return Err(TaskManagerError::TaskAlreadyCompleted(task_id));
        }

        let duration_ms = task.duration_ms().unwrap_or(0);
        task.complete();

        // Store result
        let result = TaskResult {
            task_id,
            success: true,
            output,
            duration_ms,
        };
        self.results
            .write()
            .unwrap()
            .insert(task_id, result.clone());

        // Notify subscribers
        if let Some(tx) = self.subscribers.read().unwrap().get(&task_id) {
            let _ = tx.send(TaskEvent::Completed { task_id, result });
        }

        Ok(())
    }

    /// Fail a task with error
    pub fn fail(&self, task_id: TaskId, error: impl Into<String>) -> Result<(), TaskManagerError> {
        let mut tasks = self.tasks.write().unwrap();

        let task = tasks
            .get_mut(&task_id)
            .ok_or(TaskManagerError::TaskNotFound(task_id))?;

        if task.status.is_terminal() {
            return Err(TaskManagerError::TaskAlreadyCompleted(task_id));
        }

        let error_msg = error.into();
        task.fail(&error_msg);

        // Store error result so wait() can retrieve it even if called after fail
        let result = TaskResult {
            task_id,
            success: false,
            output: Some(serde_json::json!({ "error": error_msg.clone() })),
            duration_ms: task.duration_ms().unwrap_or(0),
        };
        self.results.write().unwrap().insert(task_id, result);

        // Notify subscribers
        if let Some(tx) = self.subscribers.read().unwrap().get(&task_id) {
            let _ = tx.send(TaskEvent::Failed {
                task_id,
                error: error_msg,
            });
        }

        Ok(())
    }

    /// Kill a task
    pub fn kill(&self, task_id: TaskId) -> Result<(), TaskManagerError> {
        let mut tasks = self.tasks.write().unwrap();

        let task = tasks
            .get_mut(&task_id)
            .ok_or(TaskManagerError::TaskNotFound(task_id))?;

        if task.status.is_terminal() {
            return Err(TaskManagerError::TaskAlreadyCompleted(task_id));
        }

        task.kill();

        // Store killed result so wait() can retrieve it even if called after kill
        let result = TaskResult {
            task_id,
            success: false,
            output: Some(serde_json::json!({ "error": "Task was killed" })),
            duration_ms: task.duration_ms().unwrap_or(0),
        };
        self.results.write().unwrap().insert(task_id, result);

        // Notify subscribers
        if let Some(tx) = self.subscribers.read().unwrap().get(&task_id) {
            let _ = tx.send(TaskEvent::Killed(task_id));
        }

        Ok(())
    }

    /// Get task by ID
    pub fn get(&self, task_id: TaskId) -> Option<Task> {
        self.tasks.read().unwrap().get(&task_id).cloned()
    }

    /// Get task result
    pub fn get_result(&self, task_id: TaskId) -> Option<TaskResult> {
        self.results.read().unwrap().get(&task_id).cloned()
    }

    /// Check if task is terminal
    pub fn is_terminal(&self, task_id: TaskId) -> bool {
        self.tasks
            .read()
            .unwrap()
            .get(&task_id)
            .map(|t| t.is_terminal())
            .unwrap_or(true)
    }

    /// Subscribe to task events
    ///
    /// Returns a receiver that will receive all events for the task.
    /// The receiver is closed when the task completes.
    pub fn subscribe(&self, task_id: TaskId) -> Option<broadcast::Receiver<TaskEvent>> {
        self.subscribers
            .read()
            .unwrap()
            .get(&task_id)
            .map(|tx| tx.subscribe())
    }

    /// Subscribe to all task events (global)
    pub fn subscribe_all(&self) -> broadcast::Receiver<TaskEvent> {
        self.global_events.subscribe()
    }

    /// Wait for a task to complete
    ///
    /// Returns the task result when the task completes.
    pub async fn wait(&self, task_id: TaskId) -> Result<TaskResult, TaskManagerError> {
        // First check if already completed
        if let Some(result) = self.get_result(task_id) {
            if !result.success {
                let error = result
                    .output
                    .as_ref()
                    .and_then(|v| v.get("error"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                return Err(TaskManagerError::SendError(error));
            }
            return Ok(result);
        }

        // Subscribe to events
        let mut rx = self
            .subscribe(task_id)
            .ok_or(TaskManagerError::TaskNotFound(task_id))?;

        // Wait for completion
        while let Ok(event) = rx.recv().await {
            match event {
                TaskEvent::Completed {
                    task_id: id,
                    result,
                } if id == task_id => {
                    return Ok(result);
                }
                TaskEvent::Failed { task_id: id, error } if id == task_id => {
                    return Err(TaskManagerError::SendError(error));
                }
                TaskEvent::Killed(id) if id == task_id => {
                    return Err(TaskManagerError::SendError("Task was killed".to_string()));
                }
                _ => {}
            }
        }

        Err(TaskManagerError::Shutdown)
    }

    /// Add a child task to a parent
    pub fn add_child(&self, parent_id: TaskId, child_id: TaskId) -> Result<(), TaskManagerError> {
        let mut tasks = self.tasks.write().unwrap();

        let parent = tasks
            .get_mut(&parent_id)
            .ok_or(TaskManagerError::TaskNotFound(parent_id))?;
        parent.add_child(child_id);

        // Notify about child spawn
        if let Some(tx) = self.subscribers.read().unwrap().get(&parent_id) {
            let _ = tx.send(TaskEvent::ChildSpawned {
                parent_id,
                child_id,
            });
        }

        Ok(())
    }

    /// Spawn a child task with the given parent ID, setting up parent-child relationship.
    ///
    /// This is a convenience method that combines spawn + add_child in one call.
    pub fn spawn_child(&self, parent_id: TaskId, task: Task) -> Result<TaskId, TaskManagerError> {
        // Verify parent exists
        if !self.tasks.read().unwrap().contains_key(&parent_id) {
            return Err(TaskManagerError::TaskNotFound(parent_id));
        }

        let child_id = self.spawn(task);
        self.add_child(parent_id, child_id)?;
        Ok(child_id)
    }

    /// Wait for all child tasks of a parent to complete.
    ///
    /// Returns a list of results for each child task.
    pub async fn wait_children(
        &self,
        parent_id: TaskId,
    ) -> Result<Vec<TaskResult>, TaskManagerError> {
        let children: Vec<TaskId> = {
            let tasks = self.tasks.read().unwrap();
            let parent = tasks
                .get(&parent_id)
                .ok_or(TaskManagerError::TaskNotFound(parent_id))?;
            parent.child_ids.clone()
        };

        let mut results = Vec::new();
        for child_id in children {
            match self.wait(child_id).await {
                Ok(result) => results.push(result),
                Err(TaskManagerError::TaskStillRunning(_)) => {
                    // Child still running, wait for it
                    let result = self.wait(child_id).await?;
                    results.push(result);
                }
                Err(e) => return Err(e),
            }
        }

        Ok(results)
    }

    /// Get all child task IDs for a parent task.
    pub fn get_children(&self, parent_id: TaskId) -> Result<Vec<TaskId>, TaskManagerError> {
        let tasks = self.tasks.read().unwrap();
        let parent = tasks
            .get(&parent_id)
            .ok_or(TaskManagerError::TaskNotFound(parent_id))?;
        Ok(parent.child_ids.clone())
    }

    /// Check if all children of a parent task are complete.
    pub fn all_children_complete(&self, parent_id: TaskId) -> bool {
        if let Some(children) = self
            .tasks
            .read()
            .unwrap()
            .get(&parent_id)
            .map(|t| &t.child_ids)
        {
            children.iter().all(|id| self.is_terminal(*id))
        } else {
            true
        }
    }

    /// Get pending task IDs (FIFO)
    pub fn pending_tasks(&self) -> Vec<TaskId> {
        self.pending_queue.read().unwrap().iter().copied().collect()
    }

    /// Pop the next pending task
    pub fn pop_pending(&self) -> Option<TaskId> {
        self.pending_queue.write().unwrap().pop_front()
    }

    /// Shutdown the manager
    pub fn shutdown(&self) {
        *self.shutdown.write().unwrap() = true;
        self.tasks.write().unwrap().clear();
        self.results.write().unwrap().clear();
        self.subscribers.write().unwrap().clear();
        self.pending_queue.write().unwrap().clear();
    }

    /// Get all tasks
    pub fn all_tasks(&self) -> HashMap<TaskId, Task> {
        self.tasks.read().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskStatus;
    use serde_json::json;

    #[tokio::test]
    async fn test_task_manager_spawn_and_wait() {
        let manager = TaskManager::new();

        let task = Task::tool("read", json!({"file_path": "test.txt"}));
        let task_id = manager.spawn(task);

        manager.start(task_id).unwrap();

        let result_json = json!({"success": true, "output": "file content"});
        manager
            .complete(task_id, Some(result_json.clone()))
            .unwrap();

        let result = manager.wait(task_id).await.unwrap();
        assert_eq!(result.output, Some(result_json));
        assert!(manager.is_terminal(task_id));
    }

    #[tokio::test]
    async fn test_task_manager_fail() {
        let manager = TaskManager::new();

        let task = Task::tool("read", json!({"file_path": "nonexistent.txt"}));
        let task_id = manager.spawn(task);

        manager.start(task_id).unwrap();
        manager.fail(task_id, "File not found").unwrap();

        let result = manager.wait(task_id).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_task_manager_kill() {
        let manager = TaskManager::new();

        let task = Task::tool("bash", json!({"command": "sleep 100"}));
        let task_id = manager.spawn(task);

        manager.start(task_id).unwrap();
        manager.kill(task_id).unwrap();

        assert!(manager.is_terminal(task_id));
        assert_eq!(manager.get(task_id).unwrap().status, TaskStatus::Killed);
    }
}
