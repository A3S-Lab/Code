//! A3S Code Python Bindings
//!
//! Native Python module via PyO3 that wraps `a3s-code-core`'s Agent API.
//!
//! ## Usage
//!
//! ```python
//! from a3s_code import Agent
//!
//! agent = Agent(model="claude-sonnet-4-20250514", api_key="sk-ant-...", workspace="/project")
//!
//! # Synchronous (blocking)
//! result = agent.send("What files handle auth?")
//! print(result.text)
//!
//! # Streaming (iterator)
//! for event in agent.stream("Refactor auth"):
//!     if event.type == "text_delta":
//!         print(event.text, end="", flush=True)
//!
//! # Direct tool calls
//! content = agent.read_file("src/main.rs")
//! files = agent.glob("**/*.rs")
//! output = agent.bash("cargo test")
//! ```

use a3s_code_core::agent::{AgentEvent as RustAgentEvent, AgentResult as RustAgentResult};
use a3s_code_core::config::{ModelConfig, ProviderConfig};
use a3s_code_core::{Agent as RustAgent, CodeConfig};
use pyo3::exceptions::{PyRuntimeError, PyStopIteration, PyValueError};
use pyo3::prelude::*;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;

// ============================================================================
// Tokio Runtime
// ============================================================================

/// Get or create a shared tokio runtime for the module.
fn get_runtime() -> &'static Runtime {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Runtime::new().expect("Failed to create tokio runtime")
    })
}

// ============================================================================
// AgentResult
// ============================================================================

/// Result of a non-streaming agent execution.
#[pyclass(name = "AgentResult")]
#[derive(Clone)]
struct PyAgentResult {
    /// The final text response from the agent.
    #[pyo3(get)]
    text: String,
    /// Number of tool calls made during execution.
    #[pyo3(get)]
    tool_calls_count: usize,
    /// Total prompt tokens used.
    #[pyo3(get)]
    prompt_tokens: usize,
    /// Total completion tokens used.
    #[pyo3(get)]
    completion_tokens: usize,
    /// Total tokens used.
    #[pyo3(get)]
    total_tokens: usize,
}

#[pymethods]
impl PyAgentResult {
    fn __repr__(&self) -> String {
        format!(
            "AgentResult(text={:?}, tool_calls={}, tokens={})",
            if self.text.len() > 80 {
                format!("{}...", &self.text[..80])
            } else {
                self.text.clone()
            },
            self.tool_calls_count,
            self.total_tokens,
        )
    }

    fn __str__(&self) -> &str {
        &self.text
    }
}

impl From<RustAgentResult> for PyAgentResult {
    fn from(r: RustAgentResult) -> Self {
        Self {
            text: r.text,
            tool_calls_count: r.tool_calls_count,
            prompt_tokens: r.usage.prompt_tokens,
            completion_tokens: r.usage.completion_tokens,
            total_tokens: r.usage.total_tokens,
        }
    }
}

// ============================================================================
// AgentEvent
// ============================================================================

/// A single event from the agent's streaming output.
#[pyclass(name = "AgentEvent")]
#[derive(Clone)]
struct PyAgentEvent {
    /// Event type: "start", "text_delta", "tool_start", "tool_end",
    /// "turn_start", "turn_end", "end", "error", etc.
    #[pyo3(get)]
    event_type: String,
    /// Text content (for text_delta and end events).
    #[pyo3(get)]
    text: Option<String>,
    /// Tool name (for tool_start and tool_end events).
    #[pyo3(get)]
    tool_name: Option<String>,
    /// Tool ID (for tool_start and tool_end events).
    #[pyo3(get)]
    tool_id: Option<String>,
    /// Tool output (for tool_end events).
    #[pyo3(get)]
    tool_output: Option<String>,
    /// Exit code (for tool_end events).
    #[pyo3(get)]
    exit_code: Option<i32>,
    /// Turn number (for turn_start and turn_end events).
    #[pyo3(get)]
    turn: Option<usize>,
    /// Prompt text (for start events).
    #[pyo3(get)]
    prompt: Option<String>,
    /// Error message (for error events).
    #[pyo3(get)]
    error: Option<String>,
    /// Token usage (for turn_end and end events).
    #[pyo3(get)]
    total_tokens: Option<usize>,
}

#[pymethods]
impl PyAgentEvent {
    fn __repr__(&self) -> String {
        match self.event_type.as_str() {
            "text_delta" => format!(
                "AgentEvent(type='text_delta', text={:?})",
                self.text.as_deref().unwrap_or("")
            ),
            "tool_start" => format!(
                "AgentEvent(type='tool_start', tool='{}')",
                self.tool_name.as_deref().unwrap_or("")
            ),
            "end" => format!(
                "AgentEvent(type='end', tokens={})",
                self.total_tokens.unwrap_or(0)
            ),
            _ => format!("AgentEvent(type='{}')", self.event_type),
        }
    }
}

impl From<RustAgentEvent> for PyAgentEvent {
    fn from(e: RustAgentEvent) -> Self {
        match e {
            RustAgentEvent::Start { prompt } => Self {
                event_type: "start".into(),
                prompt: Some(prompt),
                text: None,
                tool_name: None,
                tool_id: None,
                tool_output: None,
                exit_code: None,
                turn: None,
                error: None,
                total_tokens: None,
            },
            RustAgentEvent::TextDelta { text } => Self {
                event_type: "text_delta".into(),
                text: Some(text),
                prompt: None,
                tool_name: None,
                tool_id: None,
                tool_output: None,
                exit_code: None,
                turn: None,
                error: None,
                total_tokens: None,
            },
            RustAgentEvent::TurnStart { turn } => Self {
                event_type: "turn_start".into(),
                turn: Some(turn),
                text: None,
                prompt: None,
                tool_name: None,
                tool_id: None,
                tool_output: None,
                exit_code: None,
                error: None,
                total_tokens: None,
            },
            RustAgentEvent::TurnEnd { turn, usage } => Self {
                event_type: "turn_end".into(),
                turn: Some(turn),
                total_tokens: Some(usage.total_tokens),
                text: None,
                prompt: None,
                tool_name: None,
                tool_id: None,
                tool_output: None,
                exit_code: None,
                error: None,
            },
            RustAgentEvent::ToolStart { id, name } => Self {
                event_type: "tool_start".into(),
                tool_id: Some(id),
                tool_name: Some(name),
                text: None,
                prompt: None,
                tool_output: None,
                exit_code: None,
                turn: None,
                error: None,
                total_tokens: None,
            },
            RustAgentEvent::ToolEnd {
                id,
                name,
                output,
                exit_code,
            } => Self {
                event_type: "tool_end".into(),
                tool_id: Some(id),
                tool_name: Some(name),
                tool_output: Some(output),
                exit_code: Some(exit_code),
                text: None,
                prompt: None,
                turn: None,
                error: None,
                total_tokens: None,
            },
            RustAgentEvent::ToolOutputDelta { id, name, delta } => Self {
                event_type: "tool_output_delta".into(),
                tool_id: Some(id),
                tool_name: Some(name),
                text: Some(delta),
                prompt: None,
                tool_output: None,
                exit_code: None,
                turn: None,
                error: None,
                total_tokens: None,
            },
            RustAgentEvent::End { text, usage } => Self {
                event_type: "end".into(),
                text: Some(text),
                total_tokens: Some(usage.total_tokens),
                prompt: None,
                tool_name: None,
                tool_id: None,
                tool_output: None,
                exit_code: None,
                turn: None,
                error: None,
            },
            RustAgentEvent::Error { message } => Self {
                event_type: "error".into(),
                error: Some(message),
                text: None,
                prompt: None,
                tool_name: None,
                tool_id: None,
                tool_output: None,
                exit_code: None,
                turn: None,
                total_tokens: None,
            },
            // Map all other events to a generic representation
            other => {
                let event_type = match &other {
                    RustAgentEvent::ConfirmationRequired { .. } => "confirmation_required",
                    RustAgentEvent::ConfirmationReceived { .. } => "confirmation_received",
                    RustAgentEvent::ConfirmationTimeout { .. } => "confirmation_timeout",
                    RustAgentEvent::ExternalTaskPending { .. } => "external_task_pending",
                    RustAgentEvent::ExternalTaskCompleted { .. } => "external_task_completed",
                    RustAgentEvent::PermissionDenied { .. } => "permission_denied",
                    RustAgentEvent::ContextResolving { .. } => "context_resolving",
                    RustAgentEvent::ContextResolved { .. } => "context_resolved",
                    _ => "unknown",
                };
                Self {
                    event_type: event_type.into(),
                    text: None,
                    prompt: None,
                    tool_name: None,
                    tool_id: None,
                    tool_output: None,
                    exit_code: None,
                    turn: None,
                    error: None,
                    total_tokens: None,
                }
            }
        }
    }
}

// ============================================================================
// EventStream (Python Iterator)
// ============================================================================

/// Iterator that yields AgentEvents from a streaming execution.
#[pyclass(name = "EventStream")]
struct PyEventStream {
    rx: Arc<Mutex<tokio::sync::mpsc::Receiver<RustAgentEvent>>>,
    #[allow(dead_code)]
    handle: Arc<Mutex<Option<tokio::task::JoinHandle<anyhow::Result<RustAgentResult>>>>>,
    done: bool,
}

#[pymethods]
impl PyEventStream {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<PyAgentEvent>> {
        if self.done {
            return Err(PyStopIteration::new_err("stream exhausted"));
        }

        let rx = self.rx.clone();
        let result = py.allow_threads(|| {
            get_runtime().block_on(async {
                let mut rx = rx.lock().await;
                rx.recv().await
            })
        });

        match result {
            Some(event) => {
                let is_end = matches!(event, RustAgentEvent::End { .. });
                let is_error = matches!(event, RustAgentEvent::Error { .. });
                let py_event = PyAgentEvent::from(event);
                if is_end || is_error {
                    self.done = true;
                }
                Ok(Some(py_event))
            }
            None => {
                self.done = true;
                Err(PyStopIteration::new_err("stream exhausted"))
            }
        }
    }
}

// ============================================================================
// Agent
// ============================================================================

/// AI coding agent with tool execution capabilities.
///
/// Create an Agent by providing a model name, API key, and workspace path.
/// The agent auto-detects whether to use the Anthropic or OpenAI API based
/// on the model name.
///
/// Args:
///     model: LLM model identifier (e.g., "claude-sonnet-4-20250514", "gpt-4o")
///     api_key: API key for the LLM provider
///     workspace: Path to the workspace directory (sandbox root)
///     system_prompt: Optional system prompt for the agent
///     base_url: Optional base URL override for the LLM API
///     max_tool_rounds: Maximum tool execution rounds per turn (default: 50)
#[pyclass(name = "Agent")]
struct PyAgent {
    agent: Arc<RustAgent>,
}

#[pymethods]
impl PyAgent {
    #[new]
    #[pyo3(signature = (model, api_key, workspace=None, system_prompt=None, base_url=None, max_tool_rounds=None))]
    fn new(
        py: Python<'_>,
        model: &str,
        api_key: &str,
        workspace: Option<&str>,
        system_prompt: Option<&str>,
        base_url: Option<&str>,
        max_tool_rounds: Option<usize>,
    ) -> PyResult<Self> {
        // Detect provider from model name
        let provider_name = if model.starts_with("claude") || model.starts_with("anthropic") {
            "anthropic"
        } else {
            "openai"
        };

        let model_config: ModelConfig = serde_json::from_value(serde_json::json!({
            "id": model,
            "name": model,
        }))
        .map_err(|e| PyRuntimeError::new_err(format!("Failed to create model config: {e}")))?;

        let mut provider = ProviderConfig {
            name: provider_name.to_string(),
            api_key: Some(api_key.to_string()),
            base_url: base_url.map(|s| s.to_string()),
            models: vec![model_config],
        };

        // If base_url is set on provider, also check model-level override
        if base_url.is_some() {
            provider.base_url = base_url.map(|s| s.to_string());
        }

        let config = CodeConfig {
            default_provider: Some(provider_name.to_string()),
            default_model: Some(model.to_string()),
            providers: vec![provider],
            system_prompt: system_prompt.map(|s| s.to_string()),
            max_tool_rounds,
            ..Default::default()
        };

        let agent = py.allow_threads(|| {
            get_runtime().block_on(RustAgent::from_config(config))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("Failed to build agent: {e}")))?;

        Ok(Self {
            agent: Arc::new(agent),
        })
    }

    /// Send a prompt and wait for the complete response.
    ///
    /// Args:
    ///     prompt: The user prompt to send to the agent.
    ///
    /// Returns:
    ///     AgentResult with text, token usage, and tool call count.
    fn send(&self, py: Python<'_>, prompt: &str) -> PyResult<PyAgentResult> {
        let agent = self.agent.clone();
        let prompt = prompt.to_string();

        let result = py.allow_threads(move || {
            get_runtime().block_on(agent.send(&prompt))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("Agent execution failed: {e}")))?;

        Ok(PyAgentResult::from(result))
    }

    /// Send a prompt and get a streaming iterator of events.
    ///
    /// Args:
    ///     prompt: The user prompt to send to the agent.
    ///
    /// Returns:
    ///     EventStream iterator yielding AgentEvent objects.
    ///
    /// Example:
    ///     ```python
    ///     for event in agent.stream("Refactor auth"):
    ///         if event.event_type == "text_delta":
    ///             print(event.text, end="", flush=True)
    ///     ```
    fn stream(&self, py: Python<'_>, prompt: &str) -> PyResult<PyEventStream> {
        let agent = self.agent.clone();
        let prompt = prompt.to_string();

        let (rx, handle) = py
            .allow_threads(move || {
                get_runtime().block_on(agent.stream(&prompt))
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to start stream: {e}")))?;

        Ok(PyEventStream {
            rx: Arc::new(Mutex::new(rx)),
            handle: Arc::new(Mutex::new(Some(handle))),
            done: false,
        })
    }

    // ========================================================================
    // Direct tool calls
    // ========================================================================

    /// Execute a tool by name, bypassing the LLM.
    ///
    /// Args:
    ///     name: Tool name (e.g., "read", "bash", "glob", "grep")
    ///     args: Tool arguments as a JSON-compatible dict
    ///
    /// Returns:
    ///     Dict with "output" (str), "exit_code" (int), and "name" (str)
    fn tool(&self, py: Python<'_>, name: &str, args: &Bound<'_, pyo3::types::PyDict>) -> PyResult<PyObject> {
        let json_str = py_dict_to_json(args)?;
        let json_value: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("Invalid JSON args: {e}")))?;

        let agent = self.agent.clone();
        let name = name.to_string();

        let result = py
            .allow_threads(move || {
                get_runtime().block_on(agent.tool(&name, json_value))
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Tool execution failed: {e}")))?;

        let dict = pyo3::types::PyDict::new(py);
        dict.set_item("name", result.name)?;
        dict.set_item("output", result.output)?;
        dict.set_item("exit_code", result.exit_code)?;
        Ok(dict.into_any().unbind())
    }

    /// Read a file from the workspace.
    fn read_file(&self, py: Python<'_>, path: &str) -> PyResult<String> {
        let agent = self.agent.clone();
        let path = path.to_string();

        py.allow_threads(move || {
            get_runtime().block_on(agent.read_file(&path))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("{e}")))
    }

    /// Execute a bash command in the workspace.
    fn bash(&self, py: Python<'_>, command: &str) -> PyResult<String> {
        let agent = self.agent.clone();
        let command = command.to_string();

        py.allow_threads(move || {
            get_runtime().block_on(agent.bash(&command))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("{e}")))
    }

    /// Search for files matching a glob pattern.
    fn glob(&self, py: Python<'_>, pattern: &str) -> PyResult<Vec<String>> {
        let agent = self.agent.clone();
        let pattern = pattern.to_string();

        py.allow_threads(move || {
            get_runtime().block_on(agent.glob(&pattern))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("{e}")))
    }

    /// Search file contents with a regex pattern.
    fn grep(&self, py: Python<'_>, pattern: &str) -> PyResult<String> {
        let agent = self.agent.clone();
        let pattern = pattern.to_string();

        py.allow_threads(move || {
            get_runtime().block_on(agent.grep(&pattern))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("{e}")))
    }

    fn __repr__(&self) -> String {
        "Agent(...)".to_string()
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Convert a Python dict to a JSON string using Python's json.dumps.
fn py_dict_to_json(dict: &Bound<'_, pyo3::types::PyDict>) -> PyResult<String> {
    let py = dict.py();
    let json_mod = py.import("json")?;
    let json_str = json_mod.call_method1("dumps", (dict,))?;
    json_str.extract::<String>()
}

// ============================================================================
// Python Module
// ============================================================================

/// A3S Code - Native AI coding agent library for Python.
///
/// This module provides direct access to the A3S Code agent engine
/// compiled as a native Python extension. No gRPC server needed.
///
/// Example:
///     ```python
///     from a3s_code import Agent
///
///     agent = Agent(
///         model="claude-sonnet-4-20250514",
///         api_key="sk-ant-...",
///         workspace="/my-project",
///     )
///     result = agent.send("What files handle auth?")
///     print(result.text)
///     ```
#[pymodule]
fn a3s_code(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyAgent>()?;
    m.add_class::<PyAgentResult>()?;
    m.add_class::<PyAgentEvent>()?;
    m.add_class::<PyEventStream>()?;
    Ok(())
}
