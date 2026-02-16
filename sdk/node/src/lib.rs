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
use a3s_code_core::{
    Agent as RustAgent, AgentSession as RustAgentSession,
    SessionOptions as RustSessionOptions,
};
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
// SessionOptions
// ============================================================================

#[napi(object)]
#[derive(Clone, Default)]
pub struct SessionOptions {
    /// Override the default model. Format: "provider/model" (e.g., "openai/gpt-4o").
    pub model: Option<String>,
    /// Additional skill directories for this session.
    pub skill_dirs: Option<Vec<String>>,
    /// Additional agent directories for this session.
    pub agent_dirs: Option<Vec<String>>,
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
    /// @param options - Optional session overrides (model, skillDirs, agentDirs)
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
            if let Some(dirs) = o.skill_dirs {
                for dir in dirs {
                    opts = opts.with_skill_dir(dir);
                }
            }
            if let Some(dirs) = o.agent_dirs {
                for dir in dirs {
                    opts = opts.with_agent_dir(dir);
                }
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
    #[napi]
    pub async fn send(&self, prompt: String) -> napi::Result<AgentResult> {
        let session = self.inner.clone();
        let result = get_runtime()
            .spawn(async move { session.send(&prompt).await })
            .await
            .map_err(|e| napi::Error::from_reason(format!("Task join error: {e}")))?
            .map_err(|e| napi::Error::from_reason(format!("Agent execution failed: {e}")))?;
        Ok(AgentResult::from(result))
    }

    /// Send a prompt and get a stream of events.
    #[napi]
    pub async fn stream(&self, prompt: String) -> napi::Result<Vec<AgentEvent>> {
        let session = self.inner.clone();
        let (rx, _handle) = get_runtime()
            .spawn(async move { session.stream(&prompt).await })
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
}
