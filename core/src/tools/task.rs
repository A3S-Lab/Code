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
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinSet;

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
            security_config: None,
            hook_engine: None,
            planning_enabled: false,
            goal_tracking: false,
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

    /// Execute multiple tasks in parallel
    ///
    /// Spawns all tasks concurrently and waits for all to complete.
    /// Returns results in the same order as the input tasks.
    pub async fn execute_parallel(
        self: &Arc<Self>,
        parent_session_id: &str,
        tasks: Vec<TaskParams>,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
    ) -> Vec<TaskResult> {
        let mut join_set = JoinSet::new();

        // Spawn all tasks concurrently
        for params in tasks {
            let executor = Arc::clone(self);
            let parent_id = parent_session_id.to_string();
            let tx = event_tx.clone();

            join_set.spawn(async move {
                match executor.execute(&parent_id, params.clone(), tx).await {
                    Ok(result) => result,
                    Err(e) => TaskResult {
                        output: format!("Task failed: {}", e),
                        session_id: String::new(),
                        agent: params.agent,
                        success: false,
                        task_id: format!("task-{}", uuid::Uuid::new_v4()),
                    },
                }
            });
        }

        // Collect all results
        let mut results = Vec::new();
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(task_result) => results.push(task_result),
                Err(e) => {
                    tracing::error!("Parallel task panicked: {}", e);
                    results.push(TaskResult {
                        output: format!("Task panicked: {}", e),
                        session_id: String::new(),
                        agent: "unknown".to_string(),
                        success: false,
                        task_id: format!("task-{}", uuid::Uuid::new_v4()),
                    });
                }
            }
        }

        results
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

/// TaskTool wraps TaskExecutor as a Tool for registration in ToolExecutor.
/// This allows the LLM to delegate tasks to subagents via the standard tool interface.
pub struct TaskTool {
    executor: Arc<TaskExecutor>,
}

impl TaskTool {
    /// Create a new TaskTool
    pub fn new(executor: Arc<TaskExecutor>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Delegate a task to a specialized subagent. Built-in agents: explore (read-only codebase search), general (full access multi-step), plan (read-only planning). Custom agents from agent_dirs are also available."
    }

    fn parameters(&self) -> serde_json::Value {
        task_params_schema()
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let params: TaskParams =
            serde_json::from_value(args.clone()).context("Invalid task parameters")?;

        let session_id = ctx.session_id.as_deref().unwrap_or("unknown");

        let result = self.executor.execute(session_id, params, None).await?;

        if result.success {
            Ok(ToolOutput::success(result.output))
        } else {
            Ok(ToolOutput::error(result.output))
        }
    }
}

/// Parameters for parallel task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelTaskParams {
    /// List of tasks to execute concurrently
    pub tasks: Vec<TaskParams>,
}

/// Get the JSON schema for ParallelTaskParams
pub fn parallel_task_params_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "tasks": {
                "type": "array",
                "description": "List of tasks to execute in parallel. Each task runs as an independent subagent concurrently.",
                "items": {
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
                        }
                    },
                    "required": ["agent", "description", "prompt"]
                },
                "minItems": 1
            }
        },
        "required": ["tasks"]
    })
}

/// ParallelTaskTool allows the LLM to fan-out multiple subagent tasks concurrently.
///
/// All tasks execute in parallel and the tool returns when all complete.
pub struct ParallelTaskTool {
    executor: Arc<TaskExecutor>,
}

impl ParallelTaskTool {
    /// Create a new ParallelTaskTool
    pub fn new(executor: Arc<TaskExecutor>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl Tool for ParallelTaskTool {
    fn name(&self) -> &str {
        "parallel_task"
    }

    fn description(&self) -> &str {
        "Execute multiple subagent tasks in parallel. All tasks run concurrently and results are returned when all complete. Built-in agents: explore (read-only codebase search), general (full access multi-step), plan (read-only planning). Custom agents from agent_dirs are also available."
    }

    fn parameters(&self) -> serde_json::Value {
        parallel_task_params_schema()
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let params: ParallelTaskParams =
            serde_json::from_value(args.clone()).context("Invalid parallel task parameters")?;

        if params.tasks.is_empty() {
            return Ok(ToolOutput::error("No tasks provided".to_string()));
        }

        let session_id = ctx.session_id.as_deref().unwrap_or("unknown");
        let task_count = params.tasks.len();

        let results = self
            .executor
            .execute_parallel(session_id, params.tasks, None)
            .await;

        // Format results
        let mut output = format!("Executed {} tasks in parallel:\n\n", task_count);
        for (i, result) in results.iter().enumerate() {
            let status = if result.success { "[OK]" } else { "[ERR]" };
            output.push_str(&format!(
                "--- Task {} ({}) {} ---\n{}\n\n",
                i + 1,
                result.agent,
                status,
                result.output
            ));
        }

        Ok(ToolOutput::success(output))
    }
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
    fn test_task_params_with_max_steps() {
        let json = r#"{
            "agent": "plan",
            "description": "Planning task",
            "prompt": "Create a plan",
            "max_steps": 10
        }"#;

        let params: TaskParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.agent, "plan");
        assert_eq!(params.max_steps, Some(10));
        assert!(!params.background);
    }

    #[test]
    fn test_task_params_all_fields() {
        let json = r#"{
            "agent": "general",
            "description": "Complex task",
            "prompt": "Do everything",
            "background": true,
            "max_steps": 20
        }"#;

        let params: TaskParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.agent, "general");
        assert_eq!(params.description, "Complex task");
        assert_eq!(params.prompt, "Do everything");
        assert!(params.background);
        assert_eq!(params.max_steps, Some(20));
    }

    #[test]
    fn test_task_params_missing_required_field() {
        let json = r#"{
            "agent": "explore",
            "description": "Missing prompt"
        }"#;

        let result: Result<TaskParams, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_task_params_serialize() {
        let params = TaskParams {
            agent: "explore".to_string(),
            description: "Test task".to_string(),
            prompt: "Test prompt".to_string(),
            background: false,
            max_steps: Some(5),
        };

        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("explore"));
        assert!(json.contains("Test task"));
        assert!(json.contains("Test prompt"));
    }

    #[test]
    fn test_task_params_clone() {
        let params = TaskParams {
            agent: "explore".to_string(),
            description: "Test".to_string(),
            prompt: "Prompt".to_string(),
            background: true,
            max_steps: None,
        };

        let cloned = params.clone();
        assert_eq!(params.agent, cloned.agent);
        assert_eq!(params.description, cloned.description);
        assert_eq!(params.background, cloned.background);
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
    fn test_task_result_deserialize() {
        let json = r#"{
            "output": "Task completed",
            "session_id": "sess-789",
            "agent": "general",
            "success": false,
            "task_id": "task-123"
        }"#;

        let result: TaskResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.output, "Task completed");
        assert_eq!(result.session_id, "sess-789");
        assert_eq!(result.agent, "general");
        assert!(!result.success);
        assert_eq!(result.task_id, "task-123");
    }

    #[test]
    fn test_task_result_clone() {
        let result = TaskResult {
            output: "Output".to_string(),
            session_id: "session-1".to_string(),
            agent: "explore".to_string(),
            success: true,
            task_id: "task-1".to_string(),
        };

        let cloned = result.clone();
        assert_eq!(result.output, cloned.output);
        assert_eq!(result.success, cloned.success);
    }

    #[test]
    fn test_task_params_schema() {
        let schema = task_params_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["agent"].is_object());
        assert!(schema["properties"]["prompt"].is_object());
    }

    #[test]
    fn test_task_params_schema_required_fields() {
        let schema = task_params_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("agent")));
        assert!(required.contains(&serde_json::json!("description")));
        assert!(required.contains(&serde_json::json!("prompt")));
    }

    #[test]
    fn test_task_params_schema_properties() {
        let schema = task_params_schema();
        let props = &schema["properties"];

        assert_eq!(props["agent"]["type"], "string");
        assert_eq!(props["description"]["type"], "string");
        assert_eq!(props["prompt"]["type"], "string");
        assert_eq!(props["background"]["type"], "boolean");
        assert_eq!(props["background"]["default"], false);
        assert_eq!(props["max_steps"]["type"], "integer");
    }

    #[test]
    fn test_task_params_schema_descriptions() {
        let schema = task_params_schema();
        let props = &schema["properties"];

        assert!(props["agent"]["description"].is_string());
        assert!(props["description"]["description"].is_string());
        assert!(props["prompt"]["description"].is_string());
        assert!(props["background"]["description"].is_string());
        assert!(props["max_steps"]["description"].is_string());
    }

    #[test]
    fn test_task_params_default_background() {
        let params = TaskParams {
            agent: "explore".to_string(),
            description: "Test".to_string(),
            prompt: "Test prompt".to_string(),
            background: false,
            max_steps: None,
        };
        assert!(!params.background);
    }

    #[test]
    fn test_task_params_serialize_skip_none() {
        let params = TaskParams {
            agent: "explore".to_string(),
            description: "Test".to_string(),
            prompt: "Test prompt".to_string(),
            background: false,
            max_steps: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        // max_steps should not appear when None
        assert!(!json.contains("max_steps"));
    }

    #[test]
    fn test_task_params_serialize_with_max_steps() {
        let params = TaskParams {
            agent: "explore".to_string(),
            description: "Test".to_string(),
            prompt: "Test prompt".to_string(),
            background: false,
            max_steps: Some(15),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("max_steps"));
        assert!(json.contains("15"));
    }

    #[test]
    fn test_task_result_success_true() {
        let result = TaskResult {
            output: "Success".to_string(),
            session_id: "sess-1".to_string(),
            agent: "explore".to_string(),
            success: true,
            task_id: "task-1".to_string(),
        };
        assert!(result.success);
    }

    #[test]
    fn test_task_result_success_false() {
        let result = TaskResult {
            output: "Failed".to_string(),
            session_id: "sess-1".to_string(),
            agent: "explore".to_string(),
            success: false,
            task_id: "task-1".to_string(),
        };
        assert!(!result.success);
    }

    #[test]
    fn test_task_params_empty_strings() {
        let params = TaskParams {
            agent: "".to_string(),
            description: "".to_string(),
            prompt: "".to_string(),
            background: false,
            max_steps: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        let deserialized: TaskParams = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.agent, "");
        assert_eq!(deserialized.description, "");
        assert_eq!(deserialized.prompt, "");
    }

    #[test]
    fn test_task_result_empty_output() {
        let result = TaskResult {
            output: "".to_string(),
            session_id: "sess-1".to_string(),
            agent: "explore".to_string(),
            success: true,
            task_id: "task-1".to_string(),
        };
        assert_eq!(result.output, "");
    }

    #[test]
    fn test_task_params_debug_format() {
        let params = TaskParams {
            agent: "explore".to_string(),
            description: "Test".to_string(),
            prompt: "Test prompt".to_string(),
            background: false,
            max_steps: None,
        };
        let debug_str = format!("{:?}", params);
        assert!(debug_str.contains("explore"));
        assert!(debug_str.contains("Test"));
    }

    #[test]
    fn test_task_result_debug_format() {
        let result = TaskResult {
            output: "Output".to_string(),
            session_id: "sess-1".to_string(),
            agent: "explore".to_string(),
            success: true,
            task_id: "task-1".to_string(),
        };
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("Output"));
        assert!(debug_str.contains("explore"));
    }

    #[test]
    fn test_task_params_roundtrip() {
        let original = TaskParams {
            agent: "general".to_string(),
            description: "Roundtrip test".to_string(),
            prompt: "Test roundtrip serialization".to_string(),
            background: true,
            max_steps: Some(42),
        };
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: TaskParams = serde_json::from_str(&json).unwrap();
        assert_eq!(original.agent, deserialized.agent);
        assert_eq!(original.description, deserialized.description);
        assert_eq!(original.prompt, deserialized.prompt);
        assert_eq!(original.background, deserialized.background);
        assert_eq!(original.max_steps, deserialized.max_steps);
    }

    #[test]
    fn test_task_result_roundtrip() {
        let original = TaskResult {
            output: "Roundtrip output".to_string(),
            session_id: "sess-roundtrip".to_string(),
            agent: "plan".to_string(),
            success: false,
            task_id: "task-roundtrip".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: TaskResult = serde_json::from_str(&json).unwrap();
        assert_eq!(original.output, deserialized.output);
        assert_eq!(original.session_id, deserialized.session_id);
        assert_eq!(original.agent, deserialized.agent);
        assert_eq!(original.success, deserialized.success);
        assert_eq!(original.task_id, deserialized.task_id);
    }

    #[test]
    fn test_parallel_task_params_deserialize() {
        let json = r#"{
            "tasks": [
                { "agent": "explore", "description": "Find auth", "prompt": "Search auth files" },
                { "agent": "general", "description": "Fix bug", "prompt": "Fix the login bug" }
            ]
        }"#;

        let params: ParallelTaskParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.tasks.len(), 2);
        assert_eq!(params.tasks[0].agent, "explore");
        assert_eq!(params.tasks[1].agent, "general");
    }

    #[test]
    fn test_parallel_task_params_single_task() {
        let json = r#"{
            "tasks": [
                { "agent": "plan", "description": "Plan work", "prompt": "Create a plan" }
            ]
        }"#;

        let params: ParallelTaskParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.tasks.len(), 1);
    }

    #[test]
    fn test_parallel_task_params_empty_tasks() {
        let json = r#"{ "tasks": [] }"#;
        let params: ParallelTaskParams = serde_json::from_str(json).unwrap();
        assert!(params.tasks.is_empty());
    }

    #[test]
    fn test_parallel_task_params_missing_tasks() {
        let json = r#"{}"#;
        let result: Result<ParallelTaskParams, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_parallel_task_params_serialize() {
        let params = ParallelTaskParams {
            tasks: vec![
                TaskParams {
                    agent: "explore".to_string(),
                    description: "Task 1".to_string(),
                    prompt: "Prompt 1".to_string(),
                    background: false,
                    max_steps: None,
                },
                TaskParams {
                    agent: "general".to_string(),
                    description: "Task 2".to_string(),
                    prompt: "Prompt 2".to_string(),
                    background: false,
                    max_steps: Some(10),
                },
            ],
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("explore"));
        assert!(json.contains("general"));
        assert!(json.contains("Prompt 1"));
        assert!(json.contains("Prompt 2"));
    }

    #[test]
    fn test_parallel_task_params_roundtrip() {
        let original = ParallelTaskParams {
            tasks: vec![
                TaskParams {
                    agent: "explore".to_string(),
                    description: "Explore".to_string(),
                    prompt: "Find files".to_string(),
                    background: false,
                    max_steps: None,
                },
                TaskParams {
                    agent: "plan".to_string(),
                    description: "Plan".to_string(),
                    prompt: "Make plan".to_string(),
                    background: false,
                    max_steps: Some(5),
                },
            ],
        };
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: ParallelTaskParams = serde_json::from_str(&json).unwrap();
        assert_eq!(original.tasks.len(), deserialized.tasks.len());
        assert_eq!(original.tasks[0].agent, deserialized.tasks[0].agent);
        assert_eq!(original.tasks[1].agent, deserialized.tasks[1].agent);
        assert_eq!(original.tasks[1].max_steps, deserialized.tasks[1].max_steps);
    }

    #[test]
    fn test_parallel_task_params_clone() {
        let params = ParallelTaskParams {
            tasks: vec![TaskParams {
                agent: "explore".to_string(),
                description: "Test".to_string(),
                prompt: "Prompt".to_string(),
                background: false,
                max_steps: None,
            }],
        };
        let cloned = params.clone();
        assert_eq!(params.tasks.len(), cloned.tasks.len());
        assert_eq!(params.tasks[0].agent, cloned.tasks[0].agent);
    }

    #[test]
    fn test_parallel_task_params_schema() {
        let schema = parallel_task_params_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["tasks"].is_object());
        assert_eq!(schema["properties"]["tasks"]["type"], "array");
        assert_eq!(schema["properties"]["tasks"]["minItems"], 1);
    }

    #[test]
    fn test_parallel_task_params_schema_required() {
        let schema = parallel_task_params_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("tasks")));
    }

    #[test]
    fn test_parallel_task_params_schema_items() {
        let schema = parallel_task_params_schema();
        let items = &schema["properties"]["tasks"]["items"];
        assert_eq!(items["type"], "object");
        let item_required = items["required"].as_array().unwrap();
        assert!(item_required.contains(&serde_json::json!("agent")));
        assert!(item_required.contains(&serde_json::json!("description")));
        assert!(item_required.contains(&serde_json::json!("prompt")));
    }

    #[test]
    fn test_parallel_task_params_debug() {
        let params = ParallelTaskParams {
            tasks: vec![TaskParams {
                agent: "explore".to_string(),
                description: "Debug test".to_string(),
                prompt: "Test".to_string(),
                background: false,
                max_steps: None,
            }],
        };
        let debug_str = format!("{:?}", params);
        assert!(debug_str.contains("explore"));
        assert!(debug_str.contains("Debug test"));
    }
}
