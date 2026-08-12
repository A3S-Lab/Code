//! Node Agent lifecycle binding.

use super::*;

// ============================================================================
// Agent
// ============================================================================

/// AI coding agent. Create with `Agent.create()`, then call `agent.session()`.
#[napi]
pub struct Agent {
    inner: Arc<RustAgent>,
}

#[napi]
impl Agent {
    /// Create an Agent from a config file path or inline config string.
    ///
    /// Accepts ACL-compatible config files (.acl) or inline config strings.
    /// JSON config is not supported.
    ///
    /// @param configSource - Path to a config file (.acl), or inline config string
    #[napi(factory)]
    pub async fn create(config_source: String) -> napi::Result<Self> {
        let agent = get_runtime()
            .spawn(async move { RustAgent::new(config_source).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;

        Ok(Self {
            inner: Arc::new(agent),
        })
    }

    /// Re-fetch tool definitions from all connected global MCP servers and
    /// update the agent-level cache.
    ///
    /// New sessions created after this call will see the refreshed tool list.
    /// Existing sessions are unaffected.
    #[napi]
    pub async fn refresh_mcp_tools(&self) -> napi::Result<()> {
        let agent = self.inner.clone();
        agent.refresh_mcp_tools().await.map_err(node_code_error)?;
        Ok(())
    }

    /// Return current occupancy of the priority scheduler shared by every
    /// session created from this Agent.
    #[napi]
    pub async fn task_scheduler_stats(&self) -> napi::Result<TaskSchedulerStats> {
        self.inner
            .task_scheduler_stats()
            .await
            .map(TaskSchedulerStats::from)
            .map_err(node_task_scheduler_error)
    }

    /// Bind to a workspace directory, returning a Session.
    ///
    /// @param workspace - Path to the workspace directory
    /// @param options - Optional session overrides
    /// @deprecated Prefer `sessionAsync()` to avoid blocking the JavaScript event loop.
    #[napi]
    pub fn session(
        &self,
        workspace: String,
        options: Option<SessionOptions>,
    ) -> napi::Result<Session> {
        let rust_opts = js_session_options_to_rust(options)?;
        let agent = Arc::clone(&self.inner);
        let session = get_runtime()
            .block_on(agent.session_async(workspace, Some(rust_opts)))
            .map_err(node_code_error)?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }

    /// Asynchronously bind to a workspace without blocking the JavaScript
    /// event loop. New applications should prefer this over `session()`.
    #[napi(js_name = "sessionAsync")]
    pub async fn session_async(
        &self,
        workspace: String,
        options: Option<SessionOptions>,
    ) -> napi::Result<Session> {
        let rust_opts = js_session_options_to_rust(options)?;
        let agent = Arc::clone(&self.inner);
        let session = get_runtime()
            .spawn(async move { agent.session_async(workspace, Some(rust_opts)).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }

    /// Resume a previously saved session by ID.
    ///
    /// `options.sessionStore` must be set to a `FileSessionStore` (or `MemorySessionStore`)
    /// that points to the directory where the session was originally saved.
    ///
    /// ```js
    /// const session = agent.resumeSession('my-session', {
    ///   sessionStore: new FileSessionStore('./sessions'),
    /// });
    /// ```
    ///
    /// @param sessionId - The session ID to resume
    /// @param options - Session options; `sessionStore` is required
    /// @deprecated Prefer `resumeSessionAsync()` to avoid blocking the JavaScript event loop.
    #[napi]
    pub fn resume_session(
        &self,
        session_id: String,
        options: SessionOptions,
    ) -> napi::Result<Session> {
        let opts = js_session_options_to_rust(Some(options))?;
        let agent = Arc::clone(&self.inner);
        let session = get_runtime()
            .block_on(agent.resume_session_async(&session_id, opts))
            .map_err(node_code_error)?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }

    /// Asynchronously resume a saved session without blocking the JavaScript
    /// event loop. New applications should prefer this over `resumeSession()`.
    #[napi(js_name = "resumeSessionAsync")]
    pub async fn resume_session_async(
        &self,
        session_id: String,
        options: SessionOptions,
    ) -> napi::Result<Session> {
        let opts = js_session_options_to_rust(Some(options))?;
        let agent = Arc::clone(&self.inner);
        let session = get_runtime()
            .spawn(async move { agent.resume_session_async(&session_id, opts).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }

    /// Atomically rebuild a live, idle session with new options.
    ///
    /// The current session remains registered and usable if replacement fails.
    /// On success, the returned session keeps the same session ID and the
    /// previous `Session` object is closed. Call this only while no conversation
    /// operation is running on `current`.
    ///
    /// @param current - The live session to replace
    /// @param options - Replacement options; must resolve the same session store
    #[napi(js_name = "replaceSessionAsync")]
    pub async fn replace_session_async(
        &self,
        current: &Session,
        options: SessionOptions,
    ) -> napi::Result<Session> {
        let opts = js_session_options_to_rust(Some(options))?;
        let agent = Arc::clone(&self.inner);
        let current = Arc::clone(&current.inner);
        let session = get_runtime()
            .spawn(async move { agent.replace_session_async(current.as_ref(), opts).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }

    /// Create a session pre-configured from a named agent definition.
    ///
    /// Loads the agent by name from built-in agents and optionally from
    /// additional directories, then creates a session with the agent's
    /// permissions, system prompt, model, and step limit applied.
    ///
    /// @param workspace - Path to the workspace directory
    /// @param agentName - Name of the agent to load (e.g. "explore", "general")
    /// @param agentDirs - Optional directories to scan for agent files
    /// @param options - Optional session overrides layered on top of the agent definition
    /// @deprecated Prefer `sessionForAgentAsync()` to avoid blocking the JavaScript event loop.
    #[napi]
    pub fn session_for_agent(
        &self,
        workspace: String,
        agent_name: String,
        agent_dirs: Option<Vec<String>>,
        options: Option<SessionOptions>,
    ) -> napi::Result<Session> {
        let registry = a3s_code_core::subagent::AgentRegistry::new();
        for dir in agent_dirs.unwrap_or_default() {
            let agents = a3s_code_core::subagent::load_agents_from_dir(std::path::Path::new(&dir));
            for agent in agents {
                registry.register(agent);
            }
        }
        let def = registry
            .get(&agent_name)
            .ok_or_else(|| napi::Error::from_reason(format!("agent '{}' not found", agent_name)))?;
        let opts = options
            .map(|o| js_session_options_to_rust(Some(o)))
            .transpose()?;
        let agent = Arc::clone(&self.inner);
        let session = get_runtime()
            .block_on(agent.session_for_agent_async(workspace, &def, opts))
            .map_err(node_code_error)?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }

    /// Asynchronously create a session from a named agent definition.
    #[napi(js_name = "sessionForAgentAsync")]
    pub async fn session_for_agent_async(
        &self,
        workspace: String,
        agent_name: String,
        agent_dirs: Option<Vec<String>>,
        options: Option<SessionOptions>,
    ) -> napi::Result<Session> {
        let registry = a3s_code_core::subagent::AgentRegistry::new();
        for dir in agent_dirs.unwrap_or_default() {
            let agents = a3s_code_core::subagent::load_agents_from_dir(std::path::Path::new(&dir));
            for agent in agents {
                registry.register(agent);
            }
        }
        let def = registry
            .get(&agent_name)
            .ok_or_else(|| napi::Error::from_reason(format!("agent '{}' not found", agent_name)))?;
        let opts = options
            .map(|option| js_session_options_to_rust(Some(option)))
            .transpose()?;
        let agent = Arc::clone(&self.inner);
        let session = get_runtime()
            .spawn(async move { agent.session_for_agent_async(workspace, &def, opts).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }

    /// Create a session pre-configured from a disposable worker spec.
    ///
    /// This avoids writing temporary agent files for one-off cattle workers.
    ///
    /// @param workspace - Path to the workspace directory
    /// @param worker - Worker spec to compile into an agent definition
    /// @param options - Optional session overrides layered on top of the worker definition
    /// @deprecated Prefer `sessionForWorkerAsync()` to avoid blocking the JavaScript event loop.
    #[napi]
    pub fn session_for_worker(
        &self,
        workspace: String,
        worker: WorkerAgentSpec,
        options: Option<SessionOptions>,
    ) -> napi::Result<Session> {
        let worker = js_worker_agent_spec_to_rust(worker)?;
        let opts = options
            .map(|o| js_session_options_to_rust(Some(o)))
            .transpose()?;
        let agent = Arc::clone(&self.inner);
        let session = get_runtime()
            .block_on(agent.session_for_worker_async(workspace, worker, opts))
            .map_err(node_code_error)?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }

    /// Asynchronously create a session from a disposable worker spec.
    #[napi(js_name = "sessionForWorkerAsync")]
    pub async fn session_for_worker_async(
        &self,
        workspace: String,
        worker: WorkerAgentSpec,
        options: Option<SessionOptions>,
    ) -> napi::Result<Session> {
        let worker = js_worker_agent_spec_to_rust(worker)?;
        let opts = options
            .map(|option| js_session_options_to_rust(Some(option)))
            .transpose()?;
        let agent = Arc::clone(&self.inner);
        let session = get_runtime()
            .spawn(async move {
                agent
                    .session_for_worker_async(workspace, worker, opts)
                    .await
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(node_code_error)?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }

    /// List session IDs for every live session created from this agent.
    ///
    /// Sessions that have been dropped (no JS-side references remain) are
    /// pruned lazily on each call. Result is sorted for stable output.
    #[napi]
    pub async fn list_sessions(&self) -> Vec<String> {
        self.inner.list_sessions().await
    }

    /// Close a specific live session by its session ID.
    ///
    /// Returns `true` when a live session with the given id was found and
    /// transitioned from open to closed by this call; `false` when no live
    /// session has that id, or when it was already closed.
    ///
    /// Equivalent to calling `session.close()` directly, but does not
    /// require holding a reference to the session — handy for control-plane
    /// code that only knows the session ID.
    #[napi]
    pub async fn close_session(&self, session_id: String) -> bool {
        self.inner.close_session(&session_id).await
    }

    /// Close every live session created from this agent and disconnect
    /// background resources owned by the agent (global MCP connections).
    ///
    /// After this call, `agent.session(...)` and `agent.resumeSession(...)`
    /// reject with a "Session closed" error. Idempotent.
    #[napi]
    pub async fn close(&self) {
        self.inner.close().await
    }

    /// Whether `close()` has been called on this agent.
    #[napi]
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Disconnect every global MCP server idle longer than
    /// `idleThresholdMs`, returning the names disconnected. The server's
    /// registered config is kept — a later tool call reconnects on
    /// demand. Call periodically (e.g. every 60s with a 5-min threshold)
    /// from a host-side sweeper to release file descriptors and
    /// background workers from quiet MCP servers in long-running
    /// deployments.
    #[napi]
    pub async fn disconnect_idle_mcp(&self, idle_threshold_ms: i64) -> Vec<String> {
        self.inner
            .disconnect_idle_mcp(idle_threshold_ms.max(0) as u64)
            .await
    }

    /// Serve a filesystem-first agent directory's cron schedules until stopped.
    ///
    /// Loads the directory by convention: `instructions.md` (required), optional
    /// `agent.acl`, `skills/`, `schedules/*.md` (cron jobs), and `tools/*.md`
    /// (`kind: mcp` servers or `kind: script` sandboxed QuickJS tools). It starts
    /// one durable session per enabled schedule (stable id `schedule:<name>`) with
    /// the agent dir's tools installed; each schedule fires as a FULL harness turn
    /// (context, tool visibility, safety gate, verification), never a raw model call.
    ///
    /// Resolves with a {@link ServeHandle} only after all enabled schedule
    /// sessions and tools have been prepared. Startup failures reject this call,
    /// so the returned handle is ready to accept scheduled work. The daemon then
    /// runs in the background until `handle.stop()` is called. The handle MUST be
    /// kept and stopped explicitly — dropping it does NOT cancel the daemon.
    ///
    /// ```js
    /// const handle = await agent.serveAgentDir('./my-agent', '/my-project');
    /// // ... later ...
    /// await handle.stop();
    /// ```
    ///
    /// @param dir - Path to the agent directory (prompt/skills/schedules/tools)
    /// @param workspace - Workspace directory each scheduled turn operates in
    /// @param options - Optional session overrides merged into every schedule session
    ///   (model, llmClient, sessionStore, …). `promptSlots` is honored when
    ///   provided; otherwise the AgentDir `instructions.md` slot is used.
    ///   `sessionId` is always owned by the daemon and set to `schedule:<name>`.
    #[napi]
    pub async fn serve_agent_dir(
        &self,
        dir: String,
        workspace: String,
        options: Option<SessionOptions>,
    ) -> napi::Result<ServeHandle> {
        let agent_dir = RustAgentDir::load(&dir)
            .map_err(|e| napi::Error::from_reason(format!("Failed to load agent dir: {e}")))?;
        let extra = js_session_options_to_rust(options)?;

        let agent = self.inner.clone();
        let handle = get_runtime()
            .spawn(async move {
                let handle =
                    match rust_spawn_agent_dir_daemon(agent, agent_dir, workspace, Some(extra)) {
                        Ok(handle) => handle,
                        Err(error) => return Err((None, error)),
                    };
                if let Err(error) = handle.wait_ready().await {
                    return Err((handle.failure_code(), error));
                }
                Ok(handle)
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|(failure_code, error)| node_serve_error_code(failure_code, error))?;

        Ok(ServeHandle {
            inner: Arc::new(handle),
        })
    }
}
