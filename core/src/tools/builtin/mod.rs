//! Native Rust implementations of all built-in tools
//!
//! These replace the previous `a3s-tools` binary backend with direct Rust
//! implementations that execute in-process. Each tool implements the `Tool` trait.
//!
//! ## Plugin tools
//!
//! `agentic_search` and `agentic_parse` are NOT registered by default.
//! They are opt-in plugins: users enable them via `SessionOptions` or by
//! calling `register_agentic_tools` directly.

pub(crate) mod agentic_parse;
pub(crate) mod agentic_search;
mod bash;
pub mod batch;
mod edit;
mod git_worktree;
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

/// Register all core built-in tools with the registry.
///
/// Does NOT include plugin tools (`agentic_search`, `agentic_parse`).
/// Those are opt-in — call `register_agentic_tools` to enable them.
///
/// Note: `batch` is NOT registered here — it requires an `Arc<ToolRegistry>`
/// and must be registered after the registry is wrapped in an Arc.
pub fn register_builtins(registry: &ToolRegistry) {
    registry.register_builtin(Arc::new(read::ReadTool));
    registry.register_builtin(Arc::new(write::WriteTool));
    registry.register_builtin(Arc::new(edit::EditTool));
    registry.register_builtin(Arc::new(patch::PatchTool));
    registry.register_builtin(Arc::new(bash::BashTool));
    registry.register_builtin(Arc::new(grep::GrepTool));
    registry.register_builtin(Arc::new(glob_tool::GlobTool));
    registry.register_builtin(Arc::new(ls::LsTool));
    registry.register_builtin(Arc::new(web_fetch::WebFetchTool));
    registry.register_builtin(Arc::new(web_search::WebSearchTool));
    registry.register_builtin(Arc::new(git_worktree::GitWorktreeTool));
}

/// Register the batch tool. Must be called after the registry is wrapped in Arc.
pub fn register_batch(registry: &Arc<ToolRegistry>) {
    registry.register_builtin(Arc::new(batch::BatchTool::new(Arc::clone(registry))));
}

/// Register the agentic plugin tools (`agentic_search`, `agentic_parse`).
///
/// These are opt-in — they are NOT part of the default toolset.
///
/// - `agentic_search`: multi-phase semantic code search
/// - `agentic_parse`: LLM-enhanced document parsing
///
/// `llm` is required by `agentic_parse` for its extraction pass.
/// Pass `None` to register only `agentic_search`.
pub fn register_agentic_tools(
    registry: &Arc<ToolRegistry>,
    llm: Option<Arc<dyn crate::llm::LlmClient>>,
) {
    registry.register_builtin(Arc::new(agentic_search::AgenticSearchTool::new()));
    if let Some(llm_client) = llm {
        registry.register_builtin(Arc::new(agentic_parse::AgenticParseTool::new(llm_client)));
    }
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
    register_task_with_mcp(registry, llm_client, agent_registry, workspace, None);
}

/// Register the task delegation tools with optional MCP manager.
///
/// When `mcp_manager` is provided, child subagent sessions will have access
/// to all MCP tools from connected servers.
pub fn register_task_with_mcp(
    registry: &Arc<ToolRegistry>,
    llm_client: Arc<dyn crate::llm::LlmClient>,
    agent_registry: Arc<crate::subagent::AgentRegistry>,
    workspace: String,
    mcp_manager: Option<Arc<crate::mcp::manager::McpManager>>,
) {
    use crate::tools::task::{ParallelTaskTool, RunTeamTool, TaskExecutor, TaskTool};
    let executor = Arc::new(match mcp_manager {
        Some(mcp) => TaskExecutor::with_mcp(agent_registry, llm_client, workspace, mcp),
        None => TaskExecutor::new(agent_registry, llm_client, workspace),
    });
    registry.register_builtin(Arc::new(TaskTool::new(Arc::clone(&executor))));
    registry.register_builtin(Arc::new(ParallelTaskTool::new(Arc::clone(&executor))));
    registry.register_builtin(Arc::new(RunTeamTool::new(executor)));
}

/// Register the Skill tool for skill-based tool access control.
pub fn register_skill(
    registry: &Arc<ToolRegistry>,
    llm_client: Arc<dyn crate::llm::LlmClient>,
    skill_registry: Arc<crate::skills::SkillRegistry>,
    tool_executor: Arc<crate::tools::ToolExecutor>,
    base_config: crate::agent::AgentConfig,
) {
    use crate::tools::skill::SkillTool;
    registry.register_builtin(Arc::new(SkillTool::new(
        skill_registry,
        llm_client,
        tool_executor,
        base_config,
    )));
}
