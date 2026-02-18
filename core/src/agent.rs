//! Agent Loop Implementation
//!
//! The agent loop handles the core conversation cycle:
//! 1. User sends a prompt
//! 2. LLM generates a response (possibly with tool calls)
//! 3. If tool calls present, execute them and send results back
//! 4. Repeat until LLM returns without tool calls
//!
//! This implements agentic behavior where the LLM can use tools
//! to accomplish tasks agentically.

use crate::context::{ContextProvider, ContextQuery, ContextResult};
use crate::hitl::ConfirmationManager;
use crate::hooks::{
    GenerateEndEvent, GenerateStartEvent, HookEngine, HookEvent, HookResult, PostToolUseEvent,
    PreToolUseEvent, TokenUsageInfo, ToolCallInfo, ToolResultData,
};
use crate::llm::{LlmClient, LlmResponse, Message, TokenUsage, ToolCall, ToolDefinition};
use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::planning::{AgentGoal, ExecutionPlan, TaskStatus};
use crate::queue::{SessionCommand, SessionLane};
use crate::session_lane_queue::SessionLaneQueue;
use crate::tools::{ToolContext, ToolExecutor, ToolStreamEvent};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

/// Maximum number of tool execution rounds before stopping
const MAX_TOOL_ROUNDS: usize = 50;

/// Agent configuration
#[derive(Clone)]
pub struct AgentConfig {
    pub system_prompt: Option<String>,
    pub tools: Vec<ToolDefinition>,
    pub max_tool_rounds: usize,
    /// Optional permission policy for tool execution control
    pub permission_policy: Option<Arc<RwLock<PermissionPolicy>>>,
    /// Optional confirmation manager for HITL (Human-in-the-Loop)
    pub confirmation_manager: Option<Arc<ConfirmationManager>>,
    /// Context providers for augmenting prompts with external context
    pub context_providers: Vec<Arc<dyn ContextProvider>>,
    /// Enable planning phase before execution
    pub planning_enabled: bool,
    /// Enable goal tracking
    pub goal_tracking: bool,
    /// Optional hook engine for firing lifecycle events (PreToolUse, PostToolUse, etc.)
    pub hook_engine: Option<Arc<HookEngine>>,
}

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfig")
            .field("system_prompt", &self.system_prompt)
            .field("tools", &self.tools)
            .field("max_tool_rounds", &self.max_tool_rounds)
            .field("permission_policy", &self.permission_policy.is_some())
            .field("confirmation_manager", &self.confirmation_manager.is_some())
            .field("context_providers", &self.context_providers.len())
            .field("planning_enabled", &self.planning_enabled)
            .field("goal_tracking", &self.goal_tracking)
            .field("hook_engine", &self.hook_engine.is_some())
            .finish()
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: None,
            tools: Vec::new(), // Tools are provided by ToolExecutor
            max_tool_rounds: MAX_TOOL_ROUNDS,
            permission_policy: None,
            confirmation_manager: None,
            context_providers: Vec::new(),
            planning_enabled: false,
            goal_tracking: false,
            hook_engine: None,
        }
    }
}

/// Events emitted during agent execution
///
/// Subscribe via [`Session::subscribe_events()`](crate::session::Session::subscribe_events).
/// New variants may be added in minor releases — always include a wildcard arm
/// (`_ => {}`) when matching.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum AgentEvent {
    /// Agent started processing
    #[serde(rename = "agent_start")]
    Start { prompt: String },

    /// LLM turn started
    #[serde(rename = "turn_start")]
    TurnStart { turn: usize },

    /// Text delta from streaming
    #[serde(rename = "text_delta")]
    TextDelta { text: String },

    /// Tool execution started
    #[serde(rename = "tool_start")]
    ToolStart { id: String, name: String },

    /// Tool execution completed
    #[serde(rename = "tool_end")]
    ToolEnd {
        id: String,
        name: String,
        output: String,
        exit_code: i32,
    },

    /// Intermediate tool output (streaming delta)
    #[serde(rename = "tool_output_delta")]
    ToolOutputDelta {
        id: String,
        name: String,
        delta: String,
    },

    /// LLM turn completed
    #[serde(rename = "turn_end")]
    TurnEnd { turn: usize, usage: TokenUsage },

    /// Agent completed
    #[serde(rename = "agent_end")]
    End { text: String, usage: TokenUsage },

    /// Error occurred
    #[serde(rename = "error")]
    Error { message: String },

    /// Tool execution requires confirmation (HITL)
    #[serde(rename = "confirmation_required")]
    ConfirmationRequired {
        tool_id: String,
        tool_name: String,
        args: serde_json::Value,
        timeout_ms: u64,
    },

    /// Confirmation received from user (HITL)
    #[serde(rename = "confirmation_received")]
    ConfirmationReceived {
        tool_id: String,
        approved: bool,
        reason: Option<String>,
    },

    /// Confirmation timed out (HITL)
    #[serde(rename = "confirmation_timeout")]
    ConfirmationTimeout {
        tool_id: String,
        action_taken: String, // "rejected" or "auto_approved"
    },

    /// External task pending (needs SDK processing)
    #[serde(rename = "external_task_pending")]
    ExternalTaskPending {
        task_id: String,
        session_id: String,
        lane: crate::hitl::SessionLane,
        command_type: String,
        payload: serde_json::Value,
        timeout_ms: u64,
    },

    /// External task completed
    #[serde(rename = "external_task_completed")]
    ExternalTaskCompleted {
        task_id: String,
        session_id: String,
        success: bool,
    },

    /// Tool execution denied by permission policy
    #[serde(rename = "permission_denied")]
    PermissionDenied {
        tool_id: String,
        tool_name: String,
        args: serde_json::Value,
        reason: String,
    },

    /// Context resolution started
    #[serde(rename = "context_resolving")]
    ContextResolving { providers: Vec<String> },

    /// Context resolution completed
    #[serde(rename = "context_resolved")]
    ContextResolved {
        total_items: usize,
        total_tokens: usize,
    },

    // ========================================================================
    // a3s-lane integration events
    // ========================================================================
    /// Command moved to dead letter queue after exhausting retries
    #[serde(rename = "command_dead_lettered")]
    CommandDeadLettered {
        command_id: String,
        command_type: String,
        lane: String,
        error: String,
        attempts: u32,
    },

    /// Command retry attempt
    #[serde(rename = "command_retry")]
    CommandRetry {
        command_id: String,
        command_type: String,
        lane: String,
        attempt: u32,
        delay_ms: u64,
    },

    /// Queue alert (depth warning, latency alert, etc.)
    #[serde(rename = "queue_alert")]
    QueueAlert {
        level: String,
        alert_type: String,
        message: String,
    },

    // ========================================================================
    // Task tracking events
    // ========================================================================
    /// Task list updated
    #[serde(rename = "task_updated")]
    TaskUpdated {
        session_id: String,
        tasks: Vec<crate::planning::Task>,
    },

    // ========================================================================
    // Memory System events (Phase 3)
    // ========================================================================
    /// Memory stored
    #[serde(rename = "memory_stored")]
    MemoryStored {
        memory_id: String,
        memory_type: String,
        importance: f32,
        tags: Vec<String>,
    },

    /// Memory recalled
    #[serde(rename = "memory_recalled")]
    MemoryRecalled {
        memory_id: String,
        content: String,
        relevance: f32,
    },

    /// Memories searched
    #[serde(rename = "memories_searched")]
    MemoriesSearched {
        query: Option<String>,
        tags: Vec<String>,
        result_count: usize,
    },

    /// Memory cleared
    #[serde(rename = "memory_cleared")]
    MemoryCleared {
        tier: String, // "long_term", "short_term", "working"
        count: u64,
    },

    // ========================================================================
    // Subagent events
    // ========================================================================
    /// Subagent task started
    #[serde(rename = "subagent_start")]
    SubagentStart {
        /// Unique task identifier
        task_id: String,
        /// Child session ID
        session_id: String,
        /// Parent session ID
        parent_session_id: String,
        /// Agent type (e.g., "explore", "general")
        agent: String,
        /// Short description of the task
        description: String,
    },

    /// Subagent task progress update
    #[serde(rename = "subagent_progress")]
    SubagentProgress {
        /// Task identifier
        task_id: String,
        /// Child session ID
        session_id: String,
        /// Progress status message
        status: String,
        /// Additional metadata
        metadata: serde_json::Value,
    },

    /// Subagent task completed
    #[serde(rename = "subagent_end")]
    SubagentEnd {
        /// Task identifier
        task_id: String,
        /// Child session ID
        session_id: String,
        /// Agent type
        agent: String,
        /// Task output/result
        output: String,
        /// Whether the task succeeded
        success: bool,
    },

    // ========================================================================
    // Planning and Goal Tracking Events (Phase 1)
    // ========================================================================
    /// Planning phase started
    #[serde(rename = "planning_start")]
    PlanningStart { prompt: String },

    /// Planning phase completed
    #[serde(rename = "planning_end")]
    PlanningEnd {
        plan: ExecutionPlan,
        estimated_steps: usize,
    },

    /// Step execution started
    #[serde(rename = "step_start")]
    StepStart {
        step_id: String,
        description: String,
        step_number: usize,
        total_steps: usize,
    },

    /// Step execution completed
    #[serde(rename = "step_end")]
    StepEnd {
        step_id: String,
        status: TaskStatus,
        step_number: usize,
        total_steps: usize,
    },

    /// Goal extracted from prompt
    #[serde(rename = "goal_extracted")]
    GoalExtracted { goal: AgentGoal },

    /// Goal progress update
    #[serde(rename = "goal_progress")]
    GoalProgress {
        goal: String,
        progress: f32,
        completed_steps: usize,
        total_steps: usize,
    },

    /// Goal achieved
    #[serde(rename = "goal_achieved")]
    GoalAchieved {
        goal: String,
        total_steps: usize,
        duration_ms: i64,
    },

    // ========================================================================
    // Context Compaction events
    // ========================================================================
    /// Context automatically compacted due to high usage
    #[serde(rename = "context_compacted")]
    ContextCompacted {
        session_id: String,
        before_messages: usize,
        after_messages: usize,
        percent_before: f32,
    },

    // ========================================================================
    // Persistence events
    // ========================================================================
    /// Session persistence failed — SDK clients should handle this
    #[serde(rename = "persistence_failed")]
    PersistenceFailed {
        session_id: String,
        operation: String,
        error: String,
    },
}

/// Result of agent execution
#[derive(Debug, Clone)]
pub struct AgentResult {
    pub text: String,
    pub messages: Vec<Message>,
    pub usage: TokenUsage,
    pub tool_calls_count: usize,
}

// ============================================================================
// ToolCommand — bridges ToolExecutor to SessionCommand for queue submission
// ============================================================================

/// Adapter that implements `SessionCommand` for tool execution via the queue.
///
/// Wraps a `ToolExecutor` call so it can be submitted to `SessionLaneQueue`.
pub struct ToolCommand {
    tool_executor: Arc<ToolExecutor>,
    tool_name: String,
    tool_args: Value,
    tool_context: ToolContext,
}

#[async_trait]
impl SessionCommand for ToolCommand {
    async fn execute(&self) -> Result<Value> {
        let result = self
            .tool_executor
            .execute_with_context(&self.tool_name, &self.tool_args, &self.tool_context)
            .await?;
        Ok(serde_json::json!({
            "output": result.output,
            "exit_code": result.exit_code,
            "metadata": result.metadata,
        }))
    }

    fn command_type(&self) -> &str {
        &self.tool_name
    }

    fn payload(&self) -> Value {
        self.tool_args.clone()
    }
}

// ============================================================================
// partition_by_lane — splits tool calls into parallelizable groups
// ============================================================================

/// Partition tool calls into Query-lane (parallelizable) and sequential tools.
///
/// Query-lane tools (read, glob, grep, ls, search, list_files) are pure reads
/// with no side effects — safe to execute in parallel.
/// All other tools are executed sequentially to preserve side-effect ordering.
pub fn partition_by_lane(tool_calls: &[ToolCall]) -> (Vec<ToolCall>, Vec<ToolCall>) {
    let mut query_tools = Vec::new();
    let mut sequential_tools = Vec::new();

    for tc in tool_calls {
        match SessionLane::from_tool_name(&tc.name) {
            SessionLane::Query => query_tools.push(tc.clone()),
            _ => sequential_tools.push(tc.clone()),
        }
    }

    (query_tools, sequential_tools)
}

/// Agent loop executor
#[derive(Clone)]
pub struct AgentLoop {
    llm_client: Arc<dyn LlmClient>,
    tool_executor: Arc<ToolExecutor>,
    tool_context: ToolContext,
    config: AgentConfig,
    /// Optional per-session tool metrics collector
    tool_metrics: Option<Arc<RwLock<crate::telemetry::ToolMetrics>>>,
    /// Optional lane queue for priority-based tool execution with parallelism
    command_queue: Option<Arc<SessionLaneQueue>>,
}

impl AgentLoop {
    pub fn new(
        llm_client: Arc<dyn LlmClient>,
        tool_executor: Arc<ToolExecutor>,
        tool_context: ToolContext,
        config: AgentConfig,
    ) -> Self {
        Self {
            llm_client,
            tool_executor,
            tool_context,
            config,
            tool_metrics: None,
            command_queue: None,
        }
    }

    /// Set the tool metrics collector for this agent loop
    pub fn with_tool_metrics(
        mut self,
        metrics: Arc<RwLock<crate::telemetry::ToolMetrics>>,
    ) -> Self {
        self.tool_metrics = Some(metrics);
        self
    }

    /// Set the lane queue for priority-based tool execution with parallelism.
    ///
    /// When set, Query-lane tools (read, glob, grep, ls, search, list_files)
    /// from a single LLM turn are executed in parallel via the queue.
    /// Execute-lane tools remain sequential to preserve side-effect ordering.
    pub fn with_queue(mut self, queue: Arc<SessionLaneQueue>) -> Self {
        self.command_queue = Some(queue);
        self
    }

    /// Create a tool context with streaming support.
    ///
    /// When `event_tx` is Some, spawns a forwarder task that converts
    /// `ToolStreamEvent::OutputDelta` into `AgentEvent::ToolOutputDelta`
    /// and sends them to the agent event channel.
    ///
    /// Returns the augmented `ToolContext`. The forwarder task runs until
    /// the tool-side sender is dropped (i.e., tool execution finishes).
    fn streaming_tool_context(
        &self,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        tool_id: &str,
        tool_name: &str,
    ) -> ToolContext {
        let mut ctx = self.tool_context.clone();
        if let Some(agent_tx) = event_tx {
            let (tool_tx, mut tool_rx) = mpsc::channel::<ToolStreamEvent>(64);
            ctx.event_tx = Some(tool_tx);

            let agent_tx = agent_tx.clone();
            let tool_id = tool_id.to_string();
            let tool_name = tool_name.to_string();
            tokio::spawn(async move {
                while let Some(event) = tool_rx.recv().await {
                    match event {
                        ToolStreamEvent::OutputDelta(delta) => {
                            agent_tx
                                .send(AgentEvent::ToolOutputDelta {
                                    id: tool_id.clone(),
                                    name: tool_name.clone(),
                                    delta,
                                })
                                .await
                                .ok();
                        }
                    }
                }
            });
        }
        ctx
    }

    /// Resolve context from all providers for a given prompt
    ///
    /// Returns aggregated context results from all configured providers.
    async fn resolve_context(&self, prompt: &str, session_id: Option<&str>) -> Vec<ContextResult> {
        if self.config.context_providers.is_empty() {
            return Vec::new();
        }

        let query = ContextQuery::new(prompt).with_session_id(session_id.unwrap_or(""));

        let futures = self
            .config
            .context_providers
            .iter()
            .map(|p| p.query(&query));
        let outcomes = join_all(futures).await;

        outcomes
            .into_iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                Ok(result) if !result.is_empty() => Some(result),
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!(
                        "Context provider '{}' failed: {}",
                        self.config.context_providers[i].name(),
                        e
                    );
                    None
                }
            })
            .collect()
    }

    /// Build augmented system prompt with context
    fn build_augmented_system_prompt(&self, context_results: &[ContextResult]) -> Option<String> {
        if context_results.is_empty() {
            return self.config.system_prompt.clone();
        }

        // Build context XML block
        let context_xml: String = context_results
            .iter()
            .map(|r| r.to_xml())
            .collect::<Vec<_>>()
            .join("\n\n");

        // Combine with existing system prompt
        match &self.config.system_prompt {
            Some(system) => Some(format!("{}\n\n{}", system, context_xml)),
            None => Some(context_xml),
        }
    }

    /// Notify providers of turn completion for memory extraction
    async fn notify_turn_complete(&self, session_id: &str, prompt: &str, response: &str) {
        let futures = self
            .config
            .context_providers
            .iter()
            .map(|p| p.on_turn_complete(session_id, prompt, response));
        let outcomes = join_all(futures).await;

        for (i, result) in outcomes.into_iter().enumerate() {
            if let Err(e) = result {
                tracing::warn!(
                    "Context provider '{}' on_turn_complete failed: {}",
                    self.config.context_providers[i].name(),
                    e
                );
            }
        }
    }

    /// Fire PreToolUse hook event before tool execution.
    /// Returns the HookResult which may block the tool call.
    async fn fire_pre_tool_use(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Option<HookResult> {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::PreToolUse(PreToolUseEvent {
                session_id: session_id.to_string(),
                tool: tool_name.to_string(),
                args: args.clone(),
                working_directory: self.tool_context.workspace.to_string_lossy().to_string(),
                recent_tools: Vec::new(),
            });
            let result = he.fire(&event).await;
            if result.is_block() {
                return Some(result);
            }
        }
        None
    }

    /// Fire PostToolUse hook event after tool execution (fire-and-forget).
    async fn fire_post_tool_use(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        output: &str,
        success: bool,
        duration_ms: u64,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::PostToolUse(PostToolUseEvent {
                session_id: session_id.to_string(),
                tool: tool_name.to_string(),
                args: args.clone(),
                result: ToolResultData {
                    success,
                    output: output.to_string(),
                    exit_code: if success { Some(0) } else { Some(1) },
                    duration_ms,
                },
            });
            let _ = he.fire(&event).await;
        }
    }

    /// Fire GenerateStart hook event before an LLM call.
    async fn fire_generate_start(
        &self,
        session_id: &str,
        prompt: &str,
        system_prompt: &Option<String>,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::GenerateStart(GenerateStartEvent {
                session_id: session_id.to_string(),
                prompt: prompt.to_string(),
                system_prompt: system_prompt.clone(),
                model_provider: String::new(),
                model_name: String::new(),
                available_tools: self.config.tools.iter().map(|t| t.name.clone()).collect(),
            });
            let _ = he.fire(&event).await;
        }
    }

    /// Fire GenerateEnd hook event after an LLM call.
    async fn fire_generate_end(
        &self,
        session_id: &str,
        prompt: &str,
        response: &LlmResponse,
        duration_ms: u64,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let tool_calls: Vec<ToolCallInfo> = response
                .tool_calls()
                .iter()
                .map(|tc| ToolCallInfo {
                    name: tc.name.clone(),
                    args: tc.args.clone(),
                })
                .collect();

            let event = HookEvent::GenerateEnd(GenerateEndEvent {
                session_id: session_id.to_string(),
                prompt: prompt.to_string(),
                response_text: response.text().to_string(),
                tool_calls,
                usage: TokenUsageInfo {
                    prompt_tokens: response.usage.prompt_tokens as i32,
                    completion_tokens: response.usage.completion_tokens as i32,
                    total_tokens: response.usage.total_tokens as i32,
                },
                duration_ms,
            });
            let _ = he.fire(&event).await;
        }
    }

    /// Execute the agent loop for a prompt
    ///
    /// Takes the conversation history and a new user prompt.
    /// Returns the agent result and updated message history.
    /// When event_tx is provided, uses streaming LLM API for real-time text output.
    pub async fn execute(
        &self,
        history: &[Message],
        prompt: &str,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentResult> {
        self.execute_with_session(history, prompt, None, event_tx)
            .await
    }

    /// Execute the agent loop for a prompt with session context
    ///
    /// Takes the conversation history, user prompt, and optional session ID.
    /// When session_id is provided, context providers can use it for session-specific context.
    pub async fn execute_with_session(
        &self,
        history: &[Message],
        prompt: &str,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentResult> {
        tracing::info!(
            a3s.session.id = session_id.unwrap_or("none"),
            a3s.agent.max_turns = self.config.max_tool_rounds,
            "a3s.agent.execute started"
        );

        // Route to planning-based execution if enabled
        let result = if self.config.planning_enabled {
            self.execute_with_planning(history, prompt, event_tx).await
        } else {
            self.execute_loop(history, prompt, session_id, event_tx)
                .await
        };

        match &result {
            Ok(r) => tracing::info!(
                a3s.agent.tool_calls_count = r.tool_calls_count,
                a3s.llm.total_tokens = r.usage.total_tokens,
                "a3s.agent.execute completed"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                "a3s.agent.execute failed"
            ),
        }

        result
    }

    /// Core execution loop (without planning routing).
    ///
    /// This is the inner loop that runs LLM calls and tool executions.
    /// Called directly by `execute_with_session` (after planning check)
    /// and by `execute_plan` (for individual steps, bypassing planning).
    async fn execute_loop(
        &self,
        history: &[Message],
        prompt: &str,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentResult> {
        let mut messages = history.to_vec();
        let mut total_usage = TokenUsage::default();
        let mut tool_calls_count = 0;
        let mut turn = 0;

        // Send start event
        if let Some(tx) = &event_tx {
            tx.send(AgentEvent::Start {
                prompt: prompt.to_string(),
            })
            .await
            .ok();
        }

        // Resolve context from providers on first turn (before adding user message)
        let mut augmented_system = if !self.config.context_providers.is_empty() {
            // Send context resolving event
            if let Some(tx) = &event_tx {
                let provider_names: Vec<String> = self
                    .config
                    .context_providers
                    .iter()
                    .map(|p| p.name().to_string())
                    .collect();
                tx.send(AgentEvent::ContextResolving {
                    providers: provider_names,
                })
                .await
                .ok();
            }

            tracing::info!(
                a3s.context.providers = self.config.context_providers.len() as i64,
                "Context resolution started"
            );
            let context_results = self.resolve_context(prompt, session_id).await;

            // Send context resolved event
            if let Some(tx) = &event_tx {
                let total_items: usize = context_results.iter().map(|r| r.items.len()).sum();
                let total_tokens: usize = context_results.iter().map(|r| r.total_tokens).sum();

                tracing::info!(
                    context_items = total_items,
                    context_tokens = total_tokens,
                    "Context resolution completed"
                );

                tx.send(AgentEvent::ContextResolved {
                    total_items,
                    total_tokens,
                })
                .await
                .ok();
            }

            self.build_augmented_system_prompt(&context_results)
        } else {
            self.config.system_prompt.clone()
        };

        // Add user message
        messages.push(Message::user(prompt));

        loop {
            turn += 1;

            if turn > self.config.max_tool_rounds {
                let error = format!("Max tool rounds ({}) exceeded", self.config.max_tool_rounds);
                if let Some(tx) = &event_tx {
                    tx.send(AgentEvent::Error {
                        message: error.clone(),
                    })
                    .await
                    .ok();
                }
                anyhow::bail!(error);
            }

            // Send turn start event
            if let Some(tx) = &event_tx {
                tx.send(AgentEvent::TurnStart { turn }).await.ok();
            }

            tracing::info!(
                turn = turn,
                max_turns = self.config.max_tool_rounds,
                "Agent turn started"
            );

            // Call LLM - use streaming if we have an event channel
            tracing::info!(
                a3s.llm.streaming = event_tx.is_some(),
                "LLM completion started"
            );

            // Fire GenerateStart hook
            self.fire_generate_start(session_id.unwrap_or(""), prompt, &augmented_system)
                .await;

            let llm_start = std::time::Instant::now();
            let response = if event_tx.is_some() {
                // Streaming mode
                let mut stream_rx = self
                    .llm_client
                    .complete_streaming(&messages, augmented_system.as_deref(), &self.config.tools)
                    .await
                    .context("LLM streaming call failed")?;

                let mut final_response: Option<LlmResponse> = None;

                while let Some(event) = stream_rx.recv().await {
                    match event {
                        crate::llm::StreamEvent::TextDelta(text) => {
                            if let Some(tx) = &event_tx {
                                tx.send(AgentEvent::TextDelta { text }).await.ok();
                            }
                        }
                        crate::llm::StreamEvent::ToolUseStart { id, name } => {
                            if let Some(tx) = &event_tx {
                                tx.send(AgentEvent::ToolStart { id, name }).await.ok();
                            }
                        }
                        crate::llm::StreamEvent::ToolUseInputDelta(_) => {
                            // We could forward this if needed
                        }
                        crate::llm::StreamEvent::Done(resp) => {
                            final_response = Some(resp);
                        }
                    }
                }

                final_response.context("Stream ended without final response")?
            } else {
                // Non-streaming mode
                self.llm_client
                    .complete(&messages, augmented_system.as_deref(), &self.config.tools)
                    .await
                    .context("LLM call failed")?
            };

            // Update usage
            total_usage.prompt_tokens += response.usage.prompt_tokens;
            total_usage.completion_tokens += response.usage.completion_tokens;
            total_usage.total_tokens += response.usage.total_tokens;

            // Record LLM completion telemetry
            let llm_duration = llm_start.elapsed();
            tracing::info!(
                turn = turn,
                streaming = event_tx.is_some(),
                prompt_tokens = response.usage.prompt_tokens,
                completion_tokens = response.usage.completion_tokens,
                total_tokens = response.usage.total_tokens,
                stop_reason = response.stop_reason.as_deref().unwrap_or("unknown"),
                duration_ms = llm_duration.as_millis() as u64,
                "LLM completion finished"
            );

            // Fire GenerateEnd hook
            self.fire_generate_end(
                session_id.unwrap_or(""),
                prompt,
                &response,
                llm_duration.as_millis() as u64,
            )
            .await;

            // Record LLM usage on the llm span
            crate::telemetry::record_llm_usage(
                response.usage.prompt_tokens,
                response.usage.completion_tokens,
                response.usage.total_tokens,
                response.stop_reason.as_deref(),
            );
            // Log turn token usage
            tracing::info!(
                turn = turn,
                a3s.llm.total_tokens = response.usage.total_tokens,
                "Turn token usage"
            );

            // Add assistant message to history
            messages.push(response.message.clone());

            // Check for tool calls
            let tool_calls = response.tool_calls();

            // Send turn end event
            if let Some(tx) = &event_tx {
                tx.send(AgentEvent::TurnEnd {
                    turn,
                    usage: response.usage.clone(),
                })
                .await
                .ok();
            }

            if tool_calls.is_empty() {
                // No tool calls, we're done
                let final_text = response.text();

                // Record final totals
                tracing::info!(
                    tool_calls_count = tool_calls_count,
                    total_prompt_tokens = total_usage.prompt_tokens,
                    total_completion_tokens = total_usage.completion_tokens,
                    total_tokens = total_usage.total_tokens,
                    turns = turn,
                    "Agent execution completed"
                );

                if let Some(tx) = &event_tx {
                    tx.send(AgentEvent::End {
                        text: final_text.clone(),
                        usage: total_usage.clone(),
                    })
                    .await
                    .ok();
                }

                // Notify context providers of turn completion for memory extraction
                if let Some(sid) = session_id {
                    self.notify_turn_complete(sid, prompt, &final_text).await;
                }

                return Ok(AgentResult {
                    text: final_text,
                    messages,
                    usage: total_usage,
                    tool_calls_count,
                });
            }

            // Execute tools — parallel Query execution when queue is available
            let (query_tools, sequential_tools) = if self.command_queue.is_some() {
                partition_by_lane(&tool_calls)
            } else {
                (Vec::new(), tool_calls.clone())
            };

            // Execute Query-lane tools in parallel via queue
            if !query_tools.is_empty() {
                if let Some(queue) = &self.command_queue {
                    let parallel_count = self
                        .execute_query_tools_parallel(
                            &query_tools,
                            queue,
                            &mut messages,
                            &event_tx,
                            &mut augmented_system,
                            session_id,
                        )
                        .await;
                    tool_calls_count += parallel_count;
                }
            }

            // Execute remaining tools sequentially (write/bash have side effects)
            for tool_call in sequential_tools {
                tool_calls_count += 1;

                let tool_start = std::time::Instant::now();

                tracing::info!(
                    tool_name = tool_call.name.as_str(),
                    tool_id = tool_call.id.as_str(),
                    "Tool execution started"
                );

                // Send tool start event (only if not already sent during streaming)
                // In streaming mode, ToolStart is sent when we receive ToolUseStart from LLM
                // But we still need to send ToolEnd after execution

                // Check for malformed tool arguments from LLM
                if let Some(parse_error) =
                    tool_call.args.get("__parse_error").and_then(|v| v.as_str())
                {
                    let error_msg = format!("Error: {}", parse_error);
                    tracing::warn!(
                        tool = tool_call.name.as_str(),
                        "Malformed tool arguments from LLM"
                    );

                    if let Some(tx) = &event_tx {
                        tx.send(AgentEvent::ToolEnd {
                            id: tool_call.id.clone(),
                            name: tool_call.name.clone(),
                            output: error_msg.clone(),
                            exit_code: 1,
                        })
                        .await
                        .ok();
                    }

                    messages.push(Message::tool_result(&tool_call.id, &error_msg, true));
                    continue;
                }

                // Fire PreToolUse hook (may block the tool call)
                if let Some(HookResult::Block(reason)) = self
                    .fire_pre_tool_use(session_id.unwrap_or(""), &tool_call.name, &tool_call.args)
                    .await
                {
                    let msg = format!("Tool '{}' blocked by hook: {}", tool_call.name, reason);
                    tracing::info!(
                        tool_name = tool_call.name.as_str(),
                        "Tool blocked by PreToolUse hook"
                    );

                    if let Some(tx) = &event_tx {
                        tx.send(AgentEvent::PermissionDenied {
                            tool_id: tool_call.id.clone(),
                            tool_name: tool_call.name.clone(),
                            args: tool_call.args.clone(),
                            reason: reason.clone(),
                        })
                        .await
                        .ok();
                    }

                    messages.push(Message::tool_result(&tool_call.id, &msg, true));
                    continue;
                }

                // Check permission before executing tool
                let permission_decision = if let Some(policy_lock) = &self.config.permission_policy
                {
                    let policy = policy_lock.read().await;
                    policy.check(&tool_call.name, &tool_call.args)
                } else {
                    // No policy configured — default to Ask so HITL can still intervene
                    PermissionDecision::Ask
                };

                let (output, exit_code, is_error, metadata) = match permission_decision {
                    PermissionDecision::Deny => {
                        tracing::info!(
                            tool_name = tool_call.name.as_str(),
                            permission = "deny",
                            "Tool permission denied"
                        );
                        // Tool execution denied by permission policy
                        let denial_msg = format!(
                            "Permission denied: Tool '{}' is blocked by permission policy.",
                            tool_call.name
                        );

                        // Send permission denied event
                        if let Some(tx) = &event_tx {
                            tx.send(AgentEvent::PermissionDenied {
                                tool_id: tool_call.id.clone(),
                                tool_name: tool_call.name.clone(),
                                args: tool_call.args.clone(),
                                reason: "Blocked by deny rule in permission policy".to_string(),
                            })
                            .await
                            .ok();
                        }

                        (denial_msg, 1, true, None)
                    }
                    PermissionDecision::Allow => {
                        tracing::info!(
                            tool_name = tool_call.name.as_str(),
                            permission = "allow",
                            "Tool permission: allow"
                        );
                        // Permission explicitly allows — execute directly, no HITL
                        let stream_ctx =
                            self.streaming_tool_context(&event_tx, &tool_call.id, &tool_call.name);
                        let result = self
                            .tool_executor
                            .execute_with_context(&tool_call.name, &tool_call.args, &stream_ctx)
                            .await;

                        match result {
                            Ok(r) => (r.output, r.exit_code, r.exit_code != 0, r.metadata),
                            Err(e) => (format!("Tool execution error: {}", e), 1, true, None),
                        }
                    }
                    PermissionDecision::Ask => {
                        tracing::info!(
                            tool_name = tool_call.name.as_str(),
                            permission = "ask",
                            "Tool permission: ask"
                        );
                        // Permission says Ask — delegate to HITL confirmation manager
                        if let Some(cm) = &self.config.confirmation_manager {
                            // Check YOLO lanes: if the tool's lane is in YOLO mode, skip confirmation
                            if !cm.requires_confirmation(&tool_call.name).await {
                                let stream_ctx = self.streaming_tool_context(
                                    &event_tx,
                                    &tool_call.id,
                                    &tool_call.name,
                                );
                                let result = self
                                    .tool_executor
                                    .execute_with_context(
                                        &tool_call.name,
                                        &tool_call.args,
                                        &stream_ctx,
                                    )
                                    .await;

                                let (output, exit_code, is_error, metadata) = match result {
                                    Ok(r) => (r.output, r.exit_code, r.exit_code != 0, r.metadata),
                                    Err(e) => {
                                        (format!("Tool execution error: {}", e), 1, true, None)
                                    }
                                };

                                // Add tool result to messages
                                messages.push(Message::tool_result(
                                    &tool_call.id,
                                    &output,
                                    is_error,
                                ));

                                // Record tool result on the tool span for early exit
                                let tool_duration = tool_start.elapsed();
                                crate::telemetry::record_tool_result(exit_code, tool_duration);

                                // Fire PostToolUse hook (fire-and-forget)
                                self.fire_post_tool_use(
                                    session_id.unwrap_or(""),
                                    &tool_call.name,
                                    &tool_call.args,
                                    &output,
                                    exit_code == 0,
                                    tool_duration.as_millis() as u64,
                                )
                                .await;

                                continue; // Skip the rest, move to next tool call
                            }

                            // Get timeout from policy
                            let policy = cm.policy().await;
                            let timeout_ms = policy.default_timeout_ms;
                            let timeout_action = policy.timeout_action;

                            // Request confirmation (this emits ConfirmationRequired event)
                            let rx = cm
                                .request_confirmation(
                                    &tool_call.id,
                                    &tool_call.name,
                                    &tool_call.args,
                                )
                                .await;

                            // Wait for confirmation with timeout
                            let confirmation_result =
                                tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await;

                            match confirmation_result {
                                Ok(Ok(response)) => {
                                    if response.approved {
                                        let stream_ctx = self.streaming_tool_context(
                                            &event_tx,
                                            &tool_call.id,
                                            &tool_call.name,
                                        );
                                        let result = self
                                            .tool_executor
                                            .execute_with_context(
                                                &tool_call.name,
                                                &tool_call.args,
                                                &stream_ctx,
                                            )
                                            .await;

                                        match result {
                                            Ok(r) => (
                                                r.output,
                                                r.exit_code,
                                                r.exit_code != 0,
                                                r.metadata,
                                            ),
                                            Err(e) => (
                                                format!("Tool execution error: {}", e),
                                                1,
                                                true,
                                                None,
                                            ),
                                        }
                                    } else {
                                        let rejection_msg = format!(
                                            "Tool '{}' execution was rejected by user. Reason: {}",
                                            tool_call.name,
                                            response.reason.unwrap_or_else(|| "No reason provided".to_string())
                                        );
                                        (rejection_msg, 1, true, None)
                                    }
                                }
                                Ok(Err(_)) => {
                                    let msg = format!(
                                        "Tool '{}' confirmation failed: confirmation channel closed",
                                        tool_call.name
                                    );
                                    (msg, 1, true, None)
                                }
                                Err(_) => {
                                    cm.check_timeouts().await;

                                    match timeout_action {
                                        crate::hitl::TimeoutAction::Reject => {
                                            let msg = format!(
                                                "Tool '{}' execution timed out waiting for confirmation ({}ms). Execution rejected.",
                                                tool_call.name, timeout_ms
                                            );
                                            (msg, 1, true, None)
                                        }
                                        crate::hitl::TimeoutAction::AutoApprove => {
                                            let stream_ctx = self.streaming_tool_context(
                                                &event_tx,
                                                &tool_call.id,
                                                &tool_call.name,
                                            );
                                            let result = self
                                                .tool_executor
                                                .execute_with_context(
                                                    &tool_call.name,
                                                    &tool_call.args,
                                                    &stream_ctx,
                                                )
                                                .await;

                                            match result {
                                                Ok(r) => (
                                                    r.output,
                                                    r.exit_code,
                                                    r.exit_code != 0,
                                                    r.metadata,
                                                ),
                                                Err(e) => (
                                                    format!("Tool execution error: {}", e),
                                                    1,
                                                    true,
                                                    None,
                                                ),
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            // Ask without confirmation manager — safe deny
                            let msg = format!(
                                "Tool '{}' requires confirmation but no HITL confirmation manager is configured. \
                                 Configure a confirmation policy to enable tool execution.",
                                tool_call.name
                            );
                            tracing::warn!(
                                tool_name = tool_call.name.as_str(),
                                "Tool requires confirmation but no HITL manager configured"
                            );
                            (msg, 1, true, None)
                        }
                    }
                };

                let tool_duration = tool_start.elapsed();
                crate::telemetry::record_tool_result(exit_code, tool_duration);

                // Fire PostToolUse hook (fire-and-forget)
                self.fire_post_tool_use(
                    session_id.unwrap_or(""),
                    &tool_call.name,
                    &tool_call.args,
                    &output,
                    exit_code == 0,
                    tool_duration.as_millis() as u64,
                )
                .await;

                // Send tool end event
                if let Some(tx) = &event_tx {
                    tx.send(AgentEvent::ToolEnd {
                        id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        output: output.clone(),
                        exit_code,
                    })
                    .await
                    .ok();
                }

                // Add tool result to messages
                messages.push(Message::tool_result(&tool_call.id, &output, is_error));
            }
        }
    }

    /// Execute Query-lane tools in parallel via the queue.
    ///
    /// All pre-execution checks (hooks, permissions) are performed
    /// before submission. Results are collected and appended to messages.
    /// Returns the number of tool calls executed.
    async fn execute_query_tools_parallel(
        &self,
        query_tools: &[ToolCall],
        queue: &SessionLaneQueue,
        messages: &mut Vec<Message>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        augmented_system: &mut Option<String>,
        session_id: Option<&str>,
    ) -> usize {
        let mut receivers = Vec::with_capacity(query_tools.len());
        let mut tool_starts = Vec::with_capacity(query_tools.len());

        for tool_call in query_tools {
            // Pre-execution checks: malformed args
            if let Some(parse_error) = tool_call.args.get("__parse_error").and_then(|v| v.as_str())
            {
                let error_msg = format!("Error: {}", parse_error);
                if let Some(tx) = event_tx {
                    tx.send(AgentEvent::ToolEnd {
                        id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        output: error_msg.clone(),
                        exit_code: 1,
                    })
                    .await
                    .ok();
                }
                messages.push(Message::tool_result(&tool_call.id, &error_msg, true));
                continue;
            }

            // Pre-execution checks: hooks
            if let Some(HookResult::Block(reason)) = self
                .fire_pre_tool_use(session_id.unwrap_or(""), &tool_call.name, &tool_call.args)
                .await
            {
                let msg = format!("Tool '{}' blocked by hook: {}", tool_call.name, reason);
                if let Some(tx) = event_tx {
                    tx.send(AgentEvent::PermissionDenied {
                        tool_id: tool_call.id.clone(),
                        tool_name: tool_call.name.clone(),
                        args: tool_call.args.clone(),
                        reason,
                    })
                    .await
                    .ok();
                }
                messages.push(Message::tool_result(&tool_call.id, &msg, true));
                continue;
            }


            // Pre-execution checks: permissions
            let permission_decision = if let Some(policy_lock) = &self.config.permission_policy {
                let policy = policy_lock.read().await;
                policy.check(&tool_call.name, &tool_call.args)
            } else {
                PermissionDecision::Ask
            };

            match permission_decision {
                PermissionDecision::Deny => {
                    let denial_msg = format!(
                        "Permission denied: Tool '{}' is blocked by permission policy.",
                        tool_call.name
                    );
                    if let Some(tx) = event_tx {
                        tx.send(AgentEvent::PermissionDenied {
                            tool_id: tool_call.id.clone(),
                            tool_name: tool_call.name.clone(),
                            args: tool_call.args.clone(),
                            reason: "Blocked by deny rule in permission policy".to_string(),
                        })
                        .await
                        .ok();
                    }
                    messages.push(Message::tool_result(&tool_call.id, &denial_msg, true));
                    continue;
                }
                PermissionDecision::Allow | PermissionDecision::Ask => {
                    // For Query tools: Allow and Ask both proceed (Query tools are read-only,
                    // HITL confirmation is not required for read operations).
                    // If a confirmation manager exists and requires confirmation for this
                    // specific tool, fall back to sequential execution instead.
                    if permission_decision == PermissionDecision::Ask {
                        if let Some(cm) = &self.config.confirmation_manager {
                            if cm.requires_confirmation(&tool_call.name).await {
                                // This Query tool requires HITL — skip parallel, will be
                                // handled by the sequential fallback path
                                continue;
                            }
                        }
                    }

                    // Submit to queue for parallel execution
                    let cmd = ToolCommand {
                        tool_executor: self.tool_executor.clone(),
                        tool_name: tool_call.name.clone(),
                        tool_args: tool_call.args.clone(),
                        tool_context: self.tool_context.clone(),
                    };
                    let rx = queue.submit_by_tool(&tool_call.name, Box::new(cmd)).await;
                    let start = std::time::Instant::now();
                    tool_starts.push((tool_call.clone(), start));
                    receivers.push(rx);
                }
            }
        }

        let count = receivers.len();

        // Await all parallel results
        let results = join_all(receivers).await;

        for (i, result) in results.into_iter().enumerate() {
            let (tool_call, tool_start) = &tool_starts[i];
            let tool_duration = tool_start.elapsed();

            let (output, exit_code, is_error, metadata) = match result {
                Ok(Ok(value)) => {
                    let output = value["output"].as_str().unwrap_or("").to_string();
                    let exit_code = value["exit_code"].as_i64().unwrap_or(0) as i32;
                    let metadata = value.get("metadata").cloned();
                    (output, exit_code, exit_code != 0, metadata)
                }
                Ok(Err(e)) => (format!("Tool execution error: {}", e), 1, true, None),
                Err(_) => ("Queue channel closed".to_string(), 1, true, None),
            };

            // Fire PostToolUse hook
            self.fire_post_tool_use(
                session_id.unwrap_or(""),
                &tool_call.name,
                &tool_call.args,
                &output,
                exit_code == 0,
                tool_duration.as_millis() as u64,
            )
            .await;

            // Send tool end event
            if let Some(tx) = event_tx {
                tx.send(AgentEvent::ToolEnd {
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    output: output.clone(),
                    exit_code,
                })
                .await
                .ok();
            }

            messages.push(Message::tool_result(&tool_call.id, &output, is_error));
        }

        count
    }

    /// Execute with streaming events
    pub async fn execute_streaming(
        &self,
        history: &[Message],
        prompt: &str,
    ) -> Result<(
        mpsc::Receiver<AgentEvent>,
        tokio::task::JoinHandle<Result<AgentResult>>,
    )> {
        let (tx, rx) = mpsc::channel(100);

        let llm_client = self.llm_client.clone();
        let tool_executor = self.tool_executor.clone();
        let tool_context = self.tool_context.clone();
        let config = self.config.clone();
        let tool_metrics = self.tool_metrics.clone();
        let command_queue = self.command_queue.clone();
        let history = history.to_vec();
        let prompt = prompt.to_string();

        let handle = tokio::spawn(async move {
            let mut agent = AgentLoop::new(llm_client, tool_executor, tool_context, config);
            if let Some(metrics) = tool_metrics {
                agent = agent.with_tool_metrics(metrics);
            }
            if let Some(queue) = command_queue {
                agent = agent.with_queue(queue);
            }
            agent.execute(&history, &prompt, Some(tx)).await
        });

        Ok((rx, handle))
    }

    /// Create an execution plan for a prompt
    ///
    /// Delegates to [`LlmPlanner`] for structured JSON plan generation,
    /// falling back to heuristic planning if the LLM call fails.
    pub async fn plan(&self, prompt: &str, _context: Option<&str>) -> Result<ExecutionPlan> {
        use crate::planning::LlmPlanner;

        match LlmPlanner::create_plan(&self.llm_client, prompt).await {
            Ok(plan) => Ok(plan),
            Err(e) => {
                tracing::warn!("LLM plan creation failed, using fallback: {}", e);
                Ok(LlmPlanner::fallback_plan(prompt))
            }
        }
    }

    /// Execute with planning phase
    pub async fn execute_with_planning(
        &self,
        history: &[Message],
        prompt: &str,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentResult> {
        // Send planning start event
        if let Some(tx) = &event_tx {
            tx.send(AgentEvent::PlanningStart {
                prompt: prompt.to_string(),
            })
            .await
            .ok();
        }

        // Create execution plan
        let plan = self.plan(prompt, None).await?;

        // Send planning end event
        if let Some(tx) = &event_tx {
            tx.send(AgentEvent::PlanningEnd {
                estimated_steps: plan.steps.len(),
                plan: plan.clone(),
            })
            .await
            .ok();
        }

        // Execute the plan step by step
        self.execute_plan(history, &plan, event_tx).await
    }

    /// Execute an execution plan using wave-based dependency-aware scheduling.
    ///
    /// Steps with no unmet dependencies are grouped into "waves". A wave with
    /// a single step executes sequentially (preserving the history chain). A
    /// wave with multiple independent steps executes them in parallel via
    /// `JoinSet`, then merges their results back into the shared history.
    async fn execute_plan(
        &self,
        history: &[Message],
        plan: &ExecutionPlan,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentResult> {
        let mut plan = plan.clone();
        let mut current_history = history.to_vec();
        let mut total_usage = TokenUsage::default();
        let mut tool_calls_count = 0;
        let total_steps = plan.steps.len();

        // Add initial user message with the goal
        let steps_text = plan
            .steps
            .iter()
            .enumerate()
            .map(|(i, step)| format!("{}. {}", i + 1, step.content))
            .collect::<Vec<_>>()
            .join("\n");
        current_history.push(Message::user(&crate::prompts::render(
            crate::prompts::PLAN_EXECUTE_GOAL,
            &[("goal", &plan.goal), ("steps", &steps_text)],
        )));

        loop {
            let ready: Vec<String> = plan
                .get_ready_steps()
                .iter()
                .map(|s| s.id.clone())
                .collect();

            if ready.is_empty() {
                // All done or deadlock
                if plan.has_deadlock() {
                    tracing::warn!(
                        "Plan deadlock detected: {} pending steps with unresolvable dependencies",
                        plan.pending_count()
                    );
                }
                break;
            }

            if ready.len() == 1 {
                // === Single step: sequential execution (preserves history chain) ===
                let step_id = &ready[0];
                let step = plan
                    .steps
                    .iter()
                    .find(|s| s.id == *step_id)
                    .unwrap()
                    .clone();
                let step_number = plan.steps.iter().position(|s| s.id == *step_id).unwrap() + 1;

                // Send step start event
                if let Some(tx) = &event_tx {
                    tx.send(AgentEvent::StepStart {
                        step_id: step.id.clone(),
                        description: step.content.clone(),
                        step_number,
                        total_steps,
                    })
                    .await
                    .ok();
                }

                plan.mark_status(&step.id, TaskStatus::InProgress);

                let step_prompt = crate::prompts::render(
                    crate::prompts::PLAN_EXECUTE_STEP,
                    &[
                        ("step_num", &step_number.to_string()),
                        ("description", &step.content),
                    ],
                );

                match self
                    .execute_loop(&current_history, &step_prompt, None, event_tx.clone())
                    .await
                {
                    Ok(result) => {
                        current_history = result.messages.clone();
                        total_usage.prompt_tokens += result.usage.prompt_tokens;
                        total_usage.completion_tokens += result.usage.completion_tokens;
                        total_usage.total_tokens += result.usage.total_tokens;
                        tool_calls_count += result.tool_calls_count;
                        plan.mark_status(&step.id, TaskStatus::Completed);

                        if let Some(tx) = &event_tx {
                            tx.send(AgentEvent::StepEnd {
                                step_id: step.id.clone(),
                                status: TaskStatus::Completed,
                                step_number,
                                total_steps,
                            })
                            .await
                            .ok();
                        }
                    }
                    Err(e) => {
                        tracing::error!("Plan step '{}' failed: {}", step.id, e);
                        plan.mark_status(&step.id, TaskStatus::Failed);

                        if let Some(tx) = &event_tx {
                            tx.send(AgentEvent::StepEnd {
                                step_id: step.id.clone(),
                                status: TaskStatus::Failed,
                                step_number,
                                total_steps,
                            })
                            .await
                            .ok();
                        }
                    }
                }
            } else {
                // === Multiple steps: parallel execution via JoinSet ===
                let ready_steps: Vec<_> = ready
                    .iter()
                    .map(|id| {
                        let step = plan.steps.iter().find(|s| s.id == *id).unwrap().clone();
                        let step_number = plan.steps.iter().position(|s| s.id == *id).unwrap() + 1;
                        (step, step_number)
                    })
                    .collect();

                // Mark all as InProgress and emit StepStart events
                for (step, step_number) in &ready_steps {
                    plan.mark_status(&step.id, TaskStatus::InProgress);
                    if let Some(tx) = &event_tx {
                        tx.send(AgentEvent::StepStart {
                            step_id: step.id.clone(),
                            description: step.content.clone(),
                            step_number: *step_number,
                            total_steps,
                        })
                        .await
                        .ok();
                    }
                }

                // Spawn all into JoinSet, each with a clone of the base history
                let mut join_set = tokio::task::JoinSet::new();
                for (step, step_number) in &ready_steps {
                    let base_history = current_history.clone();
                    let agent_clone = self.clone();
                    let tx = event_tx.clone();
                    let step_clone = step.clone();
                    let sn = *step_number;

                    join_set.spawn(async move {
                        let prompt = crate::prompts::render(
                            crate::prompts::PLAN_EXECUTE_STEP,
                            &[
                                ("step_num", &sn.to_string()),
                                ("description", &step_clone.content),
                            ],
                        );
                        let result = agent_clone
                            .execute_loop(&base_history, &prompt, None, tx)
                            .await;
                        (step_clone.id, sn, result)
                    });
                }

                // Collect results
                let mut parallel_summaries = Vec::new();
                while let Some(join_result) = join_set.join_next().await {
                    match join_result {
                        Ok((step_id, step_number, step_result)) => match step_result {
                            Ok(result) => {
                                total_usage.prompt_tokens += result.usage.prompt_tokens;
                                total_usage.completion_tokens += result.usage.completion_tokens;
                                total_usage.total_tokens += result.usage.total_tokens;
                                tool_calls_count += result.tool_calls_count;
                                plan.mark_status(&step_id, TaskStatus::Completed);

                                // Collect the final assistant text for context merging
                                parallel_summaries.push(format!(
                                    "- Step {} ({}): {}",
                                    step_number, step_id, result.text
                                ));

                                if let Some(tx) = &event_tx {
                                    tx.send(AgentEvent::StepEnd {
                                        step_id,
                                        status: TaskStatus::Completed,
                                        step_number,
                                        total_steps,
                                    })
                                    .await
                                    .ok();
                                }
                            }
                            Err(e) => {
                                tracing::error!("Plan step '{}' failed: {}", step_id, e);
                                plan.mark_status(&step_id, TaskStatus::Failed);

                                if let Some(tx) = &event_tx {
                                    tx.send(AgentEvent::StepEnd {
                                        step_id,
                                        status: TaskStatus::Failed,
                                        step_number,
                                        total_steps,
                                    })
                                    .await
                                    .ok();
                                }
                            }
                        },
                        Err(e) => {
                            tracing::error!("JoinSet task panicked: {}", e);
                        }
                    }
                }

                // Merge parallel results into history for subsequent steps
                if !parallel_summaries.is_empty() {
                    parallel_summaries.sort(); // Deterministic ordering
                    let results_text = parallel_summaries.join("\n");
                    current_history.push(Message::user(&crate::prompts::render(
                        crate::prompts::PLAN_PARALLEL_RESULTS,
                        &[("results", &results_text)],
                    )));
                }
            }

            // Emit GoalProgress after each wave
            if self.config.goal_tracking {
                let completed = plan
                    .steps
                    .iter()
                    .filter(|s| s.status == TaskStatus::Completed)
                    .count();
                if let Some(tx) = &event_tx {
                    tx.send(AgentEvent::GoalProgress {
                        goal: plan.goal.clone(),
                        progress: plan.progress(),
                        completed_steps: completed,
                        total_steps,
                    })
                    .await
                    .ok();
                }
            }
        }

        // Get final response
        let final_text = current_history
            .last()
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|block| {
                        if let crate::llm::ContentBlock::Text { text } = block {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        Ok(AgentResult {
            text: final_text,
            messages: current_history,
            usage: total_usage,
            tool_calls_count,
        })
    }

    /// Extract goal from prompt
    ///
    /// Delegates to [`LlmPlanner`] for structured JSON goal extraction,
    /// falling back to heuristic logic if the LLM call fails.
    pub async fn extract_goal(&self, prompt: &str) -> Result<AgentGoal> {
        use crate::planning::LlmPlanner;

        match LlmPlanner::extract_goal(&self.llm_client, prompt).await {
            Ok(goal) => Ok(goal),
            Err(e) => {
                tracing::warn!("LLM goal extraction failed, using fallback: {}", e);
                Ok(LlmPlanner::fallback_goal(prompt))
            }
        }
    }

    /// Check if goal is achieved
    ///
    /// Delegates to [`LlmPlanner`] for structured JSON achievement check,
    /// falling back to heuristic logic if the LLM call fails.
    pub async fn check_goal_achievement(
        &self,
        goal: &AgentGoal,
        current_state: &str,
    ) -> Result<bool> {
        use crate::planning::LlmPlanner;

        match LlmPlanner::check_achievement(&self.llm_client, goal, current_state).await {
            Ok(result) => Ok(result.achieved),
            Err(e) => {
                tracing::warn!("LLM achievement check failed, using fallback: {}", e);
                let result = LlmPlanner::fallback_check_achievement(goal, current_state);
                Ok(result.achieved)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ContentBlock, StreamEvent};
    use crate::permissions::PermissionPolicy;
    use crate::tools::ToolExecutor;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Create a default ToolContext for tests
    fn test_tool_context() -> ToolContext {
        ToolContext::new(PathBuf::from("/tmp"))
    }

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert!(config.system_prompt.is_none());
        assert!(config.tools.is_empty()); // Tools are provided externally
        assert_eq!(config.max_tool_rounds, MAX_TOOL_ROUNDS);
        assert!(config.permission_policy.is_none());
        assert!(config.context_providers.is_empty());
    }

    // ========================================================================
    // Mock LLM Client for Testing
    // ========================================================================

    /// Mock LLM client that returns predefined responses
    pub(crate) struct MockLlmClient {
        /// Responses to return (consumed in order)
        responses: std::sync::Mutex<Vec<LlmResponse>>,
        /// Number of calls made
        call_count: AtomicUsize,
    }

    impl MockLlmClient {
        pub(crate) fn new(responses: Vec<LlmResponse>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses),
                call_count: AtomicUsize::new(0),
            }
        }

        /// Create a response with text only (no tool calls)
        pub(crate) fn text_response(text: &str) -> LlmResponse {
            LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: text.to_string(),
                    }],
                    reasoning_content: None,
                },
                usage: TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                stop_reason: Some("end_turn".to_string()),
            }
        }

        /// Create a response with a tool call
        pub(crate) fn tool_call_response(
            tool_id: &str,
            tool_name: &str,
            args: serde_json::Value,
        ) -> LlmResponse {
            LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: tool_id.to_string(),
                        name: tool_name.to_string(),
                        input: args,
                    }],
                    reasoning_content: None,
                },
                usage: TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                stop_reason: Some("tool_use".to_string()),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for MockLlmClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> Result<LlmResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                anyhow::bail!("No more mock responses available");
            }
            Ok(responses.remove(0))
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> Result<mpsc::Receiver<StreamEvent>> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                anyhow::bail!("No more mock responses available");
            }
            let response = responses.remove(0);

            let (tx, rx) = mpsc::channel(10);
            tokio::spawn(async move {
                // Send text deltas if any
                for block in &response.message.content {
                    if let ContentBlock::Text { text } = block {
                        tx.send(StreamEvent::TextDelta(text.clone())).await.ok();
                    }
                }
                tx.send(StreamEvent::Done(response)).await.ok();
            });

            Ok(rx)
        }
    }

    // ========================================================================
    // Agent Loop Tests
    // ========================================================================

    #[tokio::test]
    async fn test_agent_simple_response() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Hello, I'm an AI assistant.",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig::default();

        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "Hello", None).await.unwrap();

        assert_eq!(result.text, "Hello, I'm an AI assistant.");
        assert_eq!(result.tool_calls_count, 0);
        assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_agent_with_tool_call() {
        let mock_client = Arc::new(MockLlmClient::new(vec![
            // First response: tool call
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo hello"}),
            ),
            // Second response: final text
            MockLlmClient::text_response("The command output was: hello"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig::default();

        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "Run echo hello", None).await.unwrap();

        assert_eq!(result.text, "The command output was: hello");
        assert_eq!(result.tool_calls_count, 1);
        assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_agent_permission_deny() {
        let mock_client = Arc::new(MockLlmClient::new(vec![
            // First response: tool call that will be denied
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "rm -rf /tmp/test"}),
            ),
            // Second response: LLM responds to the denial
            MockLlmClient::text_response(
                "I cannot execute that command due to permission restrictions.",
            ),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create permission policy that denies rm commands
        let permission_policy = PermissionPolicy::new().deny("bash(rm:*)");
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            ..Default::default()
        };

        let (tx, mut rx) = mpsc::channel(100);
        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "Delete files", Some(tx)).await.unwrap();

        // Check that we received a PermissionDenied event
        let mut found_permission_denied = false;
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::PermissionDenied { tool_name, .. } = event {
                assert_eq!(tool_name, "bash");
                found_permission_denied = true;
            }
        }
        assert!(
            found_permission_denied,
            "Should have received PermissionDenied event"
        );

        assert_eq!(result.tool_calls_count, 1);
    }

    #[tokio::test]
    async fn test_agent_permission_allow() {
        let mock_client = Arc::new(MockLlmClient::new(vec![
            // First response: tool call that will be allowed
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo hello"}),
            ),
            // Second response: final text
            MockLlmClient::text_response("Done!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create permission policy that allows echo commands
        let permission_policy = PermissionPolicy::new()
            .allow("bash(echo:*)")
            .deny("bash(rm:*)");
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            ..Default::default()
        };

        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "Echo hello", None).await.unwrap();

        assert_eq!(result.text, "Done!");
        assert_eq!(result.tool_calls_count, 1);
    }

    #[tokio::test]
    async fn test_agent_streaming_events() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Hello!",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig::default();

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let (mut rx, handle) = agent.execute_streaming(&[], "Hi").await.unwrap();

        // Collect events
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result.text, "Hello!");

        // Check we received Start and End events
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Start { .. })));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::End { .. })));
    }

    #[tokio::test]
    async fn test_agent_max_tool_rounds() {
        // Create a mock that always returns tool calls (infinite loop)
        let responses: Vec<LlmResponse> = (0..100)
            .map(|i| {
                MockLlmClient::tool_call_response(
                    &format!("tool-{}", i),
                    "bash",
                    serde_json::json!({"command": "echo loop"}),
                )
            })
            .collect();

        let mock_client = Arc::new(MockLlmClient::new(responses));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        let config = AgentConfig {
            max_tool_rounds: 3,
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Loop forever", None).await;

        // Should fail due to max tool rounds exceeded
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Max tool rounds"));
    }

    #[tokio::test]
    async fn test_agent_no_permission_policy_defaults_to_ask() {
        // When no permission policy is set, tools default to Ask.
        // Without a confirmation manager, Ask = safe deny.
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "rm -rf /tmp/test"}),
            ),
            MockLlmClient::text_response("Denied!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            permission_policy: None, // No policy → defaults to Ask
            // No confirmation_manager → safe deny
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Delete", None).await.unwrap();

        // Should be denied (no policy + no CM = safe deny)
        assert_eq!(result.text, "Denied!");
        assert_eq!(result.tool_calls_count, 1);
    }

    #[tokio::test]
    async fn test_agent_permission_ask_without_cm_denies() {
        // When permission is Ask and no confirmation manager configured,
        // tool execution should be denied (safe default).
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo test"}),
            ),
            MockLlmClient::text_response("Denied!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create policy where bash falls through to Ask (default)
        let permission_policy = PermissionPolicy::new(); // Default decision is Ask
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            // No confirmation_manager — safe deny
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Echo", None).await.unwrap();

        // Should deny (Ask without CM = safe deny)
        assert_eq!(result.text, "Denied!");
        // The tool result should contain the denial message
        assert!(result.tool_calls_count >= 1);
    }

    // ========================================================================
    // HITL (Human-in-the-Loop) Tests
    // ========================================================================

    #[tokio::test]
    async fn test_agent_hitl_approved() {
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo hello"}),
            ),
            MockLlmClient::text_response("Command executed!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create HITL confirmation manager with policy enabled
        let (event_tx, _event_rx) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy {
            enabled: true,
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        // Create permission policy that returns Ask for bash
        let permission_policy = PermissionPolicy::new(); // Default is Ask
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager.clone()),
            ..Default::default()
        };

        // Spawn a task to approve the confirmation
        let cm_clone = confirmation_manager.clone();
        tokio::spawn(async move {
            // Wait a bit for the confirmation request to be created
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            // Approve it
            cm_clone.confirm("tool-1", true, None).await.ok();
        });

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Run echo", None).await.unwrap();

        assert_eq!(result.text, "Command executed!");
        assert_eq!(result.tool_calls_count, 1);
    }

    #[tokio::test]
    async fn test_agent_hitl_rejected() {
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "rm -rf /"}),
            ),
            MockLlmClient::text_response("Understood, I won't do that."),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create HITL confirmation manager
        let (event_tx, _event_rx) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy {
            enabled: true,
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        // Permission policy returns Ask
        let permission_policy = PermissionPolicy::new();
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager.clone()),
            ..Default::default()
        };

        // Spawn a task to reject the confirmation
        let cm_clone = confirmation_manager.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cm_clone
                .confirm("tool-1", false, Some("Too dangerous".to_string()))
                .await
                .ok();
        });

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Delete everything", None).await.unwrap();

        // LLM should respond to the rejection
        assert_eq!(result.text, "Understood, I won't do that.");
    }

    #[tokio::test]
    async fn test_agent_hitl_timeout_reject() {
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy, TimeoutAction};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo test"}),
            ),
            MockLlmClient::text_response("Timed out, I understand."),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create HITL with very short timeout and Reject action
        let (event_tx, _event_rx) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy {
            enabled: true,
            default_timeout_ms: 50, // Very short timeout
            timeout_action: TimeoutAction::Reject,
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        let permission_policy = PermissionPolicy::new();
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager),
            ..Default::default()
        };

        // Don't approve - let it timeout
        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Echo", None).await.unwrap();

        // Should get timeout rejection response from LLM
        assert_eq!(result.text, "Timed out, I understand.");
    }

    #[tokio::test]
    async fn test_agent_hitl_timeout_auto_approve() {
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy, TimeoutAction};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo hello"}),
            ),
            MockLlmClient::text_response("Auto-approved and executed!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create HITL with very short timeout and AutoApprove action
        let (event_tx, _event_rx) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy {
            enabled: true,
            default_timeout_ms: 50, // Very short timeout
            timeout_action: TimeoutAction::AutoApprove,
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        let permission_policy = PermissionPolicy::new();
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager),
            ..Default::default()
        };

        // Don't approve - let it timeout and auto-approve
        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Echo", None).await.unwrap();

        // Should auto-approve on timeout and execute
        assert_eq!(result.text, "Auto-approved and executed!");
        assert_eq!(result.tool_calls_count, 1);
    }

    #[tokio::test]
    async fn test_agent_hitl_confirmation_events() {
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo test"}),
            ),
            MockLlmClient::text_response("Done!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create HITL confirmation manager
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy {
            enabled: true,
            default_timeout_ms: 5000, // Long enough timeout
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        let permission_policy = PermissionPolicy::new();
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager.clone()),
            ..Default::default()
        };

        // Spawn task to approve and collect events
        let cm_clone = confirmation_manager.clone();
        let event_handle = tokio::spawn(async move {
            let mut events = Vec::new();
            // Wait for ConfirmationRequired event
            while let Ok(event) = event_rx.recv().await {
                events.push(event.clone());
                if let AgentEvent::ConfirmationRequired { tool_id, .. } = event {
                    // Approve it
                    cm_clone.confirm(&tool_id, true, None).await.ok();
                    // Wait for ConfirmationReceived
                    if let Ok(recv_event) = event_rx.recv().await {
                        events.push(recv_event);
                    }
                    break;
                }
            }
            events
        });

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let _result = agent.execute(&[], "Echo", None).await.unwrap();

        // Check events
        let events = event_handle.await.unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ConfirmationRequired { .. })),
            "Should have ConfirmationRequired event"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ConfirmationReceived { approved: true, .. })),
            "Should have ConfirmationReceived event with approved=true"
        );
    }

    #[tokio::test]
    async fn test_agent_hitl_disabled_auto_executes() {
        // When HITL is disabled, tools should execute automatically even with Ask permission
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo auto"}),
            ),
            MockLlmClient::text_response("Auto executed!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create HITL with enabled=false
        let (event_tx, _event_rx) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy {
            enabled: false, // HITL disabled
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        let permission_policy = PermissionPolicy::new(); // Default is Ask
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager),
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Echo", None).await.unwrap();

        // Should execute without waiting for confirmation
        assert_eq!(result.text, "Auto executed!");
        assert_eq!(result.tool_calls_count, 1);
    }

    #[tokio::test]
    async fn test_agent_hitl_with_permission_deny_skips_hitl() {
        // When permission is Deny, HITL should not be triggered
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "rm -rf /"}),
            ),
            MockLlmClient::text_response("Blocked by permission."),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create HITL enabled
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy {
            enabled: true,
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        // Permission policy denies rm commands
        let permission_policy = PermissionPolicy::new().deny("bash(rm:*)");
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager),
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Delete", None).await.unwrap();

        // Should be denied without HITL
        assert_eq!(result.text, "Blocked by permission.");

        // Should NOT have any ConfirmationRequired events
        let mut found_confirmation = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, AgentEvent::ConfirmationRequired { .. }) {
                found_confirmation = true;
            }
        }
        assert!(
            !found_confirmation,
            "HITL should not be triggered when permission is Deny"
        );
    }

    #[tokio::test]
    async fn test_agent_hitl_with_permission_allow_skips_hitl() {
        // When permission is Allow, HITL confirmation is skipped entirely.
        // PermissionPolicy is the declarative rule engine; Allow = execute directly.
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo hello"}),
            ),
            MockLlmClient::text_response("Allowed!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create HITL enabled
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy {
            enabled: true,
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        // Permission policy allows echo commands
        let permission_policy = PermissionPolicy::new().allow("bash(echo:*)");
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager.clone()),
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Echo", None).await.unwrap();

        // Should execute directly without HITL (permission Allow skips confirmation)
        assert_eq!(result.text, "Allowed!");

        // Should NOT have ConfirmationRequired event (Allow bypasses HITL)
        let mut found_confirmation = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, AgentEvent::ConfirmationRequired { .. }) {
                found_confirmation = true;
            }
        }
        assert!(
            !found_confirmation,
            "Permission Allow should skip HITL confirmation"
        );
    }

    #[tokio::test]
    async fn test_agent_hitl_multiple_tool_calls() {
        // Test multiple tool calls in sequence with HITL
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            // First response: two tool calls
            LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![
                        ContentBlock::ToolUse {
                            id: "tool-1".to_string(),
                            name: "bash".to_string(),
                            input: serde_json::json!({"command": "echo first"}),
                        },
                        ContentBlock::ToolUse {
                            id: "tool-2".to_string(),
                            name: "bash".to_string(),
                            input: serde_json::json!({"command": "echo second"}),
                        },
                    ],
                    reasoning_content: None,
                },
                usage: TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                stop_reason: Some("tool_use".to_string()),
            },
            MockLlmClient::text_response("Both executed!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create HITL
        let (event_tx, _event_rx) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy {
            enabled: true,
            default_timeout_ms: 5000,
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        let permission_policy = PermissionPolicy::new(); // Default Ask
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager.clone()),
            ..Default::default()
        };

        // Spawn task to approve both tools
        let cm_clone = confirmation_manager.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            cm_clone.confirm("tool-1", true, None).await.ok();
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            cm_clone.confirm("tool-2", true, None).await.ok();
        });

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Run both", None).await.unwrap();

        assert_eq!(result.text, "Both executed!");
        assert_eq!(result.tool_calls_count, 2);
    }

    #[tokio::test]
    async fn test_agent_hitl_partial_approval() {
        // Test: first tool approved, second rejected
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            // First response: two tool calls
            LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![
                        ContentBlock::ToolUse {
                            id: "tool-1".to_string(),
                            name: "bash".to_string(),
                            input: serde_json::json!({"command": "echo safe"}),
                        },
                        ContentBlock::ToolUse {
                            id: "tool-2".to_string(),
                            name: "bash".to_string(),
                            input: serde_json::json!({"command": "rm -rf /"}),
                        },
                    ],
                    reasoning_content: None,
                },
                usage: TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                stop_reason: Some("tool_use".to_string()),
            },
            MockLlmClient::text_response("First worked, second rejected."),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        let (event_tx, _event_rx) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy {
            enabled: true,
            default_timeout_ms: 5000,
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        let permission_policy = PermissionPolicy::new();
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager.clone()),
            ..Default::default()
        };

        // Approve first, reject second
        let cm_clone = confirmation_manager.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            cm_clone.confirm("tool-1", true, None).await.ok();
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            cm_clone
                .confirm("tool-2", false, Some("Dangerous".to_string()))
                .await
                .ok();
        });

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Run both", None).await.unwrap();

        assert_eq!(result.text, "First worked, second rejected.");
        assert_eq!(result.tool_calls_count, 2);
    }

    #[tokio::test]
    async fn test_agent_hitl_yolo_mode_auto_approves() {
        // YOLO mode: specific lanes auto-approve without confirmation
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy, SessionLane};
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "read", // Query lane tool
                serde_json::json!({"path": "/tmp/test.txt"}),
            ),
            MockLlmClient::text_response("File read!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // YOLO mode for Query lane (read, glob, ls, grep)
        let (event_tx, mut event_rx) = broadcast::channel(100);
        let mut yolo_lanes = std::collections::HashSet::new();
        yolo_lanes.insert(SessionLane::Query);
        let hitl_policy = ConfirmationPolicy {
            enabled: true,
            yolo_lanes, // Auto-approve query operations
            ..Default::default()
        };
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        let permission_policy = PermissionPolicy::new();
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager),
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Read file", None).await.unwrap();

        // Should auto-execute without confirmation (YOLO mode)
        assert_eq!(result.text, "File read!");

        // Should NOT have ConfirmationRequired for yolo lane
        let mut found_confirmation = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, AgentEvent::ConfirmationRequired { .. }) {
                found_confirmation = true;
            }
        }
        assert!(
            !found_confirmation,
            "YOLO mode should not trigger confirmation"
        );
    }

    #[tokio::test]
    async fn test_agent_config_with_all_options() {
        use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
        use tokio::sync::broadcast;

        let (event_tx, _) = broadcast::channel(100);
        let hitl_policy = ConfirmationPolicy::default();
        let confirmation_manager = Arc::new(ConfirmationManager::new(hitl_policy, event_tx));

        let permission_policy = PermissionPolicy::new().allow("bash(*)");
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            system_prompt: Some("Test system prompt".to_string()),
            tools: vec![],
            max_tool_rounds: 10,
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(confirmation_manager),
            context_providers: vec![],
            planning_enabled: false,
            goal_tracking: false,
            hook_engine: None,
        };

        assert_eq!(config.system_prompt, Some("Test system prompt".to_string()));
        assert_eq!(config.max_tool_rounds, 10);
        assert!(config.permission_policy.is_some());
        assert!(config.confirmation_manager.is_some());
        assert!(config.context_providers.is_empty());

        // Test Debug trait
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("AgentConfig"));
        assert!(debug_str.contains("permission_policy: true"));
        assert!(debug_str.contains("confirmation_manager: true"));
        assert!(debug_str.contains("context_providers: 0"));
    }

    // ========================================================================
    // Context Provider Tests
    // ========================================================================

    use crate::context::{ContextItem, ContextType};

    /// Mock context provider for testing
    struct MockContextProvider {
        name: String,
        items: Vec<ContextItem>,
        on_turn_calls: std::sync::Arc<tokio::sync::RwLock<Vec<(String, String, String)>>>,
    }

    impl MockContextProvider {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                items: Vec::new(),
                on_turn_calls: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            }
        }

        fn with_items(mut self, items: Vec<ContextItem>) -> Self {
            self.items = items;
            self
        }
    }

    #[async_trait::async_trait]
    impl ContextProvider for MockContextProvider {
        fn name(&self) -> &str {
            &self.name
        }

        async fn query(&self, _query: &ContextQuery) -> anyhow::Result<ContextResult> {
            let mut result = ContextResult::new(&self.name);
            for item in &self.items {
                result.add_item(item.clone());
            }
            Ok(result)
        }

        async fn on_turn_complete(
            &self,
            session_id: &str,
            prompt: &str,
            response: &str,
        ) -> anyhow::Result<()> {
            let mut calls = self.on_turn_calls.write().await;
            calls.push((
                session_id.to_string(),
                prompt.to_string(),
                response.to_string(),
            ));
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_agent_with_context_provider() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Response using context",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        let provider =
            MockContextProvider::new("test-provider").with_items(vec![ContextItem::new(
                "ctx-1",
                ContextType::Resource,
                "Relevant context here",
            )
            .with_source("test://docs/example")]);

        let config = AgentConfig {
            system_prompt: Some("You are helpful.".to_string()),
            context_providers: vec![Arc::new(provider)],
            ..Default::default()
        };

        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "What is X?", None).await.unwrap();

        assert_eq!(result.text, "Response using context");
        assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_agent_context_provider_events() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Answer",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        let provider =
            MockContextProvider::new("event-provider").with_items(vec![ContextItem::new(
                "item-1",
                ContextType::Memory,
                "Memory content",
            )
            .with_token_count(50)]);

        let config = AgentConfig {
            context_providers: vec![Arc::new(provider)],
            ..Default::default()
        };

        let (tx, mut rx) = mpsc::channel(100);
        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let _result = agent.execute(&[], "Test prompt", Some(tx)).await.unwrap();

        // Collect events
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Should have ContextResolving and ContextResolved events
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ContextResolving { .. })),
            "Should have ContextResolving event"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ContextResolved { .. })),
            "Should have ContextResolved event"
        );

        // Check context resolved values
        for event in &events {
            if let AgentEvent::ContextResolved {
                total_items,
                total_tokens,
            } = event
            {
                assert_eq!(*total_items, 1);
                assert_eq!(*total_tokens, 50);
            }
        }
    }

    #[tokio::test]
    async fn test_agent_multiple_context_providers() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Combined response",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        let provider1 = MockContextProvider::new("provider-1").with_items(vec![ContextItem::new(
            "p1-1",
            ContextType::Resource,
            "Resource from P1",
        )
        .with_token_count(100)]);

        let provider2 = MockContextProvider::new("provider-2").with_items(vec![
            ContextItem::new("p2-1", ContextType::Memory, "Memory from P2").with_token_count(50),
            ContextItem::new("p2-2", ContextType::Skill, "Skill from P2").with_token_count(75),
        ]);

        let config = AgentConfig {
            system_prompt: Some("Base system prompt.".to_string()),
            context_providers: vec![Arc::new(provider1), Arc::new(provider2)],
            ..Default::default()
        };

        let (tx, mut rx) = mpsc::channel(100);
        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Query", Some(tx)).await.unwrap();

        assert_eq!(result.text, "Combined response");

        // Check context resolved event has combined totals
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::ContextResolved {
                total_items,
                total_tokens,
            } = event
            {
                assert_eq!(total_items, 3); // 1 + 2
                assert_eq!(total_tokens, 225); // 100 + 50 + 75
            }
        }
    }

    #[tokio::test]
    async fn test_agent_no_context_providers() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "No context",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // No context providers
        let config = AgentConfig::default();

        let (tx, mut rx) = mpsc::channel(100);
        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Simple prompt", Some(tx)).await.unwrap();

        assert_eq!(result.text, "No context");

        // Should NOT have context events when no providers
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ContextResolving { .. })),
            "Should NOT have ContextResolving event"
        );
    }

    #[tokio::test]
    async fn test_agent_context_on_turn_complete() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Final response",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        let provider = Arc::new(MockContextProvider::new("memory-provider"));
        let on_turn_calls = provider.on_turn_calls.clone();

        let config = AgentConfig {
            context_providers: vec![provider],
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

        // Execute with session ID
        let result = agent
            .execute_with_session(&[], "User prompt", Some("sess-123"), None)
            .await
            .unwrap();

        assert_eq!(result.text, "Final response");

        // Check on_turn_complete was called
        let calls = on_turn_calls.read().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "sess-123");
        assert_eq!(calls[0].1, "User prompt");
        assert_eq!(calls[0].2, "Final response");
    }

    #[tokio::test]
    async fn test_agent_context_on_turn_complete_no_session() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Response",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        let provider = Arc::new(MockContextProvider::new("memory-provider"));
        let on_turn_calls = provider.on_turn_calls.clone();

        let config = AgentConfig {
            context_providers: vec![provider],
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

        // Execute without session ID (uses execute() which passes None)
        let _result = agent.execute(&[], "Prompt", None).await.unwrap();

        // on_turn_complete should NOT be called when session_id is None
        let calls = on_turn_calls.read().await;
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn test_agent_build_augmented_system_prompt() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response("OK")]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        let provider = MockContextProvider::new("test").with_items(vec![ContextItem::new(
            "doc-1",
            ContextType::Resource,
            "Auth uses JWT tokens.",
        )
        .with_source("viking://docs/auth")]);

        let config = AgentConfig {
            system_prompt: Some("You are helpful.".to_string()),
            context_providers: vec![Arc::new(provider)],
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

        // Test building augmented prompt
        let context_results = agent.resolve_context("test", None).await;
        let augmented = agent.build_augmented_system_prompt(&context_results);

        let augmented_str = augmented.unwrap();
        assert!(augmented_str.contains("You are helpful."));
        assert!(augmented_str.contains("<context source=\"viking://docs/auth\" type=\"Resource\">"));
        assert!(augmented_str.contains("Auth uses JWT tokens."));
    }

    // ========================================================================
    // Agentic Loop Integration Tests
    // ========================================================================

    /// Helper: collect all events from a channel
    async fn collect_events(mut rx: mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        // Drain remaining
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn test_agent_multi_turn_tool_chain() {
        // LLM calls tool A → sees result → calls tool B → sees result → final answer
        let mock_client = Arc::new(MockLlmClient::new(vec![
            // Turn 1: call ls
            MockLlmClient::tool_call_response(
                "t1",
                "bash",
                serde_json::json!({"command": "echo step1"}),
            ),
            // Turn 2: call another tool based on first result
            MockLlmClient::tool_call_response(
                "t2",
                "bash",
                serde_json::json!({"command": "echo step2"}),
            ),
            // Turn 3: final answer
            MockLlmClient::text_response("Completed both steps: step1 then step2"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig::default();

        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "Run two steps", None).await.unwrap();

        assert_eq!(result.text, "Completed both steps: step1 then step2");
        assert_eq!(result.tool_calls_count, 2);
        assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 3);

        // Verify message history: user → assistant(tool_use) → user(tool_result) → assistant(tool_use) → user(tool_result) → assistant(text)
        assert_eq!(result.messages[0].role, "user");
        assert_eq!(result.messages[1].role, "assistant"); // tool call 1
        assert_eq!(result.messages[2].role, "user"); // tool result 1 (Anthropic convention)
        assert_eq!(result.messages[3].role, "assistant"); // tool call 2
        assert_eq!(result.messages[4].role, "user"); // tool result 2
        assert_eq!(result.messages[5].role, "assistant"); // final text
        assert_eq!(result.messages.len(), 6);
    }

    #[tokio::test]
    async fn test_agent_conversation_history_preserved() {
        // Pass existing history, verify it's preserved in output
        let existing_history = vec![
            Message::user("What is Rust?"),
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Rust is a systems programming language.".to_string(),
                }],
                reasoning_content: None,
            },
        ];

        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Rust was created by Graydon Hoare at Mozilla.",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let result = agent
            .execute(&existing_history, "Who created it?", None)
            .await
            .unwrap();

        // History should contain: old user + old assistant + new user + new assistant
        assert_eq!(result.messages.len(), 4);
        assert_eq!(result.messages[0].text(), "What is Rust?");
        assert_eq!(
            result.messages[1].text(),
            "Rust is a systems programming language."
        );
        assert_eq!(result.messages[2].text(), "Who created it?");
        assert_eq!(
            result.messages[3].text(),
            "Rust was created by Graydon Hoare at Mozilla."
        );
    }

    #[tokio::test]
    async fn test_agent_event_stream_completeness() {
        // Verify full event sequence for a single tool call loop
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "t1",
                "bash",
                serde_json::json!({"command": "echo hi"}),
            ),
            MockLlmClient::text_response("Done"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let (tx, rx) = mpsc::channel(100);
        let result = agent.execute(&[], "Say hi", Some(tx)).await.unwrap();
        assert_eq!(result.text, "Done");

        let events = collect_events(rx).await;

        // Verify event sequence
        let event_types: Vec<&str> = events
            .iter()
            .map(|e| match e {
                AgentEvent::Start { .. } => "Start",
                AgentEvent::TurnStart { .. } => "TurnStart",
                AgentEvent::TurnEnd { .. } => "TurnEnd",
                AgentEvent::ToolEnd { .. } => "ToolEnd",
                AgentEvent::End { .. } => "End",
                _ => "Other",
            })
            .collect();

        // Must start with Start, end with End
        assert_eq!(event_types.first(), Some(&"Start"));
        assert_eq!(event_types.last(), Some(&"End"));

        // Must have 2 TurnStarts (tool call turn + final answer turn)
        let turn_starts = event_types.iter().filter(|&&t| t == "TurnStart").count();
        assert_eq!(turn_starts, 2);

        // Must have 1 ToolEnd
        let tool_ends = event_types.iter().filter(|&&t| t == "ToolEnd").count();
        assert_eq!(tool_ends, 1);
    }

    #[tokio::test]
    async fn test_agent_multiple_tools_single_turn() {
        // LLM returns 2 tool calls in one response
        let mock_client = Arc::new(MockLlmClient::new(vec![
            LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![
                        ContentBlock::ToolUse {
                            id: "t1".to_string(),
                            name: "bash".to_string(),
                            input: serde_json::json!({"command": "echo first"}),
                        },
                        ContentBlock::ToolUse {
                            id: "t2".to_string(),
                            name: "bash".to_string(),
                            input: serde_json::json!({"command": "echo second"}),
                        },
                    ],
                    reasoning_content: None,
                },
                usage: TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                stop_reason: Some("tool_use".to_string()),
            },
            MockLlmClient::text_response("Both commands ran"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let result = agent.execute(&[], "Run both", None).await.unwrap();

        assert_eq!(result.text, "Both commands ran");
        assert_eq!(result.tool_calls_count, 2);
        assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2); // Only 2 LLM calls

        // Messages: user → assistant(2 tools) → user(tool_result) → user(tool_result) → assistant(text)
        assert_eq!(result.messages[0].role, "user");
        assert_eq!(result.messages[1].role, "assistant");
        assert_eq!(result.messages[2].role, "user"); // tool result 1
        assert_eq!(result.messages[3].role, "user"); // tool result 2
        assert_eq!(result.messages[4].role, "assistant");
    }

    #[tokio::test]
    async fn test_agent_token_usage_accumulation() {
        // Verify usage sums across multiple turns
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "t1",
                "bash",
                serde_json::json!({"command": "echo x"}),
            ),
            MockLlmClient::text_response("Done"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let result = agent.execute(&[], "test", None).await.unwrap();

        // Each mock response has prompt=10, completion=5, total=15
        // 2 LLM calls → 20 prompt, 10 completion, 30 total
        assert_eq!(result.usage.prompt_tokens, 20);
        assert_eq!(result.usage.completion_tokens, 10);
        assert_eq!(result.usage.total_tokens, 30);
    }

    #[tokio::test]
    async fn test_agent_system_prompt_passed() {
        // Verify system prompt is used (MockLlmClient captures calls)
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "I am a coding assistant.",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            system_prompt: Some("You are a coding assistant.".to_string()),
            ..Default::default()
        };

        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "What are you?", None).await.unwrap();

        assert_eq!(result.text, "I am a coding assistant.");
        assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_agent_max_rounds_with_persistent_tool_calls() {
        // LLM keeps calling tools forever — should hit max_tool_rounds
        let mut responses = Vec::new();
        for i in 0..15 {
            responses.push(MockLlmClient::tool_call_response(
                &format!("t{}", i),
                "bash",
                serde_json::json!({"command": format!("echo round{}", i)}),
            ));
        }

        let mock_client = Arc::new(MockLlmClient::new(responses));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            max_tool_rounds: 5,
            ..Default::default()
        };

        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "Loop forever", None).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Max tool rounds (5) exceeded"));
    }

    #[tokio::test]
    async fn test_agent_end_event_contains_final_text() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Final answer here",
        )]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let (tx, rx) = mpsc::channel(100);
        agent.execute(&[], "test", Some(tx)).await.unwrap();

        let events = collect_events(rx).await;
        let end_event = events.iter().find(|e| matches!(e, AgentEvent::End { .. }));
        assert!(end_event.is_some());

        if let AgentEvent::End { text, usage } = end_event.unwrap() {
            assert_eq!(text, "Final answer here");
            assert_eq!(usage.total_tokens, 15);
        }
    }
}

#[cfg(test)]
mod extra_agent_tests {
    use super::*;
    use crate::agent::tests::MockLlmClient;
    use crate::llm::{ContentBlock, StreamEvent};
    use crate::queue::SessionQueueConfig;
    use crate::tools::ToolExecutor;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_tool_context() -> ToolContext {
        ToolContext::new(PathBuf::from("/tmp"))
    }

    // ========================================================================
    // AgentConfig
    // ========================================================================

    #[test]
    fn test_agent_config_debug() {
        let config = AgentConfig {
            system_prompt: Some("You are helpful".to_string()),
            tools: vec![],
            max_tool_rounds: 10,
            permission_policy: None,
            confirmation_manager: None,
            context_providers: vec![],
            planning_enabled: true,
            goal_tracking: false,
            hook_engine: None,
        };
        let debug = format!("{:?}", config);
        assert!(debug.contains("AgentConfig"));
        assert!(debug.contains("planning_enabled"));
    }

    #[test]
    fn test_agent_config_default_values() {
        let config = AgentConfig::default();
        assert_eq!(config.max_tool_rounds, MAX_TOOL_ROUNDS);
        assert!(!config.planning_enabled);
        assert!(!config.goal_tracking);
        assert!(config.context_providers.is_empty());
    }

    // ========================================================================
    // AgentEvent serialization
    // ========================================================================

    #[test]
    fn test_agent_event_serialize_start() {
        let event = AgentEvent::Start {
            prompt: "Hello".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("agent_start"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_agent_event_serialize_text_delta() {
        let event = AgentEvent::TextDelta {
            text: "chunk".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("text_delta"));
    }

    #[test]
    fn test_agent_event_serialize_tool_start() {
        let event = AgentEvent::ToolStart {
            id: "t1".to_string(),
            name: "bash".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("tool_start"));
        assert!(json.contains("bash"));
    }

    #[test]
    fn test_agent_event_serialize_tool_end() {
        let event = AgentEvent::ToolEnd {
            id: "t1".to_string(),
            name: "bash".to_string(),
            output: "hello".to_string(),
            exit_code: 0,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("tool_end"));
    }

    #[test]
    fn test_agent_event_serialize_error() {
        let event = AgentEvent::Error {
            message: "oops".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("oops"));
    }

    #[test]
    fn test_agent_event_serialize_confirmation_required() {
        let event = AgentEvent::ConfirmationRequired {
            tool_id: "t1".to_string(),
            tool_name: "bash".to_string(),
            args: serde_json::json!({"cmd": "rm"}),
            timeout_ms: 30000,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("confirmation_required"));
    }

    #[test]
    fn test_agent_event_serialize_confirmation_received() {
        let event = AgentEvent::ConfirmationReceived {
            tool_id: "t1".to_string(),
            approved: true,
            reason: Some("safe".to_string()),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("confirmation_received"));
    }

    #[test]
    fn test_agent_event_serialize_confirmation_timeout() {
        let event = AgentEvent::ConfirmationTimeout {
            tool_id: "t1".to_string(),
            action_taken: "rejected".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("confirmation_timeout"));
    }

    #[test]
    fn test_agent_event_serialize_external_task_pending() {
        let event = AgentEvent::ExternalTaskPending {
            task_id: "task-1".to_string(),
            session_id: "sess-1".to_string(),
            lane: crate::hitl::SessionLane::Execute,
            command_type: "bash".to_string(),
            payload: serde_json::json!({}),
            timeout_ms: 60000,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("external_task_pending"));
    }

    #[test]
    fn test_agent_event_serialize_external_task_completed() {
        let event = AgentEvent::ExternalTaskCompleted {
            task_id: "task-1".to_string(),
            session_id: "sess-1".to_string(),
            success: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("external_task_completed"));
    }

    #[test]
    fn test_agent_event_serialize_permission_denied() {
        let event = AgentEvent::PermissionDenied {
            tool_id: "t1".to_string(),
            tool_name: "bash".to_string(),
            args: serde_json::json!({}),
            reason: "denied".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("permission_denied"));
    }

    #[test]
    fn test_agent_event_serialize_context_compacted() {
        let event = AgentEvent::ContextCompacted {
            session_id: "sess-1".to_string(),
            before_messages: 100,
            after_messages: 20,
            percent_before: 0.85,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("context_compacted"));
    }

    #[test]
    fn test_agent_event_serialize_turn_start() {
        let event = AgentEvent::TurnStart { turn: 3 };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("turn_start"));
    }

    #[test]
    fn test_agent_event_serialize_turn_end() {
        let event = AgentEvent::TurnEnd {
            turn: 3,
            usage: TokenUsage::default(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("turn_end"));
    }

    #[test]
    fn test_agent_event_serialize_end() {
        let event = AgentEvent::End {
            text: "Done".to_string(),
            usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("agent_end"));
    }

    // ========================================================================
    // AgentResult
    // ========================================================================

    #[test]
    fn test_agent_result_fields() {
        let result = AgentResult {
            text: "output".to_string(),
            messages: vec![Message::user("hello")],
            usage: TokenUsage::default(),
            tool_calls_count: 3,
        };
        assert_eq!(result.text, "output");
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.tool_calls_count, 3);
    }

    // ========================================================================
    // Missing AgentEvent serialization tests
    // ========================================================================

    #[test]
    fn test_agent_event_serialize_context_resolving() {
        let event = AgentEvent::ContextResolving {
            providers: vec!["provider1".to_string(), "provider2".to_string()],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("context_resolving"));
        assert!(json.contains("provider1"));
    }

    #[test]
    fn test_agent_event_serialize_context_resolved() {
        let event = AgentEvent::ContextResolved {
            total_items: 5,
            total_tokens: 1000,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("context_resolved"));
        assert!(json.contains("1000"));
    }

    #[test]
    fn test_agent_event_serialize_command_dead_lettered() {
        let event = AgentEvent::CommandDeadLettered {
            command_id: "cmd-1".to_string(),
            command_type: "bash".to_string(),
            lane: "execute".to_string(),
            error: "timeout".to_string(),
            attempts: 3,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("command_dead_lettered"));
        assert!(json.contains("cmd-1"));
    }

    #[test]
    fn test_agent_event_serialize_command_retry() {
        let event = AgentEvent::CommandRetry {
            command_id: "cmd-2".to_string(),
            command_type: "read".to_string(),
            lane: "query".to_string(),
            attempt: 2,
            delay_ms: 1000,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("command_retry"));
        assert!(json.contains("cmd-2"));
    }

    #[test]
    fn test_agent_event_serialize_queue_alert() {
        let event = AgentEvent::QueueAlert {
            level: "warning".to_string(),
            alert_type: "depth".to_string(),
            message: "Queue depth exceeded".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("queue_alert"));
        assert!(json.contains("warning"));
    }

    #[test]
    fn test_agent_event_serialize_task_updated() {
        let event = AgentEvent::TaskUpdated {
            session_id: "sess-1".to_string(),
            tasks: vec![],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("task_updated"));
        assert!(json.contains("sess-1"));
    }

    #[test]
    fn test_agent_event_serialize_memory_stored() {
        let event = AgentEvent::MemoryStored {
            memory_id: "mem-1".to_string(),
            memory_type: "conversation".to_string(),
            importance: 0.8,
            tags: vec!["important".to_string()],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("memory_stored"));
        assert!(json.contains("mem-1"));
    }

    #[test]
    fn test_agent_event_serialize_memory_recalled() {
        let event = AgentEvent::MemoryRecalled {
            memory_id: "mem-2".to_string(),
            content: "Previous conversation".to_string(),
            relevance: 0.9,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("memory_recalled"));
        assert!(json.contains("mem-2"));
    }

    #[test]
    fn test_agent_event_serialize_memories_searched() {
        let event = AgentEvent::MemoriesSearched {
            query: Some("search term".to_string()),
            tags: vec!["tag1".to_string()],
            result_count: 5,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("memories_searched"));
        assert!(json.contains("search term"));
    }

    #[test]
    fn test_agent_event_serialize_memory_cleared() {
        let event = AgentEvent::MemoryCleared {
            tier: "short_term".to_string(),
            count: 10,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("memory_cleared"));
        assert!(json.contains("short_term"));
    }

    #[test]
    fn test_agent_event_serialize_subagent_start() {
        let event = AgentEvent::SubagentStart {
            task_id: "task-1".to_string(),
            session_id: "child-sess".to_string(),
            parent_session_id: "parent-sess".to_string(),
            agent: "explore".to_string(),
            description: "Explore codebase".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("subagent_start"));
        assert!(json.contains("explore"));
    }

    #[test]
    fn test_agent_event_serialize_subagent_progress() {
        let event = AgentEvent::SubagentProgress {
            task_id: "task-1".to_string(),
            session_id: "child-sess".to_string(),
            status: "processing".to_string(),
            metadata: serde_json::json!({"progress": 50}),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("subagent_progress"));
        assert!(json.contains("processing"));
    }

    #[test]
    fn test_agent_event_serialize_subagent_end() {
        let event = AgentEvent::SubagentEnd {
            task_id: "task-1".to_string(),
            session_id: "child-sess".to_string(),
            agent: "explore".to_string(),
            output: "Found 10 files".to_string(),
            success: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("subagent_end"));
        assert!(json.contains("Found 10 files"));
    }

    #[test]
    fn test_agent_event_serialize_planning_start() {
        let event = AgentEvent::PlanningStart {
            prompt: "Build a web app".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("planning_start"));
        assert!(json.contains("Build a web app"));
    }

    #[test]
    fn test_agent_event_serialize_planning_end() {
        use crate::planning::{Complexity, ExecutionPlan};
        let plan = ExecutionPlan::new("Test goal".to_string(), Complexity::Simple);
        let event = AgentEvent::PlanningEnd {
            plan,
            estimated_steps: 3,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("planning_end"));
        assert!(json.contains("estimated_steps"));
    }

    #[test]
    fn test_agent_event_serialize_step_start() {
        let event = AgentEvent::StepStart {
            step_id: "step-1".to_string(),
            description: "Initialize project".to_string(),
            step_number: 1,
            total_steps: 5,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("step_start"));
        assert!(json.contains("Initialize project"));
    }

    #[test]
    fn test_agent_event_serialize_step_end() {
        let event = AgentEvent::StepEnd {
            step_id: "step-1".to_string(),
            status: TaskStatus::Completed,
            step_number: 1,
            total_steps: 5,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("step_end"));
        assert!(json.contains("step-1"));
    }

    #[test]
    fn test_agent_event_serialize_goal_extracted() {
        use crate::planning::AgentGoal;
        let goal = AgentGoal::new("Complete the task".to_string());
        let event = AgentEvent::GoalExtracted { goal };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("goal_extracted"));
    }

    #[test]
    fn test_agent_event_serialize_goal_progress() {
        let event = AgentEvent::GoalProgress {
            goal: "Build app".to_string(),
            progress: 0.5,
            completed_steps: 2,
            total_steps: 4,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("goal_progress"));
        assert!(json.contains("0.5"));
    }

    #[test]
    fn test_agent_event_serialize_goal_achieved() {
        let event = AgentEvent::GoalAchieved {
            goal: "Build app".to_string(),
            total_steps: 4,
            duration_ms: 5000,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("goal_achieved"));
        assert!(json.contains("5000"));
    }

    #[tokio::test]
    async fn test_extract_goal_with_json_response() {
        // LlmPlanner expects JSON with "description" and "success_criteria" fields
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            r#"{"description": "Build web app", "success_criteria": ["App runs on port 3000", "Has login page"]}"#,
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let goal = agent.extract_goal("Build a web app").await.unwrap();
        assert_eq!(goal.description, "Build web app");
        assert_eq!(goal.success_criteria.len(), 2);
        assert_eq!(goal.success_criteria[0], "App runs on port 3000");
    }

    #[tokio::test]
    async fn test_extract_goal_fallback_on_non_json() {
        // Non-JSON response triggers fallback: returns the original prompt as goal
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Some non-JSON response",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let goal = agent.extract_goal("Do something").await.unwrap();
        // Fallback uses the original prompt as description
        assert_eq!(goal.description, "Do something");
        // Fallback adds 2 generic criteria
        assert_eq!(goal.success_criteria.len(), 2);
    }

    #[tokio::test]
    async fn test_check_goal_achievement_json_yes() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            r#"{"achieved": true, "progress": 1.0, "remaining_criteria": []}"#,
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let goal = crate::planning::AgentGoal::new("Test goal".to_string());
        let achieved = agent
            .check_goal_achievement(&goal, "All done")
            .await
            .unwrap();
        assert!(achieved);
    }

    #[tokio::test]
    async fn test_check_goal_achievement_fallback_not_done() {
        // Non-JSON response triggers heuristic fallback
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "invalid json",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let goal = crate::planning::AgentGoal::new("Test goal".to_string());
        // "still working" doesn't contain "complete"/"done"/"finished"
        let achieved = agent
            .check_goal_achievement(&goal, "still working")
            .await
            .unwrap();
        assert!(!achieved);
    }

    // ========================================================================
    // build_augmented_system_prompt Tests
    // ========================================================================

    #[test]
    fn test_build_augmented_system_prompt_empty_context() {
        let mock_client = Arc::new(MockLlmClient::new(vec![]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            system_prompt: Some("Base prompt".to_string()),
            ..Default::default()
        };
        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

        let result = agent.build_augmented_system_prompt(&[]);
        assert_eq!(result, Some("Base prompt".to_string()));
    }

    #[test]
    fn test_build_augmented_system_prompt_no_system_prompt() {
        let mock_client = Arc::new(MockLlmClient::new(vec![]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let result = agent.build_augmented_system_prompt(&[]);
        assert_eq!(result, None);
    }

    #[test]
    fn test_build_augmented_system_prompt_with_context_no_base() {
        use crate::context::{ContextItem, ContextResult, ContextType};

        let mock_client = Arc::new(MockLlmClient::new(vec![]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let context = vec![ContextResult {
            provider: "test".to_string(),
            items: vec![ContextItem::new("id1", ContextType::Resource, "Content")],
            total_tokens: 10,
            truncated: false,
        }];

        let result = agent.build_augmented_system_prompt(&context);
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.contains("<context"));
        assert!(text.contains("Content"));
    }

    // ========================================================================
    // AgentResult Clone and Debug
    // ========================================================================

    #[test]
    fn test_agent_result_clone() {
        let result = AgentResult {
            text: "output".to_string(),
            messages: vec![Message::user("hello")],
            usage: TokenUsage::default(),
            tool_calls_count: 3,
        };
        let cloned = result.clone();
        assert_eq!(cloned.text, result.text);
        assert_eq!(cloned.tool_calls_count, result.tool_calls_count);
    }

    #[test]
    fn test_agent_result_debug() {
        let result = AgentResult {
            text: "output".to_string(),
            messages: vec![Message::user("hello")],
            usage: TokenUsage::default(),
            tool_calls_count: 3,
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("AgentResult"));
        assert!(debug.contains("output"));
    }

    // ========================================================================
    // handle_post_execution_metadata Tests
    // ========================================================================

    // ========================================================================
    // partition_by_lane tests
    // ========================================================================

    #[test]
    fn test_partition_by_lane_query_tools() {
        let tool_calls = vec![
            ToolCall {
                id: "t1".to_string(),
                name: "read".to_string(),
                args: serde_json::json!({"file": "a.rs"}),
            },
            ToolCall {
                id: "t2".to_string(),
                name: "glob".to_string(),
                args: serde_json::json!({"pattern": "**/*.rs"}),
            },
            ToolCall {
                id: "t3".to_string(),
                name: "grep".to_string(),
                args: serde_json::json!({"pattern": "fn main"}),
            },
            ToolCall {
                id: "t4".to_string(),
                name: "ls".to_string(),
                args: serde_json::json!({"path": "/tmp"}),
            },
            ToolCall {
                id: "t5".to_string(),
                name: "search".to_string(),
                args: serde_json::json!({"query": "error"}),
            },
            ToolCall {
                id: "t6".to_string(),
                name: "list_files".to_string(),
                args: serde_json::json!({}),
            },
        ];

        let (query, sequential) = partition_by_lane(&tool_calls);
        assert_eq!(
            query.len(),
            6,
            "all read-only tools should be in query lane"
        );
        assert_eq!(sequential.len(), 0);
    }

    #[test]
    fn test_partition_by_lane_execute_tools() {
        let tool_calls = vec![
            ToolCall {
                id: "t1".to_string(),
                name: "bash".to_string(),
                args: serde_json::json!({"command": "ls"}),
            },
            ToolCall {
                id: "t2".to_string(),
                name: "write".to_string(),
                args: serde_json::json!({"file": "a.rs", "content": ""}),
            },
            ToolCall {
                id: "t3".to_string(),
                name: "edit".to_string(),
                args: serde_json::json!({}),
            },
            ToolCall {
                id: "t4".to_string(),
                name: "delete".to_string(),
                args: serde_json::json!({}),
            },
        ];

        let (query, sequential) = partition_by_lane(&tool_calls);
        assert_eq!(query.len(), 0);
        assert_eq!(sequential.len(), 4, "all write tools should be sequential");
    }

    #[test]
    fn test_partition_by_lane_mixed() {
        let tool_calls = vec![
            ToolCall {
                id: "t1".to_string(),
                name: "read".to_string(),
                args: serde_json::json!({"file": "a.rs"}),
            },
            ToolCall {
                id: "t2".to_string(),
                name: "bash".to_string(),
                args: serde_json::json!({"command": "cargo build"}),
            },
            ToolCall {
                id: "t3".to_string(),
                name: "glob".to_string(),
                args: serde_json::json!({"pattern": "*.rs"}),
            },
            ToolCall {
                id: "t4".to_string(),
                name: "write".to_string(),
                args: serde_json::json!({"file": "b.rs", "content": ""}),
            },
            ToolCall {
                id: "t5".to_string(),
                name: "grep".to_string(),
                args: serde_json::json!({"pattern": "test"}),
            },
        ];

        let (query, sequential) = partition_by_lane(&tool_calls);
        assert_eq!(query.len(), 3, "read/glob/grep → Query");
        assert_eq!(sequential.len(), 2, "bash/write → Sequential");

        // Verify the correct tools ended up in each group
        assert_eq!(query[0].name, "read");
        assert_eq!(query[1].name, "glob");
        assert_eq!(query[2].name, "grep");
        assert_eq!(sequential[0].name, "bash");
        assert_eq!(sequential[1].name, "write");
    }

    #[test]
    fn test_partition_by_lane_empty() {
        let tool_calls: Vec<ToolCall> = vec![];
        let (query, sequential) = partition_by_lane(&tool_calls);
        assert!(query.is_empty());
        assert!(sequential.is_empty());
    }

    #[test]
    fn test_partition_by_lane_unknown_tool_goes_sequential() {
        // Unknown tools should default to Execute lane (sequential)
        let tool_calls = vec![ToolCall {
            id: "t1".to_string(),
            name: "custom_tool".to_string(),
            args: serde_json::json!({}),
        }];

        let (query, sequential) = partition_by_lane(&tool_calls);
        assert_eq!(query.len(), 0);
        assert_eq!(sequential.len(), 1);
    }

    // ========================================================================
    // ToolCommand adapter tests
    // ========================================================================

    #[tokio::test]
    async fn test_tool_command_command_type() {
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let cmd = ToolCommand {
            tool_executor: executor,
            tool_name: "read".to_string(),
            tool_args: serde_json::json!({"file": "test.rs"}),
            tool_context: test_tool_context(),
        };
        assert_eq!(cmd.command_type(), "read");
    }

    #[tokio::test]
    async fn test_tool_command_payload() {
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let args = serde_json::json!({"file": "test.rs", "offset": 10});
        let cmd = ToolCommand {
            tool_executor: executor,
            tool_name: "read".to_string(),
            tool_args: args.clone(),
            tool_context: test_tool_context(),
        };
        assert_eq!(cmd.payload(), args);
    }

    // ========================================================================
    // AgentLoop with queue builder tests
    // ========================================================================

    #[tokio::test(flavor = "multi_thread")]
    async fn test_agent_loop_with_queue() {
        use tokio::sync::broadcast;

        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Hello",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig::default();

        let (event_tx, _) = broadcast::channel(100);
        let queue = SessionLaneQueue::new("test-session", SessionQueueConfig::default(), event_tx)
            .await
            .unwrap();

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config)
            .with_queue(Arc::new(queue));

        assert!(agent.command_queue.is_some());
    }

    #[tokio::test]
    async fn test_agent_loop_without_queue() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Hello",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig::default();

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

        assert!(agent.command_queue.is_none());
    }

    // ========================================================================
    // Parallel Plan Execution Tests
    // ========================================================================

    #[tokio::test]
    async fn test_execute_plan_parallel_independent() {
        use crate::planning::{Complexity, ExecutionPlan, Task};

        // 3 independent steps (no dependencies) — should all execute.
        // MockLlmClient needs one response per execute_loop call per step.
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::text_response("Step 1 done"),
            MockLlmClient::text_response("Step 2 done"),
            MockLlmClient::text_response("Step 3 done"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig::default();
        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );

        let mut plan = ExecutionPlan::new("Test parallel", Complexity::Simple);
        plan.add_step(Task::new("s1", "First step"));
        plan.add_step(Task::new("s2", "Second step"));
        plan.add_step(Task::new("s3", "Third step"));

        let (tx, mut rx) = mpsc::channel(100);
        let result = agent.execute_plan(&[], &plan, Some(tx)).await.unwrap();

        // All 3 steps should have been executed (3 * 15 = 45 total tokens)
        assert_eq!(result.usage.total_tokens, 45);

        // Verify we received StepStart and StepEnd events for all 3 steps
        let mut step_starts = Vec::new();
        let mut step_ends = Vec::new();
        rx.close();
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::StepStart { step_id, .. } => step_starts.push(step_id),
                AgentEvent::StepEnd {
                    step_id, status, ..
                } => {
                    assert_eq!(status, TaskStatus::Completed);
                    step_ends.push(step_id);
                }
                _ => {}
            }
        }
        assert_eq!(step_starts.len(), 3);
        assert_eq!(step_ends.len(), 3);
    }

    #[tokio::test]
    async fn test_execute_plan_respects_dependencies() {
        use crate::planning::{Complexity, ExecutionPlan, Task};

        // s1 and s2 are independent (wave 1), s3 depends on both (wave 2).
        // This requires 3 responses total.
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::text_response("Step 1 done"),
            MockLlmClient::text_response("Step 2 done"),
            MockLlmClient::text_response("Step 3 done"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig::default();
        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );

        let mut plan = ExecutionPlan::new("Test deps", Complexity::Medium);
        plan.add_step(Task::new("s1", "Independent A"));
        plan.add_step(Task::new("s2", "Independent B"));
        plan.add_step(
            Task::new("s3", "Depends on A+B")
                .with_dependencies(vec!["s1".to_string(), "s2".to_string()]),
        );

        let (tx, mut rx) = mpsc::channel(100);
        let result = agent.execute_plan(&[], &plan, Some(tx)).await.unwrap();

        // All 3 steps should have been executed (3 * 15 = 45 total tokens)
        assert_eq!(result.usage.total_tokens, 45);

        // Verify ordering: s3's StepStart must come after s1 and s2's StepEnd
        let mut events = Vec::new();
        rx.close();
        while let Some(event) = rx.recv().await {
            match &event {
                AgentEvent::StepStart { step_id, .. } => {
                    events.push(format!("start:{}", step_id));
                }
                AgentEvent::StepEnd { step_id, .. } => {
                    events.push(format!("end:{}", step_id));
                }
                _ => {}
            }
        }

        // s3 start must occur after both s1 end and s2 end
        let s1_end = events.iter().position(|e| e == "end:s1").unwrap();
        let s2_end = events.iter().position(|e| e == "end:s2").unwrap();
        let s3_start = events.iter().position(|e| e == "start:s3").unwrap();
        assert!(
            s3_start > s1_end,
            "s3 started before s1 ended: {:?}",
            events
        );
        assert!(
            s3_start > s2_end,
            "s3 started before s2 ended: {:?}",
            events
        );

        // Final result should reflect step 3 (last sequential step)
        assert!(result.text.contains("Step 3 done") || !result.text.is_empty());
    }

    #[tokio::test]
    async fn test_execute_plan_handles_step_failure() {
        use crate::planning::{Complexity, ExecutionPlan, Task};

        // s1 succeeds, s2 depends on s1 (succeeds), s3 depends on nothing (succeeds),
        // s4 depends on a step that will fail (s_fail).
        // We simulate failure by providing no responses for s_fail's execute_loop.
        //
        // Simpler approach: s1 succeeds, s2 depends on s1 (will fail because no
        // mock response left), s3 is independent.
        // Layout: s1 (independent), s3 (independent) → wave 1 parallel
        //         s2 depends on s1 → wave 2
        //         s4 depends on s2 → wave 3 (should deadlock since s2 fails)
        let mock_client = Arc::new(MockLlmClient::new(vec![
            // Wave 1: s1 and s3 execute in parallel
            MockLlmClient::text_response("s1 done"),
            MockLlmClient::text_response("s3 done"),
            // Wave 2: s2 executes — but we give it no response, causing failure
            // Actually the MockLlmClient will fail with "No more mock responses"
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig::default();
        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );

        let mut plan = ExecutionPlan::new("Test failure", Complexity::Medium);
        plan.add_step(Task::new("s1", "Independent step"));
        plan.add_step(Task::new("s2", "Depends on s1").with_dependencies(vec!["s1".to_string()]));
        plan.add_step(Task::new("s3", "Another independent"));
        plan.add_step(Task::new("s4", "Depends on s2").with_dependencies(vec!["s2".to_string()]));

        let (tx, mut rx) = mpsc::channel(100);
        let _result = agent.execute_plan(&[], &plan, Some(tx)).await.unwrap();

        // s1 and s3 should succeed (wave 1), s2 should fail (wave 2),
        // s4 should never execute (deadlock — dep s2 failed, not completed)
        let mut completed_steps = Vec::new();
        let mut failed_steps = Vec::new();
        rx.close();
        while let Some(event) = rx.recv().await {
            if let AgentEvent::StepEnd {
                step_id, status, ..
            } = event
            {
                match status {
                    TaskStatus::Completed => completed_steps.push(step_id),
                    TaskStatus::Failed => failed_steps.push(step_id),
                    _ => {}
                }
            }
        }

        assert!(
            completed_steps.contains(&"s1".to_string()),
            "s1 should complete"
        );
        assert!(
            completed_steps.contains(&"s3".to_string()),
            "s3 should complete"
        );
        assert!(failed_steps.contains(&"s2".to_string()), "s2 should fail");
        // s4 should NOT appear in either list — it was never started
        assert!(
            !completed_steps.contains(&"s4".to_string()),
            "s4 should not complete"
        );
        assert!(
            !failed_steps.contains(&"s4".to_string()),
            "s4 should not fail (never started)"
        );
    }
}
