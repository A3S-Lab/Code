//! Agent Loop Implementation
//!
//! The agent loop handles the core conversation cycle:
//! 1. User sends a prompt
//! 2. LLM generates a response (possibly with tool calls)
//! 3. If tool calls present, execute them and send results back
//! 4. Repeat until LLM returns without tool calls
//!
//! This implements agentic behavior where the LLM can use tools
//! to accomplish tasks autonomously.

use crate::hitl::ConfirmationManager;
use crate::llm::{LlmClient, LlmResponse, Message, TokenUsage, ToolDefinition};
use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::tools::ToolExecutor;
use crate::context::{ContextProvider, ContextQuery, ContextResult};
use crate::planning::{AgentGoal, Complexity, ExecutionPlan, PlanStep, StepStatus};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
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
        }
    }
}

/// Events emitted during agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
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
    // Todo tracking events
    // ========================================================================

    /// Todo list updated
    #[serde(rename = "todo_updated")]
    TodoUpdated {
        session_id: String,
        todos: Vec<crate::todo::Todo>,
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
        status: StepStatus,
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
}

/// Result of agent execution
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AgentResult {
    pub text: String,
    pub messages: Vec<Message>,
    pub usage: TokenUsage,
    pub tool_calls_count: usize,
}

/// Agent loop executor
pub struct AgentLoop {
    llm_client: Arc<dyn LlmClient>,
    tool_executor: Arc<ToolExecutor>,
    config: AgentConfig,
}

impl AgentLoop {
    pub fn new(
        llm_client: Arc<dyn LlmClient>,
        tool_executor: Arc<ToolExecutor>,
        config: AgentConfig,
    ) -> Self {
        Self {
            llm_client,
            tool_executor,
            config,
        }
    }

    /// Resolve context from all providers for a given prompt
    ///
    /// Returns aggregated context results from all configured providers.
    async fn resolve_context(&self, prompt: &str, session_id: Option<&str>) -> Vec<ContextResult> {
        if self.config.context_providers.is_empty() {
            return Vec::new();
        }

        let query = ContextQuery::new(prompt).with_session_id(session_id.unwrap_or(""));

        let mut results = Vec::new();
        for provider in &self.config.context_providers {
            match provider.query(&query).await {
                Ok(result) => {
                    if !result.is_empty() {
                        results.push(result);
                    }
                }
                Err(e) => {
                    tracing::warn!("Context provider '{}' failed: {}", provider.name(), e);
                }
            }
        }
        results
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
        for provider in &self.config.context_providers {
            if let Err(e) = provider
                .on_turn_complete(session_id, prompt, response)
                .await
            {
                tracing::warn!(
                    "Context provider '{}' on_turn_complete failed: {}",
                    provider.name(),
                    e
                );
            }
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
        let augmented_system = if !self.config.context_providers.is_empty() {
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

            let context_results = self.resolve_context(prompt, session_id).await;

            // Send context resolved event
            if let Some(tx) = &event_tx {
                let total_items: usize = context_results.iter().map(|r| r.items.len()).sum();
                let total_tokens: usize = context_results.iter().map(|r| r.total_tokens).sum();
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

            // Call LLM - use streaming if we have an event channel
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

            // Execute tools
            for tool_call in tool_calls {
                tool_calls_count += 1;

                // Send tool start event (only if not already sent during streaming)
                // In streaming mode, ToolStart is sent when we receive ToolUseStart from LLM
                // But we still need to send ToolEnd after execution

                // Check permission before executing tool
                let permission_decision = if let Some(policy_lock) = &self.config.permission_policy
                {
                    let policy = policy_lock.read().await;
                    policy.check(&tool_call.name, &tool_call.args)
                } else {
                    PermissionDecision::Allow // No policy = allow all
                };

                let (output, exit_code, is_error) = match permission_decision {
                    PermissionDecision::Deny => {
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

                        (denial_msg, 1, true)
                    }
                    PermissionDecision::Ask => {
                        // HITL: Check if this tool requires confirmation
                        if let Some(cm) = &self.config.confirmation_manager {
                            // First check if this tool actually requires confirmation
                            // (considers HITL enabled, YOLO lanes, auto-approve lists, etc.)
                            if !cm.requires_confirmation(&tool_call.name).await {
                                // No confirmation needed - execute directly
                                let result = self
                                    .tool_executor
                                    .execute(&tool_call.name, &tool_call.args)
                                    .await;

                                let (output, exit_code, is_error) = match result {
                                    Ok(r) => (r.output, r.exit_code, r.exit_code != 0),
                                    Err(e) => (format!("Tool execution error: {}", e), 1, true),
                                };

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
                                messages.push(Message::tool_result(
                                    &tool_call.id,
                                    &output,
                                    is_error,
                                ));
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
                                    // Got confirmation response
                                    if response.approved {
                                        // Approved: execute the tool
                                        let result = self
                                            .tool_executor
                                            .execute(&tool_call.name, &tool_call.args)
                                            .await;

                                        match result {
                                            Ok(r) => (r.output, r.exit_code, r.exit_code != 0),
                                            Err(e) => {
                                                (format!("Tool execution error: {}", e), 1, true)
                                            }
                                        }
                                    } else {
                                        // Rejected by user
                                        let rejection_msg = format!(
                                            "Tool '{}' execution was rejected by user. Reason: {}",
                                            tool_call.name,
                                            response.reason.unwrap_or_else(|| "No reason provided".to_string())
                                        );
                                        (rejection_msg, 1, true)
                                    }
                                }
                                Ok(Err(_)) => {
                                    // Channel closed (confirmation manager dropped)
                                    let msg = format!(
                                        "Tool '{}' confirmation failed: confirmation channel closed",
                                        tool_call.name
                                    );
                                    (msg, 1, true)
                                }
                                Err(_) => {
                                    // Timeout - check timeout action
                                    // Note: check_timeouts() should be called by a background task,
                                    // but we handle it here as well for safety
                                    cm.check_timeouts().await;

                                    match timeout_action {
                                        crate::hitl::TimeoutAction::Reject => {
                                            let msg = format!(
                                                "Tool '{}' execution timed out waiting for confirmation ({}ms). Execution rejected.",
                                                tool_call.name, timeout_ms
                                            );
                                            (msg, 1, true)
                                        }
                                        crate::hitl::TimeoutAction::AutoApprove => {
                                            // Auto-approve on timeout: execute the tool
                                            let result = self
                                                .tool_executor
                                                .execute(&tool_call.name, &tool_call.args)
                                                .await;

                                            match result {
                                                Ok(r) => (r.output, r.exit_code, r.exit_code != 0),
                                                Err(e) => (
                                                    format!("Tool execution error: {}", e),
                                                    1,
                                                    true,
                                                ),
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            // No confirmation manager configured, treat as Allow
                            let result = self
                                .tool_executor
                                .execute(&tool_call.name, &tool_call.args)
                                .await;

                            match result {
                                Ok(r) => (r.output, r.exit_code, r.exit_code != 0),
                                Err(e) => (format!("Tool execution error: {}", e), 1, true),
                            }
                        }
                    }
                    PermissionDecision::Allow => {
                        // Execute the tool
                        let result = self
                            .tool_executor
                            .execute(&tool_call.name, &tool_call.args)
                            .await;

                        match result {
                            Ok(r) => (r.output, r.exit_code, r.exit_code != 0),
                            Err(e) => (format!("Tool execution error: {}", e), 1, true),
                        }
                    }
                };

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
        let config = self.config.clone();
        let history = history.to_vec();
        let prompt = prompt.to_string();

        let handle = tokio::spawn(async move {
            let agent = AgentLoop::new(llm_client, tool_executor, config);
            agent.execute(&history, &prompt, Some(tx)).await
        });

        Ok((rx, handle))
    }

    /// Analyze prompt complexity
    async fn analyze_complexity(&self, prompt: &str) -> Result<Complexity> {
        // Use LLM to analyze complexity
        let analysis_prompt = format!(
            "Analyze the complexity of this task and respond with ONLY one word: Simple, Medium, Complex, or VeryComplex\n\nTask: {}",
            prompt
        );

        let response = self
            .llm_client
            .complete(
                &[Message::user(&analysis_prompt)],
                Some("You are a task complexity analyzer. Respond with only one word."),
                &[],
            )
            .await?;

        let text = response.text().to_lowercase();
        let complexity = if text.contains("simple") {
            Complexity::Simple
        } else if text.contains("medium") {
            Complexity::Medium
        } else if text.contains("verycomplex") || text.contains("very complex") {
            Complexity::VeryComplex
        } else if text.contains("complex") {
            Complexity::Complex
        } else {
            Complexity::Medium // Default
        };

        Ok(complexity)
    }

    /// Create an execution plan for a prompt
    pub async fn plan(&self, prompt: &str, context: Option<&str>) -> Result<ExecutionPlan> {
        // Analyze complexity first
        let complexity = self.analyze_complexity(prompt).await?;

        // Create planning prompt
        let planning_prompt = if let Some(ctx) = context {
            format!(
                "Create a detailed execution plan for the following task.\n\nContext: {}\n\nTask: {}\n\nProvide a step-by-step plan with:\n1. Clear goal\n2. Numbered steps\n3. Required tools for each step\n4. Dependencies between steps\n\nFormat your response as:\nGOAL: <goal description>\nSTEPS:\n1. [tool: <tool_name>] <step description>\n2. [tool: <tool_name>] <step description> (depends on: 1)\n...",
                ctx, prompt
            )
        } else {
            format!(
                "Create a detailed execution plan for the following task.\n\nTask: {}\n\nProvide a step-by-step plan with:\n1. Clear goal\n2. Numbered steps\n3. Required tools for each step\n4. Dependencies between steps\n\nFormat your response as:\nGOAL: <goal description>\nSTEPS:\n1. [tool: <tool_name>] <step description>\n2. [tool: <tool_name>] <step description> (depends on: 1)\n...",
                prompt
            )
        };

        let response = self
            .llm_client
            .complete(
                &[Message::user(&planning_prompt)],
                Some("You are a planning assistant. Create clear, actionable execution plans."),
                &[],
            )
            .await?;

        // Parse the plan from LLM response
        let plan_text = response.text();
        let plan = self.parse_plan(&plan_text, complexity)?;

        Ok(plan)
    }

    /// Parse execution plan from LLM response
    fn parse_plan(&self, plan_text: &str, complexity: Complexity) -> Result<ExecutionPlan> {
        let lines: Vec<&str> = plan_text.lines().collect();

        // Extract goal
        let goal = lines
            .iter()
            .find(|line| line.to_lowercase().starts_with("goal:"))
            .map(|line| line.split(':').nth(1).unwrap_or("").trim().to_string())
            .unwrap_or_else(|| "Complete the task".to_string());

        let mut plan = ExecutionPlan::new(goal, complexity);

        // Find STEPS section
        let steps_start = lines
            .iter()
            .position(|line| line.to_lowercase().contains("steps:"))
            .unwrap_or(0);

        // Parse steps
        for line in lines.iter().skip(steps_start + 1) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Match pattern: "1. [tool: bash] Description (depends on: 0)"
            if let Some(step_num_end) = line.find('.') {
                let step_id = format!("step-{}", line[..step_num_end].trim());
                let rest = &line[step_num_end + 1..].trim();

                // Extract tool
                let tool = if rest.starts_with('[') {
                    if let Some(tool_end) = rest.find(']') {
                        let tool_part = &rest[1..tool_end];
                        if let Some(colon) = tool_part.find(':') {
                            Some(tool_part[colon + 1..].trim().to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Extract description and dependencies
                let desc_start = if tool.is_some() {
                    rest.find(']').map(|i| i + 1).unwrap_or(0)
                } else {
                    0
                };

                let desc_part = &rest[desc_start..].trim();
                let (description, dependencies) = if let Some(depends_pos) = desc_part.find("(depends on:") {
                    let desc = desc_part[..depends_pos].trim().to_string();
                    let deps_str = &desc_part[depends_pos + 12..];
                    let deps_end = deps_str.find(')').unwrap_or(deps_str.len());
                    let deps: Vec<String> = deps_str[..deps_end]
                        .split(',')
                        .map(|d| format!("step-{}", d.trim()))
                        .collect();
                    (desc, deps)
                } else {
                    (desc_part.to_string(), Vec::new())
                };

                let mut step = PlanStep::new(step_id, description);
                if let Some(t) = tool {
                    step = step.with_tool(t.clone());
                    plan.add_required_tool(t);
                }
                if !dependencies.is_empty() {
                    step = step.with_dependencies(dependencies);
                }

                plan.add_step(step);
            }
        }

        // If no steps were parsed, create a simple single-step plan
        if plan.steps.is_empty() {
            plan.add_step(PlanStep::new("step-1", "Execute the task"));
        }

        Ok(plan)
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

    /// Execute an execution plan
    async fn execute_plan(
        &self,
        history: &[Message],
        plan: &ExecutionPlan,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentResult> {
        let mut current_history = history.to_vec();
        let mut total_usage = TokenUsage::default();
        let mut tool_calls_count = 0;

        // Add initial user message with the goal
        current_history.push(Message::user(&format!(
            "Goal: {}\n\nExecute the following plan step by step:\n{}",
            plan.goal,
            plan.steps
                .iter()
                .enumerate()
                .map(|(i, step)| format!("{}. {}", i + 1, step.description))
                .collect::<Vec<_>>()
                .join("\n")
        )));

        // Execute each step
        for (step_idx, step) in plan.steps.iter().enumerate() {
            // Send step start event
            if let Some(tx) = &event_tx {
                tx.send(AgentEvent::StepStart {
                    step_id: step.id.clone(),
                    description: step.description.clone(),
                    step_number: step_idx + 1,
                    total_steps: plan.steps.len(),
                })
                .await
                .ok();
            }

            // Execute this step
            let step_prompt = format!("Execute step {}: {}", step_idx + 1, step.description);
            let step_result = self
                .execute(&current_history, &step_prompt, event_tx.clone())
                .await?;

            // Update history and usage
            current_history = step_result.messages.clone();
            total_usage.prompt_tokens += step_result.usage.prompt_tokens;
            total_usage.completion_tokens += step_result.usage.completion_tokens;
            total_usage.total_tokens += step_result.usage.total_tokens;
            tool_calls_count += step_result.tool_calls_count;

            // Send step end event
            if let Some(tx) = &event_tx {
                tx.send(AgentEvent::StepEnd {
                    step_id: step.id.clone(),
                    status: StepStatus::Completed,
                    step_number: step_idx + 1,
                    total_steps: plan.steps.len(),
                })
                .await
                .ok();
            }

            // Send progress event if goal tracking is enabled
            if self.config.goal_tracking {
                if let Some(tx) = &event_tx {
                    tx.send(AgentEvent::GoalProgress {
                        goal: plan.goal.clone(),
                        progress: (step_idx + 1) as f32 / plan.steps.len() as f32,
                        completed_steps: step_idx + 1,
                        total_steps: plan.steps.len(),
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
    pub async fn extract_goal(&self, prompt: &str) -> Result<AgentGoal> {
        let goal_prompt = format!(
            "Extract the main goal from this task. Respond in this format:\nGOAL: <goal description>\nCRITERIA:\n- <success criterion 1>\n- <success criterion 2>\n\nTask: {}",
            prompt
        );

        let response = self
            .llm_client
            .complete(
                &[Message::user(&goal_prompt)],
                Some("You are a goal extraction assistant."),
                &[],
            )
            .await?;

        let text = response.text();
        let lines: Vec<&str> = text.lines().collect();

        // Extract goal
        let goal_desc = lines
            .iter()
            .find(|line| line.to_lowercase().starts_with("goal:"))
            .map(|line| line.split(':').nth(1).unwrap_or("").trim().to_string())
            .unwrap_or_else(|| prompt.to_string());

        // Extract criteria
        let criteria_start = lines
            .iter()
            .position(|line| line.to_lowercase().contains("criteria:"))
            .unwrap_or(lines.len());

        let criteria: Vec<String> = lines
            .iter()
            .skip(criteria_start + 1)
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with('-') || trimmed.starts_with('•') {
                    Some(trimmed[1..].trim().to_string())
                } else {
                    None
                }
            })
            .collect();

        Ok(AgentGoal::new(goal_desc).with_criteria(criteria))
    }

    /// Check if goal is achieved
    pub async fn check_goal_achievement(
        &self,
        goal: &AgentGoal,
        current_state: &str,
    ) -> Result<bool> {
        let check_prompt = format!(
            "Goal: {}\n\nSuccess Criteria:\n{}\n\nCurrent State:\n{}\n\nIs the goal achieved? Respond with only YES or NO.",
            goal.description,
            goal.success_criteria
                .iter()
                .map(|c| format!("- {}", c))
                .collect::<Vec<_>>()
                .join("\n"),
            current_state
        );

        let response = self
            .llm_client
            .complete(
                &[Message::user(&check_prompt)],
                Some("You are a goal achievement checker. Respond with only YES or NO."),
                &[],
            )
            .await?;

        let text = response.text().to_lowercase();
        Ok(text.contains("yes"))
    }
}

/// Builder for creating an agent
#[allow(dead_code)]
pub struct AgentBuilder {
    llm_client: Option<Arc<dyn LlmClient>>,
    tool_executor: Option<Arc<ToolExecutor>>,
    config: AgentConfig,
}

#[allow(dead_code)]
impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            llm_client: None,
            tool_executor: None,
            config: AgentConfig::default(),
        }
    }

    pub fn llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.llm_client = Some(client);
        self
    }

    pub fn tool_executor(mut self, executor: Arc<ToolExecutor>) -> Self {
        self.tool_executor = Some(executor);
        self
    }

    pub fn system_prompt(mut self, prompt: &str) -> Self {
        self.config.system_prompt = Some(prompt.to_string());
        self
    }

    pub fn tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.config.tools = tools;
        self
    }

    pub fn max_tool_rounds(mut self, max: usize) -> Self {
        self.config.max_tool_rounds = max;
        self
    }

    pub fn build(self) -> Result<AgentLoop> {
        let llm_client = self.llm_client.context("LLM client is required")?;
        let tool_executor = self.tool_executor.context("Tool executor is required")?;

        Ok(AgentLoop::new(llm_client, tool_executor, self.config))
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ContentBlock, StreamEvent};
    use crate::permissions::PermissionPolicy;
    use crate::tools::ToolExecutor;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    struct MockLlmClient {
        /// Responses to return (consumed in order)
        responses: std::sync::Mutex<Vec<LlmResponse>>,
        /// Number of calls made
        call_count: AtomicUsize,
    }

    impl MockLlmClient {
        fn new(responses: Vec<LlmResponse>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses),
                call_count: AtomicUsize::new(0),
            }
        }

        /// Create a response with text only (no tool calls)
        fn text_response(text: &str) -> LlmResponse {
            LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: text.to_string(),
                    }],
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
        fn tool_call_response(
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

        let agent = AgentLoop::new(mock_client.clone(), tool_executor, config);
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

        let agent = AgentLoop::new(mock_client.clone(), tool_executor, config);
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
        let agent = AgentLoop::new(mock_client.clone(), tool_executor, config);
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

        let agent = AgentLoop::new(mock_client.clone(), tool_executor, config);
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);
        let result = agent.execute(&[], "Loop forever", None).await;

        // Should fail due to max tool rounds exceeded
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Max tool rounds"));
    }

    #[tokio::test]
    async fn test_agent_no_permission_policy() {
        // When no permission policy is set, all tools should be allowed
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "rm -rf /tmp/test"}),
            ),
            MockLlmClient::text_response("Done!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            permission_policy: None, // No policy
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, config);
        let result = agent.execute(&[], "Delete", None).await.unwrap();

        // Should execute without permission denied
        assert_eq!(result.text, "Done!");
        assert_eq!(result.tool_calls_count, 1);
    }

    #[tokio::test]
    async fn test_agent_permission_ask_executes() {
        // When permission is Ask (and no HITL), it should execute the tool
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo test"}),
            ),
            MockLlmClient::text_response("Done!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create policy where bash falls through to Ask (default)
        let permission_policy = PermissionPolicy::new(); // Default decision is Ask
        let policy_lock = Arc::new(RwLock::new(permission_policy));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, config);
        let result = agent.execute(&[], "Echo", None).await.unwrap();

        // Should execute (Ask without HITL = execute)
        assert_eq!(result.text, "Done!");
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);
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
        let agent = AgentLoop::new(mock_client, tool_executor, config);
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
        let agent = AgentLoop::new(mock_client, tool_executor, config);
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);
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
        // When permission is Allow, HITL should not be triggered
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
            confirmation_manager: Some(confirmation_manager),
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, config);
        let result = agent.execute(&[], "Echo", None).await.unwrap();

        // Should execute without HITL
        assert_eq!(result.text, "Allowed!");

        // Should NOT have any ConfirmationRequired events
        let mut found_confirmation = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, AgentEvent::ConfirmationRequired { .. }) {
                found_confirmation = true;
            }
        }
        assert!(
            !found_confirmation,
            "HITL should not be triggered when permission is Allow"
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);
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

        let agent = AgentLoop::new(mock_client.clone(), tool_executor, config);
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
        let agent = AgentLoop::new(mock_client, tool_executor, config);
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
        let agent = AgentLoop::new(mock_client, tool_executor, config);
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
        let agent = AgentLoop::new(mock_client, tool_executor, config);
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

        let agent = AgentLoop::new(mock_client, tool_executor, config);

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

        let agent = AgentLoop::new(mock_client, tool_executor, config);

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

        let agent = AgentLoop::new(mock_client, tool_executor, config);

        // Test building augmented prompt
        let context_results = agent.resolve_context("test", None).await;
        let augmented = agent.build_augmented_system_prompt(&context_results);

        let augmented_str = augmented.unwrap();
        assert!(augmented_str.contains("You are helpful."));
        assert!(augmented_str.contains("<context source=\"viking://docs/auth\" type=\"Resource\">"));
        assert!(augmented_str.contains("Auth uses JWT tokens."));
    }
}
