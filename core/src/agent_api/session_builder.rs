//! Session construction runtime.
//!
//! This module owns the harness assembly path for a workspace-bound session:
//! capabilities, runtime wiring, memory, persistence, and live control state.

use super::{safe_canonicalize, Agent, AgentSession, SessionOptions};
use crate::agent::AgentConfig;
use crate::commands::CommandRegistry;
use crate::error::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use super::capabilities::{
    build_session_capabilities, register_skill_capability, SessionCapabilities,
    SessionCapabilityInput,
};
use super::session_config::{resolve_auto_delegation_config, ResolvedSessionConfig};
use super::session_runtime::{
    build_session_runtime, build_session_runtime_sync, SessionRuntime, SessionRuntimeInput,
};

pub(super) fn prepare_session_options(agent: &Agent, opts: SessionOptions) -> SessionOptions {
    let mut opts = opts;
    if opts.session_id.is_none() {
        // Use the host-provided ID generator if one was supplied via
        // SessionOptions — this is the entry point that enables
        // deterministic-replay tooling to pin session ids.
        let env = opts
            .host_env
            .clone()
            .unwrap_or_else(|| Arc::clone(&agent.config.host_env));
        opts.session_id = Some(env.next_id());
    }
    opts
}

pub(super) async fn build_agent_session(
    agent: &Agent,
    workspace: String,
    resolved: ResolvedSessionConfig,
) -> Result<AgentSession> {
    let canonical = safe_canonicalize(Path::new(&workspace));
    let session_cancel = tokio_util::sync::CancellationToken::new();
    let capabilities =
        build_resolved_capabilities(agent, &canonical, &resolved, session_cancel.clone())?;
    let session_id = resolved_session_id(&resolved);
    let runtime = match build_session_runtime(SessionRuntimeInput {
        code_config: &agent.code_config,
        workspace: &canonical,
        session_id: &session_id,
        opts: &resolved.options,
        tool_executor: Arc::clone(&capabilities.tool_executor),
    })
    .await
    {
        Ok(runtime) => runtime,
        Err(error) => {
            cleanup_unfinished_capabilities(&capabilities, &session_cancel).await;
            return Err(error);
        }
    };
    let session = finish_agent_session(
        agent,
        canonical,
        resolved,
        capabilities,
        runtime,
        session_cancel,
    )?;
    fire_session_start(&session).await;
    Ok(session)
}

async fn cleanup_unfinished_capabilities(
    capabilities: &SessionCapabilities,
    session_cancel: &tokio_util::sync::CancellationToken,
) {
    session_cancel.cancel();
    if let Some(retrieval) = &capabilities.workspace_retrieval {
        retrieval.close().await;
    }
    if let Some(backend) = &capabilities.owned_workspace_backend {
        backend.shutdown();
    }
}

pub(super) fn build_agent_session_sync(
    agent: &Agent,
    workspace: String,
    resolved: ResolvedSessionConfig,
) -> Result<AgentSession> {
    let canonical = safe_canonicalize(Path::new(&workspace));
    let session_cancel = tokio_util::sync::CancellationToken::new();
    let capabilities =
        build_resolved_capabilities(agent, &canonical, &resolved, session_cancel.clone())?;
    let session_id = resolved_session_id(&resolved);
    let runtime = build_session_runtime_sync(SessionRuntimeInput {
        code_config: &agent.code_config,
        workspace: &canonical,
        session_id: &session_id,
        opts: &resolved.options,
        tool_executor: Arc::clone(&capabilities.tool_executor),
    });
    let session = finish_agent_session(
        agent,
        canonical,
        resolved,
        capabilities,
        runtime,
        session_cancel,
    )?;
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let executor = effective_hook_executor(&session);
        let event = session_start_event(&session);
        handle.spawn(async move {
            let _ = executor.fire(&event).await;
        });
    }
    Ok(session)
}

fn effective_hook_executor(session: &AgentSession) -> Arc<dyn crate::hooks::HookExecutor> {
    session
        .hook_executor
        .clone()
        .unwrap_or_else(|| Arc::clone(&session.hook_engine) as Arc<dyn crate::hooks::HookExecutor>)
}

fn session_start_event(session: &AgentSession) -> crate::hooks::HookEvent {
    let (provider, model) = session
        .model_name
        .split_once('/')
        .unwrap_or(("", session.model_name.as_str()));
    crate::hooks::HookEvent::SessionStart(crate::hooks::SessionStartEvent {
        session_id: session.session_id.clone(),
        system_prompt: None,
        model_provider: provider.to_string(),
        model_name: model.to_string(),
    })
}

async fn fire_session_start(session: &AgentSession) {
    let executor = effective_hook_executor(session);
    let event = session_start_event(session);
    let _ = executor.fire(&event).await;
}

fn resolved_session_id(resolved: &ResolvedSessionConfig) -> String {
    resolved.session_id.clone()
}

fn build_resolved_capabilities(
    agent: &Agent,
    canonical: &Path,
    resolved: &ResolvedSessionConfig,
    session_lifetime: tokio_util::sync::CancellationToken,
) -> Result<SessionCapabilities> {
    let opts = &resolved.options;
    build_session_capabilities(
        SessionCapabilityInput {
            code_config: &agent.code_config,
            base_config: &agent.config,
            workspace: canonical,
            llm_client: Arc::clone(&resolved.llm_client),
            opts,
            mcp_sources: resolved.mcp_sources.clone(),
        },
        session_lifetime,
    )
}

fn finish_agent_session(
    agent: &Agent,
    canonical: std::path::PathBuf,
    resolved: ResolvedSessionConfig,
    capabilities: SessionCapabilities,
    runtime: SessionRuntime,
    session_cancel: tokio_util::sync::CancellationToken,
) -> Result<AgentSession> {
    let opts = &resolved.options;
    let llm_client = Arc::clone(&resolved.llm_client);
    let model_generation_admission =
        crate::llm::ModelGenerationAdmission::new(llm_client.model_generation_concurrency());
    let tool_executor = capabilities.tool_executor;
    let trace_sink = capabilities.trace_sink;
    let agent_registry = capabilities.agent_registry;
    let tool_defs = capabilities.tool_defs;
    let context_providers = capabilities.context_providers;
    let effective_registry = capabilities.skill_registry;
    let subagent_tasks = capabilities.subagent_tasks;
    let workspace_retrieval = capabilities.workspace_retrieval;
    let owned_workspace_backend = capabilities.owned_workspace_backend;

    let prompt_slots = opts
        .prompt_slots
        .clone()
        .unwrap_or_else(|| agent.config.prompt_slots.clone());

    let session_id = resolved.session_id.clone();

    let memory = Some(Arc::clone(&resolved.memory));

    let base = agent.config.clone();
    let auto_delegation = resolve_auto_delegation_config(&agent.code_config, opts);
    let rl_trajectory_recorder = resolved.rl_trajectory_recorder.clone();
    let config = AgentConfig {
        prompt_slots,
        tools: tool_defs,
        security_provider: opts.security_provider.clone(),
        permission_checker: opts.permission_checker.clone(),
        permission_policy: opts.permission_policy.clone(),
        confirmation_manager: runtime.confirmation_manager.clone(),
        confirmation_policy: opts.confirmation_policy.clone(),
        queue_config: resolved.queue_config.clone(),
        context_providers,
        planning_mode: opts.planning_mode,
        goal_tracking: opts.goal_tracking,
        skill_registry: Some(Arc::clone(&effective_registry)),
        enforce_active_skill_tool_restrictions: opts
            .enforce_active_skill_tool_restrictions
            .unwrap_or(base.enforce_active_skill_tool_restrictions),
        max_parse_retries: resolved.limits.max_parse_retries,
        tool_timeout_ms: resolved.limits.tool_timeout_ms,
        llm_api_timeout_ms: resolved.limits.llm_api_timeout_ms,
        circuit_breaker_threshold: resolved.limits.circuit_breaker_threshold,
        duplicate_tool_call_threshold: resolved.limits.duplicate_tool_call_threshold,
        auto_compact: opts.auto_compact,
        auto_compact_threshold: opts
            .auto_compact_threshold
            .unwrap_or(crate::store::DEFAULT_AUTO_COMPACT_THRESHOLD),
        max_context_tokens: resolved.limits.max_context_tokens,
        memory: memory.clone(),
        continuation_enabled: opts
            .continuation_enabled
            .unwrap_or(base.continuation_enabled),
        max_continuation_turns: opts
            .max_continuation_turns
            .unwrap_or(base.max_continuation_turns),
        max_tool_rounds: resolved.limits.max_tool_rounds,
        max_parallel_tasks: resolved.limits.max_parallel_tasks,
        auto_delegation,
        agent_registry: Some(Arc::clone(&agent_registry)),
        max_execution_time_ms: resolved.limits.max_execution_time_ms,
        budget_guard: opts.budget_guard.clone().or(base.budget_guard.clone()),
        rl_trajectory_recorder,
        host_env: opts
            .host_env
            .clone()
            .unwrap_or_else(|| Arc::clone(&base.host_env)),
        ..base
    };

    // Register Skill after config is built so it can spawn child loops with
    // the same harness configuration while applying skill-local restrictions.
    register_skill_capability(
        Arc::clone(&tool_executor),
        Arc::clone(&llm_client),
        Arc::clone(&effective_registry),
        config.clone(),
    );

    let command_queue = runtime.command_queue;
    let tool_context = runtime.tool_context;
    let session_store = resolved.session_store.clone();
    let mcp_managers = resolved
        .mcp_sources
        .iter()
        .map(|source| Arc::clone(&source.manager))
        .collect();
    let command_registry = CommandRegistry::new();

    let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel_token = Arc::new(tokio::sync::Mutex::new(None));
    let current_run_id = Arc::new(tokio::sync::Mutex::new(None));
    let run_admission = Arc::new(super::run_admission::RunAdmission::default());
    let run_store = Arc::new({
        let limits = resolved.limits.retention;
        crate::run::InMemoryRunStore::with_retention_limits(
            limits.and_then(|l| l.max_runs_retained),
            limits.and_then(|l| l.max_events_per_run),
            limits.and_then(|l| l.max_event_bytes_per_run),
        )
    });

    let hook_engine = Arc::new(crate::hooks::HookEngine::new());
    let capability_catalog = empty_capability_catalog()?;
    let lifecycle_hook_executor: Arc<dyn crate::hooks::HookExecutor> = opts
        .hook_executor
        .clone()
        .unwrap_or_else(|| Arc::clone(&hook_engine) as Arc<dyn crate::hooks::HookExecutor>);
    let close_handle = Arc::new(super::session_close::SessionCloseHandle {
        session_id: session_id.clone(),
        closed: Arc::clone(&closed),
        session_cancel: session_cancel.clone(),
        cancel_token: Arc::clone(&cancel_token),
        current_run_id: Arc::clone(&current_run_id),
        run_store: Arc::clone(&run_store),
        subagent_tasks: Arc::clone(&subagent_tasks),
        confirmation_manager: config.confirmation_manager.clone(),
        hook_executor: Some(lifecycle_hook_executor),
        started_at: std::time::Instant::now(),
        command_queue: command_queue.clone(),
        run_admission: Arc::clone(&run_admission),
        memory: memory.clone(),
        mcp_manager: Arc::clone(&resolved.mcp_manager),
        tool_executor: Arc::clone(&tool_executor),
        capability_catalog: Arc::clone(&capability_catalog),
        extension_mutation: tokio::sync::Mutex::new(()),
        immediate_extension_mutation: std::sync::Mutex::new(()),
        mcp_tool_ownership: std::sync::Mutex::new(
            super::session_extensions::SessionMcpToolOwnership::default(),
        ),
        skill_registry: Arc::clone(&effective_registry),
        skill_ownership: std::sync::Mutex::new(
            super::session_extensions::SessionSkillOwnership::default(),
        ),
        workspace_retrieval: workspace_retrieval.clone(),
        owned_workspace_backend,
    });

    let session = AgentSession {
        llm_client,
        model_generation_admission,
        task_scheduler: Arc::clone(&agent.task_scheduler),
        task_priority: opts.task_priority,
        tool_executor,
        capability_catalog,
        tool_context,
        memory: config.memory.clone(),
        config,
        tool_result_transform_policy: opts
            .tool_result_transform_policy
            .clone()
            .unwrap_or_default(),
        cognitive_context: opts.cognitive_context.clone(),
        workspace: canonical,
        session_id,
        history: Arc::new(RwLock::new(Vec::new())),
        run_admission,
        command_queue,
        session_store,
        persistence_state: Arc::new(RwLock::new(
            super::session_persistence::SessionPersistenceState::default(),
        )),
        auto_save: opts.auto_save,
        hook_engine,
        hook_executor: opts.hook_executor.clone(),
        init_warning: None,
        command_registry: std::sync::Mutex::new(command_registry),
        model_name: resolved.model_name.clone(),
        mcp_manager: Arc::clone(&resolved.mcp_manager),
        inherited_mcp_managers: resolved.inherited_mcp_managers.clone(),
        mcp_managers,
        agent_registry,
        cancel_token,
        current_run_id,
        run_store,
        subagent_tasks,
        active_tools: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        trace_sink,
        verification_reports: Arc::new(RwLock::new(Vec::new())),
        closed,
        session_cancel,
        workspace_retrieval,
        close_handle,
        tenant_id: opts.tenant_id.clone(),
        principal: opts.principal.clone(),
        agent_template_id: opts.agent_template_id.clone(),
        correlation_id: opts.correlation_id.clone(),
        runtime_budget_guard: std::sync::Mutex::new(None),
    };
    session.refresh_task_delegation_tools();
    Ok(session)
}

fn empty_capability_catalog() -> Result<Arc<crate::capability::CapabilityCatalog>> {
    let set = crate::capability::CapabilitySet::empty().map_err(|error| {
        crate::error::CodeError::SessionInitialization {
            resource: crate::error::SessionBuildResource::Capability,
            message: format!("failed to build the initial capability identity set: {error}"),
        }
    })?;
    let projection =
        crate::capability::CapabilityProjection::new(set, std::collections::BTreeMap::new())
            .map_err(|error| crate::error::CodeError::SessionInitialization {
                resource: crate::error::SessionBuildResource::Capability,
                message: format!("failed to build the initial capability projection: {error}"),
            })?;
    Ok(Arc::new(crate::capability::CapabilityCatalog::new(
        projection,
    )))
}
