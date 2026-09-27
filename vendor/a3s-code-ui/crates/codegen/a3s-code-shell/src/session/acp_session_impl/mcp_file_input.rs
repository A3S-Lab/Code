//! Source-authorized MCP loading and immutable dispatch payloads.

use a3s_code_tools::{
    implementations::use_tool::{
        InlineMcpInvocation, UseToolInput, parse_arguments_file, validate_mcp_target,
    },
    types::{
        ToolInput,
        resources::{FileSystem, resolve_model_path},
    },
};
use a3s_code_workspace::permission::{AccessKind, Decision, PermissionRequest};
use agent_client_protocol as acp;
use serde_json::Value;
use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

use crate::session::acp_session::{PreparedToolCall, SessionActor, ToolLoop};

pub(super) const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_BATCH_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_BATCH_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;
const FILE_OPERATION_TIMEOUT: Duration = Duration::from_secs(10);

fn log_file_event<T: a3s_code_telemetry::TelemetryEvent>(event: T) {
    #[cfg(test)]
    if tests::record_event(&event) {
        return;
    }
    a3s_code_telemetry::session_ctx::log_event(event);
}

#[derive(Debug)]
pub(super) struct McpFileSource {
    pub(super) path: PathBuf,
    pub(super) bytes: usize,
    snapshot_bytes: usize,
    operation_remaining: Duration,
    pub(super) kind: a3s_code_telemetry::events::McpFileInputKind,
    model_id: String,
    pub(super) started: Instant,
    completed: std::sync::atomic::AtomicBool,
}

impl Drop for McpFileSource {
    fn drop(&mut self) {
        self.complete(false);
    }
}

impl McpFileSource {
    fn start(
        path: PathBuf,
        kind: a3s_code_telemetry::events::McpFileInputKind,
        model_id: String,
    ) -> Self {
        log_file_event(a3s_code_telemetry::events::McpFileInputUsed {
            kind,
            model_id: model_id.clone(),
        });
        McpFileSource {
            path,
            kind,
            model_id,
            bytes: 0,
            snapshot_bytes: 0,
            operation_remaining: FILE_OPERATION_TIMEOUT,
            started: Instant::now(),
            completed: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[derive(Clone)]
pub(crate) struct PreparedMcpFile {
    source: std::sync::Arc<McpFileSource>,
    arguments: std::sync::Arc<Value>,
}

impl std::fmt::Debug for PreparedMcpFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedMcpFile").finish_non_exhaustive()
    }
}

impl PreparedMcpFile {
    pub(super) async fn freeze(
        mut source: McpFileSource,
        arguments: Value,
    ) -> Result<Self, String> {
        let arguments = std::sync::Arc::new(arguments);
        let measured_arguments = arguments.clone();
        let measured =
            tokio_util::task::AbortOnDropHandle::new(tokio::task::spawn_blocking(move || {
                let mut counter = SnapshotCounter { bytes: 0 };
                serde_json::to_writer(&mut counter, measured_arguments.as_ref())
                    .map(|()| counter.bytes)
                    .map_err(|_| counter.bytes)
            }))
            .await
            .map_err(|_| "MCP snapshot worker failed".to_owned())?;
        source.snapshot_bytes = match measured {
            Ok(bytes) | Err(bytes) => bytes,
        };
        measured.map_err(|observed| {
            source.log_limit(
                a3s_code_telemetry::events::McpFileLimitKind::Snapshots,
                MAX_BATCH_SNAPSHOT_BYTES,
                observed,
            );
            "MCP effective invocation exceeds the 32 MiB snapshot limit".to_owned()
        })?;
        Ok(PreparedMcpFile {
            source: std::sync::Arc::new(source),
            arguments,
        })
    }

    pub(super) fn arguments(&self) -> &Value {
        &self.arguments
    }

    pub(super) fn complete(&self, success: bool) {
        self.source.complete(success);
    }
}

impl McpFileSource {
    fn complete(&self, success: bool) {
        if self
            .completed
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        log_file_event(a3s_code_telemetry::events::McpFileInputCompleted {
            kind: self.kind,
            outcome: if success {
                a3s_code_telemetry::events::McpFileInputOutcome::Success
            } else {
                a3s_code_telemetry::events::McpFileInputOutcome::Failed
            },
            source_bytes: self.bytes as u64,
            snapshot_bytes: self.snapshot_bytes as u64,
            duration_ms: self.started.elapsed().as_millis() as u64,
            model_id: self.model_id.clone(),
        });
    }

    fn log_limit(
        &self,
        kind: a3s_code_telemetry::events::McpFileLimitKind,
        limit: usize,
        observed: usize,
    ) {
        log_file_event(a3s_code_telemetry::events::McpFileInputLimitHit {
            kind,
            limit_bytes: limit as u64,
            observed_bytes: observed as u64,
            model_id: self.model_id.clone(),
        });
    }
}

struct SnapshotCounter {
    bytes: usize,
}
impl io::Write for SnapshotCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > MAX_BATCH_SNAPSHOT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "snapshot limit",
            ));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct McpFileBatchBudget {
    source_bytes: usize,
    snapshot_bytes: usize,
}
impl McpFileBatchBudget {
    pub(super) fn admit(&mut self, prepared: &PreparedToolCall) -> Result<(), String> {
        let Some(file) = &prepared.mcp_file else {
            return Ok(());
        };
        for (kind, observed, limit) in [
            (
                a3s_code_telemetry::events::McpFileLimitKind::Sources,
                self.source_bytes.saturating_add(file.source.bytes),
                MAX_BATCH_SOURCE_BYTES,
            ),
            (
                a3s_code_telemetry::events::McpFileLimitKind::Snapshots,
                self.snapshot_bytes
                    .saturating_add(file.source.snapshot_bytes),
                MAX_BATCH_SNAPSHOT_BYTES,
            ),
        ] {
            if observed > limit {
                file.source.log_limit(kind, limit, observed);
                return Err("MCP file input batch budget exceeded".to_owned());
            }
        }
        self.source_bytes += file.source.bytes;
        self.snapshot_bytes += file.source.snapshot_bytes;
        Ok(())
    }
}

pub(super) enum McpFilePreparation {
    Ordinary,
    Resolved {
        source: McpFileSource,
        authored: Value,
        authored_json: String,
    },
}

pub(super) struct PreparedArgumentViews {
    pub(super) authored_json: String,
    pub(super) authored: Value,
    pub(super) file: Option<PreparedMcpFile>,
}

impl McpFilePreparation {
    pub(super) fn validate_rewrite(&self, input: &ToolInput) -> Result<(), String> {
        if matches!(self, McpFilePreparation::Resolved { .. })
            && !matches!(input, ToolInput::UseTool(UseToolInput::Inline(input)) if input.tool_input.is_object())
        {
            return Err("file-backed invocation requires object arguments".to_owned());
        }
        Ok(())
    }

    pub(super) fn requires_resolved_permission(&self) -> bool {
        matches!(self, McpFilePreparation::Resolved { .. })
    }

    pub(super) async fn approval(
        &self,
        actor: &SessionActor,
        id: &acp::ToolCallId,
        wire_name: &str,
        input: &mut ToolInput,
    ) -> Result<(String, acp::ToolKind, Value), acp::Error> {
        let preview = match (self, &mut *input) {
            (
                McpFilePreparation::Resolved { source, .. },
                ToolInput::UseTool(UseToolInput::Inline(invocation)),
            ) => {
                if source.operation_remaining.is_zero() {
                    return Err(acp::Error::internal_error().data("MCP approval preview timed out"));
                }
                let arguments = std::mem::take(&mut invocation.tool_input);
                let (arguments, preview) = tokio::time::timeout(
                    source.operation_remaining,
                    tokio_util::task::AbortOnDropHandle::new(tokio::task::spawn_blocking(
                        move || {
                            let (preview, _) =
                                a3s_code_hooks::event::truncate_payload(arguments.clone());
                            (arguments, preview)
                        },
                    )),
                )
                .await
                .map_err(|_| acp::Error::internal_error().data("MCP approval preview timed out"))?
                .map_err(|_| {
                    acp::Error::internal_error().data("MCP approval preview worker failed")
                })?;
                invocation.tool_input = arguments;
                ToolInput::UseTool(UseToolInput::Inline(InlineMcpInvocation {
                    tool_name: invocation.tool_name.clone(),
                    tool_input: preview,
                }))
            }
            (_, input) => input.clone(),
        };
        let (mut title, kind, preview) = actor.send_tool_call_start(id, wire_name, preview).await?;
        if let McpFilePreparation::Resolved { source, .. } = self {
            title = format!("{title} — source file: {}", source.path.display());
            actor
                .send_update(
                    acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                        id.clone(),
                        acp::ToolCallUpdateFields::new()
                            .title(Some(title.clone()))
                            .kind(Some(kind))
                            .raw_input(Some(preview.clone())),
                    )),
                    None,
                )
                .await;
        }
        Ok((title, kind, preview))
    }

    pub(super) async fn finish(
        self,
        effective: Value,
        effective_json: String,
    ) -> Result<PreparedArgumentViews, String> {
        match self {
            McpFilePreparation::Ordinary => Ok(PreparedArgumentViews {
                authored_json: effective_json,
                authored: effective,
                file: None,
            }),
            McpFilePreparation::Resolved {
                source,
                authored,
                authored_json,
            } => Ok(PreparedArgumentViews {
                authored,
                authored_json,
                file: Some(PreparedMcpFile::freeze(source, effective).await?),
            }),
        }
    }
}

impl PreparedToolCall {
    pub(super) fn authored_arguments(&self) -> &Value {
        &self.parsed_args
    }

    pub(super) fn execution_arguments(&self) -> &Value {
        self.mcp_file
            .as_ref()
            .map_or(&self.parsed_args, PreparedMcpFile::arguments)
    }

    pub(super) fn hook_arguments(&self) -> std::borrow::Cow<'_, Value> {
        if self.mcp_file.is_some() {
            std::borrow::Cow::Borrowed(self.execution_arguments())
        } else {
            std::borrow::Cow::Owned(
                serde_json::from_str(&self.raw_arguments).unwrap_or(Value::Null),
            )
        }
    }
}

pub(super) fn recover_concatenated_inline(
    toolset: &a3s_code_tools::registry::types::FinalizedToolset,
    tool_name: &str,
    arguments: &str,
) -> Result<UseToolInput, a3s_tool_runtime::ToolError> {
    // Legacy recovery executes the first object; its raw keys must survive validation.
    let first = serde_json::Deserializer::from_str(arguments)
        .into_iter::<&serde_json::value::RawValue>()
        .next()
        .ok_or_else(|| a3s_tool_runtime::ToolError::invalid_arguments("Missing inline MCP call"))?
        .map_err(|error| a3s_tool_runtime::ToolError::invalid_arguments(error.to_string()))?;
    let input = toolset.parse_mcp_wrapper_json(tool_name, first.get())?;
    if !matches!(&input, UseToolInput::Inline(_)) {
        return Err(a3s_tool_runtime::ToolError::invalid_arguments(
            "Concatenated MCP recovery supports inline calls only",
        ));
    }
    Ok(input)
}

impl SessionActor {
    pub(super) async fn resolve_mcp_file(
        &self,
        call: &crate::sampling::types::ToolCallResponse,
        tool_call_id: &acp::ToolCallId,
        input: &UseToolInput,
        model_id: &str,
    ) -> Result<Result<(ToolInput, Value, McpFileSource), ToolLoop>, acp::Error> {
        let Some(path) = input.source_path() else {
            return Err(acp::Error::internal_error().data("expected file-backed invocation"));
        };
        let kind = match input {
            UseToolInput::ArgumentsFile { .. } => {
                a3s_code_telemetry::events::McpFileInputKind::Arguments
            }
            UseToolInput::InvocationFile { .. } => {
                a3s_code_telemetry::events::McpFileInputKind::Invocation
            }
            UseToolInput::Inline(_) => return Err(acp::Error::internal_error()),
        };
        let mut source = McpFileSource::start(path.to_path_buf(), kind, model_id.to_owned());
        let resources = self.tool_bridge_handle().shared_resources().await;
        let fs = resources
            .lock()
            .await
            .get::<FileSystem>()
            .map(|fs| fs.0.clone());
        let Some(fs) = fs.filter(|fs| fs.supports_bounded_read()) else {
            return self.reject_mcp_file(call, tool_call_id, "File-backed MCP input is unsupported by this filesystem backend; no local fallback is allowed").await;
        };
        let path_context = a3s_code_workspace::permission::types::RequestPathContext {
            real_cwd: PathBuf::from(self.session_info.cwd.as_str()),
            display_cwd: self.display_cwd.get().map(PathBuf::from),
        };
        let logical = resolve_model_path(
            &path_context.real_cwd,
            path_context.display_cwd.as_deref(),
            &path.to_string_lossy(),
        );
        if let Some(blocked) = self
            .authorize_mcp_source(call, tool_call_id, &logical, &path_context)
            .await?
        {
            return Ok(Err(blocked));
        }
        let operation_start = tokio::time::Instant::now();
        let resolved = tokio::time::timeout(FILE_OPERATION_TIMEOUT, async {
            let (physical, _) =
                a3s_code_tools::util::read_policy::resolve_read_path(&logical).await;
            a3s_code_tools::util::read_policy::validate_read_paths(
                &resources,
                &logical,
                &physical,
                Some(&logical),
            )
            .await?;
            Ok::<_, String>(physical)
        })
        .await;
        let physical = match resolved {
            Ok(Ok(physical)) => physical,
            Ok(Err(error)) => return self.reject_mcp_file(call, tool_call_id, &error).await,
            Err(_) => {
                return self
                    .reject_mcp_file(call, tool_call_id, "MCP source file operation timed out")
                    .await;
            }
        };
        let remaining = FILE_OPERATION_TIMEOUT.saturating_sub(operation_start.elapsed());
        if remaining.is_zero() {
            return self
                .reject_mcp_file(call, tool_call_id, "MCP source file operation timed out")
                .await;
        }
        if physical != logical
            && let Some(blocked) = self
                .authorize_mcp_source(call, tool_call_id, &physical, &path_context)
                .await?
        {
            return Ok(Err(blocked));
        }
        let input = input.clone();
        let projection_toolset = self.tool_bridge_handle().toolset();
        let wire_name = call.function.name.clone();
        let load_start = tokio::time::Instant::now();
        let loaded = tokio::time::timeout(remaining, async {
            let bytes = fs
                .read_file_bounded(&physical, MAX_SOURCE_BYTES)
                .await
                .map_err(|error| {
                    if error.io_error_kind() == Some(io::ErrorKind::FileTooLarge) {
                        source.log_limit(
                            a3s_code_telemetry::events::McpFileLimitKind::Source,
                            MAX_SOURCE_BYTES,
                            MAX_SOURCE_BYTES + 1,
                        );
                    }
                    format!("Cannot read complete MCP source file: {error}")
                })?;
            source.bytes = bytes.len();
            tokio_util::task::AbortOnDropHandle::new(tokio::task::spawn_blocking(move || {
                let document = String::from_utf8(bytes)
                    .map_err(|_| "MCP source file is not valid UTF-8".to_owned())?;
                let invocation = match input {
                    UseToolInput::ArgumentsFile { tool_name, .. } => {
                        parse_arguments_file(&document).map(|tool_input| InlineMcpInvocation {
                            tool_name,
                            tool_input,
                        })
                    }
                    UseToolInput::InvocationFile { .. } => {
                        UseToolInput::from_invocation_file(&document)
                    }
                    UseToolInput::Inline(_) => Err("expected file-backed invocation".to_owned()),
                }?;
                let effective = UseToolInput::Inline(invocation);
                let model_arguments = projection_toolset
                    .model_mcp_arguments(&wire_name, &effective)
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>((effective, model_arguments))
            }))
            .await
            .map_err(|_| "MCP source parsing worker failed".to_owned())?
        })
        .await;
        source.operation_remaining = remaining.saturating_sub(load_start.elapsed());
        let (effective, model_arguments) = match loaded {
            Ok(Ok(loaded)) => loaded,
            Ok(Err(error)) => return self.reject_mcp_file(call, tool_call_id, &error).await,
            Err(_) => {
                return self
                    .reject_mcp_file(call, tool_call_id, "MCP source file operation timed out")
                    .await;
            }
        };
        let target_name = effective
            .target_name()
            .ok_or_else(acp::Error::internal_error)?;
        if validate_mcp_target(Some(&resources), target_name)
            .await
            .is_err()
        {
            return self.reject_mcp_file(call, tool_call_id, "MCP source target is not an eligible integration tool; discover its schema before calling").await;
        }
        let toolset = self.tool_bridge_handle().toolset();
        let eligible = match toolset.is_mcp_target(target_name) {
            Some(is_mcp) => is_mcp,
            None => resources
                .lock()
                .await
                .get::<a3s_code_tools::types::resources::ManagedGatewayToolCatalog>()
                .is_some_and(|catalog| catalog.get(target_name).is_some()),
        };
        if !eligible {
            return self
                .reject_mcp_file(
                    call,
                    tool_call_id,
                    "MCP source target is not an available MCP integration tool",
                )
                .await;
        }
        Ok(Ok((ToolInput::UseTool(effective), model_arguments, source)))
    }

    async fn authorize_mcp_source(
        &self,
        call: &crate::sampling::types::ToolCallResponse,
        id: &acp::ToolCallId,
        path: &std::path::Path,
        context: &a3s_code_workspace::permission::types::RequestPathContext,
    ) -> Result<Option<ToolLoop>, acp::Error> {
        let update = acp::ToolCallUpdate::new(
            id.clone(),
            acp::ToolCallUpdateFields::new()
                .title(Some(format!("Read MCP source: {}", path.display())))
                .kind(Some(acp::ToolKind::Read))
                .raw_input(Some(serde_json::json!({"source_file": path}))),
        );
        self.send_update(acp::SessionUpdate::ToolCallUpdate(update.clone()), None)
            .await;
        let resolution = {
            let _pending = crate::session::pending_interaction::PendingInteractionGuard::new(
                self.pending_interactions.clone(),
                self.notifications.gateway.clone(),
                self.session_info.id.clone(),
                id.to_string(),
                crate::session::pending_interaction::PendingKind::Permission,
            );
            self.permissions
                .request(PermissionRequest {
                    path_context: Some(context.clone()),
                    session_id: Some(self.session_info.id.0.to_string()),
                    ..PermissionRequest::new(
                        AccessKind::Read(Some(path.to_string_lossy().into_owned())),
                        update,
                    )
                })
                .await
        };
        let message = match &resolution.decision {
            Decision::PolicyDeny(reason) | Decision::Reject(reason) => {
                format!("MCP source Read denied: {reason}")
            }
            Decision::Allow
            | Decision::Ask
            | Decision::Cancelled
            | Decision::FollowupMessage(_) => {
                "MCP source Read permission was not granted; no MCP call was sent".to_owned()
            }
        };
        let action = match resolution.decision {
            Decision::Allow => return Ok(None),
            Decision::PolicyDeny(_) | Decision::Ask => ToolLoop::Continue,
            Decision::Reject(reason) => ToolLoop::PermissionReject {
                tool_name: call.function.name.clone(),
                reason,
            },
            Decision::Cancelled => ToolLoop::Cancelled,
            Decision::FollowupMessage(message) => ToolLoop::FollowupMessage(message),
        };
        self.handle_tool_not_executed(&call.id, id, message).await?;
        Ok(Some(action))
    }

    async fn reject_mcp_file<T>(
        &self,
        call: &crate::sampling::types::ToolCallResponse,
        id: &acp::ToolCallId,
        error: &str,
    ) -> Result<Result<T, ToolLoop>, acp::Error> {
        self.handle_tool_not_executed(&call.id, id, error.to_owned())
            .await?;
        Ok(Err(ToolLoop::Continue))
    }
}

#[cfg(test)]
#[path = "mcp_file_input_tests.rs"]
mod tests;
