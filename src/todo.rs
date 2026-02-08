//! Task/Todo tracking system for sessions
//!
//! Provides per-session task tracking capabilities for LLM agents to manage
//! multi-step coding tasks. Each session has its own independent todo list.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Task status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Task is waiting to be started
    #[default]
    Pending,
    /// Task is currently being worked on
    InProgress,
    /// Task has been completed successfully
    Completed,
    /// Task was cancelled and will not be done
    Cancelled,
}

impl fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TodoStatus::Pending => write!(f, "pending"),
            TodoStatus::InProgress => write!(f, "in_progress"),
            TodoStatus::Completed => write!(f, "completed"),
            TodoStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl FromStr for TodoStatus {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "pending" => TodoStatus::Pending,
            "in_progress" | "inprogress" => TodoStatus::InProgress,
            "completed" | "done" => TodoStatus::Completed,
            "cancelled" | "canceled" => TodoStatus::Cancelled,
            _ => TodoStatus::Pending,
        })
    }
}

impl TodoStatus {
    /// Check if task is still active (not completed or cancelled)
    pub fn is_active(&self) -> bool {
        matches!(self, TodoStatus::Pending | TodoStatus::InProgress)
    }
}

/// Task priority level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    /// High priority - should be done first
    High,
    /// Medium priority - normal importance
    #[default]
    Medium,
    /// Low priority - can be deferred
    Low,
}

impl fmt::Display for TodoPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TodoPriority::High => write!(f, "high"),
            TodoPriority::Medium => write!(f, "medium"),
            TodoPriority::Low => write!(f, "low"),
        }
    }
}

impl FromStr for TodoPriority {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "high" | "h" | "1" => TodoPriority::High,
            "medium" | "med" | "m" | "2" => TodoPriority::Medium,
            "low" | "l" | "3" => TodoPriority::Low,
            _ => TodoPriority::Medium,
        })
    }
}

/// A task/todo item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    /// Unique identifier for the todo
    pub id: String,
    /// Brief description of the task
    pub content: String,
    /// Current status of the task
    pub status: TodoStatus,
    /// Priority level
    pub priority: TodoPriority,
}

impl Todo {
    /// Create a new todo with pending status
    pub fn new(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            status: TodoStatus::Pending,
            priority: TodoPriority::Medium,
        }
    }

    /// Create a new todo with specified priority
    pub fn with_priority(mut self, priority: TodoPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Create a new todo with specified status
    pub fn with_status(mut self, status: TodoStatus) -> Self {
        self.status = status;
        self
    }

    /// Check if task is still active
    pub fn is_active(&self) -> bool {
        self.status.is_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_todo_status_display() {
        assert_eq!(TodoStatus::Pending.to_string(), "pending");
        assert_eq!(TodoStatus::InProgress.to_string(), "in_progress");
        assert_eq!(TodoStatus::Completed.to_string(), "completed");
        assert_eq!(TodoStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn test_todo_status_from_str() {
        assert_eq!(
            TodoStatus::from_str("pending").unwrap(),
            TodoStatus::Pending
        );
        assert_eq!(
            TodoStatus::from_str("in_progress").unwrap(),
            TodoStatus::InProgress
        );
        assert_eq!(
            TodoStatus::from_str("inprogress").unwrap(),
            TodoStatus::InProgress
        );
        assert_eq!(
            TodoStatus::from_str("completed").unwrap(),
            TodoStatus::Completed
        );
        assert_eq!(TodoStatus::from_str("done").unwrap(), TodoStatus::Completed);
        assert_eq!(
            TodoStatus::from_str("cancelled").unwrap(),
            TodoStatus::Cancelled
        );
        assert_eq!(
            TodoStatus::from_str("canceled").unwrap(),
            TodoStatus::Cancelled
        );
        assert_eq!(
            TodoStatus::from_str("unknown").unwrap(),
            TodoStatus::Pending
        );
    }

    #[test]
    fn test_todo_status_is_active() {
        assert!(TodoStatus::Pending.is_active());
        assert!(TodoStatus::InProgress.is_active());
        assert!(!TodoStatus::Completed.is_active());
        assert!(!TodoStatus::Cancelled.is_active());
    }

    #[test]
    fn test_todo_priority_display() {
        assert_eq!(TodoPriority::High.to_string(), "high");
        assert_eq!(TodoPriority::Medium.to_string(), "medium");
        assert_eq!(TodoPriority::Low.to_string(), "low");
    }

    #[test]
    fn test_todo_priority_from_str() {
        assert_eq!(TodoPriority::from_str("high").unwrap(), TodoPriority::High);
        assert_eq!(TodoPriority::from_str("h").unwrap(), TodoPriority::High);
        assert_eq!(
            TodoPriority::from_str("medium").unwrap(),
            TodoPriority::Medium
        );
        assert_eq!(TodoPriority::from_str("med").unwrap(), TodoPriority::Medium);
        assert_eq!(TodoPriority::from_str("low").unwrap(), TodoPriority::Low);
        assert_eq!(TodoPriority::from_str("l").unwrap(), TodoPriority::Low);
        assert_eq!(
            TodoPriority::from_str("unknown").unwrap(),
            TodoPriority::Medium
        );
    }

    #[test]
    fn test_todo_new() {
        let todo = Todo::new("1", "Test task");
        assert_eq!(todo.id, "1");
        assert_eq!(todo.content, "Test task");
        assert_eq!(todo.status, TodoStatus::Pending);
        assert_eq!(todo.priority, TodoPriority::Medium);
    }

    #[test]
    fn test_todo_builder() {
        let todo = Todo::new("1", "Test task")
            .with_priority(TodoPriority::High)
            .with_status(TodoStatus::InProgress);

        assert_eq!(todo.priority, TodoPriority::High);
        assert_eq!(todo.status, TodoStatus::InProgress);
    }

    #[test]
    fn test_todo_is_active() {
        let pending = Todo::new("1", "Pending task");
        let in_progress = Todo::new("2", "In progress").with_status(TodoStatus::InProgress);
        let completed = Todo::new("3", "Completed").with_status(TodoStatus::Completed);
        let cancelled = Todo::new("4", "Cancelled").with_status(TodoStatus::Cancelled);

        assert!(pending.is_active());
        assert!(in_progress.is_active());
        assert!(!completed.is_active());
        assert!(!cancelled.is_active());
    }

    #[test]
    fn test_todo_serialization() {
        let todo = Todo::new("1", "Test task")
            .with_priority(TodoPriority::High)
            .with_status(TodoStatus::InProgress);

        let json = serde_json::to_string(&todo).unwrap();
        let parsed: Todo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, todo.id);
        assert_eq!(parsed.content, todo.content);
        assert_eq!(parsed.status, todo.status);
        assert_eq!(parsed.priority, todo.priority);
    }
}
