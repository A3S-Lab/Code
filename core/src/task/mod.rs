//! Task Lifecycle Management
//!
//! Provides unified Task abstraction for all operations in a3s-code.
//! This module brings together tool calls, agents, workflows, and background
//! tasks under a common lifecycle management framework.
//!
//! ## Task Types
//!
//! - `Tool` - Direct tool execution
//! - `Agent` - Sub-agent execution
//! - `Workflow` - DAG-based workflow execution
//! - `Idle` - Memory consolidation task
//!
//! ## Example
//!
//! ```rust,ignore
//! use a3s_code_core::task::{TaskManager, TaskId, TaskType};
//!
//! let manager = TaskManager::new();
//! let task_id = manager.spawn(Task {
//!     kind: TaskType::Tool { name: "read".into(), args: json!({"file_path": "test.txt"}) },
//!     ..Default::default()
//! });
//!
//! let result = manager.wait(task_id).await;
//! ```

pub mod coordinator;
pub mod idle;
pub mod manager;
pub mod progress;
pub mod types;

pub use coordinator::Coordinator;
pub use idle::{IdlePhase, IdleTask, IdleTurn};
pub use manager::{TaskManager, TaskResult};
pub use progress::{AgentProgress, ProgressTracker, TaskTokenUsage, ToolActivity};
pub use types::{Task, TaskId, TaskStatus, TaskType};
