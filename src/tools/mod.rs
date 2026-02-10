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

mod claude_code_skill;
mod dynamic;
mod registry;
mod skill_loader;
pub mod task;
mod types;

pub use claude_code_skill::{load_claude_code_skills, ClaudeCodeSkill, ToolPermission};
pub use registry::ToolRegistry;
pub use skill_loader::{
    load_skills_from_dir, load_tools_from_skill, parse_skill_tools, SkillToolDef,
};
pub use task::{task_params_schema, TaskExecutor, TaskParams, TaskResult};
pub use types::{Tool, ToolBackend, ToolContext, ToolOutput};

use crate::config::CodeConfig;
use crate::file_history::{self, FileHistory};
use crate::llm::ToolDefinition;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

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
/// and provides backward-compatible API. Includes file version history tracking
/// for write/edit/patch operations.
pub struct ToolExecutor {
    workspace: PathBuf,
    registry: ToolRegistry,
    file_history: Arc<FileHistory>,
}

impl ToolExecutor {
    pub fn new(workspace: String) -> Self {
        let workspace_path = PathBuf::from(&workspace);
        tracing::info!(
            "ToolExecutor initialized with workspace: {}",
            workspace_path.display()
        );

        let registry = ToolRegistry::new(workspace_path.clone());

        // Load built-in tools from skill definition
        let builtin_skill = include_str!("../../skills/builtin-tools.md");
        let tools = parse_skill_tools(builtin_skill);
        for tool in tools {
            registry.register(tool);
        }

        Self {
            workspace: workspace_path,
            registry,
            file_history: Arc::new(FileHistory::new(500)),
        }
    }

    /// Create a new ToolExecutor with configuration
    ///
    /// Loads built-in tools first, then loads skills from configured directories.
    /// Supports both A3S skill format and Claude Code skill format.
    pub fn with_config(workspace: String, config: &CodeConfig) -> Self {
        let workspace_path = PathBuf::from(&workspace);
        tracing::info!(
            "ToolExecutor initialized with workspace: {} and config",
            workspace_path.display()
        );

        let registry = ToolRegistry::new(workspace_path.clone());

        // Load built-in tools from skill definition
        let builtin_skill = include_str!("../../skills/builtin-tools.md");
        let tools = parse_skill_tools(builtin_skill);
        for tool in tools {
            registry.register(tool);
        }

        // Load skills from configured directories
        for dir in &config.skill_dirs {
            // Load A3S format skills (with tools array)
            let skill_tools = load_skills_from_dir(dir);
            for tool in skill_tools {
                tracing::info!("Loaded skill tool '{}' from {}", tool.name(), dir.display());
                registry.register(tool);
            }

            // Load Claude Code format skills (prompt-based)
            let claude_skills = load_claude_code_skills(dir);
            for skill in claude_skills {
                tracing::info!(
                    "Loaded Claude Code skill '{}' from {}",
                    skill.name,
                    dir.display()
                );
                // Claude Code skills are stored for prompt injection, not as tools
                // They will be used by the session to provide context
            }
        }

        Self {
            workspace: workspace_path,
            registry,
            file_history: Arc::new(FileHistory::new(500)),
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

    /// Get the file version history tracker
    pub fn file_history(&self) -> &Arc<FileHistory> {
        &self.file_history
    }

    /// Capture a file snapshot before a modifying tool executes
    fn capture_snapshot(&self, name: &str, args: &serde_json::Value) {
        if let Some(file_path) = file_history::extract_file_path(name, args) {
            let resolved = self.workspace.join(&file_path);
            // Also try the raw path if it's absolute
            let path_to_read = if resolved.exists() {
                resolved
            } else if std::path::Path::new(&file_path).exists() {
                std::path::PathBuf::from(&file_path)
            } else {
                // New file, save empty snapshot
                self.file_history.save_snapshot(&file_path, "", name);
                return;
            };

            match std::fs::read_to_string(&path_to_read) {
                Ok(content) => {
                    self.file_history.save_snapshot(&file_path, &content, name);
                    tracing::debug!(
                        "Captured file snapshot for {} before {} (version {})",
                        file_path,
                        name,
                        self.file_history.list_versions(&file_path).len() - 1,
                    );
                }
                Err(e) => {
                    tracing::warn!("Failed to capture snapshot for {}: {}", file_path, e);
                }
            }
        }
    }

    /// Execute a tool by name using the server-level default context
    pub async fn execute(&self, name: &str, args: &serde_json::Value) -> Result<ToolResult> {
        tracing::info!("Executing tool: {} with args: {}", name, args);

        // Capture file snapshot before modification
        self.capture_snapshot(name, args);

        let result = self.registry.execute(name, args).await;

        match &result {
            Ok(r) => tracing::info!("Tool {} completed with exit_code={}", name, r.exit_code),
            Err(e) => tracing::error!("Tool {} failed: {}", name, e),
        }

        result
    }

    /// Execute a tool by name with a per-session context
    pub async fn execute_with_context(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult> {
        tracing::info!("Executing tool: {} with args: {}", name, args);

        // Capture file snapshot before modification
        self.capture_snapshot(name, args);

        let result = self.registry.execute_with_context(name, args, ctx).await;

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

    #[test]
    fn test_builtin_skill_parsing() {
        let builtin_skill = include_str!("../../skills/builtin-tools.md");
        let tools = parse_skill_tools(builtin_skill);
        assert_eq!(tools.len(), 11); // 11 built-in tools (including patch, web_fetch, cron)
    }

    #[tokio::test]
    async fn test_tool_executor_creation() {
        let executor = ToolExecutor::new("/tmp".to_string());
        assert_eq!(executor.registry.len(), 11); // 11 built-in tools (including patch, web_fetch, cron)
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

        // Should have all 8 built-in tools (including web_fetch)
        assert!(definitions.iter().any(|t| t.name == "bash"));
        assert!(definitions.iter().any(|t| t.name == "read"));
        assert!(definitions.iter().any(|t| t.name == "write"));
        assert!(definitions.iter().any(|t| t.name == "edit"));
        assert!(definitions.iter().any(|t| t.name == "grep"));
        assert!(definitions.iter().any(|t| t.name == "glob"));
        assert!(definitions.iter().any(|t| t.name == "ls"));
        assert!(definitions.iter().any(|t| t.name == "patch"));
        assert!(definitions.iter().any(|t| t.name == "web_fetch"));
    }

    #[tokio::test]
    async fn test_register_skill_tools() {
        let executor = ToolExecutor::new("/tmp".to_string());

        // Initial count: 11 built-in tools (including patch, web_fetch, cron)
        assert_eq!(executor.definitions().len(), 11);

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

        // Now should have 12 tools (11 built-in + 1 custom)
        assert_eq!(executor.definitions().len(), 12);
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
        assert_eq!(executor.definitions().len(), 12);

        // Unregister the tool
        let removed = executor.unregister_tools(&registered);
        assert_eq!(removed, vec!["temp-tool"]);
        assert_eq!(executor.definitions().len(), 11);
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

    #[tokio::test]
    async fn test_tool_executor_with_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let skill_dir = temp_dir.path().join("skills");
        std::fs::create_dir(&skill_dir).unwrap();

        // Create a skill file
        std::fs::write(
            skill_dir.join("custom.md"),
            r#"---
name: custom-skill
tools:
  - name: custom-tool
    description: A custom tool from config
    backend:
      type: script
      interpreter: bash
      script: echo "custom"
---
"#,
        )
        .unwrap();

        let config = CodeConfig::new().add_skill_dir(&skill_dir);
        let executor =
            ToolExecutor::with_config(temp_dir.path().to_string_lossy().to_string(), &config);

        // Should have built-in tools plus custom tool
        assert_eq!(executor.definitions().len(), 12); // 11 built-in + 1 custom
        assert!(executor
            .definitions()
            .iter()
            .any(|t| t.name == "custom-tool"));
    }
}
