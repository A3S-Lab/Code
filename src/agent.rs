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
use crate::llm::{LlmClient, LlmResponse, Message, TokenUsage, ToolDefinition};
use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::planning::{AgentGoal, Complexity, ExecutionPlan, PlanStep, StepStatus};
use crate::tools::skill::Skill;
use crate::tools::{ToolContext, ToolExecutor};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tracing::Instrument;

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
    /// Loaded skills with allowed_tools restrictions.
    /// When non-empty, tool calls are checked against each skill's
    /// `allowed_tools` list. A tool is allowed if ANY skill permits it,
    /// or if no skill has an `allowed_tools` restriction.
    pub skill_tool_filters: Vec<Skill>,
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
            .field("skill_tool_filters", &self.skill_tool_filters.len())
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
            skill_tool_filters: Vec::new(),
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
    tool_context: ToolContext,
    config: AgentConfig,
    /// Optional per-session tool metrics collector
    tool_metrics: Option<Arc<RwLock<crate::telemetry::ToolMetrics>>>,
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
        }
    }

    /// Set the tool metrics collector for this agent loop
    pub fn with_tool_metrics(mut self, metrics: Arc<RwLock<crate::telemetry::ToolMetrics>>) -> Self {
        self.tool_metrics = Some(metrics);
        self
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
    #[tracing::instrument(
        name = "a3s.agent.execute",
        skip(self, history, prompt, event_tx),
        fields(
            a3s.session.id = session_id.unwrap_or("none"),
            a3s.agent.max_turns = self.config.max_tool_rounds,
            a3s.agent.tool_calls_count = tracing::field::Empty,
            a3s.llm.total_tokens = tracing::field::Empty,
        )
    )]
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

            let context_results = {
                let context_span = tracing::info_span!(
                    "a3s.agent.context_resolve",
                    a3s.context.providers = self.config.context_providers.len() as i64,
                    a3s.context.items = tracing::field::Empty,
                    a3s.context.tokens = tracing::field::Empty,
                );

                self.resolve_context(prompt, session_id)
                    .instrument(context_span)
                    .await
            };

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

            let turn_span = tracing::info_span!(
                "a3s.agent.turn",
                a3s.agent.turn_number = turn as i64,
                a3s.llm.total_tokens = tracing::field::Empty,
            );
            let _turn_guard = turn_span.enter();

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
            let llm_span = tracing::info_span!(
                "a3s.llm.completion",
                a3s.llm.streaming = event_tx.is_some(),
                a3s.llm.prompt_tokens = tracing::field::Empty,
                a3s.llm.completion_tokens = tracing::field::Empty,
                a3s.llm.total_tokens = tracing::field::Empty,
                a3s.llm.stop_reason = tracing::field::Empty,
            );
            let _llm_guard = llm_span.enter();

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

            // Record LLM usage on the llm span
            crate::telemetry::record_llm_usage(
                response.usage.prompt_tokens,
                response.usage.completion_tokens,
                response.usage.total_tokens,
                response.stop_reason.as_deref(),
            );
            drop(_llm_guard);

            // Record total tokens on the turn span
            turn_span.record("a3s.llm.total_tokens", response.usage.total_tokens as i64);

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

            // Execute tools
            for tool_call in tool_calls {
                tool_calls_count += 1;

                let tool_span = tracing::info_span!(
                    "a3s.tool.execute",
                    a3s.tool.name = tool_call.name.as_str(),
                    a3s.tool.id = tool_call.id.as_str(),
                    a3s.tool.exit_code = tracing::field::Empty,
                    a3s.tool.success = tracing::field::Empty,
                    a3s.tool.duration_ms = tracing::field::Empty,
                    a3s.tool.permission = tracing::field::Empty,
                );
                let _tool_guard = tool_span.enter();

                let tool_start = std::time::Instant::now();

                tracing::info!(
                    tool_name = tool_call.name.as_str(),
                    tool_id = tool_call.id.as_str(),
                    "Tool execution started"
                );

                // Send tool start event (only if not already sent during streaming)
                // In streaming mode, ToolStart is sent when we receive ToolUseStart from LLM
                // But we still need to send ToolEnd after execution

                // Enforce skill allowed_tools restrictions.
                // If any loaded skill has an allowed_tools list, check that the tool
                // is permitted. A tool is allowed if: (a) no skill has restrictions,
                // or (b) at least one skill with restrictions permits the tool.
                if !self.config.skill_tool_filters.is_empty() {
                    let has_restrictions = self
                        .config
                        .skill_tool_filters
                        .iter()
                        .any(|s| s.allowed_tools.is_some());

                    if has_restrictions {
                        let args_str =
                            serde_json::to_string(&tool_call.args).unwrap_or_default();
                        let tool_allowed = self
                            .config
                            .skill_tool_filters
                            .iter()
                            .filter(|s| s.allowed_tools.is_some())
                            .any(|s| s.is_tool_allowed(&tool_call.name, &args_str));

                        if !tool_allowed {
                            tracing::info!(
                                tool_name = tool_call.name.as_str(),
                                "Tool blocked by skill allowed_tools restriction"
                            );
                            let msg = format!(
                                "Tool '{}' is not permitted by any loaded skill's allowed_tools policy.",
                                tool_call.name
                            );

                            if let Some(tx) = &event_tx {
                                tx.send(AgentEvent::PermissionDenied {
                                    tool_id: tool_call.id.clone(),
                                    tool_name: tool_call.name.clone(),
                                    args: tool_call.args.clone(),
                                    reason: "Blocked by skill allowed_tools restriction"
                                        .to_string(),
                                })
                                .await
                                .ok();
                            }

                            messages.push(Message::tool_result(
                                &tool_call.id,
                                &msg,
                                true,
                            ));
                            continue;
                        }
                    }
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

                let (output, exit_code, is_error) = match permission_decision {
                    PermissionDecision::Deny => {
                        tracing::info!(
                            tool_name = tool_call.name.as_str(),
                            permission = "deny",
                            "Tool permission denied"
                        );
                        tool_span.record("a3s.tool.permission", "deny");
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
                    // Both Allow and Ask go through HITL confirmation check.
                    // Permission Allow means "not denied by policy", but HITL
                    // confirmation is an independent safety layer that still applies
                    // for mutating operations.
                    PermissionDecision::Allow | PermissionDecision::Ask => {
                        let decision_str = if permission_decision == PermissionDecision::Allow {
                            "allow"
                        } else {
                            "ask"
                        };
                        tracing::info!(
                            tool_name = tool_call.name.as_str(),
                            permission = decision_str,
                            "Tool permission: {}",
                            decision_str
                        );
                        tool_span.record("a3s.tool.permission", decision_str);

                        // HITL: Check if this tool requires confirmation
                        if let Some(cm) = &self.config.confirmation_manager {
                            // Check if this tool actually requires confirmation
                            // (considers HITL enabled, YOLO lanes, auto-approve lists, etc.)
                            if !cm.requires_confirmation(&tool_call.name).await {
                                // No confirmation needed - execute directly
                                let result = self
                                    .tool_executor
                                    .execute_with_context(
                                        &tool_call.name,
                                        &tool_call.args,
                                        &self.tool_context,
                                    )
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

                                // Record tool result on the tool span for early exit
                                let tool_duration = tool_start.elapsed();
                                crate::telemetry::record_tool_result(exit_code, tool_duration);

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
                                            .execute_with_context(
                                                &tool_call.name,
                                                &tool_call.args,
                                                &self.tool_context,
                                            )
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
                                                .execute_with_context(
                                                    &tool_call.name,
                                                    &tool_call.args,
                                                    &self.tool_context,
                                                )
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
                            // No confirmation manager configured
                            if permission_decision == PermissionDecision::Allow {
                                // Permission explicitly allows and no CM - execute directly
                                let result = self
                                    .tool_executor
                                    .execute_with_context(
                                        &tool_call.name,
                                        &tool_call.args,
                                        &self.tool_context,
                                    )
                                    .await;

                                match result {
                                    Ok(r) => (r.output, r.exit_code, r.exit_code != 0),
                                    Err(e) => (format!("Tool execution error: {}", e), 1, true),
                                }
                            } else {
                                // Ask without confirmation manager - safe deny
                                let msg = format!(
                                    "Tool '{}' requires confirmation but no HITL confirmation manager is configured. \
                                     Configure a confirmation policy to enable tool execution.",
                                    tool_call.name
                                );
                                tracing::warn!(
                                    tool_name = tool_call.name.as_str(),
                                    "Tool requires confirmation but no HITL manager configured"
                                );
                                (msg, 1, true)
                            }
                        }
                    }
                };

                // Record tool execution metrics
                let tool_duration = tool_start.elapsed();
                tracing::info!(
                    tool_name = tool_call.name.as_str(),
                    tool_id = tool_call.id.as_str(),
                    exit_code = exit_code,
                    success = (exit_code == 0),
                    duration_ms = tool_duration.as_millis() as u64,
                    "Tool execution finished"
                );

                // Record tool result on the tool span
                crate::telemetry::record_tool_result(exit_code, tool_duration);

                // Record to per-session tool metrics
                if let Some(ref metrics) = self.tool_metrics {
                    metrics.write().await.record(
                        &tool_call.name,
                        exit_code == 0,
                        tool_duration.as_millis() as u64,
                    );
                }

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
        let tool_context = self.tool_context.clone();
        let config = self.config.clone();
        let tool_metrics = self.tool_metrics.clone();
        let history = history.to_vec();
        let prompt = prompt.to_string();

        let handle = tokio::spawn(async move {
            let mut agent = AgentLoop::new(llm_client, tool_executor, tool_context, config);
            if let Some(metrics) = tool_metrics {
                agent = agent.with_tool_metrics(metrics);
            }
            agent.execute(&history, &prompt, Some(tx)).await
        });

        Ok((rx, handle))
    }

    /// Analyze prompt complexity
    async fn analyze_complexity(&self, prompt: &str) -> Result<Complexity> {
        // Use LLM to analyze complexity
        let analysis_prompt = crate::prompts::render(
            crate::prompts::COMPLEXITY_USER,
            &[("task", prompt)],
        );

        let response = self
            .llm_client
            .complete(
                &[Message::user(&analysis_prompt)],
                Some(crate::prompts::COMPLEXITY_SYSTEM),
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
        let context_section = if let Some(ctx) = context {
            format!("Context: {}\n\n", ctx)
        } else {
            String::new()
        };
        let planning_prompt = crate::prompts::render(
            crate::prompts::PLAN_USER,
            &[("context", &context_section), ("task", prompt)],
        );

        let response = self
            .llm_client
            .complete(
                &[Message::user(&planning_prompt)],
                Some(crate::prompts::PLAN_SYSTEM),
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
                    rest.find(']').and_then(|tool_end| {
                        let tool_part = &rest[1..tool_end];
                        tool_part
                            .find(':')
                            .map(|colon| tool_part[colon + 1..].trim().to_string())
                    })
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
                let (description, dependencies) =
                    if let Some(depends_pos) = desc_part.find("(depends on:") {
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
        let steps_text = plan.steps
            .iter()
            .enumerate()
            .map(|(i, step)| format!("{}. {}", i + 1, step.description))
            .collect::<Vec<_>>()
            .join("\n");
        current_history.push(Message::user(&crate::prompts::render(
            crate::prompts::PLAN_EXECUTE_GOAL,
            &[("goal", &plan.goal), ("steps", &steps_text)],
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
            let step_prompt = crate::prompts::render(
                crate::prompts::PLAN_EXECUTE_STEP,
                &[("step_num", &(step_idx + 1).to_string()), ("description", &step.description)],
            );
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
        let goal_prompt = crate::prompts::render(
            crate::prompts::GOAL_EXTRACT_USER,
            &[("task", prompt)],
        );

        let response = self
            .llm_client
            .complete(
                &[Message::user(&goal_prompt)],
                Some(crate::prompts::GOAL_EXTRACT_SYSTEM),
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
        let criteria_text = goal.success_criteria
            .iter()
            .map(|c| format!("- {}", c))
            .collect::<Vec<_>>()
            .join("\n");
        let check_prompt = crate::prompts::render(
            crate::prompts::GOAL_CHECK_USER,
            &[
                ("goal", &goal.description),
                ("criteria", &criteria_text),
                ("current_state", current_state),
            ],
        );

        let response = self
            .llm_client
            .complete(
                &[Message::user(&check_prompt)],
                Some(crate::prompts::GOAL_CHECK_SYSTEM),
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
    tool_context: Option<ToolContext>,
    config: AgentConfig,
}

#[allow(dead_code)]
impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            llm_client: None,
            tool_executor: None,
            tool_context: None,
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

    pub fn tool_context(mut self, ctx: ToolContext) -> Self {
        self.tool_context = Some(ctx);
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
        let tool_context = self
            .tool_context
            .unwrap_or_else(|| ToolContext::new(tool_executor.workspace().clone()));

        Ok(AgentLoop::new(
            llm_client,
            tool_executor,
            tool_context,
            self.config,
        ))
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
    async fn test_agent_hitl_with_permission_allow_still_checks_hitl() {
        // Even when permission is Allow, HITL confirmation should still be
        // triggered for mutating tools (defense-in-depth).
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

        // Spawn a task to approve the confirmation
        let cm_clone = confirmation_manager.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cm_clone.confirm("tool-1", true, None).await.ok();
        });

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Echo", None).await.unwrap();

        // Should execute after HITL approval
        assert_eq!(result.text, "Allowed!");

        // Should have ConfirmationRequired event (Allow no longer bypasses HITL)
        let mut found_confirmation = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, AgentEvent::ConfirmationRequired { .. }) {
                found_confirmation = true;
            }
        }
        assert!(
            found_confirmation,
            "HITL should be triggered even when permission is Allow (defense-in-depth)"
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
            skill_tool_filters: vec![],
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
        assert_eq!(result.messages[2].role, "user");      // tool result 1 (Anthropic convention)
        assert_eq!(result.messages[3].role, "assistant"); // tool call 2
        assert_eq!(result.messages[4].role, "user");      // tool result 2
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

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::text_response("Rust was created by Graydon Hoare at Mozilla."),
        ]));

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
        assert_eq!(result.messages[1].text(), "Rust is a systems programming language.");
        assert_eq!(result.messages[2].text(), "Who created it?");
        assert_eq!(result.messages[3].text(), "Rust was created by Graydon Hoare at Mozilla.");
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
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::text_response("I am a coding assistant."),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            system_prompt: Some("You are a coding assistant.".to_string()),
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client.clone(), tool_executor, test_tool_context(), config);
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

        let agent = AgentLoop::new(mock_client.clone(), tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Loop forever", None).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Max tool rounds (5) exceeded"));
    }

    #[tokio::test]
    async fn test_agent_end_event_contains_final_text() {
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::text_response("Final answer here"),
        ]));

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
            skill_tool_filters: vec![],
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
        assert!(config.skill_tool_filters.is_empty());
    }

    #[tokio::test]
    async fn test_agent_skill_tool_filters_blocks_unauthorized() {
        // When skills have allowed_tools restrictions, tools not in any
        // skill's allowed list should be blocked.
        use crate::tools::skill::Skill;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "rm -rf /"}),
            ),
            MockLlmClient::text_response("Blocked!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create a skill that only allows "read" tool
        let skill = Skill {
            name: "read-only".to_string(),
            description: "Read-only skill".to_string(),
            content: "Read files only".to_string(),
            allowed_tools: Some("read(*)".to_string()),
            disable_model_invocation: false,
        };

        // Use Allow policy so permission doesn't block first
        let policy = PermissionPolicy::new().allow("bash(*)");
        let policy_lock = Arc::new(RwLock::new(policy));

        // Use HITL disabled CM so it doesn't interfere
        let (event_tx, _) = tokio::sync::broadcast::channel(10);
        let cm = Arc::new(crate::hitl::ConfirmationManager::new(
            crate::hitl::ConfirmationPolicy::default(), // disabled
            event_tx,
        ));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(cm),
            skill_tool_filters: vec![skill],
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Delete", None).await.unwrap();

        // bash should be blocked by skill_tool_filters
        assert_eq!(result.text, "Blocked!");
    }

    #[tokio::test]
    async fn test_agent_skill_tool_filters_allows_authorized() {
        // When a skill allows a specific tool, it should pass through.
        use crate::tools::skill::Skill;

        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "tool-1",
                "bash",
                serde_json::json!({"command": "echo hello"}),
            ),
            MockLlmClient::text_response("Allowed!"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));

        // Create a skill that allows bash
        let skill = Skill {
            name: "bash-skill".to_string(),
            description: "Bash skill".to_string(),
            content: "Run bash".to_string(),
            allowed_tools: Some("bash(*)".to_string()),
            disable_model_invocation: false,
        };

        // Use Allow policy
        let policy = PermissionPolicy::new().allow("bash(*)");
        let policy_lock = Arc::new(RwLock::new(policy));

        // Use HITL disabled CM
        let (event_tx, _) = tokio::sync::broadcast::channel(10);
        let cm = Arc::new(crate::hitl::ConfirmationManager::new(
            crate::hitl::ConfirmationPolicy::default(), // disabled
            event_tx,
        ));

        let config = AgentConfig {
            permission_policy: Some(policy_lock),
            confirmation_manager: Some(cm),
            skill_tool_filters: vec![skill],
            ..Default::default()
        };

        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Echo", None).await.unwrap();

        // bash should be allowed by skill_tool_filters
        assert_eq!(result.text, "Allowed!");
    }

    // ========================================================================
    // AgentBuilder
    // ========================================================================

    #[test]
    fn test_agent_builder_default() {
        let builder = AgentBuilder::default();
        // Should fail to build without required fields
        let result = builder.build();
        assert!(result.is_err());
    }

    #[test]
    fn test_agent_builder_missing_tool_executor() {
        struct DummyClient;
        #[async_trait::async_trait]
        impl LlmClient for DummyClient {
            async fn complete(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<LlmResponse> {
                unimplemented!()
            }
            async fn complete_streaming(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<mpsc::Receiver<StreamEvent>> {
                unimplemented!()
            }
        }

        let result = AgentBuilder::new()
            .llm_client(Arc::new(DummyClient))
            .build();
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("Tool executor"), "Got: {}", err_msg);
    }

    #[test]
    fn test_agent_builder_complete() {
        struct DummyClient;
        #[async_trait::async_trait]
        impl LlmClient for DummyClient {
            async fn complete(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<LlmResponse> {
                unimplemented!()
            }
            async fn complete_streaming(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<mpsc::Receiver<StreamEvent>> {
                unimplemented!()
            }
        }

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let result = AgentBuilder::new()
            .llm_client(Arc::new(DummyClient))
            .tool_executor(tool_executor)
            .system_prompt("You are helpful")
            .max_tool_rounds(5)
            .tools(vec![])
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_agent_builder_with_tool_context() {
        struct DummyClient;
        #[async_trait::async_trait]
        impl LlmClient for DummyClient {
            async fn complete(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<LlmResponse> {
                unimplemented!()
            }
            async fn complete_streaming(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<mpsc::Receiver<StreamEvent>> {
                unimplemented!()
            }
        }

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let ctx = ToolContext::new(PathBuf::from("/workspace"));
        let result = AgentBuilder::new()
            .llm_client(Arc::new(DummyClient))
            .tool_executor(tool_executor)
            .tool_context(ctx)
            .build();
        assert!(result.is_ok());
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
    fn test_agent_event_serialize_todo_updated() {
        let event = AgentEvent::TodoUpdated {
            session_id: "sess-1".to_string(),
            todos: vec![],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("todo_updated"));
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
        use crate::planning::StepStatus;
        let event = AgentEvent::StepEnd {
            step_id: "step-1".to_string(),
            status: StepStatus::Completed,
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

    // ========================================================================
    // Planning and Goal Tracking Tests
    // ========================================================================

    #[tokio::test]
    async fn test_parse_plan_simple() {
        let mock_client = Arc::new(MockLlmClient::new(vec![]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let plan_text = "GOAL: Build a web app\nSTEPS:\n1. [tool: bash] Create project directory\n2. [tool: write] Write index.html";
        let plan = agent
            .parse_plan(plan_text, crate::planning::Complexity::Simple)
            .unwrap();

        assert_eq!(plan.goal, "Build a web app");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].description, "Create project directory");
        assert_eq!(plan.steps[0].tool, Some("bash".to_string()));
    }

    #[tokio::test]
    async fn test_parse_plan_with_dependencies() {
        let mock_client = Arc::new(MockLlmClient::new(vec![]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let plan_text = "GOAL: Setup project\nSTEPS:\n1. [tool: bash] Init repo\n2. [tool: write] Add config (depends on: 1)";
        let plan = agent
            .parse_plan(plan_text, crate::planning::Complexity::Medium)
            .unwrap();

        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[1].dependencies, vec!["step-1"]);
    }

    #[tokio::test]
    async fn test_parse_plan_no_goal() {
        let mock_client = Arc::new(MockLlmClient::new(vec![]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let plan_text = "STEPS:\n1. Do something";
        let plan = agent
            .parse_plan(plan_text, crate::planning::Complexity::Simple)
            .unwrap();

        assert_eq!(plan.goal, "Complete the task");
    }

    #[tokio::test]
    async fn test_parse_plan_no_steps() {
        let mock_client = Arc::new(MockLlmClient::new(vec![]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let plan_text = "GOAL: Do something";
        let plan = agent
            .parse_plan(plan_text, crate::planning::Complexity::Simple)
            .unwrap();

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].description, "Execute the task");
    }

    #[tokio::test]
    async fn test_parse_plan_no_tool() {
        let mock_client = Arc::new(MockLlmClient::new(vec![]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let plan_text = "GOAL: Test\nSTEPS:\n1. Do manual task";
        let plan = agent
            .parse_plan(plan_text, crate::planning::Complexity::Simple)
            .unwrap();

        assert_eq!(plan.steps[0].tool, None);
    }

    #[tokio::test]
    async fn test_analyze_complexity_simple() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Simple",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let complexity = agent.analyze_complexity("Print hello world").await.unwrap();
        assert!(matches!(complexity, crate::planning::Complexity::Simple));
    }

    #[tokio::test]
    async fn test_analyze_complexity_medium() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Medium",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let complexity = agent.analyze_complexity("Build a REST API").await.unwrap();
        assert!(matches!(complexity, crate::planning::Complexity::Medium));
    }

    #[tokio::test]
    async fn test_analyze_complexity_complex() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Complex",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let complexity = agent
            .analyze_complexity("Build distributed system")
            .await
            .unwrap();
        assert!(matches!(complexity, crate::planning::Complexity::Complex));
    }

    #[tokio::test]
    async fn test_analyze_complexity_very_complex() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "VeryComplex",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let complexity = agent
            .analyze_complexity("Build operating system")
            .await
            .unwrap();
        assert!(matches!(
            complexity,
            crate::planning::Complexity::VeryComplex
        ));
    }

    #[tokio::test]
    async fn test_analyze_complexity_default() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Unknown response",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let complexity = agent.analyze_complexity("Some task").await.unwrap();
        assert!(matches!(complexity, crate::planning::Complexity::Medium));
    }

    #[tokio::test]
    async fn test_extract_goal_with_criteria() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "GOAL: Build web app\nCRITERIA:\n- App runs on port 3000\n- Has login page",
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
    async fn test_extract_goal_no_criteria() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "GOAL: Simple task",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let goal = agent.extract_goal("Do something").await.unwrap();
        assert_eq!(goal.description, "Simple task");
        assert!(goal.success_criteria.is_empty());
    }

    #[tokio::test]
    async fn test_extract_goal_no_goal_line() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "Some response without goal",
        )]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let goal = agent.extract_goal("Original prompt").await.unwrap();
        assert_eq!(goal.description, "Original prompt");
    }

    #[tokio::test]
    async fn test_check_goal_achievement_yes() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "YES",
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
    async fn test_check_goal_achievement_no() {
        let mock_client = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response("NO")]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let goal = crate::planning::AgentGoal::new("Test goal".to_string());
        let achieved = agent
            .check_goal_achievement(&goal, "Not done")
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
    // AgentBuilder Additional Tests
    // ========================================================================

    #[test]
    fn test_agent_builder_with_permission_policy() {
        struct DummyClient;
        #[async_trait::async_trait]
        impl LlmClient for DummyClient {
            async fn complete(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<LlmResponse> {
                unimplemented!()
            }
            async fn complete_streaming(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<mpsc::Receiver<StreamEvent>> {
                unimplemented!()
            }
        }

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let mut builder = AgentBuilder::new()
            .llm_client(Arc::new(DummyClient))
            .tool_executor(tool_executor);

        builder.config.permission_policy = Some(Arc::new(RwLock::new(PermissionPolicy::default())));
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_agent_builder_with_context_providers() {
        struct DummyClient;
        #[async_trait::async_trait]
        impl LlmClient for DummyClient {
            async fn complete(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<LlmResponse> {
                unimplemented!()
            }
            async fn complete_streaming(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<mpsc::Receiver<StreamEvent>> {
                unimplemented!()
            }
        }

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let mut builder = AgentBuilder::new()
            .llm_client(Arc::new(DummyClient))
            .tool_executor(tool_executor);

        builder.config.context_providers = vec![];
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_agent_builder_with_planning_enabled() {
        struct DummyClient;
        #[async_trait::async_trait]
        impl LlmClient for DummyClient {
            async fn complete(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<LlmResponse> {
                unimplemented!()
            }
            async fn complete_streaming(
                &self,
                _: &[Message],
                _: Option<&str>,
                _: &[ToolDefinition],
            ) -> Result<mpsc::Receiver<StreamEvent>> {
                unimplemented!()
            }
        }

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let mut builder = AgentBuilder::new()
            .llm_client(Arc::new(DummyClient))
            .tool_executor(tool_executor);

        builder.config.planning_enabled = true;
        builder.config.goal_tracking = true;
        let result = builder.build();
        assert!(result.is_ok());
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
}
