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
use crate::hitl::ConfirmationProvider;
use crate::hooks::{
    ErrorType, GenerateEndEvent, GenerateStartEvent, HookEvent, HookExecutor, HookResult,
    OnErrorEvent, PostResponseEvent, PostToolUseEvent, PrePromptEvent, PreToolUseEvent,
    TokenUsageInfo, ToolCallInfo, ToolResultData,
};
use crate::llm::{LlmClient, LlmResponse, Message, TokenUsage, ToolDefinition};
use crate::permissions::{PermissionChecker, PermissionDecision};
use crate::planning::{AgentGoal, ExecutionPlan, TaskStatus};
use crate::prompts::SystemPromptSlots;
use crate::queue::SessionCommand;
use crate::session_lane_queue::SessionLaneQueue;
use crate::tool_search::ToolIndex;
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
    /// Slot-based system prompt customization.
    ///
    /// Users can customize specific parts (role, guidelines, response style, extra)
    /// without overriding the core agentic capabilities. The default agentic core
    /// (tool usage, autonomous behavior, completion criteria) is always preserved.
    pub prompt_slots: SystemPromptSlots,
    pub tools: Vec<ToolDefinition>,
    pub max_tool_rounds: usize,
    /// Optional security provider for input taint tracking and output sanitization
    pub security_provider: Option<Arc<dyn crate::security::SecurityProvider>>,
    /// Optional permission checker for tool execution control
    pub permission_checker: Option<Arc<dyn PermissionChecker>>,
    /// Optional confirmation manager for HITL (Human-in-the-Loop)
    pub confirmation_manager: Option<Arc<dyn ConfirmationProvider>>,
    /// Context providers for augmenting prompts with external context
    pub context_providers: Vec<Arc<dyn ContextProvider>>,
    /// Enable planning phase before execution
    pub planning_enabled: bool,
    /// Enable goal tracking
    pub goal_tracking: bool,
    /// Optional hook engine for firing lifecycle events (PreToolUse, PostToolUse, etc.)
    pub hook_engine: Option<Arc<dyn HookExecutor>>,
    /// Optional skill registry for tool permission enforcement
    pub skill_registry: Option<Arc<crate::skills::SkillRegistry>>,
    /// Max consecutive malformed-tool-args errors before aborting (default: 2).
    ///
    /// When the LLM returns tool arguments with `__parse_error`, the error is
    /// fed back as a tool result. After this many consecutive parse errors the
    /// loop bails instead of retrying indefinitely.
    pub max_parse_retries: u32,
    /// Per-tool execution timeout in milliseconds (`None` = no timeout).
    ///
    /// When set, each tool execution is wrapped in `tokio::time::timeout`.
    /// A timeout produces an error result sent back to the LLM rather than
    /// crashing the session.
    pub tool_timeout_ms: Option<u64>,
    /// Circuit-breaker threshold: max consecutive LLM API failures before
    /// aborting (default: 3).
    ///
    /// In non-streaming mode, transient LLM failures are retried up to this
    /// many times (with short exponential backoff) before the loop bails.
    /// In streaming mode, any failure is fatal (events cannot be replayed).
    pub circuit_breaker_threshold: u32,
    /// Enable auto-compaction when context usage exceeds threshold.
    pub auto_compact: bool,
    /// Context usage percentage threshold to trigger auto-compaction (0.0 - 1.0).
    /// Default: 0.80 (80%).
    pub auto_compact_threshold: f32,
    /// Maximum context window size in tokens (used for auto-compact calculation).
    /// Default: 200_000.
    pub max_context_tokens: usize,
    /// LLM client reference for auto-compaction (needs to call LLM for summarization).
    pub llm_client: Option<Arc<dyn LlmClient>>,
    /// Optional agent memory for auto-remember after tool execution and recall before prompts.
    pub memory: Option<Arc<crate::memory::AgentMemory>>,
    /// Inject a continuation message when the LLM stops calling tools before the
    /// task is complete. Enabled by default. Set to `false` to disable.
    ///
    /// When enabled, if the LLM produces a response with no tool calls but the
    /// response text looks like an intermediate step (not a final answer), the
    /// loop injects [`crate::prompts::CONTINUATION`] as a user message and
    /// continues for up to `max_continuation_turns` additional turns.
    pub continuation_enabled: bool,
    /// Maximum number of continuation injections per execution (default: 3).
    ///
    /// Prevents infinite loops when the LLM repeatedly stops without completing.
    pub max_continuation_turns: u32,
    /// Optional tool search index for filtering tools per-turn.
    ///
    /// When set, only tools matching the user prompt are sent to the LLM,
    /// reducing context usage when many MCP tools are registered.
    pub tool_index: Option<ToolIndex>,
}

impl std::fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConfig")
            .field("prompt_slots", &self.prompt_slots)
            .field("tools", &self.tools)
            .field("max_tool_rounds", &self.max_tool_rounds)
            .field("security_provider", &self.security_provider.is_some())
            .field("permission_checker", &self.permission_checker.is_some())
            .field("confirmation_manager", &self.confirmation_manager.is_some())
            .field("context_providers", &self.context_providers.len())
            .field("planning_enabled", &self.planning_enabled)
            .field("goal_tracking", &self.goal_tracking)
            .field("hook_engine", &self.hook_engine.is_some())
            .field(
                "skill_registry",
                &self.skill_registry.as_ref().map(|r| r.len()),
            )
            .field("max_parse_retries", &self.max_parse_retries)
            .field("tool_timeout_ms", &self.tool_timeout_ms)
            .field("circuit_breaker_threshold", &self.circuit_breaker_threshold)
            .field("auto_compact", &self.auto_compact)
            .field("auto_compact_threshold", &self.auto_compact_threshold)
            .field("max_context_tokens", &self.max_context_tokens)
            .field("continuation_enabled", &self.continuation_enabled)
            .field("max_continuation_turns", &self.max_continuation_turns)
            .field("memory", &self.memory.is_some())
            .field("tool_index", &self.tool_index.as_ref().map(|i| i.len()))
            .finish()
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            prompt_slots: SystemPromptSlots::default(),
            tools: Vec::new(), // Tools are provided by ToolExecutor
            max_tool_rounds: MAX_TOOL_ROUNDS,
            security_provider: None,
            permission_checker: None,
            confirmation_manager: None,
            context_providers: Vec::new(),
            planning_enabled: false,
            goal_tracking: false,
            hook_engine: None,
            skill_registry: Some(Arc::new(crate::skills::SkillRegistry::with_builtins())),
            max_parse_retries: 2,
            tool_timeout_ms: None,
            circuit_breaker_threshold: 3,
            auto_compact: false,
            auto_compact_threshold: 0.80,
            max_context_tokens: 200_000,
            llm_client: None,
            memory: None,
            continuation_enabled: true,
            max_continuation_turns: 3,
            tool_index: None,
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

    /// Tool input delta from streaming (partial JSON arguments)
    #[serde(rename = "tool_input_delta")]
    ToolInputDelta { delta: String },

    /// Tool execution completed
    #[serde(rename = "tool_end")]
    ToolEnd {
        id: String,
        name: String,
        output: String,
        exit_code: i32,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
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
    skill_registry: Option<Arc<crate::skills::SkillRegistry>>,
}

impl ToolCommand {
    /// Create a new ToolCommand
    pub fn new(
        tool_executor: Arc<ToolExecutor>,
        tool_name: String,
        tool_args: Value,
        tool_context: ToolContext,
        skill_registry: Option<Arc<crate::skills::SkillRegistry>>,
    ) -> Self {
        Self {
            tool_executor,
            tool_name,
            tool_args,
            tool_context,
            skill_registry,
        }
    }
}

#[async_trait]
impl SessionCommand for ToolCommand {
    async fn execute(&self) -> Result<Value> {
        // Check skill-based tool permissions
        if let Some(registry) = &self.skill_registry {
            let instruction_skills = registry.by_kind(crate::skills::SkillKind::Instruction);

            // If there are instruction skills with tool restrictions, check permissions
            let has_restrictions = instruction_skills.iter().any(|s| s.allowed_tools.is_some());

            if has_restrictions {
                let mut allowed = false;

                for skill in &instruction_skills {
                    if skill.is_tool_allowed(&self.tool_name) {
                        allowed = true;
                        break;
                    }
                }

                if !allowed {
                    return Err(anyhow::anyhow!(
                        "Tool '{}' is not allowed by any active skill. Active skills restrict tools to their allowed-tools lists.",
                        self.tool_name
                    ));
                }
            }
        }

        // Execute the tool
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
// AgentLoop
// ============================================================================

/// Agent loop executor
#[derive(Clone)]
pub struct AgentLoop {
    llm_client: Arc<dyn LlmClient>,
    tool_executor: Arc<ToolExecutor>,
    tool_context: ToolContext,
    config: AgentConfig,
    /// Optional per-session tool metrics collector
    tool_metrics: Option<Arc<RwLock<crate::telemetry::ToolMetrics>>>,
    /// Optional lane queue for priority-based tool execution
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

    /// Set the lane queue for priority-based tool execution.
    ///
    /// When set, tools are routed through the lane queue which supports
    /// External task handling for multi-machine parallel processing.
    pub fn with_queue(mut self, queue: Arc<SessionLaneQueue>) -> Self {
        self.command_queue = Some(queue);
        self
    }

    /// Execute a tool, applying the configured timeout if set.
    ///
    /// On timeout, returns an error describing which tool timed out and after
    /// how many milliseconds. The caller converts this to a tool-result error
    /// message that is fed back to the LLM.
    async fn execute_tool_timed(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<crate::tools::ToolResult> {
        let fut = self.tool_executor.execute_with_context(name, args, ctx);
        if let Some(timeout_ms) = self.config.tool_timeout_ms {
            match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!(
                    "Tool '{}' timed out after {}ms",
                    name,
                    timeout_ms
                )),
            }
        } else {
            fut.await
        }
    }

    /// Convert a tool execution result into the (output, exit_code, is_error, metadata, images) tuple.
    fn tool_result_to_tuple(
        result: anyhow::Result<crate::tools::ToolResult>,
    ) -> (
        String,
        i32,
        bool,
        Option<serde_json::Value>,
        Vec<crate::llm::Attachment>,
    ) {
        match result {
            Ok(r) => (
                r.output,
                r.exit_code,
                r.exit_code != 0,
                r.metadata,
                r.images,
            ),
            Err(e) => {
                let msg = e.to_string();
                // Classify the error so the LLM knows whether retrying makes sense.
                let hint = if Self::is_transient_error(&msg) {
                    " [transient — you may retry this tool call]"
                } else {
                    " [permanent — do not retry without changing the arguments]"
                };
                (
                    format!("Tool execution error: {}{}", msg, hint),
                    1,
                    true,
                    None,
                    Vec::new(),
                )
            }
        }
    }

    /// Inspect the workspace for well-known project marker files and return a short
    /// `## Project Context` section that the agent can use without any manual configuration.
    /// Returns an empty string when the workspace type cannot be determined.
    fn detect_project_hint(workspace: &std::path::Path) -> String {
        struct Marker {
            file: &'static str,
            lang: &'static str,
            tip: &'static str,
        }

        let markers = [
            Marker {
                file: "Cargo.toml",
                lang: "Rust",
                tip: "Use `cargo build`, `cargo test`, `cargo clippy`, and `cargo fmt`. \
                  Prefer `anyhow` / `thiserror` for error handling. \
                  Follow the Microsoft Rust Guidelines (no panics in library code, \
                  async-first with Tokio).",
            },
            Marker {
                file: "package.json",
                lang: "Node.js / TypeScript",
                tip: "Check `package.json` for the package manager (npm/yarn/pnpm/bun) \
                  and available scripts. Prefer TypeScript with strict mode. \
                  Use ESM imports unless the project is CommonJS.",
            },
            Marker {
                file: "pyproject.toml",
                lang: "Python",
                tip: "Use the package manager declared in `pyproject.toml` \
                  (uv, poetry, hatch, etc.). Prefer type hints and async/await for I/O.",
            },
            Marker {
                file: "setup.py",
                lang: "Python",
                tip: "Legacy Python project. Prefer type hints and async/await for I/O.",
            },
            Marker {
                file: "requirements.txt",
                lang: "Python",
                tip: "Python project with pip-style dependencies. \
                  Prefer type hints and async/await for I/O.",
            },
            Marker {
                file: "go.mod",
                lang: "Go",
                tip: "Use `go build ./...` and `go test ./...`. \
                  Follow standard Go project layout. Use `gofmt` for formatting.",
            },
            Marker {
                file: "pom.xml",
                lang: "Java / Maven",
                tip: "Use `mvn compile`, `mvn test`, `mvn package`. \
                  Follow standard Maven project structure.",
            },
            Marker {
                file: "build.gradle",
                lang: "Java / Gradle",
                tip: "Use `./gradlew build` and `./gradlew test`. \
                  Follow standard Gradle project structure.",
            },
            Marker {
                file: "build.gradle.kts",
                lang: "Kotlin / Gradle",
                tip: "Use `./gradlew build` and `./gradlew test`. \
                  Prefer Kotlin coroutines for async work.",
            },
            Marker {
                file: "CMakeLists.txt",
                lang: "C / C++",
                tip: "Use `cmake -B build && cmake --build build`. \
                  Check for `compile_commands.json` for IDE tooling.",
            },
            Marker {
                file: "Makefile",
                lang: "C / C++ (or generic)",
                tip: "Use `make` or `make <target>`. \
                  Check available targets with `make help` or by reading the Makefile.",
            },
        ];

        // Check for C# / .NET — no single fixed filename, so glob for *.csproj / *.sln
        let is_dotnet = workspace.join("*.csproj").exists() || {
            // Fast check: look for any .csproj or .sln in the workspace root
            std::fs::read_dir(workspace)
                .map(|entries| {
                    entries.flatten().any(|e| {
                        let name = e.file_name();
                        let s = name.to_string_lossy();
                        s.ends_with(".csproj") || s.ends_with(".sln")
                    })
                })
                .unwrap_or(false)
        };

        if is_dotnet {
            return "## Project Context\n\nThis is a **C# / .NET** project. \
             Use `dotnet build`, `dotnet test`, and `dotnet run`. \
             Follow C# coding conventions and async/await patterns."
                .to_string();
        }

        for marker in &markers {
            if workspace.join(marker.file).exists() {
                return format!(
                    "## Project Context\n\nThis is a **{}** project. {}",
                    marker.lang, marker.tip
                );
            }
        }

        String::new()
    }

    /// Returns `true` for errors that are likely transient (network, timeout, I/O contention).
    /// Used to annotate tool error messages so the LLM knows whether retrying is safe.
    fn is_transient_error(msg: &str) -> bool {
        let lower = msg.to_lowercase();
        lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("temporarily unavailable")
        || lower.contains("resource temporarily unavailable")
        || lower.contains("os error 11")  // EAGAIN
        || lower.contains("os error 35")  // EAGAIN on macOS
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("service unavailable")
        || lower.contains("network unreachable")
    }

    /// Returns `true` when a tool writes a file and is safe to run concurrently with other
    /// independent writes (no ordering dependencies, no side-channel output).
    fn is_parallel_safe_write(name: &str, _args: &serde_json::Value) -> bool {
        matches!(
            name,
            "write_file" | "edit_file" | "create_file" | "append_to_file" | "replace_in_file"
        )
    }

    /// Extract the target file path from write-tool arguments so we can check for conflicts.
    fn extract_write_path(args: &serde_json::Value) -> Option<String> {
        // write_file / create_file / append_to_file / replace_in_file use "path"
        // edit_file uses "path" as well
        args.get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Execute a tool through the lane queue (if configured) or directly.
    async fn execute_tool_queued_or_direct(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<crate::tools::ToolResult> {
        if let Some(ref queue) = self.command_queue {
            let command = ToolCommand::new(
                Arc::clone(&self.tool_executor),
                name.to_string(),
                args.clone(),
                ctx.clone(),
                self.config.skill_registry.clone(),
            );
            let rx = queue.submit_by_tool(name, Box::new(command)).await;
            match rx.await {
                Ok(Ok(value)) => {
                    let output = value["output"]
                        .as_str()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Queue result missing 'output' field for tool '{}'",
                                name
                            )
                        })?
                        .to_string();
                    let exit_code = value["exit_code"].as_i64().unwrap_or(0) as i32;
                    return Ok(crate::tools::ToolResult {
                        name: name.to_string(),
                        output,
                        exit_code,
                        metadata: None,
                        images: Vec::new(),
                    });
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        "Queue execution failed for tool '{}', falling back to direct: {}",
                        name,
                        e
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        "Queue channel closed for tool '{}', falling back to direct",
                        name
                    );
                }
            }
        }
        self.execute_tool_timed(name, args, ctx).await
    }

    /// Call the LLM, handling streaming vs non-streaming internally.
    ///
    /// Streaming events (`TextDelta`, `ToolStart`) are forwarded to `event_tx`
    /// as they arrive. Non-streaming mode simply awaits the complete response.
    ///
    /// When a `ToolIndex` is configured, tools are filtered per-turn based on
    /// the last user message, reducing context usage with large tool sets.
    ///
    /// Returns `Err` on any LLM API failure. The circuit breaker in
    /// `execute_loop` wraps this call with retry logic for non-streaming mode.
    async fn call_llm(
        &self,
        messages: &[Message],
        system: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<LlmResponse> {
        // Filter tools through ToolIndex if configured
        let tools = if let Some(ref index) = self.config.tool_index {
            let query = messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .and_then(|m| {
                    m.content.iter().find_map(|b| match b {
                        crate::llm::ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                })
                .unwrap_or("");
            let matches = index.search(query, index.len());
            let matched_names: std::collections::HashSet<&str> =
                matches.iter().map(|m| m.name.as_str()).collect();
            self.config
                .tools
                .iter()
                .filter(|t| matched_names.contains(t.name.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            self.config.tools.clone()
        };

        if event_tx.is_some() {
            let mut stream_rx = self
                .llm_client
                .complete_streaming(messages, system, &tools)
                .await
                .context("LLM streaming call failed")?;

            let mut final_response: Option<LlmResponse> = None;
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        tracing::info!("LLM streaming cancelled by user");
                        anyhow::bail!("Operation cancelled by user");
                    }
                    event = stream_rx.recv() => {
                        match event {
                            Some(crate::llm::StreamEvent::TextDelta(text)) => {
                                if let Some(tx) = event_tx {
                                    tx.send(AgentEvent::TextDelta { text }).await.ok();
                                }
                            }
                            Some(crate::llm::StreamEvent::ToolUseStart { id, name }) => {
                                if let Some(tx) = event_tx {
                                    tx.send(AgentEvent::ToolStart { id, name }).await.ok();
                                }
                            }
                            Some(crate::llm::StreamEvent::ToolUseInputDelta(delta)) => {
                                if let Some(tx) = event_tx {
                                    tx.send(AgentEvent::ToolInputDelta { delta }).await.ok();
                                }
                            }
                            Some(crate::llm::StreamEvent::Done(resp)) => {
                                final_response = Some(resp);
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
            final_response.context("Stream ended without final response")
        } else {
            self.llm_client
                .complete(messages, system, &tools)
                .await
                .context("LLM call failed")
        }
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

    /// Detect whether the LLM's no-tool-call response looks like an intermediate
    /// step rather than a genuine final answer.
    ///
    /// Returns `true` when continuation should be injected. Heuristics:
    /// - Response ends with a colon or ellipsis (mid-thought)
    /// - Response contains phrases that signal incomplete work
    /// - Response is very short (< 80 chars) and doesn't look like a summary
    fn looks_incomplete(text: &str) -> bool {
        let t = text.trim();
        if t.is_empty() {
            return true;
        }
        // Very short responses that aren't clearly a final answer
        if t.len() < 80 && !t.contains('\n') {
            // Short single-line — could be a genuine one-liner answer, keep going
            // only if it ends with punctuation that signals continuation
            let ends_continuation =
                t.ends_with(':') || t.ends_with("...") || t.ends_with('…') || t.ends_with(',');
            if ends_continuation {
                return true;
            }
        }
        // Phrases that signal the LLM is describing what it will do rather than doing it
        let incomplete_phrases = [
            "i'll ",
            "i will ",
            "let me ",
            "i need to ",
            "i should ",
            "next, i",
            "first, i",
            "now i",
            "i'll start",
            "i'll begin",
            "i'll now",
            "let's start",
            "let's begin",
            "to do this",
            "i'm going to",
        ];
        let lower = t.to_lowercase();
        for phrase in &incomplete_phrases {
            if lower.contains(phrase) {
                return true;
            }
        }
        false
    }

    /// Get the assembled system prompt from slots.
    fn system_prompt(&self) -> String {
        self.config.prompt_slots.build()
    }

    /// Build augmented system prompt with context
    fn build_augmented_system_prompt(&self, context_results: &[ContextResult]) -> Option<String> {
        let base = self.system_prompt();

        // Use live tool executor definitions so tools added via add_mcp_server() are included
        let live_tools = self.tool_executor.definitions();
        let mcp_tools: Vec<&ToolDefinition> = live_tools
            .iter()
            .filter(|t| t.name.starts_with("mcp__"))
            .collect();

        let mcp_section = if mcp_tools.is_empty() {
            String::new()
        } else {
            let mut lines = vec![
                "## MCP Tools".to_string(),
                String::new(),
                "The following MCP (Model Context Protocol) tools are available. Use them when the task requires external capabilities beyond the built-in tools:".to_string(),
                String::new(),
            ];
            for tool in &mcp_tools {
                let display = format!("- `{}` — {}", tool.name, tool.description);
                lines.push(display);
            }
            lines.join("\n")
        };

        let parts: Vec<&str> = [base.as_str(), mcp_section.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect();

        // Auto-detect project type from workspace and inject language-specific guidelines,
        // but only when the user hasn't already set a custom `guidelines` slot.
        let project_hint = if self.config.prompt_slots.guidelines.is_none() {
            Self::detect_project_hint(&self.tool_context.workspace)
        } else {
            String::new()
        };

        if context_results.is_empty() {
            if project_hint.is_empty() {
                return Some(parts.join("\n\n"));
            }
            return Some(format!("{}\n\n{}", parts.join("\n\n"), project_hint));
        }

        // Build context XML block
        let context_xml: String = context_results
            .iter()
            .map(|r| r.to_xml())
            .collect::<Vec<_>>()
            .join("\n\n");

        if project_hint.is_empty() {
            Some(format!("{}\n\n{}", parts.join("\n\n"), context_xml))
        } else {
            Some(format!(
                "{}\n\n{}\n\n{}",
                parts.join("\n\n"),
                project_hint,
                context_xml
            ))
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
            let he = Arc::clone(he);
            tokio::spawn(async move {
                let _ = he.fire(&event).await;
            });
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

    /// Fire PrePrompt hook event before prompt augmentation.
    /// Returns optional modified prompt text from the hook.
    async fn fire_pre_prompt(
        &self,
        session_id: &str,
        prompt: &str,
        system_prompt: &Option<String>,
        message_count: usize,
    ) -> Option<String> {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::PrePrompt(PrePromptEvent {
                session_id: session_id.to_string(),
                prompt: prompt.to_string(),
                system_prompt: system_prompt.clone(),
                message_count,
            });
            let result = he.fire(&event).await;
            if let HookResult::Continue(Some(modified)) = result {
                // Extract modified prompt from hook response
                if let Some(new_prompt) = modified.get("prompt").and_then(|v| v.as_str()) {
                    return Some(new_prompt.to_string());
                }
            }
        }
        None
    }

    /// Fire PostResponse hook event after the agent loop completes.
    async fn fire_post_response(
        &self,
        session_id: &str,
        response_text: &str,
        tool_calls_count: usize,
        usage: &TokenUsage,
        duration_ms: u64,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::PostResponse(PostResponseEvent {
                session_id: session_id.to_string(),
                response_text: response_text.to_string(),
                tool_calls_count,
                usage: TokenUsageInfo {
                    prompt_tokens: usage.prompt_tokens as i32,
                    completion_tokens: usage.completion_tokens as i32,
                    total_tokens: usage.total_tokens as i32,
                },
                duration_ms,
            });
            let he = Arc::clone(he);
            tokio::spawn(async move {
                let _ = he.fire(&event).await;
            });
        }
    }

    /// Fire OnError hook event when an error occurs.
    async fn fire_on_error(
        &self,
        session_id: &str,
        error_type: ErrorType,
        error_message: &str,
        context: serde_json::Value,
    ) {
        if let Some(he) = &self.config.hook_engine {
            let event = HookEvent::OnError(OnErrorEvent {
                session_id: session_id.to_string(),
                error_type,
                error_message: error_message.to_string(),
                context,
            });
            let he = Arc::clone(he);
            tokio::spawn(async move {
                let _ = he.fire(&event).await;
            });
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
        self.execute_with_session(history, prompt, None, event_tx, None)
            .await
    }

    /// Execute the agent loop with pre-built messages (user message already included).
    ///
    /// Used by `send_with_attachments` / `stream_with_attachments` where the
    /// user message contains multi-modal content and is already appended to
    /// the messages vec.
    pub async fn execute_from_messages(
        &self,
        messages: Vec<Message>,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<AgentResult> {
        let default_token = tokio_util::sync::CancellationToken::new();
        let token = cancel_token.unwrap_or(&default_token);
        tracing::info!(
            a3s.session.id = session_id.unwrap_or("none"),
            a3s.agent.max_turns = self.config.max_tool_rounds,
            "a3s.agent.execute_from_messages started"
        );

        // Extract the last user message text for hooks, memory recall, and events.
        // Pass empty prompt so execute_loop skips adding a duplicate user message,
        // but provide effective_prompt for hook/memory/event purposes.
        let effective_prompt = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.text())
            .unwrap_or_default();

        let result = self
            .execute_loop_inner(&messages, "", &effective_prompt, session_id, event_tx, token)
            .await;

        match &result {
            Ok(r) => tracing::info!(
                a3s.agent.tool_calls_count = r.tool_calls_count,
                a3s.llm.total_tokens = r.usage.total_tokens,
                "a3s.agent.execute_from_messages completed"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                "a3s.agent.execute_from_messages failed"
            ),
        }

        result
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
        cancel_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<AgentResult> {
        let default_token = tokio_util::sync::CancellationToken::new();
        let token = cancel_token.unwrap_or(&default_token);
        tracing::info!(
            a3s.session.id = session_id.unwrap_or("none"),
            a3s.agent.max_turns = self.config.max_tool_rounds,
            "a3s.agent.execute started"
        );

        // Route to planning-based execution if enabled
        let result = if self.config.planning_enabled {
            self.execute_with_planning(history, prompt, event_tx).await
        } else {
            self.execute_loop(history, prompt, session_id, event_tx, token)
                .await
        };

        match &result {
            Ok(r) => {
                tracing::info!(
                    a3s.agent.tool_calls_count = r.tool_calls_count,
                    a3s.llm.total_tokens = r.usage.total_tokens,
                    "a3s.agent.execute completed"
                );
                // Fire PostResponse hook
                self.fire_post_response(
                    session_id.unwrap_or(""),
                    &r.text,
                    r.tool_calls_count,
                    &r.usage,
                    0, // duration tracked externally
                )
                .await;
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "a3s.agent.execute failed"
                );
                // Fire OnError hook
                self.fire_on_error(
                    session_id.unwrap_or(""),
                    ErrorType::Other,
                    &e.to_string(),
                    serde_json::json!({"phase": "execute"}),
                )
                .await;
            }
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
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> Result<AgentResult> {
        // When called via execute_loop, the prompt is used for both
        // message-adding and hook/memory/event purposes.
        self.execute_loop_inner(history, prompt, prompt, session_id, event_tx, cancel_token)
            .await
    }

    /// Inner execution loop.
    ///
    /// `msg_prompt` controls whether a user message is appended (empty = skip).
    /// `effective_prompt` is used for hooks, memory recall, taint tracking, and events.
    async fn execute_loop_inner(
        &self,
        history: &[Message],
        msg_prompt: &str,
        effective_prompt: &str,
        session_id: Option<&str>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> Result<AgentResult> {
        let mut messages = history.to_vec();
        let mut total_usage = TokenUsage::default();
        let mut tool_calls_count = 0;
        let mut turn = 0;
        // Consecutive malformed-tool-args errors (4.1 parse error recovery)
        let mut parse_error_count: u32 = 0;
        // Continuation injection counter
        let mut continuation_count: u32 = 0;

        // Send start event
        if let Some(tx) = &event_tx {
            tx.send(AgentEvent::Start {
                prompt: effective_prompt.to_string(),
            })
            .await
            .ok();
        }

        // Forward queue events (CommandDeadLettered, CommandRetry, QueueAlert) to event stream
        let _queue_forward_handle =
            if let (Some(ref queue), Some(ref tx)) = (&self.command_queue, &event_tx) {
                let mut rx = queue.subscribe();
                let tx = tx.clone();
                Some(tokio::spawn(async move {
                    while let Ok(event) = rx.recv().await {
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                }))
            } else {
                None
            };

        // Fire PrePrompt hook (may modify the prompt)
        let built_system_prompt = Some(self.system_prompt());
        let hooked_prompt = if let Some(modified) = self
            .fire_pre_prompt(
                session_id.unwrap_or(""),
                effective_prompt,
                &built_system_prompt,
                messages.len(),
            )
            .await
        {
            modified
        } else {
            effective_prompt.to_string()
        };
        let effective_prompt = hooked_prompt.as_str();

        // Taint-track the incoming prompt for sensitive data detection
        if let Some(ref sp) = self.config.security_provider {
            sp.taint_input(effective_prompt);
        }

        // Recall relevant memories and inject into system prompt
        let system_with_memory = if let Some(ref memory) = self.config.memory {
            match memory.recall_similar(effective_prompt, 5).await {
                Ok(items) if !items.is_empty() => {
                    if let Some(tx) = &event_tx {
                        for item in &items {
                            tx.send(AgentEvent::MemoryRecalled {
                                memory_id: item.id.clone(),
                                content: item.content.clone(),
                                relevance: item.relevance_score(),
                            })
                            .await
                            .ok();
                        }
                        tx.send(AgentEvent::MemoriesSearched {
                            query: Some(effective_prompt.to_string()),
                            tags: Vec::new(),
                            result_count: items.len(),
                        })
                        .await
                        .ok();
                    }
                    let memory_context = items
                        .iter()
                        .map(|i| format!("- {}", i.content))
                        .collect::<Vec<_>>()
                        .join(
                            "
",
                        );
                    let base = self.system_prompt();
                    Some(format!(
                        "{}

## Relevant past experience
{}",
                        base, memory_context
                    ))
                }
                _ => Some(self.system_prompt()),
            }
        } else {
            Some(self.system_prompt())
        };

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

            tracing::info!(
                a3s.context.providers = self.config.context_providers.len() as i64,
                "Context resolution started"
            );
            let context_results = self.resolve_context(effective_prompt, session_id).await;

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
            Some(self.system_prompt())
        };

        // Merge memory context into system prompt
        let base_prompt = self.system_prompt();
        let augmented_system = match (augmented_system, system_with_memory) {
            (Some(ctx), Some(mem)) if ctx != mem => Some(ctx.replacen(&base_prompt, &mem, 1)),
            (Some(ctx), _) => Some(ctx),
            (None, mem) => mem,
        };

        // Add user message
        if !msg_prompt.is_empty() {
            messages.push(Message::user(msg_prompt));
        }

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
            self.fire_generate_start(
                session_id.unwrap_or(""),
                effective_prompt,
                &augmented_system,
            )
            .await;

            let llm_start = std::time::Instant::now();
            // Circuit breaker (4.3): retry non-streaming LLM calls on transient failures.
            // Each failure increments `consecutive_llm_errors`; on success it resets to 0.
            // Streaming mode bails immediately on failure (events can't be replayed).
            let response = {
                let threshold = self.config.circuit_breaker_threshold.max(1);
                let mut attempt = 0u32;
                loop {
                    attempt += 1;
                    let result = self
                        .call_llm(&messages, augmented_system.as_deref(), &event_tx, cancel_token)
                        .await;
                    match result {
                        Ok(r) => {
                            break r;
                        }
                        // Retry when: non-streaming under threshold, OR first streaming attempt
                        Err(e) if attempt < threshold && (event_tx.is_none() || attempt == 1) => {
                            tracing::warn!(
                                turn = turn,
                                attempt = attempt,
                                threshold = threshold,
                                error = %e,
                                "LLM call failed, will retry"
                            );
                            tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                        }
                        // Threshold exceeded or streaming mid-stream: bail
                        Err(e) => {
                            let msg = if attempt > 1 {
                                format!(
                                    "LLM circuit breaker triggered: failed after {} attempt(s): {}",
                                    attempt, e
                                )
                            } else {
                                format!("LLM call failed: {}", e)
                            };
                            tracing::error!(turn = turn, attempt = attempt, "{}", msg);
                            // Fire OnError hook for LLM failure
                            self.fire_on_error(
                                session_id.unwrap_or(""),
                                ErrorType::LlmFailure,
                                &msg,
                                serde_json::json!({"turn": turn, "attempt": attempt}),
                            )
                            .await;
                            if let Some(tx) = &event_tx {
                                tx.send(AgentEvent::Error {
                                    message: msg.clone(),
                                })
                                .await
                                .ok();
                            }
                            anyhow::bail!(msg);
                        }
                    }
                }
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
                effective_prompt,
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

            // Auto-compact: check if context usage exceeds threshold
            if self.config.auto_compact {
                let used = response.usage.prompt_tokens;
                let max = self.config.max_context_tokens;
                let threshold = self.config.auto_compact_threshold;

                if crate::session::compaction::should_auto_compact(used, max, threshold) {
                    let before_len = messages.len();
                    let percent_before = used as f32 / max as f32;

                    tracing::info!(
                        used_tokens = used,
                        max_tokens = max,
                        percent = percent_before,
                        threshold = threshold,
                        "Auto-compact triggered"
                    );

                    // Step 1: Prune large tool outputs first (cheap, no LLM call)
                    if let Some(pruned) = crate::session::compaction::prune_tool_outputs(&messages)
                    {
                        messages = pruned;
                        tracing::info!("Tool output pruning applied");
                    }

                    // Step 2: Full summarization using the agent's LLM client
                    if let Ok(Some(compacted)) = crate::session::compaction::compact_messages(
                        session_id.unwrap_or(""),
                        &messages,
                        &self.llm_client,
                    )
                    .await
                    {
                        messages = compacted;
                    }

                    // Emit compaction event
                    if let Some(tx) = &event_tx {
                        tx.send(AgentEvent::ContextCompacted {
                            session_id: session_id.unwrap_or("").to_string(),
                            before_messages: before_len,
                            after_messages: messages.len(),
                            percent_before,
                        })
                        .await
                        .ok();
                    }
                }
            }

            if tool_calls.is_empty() {
                // No tool calls — check if we should inject a continuation message
                // before treating this as a final answer.
                let final_text = response.text();

                if self.config.continuation_enabled
                    && continuation_count < self.config.max_continuation_turns
                    && turn < self.config.max_tool_rounds  // never inject past the turn limit
                    && Self::looks_incomplete(&final_text)
                {
                    continuation_count += 1;
                    tracing::info!(
                        turn = turn,
                        continuation = continuation_count,
                        max_continuation = self.config.max_continuation_turns,
                        "Injecting continuation message — response looks incomplete"
                    );
                    // Inject continuation as a user message and keep looping
                    messages.push(Message::user(crate::prompts::CONTINUATION));
                    continue;
                }

                // Sanitize output to redact any sensitive data before returning
                let final_text = if let Some(ref sp) = self.config.security_provider {
                    sp.sanitize_output(&final_text)
                } else {
                    final_text
                };

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
                    self.notify_turn_complete(sid, effective_prompt, &final_text)
                        .await;
                }

                return Ok(AgentResult {
                    text: final_text,
                    messages,
                    usage: total_usage,
                    tool_calls_count,
                });
            }

            // Execute tools sequentially
            // Fast path: when all tool calls are independent file writes and no hooks/HITL
            // are configured, execute them concurrently to avoid serial I/O bottleneck.
            let tool_calls = if self.config.hook_engine.is_none()
                && self.config.confirmation_manager.is_none()
                && tool_calls.len() > 1
                && tool_calls
                    .iter()
                    .all(|tc| Self::is_parallel_safe_write(&tc.name, &tc.args))
                && {
                    // All target paths must be distinct (no write-write conflicts)
                    let paths: Vec<_> = tool_calls
                        .iter()
                        .filter_map(|tc| Self::extract_write_path(&tc.args))
                        .collect();
                    paths.len() == tool_calls.len()
                        && paths.iter().collect::<std::collections::HashSet<_>>().len()
                            == paths.len()
                } {
                tracing::info!(
                    count = tool_calls.len(),
                    "Parallel write batch: executing {} independent file writes concurrently",
                    tool_calls.len()
                );

                let futures: Vec<_> = tool_calls
                    .iter()
                    .map(|tc| {
                        let ctx = self.tool_context.clone();
                        let executor = Arc::clone(&self.tool_executor);
                        let name = tc.name.clone();
                        let args = tc.args.clone();
                        async move { executor.execute_with_context(&name, &args, &ctx).await }
                    })
                    .collect();

                let results = join_all(futures).await;

                // Post-process results in original order (sequential, preserves message ordering)
                for (tc, result) in tool_calls.iter().zip(results) {
                    tool_calls_count += 1;
                    let (output, exit_code, is_error, metadata, images) =
                        Self::tool_result_to_tuple(result);

                    let output = if let Some(ref sp) = self.config.security_provider {
                        sp.sanitize_output(&output)
                    } else {
                        output
                    };

                    if let Some(tx) = &event_tx {
                        tx.send(AgentEvent::ToolEnd {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            output: output.clone(),
                            exit_code,
                            metadata,
                        })
                        .await
                        .ok();
                    }

                    if images.is_empty() {
                        messages.push(Message::tool_result(&tc.id, &output, is_error));
                    } else {
                        messages.push(Message::tool_result_with_images(
                            &tc.id, &output, &images, is_error,
                        ));
                    }
                }

                // Skip the sequential loop below
                continue;
            } else {
                tool_calls
            };

            for tool_call in tool_calls {
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

                // Check for malformed tool arguments from LLM (4.1 parse error recovery)
                if let Some(parse_error) =
                    tool_call.args.get("__parse_error").and_then(|v| v.as_str())
                {
                    parse_error_count += 1;
                    let error_msg = format!("Error: {}", parse_error);
                    tracing::warn!(
                        tool = tool_call.name.as_str(),
                        parse_error_count = parse_error_count,
                        max_parse_retries = self.config.max_parse_retries,
                        "Malformed tool arguments from LLM"
                    );

                    if let Some(tx) = &event_tx {
                        tx.send(AgentEvent::ToolEnd {
                            id: tool_call.id.clone(),
                            name: tool_call.name.clone(),
                            output: error_msg.clone(),
                            exit_code: 1,
                            metadata: None,
                        })
                        .await
                        .ok();
                    }

                    messages.push(Message::tool_result(&tool_call.id, &error_msg, true));

                    if parse_error_count > self.config.max_parse_retries {
                        let msg = format!(
                            "LLM produced malformed tool arguments {} time(s) in a row \
                             (max_parse_retries={}); giving up",
                            parse_error_count, self.config.max_parse_retries
                        );
                        tracing::error!("{}", msg);
                        if let Some(tx) = &event_tx {
                            tx.send(AgentEvent::Error {
                                message: msg.clone(),
                            })
                            .await
                            .ok();
                        }
                        anyhow::bail!(msg);
                    }
                    continue;
                }

                // Tool args are valid — reset parse error counter
                parse_error_count = 0;

                // Check skill-based tool permissions
                if let Some(ref registry) = self.config.skill_registry {
                    let instruction_skills =
                        registry.by_kind(crate::skills::SkillKind::Instruction);
                    let has_restrictions =
                        instruction_skills.iter().any(|s| s.allowed_tools.is_some());
                    if has_restrictions {
                        let allowed = instruction_skills
                            .iter()
                            .any(|s| s.is_tool_allowed(&tool_call.name));
                        if !allowed {
                            let msg = format!(
                                "Tool '{}' is not allowed by any active skill.",
                                tool_call.name
                            );
                            tracing::info!(
                                tool_name = tool_call.name.as_str(),
                                "Tool blocked by skill registry"
                            );
                            if let Some(tx) = &event_tx {
                                tx.send(AgentEvent::PermissionDenied {
                                    tool_id: tool_call.id.clone(),
                                    tool_name: tool_call.name.clone(),
                                    args: tool_call.args.clone(),
                                    reason: msg.clone(),
                                })
                                .await
                                .ok();
                            }
                            messages.push(Message::tool_result(&tool_call.id, &msg, true));
                            continue;
                        }
                    }
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
                let permission_decision = if let Some(checker) = &self.config.permission_checker {
                    checker.check(&tool_call.name, &tool_call.args)
                } else {
                    // No policy configured — default to Ask so HITL can still intervene
                    PermissionDecision::Ask
                };

                let (output, exit_code, is_error, metadata, images) = match permission_decision {
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

                        (denial_msg, 1, true, None, Vec::new())
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
                            .execute_tool_queued_or_direct(
                                &tool_call.name,
                                &tool_call.args,
                                &stream_ctx,
                            )
                            .await;

                        Self::tool_result_to_tuple(result)
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
                                    .execute_tool_queued_or_direct(
                                        &tool_call.name,
                                        &tool_call.args,
                                        &stream_ctx,
                                    )
                                    .await;

                                let (output, exit_code, is_error, metadata, images) =
                                    Self::tool_result_to_tuple(result);

                                // Add tool result to messages
                                if images.is_empty() {
                                    messages.push(Message::tool_result(
                                        &tool_call.id,
                                        &output,
                                        is_error,
                                    ));
                                } else {
                                    messages.push(Message::tool_result_with_images(
                                        &tool_call.id,
                                        &output,
                                        &images,
                                        is_error,
                                    ));
                                }

                                // Record tool result on the tool span for early exit
                                let tool_duration = tool_start.elapsed();
                                crate::telemetry::record_tool_result(exit_code, tool_duration);

                                // Send ToolEnd event
                                if let Some(tx) = &event_tx {
                                    tx.send(AgentEvent::ToolEnd {
                                        id: tool_call.id.clone(),
                                        name: tool_call.name.clone(),
                                        output: output.clone(),
                                        exit_code,
                                        metadata,
                                    })
                                    .await
                                    .ok();
                                }

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

                            // Forward ConfirmationRequired to the streaming event channel
                            // so external consumers (e.g. SafeClaw engine) can relay it
                            // to the browser UI.
                            if let Some(tx) = &event_tx {
                                tx.send(AgentEvent::ConfirmationRequired {
                                    tool_id: tool_call.id.clone(),
                                    tool_name: tool_call.name.clone(),
                                    args: tool_call.args.clone(),
                                    timeout_ms,
                                })
                                .await
                                .ok();
                            }

                            // Wait for confirmation with timeout
                            let confirmation_result =
                                tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await;

                            match confirmation_result {
                                Ok(Ok(response)) => {
                                    // Forward ConfirmationReceived
                                    if let Some(tx) = &event_tx {
                                        tx.send(AgentEvent::ConfirmationReceived {
                                            tool_id: tool_call.id.clone(),
                                            approved: response.approved,
                                            reason: response.reason.clone(),
                                        })
                                        .await
                                        .ok();
                                    }
                                    if response.approved {
                                        let stream_ctx = self.streaming_tool_context(
                                            &event_tx,
                                            &tool_call.id,
                                            &tool_call.name,
                                        );
                                        let result = self
                                            .execute_tool_queued_or_direct(
                                                &tool_call.name,
                                                &tool_call.args,
                                                &stream_ctx,
                                            )
                                            .await;

                                        Self::tool_result_to_tuple(result)
                                    } else {
                                        let rejection_msg = format!(
                                            "Tool '{}' execution was REJECTED by the user. Reason: {}. \
                                             DO NOT retry this tool call unless the user explicitly asks you to.",
                                            tool_call.name,
                                            response.reason.unwrap_or_else(|| "No reason provided".to_string())
                                        );
                                        (rejection_msg, 1, true, None, Vec::new())
                                    }
                                }
                                Ok(Err(_)) => {
                                    // Forward ConfirmationTimeout (channel closed = effectively timed out)
                                    if let Some(tx) = &event_tx {
                                        tx.send(AgentEvent::ConfirmationTimeout {
                                            tool_id: tool_call.id.clone(),
                                            action_taken: "rejected".to_string(),
                                        })
                                        .await
                                        .ok();
                                    }
                                    let msg = format!(
                                        "Tool '{}' confirmation failed: confirmation channel closed",
                                        tool_call.name
                                    );
                                    (msg, 1, true, None, Vec::new())
                                }
                                Err(_) => {
                                    cm.check_timeouts().await;

                                    // Forward ConfirmationTimeout
                                    if let Some(tx) = &event_tx {
                                        tx.send(AgentEvent::ConfirmationTimeout {
                                            tool_id: tool_call.id.clone(),
                                            action_taken: match timeout_action {
                                                crate::hitl::TimeoutAction::Reject => {
                                                    "rejected".to_string()
                                                }
                                                crate::hitl::TimeoutAction::AutoApprove => {
                                                    "auto_approved".to_string()
                                                }
                                            },
                                        })
                                        .await
                                        .ok();
                                    }

                                    match timeout_action {
                                        crate::hitl::TimeoutAction::Reject => {
                                            let msg = format!(
                                                "Tool '{}' execution was REJECTED: user confirmation timed out after {}ms. \
                                                 DO NOT retry this tool call — the user did not approve it. \
                                                 Inform the user that the operation requires their approval and ask them to try again.",
                                                tool_call.name, timeout_ms
                                            );
                                            (msg, 1, true, None, Vec::new())
                                        }
                                        crate::hitl::TimeoutAction::AutoApprove => {
                                            let stream_ctx = self.streaming_tool_context(
                                                &event_tx,
                                                &tool_call.id,
                                                &tool_call.name,
                                            );
                                            let result = self
                                                .execute_tool_queued_or_direct(
                                                    &tool_call.name,
                                                    &tool_call.args,
                                                    &stream_ctx,
                                                )
                                                .await;

                                            Self::tool_result_to_tuple(result)
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
                            (msg, 1, true, None, Vec::new())
                        }
                    }
                };

                let tool_duration = tool_start.elapsed();
                crate::telemetry::record_tool_result(exit_code, tool_duration);

                // Sanitize tool output for sensitive data before it enters the message history
                let output = if let Some(ref sp) = self.config.security_provider {
                    sp.sanitize_output(&output)
                } else {
                    output
                };

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

                // Auto-remember tool result in long-term memory
                if let Some(ref memory) = self.config.memory {
                    let tools_used = [tool_call.name.clone()];
                    let remember_result = if exit_code == 0 {
                        memory
                            .remember_success(effective_prompt, &tools_used, &output)
                            .await
                    } else {
                        memory
                            .remember_failure(effective_prompt, &output, &tools_used)
                            .await
                    };
                    match remember_result {
                        Ok(()) => {
                            if let Some(tx) = &event_tx {
                                let item_type = if exit_code == 0 { "success" } else { "failure" };
                                tx.send(AgentEvent::MemoryStored {
                                    memory_id: uuid::Uuid::new_v4().to_string(),
                                    memory_type: item_type.to_string(),
                                    importance: if exit_code == 0 { 0.8 } else { 0.9 },
                                    tags: vec![item_type.to_string(), tool_call.name.clone()],
                                })
                                .await
                                .ok();
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to store memory after tool execution: {}", e);
                        }
                    }
                }

                // Send tool end event
                if let Some(tx) = &event_tx {
                    tx.send(AgentEvent::ToolEnd {
                        id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        output: output.clone(),
                        exit_code,
                        metadata,
                    })
                    .await
                    .ok();
                }

                // Add tool result to messages
                if images.is_empty() {
                    messages.push(Message::tool_result(&tool_call.id, &output, is_error));
                } else {
                    messages.push(Message::tool_result_with_images(
                        &tool_call.id,
                        &output,
                        &images,
                        is_error,
                    ));
                }
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
        tokio_util::sync::CancellationToken,
    )> {
        let (tx, rx) = mpsc::channel(100);
        let cancel_token = tokio_util::sync::CancellationToken::new();

        let llm_client = self.llm_client.clone();
        let tool_executor = self.tool_executor.clone();
        let tool_context = self.tool_context.clone();
        let config = self.config.clone();
        let tool_metrics = self.tool_metrics.clone();
        let command_queue = self.command_queue.clone();
        let history = history.to_vec();
        let prompt = prompt.to_string();
        let token_clone = cancel_token.clone();

        let handle = tokio::spawn(async move {
            let mut agent = AgentLoop::new(llm_client, tool_executor, tool_context, config);
            if let Some(metrics) = tool_metrics {
                agent = agent.with_tool_metrics(metrics);
            }
            if let Some(queue) = command_queue {
                agent = agent.with_queue(queue);
            }
            agent.execute_with_session(&history, &prompt, None, Some(tx), Some(&token_clone)).await
        });

        Ok((rx, handle, cancel_token))
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

        // Extract goal when goal_tracking is enabled
        let goal = if self.config.goal_tracking {
            let g = self.extract_goal(prompt).await?;
            if let Some(tx) = &event_tx {
                tx.send(AgentEvent::GoalExtracted { goal: g.clone() })
                    .await
                    .ok();
            }
            Some(g)
        } else {
            None
        };

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

        let plan_start = std::time::Instant::now();

        // Execute the plan step by step
        let result = self.execute_plan(history, &plan, event_tx.clone()).await?;

        // Check goal achievement when goal_tracking is enabled
        if self.config.goal_tracking {
            if let Some(ref g) = goal {
                let achieved = self.check_goal_achievement(g, &result.text).await?;
                if achieved {
                    if let Some(tx) = &event_tx {
                        tx.send(AgentEvent::GoalAchieved {
                            goal: g.description.clone(),
                            total_steps: result.messages.len(),
                            duration_ms: plan_start.elapsed().as_millis() as i64,
                        })
                        .await
                        .ok();
                    }
                }
            }
        }

        Ok(result)
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
                    .ok_or_else(|| anyhow::anyhow!("step '{}' not found in plan", step_id))?
                    .clone();
                let step_number = plan
                    .steps
                    .iter()
                    .position(|s| s.id == *step_id)
                    .unwrap_or(0)
                    + 1;

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
                    .execute_loop(&current_history, &step_prompt, None, event_tx.clone(), &tokio_util::sync::CancellationToken::new())
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
                // NOTE: Each parallel branch gets a clone of the base history.
                // Individual branch histories (tool calls, LLM turns) are NOT merged
                // back — only a summary message is appended. This is a deliberate
                // trade-off: merging divergent histories in a deterministic order is
                // complex and the summary approach keeps the context window manageable.
                let ready_steps: Vec<_> = ready
                    .iter()
                    .filter_map(|id| {
                        let step = plan.steps.iter().find(|s| s.id == *id)?.clone();
                        let step_number =
                            plan.steps.iter().position(|s| s.id == *id).unwrap_or(0) + 1;
                        Some((step, step_number))
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
                            .execute_loop(&base_history, &prompt, None, tx, &tokio_util::sync::CancellationToken::new())
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
        assert!(config.prompt_slots.is_empty());
        assert!(config.tools.is_empty()); // Tools are provided externally
        assert_eq!(config.max_tool_rounds, MAX_TOOL_ROUNDS);
        assert!(config.permission_checker.is_none());
        assert!(config.context_providers.is_empty());
        // Built-in skills are always present by default
        let registry = config
            .skill_registry
            .expect("skill_registry must be Some by default");
        assert!(registry.len() >= 7, "expected at least 7 built-in skills");
        assert!(registry.get("code-search").is_some());
        assert!(registry.get("find-bugs").is_some());
    }

    // ========================================================================
    // Mock LLM Client for Testing
    // ========================================================================

    /// Mock LLM client that returns predefined responses
    pub(crate) struct MockLlmClient {
        /// Responses to return (consumed in order)
        responses: std::sync::Mutex<Vec<LlmResponse>>,
        /// Number of calls made
        pub(crate) call_count: AtomicUsize,
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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
            permission_checker: None, // No policy → defaults to Ask
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            permission_checker: Some(Arc::new(permission_policy)),
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

        let config = AgentConfig {
            prompt_slots: SystemPromptSlots {
                extra: Some("Test system prompt".to_string()),
                ..Default::default()
            },
            tools: vec![],
            max_tool_rounds: 10,
            permission_checker: Some(Arc::new(permission_policy)),
            confirmation_manager: Some(confirmation_manager),
            context_providers: vec![],
            planning_enabled: false,
            goal_tracking: false,
            hook_engine: None,
            skill_registry: None,
            ..AgentConfig::default()
        };

        assert!(config.prompt_slots.build().contains("Test system prompt"));
        assert_eq!(config.max_tool_rounds, 10);
        assert!(config.permission_checker.is_some());
        assert!(config.confirmation_manager.is_some());
        assert!(config.context_providers.is_empty());

        // Test Debug trait
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("AgentConfig"));
        assert!(debug_str.contains("permission_checker: true"));
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
            prompt_slots: SystemPromptSlots {
                extra: Some("You are helpful.".to_string()),
                ..Default::default()
            },
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
            prompt_slots: SystemPromptSlots {
                extra: Some("Base system prompt.".to_string()),
                ..Default::default()
            },
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
            prompt_slots: SystemPromptSlots {
                extra: Some("You are helpful.".to_string()),
                ..Default::default()
            },
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
            prompt_slots: SystemPromptSlots {
                extra: Some("You are a coding assistant.".to_string()),
                ..Default::default()
            },
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
            prompt_slots: SystemPromptSlots {
                extra: Some("You are helpful".to_string()),
                ..Default::default()
            },
            tools: vec![],
            max_tool_rounds: 10,
            permission_checker: None,
            confirmation_manager: None,
            context_providers: vec![],
            planning_enabled: true,
            goal_tracking: false,
            hook_engine: None,
            skill_registry: None,
            ..AgentConfig::default()
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
            metadata: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("tool_end"));
    }

    #[test]
    fn test_agent_event_tool_end_has_metadata_field() {
        let event = AgentEvent::ToolEnd {
            id: "t1".to_string(),
            name: "write".to_string(),
            output: "Wrote 5 bytes".to_string(),
            exit_code: 0,
            metadata: Some(
                serde_json::json!({ "before": "old", "after": "new", "file_path": "f.txt" }),
            ),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"before\""));
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
            prompt_slots: SystemPromptSlots {
                extra: Some("Base prompt".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);

        let result = agent.build_augmented_system_prompt(&[]);
        assert!(result.unwrap().contains("Base prompt"));
    }

    #[test]
    fn test_build_augmented_system_prompt_no_custom_slots() {
        let mock_client = Arc::new(MockLlmClient::new(vec![]));
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let agent = AgentLoop::new(
            mock_client,
            tool_executor,
            test_tool_context(),
            AgentConfig::default(),
        );

        let result = agent.build_augmented_system_prompt(&[]);
        // Default slots still produce the default agentic prompt
        assert!(result.is_some());
        assert!(result.unwrap().contains("Core Behaviour"));
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
    // ToolCommand adapter tests
    // ========================================================================

    #[tokio::test]
    async fn test_tool_command_command_type() {
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let cmd = ToolCommand {
            tool_executor: executor,
            tool_name: "read".to_string(),
            tool_args: serde_json::json!({"file": "test.rs"}),
            skill_registry: None,
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
            skill_registry: None,
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

    // ========================================================================
    // Phase 4: Error Recovery & Resilience Tests
    // ========================================================================

    #[test]
    fn test_agent_config_resilience_defaults() {
        let config = AgentConfig::default();
        assert_eq!(config.max_parse_retries, 2);
        assert_eq!(config.tool_timeout_ms, None);
        assert_eq!(config.circuit_breaker_threshold, 3);
    }

    /// 4.1 — Parse error recovery: bails after max_parse_retries exceeded
    #[tokio::test]
    async fn test_parse_error_recovery_bails_after_threshold() {
        // 3 parse errors with max_parse_retries=2: count reaches 3 > 2 → bail
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "c1",
                "bash",
                serde_json::json!({"__parse_error": "unexpected token at position 5"}),
            ),
            MockLlmClient::tool_call_response(
                "c2",
                "bash",
                serde_json::json!({"__parse_error": "missing closing brace"}),
            ),
            MockLlmClient::tool_call_response(
                "c3",
                "bash",
                serde_json::json!({"__parse_error": "still broken"}),
            ),
            MockLlmClient::text_response("Done"), // never reached
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            max_parse_retries: 2,
            ..AgentConfig::default()
        };
        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Do something", None).await;
        assert!(result.is_err(), "should bail after parse error threshold");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("malformed tool arguments"),
            "error should mention malformed tool arguments, got: {}",
            err
        );
    }

    /// 4.1 — Parse error recovery: counter resets after a valid tool execution
    #[tokio::test]
    async fn test_parse_error_counter_resets_on_success() {
        // 2 parse errors (= max_parse_retries, not yet exceeded)
        // Then a valid tool call (resets counter)
        // Then final text — should NOT bail
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "c1",
                "bash",
                serde_json::json!({"__parse_error": "bad args"}),
            ),
            MockLlmClient::tool_call_response(
                "c2",
                "bash",
                serde_json::json!({"__parse_error": "bad args again"}),
            ),
            // Valid call — resets parse_error_count to 0
            MockLlmClient::tool_call_response(
                "c3",
                "bash",
                serde_json::json!({"command": "echo ok"}),
            ),
            MockLlmClient::text_response("All done"),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            max_parse_retries: 2,
            ..AgentConfig::default()
        };
        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Do something", None).await;
        assert!(
            result.is_ok(),
            "should not bail — counter reset after successful tool, got: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap().text, "All done");
    }

    /// 4.2 — Tool timeout: slow tool produces a timeout error result; session continues
    #[tokio::test]
    async fn test_tool_timeout_produces_error_result() {
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "t1",
                "bash",
                serde_json::json!({"command": "sleep 10"}),
            ),
            MockLlmClient::text_response("The command timed out."),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            // 50ms — sleep 10 will never finish
            tool_timeout_ms: Some(50),
            ..AgentConfig::default()
        };
        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "Run sleep", None).await;
        assert!(
            result.is_ok(),
            "session should continue after tool timeout: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap().text, "The command timed out.");
        // LLM called twice: initial request + response after timeout error
        assert_eq!(mock_client.call_count.load(Ordering::SeqCst), 2);
    }

    /// 4.2 — Tool timeout: tool that finishes before the deadline succeeds normally
    #[tokio::test]
    async fn test_tool_within_timeout_succeeds() {
        let mock_client = Arc::new(MockLlmClient::new(vec![
            MockLlmClient::tool_call_response(
                "t1",
                "bash",
                serde_json::json!({"command": "echo fast"}),
            ),
            MockLlmClient::text_response("Command succeeded."),
        ]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            tool_timeout_ms: Some(5_000), // 5 s — echo completes in <100ms
            ..AgentConfig::default()
        };
        let agent = AgentLoop::new(mock_client, tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Run something fast", None).await;
        assert!(
            result.is_ok(),
            "fast tool should succeed: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap().text, "Command succeeded.");
    }

    /// 4.3 — Circuit breaker: retries non-streaming LLM failures up to threshold
    #[tokio::test]
    async fn test_circuit_breaker_retries_non_streaming() {
        // Empty response list → every call bails with "No more mock responses"
        // threshold=2 → tries twice, then bails with circuit-breaker message
        let mock_client = Arc::new(MockLlmClient::new(vec![]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            circuit_breaker_threshold: 2,
            ..AgentConfig::default()
        };
        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "Hello", None).await;
        assert!(result.is_err(), "should fail when LLM always errors");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("circuit breaker"),
            "error should mention circuit breaker, got: {}",
            err
        );
        assert_eq!(
            mock_client.call_count.load(Ordering::SeqCst),
            2,
            "should make exactly threshold=2 LLM calls"
        );
    }

    /// 4.3 — Circuit breaker: threshold=1 bails on the very first failure
    #[tokio::test]
    async fn test_circuit_breaker_threshold_one_no_retry() {
        let mock_client = Arc::new(MockLlmClient::new(vec![]));

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            circuit_breaker_threshold: 1,
            ..AgentConfig::default()
        };
        let agent = AgentLoop::new(
            mock_client.clone(),
            tool_executor,
            test_tool_context(),
            config,
        );
        let result = agent.execute(&[], "Hello", None).await;
        assert!(result.is_err());
        assert_eq!(
            mock_client.call_count.load(Ordering::SeqCst),
            1,
            "with threshold=1 exactly one attempt should be made"
        );
    }

    /// 4.3 — Circuit breaker: succeeds when LLM recovers before hitting threshold
    #[tokio::test]
    async fn test_circuit_breaker_succeeds_if_llm_recovers() {
        // First call fails, second call succeeds; threshold=3 — recovery within threshold
        struct FailOnceThenSucceed {
            inner: MockLlmClient,
            failed_once: std::sync::atomic::AtomicBool,
            call_count: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl LlmClient for FailOnceThenSucceed {
            async fn complete(
                &self,
                messages: &[Message],
                system: Option<&str>,
                tools: &[ToolDefinition],
            ) -> Result<LlmResponse> {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                let already_failed = self
                    .failed_once
                    .swap(true, std::sync::atomic::Ordering::SeqCst);
                if !already_failed {
                    anyhow::bail!("transient network error");
                }
                self.inner.complete(messages, system, tools).await
            }

            async fn complete_streaming(
                &self,
                messages: &[Message],
                system: Option<&str>,
                tools: &[ToolDefinition],
            ) -> Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
                self.inner.complete_streaming(messages, system, tools).await
            }
        }

        let mock = Arc::new(FailOnceThenSucceed {
            inner: MockLlmClient::new(vec![MockLlmClient::text_response("Recovered!")]),
            failed_once: std::sync::atomic::AtomicBool::new(false),
            call_count: AtomicUsize::new(0),
        });

        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let config = AgentConfig {
            circuit_breaker_threshold: 3,
            ..AgentConfig::default()
        };
        let agent = AgentLoop::new(mock.clone(), tool_executor, test_tool_context(), config);
        let result = agent.execute(&[], "Hello", None).await;
        assert!(
            result.is_ok(),
            "should succeed when LLM recovers within threshold: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap().text, "Recovered!");
        assert_eq!(
            mock.call_count.load(Ordering::SeqCst),
            2,
            "should have made exactly 2 calls (1 fail + 1 success)"
        );
    }

    // ── Continuation detection tests ─────────────────────────────────────────

    #[test]
    fn test_looks_incomplete_empty() {
        assert!(AgentLoop::looks_incomplete(""));
        assert!(AgentLoop::looks_incomplete("   "));
    }

    #[test]
    fn test_looks_incomplete_trailing_colon() {
        assert!(AgentLoop::looks_incomplete("Let me check the file:"));
        assert!(AgentLoop::looks_incomplete("Next steps:"));
    }

    #[test]
    fn test_looks_incomplete_ellipsis() {
        assert!(AgentLoop::looks_incomplete("Working on it..."));
        assert!(AgentLoop::looks_incomplete("Processing…"));
    }

    #[test]
    fn test_looks_incomplete_intent_phrases() {
        assert!(AgentLoop::looks_incomplete(
            "I'll start by reading the file."
        ));
        assert!(AgentLoop::looks_incomplete(
            "Let me check the configuration."
        ));
        assert!(AgentLoop::looks_incomplete("I will now run the tests."));
        assert!(AgentLoop::looks_incomplete(
            "I need to update the Cargo.toml."
        ));
    }

    #[test]
    fn test_looks_complete_final_answer() {
        // Clear final answers should NOT trigger continuation
        assert!(!AgentLoop::looks_incomplete(
            "The tests pass. All changes have been applied successfully."
        ));
        assert!(!AgentLoop::looks_incomplete(
            "Done. I've updated the three files and verified the build succeeds."
        ));
        assert!(!AgentLoop::looks_incomplete("42"));
        assert!(!AgentLoop::looks_incomplete("Yes."));
    }

    #[test]
    fn test_looks_incomplete_multiline_complete() {
        let text = "Here is the summary:\n\n- Fixed the bug in agent.rs\n- All tests pass\n- Build succeeds";
        assert!(!AgentLoop::looks_incomplete(text));
    }
}
