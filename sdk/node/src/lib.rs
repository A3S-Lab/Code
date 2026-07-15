//! A3S Code Node.js Bindings
//!
//! Native Node.js addon via napi-rs that wraps `a3s-code-core`'s Agent API.
//!
//! ## Usage
//!
//! ```javascript
//! const { Agent } = require('@a3s-lab/code');
//!
//! const agent = await Agent.create('agent.acl');
//! const session = agent.session('/my-project');
//!
//! const result = await session.send('What files handle auth?');
//! console.log(result.text);
//! ```
//!
//! ## Panic safety at the FFI boundary
//!
//! napi 2.x does **not** wrap exported bodies in `catch_unwind` by default. A
//! Rust panic that reaches the `extern "C"` boundary aborts the whole Node
//! process (Rust ≥ 1.81) — it does *not* become a catchable JS error. Only two
//! contexts are panic-safe: a `#[napi]` **async** fn / `impl Future` (panic →
//! rejected Promise) and a sync fn explicitly tagged `#[napi(catch_unwind)]`.
//! Everything else aborts (or silently loses the panic): default **sync**
//! `#[napi]` fns, `ThreadsafeFunction` callbacks (a panic there — or a
//! return-value conversion `Err` — aborts via `napi_fatal_error` under *both*
//! `ErrorStrategy` variants), `tokio::spawn`'d task bodies (panic swallowed,
//! never surfaced), `Drop`/finalizers, and module init.
//!
//! Convention this crate follows so the boundary stays safe: never
//! `.unwrap()` / `.expect()` / `panic!` in those contexts. Propagate with `?`
//! into a `napi::Error`, or fail closed with `unwrap_or_else` inside
//! threadsafe callbacks. (Audited 2026-05: the only production panic site is
//! the lazy Tokio-runtime build in `fallback_runtime()`, reached from within
//! `#[napi]` bodies; the spawned-task and threadsafe-callback paths are
//! panic-free by construction.)

#[macro_use]
extern crate napi_derive;

mod js_callback_bridge;
mod js_slash_command;
mod state_graph;
use js_callback_bridge::{decode_callback_outcome, wrap_sync_callback, JsCallbackOutcome};
use js_slash_command::{js_command_context_to_object, JsSlashCommand};
pub use state_graph::JsStateGraphRuntime;

use a3s_code_core::commands::CommandContext as RustCommandContext;
use a3s_code_core::config::AgentDir as RustAgentDir;
use a3s_code_core::hitl::{
    ConfirmationPolicy as RustConfirmationPolicy, TimeoutAction as RustTimeoutAction,
};
use a3s_code_core::hooks::{
    Hook as RustHook, HookConfig as RustHookConfig, HookEvent as RustHookEvent,
    HookEventType as RustHookEventType, HookHandler as RustHookHandler,
    HookMatcher as RustHookMatcher, HookResponse as RustHookResponse,
};
use a3s_code_core::llm::{ContentBlock as RustContentBlock, Message as RustMessage};
use a3s_code_core::orchestration::{
    execute_pipeline, execute_steps_parallel, execute_steps_parallel_resumable,
    AgentStepSpec as RustAgentStepSpec, PipelineStage as RustPipelineStage,
    StepOutcome as RustStepOutcome, ToolSourceAnchor as RustToolSourceAnchor,
};
use a3s_code_core::permissions::{
    PermissionDecision as RustPermissionDecision, PermissionPolicy as RustPermissionPolicy,
    PermissionRule as RustPermissionRule,
};
use a3s_code_core::queue::{
    ExternalTaskResult as RustExternalTaskResult, LaneHandlerConfig as RustLaneHandlerConfig,
    MetricsSnapshot as RustMetricsSnapshot, SessionLane as RustSessionLane,
    SessionQueueConfig as RustSessionQueueConfig, TaskHandlerMode as RustTaskHandlerMode,
};
use a3s_code_core::serve::serve_agent_dir as rust_serve_agent_dir;
use a3s_code_core::skills::{
    builtin_skills as rust_builtin_skills, Skill as RustSkill, SkillKind as RustSkillKind,
};
use a3s_code_core::subagent::{
    AgentDefinition as RustAgentDefinition, ModelConfig as RustAgentModelConfig,
    WorkerAgentKind as RustWorkerAgentKind, WorkerAgentSpec as RustWorkerAgentSpec,
};
use a3s_code_core::verification::{
    format_verification_summary as rust_format_verification_summary,
    VerificationCommand as RustVerificationCommand, VerificationReport as RustVerificationReport,
    VerificationStatus as RustVerificationStatus, VerificationSummary as RustVerificationSummary,
};
use a3s_code_core::{
    run_event_envelope_v1 as rust_run_event_envelope_v1, Agent as RustAgent,
    AgentEvent as RustAgentEvent, AgentEventProjectionV1 as RustAgentEventProjectionV1,
    AgentResult as RustAgentResult, AgentSession as RustAgentSession,
    EventProtocolError as RustEventProtocolError, PlanningMode as RustPlanningMode,
    SessionOptions as RustSessionOptions, AGENT_EVENT_TYPES_V1, EVENT_ENVELOPE_V1_VERSION,
};
use napi::Either;
use napi::Env;
use tokio_util::sync::CancellationToken;

const MEMORY_UNAVAILABLE_MESSAGE: &str =
    "Memory unavailable for this session; check session initWarning";

fn node_code_error(error: a3s_code_core::CodeError) -> napi::Error {
    napi::Error::from_reason(format!("[A3S_CODE_ERROR:{}] {}", error.code(), error))
}

use std::future::Future;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock, Weak,
};

// ============================================================================
// Tokio Runtime
// ============================================================================

struct NapiRuntime;

fn fallback_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("a3s-code-node-worker")
            .build()
            .expect("failed to create Tokio runtime for Node bindings")
    })
}

impl NapiRuntime {
    fn spawn<F>(&self, fut: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        // Try the current runtime first; otherwise use the binding-owned runtime.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(fut)
        } else {
            fallback_runtime().spawn(fut)
        }
    }

    fn block_on<F: Future>(&self, fut: F) -> F::Output {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.block_on(fut)
        } else {
            fallback_runtime().block_on(fut)
        }
    }
}

fn get_runtime() -> NapiRuntime {
    NapiRuntime
}

// ============================================================================
// ToolResult
// ============================================================================

#[napi(object)]
#[derive(Clone)]
pub struct ToolResult {
    pub name: String,
    pub output: String,
    pub exit_code: i32,
    /// Raw JSON-encoded tool metadata returned by the Rust core API.
    pub metadata_json: Option<String>,
    /// Convenience JSON view of `metadata.document_runtime` when present.
    pub document_runtime_json: Option<String>,
    /// Structured discriminant for tool failures, JSON-encoded with a
    /// `type` field on the top level — e.g.
    /// `{"type":"version_conflict","path":"doc.md","expected":"etag-1","actual":"etag-2"}`.
    /// `None` on success or untyped failure. SDK callers parse it to
    /// branch on the failure kind without scanning the `output` string.
    pub error_kind_json: Option<String>,
}

/// Optional line-range controls for `Session.readFile`.
#[napi(object)]
#[derive(Clone)]
pub struct ReadFileOptions {
    /// 0-indexed line offset to start reading from.
    pub offset: Option<u32>,
    /// Maximum number of lines to read.
    pub limit: Option<u32>,
}

/// Execution limits for `Session.program`.
#[napi(object)]
#[derive(Clone)]
pub struct ProgramScriptLimits {
    pub timeout_ms: Option<u32>,
    pub max_tool_calls: Option<u32>,
    pub max_output_bytes: Option<u32>,
}

/// Options for `Session.program`.
#[napi(object)]
#[derive(Clone)]
pub struct ProgramScriptOptions {
    pub source: Option<String>,
    pub path: Option<String>,
    pub inputs: Option<serde_json::Value>,
    pub allowed_tools: Option<Vec<String>>,
    pub limits: Option<ProgramScriptLimits>,
}

/// Options for `Session.delegateTask`.
#[napi(object)]
#[derive(Clone)]
pub struct DelegateTaskOptions {
    pub agent: String,
    pub description: String,
    pub prompt: String,
    pub background: Option<bool>,
    pub max_steps: Option<u32>,
}

/// Object-shaped request for `Session.sendRequest` and `Session.streamRequest`.
#[napi(object)]
#[derive(Clone)]
pub struct SessionRequestOptions {
    pub prompt: String,
    pub history: Option<Vec<MessageObject>>,
    pub attachments: Option<Vec<AttachmentObject>>,
}

fn session_request_parts(
    request: Either<String, SessionRequestOptions>,
    history: Option<Vec<MessageObject>>,
) -> napi::Result<(
    String,
    Option<Vec<RustMessage>>,
    Vec<a3s_code_core::llm::Attachment>,
)> {
    match request {
        Either::A(prompt) => {
            let rust_history = history.map(|h| js_messages_to_rust(&h)).transpose()?;
            Ok((prompt, rust_history, Vec::new()))
        }
        Either::B(request) => {
            let rust_history = request
                .history
                .map(|h| js_messages_to_rust(&h))
                .transpose()?;
            let rust_attachments = request
                .attachments
                .as_deref()
                .map(js_attachments_to_rust)
                .unwrap_or_default();
            Ok((request.prompt, rust_history, rust_attachments))
        }
    }
}

async fn send_session_request(
    session: Arc<RustAgentSession>,
    prompt: String,
    history: Option<Vec<RustMessage>>,
    attachments: Vec<a3s_code_core::llm::Attachment>,
) -> napi::Result<AgentResult> {
    let result = if attachments.is_empty() {
        get_runtime()
            .spawn(async move { session.send(&prompt, history.as_deref()).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
    } else {
        get_runtime()
            .spawn(async move {
                session
                    .send_with_attachments(&prompt, &attachments, history.as_deref())
                    .await
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
    }
    .map_err(node_code_error)?;

    Ok(AgentResult::from(result))
}

async fn stream_session_request(
    session: Arc<RustAgentSession>,
    prompt: String,
    history: Option<Vec<RustMessage>>,
    attachments: Vec<a3s_code_core::llm::Attachment>,
) -> napi::Result<EventStream> {
    let (rx, handle) = if attachments.is_empty() {
        get_runtime()
            .spawn(async move { session.stream(&prompt, history.as_deref()).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
    } else {
        get_runtime()
            .spawn(async move {
                session
                    .stream_with_attachments(&prompt, &attachments, history.as_deref())
                    .await
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
    }
    .map_err(node_code_error)?;

    Ok(EventStream {
        rx: Arc::new(tokio::sync::Mutex::new(rx)),
        done: Arc::new(AtomicBool::new(false)),
        lifecycle: Arc::new(tokio::sync::Mutex::new(Some(handle))),
    })
}

fn tool_result_from_core(result: a3s_code_core::ToolCallResult) -> ToolResult {
    ToolResult {
        name: result.name,
        output: result.output,
        exit_code: result.exit_code,
        metadata_json: result.metadata.as_ref().map(serde_json::Value::to_string),
        document_runtime_json: result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("document_runtime"))
            .map(serde_json::Value::to_string),
        error_kind_json: result
            .error_kind
            .as_ref()
            .and_then(|k| serde_json::to_string(k).ok()),
    }
}

fn normalize_program_script_options(options: serde_json::Value) -> napi::Result<serde_json::Value> {
    let obj = options
        .as_object()
        .ok_or_else(|| napi::Error::from_reason("program options must be an object"))?;

    let mut args = serde_json::Map::new();
    args.insert("type".to_string(), serde_json::json!("script"));
    args.insert("language".to_string(), serde_json::json!("javascript"));

    for key in ["source", "path", "inputs", "limits"] {
        if let Some(value) = obj.get(key) {
            args.insert(key.to_string(), value.clone());
        }
    }

    if let Some(value) = obj.get("allowedTools").or_else(|| obj.get("allowed_tools")) {
        args.insert("allowed_tools".to_string(), value.clone());
    }

    Ok(serde_json::Value::Object(args))
}

fn delegate_task_options_to_args(options: DelegateTaskOptions) -> serde_json::Value {
    let mut args = serde_json::json!({
        "agent": options.agent,
        "description": options.description,
        "prompt": options.prompt,
    });
    if let Some(background) = options.background {
        args["background"] = serde_json::json!(background);
    }
    if let Some(max_steps) = options.max_steps {
        args["max_steps"] = serde_json::json!(max_steps);
    }
    args
}

fn parallel_task_options_to_args(tasks: Vec<DelegateTaskOptions>) -> serde_json::Value {
    let task_values = tasks
        .into_iter()
        .map(delegate_task_options_to_args)
        .collect::<Vec<_>>();
    serde_json::json!({ "tasks": task_values })
}

#[napi(object)]
#[derive(Clone)]
pub struct GitCommandOptions {
    pub command: String,
    pub subcommand: Option<String>,
    pub name: Option<String>,
    pub path: Option<String>,
    pub new_branch: Option<bool>,
    pub base: Option<String>,
    pub force: Option<bool>,
    pub max_count: Option<u32>,
    pub message: Option<String>,
    pub include_untracked: Option<bool>,
    pub target: Option<String>,
    pub r#ref: Option<String>,
    pub reference: Option<String>,
}

fn git_command_options_to_args(options: GitCommandOptions) -> serde_json::Value {
    let mut args = serde_json::json!({ "command": options.command });
    if let Some(value) = options.subcommand {
        args["subcommand"] = serde_json::json!(value);
    }
    if let Some(value) = options.name {
        args["name"] = serde_json::json!(value);
    }
    if let Some(value) = options.path {
        args["path"] = serde_json::json!(value);
    }
    if let Some(value) = options.new_branch {
        args["new_branch"] = serde_json::json!(value);
    }
    if let Some(value) = options.base {
        args["base"] = serde_json::json!(value);
    }
    if let Some(value) = options.force {
        args["force"] = serde_json::json!(value);
    }
    if let Some(value) = options.max_count {
        args["max_count"] = serde_json::json!(value);
    }
    if let Some(value) = options.message {
        args["message"] = serde_json::json!(value);
    }
    if let Some(value) = options.include_untracked {
        args["include_untracked"] = serde_json::json!(value);
    }
    if let Some(value) = options.target {
        args["target"] = serde_json::json!(value);
    }
    if let Some(value) = options.r#ref.or(options.reference) {
        args["ref"] = serde_json::json!(value);
    }
    args
}

fn normalize_git_args(mut args: serde_json::Value) -> napi::Result<serde_json::Value> {
    let obj = args
        .as_object_mut()
        .ok_or_else(|| napi::Error::from_reason("git options must be an object"))?;

    if !obj.contains_key("command") {
        return Err(napi::Error::from_reason(
            "git options must include a command field",
        ));
    }

    for (from, to) in [
        ("newBranch", "new_branch"),
        ("maxCount", "max_count"),
        ("includeUntracked", "include_untracked"),
    ] {
        if let Some(value) = obj.remove(from) {
            obj.entry(to.to_string()).or_insert(value);
        }
    }

    if let Some(value) = obj.remove("reference") {
        obj.entry("ref".to_string()).or_insert(value);
    }

    Ok(args)
}

fn timeout_ms_to_secs(timeout_ms: u64) -> u64 {
    timeout_ms.div_ceil(1000).max(1)
}

fn normalize_mcp_server_config(
    mut value: serde_json::Value,
) -> napi::Result<a3s_code_core::mcp::protocol::McpServerConfig> {
    let obj = value
        .as_object_mut()
        .ok_or_else(|| napi::Error::from_reason("MCP server config must be an object"))?;

    for key in [
        "timeoutMs",
        "timeout_ms",
        "toolTimeoutMs",
        "tool_timeout_ms",
    ] {
        if let Some(timeout_ms) = obj.remove(key) {
            let timeout_ms = timeout_ms
                .as_u64()
                .ok_or_else(|| napi::Error::from_reason(format!("{key} must be a number")))?;
            obj.entry("toolTimeoutSecs".to_string())
                .or_insert_with(|| serde_json::json!(timeout_ms_to_secs(timeout_ms)));
            break;
        }
    }

    if let Some(transport) = obj.get_mut("transport") {
        normalize_mcp_transport_alias(transport);
    }

    serde_json::from_value(value)
        .map_err(|e| napi::Error::from_reason(format!("Invalid MCP server config: {e}")))
}

fn normalize_mcp_transport_alias(transport: &mut serde_json::Value) {
    match transport {
        serde_json::Value::String(kind) => {
            if matches!(kind.as_str(), "streamable_http" | "streamableHttp") {
                *kind = "streamable-http".to_string();
            }
        }
        serde_json::Value::Object(obj) => {
            if let Some(serde_json::Value::String(kind)) = obj.get_mut("type") {
                if matches!(kind.as_str(), "streamable_http" | "streamableHttp") {
                    *kind = "streamable-http".to_string();
                }
            }
        }
        _ => {}
    }
}

// ============================================================================
// WebSearchParams
// ============================================================================

/// Parameters for the web_search tool.
#[napi(object)]
#[derive(Clone)]
pub struct JsWebSearchParams {
    /// The search query.
    pub query: String,
    /// List of search engines to use.
    pub engines: Option<Vec<String>>,
    /// Maximum number of results to return (default: 10, max: 50).
    pub limit: Option<u32>,
    /// Search timeout in seconds (default: 10, max: 60).
    pub timeout: Option<u32>,
    /// Proxy URL (e.g., http://127.0.0.1:8080 or socks5://127.0.0.1:1080).
    pub proxy: Option<String>,
    /// Output format: "text" or "json".
    pub format: Option<String>,
}

// ============================================================================
// EventStream
// ============================================================================

/// Result of a single `EventStream.next()` call.
#[napi(object)]
#[derive(Clone)]
pub struct NextResult {
    pub value: Option<AgentEvent>,
    pub done: bool,
}

/// Streaming event iterator. Use `for await (const event of stream)` or call `.next()` manually.
#[napi]
pub struct EventStream {
    rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<RustAgentEvent>>>,
    done: Arc<AtomicBool>,
    lifecycle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

async fn recv_stream_event(
    rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<RustAgentEvent>>>,
    lifecycle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
) -> Option<RustAgentEvent> {
    let event = {
        let mut guard = rx.lock().await;
        guard.recv().await
    };
    let terminal = matches!(
        event,
        Some(RustAgentEvent::End { .. } | RustAgentEvent::Error { .. })
    );
    if terminal || event.is_none() {
        if let Some(handle) = lifecycle.lock().await.take() {
            let _ = handle.await;
        }
    }
    event
}

#[napi]
impl EventStream {
    /// Get the next event from the stream.
    ///
    /// Returns `{ value: AgentEvent | null, done: boolean }`.
    /// When `done` is true, the stream is exhausted.
    #[napi]
    pub async fn next(&self) -> napi::Result<NextResult> {
        if self.done.load(Ordering::Relaxed) {
            return Ok(NextResult {
                value: None,
                done: true,
            });
        }
        let rx = self.rx.clone();
        let done_flag = self.done.clone();
        let lifecycle = self.lifecycle.clone();
        let result = get_runtime()
            .spawn(async move { recv_stream_event(rx, lifecycle).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        match result {
            Some(event) => {
                let is_end = matches!(event, RustAgentEvent::End { .. });
                let is_error = matches!(event, RustAgentEvent::Error { .. });
                let js_event = AgentEvent::try_from(event).map_err(|error| {
                    napi::Error::from_reason(format!("Failed to project agent event: {error}"))
                })?;
                if is_end || is_error {
                    done_flag.store(true, Ordering::Relaxed);
                }
                // Follow the standard iterator contract: a yielded value is
                // never accompanied by `done: true`. Terminal events are
                // observable now; the following call reports exhaustion.
                Ok(NextResult {
                    value: Some(js_event),
                    done: false,
                })
            }
            None => {
                done_flag.store(true, Ordering::Relaxed);
                Ok(NextResult {
                    value: None,
                    done: true,
                })
            }
        }
    }
}

// ============================================================================
// SessionOptions
// ============================================================================

/// An inline skill registered programmatically (no file required).
///
/// Use `kind: "instruction"` for prompt injections or `kind: "persona"` to
/// replace the default role section of the system prompt.
#[napi(object)]
#[derive(Clone, Default)]
pub struct InlineSkill {
    /// Unique skill name (kebab-case recommended, e.g. "type-hints").
    pub name: String,
    /// Skill kind: `"instruction"` or `"persona"`.
    pub kind: String,
    /// Markdown content for the skill.
    pub content: String,
}

fn inline_skill_to_rust(skill: InlineSkill) -> napi::Result<Arc<RustSkill>> {
    let name = skill.name.trim().to_string();
    if name.is_empty() {
        return Err(napi::Error::from_reason("skill name must not be empty"));
    }

    let kind = match skill.kind.trim() {
        "" | "instruction" => RustSkillKind::Instruction,
        "persona" => RustSkillKind::Persona,
        "tool" => RustSkillKind::Tool,
        other => {
            return Err(napi::Error::from_reason(format!(
                "unknown skill kind '{other}'; use 'instruction', 'persona', or 'tool'"
            )))
        }
    };

    Ok(Arc::new(RustSkill {
        name,
        description: String::new(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind,
        content: skill.content,
        tags: Vec::new(),
        version: None,
    }))
}

mod typed_providers;
use typed_providers::*;

mod session_options;
use session_options::*;

mod search_config;

// ============================================================================
// SessionOptions

/// A single message in conversation history.
#[napi(object)]
#[derive(Clone)]
pub struct MessageObject {
    pub role: String,
    pub content: Vec<ContentBlockObject>,
}

/// A content block within a message.
#[napi(object)]
#[derive(Clone)]
pub struct ContentBlockObject {
    #[napi(js_name = "type")]
    pub block_type: String,
    /// Text content (for "text" blocks).
    pub text: Option<String>,
    /// Tool use ID (for "tool_use" blocks).
    pub id: Option<String>,
    /// Tool name (for "tool_use" blocks).
    pub name: Option<String>,
    /// Tool input (for "tool_use" blocks).
    pub input: Option<serde_json::Value>,
    /// Tool use ID reference (for "tool_result" blocks).
    pub tool_use_id: Option<String>,
    /// Tool result content (for "tool_result" blocks).
    pub result_content: Option<String>,
    /// Whether this is an error result (for "tool_result" blocks).
    pub is_error: Option<bool>,
}

/// An image attachment for multi-modal prompts.
#[napi(object)]
#[derive(Clone)]
pub struct AttachmentObject {
    /// Raw image bytes.
    pub data: napi::bindgen_prelude::Buffer,
    /// MIME type (e.g., "image/jpeg", "image/png").
    pub media_type: String,
}

fn verification_reports_from_value(
    reports: serde_json::Value,
) -> napi::Result<Vec<RustVerificationReport>> {
    let reports = match reports {
        serde_json::Value::Array(_) => serde_json::from_value(reports),
        serde_json::Value::Object(_) => {
            serde_json::from_value::<RustVerificationReport>(reports).map(|report| vec![report])
        }
        _ => {
            return Err(napi::Error::from_reason(
                "verification reports must be an array or object",
            ));
        }
    };
    reports.map_err(|e| napi::Error::from_reason(format!("Invalid verification report: {e}")))
}

// ============================================================================
// ServeHandle
// ============================================================================

/// Lifetime handle for a running serve daemon (see {@link Agent.serveAgentDir}).
///
/// The daemon keeps running until `stop()` is called. Dropping the handle does
/// NOT cancel the daemon — call `stop()` explicitly for graceful shutdown.
#[napi]
pub struct ServeHandle {
    cancel: CancellationToken,
}

#[napi]
impl ServeHandle {
    /// Request graceful shutdown of the serve daemon.
    ///
    /// Signals every per-schedule job to stop after its current fire. Idempotent:
    /// calling `stop()` more than once is a no-op. Resolves once the cancellation
    /// has been signalled.
    #[napi]
    pub async fn stop(&self) {
        self.cancel.cancel();
    }

    /// Whether `stop()` has been called on this handle.
    #[napi]
    pub fn is_stopped(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

// ============================================================================
// McpServerStatusEntry
// ============================================================================

#[napi(object)]
#[derive(Clone)]
pub struct McpServerStatusEntry {
    pub name: String,
    pub connected: bool,
    pub tool_count: u32,
    pub error: Option<String>,
}

mod workflow_budget;
pub use workflow_budget::*;

mod session;
use session::*;

mod session_memory;

mod session_tools;

mod session_governance;

mod session_capabilities;

mod agent;

mod event_protocol;
pub use event_protocol::*;

// ============================================================================
// Node-side BudgetGuard wrapper
// ============================================================================

/// FIFO retention caps on the session's in-memory stores. Missing cap fields
/// keep the finite framework default. Set `unbounded: true` to opt into legacy
/// unlimited retention; explicit cap fields then override individual stores.
#[napi(object)]
pub struct RetentionLimitsObject {
    /// Deliberately disable every finite default before applying explicit caps.
    pub unbounded: Option<bool>,
    /// Cap on the number of runs retained in InMemoryRunStore.
    /// When exceeded the oldest run is dropped along with its events.
    pub max_runs_retained: Option<u32>,
    /// Cap on event records retained per run. Oldest events
    /// FIFO-dropped from each run's buffer past this cap. The
    /// snapshot's cumulative `eventCount` is not decremented.
    pub max_events_per_run: Option<u32>,
    /// Cap on events retained in InMemoryTraceSink.
    pub max_trace_events: Option<u32>,
    /// Cap on **terminal** (Completed / Failed / Cancelled) subagent
    /// task snapshots. Running tasks are never evicted.
    pub max_terminal_subagent_tasks: Option<u32>,
}

// ============================================================================
// Slash Command Types
// ============================================================================

/// MCP server metadata exposed to slash command handlers.
#[napi(object)]
#[derive(Clone)]
pub struct CommandMcpServerInfo {
    /// MCP server name.
    pub name: String,
    /// Number of tools currently exposed by the server.
    pub tool_count: u32,
}

/// Context passed to custom slash command handlers.
#[napi(object)]
#[derive(Clone)]
pub struct CommandContext {
    /// Current session ID.
    pub session_id: String,
    /// Current workspace path.
    pub workspace: String,
    /// Current active model identifier.
    pub model: String,
    /// Number of messages in session history.
    pub history_len: u32,
    /// Total tokens used in this session so far.
    pub total_tokens: i64,
    /// Estimated session cost in USD.
    pub total_cost: f64,
    /// Registered tool names (builtin + MCP).
    pub tool_names: Vec<String>,
    /// Connected MCP servers and their tool counts.
    pub mcp_servers: Vec<CommandMcpServerInfo>,
}

/// Metadata about a registered slash command.
#[napi(object)]
#[derive(Clone)]
pub struct CommandInfo {
    /// Command name without the leading `/` (e.g., `"help"`, `"model"`)
    pub name: String,
    /// Short description shown in `/help`
    pub description: String,
    /// Optional usage hint (e.g., `"/model <provider/model>"`)
    pub usage: Option<String>,
}

// ============================================================================
// Hook Types
// ============================================================================

mod hook_bridge;
pub use hook_bridge::*;
use hook_bridge::{metrics_snapshot_to_json, parse_hook_event_type, NodeCallbackHandler};

// ============================================================================
// SkillInfo
// ============================================================================

/// Metadata about a compatibility built-in skill entry.
#[napi(object)]
#[derive(Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// Skill kind: "instruction", "tool", or "agent".
    pub kind: String,
}

/// Return the compatibility built-in skill list.
///
/// A3S Code no longer ships embedded built-in skills, so this returns an empty
/// list unless embedded skills are reintroduced in a future release.
#[napi]
pub fn builtin_skills() -> Vec<SkillInfo> {
    rust_builtin_skills()
        .into_iter()
        .map(|s| SkillInfo {
            name: s.name.clone(),
            description: s.description.clone(),
            kind: match s.kind {
                RustSkillKind::Instruction => "instruction".to_string(),
                RustSkillKind::Persona => "persona".to_string(),
                RustSkillKind::Tool => "tool".to_string(),
            },
        })
        .collect()
}

// ============================================================================
// Conversion Helpers
// ============================================================================

fn js_content_block_to_rust(block: &ContentBlockObject) -> RustContentBlock {
    match block.block_type.as_str() {
        "tool_use" => RustContentBlock::ToolUse {
            id: block.id.clone().unwrap_or_default(),
            name: block.name.clone().unwrap_or_default(),
            input: block.input.clone().unwrap_or(serde_json::Value::Null),
        },
        "tool_result" => RustContentBlock::ToolResult {
            tool_use_id: block.tool_use_id.clone().unwrap_or_default(),
            content: a3s_code_core::llm::ToolResultContentField::Text(
                block.result_content.clone().unwrap_or_default(),
            ),
            is_error: block.is_error,
        },
        _ => RustContentBlock::Text {
            text: block.text.clone().unwrap_or_default(),
        },
    }
}

fn rust_content_block_to_js(block: &RustContentBlock) -> ContentBlockObject {
    match block {
        RustContentBlock::Text { text } => ContentBlockObject {
            block_type: "text".to_string(),
            text: Some(text.clone()),
            id: None,
            name: None,
            input: None,
            tool_use_id: None,
            result_content: None,
            is_error: None,
        },
        RustContentBlock::ToolUse { id, name, input } => ContentBlockObject {
            block_type: "tool_use".to_string(),
            text: None,
            id: Some(id.clone()),
            name: Some(name.clone()),
            input: Some(input.clone()),
            tool_use_id: None,
            result_content: None,
            is_error: None,
        },
        RustContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => ContentBlockObject {
            block_type: "tool_result".to_string(),
            text: None,
            id: None,
            name: None,
            input: None,
            tool_use_id: Some(tool_use_id.clone()),
            result_content: Some(match content {
                a3s_code_core::llm::ToolResultContentField::Text(s) => s.clone(),
                a3s_code_core::llm::ToolResultContentField::Blocks(blocks) => blocks
                    .iter()
                    .filter_map(|b| {
                        if let a3s_code_core::llm::ToolResultContent::Text { text } = b {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            }),
            is_error: *is_error,
        },
        RustContentBlock::Image { .. } => ContentBlockObject {
            block_type: "image".to_string(),
            text: None,
            id: None,
            name: None,
            input: None,
            tool_use_id: None,
            result_content: None,
            is_error: None,
        },
    }
}

/// Convert JS AttachmentObject array to Rust Attachment vec.
fn js_attachments_to_rust(attachments: &[AttachmentObject]) -> Vec<a3s_code_core::llm::Attachment> {
    attachments
        .iter()
        .map(|a| a3s_code_core::llm::Attachment::new(a.data.to_vec(), a.media_type.clone()))
        .collect()
}

fn js_messages_to_rust(messages: &[MessageObject]) -> napi::Result<Vec<RustMessage>> {
    Ok(messages
        .iter()
        .map(|m| RustMessage {
            role: m.role.clone(),
            content: m.content.iter().map(js_content_block_to_rust).collect(),
            reasoning_content: None,
        })
        .collect())
}

fn rust_messages_to_js(messages: &[RustMessage]) -> Vec<MessageObject> {
    messages
        .iter()
        .map(|m| MessageObject {
            role: m.role.clone(),
            content: m.content.iter().map(rust_content_block_to_js).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
