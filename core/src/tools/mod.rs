//! Extensible Tool System
//!
//! Provides a trait-based abstraction for tools.
//!
//! ## Architecture
//!
//! ```text
//! ToolRegistry
//!   └── builtin tools (file, search, execution, web, and Code Intelligence queries)
//! ```

mod agent_dir_script_tool;
mod artifacts;
pub(crate) mod builtin;
mod invocation;
mod pagination;
pub(crate) mod process;
mod program_tool;
mod registry;
mod selector;
pub mod skill;
pub mod task;
mod types;

pub use crate::dynamic_workflow::register_dynamic_workflow;
pub use agent_dir_script_tool::AgentDirScriptTool;
pub use artifacts::{ArtifactStore, ArtifactStoreLimits, ToolArtifact};
pub(crate) use builtin::register_skill;
pub use builtin::{
    register_generate_object, register_program, register_program_with_catalog, register_task,
    register_task_with_mcp, register_task_with_mcp_managers,
};
pub(crate) use invocation::{
    registry_tool_invoker, HostDirectPolicy, InvocationOrigin, ToolInvocation, ToolInvoker,
};
pub use program_tool::{ProgramTool, MAX_PROGRAM_SCRIPT_SOURCE_BYTES};
pub use registry::ToolRegistry;
pub use selector::{select_tools_for_messages, select_tools_for_prompt};
pub use task::{
    parallel_task_params_schema, task_params_schema, ParallelTaskParams, ParallelTaskTool,
    TaskExecutor, TaskParams, TaskResult, TaskTool,
};
pub(crate) use types::{AgentEventBarrier, AgentEventBarrierReceiver};
pub use types::{
    InvocationRuntime, Tool, ToolCapabilities, ToolContext, ToolErrorKind, ToolEventSender,
    ToolOutput, ToolOutputKind, ToolStreamEvent,
};

use crate::llm::ToolDefinition;
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
    /// Structured discriminant for tool failures. Populated by built-in
    /// tools that can map their failure into a typed [`ToolErrorKind`]
    /// (e.g. `edit`/`patch` setting `VersionConflict` on a CAS rejection
    /// from `WorkspaceError`). Forwarded to the SDK so callers can react
    /// programmatically without parsing `output`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<types::ToolErrorKind>,
}

impl ToolResult {
    pub fn success(name: &str, output: String) -> Self {
        Self {
            name: name.to_string(),
            output,
            exit_code: 0,
            metadata: None,
            images: Vec::new(),
            error_kind: None,
        }
    }

    pub fn error(name: &str, message: String) -> Self {
        Self {
            name: name.to_string(),
            output: message,
            exit_code: 1,
            metadata: None,
            images: Vec::new(),
            error_kind: None,
        }
    }

    pub fn error_with_kind(name: &str, message: String, kind: types::ToolErrorKind) -> Self {
        let mut result = Self::error(name, message);
        result.error_kind = Some(kind);
        result
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
            error_kind: output.error_kind,
        }
    }
}

/// Tool executor with workspace sandboxing
///
/// This is the main entry point for tool execution. It wraps the ToolRegistry.
pub struct ToolExecutor {
    workspace: PathBuf,
    registry: Arc<ToolRegistry>,
    command_env: Option<Arc<HashMap<String, String>>>,
}

/// Build a log line for a tool invocation that excludes argument *values*.
///
/// Argument values (full bash commands, file contents written by `write`/`edit`)
/// can contain secrets, so the summary records only the tool name, the sorted
/// argument field names, and the serialized payload size — never the values. This
/// keeps the always-on `info!` tool trace (also exported to OTLP) compliant with
/// the "never log secrets" boundary. Use `trace!` for full args when debugging.
fn redacted_tool_log_summary(name: &str, args: &serde_json::Value) -> String {
    let arg_keys: Vec<&str> = match args.as_object() {
        Some(map) => {
            let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
            keys.sort_unstable();
            keys
        }
        None => Vec::new(),
    };
    format!(
        "Executing tool: {} (arg_keys={:?}, {} bytes)",
        name,
        arg_keys,
        args.to_string().len()
    )
}

/// Log a tool invocation without leaking argument values. See
/// [`redacted_tool_log_summary`] for the redaction rationale.
fn log_tool_invocation(name: &str, args: &serde_json::Value) {
    tracing::info!("{}", redacted_tool_log_summary(name, args));
    tracing::trace!("Tool {} full args: {}", name, args);
}

impl ToolExecutor {
    pub fn new(workspace: String) -> Self {
        let workspace_services =
            crate::workspace::WorkspaceServices::local(PathBuf::from(&workspace));
        Self::build(
            workspace,
            None,
            ArtifactStoreLimits::default(),
            workspace_services,
        )
    }

    pub fn new_with_artifact_limits(
        workspace: String,
        artifact_limits: ArtifactStoreLimits,
    ) -> Self {
        let workspace_services =
            crate::workspace::WorkspaceServices::local(PathBuf::from(&workspace));
        Self::build(workspace, None, artifact_limits, workspace_services)
    }

    pub fn new_with_workspace_services(
        workspace: String,
        workspace_services: Arc<crate::workspace::WorkspaceServices>,
    ) -> Self {
        Self::build(
            workspace,
            None,
            ArtifactStoreLimits::default(),
            workspace_services,
        )
    }

    pub fn new_with_workspace_services_and_artifact_limits(
        workspace: String,
        workspace_services: Arc<crate::workspace::WorkspaceServices>,
        artifact_limits: ArtifactStoreLimits,
    ) -> Self {
        Self::build(workspace, None, artifact_limits, workspace_services)
    }

    fn build(
        workspace: String,
        command_env: Option<HashMap<String, String>>,
        artifact_limits: ArtifactStoreLimits,
        workspace_services: Arc<crate::workspace::WorkspaceServices>,
    ) -> Self {
        let workspace_path = PathBuf::from(&workspace);
        let command_env = command_env.map(Arc::new);
        let registry = Arc::new(ToolRegistry::with_artifact_limits_and_workspace_services(
            workspace_path.clone(),
            artifact_limits,
            Arc::clone(&workspace_services),
        ));
        if let Some(env) = command_env.clone() {
            registry.set_command_env(env);
        }

        // Register native Rust built-in tools — only those whose required
        // workspace capability is available, so the model never sees a tool
        // the backend cannot service.
        builtin::register_builtins(&registry, &workspace_services);
        // Batch tool requires Arc<ToolRegistry>, registered separately
        builtin::register_batch(&registry);
        builtin::register_program(&registry);

        Self {
            workspace: workspace_path,
            registry,
            command_env,
        }
    }

    fn check_workspace_boundary(
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<()> {
        let path_field = match name {
            "read" | "write" | "edit" | "patch" => Some("file_path"),
            "ls" | "grep" | "glob" | "code_symbols" | "code_navigation" | "code_diagnostics" => {
                Some("path")
            }
            _ => None,
        };

        if let Some(field) = path_field {
            if let Some(path_str) = args.get(field).and_then(|v| v.as_str()) {
                ctx.resolve_workspace_path(path_str).map_err(|e| {
                    anyhow::anyhow!(
                        "Workspace boundary check failed for tool '{}' path '{}': {}",
                        name,
                        path_str,
                        e
                    )
                })?;
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

    pub(crate) fn register_dynamic_tool_with_shadow(
        &self,
        tool: Arc<dyn Tool>,
    ) -> (bool, Option<Arc<dyn Tool>>) {
        self.registry.register_with_shadow(tool)
    }

    pub(crate) fn restore_dynamic_tool_if_same(
        &self,
        name: &str,
        expected: &Arc<dyn Tool>,
        replacement: Option<Arc<dyn Tool>>,
    ) -> bool {
        self.registry.restore_if_same(name, expected, replacement)
    }

    pub(crate) fn register_dynamic_tool_if_absent(&self, tool: Arc<dyn Tool>) -> bool {
        self.registry.register_if_absent(tool)
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

    /// Execute directly against this low-level executor.
    ///
    /// This API intentionally does not install agent/session permission, HITL,
    /// hook, budget, queue, timeout, cancellation, or sanitization policy.
    /// Session hosts should use [`crate::AgentSession::tool`] (or its typed
    /// helpers), and agent runtimes must dispatch through their scoped tool
    /// invocation gateway.
    pub async fn execute(&self, name: &str, args: &serde_json::Value) -> Result<ToolResult> {
        let ctx = self.registry.context();
        if let Err(e) = Self::check_workspace_boundary(name, args, &ctx) {
            return Ok(ToolResult::error(name, e.to_string()));
        }

        log_tool_invocation(name, args);
        let mut result = self.registry.execute_with_context(name, args, &ctx).await;
        if let Ok(ref mut r) = result {
            self.attach_diff_metadata(name, args, r);
        }
        match &result {
            Ok(r) => tracing::info!("Tool {} completed with exit_code={}", name, r.exit_code),
            Err(e) => tracing::error!("Tool {} failed: {}", name, e),
        }
        result
    }

    /// Execute directly with a caller-owned context.
    ///
    /// Like [`Self::execute`], this is an ungoverned standalone boundary. A
    /// `ToolContext` supplies capabilities to the tool but is not itself a
    /// substitute for the agent/session invocation gateway.
    pub async fn execute_with_context(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult> {
        Self::check_workspace_boundary(name, args, ctx)?;
        log_tool_invocation(name, args);
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
        if !matches!(name, "write" | "edit" | "patch") {
            return;
        }
        let Some(file_path) = args.get("file_path").and_then(serde_json::Value::as_str) else {
            return;
        };
        // Only store file_path in metadata, let translate_event read the actual content
        // using the session's correct workspace
        let meta = result.metadata.get_or_insert_with(|| serde_json::json!({}));
        meta["file_path"] = serde_json::Value::String(file_path.to_string());
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.registry.definitions()
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
