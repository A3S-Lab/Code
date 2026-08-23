//! Agent Facade API
//!
//! High-level, ergonomic API for using A3S Code as an embedded library.
//!
//! ## Example
//!
//! ```rust,no_run
//! use a3s_code_core::Agent;
//!
//! # async fn run() -> anyhow::Result<()> {
//! let agent = Agent::new("agent.acl").await?;
//! let session = agent.session_async("/my-project", None).await?;
//! let result = session.send("Explain the auth module", None).await?;
//! println!("{}", result.text);
//! # Ok(())
//! # }
//! ```

use crate::agent::{AgentConfig, AgentEvent, AgentResult};
use crate::commands::CommandRegistry;
use crate::config::CodeConfig;
use crate::error::Result;
use crate::hitl::PendingConfirmationInfo;
use crate::llm::{LlmClient, Message};
use crate::prompts::{PlanningMode, SystemPromptSlots};
use crate::queue::{
    ExternalTask, ExternalTaskResult, LaneHandlerConfig, SessionLane, SessionQueueConfig,
    SessionQueueStats,
};
use crate::tools::{ToolContext, ToolExecutor};
use a3s_lane::{DeadLetter, MetricsSnapshot};
use a3s_memory::MemoryStore;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
mod agent_binding;
mod agent_bootstrap;
mod agent_facade;
mod agent_loop_runtime;
mod agent_sessions;
mod capabilities;
mod capability_facade;
mod command_runtime;
mod conversation_runtime;
mod direct_tool_facade;
mod direct_tools;
mod governance_facade;
mod hook_control;
mod project_instructions;
mod run_admission;
mod run_facade;
mod run_lifecycle;
mod runtime;
mod runtime_events;
mod session_builder;
mod session_clock;
mod session_close;
mod session_commands;
mod session_config;
mod session_extensions;
mod session_facade;
mod session_hitl;
mod session_options;
mod session_persistence;
mod session_queue;
mod session_runs;
mod session_runtime;
mod session_save;
mod session_verification;
mod session_view;
mod workflow_facade;
pub use agent_facade::{Agent, SessionBuilder};
use direct_tools::DirectToolRuntime;
use hook_control::HookControl;
use runtime_events::ActiveToolState;
use session_close::SessionCloseHandle;
use session_extensions::SessionExtensionRuntime;
use session_hitl::HitlControl;
use session_queue::QueueControl;
use session_runs::RunControl;
use session_verification::VerificationRuntime;
use session_view::SessionView;

/// Canonicalize a path, stripping the Windows `\\?\` UNC prefix to avoid
/// polluting workspace strings throughout the system (prompts, session data, etc.).
fn safe_canonicalize(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(p) => strip_unc_prefix(p),
        Err(_) => path.to_path_buf(),
    }
}

/// Strip the Windows extended-length path prefix (`\\?\`) that `canonicalize()` adds.
/// On non-Windows this is a no-op.
fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path
}

// ============================================================================
// ToolCallResult
// ============================================================================

/// Result of a direct tool execution (no LLM).
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub name: String,
    pub output: String,
    pub exit_code: i32,
    pub metadata: Option<serde_json::Value>,
    /// Structured discriminant for tool failures. `None` when the tool
    /// either succeeded or failed without a typed reason (the message in
    /// `output` is then the only diagnostic). Populated for known
    /// kinds such as `VersionConflict` so SDK callers can branch on the
    /// `type` field instead of regex-matching `output`.
    pub error_kind: Option<crate::tools::ToolErrorKind>,
}

/// Result of admitting a host-selected run identity for detached execution.
///
/// `Started` owns a supervisor task that drains the ordinary Code event stream
/// into the authoritative run store. Dropping the handle detaches the task;
/// call [`AgentSession::close`] for graceful cancellation and cleanup.
pub enum AgentRunSpawn {
    Started {
        snapshot: crate::run::RunSnapshot,
        worker: JoinHandle<()>,
    },
    Replayed {
        snapshot: crate::run::RunSnapshot,
    },
}

impl AgentRunSpawn {
    pub const fn replayed(&self) -> bool {
        matches!(self, Self::Replayed { .. })
    }

    pub fn snapshot(&self) -> &crate::run::RunSnapshot {
        match self {
            Self::Started { snapshot, .. } | Self::Replayed { snapshot } => snapshot,
        }
    }
}

// ============================================================================
// ReadFileOptions
// ============================================================================

/// Optional line-range controls for direct `read` tool calls.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadFileOptions {
    /// 0-indexed line offset to start reading from.
    pub offset: Option<usize>,
    /// Maximum number of lines to read.
    pub limit: Option<usize>,
}

// ============================================================================
// SessionOptions
// ============================================================================

/// Optional per-session overrides.
#[derive(Clone, Default)]
pub struct SessionOptions {
    /// Override the default model. Format: `"provider/model"` (e.g., `"openai/gpt-4o"`).
    pub model: Option<String>,
    /// Priority used for top-level operations created by this session.
    pub task_priority: crate::task_scheduler::TaskPriority,
    /// Extra directories to scan for agent files.
    /// Merged with any global `agent_dirs` from [`CodeConfig`].
    pub agent_dirs: Vec<PathBuf>,
    /// Reproducible disposable workers registered for task delegation.
    /// Explicit session workers override agents loaded from directories by name.
    pub worker_agents: Vec<crate::subagent::WorkerAgentSpec>,
    /// Optional queue configuration for lane-based tool execution.
    ///
    /// When set, enables priority-based tool scheduling with parallel execution
    /// of read-only (Query-lane) tools, DLQ, metrics, and external task handling.
    pub queue_config: Option<SessionQueueConfig>,
    /// Optional security provider for taint tracking and output sanitization
    pub security_provider: Option<Arc<dyn crate::security::SecurityProvider>>,
    /// Optional host-supplied LLM client.
    ///
    /// When set, it is used directly, overriding the `provider/model`
    /// factory resolution — the one Action-layer backend that was previously
    /// only injectable in test code. Lets a host plug in a provider the
    /// built-in factory does not cover, a deterministic record/replay client,
    /// or an HTTP-layer proxy/audit wrapper. Mirrors `workspace_services`.
    pub llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
    /// Optional context providers for RAG
    pub context_providers: Vec<Arc<dyn crate::context::ContextProvider>>,
    /// One exact-generation cognitive package supplied by the embedding host.
    ///
    /// This is separate from general-purpose RAG providers: Code persists the
    /// immutable package binding and treats provider failure as fatal. The host
    /// retains A3S Use lease, Registry, installation, and lifecycle authority.
    pub cognitive_context: Option<crate::cognitive_context::CognitiveContextSession>,
    /// Optional confirmation manager for HITL
    pub confirmation_manager: Option<Arc<dyn crate::hitl::ConfirmationProvider>>,
    /// Optional confirmation policy (will be used to create ConfirmationManager if confirmation_manager is not set)
    pub confirmation_policy: Option<crate::hitl::ConfirmationPolicy>,
    /// Optional permission checker
    pub permission_checker: Option<Arc<dyn crate::permissions::PermissionChecker>>,
    /// Serializable permission policy used to build the checker, when available.
    pub permission_policy: Option<crate::permissions::PermissionPolicy>,
    /// Enable planning
    pub planning_mode: PlanningMode,
    /// Enable goal tracking
    pub goal_tracking: bool,
    /// Extra directories to scan for skill files (*.md).
    /// Merged with any global `skill_dirs` from [`CodeConfig`].
    pub skill_dirs: Vec<PathBuf>,
    /// Optional skill registry for instruction injection
    pub skill_registry: Option<Arc<crate::skills::SkillRegistry>>,
    /// Whether active skill `allowed-tools` restrict ordinary session tool calls.
    ///
    /// Defaults to false so ordinary tools continue through permission policy,
    /// hooks, and HITL. Set true to restore the legacy global active-skill
    /// restriction behavior.
    pub enforce_active_skill_tool_restrictions: Option<bool>,
    /// Custom memory store override for long-term memory persistence.
    ///
    /// Sessions resolve a default store when this is not set.
    pub memory_store: Option<Arc<dyn MemoryStore>>,
    /// Host observers notified after successful durable memory writes.
    pub memory_observers: Vec<Arc<dyn crate::memory::MemoryObserver>>,
    /// Deferred file memory directory — constructed async in `build_session()`
    pub(crate) file_memory_dir: Option<PathBuf>,
    /// Optional session store for persistence
    pub session_store: Option<Arc<dyn crate::store::SessionStore>>,
    /// Deferred file session-store directory, initialized by the async builder.
    pub(crate) file_session_store_dir: Option<PathBuf>,
    /// Explicit session ID (auto-generated if not set)
    pub session_id: Option<String>,
    /// Multi-tenant identifier. Framework only transports this string;
    /// the host decides what "tenant" means and how to
    /// aggregate/bill on it. Emitted to hooks/traces, persisted in
    /// `SessionData`, never interpreted by core.
    pub tenant_id: Option<String>,
    /// Identity of the principal that triggered this session (user id,
    /// service account, etc). Treated as opaque.
    pub principal: Option<String>,
    /// Logical identifier of the agent template / definition the session
    /// was instantiated from. Lets the host aggregate sessions by
    /// "which agent recipe" independent of the concrete session id.
    pub agent_template_id: Option<String>,
    /// Distributed-trace correlation id. Propagated through hooks/traces
    /// so a session's events join with upstream/downstream work in the
    /// host's observability pipeline.
    pub correlation_id: Option<String>,
    /// Optional host-supplied budget / quota guard. The framework calls
    /// into it before each LLM call (and reports actuals after) so the
    /// host can refuse or rate-limit at the cluster level. Default is
    /// `None` (no enforcement — equivalent to
    /// [`NoopBudgetGuard`](crate::budget::NoopBudgetGuard)).
    pub budget_guard: Option<Arc<dyn crate::budget::BudgetGuard>>,
    /// Optional host-provided ID/Clock pair. Replaces the default
    /// random-UUID + wall-clock pair, enabling deterministic replay
    /// on another node. `None` keeps pre-P2 behaviour.
    pub host_env: Option<Arc<crate::host_env::HostEnv>>,
    /// Optional FIFO retention caps on the session's in-memory stores
    /// (run records, run events, trace events, terminal subagent
    /// tasks). `None` selects [`SessionRetentionLimits::default`](crate::retention::SessionRetentionLimits::default),
    /// which is finite. Pass [`SessionRetentionLimits::unbounded`](crate::retention::SessionRetentionLimits::unbounded)
    /// explicitly to retain everything.
    pub retention_limits: Option<crate::retention::SessionRetentionLimits>,
    /// Optional structured JSONL trajectory config.
    ///
    /// When set, a3s-code records user prompts, LLM turns, tool calls,
    /// tool observations, token usage, and execution end status for RL
    /// training or service data collection. If unset, the same config can
    /// be enabled by `A3S_CODE_TRAJECTORY_PATH`.
    pub rl_trajectory: Option<crate::rl_trajectory::RlTrajectoryConfig>,
    /// Request token-level log probabilities from compatible LLM providers.
    ///
    /// This is off by default because many public providers reject logprob
    /// requests with tool calls. Training/evaluation harnesses using compatible
    /// OpenAI-style backends can enable it explicitly.
    pub llm_logprobs: Option<bool>,
    /// Number of alternative token logprobs to request per generated token.
    pub llm_top_logprobs: Option<usize>,
    /// Auto-save after each completed `send()` or default-history `stream()` call.
    pub auto_save: bool,
    /// Optional artifact retention limits for large tool/program outputs.
    pub artifact_store_limits: Option<crate::tools::ArtifactStoreLimits>,
    /// Host-pinned deterministic policy for projecting large tool results.
    ///
    /// `None` selects the conservative compatibility profile. The resolved
    /// policy is persisted and cannot drift when the session is resumed.
    pub tool_result_transform_policy: Option<crate::tools::ToolResultTransformPolicyV1>,
    /// Max consecutive parse errors before aborting (overrides default of 2).
    /// `None` uses the `AgentConfig` default.
    pub max_parse_retries: Option<u32>,
    /// Per-tool execution timeout in milliseconds.
    /// `None` = no timeout (default).
    pub tool_timeout_ms: Option<u64>,
    /// Per-model API HTTP timeout in milliseconds.
    /// `None` = no timeout (default).
    pub llm_api_timeout_ms: Option<u64>,
    /// Circuit-breaker threshold: max consecutive LLM API failures before
    /// aborting in non-streaming mode (overrides default of 3).
    /// `None` uses the `AgentConfig` default.
    pub circuit_breaker_threshold: Option<u32>,
    /// Max consecutive identical tool signatures before the guard turns the
    /// duplicate call into a failed tool observation.
    ///
    /// `None` uses the `AgentConfig` default. Hosts can raise this for long
    /// research sessions where repeating a read/search with the same arguments
    /// is wasteful but should not make the whole run brittle.
    pub duplicate_tool_call_threshold: Option<u32>,
    /// Optional concrete sandbox implementation.
    ///
    /// When set, `bash` tool commands are routed through this sandbox instead
    /// of `std::process::Command`. The host application constructs and owns
    /// the implementation (e.g., an A3S Box–backed handle).
    pub sandbox_handle: Option<Arc<dyn crate::sandbox::BashSandbox>>,
    /// Optional host-provided workspace backend.
    ///
    /// When set, built-in tools such as `read`, `write`, `ls`, and `bash`
    /// execute against these workspace capabilities instead of assuming the
    /// server-local filesystem. This is the primary extension point for DFS,
    /// browser, container, and remote workspace deployments.
    pub workspace_services: Option<Arc<crate::workspace::WorkspaceServices>>,
    /// Optional session-owned semantic workspace retrieval configuration.
    ///
    /// Construction is asynchronous and ephemeral. Source chunks stay in the
    /// workspace catalog, vectors stay in memory, and no vector state is
    /// serialized with the session snapshot.
    pub workspace_retrieval: Option<crate::workspace::WorkspaceRetrievalOptions>,
    /// Enable auto-compaction when context usage exceeds threshold.
    pub auto_compact: bool,
    /// Context usage percentage threshold for auto-compaction (0.0 - 1.0).
    /// Default: 0.80 (80%).
    pub auto_compact_threshold: Option<f32>,
    /// Model context window used for automatic compaction accounting.
    ///
    /// When omitted, the configured model's declared context limit is used,
    /// falling back to the agent default when the model has no declaration.
    pub max_context_tokens: Option<usize>,
    /// Inject a continuation message when the LLM stops without completing the task.
    /// `None` uses the `AgentConfig` default (true).
    pub continuation_enabled: Option<bool>,
    /// Maximum continuation injections per execution.
    /// `None` uses the `AgentConfig` default (3).
    pub max_continuation_turns: Option<u32>,
    /// Maximum execution time in milliseconds.
    /// `None` = no timeout (default).
    /// When set, the execution loop will abort if it exceeds this duration.
    pub max_execution_time_ms: Option<u64>,
    /// Optional MCP manager for connecting to external MCP servers.
    ///
    /// When set, all tools from connected MCP servers are registered and
    /// available during agent execution with names like `mcp__server__tool`.
    pub mcp_manager: Option<Arc<crate::mcp::manager::McpManager>>,
    /// Sampling temperature (0.0–1.0). Overrides the provider default.
    pub temperature: Option<f32>,
    /// Extended thinking budget in tokens (Anthropic only).
    pub thinking_budget: Option<usize>,
    /// Per-session tool round limit override.
    ///
    /// When set, overrides the agent-level `max_tool_rounds` for this session only.
    /// Maps directly from [`AgentDefinition::max_steps`](crate::subagent::AgentDefinition::max_steps)
    /// when creating sessions
    /// via [`Agent::session_for_agent`].
    pub max_tool_rounds: Option<usize>,
    /// Per-session parallel fan-out limit override.
    ///
    /// Applies to delegated `task` fan-out, the legacy `parallel_task` alias,
    /// plan wave execution, and safe parallel write batches.
    pub max_parallel_tasks: Option<usize>,
    /// Per-session automatic subagent delegation override.
    pub auto_delegation: Option<crate::config::AutoDelegationConfig>,
    /// Per-session switch for model-visible manual child-agent tools.
    ///
    /// This overlays the effective automatic delegation config instead of
    /// replacing it, so callers can hide `task` and its compatibility alias
    /// while preserving other delegation settings.
    pub manual_delegation_enabled: Option<bool>,
    /// Per-session kill switch for automatic parallel child-agent fan-out.
    ///
    /// This overlays the effective automatic delegation config instead of
    /// replacing it, so callers can disable auto fan-out without disabling
    /// automatic delegation itself.
    pub auto_parallel_delegation: Option<bool>,
    /// Slot-based system prompt customization.
    ///
    /// When set, overrides the agent-level prompt slots for this session.
    /// Users can customize role, guidelines, response style, and extra instructions
    /// without losing the core agentic capabilities.
    pub prompt_slots: Option<SystemPromptSlots>,
    /// Optional external hook executor.
    ///
    /// When set, it replaces the built-in `HookEngine` for this session.
    pub hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
}

// ============================================================================
// AgentSession
// ============================================================================

/// Workspace-bound session. All LLM and tool operations happen here.
///
/// History is automatically accumulated after each `send()` call and after
/// `stream()` completes when no custom history is supplied.
/// Use `history()` to retrieve the current conversation log.
///
/// Conversation operations are single-flight, including slash commands and
/// checkpoint resume. An overlapping call returns
/// [`CodeError::SessionBusy`](crate::error::CodeError::SessionBusy) immediately.
/// A streaming operation remains active until its returned handle completes.
pub struct AgentSession {
    llm_client: Arc<dyn LlmClient>,
    /// Provider-reported generation capacity shared by every loop and
    /// host-direct tool call created for this session.
    model_generation_admission: crate::llm::ModelGenerationAdmission,
    /// Agent-wide execution admission shared across sibling sessions.
    task_scheduler: Arc<crate::task_scheduler::TaskScheduler>,
    /// Base priority for this session's top-level operations.
    task_priority: crate::task_scheduler::TaskPriority,
    tool_executor: Arc<ToolExecutor>,
    /// Session-owned atomic host capability catalog. A Run pins one immutable
    /// generation before its model definitions and executor are assembled.
    capability_catalog: Arc<crate::capability::CapabilityCatalog>,
    tool_context: ToolContext,
    config: AgentConfig,
    tool_result_transform_policy: crate::tools::ToolResultTransformPolicyV1,
    /// Host provider plus the durable exact-generation package binding.
    cognitive_context: Option<crate::cognitive_context::CognitiveContextSession>,
    workspace: PathBuf,
    /// Unique session identifier.
    session_id: String,
    /// Internal conversation history, auto-updated after each `send()` and default-history `stream()`.
    history: Arc<RwLock<Vec<Message>>>,
    /// Fail-fast single-flight admission for transcript-affecting operations.
    run_admission: Arc<run_admission::RunAdmission>,
    /// Optional lane queue for priority-based tool execution.
    command_queue: Option<Arc<crate::session_lane_queue::SessionLaneQueue>>,
    /// Long-term memory handle.
    ///
    /// Built sessions resolve a default memory store. This remains optional for
    /// compatibility with lower-level/manual construction paths.
    memory: Option<Arc<crate::memory::AgentMemory>>,
    /// Optional session store for persistence.
    session_store: Option<Arc<dyn crate::store::SessionStore>>,
    /// Runtime-owned fields used to build lossless persistence generations.
    persistence_state: Arc<RwLock<session_persistence::SessionPersistenceState>>,
    /// Auto-save after each completed `send()` or default-history `stream()`.
    auto_save: bool,
    /// Hook engine for lifecycle event interception.
    hook_engine: Arc<crate::hooks::HookEngine>,
    /// Optional external hook executor. When set, replaces `hook_engine` as the
    /// executor passed to each `AgentLoop`.
    hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
    /// Deferred init warning: emitted as PersistenceFailed on first send() if set.
    init_warning: Option<String>,
    /// Slash command registry for `/command` dispatch.
    /// Uses interior mutability so commands can be registered on a shared `Arc<AgentSession>`.
    command_registry: std::sync::Mutex<CommandRegistry>,
    /// Model identifier for display (e.g., "anthropic/claude-sonnet-4-20250514").
    model_name: String,
    /// Session-private MCP manager used by live add/remove operations.
    mcp_manager: Arc<crate::mcp::manager::McpManager>,
    /// Read-only MCP sources inherited from the parent agent and session options.
    inherited_mcp_managers: Vec<Arc<crate::mcp::manager::McpManager>>,
    /// Ordered MCP capability sources inherited by delegated child runs.
    mcp_managers: Vec<Arc<crate::mcp::manager::McpManager>>,
    /// Shared agent registry — populated at session creation; extended via register_agent_dir().
    agent_registry: Arc<crate::subagent::AgentRegistry>,
    /// Cancellation token for the current operation (send/stream).
    /// Stored so that cancel() can abort ongoing LLM calls.
    cancel_token: Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    /// ID of the run currently attached to the active cancellation token.
    current_run_id: Arc<tokio::sync::Mutex<Option<String>>>,
    /// In-memory run snapshots and event replay buffer for this session.
    run_store: Arc<crate::run::InMemoryRunStore>,
    /// Materialized view of delegated subagent task lifecycle, populated from runtime events.
    subagent_tasks: Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>,
    /// Currently executing tools observed from runtime events.
    active_tools: Arc<tokio::sync::RwLock<HashMap<String, ActiveToolState>>>,
    /// Compact execution traces for this session.
    trace_sink: crate::trace::InMemoryTraceSink,
    /// Structured completion evidence collected from agent and explicit verification runs.
    verification_reports: Arc<RwLock<Vec<crate::verification::VerificationReport>>>,
    /// Set once `close()` has been called. Subsequent send/stream calls
    /// fast-fail with [`crate::error::CodeError::SessionClosed`].
    closed: Arc<std::sync::atomic::AtomicBool>,
    /// Session-level parent cancellation token.
    ///
    /// Every in-flight run (blocking send, stream, delegated subagent task)
    /// derives its per-operation token from this one via `child_token()`,
    /// so `session_cancel.cancel()` cascades to all of them. `close()` fires
    /// this token first, after which any new `child_token()` returns an
    /// already-cancelled token (defending against close/spawn races).
    pub(crate) session_cancel: tokio_util::sync::CancellationToken,
    /// Optional asynchronous semantic workspace index owned by this session.
    workspace_retrieval: Option<Arc<crate::workspace::WorkspaceRetrievalRuntime>>,
    /// Shared `Arc`-handle used by both [`AgentSession::close`] and the
    /// parent [`Agent`]'s registry. The handle bundles every field needed
    /// to perform the close sequence so the two entry points cannot drift.
    close_handle: Arc<SessionCloseHandle>,
    /// Runtime-mutable override for the budget guard. When set, takes
    /// precedence over `config.budget_guard` on the next agent-loop
    /// build. Lets SDK callers (Node especially) install a host-side
    /// guard after `session()` has returned without ever putting a
    /// JS callable into `SessionOptions`.
    runtime_budget_guard: std::sync::Mutex<Option<Arc<dyn crate::budget::BudgetGuard>>>,
    /// Multi-tenant label. Framework only carries the string; semantics
    /// belong to the host.
    pub(crate) tenant_id: Option<String>,
    /// Principal that triggered the session (user / service / etc.).
    pub(crate) principal: Option<String>,
    /// Logical identifier of the agent template the session was
    /// instantiated from.
    pub(crate) agent_template_id: Option<String>,
    /// Distributed-trace correlation id propagated to hooks / traces.
    pub(crate) correlation_id: Option<String>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod capability_runtime_tests;
#[cfg(test)]
mod replacement_tests;
#[cfg(test)]
mod retrieval_qa_tests;
#[cfg(test)]
mod retrieval_tests;
#[cfg(test)]
mod tests;
