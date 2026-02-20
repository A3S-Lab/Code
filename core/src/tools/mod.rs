//! Extensible Tool System
//!
//! Provides a trait-based abstraction for tools.
//!
//! ## Architecture
//!
//! ```text
//! ToolRegistry
//!   └── builtin tools (bash, read, write, edit, grep, glob, ls, patch, web_fetch, web_search)
//! ```

mod builtin;
mod process;
mod registry;
pub mod task;
mod types;

pub use registry::ToolRegistry;
pub use task::{
    parallel_task_params_schema, task_params_schema, ParallelTaskParams, ParallelTaskTool,
    TaskExecutor, TaskParams, TaskResult,
};
pub use types::{Tool, ToolContext, ToolEventSender, ToolOutput, ToolStreamEvent};

use crate::file_history::{self, FileHistory};
use crate::llm::ToolDefinition;
use crate::permissions::{PermissionChecker, PermissionDecision};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Image attachments from tool execution (multi-modal output).
    #[serde(skip)]
    pub images: Vec<crate::llm::Attachment>,
}

impl ToolResult {
    pub fn success(name: &str, output: String) -> Self {
        Self {
            name: name.to_string(),
            output,
            exit_code: 0,
            metadata: None,
            images: Vec::new(),
        }
    }

    pub fn error(name: &str, message: String) -> Self {
        Self {
            name: name.to_string(),
            output: message,
            exit_code: 1,
            metadata: None,
            images: Vec::new(),
        }
    }
}

impl From<ToolOutput> for ToolResult {
    fn from(output: ToolOutput) -> Self {
        Self {
            name: String::new(),
            output: output.content,
            exit_code: if output.success { 0 } else { 1 },
            metadata: output.metadata,
            images: output.images,
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
    registry: Arc<ToolRegistry>,
    file_history: Arc<FileHistory>,
    guard_policy: Option<Arc<dyn PermissionChecker>>,
}

impl ToolExecutor {
    pub fn new(workspace: String) -> Self {
        let workspace_path = PathBuf::from(&workspace);
        let registry = Arc::new(ToolRegistry::new(workspace_path.clone()));

        // Register native Rust built-in tools
        builtin::register_builtins(&registry);
        // Batch tool requires Arc<ToolRegistry>, registered separately
        builtin::register_batch(&registry);

        Self {
            workspace: workspace_path,
            registry,
            file_history: Arc::new(FileHistory::new(500)),
            guard_policy: None,
        }
    }

    pub fn set_guard_policy(&mut self, policy: Arc<dyn PermissionChecker>) {
        self.guard_policy = Some(policy);
    }

    fn check_guard(&self, name: &str, args: &serde_json::Value) -> Result<()> {
        if let Some(checker) = &self.guard_policy {
            if checker.check(name, args) == PermissionDecision::Deny {
                anyhow::bail!(
                    "Defense-in-depth: Tool '{}' is blocked by guard permission policy",
                    name
                );
            }
        }
        Ok(())
    }

    fn check_workspace_boundary(
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<()> {
        let path_field = match name {
            "read" | "write" | "edit" | "patch" => Some("file_path"),
            "ls" | "grep" | "glob" => Some("path"),
            _ => None,
        };

        if let Some(field) = path_field {
            if let Some(path_str) = args.get(field).and_then(|v| v.as_str()) {
                let target = if std::path::Path::new(path_str).is_absolute() {
                    std::path::PathBuf::from(path_str)
                } else {
                    ctx.workspace.join(path_str)
                };

                if let (Ok(canonical_target), Ok(canonical_workspace)) = (
                    target.canonicalize().or_else(|_| {
                        target
                            .parent()
                            .and_then(|p| p.canonicalize().ok())
                            .ok_or_else(|| {
                                std::io::Error::new(
                                    std::io::ErrorKind::NotFound,
                                    "parent not found",
                                )
                            })
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

    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    pub fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    pub fn register_dynamic_tool(&self, tool: Arc<dyn Tool>) {
        self.registry.register(tool);
    }

    pub fn unregister_dynamic_tool(&self, name: &str) {
        self.registry.unregister(name);
    }

    pub fn file_history(&self) -> &Arc<FileHistory> {
        &self.file_history
    }

    fn capture_snapshot(&self, name: &str, args: &serde_json::Value) {
        if let Some(file_path) = file_history::extract_file_path(name, args) {
            let resolved = self.workspace.join(&file_path);
            let path_to_read = if resolved.exists() {
                resolved
            } else if std::path::Path::new(&file_path).exists() {
                std::path::PathBuf::from(&file_path)
            } else {
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

    pub async fn execute(&self, name: &str, args: &serde_json::Value) -> Result<ToolResult> {
        self.check_guard(name, args)?;
        tracing::info!("Executing tool: {} with args: {}", name, args);
        self.capture_snapshot(name, args);
        let result = self.registry.execute(name, args).await;
        match &result {
            Ok(r) => tracing::info!("Tool {} completed with exit_code={}", name, r.exit_code),
            Err(e) => tracing::error!("Tool {} failed: {}", name, e),
        }
        result
    }

    pub async fn execute_with_context(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult> {
        self.check_guard(name, args)?;
        Self::check_workspace_boundary(name, args, ctx)?;
        tracing::info!("Executing tool: {} with args: {}", name, args);
        self.capture_snapshot(name, args);
        let result = self.registry.execute_with_context(name, args, ctx).await;
        match &result {
            Ok(r) => tracing::info!("Tool {} completed with exit_code={}", name, r.exit_code),
            Err(e) => tracing::error!("Tool {} failed: {}", name, e),
        }
        result
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.registry.definitions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tool_executor_creation() {
        let executor = ToolExecutor::new("/tmp".to_string());
        assert_eq!(executor.registry.len(), 11);
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

        assert!(definitions.iter().any(|t| t.name == "bash"));
        assert!(definitions.iter().any(|t| t.name == "read"));
        assert!(definitions.iter().any(|t| t.name == "write"));
        assert!(definitions.iter().any(|t| t.name == "edit"));
        assert!(definitions.iter().any(|t| t.name == "grep"));
        assert!(definitions.iter().any(|t| t.name == "glob"));
        assert!(definitions.iter().any(|t| t.name == "ls"));
        assert!(definitions.iter().any(|t| t.name == "patch"));
        assert!(definitions.iter().any(|t| t.name == "web_fetch"));
        assert!(definitions.iter().any(|t| t.name == "web_search"));
        assert!(definitions.iter().any(|t| t.name == "batch"));
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
            images: Vec::new(),
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
            images: Vec::new(),
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
        assert_eq!(registry.len(), 11);
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
