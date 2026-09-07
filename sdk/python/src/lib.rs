//! A3S Code Python Bindings
//!
//! Native Python module via PyO3 that wraps `a3s-code-core`'s Agent API.
//!
//! ## Usage
//!
//! ```python
//! from a3s_code import Agent
//!
//! agent = Agent.create("agent.acl")
//! session = agent.session("/my-project")
//!
//! result = session.send("What files handle auth?")
//! print(result.text)
//! ```
//!
//! ## Panic safety at the FFI boundary
//!
//! PyO3 0.23 wraps `#[pyfunction]` / `#[pymethods]` / `#[pymodule]`-init bodies
//! in `catch_unwind`, so a panic there surfaces as a Python `PanicException`
//! (a `BaseException` subclass) rather than UB. It does **not** cover panics
//! inside `std::thread` / `tokio::spawn` task bodies, or `Python::with_gil`
//! closures invoked from a worker thread *outside* a pyfunction frame — those
//! are silently lost, and a panicking `Drop` during an unwind aborts the
//! process.
//!
//! Convention this crate follows so the boundary stays safe: the Rust→Python
//! bridges that run on tokio worker threads (`PythonCallbackHandler`,
//! `PyBudgetGuard`, `PySlashCommand`) never `.unwrap()` / `panic!`; they use
//! `.ok()` / `unwrap_or_else` and fail closed. (Audited 2026-05: the only
//! production panic site is the lazy Tokio-runtime build in `get_runtime()`,
//! reached only from caught pyfunction frames.)

use a3s_code_core::commands::{
    CommandContext as RustCommandContext, CommandOutput as RustCommandOutput,
    SlashCommand as RustSlashCommand,
};
use a3s_code_core::hooks::{
    Hook as RustHook, HookConfig as RustHookConfig, HookEvent as RustHookEvent,
    HookEventType as RustHookEventType, HookHandler as RustHookHandler,
    HookMatcher as RustHookMatcher, HookResponse as RustHookResponse,
};
use a3s_code_core::llm::Message as RustMessage;
use a3s_code_core::orchestration::{
    execute_pipeline, execute_steps_parallel, execute_steps_parallel_resumable,
    AgentStepSpec as RustAgentStepSpec, PipelineStage as RustPipelineStage,
    StepOutcome as RustStepOutcome,
};
use a3s_code_core::queue::{
    ExternalTaskResult as RustExternalTaskResult, LaneHandlerConfig as RustLaneHandlerConfig,
    MetricsSnapshot as RustMetricsSnapshot, SessionLane as RustSessionLane,
    SessionQueueConfig as RustSessionQueueConfig, TaskHandlerMode as RustTaskHandlerMode,
};
use a3s_code_core::skills::{
    builtin_skills as rust_builtin_skills, Skill as RustSkill, SkillKind as RustSkillKind,
};
use a3s_code_core::verification::{
    format_verification_summary as rust_format_verification_summary,
    VerificationCommand as RustVerificationCommand, VerificationReport as RustVerificationReport,
    VerificationStatus as RustVerificationStatus, VerificationSummary as RustVerificationSummary,
};
use a3s_code_core::{
    run_event_envelope_v1 as rust_run_event_envelope_v1, Agent as RustAgent,
    AgentEvent as RustAgentEvent, AgentEventProjectionV1 as RustAgentEventProjectionV1,
    AgentResult as RustAgentResult, AgentRunSpawn as RustAgentRunSpawn,
    AgentSession as RustAgentSession, EventProtocolError as RustEventProtocolError,
    InterruptRequest as RustInterruptRequest, SteerRequest as RustSteerRequest,
    PlanningMode as RustPlanningMode, SessionOptions as RustSessionOptions, AGENT_EVENT_TYPES_V1,
    EVENT_ENVELOPE_V1_VERSION, SdkCapability as RustSdkCapability,
};
use pyo3::exceptions::{
    PyRuntimeError, PyStopAsyncIteration, PyStopIteration, PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::runtime::Runtime;
use tokio::sync::Mutex;

const MEMORY_UNAVAILABLE_MESSAGE: &str =
    "Memory unavailable for this session; check session init_warning";

fn py_code_error(error: a3s_code_core::CodeError) -> PyErr {
    let code = error.code();
    py_error_with_code(code, error.to_string())
}

fn py_task_scheduler_error(error: a3s_code_core::TaskSchedulerError) -> PyErr {
    let code = match error {
        a3s_code_core::TaskSchedulerError::InvalidConfig(_) => "INVALID_CONFIG",
        a3s_code_core::TaskSchedulerError::Cancelled => "TASK_ADMISSION_CANCELLED",
        a3s_code_core::TaskSchedulerError::Closed => "TASK_SCHEDULER_CLOSED",
    };
    py_error_with_code(code, error.to_string())
}

fn py_serve_error(failure_code: Option<&'static str>, error: a3s_code_core::CodeError) -> PyErr {
    let code = failure_code.unwrap_or(error.code());
    py_error_with_code(code, error.to_string())
}

fn py_error_with_code(code: &str, message: String) -> PyErr {
    let py_error = PyRuntimeError::new_err(message);
    Python::with_gil(|py| {
        let _ = py_error.value(py).setattr("code", code);
    });
    py_error
}

fn inline_skill_to_rust(name: String, content: String, kind: &str) -> PyResult<Arc<RustSkill>> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(PyValueError::new_err("skill name must not be empty"));
    }

    let kind = match kind.trim() {
        "" | "instruction" => RustSkillKind::Instruction,
        "persona" => RustSkillKind::Persona,
        "tool" => RustSkillKind::Tool,
        other => {
            return Err(PyValueError::new_err(format!(
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
        content,
        tags: Vec::new(),
        version: None,
    }))
}

use a3s_code_core::config::AgentDir as RustAgentDir;
use a3s_code_core::serve::{
    spawn_agent_dir_daemon as rust_spawn_agent_dir_daemon,
    ServeDaemonHandle as RustServeDaemonHandle,
};

// ============================================================================
// Utilities
// ============================================================================

/// Truncate a UTF-8 string to at most `max_bytes` bytes, without splitting
/// a multibyte character. Falls back to the full string if it's already
/// within the limit.
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn get_runtime() -> &'static Runtime {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        // Optimized runtime configuration for I/O-intensive workloads
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(num_cpus::get() * 2) // 2x CPU cores for better I/O handling
            .max_blocking_threads(512) // More blocking threads for CPU-intensive tasks
            .thread_name("a3s-code-worker")
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime")
    })
}

fn json_string_to_py(py: Python<'_>, json: &str) -> PyResult<PyObject> {
    let json_module = py.import("json")?;
    let parsed = json_module.call_method1("loads", (json,))?;
    Ok(parsed.into())
}

fn agent_run_spawn_to_py(py: Python<'_>, spawn: &RustAgentRunSpawn) -> PyResult<PyObject> {
    let json = serde_json::to_string(&serde_json::json!({
        "snapshot": spawn.snapshot(),
        "replayed": spawn.replayed(),
    }))
    .map_err(|e| PyRuntimeError::new_err(format!("Failed to serialize run spawn: {e}")))?;
    json_string_to_py(py, &json)
}

fn task_scheduler_stats_to_py(
    py: Python<'_>,
    stats: &a3s_code_core::TaskSchedulerStats,
) -> PyResult<PyObject> {
    let json = serde_json::to_string(stats).map_err(|error| {
        PyRuntimeError::new_err(format!("Failed to serialize task scheduler stats: {error}"))
    })?;
    json_string_to_py(py, &json)
}

fn task_scheduler_health_to_py(
    py: Python<'_>,
    health: &a3s_code_core::TaskSchedulerHealthSnapshot,
) -> PyResult<PyObject> {
    let json = serde_json::to_string(health).map_err(|error| {
        PyRuntimeError::new_err(format!(
            "Failed to serialize task scheduler health: {error}"
        ))
    })?;
    json_string_to_py(py, &json)
}

fn memory_maintenance_health_to_py(
    py: Python<'_>,
    health: &a3s_code_core::memory::MemoryMaintenanceHealth,
) -> PyResult<PyObject> {
    let json = serde_json::to_string(health).map_err(|error| {
        PyRuntimeError::new_err(format!(
            "Failed to serialize memory maintenance health: {error}"
        ))
    })?;
    json_string_to_py(py, &json)
}

// ============================================================================
// AgentResult
// ============================================================================

mod agent_result;
use agent_result::{format_verification_summary, PyAgentResult};

// ============================================================================
// AgentEvent
// ============================================================================

mod event_stream;
#[cfg(test)]
use event_stream::recv_stream_event;
use event_stream::{agent_event_types_v1, event_envelope_v1_version, PyAgentEvent, PyEventStream};
mod state_graph;
use state_graph::PyStateGraphRuntime;

#[cfg(test)]
mod agent_event_protocol_tests;

// ============================================================================
// ToolResult
// ============================================================================

/// Result of a direct tool execution (no LLM).
#[pyclass(name = "ToolResult")]
#[derive(Clone)]
struct PyToolResult {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    output: String,
    #[pyo3(get)]
    exit_code: i32,
    /// Raw JSON-encoded tool metadata returned by the Rust core API.
    #[pyo3(get)]
    metadata_json: Option<String>,
    /// Structured discriminant for tool failures, JSON-encoded with a
    /// ``type`` field on the top level —
    /// e.g. ``{"type":"version_conflict","path":"doc.md","expected":"etag-1","actual":"etag-2"}``.
    /// ``None`` on success or untyped failure. SDK callers parse it via
    /// the ``error_kind`` property below to branch on the failure kind
    /// without scanning the ``output`` string.
    #[pyo3(get)]
    error_kind_json: Option<String>,
}

impl From<a3s_code_core::ToolCallResult> for PyToolResult {
    fn from(result: a3s_code_core::ToolCallResult) -> Self {
        Self {
            name: result.name,
            output: result.output,
            exit_code: result.exit_code,
            metadata_json: result.metadata.as_ref().map(serde_json::Value::to_string),
            error_kind_json: result
                .error_kind
                .as_ref()
                .and_then(|kind| serde_json::to_string(kind).ok()),
        }
    }
}

#[pymethods]
impl PyToolResult {
    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        self.metadata_json
            .as_deref()
            .map(|json| json_string_to_py(py, json))
            .transpose()
    }

    /// Parsed `error_kind_json` as a dict. The discriminator lives on the
    /// ``type`` key; downstream code matches on that to decide retry
    /// behaviour without parsing ``output``.
    #[getter]
    fn error_kind(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        self.error_kind_json
            .as_deref()
            .map(|json| json_string_to_py(py, json))
            .transpose()
    }

    fn __repr__(&self) -> String {
        format!(
            "ToolResult(name='{}', exit_code={})",
            self.name, self.exit_code
        )
    }
}

// ============================================================================
// WebSearchParams
// ============================================================================

/// Parameters for the web_search tool.
#[pyclass(name = "WebSearchParams")]
#[derive(Clone)]
struct PyWebSearchParams {
    /// The search query.
    #[pyo3(get, set)]
    query: String,
    /// List of search engines to use.
    #[pyo3(get, set)]
    engines: Option<Vec<String>>,
    /// Maximum number of results to return (default: 10, max: 50).
    #[pyo3(get, set)]
    limit: Option<u32>,
    /// Search timeout in seconds (default: 10, max: 60).
    #[pyo3(get, set)]
    timeout: Option<u32>,
    /// Proxy URL (e.g., http://127.0.0.1:8080 or socks5://127.0.0.1:1080).
    #[pyo3(get, set)]
    proxy: Option<String>,
    /// Output format: "text" or "json".
    #[pyo3(get, set)]
    format: Option<String>,
}

#[pymethods]
impl PyWebSearchParams {
    #[new]
    #[pyo3(signature = (query, engines=None, limit=None, timeout=None, proxy=None, format=None))]
    fn new(
        query: String,
        engines: Option<Vec<String>>,
        limit: Option<u32>,
        timeout: Option<u32>,
        proxy: Option<String>,
        format: Option<String>,
    ) -> Self {
        Self {
            query,
            engines,
            limit,
            timeout,
            proxy,
            format,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "WebSearchParams(query='{}', engines={:?}, limit={:?}, timeout={:?}, format={:?})",
            self.query, self.engines, self.limit, self.timeout, self.format
        )
    }
}

// ============================================================================
// EventStream (Python Iterator + Async Iterator)
// ============================================================================

// ============================================================================
// Agent
// ============================================================================

mod async_bridge;
mod search_config;
use async_bridge::*;
use search_config::*;

mod moli_runtime;
use moli_runtime::{py_ensure_moli, py_moli_default_version, py_moli_runtime_info};

mod serve_handle;
use serve_handle::PyServeHandle;

mod agent;
use agent::PyAgent;

mod session;
use session::*;

mod session_memory;

mod session_tools;

mod workspace_retrieval;
use workspace_retrieval::*;

mod module_registration;

mod session_queue_api;

mod session_capabilities;

// ============================================================================
// Hook Helpers
// ============================================================================

mod hook_bridge;
#[cfg(test)]
use hook_bridge::parse_py_hook_response;
use hook_bridge::{py_parse_hook_event_type, PythonCallbackHandler};

// ============================================================================
// Orchestration: Python <-> Rust conversion + pipeline-stage bridge
// ============================================================================

mod orchestration_bridge;
use orchestration_bridge::{
    py_to_json_value, py_to_step_spec, step_outcome_to_py, PyBudgetGuard, PythonPipelineStage,
    DEFAULT_BUDGET_GUARD_TIMEOUT_MS,
};

/// Convert a Python dict (`{max_runs_retained: int, ...}`) into a
/// [`SessionRetentionLimits`](a3s_code_core::retention::SessionRetentionLimits).
/// Returns `None` if the supplied object is not a dict (caller treats
/// that as "no caps" and the framework default applies).
fn parse_py_retention_limits(
    py_obj: &pyo3::PyObject,
) -> Option<a3s_code_core::retention::SessionRetentionLimits> {
    use a3s_code_core::retention::SessionRetentionLimits;
    use pyo3::types::PyDict;

    pyo3::Python::with_gil(|py| {
        let bound = py_obj.bind(py);
        let dict = bound.downcast::<PyDict>().ok()?;
        let unbounded = dict
            .get_item("unbounded")
            .ok()
            .flatten()
            .and_then(|value| value.extract::<bool>().ok())
            .unwrap_or(false);
        let mut limits = if unbounded {
            SessionRetentionLimits::unbounded()
        } else {
            SessionRetentionLimits::default()
        };
        if let Some(v) = dict.get_item("max_runs_retained").ok().flatten() {
            if let Ok(n) = v.extract::<usize>() {
                limits.max_runs_retained = Some(n);
            }
        }
        if let Some(v) = dict.get_item("max_events_per_run").ok().flatten() {
            if let Ok(n) = v.extract::<usize>() {
                limits.max_events_per_run = Some(n);
            }
        }
        if let Some(v) = dict.get_item("max_event_bytes_per_run").ok().flatten() {
            if let Ok(n) = v.extract::<usize>() {
                limits.max_event_bytes_per_run = Some(n);
            }
        }
        if let Some(v) = dict.get_item("max_trace_events").ok().flatten() {
            if let Ok(n) = v.extract::<usize>() {
                limits.max_trace_events = Some(n);
            }
        }
        if let Some(v) = dict.get_item("max_terminal_subagent_tasks").ok().flatten() {
            if let Ok(n) = v.extract::<usize>() {
                limits.max_terminal_subagent_tasks = Some(n);
            }
        }
        Some(limits)
    })
}

// ============================================================================
// PySlashCommand — bridges Python callables into the Rust SlashCommand trait
// ============================================================================

/// Wraps a Python callable so it can be registered as a slash command handler.
///
/// GIL safety: `SlashCommand::execute()` is called from within an async Rust
/// context. `Python::with_gil` is safe to call from any Rust thread as long as
/// the caller releases the GIL before blocking (which `send()` does via
/// `py.allow_threads()`), so this does not deadlock.
struct PySlashCommand {
    name: String,
    description: String,
    /// Python callable: `(args: str, ctx: dict) -> str`
    handler: pyo3::Py<pyo3::PyAny>,
}

impl RustSlashCommand for PySlashCommand {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn execute(&self, args: &str, ctx: &RustCommandContext) -> RustCommandOutput {
        Python::with_gil(|py| {
            let result = (|| -> pyo3::PyResult<String> {
                let ctx_dict = PyDict::new(py);
                ctx_dict.set_item("session_id", &ctx.session_id)?;
                ctx_dict.set_item("workspace", &ctx.workspace)?;
                ctx_dict.set_item("model", &ctx.model)?;
                ctx_dict.set_item("history_len", ctx.history_len)?;
                ctx_dict.set_item("total_tokens", ctx.total_tokens)?;
                ctx_dict.set_item("total_cost", ctx.total_cost)?;
                ctx_dict.set_item("tool_names", ctx.tool_names.clone())?;
                let ret = self.handler.call1(py, (args, ctx_dict))?;
                ret.extract::<String>(py)
            })();
            match result {
                Ok(text) => RustCommandOutput::text(text),
                Err(e) => RustCommandOutput::text(format!("Command error: {e}")),
            }
        })
    }
}

mod typed_providers;
use typed_providers::*;

mod session_config;
use session_config::*;

mod session_options;
use session_options::*;

mod session_options_conversion;
use session_options_conversion::*;

mod session_queue;
use session_queue::*;
// ============================================================================
// Helpers
// ============================================================================

fn delegate_task_args(
    agent: String,
    description: String,
    prompt: String,
    background: bool,
    max_steps: Option<u32>,
) -> serde_json::Value {
    let mut args = serde_json::json!({
        "agent": agent,
        "description": description,
        "prompt": prompt,
    });
    if background {
        args["background"] = serde_json::json!(true);
    }
    if let Some(max_steps) = max_steps {
        args["max_steps"] = serde_json::json!(max_steps);
    }
    args
}

fn delegated_tasks_args(tasks: serde_json::Value) -> PyResult<serde_json::Value> {
    if !tasks.is_array() {
        return Err(PyValueError::new_err(
            "tasks must be a list of dictionaries",
        ));
    }
    Ok(serde_json::json!({ "tasks": tasks }))
}

fn metrics_snapshot_to_json_str(s: RustMetricsSnapshot) -> Result<String, serde_json::Error> {
    let counters: serde_json::Map<String, serde_json::Value> = s
        .counters
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::Number(v.into())))
        .collect();
    let gauges: serde_json::Map<String, serde_json::Value> = s
        .gauges
        .into_iter()
        .map(|(k, v)| {
            let n = serde_json::Number::from_f64(v).unwrap_or_else(|| 0.into());
            (k, serde_json::Value::Number(n))
        })
        .collect();
    let histograms: serde_json::Map<String, serde_json::Value> = s
        .histograms
        .into_iter()
        .map(|(k, h)| {
            let to_f = |v: f64| serde_json::Number::from_f64(v).unwrap_or_else(|| 0.into());
            let (min, max) = if h.count == 0 {
                (0.into(), 0.into())
            } else {
                (to_f(h.min), to_f(h.max))
            };
            let v = serde_json::json!({
                "count": h.count,
                "sum": to_f(h.sum),
                "min": min,
                "max": max,
                "mean": to_f(h.mean),
                "p50": to_f(h.percentiles.p50),
                "p90": to_f(h.percentiles.p90),
                "p95": to_f(h.percentiles.p95),
                "p99": to_f(h.percentiles.p99),
            });
            (k, v)
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "counters": serde_json::Value::Object(counters),
        "gauges": serde_json::Value::Object(gauges),
        "histograms": serde_json::Value::Object(histograms),
    }))
}

fn py_dict_to_json(dict: &Bound<'_, pyo3::types::PyDict>) -> PyResult<String> {
    let py = dict.py();
    let json_mod = py.import("json")?;
    let json_str = json_mod.call_method1("dumps", (dict,))?;
    json_str.extract::<String>()
}

fn py_any_to_json(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let json_mod = value.py().import("json")?;
    let json_str = json_mod.call_method1("dumps", (value,))?;
    json_str.extract::<String>()
}

fn verification_reports_from_value(
    reports: serde_json::Value,
) -> PyResult<Vec<RustVerificationReport>> {
    let reports = match reports {
        serde_json::Value::Array(_) => serde_json::from_value(reports),
        serde_json::Value::Object(_) => {
            serde_json::from_value::<RustVerificationReport>(reports).map(|report| vec![report])
        }
        _ => {
            return Err(PyTypeError::new_err(
                "verification reports must be a list or dict",
            ));
        }
    };
    reports.map_err(|e| PyValueError::new_err(format!("Invalid verification report: {e}")))
}

fn py_verification_reports_to_rust(
    _py: Python<'_>,
    reports: &Bound<'_, PyAny>,
) -> PyResult<Vec<RustVerificationReport>> {
    let json_str = py_any_to_json(reports)?;
    let value: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| PyValueError::new_err(format!("Invalid verification report JSON: {e}")))?;
    verification_reports_from_value(value)
}

fn normalize_task_options(mut value: serde_json::Value) -> PyResult<serde_json::Value> {
    let obj = value
        .as_object_mut()
        .ok_or_else(|| PyValueError::new_err("task options must be a dict"))?;

    for field in ["agent", "description", "prompt"] {
        if !obj.get(field).is_some_and(|v| v.is_string()) {
            return Err(PyValueError::new_err(format!(
                "task options must include string field '{field}'"
            )));
        }
    }

    if let Some(value) = obj.remove("maxSteps") {
        obj.entry("max_steps".to_string()).or_insert(value);
    }

    Ok(value)
}

fn normalize_git_args(mut args: serde_json::Value) -> PyResult<serde_json::Value> {
    let obj = args
        .as_object_mut()
        .ok_or_else(|| PyValueError::new_err("git options must be a dict"))?;

    if !obj.contains_key("command") {
        return Err(PyValueError::new_err(
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

fn normalize_program_script_options(
    options: &Bound<'_, pyo3::types::PyDict>,
) -> PyResult<serde_json::Value> {
    let json_str = py_dict_to_json(options)?;
    let value: serde_json::Value = serde_json::from_str(&json_str)
        .map_err(|e| PyValueError::new_err(format!("Invalid program options: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| PyValueError::new_err("program options must be a dict"))?;

    let mut args = serde_json::Map::new();
    args.insert("type".to_string(), serde_json::json!("script"));
    args.insert("language".to_string(), serde_json::json!("javascript"));

    for key in ["source", "path", "inputs", "limits"] {
        if let Some(field) = obj.get(key) {
            args.insert(key.to_string(), field.clone());
        }
    }

    if let Some(field) = obj.get("allowed_tools").or_else(|| obj.get("allowedTools")) {
        args.insert("allowed_tools".to_string(), field.clone());
    }

    Ok(serde_json::Value::Object(args))
}

fn timeout_ms_to_secs(timeout_ms: u64) -> u64 {
    timeout_ms.div_ceil(1000).max(1)
}

fn normalize_mcp_server_config(
    mut value: serde_json::Value,
) -> PyResult<a3s_code_core::mcp::protocol::McpServerConfig> {
    let obj = value
        .as_object_mut()
        .ok_or_else(|| PyValueError::new_err("MCP server config must be a dict"))?;

    for key in [
        "timeout_ms",
        "timeoutMs",
        "tool_timeout_ms",
        "toolTimeoutMs",
    ] {
        if let Some(timeout_ms) = obj.remove(key) {
            let timeout_ms = timeout_ms
                .as_u64()
                .ok_or_else(|| PyValueError::new_err(format!("{key} must be an integer")))?;
            obj.entry("toolTimeoutSecs".to_string())
                .or_insert_with(|| serde_json::json!(timeout_ms_to_secs(timeout_ms)));
            break;
        }
    }

    if let Some(transport) = obj.get_mut("transport") {
        normalize_mcp_transport_alias(transport);
    }

    serde_json::from_value(value)
        .map_err(|e| PyValueError::new_err(format!("Invalid MCP server config: {e}")))
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

/// Convert Python attachment dicts to Rust Attachment vec.
fn py_attachments_to_rust(
    attachments: &[Bound<'_, PyDict>],
) -> PyResult<Vec<a3s_code_core::llm::Attachment>> {
    attachments
        .iter()
        .map(|dict| {
            let data: Vec<u8> = dict
                .get_item("data")?
                .ok_or_else(|| PyValueError::new_err("Attachment missing 'data' field"))?
                .extract()?;
            let media_type: String = dict
                .get_item("media_type")?
                .ok_or_else(|| PyValueError::new_err("Attachment missing 'media_type' field"))?
                .extract()?;
            Ok(a3s_code_core::llm::Attachment::new(data, media_type))
        })
        .collect()
}

fn py_attachment_list_to_rust(
    attachments: &Bound<'_, PyList>,
) -> PyResult<Vec<a3s_code_core::llm::Attachment>> {
    attachments
        .iter()
        .map(|item| {
            let dict = item
                .downcast::<PyDict>()
                .map_err(|_| PyTypeError::new_err("attachments must contain dict items"))?;
            let data: Vec<u8> = dict
                .get_item("data")?
                .ok_or_else(|| PyValueError::new_err("Attachment missing 'data' field"))?
                .extract()?;
            let media_type: String = dict
                .get_item("media_type")?
                .ok_or_else(|| PyValueError::new_err("Attachment missing 'media_type' field"))?
                .extract()?;
            Ok(a3s_code_core::llm::Attachment::new(data, media_type))
        })
        .collect()
}

fn py_session_request_to_parts(
    request: &Bound<'_, PyDict>,
) -> PyResult<(
    String,
    Option<Vec<RustMessage>>,
    Vec<a3s_code_core::llm::Attachment>,
)> {
    let prompt = request
        .get_item("prompt")?
        .ok_or_else(|| PyValueError::new_err("request missing 'prompt' field"))?
        .extract::<String>()?;

    let history = match request.get_item("history")? {
        Some(value) => {
            let list = value
                .downcast::<PyList>()
                .map_err(|_| PyTypeError::new_err("request.history must be a list"))?;
            Some(py_list_to_messages(list)?)
        }
        None => None,
    };

    let attachments = match request.get_item("attachments")? {
        Some(value) => {
            let list = value
                .downcast::<PyList>()
                .map_err(|_| PyTypeError::new_err("request.attachments must be a list"))?;
            py_attachment_list_to_rust(list)?
        }
        None => Vec::new(),
    };

    Ok((prompt, history, attachments))
}

fn py_session_input_to_parts(
    input: &Bound<'_, PyAny>,
    history: Option<&Bound<'_, PyList>>,
) -> PyResult<(
    String,
    Option<Vec<RustMessage>>,
    Vec<a3s_code_core::llm::Attachment>,
)> {
    if let Ok(prompt) = input.extract::<String>() {
        let rust_history = history.map(py_list_to_messages).transpose()?;
        return Ok((prompt, rust_history, Vec::new()));
    }

    if let Ok(request) = input.downcast::<PyDict>() {
        return py_session_request_to_parts(request);
    }

    Err(PyTypeError::new_err(
        "session input must be a prompt string or request dict",
    ))
}

/// Convert a Python list of message dicts to `Vec<RustMessage>`.
///
/// Expected format: `[{"role": "user", "content": [{"type": "text", "text": "Hello"}]}]`
fn py_list_to_messages(list: &Bound<'_, PyList>) -> PyResult<Vec<RustMessage>> {
    let py = list.py();
    let json_mod = py.import("json")?;
    let json_str: String = json_mod.call_method1("dumps", (list,))?.extract()?;
    serde_json::from_str::<Vec<RustMessage>>(&json_str)
        .map_err(|e| PyTypeError::new_err(format!("Invalid history format: {e}")))
}

/// Convert a Python list of verification command dicts to Rust commands.
///
/// Expected format:
/// `[{"id": "check:test", "kind": "test", "description": "Run tests", "command": "cargo test"}]`
fn py_list_to_verification_commands(
    list: &Bound<'_, PyList>,
) -> PyResult<Vec<RustVerificationCommand>> {
    let py = list.py();
    let json_mod = py.import("json")?;
    let json_str: String = json_mod.call_method1("dumps", (list,))?.extract()?;
    serde_json::from_str::<Vec<RustVerificationCommand>>(&json_str)
        .map_err(|e| PyTypeError::new_err(format!("Invalid verification command format: {e}")))
}

/// Convert `&[RustMessage]` to a Python list of dicts.
fn messages_to_py_list<'py>(
    py: Python<'py>,
    messages: &[RustMessage],
) -> PyResult<Bound<'py, PyList>> {
    let json_str = serde_json::to_string(messages)
        .map_err(|e| PyRuntimeError::new_err(format!("Failed to serialize history: {e}")))?;
    let json_mod = py.import("json")?;
    let py_obj = json_mod.call_method1("loads", (json_str,))?;
    py_obj
        .downcast::<PyList>()
        .cloned()
        .map_err(|e| PyRuntimeError::new_err(format!("Unexpected serialization result: {e}")))
}

// ============================================================================
// SkillInfo
// ============================================================================

/// Metadata about a compatibility built-in skill entry.
#[pyclass(name = "SkillInfo")]
#[derive(Clone)]
struct PySkillInfo {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    description: String,
    #[pyo3(get)]
    kind: String,
}

// ============================================================================
// SDK capability inventory
// ============================================================================

/// One product capability exposed through the Python SDK.
#[pyclass(name = "SdkCapability")]
#[derive(Clone)]
struct PySdkCapability {
    #[pyo3(get)]
    id: String,
    #[pyo3(get)]
    category: String,
    #[pyo3(get)]
    description: String,
    #[pyo3(get)]
    operations: Vec<String>,
    #[pyo3(get)]
    host_owned: bool,
}

impl From<RustSdkCapability> for PySdkCapability {
    fn from(value: RustSdkCapability) -> Self {
        Self {
            id: value.id,
            category: value.category,
            description: value.description,
            operations: value.operations,
            host_owned: value.host_owned,
        }
    }
}

#[pymethods]
impl PySdkCapability {
    fn __repr__(&self) -> String {
        format!("SdkCapability(id='{}', category='{}')", self.id, self.category)
    }
}

#[pymethods]
impl PySkillInfo {
    fn __repr__(&self) -> String {
        format!(
            "SkillInfo(name='{}', kind='{}', description='{}')",
            self.name,
            self.kind,
            if self.description.len() > 60 {
                format!("{}...", truncate_utf8(&self.description, 60))
            } else {
                self.description.clone()
            }
        )
    }
}

// ============================================================================
// Python Module
// ============================================================================

/// A3S Code - Native AI coding agent library for Python.
#[pymodule(name = "_native")]
fn a3s_code_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    module_registration::register(m)
}

/// Return the compatibility built-in skill list.
///
/// A3S Code no longer ships embedded built-in skills, so this returns an empty
/// list unless embedded skills are reintroduced in a future release.
#[pyfunction(name = "builtin_skills")]
fn py_builtin_skills() -> Vec<PySkillInfo> {
    rust_builtin_skills()
        .into_iter()
        .map(|s| PySkillInfo {
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

/// Return the complete Core capability inventory exposed by this binding.
#[pyfunction(name = "sdk_capabilities")]
fn py_sdk_capabilities() -> Vec<PySdkCapability> {
    a3s_code_core::sdk_capabilities()
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Return the stable schema identifier for the capability inventory.
#[pyfunction(name = "sdk_capabilities_schema")]
fn py_sdk_capabilities_schema() -> String {
    a3s_code_core::sdk_capabilities_schema().to_owned()
}

#[cfg(test)]
mod tests;
