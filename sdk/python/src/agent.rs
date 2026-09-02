use super::*;

/// AI coding agent. Create with `Agent.create()`, then call `agent.session()`.
#[pyclass(name = "Agent")]
pub(super) struct PyAgent {
    pub(super) inner: Arc<RustAgent>,
}

#[pymethods]
impl PyAgent {
    /// Create an Agent from a config file path or inline config string.
    ///
    /// Accepts ACL-compatible config files (.acl) or inline config strings.
    /// JSON config is not supported.
    ///
    /// Args:
    ///     config_source: Path to a config file (.acl), or inline config string
    #[staticmethod]
    fn create(py: Python<'_>, config_source: String) -> PyResult<Self> {
        let agent = py
            .allow_threads(move || get_runtime().block_on(RustAgent::new(config_source)))
            .map_err(py_code_error)?;

        Ok(Self {
            inner: Arc::new(agent),
        })
    }

    /// Create an Agent from a typed JSON-compatible `CodeConfig` mapping.
    #[staticmethod]
    fn create_from_config(py: Python<'_>, config: &Bound<'_, PyAny>) -> PyResult<Self> {
        let config: a3s_code_core::CodeConfig = serde_json::from_str(&py_any_to_json(config)?)
            .map_err(|error| PyValueError::new_err(format!("Invalid CodeConfig: {error}")))?;
        let agent = py
            .allow_threads(move || get_runtime().block_on(RustAgent::from_config(config)))
            .map_err(py_code_error)?;
        Ok(Self {
            inner: Arc::new(agent),
        })
    }

    /// Return an asyncio Future that creates an Agent from a typed config
    /// without blocking the current event-loop task.
    #[staticmethod]
    fn create_from_config_async<'py>(
        py: Python<'py>,
        config: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let config: a3s_code_core::CodeConfig = serde_json::from_str(&py_any_to_json(config)?)
            .map_err(|error| PyValueError::new_err(format!("Invalid CodeConfig: {error}")))?;
        let callable = Bound::new(
            py,
            AsyncAgentCreateConfigCall {
                config: Some(config),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    /// Return an asyncio Future that creates an Agent without blocking the
    /// current event-loop task.
    #[staticmethod]
    fn create_async<'py>(py: Python<'py>, config_source: String) -> PyResult<Bound<'py, PyAny>> {
        let callable = Bound::new(
            py,
            AsyncAgentCreateCall {
                config_source: Some(config_source),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
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
    /// Returns a `ServeHandle` only after all enabled schedule sessions and
    /// tools have been prepared. Startup failures raise from this call, so the
    /// returned handle is ready to accept scheduled work. The daemon then runs
    /// in the background until `handle.stop()` is called. Dropping the handle
    /// does NOT cancel the daemon.
    ///
    /// Args:
    ///     dir: Path to the agent directory (prompt/skills/schedules/tools)
    ///     workspace: Workspace directory each scheduled turn operates in
    ///     options: Optional SessionOptions merged into every schedule session
    ///         (model, llm_client, session_store, …)
    #[pyo3(signature = (dir, workspace, options=None))]
    fn serve_agent_dir(
        &self,
        py: Python<'_>,
        dir: String,
        workspace: String,
        options: Option<PySessionOptions>,
    ) -> PyResult<PyServeHandle> {
        let agent_dir = RustAgentDir::load(&dir)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to load agent dir: {e}")))?;
        let extra = match options {
            Some(o) => Some(build_rust_session_options(o)?),
            None => None,
        };

        let agent = self.inner.clone();
        let started = py.allow_threads(move || {
            get_runtime().block_on(async move {
                let handle = match rust_spawn_agent_dir_daemon(agent, agent_dir, workspace, extra) {
                    Ok(handle) => handle,
                    Err(error) => return Err((None, error)),
                };
                if let Err(error) = handle.wait_ready().await {
                    return Err((handle.failure_code(), error));
                }
                Ok(handle)
            })
        });
        let handle =
            started.map_err(|(failure_code, error)| py_serve_error(failure_code, error))?;

        Ok(PyServeHandle {
            inner: Arc::new(handle),
        })
    }

    /// Re-fetch tool definitions from all connected global MCP servers and
    /// update the agent-level cache.
    ///
    /// New sessions created after this call will see the refreshed tool list.
    /// Existing sessions are unaffected.
    fn refresh_mcp_tools(&self, py: Python<'_>) -> PyResult<()> {
        let agent = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async {
                agent
                    .refresh_mcp_tools()
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("refresh_mcp_tools failed: {e}")))
            })
        })
    }

    /// Return current occupancy of the priority scheduler shared by every
    /// session created from this Agent.
    fn task_scheduler_stats(&self, py: Python<'_>) -> PyResult<PyObject> {
        let agent = self.inner.clone();
        let stats = py
            .allow_threads(move || get_runtime().block_on(agent.task_scheduler_stats()))
            .map_err(py_task_scheduler_error)?;
        task_scheduler_stats_to_py(py, &stats)
    }

    /// Bind to a workspace directory, returning a Session.
    ///
    /// Args:
    ///     workspace: Path to the workspace directory
    ///     options: Optional SessionOptions object
    ///     model: Optional model override, format "provider/model" (e.g., "openai/gpt-4o")
    ///     builtin_skills: Compatibility bool for the built-in skill registry.
    ///         A3S Code currently ships no embedded built-in skills; True requests
    ///         the empty compatibility registry.
    ///     skill_dirs: Optional list of directories to scan for skill files
    ///     agent_dirs: Optional list of directories to scan for agent files
    ///     queue_config: Optional advanced SessionQueueConfig for explicit external/hybrid lane dispatch
    ///     planning_mode: Optional string: "auto", "enabled", or "disabled"
    ///     planning: Legacy optional bool. None = auto planning, True = force planning, False = disable planning
    ///     goal_tracking: Optional bool to enable goal tracking (default: False)
    ///     max_parse_retries: Optional max consecutive parse errors before abort
    ///     tool_timeout_ms: Optional per-tool execution timeout in milliseconds
    ///     llm_api_timeout_ms: Optional per-model API HTTP timeout in milliseconds
    ///     circuit_breaker_threshold: Optional max LLM API failures before abort
    ///     duplicate_tool_call_threshold: Optional duplicate tool-call guard threshold
    ///     max_parallel_tasks: Optional maximum sibling parallel branches
    ///     auto_parallel: Optional kill switch for automatic parallel child-agent fan-out
    ///     manual_delegation_enabled: Optional switch for model-visible manual delegation tools
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (workspace, options=None, model=None, builtin_skills=None, skill_dirs=None, agent_dirs=None, queue_config=None, planning_mode=None, planning=None, goal_tracking=None, max_parse_retries=None, tool_timeout_ms=None, llm_api_timeout_ms=None, circuit_breaker_threshold=None, duplicate_tool_call_threshold=None, max_parallel_tasks=None, auto_parallel=None, manual_delegation_enabled=None))]
    fn session(
        &self,
        py: Python<'_>,
        workspace: String,
        options: Option<PySessionOptions>,
        model: Option<String>,
        builtin_skills: Option<bool>,
        skill_dirs: Option<Vec<String>>,
        agent_dirs: Option<Vec<String>>,
        queue_config: Option<PySessionQueueConfig>,
        planning_mode: Option<String>,
        planning: Option<bool>,
        goal_tracking: Option<bool>,
        max_parse_retries: Option<u32>,
        tool_timeout_ms: Option<u64>,
        llm_api_timeout_ms: Option<u64>,
        circuit_breaker_threshold: Option<u32>,
        duplicate_tool_call_threshold: Option<u32>,
        max_parallel_tasks: Option<usize>,
        auto_parallel: Option<bool>,
        manual_delegation_enabled: Option<bool>,
    ) -> PyResult<PySession> {
        // If a SessionOptions object is provided, build from it then apply named-argument overrides.
        let opts = if let Some(so) = options {
            let mut o = build_rust_session_options(so)?;
            // Named args take precedence over SessionOptions fields.
            o = apply_planning_mode(o, planning_mode.as_deref(), planning)?;
            if goal_tracking.unwrap_or(false) {
                o = o.with_goal_tracking(true);
            }
            if let Some(n) = max_parse_retries {
                o = o.with_parse_retries(n);
            }
            if let Some(ms) = tool_timeout_ms {
                o = o.with_tool_timeout(ms);
            }
            if let Some(ms) = llm_api_timeout_ms {
                o = o.with_llm_api_timeout(ms);
            }
            if let Some(n) = circuit_breaker_threshold {
                o = o.with_circuit_breaker(n);
            }
            if let Some(n) = duplicate_tool_call_threshold {
                o = o.with_duplicate_tool_call_threshold(n);
            }
            if let Some(max_parallel_tasks) = max_parallel_tasks {
                o = o.with_max_parallel_tasks(max_parallel_tasks);
            }
            if let Some(auto_parallel) = auto_parallel {
                o = o.with_auto_parallel_delegation(auto_parallel);
            }
            if let Some(manual_delegation_enabled) = manual_delegation_enabled {
                o = o.with_manual_delegation_enabled(manual_delegation_enabled);
            }
            Some(o)
        } else {
            // Fall back to individual named arguments.
            let has_overrides = model.is_some()
                || builtin_skills.is_some()
                || skill_dirs.is_some()
                || agent_dirs.is_some()
                || queue_config.is_some()
                || planning_mode.is_some()
                || planning.is_some()
                || goal_tracking.is_some()
                || max_parse_retries.is_some()
                || tool_timeout_ms.is_some()
                || llm_api_timeout_ms.is_some()
                || circuit_breaker_threshold.is_some()
                || duplicate_tool_call_threshold.is_some()
                || max_parallel_tasks.is_some()
                || auto_parallel.is_some()
                || manual_delegation_enabled.is_some();

            if has_overrides {
                let mut o = RustSessionOptions::new();
                if let Some(m) = model {
                    o = o.with_model(m);
                }
                if builtin_skills.unwrap_or(false) {
                    o = o.with_builtin_skills();
                }
                if let Some(dirs) = skill_dirs {
                    for d in dirs {
                        o = o.with_skills_from_dir(d);
                    }
                }
                if let Some(dirs) = agent_dirs {
                    for d in dirs {
                        o = o.with_agent_dir(d);
                    }
                }
                if let Some(qc) = queue_config {
                    o = o.with_queue_config(qc.inner);
                }
                o = apply_planning_mode(o, planning_mode.as_deref(), planning)?;
                if goal_tracking.unwrap_or(false) {
                    o = o.with_goal_tracking(true);
                }
                if let Some(n) = max_parse_retries {
                    o = o.with_parse_retries(n);
                }
                if let Some(ms) = tool_timeout_ms {
                    o = o.with_tool_timeout(ms);
                }
                if let Some(ms) = llm_api_timeout_ms {
                    o = o.with_llm_api_timeout(ms);
                }
                if let Some(n) = circuit_breaker_threshold {
                    o = o.with_circuit_breaker(n);
                }
                if let Some(n) = duplicate_tool_call_threshold {
                    o = o.with_duplicate_tool_call_threshold(n);
                }
                if let Some(max_parallel_tasks) = max_parallel_tasks {
                    o = o.with_max_parallel_tasks(max_parallel_tasks);
                }
                if let Some(auto_parallel) = auto_parallel {
                    o = o.with_auto_parallel_delegation(auto_parallel);
                }
                if let Some(manual_delegation_enabled) = manual_delegation_enabled {
                    o = o.with_manual_delegation_enabled(manual_delegation_enabled);
                }
                Some(o)
            } else {
                None
            }
        };

        let agent = Arc::clone(&self.inner);
        let session = py
            .allow_threads(move || get_runtime().block_on(agent.session_async(workspace, opts)))
            .map_err(py_code_error)?;
        Ok(PySession {
            inner: Arc::new(session),
        })
    }

    /// Return an asyncio Future that creates a session without blocking the
    /// current event-loop task.
    #[pyo3(signature = (workspace, options=None))]
    fn session_async<'py>(
        &self,
        py: Python<'py>,
        workspace: String,
        options: Option<PySessionOptions>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let options = options
            .map(|options| build_rust_session_options_async(py, options))
            .transpose()?;
        let callable = Bound::new(
            py,
            AsyncSessionCall {
                operation: Some(AsyncSessionOperation::Create {
                    agent: Arc::clone(&self.inner),
                    workspace,
                    options,
                }),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    fn __repr__(&self) -> String {
        "Agent(...)".to_string()
    }

    /// Resume a previously saved session by ID.
    ///
    /// ``options.session_store`` must point to the store where the session was saved.
    ///
    /// .. code-block:: python
    ///
    ///     opts = SessionOptions()
    ///     opts.session_store = FileSessionStore('./sessions')
    ///     session = agent.resume_session('my-session', opts)
    ///
    /// Args:
    ///     session_id: The session ID to resume
    ///     options: SessionOptions with ``session_store`` set to the backing store
    #[pyo3(signature = (session_id, options))]
    fn resume_session(
        &self,
        py: Python<'_>,
        session_id: String,
        options: PySessionOptions,
    ) -> PyResult<PySession> {
        let opts = build_rust_session_options(options)?;
        let agent = Arc::clone(&self.inner);
        let session = py
            .allow_threads(move || {
                get_runtime().block_on(agent.resume_session_async(&session_id, opts))
            })
            .map_err(py_code_error)?;
        Ok(PySession {
            inner: Arc::new(session),
        })
    }

    /// Return an asyncio Future that resumes a session without blocking the
    /// current event-loop task.
    #[pyo3(signature = (session_id, options))]
    fn resume_session_async<'py>(
        &self,
        py: Python<'py>,
        session_id: String,
        options: PySessionOptions,
    ) -> PyResult<Bound<'py, PyAny>> {
        let options = build_rust_session_options_async(py, options)?;
        let callable = Bound::new(
            py,
            AsyncSessionCall {
                operation: Some(AsyncSessionOperation::Resume {
                    agent: Arc::clone(&self.inner),
                    session_id,
                    options,
                }),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    /// Return an asyncio Future that atomically rebuilds a live, idle session
    /// with new options.
    ///
    /// The current session remains registered and usable if replacement fails.
    /// On success, the returned session keeps the same session ID and the
    /// previous ``Session`` object is closed. Call this only while no
    /// conversation operation is running on ``current``.
    #[pyo3(signature = (current, options))]
    fn replace_session_async<'py>(
        &self,
        py: Python<'py>,
        current: PyRef<'_, PySession>,
        options: PySessionOptions,
    ) -> PyResult<Bound<'py, PyAny>> {
        let options = build_rust_session_options_async(py, options)?;
        let callable = Bound::new(
            py,
            AsyncSessionCall {
                operation: Some(AsyncSessionOperation::Replace {
                    agent: Arc::clone(&self.inner),
                    current: Arc::clone(&current.inner),
                    options,
                }),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    /// Create a session pre-configured from a named agent definition.
    ///
    /// Loads the agent by name from built-in agents and optionally from
    /// additional directories, then creates a session with the agent's
    /// permissions, system prompt, model, and step limit applied.
    ///
    /// Args:
    ///     workspace: Path to the workspace directory
    ///     agent_name: Name of the agent to load (e.g. "explore", "general")
    ///     agent_dirs: Optional list of directories to scan for agent files
    ///     options: Optional session overrides layered on top of the agent definition
    #[pyo3(signature = (workspace, agent_name, agent_dirs=None, options=None))]
    fn session_for_agent(
        &self,
        py: Python<'_>,
        workspace: String,
        agent_name: String,
        agent_dirs: Option<Vec<String>>,
        options: Option<PySessionOptions>,
    ) -> PyResult<PySession> {
        let registry = a3s_code_core::subagent::AgentRegistry::new();
        for dir in agent_dirs.unwrap_or_default() {
            let agents = a3s_code_core::subagent::load_agents_from_dir(std::path::Path::new(&dir));
            for agent in agents {
                registry.register(agent);
            }
        }
        let def = registry
            .get(&agent_name)
            .ok_or_else(|| PyRuntimeError::new_err(format!("agent '{}' not found", agent_name)))?;
        let opts = options.map(build_rust_session_options).transpose()?;
        let agent = Arc::clone(&self.inner);
        let session = py
            .allow_threads(move || {
                get_runtime().block_on(agent.session_for_agent_async(workspace, &def, opts))
            })
            .map_err(py_code_error)?;
        Ok(PySession {
            inner: Arc::new(session),
        })
    }

    /// Create a session pre-configured from a disposable worker spec.
    #[pyo3(signature = (workspace, worker, options=None))]
    fn session_for_worker(
        &self,
        py: Python<'_>,
        workspace: String,
        worker: PyWorkerAgentSpec,
        options: Option<PySessionOptions>,
    ) -> PyResult<PySession> {
        let worker = py_worker_agent_spec_to_rust(worker)?;
        let opts = options.map(build_rust_session_options).transpose()?;
        let agent = Arc::clone(&self.inner);
        let session = py
            .allow_threads(move || {
                get_runtime().block_on(agent.session_for_worker_async(workspace, worker, opts))
            })
            .map_err(py_code_error)?;
        Ok(PySession {
            inner: Arc::new(session),
        })
    }

    /// List session IDs for every live session created from this agent.
    ///
    /// Sessions that have been dropped (no Python references remain) are
    /// pruned lazily on each call. Result is sorted for stable output.
    fn list_sessions(&self, py: Python<'_>) -> Vec<String> {
        let agent = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(agent.list_sessions()))
    }

    /// Close a specific live session by its session ID.
    ///
    /// Returns ``True`` when a live session with the given id was found and
    /// transitioned from open to closed by this call; ``False`` when no
    /// live session has that id, or when it was already closed.
    fn close_session(&self, py: Python<'_>, session_id: String) -> bool {
        let agent = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(agent.close_session(&session_id)))
    }

    /// Close every live session created from this agent and disconnect
    /// background resources owned by the agent (global MCP connections).
    ///
    /// After this call, ``agent.session(...)`` and ``agent.resume_session(...)``
    /// raise ``RuntimeError`` with a "Session closed" message. Idempotent.
    fn close(&self, py: Python<'_>) {
        let agent = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(agent.close()));
    }

    /// Return an asyncio Future that closes the Agent and all live sessions.
    fn close_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let callable = Bound::new(
            py,
            AsyncAgentCloseCall {
                agent: Some(Arc::clone(&self.inner)),
            },
        )?;
        run_in_asyncio_executor(py, callable.into_any())
    }

    /// Whether ``close()`` has been called on this agent.
    #[getter]
    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// Disconnect every global MCP server idle longer than
    /// ``idle_threshold_ms``, returning the names disconnected. The
    /// server's registered config is kept — a later tool call reconnects
    /// on demand. Call periodically (e.g. every 60s with a 5-min
    /// threshold) from a host-side sweeper to release file descriptors
    /// and background workers from quiet MCP servers in long-running
    /// deployments.
    fn disconnect_idle_mcp(&self, py: Python<'_>, idle_threshold_ms: u64) -> Vec<String> {
        let agent = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(agent.disconnect_idle_mcp(idle_threshold_ms))
        })
    }
}
