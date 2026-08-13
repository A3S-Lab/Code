use super::*;

// ============================================================================
// Agent
// ============================================================================

/// High-level agent facade.
///
/// Holds the LLM client and agent config. Workspace-independent.
/// Use [`Agent::session()`] to bind to a workspace.
pub struct Agent {
    pub(super) code_config: CodeConfig,
    pub(super) config: AgentConfig,
    /// Shared priority admission for every session and independent task.
    pub(super) task_scheduler: Arc<crate::task_scheduler::TaskScheduler>,
    /// Global MCP manager loaded from config.mcp_servers
    pub(super) global_mcp: Option<Arc<crate::mcp::manager::McpManager>>,
    /// Pre-fetched MCP tool definitions from global_mcp (cached at creation time).
    /// Wrapped in Mutex so `refresh_mcp_tools()` can update the cache without `&mut self`.
    pub(super) global_mcp_tools: std::sync::Mutex<Vec<(String, crate::mcp::McpTool)>>,
    /// Tracks session IDs reserved by in-progress builds and every live session
    /// created by this agent. Build reservations prevent duplicate IDs and make
    /// construction finalization atomic with [`Agent::close`]. Live sessions
    /// are held via `Weak` refs and pruned on registry access after drop.
    ///
    /// Uses a synchronous lock so the sync `Agent::session()` factory can
    /// insert without nesting tokio runtimes. The lock is only held for
    /// brief insert/scan operations — async close work happens after the
    /// lock is released.
    pub(super) sessions: Arc<std::sync::Mutex<agent_sessions::SessionRegistry>>,
    /// Set once `Agent::close()` has been called. Subsequent `session()` /
    /// `resume_session()` calls fail fast with `CodeError::SessionClosed`.
    pub(super) closed: Arc<std::sync::atomic::AtomicBool>,
}

/// Async-first session construction API.
///
/// Builder methods only record typed configuration. Filesystem access, queue
/// startup, and MCP discovery happen once in [`build`](Self::build).
#[must_use = "a session builder does nothing until build() is awaited"]
pub struct SessionBuilder<'a> {
    agent: &'a Agent,
    workspace: String,
    options: SessionOptions,
}

impl<'a> SessionBuilder<'a> {
    fn new(agent: &'a Agent, workspace: impl Into<String>) -> Self {
        Self {
            agent,
            workspace: workspace.into(),
            options: SessionOptions::default(),
        }
    }

    /// Replace the per-session option patch.
    pub fn options(mut self, options: SessionOptions) -> Self {
        self.options = options;
        self
    }

    /// Resolve configuration, initialize async resources, and build the session.
    pub async fn build(self) -> Result<AgentSession> {
        agent_sessions::create_session_async(self.agent, self.workspace, Some(self.options)).await
    }
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
        let config = agent_bootstrap::load_code_config(config_source.into())?;
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
        agent_bootstrap::build_agent_from_config(config).await
    }

    /// Re-fetch tool definitions from all connected global MCP servers and
    /// update the internal cache.
    ///
    /// Call this when an MCP server has added or removed tools since the
    /// agent was created. The refreshed tools will be visible to all
    /// **new** sessions created after this call; existing sessions are
    /// unaffected (their `ToolExecutor` snapshot is already built).
    pub async fn refresh_mcp_tools(&self) -> Result<()> {
        agent_sessions::refresh_mcp_tools(self).await
    }

    /// Start async-first construction of a workspace-bound session.
    pub fn session_builder(&self, workspace: impl Into<String>) -> SessionBuilder<'_> {
        SessionBuilder::new(self, workspace)
    }

    /// Build a workspace-bound session asynchronously.
    ///
    /// A session ID may have only one live or in-progress session per `Agent`.
    /// Reusing an occupied ID returns a typed session-configuration error.
    pub async fn session_async(
        &self,
        workspace: impl Into<String>,
        options: Option<SessionOptions>,
    ) -> Result<AgentSession> {
        agent_sessions::create_session_async(self, workspace, options).await
    }

    /// Open a protocol-owned session, resuming its complete persisted snapshot
    /// when one exists and optionally creating a fresh session otherwise.
    ///
    /// This remains crate-private because public SDK callers should choose
    /// explicitly between `session_async` and `resume_session_async`. The
    /// native headless Harness needs the atomic policy so process restarts do
    /// not bypass Code's existing session/run store.
    pub(crate) async fn open_protocol_session_async(
        &self,
        workspace: impl Into<String>,
        options: SessionOptions,
        create_if_missing: bool,
    ) -> Result<Option<AgentSession>> {
        agent_sessions::open_protocol_session_async(self, workspace, options, create_if_missing)
            .await
    }

    /// Bind to a workspace directory, returning an [`AgentSession`].
    ///
    /// This compatibility entry point never starts or blocks an async runtime.
    /// It requires an explicit, pre-initialized `memory_store`; an optional
    /// pre-initialized session store and cached global MCP tools are accepted.
    /// File-store specs/defaults, queues, RL trajectory files, and host-supplied
    /// MCP sources require [`session_builder`](Self::session_builder).
    /// A session ID may have only one live or in-progress session per `Agent`.
    pub fn session(
        &self,
        workspace: impl Into<String>,
        options: Option<SessionOptions>,
    ) -> Result<AgentSession> {
        agent_sessions::create_session(self, workspace, options)
    }

    /// Create a session pre-configured from an
    /// [`AgentDefinition`](crate::subagent::AgentDefinition).
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
        agent_sessions::create_session_for_agent(self, workspace, def, extra)
    }

    /// Async-first variant of [`session_for_agent`](Self::session_for_agent).
    pub async fn session_for_agent_async(
        &self,
        workspace: impl Into<String>,
        def: &crate::subagent::AgentDefinition,
        extra: Option<SessionOptions>,
    ) -> Result<AgentSession> {
        agent_sessions::create_session_for_agent_async(self, workspace, def, extra).await
    }

    /// Create a session from a reproducible disposable worker recipe.
    ///
    /// This is the cattle-mode companion to [`Agent::session_for_agent`]: callers
    /// provide a small [`WorkerAgentSpec`](crate::subagent::WorkerAgentSpec), and
    /// A3S Code compiles it into the same runtime definition used by delegated agents.
    pub fn session_for_worker(
        &self,
        workspace: impl Into<String>,
        spec: crate::subagent::WorkerAgentSpec,
        extra: Option<SessionOptions>,
    ) -> Result<AgentSession> {
        let def = spec.into_agent_definition();
        self.session_for_agent(workspace, &def, extra)
    }

    /// Async-first variant of [`session_for_worker`](Self::session_for_worker).
    pub async fn session_for_worker_async(
        &self,
        workspace: impl Into<String>,
        spec: crate::subagent::WorkerAgentSpec,
        extra: Option<SessionOptions>,
    ) -> Result<AgentSession> {
        let def = spec.into_agent_definition();
        self.session_for_agent_async(workspace, &def, extra).await
    }

    /// Resume a previously saved session by ID.
    ///
    /// Loads the session data from the store, rebuilds the `AgentSession` with
    /// the saved conversation history, and returns it ready for continued use.
    ///
    /// The `options` must include a `session_store` (or `with_file_session_store`)
    /// that contains the saved session.
    ///
    /// The resumed session uses the **workspace stored in the snapshot**, not a
    /// workspace from `options`. The store is therefore a trust boundary: its
    /// contents drive the resumed workspace and the persisted runtime policies.
    /// The requested ID must not already be live or under construction on this
    /// `Agent`.
    ///
    /// This synchronous compatibility entry point returns
    /// [`CodeError::AsyncSessionBuildRequired`](crate::error::CodeError::AsyncSessionBuildRequired);
    /// use [`resume_session_async`](Self::resume_session_async).
    pub fn resume_session(
        &self,
        session_id: &str,
        options: SessionOptions,
    ) -> Result<AgentSession> {
        agent_sessions::resume_session(self, session_id, options)
    }

    /// Resume a persisted session without blocking the async runtime.
    ///
    /// The requested ID must not already be live or under construction on this
    /// `Agent`.
    pub async fn resume_session_async(
        &self,
        session_id: &str,
        options: SessionOptions,
    ) -> Result<AgentSession> {
        agent_sessions::resume_session_async(self, session_id, options).await
    }

    /// Rebuild a live persisted session with new options without exposing a
    /// closed-session gap to the caller.
    ///
    /// The current session is saved first and remains live while the
    /// replacement is constructed and restored. If construction fails, the
    /// current session stays registered and usable. On success, the registry
    /// is switched to the replacement before the old session is closed.
    ///
    /// Callers must serialize this operation with conversation work on
    /// `current` (for example, only reconfigure an idle interactive session).
    /// The replacement keeps the same session ID and persisted history. It may
    /// bind or advance a host-supplied cognitive package; historical
    /// run evidence retains the exact binding that produced each run. The
    /// replacement snapshot is committed before registry cutover; ordinary
    /// resume still requires the persisted current binding byte-for-byte.
    pub async fn replace_session_async(
        &self,
        current: &AgentSession,
        options: SessionOptions,
    ) -> Result<AgentSession> {
        agent_sessions::replace_session_async(self, current, options).await
    }

    /// Return the IDs of every live session created from this agent.
    ///
    /// "Live" means the caller still holds an [`AgentSession`] — sessions
    /// that have been dropped are pruned lazily on each call. The list is
    /// sorted to make output stable for tests/UIs.
    pub async fn list_sessions(&self) -> Vec<String> {
        agent_sessions::list_sessions(self).await
    }

    /// Close a specific live session by its session ID.
    ///
    /// Returns `true` when a live session with the given id was found and
    /// transitioned from open to closed by this call; `false` when no live
    /// session has that id, or when the session was already closed.
    ///
    /// This is the out-of-band counterpart to [`AgentSession::close`]: it
    /// performs exactly the same cleanup but can be invoked without holding
    /// a reference to the session itself — useful for control-plane code
    /// that only knows the session ID.
    pub async fn close_session(&self, session_id: &str) -> bool {
        agent_sessions::close_session(self, session_id).await
    }

    /// Close every live session created from this agent and tear down
    /// background resources owned by the agent (global MCP connections).
    ///
    /// After this call:
    /// - Every live `AgentSession` is closed (same effect as calling
    ///   [`AgentSession::close`] on each).
    /// - Subsequent [`Agent::session`] / [`Agent::resume_session`] calls
    ///   fail fast with [`CodeError::SessionClosed`](crate::error::CodeError::SessionClosed).
    /// - Session builds admitted before close but not yet finalized are rejected
    ///   at finalization and cannot return an open session.
    ///
    /// Idempotent: subsequent calls are no-ops and are guaranteed not to
    /// panic.
    pub async fn close(&self) {
        agent_sessions::close_agent(self).await
    }

    /// Return current cross-session priority scheduler occupancy.
    pub async fn task_scheduler_stats(
        &self,
    ) -> std::result::Result<
        crate::task_scheduler::TaskSchedulerStats,
        crate::task_scheduler::TaskSchedulerError,
    > {
        self.task_scheduler.stats().await
    }

    /// Return whether [`close`](Self::close) has been called on this agent.
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Disconnect every global MCP server whose last activity is older
    /// than `idle_threshold_ms`. Returns the names of disconnected
    /// servers (empty when there is no global MCP manager or when
    /// nothing is idle).
    ///
    /// Hosts running thousands of long-lived sessions should call this
    /// periodically (e.g. every 60s with a 5-min threshold) to release
    /// file descriptors and background workers from quiet MCP servers
    /// without losing the server's configuration. A subsequent tool
    /// call on the same server will require an explicit reconnect.
    pub async fn disconnect_idle_mcp(&self, idle_threshold_ms: u64) -> Vec<String> {
        match &self.global_mcp {
            Some(mcp) => mcp.disconnect_idle(idle_threshold_ms).await,
            None => Vec::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn build_session(
        &self,
        workspace: String,
        llm_client: Arc<dyn LlmClient>,
        opts: &SessionOptions,
    ) -> Result<AgentSession> {
        let mut opts = opts.clone().with_llm_client(llm_client);
        if opts.memory_store.is_none() && opts.file_memory_dir.is_none() {
            opts = opts.with_memory(Arc::new(a3s_memory::InMemoryStore::new()));
        }
        agent_sessions::create_session(self, workspace, Some(opts))
    }
}
