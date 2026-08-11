//! Tool Registry
//!
//! Central registry for all tools (built-in and dynamic).
//! Provides thread-safe registration, lookup, and execution.

use super::artifacts::{ArtifactStore, ArtifactStoreLimits, ToolArtifact};
use super::types::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use super::ToolResult;
use super::{
    merge_tool_output_artifact_metadata, tool_output_artifact, truncate_tool_output_with_artifact,
    ToolOutputArtifact,
};
use crate::llm::ToolDefinition;
use crate::trace::{InMemoryTraceSink, TraceEvent, TraceSink};
use anyhow::Result;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

const MAX_TOOL_SCHEMA_BYTES: usize = 256 * 1024;
const MAX_ARGUMENT_VALIDATION_ERRORS: usize = 8;
const MAX_ARGUMENT_VALIDATION_MESSAGE_BYTES: usize = 4 * 1024;
const MAX_INLINE_CHANGE_BYTES: usize = 64 * 1024;
const CHANGE_SIDE_PREVIEW_BYTES: usize = 8 * 1024;
const CHANGE_DIFF_PREVIEW_BYTES: usize = 32 * 1024;
const MAX_DIFF_COMPUTE_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
enum CachedArgumentValidator {
    Valid(Arc<jsonschema::Validator>),
    Invalid(String),
}

#[derive(Clone)]
struct ArgumentValidatorCacheEntry {
    schema_fingerprint: u64,
    validator: CachedArgumentValidator,
}

/// Tool registry for managing all available tools
pub struct ToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    /// Names of builtin tools that cannot be overridden
    builtins: RwLock<std::collections::HashSet<String>>,
    context: RwLock<ToolContext>,
    artifact_store: ArtifactStore,
    trace_sink: RwLock<Arc<dyn TraceSink>>,
    argument_validators: RwLock<HashMap<String, ArgumentValidatorCacheEntry>>,
}

impl ToolRegistry {
    /// Create a new tool registry
    pub fn new(workspace: PathBuf) -> Self {
        Self::with_artifact_limits(workspace, ArtifactStoreLimits::default())
    }

    /// Create a new tool registry with custom artifact retention limits.
    pub fn with_artifact_limits(workspace: PathBuf, artifact_limits: ArtifactStoreLimits) -> Self {
        Self::with_artifact_limits_and_workspace_services(
            workspace.clone(),
            artifact_limits,
            crate::workspace::WorkspaceServices::local(workspace),
        )
    }

    /// Create a new tool registry with custom artifact limits and workspace backend.
    pub fn with_artifact_limits_and_workspace_services(
        workspace: PathBuf,
        artifact_limits: ArtifactStoreLimits,
        workspace_services: Arc<crate::workspace::WorkspaceServices>,
    ) -> Self {
        let context = ToolContext::new(workspace).with_workspace_services(workspace_services);
        Self {
            tools: RwLock::new(HashMap::new()),
            builtins: RwLock::new(std::collections::HashSet::new()),
            context: RwLock::new(context),
            artifact_store: ArtifactStore::with_limits(artifact_limits),
            trace_sink: RwLock::new(Arc::new(InMemoryTraceSink::default())),
            argument_validators: RwLock::new(HashMap::new()),
        }
    }

    /// Register a builtin tool (cannot be overridden by dynamic tools)
    pub fn register_builtin(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        let mut tools = self.tools.write().unwrap();
        let mut builtins = self.builtins.write().unwrap();
        tracing::debug!("Registering builtin tool: {}", name);
        tools.insert(name.clone(), tool);
        builtins.insert(name);
    }

    /// Register a tool
    ///
    /// If a tool with the same name already exists as a builtin, the registration
    /// is rejected to prevent shadowing of core tools.
    pub fn register(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        // All operations that need both registry locks take `tools` first.
        // This keeps the builtin check and insertion atomic with
        // `register_builtin` and avoids lock-order inversion.
        let mut tools = self.tools.write().unwrap();
        let builtins = self.builtins.read().unwrap();
        if builtins.contains(&name) {
            tracing::warn!(
                "Rejected registration of tool '{}': cannot shadow builtin",
                name
            );
            return;
        }
        tracing::debug!("Registering tool: {}", name);
        tools.insert(name, tool);
    }

    /// Register a dynamic tool and return the tool it shadowed.
    ///
    /// The lookup and replacement happen under one write lock so lifecycle
    /// owners can later restore the exact prior registration without racing a
    /// concurrent dynamic registration. The boolean is `false` when a builtin
    /// owns the name and the dynamic registration was rejected.
    pub(crate) fn register_with_shadow(
        &self,
        tool: Arc<dyn Tool>,
    ) -> (bool, Option<Arc<dyn Tool>>) {
        let name = tool.name().to_string();
        let mut tools = self.tools.write().unwrap();
        let builtins = self.builtins.read().unwrap();
        if builtins.contains(&name) {
            tracing::warn!(
                "Rejected registration of tool '{}': cannot shadow builtin",
                name
            );
            return (false, None);
        }
        tracing::debug!("Registering owned dynamic tool: {}", name);
        (true, tools.insert(name, tool))
    }

    /// Restore a shadowed registration only while `expected` still owns the
    /// name.
    ///
    /// This compare-and-replace prevents one lifecycle owner from deleting or
    /// overwriting a tool installed later by another dynamic source.
    pub(crate) fn restore_if_same(
        &self,
        name: &str,
        expected: &Arc<dyn Tool>,
        replacement: Option<Arc<dyn Tool>>,
    ) -> bool {
        let mut tools = self.tools.write().unwrap();
        let Some(current) = tools.get(name) else {
            return false;
        };
        if !Arc::ptr_eq(current, expected) {
            return false;
        }

        match replacement {
            Some(tool) => {
                tools.insert(name.to_string(), tool);
            }
            None => {
                tools.remove(name);
            }
        }
        true
    }

    /// Register a dynamic tool only when no source currently owns its name.
    pub(crate) fn register_if_absent(&self, tool: Arc<dyn Tool>) -> bool {
        let name = tool.name().to_string();
        let mut tools = self.tools.write().unwrap();
        if tools.contains_key(&name) {
            return false;
        }
        tracing::debug!("Registering previously absent dynamic tool: {}", name);
        tools.insert(name, tool);
        true
    }

    /// Unregister a tool by name
    ///
    /// Returns true if the tool was found and removed.
    pub fn unregister(&self, name: &str) -> bool {
        let mut tools = self.tools.write().unwrap();
        let builtins = self.builtins.read().unwrap();
        if builtins.contains(name) {
            tracing::warn!(
                "Rejected unregister of tool '{}': builtin tools cannot be removed through dynamic unregister",
                name
            );
            return false;
        }
        tracing::debug!("Unregistering tool: {}", name);
        tools.remove(name).is_some()
    }

    /// Unregister all tools whose names start with the given prefix.
    pub fn unregister_by_prefix(&self, prefix: &str) {
        let mut tools = self.tools.write().unwrap();
        let builtins = self.builtins.read().unwrap();
        tools.retain(|name, _| builtins.contains(name) || !name.starts_with(prefix));
        tracing::debug!("Unregistered tools with prefix: {}", prefix);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let tools = self.tools.read().unwrap();
        tools.get(name).cloned()
    }

    pub(crate) fn capabilities(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Option<ToolCapabilities> {
        self.get(name).map(|tool| tool.capabilities(args))
    }

    pub(crate) fn requires_confirmation(&self, name: &str, args: &serde_json::Value) -> bool {
        self.get(name)
            .is_some_and(|tool| tool.requires_confirmation(args))
    }

    /// Check if a tool exists
    pub fn contains(&self, name: &str) -> bool {
        let tools = self.tools.read().unwrap();
        tools.contains_key(name)
    }

    /// Get all tool definitions for LLM
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let tools = self.tools.read().unwrap();
        let mut definitions = tools
            .values()
            .filter(|tool| tool.is_model_visible())
            .map(|tool| tool.definition())
            .collect::<Vec<_>>();
        definitions.sort_by(|a, b| a.name.cmp(&b.name));
        definitions
    }

    /// List all registered tool names
    pub fn list(&self) -> Vec<String> {
        let tools = self.tools.read().unwrap();
        let mut names = tools.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    /// Validate model- or orchestrator-supplied arguments against the tool's
    /// declared JSON Schema before permissions or execution side effects.
    ///
    /// Low-level standalone registry calls remain compatibility-oriented and
    /// do not invoke this automatically. The governed agent/session gateway is
    /// the enforcement boundary.
    pub(crate) fn validate_arguments(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> std::result::Result<(), String> {
        let Some(tool) = self.get(name) else {
            return Ok(());
        };
        let schema = tool.parameters();
        let schema_bytes = serde_json::to_vec(&schema)
            .map_err(|error| format!("tool parameter schema is not serializable: {error}"))?;
        if schema_bytes.len() > MAX_TOOL_SCHEMA_BYTES {
            return Err(format!(
                "tool parameter schema exceeds the {} byte safety limit",
                MAX_TOOL_SCHEMA_BYTES
            ));
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        schema_bytes.hash(&mut hasher);
        let schema_fingerprint = hasher.finish();
        let cached = self
            .argument_validators
            .read()
            .unwrap()
            .get(name)
            .filter(|entry| entry.schema_fingerprint == schema_fingerprint)
            .cloned();
        let validator = match cached.map(|entry| entry.validator) {
            Some(CachedArgumentValidator::Valid(validator)) => validator,
            Some(CachedArgumentValidator::Invalid(error)) => return Err(error),
            None => {
                let compiled = match jsonschema::draft202012::options().build(&schema) {
                    Ok(validator) => CachedArgumentValidator::Valid(Arc::new(validator)),
                    Err(error) => CachedArgumentValidator::Invalid(format!(
                        "tool has an invalid parameter schema: {error}"
                    )),
                };
                self.argument_validators.write().unwrap().insert(
                    name.to_string(),
                    ArgumentValidatorCacheEntry {
                        schema_fingerprint,
                        validator: compiled.clone(),
                    },
                );
                match compiled {
                    CachedArgumentValidator::Valid(validator) => validator,
                    CachedArgumentValidator::Invalid(error) => return Err(error),
                }
            }
        };
        let mut errors = validator
            .iter_errors(args)
            .take(MAX_ARGUMENT_VALIDATION_ERRORS + 1)
            .map(|error| {
                let path = error.instance_path().to_string();
                if path.is_empty() {
                    format!("$: {error}")
                } else {
                    format!("{path}: {error}")
                }
            })
            .collect::<Vec<_>>();
        if errors.is_empty() {
            return Ok(());
        }

        let omitted = errors.len() > MAX_ARGUMENT_VALIDATION_ERRORS;
        errors.truncate(MAX_ARGUMENT_VALIDATION_ERRORS);
        let mut message = errors.join("; ");
        if omitted {
            message.push_str("; additional validation errors omitted");
        }
        Err(crate::text::truncate_utf8(&message, MAX_ARGUMENT_VALIDATION_MESSAGE_BYTES).to_string())
    }

    /// Get the number of registered tools
    pub fn len(&self) -> usize {
        let tools = self.tools.read().unwrap();
        tools.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the tool context
    pub fn context(&self) -> ToolContext {
        self.context.read().unwrap().clone()
    }

    /// Return a clone of the registry's artifact store handle.
    pub fn artifact_store(&self) -> ArtifactStore {
        self.artifact_store.clone()
    }

    /// Get a stored tool artifact by URI.
    pub fn get_artifact(&self, artifact_uri: &str) -> Option<ToolArtifact> {
        self.artifact_store.get(artifact_uri)
    }

    /// Replace the trace sink used for compact tool/program execution events.
    pub fn set_trace_sink(&self, sink: Arc<dyn TraceSink>) {
        *self.trace_sink.write().unwrap() = sink;
    }

    /// Return the current trace sink.
    pub fn trace_sink(&self) -> Arc<dyn TraceSink> {
        Arc::clone(&self.trace_sink.read().unwrap())
    }

    /// Set the search configuration for the tool context
    pub fn set_search_config(&self, config: crate::config::SearchConfig) {
        let mut ctx = self.context.write().unwrap();
        *ctx = ctx.clone().with_search_config(config);
    }

    /// Set a sandbox executor so that `bash` tool calls use the sandbox even
    /// when executed without an explicit `ToolContext` (i.e., via `execute()`).
    pub fn set_sandbox(&self, sandbox: std::sync::Arc<dyn crate::sandbox::BashSandbox>) {
        let mut ctx = self.context.write().unwrap();
        *ctx = ctx.clone().with_sandbox(sandbox);
    }

    /// Set environment overrides used by subprocess-backed tools when executed
    /// without an explicit context.
    pub fn set_command_env(&self, env: Arc<HashMap<String, String>>) {
        let mut ctx = self.context.write().unwrap();
        *ctx = ctx.clone().with_command_env(env);
    }

    /// Execute a tool by name using the registry's default context.
    ///
    /// This is the lowest-level standalone registry boundary. It does not run
    /// agent/session permission, HITL, hook, budget, queue, timeout,
    /// cancellation, or sanitization policy.
    pub async fn execute(&self, name: &str, args: &serde_json::Value) -> Result<ToolResult> {
        let ctx = self.context();
        self.execute_with_context(name, args, &ctx).await
    }

    /// Execute a tool by name with an external caller-owned context.
    ///
    /// This remains a low-level ungoverned call; agent/session paths must use
    /// their scoped invocation gateway instead.
    pub async fn execute_with_context(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult> {
        let start = std::time::Instant::now();

        let tool = self.get(name);

        let mut result = match tool {
            Some(tool) => {
                let mut output = tool.execute(args, ctx).await?;
                self.compact_change_metadata(name, &mut output.metadata);
                let original_content = output.content.clone();
                let truncated = truncate_tool_output_with_artifact(name, &output.content);
                output.content = truncated.content;
                if let Some(artifact) = truncated.artifact {
                    self.store_tool_artifact(name, &original_content, &artifact);
                    output.metadata = Some(merge_tool_output_artifact_metadata(
                        output.metadata,
                        &artifact,
                    ));
                }
                output.metadata = Some(super::attach_tool_result_evidence(
                    output.metadata,
                    &original_content,
                    &output.content,
                ));
                Ok(ToolResult {
                    name: name.to_string(),
                    output: output.content,
                    exit_code: if output.success { 0 } else { 1 },
                    metadata: output.metadata,
                    images: output.images,
                    error_kind: output.error_kind,
                })
            }
            None => Ok(ToolResult::error(name, format!("Unknown tool: {}", name))),
        };

        if let Ok(result) = &mut result {
            result.metadata = Some(super::ensure_tool_result_evidence(
                result.metadata.take(),
                &result.output,
            ));
        }

        if let Ok(ref r) = result {
            crate::telemetry::record_tool_result(r.exit_code, start.elapsed());
            self.record_trace_event(name, r, start.elapsed());
        }

        result
    }

    /// Execute a tool and return raw output using the registry's default context
    pub async fn execute_raw(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<Option<ToolOutput>> {
        let ctx = self.context();
        self.execute_raw_with_context(name, args, &ctx).await
    }

    /// Execute a tool and return raw output with an external context
    pub async fn execute_raw_with_context(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<Option<ToolOutput>> {
        let tool = self.get(name);

        match tool {
            Some(tool) => {
                let mut output = tool.execute(args, ctx).await?;
                self.compact_change_metadata(name, &mut output.metadata);
                let original_content = output.content.clone();
                let truncated = truncate_tool_output_with_artifact(name, &output.content);
                output.content = truncated.content;
                if let Some(artifact) = truncated.artifact {
                    self.store_tool_artifact(name, &original_content, &artifact);
                    output.metadata = Some(merge_tool_output_artifact_metadata(
                        output.metadata,
                        &artifact,
                    ));
                }
                output.metadata = Some(super::attach_tool_result_evidence(
                    output.metadata,
                    &original_content,
                    &output.content,
                ));
                Ok(Some(output))
            }
            None => Ok(None),
        }
    }

    fn store_tool_artifact(&self, tool_name: &str, content: &str, artifact: &ToolOutputArtifact) {
        self.artifact_store.put(ToolArtifact {
            artifact_id: artifact.artifact_id.clone(),
            artifact_uri: artifact.artifact_uri.clone(),
            tool_name: tool_name.to_string(),
            content: content.to_string(),
            original_bytes: artifact.original_bytes,
            shown_bytes: artifact.shown_bytes,
        });
    }

    fn compact_change_metadata(&self, tool_name: &str, metadata: &mut Option<serde_json::Value>) {
        let Some(serde_json::Value::Object(object)) = metadata.as_mut() else {
            return;
        };
        let before = object
            .get("before")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        let after = object
            .get("after")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        if before.is_none() && after.is_none() {
            return;
        }

        let before_bytes = before.as_ref().map_or(0, String::len);
        let after_bytes = after.as_ref().map_or(0, String::len);
        let total_bytes = before_bytes.saturating_add(after_bytes);
        let compacted = total_bytes > MAX_INLINE_CHANGE_BYTES;
        let before_artifact = before.as_deref().and_then(|content| {
            self.store_change_artifact(tool_name, "before", content, compacted)
        });
        let after_artifact = after
            .as_deref()
            .and_then(|content| self.store_change_artifact(tool_name, "after", content, compacted));

        let unified_diff = if compacted && total_bytes <= MAX_DIFF_COMPUTE_BYTES {
            let diff = similar::TextDiff::from_lines(
                before.as_deref().unwrap_or_default(),
                after.as_deref().unwrap_or_default(),
            )
            .unified_diff()
            .context_radius(3)
            .header("before", "after")
            .to_string();
            Some(bounded_head_tail(&diff, CHANGE_DIFF_PREVIEW_BYTES))
        } else {
            None
        };

        if compacted {
            if let Some(content) = before.as_deref() {
                object.insert(
                    "before".to_string(),
                    serde_json::Value::String(bounded_head_tail(
                        content,
                        CHANGE_SIDE_PREVIEW_BYTES,
                    )),
                );
            }
            if let Some(content) = after.as_deref() {
                object.insert(
                    "after".to_string(),
                    serde_json::Value::String(bounded_head_tail(
                        content,
                        CHANGE_SIDE_PREVIEW_BYTES,
                    )),
                );
            }
        }

        object.insert(
            "change".to_string(),
            serde_json::json!({
                "compacted": compacted,
                "before": before.as_deref().map(|content| serde_json::json!({
                    "bytes": content.len(),
                    "sha256": sha256::digest(content.as_bytes()),
                    "artifact": before_artifact,
                })),
                "after": after.as_deref().map(|content| serde_json::json!({
                    "bytes": content.len(),
                    "sha256": sha256::digest(content.as_bytes()),
                    "artifact": after_artifact,
                })),
                "unified_diff": unified_diff,
                "diff_omitted": compacted && total_bytes > MAX_DIFF_COMPUTE_BYTES,
            }),
        );
    }

    fn store_change_artifact(
        &self,
        tool_name: &str,
        side: &str,
        content: &str,
        store: bool,
    ) -> Option<serde_json::Value> {
        if !store || content.len() > self.artifact_store.limits().max_bytes {
            return None;
        }
        let artifact = tool_output_artifact(&format!("{tool_name}-{side}"), content, 0);
        self.store_tool_artifact(tool_name, content, &artifact);
        Some(serde_json::json!({
            "artifact_id": artifact.artifact_id,
            "artifact_uri": artifact.artifact_uri,
        }))
    }

    fn record_trace_event(&self, name: &str, result: &ToolResult, duration: std::time::Duration) {
        let sink = self.trace_sink();
        sink.record(TraceEvent::tool_execution(
            name,
            result.exit_code == 0,
            result.exit_code,
            duration,
            result.output.len(),
            result.metadata.as_ref(),
        ));

        if name == "program" {
            sink.record(TraceEvent::program_execution(
                name,
                result.exit_code == 0,
                result.exit_code,
                duration,
                result.output.len(),
                result.metadata.as_ref(),
            ));
        }
    }
}

fn bounded_head_tail(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }
    let head_limit = max_bytes / 2;
    let tail_limit = max_bytes.saturating_sub(head_limit);
    let head = crate::text::truncate_utf8(content, head_limit);
    let mut tail_start = content.len().saturating_sub(tail_limit);
    while tail_start < content.len() && !content.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}\n\n... [{} bytes omitted from middle] ...\n\n{}",
        head,
        content
            .len()
            .saturating_sub(head.len())
            .saturating_sub(content.len().saturating_sub(tail_start)),
        &content[tail_start..]
    )
}

#[cfg(test)]
#[path = "registry/tests.rs"]
mod tests;
