//! Native Rust implementations of all built-in tools
//!
//! These replace the previous `a3s-tools` binary backend with direct Rust
//! implementations that execute in-process. Each tool implements the `Tool` trait.

mod bash;
mod edit;
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

/// Register all built-in tools with the registry.
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
}
