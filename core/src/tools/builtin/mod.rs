//! Native Rust implementations of all built-in tools
//!
//! These replace the previous `a3s-tools` binary backend with direct Rust
//! implementations that execute in-process. Each tool implements the `Tool` trait.

mod bash;
pub mod codesearch;
mod edit;
mod glob_tool;
mod grep;
mod ls;
mod patch;
mod read;
#[cfg(feature = "sandbox")]
mod sandbox_tool;
mod web_fetch;
mod web_search;
mod write;

use super::registry::ToolRegistry;
use std::sync::Arc;

/// Register all built-in tools with the registry.
///
/// The `sandbox` tool is only registered when the `sandbox` Cargo feature is
/// enabled. All other tools are always registered.
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

    // Register sandbox tool only when A3S Box feature is enabled.
    #[cfg(feature = "sandbox")]
    registry.register_builtin(Arc::new(sandbox_tool::SandboxTool::new()));
}
