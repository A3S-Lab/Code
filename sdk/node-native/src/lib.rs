//! A3S Code Node.js Bindings
//!
//! Native Node.js addon via napi-rs that wraps `a3s-code-core`'s Agent API.
//!
//! ## Usage
//!
//! ```javascript
//! const { Agent } = require('@a3s-lab/code');
//!
//! const agent = new Agent({
//!   model: 'claude-sonnet-4-20250514',
//!   apiKey: 'sk-ant-...',
//!   workspace: '/my-project',
//! });
//!
//! // Non-streaming
//! const result = await agent.send('What files handle auth?');
//! console.log(result.text);
//!
//! // Streaming
//! const stream = await agent.stream('Refactor auth');
//! for await (const event of stream) {
//!   if (event.type === 'text_delta') process.stdout.write(event.text);
//! }
//!
//! // Direct tool calls
//! const content = await agent.readFile('src/main.rs');
//! const files = await agent.glob('**/*.rs');
//! const output = await agent.bash('cargo test');
//! ```

#[macro_use]
extern crate napi_derive;

use a3s_code_core::agent::{AgentEvent as RustAgentEvent, AgentResult as RustAgentResult};
use a3s_code_core::Agent as RustAgent;
use std::sync::Arc;
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
// AgentOptions
// ============================================================================

/// Options for creating an Agent.
#[napi(object)]
#[derive(Clone)]
pub struct AgentOptions {
    /// LLM model identifier (e.g., "claude-sonnet-4-20250514", "gpt-4o")
    pub model: String,
    /// API key for the LLM provider
    pub api_key: String,
    /// Path to the workspace directory (sandbox root)
    pub workspace: Option<String>,
    /// System prompt for the agent
    pub system_prompt: Option<String>,
    /// Base URL override for the LLM API
    pub base_url: Option<String>,
    /// Maximum tool execution rounds per turn
    pub max_tool_rounds: Option<u32>,
}

// ============================================================================
// AgentResult
// ============================================================================

/// Result of a non-streaming agent execution.
#[napi(object)]
#[derive(Clone)]
pub struct AgentResult {
    /// The final text response from the agent.
    pub text: String,
    /// Number of tool calls made during execution.
    pub tool_calls_count: u32,
    /// Total prompt tokens used.
    pub prompt_tokens: u32,
    /// Total completion tokens used.
    pub completion_tokens: u32,
    /// Total tokens used.
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

/// A single event from the agent's streaming output.
#[napi(object)]
#[derive(Clone)]
pub struct AgentEvent {
    /// Event type: "start", "text_delta", "tool_start", "tool_end",
    /// "turn_start", "turn_end", "end", "error", etc.
    #[napi(js_name = "type")]
    pub event_type: String,
    /// Text content (for text_delta and end events).
    pub text: Option<String>,
    /// Tool name (for tool_start and tool_end events).
    pub tool_name: Option<String>,
    /// Tool ID (for tool_start and tool_end events).
    pub tool_id: Option<String>,
    /// Tool output (for tool_end events).
    pub tool_output: Option<String>,
    /// Exit code (for tool_end events).
    pub exit_code: Option<i32>,
    /// Turn number (for turn_start and turn_end events).
    pub turn: Option<u32>,
    /// Prompt text (for start events).
    pub prompt: Option<String>,
    /// Error message (for error events).
    pub error: Option<String>,
    /// Token usage (for turn_end and end events).
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
            RustAgentEvent::End { text, usage } => Self {
                text: Some(text),
                total_tokens: Some(usage.total_tokens as u32),
                ..Self::empty("end")
            },
            RustAgentEvent::Error { message } => Self {
                error: Some(message),
                ..Self::empty("error")
            },
            RustAgentEvent::ConfirmationRequired { .. } => Self::empty("confirmation_required"),
            RustAgentEvent::ConfirmationReceived { .. } => Self::empty("confirmation_received"),
            RustAgentEvent::ConfirmationTimeout { .. } => Self::empty("confirmation_timeout"),
            RustAgentEvent::ExternalTaskPending { .. } => Self::empty("external_task_pending"),
            RustAgentEvent::ExternalTaskCompleted { .. } => Self::empty("external_task_completed"),
            RustAgentEvent::PermissionDenied { .. } => Self::empty("permission_denied"),
            RustAgentEvent::ContextResolving { .. } => Self::empty("context_resolving"),
            RustAgentEvent::ContextResolved { .. } => Self::empty("context_resolved"),
            _ => Self::empty("unknown"),
        }
    }
}

// ============================================================================
// ToolResult
// ============================================================================

/// Result of a direct tool execution.
#[napi(object)]
#[derive(Clone)]
pub struct ToolResult {
    /// Tool name.
    pub name: String,
    /// Tool output text.
    pub output: String,
    /// Exit code (0 = success).
    pub exit_code: i32,
}

// ============================================================================
// Agent
// ============================================================================

/// AI coding agent with tool execution capabilities.
///
/// Create an Agent by providing a model name, API key, and workspace path.
/// The agent auto-detects whether to use the Anthropic or OpenAI API based
/// on the model name.
#[napi]
pub struct Agent {
    inner: Arc<RustAgent>,
}

#[napi]
impl Agent {
    /// Create a new Agent instance.
    ///
    /// @param options - Configuration options for the agent
    #[napi(constructor)]
    pub fn new(options: AgentOptions) -> napi::Result<Self> {
        let mut builder = RustAgent::builder()
            .model(&options.model)
            .api_key(&options.api_key);

        if let Some(ref ws) = options.workspace {
            builder = builder.workspace(ws);
        }
        if let Some(ref sp) = options.system_prompt {
            builder = builder.system_prompt(sp);
        }
        if let Some(ref url) = options.base_url {
            builder = builder.base_url(url);
        }
        if let Some(max) = options.max_tool_rounds {
            builder = builder.max_tool_rounds(max as usize);
        }

        let agent = get_runtime()
            .block_on(builder.build())
            .map_err(|e| napi::Error::from_reason(format!("Failed to build agent: {e}")))?;

        Ok(Self {
            inner: Arc::new(agent),
        })
    }

    /// Send a prompt and wait for the complete response.
    ///
    /// @param prompt - The user prompt to send to the agent
    /// @returns AgentResult with text, token usage, and tool call count
    #[napi]
    pub async fn send(&self, prompt: String) -> napi::Result<AgentResult> {
        let agent = self.inner.clone();

        let result = get_runtime()
            .spawn(async move { agent.send(&prompt).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Agent execution failed: {e}")))?;

        Ok(AgentResult::from(result))
    }

    /// Send a prompt and get a stream of events.
    ///
    /// Returns an array of all events. For true streaming, use `stream()` with
    /// an async iterator pattern.
    ///
    /// @param prompt - The user prompt to send to the agent
    /// @returns Array of AgentEvent objects
    #[napi]
    pub async fn stream(&self, prompt: String) -> napi::Result<Vec<AgentEvent>> {
        let agent = self.inner.clone();

        let (rx, _handle) = get_runtime()
            .spawn(async move { agent.stream(&prompt).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Failed to start stream: {e}")))?;

        let rx_arc: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<RustAgentEvent>>> =
            Arc::new(tokio::sync::Mutex::new(rx));

        let collected = get_runtime()
            .spawn(async move {
                let mut guard = rx_arc.lock().await;
                let mut events: Vec<AgentEvent> = Vec::new();
                while let Some(event) = guard.recv().await {
                    let is_end = matches!(event, RustAgentEvent::End { .. });
                    let is_error = matches!(event, RustAgentEvent::Error { .. });
                    events.push(AgentEvent::from(event));
                    if is_end || is_error {
                        break;
                    }
                }
                events
            })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?;

        Ok(collected)
    }

    /// Execute a tool by name, bypassing the LLM.
    ///
    /// @param name - Tool name (e.g., "read", "bash", "glob", "grep")
    /// @param args - Tool arguments as a JSON-compatible object
    /// @returns ToolResult with output and exit_code
    #[napi]
    pub async fn tool(
        &self,
        name: String,
        args: serde_json::Value,
    ) -> napi::Result<ToolResult> {
        let agent = self.inner.clone();

        let result = get_runtime()
            .spawn(async move { agent.tool(&name, args).await })
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
    ///
    /// @param path - File path relative to workspace root
    /// @returns File contents as a string
    #[napi]
    pub async fn read_file(&self, path: String) -> napi::Result<String> {
        let agent = self.inner.clone();

        get_runtime()
            .spawn(async move { agent.read_file(&path).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Execute a bash command in the workspace.
    ///
    /// @param command - Shell command to execute
    /// @returns Command stdout as a string
    #[napi]
    pub async fn bash(&self, command: String) -> napi::Result<String> {
        let agent = self.inner.clone();

        get_runtime()
            .spawn(async move { agent.bash(&command).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Search for files matching a glob pattern.
    ///
    /// @param pattern - Glob pattern (e.g., "**\/*.rs", "src\/*.py")
    /// @returns Array of matching file paths
    #[napi]
    pub async fn glob(&self, pattern: String) -> napi::Result<Vec<String>> {
        let agent = self.inner.clone();

        get_runtime()
            .spawn(async move { agent.glob(&pattern).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("{e}")))
    }

    /// Search file contents with a regex pattern.
    ///
    /// @param pattern - Regex pattern to search for
    /// @returns Matching lines as a string
    #[napi]
    pub async fn grep(&self, pattern: String) -> napi::Result<String> {
        let agent = self.inner.clone();

        get_runtime()
            .spawn(async move { agent.grep(&pattern).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("{e}")))
    }
}
