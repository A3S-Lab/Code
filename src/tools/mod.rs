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
//!   ├── native tools (search_skills, install_skill, load_skill)
//!   └── dynamic tools (loaded from skills)
//!         ├── BinaryTool
//!         ├── HttpTool
//!         └── ScriptTool
//! ```

mod dynamic;
mod registry;
pub mod skill;
mod skill_catalog;
mod skill_discovery;
mod skill_loader;
pub mod task;
mod types;

pub use registry::ToolRegistry;
pub use skill::{builtin_skills, load_skills, Skill, SkillKind, ToolPermission};
pub use skill_catalog::{build_skills_injection, DEFAULT_CATALOG_THRESHOLD};
pub use task::{task_params_schema, TaskExecutor, TaskParams, TaskResult};
pub use types::{Tool, ToolBackend, ToolContext, ToolOutput};

pub(crate) use skill_loader::parse_skill_tools;

use crate::file_history::{self, FileHistory};
use crate::llm::ToolDefinition;
use crate::permissions::{PermissionDecision, PermissionPolicy};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

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
    /// Optional metadata propagated from tool execution (e.g., skill auto-load signals)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl ToolResult {
    pub fn success(name: &str, output: String) -> Self {
        Self {
            name: name.to_string(),
            output,
            exit_code: 0,
            metadata: None,
        }
    }

    pub fn error(name: &str, message: String) -> Self {
        Self {
            name: name.to_string(),
            output: message,
            exit_code: 1,
            metadata: None,
        }
    }
}

impl From<ToolOutput> for ToolResult {
    fn from(output: ToolOutput) -> Self {
        Self {
            name: String::new(), // Will be set by executor
            output: output.content,
            exit_code: if output.success { 0 } else { 1 },
            metadata: output.metadata,
        }
    }
}

/// Tool executor with workspace sandboxing
///
/// This is the main entry point for tool execution. It wraps the ToolRegistry
/// and provides backward-compatible API. Includes file version history tracking
/// for write/edit/patch operations.
///
/// Defense-in-depth: An optional permission policy can be set to block
/// denied tools even if the caller bypasses the agent loop's authorization.
pub struct ToolExecutor {
    workspace: PathBuf,
    registry: ToolRegistry,
    file_history: Arc<FileHistory>,
    /// Defense-in-depth: optional permission policy checked before every execution
    guard_policy: Option<Arc<RwLock<PermissionPolicy>>>,
}

impl ToolExecutor {
    pub fn new(workspace: String) -> Self {
        let workspace_path = PathBuf::from(&workspace);

        let registry = ToolRegistry::new(workspace_path.clone());

        // Load built-in tools from skill definition
        let builtin_skill = include_str!("../../skills/builtin-tools.md");
        let tools = parse_skill_tools(builtin_skill);
        for tool in tools {
            registry.register_builtin(tool);
        }

        // Register native Rust tools (skill discovery + on-demand loading)
        registry.register_builtin(Arc::new(skill_discovery::SearchSkillsTool::new()));
        registry.register_builtin(Arc::new(skill_discovery::InstallSkillTool::new()));
        registry.register_builtin(Arc::new(skill_discovery::LoadSkillTool::new()));

        Self {
            workspace: workspace_path,
            registry,
            file_history: Arc::new(FileHistory::new(500)),
            guard_policy: None,
        }
    }

    /// Set a defense-in-depth permission policy.
    ///
    /// When set, every tool execution is checked against this policy.
    /// Tools that match a `Deny` rule are blocked before execution.
    /// This is a safety net for code paths that bypass the agent loop.
    pub fn set_guard_policy(&mut self, policy: Arc<RwLock<PermissionPolicy>>) {
        self.guard_policy = Some(policy);
    }

    /// Check defense-in-depth guard policy. Returns Err if tool is denied.
    async fn check_guard(&self, name: &str, args: &serde_json::Value) -> Result<()> {
        if let Some(policy_lock) = &self.guard_policy {
            let policy = policy_lock.read().await;
            if policy.check(name, args) == PermissionDecision::Deny {
                anyhow::bail!(
                    "Defense-in-depth: Tool '{}' is blocked by guard permission policy",
                    name
                );
            }
        }
        Ok(())
    }

    /// Workspace boundary enforcement for file-accessing tools.
    /// Validates that file paths resolve within the workspace directory.
    fn check_workspace_boundary(
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<()> {
        // Only check file-accessing tools
        let path_field = match name {
            "read" | "write" | "edit" | "patch" | "ls" => Some("path"),
            "grep" | "glob" => Some("path"),
            _ => None,
        };

        if let Some(field) = path_field {
            if let Some(path_str) = args.get(field).and_then(|v| v.as_str()) {
                // Resolve the path relative to workspace
                let target = if std::path::Path::new(path_str).is_absolute() {
                    std::path::PathBuf::from(path_str)
                } else {
                    ctx.workspace.join(path_str)
                };

                // Canonicalize to resolve symlinks and ..
                // Use the workspace canonical path for comparison
                if let (Ok(canonical_target), Ok(canonical_workspace)) = (
                    target.canonicalize().or_else(|_| {
                        // File may not exist yet (write); check parent
                        target.parent()
                            .and_then(|p| p.canonicalize().ok())
                            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "parent not found"))
                    }),
                    ctx.workspace.canonicalize(),
                ) {
                    if !canonical_target.starts_with(&canonical_workspace) {
                        anyhow::bail!(
                            "Workspace boundary violation: tool '{}' path '{}' escapes workspace '{}'",
                            name,
                            path_str,
                            ctx.workspace.display()
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Get the workspace path
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Get the tool registry for dynamic tool registration
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Register a dynamic tool at runtime (e.g., MCP tools, LSP tools, task tool).
    /// The tool becomes immediately available in all future tool executions.
    pub fn register_dynamic_tool(&self, tool: Arc<dyn Tool>) {
        self.registry.register(tool);
    }

    /// Unregister a dynamic tool by name
    pub fn unregister_dynamic_tool(&self, name: &str) {
        self.registry.unregister(name);
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
        // Defense-in-depth: check guard policy before execution
        self.check_guard(name, args).await?;

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
        // Defense-in-depth: check guard policy before execution
        self.check_guard(name, args).await?;

        // Workspace boundary enforcement for file-accessing tools
        Self::check_workspace_boundary(name, args, ctx)?;

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
        assert_eq!(executor.registry.len(), 14); // 11 from builtin-tools.md + 3 native (search_skills, install_skill, load_skill)
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

        // Should have all built-in tools + native tools
        assert!(definitions.iter().any(|t| t.name == "bash"));
        assert!(definitions.iter().any(|t| t.name == "read"));
        assert!(definitions.iter().any(|t| t.name == "write"));
        assert!(definitions.iter().any(|t| t.name == "edit"));
        assert!(definitions.iter().any(|t| t.name == "grep"));
        assert!(definitions.iter().any(|t| t.name == "glob"));
        assert!(definitions.iter().any(|t| t.name == "ls"));
        assert!(definitions.iter().any(|t| t.name == "patch"));
        assert!(definitions.iter().any(|t| t.name == "web_fetch"));
        assert!(definitions.iter().any(|t| t.name == "search_skills"));
        assert!(definitions.iter().any(|t| t.name == "install_skill"));
        assert!(definitions.iter().any(|t| t.name == "load_skill"));
    }

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("test_tool", "output text".to_string());
        assert_eq!(result.name, "test_tool");
        assert_eq!(result.output, "output text");
        assert_eq!(result.exit_code, 0);
        assert!(result.metadata.is_none());
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("test_tool", "error message".to_string());
        assert_eq!(result.name, "test_tool");
        assert_eq!(result.output, "error message");
        assert_eq!(result.exit_code, 1);
        assert!(result.metadata.is_none());
    }

    #[test]
    fn test_tool_result_from_tool_output_success() {
        let output = ToolOutput {
            content: "success content".to_string(),
            success: true,
            metadata: None,
        };
        let result: ToolResult = output.into();
        assert_eq!(result.output, "success content");
        assert_eq!(result.exit_code, 0);
        assert!(result.metadata.is_none());
    }

    #[test]
    fn test_tool_result_from_tool_output_failure() {
        let output = ToolOutput {
            content: "failure content".to_string(),
            success: false,
            metadata: Some(serde_json::json!({"error": "test"})),
        };
        let result: ToolResult = output.into();
        assert_eq!(result.output, "failure content");
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.metadata, Some(serde_json::json!({"error": "test"})));
    }

    #[test]
    fn test_tool_result_metadata_propagation() {
        let output = ToolOutput::success("content")
            .with_metadata(serde_json::json!({"_load_skill": true, "skill_name": "test"}));
        let result: ToolResult = output.into();
        assert_eq!(result.exit_code, 0);
        let meta = result.metadata.unwrap();
        assert_eq!(meta["_load_skill"], true);
        assert_eq!(meta["skill_name"], "test");
    }

    #[test]
    fn test_tool_executor_workspace() {
        let executor = ToolExecutor::new("/test/workspace".to_string());
        assert_eq!(executor.workspace().to_str().unwrap(), "/test/workspace");
    }

    #[test]
    fn test_tool_executor_registry() {
        let executor = ToolExecutor::new("/tmp".to_string());
        let registry = executor.registry();
        assert_eq!(registry.len(), 14); // 11 from builtin-tools.md + 3 native
    }

    #[test]
    fn test_tool_executor_file_history() {
        let executor = ToolExecutor::new("/tmp".to_string());
        let history = executor.file_history();
        assert_eq!(history.list_versions("nonexistent.txt").len(), 0);
    }

    #[test]
    fn test_max_output_size_constant() {
        assert_eq!(MAX_OUTPUT_SIZE, 100 * 1024);
    }

    #[test]
    fn test_max_read_lines_constant() {
        assert_eq!(MAX_READ_LINES, 2000);
    }

    #[test]
    fn test_max_line_length_constant() {
        assert_eq!(MAX_LINE_LENGTH, 2000);
    }

    #[test]
    fn test_tool_result_clone() {
        let result = ToolResult::success("test", "output".to_string());
        let cloned = result.clone();
        assert_eq!(result.name, cloned.name);
        assert_eq!(result.output, cloned.output);
        assert_eq!(result.exit_code, cloned.exit_code);
        assert_eq!(result.metadata, cloned.metadata);
    }

    #[test]
    fn test_tool_result_debug() {
        let result = ToolResult::success("test", "output".to_string());
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("output"));
    }
}
