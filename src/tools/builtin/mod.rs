//! Built-in tool implementations
//!
//! These are the core tools that come with the agent:
//! - bash: Execute shell commands
//! - read: Read file contents
//! - write: Write content to files
//! - edit: Edit files with string replacement
//! - grep: Search file contents
//! - glob: Find files by pattern
//! - ls: List directory contents

mod bash;
mod edit;
mod glob;
mod grep;
mod ls;
mod read;
mod write;

use super::registry::ToolRegistry;
use std::sync::Arc;

pub use bash::BashTool;
pub use edit::EditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use read::ReadTool;
pub use write::WriteTool;

/// Register all built-in tools with the registry
pub fn register_builtin_tools(registry: &ToolRegistry) {
    registry.register(Arc::new(BashTool));
    registry.register(Arc::new(ReadTool));
    registry.register(Arc::new(WriteTool));
    registry.register(Arc::new(EditTool));
    registry.register(Arc::new(GrepTool));
    registry.register(Arc::new(GlobTool));
    registry.register(Arc::new(LsTool));

    tracing::info!("Registered {} built-in tools", registry.len());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_register_builtin_tools() {
        let registry = ToolRegistry::new(PathBuf::from("/tmp"));
        register_builtin_tools(&registry);

        assert_eq!(registry.len(), 7);
        assert!(registry.contains("bash"));
        assert!(registry.contains("read"));
        assert!(registry.contains("write"));
        assert!(registry.contains("edit"));
        assert!(registry.contains("grep"));
        assert!(registry.contains("glob"));
        assert!(registry.contains("ls"));
    }
}
