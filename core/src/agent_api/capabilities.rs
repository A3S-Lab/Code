//! Session capability wiring.
//!
//! This module owns the model-visible and context-visible capability set for a
//! session: built-in tools, delegated agent tools, MCP tools, workspace
//! instructions, and skills. The `Agent` facade passes configuration in and gets
//! back a ready-to-wire capability set.

use super::SessionOptions;
use crate::agent::AgentConfig;
use crate::config::CodeConfig;
use crate::context::{ContextProvider, SkillCatalogContextProvider, StaticContextProvider};
use crate::llm::{LlmClient, ToolDefinition};
use crate::mcp::McpTool;
use crate::skills::SkillRegistry;
use crate::subagent::AgentRegistry;
use crate::tools::ToolExecutor;
use crate::{CodeError, Result, SessionBuildResource};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) struct SessionCapabilityInput<'a> {
    pub(super) code_config: &'a CodeConfig,
    pub(super) base_config: &'a AgentConfig,
    pub(super) workspace: &'a Path,
    pub(super) llm_client: Arc<dyn LlmClient>,
    pub(super) opts: &'a SessionOptions,
    pub(super) mcp_sources: Vec<super::session_config::ResolvedMcpSource>,
}

pub(super) struct SessionCapabilities {
    pub(super) tool_executor: Arc<ToolExecutor>,
    pub(super) trace_sink: crate::trace::InMemoryTraceSink,
    pub(super) tool_defs: Vec<ToolDefinition>,
    pub(super) context_providers: Vec<Arc<dyn ContextProvider>>,
    pub(super) skill_registry: Arc<SkillRegistry>,
    pub(super) agent_registry: Arc<AgentRegistry>,
    pub(super) subagent_tasks: Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>,
    pub(super) workspace_retrieval: Option<Arc<crate::workspace::WorkspaceRetrievalRuntime>>,
    pub(super) owned_workspace_backend: Option<Arc<crate::workspace::ManifestWorkspaceBackend>>,
}

struct ResolvedWorkspaceServices {
    services: Arc<crate::workspace::WorkspaceServices>,
    retrieval: Option<Arc<crate::workspace::WorkspaceRetrievalRuntime>>,
    owned_backend: Option<Arc<crate::workspace::ManifestWorkspaceBackend>>,
}

pub(super) fn build_session_capabilities(
    input: SessionCapabilityInput<'_>,
    session_lifetime: tokio_util::sync::CancellationToken,
) -> Result<SessionCapabilities> {
    let artifact_limits = input.opts.artifact_store_limits.unwrap_or_default();
    let retention_limits = input.opts.retention_limits.unwrap_or_default();
    let workspace = resolve_workspace_services(&input, session_lifetime)?;
    let workspace_services = workspace.services;
    let tool_executor = Arc::new(
        ToolExecutor::new_with_workspace_services_and_artifact_limits(
            input.workspace.display().to_string(),
            workspace_services,
            artifact_limits,
        ),
    );
    tool_executor
        .registry()
        .set_tool_result_transform_policy(
            input
                .opts
                .tool_result_transform_policy
                .clone()
                .unwrap_or_default(),
        )
        .expect("resolved Tool result transform policy must be valid");
    let trace_sink = match retention_limits.max_trace_events {
        Some(cap) => crate::trace::InMemoryTraceSink::with_max_events(cap),
        None => crate::trace::InMemoryTraceSink::new(),
    };
    tool_executor.set_trace_sink(Arc::new(trace_sink.clone()));

    if let Some(ref search_config) = input.code_config.search {
        tool_executor
            .registry()
            .set_search_config(search_config.clone());
    }

    let subagent_tasks = Arc::new(match retention_limits.max_terminal_subagent_tasks {
        Some(cap) => {
            crate::subagent_task_tracker::InMemorySubagentTaskTracker::with_max_terminal_tasks(cap)
        }
        None => crate::subagent_task_tracker::InMemorySubagentTaskTracker::new(),
    });
    let mcp_managers = input
        .mcp_sources
        .iter()
        .map(|source| Arc::clone(&source.manager))
        .collect();
    let skill_registry =
        build_effective_skill_registry(input.base_config.skill_registry.as_deref(), input.opts);
    let agent_registry = register_task_capability(
        &input,
        &tool_executor,
        Arc::clone(&subagent_tasks),
        mcp_managers,
        Arc::clone(&skill_registry),
    );

    // Register generate_object tool (structured JSON output)
    crate::tools::register_generate_object(tool_executor.registry(), Arc::clone(&input.llm_client));

    register_mcp_capabilities(&tool_executor, input.mcp_sources);

    let context_providers = build_context_providers(
        input.code_config,
        input.opts,
        input.workspace,
        Arc::clone(&skill_registry),
    );
    let tool_defs = tool_executor.definitions();

    Ok(SessionCapabilities {
        tool_executor,
        trace_sink,
        tool_defs,
        context_providers,
        skill_registry,
        agent_registry,
        subagent_tasks,
        workspace_retrieval: workspace.retrieval,
        owned_workspace_backend: workspace.owned_backend,
    })
}

fn resolve_workspace_services(
    input: &SessionCapabilityInput<'_>,
    session_lifetime: tokio_util::sync::CancellationToken,
) -> Result<ResolvedWorkspaceServices> {
    let (mut services, owned_workspace_backend) = match &input.opts.workspace_services {
        Some(services) => (Arc::clone(services), None),
        None if input.opts.workspace_retrieval.is_some() => {
            let backend = crate::workspace::ManifestWorkspaceBackend::new(input.workspace);
            (
                crate::workspace::WorkspaceServices::local_with_retrieval_backend(Arc::clone(
                    &backend,
                )),
                Some(backend),
            )
        }
        None => (
            crate::workspace::WorkspaceServices::local(input.workspace),
            None,
        ),
    };
    if services.workspace_retrieval().is_some() {
        return Err(CodeError::SessionConfiguration {
            field: "workspace_services",
            message: "workspace services already belong to another semantic retrieval runtime"
                .to_owned(),
        });
    }

    let Some(options) = input.opts.workspace_retrieval.clone() else {
        return Ok(ResolvedWorkspaceServices {
            services,
            retrieval: None,
            owned_backend: owned_workspace_backend,
        });
    };
    if !services.capabilities().read {
        return Err(CodeError::SessionConfiguration {
            field: "workspace_retrieval",
            message: "semantic retrieval requires workspace read capability".to_owned(),
        });
    }
    let catalog = services
        .chunk_catalog()
        .ok_or_else(|| CodeError::SessionConfiguration {
            field: "workspace_retrieval",
            message: "semantic retrieval requires workspace services with a chunk catalog"
                .to_owned(),
        })?;
    let runtime =
        crate::workspace::WorkspaceRetrievalRuntime::start(catalog, options, session_lifetime)
            .map_err(|error| CodeError::SessionInitialization {
                resource: SessionBuildResource::WorkspaceRetrieval,
                message: error.to_string(),
            })?;
    services = services
        .with_workspace_retrieval(Arc::clone(&runtime))
        .ok_or_else(|| CodeError::SessionConfiguration {
            field: "workspace_services",
            message: "workspace services already belong to another semantic retrieval runtime"
                .to_owned(),
        })?;
    Ok(ResolvedWorkspaceServices {
        services,
        retrieval: Some(runtime),
        owned_backend: owned_workspace_backend,
    })
}

pub(super) fn register_skill_capability(
    tool_executor: Arc<ToolExecutor>,
    llm_client: Arc<dyn LlmClient>,
    skill_registry: Arc<SkillRegistry>,
    config: AgentConfig,
) {
    let registry = Arc::clone(tool_executor.registry());
    crate::tools::register_skill(&registry, llm_client, skill_registry, tool_executor, config);
}

pub(super) fn build_effective_skill_registry(
    agent_registry: Option<&SkillRegistry>,
    opts: &SessionOptions,
) -> Arc<SkillRegistry> {
    let base_registry = agent_registry
        .map(|r| r.fork())
        .unwrap_or_else(SkillRegistry::with_builtins);

    if let Some(ref registry) = opts.skill_registry {
        for skill in registry.all() {
            base_registry.register_unchecked(skill);
        }
    }

    for dir in &opts.skill_dirs {
        if let Err(e) = base_registry.load_from_dir(dir) {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "Failed to load session skill dir - skipping"
            );
        }
    }

    Arc::new(base_registry)
}

fn register_task_capability(
    input: &SessionCapabilityInput<'_>,
    tool_executor: &Arc<ToolExecutor>,
    subagent_tasks: Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>,
    mcp_managers: Vec<Arc<crate::mcp::manager::McpManager>>,
    skill_registry: Arc<SkillRegistry>,
) -> Arc<AgentRegistry> {
    use crate::child_run::ChildRunContext;
    use crate::subagent::load_agents_from_dir;
    use crate::tools::register_task_with_mcp_managers;

    let registry = AgentRegistry::new();
    let auto_delegation =
        super::session_config::resolve_auto_delegation_config(input.code_config, input.opts);
    let built_in_agent_dirs = built_in_agent_dirs(input.workspace);
    for dir in input
        .code_config
        .agent_dirs
        .iter()
        .chain(built_in_agent_dirs.iter())
        .chain(input.opts.agent_dirs.iter())
    {
        for agent in load_agents_from_dir(dir) {
            registry.register(agent);
        }
    }
    for worker in &input.opts.worker_agents {
        registry.register_worker(worker.clone());
    }

    if !auto_delegation.allow_manual_delegation {
        // Keep the registry populated for introspection and host-managed worker
        // registration even when the model-visible delegation tools are hidden.
        return Arc::new(registry);
    }

    let parent_context = ChildRunContext {
        security_provider: input.opts.security_provider.clone(),
        hook_engine: None,
        skill_registry: Some(skill_registry),
        permission_checker: input.opts.permission_checker.clone(),
        permission_policy: input.opts.permission_policy.clone(),
        tool_timeout_ms: input.opts.tool_timeout_ms,
        llm_api_timeout_ms: input
            .opts
            .llm_api_timeout_ms
            .or(input.code_config.llm_api_timeout_ms),
        max_parallel_tasks: input
            .opts
            .max_parallel_tasks
            .or(input.code_config.max_parallel_tasks),
        max_execution_time_ms: input.opts.max_execution_time_ms,
        circuit_breaker_threshold: input.opts.circuit_breaker_threshold,
        duplicate_tool_call_threshold: input.opts.duplicate_tool_call_threshold,
        confirmation_manager: input.opts.confirmation_manager.clone(),
        enforce_active_skill_tool_restrictions: input.opts.enforce_active_skill_tool_restrictions,
        workspace_services: Some(tool_executor.registry().context().workspace_services),
        sandbox_handle: input.opts.sandbox_handle.clone(),
        budget_guard: input.opts.budget_guard.clone(),
    };

    let registry = Arc::new(registry);
    register_task_with_mcp_managers(
        tool_executor.registry(),
        Arc::clone(&input.llm_client),
        Arc::clone(&registry),
        input.workspace.display().to_string(),
        mcp_managers,
        Some(parent_context),
        Some(subagent_tasks),
    );
    registry
}

fn built_in_agent_dirs(workspace: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        dirs.push(home.join(".claude").join("agents"));
        dirs.push(home.join(".a3s").join("agents"));
    }
    dirs.push(workspace.join(".claude").join("agents"));
    dirs.push(workspace.join(".a3s").join("agents"));
    dirs
}

fn register_mcp_capabilities(
    tool_executor: &Arc<ToolExecutor>,
    sources: Vec<super::session_config::ResolvedMcpSource>,
) {
    // Global sources are registered first. A session source with the same
    // fully-qualified tool name intentionally shadows it only in this session.
    for source in sources {
        for (server_name, tools) in group_mcp_tools_by_server(source.tools) {
            for tool in crate::mcp::tools::create_mcp_tools(
                &server_name,
                tools,
                Arc::clone(&source.manager),
            ) {
                tool_executor.register_dynamic_tool(tool);
            }
        }
    }
}

fn group_mcp_tools_by_server(all_tools: Vec<(String, McpTool)>) -> HashMap<String, Vec<McpTool>> {
    let mut by_server = HashMap::new();
    for (server, tool) in all_tools {
        by_server.entry(server).or_insert_with(Vec::new).push(tool);
    }
    by_server
}

fn build_context_providers(
    code_config: &CodeConfig,
    opts: &SessionOptions,
    workspace: &Path,
    skill_registry: Arc<SkillRegistry>,
) -> Vec<Arc<dyn ContextProvider>> {
    let mut providers = opts.context_providers.clone();
    if let Some(cognitive_context) = &opts.cognitive_context {
        providers.push(Arc::new(cognitive_context.clone()));
    }
    push_agents_md_context(&mut providers, code_config, workspace);
    push_skill_catalog_context(&mut providers, skill_registry);
    providers
}

fn push_agents_md_context(
    providers: &mut Vec<Arc<dyn ContextProvider>>,
    code_config: &CodeConfig,
    workspace: &Path,
) {
    let Some(item) = super::project_instructions::load_context_item(code_config, workspace) else {
        return;
    };
    providers.push(Arc::new(
        StaticContextProvider::new("agents_md").with_item(item),
    ));
}

fn push_skill_catalog_context(
    providers: &mut Vec<Arc<dyn ContextProvider>>,
    skill_registry: Arc<SkillRegistry>,
) {
    providers.push(Arc::new(SkillCatalogContextProvider::new(skill_registry)));
}
