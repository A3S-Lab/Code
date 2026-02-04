//! Extensible Tool System
//!
//! Provides a trait-based abstraction for tools that can be:
//! - Built-in (Rust implementations)
//! - Binary (external executables)
//! - HTTP (API calls)
//! - Script (interpreted scripts)
//!
//! ## Architecture
//!
//! ```text
//! ToolRegistry
//!   ├── builtin tools (bash, read, write, edit, grep, glob, ls)
//!   └── dynamic tools (loaded from skills)
//!         ├── BinaryTool
//!         ├── HttpTool
//!         └── ScriptTool
//! ```

mod builtin;
mod dynamic;
mod registry;
mod skill_loader;
mod types;

pub use builtin::register_builtin_tools;
pub use registry::ToolRegistry;
pub use skill_loader::{load_tools_from_skill, parse_skill_tools, SkillToolDef};
pub use types::{Tool, ToolBackend, ToolContext, ToolOutput};

use crate::llm::ToolDefinition;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Maximum output size in bytes before truncation
pub const MAX_OUTPUT_SIZE: usize = 100 * 1024; // 100KB

/// Maximum lines to read from a file
pub const MAX_READ_LINES: usize = 2000;

/// Maximum line length before truncation
pub const MAX_LINE_LENGTH: usize = 2000;

/// Tool execution result (legacy format for backward compatibility)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub name: String,
    pub output: String,
    pub exit_code: i32,
}

impl ToolResult {
    pub fn success(name: &str, output: String) -> Self {
        Self {
            name: name.to_string(),
            output,
            exit_code: 0,
        }
    }

    pub fn error(name: &str, message: String) -> Self {
        Self {
            name: name.to_string(),
            output: message,
            exit_code: 1,
        }
    }
}

impl From<ToolOutput> for ToolResult {
    fn from(output: ToolOutput) -> Self {
        Self {
            name: String::new(), // Will be set by executor
            output: output.content,
            exit_code: if output.success { 0 } else { 1 },
        }
    }
}

/// Tool executor with workspace sandboxing
///
/// This is the main entry point for tool execution. It wraps the ToolRegistry
/// and provides backward-compatible API.
pub struct ToolExecutor {
    workspace: PathBuf,
    registry: ToolRegistry,
}

impl ToolExecutor {
    pub fn new(workspace: String) -> Self {
        let workspace_path = PathBuf::from(&workspace);
        tracing::info!(
            "ToolExecutor initialized with workspace: {}",
            workspace_path.display()
        );

        let registry = ToolRegistry::new(workspace_path.clone());

        // Register built-in tools
        register_builtin_tools(&registry);

        Self {
            workspace: workspace_path,
            registry,
        }
    }

    /// Get the workspace path
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Get the tool registry for dynamic tool registration
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, args: &serde_json::Value) -> Result<ToolResult> {
        tracing::info!("Executing tool: {} with args: {}", name, args);

        let result = self.registry.execute(name, args).await;

        match &result {
            Ok(r) => tracing::info!("Tool {} completed with exit_code={}", name, r.exit_code),
            Err(e) => tracing::error!("Tool {} failed: {}", name, e),
        }

        result
    }

    /// Get all tool definitions for LLM
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.registry.definitions()
    }

    /// Register tools from a skill (SKILL.md content)
    ///
    /// Returns the names of tools that were registered.
    pub fn register_skill_tools(&self, skill_content: &str) -> Vec<String> {
        let tools = parse_skill_tools(skill_content);
        let mut registered = Vec::new();

        for tool in tools {
            let name = tool.name().to_string();
            self.registry.register(tool);
            registered.push(name);
        }

        if !registered.is_empty() {
            tracing::info!(
                "Registered {} skill tools: {:?}",
                registered.len(),
                registered
            );
        }

        registered
    }

    /// Unregister tools by name
    ///
    /// Returns the names of tools that were actually removed.
    pub fn unregister_tools(&self, names: &[String]) -> Vec<String> {
        let mut removed = Vec::new();

        for name in names {
            if self.registry.unregister(name) {
                removed.push(name.clone());
            }
        }

        if !removed.is_empty() {
            tracing::info!("Unregistered {} tools: {:?}", removed.len(), removed);
        }

        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_executor_creation() {
        let executor = ToolExecutor::new("/tmp".to_string());
        assert_eq!(executor.registry.len(), 7); // 7 built-in tools
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let executor = ToolExecutor::new("/tmp".to_string());
        let result = executor
            .execute("unknown", &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(result.output.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_builtin_tools_registered() {
        let executor = ToolExecutor::new("/tmp".to_string());
        let definitions = executor.definitions();

        // Should have all 7 built-in tools
        assert!(definitions.iter().any(|t| t.name == "bash"));
        assert!(definitions.iter().any(|t| t.name == "read"));
        assert!(definitions.iter().any(|t| t.name == "write"));
        assert!(definitions.iter().any(|t| t.name == "edit"));
        assert!(definitions.iter().any(|t| t.name == "grep"));
        assert!(definitions.iter().any(|t| t.name == "glob"));
        assert!(definitions.iter().any(|t| t.name == "ls"));
    }

    #[tokio::test]
    async fn test_register_skill_tools() {
        let executor = ToolExecutor::new("/tmp".to_string());

        // Initial count: 7 built-in tools
        assert_eq!(executor.definitions().len(), 7);

        // Register skill tools
        let skill_content = r#"---
name: test-skill
tools:
  - name: custom-echo
    description: Custom echo tool
    backend:
      type: script
      interpreter: bash
      script: echo "$TOOL_ARG_MESSAGE"
    parameters:
      type: object
      properties:
        message:
          type: string
      required:
        - message
---
Test skill content
"#;

        let registered = executor.register_skill_tools(skill_content);
        assert_eq!(registered, vec!["custom-echo"]);

        // Now should have 8 tools
        assert_eq!(executor.definitions().len(), 8);
        assert!(executor
            .definitions()
            .iter()
            .any(|t| t.name == "custom-echo"));
    }

    #[tokio::test]
    async fn test_unregister_tools() {
        let executor = ToolExecutor::new("/tmp".to_string());

        // Register a skill tool
        let skill_content = r#"---
name: test-skill
tools:
  - name: temp-tool
    description: Temporary tool
    backend:
      type: script
      interpreter: bash
      script: echo "temp"
---
"#;

        let registered = executor.register_skill_tools(skill_content);
        assert_eq!(registered.len(), 1);
        assert_eq!(executor.definitions().len(), 8);

        // Unregister the tool
        let removed = executor.unregister_tools(&registered);
        assert_eq!(removed, vec!["temp-tool"]);
        assert_eq!(executor.definitions().len(), 7);
    }

    #[tokio::test]
    async fn test_execute_skill_tool() {
        let temp_dir = tempfile::tempdir().unwrap();
        let executor = ToolExecutor::new(temp_dir.path().to_string_lossy().to_string());

        // Register a simple script tool
        let skill_content = r#"---
name: test-skill
tools:
  - name: greet
    description: Greet someone
    backend:
      type: script
      interpreter: bash
      script: echo "Hello, $TOOL_ARG_NAME!"
    parameters:
      type: object
      properties:
        name:
          type: string
---
"#;

        executor.register_skill_tools(skill_content);

        // Execute the skill tool
        let result = executor
            .execute("greet", &serde_json::json!({"name": "World"}))
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("Hello, World!"));
    }
}
