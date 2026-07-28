//! Versioned, machine-only JSONL bridge for the A3S Code Go SDK.
//!
//! A bridge process owns Rust `Agent` and `AgentSession` values for its entire
//! lifetime. Requests may complete out of order and are correlated by `id`.
//! Streaming requests emit zero or more `event` envelopes followed by exactly
//! one `response` envelope.

use a3s_code_core::{
    run_event_envelope_v1, Agent, AgentResult, AgentSession, CodeError, EventEnvelopeV1, Message,
    PlanningMode, ReadFileOptions, SessionOptions, SystemPromptSlots, ToolCallResult,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, RwLock};

pub const BRIDGE_PROTOCOL_VERSION: u16 = 1;

pub const BRIDGE_OPERATIONS: &[&str] = &[
    "sdk_capabilities",
    "agent_create",
    "agent_refresh_mcp_tools",
    "agent_list_sessions",
    "agent_close_session",
    "agent_is_closed",
    "agent_close",
    "session_create",
    "session_resume",
    "session_info",
    "session_is_closed",
    "session_send",
    "session_stream",
    "session_cancel",
    "session_cancel_and_settle",
    "session_history",
    "session_close",
    "session_save",
    "session_tool_names",
    "session_tool_definitions",
    "session_trace_events",
    "session_get_artifact",
    "session_read_file",
    "session_write_file",
    "session_ls",
    "session_edit_file",
    "session_patch_file",
    "session_bash",
    "session_glob",
    "session_grep",
    "session_tool",
    "session_runs",
    "session_run_snapshot",
    "session_run_events",
    "session_run_event_page",
    "session_current_run",
    "session_active_tools",
    "session_cancel_run",
    "session_pending_confirmations",
    "session_confirm_tool_use",
    "session_cancel_confirmations",
    "session_verification_reports",
    "session_verification_summary",
    "session_verification_summary_text",
    "session_verification_presets",
    "session_verify_commands",
    "session_register_agent_dir",
    "session_skill_names",
    "session_add_mcp_server",
    "session_remove_mcp_server",
    "session_mcp_status",
];

#[derive(Debug, Deserialize)]
pub struct BridgeRequest {
    pub protocol_version: u16,
    pub id: u64,
    pub operation: String,
    #[serde(default = "empty_object")]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeEnvelope {
    pub protocol_version: u16,
    pub id: u64,
    pub kind: &'static str,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<EventEnvelopeV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BridgeError>,
}

impl BridgeEnvelope {
    fn success(id: u64, result: Value) -> Self {
        Self {
            protocol_version: BRIDGE_PROTOCOL_VERSION,
            id,
            kind: "response",
            ok: true,
            result: Some(result),
            event: None,
            error: None,
        }
    }

    fn event(id: u64, event: EventEnvelopeV1) -> Self {
        Self {
            protocol_version: BRIDGE_PROTOCOL_VERSION,
            id,
            kind: "event",
            ok: true,
            result: None,
            event: Some(event),
            error: None,
        }
    }

    fn failure(id: u64, error: BridgeFailure) -> Self {
        Self {
            protocol_version: BRIDGE_PROTOCOL_VERSION,
            id,
            kind: "response",
            ok: false,
            result: None,
            event: None,
            error: Some(BridgeError {
                code: error.code,
                message: error.message,
            }),
        }
    }
}

#[derive(Debug)]
struct BridgeFailure {
    code: String,
    message: String,
}

impl BridgeFailure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<CodeError> for BridgeFailure {
    fn from(error: CodeError) -> Self {
        Self::new(error.code(), error.to_string())
    }
}

impl From<serde_json::Error> for BridgeFailure {
    fn from(error: serde_json::Error) -> Self {
        Self::new("INVALID_REQUEST", error.to_string())
    }
}

struct SessionEntry {
    agent_id: String,
    session: Arc<AgentSession>,
}

pub struct BridgeState {
    next_handle: AtomicU64,
    agents: RwLock<HashMap<String, Arc<Agent>>>,
    sessions: RwLock<HashMap<String, SessionEntry>>,
}

impl Default for BridgeState {
    fn default() -> Self {
        Self {
            next_handle: AtomicU64::new(1),
            agents: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
        }
    }
}

impl BridgeState {
    pub fn new() -> Self {
        Self::default()
    }

    fn handle(&self, prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            self.next_handle.fetch_add(1, Ordering::Relaxed)
        )
    }

    async fn agent(&self, id: &str) -> Result<Arc<Agent>, BridgeFailure> {
        self.agents
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| BridgeFailure::new("NOT_FOUND", format!("agent {id:?} was not found")))
    }

    async fn session(&self, id: &str) -> Result<Arc<AgentSession>, BridgeFailure> {
        self.sessions
            .read()
            .await
            .get(id)
            .map(|entry| Arc::clone(&entry.session))
            .ok_or_else(|| BridgeFailure::new("NOT_FOUND", format!("session {id:?} was not found")))
    }

    async fn dispatch(&self, request: &BridgeRequest) -> Result<Value, BridgeFailure> {
        if request.protocol_version != BRIDGE_PROTOCOL_VERSION {
            return Err(BridgeFailure::new(
                "PROTOCOL_ERROR",
                format!(
                    "unsupported bridge protocol version {}; expected {}",
                    request.protocol_version, BRIDGE_PROTOCOL_VERSION
                ),
            ));
        }
        if !request.params.is_object() {
            return Err(BridgeFailure::new(
                "INVALID_REQUEST",
                "params must be a JSON object",
            ));
        }

        match request.operation.as_str() {
            "sdk_capabilities" => Ok(json!({
                "protocol_version": BRIDGE_PROTOCOL_VERSION,
                "operations": BRIDGE_OPERATIONS,
                "event_protocol_version": a3s_code_core::EVENT_ENVELOPE_V1_VERSION,
            })),
            "agent_create" => {
                let config_source: String = required(&request.params, "config_source")?;
                let agent = Arc::new(Agent::create(config_source).await?);
                let agent_id = self.handle("agent");
                self.agents.write().await.insert(agent_id.clone(), agent);
                Ok(json!({ "agent_id": agent_id }))
            }
            "agent_refresh_mcp_tools" => {
                self.agent(&required::<String>(&request.params, "agent_id")?)
                    .await?
                    .refresh_mcp_tools()
                    .await?;
                Ok(json!({ "refreshed": true }))
            }
            "agent_list_sessions" => {
                let sessions = self
                    .agent(&required::<String>(&request.params, "agent_id")?)
                    .await?
                    .list_sessions()
                    .await;
                Ok(json!({ "session_ids": sessions }))
            }
            "agent_close_session" => {
                let agent_id: String = required(&request.params, "agent_id")?;
                let session_id: String = required(&request.params, "session_id")?;
                let closed = self
                    .agent(&agent_id)
                    .await?
                    .close_session(&session_id)
                    .await;
                Ok(json!({ "closed": closed }))
            }
            "agent_is_closed" => {
                let closed = self
                    .agent(&required::<String>(&request.params, "agent_id")?)
                    .await?
                    .is_closed();
                Ok(json!({ "closed": closed }))
            }
            "agent_close" => {
                let agent_id: String = required(&request.params, "agent_id")?;
                let sessions = self
                    .sessions
                    .read()
                    .await
                    .values()
                    .filter(|entry| entry.agent_id == agent_id)
                    .map(|entry| Arc::clone(&entry.session))
                    .collect::<Vec<_>>();
                for session in sessions {
                    session.close().await;
                }
                self.agent(&agent_id).await?.close().await;
                Ok(json!({ "closed": true }))
            }
            "session_create" => {
                let agent_id: String = required(&request.params, "agent_id")?;
                let workspace: String = required(&request.params, "workspace")?;
                let options = optional::<BridgeSessionOptions>(&request.params, "options")?
                    .unwrap_or_default()
                    .into_core()?;
                let session = Arc::new(
                    self.agent(&agent_id)
                        .await?
                        .session_async(workspace, Some(options))
                        .await?,
                );
                self.insert_session(agent_id, session).await
            }
            "session_resume" => {
                let agent_id: String = required(&request.params, "agent_id")?;
                let persisted_id: String = required(&request.params, "persisted_session_id")?;
                let options = optional::<BridgeSessionOptions>(&request.params, "options")?
                    .unwrap_or_default()
                    .into_core()?;
                let session = Arc::new(
                    self.agent(&agent_id)
                        .await?
                        .resume_session_async(&persisted_id, options)
                        .await?,
                );
                self.insert_session(agent_id, session).await
            }
            "session_info" => {
                let session = self.request_session(&request.params).await?;
                Ok(session_info(&session))
            }
            "session_is_closed" => {
                let closed = self.request_session(&request.params).await?.is_closed();
                Ok(json!({ "closed": closed }))
            }
            "session_send" => {
                let session = self.request_session(&request.params).await?;
                let prompt: String = required(&request.params, "prompt")?;
                let history = optional::<Vec<Message>>(&request.params, "history")?;
                let result = session.send(&prompt, history.as_deref()).await?;
                agent_result_value(result)
            }
            "session_cancel" => {
                let cancelled = self.request_session(&request.params).await?.cancel().await;
                Ok(json!({ "cancelled": cancelled }))
            }
            "session_cancel_and_settle" => {
                let grace_ms = optional::<u64>(&request.params, "grace_ms")?.unwrap_or(2_000);
                let abort_grace_ms =
                    optional::<u64>(&request.params, "abort_grace_ms")?.unwrap_or(2_000);
                let settled = self
                    .request_session(&request.params)
                    .await?
                    .cancel_and_settle(
                        std::time::Duration::from_millis(grace_ms),
                        std::time::Duration::from_millis(abort_grace_ms),
                    )
                    .await;
                Ok(json!({ "settled": settled }))
            }
            "session_history" => {
                let history = self.request_session(&request.params).await?.history();
                Ok(json!({ "messages": history }))
            }
            "session_close" => {
                self.request_session(&request.params).await?.close().await;
                Ok(json!({ "closed": true }))
            }
            "session_save" => {
                self.request_session(&request.params).await?.save().await?;
                Ok(json!({ "saved": true }))
            }
            "session_tool_names" => {
                let names = self.request_session(&request.params).await?.tool_names();
                Ok(json!({ "names": names }))
            }
            "session_tool_definitions" => {
                let definitions = self
                    .request_session(&request.params)
                    .await?
                    .tool_definitions();
                encode(definitions)
            }
            "session_trace_events" => {
                let events = self.request_session(&request.params).await?.trace_events();
                encode(events)
            }
            "session_get_artifact" => {
                let artifact_uri: String = required(&request.params, "artifact_uri")?;
                let artifact = self
                    .request_session(&request.params)
                    .await?
                    .get_artifact(&artifact_uri);
                encode(json!({ "artifact": artifact }))
            }
            "session_read_file" => {
                let path: String = required(&request.params, "path")?;
                let options = ReadFileOptions {
                    offset: optional(&request.params, "offset")?,
                    limit: optional(&request.params, "limit")?,
                };
                let content = self
                    .request_session(&request.params)
                    .await?
                    .read_file_with_options(&path, options)
                    .await?;
                Ok(json!({ "content": content }))
            }
            "session_write_file" => {
                let path: String = required(&request.params, "path")?;
                let content: String = required(&request.params, "content")?;
                let result = self
                    .request_session(&request.params)
                    .await?
                    .write_file(&path, &content)
                    .await?;
                tool_result_value(result)
            }
            "session_ls" => {
                let path = optional::<String>(&request.params, "path")?;
                let result = self
                    .request_session(&request.params)
                    .await?
                    .ls(path.as_deref())
                    .await?;
                tool_result_value(result)
            }
            "session_edit_file" => {
                let path: String = required(&request.params, "path")?;
                let old_string: String = required(&request.params, "old_string")?;
                let new_string: String = required(&request.params, "new_string")?;
                let replace_all =
                    optional::<bool>(&request.params, "replace_all")?.unwrap_or(false);
                let result = self
                    .request_session(&request.params)
                    .await?
                    .edit_file(&path, &old_string, &new_string, replace_all)
                    .await?;
                tool_result_value(result)
            }
            "session_patch_file" => {
                let path: String = required(&request.params, "path")?;
                let diff: String = required(&request.params, "diff")?;
                let result = self
                    .request_session(&request.params)
                    .await?
                    .patch_file(&path, &diff)
                    .await?;
                tool_result_value(result)
            }
            "session_bash" => {
                let command: String = required(&request.params, "command")?;
                let output = self
                    .request_session(&request.params)
                    .await?
                    .bash(&command)
                    .await?;
                Ok(json!({ "output": output }))
            }
            "session_glob" => {
                let pattern: String = required(&request.params, "pattern")?;
                let paths = self
                    .request_session(&request.params)
                    .await?
                    .glob(&pattern)
                    .await?;
                Ok(json!({ "paths": paths }))
            }
            "session_grep" => {
                let pattern: String = required(&request.params, "pattern")?;
                let output = self
                    .request_session(&request.params)
                    .await?
                    .grep(&pattern)
                    .await?;
                Ok(json!({ "output": output }))
            }
            "session_tool" => {
                let name: String = required(&request.params, "name")?;
                let args = request
                    .params
                    .get("args")
                    .cloned()
                    .unwrap_or_else(empty_object);
                let result = self
                    .request_session(&request.params)
                    .await?
                    .tool(&name, args)
                    .await?;
                tool_result_value(result)
            }
            "session_runs" => {
                let runs = self.request_session(&request.params).await?.runs().await;
                encode(runs)
            }
            "session_run_snapshot" => {
                let run_id: String = required(&request.params, "run_id")?;
                let snapshot = self
                    .request_session(&request.params)
                    .await?
                    .run_snapshot(&run_id)
                    .await;
                encode(json!({ "snapshot": snapshot }))
            }
            "session_run_events" => {
                let run_id: String = required(&request.params, "run_id")?;
                let session = self.request_session(&request.params).await?;
                let events = session.run_events(&run_id).await;
                let envelopes = events
                    .iter()
                    .map(|record| run_event_envelope_v1(record, &run_id, session.id()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        BridgeFailure::new("SERIALIZATION_ERROR", error.to_string())
                    })?;
                Ok(json!({ "events": envelopes }))
            }
            "session_run_event_page" => {
                let run_id: String = required(&request.params, "run_id")?;
                let after_sequence = optional::<usize>(&request.params, "after_sequence")?;
                let limit = optional::<usize>(&request.params, "limit")?.unwrap_or(100);
                let session = self.request_session(&request.params).await?;
                let page = session.run_event_page(&run_id, after_sequence, limit).await;
                match page {
                    None => Ok(json!({ "page": null })),
                    Some(page) => {
                        let events = page
                            .events
                            .iter()
                            .map(|record| run_event_envelope_v1(record, &run_id, session.id()))
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|error| {
                                BridgeFailure::new("SERIALIZATION_ERROR", error.to_string())
                            })?;
                        Ok(json!({
                            "page": {
                                "events": events,
                                "first_available_sequence": page.first_available_sequence,
                                "latest_sequence_exclusive": page.latest_sequence_exclusive,
                                "next_after_sequence": page.next_after_sequence,
                                "retention_gap": page.retention_gap,
                                "has_more": page.has_more,
                            }
                        }))
                    }
                }
            }
            "session_current_run" => {
                let run = self
                    .request_session(&request.params)
                    .await?
                    .current_run()
                    .await;
                match run {
                    None => Ok(json!({ "run": null })),
                    Some(run) => Ok(json!({
                        "run": {
                            "id": run.id(),
                            "session_id": run.session_id(),
                            "snapshot": run.snapshot().await,
                        }
                    })),
                }
            }
            "session_active_tools" => {
                let tools = self
                    .request_session(&request.params)
                    .await?
                    .active_tools()
                    .await;
                encode(tools)
            }
            "session_cancel_run" => {
                let run_id: String = required(&request.params, "run_id")?;
                let cancelled = self
                    .request_session(&request.params)
                    .await?
                    .cancel_run(&run_id)
                    .await;
                Ok(json!({ "cancelled": cancelled }))
            }
            "session_pending_confirmations" => {
                let confirmations = self
                    .request_session(&request.params)
                    .await?
                    .pending_confirmations()
                    .await;
                encode(confirmations)
            }
            "session_confirm_tool_use" => {
                let tool_id: String = required(&request.params, "tool_id")?;
                let approved: bool = required(&request.params, "approved")?;
                let reason = optional::<String>(&request.params, "reason")?;
                let confirmed = self
                    .request_session(&request.params)
                    .await?
                    .confirm_tool_use(&tool_id, approved, reason)
                    .await?;
                Ok(json!({ "confirmed": confirmed }))
            }
            "session_cancel_confirmations" => {
                let count = self
                    .request_session(&request.params)
                    .await?
                    .cancel_confirmations()
                    .await;
                Ok(json!({ "count": count }))
            }
            "session_verification_reports" => {
                let reports = self
                    .request_session(&request.params)
                    .await?
                    .verification_reports();
                encode(reports)
            }
            "session_verification_summary" => {
                let summary = self
                    .request_session(&request.params)
                    .await?
                    .verification_summary();
                encode(summary)
            }
            "session_verification_summary_text" => {
                let text = self
                    .request_session(&request.params)
                    .await?
                    .verification_summary_text();
                Ok(json!({ "text": text }))
            }
            "session_verification_presets" => {
                let presets = self
                    .request_session(&request.params)
                    .await?
                    .verification_presets();
                encode(presets)
            }
            "session_verify_commands" => {
                let subject: String = required(&request.params, "subject")?;
                let commands: Vec<a3s_code_core::verification::VerificationCommand> =
                    required(&request.params, "commands")?;
                let report = self
                    .request_session(&request.params)
                    .await?
                    .verify_commands(&subject, &commands)
                    .await?;
                encode(report)
            }
            "session_register_agent_dir" => {
                let path: String = required(&request.params, "path")?;
                let count = self
                    .request_session(&request.params)
                    .await?
                    .register_agent_dir(std::path::Path::new(&path))?;
                Ok(json!({ "count": count }))
            }
            "session_skill_names" => {
                let names = self.request_session(&request.params).await?.skill_names();
                Ok(json!({ "names": names }))
            }
            "session_add_mcp_server" => {
                let config: a3s_code_core::mcp::McpServerConfig =
                    required(&request.params, "config")?;
                let count = self
                    .request_session(&request.params)
                    .await?
                    .add_mcp_server(config)
                    .await?;
                Ok(json!({ "tool_count": count }))
            }
            "session_remove_mcp_server" => {
                let name: String = required(&request.params, "name")?;
                self.request_session(&request.params)
                    .await?
                    .remove_mcp_server(&name)
                    .await?;
                Ok(json!({ "removed": true }))
            }
            "session_mcp_status" => {
                let statuses = self
                    .request_session(&request.params)
                    .await?
                    .mcp_status()
                    .await;
                encode(statuses)
            }
            "session_stream" => Err(BridgeFailure::new(
                "INTERNAL_ERROR",
                "session_stream must be dispatched through the streaming path",
            )),
            operation => Err(BridgeFailure::new(
                "UNSUPPORTED_OPERATION",
                format!("unsupported bridge operation {operation:?}"),
            )),
        }
    }

    async fn insert_session(
        &self,
        agent_id: String,
        session: Arc<AgentSession>,
    ) -> Result<Value, BridgeFailure> {
        let handle = self.handle("session");
        let result = json!({
            "session_handle": handle,
            "session_id": session.id(),
            "workspace": session.workspace().display().to_string(),
            "init_warning": session.init_warning(),
        });
        self.sessions
            .write()
            .await
            .insert(handle, SessionEntry { agent_id, session });
        Ok(result)
    }

    async fn request_session(&self, params: &Value) -> Result<Arc<AgentSession>, BridgeFailure> {
        self.session(&required::<String>(params, "session_handle")?)
            .await
    }

    async fn stream(
        &self,
        request: &BridgeRequest,
        writer: &mpsc::UnboundedSender<BridgeEnvelope>,
    ) -> Result<Value, BridgeFailure> {
        let session = self.request_session(&request.params).await?;
        let prompt: String = required(&request.params, "prompt")?;
        let history = optional::<Vec<Message>>(&request.params, "history")?;
        let (mut events, handle) = session.stream(&prompt, history.as_deref()).await?;
        while let Some(event) = events.recv().await {
            let envelope = EventEnvelopeV1::try_from(event)
                .map_err(|error| BridgeFailure::new("SERIALIZATION_ERROR", error.to_string()))?;
            writer
                .send(BridgeEnvelope::event(request.id, envelope))
                .map_err(|_| BridgeFailure::new("BRIDGE_CLOSED", "bridge output is closed"))?;
        }
        handle
            .await
            .map_err(|error| BridgeFailure::new("RUNTIME_ERROR", error.to_string()))?;
        Ok(json!({ "completed": true }))
    }

    pub async fn close_all(&self) {
        let sessions = self
            .sessions
            .read()
            .await
            .values()
            .map(|entry| Arc::clone(&entry.session))
            .collect::<Vec<_>>();
        for session in sessions {
            session.close().await;
        }
        let agents = self
            .agents
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for agent in agents {
            agent.close().await;
        }
    }
}

/// Serve requests from stdin until EOF and write correlated JSONL envelopes to
/// stdout. Stdout is exclusively reserved for protocol data.
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(BridgeState::new());
    let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<BridgeEnvelope>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(envelope) = writer_rx.recv().await {
            let mut encoded = serde_json::to_vec(&envelope)?;
            encoded.push(b'\n');
            stdout.write_all(&encoded).await?;
            stdout.flush().await?;
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let writer_tx = writer_tx.clone();
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let request = match serde_json::from_str::<BridgeRequest>(&line) {
                Ok(request) => request,
                Err(error) => {
                    let id = serde_json::from_str::<Value>(&line)
                        .ok()
                        .and_then(|value| value.get("id").and_then(Value::as_u64))
                        .unwrap_or(0);
                    let _ = writer_tx.send(BridgeEnvelope::failure(
                        id,
                        BridgeFailure::new(
                            "INVALID_REQUEST",
                            format!("invalid bridge request: {error}"),
                        ),
                    ));
                    return;
                }
            };
            let result = if request.operation == "session_stream" {
                state.stream(&request, &writer_tx).await
            } else {
                state.dispatch(&request).await
            };
            let response = match result {
                Ok(value) => BridgeEnvelope::success(request.id, value),
                Err(error) => BridgeEnvelope::failure(request.id, error),
            };
            let _ = writer_tx.send(response);
        });
    }

    state.close_all().await;
    drop(writer_tx);
    writer
        .await
        .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })?
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BridgeSessionOptions {
    model: Option<String>,
    agent_dirs: Vec<String>,
    skill_dirs: Vec<String>,
    enforce_active_skill_tool_restrictions: Option<bool>,
    file_memory_dir: Option<String>,
    file_session_store_dir: Option<String>,
    session_id: Option<String>,
    tenant_id: Option<String>,
    principal: Option<String>,
    agent_template_id: Option<String>,
    correlation_id: Option<String>,
    planning_mode: Option<String>,
    goal_tracking: Option<bool>,
    auto_save: Option<bool>,
    max_parse_retries: Option<u32>,
    tool_timeout_ms: Option<u64>,
    llm_api_timeout_ms: Option<u64>,
    circuit_breaker_threshold: Option<u32>,
    duplicate_tool_call_threshold: Option<u32>,
    auto_compact: Option<bool>,
    auto_compact_threshold: Option<f32>,
    max_context_tokens: Option<usize>,
    continuation_enabled: Option<bool>,
    max_continuation_turns: Option<u32>,
    temperature: Option<f32>,
    thinking_budget: Option<usize>,
    max_tool_rounds: Option<usize>,
    max_parallel_tasks: Option<usize>,
    auto_delegation_enabled: Option<bool>,
    manual_delegation_enabled: Option<bool>,
    auto_parallel_delegation: Option<bool>,
    prompt_slots: Option<BridgePromptSlots>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BridgePromptSlots {
    role: Option<String>,
    guidelines: Option<String>,
    response_style: Option<String>,
    extra: Option<String>,
}

impl BridgeSessionOptions {
    fn into_core(self) -> Result<SessionOptions, BridgeFailure> {
        let mut options = SessionOptions::new();
        if let Some(value) = self.model {
            options = options.with_model(value);
        }
        for value in self.agent_dirs {
            options = options.with_agent_dir(PathBuf::from(value));
        }
        if !self.skill_dirs.is_empty() {
            options = options.with_skill_dirs(self.skill_dirs.into_iter().map(PathBuf::from));
        }
        if let Some(value) = self.enforce_active_skill_tool_restrictions {
            options = options.with_active_skill_tool_restrictions(value);
        }
        if let Some(value) = self.file_memory_dir {
            options = options.with_file_memory(value);
        }
        if let Some(value) = self.file_session_store_dir {
            options = options.with_file_session_store(value);
        }
        if let Some(value) = self.session_id {
            options = options.with_session_id(value);
        }
        if let Some(value) = self.tenant_id {
            options = options.with_tenant_id(value);
        }
        if let Some(value) = self.principal {
            options = options.with_principal(value);
        }
        if let Some(value) = self.agent_template_id {
            options = options.with_agent_template_id(value);
        }
        if let Some(value) = self.correlation_id {
            options = options.with_correlation_id(value);
        }
        if let Some(value) = self.planning_mode {
            options = options.with_planning_mode(match value.to_ascii_lowercase().as_str() {
                "auto" => PlanningMode::Auto,
                "enabled" => PlanningMode::Enabled,
                "disabled" => PlanningMode::Disabled,
                _ => {
                    return Err(BridgeFailure::new(
                        "INVALID_REQUEST",
                        "planning_mode must be auto, enabled, or disabled",
                    ))
                }
            });
        }
        if let Some(value) = self.goal_tracking {
            options = options.with_goal_tracking(value);
        }
        if let Some(value) = self.auto_save {
            options = options.with_auto_save(value);
        }
        if let Some(value) = self.max_parse_retries {
            options = options.with_parse_retries(value);
        }
        if let Some(value) = self.tool_timeout_ms {
            options = options.with_tool_timeout(value);
        }
        if let Some(value) = self.llm_api_timeout_ms {
            options = options.with_llm_api_timeout(value);
        }
        if let Some(value) = self.circuit_breaker_threshold {
            options = options.with_circuit_breaker(value);
        }
        if let Some(value) = self.duplicate_tool_call_threshold {
            options = options.with_duplicate_tool_call_threshold(value);
        }
        if let Some(value) = self.auto_compact {
            options = options.with_auto_compact(value);
        }
        if let Some(value) = self.auto_compact_threshold {
            options = options.with_auto_compact_threshold(value);
        }
        if let Some(value) = self.max_context_tokens {
            options = options.with_max_context_tokens(value);
        }
        if let Some(value) = self.continuation_enabled {
            options = options.with_continuation(value);
        }
        if let Some(value) = self.max_continuation_turns {
            options = options.with_max_continuation_turns(value);
        }
        if let Some(value) = self.temperature {
            options = options.with_temperature(value);
        }
        if let Some(value) = self.thinking_budget {
            options = options.with_thinking_budget(value);
        }
        if let Some(value) = self.max_tool_rounds {
            options = options.with_max_tool_rounds(value);
        }
        if let Some(value) = self.max_parallel_tasks {
            options = options.with_max_parallel_tasks(value);
        }
        if let Some(value) = self.auto_delegation_enabled {
            options = options.with_auto_delegation_enabled(value);
        }
        if let Some(value) = self.manual_delegation_enabled {
            options = options.with_manual_delegation_enabled(value);
        }
        if let Some(value) = self.auto_parallel_delegation {
            options = options.with_auto_parallel_delegation(value);
        }
        if let Some(value) = self.prompt_slots {
            options = options.with_prompt_slots(SystemPromptSlots {
                style: None,
                role: value.role,
                guidelines: value.guidelines,
                response_style: value.response_style,
                extra: value.extra,
            });
        }
        Ok(options)
    }
}

fn required<T: DeserializeOwned>(params: &Value, key: &str) -> Result<T, BridgeFailure> {
    let value = params.get(key).cloned().ok_or_else(|| {
        BridgeFailure::new("INVALID_REQUEST", format!("missing required field {key:?}"))
    })?;
    serde_json::from_value(value).map_err(|error| {
        BridgeFailure::new("INVALID_REQUEST", format!("invalid field {key:?}: {error}"))
    })
}

fn optional<T: DeserializeOwned>(params: &Value, key: &str) -> Result<Option<T>, BridgeFailure> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| {
                BridgeFailure::new("INVALID_REQUEST", format!("invalid field {key:?}: {error}"))
            }),
    }
}

fn encode(value: impl Serialize) -> Result<Value, BridgeFailure> {
    serde_json::to_value(value)
        .map_err(|error| BridgeFailure::new("SERIALIZATION_ERROR", error.to_string()))
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

fn session_info(session: &AgentSession) -> Value {
    json!({
        "session_id": session.id(),
        "workspace": session.workspace().display().to_string(),
        "init_warning": session.init_warning(),
        "tenant_id": session.tenant_id(),
        "principal": session.principal(),
        "agent_template_id": session.agent_template_id(),
        "correlation_id": session.correlation_id(),
    })
}

fn tool_result_value(result: ToolCallResult) -> Result<Value, BridgeFailure> {
    Ok(json!({
        "name": result.name,
        "output": result.output,
        "exit_code": result.exit_code,
        "metadata": result.metadata,
        "error_kind": result.error_kind,
    }))
}

fn agent_result_value(result: AgentResult) -> Result<Value, BridgeFailure> {
    Ok(json!({
        "text": result.text,
        "messages": result.messages,
        "usage": result.usage,
        "tool_calls_count": result.tool_calls_count,
        "verification_reports": result.verification_reports,
        "verification_summary": result.verification_summary(),
        "verification_summary_text": result.verification_summary_text(),
        "has_pending_verification": result.has_pending_verification(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(operation: &str, params: Value) -> BridgeRequest {
        BridgeRequest {
            protocol_version: BRIDGE_PROTOCOL_VERSION,
            id: 1,
            operation: operation.to_string(),
            params,
        }
    }

    fn test_acl() -> &'static str {
        r#"
            default_model = "anthropic/test-model"
            providers "anthropic" {
                apiKey = "test-key"
                models "test-model" {
                    name = "Test Model"
                }
            }
        "#
    }

    #[tokio::test]
    async fn capabilities_are_complete_and_unique() {
        let state = BridgeState::new();
        let result = state
            .dispatch(&request("sdk_capabilities", json!({})))
            .await
            .unwrap();
        assert_eq!(
            result["protocol_version"],
            Value::from(BRIDGE_PROTOCOL_VERSION)
        );
        let operations = result["operations"].as_array().unwrap();
        assert_eq!(operations.len(), BRIDGE_OPERATIONS.len());
        let mut sorted = BRIDGE_OPERATIONS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), BRIDGE_OPERATIONS.len());
    }

    #[tokio::test]
    async fn direct_file_lifecycle_uses_real_core_session() {
        let workspace = tempfile::tempdir().unwrap();
        let state = BridgeState::new();
        let created = state
            .dispatch(&request(
                "agent_create",
                json!({ "config_source": test_acl() }),
            ))
            .await
            .unwrap();
        let agent_id = created["agent_id"].as_str().unwrap();
        let created = state
            .dispatch(&request(
                "session_create",
                json!({
                    "agent_id": agent_id,
                    "workspace": workspace.path(),
                    "options": { "session_id": "go-bridge-test" }
                }),
            ))
            .await
            .unwrap();
        let session_handle = created["session_handle"].as_str().unwrap();

        state
            .dispatch(&request(
                "session_write_file",
                json!({
                    "session_handle": session_handle,
                    "path": "hello.txt",
                    "content": "hello from go bridge"
                }),
            ))
            .await
            .unwrap();
        let read = state
            .dispatch(&request(
                "session_read_file",
                json!({ "session_handle": session_handle, "path": "hello.txt" }),
            ))
            .await
            .unwrap();
        assert!(read["content"]
            .as_str()
            .unwrap()
            .contains("hello from go bridge"));

        state
            .dispatch(&request(
                "session_close",
                json!({ "session_handle": session_handle }),
            ))
            .await
            .unwrap();
        let closed = state
            .dispatch(&request(
                "session_is_closed",
                json!({ "session_handle": session_handle }),
            ))
            .await
            .unwrap();
        assert_eq!(closed["closed"], Value::Bool(true));
    }

    #[tokio::test]
    async fn rejects_protocol_and_parameter_shape_errors() {
        let state = BridgeState::new();
        let mut bad_version = request("sdk_capabilities", json!({}));
        bad_version.protocol_version += 1;
        let error = state.dispatch(&bad_version).await.unwrap_err();
        assert_eq!(error.code, "PROTOCOL_ERROR");

        let error = state
            .dispatch(&request("sdk_capabilities", json!([])))
            .await
            .unwrap_err();
        assert_eq!(error.code, "INVALID_REQUEST");
    }
}
