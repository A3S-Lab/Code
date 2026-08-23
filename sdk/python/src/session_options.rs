use super::*;

/// Per-session configuration options.
///
/// Pass to `agent.session(workspace, options)` to override defaults.
#[pyclass(name = "SessionOptions")]
pub(super) struct PySessionOptions {
    pub(super) model: Option<String>,
    pub(super) task_priority: Option<String>,
    pub(super) builtin_skills: bool,
    pub(super) skill_dirs: Vec<String>,
    pub(super) enforce_active_skill_tool_restrictions: Option<bool>,
    pub(super) agent_dirs: Vec<String>,
    pub(super) worker_agents: Vec<PyWorkerAgentSpec>,
    pub(super) queue_config: Option<PySessionQueueConfig>,
    pub(super) permission_policy: Option<PyPermissionPolicy>,
    pub(super) confirmation_policy: Option<PyConfirmationPolicy>,
    pub(super) auto_compact: bool,
    pub(super) auto_compact_threshold: Option<f32>,
    pub(super) max_context_tokens: Option<usize>,
    /// Retention limits for large tool/program artifacts.
    pub(super) artifact_store_limits: Option<PyArtifactStoreLimits>,
    pub(super) tool_result_transform_policy: Option<PyToolResultTransformPolicy>,
    pub(super) tool_presentation_profile: Option<PyToolPresentationProfile>,
    /// Long-term memory store backend override. Sessions resolve a default store
    /// when this is not set. Set to a ``FileMemoryStore`` instance to customize it.
    pub(super) memory_store: Option<pyo3::PyObject>,
    /// Session persistence store backend. Set to ``FileSessionStore`` or ``MemorySessionStore``.
    pub(super) session_store: Option<pyo3::PyObject>,
    /// Security provider. Set to ``DefaultSecurityProvider`` to enable taint tracking.
    pub(super) security_provider: Option<pyo3::PyObject>,
    /// Workspace backend. Set to ``LocalWorkspaceBackend`` to use local filesystem tools explicitly.
    pub(super) workspace_backend: Option<pyo3::PyObject>,
    /// Session-bound ephemeral semantic retrieval options.
    pub(super) workspace_retrieval: Option<PyWorkspaceRetrievalOptions>,
    /// Optional remote git provider. When set, the session attaches a
    /// ``RemoteGitBackend`` on top of ``workspace_backend`` so the built-in
    /// ``git`` tool is available on object-storage workspaces. Requires
    /// ``workspace_backend`` to be set; otherwise the session raises a clear
    /// error at construction.
    pub(super) remote_git: Option<PyRemoteGitBackendConfig>,
    /// Custom role/identity (e.g. "You are a Python expert")
    pub(super) role: Option<String>,
    /// Custom coding guidelines
    pub(super) guidelines: Option<String>,
    /// Custom response style (replaces default)
    pub(super) response_style: Option<String>,
    /// Freeform extra instructions
    pub(super) extra: Option<String>,
    /// Inline skills registered programmatically: (name, kind, content).
    /// Populated via `add_instruction()` / `add_persona()` — not exposed directly to Python.
    pub(super) inline_skills: Vec<(String, String, String)>,
    /// Override maximum number of tool-call rounds per session.
    pub(super) max_tool_rounds: Option<usize>,
    /// Override maximum sibling parallel branches for this session.
    pub(super) max_parallel_tasks: Option<usize>,
    /// Override automatic child-agent delegation for this session.
    pub(super) auto_delegation: Option<PyAutoDelegationConfig>,
    /// Global session-level kill switch for automatic parallel child-agent fan-out.
    ///
    /// Manual ``task`` fan-out and legacy ``parallel_task`` calls remain
    /// available when this is false.
    pub(super) auto_parallel: Option<bool>,
    /// Session-level switch for the model-visible ``task`` tool and hidden
    /// ``parallel_task`` compatibility alias.
    pub(super) manual_delegation_enabled: Option<bool>,
    /// Explicit planning mode: "auto", "enabled", or "disabled".
    ///
    /// Prefer this over ``planning`` for an unambiguous SDK contract.
    /// If both are set, ``planning_mode`` wins.
    pub(super) planning_mode: Option<String>,
    /// Legacy planning shortcut. None = auto, True = force, False = disabled.
    pub(super) planning: Option<bool>,
    /// Enable goal tracking (default: False).
    pub(super) goal_tracking: bool,
    /// Max consecutive parse errors before abort (default: 2).
    pub(super) max_parse_retries: Option<u32>,
    /// Per-tool execution timeout in milliseconds.
    pub(super) tool_timeout_ms: Option<u64>,
    /// Per-model API HTTP timeout in milliseconds.
    pub(super) llm_api_timeout_ms: Option<u64>,
    /// Max LLM API failures before abort (default: 3).
    pub(super) circuit_breaker_threshold: Option<u32>,
    /// Max consecutive identical tool signatures before duplicate-call guard failure.
    pub(super) duplicate_tool_call_threshold: Option<u32>,
    /// Sampling temperature (0.0–1.0). Overrides the provider default.
    /// Only applied when ``model`` is also set.
    pub(super) temperature: Option<f32>,
    /// Extended thinking token budget (e.g. 10_000). Enables chain-of-thought reasoning.
    /// Only applied when ``model`` is also set. Provider must support extended thinking.
    pub(super) thinking_budget: Option<usize>,
    /// Request token-level log probabilities from OpenAI-compatible backends.
    pub(super) llm_logprobs: Option<bool>,
    /// Number of top token log probabilities to request when logprobs are enabled.
    pub(super) llm_top_logprobs: Option<usize>,
    /// Enable continuation injection (default: True).
    /// When enabled, the loop injects a follow-up prompt when the LLM stops without completing.
    pub(super) continuation_enabled: Option<bool>,
    /// Maximum continuation injections per execution (default: 3).
    pub(super) max_continuation_turns: Option<u32>,
    /// Maximum execution time in milliseconds.
    /// When set, the execution loop will abort if it exceeds this duration.
    pub(super) max_execution_time_ms: Option<u64>,
    /// Session ID for this session (auto-generated if not set).
    ///
    /// Set a stable ID to save and resume the session later:
    ///
    /// .. code-block:: python
    ///
    ///     opts = SessionOptions()
    ///     opts.session_store = FileSessionStore('./sessions')
    ///     opts.session_id = 'my-session'
    ///     opts.auto_save = True
    ///     session = agent.session('.', opts)
    ///     # Later:
    ///     resumed = agent.resume_session('my-session', opts)
    pub(super) session_id: Option<String>,
    /// Host-defined tenant id. Opaque to the framework — propagated to
    /// SessionData / hooks / traces for multi-tenant aggregation.
    pub(super) tenant_id: Option<String>,
    /// Principal identity (user / service / etc) that triggered the
    /// session. Treated as opaque.
    pub(super) principal: Option<String>,
    /// Logical id of the agent template the session was instantiated
    /// from.
    pub(super) agent_template_id: Option<String>,
    /// Distributed-trace correlation id propagated through this
    /// session's events.
    pub(super) correlation_id: Option<String>,
    /// Deterministic ID and clock configuration for replay and tests.
    pub(super) host_env: Option<PyHostEnvConfig>,
    /// Automatically save the session to the configured store after each turn (default: False).
    pub(super) auto_save: bool,
    /// Optional Python-side BudgetGuard. The framework calls
    /// `check_before_llm(session_id, estimated_tokens)`,
    /// `record_after_llm(session_id, usage_dict)`, and
    /// `check_before_tool(session_id, tool_name)` on this object.
    /// Methods that aren't defined behave as Allow / no-op.
    ///
    /// Return shapes for check_*: ``None`` or ``{"decision":"allow"}``
    /// allows; ``{"decision":"soft","resource":...,"consumed":...,"limit":...,"message":...}``
    /// emits BudgetThresholdHit("soft"); ``{"decision":"deny","resource":...,"reason":...}``
    /// aborts the call with a ``Budget exhausted`` RuntimeError.
    pub(super) budget_guard: Option<pyo3::PyObject>,
    /// Optional FIFO retention caps on the session's in-memory stores.
    /// Accepts a dict with optional integer keys:
    ///
    ///   - ``max_runs_retained``           -- cap on InMemoryRunStore.runs
    ///   - ``max_events_per_run``          -- cap on per-run event buffers
    ///   - ``max_event_bytes_per_run``     -- cap on serialized event bytes per run
    ///   - ``max_trace_events``            -- cap on InMemoryTraceSink
    ///   - ``max_terminal_subagent_tasks`` -- cap on terminal subagent entries
    ///
    /// Missing keys keep the finite framework default for that store. Set
    /// ``unbounded=True`` only when unlimited retention is deliberate.
    pub(super) retention_limits: Option<pyo3::PyObject>,
    /// Structured JSONL trajectory path. When set, records user input,
    /// LLM turns, tool calls, tool observations, token usage, and episode end.
    pub(super) trajectory_path: Option<String>,
    /// Trajectory mode: "on" or "off". Defaults to "on" when trajectory_path is set.
    pub(super) trajectory_mode: Option<String>,
    /// Max bytes retained for any single text field before truncation.
    pub(super) trajectory_max_text_bytes: Option<usize>,
    /// Whether to include full message arrays in LLM request records.
    pub(super) trajectory_include_messages: Option<bool>,
}

impl Clone for PySessionOptions {
    fn clone(&self) -> Self {
        Self {
            model: self.model.clone(),
            task_priority: self.task_priority.clone(),
            builtin_skills: self.builtin_skills,
            skill_dirs: self.skill_dirs.clone(),
            enforce_active_skill_tool_restrictions: self.enforce_active_skill_tool_restrictions,
            agent_dirs: self.agent_dirs.clone(),
            worker_agents: self.worker_agents.clone(),
            queue_config: self.queue_config.clone(),
            permission_policy: self.permission_policy.clone(),
            confirmation_policy: self.confirmation_policy.clone(),
            auto_compact: self.auto_compact,
            auto_compact_threshold: self.auto_compact_threshold,
            max_context_tokens: self.max_context_tokens,
            artifact_store_limits: self.artifact_store_limits.clone(),
            tool_result_transform_policy: self.tool_result_transform_policy.clone(),
            tool_presentation_profile: self.tool_presentation_profile.clone(),
            memory_store: pyo3::Python::with_gil(|py| {
                self.memory_store.as_ref().map(|o| o.clone_ref(py))
            }),
            session_store: pyo3::Python::with_gil(|py| {
                self.session_store.as_ref().map(|o| o.clone_ref(py))
            }),
            security_provider: pyo3::Python::with_gil(|py| {
                self.security_provider.as_ref().map(|o| o.clone_ref(py))
            }),
            workspace_backend: pyo3::Python::with_gil(|py| {
                self.workspace_backend.as_ref().map(|o| o.clone_ref(py))
            }),
            workspace_retrieval: self.workspace_retrieval.clone(),
            remote_git: self.remote_git.clone(),
            role: self.role.clone(),
            guidelines: self.guidelines.clone(),
            response_style: self.response_style.clone(),
            extra: self.extra.clone(),
            inline_skills: self.inline_skills.clone(),
            max_tool_rounds: self.max_tool_rounds,
            max_parallel_tasks: self.max_parallel_tasks,
            auto_delegation: self.auto_delegation.clone(),
            auto_parallel: self.auto_parallel,
            manual_delegation_enabled: self.manual_delegation_enabled,
            planning_mode: self.planning_mode.clone(),
            planning: self.planning,
            goal_tracking: self.goal_tracking,
            max_parse_retries: self.max_parse_retries,
            tool_timeout_ms: self.tool_timeout_ms,
            llm_api_timeout_ms: self.llm_api_timeout_ms,
            circuit_breaker_threshold: self.circuit_breaker_threshold,
            duplicate_tool_call_threshold: self.duplicate_tool_call_threshold,
            temperature: self.temperature,
            thinking_budget: self.thinking_budget,
            llm_logprobs: self.llm_logprobs,
            llm_top_logprobs: self.llm_top_logprobs,
            continuation_enabled: self.continuation_enabled,
            max_continuation_turns: self.max_continuation_turns,
            max_execution_time_ms: self.max_execution_time_ms,
            session_id: self.session_id.clone(),
            tenant_id: self.tenant_id.clone(),
            principal: self.principal.clone(),
            agent_template_id: self.agent_template_id.clone(),
            correlation_id: self.correlation_id.clone(),
            host_env: self.host_env.clone(),
            auto_save: self.auto_save,
            budget_guard: pyo3::Python::with_gil(|py| {
                self.budget_guard.as_ref().map(|o| o.clone_ref(py))
            }),
            retention_limits: pyo3::Python::with_gil(|py| {
                self.retention_limits.as_ref().map(|o| o.clone_ref(py))
            }),
            trajectory_path: self.trajectory_path.clone(),
            trajectory_mode: self.trajectory_mode.clone(),
            trajectory_max_text_bytes: self.trajectory_max_text_bytes,
            trajectory_include_messages: self.trajectory_include_messages,
        }
    }
}

#[pymethods]
impl PySessionOptions {
    #[new]
    pub(super) fn new() -> Self {
        Self {
            model: None,
            task_priority: None,
            builtin_skills: false,
            skill_dirs: vec![],
            enforce_active_skill_tool_restrictions: None,
            agent_dirs: vec![],
            worker_agents: vec![],
            queue_config: None,
            permission_policy: None,
            confirmation_policy: None,
            auto_compact: false,
            auto_compact_threshold: None,
            max_context_tokens: None,
            artifact_store_limits: None,
            tool_result_transform_policy: None,
            tool_presentation_profile: None,
            memory_store: None,
            session_store: None,
            security_provider: None,
            workspace_backend: None,
            workspace_retrieval: None,
            remote_git: None,
            role: None,
            guidelines: None,
            response_style: None,
            extra: None,
            inline_skills: vec![],
            max_tool_rounds: None,
            max_parallel_tasks: None,
            auto_delegation: None,
            auto_parallel: None,
            manual_delegation_enabled: None,
            planning_mode: None,
            planning: None,
            goal_tracking: false,
            max_parse_retries: None,
            tool_timeout_ms: None,
            llm_api_timeout_ms: None,
            circuit_breaker_threshold: None,
            duplicate_tool_call_threshold: None,
            temperature: None,
            thinking_budget: None,
            llm_logprobs: None,
            llm_top_logprobs: None,
            continuation_enabled: None,
            max_continuation_turns: None,
            max_execution_time_ms: None,
            session_id: None,
            tenant_id: None,
            principal: None,
            agent_template_id: None,
            correlation_id: None,
            host_env: None,
            auto_save: false,
            budget_guard: None,
            retention_limits: None,
            trajectory_path: None,
            trajectory_mode: None,
            trajectory_max_text_bytes: None,
            trajectory_include_messages: None,
        }
    }

    /// Override the default model. Format: "provider/model".
    #[getter]
    fn get_model(&self) -> Option<String> {
        self.model.clone()
    }

    #[setter]
    fn set_model(&mut self, value: Option<String>) {
        self.model = value;
    }

    /// Global admission priority: urgent, interactive, foreground, background, or maintenance.
    #[getter]
    fn get_task_priority(&self) -> Option<String> {
        self.task_priority.clone()
    }

    #[setter]
    fn set_task_priority(&mut self, value: Option<String>) -> PyResult<()> {
        if let Some(priority) = value.as_deref() {
            priority
                .parse::<a3s_code_core::TaskPriority>()
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
        }
        self.task_priority = value;
        Ok(())
    }

    /// Compatibility flag for the built-in skill registry.
    ///
    /// A3S Code currently ships no embedded built-in skills. Setting this to
    /// True requests the empty compatibility registry.
    #[getter]
    fn get_builtin_skills(&self) -> bool {
        self.builtin_skills
    }

    #[setter]
    fn set_builtin_skills(&mut self, value: bool) {
        self.builtin_skills = value;
    }

    /// Extra directories to scan for skill files.
    #[getter]
    fn get_skill_dirs(&self) -> Vec<String> {
        self.skill_dirs.clone()
    }

    #[setter]
    fn set_skill_dirs(&mut self, value: Vec<String>) {
        self.skill_dirs = value;
    }

    /// Whether active skill allowed-tools restrict ordinary session tool calls.
    ///
    /// Defaults to None/False. Set True to restore the legacy global
    /// active-skill restriction before permission policy, hooks, or HITL run.
    #[getter]
    fn get_enforce_active_skill_tool_restrictions(&self) -> Option<bool> {
        self.enforce_active_skill_tool_restrictions
    }

    #[setter]
    fn set_enforce_active_skill_tool_restrictions(&mut self, value: Option<bool>) {
        self.enforce_active_skill_tool_restrictions = value;
    }

    /// Extra directories to scan for agent files.
    #[getter]
    fn get_agent_dirs(&self) -> Vec<String> {
        self.agent_dirs.clone()
    }

    #[setter]
    fn set_agent_dirs(&mut self, value: Vec<String>) {
        self.agent_dirs = value;
    }

    /// Reproducible disposable workers to register for task delegation.
    #[getter]
    fn get_worker_agents(&self) -> Vec<PyWorkerAgentSpec> {
        self.worker_agents.clone()
    }

    #[setter]
    fn set_worker_agents(&mut self, value: Vec<PyWorkerAgentSpec>) {
        self.worker_agents = value;
    }

    /// Add one disposable worker agent to this session option set.
    fn add_worker_agent(&mut self, worker: PyWorkerAgentSpec) {
        self.worker_agents.push(worker);
    }

    /// Optional advanced queue configuration for explicit external/hybrid lane dispatch.
    ///
    /// Ordinary sessions are queue-free unless this is set.
    #[getter]
    fn get_queue_config(&self) -> Option<PySessionQueueConfig> {
        self.queue_config.clone()
    }

    #[setter]
    fn set_queue_config(&mut self, value: Option<PySessionQueueConfig>) {
        self.queue_config = value;
    }

    /// Explicit permission policy for tool execution.
    ///
    /// Use this to make tool access explicit for real applications.
    #[getter]
    fn get_permission_policy(&self) -> Option<PyPermissionPolicy> {
        self.permission_policy.clone()
    }

    #[setter]
    fn set_permission_policy(&mut self, value: Option<PyPermissionPolicy>) {
        self.permission_policy = value;
    }

    /// HITL confirmation policy configuration.
    #[getter]
    fn get_confirmation_policy(&self) -> Option<PyConfirmationPolicy> {
        self.confirmation_policy.clone()
    }

    #[setter]
    fn set_confirmation_policy(&mut self, value: Option<PyConfirmationPolicy>) {
        self.confirmation_policy = value;
    }

    /// Enable auto-compaction when context window fills up.
    #[getter]
    fn get_auto_compact(&self) -> bool {
        self.auto_compact
    }

    #[setter]
    fn set_auto_compact(&mut self, value: bool) {
        self.auto_compact = value;
    }

    /// Context usage threshold (0.0–1.0) to trigger auto-compaction.
    #[getter]
    fn get_auto_compact_threshold(&self) -> Option<f32> {
        self.auto_compact_threshold
    }

    #[setter]
    fn set_auto_compact_threshold(&mut self, value: Option<f32>) {
        self.auto_compact_threshold = value;
    }

    /// Active model context window used for auto-compaction accounting.
    #[getter]
    fn get_max_context_tokens(&self) -> Option<usize> {
        self.max_context_tokens
    }

    #[setter]
    pub(super) fn set_max_context_tokens(&mut self, value: Option<usize>) -> PyResult<()> {
        if value == Some(0) {
            return Err(PyValueError::new_err(
                "max_context_tokens must be a positive integer",
            ));
        }
        self.max_context_tokens = value;
        Ok(())
    }

    /// Retention limits for large tool/program artifacts.
    #[getter]
    fn get_artifact_store_limits(&self) -> Option<PyArtifactStoreLimits> {
        self.artifact_store_limits.clone()
    }

    #[setter]
    fn set_artifact_store_limits(&mut self, value: Option<PyArtifactStoreLimits>) {
        self.artifact_store_limits = value;
    }

    #[getter]
    fn get_tool_result_transform_policy(&self) -> Option<PyToolResultTransformPolicy> {
        self.tool_result_transform_policy.clone()
    }

    #[setter]
    fn set_tool_result_transform_policy(&mut self, value: Option<PyToolResultTransformPolicy>) {
        self.tool_result_transform_policy = value;
    }

    /// Closed model-facing Tool presentation profile.
    #[getter]
    fn get_tool_presentation_profile(&self) -> Option<PyToolPresentationProfile> {
        self.tool_presentation_profile.clone()
    }

    #[setter]
    fn set_tool_presentation_profile(&mut self, value: Option<PyToolPresentationProfile>) {
        self.tool_presentation_profile = value;
    }

    /// Long-term memory store backend override.
    ///
    /// Sessions resolve a default store when this is not set.
    ///
    /// Assign a ``FileMemoryStore`` instance:
    ///
    /// .. code-block:: python
    ///
    ///     opts.memory_store = FileMemoryStore('./memory')
    #[getter]
    fn get_memory_store(&self, py: pyo3::Python<'_>) -> Option<pyo3::PyObject> {
        self.memory_store.as_ref().map(|o| o.clone_ref(py))
    }

    #[setter]
    fn set_memory_store(&mut self, value: Option<pyo3::PyObject>) {
        self.memory_store = value;
    }

    /// Session persistence store backend.
    ///
    /// Assign a ``FileSessionStore`` or ``MemorySessionStore`` instance:
    ///
    /// .. code-block:: python
    ///
    ///     opts.session_store = FileSessionStore('./sessions')  # persists to disk
    ///     opts.session_store = MemorySessionStore()           # ephemeral
    #[getter]
    fn get_session_store(&self, py: pyo3::Python<'_>) -> Option<pyo3::PyObject> {
        self.session_store.as_ref().map(|o| o.clone_ref(py))
    }

    #[setter]
    fn set_session_store(&mut self, value: Option<pyo3::PyObject>) {
        self.session_store = value;
    }

    /// Security provider.
    ///
    /// Assign a ``DefaultSecurityProvider`` to enable taint tracking and output sanitisation:
    ///
    /// .. code-block:: python
    ///
    ///     opts.security_provider = DefaultSecurityProvider()
    #[getter]
    fn get_security_provider(&self, py: pyo3::Python<'_>) -> Option<pyo3::PyObject> {
        self.security_provider.as_ref().map(|o| o.clone_ref(py))
    }

    #[setter]
    fn set_security_provider(&mut self, value: Option<pyo3::PyObject>) {
        self.security_provider = value;
    }

    /// Workspace backend used by built-in tools.
    ///
    /// Assign a ``LocalWorkspaceBackend`` instance:
    ///
    /// .. code-block:: python
    ///
    ///     opts.workspace_backend = LocalWorkspaceBackend('/repo')
    #[getter]
    fn get_workspace_backend(&self, py: pyo3::Python<'_>) -> Option<pyo3::PyObject> {
        self.workspace_backend.as_ref().map(|o| o.clone_ref(py))
    }

    #[setter]
    fn set_workspace_backend(&mut self, value: Option<pyo3::PyObject>) {
        self.workspace_backend = value;
    }

    /// Typed, session-bound ephemeral workspace retrieval configuration.
    #[getter]
    fn get_workspace_retrieval(&self) -> Option<PyWorkspaceRetrievalOptions> {
        self.workspace_retrieval.clone()
    }

    #[setter]
    fn set_workspace_retrieval(&mut self, value: Option<PyWorkspaceRetrievalOptions>) {
        self.workspace_retrieval = value;
    }

    /// Optional remote git provider. Attach a ``RemoteGitBackendConfig`` to
    /// bring the built-in ``git`` tool to a session whose ``workspace_backend``
    /// cannot natively host git (e.g. S3). Requires ``workspace_backend`` to
    /// be set.
    #[getter]
    fn get_remote_git(&self) -> Option<PyRemoteGitBackendConfig> {
        self.remote_git.clone()
    }

    #[setter]
    fn set_remote_git(&mut self, value: Option<PyRemoteGitBackendConfig>) {
        self.remote_git = value;
    }

    /// Custom role/identity prepended before the core agentic prompt.
    /// Example: "You are a senior Python developer specializing in FastAPI."
    #[getter]
    fn get_role(&self) -> Option<String> {
        self.role.clone()
    }

    #[setter]
    fn set_role(&mut self, value: Option<String>) {
        self.role = value;
    }

    /// Custom coding guidelines appended after the core prompt.
    /// Example: "Always use type hints. Follow PEP 8."
    #[getter]
    fn get_guidelines(&self) -> Option<String> {
        self.guidelines.clone()
    }

    #[setter]
    fn set_guidelines(&mut self, value: Option<String>) {
        self.guidelines = value;
    }

    /// Custom response style (replaces default Response Format section).
    #[getter]
    fn get_response_style(&self) -> Option<String> {
        self.response_style.clone()
    }

    #[setter]
    fn set_response_style(&mut self, value: Option<String>) {
        self.response_style = value;
    }

    /// Freeform extra instructions appended at the end.
    #[getter]
    fn get_extra(&self) -> Option<String> {
        self.extra.clone()
    }

    #[setter]
    fn set_extra(&mut self, value: Option<String>) {
        self.extra = value;
    }

    /// Override maximum number of tool-call rounds for this session.
    #[getter]
    fn get_max_tool_rounds(&self) -> Option<usize> {
        self.max_tool_rounds
    }

    #[setter]
    fn set_max_tool_rounds(&mut self, value: Option<usize>) {
        self.max_tool_rounds = value;
    }

    /// Override maximum sibling parallel branches for this session.
    #[getter]
    fn get_max_parallel_tasks(&self) -> Option<usize> {
        self.max_parallel_tasks
    }

    #[setter]
    fn set_max_parallel_tasks(&mut self, value: Option<usize>) {
        self.max_parallel_tasks = value.map(|tasks| tasks.max(1));
    }

    /// Override automatic child-agent delegation for this session.
    #[getter]
    fn get_auto_delegation(&self) -> Option<PyAutoDelegationConfig> {
        self.auto_delegation.clone()
    }

    #[setter]
    fn set_auto_delegation(&mut self, value: Option<PyAutoDelegationConfig>) {
        self.auto_delegation = value;
    }

    /// Global session-level kill switch for automatic parallel child-agent fan-out.
    ///
    /// Manual ``task`` fan-out and legacy ``parallel_task`` calls remain
    /// available when this is false.
    #[getter]
    fn get_auto_parallel(&self) -> Option<bool> {
        self.auto_parallel
    }

    #[setter]
    fn set_auto_parallel(&mut self, value: Option<bool>) {
        self.auto_parallel = value;
    }

    /// Session-level switch for the model-visible ``task`` tool and hidden
    /// ``parallel_task`` compatibility alias.
    #[getter]
    fn get_manual_delegation_enabled(&self) -> Option<bool> {
        self.manual_delegation_enabled
    }

    #[setter]
    fn set_manual_delegation_enabled(&mut self, value: Option<bool>) {
        self.manual_delegation_enabled = value;
    }

    /// Explicit planning mode: "auto", "enabled", or "disabled".
    #[getter]
    fn get_planning_mode(&self) -> Option<String> {
        self.planning_mode.clone()
    }

    #[setter]
    fn set_planning_mode(&mut self, value: Option<String>) -> PyResult<()> {
        if let Some(ref mode) = value {
            parse_planning_mode(mode)?;
        }
        self.planning_mode = value;
        Ok(())
    }

    /// Legacy planning shortcut. None = auto, True = force, False = disabled.
    #[getter]
    fn get_planning(&self) -> Option<bool> {
        self.planning
    }

    #[setter]
    fn set_planning(&mut self, value: Option<bool>) {
        self.planning = value;
    }

    /// Enable goal tracking (default: False).
    #[getter]
    fn get_goal_tracking(&self) -> bool {
        self.goal_tracking
    }

    #[setter]
    fn set_goal_tracking(&mut self, value: bool) {
        self.goal_tracking = value;
    }

    /// Max consecutive parse errors before abort (default: 2).
    #[getter]
    fn get_max_parse_retries(&self) -> Option<u32> {
        self.max_parse_retries
    }

    #[setter]
    fn set_max_parse_retries(&mut self, value: Option<u32>) {
        self.max_parse_retries = value;
    }

    /// Per-tool execution timeout in milliseconds.
    #[getter]
    fn get_tool_timeout_ms(&self) -> Option<u64> {
        self.tool_timeout_ms
    }

    #[setter]
    fn set_tool_timeout_ms(&mut self, value: Option<u64>) {
        self.tool_timeout_ms = value;
    }

    /// Per-model API HTTP timeout in milliseconds.
    #[getter]
    fn get_llm_api_timeout_ms(&self) -> Option<u64> {
        self.llm_api_timeout_ms
    }

    #[setter]
    fn set_llm_api_timeout_ms(&mut self, value: Option<u64>) {
        self.llm_api_timeout_ms = value;
    }

    /// Max LLM API failures before abort (default: 3).
    #[getter]
    fn get_circuit_breaker_threshold(&self) -> Option<u32> {
        self.circuit_breaker_threshold
    }

    #[setter]
    fn set_circuit_breaker_threshold(&mut self, value: Option<u32>) {
        self.circuit_breaker_threshold = value;
    }

    /// Max consecutive identical tool signatures before duplicate-call guard failure.
    #[getter]
    fn get_duplicate_tool_call_threshold(&self) -> Option<u32> {
        self.duplicate_tool_call_threshold
    }

    #[setter]
    fn set_duplicate_tool_call_threshold(&mut self, value: Option<u32>) {
        self.duplicate_tool_call_threshold = value;
    }

    /// Sampling temperature (0.0–1.0). Overrides the provider default.
    /// Only applied when ``model`` is also set.
    #[getter]
    fn get_temperature(&self) -> Option<f32> {
        self.temperature
    }

    #[setter]
    fn set_temperature(&mut self, value: Option<f32>) {
        self.temperature = value;
    }

    /// Extended thinking token budget. Enables chain-of-thought reasoning.
    /// Only applied when ``model`` is also set.
    #[getter]
    fn get_thinking_budget(&self) -> Option<usize> {
        self.thinking_budget
    }

    #[setter]
    fn set_thinking_budget(&mut self, value: Option<usize>) {
        self.thinking_budget = value;
    }

    /// Request token-level log probabilities from OpenAI-compatible backends.
    ///
    /// Providers that do not support logprobs may reject the request.
    #[getter]
    fn get_llm_logprobs(&self) -> Option<bool> {
        self.llm_logprobs
    }

    #[setter]
    fn set_llm_logprobs(&mut self, value: Option<bool>) {
        self.llm_logprobs = value;
    }

    /// Number of top token log probabilities to request.
    #[getter]
    fn get_llm_top_logprobs(&self) -> Option<usize> {
        self.llm_top_logprobs
    }

    #[setter]
    fn set_llm_top_logprobs(&mut self, value: Option<usize>) {
        self.llm_top_logprobs = value;
    }

    /// Enable or disable continuation injection (default: True).
    #[getter]
    fn get_continuation_enabled(&self) -> Option<bool> {
        self.continuation_enabled
    }

    #[setter]
    fn set_continuation_enabled(&mut self, value: Option<bool>) {
        self.continuation_enabled = value;
    }

    /// Maximum continuation injections per execution (default: 3).
    #[getter]
    fn get_max_continuation_turns(&self) -> Option<u32> {
        self.max_continuation_turns
    }

    #[setter]
    fn set_max_continuation_turns(&mut self, value: Option<u32>) {
        self.max_continuation_turns = value;
    }

    /// Maximum execution time in milliseconds.
    #[getter]
    fn get_max_execution_time_ms(&self) -> Option<u64> {
        self.max_execution_time_ms
    }

    #[setter]
    fn set_max_execution_time_ms(&mut self, value: Option<u64>) {
        self.max_execution_time_ms = value;
    }

    /// Session ID (auto-generated if not set). Set to save and resume sessions by name.
    #[getter]
    fn get_session_id(&self) -> Option<String> {
        self.session_id.clone()
    }

    #[setter]
    fn set_session_id(&mut self, value: Option<String>) {
        self.session_id = value;
    }

    /// Host-defined tenant id. Opaque to the framework — used by hooks
    /// / traces / SessionData for multi-tenant aggregation.
    #[getter]
    fn get_tenant_id(&self) -> Option<String> {
        self.tenant_id.clone()
    }

    #[setter]
    fn set_tenant_id(&mut self, value: Option<String>) {
        self.tenant_id = value;
    }

    /// Identity of the principal that triggered the session.
    #[getter]
    fn get_principal(&self) -> Option<String> {
        self.principal.clone()
    }

    #[setter]
    fn set_principal(&mut self, value: Option<String>) {
        self.principal = value;
    }

    /// Logical id of the agent template / definition.
    #[getter]
    fn get_agent_template_id(&self) -> Option<String> {
        self.agent_template_id.clone()
    }

    #[setter]
    fn set_agent_template_id(&mut self, value: Option<String>) {
        self.agent_template_id = value;
    }

    /// Distributed-trace correlation id.
    #[getter]
    fn get_correlation_id(&self) -> Option<String> {
        self.correlation_id.clone()
    }

    #[setter]
    fn set_correlation_id(&mut self, value: Option<String>) {
        self.correlation_id = value;
    }

    /// Deterministic ID and clock configuration for replay and tests.
    #[getter]
    fn get_host_env(&self) -> Option<PyHostEnvConfig> {
        self.host_env.clone()
    }

    #[setter]
    fn set_host_env(&mut self, value: Option<PyHostEnvConfig>) {
        self.host_env = value;
    }

    /// Automatically save the session after each turn (default: False).
    #[getter]
    fn get_auto_save(&self) -> bool {
        self.auto_save
    }

    #[setter]
    fn set_auto_save(&mut self, value: bool) {
        self.auto_save = value;
    }

    /// Host-supplied BudgetGuard. Any Python object implementing some
    /// subset of `check_before_llm` / `record_after_llm` /
    /// `check_before_tool`. The framework calls these around every
    /// LLM call and surfaces `{"decision": "deny", ...}` as a
    /// ``Budget exhausted`` ``RuntimeError`` on ``session.send``.
    #[getter]
    fn get_budget_guard(&self) -> Option<pyo3::PyObject> {
        pyo3::Python::with_gil(|py| self.budget_guard.as_ref().map(|o| o.clone_ref(py)))
    }

    #[setter]
    fn set_budget_guard(&mut self, value: Option<pyo3::PyObject>) {
        self.budget_guard = value;
    }

    /// Optional FIFO retention config as a dict with ``unbounded`` and any subset of:
    /// ``max_runs_retained``, ``max_events_per_run``, ``max_event_bytes_per_run``,
    /// ``max_trace_events``, ``max_terminal_subagent_tasks``.
    /// Missing cap keys keep finite defaults unless ``unbounded=True``.
    #[getter]
    fn get_retention_limits(&self) -> Option<pyo3::PyObject> {
        pyo3::Python::with_gil(|py| self.retention_limits.as_ref().map(|o| o.clone_ref(py)))
    }

    #[setter]
    fn set_retention_limits(&mut self, value: Option<pyo3::PyObject>) {
        self.retention_limits = value;
    }

    /// Structured JSONL trajectory output path.
    ///
    /// When set, a3s-code records user input, LLM turns, tool calls, tool
    /// observations, token usage, and execution end status. This is the
    /// programmatic equivalent of ``A3S_CODE_TRAJECTORY_PATH``.
    #[getter]
    fn get_trajectory_path(&self) -> Option<String> {
        self.trajectory_path.clone()
    }

    #[setter]
    fn set_trajectory_path(&mut self, value: Option<String>) {
        self.trajectory_path = value;
    }

    /// Trajectory mode: ``"on"`` or ``"off"``.
    #[getter]
    fn get_trajectory_mode(&self) -> Option<String> {
        self.trajectory_mode.clone()
    }

    #[setter]
    fn set_trajectory_mode(&mut self, value: Option<String>) {
        self.trajectory_mode = value;
    }

    /// Max bytes retained for any single text field before truncation.
    #[getter]
    fn get_trajectory_max_text_bytes(&self) -> Option<usize> {
        self.trajectory_max_text_bytes
    }

    #[setter]
    fn set_trajectory_max_text_bytes(&mut self, value: Option<usize>) {
        self.trajectory_max_text_bytes = value;
    }

    /// Whether LLM request records include full message arrays.
    #[getter]
    fn get_trajectory_include_messages(&self) -> Option<bool> {
        self.trajectory_include_messages
    }

    #[setter]
    fn set_trajectory_include_messages(&mut self, value: Option<bool>) {
        self.trajectory_include_messages = value;
    }

    /// Register an instruction skill programmatically.
    ///
    /// Instructions are injected into the system prompt at session start.
    /// Use this instead of skill files for simple, one-off guidance.
    ///
    /// Args:
    ///     name: Unique skill name (kebab-case recommended, e.g. "type-hints")
    ///     content: Markdown content describing the instruction
    fn add_instruction(&mut self, name: String, content: String) {
        self.inline_skills
            .push((name, "instruction".to_string(), content));
    }

    /// Register a persona skill programmatically.
    ///
    /// Personas replace the default role section of the system prompt.
    /// Only one persona is active at a time (last registered wins).
    ///
    /// Args:
    ///     name: Unique skill name (kebab-case recommended, e.g. "python-expert")
    ///     content: System prompt content for this persona
    fn add_persona(&mut self, name: String, content: String) {
        self.inline_skills
            .push((name, "persona".to_string(), content));
    }

    fn __repr__(&self) -> String {
        format!(
            "SessionOptions(model={:?}, builtin_skills={}, queue_config={}, auto_compact={}, max_context_tokens={:?}, artifact_store_limits={}, tool_result_transform_policy={}, tool_presentation_profile={}, memory_store={}, session_store={}, security_provider={}, workspace_backend={}, inline_skills={}, max_parallel_tasks={:?}, auto_parallel={:?})",
            self.model,
            self.builtin_skills,
            if self.queue_config.is_some() { "Some(...)" } else { "None" },
            self.auto_compact,
            self.max_context_tokens,
            if self.artifact_store_limits.is_some() { "Some(...)" } else { "None" },
            if self.tool_result_transform_policy.is_some() { "Some(...)" } else { "None" },
            if self.tool_presentation_profile.is_some() { "Some(...)" } else { "None" },
            if self.memory_store.is_some() { "Some(...)" } else { "None" },
            if self.session_store.is_some() { "Some(...)" } else { "None" },
            if self.security_provider.is_some() { "Some(...)" } else { "None" },
            if self.workspace_backend.is_some() { "Some(...)" } else { "None" },
            self.inline_skills.len(),
            self.max_parallel_tasks,
            self.auto_parallel,
        )
    }
}
