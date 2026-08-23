//! Versioned, machine-only JSONL bridge for the A3S Code Go SDK.
//!
//! A bridge process owns Rust `Agent` and `AgentSession` values for its entire
//! lifetime. Requests may complete out of order and are correlated by `id`.
//! Streaming requests emit zero or more `event` envelopes followed by exactly
//! one `response` envelope.

use a3s_code_core::serve::{spawn_agent_dir_daemon, ServeDaemonHandle};
use a3s_code_core::{
    execute_steps_parallel_resumable, run_event_envelope_v1, Agent, AgentResult, AgentSession,
    AgentStepSpec, CodeError, EventEnvelopeV1, Message, PlanningMode, ReadFileOptions,
    SessionOptions, SystemPromptSlots, TaskSchedulerError, ToolCallResult,
};
use base64::Engine as _;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, RwLock};

mod serve;
mod workspace_retrieval;
use workspace_retrieval::*;

pub const BRIDGE_PROTOCOL_VERSION: u16 = 2;

pub const BRIDGE_OPERATIONS: &[&str] = &[
    "sdk_capabilities",
    "agent_create",
    "agent_refresh_mcp_tools",
    "agent_task_scheduler_stats",
    "agent_replace_session",
    "agent_session_for_agent",
    "agent_session_for_worker",
    "agent_list_sessions",
    "agent_close_session",
    "agent_disconnect_idle_mcp",
    "agent_serve_agent_dir",
    "agent_serve_status",
    "agent_stop_serve",
    "agent_is_closed",
    "agent_close",
    "session_create",
    "session_resume",
    "session_info",
    "session_task_scheduler_stats",
    "session_workspace_retrieval_status",
    "session_semantic_search",
    "session_hybrid_search",
    "session_is_closed",
    "session_send",
    "session_resume_run",
    "session_spawn_run_with_id",
    "session_spawn_recovery_with_run_id",
    "session_send_with_attachments",
    "session_stream",
    "session_stream_with_attachments",
    "session_parallel",
    "session_parallel_resumable",
    "session_workflow_step",
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
    "session_governed_tool",
    "session_runs",
    "session_run_snapshot",
    "session_run_events",
    "session_run_event_page",
    "session_current_run",
    "session_active_tools",
    "session_subagent_task",
    "session_subagent_tasks",
    "session_pending_subagent_tasks",
    "session_cancel_subagent_task",
    "session_cancel_run",
    "session_pending_confirmations",
    "session_confirm_tool_use",
    "session_cancel_confirmations",
    "session_verification_reports",
    "session_record_verification_reports",
    "session_verification_summary",
    "session_verification_summary_text",
    "session_verification_presets",
    "session_verify_commands",
    "session_register_agent_dir",
    "session_register_worker_agent",
    "session_register_worker_agents",
    "session_add_skill",
    "session_remove_skill",
    "session_skill_names",
    "session_register_dynamic_workflow",
    "session_unregister_dynamic_tool",
    "session_add_mcp_server",
    "session_remove_mcp_server",
    "session_mcp_status",
    "session_has_memory",
    "session_remember_success",
    "session_remember_failure",
    "session_recall_similar",
    "session_recall_by_tags",
    "session_memory_recent",
    "session_memory_stats",
    "session_get_working_memory",
    "session_clear_working_memory",
    "session_get_short_term_memory",
    "session_clear_short_term_memory",
    "session_has_queue",
    "session_set_lane_handler",
    "session_complete_external_task",
    "session_pending_external_tasks",
    "session_queue_stats",
    "session_dead_letters",
    "session_queue_metrics",
    "session_register_hook",
    "session_unregister_hook",
    "session_hook_count",
    "session_set_budget_guard",
    "session_register_command",
    "session_list_commands",
    "callback_response",
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
    pub callback: Option<BridgeCallbackInvocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_cancel: Option<BridgeCallbackCancellation>,
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
            callback: None,
            callback_cancel: None,
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
            callback: None,
            callback_cancel: None,
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
            callback: None,
            callback_cancel: None,
            error: Some(BridgeError {
                code: error.code,
                message: error.message,
            }),
        }
    }

    fn callback(id: u64, callback: BridgeCallbackInvocation) -> Self {
        Self {
            protocol_version: BRIDGE_PROTOCOL_VERSION,
            id,
            kind: "callback",
            ok: true,
            result: None,
            event: None,
            callback: Some(callback),
            callback_cancel: None,
            error: None,
        }
    }

    fn callback_cancel(callback_id: u64) -> Self {
        Self {
            protocol_version: BRIDGE_PROTOCOL_VERSION,
            id: callback_id,
            kind: "callback_cancel",
            ok: true,
            result: None,
            event: None,
            callback: None,
            callback_cancel: Some(BridgeCallbackCancellation { callback_id }),
            error: None,
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

impl From<TaskSchedulerError> for BridgeFailure {
    fn from(error: TaskSchedulerError) -> Self {
        let code = match error {
            TaskSchedulerError::InvalidConfig(_) => "INVALID_CONFIG",
            TaskSchedulerError::Cancelled => "TASK_ADMISSION_CANCELLED",
            TaskSchedulerError::Closed => "TASK_SCHEDULER_CLOSED",
        };
        Self::new(code, error.to_string())
    }
}

fn serve_failure(handle: &ServeDaemonHandle, error: CodeError) -> BridgeFailure {
    BridgeFailure::new(
        handle.failure_code().unwrap_or(error.code()),
        error.to_string(),
    )
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
    serve_handles: RwLock<HashMap<String, ServeDaemonHandle>>,
    callbacks: RwLock<Option<Arc<CallbackClient>>>,
}

impl Default for BridgeState {
    fn default() -> Self {
        Self {
            next_handle: AtomicU64::new(1),
            agents: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            serve_handles: RwLock::new(HashMap::new()),
            callbacks: RwLock::new(None),
        }
    }
}

impl BridgeState {
    pub fn new() -> Self {
        Self::default()
    }

    async fn install_callback_writer(&self, writer: mpsc::UnboundedSender<BridgeEnvelope>) {
        *self.callbacks.write().await = Some(Arc::new(CallbackClient::new(writer)));
    }

    async fn callback_client(&self) -> Result<Arc<CallbackClient>, BridgeFailure> {
        self.callbacks.read().await.clone().ok_or_else(|| {
            BridgeFailure::new("CALLBACK_UNAVAILABLE", "callback transport is unavailable")
        })
    }

    async fn session_options(
        &self,
        options: BridgeSessionOptions,
    ) -> Result<SessionOptions, BridgeFailure> {
        let callbacks = if options.workspace_retrieval.is_some() {
            Some(self.callback_client().await?)
        } else {
            None
        };
        options.into_core(callbacks)
    }

    async fn optional_session_options(
        &self,
        options: Option<BridgeSessionOptions>,
    ) -> Result<Option<SessionOptions>, BridgeFailure> {
        match options {
            Some(options) => self.session_options(options).await.map(Some),
            None => Ok(None),
        }
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
            "callback_response" => {
                let callback_id: u64 = required(&request.params, "callback_id")?;
                let result = request.params.get("result").cloned();
                let error = optional::<String>(&request.params, "error")?;
                let accepted = self
                    .callback_client()
                    .await?
                    .respond(callback_id, CallbackReply { result, error });
                Ok(json!({ "accepted": accepted }))
            }
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
            "agent_task_scheduler_stats" => {
                let stats = self
                    .agent(&required::<String>(&request.params, "agent_id")?)
                    .await?
                    .task_scheduler_stats()
                    .await?;
                encode(stats)
            }
            "agent_replace_session" => {
                let agent_id: String = required(&request.params, "agent_id")?;
                let current = self.request_session(&request.params).await?;
                let options = optional::<BridgeSessionOptions>(&request.params, "options")?
                    .unwrap_or_default();
                let options = self.session_options(options).await?;
                let replacement = Arc::new(
                    self.agent(&agent_id)
                        .await?
                        .replace_session_async(&current, options)
                        .await?,
                );
                self.insert_session(agent_id, replacement).await
            }
            "agent_session_for_agent" => {
                let agent_id: String = required(&request.params, "agent_id")?;
                let workspace: String = required(&request.params, "workspace")?;
                let agent_name: String = required(&request.params, "agent_name")?;
                let agent_dirs =
                    optional::<Vec<String>>(&request.params, "agent_dirs")?.unwrap_or_default();
                let registry = a3s_code_core::AgentRegistry::new();
                for dir in agent_dirs {
                    for definition in
                        a3s_code_core::subagent::load_agents_from_dir(std::path::Path::new(&dir))
                    {
                        registry.register(definition);
                    }
                }
                let definition = registry.get(&agent_name).ok_or_else(|| {
                    BridgeFailure::new(
                        "NOT_FOUND",
                        format!("agent definition {agent_name:?} was not found"),
                    )
                })?;
                let options = self
                    .optional_session_options(optional::<BridgeSessionOptions>(
                        &request.params,
                        "options",
                    )?)
                    .await?;
                let session = Arc::new(
                    self.agent(&agent_id)
                        .await?
                        .session_for_agent_async(workspace, &definition, options)
                        .await?,
                );
                self.insert_session(agent_id, session).await
            }
            "agent_session_for_worker" => {
                let agent_id: String = required(&request.params, "agent_id")?;
                let workspace: String = required(&request.params, "workspace")?;
                let worker =
                    required::<BridgeWorkerAgentSpec>(&request.params, "worker")?.into_core()?;
                let options = self
                    .optional_session_options(optional::<BridgeSessionOptions>(
                        &request.params,
                        "options",
                    )?)
                    .await?;
                let session = Arc::new(
                    self.agent(&agent_id)
                        .await?
                        .session_for_worker_async(workspace, worker, options)
                        .await?,
                );
                self.insert_session(agent_id, session).await
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
            "agent_disconnect_idle_mcp" => {
                let idle_threshold_ms: u64 = required(&request.params, "idle_threshold_ms")?;
                let disconnected = self
                    .agent(&required::<String>(&request.params, "agent_id")?)
                    .await?
                    .disconnect_idle_mcp(idle_threshold_ms)
                    .await;
                Ok(json!({ "names": disconnected }))
            }
            "agent_serve_agent_dir" => serve::start(self, &request.params).await,
            "agent_serve_status" => serve::status(self, &request.params).await,
            "agent_stop_serve" => serve::stop(self, &request.params).await,
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
                    .unwrap_or_default();
                let options = self.session_options(options).await?;
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
                    .unwrap_or_default();
                let options = self.session_options(options).await?;
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
            "session_task_scheduler_stats" => {
                let stats = self
                    .request_session(&request.params)
                    .await?
                    .task_scheduler_stats()
                    .await?;
                encode(stats)
            }
            "session_workspace_retrieval_status" => {
                let status = self
                    .request_session(&request.params)
                    .await?
                    .workspace_retrieval_status();
                Ok(status_value(&status))
            }
            "session_semantic_search" => {
                let search = required::<BridgeWorkspaceSearchRequest>(&request.params, "request")?
                    .semantic()?;
                let result = self
                    .request_session(&request.params)
                    .await?
                    .semantic_search(search)
                    .await
                    .map_err(retrieval_failure)?;
                Ok(semantic_result_value(result))
            }
            "session_hybrid_search" => {
                let search = required::<BridgeWorkspaceSearchRequest>(&request.params, "request")?
                    .hybrid()?;
                let result = self
                    .request_session(&request.params)
                    .await?
                    .hybrid_search(search)
                    .await
                    .map_err(retrieval_failure)?;
                Ok(hybrid_result_value(result))
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
            "session_resume_run" => {
                let checkpoint_run_id: String = required(&request.params, "checkpoint_run_id")?;
                let result = self
                    .request_session(&request.params)
                    .await?
                    .resume_run(&checkpoint_run_id)
                    .await?;
                agent_result_value(result)
            }
            "session_spawn_run_with_id" => {
                let run_id: String = required(&request.params, "run_id")?;
                let prompt: String = required(&request.params, "prompt")?;
                let spawn = self
                    .request_session(&request.params)
                    .await?
                    .spawn_run_with_id(&run_id, &prompt)
                    .await?;
                run_spawn_value(&spawn)
            }
            "session_spawn_recovery_with_run_id" => {
                let checkpoint_run_id: String = required(&request.params, "checkpoint_run_id")?;
                let run_id: String = required(&request.params, "run_id")?;
                let spawn = self
                    .request_session(&request.params)
                    .await?
                    .spawn_recovery_with_run_id(&checkpoint_run_id, &run_id)
                    .await?;
                run_spawn_value(&spawn)
            }
            "session_send_with_attachments" => {
                let session = self.request_session(&request.params).await?;
                let prompt: String = required(&request.params, "prompt")?;
                let attachments =
                    required::<Vec<BridgeAttachment>>(&request.params, "attachments")?
                        .into_iter()
                        .map(BridgeAttachment::into_core)
                        .collect::<Result<Vec<_>, _>>()?;
                let history = optional::<Vec<Message>>(&request.params, "history")?;
                let result = session
                    .send_with_attachments(&prompt, &attachments, history.as_deref())
                    .await?;
                agent_result_value(result)
            }
            "session_parallel" => {
                let specs: Vec<AgentStepSpec> = required(&request.params, "specs")?;
                let budget_tokens = optional::<u64>(&request.params, "budget_tokens")?;
                let workflow = self
                    .request_session(&request.params)
                    .await?
                    .workflow_with_token_budget(budget_tokens);
                let outcomes = workflow.parallel(specs).await;
                let budget = workflow.budget_snapshot().map(|snapshot| {
                    json!({
                        "consumed_tokens": snapshot.consumed_tokens,
                        "limit_tokens": snapshot.limit_tokens,
                    })
                });
                Ok(json!({
                    "outcomes": outcomes,
                    "budget": budget,
                }))
            }
            "session_parallel_resumable" => {
                let session = self.request_session(&request.params).await?;
                let specs: Vec<AgentStepSpec> = required(&request.params, "specs")?;
                let workflow_id: String = required(&request.params, "workflow_id")?;
                let store = session.session_store().ok_or_else(|| {
                    BridgeFailure::new(
                        "SESSION_ERROR",
                        "parallel resumable requires a session store",
                    )
                })?;
                let outcomes = execute_steps_parallel_resumable(
                    session.agent_executor(),
                    specs,
                    &workflow_id,
                    store,
                    None,
                )
                .await;
                encode(outcomes)
            }
            "session_workflow_step" => {
                let spec: AgentStepSpec = required(&request.params, "spec")?;
                let outcome = self
                    .request_session(&request.params)
                    .await?
                    .workflow()
                    .agent(spec)
                    .await;
                encode(outcome)
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
            "session_governed_tool" => {
                let name: String = required(&request.params, "name")?;
                let args = request
                    .params
                    .get("args")
                    .cloned()
                    .unwrap_or_else(empty_object);
                let result = self
                    .request_session(&request.params)
                    .await?
                    .governed_tool(&name, args)
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
            "session_subagent_task" => {
                let task_id: String = required(&request.params, "task_id")?;
                let task = self
                    .request_session(&request.params)
                    .await?
                    .subagent_task(&task_id)
                    .await;
                Ok(json!({ "task": task }))
            }
            "session_subagent_tasks" => {
                let tasks = self
                    .request_session(&request.params)
                    .await?
                    .subagent_tasks()
                    .await;
                encode(tasks)
            }
            "session_pending_subagent_tasks" => {
                let tasks = self
                    .request_session(&request.params)
                    .await?
                    .pending_subagent_tasks()
                    .await;
                encode(tasks)
            }
            "session_cancel_subagent_task" => {
                let task_id: String = required(&request.params, "task_id")?;
                let cancelled = self
                    .request_session(&request.params)
                    .await?
                    .cancel_subagent_task(&task_id)
                    .await;
                Ok(json!({ "cancelled": cancelled }))
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
            "session_record_verification_reports" => {
                let reports: Vec<a3s_code_core::verification::VerificationReport> =
                    required(&request.params, "reports")?;
                self.request_session(&request.params)
                    .await?
                    .record_verification_reports(reports);
                Ok(json!({ "recorded": true }))
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
            "session_register_worker_agent" => {
                let worker =
                    required::<BridgeWorkerAgentSpec>(&request.params, "worker")?.into_core()?;
                let definition = self
                    .request_session(&request.params)
                    .await?
                    .register_worker_agent(worker)?;
                Ok(agent_definition_value(definition))
            }
            "session_register_worker_agents" => {
                let workers: Vec<BridgeWorkerAgentSpec> = required(&request.params, "workers")?;
                let workers = workers
                    .into_iter()
                    .map(BridgeWorkerAgentSpec::into_core)
                    .collect::<Result<Vec<_>, _>>()?;
                let definitions = self
                    .request_session(&request.params)
                    .await?
                    .register_worker_agents(workers)?;
                Ok(Value::Array(
                    definitions
                        .into_iter()
                        .map(agent_definition_value)
                        .collect(),
                ))
            }
            "session_add_skill" => {
                let skill: BridgeInlineSkill = required(&request.params, "skill")?;
                self.request_session(&request.params)
                    .await?
                    .add_skill(Arc::new(skill.into_core()?))?;
                Ok(json!({ "added": true }))
            }
            "session_remove_skill" => {
                let name: String = required(&request.params, "name")?;
                self.request_session(&request.params)
                    .await?
                    .remove_skill(&name)?;
                Ok(json!({ "removed": true }))
            }
            "session_skill_names" => {
                let names = self.request_session(&request.params).await?.skill_names();
                Ok(json!({ "names": names }))
            }
            "session_register_dynamic_workflow" => {
                self.request_session(&request.params)
                    .await?
                    .register_dynamic_workflow_runtime()?;
                Ok(json!({ "registered": true }))
            }
            "session_unregister_dynamic_tool" => {
                let name: String = required(&request.params, "name")?;
                self.request_session(&request.params)
                    .await?
                    .unregister_dynamic_tool(&name)?;
                Ok(json!({ "unregistered": true }))
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
            "session_has_memory" => {
                let available = self
                    .request_session(&request.params)
                    .await?
                    .memory()
                    .is_some();
                Ok(json!({ "available": available }))
            }
            "session_remember_success" => {
                let task: String = required(&request.params, "task")?;
                let tools: Vec<String> = required(&request.params, "tools")?;
                let result: String = required(&request.params, "result")?;
                self.request_memory(&request.params)
                    .await?
                    .remember_success(&task, &tools, &result)
                    .await
                    .map_err(memory_failure)?;
                Ok(json!({ "remembered": true }))
            }
            "session_remember_failure" => {
                let task: String = required(&request.params, "task")?;
                let error: String = required(&request.params, "error")?;
                let tools: Vec<String> = required(&request.params, "tools")?;
                self.request_memory(&request.params)
                    .await?
                    .remember_failure(&task, &error, &tools)
                    .await
                    .map_err(memory_failure)?;
                Ok(json!({ "remembered": true }))
            }
            "session_recall_similar" => {
                let query: String = required(&request.params, "query")?;
                let limit = optional::<usize>(&request.params, "limit")?.unwrap_or(5);
                let items = self
                    .request_memory(&request.params)
                    .await?
                    .recall_similar(&query, limit)
                    .await
                    .map_err(memory_failure)?;
                encode(items)
            }
            "session_recall_by_tags" => {
                let tags: Vec<String> = required(&request.params, "tags")?;
                let limit = optional::<usize>(&request.params, "limit")?.unwrap_or(10);
                let items = self
                    .request_memory(&request.params)
                    .await?
                    .recall_by_tags(&tags, limit)
                    .await
                    .map_err(memory_failure)?;
                encode(items)
            }
            "session_memory_recent" => {
                let limit = optional::<usize>(&request.params, "limit")?.unwrap_or(10);
                let items = self
                    .request_memory(&request.params)
                    .await?
                    .get_recent(limit)
                    .await
                    .map_err(memory_failure)?;
                encode(items)
            }
            "session_memory_stats" => {
                let stats = self
                    .request_memory(&request.params)
                    .await?
                    .stats()
                    .await
                    .map_err(memory_failure)?;
                encode(stats)
            }
            "session_get_working_memory" => {
                let items = self
                    .request_memory(&request.params)
                    .await?
                    .get_working()
                    .await;
                encode(items)
            }
            "session_clear_working_memory" => {
                self.request_memory(&request.params)
                    .await?
                    .clear_working()
                    .await;
                Ok(json!({ "cleared": true }))
            }
            "session_get_short_term_memory" => {
                let items = self
                    .request_memory(&request.params)
                    .await?
                    .get_short_term()
                    .await;
                encode(items)
            }
            "session_clear_short_term_memory" => {
                self.request_memory(&request.params)
                    .await?
                    .clear_short_term()
                    .await;
                Ok(json!({ "cleared": true }))
            }
            "session_has_queue" => {
                let available = self.request_session(&request.params).await?.has_queue();
                Ok(json!({ "available": available }))
            }
            "session_set_lane_handler" => {
                let lane = parse_lane(&required::<String>(&request.params, "lane")?)?;
                let config =
                    required::<BridgeLaneHandlerConfig>(&request.params, "config")?.into_core()?;
                self.request_session(&request.params)
                    .await?
                    .set_lane_handler(lane, config)
                    .await?;
                Ok(json!({ "configured": true }))
            }
            "session_complete_external_task" => {
                let task_id: String = required(&request.params, "task_id")?;
                let result: a3s_code_core::queue::ExternalTaskResult =
                    required(&request.params, "result")?;
                let completed = self
                    .request_session(&request.params)
                    .await?
                    .complete_external_task(&task_id, result)
                    .await;
                Ok(json!({ "completed": completed }))
            }
            "session_pending_external_tasks" => {
                let tasks = self
                    .request_session(&request.params)
                    .await?
                    .pending_external_tasks()
                    .await;
                encode(tasks)
            }
            "session_queue_stats" => {
                let stats = self
                    .request_session(&request.params)
                    .await?
                    .queue_stats()
                    .await;
                encode(stats)
            }
            "session_dead_letters" => {
                let letters = self
                    .request_session(&request.params)
                    .await?
                    .dead_letters()
                    .await;
                encode(letters)
            }
            "session_queue_metrics" => {
                let metrics = self
                    .request_session(&request.params)
                    .await?
                    .queue_metrics()
                    .await;
                Ok(queue_metrics_value(metrics))
            }
            "session_register_hook" => {
                let hook: a3s_code_core::hooks::Hook = required(&request.params, "hook")?;
                let hook_id = hook.id.clone();
                let handler_id = optional::<String>(&request.params, "handler_id")?;
                let timeout_ms = hook.config.timeout_ms;
                let session = self.request_session(&request.params).await?;
                session.register_hook(hook)?;
                if let Some(handler_id) = handler_id {
                    session.register_hook_handler(
                        &hook_id,
                        Arc::new(BridgeHookHandler {
                            client: self.callback_client().await?,
                            handler_id,
                            timeout_ms,
                            runtime: tokio::runtime::Handle::current(),
                        }),
                    )?;
                } else {
                    session.unregister_hook_handler(&hook_id)?;
                }
                Ok(json!({ "registered": true }))
            }
            "session_unregister_hook" => {
                let hook_id: String = required(&request.params, "hook_id")?;
                let session = self.request_session(&request.params).await?;
                session.unregister_hook_handler(&hook_id)?;
                let removed = session.unregister_hook(&hook_id)?.is_some();
                Ok(json!({ "removed": removed }))
            }
            "session_hook_count" => {
                let count = self.request_session(&request.params).await?.hook_count();
                Ok(json!({ "count": count }))
            }
            "session_set_budget_guard" => {
                let handler_id = optional::<String>(&request.params, "handler_id")?;
                let session = self.request_session(&request.params).await?;
                match handler_id {
                    Some(handler_id) => {
                        let timeout_ms =
                            optional::<u64>(&request.params, "timeout_ms")?.unwrap_or(5_000);
                        session.set_budget_guard(Some(Arc::new(BridgeBudgetGuard {
                            client: self.callback_client().await?,
                            handler_id,
                            timeout_ms,
                        })))?;
                    }
                    None => session.set_budget_guard(None)?,
                }
                Ok(json!({ "configured": true }))
            }
            "session_register_command" => {
                let name: String = required(&request.params, "name")?;
                let description: String = required(&request.params, "description")?;
                let usage = optional::<String>(&request.params, "usage")?;
                let handler_id: String = required(&request.params, "handler_id")?;
                let timeout_ms = optional::<u64>(&request.params, "timeout_ms")?.unwrap_or(5_000);
                self.request_session(&request.params)
                    .await?
                    .register_command(Arc::new(BridgeSlashCommand {
                        name,
                        description,
                        usage,
                        client: self.callback_client().await?,
                        handler_id,
                        timeout_ms,
                        runtime: tokio::runtime::Handle::current(),
                    }))?;
                Ok(json!({ "registered": true }))
            }
            "session_list_commands" => {
                let commands = self
                    .request_session(&request.params)
                    .await?
                    .command_registry()
                    .list_full();
                Ok(json!({
                    "commands": commands.into_iter().map(|(name, description, usage)| {
                        json!({ "name": name, "description": description, "usage": usage })
                    }).collect::<Vec<_>>()
                }))
            }
            "session_stream" => Err(BridgeFailure::new(
                "INTERNAL_ERROR",
                "session_stream must be dispatched through the streaming path",
            )),
            "session_stream_with_attachments" => Err(BridgeFailure::new(
                "INTERNAL_ERROR",
                "session_stream_with_attachments must be dispatched through the streaming path",
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
            "tenant_id": session.tenant_id(),
            "principal": session.principal(),
            "agent_template_id": session.agent_template_id(),
            "correlation_id": session.correlation_id(),
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

    async fn request_memory(
        &self,
        params: &Value,
    ) -> Result<Arc<a3s_code_core::memory::AgentMemory>, BridgeFailure> {
        self.request_session(params)
            .await?
            .memory()
            .cloned()
            .ok_or_else(|| {
                BridgeFailure::new(
                    "MEMORY_UNAVAILABLE",
                    "memory is unavailable for this session; inspect init_warning",
                )
            })
    }

    async fn stream(
        &self,
        request: &BridgeRequest,
        writer: &mpsc::UnboundedSender<BridgeEnvelope>,
    ) -> Result<Value, BridgeFailure> {
        let session = self.request_session(&request.params).await?;
        let prompt: String = required(&request.params, "prompt")?;
        let history = optional::<Vec<Message>>(&request.params, "history")?;
        let (mut events, handle) = if request.operation == "session_stream_with_attachments" {
            let attachments = required::<Vec<BridgeAttachment>>(&request.params, "attachments")?
                .into_iter()
                .map(BridgeAttachment::into_core)
                .collect::<Result<Vec<_>, _>>()?;
            session
                .stream_with_attachments(&prompt, &attachments, history.as_deref())
                .await?
        } else {
            session.stream(&prompt, history.as_deref()).await?
        };
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
        let handles = self
            .serve_handles
            .write()
            .await
            .drain()
            .map(|(_, handle)| handle)
            .collect::<Vec<_>>();
        let mut stops = tokio::task::JoinSet::new();
        for handle in handles {
            stops.spawn(async move {
                let _ = handle.stop().await;
            });
        }
        while stops.join_next().await.is_some() {}
        let sessions = self
            .sessions
            .write()
            .await
            .drain()
            .map(|(_, entry)| entry.session)
            .collect::<Vec<_>>();
        for session in sessions {
            session.close().await;
        }
        let agents = self
            .agents
            .write()
            .await
            .drain()
            .map(|(_, agent)| agent)
            .collect::<Vec<_>>();
        for agent in agents {
            agent.close().await;
        }
        *self.callbacks.write().await = None;
    }
}

/// Serve requests from stdin until EOF and write correlated JSONL envelopes to
/// stdout. Stdout is exclusively reserved for protocol data.
pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(BridgeState::new());
    let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<BridgeEnvelope>();
    let (writer_stop_tx, mut writer_stop_rx) = oneshot::channel::<()>();
    state.install_callback_writer(writer_tx.clone()).await;
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        loop {
            tokio::select! {
                biased;
                _ = &mut writer_stop_rx => {
                    while let Ok(envelope) = writer_rx.try_recv() {
                        write_bridge_envelope(&mut stdout, &envelope).await?;
                    }
                    break;
                }
                envelope = writer_rx.recv() => {
                    let Some(envelope) = envelope else {
                        break;
                    };
                    write_bridge_envelope(&mut stdout, &envelope).await?;
                }
            }
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
            let result = if matches!(
                request.operation.as_str(),
                "session_stream" | "session_stream_with_attachments"
            ) {
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
    drop(state);
    drop(writer_tx);
    let _ = writer_stop_tx.send(());
    writer
        .await
        .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })?
        .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    Ok(())
}

async fn write_bridge_envelope(
    stdout: &mut tokio::io::Stdout,
    envelope: &BridgeEnvelope,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut encoded = serde_json::to_vec(envelope)?;
    encoded.push(b'\n');
    stdout.write_all(&encoded).await?;
    stdout.flush().await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct BridgeAttachment {
    data: String,
    media_type: String,
}

impl BridgeAttachment {
    fn into_core(self) -> Result<a3s_code_core::Attachment, BridgeFailure> {
        let data = base64::engine::general_purpose::STANDARD
            .decode(self.data)
            .map_err(|error| {
                BridgeFailure::new(
                    "INVALID_REQUEST",
                    format!("invalid attachment data: {error}"),
                )
            })?;
        Ok(a3s_code_core::Attachment::new(data, self.media_type))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeCallbackInvocation {
    pub callback_id: u64,
    pub handler_id: String,
    pub method: String,
    pub payload: Value,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeCallbackCancellation {
    pub callback_id: u64,
}

struct CallbackReply {
    result: Option<Value>,
    error: Option<String>,
}

struct CallbackClient {
    next_id: AtomicU64,
    writer: mpsc::UnboundedSender<BridgeEnvelope>,
    pending: StdMutex<HashMap<u64, oneshot::Sender<CallbackReply>>>,
}

struct CallbackInvocationGuard<'a> {
    client: &'a CallbackClient,
    callback_id: u64,
    armed: bool,
}

impl CallbackInvocationGuard<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CallbackInvocationGuard<'_> {
    fn drop(&mut self) {
        if self.armed && self.client.remove_pending(self.callback_id).is_some() {
            let _ = self
                .client
                .writer
                .send(BridgeEnvelope::callback_cancel(self.callback_id));
        }
    }
}

impl CallbackClient {
    fn new(writer: mpsc::UnboundedSender<BridgeEnvelope>) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            writer,
            pending: StdMutex::new(HashMap::new()),
        }
    }

    fn pending(&self) -> StdMutexGuard<'_, HashMap<u64, oneshot::Sender<CallbackReply>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn remove_pending(&self, callback_id: u64) -> Option<oneshot::Sender<CallbackReply>> {
        self.pending().remove(&callback_id)
    }

    #[cfg(test)]
    fn pending_len(&self) -> usize {
        self.pending().len()
    }

    async fn invoke(
        &self,
        handler_id: &str,
        method: &str,
        payload: Value,
        timeout_ms: u64,
    ) -> Result<Value, BridgeFailure> {
        let callback_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending().insert(callback_id, tx);
        if self
            .writer
            .send(BridgeEnvelope::callback(
                callback_id,
                BridgeCallbackInvocation {
                    callback_id,
                    handler_id: handler_id.to_string(),
                    method: method.to_string(),
                    payload,
                    timeout_ms,
                },
            ))
            .is_err()
        {
            self.remove_pending(callback_id);
            return Err(BridgeFailure::new(
                "BRIDGE_CLOSED",
                "bridge output is closed",
            ));
        }
        let mut guard = CallbackInvocationGuard {
            client: self,
            callback_id,
            armed: true,
        };
        let reply =
            match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), rx).await {
                Ok(Ok(reply)) => reply,
                Ok(Err(_)) => {
                    return Err(BridgeFailure::new(
                        "CALLBACK_ERROR",
                        "callback response channel closed",
                    ));
                }
                Err(_) => {
                    return Err(BridgeFailure::new(
                        "CALLBACK_TIMEOUT",
                        format!("callback {handler_id:?} timed out after {timeout_ms}ms"),
                    ));
                }
            };
        guard.disarm();
        if let Some(error) = reply.error {
            return Err(BridgeFailure::new("CALLBACK_ERROR", error));
        }
        Ok(reply.result.unwrap_or(Value::Null))
    }

    fn respond(&self, callback_id: u64, reply: CallbackReply) -> bool {
        match self.remove_pending(callback_id) {
            Some(sender) => sender.send(reply).is_ok(),
            None => false,
        }
    }
}

struct BridgeHookHandler {
    client: Arc<CallbackClient>,
    handler_id: String,
    timeout_ms: u64,
    runtime: tokio::runtime::Handle,
}

impl a3s_code_core::hooks::HookHandler for BridgeHookHandler {
    fn handle(
        &self,
        event: &a3s_code_core::hooks::HookEvent,
    ) -> a3s_code_core::hooks::HookResponse {
        self.try_handle(event)
            .unwrap_or_else(|_| a3s_code_core::hooks::HookResponse::continue_())
    }

    fn try_handle(
        &self,
        event: &a3s_code_core::hooks::HookEvent,
    ) -> Result<a3s_code_core::hooks::HookResponse, String> {
        let payload = serde_json::to_value(event)
            .map_err(|error| format!("failed to serialize hook event: {error}"))?;
        let value = self
            .runtime
            .block_on(
                self.client
                    .invoke(&self.handler_id, "hook", payload, self.timeout_ms),
            )
            .map_err(|error| error.message)?;
        parse_hook_response(value)
    }
}

fn parse_hook_response(value: Value) -> Result<a3s_code_core::hooks::HookResponse, String> {
    use a3s_code_core::hooks::HookResponse;
    if value.is_null() {
        return Ok(HookResponse::continue_());
    }
    let object = value
        .as_object()
        .ok_or_else(|| "hook callback must return an object or null".to_string())?;
    match object
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("continue")
    {
        "continue" => Ok(
            match object.get("modified").filter(|value| !value.is_null()) {
                Some(modified) => HookResponse::continue_with(modified.clone()),
                None => HookResponse::continue_(),
            },
        ),
        "block" => Ok(HookResponse::block(
            object
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("Blocked by Go hook"),
        )),
        "retry" => {
            let delay_ms = object
                .get("delay_ms")
                .or_else(|| object.get("delayMs"))
                .and_then(Value::as_u64)
                .unwrap_or(1_000);
            Ok(match object.get("reason").and_then(Value::as_str) {
                Some(reason) => HookResponse::retry_with_reason(reason, delay_ms),
                None => HookResponse::retry(delay_ms),
            })
        }
        "skip" => Ok(HookResponse::skip()),
        other => Err(format!("unknown hook action {other:?}")),
    }
}

struct BridgeBudgetGuard {
    client: Arc<CallbackClient>,
    handler_id: String,
    timeout_ms: u64,
}

#[async_trait::async_trait]
impl a3s_code_core::budget::BudgetGuard for BridgeBudgetGuard {
    async fn check_before_llm(
        &self,
        session_id: &str,
        estimated_prompt_tokens: usize,
    ) -> a3s_code_core::budget::BudgetDecision {
        self.decision(
            "check_before_llm",
            json!({
                "session_id": session_id,
                "estimated_tokens": estimated_prompt_tokens,
            }),
        )
        .await
    }

    async fn record_after_llm(&self, session_id: &str, usage: &a3s_code_core::TokenUsage) {
        let _ = self
            .client
            .invoke(
                &self.handler_id,
                "record_after_llm",
                json!({ "session_id": session_id, "usage": usage }),
                self.timeout_ms,
            )
            .await;
    }

    async fn check_before_tool(
        &self,
        session_id: &str,
        tool_name: &str,
    ) -> a3s_code_core::budget::BudgetDecision {
        self.decision(
            "check_before_tool",
            json!({ "session_id": session_id, "tool_name": tool_name }),
        )
        .await
    }
}

impl BridgeBudgetGuard {
    async fn decision(
        &self,
        method: &str,
        payload: Value,
    ) -> a3s_code_core::budget::BudgetDecision {
        match self
            .client
            .invoke(&self.handler_id, method, payload, self.timeout_ms)
            .await
            .and_then(|value| {
                parse_budget_decision(value)
                    .map_err(|message| BridgeFailure::new("CALLBACK_ERROR", message))
            }) {
            Ok(decision) => decision,
            Err(error) => a3s_code_core::budget::BudgetDecision::Deny {
                resource: "budget_guard_callback".to_string(),
                reason: error.message,
            },
        }
    }
}

fn parse_budget_decision(value: Value) -> Result<a3s_code_core::budget::BudgetDecision, String> {
    use a3s_code_core::budget::BudgetDecision;
    if value.is_null() {
        return Ok(BudgetDecision::Allow);
    }
    let object = value
        .as_object()
        .ok_or_else(|| "budget callback must return an object or null".to_string())?;
    match object
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or("allow")
    {
        "allow" => Ok(BudgetDecision::Allow),
        "deny" => Ok(BudgetDecision::Deny {
            resource: required_callback_string(object, "resource")?,
            reason: required_callback_string(object, "reason")?,
        }),
        "soft" => Ok(BudgetDecision::SoftLimit {
            resource: required_callback_string(object, "resource")?,
            consumed: required_callback_number(object, "consumed")?,
            limit: required_callback_number(object, "limit")?,
            message: object
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string),
        }),
        other => Err(format!("unknown budget decision {other:?}")),
    }
}

fn required_callback_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("callback result requires string field {key:?}"))
}

fn required_callback_number(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<f64, String> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("callback result requires finite number field {key:?}"))
}

struct BridgeSlashCommand {
    name: String,
    description: String,
    usage: Option<String>,
    client: Arc<CallbackClient>,
    handler_id: String,
    timeout_ms: u64,
    runtime: tokio::runtime::Handle,
}

impl a3s_code_core::commands::SlashCommand for BridgeSlashCommand {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn usage(&self) -> Option<&str> {
        self.usage.as_deref()
    }

    fn execute(
        &self,
        args: &str,
        context: &a3s_code_core::commands::CommandContext,
    ) -> a3s_code_core::commands::CommandOutput {
        let payload = json!({
            "args": args,
            "context": {
                "session_id": context.session_id,
                "workspace": context.workspace,
                "model": context.model,
                "history_len": context.history_len,
                "total_tokens": context.total_tokens,
                "total_cost": context.total_cost,
                "tool_names": context.tool_names,
                "mcp_servers": context.mcp_servers.iter().map(|(name, count)| {
                    json!({ "name": name, "tool_count": count })
                }).collect::<Vec<_>>(),
            }
        });
        let result = tokio::task::block_in_place(|| {
            self.runtime.block_on(self.client.invoke(
                &self.handler_id,
                "command",
                payload,
                self.timeout_ms,
            ))
        });
        match result {
            Ok(Value::String(text)) => a3s_code_core::commands::CommandOutput::text(text),
            Ok(value) => a3s_code_core::commands::CommandOutput::text(
                value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("Go command returned no text"),
            ),
            Err(error) => a3s_code_core::commands::CommandOutput::text(format!(
                "Command '{}' failed: {}",
                self.name, error.message
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
struct BridgeWorkerAgentSpec {
    name: String,
    description: String,
    #[serde(default = "default_worker_kind")]
    kind: String,
    #[serde(default)]
    hidden: bool,
    permissions: Option<a3s_code_core::permissions::PermissionPolicy>,
    model: Option<String>,
    prompt: Option<String>,
    max_steps: Option<usize>,
    confirmation_inheritance: Option<String>,
}

fn default_worker_kind() -> String {
    "custom".to_string()
}

impl BridgeWorkerAgentSpec {
    fn into_core(self) -> Result<a3s_code_core::WorkerAgentSpec, BridgeFailure> {
        use a3s_code_core::{ConfirmationInheritance, WorkerAgentKind, WorkerAgentSpec};
        let kind = match self.kind.trim().to_ascii_lowercase().as_str() {
            "read_only" | "readonly" | "read-only" | "explore" => WorkerAgentKind::ReadOnly,
            "planner" | "plan" => WorkerAgentKind::Planner,
            "implementer" | "implementation" | "general" => WorkerAgentKind::Implementer,
            "verifier" | "verification" | "verify" => WorkerAgentKind::Verifier,
            "reviewer" | "review" | "code-review" => WorkerAgentKind::Reviewer,
            "custom" => WorkerAgentKind::Custom,
            other => {
                return Err(BridgeFailure::new(
                    "INVALID_REQUEST",
                    format!("unknown worker kind {other:?}"),
                ))
            }
        };
        let confirmation_inheritance = self
            .confirmation_inheritance
            .map(|value| match value.trim().to_ascii_lowercase().as_str() {
                "auto_approve" | "autoapprove" => Ok(ConfirmationInheritance::AutoApprove),
                "deny_on_ask" | "deny" => Ok(ConfirmationInheritance::DenyOnAsk),
                "inherit_parent" | "inherit" => Ok(ConfirmationInheritance::InheritParent),
                other => Err(BridgeFailure::new(
                    "INVALID_REQUEST",
                    format!("unknown confirmation inheritance {other:?}"),
                )),
            })
            .transpose()?;
        Ok(WorkerAgentSpec {
            name: self.name,
            description: self.description,
            kind,
            hidden: self.hidden,
            permissions: self.permissions,
            model: self
                .model
                .map(a3s_code_core::subagent::ModelConfig::from_model_ref),
            prompt: self.prompt,
            max_steps: self.max_steps,
            confirmation_inheritance,
        })
    }
}

#[derive(Debug, Deserialize)]
struct BridgeInlineSkill {
    name: String,
    #[serde(default)]
    kind: String,
    content: String,
}

impl BridgeInlineSkill {
    fn into_core(self) -> Result<a3s_code_core::skills::Skill, BridgeFailure> {
        let name = self.name.trim().to_string();
        if name.is_empty() {
            return Err(BridgeFailure::new(
                "INVALID_REQUEST",
                "skill name cannot be empty",
            ));
        }
        let kind = match self.kind.trim().to_ascii_lowercase().as_str() {
            "" | "instruction" => a3s_code_core::skills::SkillKind::Instruction,
            "persona" => a3s_code_core::skills::SkillKind::Persona,
            "tool" => a3s_code_core::skills::SkillKind::Tool,
            other => {
                return Err(BridgeFailure::new(
                    "INVALID_REQUEST",
                    format!("unknown skill kind {other:?}"),
                ))
            }
        };
        Ok(a3s_code_core::skills::Skill {
            name,
            description: String::new(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind,
            content: self.content,
            tags: Vec::new(),
            version: None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct BridgeLaneHandlerConfig {
    mode: String,
    #[serde(default = "default_lane_timeout")]
    timeout_ms: u64,
}

fn default_lane_timeout() -> u64 {
    60_000
}

impl BridgeLaneHandlerConfig {
    fn into_core(self) -> Result<a3s_code_core::queue::LaneHandlerConfig, BridgeFailure> {
        let mode = match self.mode.trim().to_ascii_lowercase().as_str() {
            "internal" => a3s_code_core::queue::TaskHandlerMode::Internal,
            "external" => a3s_code_core::queue::TaskHandlerMode::External,
            "hybrid" => a3s_code_core::queue::TaskHandlerMode::Hybrid,
            other => {
                return Err(BridgeFailure::new(
                    "INVALID_REQUEST",
                    format!("unknown lane handler mode {other:?}"),
                ))
            }
        };
        Ok(a3s_code_core::queue::LaneHandlerConfig {
            mode,
            timeout_ms: self.timeout_ms,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BridgeSessionQueueConfig {
    control_concurrency: Option<usize>,
    query_concurrency: Option<usize>,
    execute_concurrency: Option<usize>,
    generate_concurrency: Option<usize>,
    lane_handlers: HashMap<String, BridgeLaneHandlerConfig>,
    enable_dlq: Option<bool>,
    dlq_max_size: Option<usize>,
    enable_metrics: Option<bool>,
    enable_alerts: Option<bool>,
    timeout_ms: Option<u64>,
    storage_path: Option<String>,
    enable_all_features: Option<bool>,
}

impl BridgeSessionQueueConfig {
    fn into_core(self) -> Result<a3s_code_core::queue::SessionQueueConfig, BridgeFailure> {
        let mut config = if self.enable_all_features.unwrap_or(false) {
            a3s_code_core::queue::SessionQueueConfig::default().with_lane_features()
        } else {
            a3s_code_core::queue::SessionQueueConfig::default()
        };
        if let Some(value) = self.control_concurrency {
            config.control_max_concurrency = value;
        }
        if let Some(value) = self.query_concurrency {
            config.query_max_concurrency = value;
        }
        if let Some(value) = self.execute_concurrency {
            config.execute_max_concurrency = value;
        }
        if let Some(value) = self.generate_concurrency {
            config.generate_max_concurrency = value;
        }
        for (lane, handler) in self.lane_handlers {
            config
                .lane_handlers
                .insert(parse_lane(&lane)?, handler.into_core()?);
        }
        if self.enable_dlq.unwrap_or(false) {
            config = config.with_dlq(self.dlq_max_size);
        }
        if self.enable_metrics.unwrap_or(false) {
            config = config.with_metrics();
        }
        if self.enable_alerts.unwrap_or(false) {
            config = config.with_alerts();
        }
        if let Some(value) = self.timeout_ms {
            config = config.with_timeout(value);
        }
        if let Some(value) = self.storage_path {
            config = config.with_storage(value);
        }
        Ok(config)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BridgeConfirmationPolicy {
    enabled: Option<bool>,
    default_timeout_ms: Option<u64>,
    timeout_action: Option<String>,
    yolo_lanes: Vec<String>,
}

impl BridgeConfirmationPolicy {
    fn into_core(self) -> Result<a3s_code_core::hitl::ConfirmationPolicy, BridgeFailure> {
        let mut policy = if self.enabled.unwrap_or(false) {
            a3s_code_core::hitl::ConfirmationPolicy::enabled()
        } else {
            a3s_code_core::hitl::ConfirmationPolicy::default()
        };
        let timeout_action = match self
            .timeout_action
            .as_deref()
            .unwrap_or("reject")
            .trim()
            .to_ascii_lowercase()
            .replace('-', "_")
            .as_str()
        {
            "reject" => a3s_code_core::hitl::TimeoutAction::Reject,
            "auto_approve" | "autoapprove" => a3s_code_core::hitl::TimeoutAction::AutoApprove,
            other => {
                return Err(BridgeFailure::new(
                    "INVALID_REQUEST",
                    format!("unknown confirmation timeout action {other:?}"),
                ))
            }
        };
        if let Some(timeout) = self.default_timeout_ms {
            policy = policy.with_timeout(timeout, timeout_action);
        }
        if !self.yolo_lanes.is_empty() {
            policy = policy.with_yolo_lanes(
                self.yolo_lanes
                    .iter()
                    .map(|lane| parse_lane(lane))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        Ok(policy)
    }
}

#[derive(Debug, Deserialize)]
struct BridgeArtifactStoreLimits {
    max_artifacts: usize,
    max_bytes: usize,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BridgeAutoDelegation {
    enabled: Option<bool>,
    auto_parallel: Option<bool>,
    min_confidence: Option<f32>,
    max_tasks: Option<usize>,
}

impl BridgeAutoDelegation {
    fn into_core(self) -> a3s_code_core::AutoDelegationConfig {
        let mut config = a3s_code_core::AutoDelegationConfig::default();
        if let Some(value) = self.enabled {
            config.enabled = value;
        }
        if let Some(value) = self.auto_parallel {
            config.auto_parallel = value;
        }
        if let Some(value) = self.min_confidence {
            config.min_confidence = value.clamp(0.0, 1.0);
        }
        if let Some(value) = self.max_tasks {
            config.max_tasks = value.max(1);
        }
        config
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BridgeRetentionLimits {
    unbounded: bool,
    max_runs_retained: Option<usize>,
    max_events_per_run: Option<usize>,
    max_event_bytes_per_run: Option<usize>,
    max_trace_events: Option<usize>,
    max_terminal_subagent_tasks: Option<usize>,
}

impl BridgeRetentionLimits {
    fn into_core(self) -> a3s_code_core::retention::SessionRetentionLimits {
        let mut limits = if self.unbounded {
            a3s_code_core::retention::SessionRetentionLimits::unbounded()
        } else {
            a3s_code_core::retention::SessionRetentionLimits::default()
        };
        if let Some(value) = self.max_runs_retained {
            limits.max_runs_retained = Some(value);
        }
        if let Some(value) = self.max_events_per_run {
            limits.max_events_per_run = Some(value);
        }
        if let Some(value) = self.max_event_bytes_per_run {
            limits.max_event_bytes_per_run = Some(value);
        }
        if let Some(value) = self.max_trace_events {
            limits.max_trace_events = Some(value);
        }
        if let Some(value) = self.max_terminal_subagent_tasks {
            limits.max_terminal_subagent_tasks = Some(value);
        }
        limits
    }
}

#[derive(Debug, Deserialize)]
struct BridgeTrajectoryConfig {
    path: String,
    mode: Option<String>,
    max_text_bytes: Option<usize>,
    include_messages: Option<bool>,
}

impl BridgeTrajectoryConfig {
    fn into_core(self) -> Result<a3s_code_core::RlTrajectoryConfig, BridgeFailure> {
        let mut config = a3s_code_core::RlTrajectoryConfig::new(self.path);
        if let Some(mode) = self.mode {
            let parsed = a3s_code_core::RlTrajectoryMode::parse(&mode).ok_or_else(|| {
                BridgeFailure::new("INVALID_REQUEST", "trajectory mode must be on or off")
            })?;
            config = config.with_mode(parsed);
        }
        if let Some(value) = self.max_text_bytes {
            config = config.with_max_text_bytes(value);
        }
        if let Some(value) = self.include_messages {
            config = config.with_include_messages(value);
        }
        Ok(config)
    }
}

#[derive(Debug, Deserialize)]
struct BridgeWorkspaceBackend {
    kind: String,
    root: Option<String>,
    s3: Option<BridgeS3Config>,
}

#[derive(Debug, Deserialize)]
struct BridgeS3Config {
    endpoint: Option<String>,
    region: Option<String>,
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    bucket: String,
    prefix: String,
    force_path_style: Option<bool>,
    request_timeout_ms: Option<u64>,
    max_read_bytes: Option<u64>,
    search_enabled: Option<bool>,
    max_objects_scanned: Option<usize>,
    max_grep_bytes_per_object: Option<u64>,
    search_concurrency: Option<usize>,
}

impl BridgeS3Config {
    fn into_core(self) -> a3s_code_core::S3BackendConfig {
        let mut config = a3s_code_core::S3BackendConfig::new(
            self.bucket,
            self.prefix,
            self.access_key_id,
            self.secret_access_key,
        );
        if let Some(value) = self.endpoint {
            config = config.endpoint(value);
        }
        if let Some(value) = self.region {
            config = config.region(value);
        }
        if let Some(value) = self.session_token {
            config = config.session_token(value);
        }
        if let Some(value) = self.force_path_style {
            config = config.force_path_style(value);
        }
        if let Some(value) = self.request_timeout_ms {
            config = config.request_timeout(std::time::Duration::from_millis(value));
        }
        if let Some(value) = self.max_read_bytes {
            config = config.max_read_bytes(value);
        }
        if let Some(value) = self.search_enabled {
            config = config.enable_search(value);
        }
        if let Some(value) = self.max_objects_scanned {
            config = config.max_objects_scanned(value);
        }
        if let Some(value) = self.max_grep_bytes_per_object {
            config = config.max_grep_bytes_per_object(value);
        }
        if let Some(value) = self.search_concurrency {
            config = config.search_concurrency(value);
        }
        config
    }
}

#[derive(Debug, Deserialize)]
struct BridgeRemoteGitConfig {
    base_url: String,
    repo_id: String,
    bearer_token: Option<String>,
    client_cert_pem: Option<String>,
    client_key_pem: Option<String>,
    request_timeout_ms: Option<u64>,
    max_diff_bytes: Option<u64>,
    max_log_entries: Option<usize>,
}

impl BridgeRemoteGitConfig {
    fn into_core(self) -> a3s_code_core::RemoteGitBackendConfig {
        let mut config = a3s_code_core::RemoteGitBackendConfig::new(self.base_url, self.repo_id);
        if let Some(value) = self.bearer_token {
            config = config.bearer_token(value);
        }
        if let Some(value) = self.client_cert_pem {
            config = config.client_cert_pem(value);
        }
        if let Some(value) = self.client_key_pem {
            config = config.client_key_pem(value);
        }
        if let Some(value) = self.request_timeout_ms {
            config = config.request_timeout(std::time::Duration::from_millis(value));
        }
        if let Some(value) = self.max_diff_bytes {
            config = config.max_diff_bytes(value);
        }
        if let Some(value) = self.max_log_entries {
            config = config.max_log_entries(value);
        }
        config
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BridgeSessionOptions {
    model: Option<String>,
    task_priority: Option<String>,
    builtin_skills: Option<bool>,
    agent_dirs: Vec<String>,
    skill_dirs: Vec<String>,
    worker_agents: Vec<BridgeWorkerAgentSpec>,
    queue_config: Option<BridgeSessionQueueConfig>,
    permission_policy: Option<a3s_code_core::permissions::PermissionPolicy>,
    confirmation_policy: Option<BridgeConfirmationPolicy>,
    enforce_active_skill_tool_restrictions: Option<bool>,
    file_memory_dir: Option<String>,
    file_session_store_dir: Option<String>,
    security_provider: Option<BridgeDefaultSecurityProvider>,
    default_security: Option<bool>,
    workspace_backend: Option<BridgeWorkspaceBackend>,
    remote_git: Option<BridgeRemoteGitConfig>,
    workspace_retrieval: Option<BridgeWorkspaceRetrievalOptions>,
    session_id: Option<String>,
    tenant_id: Option<String>,
    principal: Option<String>,
    agent_template_id: Option<String>,
    correlation_id: Option<String>,
    host_env: Option<BridgeHostEnvConfig>,
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
    artifact_store_limits: Option<BridgeArtifactStoreLimits>,
    tool_result_transform_policy: Option<a3s_code_core::tools::ToolResultTransformPolicyV1>,
    tool_presentation_profile: Option<a3s_code_core::tools::ToolPresentationProfileV1>,
    continuation_enabled: Option<bool>,
    max_continuation_turns: Option<u32>,
    temperature: Option<f32>,
    thinking_budget: Option<usize>,
    max_tool_rounds: Option<usize>,
    max_parallel_tasks: Option<usize>,
    auto_delegation_enabled: Option<bool>,
    auto_delegation: Option<BridgeAutoDelegation>,
    manual_delegation_enabled: Option<bool>,
    auto_parallel_delegation: Option<bool>,
    llm_logprobs: Option<bool>,
    llm_top_logprobs: Option<usize>,
    max_execution_time_ms: Option<u64>,
    retention_limits: Option<BridgeRetentionLimits>,
    trajectory: Option<BridgeTrajectoryConfig>,
    inline_skills: Vec<BridgeInlineSkill>,
    prompt_slots: Option<BridgePromptSlots>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BridgeDefaultSecurityProvider {}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BridgePromptSlots {
    role: Option<String>,
    guidelines: Option<String>,
    response_style: Option<String>,
    extra: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BridgeHostEnvConfig {
    sequential_id_prefix: Option<String>,
    fixed_time_ms: Option<u64>,
}

impl BridgeSessionOptions {
    fn into_core(
        self,
        callbacks: Option<Arc<CallbackClient>>,
    ) -> Result<SessionOptions, BridgeFailure> {
        if self.security_provider.is_some() && self.default_security.is_some() {
            return Err(BridgeFailure::new(
                "INVALID_REQUEST",
                "security_provider and deprecated default_security cannot both be set",
            ));
        }
        let mut options = SessionOptions::new();
        if let Some(value) = self.model {
            options = options.with_model(value);
        }
        if let Some(value) = self.task_priority {
            options = options.with_task_priority(value.parse().map_err(|error| {
                BridgeFailure::new("INVALID_REQUEST", format!("task_priority: {error}"))
            })?);
        }
        if self.builtin_skills.unwrap_or(false) {
            options = options.with_builtin_skills();
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
        for worker in self.worker_agents {
            options = options.with_worker_agent(worker.into_core()?);
        }
        if let Some(value) = self.queue_config {
            options = options.with_queue_config(value.into_core()?);
        }
        if let Some(value) = self.permission_policy {
            options = options.with_permission_policy(value);
        }
        if let Some(value) = self.confirmation_policy {
            options = options.with_confirmation_policy(value.into_core()?);
        }
        if let Some(value) = self.file_memory_dir {
            options = options.with_file_memory(value);
        }
        if let Some(value) = self.file_session_store_dir {
            options = options.with_file_session_store(value);
        }
        if self.security_provider.is_some() || self.default_security.unwrap_or(false) {
            options = options.with_default_security();
        }
        if let Some(value) = self.workspace_backend {
            let services = match value.kind.trim().to_ascii_lowercase().as_str() {
                "" | "local" => {
                    let root = value.root.ok_or_else(|| {
                        BridgeFailure::new(
                            "INVALID_REQUEST",
                            "local workspace backend requires root",
                        )
                    })?;
                    a3s_code_core::WorkspaceServices::local(root)
                }
                "s3" => {
                    let config = value.s3.ok_or_else(|| {
                        BridgeFailure::new(
                            "INVALID_REQUEST",
                            "S3 workspace backend requires s3 configuration",
                        )
                    })?;
                    a3s_code_core::WorkspaceServices::s3(config.into_core())
                }
                other => {
                    return Err(BridgeFailure::new(
                        "INVALID_REQUEST",
                        format!("unsupported workspace backend kind {other:?}"),
                    ))
                }
            };
            let services = if let Some(remote_git) = self.remote_git {
                services
                    .with_remote_git(remote_git.into_core())
                    .map_err(|error| BridgeFailure::new("INVALID_REQUEST", error.to_string()))?
            } else {
                services
            };
            options = options.with_workspace_backend(services);
        } else if self.remote_git.is_some() {
            return Err(BridgeFailure::new(
                "INVALID_REQUEST",
                "remote_git requires workspace_backend",
            ));
        }
        if let Some(value) = self.workspace_retrieval {
            let callbacks = callbacks.ok_or_else(|| {
                BridgeFailure::new(
                    "CALLBACK_UNAVAILABLE",
                    "workspace_retrieval requires the callback transport",
                )
            })?;
            options = options.with_workspace_retrieval(value.into_core(callbacks)?);
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
        if let Some(value) = self.host_env {
            use a3s_code_core::host_env::{
                Clock, FixedClock, HostEnv, IdGenerator, SequentialIdGenerator, SystemClock,
                SystemIdGenerator,
            };

            let id_generator: Arc<dyn IdGenerator> =
                if let Some(prefix) = value.sequential_id_prefix {
                    Arc::new(SequentialIdGenerator::new(prefix))
                } else {
                    Arc::new(SystemIdGenerator)
                };
            let clock: Arc<dyn Clock> = if let Some(now_ms) = value.fixed_time_ms {
                Arc::new(FixedClock::new(now_ms))
            } else {
                Arc::new(SystemClock)
            };
            options = options.with_host_env(Arc::new(HostEnv::new(id_generator, clock)));
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
        if let Some(value) = self.artifact_store_limits {
            options =
                options.with_artifact_store_limits(a3s_code_core::tools::ArtifactStoreLimits {
                    max_artifacts: value.max_artifacts,
                    max_bytes: value.max_bytes,
                });
        }
        if let Some(value) = self.tool_result_transform_policy {
            options = options.with_tool_result_transform_policy(value);
        }
        if let Some(value) = self.tool_presentation_profile {
            value.validate().map_err(|error| {
                BridgeFailure::new(
                    "INVALID_REQUEST",
                    format!("tool_presentation_profile: {error}"),
                )
            })?;
            options = options.with_tool_presentation_profile(value);
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
        if let Some(value) = self.auto_delegation {
            options = options.with_auto_delegation(value.into_core());
        }
        if let Some(value) = self.manual_delegation_enabled {
            options = options.with_manual_delegation_enabled(value);
        }
        if let Some(value) = self.auto_parallel_delegation {
            options = options.with_auto_parallel_delegation(value);
        }
        if let Some(value) = self.llm_logprobs {
            options = options.with_llm_logprobs(value);
        }
        if let Some(value) = self.llm_top_logprobs {
            options = options.with_llm_top_logprobs(value);
        }
        if let Some(value) = self.retention_limits {
            options = options.with_retention_limits(value.into_core());
        }
        if let Some(value) = self.trajectory {
            options = options.with_rl_trajectory(value.into_core()?);
        }
        if !self.inline_skills.is_empty() {
            let registry = a3s_code_core::skills::SkillRegistry::new();
            for skill in self.inline_skills {
                registry.register_unchecked(Arc::new(skill.into_core()?));
            }
            options = options.with_skill_registry(Arc::new(registry));
        }
        if let Some(value) = self.max_execution_time_ms {
            options.max_execution_time_ms = Some(value);
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

fn parse_lane(value: &str) -> Result<a3s_code_core::queue::SessionLane, BridgeFailure> {
    match value.trim().to_ascii_lowercase().as_str() {
        "control" => Ok(a3s_code_core::queue::SessionLane::Control),
        "query" => Ok(a3s_code_core::queue::SessionLane::Query),
        "execute" => Ok(a3s_code_core::queue::SessionLane::Execute),
        "generate" => Ok(a3s_code_core::queue::SessionLane::Generate),
        other => Err(BridgeFailure::new(
            "INVALID_REQUEST",
            format!("unknown session lane {other:?}"),
        )),
    }
}

fn memory_failure(error: anyhow::Error) -> BridgeFailure {
    BridgeFailure::new("MEMORY_ERROR", error.to_string())
}

fn queue_metrics_value(metrics: Option<a3s_code_core::queue::MetricsSnapshot>) -> Value {
    let Some(metrics) = metrics else {
        return Value::Null;
    };
    let histograms = metrics
        .histograms
        .into_iter()
        .map(|(name, stats)| {
            (
                name,
                json!({
                    "count": stats.count,
                    "sum": stats.sum,
                    "min": if stats.count == 0 { 0.0 } else { stats.min },
                    "max": if stats.count == 0 { 0.0 } else { stats.max },
                    "mean": stats.mean,
                    "p50": stats.percentiles.p50,
                    "p90": stats.percentiles.p90,
                    "p95": stats.percentiles.p95,
                    "p99": stats.percentiles.p99,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        "counters": metrics.counters,
        "gauges": metrics.gauges,
        "histograms": histograms,
    })
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

fn run_spawn_value(spawn: &a3s_code_core::AgentRunSpawn) -> Result<Value, BridgeFailure> {
    encode(json!({
        "snapshot": spawn.snapshot(),
        "replayed": spawn.replayed(),
    }))
}

fn agent_definition_value(definition: a3s_code_core::AgentDefinition) -> Value {
    let permissions = definition.permissions;
    json!({
        "name": definition.name,
        "description": definition.description,
        "native": definition.native,
        "hidden": definition.hidden,
        "permissions": {
            "deny": permissions.deny.into_iter().map(|rule| rule.rule).collect::<Vec<_>>(),
            "allow": permissions.allow.into_iter().map(|rule| rule.rule).collect::<Vec<_>>(),
            "ask": permissions.ask.into_iter().map(|rule| rule.rule).collect::<Vec<_>>(),
            "default_decision": permissions.default_decision,
            "enabled": permissions.enabled,
        },
        "model": definition.model.map(|model| model.model_ref()),
        "prompt": definition.prompt,
        "max_steps": definition.max_steps,
        "tool_free": definition.tool_free,
        "confirmation_inheritance": definition.confirmation_inheritance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_retry_response_preserves_reason_and_delay() {
        let response = parse_hook_response(json!({
            "action": "retry",
            "reason": "approval service is temporarily unavailable",
            "delay_ms": 625,
        }))
        .unwrap();

        assert!(matches!(
            response.action,
            a3s_code_core::hooks::HookAction::Retry
        ));
        assert_eq!(
            response.reason.as_deref(),
            Some("approval service is temporarily unavailable")
        );
        assert_eq!(response.retry_delay_ms, Some(625));
    }

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
    async fn serve_lifecycle_is_ready_before_return_and_stop_is_joined() {
        let agent_dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(
            agent_dir.path().join("instructions.md"),
            "You are a test agent.",
        )
        .unwrap();
        let state = BridgeState::new();
        let created = state
            .dispatch(&request(
                "agent_create",
                json!({ "config_source": test_acl() }),
            ))
            .await
            .unwrap();
        let agent_id = created["agent_id"].as_str().unwrap();
        let started = state
            .dispatch(&request(
                "agent_serve_agent_dir",
                json!({
                    "agent_id": agent_id,
                    "dir": agent_dir.path(),
                    "workspace": workspace.path(),
                }),
            ))
            .await
            .unwrap();
        let serve_handle = started["serve_handle"].as_str().unwrap();

        let status = state
            .dispatch(&request(
                "agent_serve_status",
                json!({ "serve_handle": serve_handle }),
            ))
            .await
            .unwrap();
        assert_eq!(status["phase"], "ready");
        assert_eq!(status["ready"], true);
        assert_eq!(status["stopped"], false);
        assert!(status["failure_code"].is_null());

        let stopped = state
            .dispatch(&request(
                "agent_stop_serve",
                json!({ "serve_handle": serve_handle }),
            ))
            .await
            .unwrap();
        assert_eq!(stopped["stopped"], true);
        assert!(state.serve_handles.read().await.is_empty());
    }

    #[tokio::test]
    async fn invalid_schedule_fails_before_bridge_activation() {
        let agent_dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(
            agent_dir.path().join("instructions.md"),
            "You are a test agent.",
        )
        .unwrap();
        std::fs::create_dir(agent_dir.path().join("schedules")).unwrap();
        std::fs::write(
            agent_dir.path().join("schedules/invalid.md"),
            "---\ncron: not-a-cron\n---\nDo work.",
        )
        .unwrap();
        let state = BridgeState::new();
        let created = state
            .dispatch(&request(
                "agent_create",
                json!({ "config_source": test_acl() }),
            ))
            .await
            .unwrap();
        let error = state
            .dispatch(&request(
                "agent_serve_agent_dir",
                json!({
                    "agent_id": created["agent_id"],
                    "dir": agent_dir.path(),
                    "workspace": workspace.path(),
                }),
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code, "SERVE_STARTUP_FAILED");
        assert!(state.serve_handles.read().await.is_empty());
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

        let agent_stats = state
            .dispatch(&request(
                "agent_task_scheduler_stats",
                json!({ "agent_id": agent_id }),
            ))
            .await
            .unwrap();
        let session_stats = state
            .dispatch(&request(
                "session_task_scheduler_stats",
                json!({ "session_handle": session_handle }),
            ))
            .await
            .unwrap();
        assert_eq!(agent_stats["maxActive"], session_stats["maxActive"]);
        assert_eq!(agent_stats["pending"], Value::from(0));

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
    async fn governed_direct_tool_applies_session_permission_policy() {
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
                    "options": {
                        "session_id": "go-governed-tool-test",
                        "permission_policy": {
                            "deny": ["write"],
                            "default_decision": "allow"
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let session_handle = created["session_handle"].as_str().unwrap();

        let trusted = state
            .dispatch(&request(
                "session_tool",
                json!({
                    "session_handle": session_handle,
                    "name": "write",
                    "args": {
                        "file_path": "trusted-host-write.txt",
                        "content": "trusted"
                    }
                }),
            ))
            .await
            .unwrap();
        assert_eq!(trusted["exit_code"], Value::from(0));
        assert!(workspace.path().join("trusted-host-write.txt").exists());

        let governed = state
            .dispatch(&request(
                "session_governed_tool",
                json!({
                    "session_handle": session_handle,
                    "name": "write",
                    "args": {
                        "file_path": "denied-governed-write.txt",
                        "content": "must not exist"
                    }
                }),
            ))
            .await
            .unwrap();
        assert_ne!(governed["exit_code"], Value::from(0));
        assert!(!workspace.path().join("denied-governed-write.txt").exists());
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

    #[tokio::test]
    async fn callback_transport_round_trips_and_cleans_up_timeouts() {
        let (writer, mut envelopes) = mpsc::unbounded_channel();
        let client = Arc::new(CallbackClient::new(writer));

        let pending = {
            let client = Arc::clone(&client);
            tokio::spawn(async move {
                client
                    .invoke(
                        "go-handler",
                        "hook",
                        json!({ "event": "pre_tool_use" }),
                        1_000,
                    )
                    .await
            })
        };
        let envelope = envelopes.recv().await.unwrap();
        let callback = envelope.callback.unwrap();
        assert_eq!(callback.handler_id, "go-handler");
        assert_eq!(callback.method, "hook");
        assert!(client.respond(
            callback.callback_id,
            CallbackReply {
                result: Some(json!({ "action": "block" })),
                error: None,
            },
        ));
        assert_eq!(
            pending.await.unwrap().unwrap(),
            json!({ "action": "block" })
        );
        assert_eq!(client.pending_len(), 0);

        let timed_out = {
            let client = Arc::clone(&client);
            tokio::spawn(async move {
                client
                    .invoke("slow-handler", "command", Value::Null, 1)
                    .await
            })
        };
        let callback = envelopes.recv().await.unwrap();
        assert_eq!(callback.kind, "callback");
        let error = timed_out.await.unwrap().unwrap_err();
        assert_eq!(error.code, "CALLBACK_TIMEOUT");
        let cancellation = envelopes.recv().await.unwrap();
        assert_eq!(cancellation.kind, "callback_cancel");
        assert_eq!(
            cancellation.callback_cancel.unwrap().callback_id,
            callback.callback.unwrap().callback_id
        );
        assert_eq!(client.pending_len(), 0);

        let cancelled = {
            let client = Arc::clone(&client);
            tokio::spawn(async move {
                client
                    .invoke("cancelled-handler", "embedding", Value::Null, 10_000)
                    .await
            })
        };
        let callback = envelopes.recv().await.unwrap();
        let callback_id = callback.callback.unwrap().callback_id;
        cancelled.abort();
        let _ = cancelled.await;
        let cancellation = envelopes.recv().await.unwrap();
        assert_eq!(cancellation.kind, "callback_cancel");
        assert_eq!(
            cancellation.callback_cancel.unwrap().callback_id,
            callback_id
        );
        assert_eq!(client.pending_len(), 0);
    }

    #[test]
    fn session_options_map_sdk_value_configuration() {
        let bridge: BridgeSessionOptions = serde_json::from_value(json!({
            "model": "anthropic/test-model",
            "task_priority": "background",
            "agent_dirs": ["agents"],
            "skill_dirs": ["skills"],
            "session_id": "session-1",
            "tenant_id": "tenant-1",
            "principal": "user-1",
            "agent_template_id": "template-1",
            "correlation_id": "trace-1",
            "host_env": {
                "sequential_id_prefix": "replay",
                "fixed_time_ms": 1_700_000_000_000_u64
            },
            "goal_tracking": true,
            "auto_save": true,
            "max_parse_retries": 4,
            "tool_timeout_ms": 1_500,
            "llm_api_timeout_ms": 2_500,
            "circuit_breaker_threshold": 5,
            "duplicate_tool_call_threshold": 6,
            "auto_compact": true,
            "auto_compact_threshold": 0.7,
            "max_context_tokens": 32_000,
            "tool_presentation_profile": {
                "schema": "a3s.code.tool-presentation-profile.v1",
                "mode": "code"
            },
            "continuation_enabled": false,
            "max_continuation_turns": 7,
            "temperature": 0.25,
            "thinking_budget": 2_048,
            "max_tool_rounds": 8,
            "max_parallel_tasks": 3,
            "manual_delegation_enabled": false,
            "auto_parallel_delegation": true,
            "llm_logprobs": true,
            "llm_top_logprobs": 2,
            "max_execution_time_ms": 60_000,
            "prompt_slots": {
                "role": "reviewer",
                "guidelines": "be precise"
            }
        }))
        .unwrap();
        let options = bridge.into_core(None).unwrap();

        assert_eq!(options.model.as_deref(), Some("anthropic/test-model"));
        assert_eq!(
            options.task_priority,
            a3s_code_core::TaskPriority::Background
        );
        assert_eq!(options.agent_dirs, vec![PathBuf::from("agents")]);
        assert_eq!(options.skill_dirs, vec![PathBuf::from("skills")]);
        assert_eq!(options.session_id.as_deref(), Some("session-1"));
        assert_eq!(options.tenant_id.as_deref(), Some("tenant-1"));
        assert_eq!(options.principal.as_deref(), Some("user-1"));
        assert_eq!(options.agent_template_id.as_deref(), Some("template-1"));
        assert_eq!(options.correlation_id.as_deref(), Some("trace-1"));
        let host_env = options.host_env.as_ref().expect("host env");
        assert_eq!(host_env.next_id(), "replay-0");
        assert_eq!(host_env.next_id(), "replay-1");
        assert_eq!(host_env.now_ms(), 1_700_000_000_000);
        assert!(options.goal_tracking);
        assert!(options.auto_save);
        assert_eq!(options.max_parse_retries, Some(4));
        assert_eq!(options.tool_timeout_ms, Some(1_500));
        assert_eq!(options.llm_api_timeout_ms, Some(2_500));
        assert_eq!(options.circuit_breaker_threshold, Some(5));
        assert_eq!(options.duplicate_tool_call_threshold, Some(6));
        assert!(options.auto_compact);
        assert_eq!(options.auto_compact_threshold, Some(0.7));
        assert_eq!(options.max_context_tokens, Some(32_000));
        assert_eq!(
            options.tool_presentation_profile,
            Some(a3s_code_core::tools::ToolPresentationProfileV1::code())
        );
        assert_eq!(options.continuation_enabled, Some(false));
        assert_eq!(options.max_continuation_turns, Some(7));
        assert_eq!(options.temperature, Some(0.25));
        assert_eq!(options.thinking_budget, Some(2_048));
        assert_eq!(options.max_tool_rounds, Some(8));
        assert_eq!(options.max_parallel_tasks, Some(3));
        assert_eq!(options.manual_delegation_enabled, Some(false));
        assert_eq!(options.auto_parallel_delegation, Some(true));
        assert_eq!(options.llm_logprobs, Some(true));
        assert_eq!(options.llm_top_logprobs, Some(2));
        assert_eq!(options.max_execution_time_ms, Some(60_000));
        let slots = options.prompt_slots.unwrap();
        assert_eq!(slots.role.as_deref(), Some("reviewer"));
        assert_eq!(slots.guidelines.as_deref(), Some("be precise"));
    }

    #[test]
    fn typed_security_provider_is_closed_and_legacy_flag_remains_compatible() {
        let typed: BridgeSessionOptions = serde_json::from_value(json!({
            "security_provider": {}
        }))
        .unwrap();
        assert!(typed.into_core(None).unwrap().security_provider.is_some());

        let legacy: BridgeSessionOptions = serde_json::from_value(json!({
            "default_security": true
        }))
        .unwrap();
        assert!(legacy.into_core(None).unwrap().security_provider.is_some());

        let conflicting: BridgeSessionOptions = serde_json::from_value(json!({
            "security_provider": {},
            "default_security": true
        }))
        .unwrap();
        let error = conflicting.into_core(None).unwrap_err();
        assert_eq!(error.code, "INVALID_REQUEST");
        assert!(error.message.contains("security_provider"));
        assert!(error.message.contains("default_security"));

        assert!(serde_json::from_value::<BridgeSessionOptions>(json!({
            "security_provider": { "kind": "unknown" }
        }))
        .is_err());
    }

    #[test]
    fn tool_presentation_profile_rejects_unknown_schema_or_mode() {
        let unknown_schema: BridgeSessionOptions = serde_json::from_value(json!({
            "tool_presentation_profile": {
                "schema": "a3s.code.tool-presentation-profile.v2",
                "mode": "code"
            }
        }))
        .unwrap();
        let error = unknown_schema.into_core(None).unwrap_err();
        assert_eq!(error.code, "INVALID_REQUEST");
        assert!(error.message.contains("tool_presentation_profile"));

        assert!(serde_json::from_value::<BridgeSessionOptions>(json!({
            "tool_presentation_profile": {
                "schema": "a3s.code.tool-presentation-profile.v1",
                "mode": "automatic"
            }
        }))
        .is_err());
    }

    #[test]
    fn remote_git_requires_a_workspace_backend() {
        let bridge: BridgeSessionOptions = serde_json::from_value(json!({
            "remote_git": {
                "base_url": "https://git.example.test",
                "repo_id": "repo-1"
            }
        }))
        .unwrap();
        let error = bridge.into_core(None).err().unwrap();
        assert_eq!(error.code, "INVALID_REQUEST");
    }
}
