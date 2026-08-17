//! Conversion from Python session options to Core session options.

use super::session_options::PySessionOptions;
use super::*;

// ============================================================================
// Conversion
// ============================================================================

pub(super) fn parse_planning_mode(mode: &str) -> PyResult<RustPlanningMode> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(RustPlanningMode::Auto),
        "enabled" | "enable" | "on" | "force" | "forced" | "true" => Ok(RustPlanningMode::Enabled),
        "disabled" | "disable" | "off" | "false" => Ok(RustPlanningMode::Disabled),
        _ => Err(PyValueError::new_err(format!(
            "Invalid planning_mode '{}'. Expected 'auto', 'enabled', or 'disabled'",
            mode
        ))),
    }
}

pub(super) fn apply_planning_mode(
    opts: RustSessionOptions,
    planning_mode: Option<&str>,
    planning: Option<bool>,
) -> PyResult<RustSessionOptions> {
    if let Some(mode) = planning_mode {
        Ok(opts.with_planning_mode(parse_planning_mode(mode)?))
    } else if let Some(enabled) = planning {
        Ok(opts.with_planning(enabled))
    } else {
        Ok(opts)
    }
}

/// Build options for synchronous SDK entry points.
///
/// A host callback-backed retrieval provider must be bound to a live asyncio
/// loop, so retrieval-enabled sessions deliberately use the async conversion
/// entry point below.
pub(super) fn build_rust_session_options(so: PySessionOptions) -> PyResult<RustSessionOptions> {
    build_rust_session_options_inner(so, None)
}

/// Build options while binding any host embedding callback to the caller's
/// currently running asyncio loop.
pub(super) fn build_rust_session_options_async(
    py: Python<'_>,
    so: PySessionOptions,
) -> PyResult<RustSessionOptions> {
    let event_loop = if so.workspace_retrieval.is_some() {
        Some(
            py.import("asyncio")?
                .call_method0("get_running_loop")?
                .unbind(),
        )
    } else {
        None
    };
    build_rust_session_options_inner(so, event_loop)
}

fn build_rust_session_options_inner(
    so: PySessionOptions,
    event_loop: Option<PyObject>,
) -> PyResult<RustSessionOptions> {
    let mut o = RustSessionOptions::new();
    if let Some(m) = so.model {
        o = o.with_model(m);
    }
    if let Some(priority) = so.task_priority {
        o = o.with_task_priority(priority.parse().map_err(
            |error: a3s_code_core::TaskSchedulerError| PyValueError::new_err(error.to_string()),
        )?);
    }
    if so.builtin_skills {
        o = o.with_builtin_skills();
    }
    for d in &so.skill_dirs {
        o = o.with_skills_from_dir(d);
    }
    if let Some(enabled) = so.enforce_active_skill_tool_restrictions {
        o = o.with_active_skill_tool_restrictions(enabled);
    }
    for d in &so.agent_dirs {
        o = o.with_agent_dir(d);
    }
    for worker in so.worker_agents {
        o = o.with_worker_agent(py_worker_agent_spec_to_rust(worker)?);
    }
    if let Some(qc) = so.queue_config {
        o = o.with_queue_config(qc.inner);
    }
    if let Some(policy) = so.permission_policy {
        o = o.with_permission_policy(py_permission_policy_to_rust(policy)?);
    }
    if let Some(policy) = so.confirmation_policy {
        o = o.with_confirmation_policy(py_confirmation_policy_to_rust(policy)?);
    }
    if so.auto_compact {
        o = o.with_auto_compact(true);
    }
    if let Some(t) = so.auto_compact_threshold {
        o = o.with_auto_compact_threshold(t);
    }
    if let Some(tokens) = so.max_context_tokens {
        o = o.with_max_context_tokens(tokens);
    }
    if let Some(limits) = so.artifact_store_limits {
        o = o.with_artifact_store_limits(limits.into());
    }
    if let Some(policy) = so.tool_result_transform_policy {
        o = o.with_tool_result_transform_policy(policy.into());
    }
    if let Some(ref store) = so.memory_store {
        let dir = Python::with_gil(|py| {
            store
                .extract::<pyo3::PyRef<PyFileMemoryStore>>(py)
                .ok()
                .map(|s| s.dir.clone())
        });
        let dir = dir.ok_or_else(|| {
            PyTypeError::new_err("memory_store must be a FileMemoryStore instance")
        })?;
        o = o.with_file_memory(dir);
    }
    if let Some(ref store) = so.session_store {
        enum SessionStoreKind {
            File(String),
            Memory(Arc<a3s_code_core::store::MemorySessionStore>),
        }
        let kind = Python::with_gil(|py| {
            if let Ok(file_store) = store.extract::<pyo3::PyRef<PyFileSessionStore>>(py) {
                Some(SessionStoreKind::File(file_store.dir.clone()))
            } else if let Ok(memory_store) = store.extract::<pyo3::PyRef<PyMemorySessionStore>>(py)
            {
                Some(SessionStoreKind::Memory(Arc::clone(&memory_store.inner)))
            } else {
                None
            }
        });
        match kind {
            Some(SessionStoreKind::File(dir)) => {
                o = o.with_file_session_store(dir);
            }
            Some(SessionStoreKind::Memory(memory_store)) => {
                let s: Arc<dyn a3s_code_core::store::SessionStore> = memory_store;
                o = o.with_session_store(s);
            }
            None => {
                return Err(PyTypeError::new_err(
                    "session_store must be a FileSessionStore or MemorySessionStore instance",
                ));
            }
        }
    }
    if let Some(ref sec) = so.security_provider {
        let is_default = Python::with_gil(|py| {
            sec.extract::<pyo3::PyRef<PyDefaultSecurityProvider>>(py)
                .is_ok()
        });
        if !is_default {
            return Err(PyTypeError::new_err(
                "security_provider must be a DefaultSecurityProvider instance",
            ));
        }
        o = o.with_default_security();
    }
    if let Some(ref backend) = so.workspace_backend {
        // S3BackendConfig is significantly larger than the other variants;
        // box it to avoid a `clippy::large_enum_variant` warning.
        enum BackendKind {
            Local(String),
            S3(Box<a3s_code_core::S3BackendConfig>),
            Unknown,
        }
        let resolved = Python::with_gil(|py| -> BackendKind {
            if let Ok(local) = backend.extract::<pyo3::PyRef<PyLocalWorkspaceBackend>>(py) {
                return BackendKind::Local(local.root.clone());
            }
            if let Ok(s3) = backend.extract::<pyo3::PyRef<PyS3WorkspaceBackend>>(py) {
                return BackendKind::S3(Box::new(s3.to_core()));
            }
            BackendKind::Unknown
        });
        let services = match resolved {
            BackendKind::Local(root) => a3s_code_core::WorkspaceServices::local(root),
            BackendKind::S3(cfg) => a3s_code_core::WorkspaceServices::s3(*cfg),
            BackendKind::Unknown => {
                return Err(PyTypeError::new_err(
                    "workspace_backend must be a LocalWorkspaceBackend or S3WorkspaceBackend instance",
                ));
            }
        };
        let services = if let Some(ref git_cfg) = so.remote_git {
            services
                .with_remote_git(git_cfg.to_core())
                .map_err(|e| PyValueError::new_err(format!("remote_git: {e}")))?
        } else {
            services
        };
        o = o.with_workspace_backend(services);
    } else if so.remote_git.is_some() {
        return Err(PyValueError::new_err(
            "remote_git requires workspace_backend to be set; assign a LocalWorkspaceBackend or S3WorkspaceBackend first",
        ));
    }
    if let Some(ref retrieval) = so.workspace_retrieval {
        let event_loop = event_loop.ok_or_else(|| {
            PyRuntimeError::new_err(
                "workspace_retrieval requires session_async(), resume_session_async(), or replace_session_async() so the embedding callback can bind to a running asyncio loop",
            )
        })?;
        let retrieval =
            Python::with_gil(|py| retrieval_options_to_core(py, retrieval, event_loop))?;
        o = o.with_workspace_retrieval(retrieval);
    }
    // Build prompt slots if any slot is set
    if so.role.is_some()
        || so.guidelines.is_some()
        || so.response_style.is_some()
        || so.extra.is_some()
    {
        let slots = a3s_code_core::SystemPromptSlots {
            style: None,
            role: so.role,
            guidelines: so.guidelines,
            response_style: so.response_style,
            extra: so.extra,
        };
        o = o.with_prompt_slots(slots);
    }
    // Inline skills registered programmatically via add_instruction / add_persona
    if !so.inline_skills.is_empty() {
        let registry = a3s_code_core::skills::SkillRegistry::new();
        for (name, kind, content) in so.inline_skills {
            registry.register_unchecked(inline_skill_to_rust(name, content, &kind)?);
        }
        o = o.with_skill_registry(Arc::new(registry));
    }
    if let Some(r) = so.max_tool_rounds {
        o = o.with_max_tool_rounds(r);
    }
    if let Some(max_parallel_tasks) = so.max_parallel_tasks {
        o = o.with_max_parallel_tasks(max_parallel_tasks);
    }
    if let Some(auto_delegation) = so.auto_delegation {
        o = o.with_auto_delegation(auto_delegation.into());
    }
    if let Some(auto_parallel) = so.auto_parallel {
        o = o.with_auto_parallel_delegation(auto_parallel);
    }
    if let Some(manual_delegation_enabled) = so.manual_delegation_enabled {
        o = o.with_manual_delegation_enabled(manual_delegation_enabled);
    }
    o = apply_planning_mode(o, so.planning_mode.as_deref(), so.planning)?;
    if so.goal_tracking {
        o = o.with_goal_tracking(true);
    }
    if let Some(n) = so.max_parse_retries {
        o = o.with_parse_retries(n);
    }
    if let Some(ms) = so.tool_timeout_ms {
        o = o.with_tool_timeout(ms);
    }
    if let Some(ms) = so.llm_api_timeout_ms {
        o = o.with_llm_api_timeout(ms);
    }
    if let Some(n) = so.circuit_breaker_threshold {
        o = o.with_circuit_breaker(n);
    }
    if let Some(n) = so.duplicate_tool_call_threshold {
        o = o.with_duplicate_tool_call_threshold(n);
    }
    if let Some(t) = so.temperature {
        o = o.with_temperature(t);
    }
    if let Some(budget) = so.thinking_budget {
        o = o.with_thinking_budget(budget);
    }
    if let Some(enabled) = so.llm_logprobs {
        o = o.with_llm_logprobs(enabled);
    }
    if let Some(top_logprobs) = so.llm_top_logprobs {
        o = o.with_llm_top_logprobs(top_logprobs);
    }
    if let Some(enabled) = so.continuation_enabled {
        o = o.with_continuation(enabled);
    }
    if let Some(turns) = so.max_continuation_turns {
        o = o.with_max_continuation_turns(turns);
    }
    if let Some(timeout_ms) = so.max_execution_time_ms {
        o.max_execution_time_ms = Some(timeout_ms);
    }
    if let Some(id) = so.session_id {
        o = o.with_session_id(id);
    }
    if let Some(t) = so.tenant_id {
        o = o.with_tenant_id(t);
    }
    if let Some(p) = so.principal {
        o = o.with_principal(p);
    }
    if let Some(t) = so.agent_template_id {
        o = o.with_agent_template_id(t);
    }
    if let Some(c) = so.correlation_id {
        o = o.with_correlation_id(c);
    }
    if let Some(host_env) = so.host_env {
        use a3s_code_core::host_env::{
            Clock, FixedClock, HostEnv, IdGenerator, SequentialIdGenerator, SystemClock,
            SystemIdGenerator,
        };

        let id_generator: Arc<dyn IdGenerator> = if let Some(prefix) = host_env.sequential_id_prefix
        {
            Arc::new(SequentialIdGenerator::new(prefix))
        } else {
            Arc::new(SystemIdGenerator)
        };
        let clock: Arc<dyn Clock> = if let Some(now_ms) = host_env.fixed_time_ms {
            Arc::new(FixedClock::new(now_ms))
        } else {
            Arc::new(SystemClock)
        };
        o = o.with_host_env(Arc::new(HostEnv::new(id_generator, clock)));
    }
    if let Some(guard) = so.budget_guard {
        let wrapped: std::sync::Arc<dyn a3s_code_core::budget::BudgetGuard> =
            std::sync::Arc::new(PyBudgetGuard::new(guard));
        o = o.with_budget_guard(wrapped);
    }
    if let Some(retention) = so.retention_limits {
        if let Some(limits) = parse_py_retention_limits(&retention) {
            o = o.with_retention_limits(limits);
        }
    }
    if let Some(path) = so.trajectory_path {
        let mut config = a3s_code_core::RlTrajectoryConfig::new(path);
        if let Some(mode) = so.trajectory_mode {
            let parsed = a3s_code_core::RlTrajectoryMode::parse(&mode).ok_or_else(|| {
                PyValueError::new_err(format!("trajectory_mode must be 'on' or 'off', got {mode}"))
            })?;
            config = config.with_mode(parsed);
        }
        if let Some(max_bytes) = so.trajectory_max_text_bytes {
            config = config.with_max_text_bytes(max_bytes);
        }
        if let Some(include_messages) = so.trajectory_include_messages {
            config = config.with_include_messages(include_messages);
        }
        o = o.with_rl_trajectory(config);
    }
    if so.auto_save {
        o = o.with_auto_save(true);
    }

    Ok(o)
}
