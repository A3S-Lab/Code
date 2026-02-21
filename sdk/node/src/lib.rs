//! A3S Code Node.js Bindings
//!
//! Native Node.js addon via napi-rs that wraps `a3s-code-core`'s Agent API.
//!
//! ## Usage
//!
//! ```javascript
//! const { Agent } = require('@a3s-lab/code');
//!
//! const agent = await Agent.create('agent.hcl');
//! const session = agent.session('/my-project');
//!
//! const result = await session.send('What files handle auth?');
//! console.log(result.text);
//! ```

#[macro_use]
extern crate napi_derive;

use a3s_code_core::agent::{AgentEvent as RustAgentEvent, AgentResult as RustAgentResult};
use a3s_code_core::config::{
    SearchConfig as RustSearchConfig, SearchEngineConfig as RustSearchEngineConfig,
    SearchHealthConfig as RustSearchHealthConfig,
};
use a3s_code_core::hooks::{
    Hook as RustHook, HookConfig as RustHookConfig, HookEventType as RustHookEventType,
    HookMatcher as RustHookMatcher,
};
use a3s_code_core::llm::{ContentBlock as RustContentBlock, Message as RustMessage};
use a3s_code_core::queue::{
    ExternalTaskResult as RustExternalTaskResult, LaneHandlerConfig as RustLaneHandlerConfig,
    SessionLane as RustSessionLane, SessionQueueConfig as RustSessionQueueConfig,
    TaskHandlerMode as RustTaskHandlerMode,
};
use a3s_code_core::{builtin_skills as rust_builtin_skills, SkillKind as RustSkillKind};
use a3s_code_core::{
    Agent as RustAgent, AgentSession as RustAgentSession, SessionOptions as RustSessionOptions,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::runtime::Runtime;

// ============================================================================
// Tokio Runtime
// ============================================================================

fn get_runtime() -> &'static Runtime {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("Failed to create tokio runtime"))
}

// ============================================================================
// AgentResult
// ============================================================================

#[napi(object)]
#[derive(Clone)]
pub struct AgentResult {
    pub text: String,
    pub tool_calls_count: u32,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl From<RustAgentResult> for AgentResult {
    fn from(r: RustAgentResult) -> Self {
        Self {
            text: r.text,
            tool_calls_count: r.tool_calls_count as u32,
            prompt_tokens: r.usage.prompt_tokens as u32,
            completion_tokens: r.usage.completion_tokens as u32,
            total_tokens: r.usage.total_tokens as u32,
        }
    }
}

// ============================================================================
// AgentEvent
// ============================================================================

#[napi(object)]
#[derive(Clone)]
pub struct AgentEvent {
    #[napi(js_name = "type")]
    pub event_type: String,
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_id: Option<String>,
    pub tool_output: Option<String>,
    pub exit_code: Option<i32>,
    pub turn: Option<u32>,
    pub prompt: Option<String>,
    pub error: Option<String>,
    pub total_tokens: Option<u32>,
}

impl AgentEvent {
    fn empty(event_type: &str) -> Self {
        Self {
            event_type: event_type.to_string(),
            text: None,
            tool_name: None,
            tool_id: None,
            tool_output: None,
            exit_code: None,
            turn: None,
            prompt: None,
            error: None,
            total_tokens: None,
        }
    }
}

impl From<RustAgentEvent> for AgentEvent {
    fn from(e: RustAgentEvent) -> Self {
        match e {
            RustAgentEvent::Start { prompt } => Self {
                prompt: Some(prompt),
                ..Self::empty("start")
            },
            RustAgentEvent::TextDelta { text } => Self {
                text: Some(text),
                ..Self::empty("text_delta")
            },
            RustAgentEvent::TurnStart { turn } => Self {
                turn: Some(turn as u32),
                ..Self::empty("turn_start")
            },
            RustAgentEvent::TurnEnd { turn, usage } => Self {
                turn: Some(turn as u32),
                total_tokens: Some(usage.total_tokens as u32),
                ..Self::empty("turn_end")
            },
            RustAgentEvent::ToolStart { id, name } => Self {
                tool_id: Some(id),
                tool_name: Some(name),
                ..Self::empty("tool_start")
            },
            RustAgentEvent::ToolEnd {
                id,
                name,
                output,
                exit_code,
            } => Self {
                tool_id: Some(id),
                tool_name: Some(name),
                tool_output: Some(output),
                exit_code: Some(exit_code),
                ..Self::empty("tool_end")
            },
            RustAgentEvent::ToolOutputDelta { id, name, delta } => Self {
                tool_id: Some(id),
                tool_name: Some(name),
                text: Some(delta),
                ..Self::empty("tool_output_delta")
            },
            RustAgentEvent::End { text, usage } => Self {
                text: Some(text),
                total_tokens: Some(usage.total_tokens as u32),
                ..Self::empty("end")
            },
            RustAgentEvent::Error { message } => Self {
                error: Some(message),
                ..Self::empty("error")
            },
            _ => Self::empty("unknown"),
        }
    }
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
        let result = get_runtime()
            .spawn(async move {
                let mut guard = rx.lock().await;
                guard.recv().await
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        match result {
            Some(event) => {
                let is_end = matches!(event, RustAgentEvent::End { .. });
                let is_error = matches!(event, RustAgentEvent::Error { .. });
                let js_event = AgentEvent::from(event);
                if is_end || is_error {
                    done_flag.store(true, Ordering::Relaxed);
                    Ok(NextResult {
                        value: Some(js_event),
                        done: true,
                    })
                } else {
                    Ok(NextResult {
                        value: Some(js_event),
                        done: false,
                    })
                }
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

#[napi(object)]
#[derive(Clone, Default)]
pub struct SessionOptions {
    /// Override the default model. Format: "provider/model" (e.g., "openai/gpt-4o").
    pub model: Option<String>,
    /// Enable built-in skills (7 skills: code-search, code-review, explain-code, find-bugs, builtin-tools, delegate-task, find-skills).
    pub builtin_skills: Option<bool>,
    /// Extra directories to scan for skill files (.md with YAML frontmatter).
    pub skill_dirs: Option<Vec<String>>,
    /// Extra directories to scan for agent files.
    pub agent_dirs: Option<Vec<String>>,
    /// Optional queue configuration for lane-based tool execution.
    pub queue_config: Option<SessionQueueConfig>,
    /// Allow all tools without HITL confirmation (default: false).
    pub permissive: Option<bool>,
    /// Enable planning mode (default: false).
    pub planning: Option<bool>,
    /// Enable goal tracking (default: false).
    pub goal_tracking: Option<bool>,
    /// Max consecutive parse errors before abort.
    pub max_parse_retries: Option<u32>,
    /// Per-tool execution timeout in milliseconds.
    pub tool_timeout_ms: Option<f64>,
    /// Max LLM API failures before abort.
    pub circuit_breaker_threshold: Option<u32>,
    /// Enable auto-compaction when context window fills up (default: false).
    pub auto_compact: Option<bool>,
    /// Context usage threshold (0.0–1.0) to trigger auto-compaction (default: 0.8).
    pub auto_compact_threshold: Option<f64>,
    /// Directory for persistent file-based memory store.
    pub memory_dir: Option<String>,
    /// Enable default security provider (input taint + output sanitization).
    pub default_security: Option<bool>,
}

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

// ============================================================================
// SessionQueueConfig
// ============================================================================

/// Configuration for the session lane queue.
#[napi(object)]
#[derive(Clone, Default)]
pub struct SessionQueueConfig {
    /// Max concurrency for Query lane (default: 4).
    pub query_concurrency: Option<u32>,
    /// Max concurrency for Execute lane (default: 2).
    pub execute_concurrency: Option<u32>,
    /// Max concurrency for Generate lane (default: 1).
    pub generate_concurrency: Option<u32>,
    /// Enable dead letter queue.
    pub enable_dlq: Option<bool>,
    /// Max DLQ size (default: 1000).
    pub dlq_max_size: Option<u32>,
    /// Enable metrics collection.
    pub enable_metrics: Option<bool>,
    /// Enable queue alerts.
    pub enable_alerts: Option<bool>,
    /// Default command timeout (ms).
    pub timeout_ms: Option<u32>,
    /// Enable all features with sensible defaults.
    pub enable_all_features: Option<bool>,
}

/// Result of an external task completion.
#[napi(object)]
#[derive(Clone)]
pub struct ExternalTaskResult {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Lane handler configuration.
#[napi(object)]
#[derive(Clone)]
pub struct LaneHandlerConfig {
    /// "internal", "external", or "hybrid"
    pub mode: String,
    /// Timeout for external processing (ms).
    pub timeout_ms: Option<u32>,
}

/// Queue statistics.
#[napi(object)]
#[derive(Clone)]
pub struct QueueStats {
    pub total_pending: u32,
    pub total_active: u32,
    pub external_pending: u32,
}

fn js_queue_config_to_rust(config: &SessionQueueConfig) -> RustSessionQueueConfig {
    let mut c = if config.enable_all_features.unwrap_or(false) {
        RustSessionQueueConfig::default().with_lane_features()
    } else {
        RustSessionQueueConfig::default()
    };
    if let Some(n) = config.query_concurrency {
        c.query_max_concurrency = n as usize;
    }
    if let Some(n) = config.execute_concurrency {
        c.execute_max_concurrency = n as usize;
    }
    if let Some(n) = config.generate_concurrency {
        c.generate_max_concurrency = n as usize;
    }
    if let Some(true) = config.enable_dlq {
        c = c.with_dlq(config.dlq_max_size.map(|n| n as usize));
    }
    if let Some(true) = config.enable_metrics {
        c = c.with_metrics();
    }
    if let Some(true) = config.enable_alerts {
        c = c.with_alerts();
    }
    if let Some(ms) = config.timeout_ms {
        c = c.with_timeout(ms as u64);
    }
    c
}

fn parse_lane(lane: &str) -> napi::Result<RustSessionLane> {
    match lane {
        "control" => Ok(RustSessionLane::Control),
        "query" => Ok(RustSessionLane::Query),
        "execute" => Ok(RustSessionLane::Execute),
        "generate" => Ok(RustSessionLane::Generate),
        _ => Err(napi::Error::from_reason(format!(
            "Invalid lane '{}'. Must be: control, query, execute, or generate",
            lane
        ))),
    }
}

fn parse_handler_mode(mode: &str) -> napi::Result<RustTaskHandlerMode> {
    match mode {
        "internal" => Ok(RustTaskHandlerMode::Internal),
        "external" => Ok(RustTaskHandlerMode::External),
        "hybrid" => Ok(RustTaskHandlerMode::Hybrid),
        _ => Err(napi::Error::from_reason(format!(
            "Invalid handler mode '{}'. Must be: internal, external, or hybrid",
            mode
        ))),
    }
}

/// Build RustSessionOptions from JS SessionOptions.
fn js_session_options_to_rust(options: Option<SessionOptions>) -> RustSessionOptions {
    let Some(o) = options else {
        return RustSessionOptions::new();
    };
    let mut opts = RustSessionOptions::new();
    if let Some(model) = o.model {
        opts = opts.with_model(model);
    }
    if o.builtin_skills.unwrap_or(false) {
        opts = opts.with_builtin_skills();
    }
    if let Some(dirs) = o.skill_dirs {
        for d in dirs {
            opts = opts.with_skills_from_dir(d);
        }
    }
    if let Some(dirs) = o.agent_dirs {
        for d in dirs {
            opts = opts.with_agent_dir(d);
        }
    }
    if let Some(qc) = o.queue_config {
        opts = opts.with_queue_config(js_queue_config_to_rust(&qc));
    }
    if o.permissive.unwrap_or(false) {
        opts = opts.with_permissive_policy();
    }
    if o.planning.unwrap_or(false) {
        opts = opts.with_planning(true);
    }
    if o.goal_tracking.unwrap_or(false) {
        opts = opts.with_goal_tracking(true);
    }
    if let Some(n) = o.max_parse_retries {
        opts = opts.with_parse_retries(n);
    }
    if let Some(ms) = o.tool_timeout_ms {
        opts = opts.with_tool_timeout(ms as u64);
    }
    if let Some(n) = o.circuit_breaker_threshold {
        opts = opts.with_circuit_breaker(n);
    }
    if o.auto_compact.unwrap_or(false) {
        opts = opts.with_auto_compact(true);
    }
    if let Some(t) = o.auto_compact_threshold {
        opts = opts.with_auto_compact_threshold(t as f32);
    }
    if let Some(dir) = o.memory_dir {
        opts = opts.with_file_memory(dir);
    }
    if o.default_security.unwrap_or(false) {
        opts = opts.with_default_security();
    }
    opts
}

// ============================================================================
// Agent
// ============================================================================

/// AI coding agent. Create with `Agent.create()`, then call `agent.session()`.
#[napi]
pub struct Agent {
    inner: Arc<RustAgent>,
}

#[napi]
impl Agent {
    /// Create an Agent from a config file path or inline config string.
    ///
    /// @param configSource - Path to .hcl/.json file, or inline JSON/HCL string
    #[napi(factory)]
    pub async fn create(config_source: String) -> napi::Result<Self> {
        let agent = get_runtime()
            .spawn(async move { RustAgent::new(config_source).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Failed to create agent: {e}")))?;

        Ok(Self {
            inner: Arc::new(agent),
        })
    }

    /// Bind to a workspace directory, returning a Session.
    ///
    /// @param workspace - Path to the workspace directory
    /// @param options - Optional session overrides (model, builtinSkills, skillDirs, agentDirs, queueConfig)
    #[napi]
    pub fn session(
        &self,
        workspace: String,
        options: Option<SessionOptions>,
    ) -> napi::Result<Session> {
        let rust_opts = options.map(|o| {
            let mut opts = RustSessionOptions::new();
            if let Some(model) = o.model {
                opts = opts.with_model(model);
            }
            if o.builtin_skills.unwrap_or(false) {
                opts = opts.with_builtin_skills();
            }
            if let Some(dirs) = o.skill_dirs {
                for d in dirs {
                    opts = opts.with_skills_from_dir(d);
                }
            }
            if let Some(dirs) = o.agent_dirs {
                for d in dirs {
                    opts = opts.with_agent_dir(d);
                }
            }
            if let Some(qc) = o.queue_config {
                opts = opts.with_queue_config(js_queue_config_to_rust(&qc));
            }
            if o.permissive.unwrap_or(false) {
                opts = opts.with_permissive_policy();
            }
            if o.planning.unwrap_or(false) {
                opts = opts.with_planning(true);
            }
            if o.goal_tracking.unwrap_or(false) {
                opts = opts.with_goal_tracking(true);
            }
            if let Some(n) = o.max_parse_retries {
                opts = opts.with_parse_retries(n);
            }
            if let Some(ms) = o.tool_timeout_ms {
                opts = opts.with_tool_timeout(ms as u64);
            }
            if let Some(n) = o.circuit_breaker_threshold {
                opts = opts.with_circuit_breaker(n);
            }
            if o.auto_compact.unwrap_or(false) {
                opts = opts.with_auto_compact(true);
            }
            if let Some(t) = o.auto_compact_threshold {
                opts = opts.with_auto_compact_threshold(t as f32);
            }
            if let Some(dir) = o.memory_dir {
                opts = opts.with_file_memory(dir);
            }
            if o.default_security.unwrap_or(false) {
                opts = opts.with_default_security();
            }
            opts
        });

        let session = self
            .inner
            .session(workspace, rust_opts)
            .map_err(|e| napi::Error::from_reason(format!("{e}")))?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }

    /// Resume a previously saved session by ID.
    ///
    /// @param sessionId - The session ID to resume
    /// @param sessionStoreDir - Directory where sessions are stored
    /// @param options - Optional session overrides
    #[napi]
    pub fn resume_session(
        &self,
        session_id: String,
        session_store_dir: String,
        options: Option<SessionOptions>,
    ) -> napi::Result<Session> {
        let mut opts = js_session_options_to_rust(options);
        opts = opts.with_file_session_store(session_store_dir);

        let session = self
            .inner
            .resume_session(&session_id, opts)
            .map_err(|e| napi::Error::from_reason(format!("Failed to resume session: {e}")))?;
        Ok(Session {
            inner: Arc::new(session),
        })
    }
}

// ============================================================================
// Session
// ============================================================================

/// Workspace-bound session. All LLM and tool operations happen here.
#[napi]
pub struct Session {
    inner: Arc<RustAgentSession>,
}

#[napi]
impl Session {
    /// Send a prompt and wait for the complete response.
    ///
    /// @param prompt - The prompt to send
    /// @param history - Optional conversation history
    #[napi]
    pub async fn send(
        &self,
        prompt: String,
        history: Option<Vec<MessageObject>>,
    ) -> napi::Result<AgentResult> {
        let rust_history = history.map(|h| js_messages_to_rust(&h)).transpose()?;
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.send(&prompt, rust_history.as_deref()).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Agent execution failed: {e}")))?;
        Ok(AgentResult::from(result))
    }

    /// Send a prompt and get a streaming event iterator.
    ///
    /// Returns an `EventStream`. Use `for await (const event of stream)` or call `.next()` manually.
    ///
    /// @param prompt - The prompt to send
    /// @param history - Optional conversation history
    #[napi]
    pub async fn stream(
        &self,
        prompt: String,
        history: Option<Vec<MessageObject>>,
    ) -> napi::Result<EventStream> {
        let rust_history = history.map(|h| js_messages_to_rust(&h)).transpose()?;
        let session = self.inner.clone();
        let (rx, _handle) = get_runtime()
            .spawn(async move { session.stream(&prompt, rust_history.as_deref()).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Failed to start stream: {e}")))?;

        Ok(EventStream {
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
            done: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Send a prompt with image attachments and wait for the complete response.
    ///
    /// @param prompt - The prompt to send
    /// @param attachments - Array of `{ data: Buffer, mediaType: string }`
    /// @param history - Optional conversation history
    #[napi]
    pub async fn send_with_attachments(
        &self,
        prompt: String,
        attachments: Vec<AttachmentObject>,
        history: Option<Vec<MessageObject>>,
    ) -> napi::Result<AgentResult> {
        let rust_attachments = js_attachments_to_rust(&attachments);
        let rust_history = history.map(|h| js_messages_to_rust(&h)).transpose()?;
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move {
                session
                    .send_with_attachments(&prompt, &rust_attachments, rust_history.as_deref())
                    .await
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Agent execution failed: {e}")))?;
        Ok(AgentResult::from(result))
    }

    /// Stream a prompt with image attachments.
    ///
    /// @param prompt - The prompt to send
    /// @param attachments - Array of `{ data: Buffer, mediaType: string }`
    /// @param history - Optional conversation history
    #[napi]
    pub async fn stream_with_attachments(
        &self,
        prompt: String,
        attachments: Vec<AttachmentObject>,
        history: Option<Vec<MessageObject>>,
    ) -> napi::Result<EventStream> {
        let rust_attachments = js_attachments_to_rust(&attachments);
        let rust_history = history.map(|h| js_messages_to_rust(&h)).transpose()?;
        let session = self.inner.clone();
        let (rx, _handle) = get_runtime()
            .spawn(async move {
                session
                    .stream_with_attachments(&prompt, &rust_attachments, rust_history.as_deref())
                    .await
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Failed to start stream: {e}")))?;
        Ok(EventStream {
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
            done: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Return the session's conversation history.
    #[napi]
    pub fn history(&self) -> Vec<MessageObject> {
        rust_messages_to_js(&self.inner.history())
    }

    /// Execute a tool by name, bypassing the LLM.
    #[napi]
    pub async fn tool(&self, name: String, args: serde_json::Value) -> napi::Result<ToolResult> {
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.tool(&name, args).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Tool execution failed: {e}")))?;
        Ok(ToolResult {
            name: result.name,
            output: result.output,
            exit_code: result.exit_code,
        })
    }

    /// Read a file from the workspace.
    #[napi]
    pub async fn read_file(&self, path: String) -> napi::Result<String> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.read_file(&path).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Execute a bash command in the workspace.
    #[napi]
    pub async fn bash(&self, command: String) -> napi::Result<String> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.bash(&command).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Search for files matching a glob pattern.
    #[napi]
    pub async fn glob(&self, pattern: String) -> napi::Result<Vec<String>> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.glob(&pattern).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Search file contents with a regex pattern.
    #[napi]
    pub async fn grep(&self, pattern: String) -> napi::Result<String> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.grep(&pattern).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    // ========================================================================
    // Queue API
    // ========================================================================

    /// Check if this session has a lane queue configured.
    #[napi]
    pub fn has_queue(&self) -> bool {
        self.inner.has_queue()
    }

    /// Configure a lane's handler mode.
    ///
    /// @param lane - "control", "query", "execute", or "generate"
    /// @param config - { mode: "internal"|"external"|"hybrid", timeoutMs?: number }
    #[napi]
    pub async fn set_lane_handler(
        &self,
        lane: String,
        config: LaneHandlerConfig,
    ) -> napi::Result<()> {
        let rust_lane = parse_lane(&lane)?;
        let rust_mode = parse_handler_mode(&config.mode)?;
        let rust_config = RustLaneHandlerConfig {
            mode: rust_mode,
            timeout_ms: config.timeout_ms.unwrap_or(60000) as u64,
        };
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.set_lane_handler(rust_lane, rust_config).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        Ok(())
    }

    /// Complete an external task by ID.
    ///
    /// @param taskId - The task identifier
    /// @param result - { success: boolean, result?: any, error?: string }
    /// @returns true if found, false if not found
    #[napi]
    pub async fn complete_external_task(
        &self,
        task_id: String,
        result: ExternalTaskResult,
    ) -> napi::Result<bool> {
        let ext_result = RustExternalTaskResult {
            success: result.success,
            result: result.result.unwrap_or(serde_json::json!({})),
            error: result.error,
        };
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.complete_external_task(&task_id, ext_result).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))
    }

    /// Get pending external tasks.
    #[napi]
    pub async fn pending_external_tasks(&self) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let tasks = get_runtime()
            .spawn(async move { session.pending_external_tasks().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(&tasks)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Get queue statistics.
    #[napi]
    pub async fn queue_stats(&self) -> napi::Result<QueueStats> {
        let session = self.inner.clone();
        let stats = get_runtime()
            .spawn(async move { session.queue_stats().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        Ok(QueueStats {
            total_pending: stats.total_pending as u32,
            total_active: stats.total_active as u32,
            external_pending: stats.external_pending as u32,
        })
    }

    /// Get dead letters from the DLQ.
    #[napi]
    pub async fn dead_letters(&self) -> napi::Result<serde_json::Value> {
        let session = self.inner.clone();
        let letters = get_runtime()
            .spawn(async move { session.dead_letters().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;
        serde_json::to_value(&letters)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    // ========================================================================
    // Hook API
    // ========================================================================

    /// Register a hook for lifecycle event interception.
    ///
    /// @param hookId - Unique hook identifier
    /// @param eventType - Event type: "pre_tool_use", "post_tool_use", "generate_start",
    ///   "generate_end", "session_start", "session_end", "skill_load", "skill_unload",
    ///   "pre_prompt", "post_response", "on_error"
    /// @param matcher - Optional matcher: { tool?, pathPattern?, commandPattern?, sessionId?, skill? }
    /// @param config - Optional config: { priority?, timeoutMs?, asyncExecution?, maxRetries? }
    #[napi]
    pub fn register_hook(
        &self,
        hook_id: String,
        event_type: String,
        matcher: Option<HookMatcherObject>,
        config: Option<HookConfigObject>,
    ) -> napi::Result<()> {
        let rust_event_type = parse_hook_event_type(&event_type)?;
        let mut hook = RustHook::new(&hook_id, rust_event_type);

        if let Some(m) = matcher {
            let mut rust_matcher = RustHookMatcher::new();
            if let Some(tool) = m.tool {
                rust_matcher = rust_matcher.with_tool(tool);
            }
            if let Some(path) = m.path_pattern {
                rust_matcher = rust_matcher.with_path(path);
            }
            if let Some(cmd) = m.command_pattern {
                rust_matcher = rust_matcher.with_command(cmd);
            }
            if let Some(sid) = m.session_id {
                rust_matcher = rust_matcher.with_session(sid);
            }
            if let Some(skill) = m.skill {
                rust_matcher = rust_matcher.with_skill(skill);
            }
            hook = hook.with_matcher(rust_matcher);
        }

        if let Some(c) = config {
            hook = hook.with_config(RustHookConfig {
                priority: c.priority.unwrap_or(100),
                timeout_ms: c.timeout_ms.map(|v| v as u64).unwrap_or(30000),
                async_execution: c.async_execution.unwrap_or(false),
                max_retries: c.max_retries.unwrap_or(0),
            });
        }

        self.inner.register_hook(hook);
        Ok(())
    }

    /// Unregister a hook by ID.
    ///
    /// @param hookId - The hook identifier to remove
    /// @returns true if the hook was found and removed
    #[napi]
    pub fn unregister_hook(&self, hook_id: String) -> bool {
        self.inner.unregister_hook(&hook_id).is_some()
    }

    /// Get the number of registered hooks.
    #[napi]
    pub fn hook_count(&self) -> u32 {
        self.inner.hook_count() as u32
    }

    // ========================================================================
    // Session Metadata API
    // ========================================================================

    /// Return the session ID.
    #[napi(getter)]
    pub fn session_id(&self) -> String {
        self.inner.session_id().to_string()
    }

    /// Return the workspace path.
    #[napi(getter)]
    pub fn workspace(&self) -> String {
        self.inner.workspace().display().to_string()
    }

    /// Return any deferred init warning (e.g. memory store failed to initialize).
    #[napi(getter)]
    pub fn init_warning(&self) -> Option<String> {
        self.inner.init_warning().map(|s| s.to_string())
    }

    // ========================================================================
    // Session Persistence API
    // ========================================================================

    /// Save the session to the configured store.
    #[napi]
    pub async fn save(&self) -> napi::Result<()> {
        let session = self.inner.clone();
        get_runtime()
            .spawn(async move { session.save().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Save failed: {e}")))
    }

    // ========================================================================
    // Memory API
    // ========================================================================

    /// Check if memory is configured for this session.
    #[napi(getter)]
    pub fn has_memory(&self) -> bool {
        self.inner.memory().is_some()
    }

    /// Remember a successful task execution.
    ///
    /// @param task - Description of the task
    /// @param tools - List of tool names used
    /// @param result - Summary of the result
    #[napi]
    pub async fn remember_success(
        &self,
        task: String,
        tools: Vec<String>,
        result: String,
    ) -> napi::Result<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason("Memory not configured for this session"))?
            .clone();
        get_runtime()
            .spawn(async move { memory.remember_success(&task, &tools, &result).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Remember failed: {e}")))
    }

    /// Remember a failed task execution.
    ///
    /// @param task - Description of the task
    /// @param error - Error message
    /// @param tools - List of tool names attempted
    #[napi]
    pub async fn remember_failure(
        &self,
        task: String,
        error: String,
        tools: Vec<String>,
    ) -> napi::Result<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason("Memory not configured for this session"))?
            .clone();
        get_runtime()
            .spawn(async move { memory.remember_failure(&task, &error, &tools).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Remember failed: {e}")))
    }

    /// Recall memories similar to a query.
    ///
    /// @param query - Search query
    /// @param limit - Maximum number of results (default: 5)
    /// @returns Array of memory items
    #[napi]
    pub async fn recall_similar(
        &self,
        query: String,
        limit: Option<u32>,
    ) -> napi::Result<serde_json::Value> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason("Memory not configured for this session"))?
            .clone();
        let limit = limit.unwrap_or(5) as usize;
        let items = get_runtime()
            .spawn(async move { memory.recall_similar(&query, limit).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Recall failed: {e}")))?;
        serde_json::to_value(&items)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Recall memories by tags.
    ///
    /// @param tags - Tags to search for
    /// @param limit - Maximum number of results (default: 10)
    /// @returns Array of memory items
    #[napi]
    pub async fn recall_by_tags(
        &self,
        tags: Vec<String>,
        limit: Option<u32>,
    ) -> napi::Result<serde_json::Value> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason("Memory not configured for this session"))?
            .clone();
        let limit = limit.unwrap_or(10) as usize;
        let items = get_runtime()
            .spawn(async move { memory.recall_by_tags(&tags, limit).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Recall failed: {e}")))?;
        serde_json::to_value(&items)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Get recent memory items.
    ///
    /// @param limit - Maximum number of results (default: 10)
    /// @returns Array of memory items
    #[napi]
    pub async fn memory_recent(&self, limit: Option<u32>) -> napi::Result<serde_json::Value> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason("Memory not configured for this session"))?
            .clone();
        let limit = limit.unwrap_or(10) as usize;
        let items = get_runtime()
            .spawn(async move { memory.get_recent(limit).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Recall failed: {e}")))?;
        serde_json::to_value(&items)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Get memory statistics.
    ///
    /// @returns Object with longTermCount, shortTermCount, workingCount
    #[napi]
    pub async fn memory_stats(&self) -> napi::Result<serde_json::Value> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| napi::Error::from_reason("Memory not configured for this session"))?
            .clone();
        let stats = get_runtime()
            .spawn(async move { memory.stats().await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Stats failed: {e}")))?;
        serde_json::to_value(&stats)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }
}

// ============================================================================
// Hook Types
// ============================================================================

/// Matcher for filtering which events trigger a hook.
#[napi(object)]
#[derive(Clone)]
pub struct HookMatcherObject {
    /// Match specific tool name (exact match)
    pub tool: Option<String>,
    /// Match file path pattern (glob)
    pub path_pattern: Option<String>,
    /// Match command pattern (regex for Bash commands)
    pub command_pattern: Option<String>,
    /// Match session ID (exact match)
    pub session_id: Option<String>,
    /// Match skill name (supports glob patterns)
    pub skill: Option<String>,
}

/// Configuration for a hook.
#[napi(object)]
#[derive(Clone)]
pub struct HookConfigObject {
    /// Priority (lower values = higher priority, default: 100)
    pub priority: Option<i32>,
    /// Timeout in milliseconds (default: 30000)
    pub timeout_ms: Option<i64>,
    /// Whether to execute asynchronously (fire-and-forget)
    pub async_execution: Option<bool>,
    /// Maximum retry attempts
    pub max_retries: Option<u32>,
}

fn parse_hook_event_type(event_type: &str) -> napi::Result<RustHookEventType> {
    match event_type {
        "pre_tool_use" => Ok(RustHookEventType::PreToolUse),
        "post_tool_use" => Ok(RustHookEventType::PostToolUse),
        "generate_start" => Ok(RustHookEventType::GenerateStart),
        "generate_end" => Ok(RustHookEventType::GenerateEnd),
        "session_start" => Ok(RustHookEventType::SessionStart),
        "session_end" => Ok(RustHookEventType::SessionEnd),
        "skill_load" => Ok(RustHookEventType::SkillLoad),
        "skill_unload" => Ok(RustHookEventType::SkillUnload),
        "pre_prompt" => Ok(RustHookEventType::PrePrompt),
        "post_response" => Ok(RustHookEventType::PostResponse),
        "on_error" => Ok(RustHookEventType::OnError),
        _ => Err(napi::Error::from_reason(format!(
            "Invalid hook event type: '{}'. Expected one of: pre_tool_use, post_tool_use, \
             generate_start, generate_end, session_start, session_end, skill_load, \
             skill_unload, pre_prompt, post_response, on_error",
            event_type
        ))),
    }
}

// ============================================================================
// SkillInfo
// ============================================================================

/// Metadata about a built-in skill.
#[napi(object)]
#[derive(Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// Skill kind: "instruction", "tool", or "agent".
    pub kind: String,
}

/// Return a list of built-in skills compiled into the library.
///
/// Each entry has `name`, `description`, and `kind` (instruction, tool, or agent).
#[napi]
pub fn builtin_skills() -> Vec<SkillInfo> {
    rust_builtin_skills()
        .into_iter()
        .map(|s| SkillInfo {
            name: s.name.clone(),
            description: s.description.clone(),
            kind: match s.kind {
                RustSkillKind::Instruction => "instruction".to_string(),
                RustSkillKind::Tool => "tool".to_string(),
                RustSkillKind::Agent => "agent".to_string(),
                RustSkillKind::Persona => "persona".to_string(),
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
        .map(|a| {
            a3s_code_core::llm::Attachment::new(a.data.to_vec(), a.media_type.clone())
        })
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

// ============================================================================
// SearchConfig
// ============================================================================

/// Configuration for a search engine.
#[napi(object)]
#[derive(Clone)]
pub struct SearchEngineConfig {
    pub enabled: bool,
    pub weight: f64,
    pub timeout: Option<u32>,
}

impl From<SearchEngineConfig> for RustSearchEngineConfig {
    fn from(c: SearchEngineConfig) -> Self {
        Self {
            enabled: c.enabled,
            weight: c.weight,
            timeout: c.timeout.map(|t| t as u64),
        }
    }
}

/// Health monitor configuration for search engines.
#[napi(object)]
#[derive(Clone)]
pub struct SearchHealthConfig {
    pub max_failures: u32,
    pub suspend_seconds: u32,
}

impl From<SearchHealthConfig> for RustSearchHealthConfig {
    fn from(c: SearchHealthConfig) -> Self {
        Self {
            max_failures: c.max_failures,
            suspend_seconds: c.suspend_seconds as u64,
        }
    }
}

/// Search engine configuration (a3s-search integration).
#[napi(object)]
#[derive(Clone)]
pub struct SearchConfig {
    pub timeout: u32,
    pub health: Option<SearchHealthConfig>,
    pub engines: std::collections::HashMap<String, SearchEngineConfig>,
}

impl From<SearchConfig> for RustSearchConfig {
    fn from(c: SearchConfig) -> Self {
        Self {
            timeout: c.timeout as u64,
            health: c.health.map(|h| h.into()),
            engines: c.engines.into_iter().map(|(k, v)| (k, v.into())).collect(),
        }
    }
}
