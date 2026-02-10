//! Task Tool for Spawning Subagents
//!
//! The Task tool allows the main agent to delegate specialized tasks to
//! focused child agents (subagents). Each subagent runs in an isolated
//! child session with restricted permissions.
//!
//! ## Usage
//!
//! ```json
//! {
//!   "agent": "explore",
//!   "description": "Find authentication code",
//!   "prompt": "Search for files related to user authentication..."
//! }
//! ```

use crate::agent::AgentEvent;
use crate::session::{SessionConfig, SessionManager};
use crate::subagent::AgentRegistry;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Task tool parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskParams {
    /// Agent type to use (explore, general, plan, etc.)
    pub agent: String,
    /// Short description of the task (for display)
    pub description: String,
    /// Detailed prompt for the agent
    pub prompt: String,
    /// Optional: run in background (default: false)
    #[serde(default)]
    pub background: bool,
    /// Optional: maximum steps for this task
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<usize>,
}

/// Task tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// Task output from the subagent
    pub output: String,
    /// Child session ID
    pub session_id: String,
    /// Agent type used
    pub agent: String,
    /// Whether the task succeeded
    pub success: bool,
    /// Task ID for tracking
    pub task_id: String,
}

/// Task executor for running subagent tasks
pub struct TaskExecutor {
    /// Agent registry for looking up agent definitions
    registry: Arc<AgentRegistry>,
    /// Session manager for creating child sessions
    session_manager: Arc<SessionManager>,
}

impl TaskExecutor {
    /// Create a new task executor
    pub fn new(registry: Arc<AgentRegistry>, session_manager: Arc<SessionManager>) -> Self {
        Self {
            registry,
            session_manager,
        }
    }

    /// Execute a task in a subagent
    ///
    /// This creates a child session, runs the prompt, and returns the result.
    pub async fn execute(
        &self,
        parent_session_id: &str,
        params: TaskParams,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
    ) -> Result<TaskResult> {
        // Generate unique task ID
        let task_id = format!("task-{}", uuid::Uuid::new_v4());

        // Get agent definition
        let agent = self
            .registry
            .get(&params.agent)
            .context(format!("Unknown agent type: {}", params.agent))?;

        // Check if parent session can spawn subagents
        // (This would be checked against the parent's agent definition)

        // Create child session config
        let child_config = SessionConfig {
            name: format!("{} - {}", params.agent, params.description),
            workspace: String::new(), // Inherit from parent
            system_prompt: agent.prompt.clone(),
            max_context_length: 0,
            auto_compact: false,
            auto_compact_threshold: crate::session::DEFAULT_AUTO_COMPACT_THRESHOLD,
            storage_type: crate::config::StorageBackend::Memory, // Subagents use memory storage
            queue_config: None,
            confirmation_policy: None,
            permission_policy: Some(agent.permissions.clone()),
            parent_id: Some(parent_session_id.to_string()),
            safeclaw_config: None,
        };

        // Generate child session ID
        let child_session_id = format!("{}-{}", parent_session_id, task_id);

        // Emit SubagentStart event
        if let Some(ref tx) = event_tx {
            let _ = tx.send(AgentEvent::SubagentStart {
                task_id: task_id.clone(),
                session_id: child_session_id.clone(),
                parent_session_id: parent_session_id.to_string(),
                agent: params.agent.clone(),
                description: params.description.clone(),
            });
        }

        // Create child session
        let session_id = self
            .session_manager
            .create_child_session(parent_session_id, child_session_id.clone(), child_config)
            .await
            .context("Failed to create child session")?;

        // Execute the prompt in the child session
        let result = self
            .session_manager
            .generate(&session_id, &params.prompt)
            .await;

        // Process result
        let (output, success) = match result {
            Ok(agent_result) => (agent_result.text, true),
            Err(e) => (format!("Task failed: {}", e), false),
        };

        // Emit SubagentEnd event
        if let Some(ref tx) = event_tx {
            let _ = tx.send(AgentEvent::SubagentEnd {
                task_id: task_id.clone(),
                session_id: session_id.clone(),
                agent: params.agent.clone(),
                output: output.clone(),
                success,
            });
        }

        Ok(TaskResult {
            output,
            session_id,
            agent: params.agent,
            success,
            task_id,
        })
    }

    /// Execute a task in the background
    ///
    /// Returns immediately with the task ID. Use events to track progress.
    pub fn execute_background(
        self: Arc<Self>,
        parent_session_id: String,
        params: TaskParams,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
    ) -> String {
        let task_id = format!("task-{}", uuid::Uuid::new_v4());
        let task_id_clone = task_id.clone();

        tokio::spawn(async move {
            let result = self.execute(&parent_session_id, params, event_tx).await;

            if let Err(e) = result {
                tracing::error!("Background task {} failed: {}", task_id_clone, e);
            }
        });

        task_id
    }
}

/// Get the JSON schema for TaskParams
pub fn task_params_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "agent": {
                "type": "string",
                "description": "Agent type to use (explore, general, plan, etc.)"
            },
            "description": {
                "type": "string",
                "description": "Short description of the task (for display)"
            },
            "prompt": {
                "type": "string",
                "description": "Detailed prompt for the agent"
            },
            "background": {
                "type": "boolean",
                "description": "Run in background (default: false)",
                "default": false
            },
            "max_steps": {
                "type": "integer",
                "description": "Maximum steps for this task"
            }
        },
        "required": ["agent", "description", "prompt"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_params_deserialize() {
        let json = r#"{
            "agent": "explore",
            "description": "Find auth code",
            "prompt": "Search for authentication files"
        }"#;

        let params: TaskParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.agent, "explore");
        assert_eq!(params.description, "Find auth code");
        assert!(!params.background);
    }

    #[test]
    fn test_task_params_with_background() {
        let json = r#"{
            "agent": "general",
            "description": "Long task",
            "prompt": "Do something complex",
            "background": true
        }"#;

        let params: TaskParams = serde_json::from_str(json).unwrap();
        assert!(params.background);
    }

    #[test]
    fn test_task_result_serialize() {
        let result = TaskResult {
            output: "Found 5 files".to_string(),
            session_id: "session-123".to_string(),
            agent: "explore".to_string(),
            success: true,
            task_id: "task-456".to_string(),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("Found 5 files"));
        assert!(json.contains("explore"));
    }

    #[test]
    fn test_task_params_schema() {
        let schema = task_params_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["agent"].is_object());
        assert!(schema["properties"]["prompt"].is_object());
    }
}
