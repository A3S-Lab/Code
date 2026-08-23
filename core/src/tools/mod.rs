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
mod presentation;
pub(crate) mod process;
mod program_tool;
mod registry;
mod result_transform;
mod selector;
pub mod skill;
pub mod task;
mod types;

pub use crate::dynamic_workflow::register_dynamic_workflow;
pub use agent_dir_script_tool::AgentDirScriptTool;
pub use artifacts::{ArtifactStore, ArtifactStoreLimits, ToolArtifact};
pub use builtin::{
    register_generate_object, register_program, register_program_with_catalog, register_task,
    register_task_with_mcp, register_task_with_mcp_managers,
};
pub(crate) use builtin::{register_skill, register_task_with_mcp_managers_and_scheduler};
pub(crate) use invocation::{
    registry_tool_invoker, HostDirectPolicy, InvocationOrigin, ToolInvocation, ToolInvoker,
};
pub(crate) use presentation::{
    canonical_source as canonical_presentation_source, estimated_definition_tokens,
    is_definition_subset,
};
pub use presentation::{
    ToolPresentationError, ToolPresentationModeV1, ToolPresentationProfileV1,
    TOOL_PRESENTATION_PROFILE_V1_SCHEMA,
};
pub use program_tool::{ProgramTool, MAX_PROGRAM_SCRIPT_SOURCE_BYTES};
pub use registry::ToolRegistry;
pub(crate) use registry::ToolRegistrySnapshotError;
pub use result_transform::{
    ToolResultTransformPolicyV1, TOOL_RESULT_TRANSFORM_ALGORITHM_V1,
    TOOL_RESULT_TRANSFORM_SCHEMA_V1,
};
pub(crate) use selector::is_standalone_conversation;
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
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    pub loss_mode: ToolResultLossModeV1,
}

#[cfg(test)]
pub(crate) fn truncate_tool_output_with_artifact(
    tool_name: &str,
    output: &str,
) -> TruncatedToolOutput {
    transform_tool_output_with_artifact(
        tool_name,
        output,
        &ToolResultTransformPolicyV1::conservative(),
    )
}

pub(crate) fn transform_tool_output_with_artifact(
    tool_name: &str,
    output: &str,
    policy: &ToolResultTransformPolicyV1,
) -> TruncatedToolOutput {
    let transformed = result_transform::transform(output, policy);
    if transformed.loss_mode == ToolResultLossModeV1::None {
        return TruncatedToolOutput {
            content: transformed.content,
            artifact: None,
            loss_mode: transformed.loss_mode,
        };
    }
    let artifact = tool_output_artifact(tool_name, output, transformed.retained_original_bytes);
    let artifact_uri = artifact.artifact_uri.clone();
    let content = format!(
        "{}\n\n[Full output artifact: {artifact_uri}]",
        transformed.content
    );

    TruncatedToolOutput {
        content,
        artifact: Some(artifact),
        loss_mode: transformed.loss_mode,
    }
}

pub(crate) fn tool_output_artifact(
    tool_name: &str,
    output: &str,
    shown_bytes: usize,
) -> ToolOutputArtifact {
    let digest = format!("{:x}", Sha256::digest(output.as_bytes()));
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
    let artifact_id = format!("tool-output:{sanitized_tool}:{digest}");
    let artifact_uri = format!("a3s://tool-output/{sanitized_tool}/{digest}");

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

pub const TOOL_RESULT_EVIDENCE_SCHEMA_V1: &str = "a3s.code.tool-result-evidence.v1";
pub const TOOL_RESULT_TOKEN_ESTIMATOR_V1: &str = "utf8-bytes-ceil-div-4/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResultEvidenceV1 {
    pub schema: String,
    pub original_bytes: usize,
    pub projected_bytes: usize,
    pub original_estimated_tokens: usize,
    pub projected_estimated_tokens: usize,
    pub token_estimator: String,
    /// Versioned transform algorithm. `None` is accepted only when reading
    /// evidence emitted before CAR-03 extended the unreleased v1 schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform_algorithm: Option<String>,
    pub content_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_digest: Option<String>,
    pub repeat_key: String,
    pub content_ref: String,
    pub loss_mode: ToolResultLossModeV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_delta: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_token_delta: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultLossModeV1 {
    None,
    BoundedPreview,
    HeadTail,
    DeterministicTransform,
    Composite,
}

pub(crate) fn attach_tool_result_evidence(
    metadata: Option<serde_json::Value>,
    original: &str,
    projected: &str,
    loss_mode: ToolResultLossModeV1,
) -> serde_json::Value {
    let digest = format!("sha256:{:x}", Sha256::digest(original.as_bytes()));
    let projected_digest = format!("sha256:{:x}", Sha256::digest(projected.as_bytes()));
    let artifact_uri = metadata
        .as_ref()
        .and_then(|value| value.pointer("/artifact/artifact_uri"))
        .and_then(serde_json::Value::as_str);
    let evidence = ToolResultEvidenceV1 {
        schema: TOOL_RESULT_EVIDENCE_SCHEMA_V1.to_string(),
        original_bytes: original.len(),
        projected_bytes: projected.len(),
        original_estimated_tokens: estimated_text_tokens(original),
        projected_estimated_tokens: estimated_text_tokens(projected),
        token_estimator: TOOL_RESULT_TOKEN_ESTIMATOR_V1.to_string(),
        transform_algorithm: Some(TOOL_RESULT_TRANSFORM_ALGORITHM_V1.to_string()),
        content_digest: digest.clone(),
        projected_digest: Some(projected_digest),
        repeat_key: digest.clone(),
        content_ref: artifact_uri
            .map(str::to_owned)
            .unwrap_or_else(|| format!("inline:{digest}")),
        loss_mode,
        byte_delta: Some(signed_delta(projected.len(), original.len())),
        estimated_token_delta: Some(signed_delta(
            estimated_text_tokens(projected),
            estimated_text_tokens(original),
        )),
    };
    let evidence = serde_json::json!({
        "schema": evidence.schema,
        "original_bytes": evidence.original_bytes,
        "projected_bytes": evidence.projected_bytes,
        "original_estimated_tokens": evidence.original_estimated_tokens,
        "projected_estimated_tokens": evidence.projected_estimated_tokens,
        "token_estimator": evidence.token_estimator,
        "transform_algorithm": evidence.transform_algorithm,
        "content_digest": evidence.content_digest,
        "projected_digest": evidence.projected_digest,
        "repeat_key": evidence.repeat_key,
        "content_ref": evidence.content_ref,
        "loss_mode": evidence.loss_mode,
        "byte_delta": evidence.byte_delta,
        "estimated_token_delta": evidence.estimated_token_delta,
    });
    match metadata {
        Some(serde_json::Value::Object(mut object)) => {
            object.insert("a3s_tool_result_evidence".to_string(), evidence);
            serde_json::Value::Object(object)
        }
        Some(value) => serde_json::json!({
            "a3s_tool_result_evidence": evidence,
            "previous_metadata": value,
        }),
        None => serde_json::json!({"a3s_tool_result_evidence": evidence}),
    }
}

pub(crate) fn ensure_tool_result_evidence(
    metadata: Option<serde_json::Value>,
    output: &str,
) -> serde_json::Value {
    match metadata {
        Some(value) if value.get("a3s_tool_result_evidence").is_some() => value,
        metadata => {
            attach_tool_result_evidence(metadata, output, output, ToolResultLossModeV1::None)
        }
    }
}

pub(crate) fn has_tool_metadata_beyond_evidence(metadata: Option<&serde_json::Value>) -> bool {
    match metadata {
        None => false,
        Some(serde_json::Value::Object(object)) => {
            object.keys().any(|key| key != "a3s_tool_result_evidence")
        }
        Some(_) => true,
    }
}

fn estimated_text_tokens(value: &str) -> usize {
    value.len().saturating_add(3) / 4
}

fn signed_delta(value: usize, baseline: usize) -> i64 {
    if value >= baseline {
        i64::try_from(value - baseline).unwrap_or(i64::MAX)
    } else {
        -i64::try_from(baseline - value).unwrap_or(i64::MAX)
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
/// the "never log secrets" boundary. Full argument values are intentionally
/// absent at every log level because search queries and file content may carry
/// workspace-sensitive material.
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
            "read" | "write" | "edit" | "patch" | "download" => Some("file_path"),
            "ls" | "search" | "code_symbols" | "code_navigation" | "code_diagnostics" => {
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

    pub(crate) fn snapshot_with_external_tools(
        &self,
        external: impl IntoIterator<Item = Arc<dyn Tool>>,
    ) -> Result<Self, ToolRegistrySnapshotError> {
        Ok(Self {
            workspace: self.workspace.clone(),
            registry: Arc::new(self.registry.snapshot_with_external_tools(external)?),
            command_env: self.command_env.clone(),
        })
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
    /// Session hosts should use [`crate::AgentSession::tool`] only for already
    /// authorized control-plane calls, or [`crate::AgentSession::governed_tool`]
    /// when permission and HITL must still apply. Agent runtimes must dispatch
    /// through their scoped tool invocation gateway.
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
