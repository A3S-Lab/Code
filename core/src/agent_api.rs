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
//! let session = agent.session("/my-project", None)?;
//! let result = session.send("Explain the auth module", None).await?;
//! println!("{}", result.text);
//! # Ok(())
//! # }
//! ```

use crate::agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
use crate::commands::{CommandAction, CommandContext, CommandRegistry};
use crate::config::CodeConfig;
use crate::context::{ContextItem, ContextType, StaticContextProvider};
use crate::error::{read_or_recover, write_or_recover, CodeError, Result};
use crate::hitl::PendingConfirmationInfo;
use crate::llm::{LlmClient, Message};
use crate::prompts::{PlanningMode, SystemPromptSlots};
use crate::queue::{
    ExternalTask, ExternalTaskResult, LaneHandlerConfig, SessionLane, SessionQueueConfig,
    SessionQueueStats,
};
use crate::session_lane_queue::SessionLaneQueue;
use crate::text::truncate_utf8;
use crate::tools::{ToolContext, ToolExecutor};
use a3s_lane::{DeadLetter, MetricsSnapshot};
use a3s_memory::{FileMemoryStore, MemoryStore};
use anyhow::Context;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

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
}

// ============================================================================
// SessionOptions
// ============================================================================

/// Optional per-session overrides.
#[derive(Clone, Default)]
pub struct SessionOptions {
    /// Override the default model. Format: `"provider/model"` (e.g., `"openai/gpt-4o"`).
    pub model: Option<String>,
    /// Extra directories to scan for agent files.
    /// Merged with any global `agent_dirs` from [`CodeConfig`].
    pub agent_dirs: Vec<PathBuf>,
    /// Optional queue configuration for lane-based tool execution.
    ///
    /// When set, enables priority-based tool scheduling with parallel execution
    /// of read-only (Query-lane) tools, DLQ, metrics, and external task handling.
    pub queue_config: Option<SessionQueueConfig>,
    /// Optional security provider for taint tracking and output sanitization
    pub security_provider: Option<Arc<dyn crate::security::SecurityProvider>>,
    /// Optional context providers for RAG
    pub context_providers: Vec<Arc<dyn crate::context::ContextProvider>>,
    /// Optional confirmation manager for HITL
    pub confirmation_manager: Option<Arc<dyn crate::hitl::ConfirmationProvider>>,
    /// Optional permission checker
    pub permission_checker: Option<Arc<dyn crate::permissions::PermissionChecker>>,
    /// Enable planning
    pub planning_mode: PlanningMode,
    /// Enable goal tracking
    pub goal_tracking: bool,
    /// Extra directories to scan for skill files (*.md).
    /// Merged with any global `skill_dirs` from [`CodeConfig`].
    pub skill_dirs: Vec<PathBuf>,
    /// Optional skill registry for instruction injection
    pub skill_registry: Option<Arc<crate::skills::SkillRegistry>>,
    /// Optional memory store for long-term memory persistence
    pub memory_store: Option<Arc<dyn MemoryStore>>,
    /// Deferred file memory directory — constructed async in `build_session()`
    pub(crate) file_memory_dir: Option<PathBuf>,
    /// Optional session store for persistence
    pub session_store: Option<Arc<dyn crate::store::SessionStore>>,
    /// Explicit session ID (auto-generated if not set)
    pub session_id: Option<String>,
    /// Auto-save after each completed `send()` or default-history `stream()` call.
    pub auto_save: bool,
    /// Optional artifact retention limits for large tool/program outputs.
    pub artifact_store_limits: Option<crate::tools::ArtifactStoreLimits>,
    /// Max consecutive parse errors before aborting (overrides default of 2).
    /// `None` uses the `AgentConfig` default.
    pub max_parse_retries: Option<u32>,
    /// Per-tool execution timeout in milliseconds.
    /// `None` = no timeout (default).
    pub tool_timeout_ms: Option<u64>,
    /// Circuit-breaker threshold: max consecutive LLM API failures before
    /// aborting in non-streaming mode (overrides default of 3).
    /// `None` uses the `AgentConfig` default.
    pub circuit_breaker_threshold: Option<u32>,
    /// Optional concrete sandbox implementation.
    ///
    /// When set, `bash` tool commands are routed through this sandbox instead
    /// of `std::process::Command`. The host application constructs and owns
    /// the implementation (e.g., an A3S Box–backed handle).
    pub sandbox_handle: Option<Arc<dyn crate::sandbox::BashSandbox>>,
    /// Enable auto-compaction when context usage exceeds threshold.
    pub auto_compact: bool,
    /// Context usage percentage threshold for auto-compaction (0.0 - 1.0).
    /// Default: 0.80 (80%).
    pub auto_compact_threshold: Option<f32>,
    /// Inject a continuation message when the LLM stops without completing the task.
    /// `None` uses the `AgentConfig` default (true).
    pub continuation_enabled: Option<bool>,
    /// Maximum continuation injections per execution.
    /// `None` uses the `AgentConfig` default (3).
    pub max_continuation_turns: Option<u32>,
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
    /// Maps directly from [`AgentDefinition::max_steps`] when creating sessions
    /// via [`Agent::session_for_agent`].
    pub max_tool_rounds: Option<usize>,
    /// Slot-based system prompt customization.
    ///
    /// When set, overrides the agent-level prompt slots for this session.
    /// Users can customize role, guidelines, response style, and extra instructions
    /// without losing the core agentic capabilities.
    pub prompt_slots: Option<SystemPromptSlots>,
    /// Optional external hook executor (e.g. an AHP harness server).
    ///
    /// When set, **replaces** the built-in `HookEngine` for this session.
    /// All 11 lifecycle events are forwarded to the executor instead of being
    /// dispatched locally. The executor is also propagated to sub-agents via
    /// the sentinel hook mechanism.
    pub hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
}

impl std::fmt::Debug for SessionOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionOptions")
            .field("model", &self.model)
            .field("agent_dirs", &self.agent_dirs)
            .field("skill_dirs", &self.skill_dirs)
            .field("queue_config", &self.queue_config)
            .field("security_provider", &self.security_provider.is_some())
            .field("context_providers", &self.context_providers.len())
            .field("confirmation_manager", &self.confirmation_manager.is_some())
            .field("permission_checker", &self.permission_checker.is_some())
            .field("planning_mode", &self.planning_mode)
            .field("goal_tracking", &self.goal_tracking)
            .field(
                "skill_registry",
                &self
                    .skill_registry
                    .as_ref()
                    .map(|r| format!("{} skills", r.len())),
            )
            .field("memory_store", &self.memory_store.is_some())
            .field("session_store", &self.session_store.is_some())
            .field("session_id", &self.session_id)
            .field("auto_save", &self.auto_save)
            .field("artifact_store_limits", &self.artifact_store_limits)
            .field("max_parse_retries", &self.max_parse_retries)
            .field("tool_timeout_ms", &self.tool_timeout_ms)
            .field("circuit_breaker_threshold", &self.circuit_breaker_threshold)
            .field("sandbox_handle", &self.sandbox_handle.is_some())
            .field("auto_compact", &self.auto_compact)
            .field("auto_compact_threshold", &self.auto_compact_threshold)
            .field("continuation_enabled", &self.continuation_enabled)
            .field("max_continuation_turns", &self.max_continuation_turns)
            .field("mcp_manager", &self.mcp_manager.is_some())
            .field("temperature", &self.temperature)
            .field("thinking_budget", &self.thinking_budget)
            .field("max_tool_rounds", &self.max_tool_rounds)
            .field("prompt_slots", &self.prompt_slots.is_some())
            .finish()
    }
}

impl SessionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_agent_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.agent_dirs.push(dir.into());
        self
    }

    pub fn with_queue_config(mut self, config: SessionQueueConfig) -> Self {
        self.queue_config = Some(config);
        self
    }

    /// Enable default security provider with taint tracking and output sanitization
    pub fn with_default_security(mut self) -> Self {
        self.security_provider = Some(Arc::new(crate::security::DefaultSecurityProvider::new()));
        self
    }

    /// Set a custom security provider
    pub fn with_security_provider(
        mut self,
        provider: Arc<dyn crate::security::SecurityProvider>,
    ) -> Self {
        self.security_provider = Some(provider);
        self
    }

    /// Add a file system context provider for simple RAG
    pub fn with_fs_context(mut self, root_path: impl Into<PathBuf>) -> Self {
        let config = crate::context::FileSystemContextConfig::new(root_path);
        self.context_providers
            .push(Arc::new(crate::context::FileSystemContextProvider::new(
                config,
            )));
        self
    }

    /// Add a custom context provider
    pub fn with_context_provider(
        mut self,
        provider: Arc<dyn crate::context::ContextProvider>,
    ) -> Self {
        self.context_providers.push(provider);
        self
    }

    /// Set a confirmation manager for HITL
    pub fn with_confirmation_manager(
        mut self,
        manager: Arc<dyn crate::hitl::ConfirmationProvider>,
    ) -> Self {
        self.confirmation_manager = Some(manager);
        self
    }

    /// Set a permission checker
    pub fn with_permission_checker(
        mut self,
        checker: Arc<dyn crate::permissions::PermissionChecker>,
    ) -> Self {
        self.permission_checker = Some(checker);
        self
    }

    /// Set planning mode
    pub fn with_planning_mode(mut self, mode: PlanningMode) -> Self {
        self.planning_mode = mode;
        self
    }

    /// Enable planning (shortcut for `with_planning_mode(PlanningMode::Enabled)`)
    pub fn with_planning(mut self, enabled: bool) -> Self {
        self.planning_mode = if enabled {
            PlanningMode::Enabled
        } else {
            PlanningMode::Disabled
        };
        self
    }

    /// Enable goal tracking
    pub fn with_goal_tracking(mut self, enabled: bool) -> Self {
        self.goal_tracking = enabled;
        self
    }

    /// Add a skill registry with built-in skills
    pub fn with_builtin_skills(mut self) -> Self {
        self.skill_registry = Some(Arc::new(crate::skills::SkillRegistry::with_builtins()));
        self
    }

    /// Add a custom skill registry
    pub fn with_skill_registry(mut self, registry: Arc<crate::skills::SkillRegistry>) -> Self {
        self.skill_registry = Some(registry);
        self
    }

    /// Add skill directories to scan for skill files (*.md).
    /// Merged with any global `skill_dirs` from [`CodeConfig`] at session build time.
    pub fn with_skill_dirs(mut self, dirs: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        self.skill_dirs.extend(dirs.into_iter().map(Into::into));
        self
    }

    /// Load skills from a directory (eager — scans immediately into a registry).
    pub fn with_skills_from_dir(mut self, dir: impl AsRef<std::path::Path>) -> Self {
        let registry = self
            .skill_registry
            .unwrap_or_else(|| Arc::new(crate::skills::SkillRegistry::new()));
        if let Err(e) = registry.load_from_dir(&dir) {
            tracing::warn!(
                dir = %dir.as_ref().display(),
                error = %e,
                "Failed to load skills from directory — continuing without them"
            );
        }
        self.skill_registry = Some(registry);
        self
    }

    /// Set a custom memory store
    pub fn with_memory(mut self, store: Arc<dyn MemoryStore>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Use a file-based memory store at the given directory.
    ///
    /// The store is created lazily when the session is built (requires async).
    /// This stores the directory path; `FileMemoryStore::new()` is called during
    /// session construction.
    pub fn with_file_memory(mut self, dir: impl Into<PathBuf>) -> Self {
        self.file_memory_dir = Some(dir.into());
        self
    }

    /// Set a session store for persistence
    pub fn with_session_store(mut self, store: Arc<dyn crate::store::SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    /// Use a file-based session store at the given directory
    pub fn with_file_session_store(mut self, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                match tokio::task::block_in_place(|| {
                    handle.block_on(crate::store::FileSessionStore::new(dir))
                }) {
                    Ok(store) => {
                        self.session_store =
                            Some(Arc::new(store) as Arc<dyn crate::store::SessionStore>);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to create file session store: {}", e);
                    }
                }
            }
            Err(_) => {
                tracing::warn!(
                    "No async runtime available for file session store — persistence disabled"
                );
            }
        }
        self
    }

    /// Set an explicit session ID (auto-generated UUID if not set)
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Enable auto-save after each `send()` call
    pub fn with_auto_save(mut self, enabled: bool) -> Self {
        self.auto_save = enabled;
        self
    }

    /// Set artifact retention limits for this session.
    pub fn with_artifact_store_limits(mut self, limits: crate::tools::ArtifactStoreLimits) -> Self {
        self.artifact_store_limits = Some(limits);
        self
    }

    /// Set the maximum number of consecutive malformed-tool-args errors before
    /// the agent loop bails.
    ///
    /// Default: 2 (the LLM gets two chances to self-correct before the session
    /// is aborted).
    pub fn with_parse_retries(mut self, max: u32) -> Self {
        self.max_parse_retries = Some(max);
        self
    }

    /// Set a per-tool execution timeout.
    ///
    /// When set, each tool execution is wrapped in `tokio::time::timeout`.
    /// A timeout produces an error message that is fed back to the LLM
    /// (the session continues).
    pub fn with_tool_timeout(mut self, timeout_ms: u64) -> Self {
        self.tool_timeout_ms = Some(timeout_ms);
        self
    }

    /// Set the circuit-breaker threshold.
    ///
    /// In non-streaming mode, the agent retries transient LLM API failures up
    /// to this many times (with exponential backoff) before aborting.
    /// Default: 3 attempts.
    pub fn with_circuit_breaker(mut self, threshold: u32) -> Self {
        self.circuit_breaker_threshold = Some(threshold);
        self
    }

    /// Enable all resilience defaults with sensible values:
    ///
    /// - `max_parse_retries = 2`
    /// - `tool_timeout_ms = 120_000` (2 minutes)
    /// - `circuit_breaker_threshold = 3`
    pub fn with_resilience_defaults(self) -> Self {
        self.with_parse_retries(2)
            .with_tool_timeout(120_000)
            .with_circuit_breaker(3)
    }

    /// Provide a concrete [`BashSandbox`] implementation for this session.
    ///
    /// When set, `bash` tool commands are routed through the given sandbox
    /// instead of `std::process::Command`. The host application is responsible
    /// for constructing and lifecycle-managing the sandbox.
    ///
    /// [`BashSandbox`]: crate::sandbox::BashSandbox
    pub fn with_sandbox_handle(mut self, handle: Arc<dyn crate::sandbox::BashSandbox>) -> Self {
        self.sandbox_handle = Some(handle);
        self
    }

    /// Enable auto-compaction when context usage exceeds threshold.
    ///
    /// When enabled, the agent loop automatically prunes large tool outputs
    /// and summarizes old messages when context usage exceeds the threshold.
    pub fn with_auto_compact(mut self, enabled: bool) -> Self {
        self.auto_compact = enabled;
        self
    }

    /// Set the auto-compact threshold (0.0 - 1.0). Default: 0.80 (80%).
    pub fn with_auto_compact_threshold(mut self, threshold: f32) -> Self {
        self.auto_compact_threshold = Some(threshold.clamp(0.0, 1.0));
        self
    }

    /// Enable or disable continuation injection (default: enabled).
    ///
    /// When enabled, the loop injects a continuation message when the LLM stops
    /// calling tools before the task appears complete, nudging it to keep working.
    pub fn with_continuation(mut self, enabled: bool) -> Self {
        self.continuation_enabled = Some(enabled);
        self
    }

    /// Set the maximum number of continuation injections per execution (default: 3).
    pub fn with_max_continuation_turns(mut self, turns: u32) -> Self {
        self.max_continuation_turns = Some(turns);
        self
    }

    /// Set an MCP manager to connect to external MCP servers.
    ///
    /// All tools from connected servers will be available during execution
    /// with names like `mcp__<server>__<tool>`.
    pub fn with_mcp(mut self, manager: Arc<crate::mcp::manager::McpManager>) -> Self {
        self.mcp_manager = Some(manager);
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_thinking_budget(mut self, budget: usize) -> Self {
        self.thinking_budget = Some(budget);
        self
    }

    /// Override the maximum number of tool execution rounds for this session.
    ///
    /// Useful when binding a markdown-defined subagent to a session —
    /// pass the agent definition's `max_steps` value here to enforce its step budget.
    pub fn with_max_tool_rounds(mut self, rounds: usize) -> Self {
        self.max_tool_rounds = Some(rounds);
        self
    }

    /// Set slot-based system prompt customization for this session.
    ///
    /// Allows customizing role, guidelines, response style, and extra instructions
    /// without overriding the core agentic capabilities.
    pub fn with_prompt_slots(mut self, slots: SystemPromptSlots) -> Self {
        self.prompt_slots = Some(slots);
        self
    }

    /// Replace the built-in hook engine with an external hook executor.
    ///
    /// Use this to attach an AHP harness server (or any custom `HookExecutor`)
    /// to the session. All lifecycle events will be forwarded to the executor
    /// instead of the in-process `HookEngine`.
    pub fn with_hook_executor(mut self, executor: Arc<dyn crate::hooks::HookExecutor>) -> Self {
        self.hook_executor = Some(executor);
        self
    }
}

// ============================================================================
// Agent
// ============================================================================

/// High-level agent facade.
///
/// Holds the LLM client and agent config. Workspace-independent.
/// Use [`Agent::session()`] to bind to a workspace.
pub struct Agent {
    code_config: CodeConfig,
    config: AgentConfig,
    /// Global MCP manager loaded from config.mcp_servers
    global_mcp: Option<Arc<crate::mcp::manager::McpManager>>,
    /// Pre-fetched MCP tool definitions from global_mcp (cached at creation time).
    /// Wrapped in Mutex so `refresh_mcp_tools()` can update the cache without `&mut self`.
    global_mcp_tools: std::sync::Mutex<Vec<(String, crate::mcp::McpTool)>>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent").finish()
    }
}

impl Agent {
    /// Create from a config file path or inline ACL-compatible string.
    ///
    /// Auto-detects `.acl` file paths vs inline ACL-compatible config.
    pub async fn new(config_source: impl Into<String>) -> Result<Self> {
        let source = config_source.into();

        // Expand leading `~/` to the user's home directory (cross-platform)
        let expanded = if let Some(rest) = source.strip_prefix("~/") {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
            if let Some(home) = home {
                PathBuf::from(home).join(rest).display().to_string()
            } else {
                source.clone()
            }
        } else {
            source.clone()
        };

        let path = Path::new(&expanded);

        let ext = path.extension().and_then(|ext| ext.to_str());

        let config = if matches!(ext, Some("acl")) {
            if !path.exists() {
                return Err(CodeError::Config(format!(
                    "Config file not found: {}",
                    path.display()
                )));
            }

            CodeConfig::from_file(path)
                .with_context(|| format!("Failed to load config: {}", path.display()))?
        } else if matches!(ext, Some("hcl")) {
            return Err(CodeError::Config(
                "HCL config files are not supported in 2.0; rename the file to .acl".into(),
            ));
        } else if source.trim().starts_with('{') {
            return Err(CodeError::Config(
                "JSON config is not supported; use ACL-compatible .acl config".into(),
            ));
        } else if matches!(ext, Some("json")) {
            return Err(CodeError::Config(
                "JSON config files are not supported; use .acl".into(),
            ));
        } else {
            CodeConfig::from_acl(&source).context("Failed to parse config as ACL string")?
        };

        Self::from_config(config).await
    }

    /// Create from a config file path or inline ACL-compatible string.
    ///
    /// Alias for [`Agent::new()`] — provides a consistent API with
    /// the Python and Node.js SDKs.
    pub async fn create(config_source: impl Into<String>) -> Result<Self> {
        Self::new(config_source).await
    }

    /// Create from a [`CodeConfig`] struct.
    pub async fn from_config(config: CodeConfig) -> Result<Self> {
        config
            .default_llm_config()
            .context("default_model must be set in 'provider/model' format with a valid API key")?;

        let agent_config = AgentConfig {
            max_tool_rounds: config
                .max_tool_rounds
                .unwrap_or(AgentConfig::default().max_tool_rounds),
            ..AgentConfig::default()
        };

        // Load global MCP servers from config
        let (global_mcp, global_mcp_tools) = if config.mcp_servers.is_empty() {
            (None, vec![])
        } else {
            let manager = Arc::new(crate::mcp::manager::McpManager::new());
            for server in &config.mcp_servers {
                if !server.enabled {
                    continue;
                }
                manager.register_server(server.clone()).await;
                if let Err(e) = manager.connect(&server.name).await {
                    tracing::warn!(
                        server = %server.name,
                        error = %e,
                        "Failed to connect to MCP server — skipping"
                    );
                }
            }
            // Pre-fetch tool definitions while we're in async context
            let tools = manager.get_all_tools().await;
            (Some(manager), tools)
        };

        let mut agent = Agent {
            code_config: config,
            config: agent_config,
            global_mcp,
            global_mcp_tools: std::sync::Mutex::new(global_mcp_tools),
        };

        // Always initialize the skill registry with built-in skills, then load any user-defined dirs
        let registry = Arc::new(crate::skills::SkillRegistry::with_builtins());
        for dir in &agent.code_config.skill_dirs.clone() {
            if let Err(e) = registry.load_from_dir(dir) {
                tracing::warn!(
                    dir = %dir.display(),
                    error = %e,
                    "Failed to load skills from directory — skipping"
                );
            }
        }
        agent.config.skill_registry = Some(registry);

        Ok(agent)
    }

    /// Re-fetch tool definitions from all connected global MCP servers and
    /// update the internal cache.
    ///
    /// Call this when an MCP server has added or removed tools since the
    /// agent was created. The refreshed tools will be visible to all
    /// **new** sessions created after this call; existing sessions are
    /// unaffected (their `ToolExecutor` snapshot is already built).
    pub async fn refresh_mcp_tools(&self) -> Result<()> {
        if let Some(ref mcp) = self.global_mcp {
            let fresh = mcp.get_all_tools().await;
            *self
                .global_mcp_tools
                .lock()
                .expect("global_mcp_tools lock poisoned") = fresh;
        }
        Ok(())
    }

    /// Bind to a workspace directory, returning an [`AgentSession`].
    ///
    /// Pass `None` for defaults, or `Some(SessionOptions)` to override
    /// the model, agent directories for this session.
    pub fn session(
        &self,
        workspace: impl Into<String>,
        options: Option<SessionOptions>,
    ) -> Result<AgentSession> {
        let opts = options.unwrap_or_default();

        // Merge global MCP manager with any session-level one from opts.
        // If both exist, session-level servers are added into the global manager.
        let mut merged_opts = match (&self.global_mcp, &opts.mcp_manager) {
            (Some(global), Some(session)) => {
                let global = Arc::clone(global);
                let session_mgr = Arc::clone(session);
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => {
                        let global_for_merge = Arc::clone(&global);
                        tokio::task::block_in_place(|| {
                            handle.block_on(async move {
                                for config in session_mgr.all_configs().await {
                                    let name = config.name.clone();
                                    global_for_merge.register_server(config).await;
                                    if let Err(e) = global_for_merge.connect(&name).await {
                                        tracing::warn!(
                                            server = %name,
                                            error = %e,
                                            "Failed to connect session-level MCP server — skipping"
                                        );
                                    }
                                }
                            })
                        });
                    }
                    Err(_) => {
                        tracing::warn!(
                            "No async runtime available to merge session-level MCP servers \
                             into global manager — session MCP servers will not be available"
                        );
                    }
                }
                SessionOptions {
                    mcp_manager: Some(Arc::clone(&global)),
                    ..opts
                }
            }
            (Some(global), None) => SessionOptions {
                mcp_manager: Some(Arc::clone(global)),
                ..opts
            },
            _ => opts,
        };

        let session_id = merged_opts
            .session_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        merged_opts.session_id = Some(session_id.clone());
        let llm_client = self.resolve_session_llm_client(&merged_opts, Some(&session_id))?;

        self.build_session(workspace.into(), llm_client, &merged_opts)
    }

    /// Create a session pre-configured from an [`AgentDefinition`].
    ///
    /// Maps the definition's `permissions`, `prompt`, `model`, and `max_steps`
    /// directly into [`SessionOptions`], so markdown/YAML-defined subagents can
    /// be used by delegation and advanced control-plane flows without manual wiring.
    ///
    /// The mapping follows the same logic as the built-in `task` tool:
    /// - `permissions` → `permission_checker`
    /// - `prompt`      → `prompt_slots.extra`
    /// - `max_steps`   → `max_tool_rounds`
    /// - `model`       → `model` (as `"provider/model"` string)
    ///
    /// `extra` can supply additional overrides (e.g. `planning_enabled`) that
    /// take precedence over the definition's values.
    pub fn session_for_agent(
        &self,
        workspace: impl Into<String>,
        def: &crate::subagent::AgentDefinition,
        extra: Option<SessionOptions>,
    ) -> Result<AgentSession> {
        let mut opts = extra.unwrap_or_default();

        // Apply permission policy unless the caller supplied a custom one.
        if opts.permission_checker.is_none()
            && (!def.permissions.allow.is_empty() || !def.permissions.deny.is_empty())
        {
            opts.permission_checker = Some(Arc::new(def.permissions.clone()));
        }

        // Apply max_steps unless the caller already set max_tool_rounds.
        if opts.max_tool_rounds.is_none() {
            if let Some(steps) = def.max_steps {
                opts.max_tool_rounds = Some(steps);
            }
        }

        // Apply model override unless the caller already chose a model.
        if opts.model.is_none() {
            if let Some(ref m) = def.model {
                let provider = m.provider.as_deref().unwrap_or("anthropic");
                opts.model = Some(format!("{}/{}", provider, m.model));
            }
        }

        // Inject agent system prompt into the extra slot.
        //
        // Merge slot-by-slot rather than all-or-nothing: if the caller already
        // set some slots (e.g. `role`), only fill in `extra` from the definition
        // if the caller left it unset. This lets per-member overrides coexist
        // with per-role prompts defined in the agent file.
        if let Some(ref prompt) = def.prompt {
            let slots = opts
                .prompt_slots
                .get_or_insert_with(crate::prompts::SystemPromptSlots::default);
            if slots.extra.is_none() {
                slots.extra = Some(prompt.clone());
            }
        }

        self.session(workspace, Some(opts))
    }

    /// Resume a previously saved session by ID.
    ///
    /// Loads the session data from the store, rebuilds the `AgentSession` with
    /// the saved conversation history, and returns it ready for continued use.
    ///
    /// The `options` must include a `session_store` (or `with_file_session_store`)
    /// that contains the saved session.
    pub fn resume_session(
        &self,
        session_id: &str,
        options: SessionOptions,
    ) -> Result<AgentSession> {
        let store = options.session_store.clone().ok_or_else(|| {
            crate::error::CodeError::Session(
                "resume_session requires a session_store in SessionOptions".to_string(),
            )
        })?;

        // Load session data from store
        let data = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(store.load(session_id)))
                .map_err(|e| {
                crate::error::CodeError::Session(format!(
                    "Failed to load session {}: {}",
                    session_id, e
                ))
            })?,
            Err(_) => {
                return Err(crate::error::CodeError::Session(
                    "No async runtime available for session resume".to_string(),
                ))
            }
        };

        let data = data.ok_or_else(|| {
            crate::error::CodeError::Session(format!("Session not found: {}", session_id))
        })?;

        // Build session with the saved workspace
        let mut opts = options;
        opts.session_id = Some(data.id.clone());
        let llm_client = self.resolve_session_llm_client(&opts, Some(&data.id))?;

        let session = self.build_session(data.config.workspace.clone(), llm_client, &opts)?;

        // Restore conversation history
        *write_or_recover(&session.history) = data.messages;
        let artifacts = match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(store.load_artifacts(&data.id)))
                    .map_err(|e| {
                        crate::error::CodeError::Session(format!(
                            "Failed to load artifacts for session {}: {}",
                            data.id, e
                        ))
                    })?
            }
            Err(_) => None,
        };
        if let Some(artifacts) = artifacts {
            let target_store = session.tool_executor.artifact_store();
            for artifact in artifacts.artifacts() {
                target_store.put(artifact);
            }
        }

        let trace_events = match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(store.load_trace_events(&data.id)))
                    .map_err(|e| {
                    crate::error::CodeError::Session(format!(
                        "Failed to load trace events for session {}: {}",
                        data.id, e
                    ))
                })?
            }
            Err(_) => None,
        };
        if let Some(events) = trace_events {
            session.trace_sink.replace_events(events);
        }

        let run_records = match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(store.load_run_records(&data.id)))
                    .map_err(|e| {
                        crate::error::CodeError::Session(format!(
                            "Failed to load run records for session {}: {}",
                            data.id, e
                        ))
                    })?
            }
            Err(_) => None,
        };
        if let Some(records) = run_records {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                tokio::task::block_in_place(|| {
                    handle.block_on(session.run_store.replace_records(records))
                });
            }
        }

        let verification_reports = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(store.load_verification_reports(&data.id))
            })
            .map_err(|e| {
                crate::error::CodeError::Session(format!(
                    "Failed to load verification reports for session {}: {}",
                    data.id, e
                ))
            })?,
            Err(_) => None,
        };
        if let Some(reports) = verification_reports {
            *write_or_recover(&session.verification_reports) = reports;
        }

        Ok(session)
    }

    fn resolve_session_llm_client(
        &self,
        opts: &SessionOptions,
        session_id: Option<&str>,
    ) -> Result<Arc<dyn LlmClient>> {
        let model_ref = if let Some(ref model) = opts.model {
            model.as_str()
        } else {
            if opts.temperature.is_some() || opts.thinking_budget.is_some() {
                tracing::warn!(
                    "temperature/thinking_budget set without model override — these will be ignored. \
                     Use with_model() to apply LLM parameter overrides."
                );
            }
            self.code_config
                .default_model
                .as_deref()
                .context("default_model must be set in 'provider/model' format")?
        };

        let (provider_name, model_id) = model_ref
            .split_once('/')
            .context("model format must be 'provider/model' (e.g., 'openai/gpt-4o')")?;

        let mut llm_config = self
            .code_config
            .llm_config(provider_name, model_id)
            .with_context(|| {
                format!("provider '{provider_name}' or model '{model_id}' not found in config")
            })?;

        if opts.model.is_some() {
            if let Some(temp) = opts.temperature {
                llm_config = llm_config.with_temperature(temp);
            }
            if let Some(budget) = opts.thinking_budget {
                llm_config = llm_config.with_thinking_budget(budget);
            }
        }

        if let Some(session_id) = session_id {
            llm_config = llm_config.with_session_id(session_id);
        }

        Ok(crate::llm::create_client_with_config(llm_config))
    }

    fn build_session(
        &self,
        workspace: String,
        llm_client: Arc<dyn LlmClient>,
        opts: &SessionOptions,
    ) -> Result<AgentSession> {
        let canonical = safe_canonicalize(Path::new(&workspace));

        let artifact_limits = opts.artifact_store_limits.unwrap_or_default();
        let tool_executor = Arc::new(ToolExecutor::new_with_artifact_limits(
            canonical.display().to_string(),
            artifact_limits,
        ));
        let trace_sink = crate::trace::InMemoryTraceSink::default();
        tool_executor.set_trace_sink(Arc::new(trace_sink.clone()));

        // Seed the registry's default context so direct registry execution also sees config.
        if let Some(ref search_config) = self.code_config.search {
            tool_executor
                .registry()
                .set_search_config(search_config.clone());
        }

        // Register the single model-visible delegation surface: task/parallel_task.
        // These require an LLM client to spawn isolated child agent loops.
        // When MCP manager is available, pass it through so child sessions inherit MCP tools.
        let agent_registry = {
            use crate::subagent::{load_agents_from_dir, AgentRegistry};
            use crate::tools::register_task_with_mcp;
            let registry = AgentRegistry::new();
            for dir in self
                .code_config
                .agent_dirs
                .iter()
                .chain(opts.agent_dirs.iter())
            {
                for agent in load_agents_from_dir(dir) {
                    registry.register(agent);
                }
            }
            let registry = Arc::new(registry);
            register_task_with_mcp(
                tool_executor.registry(),
                Arc::clone(&llm_client),
                Arc::clone(&registry),
                canonical.display().to_string(),
                opts.mcp_manager.clone(),
            );
            registry
        };

        // Register MCP tools before taking tool definitions snapshot.
        // Use pre-cached tools from Agent creation (avoids async in sync SDK context).
        if let Some(ref mcp) = opts.mcp_manager {
            // Prefer cached tools from Agent::from_config(); fall back to runtime fetch
            // only when a session-level MCP manager is provided (not the global one).
            let all_tools: Vec<(String, crate::mcp::McpTool)> = if std::ptr::eq(
                Arc::as_ptr(mcp),
                self.global_mcp
                    .as_ref()
                    .map(Arc::as_ptr)
                    .unwrap_or(std::ptr::null()),
            ) {
                // Same manager as global — use cached tools
                self.global_mcp_tools
                    .lock()
                    .expect("global_mcp_tools lock poisoned")
                    .clone()
            } else {
                // Session-level or merged manager — fetch at runtime
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => {
                        tokio::task::block_in_place(|| handle.block_on(mcp.get_all_tools()))
                    }
                    Err(_) => {
                        tracing::warn!(
                            "No async runtime available for session-level MCP tools — \
                                 MCP tools will not be registered"
                        );
                        vec![]
                    }
                }
            };

            let mut by_server: std::collections::HashMap<String, Vec<crate::mcp::McpTool>> =
                std::collections::HashMap::new();
            for (server, tool) in all_tools {
                by_server.entry(server).or_default().push(tool);
            }
            for (server_name, tools) in by_server {
                for tool in
                    crate::mcp::tools::create_mcp_tools(&server_name, tools, Arc::clone(mcp))
                {
                    tool_executor.register_dynamic_tool(tool);
                }
            }
        }

        let tool_defs = tool_executor.definitions();

        // Build prompt slots: start from session options or agent-level config
        let prompt_slots = opts
            .prompt_slots
            .clone()
            .unwrap_or_else(|| self.config.prompt_slots.clone());

        let mut context_providers = opts.context_providers.clone();

        // Auto-load AGENTS.md from workspace root as structured session context.
        let agents_md_path = canonical.join("AGENTS.md");
        if agents_md_path.exists() && agents_md_path.is_file() {
            match std::fs::read_to_string(&agents_md_path) {
                Ok(content) if !content.trim().is_empty() => {
                    tracing::info!(
                        path = %agents_md_path.display(),
                        "Auto-loaded AGENTS.md from workspace root"
                    );
                    let token_count = content.split_whitespace().count().max(1);
                    let item = ContextItem::new(
                        "agents_md",
                        ContextType::Resource,
                        format!("# Project Instructions (AGENTS.md)\n\n{}", content),
                    )
                    .with_source(format!("file://{}", agents_md_path.display()))
                    .with_provenance("workspace_instructions")
                    .with_priority(0.95)
                    .with_trust(0.95)
                    .with_freshness(1.0)
                    .with_relevance(0.95)
                    .with_token_count(token_count);

                    context_providers.push(Arc::new(
                        StaticContextProvider::new("agents_md").with_item(item),
                    ));
                }
                Ok(_) => {
                    tracing::debug!(
                        path = %agents_md_path.display(),
                        "AGENTS.md exists but is empty — skipping"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        path = %agents_md_path.display(),
                        error = %e,
                        "Failed to read AGENTS.md — skipping"
                    );
                }
            }
        }

        // Build effective skill registry: fork the agent-level registry (builtins + global
        // skill_dirs), then layer session-level skills on top. Forking ensures session skills
        // never pollute the shared agent-level registry.
        let base_registry = self
            .config
            .skill_registry
            .as_deref()
            .map(|r| r.fork())
            .unwrap_or_else(crate::skills::SkillRegistry::with_builtins);
        // Merge explicit session registry on top of the fork
        if let Some(ref r) = opts.skill_registry {
            for skill in r.all() {
                base_registry.register_unchecked(skill);
            }
        }
        // Load session-level skill dirs
        for dir in &opts.skill_dirs {
            if let Err(e) = base_registry.load_from_dir(dir) {
                tracing::warn!(
                    dir = %dir.display(),
                    error = %e,
                    "Failed to load session skill dir — skipping"
                );
            }
        }
        let effective_registry = Arc::new(base_registry);

        // Route skill discovery guidance through the structured context pipeline.
        let skill_prompt = effective_registry.to_system_prompt();
        if !skill_prompt.is_empty() {
            let item = ContextItem::new("skills_catalog", ContextType::Skill, skill_prompt)
                .with_source("a3s://skills/catalog")
                .with_provenance("skill_registry")
                .with_priority(0.85)
                .with_trust(0.9)
                .with_freshness(1.0)
                .with_relevance(1.0);
            context_providers.push(Arc::new(
                StaticContextProvider::new("skills_catalog").with_item(item),
            ));
        }

        // Resolve memory store: explicit store takes priority, then file_memory_dir
        let mut init_warning: Option<String> = None;
        let memory = {
            let store = if let Some(ref store) = opts.memory_store {
                Some(Arc::clone(store))
            } else if let Some(ref dir) = opts.file_memory_dir {
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => {
                        let dir = dir.clone();
                        match tokio::task::block_in_place(|| {
                            handle.block_on(FileMemoryStore::new(dir))
                        }) {
                            Ok(store) => Some(Arc::new(store) as Arc<dyn MemoryStore>),
                            Err(e) => {
                                let msg = format!("Failed to create file memory store: {}", e);
                                tracing::warn!("{}", msg);
                                init_warning = Some(msg);
                                None
                            }
                        }
                    }
                    Err(_) => {
                        let msg =
                            "No async runtime available for file memory store — memory disabled"
                                .to_string();
                        tracing::warn!("{}", msg);
                        init_warning = Some(msg);
                        None
                    }
                }
            } else {
                None
            };
            store.map(|s| Arc::new(crate::memory::AgentMemory::new(s)))
        };

        let base = self.config.clone();
        let config = AgentConfig {
            prompt_slots,
            tools: tool_defs,
            security_provider: opts.security_provider.clone(),
            permission_checker: opts.permission_checker.clone(),
            confirmation_manager: opts.confirmation_manager.clone(),
            context_providers,
            planning_mode: opts.planning_mode,
            goal_tracking: opts.goal_tracking,
            skill_registry: Some(Arc::clone(&effective_registry)),
            max_parse_retries: opts.max_parse_retries.unwrap_or(base.max_parse_retries),
            tool_timeout_ms: opts.tool_timeout_ms.or(base.tool_timeout_ms),
            circuit_breaker_threshold: opts
                .circuit_breaker_threshold
                .unwrap_or(base.circuit_breaker_threshold),
            auto_compact: opts.auto_compact,
            auto_compact_threshold: opts
                .auto_compact_threshold
                .unwrap_or(crate::store::DEFAULT_AUTO_COMPACT_THRESHOLD),
            max_context_tokens: base.max_context_tokens,
            memory: memory.clone(),
            continuation_enabled: opts
                .continuation_enabled
                .unwrap_or(base.continuation_enabled),
            max_continuation_turns: opts
                .max_continuation_turns
                .unwrap_or(base.max_continuation_turns),
            max_tool_rounds: opts.max_tool_rounds.unwrap_or(base.max_tool_rounds),
            ..base
        };

        // Register Skill tool — enables skills to be invoked as first-class tools
        // with temporary permission grants. Must be registered after effective_registry
        // and config are built so the Skill tool has access to both.
        {
            use crate::tools::register_skill;
            register_skill(
                tool_executor.registry(),
                Arc::clone(&llm_client),
                Arc::clone(&effective_registry),
                Arc::clone(&tool_executor),
                config.clone(),
            );
        }

        // Create lane queue if configured
        // A shared broadcast channel is used for both queue events and subagent events.
        let (agent_event_tx, _) = broadcast::channel::<crate::agent::AgentEvent>(256);
        let command_queue = if let Some(ref queue_config) = opts.queue_config {
            let session_id = uuid::Uuid::new_v4().to_string();
            let rt = tokio::runtime::Handle::try_current();

            match rt {
                Ok(handle) => {
                    // We're inside an async runtime — use block_in_place
                    let queue = tokio::task::block_in_place(|| {
                        handle.block_on(SessionLaneQueue::new(
                            &session_id,
                            queue_config.clone(),
                            agent_event_tx.clone(),
                        ))
                    });
                    match queue {
                        Ok(q) => {
                            // Start the queue
                            let q = Arc::new(q);
                            let q2 = Arc::clone(&q);
                            tokio::task::block_in_place(|| {
                                handle.block_on(async { q2.start().await.ok() })
                            });
                            Some(q)
                        }
                        Err(e) => {
                            tracing::warn!("Failed to create session lane queue: {}", e);
                            None
                        }
                    }
                }
                Err(_) => {
                    tracing::warn!(
                        "No async runtime available for queue creation — queue disabled"
                    );
                    None
                }
            }
        } else {
            None
        };

        // Create tool context with search config if available
        let mut tool_context = ToolContext::new(canonical.clone());
        if let Some(ref search_config) = self.code_config.search {
            tool_context = tool_context.with_search_config(search_config.clone());
        }
        tool_context = tool_context.with_agent_event_tx(agent_event_tx);

        // Wire sandbox when a concrete handle is provided by the host application.
        if let Some(handle) = opts.sandbox_handle.clone() {
            tool_executor.registry().set_sandbox(Arc::clone(&handle));
            tool_context = tool_context.with_sandbox(handle);
        }

        let session_id = opts
            .session_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // Resolve session store: explicit opts store > config sessions_dir > None
        let session_store = if opts.session_store.is_some() {
            opts.session_store.clone()
        } else if let Some(ref dir) = self.code_config.sessions_dir {
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    let dir = dir.clone();
                    match tokio::task::block_in_place(|| {
                        handle.block_on(crate::store::FileSessionStore::new(dir))
                    }) {
                        Ok(store) => Some(Arc::new(store) as Arc<dyn crate::store::SessionStore>),
                        Err(e) => {
                            tracing::warn!(
                                "Failed to create session store from sessions_dir: {}",
                                e
                            );
                            None
                        }
                    }
                }
                Err(_) => {
                    tracing::warn!(
                        "No async runtime for sessions_dir store — persistence disabled"
                    );
                    None
                }
            }
        } else {
            None
        };

        let command_registry = CommandRegistry::new();

        Ok(AgentSession {
            llm_client,
            tool_executor,
            tool_context,
            memory: config.memory.clone(),
            config,
            workspace: canonical,
            session_id,
            history: Arc::new(RwLock::new(Vec::new())),
            command_queue,
            session_store,
            auto_save: opts.auto_save,
            hook_engine: Arc::new(crate::hooks::HookEngine::new()),
            ahp_executor: opts.hook_executor.clone(),
            init_warning,
            command_registry: std::sync::Mutex::new(command_registry),
            model_name: opts
                .model
                .clone()
                .or_else(|| self.code_config.default_model.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            mcp_manager: opts
                .mcp_manager
                .clone()
                .or_else(|| self.global_mcp.clone())
                .unwrap_or_else(|| Arc::new(crate::mcp::manager::McpManager::new())),
            agent_registry,
            cancel_token: Arc::new(tokio::sync::Mutex::new(None)),
            current_run_id: Arc::new(tokio::sync::Mutex::new(None)),
            run_store: Arc::new(crate::run::InMemoryRunStore::new()),
            active_tools: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            trace_sink,
            verification_reports: Arc::new(RwLock::new(Vec::new())),
        })
    }
}

// ============================================================================
// BtwResult
// ============================================================================

/// Result of a `/btw` ephemeral side question.
///
/// The answer is never added to conversation history.
/// Returned by [`AgentSession::btw()`].
#[derive(Debug, Clone)]
pub struct BtwResult {
    /// The original question.
    pub question: String,
    /// The LLM's answer.
    pub answer: String,
    /// Token usage for this ephemeral call.
    pub usage: crate::llm::TokenUsage,
}

// ============================================================================
// AgentSession
// ============================================================================

/// Workspace-bound session. All LLM and tool operations happen here.
///
/// History is automatically accumulated after each `send()` call and after
/// `stream()` completes when no custom history is supplied.
/// Use `history()` to retrieve the current conversation log.
pub struct AgentSession {
    llm_client: Arc<dyn LlmClient>,
    tool_executor: Arc<ToolExecutor>,
    tool_context: ToolContext,
    config: AgentConfig,
    workspace: PathBuf,
    /// Unique session identifier.
    session_id: String,
    /// Internal conversation history, auto-updated after each `send()` and default-history `stream()`.
    history: Arc<RwLock<Vec<Message>>>,
    /// Optional lane queue for priority-based tool execution.
    command_queue: Option<Arc<SessionLaneQueue>>,
    /// Optional long-term memory.
    memory: Option<Arc<crate::memory::AgentMemory>>,
    /// Optional session store for persistence.
    session_store: Option<Arc<dyn crate::store::SessionStore>>,
    /// Auto-save after each completed `send()` or default-history `stream()`.
    auto_save: bool,
    /// Hook engine for lifecycle event interception.
    hook_engine: Arc<crate::hooks::HookEngine>,
    /// Optional external hook executor (e.g. AHP harness). When set, replaces
    /// `hook_engine` as the executor passed to each `AgentLoop`.
    ahp_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
    /// Deferred init warning: emitted as PersistenceFailed on first send() if set.
    init_warning: Option<String>,
    /// Slash command registry for `/command` dispatch.
    /// Uses interior mutability so commands can be registered on a shared `Arc<AgentSession>`.
    command_registry: std::sync::Mutex<CommandRegistry>,
    /// Model identifier for display (e.g., "anthropic/claude-sonnet-4-20250514").
    model_name: String,
    /// Shared MCP manager — all add_mcp_server / remove_mcp_server calls go here.
    mcp_manager: Arc<crate::mcp::manager::McpManager>,
    /// Shared agent registry — populated at session creation; extended via register_agent_dir().
    agent_registry: Arc<crate::subagent::AgentRegistry>,
    /// Cancellation token for the current operation (send/stream).
    /// Stored so that cancel() can abort ongoing LLM calls.
    cancel_token: Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    /// ID of the run currently attached to the active cancellation token.
    current_run_id: Arc<tokio::sync::Mutex<Option<String>>>,
    /// In-memory run snapshots and event replay buffer for this session.
    run_store: Arc<crate::run::InMemoryRunStore>,
    /// Currently executing tools observed from runtime events.
    active_tools: Arc<tokio::sync::RwLock<HashMap<String, ActiveToolSnapshot>>>,
    /// Compact execution traces for this session.
    trace_sink: crate::trace::InMemoryTraceSink,
    /// Structured completion evidence collected from agent and explicit verification runs.
    verification_reports: Arc<RwLock<Vec<crate::verification::VerificationReport>>>,
}

#[derive(Debug, Clone)]
struct ActiveToolSnapshot {
    tool_name: String,
    started_at_ms: u64,
}

#[derive(Clone)]
struct SessionPersistenceContext {
    session_store: Option<Arc<dyn crate::store::SessionStore>>,
    session_id: String,
    workspace: PathBuf,
    config: AgentConfig,
    tool_executor: Arc<ToolExecutor>,
    trace_sink: crate::trace::InMemoryTraceSink,
    run_store: Arc<crate::run::InMemoryRunStore>,
    history: Arc<RwLock<Vec<Message>>>,
    verification_reports: Arc<RwLock<Vec<crate::verification::VerificationReport>>>,
    auto_save: bool,
}

impl SessionPersistenceContext {
    fn from_session(session: &AgentSession) -> Self {
        Self {
            session_store: session.session_store.clone(),
            session_id: session.session_id.clone(),
            workspace: session.workspace.clone(),
            config: session.config.clone(),
            tool_executor: Arc::clone(&session.tool_executor),
            trace_sink: session.trace_sink.clone(),
            run_store: Arc::clone(&session.run_store),
            history: Arc::clone(&session.history),
            verification_reports: Arc::clone(&session.verification_reports),
            auto_save: session.auto_save,
        }
    }

    fn record_result(&self, result: &AgentResult) {
        *write_or_recover(&self.history) = result.messages.clone();
        if !result.verification_reports.is_empty() {
            write_or_recover(&self.verification_reports)
                .extend(result.verification_reports.clone());
        }
    }

    async fn save(&self) -> Result<()> {
        let store = match &self.session_store {
            Some(store) => store,
            None => return Ok(()),
        };

        let history = read_or_recover(&self.history).clone();
        let verification_reports = read_or_recover(&self.verification_reports).clone();
        let now = chrono::Utc::now().timestamp();

        let data = crate::store::SessionData {
            id: self.session_id.clone(),
            config: crate::store::SessionConfig {
                name: String::new(),
                workspace: self.workspace.display().to_string(),
                system_prompt: Some(self.config.prompt_slots.build()),
                max_context_length: 200_000,
                auto_compact: false,
                auto_compact_threshold: crate::store::DEFAULT_AUTO_COMPACT_THRESHOLD,
                storage_type: crate::config::StorageBackend::File,
                queue_config: None,
                confirmation_policy: None,
                permission_policy: None,
                parent_id: None,
                security_config: None,
                hook_engine: None,
                planning_mode: self.config.planning_mode,
                goal_tracking: self.config.goal_tracking,
            },
            state: crate::store::SessionState::Active,
            messages: history,
            context_usage: crate::store::ContextUsage::default(),
            total_usage: crate::llm::TokenUsage::default(),
            total_cost: 0.0,
            model_name: None,
            cost_records: Vec::new(),
            tool_names: crate::store::SessionData::tool_names_from_definitions(&self.config.tools),
            thinking_enabled: false,
            thinking_budget: None,
            created_at: now,
            updated_at: now,
            llm_config: None,
            tasks: Vec::new(),
            parent_id: None,
        };

        store.save(&data).await?;
        store
            .save_artifacts(&self.session_id, &self.tool_executor.artifact_store())
            .await?;
        store
            .save_trace_events(&self.session_id, &self.trace_sink.events())
            .await?;
        store
            .save_run_records(&self.session_id, &self.run_store.records().await)
            .await?;
        store
            .save_verification_reports(&self.session_id, &verification_reports)
            .await?;
        tracing::debug!("Session {} saved", self.session_id);
        Ok(())
    }

    async fn auto_save_if_enabled(&self) {
        if self.auto_save {
            if let Err(e) = self.save().await {
                tracing::warn!("Auto-save failed for session {}: {}", self.session_id, e);
            }
        }
    }
}

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSession")
            .field("session_id", &self.session_id)
            .field("workspace", &self.workspace.display().to_string())
            .field("auto_save", &self.auto_save)
            .finish()
    }
}

impl AgentSession {
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn compact_json_value(value: &serde_json::Value) -> String {
        let raw = match value {
            serde_json::Value::Null => String::new(),
            serde_json::Value::String(s) => s.clone(),
            _ => serde_json::to_string(value).unwrap_or_default(),
        };
        let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if compact.len() > 180 {
            format!("{}...", truncate_utf8(&compact, 180))
        } else {
            compact
        }
    }

    async fn start_run(&self, prompt: &str) -> crate::run::RunHandle {
        let snapshot = self.run_store.create_run(&self.session_id, prompt).await;
        *self.current_run_id.lock().await = Some(snapshot.id.clone());
        crate::run::RunHandle::new(
            snapshot.id,
            self.session_id.clone(),
            Arc::clone(&self.run_store),
            Arc::clone(&self.cancel_token),
            Arc::clone(&self.current_run_id),
            self.ahp_executor.clone(),
        )
    }

    async fn finish_run_if_current(&self, run_id: &str) {
        let mut current = self.current_run_id.lock().await;
        if current.as_deref() == Some(run_id) {
            *current = None;
        }
    }

    async fn record_runtime_event(
        run_store: &Arc<crate::run::InMemoryRunStore>,
        run_id: &str,
        session_id: &str,
        hook_executor: &Option<Arc<dyn crate::hooks::HookExecutor>>,
        event: &AgentEvent,
    ) {
        let _ = run_store.record_event(run_id, event.clone()).await;
        if let Some(executor) = hook_executor {
            executor.record_agent_event(event, run_id, session_id).await;
        }
    }

    async fn apply_runtime_event(
        active_tools: &Arc<tokio::sync::RwLock<HashMap<String, ActiveToolSnapshot>>>,
        event: &AgentEvent,
    ) {
        match event {
            AgentEvent::ToolStart { id, name } => {
                active_tools.write().await.insert(
                    id.clone(),
                    ActiveToolSnapshot {
                        tool_name: name.clone(),
                        started_at_ms: Self::now_ms(),
                    },
                );
            }
            AgentEvent::ToolEnd { id, .. }
            | AgentEvent::PermissionDenied { tool_id: id, .. }
            | AgentEvent::ConfirmationRequired { tool_id: id, .. }
            | AgentEvent::ConfirmationReceived { tool_id: id, .. }
            | AgentEvent::ConfirmationTimeout { tool_id: id, .. } => {
                active_tools.write().await.remove(id);
            }
            _ => {}
        }
    }

    async fn clear_runtime_tracking(&self) {
        self.active_tools.write().await.clear();
    }

    /// Build an `AgentLoop` with the session's configuration.
    ///
    /// Propagates the lane queue (if configured) for external task handling.
    fn build_agent_loop(&self) -> AgentLoop {
        let mut config = self.config.clone();
        config.hook_engine = Some(if let Some(ref ahp) = self.ahp_executor {
            ahp.clone()
        } else {
            Arc::clone(&self.hook_engine) as Arc<dyn crate::hooks::HookExecutor>
        });
        // Always use live tool definitions so tools added via add_mcp_server() are visible
        // to the LLM. The config.tools snapshot taken at session creation misses dynamically
        // added MCP tools.
        config.tools = self.tool_executor.definitions();
        let mut agent_loop = AgentLoop::new(
            self.llm_client.clone(),
            self.tool_executor.clone(),
            self.tool_context.clone(),
            config,
        );
        if let Some(ref queue) = self.command_queue {
            agent_loop = agent_loop.with_queue(Arc::clone(queue));
        }
        agent_loop
    }

    /// Build a `CommandContext` from the current session state.
    fn build_command_context(&self) -> CommandContext {
        let history = read_or_recover(&self.history);

        // Collect tool names from config
        let tool_names: Vec<String> = self.config.tools.iter().map(|t| t.name.clone()).collect();

        // Derive MCP server info from tool names
        let mut mcp_map: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for name in &tool_names {
            if let Some(rest) = name.strip_prefix("mcp__") {
                if let Some((server, _)) = rest.split_once("__") {
                    *mcp_map.entry(server.to_string()).or_default() += 1;
                }
            }
        }
        let mut mcp_servers: Vec<(String, usize)> = mcp_map.into_iter().collect();
        mcp_servers.sort_by(|a, b| a.0.cmp(&b.0));

        CommandContext {
            session_id: self.session_id.clone(),
            workspace: self.workspace.display().to_string(),
            model: self.model_name.clone(),
            history_len: history.len(),
            total_tokens: 0,
            total_cost: 0.0,
            tool_names,
            mcp_servers,
        }
    }

    /// Get a snapshot of command entries (name, description, optional usage).
    ///
    /// Acquires the command registry lock briefly and returns owned data.
    pub fn command_registry(&self) -> std::sync::MutexGuard<'_, CommandRegistry> {
        self.command_registry
            .lock()
            .expect("command_registry lock poisoned")
    }

    /// Register a custom slash command.
    ///
    /// Takes `&self` so it can be called on a shared `Arc<AgentSession>`.
    pub fn register_command(&self, cmd: Arc<dyn crate::commands::SlashCommand>) {
        self.command_registry
            .lock()
            .expect("command_registry lock poisoned")
            .register(cmd);
    }

    /// Cancel any active operation and release session resources.
    pub async fn close(&self) {
        let _ = self.cancel().await;
    }

    /// Send a prompt and wait for the complete response.
    ///
    /// When `history` is `None`, uses (and auto-updates) the session's
    /// internal conversation history. When `Some`, uses the provided
    /// history instead (the internal history is **not** modified).
    ///
    /// If the prompt starts with `/`, it is dispatched as a slash command
    /// and the result is returned without calling the LLM.
    pub async fn send(&self, prompt: &str, history: Option<&[Message]>) -> Result<AgentResult> {
        // Slash command interception
        if CommandRegistry::is_command(prompt) {
            let ctx = self.build_command_context();
            let output = self.command_registry().dispatch(prompt, &ctx);
            // Drop the MutexGuard before any async operations
            if let Some(output) = output {
                // BtwQuery requires an async LLM call — handle it here.
                if let Some(CommandAction::BtwQuery(ref question)) = output.action {
                    let result = self.btw(question).await?;
                    return Ok(AgentResult {
                        text: result.answer,
                        messages: history
                            .map(|h| h.to_vec())
                            .unwrap_or_else(|| read_or_recover(&self.history).clone()),
                        tool_calls_count: 0,
                        usage: result.usage,
                        verification_reports: Vec::new(),
                    });
                }
                return Ok(AgentResult {
                    text: output.text,
                    messages: history
                        .map(|h| h.to_vec())
                        .unwrap_or_else(|| read_or_recover(&self.history).clone()),
                    tool_calls_count: 0,
                    usage: crate::llm::TokenUsage::default(),
                    verification_reports: Vec::new(),
                });
            }
        }

        if let Some(ref w) = self.init_warning {
            tracing::warn!(session_id = %self.session_id, "Session init warning: {}", w);
        }
        let run = self.start_run(prompt).await;
        let run_id = run.id().to_string();
        let agent_loop = self.build_agent_loop();
        let (runtime_tx, mut runtime_rx) = mpsc::channel(256);
        let runtime_state = Arc::clone(&self.active_tools);
        let run_store = Arc::clone(&self.run_store);
        let collector_run_id = run_id.clone();
        let collector_session_id = self.session_id.clone();
        let collector_hook_executor = self.ahp_executor.clone();
        let runtime_collector = tokio::spawn(async move {
            while let Some(event) = runtime_rx.recv().await {
                AgentSession::record_runtime_event(
                    &run_store,
                    &collector_run_id,
                    &collector_session_id,
                    &collector_hook_executor,
                    &event,
                )
                .await;
                AgentSession::apply_runtime_event(&runtime_state, &event).await;
            }
        });

        let use_internal = history.is_none();
        let effective_history = match history {
            Some(h) => h.to_vec(),
            None => read_or_recover(&self.history).clone(),
        };

        let cancel_token = tokio_util::sync::CancellationToken::new();
        *self.cancel_token.lock().await = Some(cancel_token.clone());
        let result = agent_loop
            .execute_with_session(
                &effective_history,
                prompt,
                Some(&self.session_id),
                Some(runtime_tx),
                Some(&cancel_token),
            )
            .await;
        *self.cancel_token.lock().await = None;
        let _ = runtime_collector.await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let _ = self.run_store.mark_failed(&run_id, error.to_string()).await;
                self.clear_runtime_tracking().await;
                self.finish_run_if_current(&run_id).await;
                return Err(error.into());
            }
        };

        // Auto-accumulate: only update internal history when no custom
        // history was provided.
        if use_internal {
            *write_or_recover(&self.history) = result.messages.clone();
            self.record_verification_reports(result.verification_reports.clone());

            // Auto-save if configured
            if self.auto_save {
                if let Err(e) = self.save().await {
                    tracing::warn!("Auto-save failed for session {}: {}", self.session_id, e);
                }
            }
        }

        self.clear_runtime_tracking().await;
        self.finish_run_if_current(&run_id).await;

        Ok(result)
    }

    async fn build_btw_runtime_context(&self) -> String {
        let mut sections = Vec::new();

        let active_tools = {
            let tools = self.active_tools.read().await;
            let mut items = tools
                .iter()
                .map(|(tool_id, tool)| {
                    let elapsed_ms = Self::now_ms().saturating_sub(tool.started_at_ms);
                    format!(
                        "- {} [{}] running_for={}ms",
                        tool.tool_name, tool_id, elapsed_ms
                    )
                })
                .collect::<Vec<_>>();
            items.sort();
            items
        };
        if !active_tools.is_empty() {
            sections.push(format!("[active tools]\n{}", active_tools.join("\n")));
        }

        if let Some(cm) = &self.config.confirmation_manager {
            let pending = cm.pending_confirmations().await;
            if !pending.is_empty() {
                let mut lines = pending
                    .into_iter()
                    .map(
                        |PendingConfirmationInfo {
                             tool_id,
                             tool_name,
                             args,
                             remaining_ms,
                         }| {
                            let arg_summary = Self::compact_json_value(&args);
                            if arg_summary.is_empty() {
                                format!(
                                    "- {} [{}] remaining={}ms",
                                    tool_name, tool_id, remaining_ms
                                )
                            } else {
                                format!(
                                    "- {} [{}] remaining={}ms {}",
                                    tool_name, tool_id, remaining_ms, arg_summary
                                )
                            }
                        },
                    )
                    .collect::<Vec<_>>();
                lines.sort();
                sections.push(format!("[pending confirmations]\n{}", lines.join("\n")));
            }
        }

        if let Some(queue) = &self.command_queue {
            let stats = queue.stats().await;
            if stats.total_active > 0 || stats.total_pending > 0 || stats.external_pending > 0 {
                let mut lines = vec![format!(
                    "active={}, pending={}, external_pending={}",
                    stats.total_active, stats.total_pending, stats.external_pending
                )];
                let mut lanes = stats
                    .lanes
                    .into_values()
                    .filter(|lane| lane.active > 0 || lane.pending > 0)
                    .map(|lane| {
                        format!(
                            "- {:?}: active={}, pending={}, handler={:?}",
                            lane.lane, lane.active, lane.pending, lane.handler_mode
                        )
                    })
                    .collect::<Vec<_>>();
                lanes.sort();
                lines.extend(lanes);
                sections.push(format!("[session queue]\n{}", lines.join("\n")));
            }

            let external_tasks = queue.pending_external_tasks().await;
            if !external_tasks.is_empty() {
                let mut lines = external_tasks
                    .into_iter()
                    .take(6)
                    .map(|task| {
                        let payload_summary = Self::compact_json_value(&task.payload);
                        if payload_summary.is_empty() {
                            format!(
                                "- {} {:?} remaining={}ms",
                                task.command_type,
                                task.lane,
                                task.remaining_ms()
                            )
                        } else {
                            format!(
                                "- {} {:?} remaining={}ms {}",
                                task.command_type,
                                task.lane,
                                task.remaining_ms(),
                                payload_summary
                            )
                        }
                    })
                    .collect::<Vec<_>>();
                lines.sort();
                sections.push(format!("[pending external tasks]\n{}", lines.join("\n")));
            }
        }

        if let Some(store) = &self.session_store {
            if let Ok(Some(session)) = store.load(&self.session_id).await {
                let active_tasks = session
                    .tasks
                    .into_iter()
                    .filter(|task| task.status.is_active())
                    .take(6)
                    .map(|task| match task.tool {
                        Some(tool) if !tool.is_empty() => {
                            format!("- [{}] {} ({})", task.status, task.content, tool)
                        }
                        _ => format!("- [{}] {}", task.status, task.content),
                    })
                    .collect::<Vec<_>>();
                if !active_tasks.is_empty() {
                    sections.push(format!("[tracked tasks]\n{}", active_tasks.join("\n")));
                }
            }
        }

        sections.join("\n\n")
    }

    /// Ask an ephemeral side question without affecting conversation history.
    ///
    /// Takes a read-only snapshot of the current history, makes a separate LLM
    /// call with no tools, and returns the answer. History is never modified.
    ///
    /// Safe to call concurrently with an ongoing [`send()`](Self::send) — the
    /// snapshot only acquires a read lock on the internal history.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn run(session: &a3s_code_core::AgentSession) -> anyhow::Result<()> {
    /// let result = session.btw("what file was that error in?").await?;
    /// println!("{}", result.answer);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn btw(&self, question: &str) -> Result<BtwResult> {
        self.btw_with_context(question, None).await
    }

    /// Ask an ephemeral side question with optional caller-supplied runtime context.
    ///
    /// This keeps the core BTW behavior, but allows hosts to inject extra
    /// execution-state context that is not persisted in conversation history.
    pub async fn btw_with_context(
        &self,
        question: &str,
        runtime_context: Option<&str>,
    ) -> Result<BtwResult> {
        let question = question.trim();
        if question.is_empty() {
            return Err(crate::error::CodeError::Session(
                "btw: question cannot be empty".to_string(),
            ));
        }

        // Snapshot current history — read-only, does not block send().
        let history_snapshot = read_or_recover(&self.history).clone();

        // Append the side question as a temporary user turn.
        let mut messages = history_snapshot;
        let mut injected_sections = Vec::new();
        let session_runtime = self.build_btw_runtime_context().await;
        if !session_runtime.is_empty() {
            injected_sections.push(format!("[session runtime context]\n{}", session_runtime));
        }
        if let Some(extra) = runtime_context.map(str::trim).filter(|ctx| !ctx.is_empty()) {
            injected_sections.push(format!("[host runtime context]\n{}", extra));
        }
        if !injected_sections.is_empty() {
            let injected_context = format!(
                "Use the following runtime context only as background for the next side question. Do not treat it as a new user request.\n\n{}",
                injected_sections.join("\n\n")
            );
            messages.push(Message::user(&injected_context));
        }
        messages.push(Message::user(question));

        let response = self
            .llm_client
            .complete(&messages, Some(crate::prompts::BTW_SYSTEM), &[])
            .await
            .map_err(|e| {
                crate::error::CodeError::Llm(format!("btw: ephemeral LLM call failed: {e}"))
            })?;

        Ok(BtwResult {
            question: question.to_string(),
            answer: response.text(),
            usage: response.usage,
        })
    }

    /// Send a prompt with image attachments and wait for the complete response.
    ///
    /// Images are included as multi-modal content blocks in the user message.
    /// Requires a vision-capable model (e.g., Claude Sonnet, GPT-4o).
    pub async fn send_with_attachments(
        &self,
        prompt: &str,
        attachments: &[crate::llm::Attachment],
        history: Option<&[Message]>,
    ) -> Result<AgentResult> {
        // Build a user message with text + images, then pass it as the last
        // history entry. We use an empty prompt so execute_loop doesn't add
        // a duplicate user message.
        let use_internal = history.is_none();
        let mut effective_history = match history {
            Some(h) => h.to_vec(),
            None => read_or_recover(&self.history).clone(),
        };
        effective_history.push(Message::user_with_attachments(prompt, attachments));

        let run = self.start_run(prompt).await;
        let run_id = run.id().to_string();
        let agent_loop = self.build_agent_loop();
        let (runtime_tx, mut runtime_rx) = mpsc::channel(256);
        let runtime_state = Arc::clone(&self.active_tools);
        let run_store = Arc::clone(&self.run_store);
        let collector_run_id = run_id.clone();
        let collector_session_id = self.session_id.clone();
        let collector_hook_executor = self.ahp_executor.clone();
        let runtime_collector = tokio::spawn(async move {
            while let Some(event) = runtime_rx.recv().await {
                AgentSession::record_runtime_event(
                    &run_store,
                    &collector_run_id,
                    &collector_session_id,
                    &collector_hook_executor,
                    &event,
                )
                .await;
                AgentSession::apply_runtime_event(&runtime_state, &event).await;
            }
        });

        let cancel_token = tokio_util::sync::CancellationToken::new();
        *self.cancel_token.lock().await = Some(cancel_token.clone());
        let result = agent_loop
            .execute_from_messages(
                effective_history,
                Some(&self.session_id),
                Some(runtime_tx),
                Some(&cancel_token),
            )
            .await;
        *self.cancel_token.lock().await = None;
        let _ = runtime_collector.await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let _ = self.run_store.mark_failed(&run_id, error.to_string()).await;
                self.clear_runtime_tracking().await;
                self.finish_run_if_current(&run_id).await;
                return Err(error.into());
            }
        };

        if use_internal {
            *write_or_recover(&self.history) = result.messages.clone();
            self.record_verification_reports(result.verification_reports.clone());
            if self.auto_save {
                if let Err(e) = self.save().await {
                    tracing::warn!("Auto-save failed for session {}: {}", self.session_id, e);
                }
            }
        }

        self.clear_runtime_tracking().await;
        self.finish_run_if_current(&run_id).await;

        Ok(result)
    }

    /// Stream a prompt with image attachments.
    ///
    /// Images are included as multi-modal content blocks in the user message.
    /// Requires a vision-capable model (e.g., Claude Sonnet, GPT-4o).
    pub async fn stream_with_attachments(
        &self,
        prompt: &str,
        attachments: &[crate::llm::Attachment],
        history: Option<&[Message]>,
    ) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
        let (tx, rx) = mpsc::channel(256);
        let (runtime_tx, mut runtime_rx) = mpsc::channel(256);
        let use_internal = history.is_none();
        let mut effective_history = match history {
            Some(h) => h.to_vec(),
            None => read_or_recover(&self.history).clone(),
        };
        effective_history.push(Message::user_with_attachments(prompt, attachments));

        let run = self.start_run(prompt).await;
        let run_id = run.id().to_string();
        let agent_loop = self.build_agent_loop();
        let persistence = use_internal.then(|| SessionPersistenceContext::from_session(self));
        let session_id = self.session_id.clone();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        *self.cancel_token.lock().await = Some(cancel_token.clone());
        let token_clone = cancel_token.clone();
        let runtime_state = Arc::clone(&self.active_tools);
        let run_store = Arc::clone(&self.run_store);
        let forwarder_run_id = run_id.clone();
        let forwarder_session_id = self.session_id.clone();
        let forwarder_hook_executor = self.ahp_executor.clone();
        let should_auto_save = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let forwarder = tokio::spawn(async move {
            while let Some(event) = runtime_rx.recv().await {
                AgentSession::record_runtime_event(
                    &run_store,
                    &forwarder_run_id,
                    &forwarder_session_id,
                    &forwarder_hook_executor,
                    &event,
                )
                .await;
                AgentSession::apply_runtime_event(&runtime_state, &event).await;
                if tx.send(event).await.is_err() {
                    // Receiver dropped or buffer full — stop forwarding to avoid
                    // silently dropping subsequent events (e.g., the final `End`).
                    tracing::warn!("stream forwarder: receiver dropped, stopping event forward");
                    break;
                }
            }
        });
        let run_store = Arc::clone(&self.run_store);
        let worker_run_id = run_id.clone();
        let persistence_for_worker = persistence.clone();
        let should_auto_save_for_worker = Arc::clone(&should_auto_save);
        let handle = tokio::spawn(async move {
            let result = agent_loop
                .execute_from_messages(
                    effective_history,
                    Some(&session_id),
                    Some(runtime_tx),
                    Some(&token_clone),
                )
                .await;
            match result {
                Ok(result) => {
                    if let Some(persistence) = persistence_for_worker {
                        persistence.record_result(&result);
                        should_auto_save_for_worker
                            .store(true, std::sync::atomic::Ordering::Release);
                    }
                }
                Err(error) => {
                    let _ = run_store
                        .mark_failed(&worker_run_id, error.to_string())
                        .await;
                }
            }
        });
        let active_tools = Arc::clone(&self.active_tools);
        let current_run_id = Arc::clone(&self.current_run_id);
        let cancel_token_ref = self.cancel_token.clone();
        let wrapped_handle = tokio::spawn(async move {
            let _ = handle.await;
            let _ = forwarder.await;
            if should_auto_save.load(std::sync::atomic::Ordering::Acquire) {
                if let Some(persistence) = persistence {
                    persistence.auto_save_if_enabled().await;
                }
            }
            *cancel_token_ref.lock().await = None;
            active_tools.write().await.clear();
            let mut current = current_run_id.lock().await;
            if current.as_deref() == Some(run_id.as_str()) {
                *current = None;
            }
        });

        Ok((rx, wrapped_handle))
    }

    /// Send a prompt and stream events back.
    ///
    /// When `history` is `None`, uses the session's internal history
    /// and updates it when the stream completes.
    /// When `Some`, uses the provided history instead.
    ///
    /// If the prompt starts with `/`, it is dispatched as a slash command
    /// and the result is emitted as a single `TextDelta` + `End` event.
    pub async fn stream(
        &self,
        prompt: &str,
        history: Option<&[Message]>,
    ) -> Result<(mpsc::Receiver<AgentEvent>, JoinHandle<()>)> {
        // Slash command interception for streaming
        if CommandRegistry::is_command(prompt) {
            let ctx = self.build_command_context();
            let output = self.command_registry().dispatch(prompt, &ctx);
            // Drop the MutexGuard before spawning async tasks
            if let Some(output) = output {
                let (tx, rx) = mpsc::channel(256);

                // BtwQuery: make the ephemeral call and emit BtwAnswer event.
                if let Some(CommandAction::BtwQuery(question)) = output.action {
                    // Snapshot history and clone the client before entering the task.
                    let llm_client = self.llm_client.clone();
                    let history_snapshot = read_or_recover(&self.history).clone();
                    let handle = tokio::spawn(async move {
                        let mut messages = history_snapshot;
                        messages.push(Message::user(&question));
                        match llm_client
                            .complete(&messages, Some(crate::prompts::BTW_SYSTEM), &[])
                            .await
                        {
                            Ok(response) => {
                                let answer = response.text();
                                let _ = tx
                                    .send(AgentEvent::BtwAnswer {
                                        question: question.clone(),
                                        answer: answer.clone(),
                                        usage: response.usage,
                                    })
                                    .await;
                                let _ = tx
                                    .send(AgentEvent::End {
                                        text: answer,
                                        usage: crate::llm::TokenUsage::default(),
                                        verification_summary: Box::new(
                                            crate::verification::VerificationSummary::from_reports(
                                                &[],
                                            ),
                                        ),
                                        meta: None,
                                    })
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx
                                    .send(AgentEvent::Error {
                                        message: format!("btw failed: {e}"),
                                    })
                                    .await;
                            }
                        }
                    });
                    return Ok((rx, handle));
                }

                let handle = tokio::spawn(async move {
                    let _ = tx
                        .send(AgentEvent::TextDelta {
                            text: output.text.clone(),
                        })
                        .await;
                    let _ = tx
                        .send(AgentEvent::End {
                            text: output.text.clone(),
                            usage: crate::llm::TokenUsage::default(),
                            verification_summary: Box::new(
                                crate::verification::VerificationSummary::from_reports(&[]),
                            ),
                            meta: None,
                        })
                        .await;
                });
                return Ok((rx, handle));
            }
        }

        let (tx, rx) = mpsc::channel(256);
        let (runtime_tx, mut runtime_rx) = mpsc::channel(256);
        let agent_loop = self.build_agent_loop();
        let use_internal = history.is_none();
        let effective_history = match history {
            Some(h) => h.to_vec(),
            None => read_or_recover(&self.history).clone(),
        };
        let run = self.start_run(prompt).await;
        let run_id = run.id().to_string();
        let prompt = prompt.to_string();
        let session_id = self.session_id.clone();
        let persistence = use_internal.then(|| SessionPersistenceContext::from_session(self));

        let cancel_token = tokio_util::sync::CancellationToken::new();
        *self.cancel_token.lock().await = Some(cancel_token.clone());
        let token_clone = cancel_token.clone();
        let runtime_state = Arc::clone(&self.active_tools);
        let run_store = Arc::clone(&self.run_store);
        let forwarder_run_id = run_id.clone();
        let forwarder_session_id = self.session_id.clone();
        let forwarder_hook_executor = self.ahp_executor.clone();
        let should_auto_save = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let forwarder = tokio::spawn(async move {
            while let Some(event) = runtime_rx.recv().await {
                AgentSession::record_runtime_event(
                    &run_store,
                    &forwarder_run_id,
                    &forwarder_session_id,
                    &forwarder_hook_executor,
                    &event,
                )
                .await;
                AgentSession::apply_runtime_event(&runtime_state, &event).await;
                if tx.send(event).await.is_err() {
                    // Receiver dropped or buffer full — stop forwarding to avoid
                    // silently dropping subsequent events (e.g., the final `End`).
                    tracing::warn!("stream forwarder: receiver dropped, stopping event forward");
                    break;
                }
            }
        });

        let run_store = Arc::clone(&self.run_store);
        let worker_run_id = run_id.clone();
        let persistence_for_worker = persistence.clone();
        let should_auto_save_for_worker = Arc::clone(&should_auto_save);
        let handle = tokio::spawn(async move {
            let result = agent_loop
                .execute_with_session(
                    &effective_history,
                    &prompt,
                    Some(&session_id),
                    Some(runtime_tx),
                    Some(&token_clone),
                )
                .await;
            match result {
                Ok(result) => {
                    if let Some(persistence) = persistence_for_worker {
                        persistence.record_result(&result);
                        should_auto_save_for_worker
                            .store(true, std::sync::atomic::Ordering::Release);
                    }
                }
                Err(error) => {
                    let _ = run_store
                        .mark_failed(&worker_run_id, error.to_string())
                        .await;
                }
            }
        });

        // Wrap the handle to clear the cancel token when done
        let cancel_token_ref = self.cancel_token.clone();
        let active_tools = Arc::clone(&self.active_tools);
        let current_run_id = Arc::clone(&self.current_run_id);
        let wrapped_handle = tokio::spawn(async move {
            let _ = handle.await;
            let _ = forwarder.await;
            if should_auto_save.load(std::sync::atomic::Ordering::Acquire) {
                if let Some(persistence) = persistence {
                    persistence.auto_save_if_enabled().await;
                }
            }
            *cancel_token_ref.lock().await = None;
            active_tools.write().await.clear();
            let mut current = current_run_id.lock().await;
            if current.as_deref() == Some(run_id.as_str()) {
                *current = None;
            }
        });

        Ok((rx, wrapped_handle))
    }

    /// Cancel the current ongoing operation (send/stream).
    ///
    /// If an operation is in progress, this will trigger cancellation of the LLM streaming
    /// and tool execution. The operation will terminate as soon as possible.
    ///
    /// Returns `true` if an operation was cancelled, `false` if no operation was in progress.
    pub async fn cancel(&self) -> bool {
        let token = self.cancel_token.lock().await.clone();
        if let Some(token) = token {
            token.cancel();
            if let Some(run_id) = self.current_run_id.lock().await.clone() {
                let _ = self.run_store.mark_cancelled(&run_id).await;
                if let Some(executor) = &self.ahp_executor {
                    executor
                        .record_run_cancelled(&run_id, &self.session_id, Some("cancelled by host"))
                        .await;
                }
            }
            tracing::info!(session_id = %self.session_id, "Cancelled ongoing operation");
            true
        } else {
            tracing::debug!(session_id = %self.session_id, "No ongoing operation to cancel");
            false
        }
    }

    /// Cancel a specific run only if it is still the active run.
    ///
    /// This is useful for SDK callers that hold a previously observed run ID:
    /// stale run IDs will not cancel a newer operation.
    pub async fn cancel_run(&self, run_id: &str) -> bool {
        match self.current_run().await {
            Some(run) if run.id() == run_id => run.cancel().await,
            _ => false,
        }
    }

    /// Return snapshots for runs recorded by this session.
    pub async fn runs(&self) -> Vec<crate::run::RunSnapshot> {
        self.run_store.list().await
    }

    /// Return a snapshot for a recorded run.
    pub async fn run_snapshot(&self, run_id: &str) -> Option<crate::run::RunSnapshot> {
        self.run_store.snapshot(run_id).await
    }

    /// Return recorded runtime events for a run.
    pub async fn run_events(&self, run_id: &str) -> Vec<crate::run::RunEventRecord> {
        self.run_store.events(run_id).await
    }

    /// Return a handle for the currently running operation, if any.
    pub async fn current_run(&self) -> Option<crate::run::RunHandle> {
        let run_id = self.current_run_id.lock().await.clone()?;
        let snapshot = self.run_store.snapshot(&run_id).await?;
        Some(crate::run::RunHandle::new(
            snapshot.id,
            snapshot.session_id,
            Arc::clone(&self.run_store),
            Arc::clone(&self.cancel_token),
            Arc::clone(&self.current_run_id),
            self.ahp_executor.clone(),
        ))
    }

    /// Return a snapshot of the session's conversation history.
    pub fn history(&self) -> Vec<Message> {
        read_or_recover(&self.history).clone()
    }

    /// Return a reference to the session's memory, if configured.
    pub fn memory(&self) -> Option<&Arc<crate::memory::AgentMemory>> {
        self.memory.as_ref()
    }

    /// Return the session ID.
    pub fn id(&self) -> &str {
        &self.session_id
    }

    /// Return the session workspace path.
    pub fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }

    /// Return any deferred init warning (e.g. memory store failed to initialize).
    pub fn init_warning(&self) -> Option<&str> {
        self.init_warning.as_deref()
    }

    /// Return the session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Return the definitions of all tools currently registered in this session.
    ///
    /// The list reflects the live state of the tool executor — tools added via
    /// `add_mcp_server()` appear immediately; tools removed via
    /// `remove_mcp_server()` disappear immediately.
    pub fn tool_definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        self.tool_executor.definitions()
    }

    /// Return the names of all tools currently registered on this session.
    ///
    /// Equivalent to `tool_definitions().into_iter().map(|t| t.name).collect()`.
    /// Tools added via [`add_mcp_server`] appear immediately; tools removed via
    /// [`remove_mcp_server`] disappear immediately.
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_executor
            .definitions()
            .into_iter()
            .map(|t| t.name)
            .collect()
    }

    /// Return a stored tool artifact by URI, if it exists in this session.
    pub fn get_artifact(&self, artifact_uri: &str) -> Option<crate::tools::ToolArtifact> {
        self.tool_executor.get_artifact(artifact_uri)
    }

    /// Return compact execution trace events recorded for this session.
    pub fn trace_events(&self) -> Vec<crate::trace::TraceEvent> {
        self.trace_sink.events()
    }

    /// Return structured verification reports recorded for this session.
    pub fn verification_reports(&self) -> Vec<crate::verification::VerificationReport> {
        read_or_recover(&self.verification_reports).clone()
    }

    /// Return a structured summary of all verification reports recorded for this session.
    pub fn verification_summary(&self) -> crate::verification::VerificationSummary {
        crate::verification::VerificationSummary::from_reports(&self.verification_reports())
    }

    /// Return a concise human-readable verification summary for this session.
    pub fn verification_summary_text(&self) -> String {
        crate::verification::format_verification_summary(&self.verification_summary())
    }

    /// Add externally produced verification reports to this session's completion evidence.
    pub fn record_verification_reports(
        &self,
        reports: impl IntoIterator<Item = crate::verification::VerificationReport>,
    ) {
        let mut target = write_or_recover(&self.verification_reports);
        target.extend(reports);
    }

    // ========================================================================
    // Hook API
    // ========================================================================

    /// Register a hook for lifecycle event interception.
    pub fn register_hook(&self, hook: crate::hooks::Hook) {
        self.hook_engine.register(hook);
    }

    /// Unregister a hook by ID.
    pub fn unregister_hook(&self, hook_id: &str) -> Option<crate::hooks::Hook> {
        self.hook_engine.unregister(hook_id)
    }

    /// Register a handler for a specific hook.
    pub fn register_hook_handler(
        &self,
        hook_id: &str,
        handler: Arc<dyn crate::hooks::HookHandler>,
    ) {
        self.hook_engine.register_handler(hook_id, handler);
    }

    /// Unregister a hook handler by hook ID.
    pub fn unregister_hook_handler(&self, hook_id: &str) {
        self.hook_engine.unregister_handler(hook_id);
    }

    /// Get the number of registered hooks.
    pub fn hook_count(&self) -> usize {
        self.hook_engine.hook_count()
    }

    /// Save the session to the configured store.
    ///
    /// Returns `Ok(())` if saved successfully, or if no store is configured (no-op).
    pub async fn save(&self) -> Result<()> {
        SessionPersistenceContext::from_session(self).save().await
    }

    /// Read a file from the workspace.
    pub async fn read_file(&self, path: &str) -> Result<String> {
        let args = serde_json::json!({ "file_path": path });
        let result = self.tool_executor.execute("read", &args).await?;
        Ok(result.output)
    }

    /// Execute a bash command in the workspace.
    ///
    /// When a sandbox handle is configured via
    /// [`SessionOptions::with_sandbox_handle()`], the command is routed through
    /// that sandbox.
    pub async fn bash(&self, command: &str) -> Result<String> {
        let args = serde_json::json!({ "command": command });
        let result = self
            .tool_executor
            .execute_with_context("bash", &args, &self.tool_context)
            .await?;
        Ok(result.output)
    }

    /// Run verification commands through the session's tool execution path.
    pub async fn verify_commands(
        &self,
        subject: &str,
        commands: &[crate::verification::VerificationCommand],
    ) -> Result<crate::verification::VerificationReport> {
        let mut checks = Vec::with_capacity(commands.len());

        for command in commands {
            let mut args = serde_json::json!({ "command": command.command });
            if let Some(timeout_ms) = command.timeout_ms {
                args["timeout"] = serde_json::json!(timeout_ms);
            }

            let check = match self
                .tool_executor
                .execute_with_context("bash", &args, &self.tool_context)
                .await
            {
                Ok(result) => {
                    let exit_code = result
                        .metadata
                        .as_ref()
                        .and_then(|metadata| metadata.get("exit_code"))
                        .and_then(|value| value.as_i64())
                        .and_then(|value| i32::try_from(value).ok())
                        .unwrap_or(result.exit_code);
                    command.check_from_execution(exit_code, result.metadata.as_ref(), None)
                }
                Err(err) => command.check_from_execution(1, None, Some(&err.to_string())),
            };
            checks.push(check);
        }

        let report = crate::verification::VerificationReport::new(subject, checks);
        self.record_verification_reports([report.clone()]);
        if self.auto_save {
            if let Err(e) = self.save().await {
                tracing::warn!("Auto-save failed for session {}: {}", self.session_id, e);
            }
        }

        Ok(report)
    }

    /// Return project-aware verification command presets for this workspace.
    pub fn verification_presets(&self) -> Vec<crate::verification::VerificationPreset> {
        crate::verification::verification_presets_for_workspace(&self.workspace)
    }

    /// Search for files matching a glob pattern.
    pub async fn glob(&self, pattern: &str) -> Result<Vec<String>> {
        let args = serde_json::json!({ "pattern": pattern });
        let result = self.tool_executor.execute("glob", &args).await?;
        let files: Vec<String> = result
            .output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect();
        Ok(files)
    }

    /// Search file contents with a regex pattern.
    pub async fn grep(&self, pattern: &str) -> Result<String> {
        let args = serde_json::json!({ "pattern": pattern });
        let result = self.tool_executor.execute("grep", &args).await?;
        Ok(result.output)
    }

    /// Execute a tool by name, bypassing the LLM.
    pub async fn tool(&self, name: &str, args: serde_json::Value) -> Result<ToolCallResult> {
        let result = self.tool_executor.execute(name, &args).await?;
        Ok(ToolCallResult {
            name: name.to_string(),
            output: result.output,
            exit_code: result.exit_code,
            metadata: result.metadata,
        })
    }

    // ========================================================================
    // Advanced optional Queue API
    // ========================================================================

    /// Returns whether this session has an advanced lane queue configured.
    pub fn has_queue(&self) -> bool {
        self.command_queue.is_some()
    }

    /// Configure a lane's handler mode for explicit external/hybrid dispatch.
    ///
    /// Only effective when a queue is configured via `SessionOptions::with_queue_config`.
    pub async fn set_lane_handler(&self, lane: SessionLane, config: LaneHandlerConfig) {
        if let Some(ref queue) = self.command_queue {
            queue.set_lane_handler(lane, config).await;
        }
    }

    /// Complete an external queue task by ID.
    ///
    /// Returns `true` if the task was found and completed, `false` if not found.
    pub async fn complete_external_task(&self, task_id: &str, result: ExternalTaskResult) -> bool {
        if let Some(ref queue) = self.command_queue {
            queue.complete_external_task(task_id, result).await
        } else {
            false
        }
    }

    /// Get pending external queue tasks awaiting completion by an external handler.
    pub async fn pending_external_tasks(&self) -> Vec<ExternalTask> {
        if let Some(ref queue) = self.command_queue {
            queue.pending_external_tasks().await
        } else {
            Vec::new()
        }
    }

    /// Get optional queue statistics (pending, active, external counts per lane).
    pub async fn queue_stats(&self) -> SessionQueueStats {
        if let Some(ref queue) = self.command_queue {
            queue.stats().await
        } else {
            SessionQueueStats::default()
        }
    }

    /// Get a metrics snapshot from the optional queue (if metrics are enabled).
    pub async fn queue_metrics(&self) -> Option<MetricsSnapshot> {
        if let Some(ref queue) = self.command_queue {
            queue.metrics_snapshot().await
        } else {
            None
        }
    }

    /// Get dead letters from the optional queue's DLQ (if DLQ is enabled).
    pub async fn dead_letters(&self) -> Vec<DeadLetter> {
        if let Some(ref queue) = self.command_queue {
            queue.dead_letters().await
        } else {
            Vec::new()
        }
    }

    // ========================================================================
    // MCP API
    // ========================================================================

    /// Register all agents found in a directory with the live session.
    ///
    /// Scans `dir` for `*.yaml`, `*.yml`, and `*.md` agent definition files,
    /// parses them, and adds each one to the shared `AgentRegistry` used by the
    /// `task` tool.  New agents are immediately usable via `task(agent="…")` in
    /// the same session — no restart required.
    ///
    /// Returns the number of agents successfully loaded from the directory.
    pub fn register_agent_dir(&self, dir: &std::path::Path) -> usize {
        use crate::subagent::load_agents_from_dir;
        let agents = load_agents_from_dir(dir);
        let count = agents.len();
        for agent in agents {
            tracing::info!(
                session_id = %self.session_id,
                agent = agent.name,
                dir = %dir.display(),
                "Dynamically registered agent"
            );
            self.agent_registry.register(agent);
        }
        count
    }

    /// Add an MCP server to this session.
    ///
    /// Registers, connects, and makes all tools immediately available for the
    /// agent to call. Tool names follow the convention `mcp__<name>__<tool>`.
    ///
    /// Returns the number of tools registered from the server.
    pub async fn add_mcp_server(
        &self,
        config: crate::mcp::McpServerConfig,
    ) -> crate::error::Result<usize> {
        let server_name = config.name.clone();
        self.mcp_manager.register_server(config).await;
        self.mcp_manager.connect(&server_name).await.map_err(|e| {
            crate::error::CodeError::Tool {
                tool: server_name.clone(),
                message: format!("Failed to connect MCP server: {}", e),
            }
        })?;

        let tools = self.mcp_manager.get_server_tools(&server_name).await;
        let count = tools.len();

        for tool in
            crate::mcp::tools::create_mcp_tools(&server_name, tools, Arc::clone(&self.mcp_manager))
        {
            self.tool_executor.register_dynamic_tool(tool);
        }

        tracing::info!(
            session_id = %self.session_id,
            server = server_name,
            tools = count,
            "MCP server added to live session"
        );

        Ok(count)
    }

    /// Remove an MCP server from this session.
    ///
    /// Disconnects the server and unregisters all its tools from the executor.
    /// No-op if the server was never added.
    pub async fn remove_mcp_server(&self, server_name: &str) -> crate::error::Result<()> {
        self.tool_executor
            .unregister_tools_by_prefix(&format!("mcp__{server_name}__"));
        self.mcp_manager
            .disconnect(server_name)
            .await
            .map_err(|e| crate::error::CodeError::Tool {
                tool: server_name.to_string(),
                message: format!("Failed to disconnect MCP server: {}", e),
            })?;
        tracing::info!(
            session_id = %self.session_id,
            server = server_name,
            "MCP server removed from live session"
        );
        Ok(())
    }

    /// Return the connection status of all MCP servers registered with this session.
    pub async fn mcp_status(
        &self,
    ) -> std::collections::HashMap<String, crate::mcp::McpServerStatus> {
        self.mcp_manager.get_status().await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelConfig, ModelModalities, ProviderConfig};
    use crate::llm::{ContentBlock, LlmResponse, StreamEvent, TokenUsage};
    use crate::store::SessionStore;

    #[derive(Clone)]
    struct StaticStreamingClient {
        text: String,
    }

    impl StaticStreamingClient {
        fn new(text: impl Into<String>) -> Self {
            Self { text: text.into() }
        }

        fn response(&self) -> LlmResponse {
            LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: self.text.clone(),
                    }],
                    reasoning_content: None,
                },
                usage: TokenUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    total_tokens: 2,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                stop_reason: Some("end_turn".to_string()),
                meta: None,
            }
        }
    }

    #[derive(Clone)]
    struct FailingStreamingClient;

    #[derive(Clone)]
    struct CancellableStreamingClient {
        text: String,
    }

    #[derive(Debug, Default)]
    struct RecordingRuntimeHook {
        events: std::sync::Mutex<Vec<(String, String, AgentEvent)>>,
    }

    #[derive(Debug, Default)]
    struct CapturingContextProvider {
        session_ids: std::sync::Mutex<Vec<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl crate::context::ContextProvider for CapturingContextProvider {
        fn name(&self) -> &str {
            "capturing-context"
        }

        async fn query(
            &self,
            query: &crate::context::ContextQuery,
        ) -> anyhow::Result<crate::context::ContextResult> {
            self.session_ids
                .lock()
                .unwrap()
                .push(query.session_id.clone());
            Ok(crate::context::ContextResult::new(self.name()))
        }
    }

    #[async_trait::async_trait]
    impl crate::hooks::HookExecutor for RecordingRuntimeHook {
        async fn fire(&self, _event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
            crate::hooks::HookResult::Continue(None)
        }

        async fn record_agent_event(&self, event: &AgentEvent, run_id: &str, session_id: &str) {
            self.events.lock().unwrap().push((
                run_id.to_string(),
                session_id.to_string(),
                event.clone(),
            ));
        }
    }

    impl CancellableStreamingClient {
        fn new(text: impl Into<String>) -> Self {
            Self { text: text.into() }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for StaticStreamingClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
        ) -> anyhow::Result<LlmResponse> {
            Ok(self.response())
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
            let (tx, rx) = mpsc::channel(8);
            let text = self.text.clone();
            let response = self.response();
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::TextDelta(text)).await;
                let _ = tx.send(StreamEvent::Done(response)).await;
            });
            Ok(rx)
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for FailingStreamingClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
        ) -> anyhow::Result<LlmResponse> {
            anyhow::bail!("non-streaming fallback failed")
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
            anyhow::bail!("streaming setup failed")
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for CancellableStreamingClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
        ) -> anyhow::Result<LlmResponse> {
            anyhow::bail!("cancellable client does not support fallback completion")
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
            cancel_token: tokio_util::sync::CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
            let (tx, rx) = mpsc::channel(8);
            let text = self.text.clone();
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::TextDelta(text)).await;
                cancel_token.cancelled().await;
            });
            Ok(rx)
        }
    }

    fn test_config() -> CodeConfig {
        CodeConfig {
            default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
            providers: vec![
                ProviderConfig {
                    name: "anthropic".to_string(),
                    api_key: Some("test-key".to_string()),
                    base_url: None,
                    headers: std::collections::HashMap::new(),
                    session_id_header: None,
                    models: vec![ModelConfig {
                        id: "claude-sonnet-4-20250514".to_string(),
                        name: "Claude Sonnet 4".to_string(),
                        family: "claude-sonnet".to_string(),
                        api_key: None,
                        base_url: None,
                        headers: std::collections::HashMap::new(),
                        session_id_header: None,
                        attachment: false,
                        reasoning: false,
                        tool_call: true,
                        temperature: true,
                        release_date: None,
                        modalities: ModelModalities::default(),
                        cost: Default::default(),
                        limit: Default::default(),
                    }],
                },
                ProviderConfig {
                    name: "openai".to_string(),
                    api_key: Some("test-openai-key".to_string()),
                    base_url: None,
                    headers: std::collections::HashMap::new(),
                    session_id_header: None,
                    models: vec![ModelConfig {
                        id: "gpt-4o".to_string(),
                        name: "GPT-4o".to_string(),
                        family: "gpt-4".to_string(),
                        api_key: None,
                        base_url: None,
                        headers: std::collections::HashMap::new(),
                        session_id_header: None,
                        attachment: false,
                        reasoning: false,
                        tool_call: true,
                        temperature: true,
                        release_date: None,
                        modalities: ModelModalities::default(),
                        cost: Default::default(),
                        limit: Default::default(),
                    }],
                },
            ],
            ..Default::default()
        }
    }

    fn build_effective_registry_for_test(
        agent_registry: Option<Arc<crate::skills::SkillRegistry>>,
        opts: &SessionOptions,
    ) -> Arc<crate::skills::SkillRegistry> {
        let base_registry = agent_registry
            .as_deref()
            .map(|r| r.fork())
            .unwrap_or_else(crate::skills::SkillRegistry::with_builtins);
        if let Some(ref r) = opts.skill_registry {
            for skill in r.all() {
                base_registry.register_unchecked(skill);
            }
        }
        for dir in &opts.skill_dirs {
            if let Err(e) = base_registry.load_from_dir(dir) {
                tracing::warn!(
                    dir = %dir.display(),
                    error = %e,
                    "Failed to load session skill dir — skipping"
                );
            }
        }
        Arc::new(base_registry)
    }

    #[tokio::test]
    async fn test_from_config() {
        let agent = Agent::from_config(test_config()).await;
        assert!(agent.is_ok());
    }

    #[tokio::test]
    async fn test_session_default() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-workspace", None);
        assert!(session.is_ok());
        let debug = format!("{:?}", session.unwrap());
        assert!(debug.contains("AgentSession"));
    }

    #[tokio::test]
    async fn test_session_routes_agents_md_through_context_provider() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("AGENTS.md"),
            "Always run focused tests before reporting completion.",
        )
        .unwrap();

        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent
            .session(temp_dir.path().display().to_string(), None)
            .unwrap();

        let agents_provider = session
            .config
            .context_providers
            .iter()
            .find(|provider| provider.name() == "agents_md")
            .expect("AGENTS.md provider should be registered");
        assert!(!session
            .config
            .prompt_slots
            .extra
            .as_deref()
            .unwrap_or_default()
            .contains("Project Instructions (AGENTS.md)"));

        let result = agents_provider
            .query(&crate::context::ContextQuery::new("complete the task"))
            .await
            .unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].id, "agents_md");
        assert!(result.items[0]
            .content
            .contains("Always run focused tests before reporting completion."));
        assert_eq!(result.items[0].relevance, 0.95);
    }

    #[tokio::test]
    async fn test_session_initializes_without_legacy_agentic_tools() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let _session = agent.session("/tmp/test-workspace", None).unwrap();
    }

    #[tokio::test]
    async fn test_session_with_model_override() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_model("openai/gpt-4o");
        let session = agent.session("/tmp/test-workspace", Some(opts));
        assert!(session.is_ok());
    }

    #[tokio::test]
    async fn test_session_with_invalid_model_format() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_model("gpt-4o");
        let session = agent.session("/tmp/test-workspace", Some(opts));
        assert!(session.is_err());
    }

    #[tokio::test]
    async fn test_session_with_model_not_found() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_model("openai/nonexistent");
        let session = agent.session("/tmp/test-workspace", Some(opts));
        assert!(session.is_err());
    }

    #[tokio::test]
    async fn test_session_skill_dirs_preserve_agent_registry_validator() {
        use crate::skills::validator::DefaultSkillValidator;
        use crate::skills::SkillRegistry;

        let registry = Arc::new(SkillRegistry::new());
        registry.set_validator(Arc::new(DefaultSkillValidator::default()));

        let temp_dir = tempfile::tempdir().unwrap();
        let invalid_skill = temp_dir.path().join("invalid.md");
        std::fs::write(
            &invalid_skill,
            r#"---
name: BadName
description: "invalid skill name"
kind: instruction
---
# Invalid Skill
"#,
        )
        .unwrap();

        let opts = SessionOptions::new().with_skill_dirs([temp_dir.path()]);
        let effective_registry = build_effective_registry_for_test(Some(registry), &opts);
        assert!(effective_registry.get("BadName").is_none());
    }

    #[tokio::test]
    async fn test_session_skill_registry_overrides_agent_registry_without_polluting_parent() {
        use crate::skills::{Skill, SkillKind, SkillRegistry};

        let registry = Arc::new(SkillRegistry::new());
        registry.register_unchecked(Arc::new(Skill {
            name: "shared-skill".to_string(),
            description: "agent level".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "agent content".to_string(),
            tags: vec![],
            version: None,
        }));

        let session_registry = Arc::new(SkillRegistry::new());
        session_registry.register_unchecked(Arc::new(Skill {
            name: "shared-skill".to_string(),
            description: "session level".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "session content".to_string(),
            tags: vec![],
            version: None,
        }));

        let opts = SessionOptions::new().with_skill_registry(session_registry);
        let effective_registry = build_effective_registry_for_test(Some(registry.clone()), &opts);

        assert_eq!(
            effective_registry.get("shared-skill").unwrap().content,
            "session content"
        );
        assert_eq!(
            registry.get("shared-skill").unwrap().content,
            "agent content"
        );
    }

    #[tokio::test]
    async fn test_session_skill_dirs_override_session_registry_and_skip_invalid_entries() {
        use crate::skills::{Skill, SkillKind, SkillRegistry};

        let session_registry = Arc::new(SkillRegistry::new());
        session_registry.register_unchecked(Arc::new(Skill {
            name: "shared-skill".to_string(),
            description: "session registry".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "registry content".to_string(),
            tags: vec![],
            version: None,
        }));

        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("shared.md"),
            r#"---
name: shared-skill
description: "skill dir override"
kind: instruction
---
# Shared Skill
dir content
"#,
        )
        .unwrap();
        std::fs::write(temp_dir.path().join("README.md"), "# not a skill").unwrap();

        let opts = SessionOptions::new()
            .with_skill_registry(session_registry)
            .with_skill_dirs([temp_dir.path()]);
        let effective_registry = build_effective_registry_for_test(None, &opts);

        assert_eq!(
            effective_registry.get("shared-skill").unwrap().description,
            "skill dir override"
        );
        assert!(effective_registry.get("README").is_none());
    }

    #[tokio::test]
    async fn test_session_specific_skills_do_not_leak_across_sessions() {
        use crate::skills::{Skill, SkillKind, SkillRegistry};

        let mut agent = Agent::from_config(test_config()).await.unwrap();
        let agent_registry = Arc::new(SkillRegistry::with_builtins());
        agent.config.skill_registry = Some(agent_registry);

        let session_registry = Arc::new(SkillRegistry::new());
        session_registry.register_unchecked(Arc::new(Skill {
            name: "session-only".to_string(),
            description: "only for first session".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "session one".to_string(),
            tags: vec![],
            version: None,
        }));

        let session_one = agent
            .session(
                "/tmp/test-workspace",
                Some(SessionOptions::new().with_skill_registry(session_registry)),
            )
            .unwrap();
        let session_two = agent.session("/tmp/test-workspace", None).unwrap();

        assert!(session_one
            .config
            .skill_registry
            .as_ref()
            .unwrap()
            .get("session-only")
            .is_some());
        assert!(session_two
            .config
            .skill_registry
            .as_ref()
            .unwrap()
            .get("session-only")
            .is_none());
    }

    #[tokio::test]
    async fn test_session_for_agent_applies_definition_and_keeps_skill_overrides_isolated() {
        use crate::skills::{Skill, SkillKind, SkillRegistry};
        use crate::subagent::AgentDefinition;

        let mut agent = Agent::from_config(test_config()).await.unwrap();
        agent.config.skill_registry = Some(Arc::new(SkillRegistry::with_builtins()));

        let definition = AgentDefinition::new("reviewer", "Review code")
            .with_prompt("Agent definition prompt")
            .with_max_steps(7);

        let session_registry = Arc::new(SkillRegistry::new());
        session_registry.register_unchecked(Arc::new(Skill {
            name: "agent-session-skill".to_string(),
            description: "agent session only".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "agent session content".to_string(),
            tags: vec![],
            version: None,
        }));

        let session_one = agent
            .session_for_agent(
                "/tmp/test-workspace",
                &definition,
                Some(SessionOptions::new().with_skill_registry(session_registry)),
            )
            .unwrap();
        let session_two = agent
            .session_for_agent("/tmp/test-workspace", &definition, None)
            .unwrap();

        assert_eq!(session_one.config.max_tool_rounds, 7);
        let extra = session_one.config.prompt_slots.extra.as_deref().unwrap();
        assert!(extra.contains("Agent definition prompt"));
        assert!(!extra.contains("agent-session-skill"));
        assert!(session_one
            .config
            .context_providers
            .iter()
            .any(|provider| provider.name() == "skills_catalog"));
        assert!(session_one
            .config
            .skill_registry
            .as_ref()
            .unwrap()
            .get("agent-session-skill")
            .is_some());
        assert!(session_two
            .config
            .skill_registry
            .as_ref()
            .unwrap()
            .get("agent-session-skill")
            .is_none());
    }

    #[tokio::test]
    async fn test_session_for_agent_preserves_existing_prompt_slots_when_injecting_definition_prompt(
    ) {
        use crate::prompts::SystemPromptSlots;
        use crate::subagent::AgentDefinition;

        let agent = Agent::from_config(test_config()).await.unwrap();
        let definition = AgentDefinition::new("planner", "Plan work")
            .with_prompt("Definition extra prompt")
            .with_max_steps(3);

        let opts = SessionOptions::new().with_prompt_slots(SystemPromptSlots {
            style: None,
            role: Some("Custom role".to_string()),
            guidelines: None,
            response_style: None,
            extra: None,
        });

        let session = agent
            .session_for_agent("/tmp/test-workspace", &definition, Some(opts))
            .unwrap();

        assert_eq!(
            session.config.prompt_slots.role.as_deref(),
            Some("Custom role")
        );
        assert!(session
            .config
            .prompt_slots
            .extra
            .as_deref()
            .unwrap()
            .contains("Definition extra prompt"));
        assert_eq!(session.config.max_tool_rounds, 3);
    }

    #[tokio::test]
    async fn test_new_with_acl_string() {
        let acl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers "anthropic" {
                apiKey = "test-key"
                models "claude-sonnet-4-20250514" {
                    name = "Claude Sonnet 4"
                }
            }
        "#;
        let agent = Agent::new(acl).await;
        assert!(agent.is_ok());
    }

    #[tokio::test]
    async fn test_create_alias_acl() {
        let acl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers "anthropic" {
                apiKey = "test-key"
                models "claude-sonnet-4-20250514" {
                    name = "Claude Sonnet 4"
                }
            }
        "#;
        let agent = Agent::create(acl).await;
        assert!(agent.is_ok());
    }

    #[tokio::test]
    async fn test_create_and_new_produce_same_result() {
        let acl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers "anthropic" {
                apiKey = "test-key"
                models "claude-sonnet-4-20250514" {
                    name = "Claude Sonnet 4"
                }
            }
        "#;
        let agent_new = Agent::new(acl).await;
        let agent_create = Agent::create(acl).await;
        assert!(agent_new.is_ok());
        assert!(agent_create.is_ok());

        // Both should produce working sessions
        let session_new = agent_new.unwrap().session("/tmp/test-ws-new", None);
        let session_create = agent_create.unwrap().session("/tmp/test-ws-create", None);
        assert!(session_new.is_ok());
        assert!(session_create.is_ok());
    }

    #[tokio::test]
    async fn test_new_with_existing_acl_file_uses_file_loading() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("agent.acl");
        std::fs::write(&config_path, "providers {").unwrap();

        let err = Agent::new(config_path.display().to_string())
            .await
            .unwrap_err();
        let msg = err.to_string();

        assert!(msg.contains("Failed to load config"));
        assert!(msg.contains("agent.acl"));
        assert!(!msg.contains("Failed to parse config as ACL string"));
    }

    #[tokio::test]
    async fn test_new_with_missing_acl_file_reports_not_found() {
        let temp_dir = tempfile::tempdir().unwrap();
        let missing_path = temp_dir.path().join("agent.acl");

        let err = Agent::new(missing_path.display().to_string())
            .await
            .unwrap_err();
        let msg = err.to_string();

        assert!(msg.contains("Config file not found"));
        assert!(msg.contains("agent.acl"));
        assert!(!msg.contains("Failed to parse config as ACL string"));
    }

    #[tokio::test]
    async fn test_new_rejects_hcl_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("agent.hcl");
        std::fs::write(&config_path, "default_model = \"openai/test\"").unwrap();

        let err = Agent::new(config_path.display().to_string())
            .await
            .unwrap_err();
        let msg = err.to_string();

        assert!(msg.contains("HCL config files are not supported in 2.0"));
        assert!(msg.contains(".acl"));
    }

    #[test]
    fn test_from_config_requires_default_model() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let config = CodeConfig {
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("test-key".to_string()),
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                models: vec![],
            }],
            ..Default::default()
        };
        let result = rt.block_on(Agent::from_config(config));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_history_empty_on_new_session() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-workspace", None).unwrap();
        assert!(session.history().is_empty());
    }

    #[tokio::test]
    async fn test_stream_updates_history_and_auto_saves() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("stream-history-test")
            .with_auto_save(true);
        let session = agent
            .build_session(
                "/tmp/test-stream-history".into(),
                Arc::new(StaticStreamingClient::new("streamed answer")),
                &opts,
            )
            .unwrap();

        let (mut rx, handle) = session.stream("hello", None).await.unwrap();
        let mut saw_end = false;
        while let Some(event) = rx.recv().await {
            if matches!(event, AgentEvent::End { .. }) {
                saw_end = true;
                break;
            }
        }
        handle.await.unwrap();

        assert!(saw_end);
        let history = session.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].text(), "hello");
        assert_eq!(history[1].text(), "streamed answer");

        let saved = store
            .load("stream-history-test")
            .await
            .unwrap()
            .expect("saved session");
        assert_eq!(saved.messages.len(), 2);
        assert_eq!(saved.messages[1].text(), "streamed answer");

        let run_records = store
            .load_run_records("stream-history-test")
            .await
            .unwrap()
            .expect("saved run records");
        assert_eq!(run_records.len(), 1);
        assert_eq!(
            run_records[0].snapshot.status,
            crate::run::RunStatus::Completed
        );
        assert!(run_records[0]
            .events
            .iter()
            .any(|record| matches!(record.event, AgentEvent::End { .. })));
    }

    #[tokio::test]
    async fn test_stream_with_custom_history_does_not_update_session_history() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent
            .build_session(
                "/tmp/test-stream-custom-history".into(),
                Arc::new(StaticStreamingClient::new("custom history answer")),
                &SessionOptions::new(),
            )
            .unwrap();
        let custom_history = vec![Message::user("custom prompt")];

        let (mut rx, handle) = session
            .stream("ignored", Some(&custom_history))
            .await
            .unwrap();
        while let Some(event) = rx.recv().await {
            if matches!(event, AgentEvent::End { .. }) {
                break;
            }
        }
        handle.await.unwrap();

        assert!(session.history().is_empty());
    }

    #[tokio::test]
    async fn test_stream_error_does_not_update_history_or_auto_save() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("stream-error-test")
            .with_auto_save(true);
        let session = agent
            .build_session(
                "/tmp/test-stream-error".into(),
                Arc::new(FailingStreamingClient),
                &opts,
            )
            .unwrap();

        let (mut rx, handle) = session.stream("hello", None).await.unwrap();
        let mut saw_error = false;
        while let Some(event) = rx.recv().await {
            if matches!(event, AgentEvent::Error { .. }) {
                saw_error = true;
                break;
            }
        }
        handle.await.unwrap();

        assert!(saw_error);
        assert!(session.history().is_empty());
        assert!(store.load("stream-error-test").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_stream_cancel_does_not_update_history_or_auto_save() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("stream-cancel-test")
            .with_auto_save(true);
        let session = agent
            .build_session(
                "/tmp/test-stream-cancel".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &opts,
            )
            .unwrap();

        let (mut rx, handle) = session.stream("hello", None).await.unwrap();
        let mut saw_delta = false;
        for _ in 0..16 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("stream event before timeout")
                .expect("stream should stay open until cancelled");
            if matches!(event, AgentEvent::TextDelta { ref text } if text == "partial answer") {
                saw_delta = true;
                break;
            }
        }
        assert!(saw_delta);
        assert!(session.cancel().await);

        while rx.recv().await.is_some() {}
        handle.await.unwrap();

        assert!(session.history().is_empty());
        assert!(store.load("stream-cancel-test").await.unwrap().is_none());
        assert!(!session.cancel().await);
    }

    #[tokio::test]
    async fn test_stream_with_attachments_cancel_does_not_update_history_or_auto_save() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("stream-attachments-cancel-test")
            .with_auto_save(true);
        let session = agent
            .build_session(
                "/tmp/test-stream-attachments-cancel".into(),
                Arc::new(CancellableStreamingClient::new("partial attachment answer")),
                &opts,
            )
            .unwrap();
        let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];

        let (mut rx, handle) = session
            .stream_with_attachments("hello", &attachments, None)
            .await
            .unwrap();
        let mut saw_delta = false;
        for _ in 0..16 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("stream event before timeout")
                .expect("stream should stay open until cancelled");
            if matches!(event, AgentEvent::TextDelta { .. }) {
                saw_delta = true;
                break;
            }
        }
        assert!(saw_delta);
        assert!(session.cancel().await);

        while rx.recv().await.is_some() {}
        handle.await.unwrap();

        assert!(session.history().is_empty());
        assert!(store
            .load("stream-attachments-cancel-test")
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            session.runs().await[0].status,
            crate::run::RunStatus::Cancelled
        );
        assert!(!session.cancel().await);
    }

    #[tokio::test]
    async fn test_run_handle_cancels_send_with_attachments() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = Arc::new(
            agent
                .build_session(
                    "/tmp/test-send-attachments-run-handle-cancel".into(),
                    Arc::new(CancellableStreamingClient::new("partial answer")),
                    &SessionOptions::new(),
                )
                .unwrap(),
        );
        let worker_session = Arc::clone(&session);
        let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];

        let worker = tokio::spawn(async move {
            worker_session
                .send_with_attachments("hello", &attachments, None)
                .await
        });

        let mut run = None;
        for _ in 0..20 {
            if let Some(current) = session.current_run().await {
                run = Some(current);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let run = run.expect("current run should be visible");
        assert!(run.cancel().await);

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
            .await
            .expect("send_with_attachments should stop after cancellation")
            .expect("worker should not panic");
        assert!(result.is_err());
        assert_eq!(run.status().await, Some(crate::run::RunStatus::Cancelled));
        assert!(session.history().is_empty());
        assert!(!session.cancel().await);
    }

    #[tokio::test]
    async fn test_cancel_run_only_cancels_matching_current_run() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = Arc::new(
            agent
                .build_session(
                    "/tmp/test-cancel-run-by-id".into(),
                    Arc::new(CancellableStreamingClient::new("partial answer")),
                    &SessionOptions::new(),
                )
                .unwrap(),
        );
        let worker_session = Arc::clone(&session);
        let worker = tokio::spawn(async move { worker_session.send("hello", None).await });

        let mut run_id = None;
        for _ in 0..20 {
            if let Some(current) = session.current_run().await {
                run_id = Some(current.id().to_string());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let run_id = run_id.expect("current run should be visible");

        assert!(!session.cancel_run("stale-run").await);
        assert!(session.cancel_run(&run_id).await);

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
            .await
            .expect("send should stop after cancellation")
            .expect("worker should not panic");
        assert!(result.is_err());
        assert_eq!(
            session.run_snapshot(&run_id).await.unwrap().status,
            crate::run::RunStatus::Cancelled
        );
        assert!(!session.cancel_run(&run_id).await);
    }

    #[tokio::test]
    async fn test_send_with_attachments_passes_session_id_to_context_providers() {
        let provider = Arc::new(CapturingContextProvider::default());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new()
            .with_session_id("attachments-context-session")
            .with_context_provider(provider.clone());
        let session = agent
            .build_session(
                "/tmp/test-send-attachments-context".into(),
                Arc::new(StaticStreamingClient::new("attachment answer")),
                &opts,
            )
            .unwrap();
        let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];

        session
            .send_with_attachments("hello", &attachments, None)
            .await
            .unwrap();

        let session_ids = provider.session_ids.lock().unwrap();
        assert!(!session_ids.is_empty());
        assert!(session_ids
            .iter()
            .all(|id| id.as_deref() == Some("attachments-context-session")));
    }

    #[tokio::test]
    async fn test_send_records_run_snapshot_and_events() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent
            .build_session(
                "/tmp/test-send-run-store".into(),
                Arc::new(StaticStreamingClient::new("run answer")),
                &SessionOptions::new(),
            )
            .unwrap();

        let result = session.send("hello", None).await.unwrap();
        assert_eq!(result.text, "run answer");

        let runs = session.runs().await;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, crate::run::RunStatus::Completed);
        assert_eq!(runs[0].result_text.as_deref(), Some("run answer"));

        let events = session.run_events(&runs[0].id).await;
        assert!(events
            .iter()
            .any(|record| matches!(record.event, AgentEvent::Start { .. })));
        assert!(events
            .iter()
            .any(|record| matches!(record.event, AgentEvent::End { .. })));
    }

    #[tokio::test]
    async fn test_send_publishes_runtime_events_to_hook_executor() {
        let hook = Arc::new(RecordingRuntimeHook::default());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_hook_executor(hook.clone());
        let session = agent
            .build_session(
                "/tmp/test-runtime-event-hook".into(),
                Arc::new(StaticStreamingClient::new("hooked answer")),
                &opts,
            )
            .unwrap();

        session.send("hello", None).await.unwrap();

        let events = hook.events.lock().unwrap();
        assert!(events
            .iter()
            .any(|(_, session_id, event)| session_id == session.id()
                && matches!(event, AgentEvent::Start { .. })));
        assert!(events
            .iter()
            .any(|(_, session_id, event)| session_id == session.id()
                && matches!(event, AgentEvent::End { .. })));
        assert!(events
            .iter()
            .all(|(run_id, _, _)| run_id.starts_with("run-")));
    }

    #[tokio::test]
    async fn test_stream_exposes_current_run_handle_and_replay() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent
            .build_session(
                "/tmp/test-stream-run-handle".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &SessionOptions::new(),
            )
            .unwrap();

        let (mut rx, handle) = session.stream("hello", None).await.unwrap();
        let mut saw_delta = false;
        for _ in 0..16 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("stream event before timeout")
                .expect("stream emits event");
            if matches!(event, AgentEvent::TextDelta { .. }) {
                saw_delta = true;
                break;
            }
        }
        assert!(saw_delta);

        let run = session.current_run().await.expect("current run handle");
        assert_eq!(run.session_id(), session.id());
        assert!(matches!(
            run.status().await,
            Some(crate::run::RunStatus::Executing | crate::run::RunStatus::Planning)
        ));
        assert!(run.cancel().await);

        while rx.recv().await.is_some() {}
        handle.await.unwrap();

        let snapshot = run
            .snapshot()
            .await
            .expect("run snapshot remains replayable");
        assert_eq!(snapshot.status, crate::run::RunStatus::Cancelled);
        assert!(!run.events().await.is_empty());
    }

    #[tokio::test]
    async fn test_session_options_with_agent_dir() {
        let opts = SessionOptions::new()
            .with_agent_dir("/tmp/agents")
            .with_agent_dir("/tmp/more-agents");
        assert_eq!(opts.agent_dirs.len(), 2);
        assert_eq!(opts.agent_dirs[0], PathBuf::from("/tmp/agents"));
        assert_eq!(opts.agent_dirs[1], PathBuf::from("/tmp/more-agents"));
    }

    // ========================================================================
    // Queue Integration Tests
    // ========================================================================

    #[test]
    fn test_session_options_with_queue_config() {
        let qc = SessionQueueConfig::default().with_lane_features();
        let opts = SessionOptions::new().with_queue_config(qc.clone());
        assert!(opts.queue_config.is_some());

        let config = opts.queue_config.unwrap();
        assert!(config.enable_dlq);
        assert!(config.enable_metrics);
        assert!(config.enable_alerts);
        assert_eq!(config.default_timeout_ms, Some(60_000));
    }

    #[tokio::test]
    async fn test_session_uses_single_delegation_tool_surface() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent
            .session("/tmp/test-workspace-delegation-tools", None)
            .unwrap();
        let names = session.tool_names();

        assert!(names.contains(&"task".to_string()));
        assert!(names.contains(&"parallel_task".to_string()));
        assert!(!names.contains(&"run_team".to_string()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_with_queue_config() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent.session("/tmp/test-workspace-queue", Some(opts));
        assert!(session.is_ok());
        let session = session.unwrap();
        assert!(session.has_queue());
    }

    #[tokio::test]
    async fn test_session_without_queue_config() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-workspace-noqueue", None).unwrap();
        assert!(!session.has_queue());
    }

    #[tokio::test]
    async fn test_session_queue_stats_without_queue() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-workspace-stats", None).unwrap();
        let stats = session.queue_stats().await;
        // Without a queue, stats should have zero values
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_queue_stats_with_queue() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent
            .session("/tmp/test-workspace-qstats", Some(opts))
            .unwrap();
        let stats = session.queue_stats().await;
        // Fresh queue with no commands should have zero stats
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_pending_external_tasks_empty() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent
            .session("/tmp/test-workspace-ext", Some(opts))
            .unwrap();
        let tasks = session.pending_external_tasks().await;
        assert!(tasks.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_dead_letters_empty() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default().with_dlq(Some(100));
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent
            .session("/tmp/test-workspace-dlq", Some(opts))
            .unwrap();
        let dead = session.dead_letters().await;
        assert!(dead.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_queue_metrics_disabled() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        // Metrics not enabled
        let qc = SessionQueueConfig::default();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent
            .session("/tmp/test-workspace-nomet", Some(opts))
            .unwrap();
        let metrics = session.queue_metrics().await;
        assert!(metrics.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_queue_metrics_enabled() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default().with_metrics();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent
            .session("/tmp/test-workspace-met", Some(opts))
            .unwrap();
        let metrics = session.queue_metrics().await;
        assert!(metrics.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_set_lane_handler() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let qc = SessionQueueConfig::default();
        let opts = SessionOptions::new().with_queue_config(qc);
        let session = agent
            .session("/tmp/test-workspace-handler", Some(opts))
            .unwrap();

        // Set Execute lane to External mode
        session
            .set_lane_handler(
                SessionLane::Execute,
                LaneHandlerConfig {
                    mode: crate::queue::TaskHandlerMode::External,
                    timeout_ms: 30_000,
                },
            )
            .await;

        // No panic = success. The handler config is stored internally.
        // We can't directly read it back but we verify no errors.
    }

    // ========================================================================
    // Session Persistence Tests
    // ========================================================================

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_has_id() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-ws-id", None).unwrap();
        // Auto-generated UUID
        assert!(!session.session_id().is_empty());
        assert_eq!(session.session_id().len(), 36); // UUID format
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_explicit_id() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_session_id("my-session-42");
        let session = agent.session("/tmp/test-ws-eid", Some(opts)).unwrap();
        assert_eq!(session.session_id(), "my-session-42");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_artifact_store_limits_option() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts =
            SessionOptions::new().with_artifact_store_limits(crate::tools::ArtifactStoreLimits {
                max_artifacts: 3,
                max_bytes: 4096,
            });
        let session = agent
            .session("/tmp/test-ws-artifact-limits", Some(opts))
            .unwrap();

        let limits = session.tool_executor.artifact_store().limits();
        assert_eq!(limits.max_artifacts, 3);
        assert_eq!(limits.max_bytes, 4096);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_save_no_store() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-ws-save", None).unwrap();
        // save() is a no-op when no store is configured
        session.save().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_save_and_load() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();

        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("persist-test");
        let session = agent.session("/tmp/test-ws-persist", Some(opts)).unwrap();

        // Save empty session
        session.save().await.unwrap();

        // Verify it was stored
        assert!(store.exists("persist-test").await.unwrap());

        let data = store.load("persist-test").await.unwrap().unwrap();
        assert_eq!(data.id, "persist-test");
        assert!(data.messages.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_save_with_history() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();

        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("history-test");
        let session = agent.session("/tmp/test-ws-hist", Some(opts)).unwrap();

        // Manually inject history
        {
            let mut h = session.history.write().unwrap();
            h.push(Message::user("Hello"));
            h.push(Message::user("How are you?"));
        }

        session.save().await.unwrap();

        let data = store.load("history-test").await.unwrap().unwrap();
        assert_eq!(data.messages.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_resume_session() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();

        // Create and save a session with history
        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("resume-test");
        let session = agent.session("/tmp/test-ws-resume", Some(opts)).unwrap();
        {
            let mut h = session.history.write().unwrap();
            h.push(Message::user("What is Rust?"));
            h.push(Message::user("Tell me more"));
        }
        session.save().await.unwrap();

        // Resume the session
        let opts2 = SessionOptions::new().with_session_store(store.clone());
        let resumed = agent.resume_session("resume-test", opts2).unwrap();

        assert_eq!(resumed.session_id(), "resume-test");
        let history = resumed.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].text(), "What is Rust?");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_resume_session_restores_artifacts() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();

        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("resume-artifacts-test");
        let session = agent.session("/tmp/test-ws-artifacts", Some(opts)).unwrap();
        session
            .tool_executor
            .artifact_store()
            .put(crate::tools::ToolArtifact {
                artifact_id: "tool-output:test:a".to_string(),
                artifact_uri: "a3s://tool-output/test/a".to_string(),
                tool_name: "test".to_string(),
                content: "artifact content".to_string(),
                original_bytes: 16,
                shown_bytes: 4,
            });

        session.save().await.unwrap();
        let opts2 = SessionOptions::new().with_session_store(store.clone());
        let resumed = agent
            .resume_session("resume-artifacts-test", opts2)
            .unwrap();

        let artifact = resumed
            .get_artifact("a3s://tool-output/test/a")
            .expect("artifact");
        assert_eq!(artifact.content, "artifact content");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_resume_session_restores_trace_events() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let event = crate::trace::TraceEvent::tool_execution(
            "read",
            true,
            0,
            std::time::Duration::from_millis(3),
            32,
            Some(&serde_json::json!({
                "artifact": {
                    "artifact_uri": "a3s://tool-output/read/abc"
                }
            })),
        );

        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("resume-trace-test");
        let session = agent.session("/tmp/test-ws-trace", Some(opts)).unwrap();
        session.trace_sink.replace_events(vec![event.clone()]);
        session.save().await.unwrap();

        let opts2 = SessionOptions::new().with_session_store(store.clone());
        let resumed = agent.resume_session("resume-trace-test", opts2).unwrap();

        assert_eq!(resumed.trace_events(), vec![event]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_resume_session_restores_run_records() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();

        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("resume-runs-test");
        let session = agent.session("/tmp/test-ws-runs", Some(opts)).unwrap();
        let run = session
            .run_store
            .create_run(session.session_id(), "persist run")
            .await;
        session
            .run_store
            .record_event(
                &run.id,
                AgentEvent::Start {
                    prompt: "persist run".to_string(),
                },
            )
            .await;
        session.save().await.unwrap();

        let opts2 = SessionOptions::new().with_session_store(store.clone());
        let resumed = agent.resume_session("resume-runs-test", opts2).unwrap();

        let runs = resumed.runs().await;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].prompt, "persist run");
        assert_eq!(resumed.run_events(&run.id).await.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_resume_session_restores_verification_reports() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let report = crate::verification::VerificationReport::new(
            "program:test",
            vec![crate::verification::VerificationCheck::required(
                "check:test",
                "test",
                "Run tests",
            )
            .with_status(crate::verification::VerificationStatus::Passed)],
        );

        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("resume-verification-test");
        let session = agent
            .session("/tmp/test-ws-verification", Some(opts))
            .unwrap();
        session.record_verification_reports([report.clone()]);
        session.save().await.unwrap();

        let opts2 = SessionOptions::new().with_session_store(store.clone());
        let resumed = agent
            .resume_session("resume-verification-test", opts2)
            .unwrap();

        assert_eq!(resumed.verification_reports(), vec![report]);
        assert_eq!(
            resumed.verification_summary().status,
            crate::verification::VerificationStatus::Passed
        );
        assert!(resumed
            .verification_summary_text()
            .contains("Verification passed"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_verify_commands_builds_report_from_bash_results() {
        let temp_dir = tempfile::tempdir().unwrap();
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent
            .session(temp_dir.path().display().to_string(), None)
            .unwrap();
        let commands = vec![
            crate::verification::VerificationCommand::required(
                "check:smoke",
                "smoke",
                "Run smoke command",
                "printf ok",
            ),
            crate::verification::VerificationCommand::required(
                "check:failure",
                "smoke",
                "Run failing command",
                "exit 7",
            ),
        ];

        let report = session.verify_commands("turn", &commands).await.unwrap();

        assert_eq!(report.subject, "turn");
        assert_eq!(
            report.status,
            crate::verification::VerificationStatus::Failed
        );
        assert_eq!(
            report.checks[0].status,
            crate::verification::VerificationStatus::Passed
        );
        assert_eq!(
            report.checks[1].status,
            crate::verification::VerificationStatus::Failed
        );
        assert_eq!(
            report.checks[1].residual_risk.as_deref(),
            Some("verification command exited with code 7: exit 7")
        );
        assert_eq!(session.verification_reports(), vec![report]);
        assert_eq!(
            session.verification_summary().status,
            crate::verification::VerificationStatus::Failed
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_verification_presets_reflect_workspace() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"test":"vitest","typecheck":"tsc --noEmit"}}"#,
        )
        .unwrap();
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent
            .session(temp_dir.path().display().to_string(), None)
            .unwrap();

        let presets = session.verification_presets();

        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].project_kind, "node");
        assert_eq!(presets[0].commands[0].command, "npm test");
        assert_eq!(presets[0].commands[1].command, "npm run typecheck");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_resume_session_not_found() {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();

        let opts = SessionOptions::new().with_session_store(store.clone());
        let result = agent.resume_session("nonexistent", opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_resume_session_no_store() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new();
        let result = agent.resume_session("any-id", opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("session_store"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_file_session_store_persistence() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(
            crate::store::FileSessionStore::new(dir.path())
                .await
                .unwrap(),
        );
        let agent = Agent::from_config(test_config()).await.unwrap();

        // Save
        let opts = SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_id("file-persist");
        let session = agent
            .session("/tmp/test-ws-file-persist", Some(opts))
            .unwrap();
        {
            let mut h = session.history.write().unwrap();
            h.push(Message::user("test message"));
        }
        session.save().await.unwrap();

        // Load from a fresh store instance pointing to same dir
        let store2 = Arc::new(
            crate::store::FileSessionStore::new(dir.path())
                .await
                .unwrap(),
        );
        let data = store2.load("file-persist").await.unwrap().unwrap();
        assert_eq!(data.messages.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_options_builders() {
        let opts = SessionOptions::new()
            .with_session_id("test-id")
            .with_auto_save(true);
        assert_eq!(opts.session_id, Some("test-id".to_string()));
        assert!(opts.auto_save);
    }

    // ========================================================================
    // Memory Integration Tests
    // ========================================================================

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_with_memory_store() {
        use a3s_memory::InMemoryStore;
        let store = Arc::new(InMemoryStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_memory(store);
        let session = agent.session("/tmp/test-ws-memory", Some(opts)).unwrap();
        assert!(session.memory().is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_without_memory_store() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-ws-no-memory", None).unwrap();
        assert!(session.memory().is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_memory_wired_into_config() {
        use a3s_memory::InMemoryStore;
        let store = Arc::new(InMemoryStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_memory(store);
        let session = agent
            .session("/tmp/test-ws-mem-config", Some(opts))
            .unwrap();
        // memory is accessible via the public session API
        assert!(session.memory().is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_with_file_memory() {
        let dir = tempfile::TempDir::new().unwrap();
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_file_memory(dir.path());
        let session = agent.session("/tmp/test-ws-file-mem", Some(opts)).unwrap();
        assert!(session.memory().is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_memory_remember_and_recall() {
        use a3s_memory::InMemoryStore;
        let store = Arc::new(InMemoryStore::new());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_memory(store);
        let session = agent
            .session("/tmp/test-ws-mem-recall", Some(opts))
            .unwrap();

        let memory = session.memory().unwrap();
        memory
            .remember_success("write a file", &["write".to_string()], "done")
            .await
            .unwrap();

        let results = memory.recall_similar("write", 5).await.unwrap();
        assert!(!results.is_empty());
        let stats = memory.stats().await.unwrap();
        assert_eq!(stats.long_term_count, 1);
    }

    // ========================================================================
    // Tool timeout tests
    // ========================================================================

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_tool_timeout_configured() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_tool_timeout(5000);
        let session = agent.session("/tmp/test-ws-timeout", Some(opts)).unwrap();
        assert!(!session.id().is_empty());
    }

    // ========================================================================
    // Queue fallback tests
    // ========================================================================

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_without_queue_builds_ok() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-ws-no-queue", None).unwrap();
        assert!(!session.id().is_empty());
    }

    // ========================================================================
    // Concurrent history access tests
    // ========================================================================

    #[tokio::test(flavor = "multi_thread")]
    async fn test_concurrent_history_reads() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = Arc::new(agent.session("/tmp/test-ws-concurrent", None).unwrap());

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let s = Arc::clone(&session);
                tokio::spawn(async move { s.history().len() })
            })
            .collect();

        for h in handles {
            h.await.unwrap();
        }
    }

    // ========================================================================
    // init_warning tests
    // ========================================================================

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_no_init_warning_without_file_memory() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session("/tmp/test-ws-no-warn", None).unwrap();
        assert!(session.init_warning().is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_register_agent_dir_loads_agents_into_live_session() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Write a valid agent file
        std::fs::write(
            temp_dir.path().join("my-agent.yaml"),
            "name: my-dynamic-agent\ndescription: Dynamically registered agent\n",
        )
        .unwrap();

        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session(".", None).unwrap();

        // The agent must not be known before registration
        assert!(!session.agent_registry.exists("my-dynamic-agent"));

        let count = session.register_agent_dir(temp_dir.path());
        assert_eq!(count, 1);
        assert!(session.agent_registry.exists("my-dynamic-agent"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_register_agent_dir_empty_dir_returns_zero() {
        let temp_dir = tempfile::tempdir().unwrap();
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session(".", None).unwrap();
        let count = session.register_agent_dir(temp_dir.path());
        assert_eq!(count, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_register_agent_dir_nonexistent_returns_zero() {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let session = agent.session(".", None).unwrap();
        let count = session.register_agent_dir(std::path::Path::new("/nonexistent/path/abc"));
        assert_eq!(count, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_session_with_mcp_manager_builds_ok() {
        use crate::mcp::manager::McpManager;
        let mcp = Arc::new(McpManager::new());
        let agent = Agent::from_config(test_config()).await.unwrap();
        let opts = SessionOptions::new().with_mcp(mcp);
        // No servers connected — should build fine with zero MCP tools registered
        let session = agent.session("/tmp/test-ws-mcp", Some(opts)).unwrap();
        assert!(!session.id().is_empty());
    }

    #[test]
    fn test_session_command_is_available_from_queue_module() {
        // Compile-time check: SessionCommand remains available from its owning module.
        use crate::queue::SessionCommand;
        let _ = std::marker::PhantomData::<Box<dyn SessionCommand>>;
    }
}
