//! Task tools for delegated child runs.
//!
//! The Task tool allows the main agent to delegate specialized work to focused
//! child runs. Each child run gets bounded context and the permissions declared
//! by its agent definition.
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

use crate::agent::{AgentConfig, AgentEvent, AgentLoop};
use crate::llm::structured::{
    generate_blocking, parse_validated_output, StructuredMode, StructuredRequest,
};
use crate::llm::{LlmClient, ToolDefinition};
use crate::mcp::manager::McpManager;
use crate::orchestration::{AgentExecutor, AgentStepSpec, StepOutcome, ToolSourceAnchor};
use crate::subagent::{AgentDefinition, AgentRegistry};
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const TASK_OUTPUT_CONTEXT_LIMIT: usize = 4_000;
const TASK_OUTPUT_CONTEXT_HEAD: usize = 3_000;
const TASK_OUTPUT_CONTEXT_TAIL: usize = 800;
const MAX_TASK_SOURCE_ANCHORS: usize = 64;
const MAX_TASK_SOURCE_CANDIDATES: usize = MAX_TASK_SOURCE_ANCHORS * 4;
const MAX_TASK_SOURCE_TOOL_BYTES: usize = 64;
const MAX_TASK_SOURCE_VALUE_BYTES: usize = 4 * 1024;
const MAX_PARALLEL_TASK_SOURCE_ANCHORS: usize = MAX_TASK_SOURCE_ANCHORS;
const TASK_TOOL_DESCRIPTION: &str = "Delegate a bounded task to a specialized child run. Choose the canonical worker name from the live agent catalog. Custom agents from agent_dirs and .a3s/agents are supported; .claude/agents is read for compatibility.";
const PARALLEL_TASK_TOOL_DESCRIPTION: &str = "Fan out 2 or more INDEPENDENT subtasks as delegated child runs that execute concurrently; results are returned when all complete. By default any failed child makes the tool fail; evidence-gathering callers may set allow_partial_failure=true to continue when at least one child succeeds. Child output never authorizes branch replay; provider and child runtimes own any typed retry policy below this boundary. Use this only when the work genuinely splits into branches that can be investigated or implemented separately. Do not use it for trivial, conversational, single-step, or dependent work. Choose canonical worker names from the live agent catalog.";

/// Task tool parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskParams {
    /// Agent type to use (explore, general, plan, verification, review, etc.)
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
    /// Optional: JSON schema the child result must satisfy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
}

/// Task tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// Task output from the delegated child run.
    pub output: String,
    /// Child session ID
    pub session_id: String,
    /// Agent type used
    pub agent: String,
    /// Whether the task succeeded
    pub success: bool,
    /// Task ID for tracking
    pub task_id: String,
    /// Structured child output validated against an optional output schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<serde_json::Value>,
    /// Source locations observed by successful built-in child tool calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_anchors: Vec<ToolSourceAnchor>,
}

mod result_projection;
use result_projection::*;

mod parallel_execution;

const MAX_PARALLEL_TASKS_PER_CALL: usize = 32;

/// Task executor for delegated child runs.
#[derive(Clone)]
pub struct TaskExecutor {
    /// Agent registry for looking up agent definitions
    registry: Arc<AgentRegistry>,
    /// LLM client used to power child agent loops
    llm_client: Arc<dyn LlmClient>,
    /// Workspace path shared with child agents
    workspace: String,
    /// Ordered MCP managers for registering inherited tools in child sessions.
    mcp_managers: Vec<Arc<McpManager>>,
    /// Parent capabilities to inherit into child runs.
    parent_context: Option<crate::child_run::ChildRunContext>,
    /// Optional lifetime boundary inherited from the session that created this
    /// executor. Keeping it on the executor prevents cached workflow/executor
    /// handles from starting new child runs after their session is closed.
    parent_cancellation: Option<CancellationToken>,
    max_parallel_tasks: usize,
    /// Shared across every fan-out started by this executor. Per-call wave
    /// limits alone are insufficient when a dynamic workflow launches several
    /// `parallel_task` host steps concurrently.
    parallel_permits: Arc<tokio::sync::Semaphore>,
    /// Optional shared tracker — when present each task registers a
    /// `CancellationToken` so callers can cancel by `task_id`.
    subagent_tracker: Option<Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>>,
}

impl TaskExecutor {
    /// Create a new task executor
    pub fn new(
        registry: Arc<AgentRegistry>,
        llm_client: Arc<dyn LlmClient>,
        workspace: String,
    ) -> Self {
        Self {
            registry,
            llm_client,
            workspace,
            mcp_managers: Vec::new(),
            parent_context: None,
            parent_cancellation: None,
            max_parallel_tasks: crate::agent::DEFAULT_MAX_PARALLEL_TASKS,
            parallel_permits: Arc::new(tokio::sync::Semaphore::new(
                crate::agent::DEFAULT_MAX_PARALLEL_TASKS,
            )),
            subagent_tracker: None,
        }
    }

    /// Create a new task executor with MCP manager for tool inheritance
    pub fn with_mcp(
        registry: Arc<AgentRegistry>,
        llm_client: Arc<dyn LlmClient>,
        workspace: String,
        mcp_manager: Arc<McpManager>,
    ) -> Self {
        Self::with_mcp_managers(registry, llm_client, workspace, vec![mcp_manager])
    }

    /// Create a task executor with ordered MCP capability sources.
    pub fn with_mcp_managers(
        registry: Arc<AgentRegistry>,
        llm_client: Arc<dyn LlmClient>,
        workspace: String,
        mcp_managers: Vec<Arc<McpManager>>,
    ) -> Self {
        Self {
            registry,
            llm_client,
            workspace,
            mcp_managers,
            parent_context: None,
            parent_cancellation: None,
            max_parallel_tasks: crate::agent::DEFAULT_MAX_PARALLEL_TASKS,
            parallel_permits: Arc::new(tokio::sync::Semaphore::new(
                crate::agent::DEFAULT_MAX_PARALLEL_TASKS,
            )),
            subagent_tracker: None,
        }
    }

    /// Set parent session capabilities to inherit into child runs.
    pub fn with_parent_context(mut self, ctx: crate::child_run::ChildRunContext) -> Self {
        if let Some(max_parallel_tasks) = ctx.max_parallel_tasks {
            let max_parallel_tasks = max_parallel_tasks.max(1);
            self.max_parallel_tasks = max_parallel_tasks;
            self.parallel_permits = Arc::new(tokio::sync::Semaphore::new(max_parallel_tasks));
        }
        self.parent_context = Some(ctx);
        self
    }

    fn scoped_for_invocation(self: &Arc<Self>, ctx: &ToolContext) -> Arc<Self> {
        if !ctx.has_run_governance() {
            return Arc::clone(self);
        }
        let mut scoped = self.as_ref().clone();
        scoped.parent_context = scoped.parent_context.take().map(|parent| {
            parent.with_run_governance(ctx.run_permission_checker(), ctx.run_confirmation_manager())
        });
        Arc::new(scoped)
    }

    /// Bind every run started by this executor to a parent lifetime.
    ///
    /// A token that is already cancelled makes execution fail before emitting
    /// `SubagentStart` or performing MCP/LLM work. In-flight children derive
    /// their own token so cancellation still cascades without granting them the
    /// ability to cancel the parent.
    pub fn with_parent_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.parent_cancellation = Some(cancellation);
        self
    }

    pub fn with_max_parallel_tasks(mut self, max_parallel_tasks: usize) -> Self {
        let max_parallel_tasks = max_parallel_tasks.max(1);
        self.max_parallel_tasks = max_parallel_tasks;
        self.parallel_permits = Arc::new(tokio::sync::Semaphore::new(max_parallel_tasks));
        self
    }

    /// Share a tracker with this executor. When set, each task registers
    /// a `CancellationToken` against the tracker so the parent session
    /// can cancel by `task_id`.
    pub fn with_subagent_tracker(
        mut self,
        tracker: Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>,
    ) -> Self {
        self.subagent_tracker = Some(tracker);
        self
    }

    fn visible_agents(&self) -> Vec<AgentDefinition> {
        self.registry.list_visible()
    }

    /// Execute a task by spawning an isolated child AgentLoop.
    ///
    /// `parent_session_id` flows into the emitted `SubagentStart`/`SubagentEnd`
    /// events so dashboards can associate child runs with the parent session.
    pub async fn execute(
        &self,
        params: TaskParams,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
        parent_session_id: Option<&str>,
    ) -> Result<TaskResult> {
        self.execute_with_parent_cancellation(
            params,
            event_tx,
            parent_session_id,
            self.parent_cancellation.as_ref(),
        )
        .await
    }

    async fn execute_with_parent_cancellation(
        &self,
        params: TaskParams,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
        parent_session_id: Option<&str>,
        parent_cancellation: Option<&CancellationToken>,
    ) -> Result<TaskResult> {
        let task_id = format!("task-{}", uuid::Uuid::new_v4());
        self.execute_with_task_id_scoped(
            task_id,
            params,
            event_tx,
            parent_session_id,
            true,
            parent_cancellation,
        )
        .await
    }

    /// Execute a task using a caller-supplied task id. Used by `execute_background`
    /// so the synchronously-returned task id matches the one in lifecycle events.
    /// When `emit_start` is `false` the caller is responsible for emitting
    /// `SubagentStart` themselves (e.g. to avoid a race against a tracker query).
    pub async fn execute_with_task_id(
        &self,
        task_id: String,
        params: TaskParams,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
        parent_session_id: Option<&str>,
        emit_start: bool,
    ) -> Result<TaskResult> {
        self.execute_with_task_id_scoped(
            task_id,
            params,
            event_tx,
            parent_session_id,
            emit_start,
            self.parent_cancellation.as_ref(),
        )
        .await
    }

    async fn execute_with_task_id_scoped(
        &self,
        task_id: String,
        params: TaskParams,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
        parent_session_id: Option<&str>,
        emit_start: bool,
        parent_cancellation: Option<&CancellationToken>,
    ) -> Result<TaskResult> {
        if parent_cancellation.is_some_and(CancellationToken::is_cancelled) {
            anyhow::bail!("Operation cancelled by parent session");
        }

        let session_id = format!("task-run-{}", task_id);
        let started_ms = epoch_ms();
        let output_schema = params.output_schema.clone();

        let agent = self
            .registry
            .get(&params.agent)
            .context(format!("Unknown agent type: '{}'", params.agent))?;
        let tool_free = agent.tool_free;
        let tool_free_system = agent.prompt.clone();
        let inherited_security_provider = self
            .parent_context
            .as_ref()
            .and_then(|context| context.security_provider.clone());

        if emit_start {
            let event = AgentEvent::SubagentStart {
                task_id: task_id.clone(),
                session_id: session_id.clone(),
                parent_session_id: parent_session_id.unwrap_or_default().to_string(),
                agent: params.agent.clone(),
                description: params.description.clone(),
                started_ms,
            };
            let event = inherited_security_provider
                .as_deref()
                .map(|provider| crate::security::sanitize_agent_event(provider, &event))
                .unwrap_or(event);
            if let Some(ref tracker) = self.subagent_tracker {
                tracker.record_event(&event).await;
            }
            if let Some(ref tx) = event_tx {
                let _ = tx.send(event);
            }
        }

        // Build a child ToolExecutor. Task tools are intentionally omitted
        // here to prevent unlimited delegation nesting.
        let child_executor = if let Some(ref parent_ctx) = self.parent_context {
            if let Some(ref services) = parent_ctx.workspace_services {
                crate::tools::ToolExecutor::new_with_workspace_services_and_artifact_limits(
                    self.workspace.clone(),
                    Arc::clone(services),
                    crate::tools::ArtifactStoreLimits::default(),
                )
            } else {
                crate::tools::ToolExecutor::new(self.workspace.clone())
            }
        } else {
            crate::tools::ToolExecutor::new(self.workspace.clone())
        };

        // Register MCP tools so child agents can access MCP servers.
        for mcp in &self.mcp_managers {
            let all_tools = match parent_cancellation {
                Some(cancellation) => {
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => {
                            anyhow::bail!("Operation cancelled by parent session");
                        }
                        tools = mcp.get_all_tools() => tools,
                    }
                }
                None => mcp.get_all_tools().await,
            };
            let mut by_server: std::collections::HashMap<
                String,
                Vec<crate::mcp::protocol::McpTool>,
            > = std::collections::HashMap::new();
            for (server, tool) in all_tools {
                by_server.entry(server).or_default().push(tool);
            }
            for (server_name, tools) in by_server {
                let wrappers =
                    crate::mcp::tools::create_mcp_tools(&server_name, tools, Arc::clone(mcp));
                for wrapper in wrappers {
                    child_executor.register_dynamic_tool(wrapper);
                }
            }
        }

        let child_executor = Arc::new(child_executor);

        let mut child_config = AgentConfig {
            tools: child_executor.definitions(),
            ..AgentConfig::default()
        };
        agent.apply_to(&mut child_config);
        if let Some(ref parent_ctx) = self.parent_context {
            parent_ctx.apply_to(&mut child_config);
        }
        // A delegated task is already the output of a parent planning
        // decision. Running the generic pre-analysis/planning classifier again
        // adds an unrelated LLM round to every child and can consume the whole
        // fan-out deadline before any task tool runs.
        child_config.planning_mode = crate::prompts::PlanningMode::Disabled;
        if let Some(max_steps) = params.max_steps {
            child_config.max_tool_rounds = max_steps;
        }
        let child_security_provider = child_config.security_provider.clone();
        let source_security_provider = child_security_provider.clone();

        let cancel_token = parent_cancellation
            .map(CancellationToken::child_token)
            .unwrap_or_default();
        let mut tool_context = ToolContext::new(PathBuf::from(&self.workspace))
            .with_session_id(session_id.clone())
            .with_cancellation(cancel_token.clone());
        if let Some(ref parent_ctx) = self.parent_context {
            if let Some(ref services) = parent_ctx.workspace_services {
                tool_context = tool_context.with_workspace_services(Arc::clone(services));
            }
            if let Some(ref sandbox) = parent_ctx.sandbox_handle {
                child_executor.registry().set_sandbox(Arc::clone(sandbox));
                tool_context = tool_context.with_sandbox(Arc::clone(sandbox));
            }
        }

        let source_context = tool_context.clone();
        let agent_loop = AgentLoop::new(
            Arc::clone(&self.llm_client),
            child_executor,
            tool_context,
            child_config,
        );

        // Always observe the child event stream so successful source tool calls
        // survive in TaskResult metadata even when nobody subscribed to live
        // progress. Forward the same events when a parent broadcast exists.
        let (mpsc_tx, mut mpsc_rx) = tokio::sync::mpsc::channel(100);
        let broadcast_tx = event_tx.clone();
        let progress_task_id = task_id.clone();
        let progress_session_id = session_id.clone();
        let child_event_forwarder = tokio::spawn(async move {
            let mut source_anchors = Vec::new();
            let mut seen_source_anchors = std::collections::HashSet::new();
            let mut scanned_source_candidates = 0usize;
            while let Some(event) = mpsc_rx.recv().await {
                let event = source_security_provider
                    .as_deref()
                    .map(|provider| crate::security::sanitize_agent_event(provider, &event))
                    .unwrap_or(event);
                collect_tool_source_anchors(
                    &event,
                    &source_context,
                    &mut source_anchors,
                    &mut seen_source_anchors,
                    &mut scanned_source_candidates,
                );
                if let Some(ref broadcast_tx) = broadcast_tx {
                    if let Some(progress) = synthesize_subagent_progress(
                        &event,
                        &progress_task_id,
                        &progress_session_id,
                    ) {
                        let _ = broadcast_tx.send(progress);
                    }
                    let _ = broadcast_tx.send(event);
                }
            }
            source_anchors
        });
        let child_event_tx = Some(mpsc_tx);
        let child_llm_event_tx = child_event_tx.clone();

        // Register a CancellationToken with the tracker (if shared) so the
        // parent session's `cancel_subagent_task` can interrupt this run.
        if let Some(ref tracker) = self.subagent_tracker {
            tracker
                .register_canceller(&task_id, cancel_token.clone())
                .await;
        }

        let structured_prompt = output_schema
            .as_ref()
            .filter(|_| !tool_free)
            .map(|schema| structured_task_prompt(&params.prompt, schema));
        let execution_prompt = structured_prompt.as_deref().unwrap_or(&params.prompt);

        let mut structured = None;
        let (mut output, mut success) = if tool_free && output_schema.is_some() {
            let llm_client = agent_loop.scoped_llm_client_for_parts(
                Some(&session_id),
                &child_llm_event_tx,
                &cancel_token,
            );
            match Self::generate_structured_task(
                &*llm_client,
                &params.prompt,
                tool_free_system.as_deref(),
                output_schema.clone().expect("schema checked above"),
                &cancel_token,
            )
            .await
            {
                Ok(object) => {
                    let output = serde_json::to_string_pretty(&object)
                        .unwrap_or_else(|_| object.to_string());
                    structured = Some(object);
                    (output, true)
                }
                Err(error) if cancel_token.is_cancelled() => {
                    (format!("Task cancelled by caller: {error}"), false)
                }
                Err(error) => (format!("Task failed: {error}"), false),
            }
        } else {
            match agent_loop
                .execute_with_session(
                    &[],
                    execution_prompt,
                    Some(&session_id),
                    child_event_tx.clone(),
                    Some(&cancel_token),
                )
                .await
            {
                Ok(_) if cancel_token.is_cancelled() => {
                    ("Task cancelled by caller".to_string(), false)
                }
                Ok(result) if result.text.trim().is_empty() => (
                    "Task failed: child agent returned no final output".to_string(),
                    false,
                ),
                Ok(result) if AgentLoop::is_synthetic_failure_output(&result.text) => {
                    (format!("Task failed: {}", result.text), false)
                }
                Ok(result) => (result.text, true),
                Err(e) if cancel_token.is_cancelled() => {
                    (format!("Task cancelled by caller: {}", e), false)
                }
                Err(e) => (format!("Task failed: {}", e), false),
            }
        };

        if success && !tool_free {
            if let Some(schema) = output_schema {
                if let Some(object) = parse_validated_output(&output, &schema) {
                    structured = Some(object);
                } else {
                    let llm_client = agent_loop.scoped_llm_client_for_parts(
                        Some(&session_id),
                        &child_llm_event_tx,
                        &cancel_token,
                    );
                    match Self::coerce_to_schema(&*llm_client, &output, schema, &cancel_token).await
                    {
                        Ok(object) => structured = Some(object),
                        Err(error) => {
                            success = false;
                            output = format!("{output}\n\n[structured output failed: {error}]");
                        }
                    }
                }
            }
        }
        if let Some(provider) = child_security_provider.as_deref() {
            output = provider.sanitize_output(&output);
            if let Some(value) = &mut structured {
                *value = sanitize_task_json(provider, value);
            }
        }

        // The child loop and optional structured-output pass are the only
        // producers. Close their sender and drain the bridge before emitting
        // SubagentEnd so callers never observe a terminal event followed by
        // stale child deltas or progress events.
        drop(child_event_tx);
        drop(child_llm_event_tx);
        let source_anchors = match child_event_forwarder.await {
            Ok(source_anchors) => source_anchors,
            Err(error) => {
                tracing::warn!(%error, task_id = %task_id, "subagent event bridge failed");
                Vec::new()
            }
        };

        let end_event = AgentEvent::SubagentEnd {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            agent: params.agent.clone(),
            output: output.clone(),
            success,
            finished_ms: epoch_ms(),
        };
        if let Some(ref tracker) = self.subagent_tracker {
            // The tracker is authoritative even when a background child
            // finishes after the parent run's event forwarder has closed.
            if success {
                tracker
                    .record_source_anchors(&task_id, &source_anchors)
                    .await;
            }
            tracker.record_event(&end_event).await;
            tracker.clear_canceller(&task_id).await;
        }
        if let Some(ref tx) = event_tx {
            let _ = tx.send(end_event);
        }

        Ok(TaskResult {
            output,
            session_id,
            agent: params.agent,
            success,
            task_id,
            structured,
            source_anchors,
        })
    }

    /// Execute a task in the background.
    ///
    /// Returns immediately with the task ID; the same id is used in the emitted
    /// `SubagentStart`/`SubagentEnd` events so callers can correlate. Pre-emits
    /// `SubagentStart` synchronously when an event channel is available so a
    /// caller that queries the subagent task tracker right after this call
    /// observes the task in `Running` state without a race window.
    pub fn execute_background(
        self: Arc<Self>,
        params: TaskParams,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
        parent_session_id: Option<String>,
    ) -> String {
        let parent_cancellation = self.parent_cancellation.clone();
        self.execute_background_with_parent_cancellation(
            params,
            event_tx,
            parent_session_id,
            parent_cancellation,
        )
    }

    fn execute_background_with_parent_cancellation(
        self: Arc<Self>,
        params: TaskParams,
        event_tx: Option<broadcast::Sender<AgentEvent>>,
        parent_session_id: Option<String>,
        parent_cancellation: Option<CancellationToken>,
    ) -> String {
        let task_id = format!("task-{}", uuid::Uuid::new_v4());
        let session_id = format!("task-run-{}", task_id);
        let failure_session_id = session_id.clone();
        let failure_agent = params.agent.clone();
        let start_event = AgentEvent::SubagentStart {
            task_id: task_id.clone(),
            session_id,
            parent_session_id: parent_session_id.clone().unwrap_or_default(),
            agent: params.agent.clone(),
            description: params.description.clone(),
            started_ms: epoch_ms(),
        };
        let security_provider = self
            .parent_context
            .as_ref()
            .and_then(|context| context.security_provider.clone());
        let start_event = security_provider
            .as_deref()
            .map(|provider| crate::security::sanitize_agent_event(provider, &start_event))
            .unwrap_or(start_event);

        if let Some(ref tx) = event_tx {
            let _ = tx.send(start_event.clone());
        }

        let task_id_for_spawn = task_id.clone();
        let task_id_for_log = task_id.clone();
        tokio::spawn(async move {
            if let Some(ref tracker) = self.subagent_tracker {
                tracker.record_event(&start_event).await;
            }
            let failure_event_tx = event_tx.clone();
            if let Err(error) = self
                .execute_with_task_id_scoped(
                    task_id_for_spawn,
                    params,
                    event_tx,
                    parent_session_id.as_deref(),
                    false,
                    parent_cancellation.as_ref(),
                )
                .await
            {
                let end_event = AgentEvent::SubagentEnd {
                    task_id: task_id_for_log.clone(),
                    session_id: failure_session_id,
                    agent: failure_agent,
                    output: format!("Task failed before child execution started: {error}"),
                    success: false,
                    finished_ms: epoch_ms(),
                };
                let end_event = security_provider
                    .as_deref()
                    .map(|provider| crate::security::sanitize_agent_event(provider, &end_event))
                    .unwrap_or(end_event);
                if let Some(ref tracker) = self.subagent_tracker {
                    tracker.record_event(&end_event).await;
                    tracker.clear_canceller(&task_id_for_log).await;
                }
                if let Some(tx) = failure_event_tx {
                    let _ = tx.send(end_event);
                }
                tracing::error!("Background task {} failed: {}", task_id_for_log, error);
            }
        });

        task_id
    }
}

fn structured_task_prompt(prompt: &str, schema: &serde_json::Value) -> String {
    let schema = serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string());
    format!(
        "{prompt}\n\n\
         FINAL OUTPUT CONTRACT\n\
         Complete the requested investigation before answering. Your final response must contain \
         exactly one JSON value matching the JSON Schema below, with no Markdown fence or prose \
         outside the JSON. This contract applies to the final response only; use the available \
         tools as needed before finalizing.\n\n\
         {schema}"
    )
}

#[derive(Debug, Clone)]
struct AgentCatalogEntry {
    name: String,
    description: String,
}

fn agent_catalog_entries(agents: &[AgentDefinition]) -> Vec<AgentCatalogEntry> {
    let mut entries = agents
        .iter()
        .map(|agent| AgentCatalogEntry {
            name: agent.name.clone(),
            description: agent
                .description
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

fn agent_catalog_text(agents: &[AgentDefinition]) -> String {
    agent_catalog_entries(agents)
        .into_iter()
        .map(|entry| format!("{}: {}", entry.name, entry.description))
        .collect::<Vec<_>>()
        .join("\n")
}

fn delegation_tool_description(base: &str, agents: &[AgentDefinition]) -> String {
    format!(
        "{base}\n\nAvailable agents (live catalog; use canonical names):\n{}",
        agent_catalog_text(agents)
    )
}

pub(super) fn task_agent_parameter_schema(agents: &[AgentDefinition]) -> serde_json::Value {
    let entries = agent_catalog_entries(agents);
    let examples = entries
        .iter()
        .map(|entry| serde_json::Value::String(entry.name.clone()))
        .collect::<Vec<_>>();
    let catalog = entries
        .into_iter()
        .map(|entry| format!("{}: {}", entry.name, entry.description))
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::json!({
        "type": "string",
        "description": format!(
            "Required. Canonical agent type to use. Always provide this exact field name: 'agent'. Live agent catalog:\n{catalog}"
        ),
        "examples": examples
    })
}

/// Get the JSON schema for TaskParams using the built-in agent catalog.
pub fn task_params_schema() -> serde_json::Value {
    task_params_schema_for_agents(&AgentRegistry::new().list_visible())
}

fn task_params_schema_for_agents(agents: &[AgentDefinition]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "agent": task_agent_parameter_schema(agents),
            "description": {
                "type": "string",
                "description": "Required. Short task label for display and tracking. Always provide this exact field name: 'description'."
            },
            "prompt": {
                "type": "string",
                "description": "Required. Detailed instruction for the delegated child run. Always provide this exact field name: 'prompt'."
            },
            "background": {
                "type": "boolean",
                "description": "Optional. Run the task in the background. Default: false.",
                "default": false
            },
            "max_steps": {
                "type": "integer",
                "description": "Optional. Maximum number of steps for this task."
            },
            "output_schema": {
                "type": "object",
                "description": "Optional. JSON Schema object the delegated result must satisfy. When provided, the child output is coerced into a validated structured object and returned in metadata."
            }
        },
        "required": ["agent", "description", "prompt"],
        "examples": [
            {
                "agent": "explore",
                "description": "Find Rust files",
                "prompt": "Search the workspace for Rust files and summarize the layout."
            },
            {
                "agent": "general",
                "description": "Investigate test failure",
                "prompt": "Inspect the failing tests and explain the root cause.",
                "max_steps": 6
            }
        ]
    })
}

/// TaskTool wraps TaskExecutor as a Tool for registration in ToolExecutor.
/// This allows the LLM to delegate tasks through the standard tool interface.
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
        TASK_TOOL_DESCRIPTION
    }

    fn parameters(&self) -> serde_json::Value {
        task_params_schema_for_agents(&self.executor.visible_agents())
    }

    fn definition(&self) -> ToolDefinition {
        let agents = self.executor.visible_agents();
        ToolDefinition {
            name: self.name().to_string(),
            description: delegation_tool_description(self.description(), &agents),
            parameters: task_params_schema_for_agents(&agents),
        }
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let params: TaskParams =
            serde_json::from_value(args.clone()).context("Invalid task parameters")?;
        let parent_cancellation = ctx.cancellation_token();
        let executor = self.executor.scoped_for_invocation(ctx);

        if params.background {
            let task_id = executor.execute_background_with_parent_cancellation(
                params,
                ctx.agent_event_tx.clone(),
                ctx.session_id.clone(),
                Some(parent_cancellation),
            );
            return Ok(ToolOutput::success(format!(
                "Task started in background. Task ID: {}",
                task_id
            )));
        }

        let result = executor
            .execute_with_parent_cancellation(
                params,
                ctx.agent_event_tx.clone(),
                ctx.session_id.as_deref(),
                Some(&parent_cancellation),
            )
            .await?;
        let (content, truncated) = format_task_result_for_context(&result);
        let metadata = serde_json::json!({
            "task_id": result.task_id,
            "session_id": result.session_id,
            "agent": result.agent,
            "success": result.success,
            "output_bytes": result.output.len(),
            "truncated_for_context": truncated,
            "artifact_id": task_artifact_id(&result),
            "artifact_uri": task_artifact_uri(&result),
            "structured": result.structured,
            "source_anchors": result.source_anchors,
        });

        if result.success {
            Ok(ToolOutput::success(content).with_metadata(metadata))
        } else {
            Ok(ToolOutput::error(content).with_metadata(metadata))
        }
    }
}

mod parallel_params;
pub use parallel_params::{parallel_task_params_schema, ParallelTaskParams};

/// ParallelTaskTool allows the LLM to fan out multiple delegated tasks concurrently.
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
        PARALLEL_TASK_TOOL_DESCRIPTION
    }

    fn parameters(&self) -> serde_json::Value {
        parallel_params::parallel_task_params_schema_for_agents(&self.executor.visible_agents())
    }

    fn definition(&self) -> ToolDefinition {
        let agents = self.executor.visible_agents();
        ToolDefinition {
            name: self.name().to_string(),
            description: delegation_tool_description(self.description(), &agents),
            parameters: parallel_params::parallel_task_params_schema_for_agents(&agents),
        }
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let started_at = std::time::Instant::now();
        let params: ParallelTaskParams = match serde_json::from_value(args.clone()) {
            Ok(params) => params,
            Err(error) => {
                return Ok(invalid_parallel_task_argument(format!(
                    "Invalid parallel_task parameters: {error}"
                )));
            }
        };
        let parent_cancellation = ctx.cancellation_token();
        let executor = self.executor.scoped_for_invocation(ctx);

        if params.tasks.len() < 2 {
            return Ok(invalid_parallel_task_argument(
                "parallel_task requires at least 2 independent tasks".to_string(),
            ));
        }
        if params.tasks.len() > MAX_PARALLEL_TASKS_PER_CALL {
            return Ok(invalid_parallel_task_argument(format!(
                "parallel_task accepts at most {MAX_PARALLEL_TASKS_PER_CALL} tasks"
            )));
        }
        if let Some((index, _)) = params
            .tasks
            .iter()
            .enumerate()
            .find(|(_, task)| task.background)
        {
            return Ok(invalid_parallel_task_argument(format!(
                "parallel_task task {} cannot set background=true; every branch is already executed concurrently and collected by the parent call",
                index + 1
            )));
        }
        if params.timeout_ms == Some(0) {
            return Ok(invalid_parallel_task_argument(
                "parallel_task timeout_ms must be at least 1".to_string(),
            ));
        }
        if let Some(min_success_count) = params.min_success_count {
            if !params.allow_partial_failure {
                return Ok(invalid_parallel_task_argument(
                    "parallel_task min_success_count requires allow_partial_failure=true"
                        .to_string(),
                ));
            }
            if min_success_count == 0 || min_success_count > params.tasks.len() {
                return Ok(invalid_parallel_task_argument(format!(
                    "parallel_task min_success_count must be between 1 and the task count ({})",
                    params.tasks.len()
                )));
            }
        }

        let task_count = params.tasks.len();
        let run = executor
            .execute_parallel_for_tool(
                params.tasks.clone(),
                ctx.agent_event_tx.clone(),
                parallel_execution::ParallelToolOptions {
                    parent_session_id: ctx.session_id.as_deref(),
                    timeout_ms: params.timeout_ms,
                    min_success_count: params.min_success_count,
                    allow_partial_failure: params.allow_partial_failure,
                    parent_cancellation: Some(&parent_cancellation),
                },
            )
            .await;
        let results = run.results;

        // Format results with compact per-task excerpts for parent context.
        let mut output = format!("Executed {} tasks in parallel:\n\n", task_count);
        let mut metadata_results = Vec::new();
        let source_anchor_counts = parallel_source_anchor_counts(&results);
        for (i, result) in results.iter().enumerate() {
            let status = if result.success { "[OK]" } else { "[ERR]" };
            let (formatted, truncated) = format_task_result_for_context(result);
            let (output_excerpt, _) = compact_task_output(&result.output);
            let source_anchors = &result.source_anchors[..source_anchor_counts[i]];
            metadata_results.push(serde_json::json!({
                "task_id": result.task_id,
                "session_id": result.session_id,
                "agent": result.agent,
                "success": result.success,
                "error_message": (!result.success).then(|| {
                    crate::text::truncate_utf8(&result.output, 1024).to_string()
                }),
                "output_excerpt": output_excerpt,
                "structured": result.structured,
                "source_anchors": source_anchors,
                "output_bytes": result.output.len(),
                "truncated_for_context": truncated,
                "artifact_id": task_artifact_id(result),
                "artifact_uri": task_artifact_uri(result),
            }));
            output.push_str(&format!(
                "--- Task {} ({}) {} ---\n{}\n\n",
                i + 1,
                result.agent,
                status,
                formatted
            ));
        }

        let success_count = results.iter().filter(|result| result.success).count();
        let failed_count = results.len().saturating_sub(success_count);
        let all_success = failed_count == 0;
        let partial_failure = failed_count > 0 && success_count > 0;
        if params.allow_partial_failure && partial_failure {
            output.push_str(&format!(
                "Partial failure tolerated: {success_count} succeeded, {failed_count} failed.\n"
            ));
        }
        if run.timed_out {
            output.push_str(&format!(
                "Parallel task timed out after {} ms; returned completed child results and marked unfinished children failed.\n",
                run.timeout_ms.unwrap_or_default()
            ));
        } else if run.returned_early {
            output.push_str(&format!(
                "Parallel task returned after reaching min_success_count={}; unfinished children were marked failed.\n",
                run.min_success_count.unwrap_or_default()
            ));
        }

        let tool_success = all_success || (params.allow_partial_failure && success_count > 0);
        let mut output = if tool_success {
            ToolOutput::success(output)
        } else {
            ToolOutput::error(output)
        };
        if !tool_success && failed_count > 0 {
            output.error_kind = Some(crate::tools::ToolErrorKind::PartialFailure {
                failed: failed_count,
                total: results.len(),
            });
        }

        Ok(output.with_metadata(serde_json::json!({
            "task_count": task_count,
            "result_count": results.len(),
            "success_count": success_count,
            "failed_count": failed_count,
            "all_success": all_success,
            "partial_failure": partial_failure,
            "allow_partial_failure": params.allow_partial_failure,
            "timeout_ms": params.timeout_ms,
            "timed_out": run.timed_out,
            "min_success_count": params.min_success_count,
            "returned_early": run.returned_early,
            "duration_ms": started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            "results": metadata_results,
        })))
    }
}

fn invalid_parallel_task_argument(message: String) -> ToolOutput {
    ToolOutput::error(&message)
        .with_error_kind(crate::tools::ToolErrorKind::InvalidArgument { message })
}

#[cfg(test)]
mod tests;
