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

mod artifacts;
mod builtin;
mod process;
mod program_tool;
mod registry;
mod selector;
pub mod skill;
pub mod task;
mod types;

pub use artifacts::{ArtifactStore, ArtifactStoreLimits, ToolArtifact};
pub(crate) use builtin::register_skill;
pub use builtin::{
    register_generate_object, register_program, register_program_with_catalog, register_task,
    register_task_with_mcp,
};
pub use program_tool::ProgramTool;
pub use registry::ToolRegistry;
pub use selector::{select_tools_for_messages, select_tools_for_prompt};
pub use task::{
    parallel_task_params_schema, task_params_schema, ParallelTaskParams, ParallelTaskTool,
    TaskExecutor, TaskParams, TaskResult, TaskTool,
};
pub use types::{Tool, ToolContext, ToolEventSender, ToolOutput, ToolStreamEvent};

use crate::file_history::{self, FileHistory};
use crate::llm::ToolDefinition;
use crate::permissions::{PermissionChecker, PermissionDecision};
use crate::text::truncate_utf8;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Maximum output size in bytes before truncation
pub const MAX_OUTPUT_SIZE: usize = 100 * 1024; // 100KB

/// Maximum lines to read from a file
pub const MAX_READ_LINES: usize = 2000;

/// Maximum line length before truncation
pub const MAX_LINE_LENGTH: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolOutputArtifact {
    pub artifact_id: String,
    pub artifact_uri: String,
    pub original_bytes: usize,
    pub shown_bytes: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct TruncatedToolOutput {
    pub content: String,
    pub artifact: Option<ToolOutputArtifact>,
}

pub(crate) fn truncate_tool_output_with_artifact(
    tool_name: &str,
    output: &str,
) -> TruncatedToolOutput {
    if output.len() <= MAX_OUTPUT_SIZE {
        return TruncatedToolOutput {
            content: output.to_string(),
            artifact: None,
        };
    }

    let shown = truncate_utf8(output, MAX_OUTPUT_SIZE);
    let artifact = tool_output_artifact(tool_name, output, shown.len());
    let artifact_uri = artifact.artifact_uri.clone();
    let content = format!(
        "{}\n\n[tool output truncated: showing the first {} of {} bytes. Full output artifact: {}. Use narrower arguments such as offset/limit or filtering when possible.]",
        shown,
        shown.len(),
        output.len(),
        artifact_uri,
    );

    TruncatedToolOutput {
        content,
        artifact: Some(artifact),
    }
}

pub(crate) fn tool_output_artifact(
    tool_name: &str,
    output: &str,
    shown_bytes: usize,
) -> ToolOutputArtifact {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool_name.hash(&mut hasher);
    output.len().hash(&mut hasher);
    output.hash(&mut hasher);
    let digest = hasher.finish();
    let sanitized_tool = tool_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let artifact_id = format!("tool-output:{sanitized_tool}:{digest:016x}");
    let artifact_uri = format!("a3s://tool-output/{sanitized_tool}/{digest:016x}");

    ToolOutputArtifact {
        artifact_id,
        artifact_uri,
        original_bytes: output.len(),
        shown_bytes,
    }
}

pub(crate) fn merge_tool_output_artifact_metadata(
    metadata: Option<serde_json::Value>,
    artifact: &ToolOutputArtifact,
) -> serde_json::Value {
    let artifact_json = serde_json::json!({
        "artifact_id": artifact.artifact_id,
        "artifact_uri": artifact.artifact_uri,
        "original_bytes": artifact.original_bytes,
        "shown_bytes": artifact.shown_bytes,
    });

    match metadata {
        Some(serde_json::Value::Object(mut object)) => {
            object.insert("artifact".to_string(), artifact_json);
            serde_json::Value::Object(object)
        }
        Some(value) => serde_json::json!({
            "artifact": artifact_json,
            "previous_metadata": value,
        }),
        None => serde_json::json!({
            "artifact": artifact_json,
        }),
    }
}

/// Tool execution result returned by direct tool execution.
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
/// and captures file snapshots before write/edit/patch operations.
///
/// Defense-in-depth: An optional permission policy can be set to block
/// denied tools even if the caller bypasses the agent loop's authorization.
pub struct ToolExecutor {
    workspace: PathBuf,
    registry: Arc<ToolRegistry>,
    file_history: Arc<FileHistory>,
    guard_policy: Option<Arc<dyn PermissionChecker>>,
    command_env: Option<Arc<HashMap<String, String>>>,
}

impl ToolExecutor {
    pub fn new(workspace: String) -> Self {
        Self::new_with_options(workspace, None, ArtifactStoreLimits::default())
    }

    pub fn new_with_command_env(workspace: String, command_env: HashMap<String, String>) -> Self {
        Self::new_with_options(workspace, Some(command_env), ArtifactStoreLimits::default())
    }

    pub fn new_with_artifact_limits(
        workspace: String,
        artifact_limits: ArtifactStoreLimits,
    ) -> Self {
        Self::new_with_options(workspace, None, artifact_limits)
    }

    pub fn new_with_command_env_and_artifact_limits(
        workspace: String,
        command_env: HashMap<String, String>,
        artifact_limits: ArtifactStoreLimits,
    ) -> Self {
        Self::new_with_options(workspace, Some(command_env), artifact_limits)
    }

    fn new_with_options(
        workspace: String,
        command_env: Option<HashMap<String, String>>,
        artifact_limits: ArtifactStoreLimits,
    ) -> Self {
        let workspace_path = PathBuf::from(&workspace);
        let registry = Arc::new(ToolRegistry::with_artifact_limits(
            workspace_path.clone(),
            artifact_limits,
        ));

        // Register native Rust built-in tools
        builtin::register_builtins(&registry);
        // Batch tool requires Arc<ToolRegistry>, registered separately
        builtin::register_batch(&registry);
        builtin::register_program(&registry);

        Self {
            workspace: workspace_path,
            registry,
            file_history: Arc::new(FileHistory::new(500)),
            guard_policy: None,
            command_env: command_env.map(Arc::new),
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

                // Canonicalize workspace first — fail closed if it can't be resolved
                let canonical_workspace = ctx.workspace.canonicalize().map_err(|e| {
                    anyhow::anyhow!(
                        "Workspace boundary check failed: cannot canonicalize workspace '{}': {}",
                        ctx.workspace.display(),
                        e
                    )
                })?;

                // Try to canonicalize target; fall back to parent directory for new files
                let canonical_target = target.canonicalize().or_else(|_| {
                    target
                        .parent()
                        .and_then(|p| p.canonicalize().ok())
                        .ok_or_else(|| {
                            std::io::Error::new(std::io::ErrorKind::NotFound, "parent not found")
                        })
                });

                match canonical_target {
                    Ok(canonical) => {
                        if !canonical.starts_with(&canonical_workspace) {
                            anyhow::bail!(
                                "Workspace boundary violation: tool '{}' path '{}' escapes workspace '{}'",
                                name,
                                path_str,
                                ctx.workspace.display()
                            );
                        }
                    }
                    Err(_) => {
                        // Fail closed: if we can't resolve the target path, deny the operation
                        anyhow::bail!(
                            "Workspace boundary check failed: cannot resolve path '{}' for tool '{}'",
                            path_str,
                            name
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

    /// Get a stored tool artifact by URI.
    pub fn get_artifact(&self, artifact_uri: &str) -> Option<ToolArtifact> {
        self.registry.get_artifact(artifact_uri)
    }

    /// Return a clone of the executor's artifact store handle.
    pub fn artifact_store(&self) -> ArtifactStore {
        self.registry.artifact_store()
    }

    /// Replace the sink used for compact execution trace events.
    pub fn set_trace_sink(&self, sink: Arc<dyn crate::trace::TraceSink>) {
        self.registry.set_trace_sink(sink);
    }

    /// Return the currently configured execution trace sink.
    pub fn trace_sink(&self) -> Arc<dyn crate::trace::TraceSink> {
        self.registry.trace_sink()
    }

    pub fn command_env(&self) -> Option<Arc<HashMap<String, String>>> {
        self.command_env.clone()
    }

    pub fn register_dynamic_tool(&self, tool: Arc<dyn Tool>) {
        self.registry.register(tool);
    }

    pub fn unregister_dynamic_tool(&self, name: &str) {
        self.registry.unregister(name);
    }

    /// Unregister all dynamic tools whose names start with the given prefix.
    pub fn unregister_tools_by_prefix(&self, prefix: &str) {
        self.registry.unregister_by_prefix(prefix);
    }

    /// Replace the model-visible `program` tool with a custom PTC catalog.
    pub fn register_program_catalog(&self, catalog: crate::program::ProgramCatalog) {
        builtin::register_program_with_catalog(&self.registry, catalog);
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
        let mut result = self.registry.execute(name, args).await;
        if let Ok(ref mut r) = result {
            self.attach_diff_metadata(name, args, r);
        }
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
        let mut result = self.registry.execute_with_context(name, args, ctx).await;
        if let Ok(ref mut r) = result {
            self.attach_diff_metadata(name, args, r);
        }
        match &result {
            Ok(r) => tracing::info!("Tool {} completed with exit_code={}", name, r.exit_code),
            Err(e) => tracing::error!("Tool {} failed: {}", name, e),
        }
        result
    }

    fn attach_diff_metadata(&self, name: &str, args: &serde_json::Value, result: &mut ToolResult) {
        if !file_history::is_file_modifying_tool(name) {
            return;
        }
        let Some(file_path) = file_history::extract_file_path(name, args) else {
            return;
        };
        // Only store file_path in metadata, let translate_event read the actual content
        // using the session's correct workspace
        let meta = result.metadata.get_or_insert_with(|| serde_json::json!({}));
        meta["file_path"] = serde_json::Value::String(file_path);
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.registry.definitions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct LargeArtifactTool;

    #[async_trait]
    impl Tool for LargeArtifactTool {
        fn name(&self) -> &str {
            "large_artifact"
        }

        fn description(&self) -> &str {
            "Produces large output for artifact API tests"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {},
                "required": []
            })
        }

        async fn execute(
            &self,
            args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            let suffix = args
                .get("suffix")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            Ok(ToolOutput::success(format!(
                "{}{}",
                "z".repeat(MAX_OUTPUT_SIZE + 1),
                suffix
            )))
        }
    }

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echoes the message argument"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }

        async fn execute(
            &self,
            args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::success(
                args["message"].as_str().unwrap_or_default(),
            ))
        }
    }

    #[tokio::test]
    async fn test_tool_executor_creation() {
        let executor = ToolExecutor::new("/tmp".to_string());
        // Baseline tools on a raw ToolExecutor: 13
        assert_eq!(executor.registry.len(), 13);
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
        // Baseline tools on a raw ToolExecutor: 13
        assert_eq!(registry.len(), 13);
    }

    #[tokio::test]
    async fn test_tool_executor_get_artifact() {
        let executor = ToolExecutor::new("/tmp".to_string());
        executor.register_dynamic_tool(Arc::new(LargeArtifactTool));

        let result = executor
            .execute("large_artifact", &serde_json::json!({}))
            .await
            .unwrap();

        let artifact_uri = result.metadata.as_ref().unwrap()["artifact"]["artifact_uri"]
            .as_str()
            .unwrap();
        let artifact = executor.get_artifact(artifact_uri).expect("artifact");
        assert_eq!(artifact.tool_name, "large_artifact");
        assert_eq!(artifact.content.len(), MAX_OUTPUT_SIZE + 1);
        assert!(executor.artifact_store().get(artifact_uri).is_some());
    }

    #[tokio::test]
    async fn test_tool_executor_respects_artifact_limits() {
        let executor = ToolExecutor::new_with_artifact_limits(
            "/tmp".to_string(),
            ArtifactStoreLimits {
                max_artifacts: 1,
                max_bytes: usize::MAX,
            },
        );
        executor.register_dynamic_tool(Arc::new(LargeArtifactTool));

        let first = executor
            .execute("large_artifact", &serde_json::json!({}))
            .await
            .unwrap();
        let first_uri = first.metadata.as_ref().unwrap()["artifact"]["artifact_uri"]
            .as_str()
            .unwrap()
            .to_string();

        executor
            .execute("large_artifact", &serde_json::json!({ "suffix": "again" }))
            .await
            .unwrap();

        assert_eq!(executor.artifact_store().limits().max_artifacts, 1);
        assert_eq!(executor.artifact_store().len(), 1);
        assert!(executor.get_artifact(&first_uri).is_none());
    }

    #[tokio::test]
    async fn test_tool_executor_register_program_catalog_keeps_script_only_program_tool() {
        let executor = ToolExecutor::new("/tmp".to_string());
        let trace_sink = crate::trace::InMemoryTraceSink::default();
        executor.set_trace_sink(Arc::new(trace_sink.clone()));
        executor.register_dynamic_tool(Arc::new(EchoTool));
        let mut catalog = crate::program::ProgramCatalog::new();
        catalog.register(
            crate::program::ProgramTemplate::new("custom_echo", "Run a custom echo program")
                .with_parameter(crate::program::ProgramParameter::required(
                    "message",
                    "Message to echo",
                ))
                .with_step(
                    crate::program::ProgramStepTemplate::new(
                        "echo",
                        serde_json::json!({ "message": "{{message}}" }),
                    )
                    .with_label("echo_message"),
                ),
        );
        executor.register_program_catalog(catalog);

        let result = executor
            .execute(
                "program",
                &serde_json::json!({
                    "name": "custom_echo",
                    "inputs": {
                        "message": "hello from catalog"
                    }
                }),
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 1);
        assert!(result.output.contains("type parameter is required"));

        let events = trace_sink.events();
        assert!(events.iter().any(|event| {
            event.kind == crate::trace::TraceEventKind::ToolExecution && event.name == "program"
        }));
        assert!(!events.iter().any(|event| {
            event.kind == crate::trace::TraceEventKind::ToolExecution && event.name == "echo"
        }));
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
    fn test_truncate_tool_output_with_artifact_reference() {
        let output = "x".repeat(MAX_OUTPUT_SIZE + 1);
        let truncated = truncate_tool_output_with_artifact("test/tool", &output);

        let artifact = truncated.artifact.expect("artifact");
        assert!(truncated.content.contains("Full output artifact:"));
        assert_eq!(artifact.original_bytes, MAX_OUTPUT_SIZE + 1);
        assert_eq!(artifact.shown_bytes, MAX_OUTPUT_SIZE);
        assert!(artifact.artifact_id.starts_with("tool-output:test_tool:"));
        assert!(artifact
            .artifact_uri
            .starts_with("a3s://tool-output/test_tool/"));
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

    #[tokio::test]
    async fn test_execute_attaches_diff_metadata() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, "before content\n").unwrap();

        let executor = ToolExecutor::new(dir.path().to_str().unwrap().to_string());
        let args = serde_json::json!({
            "file_path": "hello.txt",
            "content": "after content\n"
        });
        let result = executor.execute("write", &args).await.unwrap();

        let meta = result.metadata.expect("metadata should be present");
        assert_eq!(meta["before"], "before content\n");
        assert_eq!(meta["after"], "after content\n");
        assert_eq!(meta["file_path"], "hello.txt");
    }

    #[tokio::test]
    async fn test_execute_with_context_attaches_diff_metadata() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let canonical_dir = dir.path().canonicalize().unwrap();
        let file = canonical_dir.join("ctx.txt");
        std::fs::write(&file, "original\n").unwrap();

        let executor = ToolExecutor::new(canonical_dir.to_str().unwrap().to_string());
        let ctx = ToolContext {
            workspace: canonical_dir.clone(),
            session_id: None,
            event_tx: None,
            agent_event_tx: None,
            search_config: None,
            sandbox: None,
            command_env: None,
        };
        let args = serde_json::json!({
            "file_path": "ctx.txt",
            "content": "updated\n"
        });
        let result = executor
            .execute_with_context("write", &args, &ctx)
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0, "write tool failed: {}", result.output);

        let meta = result.metadata.expect("metadata should be present");
        assert_eq!(meta["before"], "original\n");
        assert_eq!(meta["after"], "updated\n");
        assert_eq!(meta["file_path"], "ctx.txt");
    }
}
