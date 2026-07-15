//! Native Rust implementations of all built-in tools
//!
//! These replace the previous `a3s-tools` binary backend with direct Rust
//! implementations that execute in-process. Each tool implements the `Tool` trait.

pub(crate) mod bash;
pub mod batch;
mod code_intelligence;
mod edit;
mod generate_object;
pub(crate) mod git;
mod glob_tool;
mod grep;
mod ls;
mod patch;
mod read;
mod web_fetch;
mod web_search;
mod write;

use super::registry::ToolRegistry;
use std::sync::Arc;

/// Normalize a source URL before it can enter durable tool/task metadata.
/// Credentials, query strings, and fragments are intentionally excluded.
pub(crate) fn safe_http_source_url(value: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str()?.is_empty() {
        return None;
    }
    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

/// Register all baseline built-in tools with the registry, gated by
/// workspace capabilities.
///
/// Tools whose required capability is missing are not registered, so the model
/// never sees a tool the backend cannot service. `web_fetch` and `web_search`
/// have no workspace capability and are always registered.
///
/// Note: `batch` is NOT registered here — it requires an `Arc<ToolRegistry>`
/// and must be registered after the registry is wrapped in an Arc.
pub fn register_builtins(
    registry: &ToolRegistry,
    workspace_services: &crate::workspace::WorkspaceServices,
) {
    let capabilities = workspace_services.capabilities();
    if capabilities.read {
        registry.register_builtin(Arc::new(read::ReadTool));
        registry.register_builtin(Arc::new(ls::LsTool));
    }
    if capabilities.write {
        registry.register_builtin(Arc::new(write::WriteTool));
    }
    if capabilities.read && capabilities.write {
        registry.register_builtin(Arc::new(edit::EditTool));
        registry.register_builtin(Arc::new(patch::PatchTool));
    }
    if capabilities.exec {
        registry.register_builtin(Arc::new(bash::BashTool));
    }
    if capabilities.search {
        registry.register_builtin(Arc::new(grep::GrepTool));
        registry.register_builtin(Arc::new(glob_tool::GlobTool));
    }
    if workspace_services.code_intelligence().is_some() {
        code_intelligence::register(registry);
    }
    if capabilities.git {
        registry.register_builtin(Arc::new(git::GitTool));
    }
    registry.register_builtin(Arc::new(web_fetch::WebFetchTool));
    registry.register_builtin(Arc::new(web_search::WebSearchTool::new()));
}

/// Register the batch tool. Must be called after the registry is wrapped in Arc.
pub fn register_batch(registry: &Arc<ToolRegistry>) {
    registry.register_builtin(Arc::new(batch::BatchTool::new(Arc::clone(registry))));
}

/// Register the programmatic tool calling wrapper.
pub fn register_program(registry: &Arc<ToolRegistry>) {
    register_program_with_catalog(
        registry,
        crate::program::ProgramCatalog::with_builtin_programs(),
    );
}

/// Register the programmatic tool calling wrapper with a custom catalog.
pub fn register_program_with_catalog(
    registry: &Arc<ToolRegistry>,
    catalog: crate::program::ProgramCatalog,
) {
    registry.register_builtin(Arc::new(crate::tools::ProgramTool::with_catalog(
        Arc::clone(registry),
        catalog,
    )));
}

/// Register the task delegation tools (task, parallel_task).
///
/// Must be called after the registry is wrapped in Arc. Requires an LLM client
/// and the workspace path so child agent loops can be spawned inline.
/// Optionally accepts an MCP manager so child sessions inherit MCP tools.
pub fn register_task(
    registry: &Arc<ToolRegistry>,
    llm_client: Arc<dyn crate::llm::LlmClient>,
    agent_registry: Arc<crate::subagent::AgentRegistry>,
    workspace: String,
) {
    register_task_with_mcp(
        registry,
        llm_client,
        agent_registry,
        workspace,
        None,
        None,
        None,
    );
}

/// Register the task delegation tools with optional MCP manager and parent context.
///
/// When `mcp_manager` is provided, delegated child sessions will have access
/// to all MCP tools from connected servers.
/// When `parent_context` is provided, child runs inherit parent capabilities.
/// When `subagent_tracker` is provided, each task registers a
/// `CancellationToken` against it so callers can cancel by `task_id`.
pub fn register_task_with_mcp(
    registry: &Arc<ToolRegistry>,
    llm_client: Arc<dyn crate::llm::LlmClient>,
    agent_registry: Arc<crate::subagent::AgentRegistry>,
    workspace: String,
    mcp_manager: Option<Arc<crate::mcp::manager::McpManager>>,
    parent_context: Option<crate::child_run::ChildRunContext>,
    subagent_tracker: Option<Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>>,
) {
    register_task_with_mcp_managers(
        registry,
        llm_client,
        agent_registry,
        workspace,
        mcp_manager.into_iter().collect(),
        parent_context,
        subagent_tracker,
    );
}

/// Register task delegation tools with ordered MCP capability sources.
///
/// Each manager keeps ownership of its own connections. Later sources shadow
/// earlier sources on identical fully-qualified tool names inside child runs.
pub fn register_task_with_mcp_managers(
    registry: &Arc<ToolRegistry>,
    llm_client: Arc<dyn crate::llm::LlmClient>,
    agent_registry: Arc<crate::subagent::AgentRegistry>,
    workspace: String,
    mcp_managers: Vec<Arc<crate::mcp::manager::McpManager>>,
    parent_context: Option<crate::child_run::ChildRunContext>,
    subagent_tracker: Option<Arc<crate::subagent_task_tracker::InMemorySubagentTaskTracker>>,
) {
    use crate::tools::task::{ParallelTaskTool, TaskExecutor, TaskTool};
    let mut executor =
        TaskExecutor::with_mcp_managers(agent_registry, llm_client, workspace, mcp_managers);
    if let Some(ctx) = parent_context {
        executor = executor.with_parent_context(ctx);
    }
    if let Some(tracker) = subagent_tracker {
        executor = executor.with_subagent_tracker(tracker);
    }
    let executor = Arc::new(executor);
    registry.register_builtin(Arc::new(TaskTool::new(Arc::clone(&executor))));
    registry.register_builtin(Arc::new(ParallelTaskTool::new(Arc::clone(&executor))));
}

/// Register the Skill tool for skill-based tool access control.
pub(crate) fn register_skill(
    registry: &Arc<ToolRegistry>,
    llm_client: Arc<dyn crate::llm::LlmClient>,
    skill_registry: Arc<crate::skills::SkillRegistry>,
    tool_executor: Arc<crate::tools::ToolExecutor>,
    base_config: crate::agent::AgentConfig,
) {
    use crate::tools::skill::{SearchSkillsTool, SkillTool};
    registry.register_builtin(Arc::new(SearchSkillsTool::new(Arc::clone(&skill_registry))));
    registry.register_builtin(Arc::new(SkillTool::new(
        skill_registry,
        llm_client,
        tool_executor,
        base_config,
    )));
}

/// Register the `generate_object` tool for structured JSON output.
///
/// Must be called after the registry is wrapped in Arc. Requires an LLM client
/// so the tool can make its own LLM calls for object generation.
pub fn register_generate_object(
    registry: &Arc<ToolRegistry>,
    llm_client: Arc<dyn crate::llm::LlmClient>,
) {
    registry.register_builtin(Arc::new(generate_object::GenerateObjectTool::new(
        llm_client,
    )));
}

#[cfg(test)]
mod tests {
    use super::safe_http_source_url;

    #[test]
    fn safe_source_url_removes_credentials_query_and_fragment() {
        assert_eq!(
            safe_http_source_url(
                "HTTPS://user:password@Example.COM/report?access_token=secret#section"
            )
            .as_deref(),
            Some("https://example.com/report")
        );
        assert!(safe_http_source_url("file:///tmp/source").is_none());
    }
}
