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

use a3s_code_core::commands::{
    CommandContext as RustCommandContext, CommandOutput as RustCommandOutput,
    SlashCommand as RustSlashCommand,
};
use a3s_code_core::config::{
    BrowserBackend as RustBrowserBackend, HeadlessConfig as RustHeadlessConfig,
    SearchConfig as RustSearchConfig, SearchEngineConfig as RustSearchEngineConfig,
    SearchHealthConfig as RustSearchHealthConfig,
};
use a3s_code_core::hooks::{
    Hook as RustHook, HookConfig as RustHookConfig, HookEvent as RustHookEvent,
    HookEventType as RustHookEventType, HookHandler as RustHookHandler,
    HookMatcher as RustHookMatcher, HookResponse as RustHookResponse,
};
use a3s_code_core::llm::Message as RustMessage;
use a3s_code_core::orchestrator::{
    AgentOrchestrator as RustOrchestrator, SubAgentActivity as RustSubAgentActivity,
    SubAgentConfig as RustSubAgentConfig, SubAgentHandle as RustSubAgentHandle,
    SubAgentInfo as RustSubAgentInfo,
};
use a3s_code_core::permissions::{
    PermissionDecision as RustPermissionDecision, PermissionPolicy as RustPermissionPolicy,
    PermissionRule as RustPermissionRule,
};
use a3s_code_core::queue::{
    ExternalTaskResult as RustExternalTaskResult, LaneHandlerConfig as RustLaneHandlerConfig,
    MetricsSnapshot as RustMetricsSnapshot,
    SessionLane as RustSessionLane, SessionQueueConfig as RustSessionQueueConfig,
    TaskHandlerMode as RustTaskHandlerMode,
};
use a3s_code_core::skills::{builtin_skills as rust_builtin_skills, SkillKind as RustSkillKind};
use a3s_code_core::verification::{
    format_verification_summary as rust_format_verification_summary,
    VerificationCommand as RustVerificationCommand, VerificationStatus as RustVerificationStatus,
    VerificationSummary as RustVerificationSummary,
};
use a3s_code_core::{
    Agent as RustAgent, AgentEvent as RustAgentEvent, AgentResult as RustAgentResult,
    AgentSession as RustAgentSession, BtwResult as RustBtwResult,
    SessionOptions as RustSessionOptions,
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

// AHP Type Bindings
// ============================================================================
mod ahp_types;

use ahp_types::{
    PyAhpEventContext, PyAhpEventType, PyFact, PyIdleDecision, PyIntentDetectionDecision,
    PyIntentDetectionEvent, PyMemorySummary, PySessionStats, PyTargetHints,
};

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

fn parse_agentic_search_results(json: &str) -> PyResult<Vec<serde_json::Value>> {
    let metadata: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| PyValueError::new_err(format!("Invalid tool metadata payload: {e}")))?;
    Ok(metadata
        .get("results")
        .and_then(|results| results.as_array())
        .cloned()
        .unwrap_or_default())
}

// ============================================================================
// AgentResult
// ============================================================================

/// Result of a non-streaming agent execution.
#[pyclass(name = "AgentResult")]
#[derive(Clone)]
struct PyAgentResult {
    #[pyo3(get)]
    text: String,
    #[pyo3(get)]
    tool_calls_count: usize,
    #[pyo3(get)]
    prompt_tokens: usize,
    #[pyo3(get)]
    completion_tokens: usize,
    #[pyo3(get)]
    total_tokens: usize,
    #[pyo3(get)]
    verification_status: String,
    #[pyo3(get)]
    pending_verification_count: usize,
    #[pyo3(get)]
    failed_verification_count: usize,
    #[pyo3(get)]
    verification_report_count: usize,
    #[pyo3(get)]
    verification_summary_json: String,
    #[pyo3(get)]
    verification_summary_text: String,
}

#[pymethods]
impl PyAgentResult {
    fn __repr__(&self) -> String {
        format!(
            "AgentResult(text={:?}, tool_calls={}, tokens={}, verification={})",
            if self.text.len() > 80 {
                format!("{}...", truncate_utf8(&self.text, 80))
            } else {
                self.text.clone()
            },
            self.tool_calls_count,
            self.total_tokens,
            self.verification_status,
        )
    }

    fn __str__(&self) -> &str {
        &self.text
    }
}

impl From<RustAgentResult> for PyAgentResult {
    fn from(r: RustAgentResult) -> Self {
        let verification_summary = r.verification_summary();
        let verification_summary_json = verification_summary.to_value().to_string();
        let verification_summary_text = rust_format_verification_summary(&verification_summary);
        Self {
            text: r.text,
            tool_calls_count: r.tool_calls_count,
            prompt_tokens: r.usage.prompt_tokens,
            completion_tokens: r.usage.completion_tokens,
            total_tokens: r.usage.total_tokens,
            verification_status: verification_status_label(verification_summary.status),
            pending_verification_count: verification_summary.pending_required_check_count,
            failed_verification_count: verification_summary.failed_check_count,
            verification_report_count: verification_summary.report_count,
            verification_summary_json,
            verification_summary_text,
        }
    }
}

fn verification_status_label(status: RustVerificationStatus) -> String {
    match status {
        RustVerificationStatus::Passed => "passed",
        RustVerificationStatus::Failed => "failed",
        RustVerificationStatus::NeedsReview => "needs_review",
        RustVerificationStatus::Skipped => "skipped",
    }
    .to_string()
}

#[pyfunction]
fn format_verification_summary(py: Python<'_>, summary: &Bound<'_, PyAny>) -> PyResult<String> {
    let summary_json = if let Ok(summary_json) = summary.extract::<String>() {
        summary_json
    } else {
        let json_mod = py.import("json")?;
        json_mod.call_method1("dumps", (summary,))?.extract()?
    };
    let summary: RustVerificationSummary = serde_json::from_str(&summary_json)
        .map_err(|e| PyValueError::new_err(format!("Invalid verification summary: {e}")))?;
    Ok(rust_format_verification_summary(&summary))
}

// ============================================================================
// BtwResult
// ============================================================================

/// Result of a `/btw` ephemeral side question.
///
/// The answer is never added to conversation history.
#[pyclass(name = "BtwResult")]
#[derive(Clone)]
struct PyBtwResult {
    #[pyo3(get)]
    question: String,
    #[pyo3(get)]
    answer: String,
    #[pyo3(get)]
    prompt_tokens: usize,
    #[pyo3(get)]
    completion_tokens: usize,
    #[pyo3(get)]
    total_tokens: usize,
}

#[pymethods]
impl PyBtwResult {
    fn __repr__(&self) -> String {
        format!(
            "BtwResult(question={:?}, answer={:?}, tokens={})",
            self.question,
            if self.answer.len() > 60 {
                format!("{}...", truncate_utf8(&self.answer, 60))
            } else {
                self.answer.clone()
            },
            self.total_tokens,
        )
    }

    fn __str__(&self) -> &str {
        &self.answer
    }
}

impl From<RustBtwResult> for PyBtwResult {
    fn from(r: RustBtwResult) -> Self {
        Self {
            question: r.question,
            answer: r.answer,
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
    #[pyo3(get)]
    event_type: String,
    #[pyo3(get)]
    text: Option<String>,
    #[pyo3(get)]
    tool_name: Option<String>,
    #[pyo3(get)]
    tool_id: Option<String>,
    #[pyo3(get)]
    tool_output: Option<String>,
    #[pyo3(get)]
    exit_code: Option<i32>,
    #[pyo3(get)]
    turn: Option<usize>,
    #[pyo3(get)]
    prompt: Option<String>,
    #[pyo3(get)]
    error: Option<String>,
    #[pyo3(get)]
    total_tokens: Option<usize>,
    #[pyo3(get)]
    verification_summary_json: Option<String>,
    #[pyo3(get)]
    verification_summary_text: Option<String>,
    /// For btw_answer event: the original question
    #[pyo3(get)]
    question: Option<String>,
    /// For btw_answer event: the LLM's answer
    #[pyo3(get)]
    answer: Option<String>,
    /// Extra data for events that don't map to standard fields (JSON-encoded)
    #[pyo3(get)]
    data: Option<String>,
}

impl PyAgentEvent {
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
            verification_summary_json: None,
            verification_summary_text: None,
            question: None,
            answer: None,
            data: None,
        }
    }
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
                prompt: Some(prompt),
                ..Self::empty("start")
            },
            RustAgentEvent::AgentModeChanged {
                mode,
                agent,
                description,
            } => Self {
                data: Some(serde_json::json!({
                    "mode": mode,
                    "agent": agent,
                    "description": description
                }).to_string()),
                ..Self::empty("agent_mode_changed")
            },
            RustAgentEvent::TurnStart { turn } => Self {
                turn: Some(turn),
                ..Self::empty("turn_start")
            },
            RustAgentEvent::TextDelta { text } => Self {
                text: Some(text),
                ..Self::empty("text_delta")
            },
            RustAgentEvent::ToolStart { id, name } => Self {
                tool_id: Some(id),
                tool_name: Some(name),
                ..Self::empty("tool_start")
            },
            RustAgentEvent::ToolInputDelta { delta } => Self {
                text: Some(delta),
                ..Self::empty("tool_input_delta")
            },
            RustAgentEvent::ToolEnd {
                id,
                name,
                output,
                exit_code,
                metadata: _,
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
            RustAgentEvent::TurnEnd { turn, usage } => Self {
                turn: Some(turn),
                total_tokens: Some(usage.total_tokens),
                ..Self::empty("turn_end")
            },
            RustAgentEvent::End {
                text,
                usage,
                verification_summary,
                ..
            } => Self {
                text: Some(text),
                total_tokens: Some(usage.total_tokens),
                verification_summary_text: Some(rust_format_verification_summary(
                    &verification_summary,
                )),
                verification_summary_json: Some(verification_summary.to_value().to_string()),
                ..Self::empty("end")
            },
            RustAgentEvent::Error { message } => Self {
                error: Some(message),
                ..Self::empty("error")
            },
            RustAgentEvent::ConfirmationRequired {
                tool_id,
                tool_name,
                args,
                timeout_ms,
            } => Self {
                tool_id: Some(tool_id),
                tool_name: Some(tool_name),
                data: Some(serde_json::json!({
                    "args": args,
                    "timeout_ms": timeout_ms
                }).to_string()),
                ..Self::empty("confirmation_required")
            },
            RustAgentEvent::ConfirmationReceived {
                tool_id,
                approved,
                reason,
            } => Self {
                tool_id: Some(tool_id),
                data: Some(serde_json::json!({
                    "approved": approved,
                    "reason": reason
                }).to_string()),
                ..Self::empty("confirmation_received")
            },
            RustAgentEvent::ConfirmationTimeout {
                tool_id,
                action_taken,
            } => Self {
                tool_id: Some(tool_id),
                data: Some(serde_json::json!({
                    "action_taken": action_taken
                }).to_string()),
                ..Self::empty("confirmation_timeout")
            },
            RustAgentEvent::ExternalTaskPending {
                task_id,
                session_id,
                command_type,
                payload,
                timeout_ms,
                ..
            } => Self {
                data: Some(serde_json::json!({
                    "task_id": task_id,
                    "session_id": session_id,
                    "command_type": command_type,
                    "payload": payload,
                    "timeout_ms": timeout_ms
                }).to_string()),
                ..Self::empty("external_task_pending")
            },
            RustAgentEvent::ExternalTaskCompleted {
                task_id,
                session_id,
                success,
            } => Self {
                data: Some(serde_json::json!({
                    "task_id": task_id,
                    "session_id": session_id,
                    "success": success
                }).to_string()),
                ..Self::empty("external_task_completed")
            },
            RustAgentEvent::PermissionDenied {
                tool_id,
                tool_name,
                args,
                reason,
            } => Self {
                tool_id: Some(tool_id),
                tool_name: Some(tool_name),
                data: Some(serde_json::json!({
                    "args": args,
                    "reason": reason
                }).to_string()),
                ..Self::empty("permission_denied")
            },
            RustAgentEvent::ContextResolving { providers } => Self {
                data: Some(serde_json::json!({ "providers": providers }).to_string()),
                ..Self::empty("context_resolving")
            },
            RustAgentEvent::ContextResolved {
                total_items,
                total_tokens,
            } => Self {
                data: Some(serde_json::json!({
                    "total_items": total_items,
                    "total_tokens": total_tokens
                }).to_string()),
                ..Self::empty("context_resolved")
            },
            RustAgentEvent::CommandDeadLettered {
                command_id,
                command_type,
                lane,
                error,
                attempts,
            } => Self {
                data: Some(serde_json::json!({
                    "command_id": command_id,
                    "command_type": command_type,
                    "lane": lane,
                    "error": error,
                    "attempts": attempts
                }).to_string()),
                ..Self::empty("command_dead_lettered")
            },
            RustAgentEvent::CommandRetry {
                command_id,
                command_type,
                lane,
                attempt,
                delay_ms,
            } => Self {
                data: Some(serde_json::json!({
                    "command_id": command_id,
                    "command_type": command_type,
                    "lane": lane,
                    "attempt": attempt,
                    "delay_ms": delay_ms
                }).to_string()),
                ..Self::empty("command_retry")
            },
            RustAgentEvent::QueueAlert {
                level,
                alert_type,
                message,
            } => Self {
                data: Some(serde_json::json!({
                    "level": level,
                    "alert_type": alert_type,
                    "message": message
                }).to_string()),
                ..Self::empty("queue_alert")
            },
            RustAgentEvent::TaskUpdated {
                session_id,
                tasks,
            } => Self {
                data: Some(serde_json::json!({
                    "session_id": session_id,
                    "tasks": tasks
                }).to_string()),
                ..Self::empty("task_updated")
            },
            RustAgentEvent::MemoryStored {
                memory_id,
                memory_type,
                importance,
                tags,
            } => Self {
                data: Some(serde_json::json!({
                    "memory_id": memory_id,
                    "memory_type": memory_type,
                    "importance": importance,
                    "tags": tags
                }).to_string()),
                ..Self::empty("memory_stored")
            },
            RustAgentEvent::MemoryRecalled {
                memory_id,
                content,
                relevance,
            } => Self {
                data: Some(serde_json::json!({
                    "memory_id": memory_id,
                    "content": content,
                    "relevance": relevance
                }).to_string()),
                ..Self::empty("memory_recalled")
            },
            RustAgentEvent::MemoriesSearched {
                query,
                tags,
                result_count,
            } => Self {
                data: Some(serde_json::json!({
                    "query": query,
                    "tags": tags,
                    "result_count": result_count
                }).to_string()),
                ..Self::empty("memories_searched")
            },
            RustAgentEvent::MemoryCleared { tier, count } => Self {
                data: Some(serde_json::json!({ "tier": tier, "count": count }).to_string()),
                ..Self::empty("memory_cleared")
            },
            RustAgentEvent::SubagentStart {
                task_id,
                session_id,
                parent_session_id: _,
                agent,
                description,
            } => Self {
                tool_id: Some(task_id),
                tool_name: Some(agent),
                text: Some(session_id),
                prompt: Some(description),
                ..Self::empty("subagent_start")
            },
            RustAgentEvent::SubagentProgress {
                task_id,
                session_id,
                status,
                metadata: _,
            } => Self {
                tool_id: Some(task_id),
                text: Some(format!("{}: {}", session_id, status)),
                ..Self::empty("subagent_progress")
            },
            RustAgentEvent::SubagentEnd {
                task_id,
                session_id,
                agent,
                output,
                success,
            } => Self {
                tool_id: Some(task_id),
                tool_name: Some(agent),
                text: Some(session_id),
                tool_output: Some(output),
                exit_code: Some(if success { 0 } else { 1 }),
                ..Self::empty("subagent_end")
            },
            RustAgentEvent::PlanningStart { prompt } => Self {
                prompt: Some(prompt),
                ..Self::empty("planning_start")
            },
            RustAgentEvent::PlanningEnd {
                plan,
                estimated_steps,
            } => Self {
                data: Some(serde_json::json!({
                    "plan": plan,
                    "estimated_steps": estimated_steps
                }).to_string()),
                ..Self::empty("planning_end")
            },
            RustAgentEvent::StepStart {
                step_id,
                description,
                step_number,
                total_steps,
            } => Self {
                data: Some(serde_json::json!({
                    "step_id": step_id,
                    "description": description,
                    "step_number": step_number,
                    "total_steps": total_steps
                }).to_string()),
                ..Self::empty("step_start")
            },
            RustAgentEvent::StepEnd {
                step_id,
                status,
                step_number,
                total_steps,
            } => Self {
                data: Some(serde_json::json!({
                    "step_id": step_id,
                    "status": status,
                    "step_number": step_number,
                    "total_steps": total_steps
                }).to_string()),
                ..Self::empty("step_end")
            },
            RustAgentEvent::ContextCompacted {
                session_id,
                before_messages,
                after_messages,
                percent_before,
            } => Self {
                data: Some(serde_json::json!({
                    "session_id": session_id,
                    "before_messages": before_messages,
                    "after_messages": after_messages,
                    "percent_before": percent_before
                }).to_string()),
                ..Self::empty("context_compacted")
            },
            RustAgentEvent::PersistenceFailed {
                session_id,
                operation,
                error,
            } => Self {
                data: Some(serde_json::json!({
                    "session_id": session_id,
                    "operation": operation,
                    "error": error
                }).to_string()),
                ..Self::empty("persistence_failed")
            },
            RustAgentEvent::BtwAnswer {
                question,
                answer,
                usage,
            } => Self {
                question: Some(question),
                answer: Some(answer),
                total_tokens: Some(usage.total_tokens),
                ..Self::empty("btw_answer")
            },
            _ => Self::empty("unknown"),
        }
    }
}

// ============================================================================
// ToolResult
// ============================================================================

#[pyclass(name = "AgenticSearchScore")]
#[derive(Clone)]
struct PyAgenticSearchScore {
    #[pyo3(get)]
    base: Option<f32>,
    #[pyo3(get)]
    path_signal: Option<f32>,
    #[pyo3(get)]
    idf_boost: Option<f32>,
    #[pyo3(get)]
    file_type_boost: Option<f32>,
    #[pyo3(get)]
    unique_keywords_matched: Option<usize>,
}

impl PyAgenticSearchScore {
    fn from_json(value: &serde_json::Value) -> Self {
        Self {
            base: value.get("base").and_then(|v| v.as_f64()).map(|v| v as f32),
            path_signal: value
                .get("path_signal")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32),
            idf_boost: value
                .get("idf_boost")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32),
            file_type_boost: value
                .get("file_type_boost")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32),
            unique_keywords_matched: value
                .get("unique_keywords_matched")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
        }
    }
}

#[pymethods]
impl PyAgenticSearchScore {
    fn __repr__(&self) -> String {
        format!(
            "AgenticSearchScore(base={:?}, path_signal={:?}, idf_boost={:?}, file_type_boost={:?}, unique_keywords_matched={:?})",
            self.base,
            self.path_signal,
            self.idf_boost,
            self.file_type_boost,
            self.unique_keywords_matched
        )
    }
}

#[pyclass(name = "AgenticSearchMatch")]
#[derive(Clone)]
struct PyAgenticSearchMatch {
    #[pyo3(get)]
    line_number: Option<usize>,
    #[pyo3(get)]
    content: Option<String>,
    #[pyo3(get)]
    locator: Option<String>,
    #[pyo3(get)]
    context_before: Vec<String>,
    #[pyo3(get)]
    context_after: Vec<String>,
}

impl PyAgenticSearchMatch {
    fn from_json(value: &serde_json::Value) -> Self {
        Self {
            line_number: value
                .get("line_number")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            content: value
                .get("content")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            locator: value
                .get("locator")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            context_before: value
                .get("context_before")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            context_after: value
                .get("context_after")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

#[pymethods]
impl PyAgenticSearchMatch {
    fn __repr__(&self) -> String {
        format!(
            "AgenticSearchMatch(line_number={:?}, locator={:?}, content={:?})",
            self.line_number, self.locator, self.content
        )
    }
}

#[pyclass(name = "AgenticSearchSampledLine")]
#[derive(Clone)]
struct PyAgenticSearchSampledLine {
    #[pyo3(get)]
    line_number: Option<usize>,
    #[pyo3(get)]
    content: Option<String>,
    #[pyo3(get)]
    locator: Option<String>,
    #[pyo3(get)]
    distance: Option<usize>,
    #[pyo3(get)]
    weight: Option<f32>,
}

impl PyAgenticSearchSampledLine {
    fn from_json(value: &serde_json::Value) -> Self {
        Self {
            line_number: value
                .get("line_number")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            content: value
                .get("content")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            locator: value
                .get("locator")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            distance: value
                .get("distance")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            weight: value
                .get("weight")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32),
        }
    }
}

#[pymethods]
impl PyAgenticSearchSampledLine {
    fn __repr__(&self) -> String {
        format!(
            "AgenticSearchSampledLine(line_number={:?}, locator={:?}, distance={:?}, weight={:?})",
            self.line_number, self.locator, self.distance, self.weight
        )
    }
}

#[pyclass(name = "AgenticSearchResult")]
#[derive(Clone)]
struct PyAgenticSearchResult {
    #[pyo3(get)]
    path: Option<String>,
    #[pyo3(get)]
    file_type: Option<String>,
    #[pyo3(get)]
    relevance: Option<f32>,
    #[pyo3(get)]
    evidence_score: Option<f32>,
    #[pyo3(get)]
    match_count: Option<usize>,
    #[pyo3(get)]
    sampled_line_count: Option<usize>,
    #[pyo3(get)]
    score: Option<PyAgenticSearchScore>,
    #[pyo3(get)]
    matches: Vec<PyAgenticSearchMatch>,
    #[pyo3(get)]
    sampled_lines: Vec<PyAgenticSearchSampledLine>,
}

impl PyAgenticSearchResult {
    fn from_json(value: &serde_json::Value) -> Self {
        Self {
            path: value
                .get("path")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            file_type: value
                .get("file_type")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            relevance: value
                .get("relevance")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32),
            evidence_score: value
                .get("evidence_score")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32),
            match_count: value
                .get("match_count")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            sampled_line_count: value
                .get("sampled_line_count")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            score: value.get("score").map(PyAgenticSearchScore::from_json),
            matches: value
                .get("matches")
                .and_then(|v| v.as_array())
                .map(|items| items.iter().map(PyAgenticSearchMatch::from_json).collect())
                .unwrap_or_default(),
            sampled_lines: value
                .get("sampled_lines")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .map(PyAgenticSearchSampledLine::from_json)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

#[pymethods]
impl PyAgenticSearchResult {
    fn __repr__(&self) -> String {
        format!(
            "AgenticSearchResult(path={:?}, file_type={:?}, relevance={:?}, evidence_score={:?}, matches={})",
            self.path, self.file_type, self.relevance, self.evidence_score, self.matches.len()
        )
    }
}

#[pyclass(name = "AgenticParseLlmBlockLocation")]
#[derive(Clone)]
struct PyAgenticParseLlmBlockLocation {
    #[pyo3(get)]
    source: Option<String>,
    #[pyo3(get)]
    page: Option<usize>,
    #[pyo3(get)]
    ordinal: Option<usize>,
    #[pyo3(get)]
    display: Option<String>,
}

impl PyAgenticParseLlmBlockLocation {
    fn from_json(value: &serde_json::Value) -> Self {
        Self {
            source: value
                .get("source")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            page: value
                .get("page")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            ordinal: value
                .get("ordinal")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            display: value
                .get("display")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
        }
    }
}

#[pymethods]
impl PyAgenticParseLlmBlockLocation {
    fn __repr__(&self) -> String {
        format!(
            "AgenticParseLlmBlockLocation(source={:?}, page={:?}, ordinal={:?}, display={:?})",
            self.source, self.page, self.ordinal, self.display
        )
    }
}

#[pyclass(name = "AgenticParseLlmBlock")]
#[derive(Clone)]
struct PyAgenticParseLlmBlock {
    #[pyo3(get)]
    index: Option<usize>,
    #[pyo3(get)]
    kind: Option<String>,
    #[pyo3(get)]
    label: Option<String>,
    #[pyo3(get)]
    location: Option<PyAgenticParseLlmBlockLocation>,
}

impl PyAgenticParseLlmBlock {
    fn from_json(value: &serde_json::Value) -> Self {
        Self {
            index: value
                .get("index")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            kind: value
                .get("kind")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            label: value
                .get("label")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            location: value
                .get("location")
                .map(PyAgenticParseLlmBlockLocation::from_json),
        }
    }
}

#[pymethods]
impl PyAgenticParseLlmBlock {
    fn __repr__(&self) -> String {
        format!(
            "AgenticParseLlmBlock(index={:?}, kind={:?}, label={:?}, location={:?})",
            self.index,
            self.kind,
            self.label,
            self.location.as_ref().map(|loc| loc.display.clone())
        )
    }
}

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

    #[getter]
    fn agentic_search_results(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        let Some(json) = self.metadata_json.as_deref() else {
            return Ok(None);
        };
        let results = parse_agentic_search_results(json)?;
        let encoded = serde_json::to_string(&results).map_err(|e| {
            PyValueError::new_err(format!("Failed to serialize agentic_search results: {e}"))
        })?;
        Ok(Some(json_string_to_py(py, &encoded)?))
    }

    #[getter]
    fn agentic_search_results_info(&self) -> PyResult<Vec<PyAgenticSearchResult>> {
        let Some(json) = self.metadata_json.as_deref() else {
            return Ok(Vec::new());
        };
        let results = parse_agentic_search_results(json)?;
        Ok(results
            .iter()
            .map(PyAgenticSearchResult::from_json)
            .collect())
    }

    #[getter]
    fn agentic_parse_llm_blocks(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        let Some(json) = self.metadata_json.as_deref() else {
            return Ok(None);
        };
        let metadata: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| PyValueError::new_err(format!("Invalid tool metadata payload: {e}")))?;
        let blocks = metadata
            .get("llm_blocks")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new()));
        let encoded = serde_json::to_string(&blocks).map_err(|e| {
            PyValueError::new_err(format!("Failed to serialize agentic_parse llm_blocks: {e}"))
        })?;
        Ok(Some(json_string_to_py(py, &encoded)?))
    }

    #[getter]
    fn agentic_parse_llm_blocks_info(&self) -> PyResult<Vec<PyAgenticParseLlmBlock>> {
        let Some(json) = self.metadata_json.as_deref() else {
            return Ok(Vec::new());
        };
        let metadata: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| PyValueError::new_err(format!("Invalid tool metadata payload: {e}")))?;
        Ok(metadata
            .get("llm_blocks")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .map(PyAgenticParseLlmBlock::from_json)
                    .collect()
            })
            .unwrap_or_default())
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

/// One-shot callable used by `run_in_executor` for async iteration.
///
/// Each `__anext__` call creates a new instance; `__call__` blocks on the
/// next channel receive and raises `StopAsyncIteration` when done.
#[pyclass]
struct BlockingRecv {
    rx: Arc<Mutex<tokio::sync::mpsc::Receiver<RustAgentEvent>>>,
    done: Arc<AtomicBool>,
}

#[pymethods]
impl BlockingRecv {
    fn __call__(&self, py: Python<'_>) -> PyResult<PyAgentEvent> {
        let rx = self.rx.clone();
        let done_flag = self.done.clone();
        let result = py.allow_threads(|| {
            get_runtime().block_on(async {
                let mut guard = rx.lock().await;
                guard.recv().await
            })
        });
        match result {
            Some(event) => {
                let is_end = matches!(event, RustAgentEvent::End { .. });
                let is_error = matches!(event, RustAgentEvent::Error { .. });
                let py_event = PyAgentEvent::from(event);
                if is_end || is_error {
                    done_flag.store(true, Ordering::Relaxed);
                }
                Ok(py_event)
            }
            None => {
                done_flag.store(true, Ordering::Relaxed);
                Err(PyStopAsyncIteration::new_err("stream exhausted"))
            }
        }
    }
}

/// Iterator / async-iterator that yields AgentEvents from a streaming execution.
///
/// Sync usage:  `for event in session.stream(prompt):`
/// Async usage: `async for event in session.stream(prompt):`
#[pyclass(name = "EventStream")]
struct PyEventStream {
    rx: Arc<Mutex<tokio::sync::mpsc::Receiver<RustAgentEvent>>>,
    done: Arc<AtomicBool>,
}

#[pymethods]
impl PyEventStream {
    // ------------------------------------------------------------------
    // Sync iterator protocol
    // ------------------------------------------------------------------

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<PyAgentEvent>> {
        if self.done.load(Ordering::Relaxed) {
            return Err(PyStopIteration::new_err("stream exhausted"));
        }

        let rx = self.rx.clone();
        let done_flag = self.done.clone();
        let result = py.allow_threads(|| {
            get_runtime().block_on(async {
                let mut guard = rx.lock().await;
                guard.recv().await
            })
        });

        match result {
            Some(event) => {
                let is_end = matches!(event, RustAgentEvent::End { .. });
                let is_error = matches!(event, RustAgentEvent::Error { .. });
                let py_event = PyAgentEvent::from(event);
                if is_end || is_error {
                    done_flag.store(true, Ordering::Relaxed);
                }
                Ok(Some(py_event))
            }
            None => {
                done_flag.store(true, Ordering::Relaxed);
                Err(PyStopIteration::new_err("stream exhausted"))
            }
        }
    }

    // ------------------------------------------------------------------
    // Async iterator protocol
    // ------------------------------------------------------------------

    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    /// Returns an `asyncio.Future` that resolves to the next `AgentEvent`.
    ///
    /// Uses `run_in_executor` to bridge the blocking channel recv into an
    /// asyncio-compatible awaitable without requiring `pyo3-async`.
    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        if self.done.load(Ordering::Relaxed) {
            return Err(PyStopAsyncIteration::new_err("stream exhausted"));
        }

        let callable = Bound::new(
            py,
            BlockingRecv {
                rx: self.rx.clone(),
                done: self.done.clone(),
            },
        )?;

        let asyncio = py.import("asyncio")?;
        let loop_ = asyncio.call_method0("get_running_loop")?;
        let future = loop_.call_method1("run_in_executor", (py.None(), callable))?;
        Ok(future)
    }
}

// ============================================================================
// Agent
// ============================================================================

/// AI coding agent. Create with `Agent.create()`, then call `agent.session()`.
#[pyclass(name = "Agent")]
struct PyAgent {
    inner: Arc<RustAgent>,
}

#[pymethods]
impl PyAgent {
    /// Create an Agent from a config file path or inline config string.
    ///
    /// Accepts ACL-compatible config files (.acl) or inline config strings.
    /// JSON config is not supported.
    ///
    /// Args:
    ///     config_source: Path to a config file (.acl), or inline config string
    #[staticmethod]
    fn create(py: Python<'_>, config_source: String) -> PyResult<Self> {
        let agent = py
            .allow_threads(move || get_runtime().block_on(RustAgent::new(config_source)))
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create agent: {e}")))?;

        Ok(Self {
            inner: Arc::new(agent),
        })
    }

    /// Bind to a workspace directory, returning a Session.
    ///
    /// Args:
    ///     workspace: Path to the workspace directory
    ///     options: Optional SessionOptions object
    ///     model: Optional model override, format "provider/model" (e.g., "openai/gpt-4o")
    ///     builtin_skills: Optional bool to enable built-in skills (default: False)
    ///     skill_dirs: Optional list of directories to scan for skill files
    ///     agent_dirs: Optional list of directories to scan for agent files
    ///     queue_config: Optional advanced SessionQueueConfig for explicit external/hybrid lane dispatch
    ///     planning: Optional bool to enable planning mode (default: False)
    ///     goal_tracking: Optional bool to enable goal tracking (default: False)
    ///     max_parse_retries: Optional max consecutive parse errors before abort
    ///     tool_timeout_ms: Optional per-tool execution timeout in milliseconds
    ///     circuit_breaker_threshold: Optional max LLM API failures before abort
    /// Re-fetch tool definitions from all connected global MCP servers and
    /// update the agent-level cache.
    ///
    /// New sessions created after this call will see the refreshed tool list.
    /// Existing sessions are unaffected.
    fn refresh_mcp_tools(&self, py: Python<'_>) -> PyResult<()> {
        let agent = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async {
                agent
                    .refresh_mcp_tools()
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("refresh_mcp_tools failed: {e}")))
            })
        })
    }

    #[pyo3(signature = (workspace, options=None, model=None, builtin_skills=None, skill_dirs=None, agent_dirs=None, queue_config=None, planning=None, goal_tracking=None, max_parse_retries=None, tool_timeout_ms=None, circuit_breaker_threshold=None))]
    fn session(
        &self,
        workspace: String,
        options: Option<PySessionOptions>,
        model: Option<String>,
        builtin_skills: Option<bool>,
        skill_dirs: Option<Vec<String>>,
        agent_dirs: Option<Vec<String>>,
        queue_config: Option<PySessionQueueConfig>,
        planning: Option<bool>,
        goal_tracking: Option<bool>,
        max_parse_retries: Option<u32>,
        tool_timeout_ms: Option<u64>,
        circuit_breaker_threshold: Option<u32>,
    ) -> PyResult<PySession> {
        // If a SessionOptions object is provided, build from it then apply keyword overrides
        let opts = if let Some(so) = options {
            let mut o = build_rust_session_options(so)?;
            // Keyword args take precedence over SessionOptions fields
            if planning.unwrap_or(false) {
                o = o.with_planning(true);
            }
            if goal_tracking.unwrap_or(false) {
                o = o.with_goal_tracking(true);
            }
            if let Some(n) = max_parse_retries {
                o = o.with_parse_retries(n);
            }
            if let Some(ms) = tool_timeout_ms {
                o = o.with_tool_timeout(ms);
            }
            if let Some(n) = circuit_breaker_threshold {
                o = o.with_circuit_breaker(n);
            }
            Some(o)
        } else {
            // Fall back to individual keyword arguments
            let has_overrides = model.is_some()
                || builtin_skills.is_some()
                || skill_dirs.is_some()
                || agent_dirs.is_some()
                || queue_config.is_some()
                || planning.is_some()
                || goal_tracking.is_some()
                || max_parse_retries.is_some()
                || tool_timeout_ms.is_some()
                || circuit_breaker_threshold.is_some();

            if has_overrides {
                let mut o = RustSessionOptions::new();
                if let Some(m) = model {
                    o = o.with_model(m);
                }
                if builtin_skills.unwrap_or(false) {
                    o = o.with_builtin_skills();
                }
                if let Some(dirs) = skill_dirs {
                    for d in dirs {
                        o = o.with_skills_from_dir(d);
                    }
                }
                if let Some(dirs) = agent_dirs {
                    for d in dirs {
                        o = o.with_agent_dir(d);
                    }
                }
                if let Some(qc) = queue_config {
                    o = o.with_queue_config(qc.inner);
                }
                if planning.unwrap_or(false) {
                    o = o.with_planning(true);
                }
                if goal_tracking.unwrap_or(false) {
                    o = o.with_goal_tracking(true);
                }
                if let Some(n) = max_parse_retries {
                    o = o.with_parse_retries(n);
                }
                if let Some(ms) = tool_timeout_ms {
                    o = o.with_tool_timeout(ms);
                }
                if let Some(n) = circuit_breaker_threshold {
                    o = o.with_circuit_breaker(n);
                }
                Some(o)
            } else {
                None
            }
        };

        let session = self
            .inner
            .session(workspace, opts)
            .map_err(|e| PyRuntimeError::new_err(format!("{e}")))?;
        Ok(PySession {
            inner: Arc::new(session),
        })
    }

    fn __repr__(&self) -> String {
        "Agent(...)".to_string()
    }

    /// Resume a previously saved session by ID.
    ///
    /// ``options.session_store`` must point to the store where the session was saved.
    ///
    /// .. code-block:: python
    ///
    ///     opts = SessionOptions()
    ///     opts.session_store = FileSessionStore('./sessions')
    ///     session = agent.resume_session('my-session', opts)
    ///
    /// Args:
    ///     session_id: The session ID to resume
    ///     options: SessionOptions with ``session_store`` set to the backing store
    #[pyo3(signature = (session_id, options))]
    fn resume_session(&self, session_id: String, options: PySessionOptions) -> PyResult<PySession> {
        let opts = build_rust_session_options(options)?;
        let session = self
            .inner
            .resume_session(&session_id, opts)
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to resume session: {e}")))?;
        Ok(PySession {
            inner: Arc::new(session),
        })
    }

    /// Create a session pre-configured from a named agent definition.
    ///
    /// Loads the agent by name from built-in agents and optionally from
    /// additional directories, then creates a session with the agent's
    /// permissions, system prompt, model, and step limit applied.
    ///
    /// Args:
    ///     workspace: Path to the workspace directory
    ///     agent_name: Name of the agent to load (e.g. "explore", "general")
    ///     agent_dirs: Optional list of directories to scan for agent files
    ///     options: Optional session overrides layered on top of the agent definition
    #[pyo3(signature = (workspace, agent_name, agent_dirs=None, options=None))]
    fn session_for_agent(
        &self,
        workspace: String,
        agent_name: String,
        agent_dirs: Option<Vec<String>>,
        options: Option<PySessionOptions>,
    ) -> PyResult<PySession> {
        let registry = a3s_code_core::subagent::AgentRegistry::new();
        for dir in agent_dirs.unwrap_or_default() {
            let agents = a3s_code_core::subagent::load_agents_from_dir(std::path::Path::new(&dir));
            for agent in agents {
                registry.register(agent);
            }
        }
        let def = registry
            .get(&agent_name)
            .ok_or_else(|| PyRuntimeError::new_err(format!("agent '{}' not found", agent_name)))?;
        let opts = options.map(build_rust_session_options).transpose()?;
        let session = self
            .inner
            .session_for_agent(workspace, &def, opts)
            .map_err(|e| PyRuntimeError::new_err(format!("{e}")))?;
        Ok(PySession {
            inner: Arc::new(session),
        })
    }
}

// ============================================================================
// Session
// ============================================================================

/// Workspace-bound session. All LLM and tool operations happen here.
#[pyclass(name = "Session")]
struct PySession {
    inner: Arc<RustAgentSession>,
}

#[pymethods]
impl PySession {
    /// Send a prompt and wait for the complete response.
    ///
    /// Args:
    ///     prompt: The prompt to send
    ///     history: Optional conversation history as list of dicts
    ///              `[{"role": "user", "content": [{"type": "text", "text": "..."}]}]`
    #[pyo3(signature = (prompt, history=None))]
    fn send(
        &self,
        py: Python<'_>,
        prompt: String,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyAgentResult> {
        let rust_history = history.map(|h| py_list_to_messages(h)).transpose()?;
        let session = self.inner.clone();
        let result = py
            .allow_threads(move || {
                get_runtime().block_on(session.send(&prompt, rust_history.as_deref()))
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Agent execution failed: {e}")))?;
        Ok(PyAgentResult::from(result))
    }

    /// Send a prompt and get a streaming iterator of events.
    ///
    /// When ``history`` is omitted, session history and verification evidence are
    /// updated after the stream completes. Supplying ``history`` keeps the stream isolated.
    ///
    /// Args:
    ///     prompt: The prompt to send
    ///     history: Optional conversation history (same format as send)
    #[pyo3(signature = (prompt, history=None))]
    fn stream(
        &self,
        py: Python<'_>,
        prompt: String,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyEventStream> {
        let rust_history = history.map(|h| py_list_to_messages(h)).transpose()?;
        let session = self.inner.clone();
        let (rx, _handle) = py
            .allow_threads(move || {
                get_runtime().block_on(session.stream(&prompt, rust_history.as_deref()))
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to start stream: {e}")))?;

        Ok(PyEventStream {
            rx: Arc::new(Mutex::new(rx)),
            done: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Send a prompt with image attachments and wait for the complete response.
    ///
    /// Args:
    ///     prompt: The prompt to send
    ///     attachments: List of dicts with `{"data": bytes, "media_type": str}`
    ///     history: Optional conversation history
    #[pyo3(signature = (prompt, attachments, history=None))]
    fn send_with_attachments(
        &self,
        py: Python<'_>,
        prompt: String,
        attachments: Vec<Bound<'_, PyDict>>,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyAgentResult> {
        let rust_attachments = py_attachments_to_rust(&attachments)?;
        let rust_history = history.map(|h| py_list_to_messages(h)).transpose()?;
        let session = self.inner.clone();
        let result = py
            .allow_threads(move || {
                get_runtime().block_on(session.send_with_attachments(
                    &prompt,
                    &rust_attachments,
                    rust_history.as_deref(),
                ))
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Agent execution failed: {e}")))?;
        Ok(PyAgentResult::from(result))
    }

    /// Stream a prompt with image attachments.
    ///
    /// When ``history`` is omitted, session history and verification evidence are
    /// updated after the stream completes. Supplying ``history`` keeps the stream isolated.
    ///
    /// Args:
    ///     prompt: The prompt to send
    ///     attachments: List of dicts with `{"data": bytes, "media_type": str}`
    ///     history: Optional conversation history
    #[pyo3(signature = (prompt, attachments, history=None))]
    fn stream_with_attachments(
        &self,
        py: Python<'_>,
        prompt: String,
        attachments: Vec<Bound<'_, PyDict>>,
        history: Option<&Bound<'_, PyList>>,
    ) -> PyResult<PyEventStream> {
        let rust_attachments = py_attachments_to_rust(&attachments)?;
        let rust_history = history.map(|h| py_list_to_messages(h)).transpose()?;
        let session = self.inner.clone();
        let (rx, _handle) = py
            .allow_threads(move || {
                get_runtime().block_on(session.stream_with_attachments(
                    &prompt,
                    &rust_attachments,
                    rust_history.as_deref(),
                ))
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to start stream: {e}")))?;
        Ok(PyEventStream {
            rx: Arc::new(Mutex::new(rx)),
            done: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Ask an ephemeral side question without affecting conversation history.
    ///
    /// Args:
    ///     question: The question to ask
    ///
    /// Returns:
    ///     BtwResult with question, answer, and usage
    fn btw(&self, py: Python<'_>, question: String) -> PyResult<PyBtwResult> {
        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.btw(&question)))
            .map_err(|e| PyRuntimeError::new_err(format!("btw query failed: {e}")))?;
        Ok(PyBtwResult::from(result))
    }

    /// Return the session's conversation history as a list of dicts.
    ///
    /// Each dict has `{"role": str, "content": [{"type": "text", "text": str}, ...]}`.
    fn history<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let messages = self.inner.history();
        messages_to_py_list(py, &messages)
    }

    /// Execute a tool by name, bypassing the LLM.
    fn tool(
        &self,
        py: Python<'_>,
        name: String,
        args: &Bound<'_, pyo3::types::PyDict>,
    ) -> PyResult<PyToolResult> {
        let json_str = py_dict_to_json(args)?;
        let json_value: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| PyValueError::new_err(format!("Invalid JSON args: {e}")))?;

        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool(&name, json_value)))
            .map_err(|e| PyRuntimeError::new_err(format!("Tool execution failed: {e}")))?;

        Ok(PyToolResult {
            name: result.name,
            output: result.output,
            exit_code: result.exit_code,
            metadata_json: result.metadata.as_ref().map(serde_json::Value::to_string),
        })
    }

    /// Read a file from the workspace.
    fn read_file(&self, py: Python<'_>, path: String) -> PyResult<String> {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.read_file(&path)))
            .map_err(|e| PyRuntimeError::new_err(format!("{e}")))
    }

    /// Execute a bash command in the workspace.
    fn bash(&self, py: Python<'_>, command: String) -> PyResult<String> {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.bash(&command)))
            .map_err(|e| PyRuntimeError::new_err(format!("{e}")))
    }

    /// Search for files matching a glob pattern.
    fn glob(&self, py: Python<'_>, pattern: String) -> PyResult<Vec<String>> {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.glob(&pattern)))
            .map_err(|e| PyRuntimeError::new_err(format!("{e}")))
    }

    /// Search file contents with a regex pattern.
    fn grep(&self, py: Python<'_>, pattern: String) -> PyResult<String> {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.grep(&pattern)))
            .map_err(|e| PyRuntimeError::new_err(format!("{e}")))
    }

    /// Search the web using multiple search engines.
    fn web_search(&self, py: Python<'_>, params: PyWebSearchParams) -> PyResult<PyToolResult> {
        let session = self.inner.clone();
        let mut args = serde_json::json!({
            "query": params.query,
        });
        if let Some(ref engines) = params.engines {
            args["engines"] = serde_json::json!(engines);
        }
        if let Some(limit) = params.limit {
            args["limit"] = serde_json::json!(limit);
        }
        if let Some(timeout) = params.timeout {
            args["timeout"] = serde_json::json!(timeout);
        }
        if let Some(ref proxy) = params.proxy {
            args["proxy"] = serde_json::json!(proxy);
        }
        if let Some(ref format) = params.format {
            args["format"] = serde_json::json!(format);
        }
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool("web_search", args)))
            .map_err(|e| PyRuntimeError::new_err(format!("Tool execution failed: {e}")))?;
        Ok(PyToolResult {
            name: result.name,
            output: result.output,
            exit_code: result.exit_code,
            metadata_json: result.metadata.as_ref().map(serde_json::Value::to_string),
        })
    }

    /// Execute a git command (status, log, branch, checkout, diff, stash, remote, worktree).
    ///
    /// For worktree subcommands, use `subcommand` ("list", "create", "remove") and
    /// related params (`name`, `path`, `new_branch`, `base`, `force`).
    #[pyo3(signature = (command, subcommand=None, name=None, path=None, new_branch=true, base=None, force=false, max_count=None, message=None, include_untracked=false, target=None, reference=None))]
    fn git(
        &self,
        py: Python<'_>,
        command: String,
        subcommand: Option<String>,
        name: Option<String>,
        path: Option<String>,
        new_branch: bool,
        base: Option<String>,
        force: bool,
        max_count: Option<usize>,
        message: Option<String>,
        include_untracked: bool,
        target: Option<String>,
        reference: Option<String>,
    ) -> PyResult<PyToolResult> {
        let mut args = serde_json::json!({
            "command": command,
        });
        if let Some(sc) = subcommand {
            args["subcommand"] = serde_json::json!(sc);
        }
        if let Some(n) = name {
            args["name"] = serde_json::json!(n);
        }
        if let Some(p) = path {
            args["path"] = serde_json::json!(p);
        }
        if !new_branch {
            args["new_branch"] = serde_json::json!(new_branch);
        }
        if let Some(b) = base {
            args["base"] = serde_json::json!(b);
        }
        if force {
            args["force"] = serde_json::json!(force);
        }
        if let Some(mc) = max_count {
            args["max_count"] = serde_json::json!(mc);
        }
        if let Some(msg) = message {
            args["message"] = serde_json::json!(msg);
        }
        if include_untracked {
            args["include_untracked"] = serde_json::json!(include_untracked);
        }
        if let Some(t) = target {
            args["target"] = serde_json::json!(t);
        }
        if let Some(r) = reference {
            args["ref"] = serde_json::json!(r);
        }

        let session = self.inner.clone();
        let result = py
            .allow_threads(move || get_runtime().block_on(session.tool("git", args)))
            .map_err(|e| PyRuntimeError::new_err(format!("git failed: {e}")))?;
        Ok(PyToolResult {
            name: result.name,
            output: result.output,
            exit_code: result.exit_code,
            metadata_json: result.metadata.as_ref().map(serde_json::Value::to_string),
        })
    }

    // ========================================================================
    // Advanced optional Queue API
    // ========================================================================

    /// Check if this session has an advanced lane queue configured.
    fn has_queue(&self) -> bool {
        self.inner.has_queue()
    }

    /// Configure a lane's handler mode for explicit external/hybrid dispatch.
    ///
    /// Args:
    ///     lane (Literal["control", "query", "execute", "generate"]): Which lane to configure.
    ///     mode (Literal["internal", "external", "hybrid"]): Execution mode for the lane's tools.
    ///     timeout_ms: Timeout for external processing in milliseconds (default 60000).
    #[pyo3(signature = (lane, mode="internal", timeout_ms=60000))]
    fn set_lane_handler(
        &self,
        py: Python<'_>,
        lane: &str,
        mode: &str,
        timeout_ms: u64,
    ) -> PyResult<()> {
        let lane = parse_lane(lane)?;
        let mode = parse_handler_mode(mode)?;
        let config = RustLaneHandlerConfig { mode, timeout_ms };
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.set_lane_handler(lane, config)));
        Ok(())
    }

    /// Complete an external queue task by ID.
    ///
    /// Args:
    ///     task_id: The task identifier
    ///     success: Whether the task succeeded
    ///     result: Result data (any JSON-serializable value)
    ///     error: Optional error message
    ///
    /// Returns:
    ///     True if the task was found and completed, False if not found.
    #[pyo3(signature = (task_id, success=true, result=None, error=None))]
    fn complete_external_task(
        &self,
        py: Python<'_>,
        task_id: String,
        success: bool,
        result: Option<&Bound<'_, PyDict>>,
        error: Option<String>,
    ) -> PyResult<bool> {
        let result_value = match result {
            Some(dict) => {
                let json_str = py_dict_to_json(dict)?;
                serde_json::from_str(&json_str)
                    .map_err(|e| PyValueError::new_err(format!("Invalid JSON: {e}")))?
            }
            None => serde_json::json!({}),
        };
        let ext_result = RustExternalTaskResult {
            success,
            result: result_value,
            error,
        };
        let session = self.inner.clone();
        let found = py.allow_threads(move || {
            get_runtime().block_on(session.complete_external_task(&task_id, ext_result))
        });
        Ok(found)
    }

    /// Get pending external queue tasks.
    ///
    /// Returns:
    ///     List of dicts with task_id, session_id, lane, command_type, payload, timeout_ms.
    fn pending_external_tasks<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let session = self.inner.clone();
        let tasks =
            py.allow_threads(move || get_runtime().block_on(session.pending_external_tasks()));
        let json_str = serde_json::to_string(&tasks)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .map(|l| l.clone())
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get optional queue statistics.
    ///
    /// Returns:
    ///     Dict with total_pending, total_active, external_pending, and per-lane status.
    fn queue_stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let session = self.inner.clone();
        let stats = py.allow_threads(move || get_runtime().block_on(session.queue_stats()));
        let json_str = serde_json::to_string(&stats)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyDict>()
            .map(|d| d.clone())
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get dead letters from the optional queue's DLQ.
    ///
    /// Returns:
    ///     List of dicts with command_id, command_type, lane, error, attempts, failed_at.
    fn dead_letters<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let session = self.inner.clone();
        let letters = py.allow_threads(move || get_runtime().block_on(session.dead_letters()));
        let json_str = serde_json::to_string(&letters)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .map(|l| l.clone())
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get a detailed metrics snapshot from the queue.
    ///
    /// Returns ``None`` if metrics are not enabled (queue not configured or
    /// ``enable_metrics`` was not set in ``SessionQueueConfig``).
    ///
    /// Returns:
    ///     Dict with ``counters``, ``gauges``, and ``histograms`` maps, or None.
    fn queue_metrics<'py>(&self, py: Python<'py>) -> PyResult<PyObject> {
        let session = self.inner.clone();
        let snapshot = py.allow_threads(move || get_runtime().block_on(session.queue_metrics()));
        match snapshot {
            None => Ok(py.None()),
            Some(s) => {
                let json_str = metrics_snapshot_to_json_str(s)
                    .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
                let json_mod = py.import("json")?;
                Ok(json_mod.call_method1("loads", (json_str,))?.into())
            }
        }
    }

    /// Add an MCP server to this live session.
    ///
    /// Connects the server and registers all its tools immediately so the agent
    /// can call them. Tool names follow the convention ``mcp__<name>__<tool>``.
    ///
    /// Args:
    ///     name: Server identifier (used as prefix in tool names)
    ///     transport: Transport type — ``"stdio"`` (default), ``"http"``, or ``"streamable-http"``
    ///     command: Executable to launch (stdio only, e.g. ``"npx"``)
    ///     args: Arguments for the command (stdio only)
    ///     url: Server URL (http / streamable-http only)
    ///     headers: HTTP headers dict (http / streamable-http only, e.g. ``{"Authorization": "Bearer ..."}``))
    ///     env: Optional dict of extra environment variables (stdio only)
    ///
    /// Returns:
    ///     Number of tools registered from the server
    ///
    /// Raises:
    ///     RuntimeError: If the server fails to connect
    #[pyo3(signature = (name, transport="stdio", command=None, args=None, url=None, headers=None, env=None, timeout_ms=None))]
    fn add_mcp_server(
        &self,
        py: Python<'_>,
        name: &str,
        transport: &str,
        command: Option<&str>,
        args: Option<Vec<String>>,
        url: Option<&str>,
        headers: Option<std::collections::HashMap<String, String>>,
        env: Option<std::collections::HashMap<String, String>>,
        timeout_ms: Option<u64>,
    ) -> PyResult<usize> {
        use a3s_code_core::mcp::protocol::{McpServerConfig, McpTransportConfig};

        let transport_config = match transport {
            "stdio" => {
                let command = command.ok_or_else(|| {
                    PyRuntimeError::new_err("'command' is required for stdio transport")
                })?;
                McpTransportConfig::Stdio {
                    command: command.to_string(),
                    args: args.unwrap_or_default(),
                }
            }
            "http" => {
                let url = url.ok_or_else(|| {
                    PyRuntimeError::new_err("'url' is required for http transport")
                })?;
                McpTransportConfig::Http {
                    url: url.to_string(),
                    headers: headers.unwrap_or_default(),
                }
            }
            "streamable-http" | "streamable_http" => {
                let url = url.ok_or_else(|| {
                    PyRuntimeError::new_err("'url' is required for streamable-http transport")
                })?;
                McpTransportConfig::StreamableHttp {
                    url: url.to_string(),
                    headers: headers.unwrap_or_default(),
                }
            }
            other => {
                return Err(PyRuntimeError::new_err(format!(
                    "Unknown transport '{}'. Use 'stdio', 'http', or 'streamable-http'",
                    other
                )))
            }
        };

        let tool_timeout_secs = timeout_ms.map(|ms| (ms / 1000).max(1)).unwrap_or(60);
        let config = McpServerConfig {
            name: name.to_string(),
            transport: transport_config,
            enabled: true,
            env: env.unwrap_or_default(),
            oauth: None,
            tool_timeout_secs,
        };
        let session = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async {
                session
                    .add_mcp_server(config)
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("add_mcp_server failed: {e}")))
            })
        })
    }

    /// Dynamically register agents from a directory with the live session.
    ///
    /// Scans the given directory for ``*.yaml``, ``*.yml``, and ``*.md`` agent
    /// definition files and adds each to the shared agent registry used by the
    /// ``task`` tool.  New agents become immediately callable via
    /// ``task(agent="…")`` without restarting the session.
    ///
    /// Args:
    ///     path: Directory path to scan for agent definition files
    ///
    /// Returns:
    ///     Number of agents successfully loaded from the directory
    #[pyo3(signature = (path))]
    fn register_agent_dir(&self, py: Python<'_>, path: &str) -> PyResult<usize> {
        let dir = std::path::PathBuf::from(path);
        let session = self.inner.clone();
        py.allow_threads(move || {
            let count = session.register_agent_dir(&dir);
            Ok(count)
        })
    }

    /// Remove an MCP server from this session.
    ///
    /// Disconnects the server and unregisters all its tools.
    /// No-op if the server was never added.
    ///
    /// Args:
    ///     name: Server identifier used when it was added
    #[pyo3(signature = (name))]
    fn remove_mcp_server(&self, py: Python<'_>, name: &str) -> PyResult<()> {
        let name = name.to_string();
        let session = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async {
                session
                    .remove_mcp_server(&name)
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("remove_mcp_server failed: {e}")))
            })
        })
    }

    /// Return the connection status of all MCP servers for this session.
    ///
    /// Returns:
    ///     Dict mapping server name to status dict with keys:
    ///     ``connected`` (bool), ``tool_count`` (int).
    fn mcp_status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let session = self.inner.clone();
        let status = py.allow_threads(move || get_runtime().block_on(session.mcp_status()));
        let dict = PyDict::new(py);
        for (name, s) in status {
            let entry = PyDict::new(py);
            entry.set_item("connected", s.connected)?;
            entry.set_item("tool_count", s.tool_count)?;
            entry.set_item("error", s.error.as_deref())?;
            dict.set_item(name, entry)?;
        }
        Ok(dict)
    }

    /// Return the names of all tools currently available in this session.
    ///
    /// Reflects the live state — MCP tools appear after ``add_mcp_server()``
    /// and disappear after ``remove_mcp_server()``.
    ///
    /// Returns:
    ///     List of tool name strings
    fn tool_names<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let names = self.inner.tool_names();
        let list = PyList::new(py, names)?;
        Ok(list)
    }

    /// Return compact execution trace events recorded for this session.
    fn trace_events(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.inner.trace_events())
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to serialize traces: {e}")))?;
        json_string_to_py(py, &json)
    }

    /// Return structured verification reports recorded for this session.
    fn verification_reports(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.inner.verification_reports()).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize verification reports: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Return a structured verification summary for this session.
    fn verification_summary(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.inner.verification_summary()).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize verification summary: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Return a concise human-readable verification summary for this session.
    fn verification_summary_text(&self) -> String {
        self.inner.verification_summary_text()
    }

    /// Run verification commands and return a structured verification report.
    fn verify_commands(
        &self,
        py: Python<'_>,
        subject: &str,
        commands: &Bound<'_, PyList>,
    ) -> PyResult<PyObject> {
        let rust_commands = py_list_to_verification_commands(commands)?;
        let session = self.inner.clone();
        let subject = subject.to_string();
        let report = py
            .allow_threads(move || {
                get_runtime().block_on(session.verify_commands(&subject, &rust_commands))
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Verification failed: {e}")))?;
        let json = serde_json::to_string(&report).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize verification report: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    /// Return project-aware verification command presets for this workspace.
    fn verification_presets(&self, py: Python<'_>) -> PyResult<PyObject> {
        let json = serde_json::to_string(&self.inner.verification_presets()).map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to serialize verification presets: {e}"))
        })?;
        json_string_to_py(py, &json)
    }

    // ========================================================================
    // Hook API
    // ========================================================================

    /// Register a hook for lifecycle event interception.
    ///
    /// Hooks registered on a session are automatically propagated to all sub-agents
    /// spawned by the `task` tool, including grandchild agents at arbitrary depth.
    /// This ensures security hooks (e.g. a sentinel) apply across the full agent tree
    /// without requiring explicit registration on each sub-agent session.
    ///
    /// Args:
    ///     hook_id: Unique hook identifier
    ///     event_type: Event type string — one of:
    ///         "pre_tool_use", "post_tool_use", "generate_start", "generate_end",
    ///         "session_start", "session_end", "skill_load", "skill_unload",
    ///         "pre_prompt", "post_response", "on_error"
    ///     matcher: Optional dict with keys: tool, path_pattern, command_pattern, session_id, skill
    ///     config: Optional dict with keys: priority, timeout_ms, async_execution, max_retries
    ///     handler: Optional callable ``(event: dict) -> dict | None``. When provided, it is called
    ///         for every matching event and its return value controls execution:
    ///         ``{"action": "block", "reason": "…"}`` cancels the operation,
    ///         ``{"action": "skip"}`` skips remaining hooks, ``None`` or
    ///         ``{"action": "continue"}`` allows execution to proceed.
    #[pyo3(signature = (hook_id, event_type, matcher=None, config=None, handler=None))]
    fn register_hook(
        &self,
        hook_id: String,
        event_type: String,
        matcher: Option<&Bound<'_, PyDict>>,
        config: Option<&Bound<'_, PyDict>>,
        handler: Option<pyo3::Py<pyo3::PyAny>>,
    ) -> PyResult<()> {
        let rust_event_type = py_parse_hook_event_type(&event_type)?;
        let mut hook = RustHook::new(&hook_id, rust_event_type);

        if let Some(m) = matcher {
            let mut rust_matcher = RustHookMatcher::new();
            if let Some(tool) = m.get_item("tool")? {
                rust_matcher = rust_matcher.with_tool(tool.extract::<String>()?);
            }
            if let Some(path) = m.get_item("path_pattern")? {
                rust_matcher = rust_matcher.with_path(path.extract::<String>()?);
            }
            if let Some(cmd) = m.get_item("command_pattern")? {
                rust_matcher = rust_matcher.with_command(cmd.extract::<String>()?);
            }
            if let Some(sid) = m.get_item("session_id")? {
                rust_matcher = rust_matcher.with_session(sid.extract::<String>()?);
            }
            if let Some(skill) = m.get_item("skill")? {
                rust_matcher = rust_matcher.with_skill(skill.extract::<String>()?);
            }
            hook = hook.with_matcher(rust_matcher);
        }

        if let Some(c) = config {
            let priority = c
                .get_item("priority")?
                .map(|v| v.extract::<i32>())
                .transpose()?
                .unwrap_or(100);
            let timeout_ms = c
                .get_item("timeout_ms")?
                .map(|v| v.extract::<u64>())
                .transpose()?
                .unwrap_or(30000);
            let async_execution = c
                .get_item("async_execution")?
                .map(|v| v.extract::<bool>())
                .transpose()?
                .unwrap_or(false);
            let max_retries = c
                .get_item("max_retries")?
                .map(|v| v.extract::<u32>())
                .transpose()?
                .unwrap_or(0);
            hook = hook.with_config(RustHookConfig {
                priority,
                timeout_ms,
                async_execution,
                max_retries,
            });
        }

        self.inner.register_hook(hook);

        if let Some(py_fn) = handler {
            self.inner.register_hook_handler(
                &hook_id,
                Arc::new(PythonCallbackHandler { callback: py_fn }),
            );
        }

        Ok(())
    }

    /// Unregister a hook by ID.
    ///
    /// Returns True if the hook was found and removed, False otherwise.
    fn unregister_hook(&self, hook_id: String) -> bool {
        self.inner.unregister_hook_handler(&hook_id);
        self.inner.unregister_hook(&hook_id).is_some()
    }

    /// Get the number of registered hooks.
    fn hook_count(&self) -> usize {
        self.inner.hook_count()
    }

    // ========================================================================
    // Session Metadata API
    // ========================================================================

    /// Return the session ID.
    #[getter]
    fn session_id(&self) -> String {
        self.inner.session_id().to_string()
    }

    /// Return the workspace path.
    #[getter]
    fn workspace(&self) -> String {
        self.inner.workspace().display().to_string()
    }

    /// Return any deferred init warning (e.g. memory store failed to initialize).
    #[getter]
    fn init_warning(&self) -> Option<String> {
        self.inner.init_warning().map(|s| s.to_string())
    }

    // ========================================================================
    // Session Persistence API
    // ========================================================================

    /// Save the session to the configured store.
    ///
    /// Returns None if no store is configured (no-op).
    fn save(&self, py: Python<'_>) -> PyResult<()> {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.save()))
            .map_err(|e| PyRuntimeError::new_err(format!("Save failed: {e}")))
    }

    // ========================================================================
    // Memory API
    // ========================================================================

    /// Check if memory is configured for this session.
    #[getter]
    fn has_memory(&self) -> bool {
        self.inner.memory().is_some()
    }

    /// Remember a successful task execution.
    ///
    /// Args:
    ///     task: Description of the task
    ///     tools: List of tool names used
    ///     result: Summary of the result
    #[pyo3(signature = (task, tools, result))]
    fn remember_success(
        &self,
        py: Python<'_>,
        task: String,
        tools: Vec<String>,
        result: String,
    ) -> PyResult<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not configured for this session"))?
            .clone();
        py.allow_threads(move || {
            get_runtime().block_on(memory.remember_success(&task, &tools, &result))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("Remember failed: {e}")))
    }

    /// Remember a failed task execution.
    ///
    /// Args:
    ///     task: Description of the task
    ///     error: Error message
    ///     tools: List of tool names attempted
    #[pyo3(signature = (task, error, tools))]
    fn remember_failure(
        &self,
        py: Python<'_>,
        task: String,
        error: String,
        tools: Vec<String>,
    ) -> PyResult<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not configured for this session"))?
            .clone();
        py.allow_threads(move || {
            get_runtime().block_on(memory.remember_failure(&task, &error, &tools))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("Remember failed: {e}")))
    }

    /// Recall memories similar to a query.
    ///
    /// Args:
    ///     query: Search query
    ///     limit: Maximum number of results (default: 5)
    ///
    /// Returns:
    ///     List of dicts with task, tools, result/error, outcome, timestamp.
    #[pyo3(signature = (query, limit=5))]
    fn recall_similar<'py>(
        &self,
        py: Python<'py>,
        query: String,
        limit: usize,
    ) -> PyResult<Bound<'py, PyList>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not configured for this session"))?
            .clone();
        let items = py
            .allow_threads(move || get_runtime().block_on(memory.recall_similar(&query, limit)))
            .map_err(|e| PyRuntimeError::new_err(format!("Recall failed: {e}")))?;
        let json_str = serde_json::to_string(&items)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .map(|l| l.clone())
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Recall memories by tags.
    ///
    /// Args:
    ///     tags: List of tags to search for
    ///     limit: Maximum number of results (default: 10)
    ///
    /// Returns:
    ///     List of memory item dicts.
    #[pyo3(signature = (tags, limit=10))]
    fn recall_by_tags<'py>(
        &self,
        py: Python<'py>,
        tags: Vec<String>,
        limit: usize,
    ) -> PyResult<Bound<'py, PyList>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not configured for this session"))?
            .clone();
        let items = py
            .allow_threads(move || get_runtime().block_on(memory.recall_by_tags(&tags, limit)))
            .map_err(|e| PyRuntimeError::new_err(format!("Recall failed: {e}")))?;
        let json_str = serde_json::to_string(&items)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .map(|l| l.clone())
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get recent memory items.
    ///
    /// Args:
    ///     limit: Maximum number of results (default: 10)
    ///
    /// Returns:
    ///     List of memory item dicts.
    #[pyo3(signature = (limit=10))]
    fn memory_recent<'py>(&self, py: Python<'py>, limit: usize) -> PyResult<Bound<'py, PyList>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not configured for this session"))?
            .clone();
        let items = py
            .allow_threads(move || get_runtime().block_on(memory.get_recent(limit)))
            .map_err(|e| PyRuntimeError::new_err(format!("Recall failed: {e}")))?;
        let json_str = serde_json::to_string(&items)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .map(|l| l.clone())
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get memory statistics.
    ///
    /// Returns:
    ///     Dict with long_term_count, short_term_count, working_count.
    fn memory_stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not configured for this session"))?
            .clone();
        let stats = py
            .allow_threads(move || get_runtime().block_on(memory.stats()))
            .map_err(|e| PyRuntimeError::new_err(format!("Stats failed: {e}")))?;
        let json_str = serde_json::to_string(&stats)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyDict>()
            .map(|d| d.clone())
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Get current working memory items.
    ///
    /// Working memory holds the active context items for the current task.
    ///
    /// Returns:
    ///     List of memory item dicts currently in working memory.
    fn get_working<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not configured for this session"))?
            .clone();
        let items = py.allow_threads(move || get_runtime().block_on(memory.get_working()));
        let json_str = serde_json::to_string(&items)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .map(|l| l.clone())
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Clear working memory.
    ///
    /// Removes all items from working memory without affecting short-term or long-term memory.
    fn clear_working(&self, py: Python<'_>) -> PyResult<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not configured for this session"))?
            .clone();
        py.allow_threads(move || get_runtime().block_on(memory.clear_working()));
        Ok(())
    }

    /// Get current short-term memory items.
    ///
    /// Short-term memory contains items stored during this session.
    ///
    /// Returns:
    ///     List of memory item dicts in short-term memory for this session.
    fn get_short_term<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not configured for this session"))?
            .clone();
        let items = py.allow_threads(move || get_runtime().block_on(memory.get_short_term()));
        let json_str = serde_json::to_string(&items)
            .map_err(|e| PyRuntimeError::new_err(format!("Serialization error: {e}")))?;
        let json_mod = py.import("json")?;
        let py_obj = json_mod.call_method1("loads", (json_str,))?;
        py_obj
            .downcast::<PyList>()
            .map(|l| l.clone())
            .map_err(|e| PyRuntimeError::new_err(format!("Unexpected result: {e}")))
    }

    /// Clear short-term memory for this session.
    ///
    /// Removes all session-scoped memory items without affecting long-term or working memory.
    fn clear_short_term(&self, py: Python<'_>) -> PyResult<()> {
        let memory = self
            .inner
            .memory()
            .ok_or_else(|| PyRuntimeError::new_err("Memory not configured for this session"))?
            .clone();
        py.allow_threads(move || get_runtime().block_on(memory.clear_short_term()));
        Ok(())
    }

    // ========================================================================
    // Slash Command & Scheduler API
    // ========================================================================

    /// List all registered slash commands.
    ///
    /// Returns a list of dicts with keys: `name`, `description`, `usage` (or `None`).
    /// Slash commands can be invoked via `session.send("/command args")`.
    fn list_commands<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let commands = self.inner.command_registry().list_full();
        let items: Vec<_> = commands
            .into_iter()
            .map(|(name, description, usage)| {
                let d = PyDict::new(py);
                let _ = d.set_item("name", &name);
                let _ = d.set_item("description", &description);
                let _ = d.set_item("usage", usage.as_deref());
                d.into_any()
            })
            .collect();
        PyList::new(py, &items)
    }

    /// Register a custom slash command with a Python callback.
    ///
    /// The `handler` receives two arguments: `args: str` (everything after the command name)
    /// and `ctx: dict` (session context with keys: `session_id`, `workspace`, `model`,
    /// `history_len`, `total_tokens`, `total_cost`, `tool_names`).
    /// It must return a `str` — the text displayed to the user.
    ///
    /// Example::
    ///
    ///     def ping_handler(args, ctx):
    ///         return f"pong! session={ctx['session_id']}"
    ///
    ///     session.register_command("ping", "Pong!", ping_handler)
    ///     result = await session.send("/ping hello")
    #[pyo3(signature = (name, description, handler))]
    fn register_command(
        &self,
        name: String,
        description: String,
        handler: pyo3::Py<pyo3::PyAny>,
    ) -> PyResult<()> {
        let cmd = Arc::new(PySlashCommand {
            name,
            description,
            handler,
        });
        self.inner.clone().register_command(cmd);
        Ok(())
    }

    /// Cancel the current ongoing operation (send/stream).
    ///
    /// If an operation is in progress, this will trigger cancellation of the LLM streaming
    /// and tool execution. The operation will terminate as soon as possible.
    ///
    /// :returns: ``True`` if an operation was cancelled, ``False`` if no operation was in progress.
    fn cancel(&self, py: Python<'_>) -> bool {
        let session = self.inner.clone();
        py.allow_threads(move || get_runtime().block_on(session.cancel()))
    }

    fn __repr__(&self) -> String {
        format!(
            "Session(id='{}', workspace='{}')",
            self.inner.session_id(),
            self.inner.workspace().display()
        )
    }
}

// ============================================================================
// Hook Helpers
// ============================================================================

fn py_parse_hook_event_type(event_type: &str) -> PyResult<RustHookEventType> {
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
        // Harness control points
        "pre_context_perception" => Ok(RustHookEventType::PreContextPerception),
        "post_context_perception" => Ok(RustHookEventType::PostContextPerception),
        "on_success" => Ok(RustHookEventType::OnSuccess),
        "pre_memory_recall" => Ok(RustHookEventType::PreMemoryRecall),
        "post_memory_recall" => Ok(RustHookEventType::PostMemoryRecall),
        "pre_planning" => Ok(RustHookEventType::PrePlanning),
        "post_planning" => Ok(RustHookEventType::PostPlanning),
        "pre_reasoning" => Ok(RustHookEventType::PreReasoning),
        "post_reasoning" => Ok(RustHookEventType::PostReasoning),
        "on_rate_limit" => Ok(RustHookEventType::OnRateLimit),
        "on_confirmation" => Ok(RustHookEventType::OnConfirmation),
        _ => Err(PyValueError::new_err(format!(
            "Invalid hook event type: '{}'. Expected one of: pre_tool_use, post_tool_use, \
             generate_start, generate_end, session_start, session_end, skill_load, \
             skill_unload, pre_prompt, post_response, on_error, pre_context_perception, \
             post_context_perception, on_success, pre_memory_recall, post_memory_recall, \
             pre_planning, post_planning, pre_reasoning, post_reasoning, on_rate_limit, \
             on_confirmation",
            event_type
        ))),
    }
}

// ============================================================================
// PythonCallbackHandler — bridges Python callables into the Rust HookHandler trait
// ============================================================================

/// Wraps a Python callable so it can be used as a `HookHandler`.
///
/// The callable receives a dict (the serialized `HookEvent`) and must return
/// `None` / `{"action": "continue"}` to allow execution, or
/// `{"action": "block", "reason": "..."}` to cancel it.
///
/// GIL safety: `send()` and `stream()` both release the GIL via `py.allow_threads()`,
/// so acquiring it here from a tokio worker thread does not deadlock.
struct PythonCallbackHandler {
    callback: pyo3::Py<pyo3::PyAny>,
}

impl RustHookHandler for PythonCallbackHandler {
    fn handle(&self, event: &RustHookEvent) -> RustHookResponse {
        let Ok(json_str) = serde_json::to_string(event) else {
            return RustHookResponse::continue_();
        };

        pyo3::Python::with_gil(|py| {
            // Deserialize the event into a Python dict via json.loads.
            let result = (|| -> pyo3::PyResult<RustHookResponse> {
                let json_mod = py.import("json")?;
                let event_dict = json_mod.call_method1("loads", (json_str.as_str(),))?;
                let ret = self.callback.call1(py, (event_dict,))?;
                parse_py_hook_response(py, ret.bind(py))
            })();

            result.unwrap_or_else(|_| RustHookResponse::continue_())
        })
    }
}

/// Parse the return value of a Python hook callback into a `HookResponse`.
///
/// Accepted shapes:
/// - `None`                                   → continue
/// - `{"action": "continue"}`                 → continue
/// - `{"action": "block", "reason": "…"}`     → block
/// - `{"action": "skip"}`                     → skip
/// - `{"action": "retry", "delay_ms": N}`     → retry
fn parse_py_hook_response(
    _py: pyo3::Python,
    val: &pyo3::Bound<pyo3::PyAny>,
) -> pyo3::PyResult<RustHookResponse> {
    use pyo3::types::PyDict;

    if val.is_none() {
        return Ok(RustHookResponse::continue_());
    }

    if let Ok(dict) = val.downcast::<PyDict>() {
        let action = dict
            .get_item("action")?
            .and_then(|v| v.extract::<String>().ok());

        match action.as_deref() {
            Some("block") => {
                let reason = dict
                    .get_item("reason")?
                    .and_then(|v| v.extract::<String>().ok())
                    .unwrap_or_else(|| "Blocked by hook".to_string());
                return Ok(RustHookResponse::block(reason));
            }
            Some("skip") => return Ok(RustHookResponse::skip()),
            Some("retry") => {
                let delay_ms = dict
                    .get_item("delay_ms")?
                    .and_then(|v| v.extract::<u64>().ok())
                    .unwrap_or(1000);
                return Ok(RustHookResponse::retry(delay_ms));
            }
            _ => {}
        }
    }

    Ok(RustHookResponse::continue_())
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

// ============================================================================
// Typed store / provider helpers
// ============================================================================

/// File-backed long-term memory store.
///
/// Pass to ``SessionOptions.memory_store``:
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.memory_store = FileMemoryStore('./memory')
///     session = agent.session('.', opts)
#[pyclass(name = "FileMemoryStore")]
#[derive(Clone)]
struct PyFileMemoryStore {
    #[pyo3(get, set)]
    dir: String,
}

#[pymethods]
impl PyFileMemoryStore {
    #[new]
    fn new(dir: String) -> Self {
        Self { dir }
    }

    fn __repr__(&self) -> String {
        format!("FileMemoryStore(dir={:?})", self.dir)
    }
}

/// File-backed session store — persists sessions to disk for later resumption.
///
/// Pass to ``SessionOptions.session_store``:
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.session_store = FileSessionStore('./sessions')
///     opts.session_id = 'my-session'
///     opts.auto_save = True
///     session = agent.session('.', opts)
#[pyclass(name = "FileSessionStore")]
#[derive(Clone)]
struct PyFileSessionStore {
    #[pyo3(get, set)]
    dir: String,
}

#[pymethods]
impl PyFileSessionStore {
    #[new]
    fn new(dir: String) -> Self {
        Self { dir }
    }

    fn __repr__(&self) -> String {
        format!("FileSessionStore(dir={:?})", self.dir)
    }
}

/// In-memory (non-persistent) session store.
///
/// Useful for testing, ephemeral runs, and CI pipelines where no disk state is needed.
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.session_store = MemorySessionStore()
#[pyclass(name = "MemorySessionStore")]
#[derive(Clone)]
struct PyMemorySessionStore {}

#[pymethods]
impl PyMemorySessionStore {
    #[new]
    fn new() -> Self {
        Self {}
    }

    fn __repr__(&self) -> String {
        "MemorySessionStore()".to_string()
    }
}

/// Default security provider: input taint tracking + output sanitisation.
///
/// Pass to ``SessionOptions.security_provider``:
///
/// .. code-block:: python
///
///     opts = SessionOptions()
///     opts.security_provider = DefaultSecurityProvider()
#[pyclass(name = "DefaultSecurityProvider")]
#[derive(Clone)]
struct PyDefaultSecurityProvider {}

#[pymethods]
impl PyDefaultSecurityProvider {
    #[new]
    fn new() -> Self {
        Self {}
    }

    fn __repr__(&self) -> String {
        "DefaultSecurityProvider()".to_string()
    }
}

// ============================================================================
// Plugin Classes
// ============================================================================

/// Skill-only plugin — injects custom skills into the session's skill registry
/// without registering any tools.
///
/// Use this to add custom LLM guidance (instructions, tool restrictions,
/// prompting strategies) directly from Python. For tools, use MCP servers.
///
/// Args:
///     name: Unique plugin identifier (kebab-case).
///     skills: List of skill YAML/markdown content strings.
///
/// Example:
///     from a3s_code import SkillPlugin
///
///     skill_md = """
///     ---
///     name: my-skill
///     description: Use bash cautiously
///     allowed-tools: "bash(*)"
///     kind: instruction
///     ---
///     Always explain what command you're about to run before executing it.
///     """
///
///     opts = SessionOptions()
///     opts.plugins = [SkillPlugin("my-plugin", [skill_md])]
///     session = agent.session(".", opts)
#[pyclass(name = "SkillPlugin")]
#[derive(Clone)]
struct PySkillPlugin {
    #[pyo3(get, set)]
    name: String,
    #[pyo3(get, set)]
    skills: Vec<String>,
}

#[pymethods]
impl PySkillPlugin {
    #[new]
    #[pyo3(signature = (name, skills=None))]
    fn new(name: String, skills: Option<Vec<String>>) -> Self {
        Self {
            name,
            skills: skills.unwrap_or_default(),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "SkillPlugin(name={:?}, skills=[{} entries])",
            self.name,
            self.skills.len()
        )
    }
}

// ============================================================================
// AHP Transport Classes
// ============================================================================

/// Stdio transport for AHP (Agent Harness Protocol).
///
/// Launches a child process and communicates via stdin/stdout using JSON-RPC 2.0.
///
/// Example:
///     transport = StdioTransport(program='python', args=['ahp_server.py'])
///     opts = SessionOptions()
///     opts.ahp_transport = transport
///     session = agent.session('.', opts)
#[pyclass(name = "StdioTransport")]
#[derive(Clone)]
struct PyStdioTransport {
    #[pyo3(get, set)]
    program: String,
    #[pyo3(get, set)]
    args: Vec<String>,
}

#[pymethods]
impl PyStdioTransport {
    #[new]
    fn new(program: String, args: Vec<String>) -> Self {
        Self { program, args }
    }

    fn __repr__(&self) -> String {
        format!(
            "StdioTransport(program={:?}, args={:?})",
            self.program, self.args
        )
    }
}

/// HTTP transport for AHP (Agent Harness Protocol).
///
/// Connects to a remote AHP harness server via HTTP.
///
/// Example:
///     transport = HttpTransport(url='http://localhost:8080/ahp')
///     opts = SessionOptions()
///     opts.ahp_transport = transport
///     session = agent.session('.', opts)
#[pyclass(name = "HttpTransport")]
#[derive(Clone)]
struct PyHttpTransport {
    #[pyo3(get, set)]
    url: String,
    #[pyo3(get, set)]
    auth_token: Option<String>,
}

#[pymethods]
impl PyHttpTransport {
    #[new]
    #[pyo3(signature = (url, auth_token=None))]
    fn new(url: String, auth_token: Option<String>) -> Self {
        Self { url, auth_token }
    }

    fn __repr__(&self) -> String {
        format!("HttpTransport(url={:?})", self.url)
    }
}

/// WebSocket transport for AHP (Agent Harness Protocol).
///
/// Connects to a remote AHP harness server via WebSocket for bidirectional streaming.
///
/// Example:
///     transport = WebSocketTransport(url='ws://localhost:8080/ahp')
///     opts = SessionOptions()
///     opts.ahp_transport = transport
///     session = agent.session('.', opts)
#[pyclass(name = "WebSocketTransport")]
#[derive(Clone)]
struct PyWebSocketTransport {
    #[pyo3(get, set)]
    url: String,
    #[pyo3(get, set)]
    auth_token: Option<String>,
}

#[pymethods]
impl PyWebSocketTransport {
    #[new]
    #[pyo3(signature = (url, auth_token=None))]
    fn new(url: String, auth_token: Option<String>) -> Self {
        Self { url, auth_token }
    }

    fn __repr__(&self) -> String {
        format!("WebSocketTransport(url={:?})", self.url)
    }
}

/// Unix socket transport for AHP (Agent Harness Protocol).
///
/// Connects to a local AHP harness server via Unix domain socket.
///
/// Example:
///     transport = UnixSocketTransport(path='/tmp/ahp.sock')
///     opts = SessionOptions()
///     opts.ahp_transport = transport
///     session = agent.session('.', opts)
#[pyclass(name = "UnixSocketTransport")]
#[derive(Clone)]
struct PyUnixSocketTransport {
    #[pyo3(get, set)]
    path: String,
}

#[pymethods]
impl PyUnixSocketTransport {
    #[new]
    fn new(path: String) -> Self {
        Self { path }
    }

    fn __repr__(&self) -> String {
        format!("UnixSocketTransport(path={:?})", self.path)
    }
}

// ============================================================================
// SessionOptions
// ============================================================================

/// Explicit allow/deny/ask tool permission policy.
#[pyclass(name = "PermissionPolicy")]
#[derive(Clone)]
struct PyPermissionPolicy {
    #[pyo3(get, set)]
    deny: Vec<String>,
    #[pyo3(get, set)]
    allow: Vec<String>,
    #[pyo3(get, set)]
    ask: Vec<String>,
    #[pyo3(get, set)]
    default_decision: String,
    #[pyo3(get, set)]
    enabled: bool,
}

#[pymethods]
impl PyPermissionPolicy {
    #[new]
    #[pyo3(signature = (allow=None, deny=None, ask=None, default_decision=None, enabled=true))]
    fn new(
        allow: Option<Vec<String>>,
        deny: Option<Vec<String>>,
        ask: Option<Vec<String>>,
        default_decision: Option<String>,
        enabled: bool,
    ) -> Self {
        Self {
            deny: deny.unwrap_or_default(),
            allow: allow.unwrap_or_default(),
            ask: ask.unwrap_or_default(),
            default_decision: default_decision.unwrap_or_else(|| "ask".to_string()),
            enabled,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "PermissionPolicy(allow={}, deny={}, ask={}, default_decision={:?}, enabled={})",
            self.allow.len(),
            self.deny.len(),
            self.ask.len(),
            self.default_decision,
            self.enabled
        )
    }
}

fn parse_py_permission_decision(value: &str) -> PyResult<RustPermissionDecision> {
    match value.trim().to_ascii_lowercase().as_str() {
        "allow" => Ok(RustPermissionDecision::Allow),
        "deny" => Ok(RustPermissionDecision::Deny),
        "ask" => Ok(RustPermissionDecision::Ask),
        other => Err(PyValueError::new_err(format!(
            "default_decision must be 'allow', 'deny', or 'ask', got {other:?}"
        ))),
    }
}

fn py_permission_policy_to_rust(policy: PyPermissionPolicy) -> PyResult<RustPermissionPolicy> {
    Ok(RustPermissionPolicy {
        deny: policy
            .deny
            .into_iter()
            .map(|rule| RustPermissionRule::new(&rule))
            .collect(),
        allow: policy
            .allow
            .into_iter()
            .map(|rule| RustPermissionRule::new(&rule))
            .collect(),
        ask: policy
            .ask
            .into_iter()
            .map(|rule| RustPermissionRule::new(&rule))
            .collect(),
        default_decision: parse_py_permission_decision(&policy.default_decision)?,
        enabled: policy.enabled,
    })
}

/// Per-session configuration options.
///
/// Pass to `agent.session(workspace, options)` to override defaults.
#[pyclass(name = "SessionOptions")]
struct PySessionOptions {
    model: Option<String>,
    builtin_skills: bool,
    skill_dirs: Vec<String>,
    agent_dirs: Vec<String>,
    queue_config: Option<PySessionQueueConfig>,
    permission_policy: Option<PyPermissionPolicy>,
    auto_compact: bool,
    auto_compact_threshold: Option<f32>,
    /// Long-term memory store backend. Set to a ``FileMemoryStore`` instance.
    memory_store: Option<pyo3::PyObject>,
    /// Session persistence store backend. Set to ``FileSessionStore`` or ``MemorySessionStore``.
    session_store: Option<pyo3::PyObject>,
    /// Security provider. Set to ``DefaultSecurityProvider`` to enable taint tracking.
    security_provider: Option<pyo3::PyObject>,
    /// Plugins to mount onto this session.
    ///
    /// Use ``SkillPlugin(...)`` to inject custom skills.
    plugins: Vec<pyo3::PyObject>,
    /// Custom role/identity (e.g. "You are a Python expert")
    role: Option<String>,
    /// Custom coding guidelines
    guidelines: Option<String>,
    /// Custom response style (replaces default)
    response_style: Option<String>,
    /// Freeform extra instructions
    extra: Option<String>,
    /// Inline skills registered programmatically: (name, kind, content).
    /// Populated via `add_instruction()` / `add_persona()` — not exposed directly to Python.
    inline_skills: Vec<(String, String, String)>,
    /// Override maximum number of tool-call rounds per session.
    max_tool_rounds: Option<usize>,
    /// Enable planning mode (default: False).
    planning: bool,
    /// Enable goal tracking (default: False).
    goal_tracking: bool,
    /// Max consecutive parse errors before abort (default: 2).
    max_parse_retries: Option<u32>,
    /// Per-tool execution timeout in milliseconds.
    tool_timeout_ms: Option<u64>,
    /// Max LLM API failures before abort (default: 3).
    circuit_breaker_threshold: Option<u32>,
    /// Sampling temperature (0.0–1.0). Overrides the provider default.
    /// Only applied when ``model`` is also set.
    temperature: Option<f32>,
    /// Extended thinking token budget (e.g. 10_000). Enables chain-of-thought reasoning.
    /// Only applied when ``model`` is also set. Provider must support extended thinking.
    thinking_budget: Option<usize>,
    /// Enable continuation injection (default: True).
    /// When enabled, the loop injects a follow-up prompt when the LLM stops without completing.
    continuation_enabled: Option<bool>,
    /// Maximum continuation injections per execution (default: 3).
    max_continuation_turns: Option<u32>,
    /// Session ID for this session (auto-generated if not set).
    ///
    /// Set a stable ID to save and resume the session later:
    ///
    /// .. code-block:: python
    ///
    ///     opts = SessionOptions()
    ///     opts.session_store = FileSessionStore('./sessions')
    ///     opts.session_id = 'my-session'
    ///     opts.auto_save = True
    ///     session = agent.session('.', opts)
    ///     # Later:
    ///     resumed = agent.resume_session('my-session', opts)
    session_id: Option<String>,
    /// Automatically save the session to the configured store after each turn (default: False).
    auto_save: bool,
    /// AHP transport configuration for external agent supervision.
    ///
    /// Set to an AHP transport instance (``StdioTransport``, ``HttpTransport``, etc.)
    /// to enable Agent Harness Protocol supervision:
    ///
    /// .. code-block:: python
    ///
    ///     opts = SessionOptions()
    ///     opts.ahp_transport = StdioTransport(program='python', args=['ahp_server.py'])
    ///     session = agent.session('.', opts)
    ahp_transport: Option<pyo3::PyObject>,
}

impl Clone for PySessionOptions {
    fn clone(&self) -> Self {
        Self {
            model: self.model.clone(),
            builtin_skills: self.builtin_skills,
            skill_dirs: self.skill_dirs.clone(),
            agent_dirs: self.agent_dirs.clone(),
            queue_config: self.queue_config.clone(),
            permission_policy: self.permission_policy.clone(),
            auto_compact: self.auto_compact,
            auto_compact_threshold: self.auto_compact_threshold,
            memory_store: pyo3::Python::with_gil(|py| {
                self.memory_store.as_ref().map(|o| o.clone_ref(py))
            }),
            session_store: pyo3::Python::with_gil(|py| {
                self.session_store.as_ref().map(|o| o.clone_ref(py))
            }),
            security_provider: pyo3::Python::with_gil(|py| {
                self.security_provider.as_ref().map(|o| o.clone_ref(py))
            }),
            plugins: pyo3::Python::with_gil(|py| {
                self.plugins.iter().map(|o| o.clone_ref(py)).collect()
            }),
            role: self.role.clone(),
            guidelines: self.guidelines.clone(),
            response_style: self.response_style.clone(),
            extra: self.extra.clone(),
            inline_skills: self.inline_skills.clone(),
            max_tool_rounds: self.max_tool_rounds,
            planning: self.planning,
            goal_tracking: self.goal_tracking,
            max_parse_retries: self.max_parse_retries,
            tool_timeout_ms: self.tool_timeout_ms,
            circuit_breaker_threshold: self.circuit_breaker_threshold,
            temperature: self.temperature,
            thinking_budget: self.thinking_budget,
            continuation_enabled: self.continuation_enabled,
            max_continuation_turns: self.max_continuation_turns,
            session_id: self.session_id.clone(),
            auto_save: self.auto_save,
            ahp_transport: pyo3::Python::with_gil(|py| {
                self.ahp_transport.as_ref().map(|o| o.clone_ref(py))
            }),
        }
    }
}

#[pymethods]
impl PySessionOptions {
    #[new]
    fn new() -> Self {
        Self {
            model: None,
            builtin_skills: false,
            skill_dirs: vec![],
            agent_dirs: vec![],
            queue_config: None,
            permission_policy: None,
            auto_compact: false,
            auto_compact_threshold: None,
            memory_store: None,
            session_store: None,
            security_provider: None,
            plugins: vec![],
            role: None,
            guidelines: None,
            response_style: None,
            extra: None,
            inline_skills: vec![],
            max_tool_rounds: None,
            planning: false,
            goal_tracking: false,
            max_parse_retries: None,
            tool_timeout_ms: None,
            circuit_breaker_threshold: None,
            temperature: None,
            thinking_budget: None,
            continuation_enabled: None,
            max_continuation_turns: None,
            session_id: None,
            auto_save: false,
            ahp_transport: None,
        }
    }

    /// Override the default model. Format: "provider/model".
    #[getter]
    fn get_model(&self) -> Option<String> {
        self.model.clone()
    }

    #[setter]
    fn set_model(&mut self, value: Option<String>) {
        self.model = value;
    }

    /// Enable built-in skills.
    #[getter]
    fn get_builtin_skills(&self) -> bool {
        self.builtin_skills
    }

    #[setter]
    fn set_builtin_skills(&mut self, value: bool) {
        self.builtin_skills = value;
    }

    /// Extra directories to scan for skill files.
    #[getter]
    fn get_skill_dirs(&self) -> Vec<String> {
        self.skill_dirs.clone()
    }

    #[setter]
    fn set_skill_dirs(&mut self, value: Vec<String>) {
        self.skill_dirs = value;
    }

    /// Extra directories to scan for agent files.
    #[getter]
    fn get_agent_dirs(&self) -> Vec<String> {
        self.agent_dirs.clone()
    }

    #[setter]
    fn set_agent_dirs(&mut self, value: Vec<String>) {
        self.agent_dirs = value;
    }

    /// Optional advanced queue configuration for explicit external/hybrid lane dispatch.
    ///
    /// Ordinary sessions are queue-free unless this is set.
    #[getter]
    fn get_queue_config(&self) -> Option<PySessionQueueConfig> {
        self.queue_config.clone()
    }

    #[setter]
    fn set_queue_config(&mut self, value: Option<PySessionQueueConfig>) {
        self.queue_config = value;
    }

    /// Explicit permission policy for tool execution.
    ///
    /// Use this to make tool access explicit for real applications.
    #[getter]
    fn get_permission_policy(&self) -> Option<PyPermissionPolicy> {
        self.permission_policy.clone()
    }

    #[setter]
    fn set_permission_policy(&mut self, value: Option<PyPermissionPolicy>) {
        self.permission_policy = value;
    }

    /// Enable auto-compaction when context window fills up.
    #[getter]
    fn get_auto_compact(&self) -> bool {
        self.auto_compact
    }

    #[setter]
    fn set_auto_compact(&mut self, value: bool) {
        self.auto_compact = value;
    }

    /// Context usage threshold (0.0–1.0) to trigger auto-compaction.
    #[getter]
    fn get_auto_compact_threshold(&self) -> Option<f32> {
        self.auto_compact_threshold
    }

    #[setter]
    fn set_auto_compact_threshold(&mut self, value: Option<f32>) {
        self.auto_compact_threshold = value;
    }

    /// Long-term memory store backend.
    ///
    /// Assign a ``FileMemoryStore`` instance:
    ///
    /// .. code-block:: python
    ///
    ///     opts.memory_store = FileMemoryStore('./memory')
    #[getter]
    fn get_memory_store(&self, py: pyo3::Python<'_>) -> Option<pyo3::PyObject> {
        self.memory_store.as_ref().map(|o| o.clone_ref(py))
    }

    #[setter]
    fn set_memory_store(&mut self, value: Option<pyo3::PyObject>) {
        self.memory_store = value;
    }

    /// Session persistence store backend.
    ///
    /// Assign a ``FileSessionStore`` or ``MemorySessionStore`` instance:
    ///
    /// .. code-block:: python
    ///
    ///     opts.session_store = FileSessionStore('./sessions')  # persists to disk
    ///     opts.session_store = MemorySessionStore()           # ephemeral
    #[getter]
    fn get_session_store(&self, py: pyo3::Python<'_>) -> Option<pyo3::PyObject> {
        self.session_store.as_ref().map(|o| o.clone_ref(py))
    }

    #[setter]
    fn set_session_store(&mut self, value: Option<pyo3::PyObject>) {
        self.session_store = value;
    }

    /// Security provider.
    ///
    /// Assign a ``DefaultSecurityProvider`` to enable taint tracking and output sanitisation:
    ///
    /// .. code-block:: python
    ///
    ///     opts.security_provider = DefaultSecurityProvider()
    #[getter]
    fn get_security_provider(&self, py: pyo3::Python<'_>) -> Option<pyo3::PyObject> {
        self.security_provider.as_ref().map(|o| o.clone_ref(py))
    }

    #[setter]
    fn set_security_provider(&mut self, value: Option<pyo3::PyObject>) {
        self.security_provider = value;
    }

    /// Plugins to mount onto this session (for example ``SkillPlugin``).
    #[getter]
    fn get_plugins(&self, py: pyo3::Python<'_>) -> Vec<pyo3::PyObject> {
        self.plugins.iter().map(|o| o.clone_ref(py)).collect()
    }

    #[setter]
    fn set_plugins(&mut self, value: Vec<pyo3::PyObject>) {
        self.plugins = value;
    }

    /// Custom role/identity prepended before the core agentic prompt.
    /// Example: "You are a senior Python developer specializing in FastAPI."
    #[getter]
    fn get_role(&self) -> Option<String> {
        self.role.clone()
    }

    #[setter]
    fn set_role(&mut self, value: Option<String>) {
        self.role = value;
    }

    /// Custom coding guidelines appended after the core prompt.
    /// Example: "Always use type hints. Follow PEP 8."
    #[getter]
    fn get_guidelines(&self) -> Option<String> {
        self.guidelines.clone()
    }

    #[setter]
    fn set_guidelines(&mut self, value: Option<String>) {
        self.guidelines = value;
    }

    /// Custom response style (replaces default Response Format section).
    #[getter]
    fn get_response_style(&self) -> Option<String> {
        self.response_style.clone()
    }

    #[setter]
    fn set_response_style(&mut self, value: Option<String>) {
        self.response_style = value;
    }

    /// Freeform extra instructions appended at the end.
    #[getter]
    fn get_extra(&self) -> Option<String> {
        self.extra.clone()
    }

    #[setter]
    fn set_extra(&mut self, value: Option<String>) {
        self.extra = value;
    }

    /// Override maximum number of tool-call rounds for this session.
    #[getter]
    fn get_max_tool_rounds(&self) -> Option<usize> {
        self.max_tool_rounds
    }

    #[setter]
    fn set_max_tool_rounds(&mut self, value: Option<usize>) {
        self.max_tool_rounds = value;
    }

    /// Enable planning mode (default: False).
    #[getter]
    fn get_planning(&self) -> bool {
        self.planning
    }

    #[setter]
    fn set_planning(&mut self, value: bool) {
        self.planning = value;
    }

    /// Enable goal tracking (default: False).
    #[getter]
    fn get_goal_tracking(&self) -> bool {
        self.goal_tracking
    }

    #[setter]
    fn set_goal_tracking(&mut self, value: bool) {
        self.goal_tracking = value;
    }

    /// Max consecutive parse errors before abort (default: 2).
    #[getter]
    fn get_max_parse_retries(&self) -> Option<u32> {
        self.max_parse_retries
    }

    #[setter]
    fn set_max_parse_retries(&mut self, value: Option<u32>) {
        self.max_parse_retries = value;
    }

    /// Per-tool execution timeout in milliseconds.
    #[getter]
    fn get_tool_timeout_ms(&self) -> Option<u64> {
        self.tool_timeout_ms
    }

    #[setter]
    fn set_tool_timeout_ms(&mut self, value: Option<u64>) {
        self.tool_timeout_ms = value;
    }

    /// Max LLM API failures before abort (default: 3).
    #[getter]
    fn get_circuit_breaker_threshold(&self) -> Option<u32> {
        self.circuit_breaker_threshold
    }

    #[setter]
    fn set_circuit_breaker_threshold(&mut self, value: Option<u32>) {
        self.circuit_breaker_threshold = value;
    }

    /// Sampling temperature (0.0–1.0). Overrides the provider default.
    /// Only applied when ``model`` is also set.
    #[getter]
    fn get_temperature(&self) -> Option<f32> {
        self.temperature
    }

    #[setter]
    fn set_temperature(&mut self, value: Option<f32>) {
        self.temperature = value;
    }

    /// Extended thinking token budget. Enables chain-of-thought reasoning.
    /// Only applied when ``model`` is also set.
    #[getter]
    fn get_thinking_budget(&self) -> Option<usize> {
        self.thinking_budget
    }

    #[setter]
    fn set_thinking_budget(&mut self, value: Option<usize>) {
        self.thinking_budget = value;
    }

    /// Enable or disable continuation injection (default: True).
    #[getter]
    fn get_continuation_enabled(&self) -> Option<bool> {
        self.continuation_enabled
    }

    #[setter]
    fn set_continuation_enabled(&mut self, value: Option<bool>) {
        self.continuation_enabled = value;
    }

    /// Maximum continuation injections per execution (default: 3).
    #[getter]
    fn get_max_continuation_turns(&self) -> Option<u32> {
        self.max_continuation_turns
    }

    #[setter]
    fn set_max_continuation_turns(&mut self, value: Option<u32>) {
        self.max_continuation_turns = value;
    }

    /// Session ID (auto-generated if not set). Set to save and resume sessions by name.
    #[getter]
    fn get_session_id(&self) -> Option<String> {
        self.session_id.clone()
    }

    #[setter]
    fn set_session_id(&mut self, value: Option<String>) {
        self.session_id = value;
    }

    /// Automatically save the session after each turn (default: False).
    #[getter]
    fn get_auto_save(&self) -> bool {
        self.auto_save
    }

    #[setter]
    fn set_auto_save(&mut self, value: bool) {
        self.auto_save = value;
    }

    /// AHP transport configuration for external agent supervision.
    #[getter]
    fn get_ahp_transport(&self) -> Option<pyo3::PyObject> {
        pyo3::Python::with_gil(|py| self.ahp_transport.as_ref().map(|o| o.clone_ref(py)))
    }

    #[setter]
    fn set_ahp_transport(&mut self, value: Option<pyo3::PyObject>) {
        self.ahp_transport = value;
    }

    /// External AHP harness server.
    ///

    /// Register an instruction skill programmatically.
    ///
    /// Instructions are injected into the system prompt at session start.
    /// Use this instead of skill files for simple, one-off guidance.
    ///
    /// Args:
    ///     name: Unique skill name (kebab-case recommended, e.g. "type-hints")
    ///     content: Markdown content describing the instruction
    fn add_instruction(&mut self, name: String, content: String) {
        self.inline_skills
            .push((name, "instruction".to_string(), content));
    }

    /// Register a persona skill programmatically.
    ///
    /// Personas replace the default role section of the system prompt.
    /// Only one persona is active at a time (last registered wins).
    ///
    /// Args:
    ///     name: Unique skill name (kebab-case recommended, e.g. "python-expert")
    ///     content: System prompt content for this persona
    fn add_persona(&mut self, name: String, content: String) {
        self.inline_skills
            .push((name, "persona".to_string(), content));
    }

    fn __repr__(&self) -> String {
        format!(
            "SessionOptions(model={:?}, builtin_skills={}, queue_config={}, auto_compact={}, memory_store={}, session_store={}, security_provider={}, inline_skills={})",
            self.model,
            self.builtin_skills,
            if self.queue_config.is_some() { "Some(...)" } else { "None" },
            self.auto_compact,
            if self.memory_store.is_some() { "Some(...)" } else { "None" },
            if self.session_store.is_some() { "Some(...)" } else { "None" },
            if self.security_provider.is_some() { "Some(...)" } else { "None" },
            self.inline_skills.len(),
        )
    }
}

// ============================================================================
// SessionQueueConfig
// ============================================================================

/// Configuration for the optional advanced session lane queue.
///
/// Ordinary sessions do not initialize queue infrastructure. Use this only for
/// explicit external/hybrid dispatch, priority experiments, or operational integrations.
#[pyclass(name = "SessionQueueConfig")]
#[derive(Clone)]
struct PySessionQueueConfig {
    inner: RustSessionQueueConfig,
}

#[pymethods]
impl PySessionQueueConfig {
    #[new]
    fn new() -> Self {
        Self {
            inner: RustSessionQueueConfig::default(),
        }
    }

    /// Enable all lane features (DLQ, metrics, alerts) with sensible defaults.
    fn with_lane_features(&mut self) {
        self.inner = self.inner.clone().with_lane_features();
    }

    /// Set max concurrency for Query lane (default: 4).
    fn set_query_concurrency(&mut self, n: usize) {
        self.inner.query_max_concurrency = n;
    }

    /// Set max concurrency for Execute lane (default: 2).
    fn set_execute_concurrency(&mut self, n: usize) {
        self.inner.execute_max_concurrency = n;
    }

    /// Set max concurrency for Generate lane (default: 1).
    fn set_generate_concurrency(&mut self, n: usize) {
        self.inner.generate_max_concurrency = n;
    }

    /// Enable dead letter queue with optional max size.
    #[pyo3(signature = (max_size=None))]
    fn enable_dlq(&mut self, max_size: Option<usize>) {
        self.inner = self.inner.clone().with_dlq(max_size);
    }

    /// Enable metrics collection.
    fn enable_metrics(&mut self) {
        self.inner = self.inner.clone().with_metrics();
    }

    /// Enable queue alerts.
    fn enable_alerts(&mut self) {
        self.inner = self.inner.clone().with_alerts();
    }

    /// Set default timeout for commands (ms).
    fn set_timeout(&mut self, timeout_ms: u64) {
        self.inner = self.inner.clone().with_timeout(timeout_ms);
    }

    /// Configure how a specific lane handles tasks.
    ///
    /// Args:
    ///     lane (Literal["control", "query", "execute", "generate"]): Which lane to configure.
    ///     mode (Literal["internal", "external", "hybrid"]): Execution mode for the lane's tools.
    ///     timeout_ms: Timeout for external tasks in milliseconds (default 60000).
    #[pyo3(signature = (lane, mode, timeout_ms=60_000))]
    fn set_lane_handler(&mut self, lane: &str, mode: &str, timeout_ms: u64) -> PyResult<()> {
        let rust_lane = parse_lane(lane)?;
        let rust_mode = parse_handler_mode(mode)?;
        let config = RustLaneHandlerConfig {
            mode: rust_mode,
            timeout_ms,
        };
        self.inner.lane_handlers.insert(rust_lane, config);
        Ok(())
    }

    /// Set max concurrency for Query lane (default: 4).
    #[getter]
    fn get_query_max_concurrency(&self) -> usize {
        self.inner.query_max_concurrency
    }

    #[setter]
    fn set_query_max_concurrency(&mut self, value: usize) {
        self.inner.query_max_concurrency = value;
    }

    fn __repr__(&self) -> String {
        format!(
            "SessionQueueConfig(query={}, execute={}, generate={}, dlq={}, metrics={})",
            self.inner.query_max_concurrency,
            self.inner.execute_max_concurrency,
            self.inner.generate_max_concurrency,
            self.inner.enable_dlq,
            self.inner.enable_metrics,
        )
    }
}

// ============================================================================
// Queue Helpers
// ============================================================================

fn parse_lane(lane: &str) -> PyResult<RustSessionLane> {
    match lane {
        "control" => Ok(RustSessionLane::Control),
        "query" => Ok(RustSessionLane::Query),
        "execute" => Ok(RustSessionLane::Execute),
        "generate" => Ok(RustSessionLane::Generate),
        _ => Err(PyValueError::new_err(format!(
            "Invalid lane '{}'. Must be: control, query, execute, or generate",
            lane
        ))),
    }
}

fn parse_handler_mode(mode: &str) -> PyResult<RustTaskHandlerMode> {
    match mode {
        "internal" => Ok(RustTaskHandlerMode::Internal),
        "external" => Ok(RustTaskHandlerMode::External),
        "hybrid" => Ok(RustTaskHandlerMode::Hybrid),
        _ => Err(PyValueError::new_err(format!(
            "Invalid handler mode '{}'. Must be: internal, external, or hybrid",
            mode
        ))),
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Build RustSessionOptions from PySessionOptions.
fn build_rust_session_options(so: PySessionOptions) -> PyResult<RustSessionOptions> {
    let mut o = RustSessionOptions::new();
    if let Some(m) = so.model {
        o = o.with_model(m);
    }
    if so.builtin_skills {
        o = o.with_builtin_skills();
    }
    for d in &so.skill_dirs {
        o = o.with_skills_from_dir(d);
    }
    for d in &so.agent_dirs {
        o = o.with_agent_dir(d);
    }
    if let Some(qc) = so.queue_config {
        o = o.with_queue_config(qc.inner);
    }
    if let Some(policy) = so.permission_policy {
        o = o.with_permission_checker(Arc::new(py_permission_policy_to_rust(policy)?));
    }
    if so.auto_compact {
        o = o.with_auto_compact(true);
    }
    if let Some(t) = so.auto_compact_threshold {
        o = o.with_auto_compact_threshold(t);
    }
    if let Some(ref store) = so.memory_store {
        let dir = Python::with_gil(|py| {
            store
                .extract::<pyo3::PyRef<PyFileMemoryStore>>(py)
                .ok()
                .map(|s| s.dir.clone())
        });
        if let Some(dir) = dir {
            o = o.with_file_memory(dir);
        }
    }
    if let Some(ref store) = so.session_store {
        enum SessionStoreKind {
            File(String),
            Memory,
        }
        let kind = Python::with_gil(|py| {
            if let Ok(file_store) = store.extract::<pyo3::PyRef<PyFileSessionStore>>(py) {
                Some(SessionStoreKind::File(file_store.dir.clone()))
            } else if store
                .extract::<pyo3::PyRef<PyMemorySessionStore>>(py)
                .is_ok()
            {
                Some(SessionStoreKind::Memory)
            } else {
                None
            }
        });
        match kind {
            Some(SessionStoreKind::File(dir)) => {
                o = o.with_file_session_store(dir);
            }
            Some(SessionStoreKind::Memory) => {
                let s: Arc<dyn a3s_code_core::store::SessionStore> =
                    Arc::new(a3s_code_core::store::MemorySessionStore::new());
                o = o.with_session_store(s);
            }
            None => {}
        }
    }
    if let Some(ref sec) = so.security_provider {
        let is_default = Python::with_gil(|py| {
            sec.extract::<pyo3::PyRef<PyDefaultSecurityProvider>>(py)
                .is_ok()
        });
        if is_default {
            o = o.with_default_security();
        }
    }
    // Mount plugins
    for plugin_obj in &so.plugins {
        enum PluginKind {
            Skill(String, Vec<String>),
        }
        let kind = Python::with_gil(|py| {
            if let Ok(s) = plugin_obj.extract::<pyo3::PyRef<PySkillPlugin>>(py) {
                Some(PluginKind::Skill(s.name.clone(), s.skills.clone()))
            } else {
                None
            }
        });
        match kind {
            Some(PluginKind::Skill(name, skills)) => {
                let sp = a3s_code_core::plugin::SkillPlugin::new(name).with_skills(skills);
                o = o.with_plugin(sp);
            }
            None => {
                eprintln!("a3s-code: unknown plugin type — skipping");
            }
        }
    }
    // Build prompt slots if any slot is set
    if so.role.is_some()
        || so.guidelines.is_some()
        || so.response_style.is_some()
        || so.extra.is_some()
    {
        let slots = a3s_code_core::SystemPromptSlots {
            style: None,
            role: so.role,
            guidelines: so.guidelines,
            response_style: so.response_style,
            extra: so.extra,
        };
        o = o.with_prompt_slots(slots);
    }
    // Inline skills registered programmatically via add_instruction / add_persona
    if !so.inline_skills.is_empty() {
        let registry = a3s_code_core::skills::SkillRegistry::new();
        for (name, kind, content) in so.inline_skills {
            let raw = format!("---\nname: {name}\nkind: {kind}\n---\n{content}");
            if let Some(skill) = a3s_code_core::skills::Skill::parse(&raw) {
                registry.register_unchecked(Arc::new(skill));
            } else {
                eprintln!(
                    "a3s-code: failed to parse inline skill '{}' — skipping",
                    name
                );
            }
        }
        o = o.with_skill_registry(Arc::new(registry));
    }
    if let Some(r) = so.max_tool_rounds {
        o = o.with_max_tool_rounds(r);
    }
    if so.planning {
        o = o.with_planning(true);
    }
    if so.goal_tracking {
        o = o.with_goal_tracking(true);
    }
    if let Some(n) = so.max_parse_retries {
        o = o.with_parse_retries(n);
    }
    if let Some(ms) = so.tool_timeout_ms {
        o = o.with_tool_timeout(ms);
    }
    if let Some(n) = so.circuit_breaker_threshold {
        o = o.with_circuit_breaker(n);
    }
    if let Some(t) = so.temperature {
        o = o.with_temperature(t);
    }
    if let Some(budget) = so.thinking_budget {
        o = o.with_thinking_budget(budget);
    }
    if let Some(enabled) = so.continuation_enabled {
        o = o.with_continuation(enabled);
    }
    if let Some(turns) = so.max_continuation_turns {
        o = o.with_max_continuation_turns(turns);
    }
    if let Some(id) = so.session_id {
        o = o.with_session_id(id);
    }
    if so.auto_save {
        o = o.with_auto_save(true);
    }

    // AHP transport configuration
    #[cfg(feature = "ahp")]
    if let Some(ref transport_obj) = so.ahp_transport {
        use a3s_code_core::ahp::{AhpHookExecutor, AhpTransport, AuthConfig};

        let transport = Python::with_gil(|py| {
            // Try stdio transport
            if let Ok(stdio) = transport_obj.extract::<pyo3::PyRef<PyStdioTransport>>(py) {
                return Some(AhpTransport::Stdio {
                    program: stdio.program.clone(),
                    args: stdio.args.clone(),
                });
            }
            // Try HTTP transport
            if let Ok(http) = transport_obj.extract::<pyo3::PyRef<PyHttpTransport>>(py) {
                let auth = http
                    .auth_token
                    .as_ref()
                    .map(|token| AuthConfig::bearer(token.clone()));
                return Some(AhpTransport::Http {
                    url: http.url.clone(),
                    auth,
                });
            }
            // Try WebSocket transport
            if let Ok(ws) = transport_obj.extract::<pyo3::PyRef<PyWebSocketTransport>>(py) {
                let auth = ws
                    .auth_token
                    .as_ref()
                    .map(|token| AuthConfig::bearer(token.clone()));
                return Some(AhpTransport::WebSocket {
                    url: ws.url.clone(),
                    auth,
                });
            }
            // Try Unix socket transport
            #[cfg(unix)]
            if let Ok(unix) = transport_obj.extract::<pyo3::PyRef<PyUnixSocketTransport>>(py) {
                return Some(AhpTransport::UnixSocket {
                    path: unix.path.clone(),
                });
            }
            None
        });

        if let Some(transport) = transport {
            // Create AHP executor asynchronously
            match get_runtime().block_on(AhpHookExecutor::new(transport)) {
                Ok(executor) => {
                    o = o.with_hook_executor(Arc::new(executor));
                }
                Err(e) => {
                    eprintln!(
                        "a3s-code: failed to create AHP executor: {} — continuing without AHP",
                        e
                    );
                }
            }
        }
    }

    Ok(o)
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
    serde_json::from_str::<Vec<RustVerificationCommand>>(&json_str).map_err(|e| {
        PyTypeError::new_err(format!("Invalid verification command format: {e}"))
    })
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
        .map(|l| l.clone())
        .map_err(|e| PyRuntimeError::new_err(format!("Unexpected serialization result: {e}")))
}

// ============================================================================
// SearchConfig
// ============================================================================

/// Configuration for a search engine.
#[pyclass(name = "SearchEngineConfig")]
#[derive(Clone)]
struct PySearchEngineConfig {
    #[pyo3(get, set)]
    enabled: bool,
    #[pyo3(get, set)]
    weight: f64,
    #[pyo3(get, set)]
    timeout: Option<u64>,
}

#[pymethods]
impl PySearchEngineConfig {
    #[new]
    #[pyo3(signature = (enabled=true, weight=1.0, timeout=None))]
    fn new(enabled: bool, weight: f64, timeout: Option<u64>) -> Self {
        Self {
            enabled,
            weight,
            timeout,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "SearchEngineConfig(enabled={}, weight={}, timeout={:?})",
            self.enabled, self.weight, self.timeout
        )
    }
}

impl From<PySearchEngineConfig> for RustSearchEngineConfig {
    fn from(c: PySearchEngineConfig) -> Self {
        Self {
            enabled: c.enabled,
            weight: c.weight,
            timeout: c.timeout,
        }
    }
}

/// Health monitor configuration for search engines.
#[pyclass(name = "SearchHealthConfig")]
#[derive(Clone)]
struct PySearchHealthConfig {
    #[pyo3(get, set)]
    max_failures: u32,
    #[pyo3(get, set)]
    suspend_seconds: u64,
}

#[pymethods]
impl PySearchHealthConfig {
    #[new]
    #[pyo3(signature = (max_failures=3, suspend_seconds=60))]
    fn new(max_failures: u32, suspend_seconds: u64) -> Self {
        Self {
            max_failures,
            suspend_seconds,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "SearchHealthConfig(max_failures={}, suspend_seconds={})",
            self.max_failures, self.suspend_seconds
        )
    }
}

impl From<PySearchHealthConfig> for RustSearchHealthConfig {
    fn from(c: PySearchHealthConfig) -> Self {
        Self {
            max_failures: c.max_failures,
            suspend_seconds: c.suspend_seconds,
        }
    }
}

/// Search engine configuration (a3s-search integration).
#[pyclass(name = "SearchConfig")]
#[derive(Clone)]
struct PySearchConfig {
    #[pyo3(get, set)]
    timeout: u64,
    #[pyo3(get, set)]
    health: Option<PySearchHealthConfig>,
    engines: std::collections::HashMap<String, PySearchEngineConfig>,
    #[pyo3(get, set)]
    headless: Option<PyHeadlessConfig>,
}

#[pymethods]
impl PySearchConfig {
    #[new]
    #[pyo3(signature = (timeout=10, health=None, headless=None))]
    fn new(
        timeout: u64,
        health: Option<PySearchHealthConfig>,
        headless: Option<PyHeadlessConfig>,
    ) -> Self {
        Self {
            timeout,
            health,
            engines: std::collections::HashMap::new(),
            headless,
        }
    }

    /// Set engine configuration.
    fn set_engine(&mut self, name: String, config: PySearchEngineConfig) {
        self.engines.insert(name, config);
    }

    /// Get engine configuration.
    fn get_engine(&self, name: String) -> Option<PySearchEngineConfig> {
        self.engines.get(&name).cloned()
    }

    /// Get all engine names.
    fn engine_names(&self) -> Vec<String> {
        self.engines.keys().cloned().collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "SearchConfig(timeout={}, engines={}, health={:?})",
            self.timeout,
            self.engines.len(),
            self.health.is_some()
        )
    }
}

/// Headless browser backend selection.
#[pyclass(name = "BrowserBackend", eq, eq_int)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PyBrowserBackend {
    /// Chrome/Chromium headless.
    Chrome,
    /// Lightpanda headless browser (Linux/macOS only).
    Lightpanda,
}

impl From<PyBrowserBackend> for RustBrowserBackend {
    fn from(b: PyBrowserBackend) -> Self {
        match b {
            PyBrowserBackend::Chrome => RustBrowserBackend::Chrome,
            PyBrowserBackend::Lightpanda => RustBrowserBackend::Lightpanda,
        }
    }
}

/// Headless browser configuration for JS-rendered search engines.
#[pyclass(name = "HeadlessConfig")]
#[derive(Clone)]
pub struct PyHeadlessConfig {
    #[pyo3(get, set)]
    backend: PyBrowserBackend,
    #[pyo3(get, set)]
    browser_path: Option<String>,
    #[pyo3(get, set)]
    max_tabs: Option<usize>,
    #[pyo3(get, set)]
    launch_args: Option<Vec<String>>,
    #[pyo3(get, set)]
    proxy_url: Option<String>,
}

#[pymethods]
impl PyHeadlessConfig {
    #[new]
    #[pyo3(signature = (backend, browser_path=None, max_tabs=None, launch_args=None, proxy_url=None))]
    fn new(
        backend: PyBrowserBackend,
        browser_path: Option<String>,
        max_tabs: Option<usize>,
        launch_args: Option<Vec<String>>,
        proxy_url: Option<String>,
    ) -> Self {
        Self {
            backend,
            browser_path,
            max_tabs,
            launch_args,
            proxy_url,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "HeadlessConfig(backend={:?}, browser_path={:?}, max_tabs={:?}, launch_args={:?}, proxy_url={:?})",
            self.backend, self.browser_path, self.max_tabs, self.launch_args, self.proxy_url
        )
    }
}

impl From<PyHeadlessConfig> for RustHeadlessConfig {
    fn from(c: PyHeadlessConfig) -> Self {
        Self {
            backend: c.backend.into(),
            browser_path: c.browser_path,
            max_tabs: c.max_tabs.unwrap_or(4),
            launch_args: c.launch_args.unwrap_or_default(),
            proxy_url: c.proxy_url,
        }
    }
}

impl From<PySearchConfig> for RustSearchConfig {
    fn from(c: PySearchConfig) -> Self {
        Self {
            timeout: c.timeout,
            health: c.health.map(|h| h.into()),
            engines: c.engines.into_iter().map(|(k, v)| (k, v.into())).collect(),
            headless: c.headless.map(|h| h.into()),
        }
    }
}

// ============================================================================
// SkillInfo
// ============================================================================

/// Metadata about a built-in skill.
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
// EventType — string constants for AgentEvent.type
// ============================================================================

/// String constants for `AgentEvent.type`.
///
/// Use these instead of raw strings to avoid typos and enable IDE completion:
///
/// ```python
/// from a3s_code import EventType
///
/// for event in session.stream("Refactor this module"):
///     if event.type == EventType.TEXT_DELTA:
///         print(event.text, end="", flush=True)
///     elif event.type == EventType.END:
///         print(f"\nDone. Tokens: {event.total_tokens}")
/// ```
#[pyclass(name = "EventType")]
struct PyEventType;

#[pymethods]
impl PyEventType {
    /// Agent started processing (carries `prompt`).
    #[classattr]
    const START: &'static str = "start";
    /// A new LLM turn began (carries `turn`).
    #[classattr]
    const TURN_START: &'static str = "turn_start";
    /// A chunk of assistant text arrived (carries `text`).
    #[classattr]
    const TEXT_DELTA: &'static str = "text_delta";
    /// A tool call started (carries `tool_id`, `tool_name`).
    #[classattr]
    const TOOL_START: &'static str = "tool_start";
    /// A tool call completed (carries `tool_id`, `tool_name`, `tool_output`, `exit_code`).
    #[classattr]
    const TOOL_END: &'static str = "tool_end";
    /// A streaming chunk from a tool (carries `tool_id`, `tool_name`, `text`).
    #[classattr]
    const TOOL_OUTPUT_DELTA: &'static str = "tool_output_delta";
    /// An LLM turn finished (carries `turn`, `total_tokens`).
    #[classattr]
    const TURN_END: &'static str = "turn_end";
    /// The agent finished (carries `text`, `total_tokens`).
    #[classattr]
    const END: &'static str = "end";
    /// An error occurred (carries `error`).
    #[classattr]
    const ERROR: &'static str = "error";
    /// Human-in-the-loop confirmation required before a tool runs.
    #[classattr]
    const CONFIRMATION_REQUIRED: &'static str = "confirmation_required";
    /// Confirmation response received.
    #[classattr]
    const CONFIRMATION_RECEIVED: &'static str = "confirmation_received";
    /// Confirmation timed out; default action was taken.
    #[classattr]
    const CONFIRMATION_TIMEOUT: &'static str = "confirmation_timeout";
    /// An external lane task is pending (carries `task_id`, `lane`).
    #[classattr]
    const EXTERNAL_TASK_PENDING: &'static str = "external_task_pending";
    /// An external lane task completed.
    #[classattr]
    const EXTERNAL_TASK_COMPLETED: &'static str = "external_task_completed";
    /// A tool was blocked by the permission policy.
    #[classattr]
    const PERMISSION_DENIED: &'static str = "permission_denied";
}

// ============================================================================
// Advanced SubAgent Control Plane
// ============================================================================

/// SubAgent configuration for the advanced orchestrator control plane.
#[pyclass(name = "SubAgentConfig")]
#[derive(Clone)]
struct PySubAgentConfig {
    inner: RustSubAgentConfig,
}

#[pymethods]
impl PySubAgentConfig {
    #[new]
    #[pyo3(signature = (agent_type, prompt, description=None, max_steps=None, timeout_ms=None, parent_id=None, workspace=None, agent_dirs=None, skill_dirs=None))]
    fn new(
        agent_type: String,
        prompt: String,
        description: Option<String>,
        max_steps: Option<usize>,
        timeout_ms: Option<u64>,
        parent_id: Option<String>,
        workspace: Option<String>,
        agent_dirs: Option<Vec<String>>,
        skill_dirs: Option<Vec<String>>,
    ) -> Self {
        let mut config = RustSubAgentConfig::new(agent_type, prompt);
        if let Some(desc) = description {
            config = config.with_description(desc);
        }
        if let Some(steps) = max_steps {
            config = config.with_max_steps(steps);
        }
        if let Some(timeout) = timeout_ms {
            config = config.with_timeout_ms(timeout);
        }
        if let Some(parent) = parent_id {
            config = config.with_parent_id(parent);
        }
        if let Some(ws) = workspace {
            config = config.with_workspace(ws);
        }
        if let Some(dirs) = agent_dirs {
            config = config.with_agent_dirs(dirs);
        }
        if let Some(dirs) = skill_dirs {
            config = config.with_skill_dirs(dirs);
        }
        Self { inner: config }
    }

    fn __repr__(&self) -> String {
        format!(
            "SubAgentConfig(agent_type={:?}, max_steps={:?})",
            self.inner.agent_type, self.inner.max_steps
        )
    }

    // Getters and setters for all fields

    #[getter]
    fn get_agent_type(&self) -> String {
        self.inner.agent_type.clone()
    }

    #[setter]
    fn set_agent_type(&mut self, value: String) {
        self.inner.agent_type = value;
    }

    #[getter]
    fn get_description(&self) -> String {
        self.inner.description.clone()
    }

    #[setter]
    fn set_description(&mut self, value: String) {
        self.inner.description = value;
    }

    #[getter]
    fn get_prompt(&self) -> String {
        self.inner.prompt.clone()
    }

    #[setter]
    fn set_prompt(&mut self, value: String) {
        self.inner.prompt = value;
    }

    #[getter]
    fn get_max_steps(&self) -> Option<usize> {
        self.inner.max_steps
    }

    #[setter]
    fn set_max_steps(&mut self, value: Option<usize>) {
        self.inner.max_steps = value;
    }

    #[getter]
    fn get_timeout_ms(&self) -> Option<u64> {
        self.inner.timeout_ms
    }

    #[setter]
    fn set_timeout_ms(&mut self, value: Option<u64>) {
        self.inner.timeout_ms = value;
    }

    #[getter]
    fn get_parent_id(&self) -> Option<String> {
        self.inner.parent_id.clone()
    }

    #[setter]
    fn set_parent_id(&mut self, value: Option<String>) {
        self.inner.parent_id = value;
    }

    #[getter]
    fn get_workspace(&self) -> String {
        self.inner.workspace.clone()
    }

    #[setter]
    fn set_workspace(&mut self, value: String) {
        self.inner.workspace = value;
    }

    #[getter]
    fn get_agent_dirs(&self) -> Vec<String> {
        self.inner.agent_dirs.clone()
    }

    #[setter]
    fn set_agent_dirs(&mut self, value: Vec<String>) {
        self.inner.agent_dirs = value;
    }

    #[getter]
    fn get_skill_dirs(&self) -> Vec<String> {
        self.inner.skill_dirs.clone()
    }

    #[setter]
    fn set_skill_dirs(&mut self, value: Vec<String>) {
        self.inner.skill_dirs = value;
    }

}

#[cfg(test)]
mod tests {
    use super::{
        parse_agentic_search_results, PyAgenticParseLlmBlock, PyAgenticSearchResult,
    };

    #[test]
    fn python_agentic_search_result_info_parses_match_locators() {
        let results = parse_agentic_search_results(
            r#"{"results":[{"path":"scan.pdf","matches":[{"line_number":12,"content":"body","locator":"page 2 | page 2: 1. Overview","context_before":["intro"],"context_after":["tail"]}],"sampled_lines":[{"line_number":12,"content":"body","locator":"page 2","distance":0,"weight":1.0}]}]}"#,
        )
        .unwrap();

        let parsed = PyAgenticSearchResult::from_json(&results[0]);
        assert_eq!(parsed.matches.len(), 1);
        assert_eq!(
            parsed.matches[0].locator.as_deref(),
            Some("page 2 | page 2: 1. Overview")
        );
        assert_eq!(parsed.matches[0].context_before, vec!["intro".to_string()]);
        assert_eq!(parsed.sampled_lines.len(), 1);
        assert_eq!(parsed.sampled_lines[0].distance, Some(0));
    }

    #[test]
    fn python_agentic_parse_llm_blocks_info_parses_locations() {
        let value = serde_json::json!({
            "index": 1,
            "kind": "section",
            "label": "page 2: 1. Overview",
            "location": {
                "source": "report.pdf",
                "page": 2,
                "ordinal": 4,
                "display": "source=report.pdf, page=2, ordinal=4"
            }
        });

        let parsed = PyAgenticParseLlmBlock::from_json(&value);
        assert_eq!(parsed.index, Some(1));
        assert_eq!(parsed.kind.as_deref(), Some("section"));
        assert_eq!(parsed.label.as_deref(), Some("page 2: 1. Overview"));
        assert_eq!(
            parsed.location.and_then(|loc| loc.display).as_deref(),
            Some("source=report.pdf, page=2, ordinal=4")
        );
    }
}

/// SubAgent handle for control and monitoring.
#[pyclass(name = "SubAgentHandle")]
struct PySubAgentHandle {
    inner: Arc<Mutex<RustSubAgentHandle>>,
}

#[pymethods]
impl PySubAgentHandle {
    #[getter]
    fn id(&self, py: Python<'_>) -> PyResult<String> {
        let handle = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async move {
                let h = handle.lock().await;
                Ok(h.id.clone())
            })
        })
    }

    /// Get current state (non-blocking).
    fn state(&self, py: Python<'_>) -> PyResult<String> {
        let handle = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async move {
                let h = handle.lock().await;
                let state = h.state_async().await;
                Ok(format!("{:?}", state))
            })
        })
    }

    /// Get current activity.
    fn activity(&self, py: Python<'_>) -> PyResult<PySubAgentActivity> {
        let handle = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async move {
                let h = handle.lock().await;
                Ok(h.activity().await.into())
            })
        })
    }

    /// Pause execution.
    fn pause(&self, py: Python<'_>) -> PyResult<()> {
        let handle = self.inner.clone();
        py.allow_threads(move || {
            get_runtime()
                .block_on(async move { handle.lock().await.pause().await })
                .map_err(|e| PyRuntimeError::new_err(format!("Pause failed: {e}")))
        })
    }

    /// Resume execution.
    fn resume(&self, py: Python<'_>) -> PyResult<()> {
        let handle = self.inner.clone();
        py.allow_threads(move || {
            get_runtime()
                .block_on(async move { handle.lock().await.resume().await })
                .map_err(|e| PyRuntimeError::new_err(format!("Resume failed: {e}")))
        })
    }

    /// Cancel execution.
    fn cancel(&self, py: Python<'_>) -> PyResult<()> {
        let handle = self.inner.clone();
        py.allow_threads(move || {
            get_runtime()
                .block_on(async move { handle.lock().await.cancel().await })
                .map_err(|e| PyRuntimeError::new_err(format!("Cancel failed: {e}")))
        })
    }

    /// Wait for completion and get result.
    fn wait(&self, py: Python<'_>) -> PyResult<String> {
        let handle = self.inner.clone();
        py.allow_threads(move || {
            get_runtime()
                .block_on(async move { handle.lock().await.wait().await })
                .map_err(|e| PyRuntimeError::new_err(format!("Wait failed: {e}")))
        })
    }

    /// Subscribe to sub-agent events.
    fn events(&self, py: Python<'_>) -> PyResult<PySubAgentEventStream> {
        let handle = self.inner.clone();
        let stream = py.allow_threads(move || {
            get_runtime().block_on(async move {
                let h = handle.lock().await;
                h.events()
            })
        });
        Ok(PySubAgentEventStream {
            inner: Arc::new(Mutex::new(stream)),
        })
    }

    fn __repr__(&self) -> String {
        "SubAgentHandle(...)".to_string()
    }
}

/// SubAgent event stream for monitoring sub-agent events.
#[pyclass(name = "SubAgentEventStream")]
struct PySubAgentEventStream {
    inner: Arc<Mutex<a3s_code_core::orchestrator::SubAgentEventStream>>,
}

#[pymethods]
impl PySubAgentEventStream {
    /// Receive next event (blocking with timeout).
    #[pyo3(signature = (timeout_ms=None))]
    fn recv(&self, py: Python<'_>, timeout_ms: Option<u64>) -> PyResult<Option<PyObject>> {
        let stream = self.inner.clone();
        let timeout = timeout_ms.unwrap_or(1000);

        py.allow_threads(move || {
            get_runtime().block_on(async move {
                let mut s = stream.lock().await;

                // Try to receive with timeout
                let result =
                    tokio::time::timeout(std::time::Duration::from_millis(timeout), s.recv()).await;

                match result {
                    Ok(Some(event)) => {
                        // Convert event JSON to a real Python dict/list structure.
                        Python::with_gil(|py| match serde_json::to_value(&event) {
                            Ok(json_value) => {
                                let json_mod = py.import("json")?;
                                let json_text =
                                    serde_json::to_string(&json_value).map_err(|e| {
                                        PyRuntimeError::new_err(format!(
                                            "Failed to encode event json: {e}"
                                        ))
                                    })?;
                                let obj = json_mod.call_method1("loads", (json_text,))?;
                                Ok(Some(obj.unbind()))
                            }
                            Err(e) => Err(PyRuntimeError::new_err(format!(
                                "Failed to serialize event: {e}"
                            ))),
                        })
                    }
                    Ok(None) => Ok(None),
                    Err(_) => Ok(None), // Timeout
                }
            })
        })
    }

    fn __repr__(&self) -> String {
        "SubAgentEventStream(...)".to_string()
    }
}

/// SubAgent activity type.
#[pyclass(name = "SubAgentActivity")]
#[derive(Clone)]
struct PySubAgentActivity {
    activity_type: String,
    data: Option<String>,
}

#[pymethods]
impl PySubAgentActivity {
    /// Get activity type (idle, calling_tool, requesting_llm, waiting_for_control).
    #[getter]
    fn activity_type(&self) -> String {
        self.activity_type.clone()
    }

    /// Get activity data (JSON string for tool args, etc.).
    #[getter]
    fn data(&self) -> Option<String> {
        self.data.clone()
    }

    fn __repr__(&self) -> String {
        format!("SubAgentActivity(type={})", self.activity_type)
    }
}

impl From<RustSubAgentActivity> for PySubAgentActivity {
    fn from(activity: RustSubAgentActivity) -> Self {
        match activity {
            RustSubAgentActivity::Idle => Self {
                activity_type: "idle".to_string(),
                data: None,
            },
            RustSubAgentActivity::CallingTool { tool_name, args } => Self {
                activity_type: "calling_tool".to_string(),
                data: Some(
                    serde_json::json!({
                        "tool_name": tool_name,
                        "args": args
                    })
                    .to_string(),
                ),
            },
            RustSubAgentActivity::RequestingLlm { message_count } => Self {
                activity_type: "requesting_llm".to_string(),
                data: Some(
                    serde_json::json!({
                        "message_count": message_count
                    })
                    .to_string(),
                ),
            },
            RustSubAgentActivity::WaitingForControl { reason } => Self {
                activity_type: "waiting_for_control".to_string(),
                data: Some(
                    serde_json::json!({
                        "reason": reason
                    })
                    .to_string(),
                ),
            },
        }
    }
}

/// SubAgent information with metadata and current activity.
#[pyclass(name = "SubAgentInfo")]
#[derive(Clone)]
struct PySubAgentInfo {
    id: String,
    agent_type: String,
    description: String,
    state: String,
    parent_id: Option<String>,
    created_at: u64,
    updated_at: u64,
    current_activity: Option<PySubAgentActivity>,
}

#[pymethods]
impl PySubAgentInfo {
    #[getter]
    fn id(&self) -> String {
        self.id.clone()
    }

    #[getter]
    fn agent_type(&self) -> String {
        self.agent_type.clone()
    }

    #[getter]
    fn description(&self) -> String {
        self.description.clone()
    }

    #[getter]
    fn state(&self) -> String {
        self.state.clone()
    }

    #[getter]
    fn parent_id(&self) -> Option<String> {
        self.parent_id.clone()
    }

    #[getter]
    fn created_at(&self) -> u64 {
        self.created_at
    }

    #[getter]
    fn updated_at(&self) -> u64 {
        self.updated_at
    }

    #[getter]
    fn current_activity(&self) -> Option<PySubAgentActivity> {
        self.current_activity.clone()
    }

    fn __repr__(&self) -> String {
        format!(
            "SubAgentInfo(id={}, type={}, state={})",
            self.id, self.agent_type, self.state
        )
    }
}

impl From<RustSubAgentInfo> for PySubAgentInfo {
    fn from(info: RustSubAgentInfo) -> Self {
        Self {
            id: info.id,
            agent_type: info.agent_type,
            description: info.description,
            state: info.state,
            parent_id: info.parent_id,
            created_at: info.created_at,
            updated_at: info.updated_at,
            current_activity: info.current_activity.map(|a| a.into()),
        }
    }
}

/// Advanced orchestrator for explicit SubAgent lifecycle control.
///
/// Routine multi-agent work should use task/parallel_task delegation; this API
/// is for monitoring and controlling long-running SubAgents directly.
#[pyclass(name = "Orchestrator")]
struct PyOrchestrator {
    inner: Arc<Mutex<RustOrchestrator>>,
}

#[pymethods]
impl PyOrchestrator {
    /// Create a new orchestrator.
    ///
    /// Args:
    ///     agent: `Agent` instance used to execute spawned SubAgents.
    #[staticmethod]
    #[pyo3(signature = (agent))]
    fn create(agent: &PyAgent) -> Self {
        let orch = RustOrchestrator::from_agent(agent.inner.clone());
        Self {
            inner: Arc::new(Mutex::new(orch)),
        }
    }

    /// Spawn a new SubAgent.
    fn spawn_subagent(
        &self,
        py: Python<'_>,
        config: PySubAgentConfig,
    ) -> PyResult<PySubAgentHandle> {
        let orch = self.inner.clone();
        let cfg = config.inner.clone();
        let handle = py
            .allow_threads(move || {
                get_runtime().block_on(async move { orch.lock().await.spawn_subagent(cfg).await })
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Spawn failed: {e}")))?;
        Ok(PySubAgentHandle {
            inner: Arc::new(Mutex::new(handle)),
        })
    }

    /// Get active SubAgent count.
    fn active_count(&self, py: Python<'_>) -> PyResult<usize> {
        let orch = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async move { Ok(orch.lock().await.active_count().await) })
        })
    }

    /// Get all SubAgent information list.
    fn list_subagents(&self, py: Python<'_>) -> PyResult<Vec<PySubAgentInfo>> {
        let orch = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async move {
                let infos = orch.lock().await.list_subagents().await;
                Ok(infos.into_iter().map(|i| i.into()).collect())
            })
        })
    }

    /// Get specific SubAgent information.
    fn get_subagent_info(&self, py: Python<'_>, id: String) -> PyResult<Option<PySubAgentInfo>> {
        let orch = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async move {
                Ok(orch
                    .lock()
                    .await
                    .get_subagent_info(&id)
                    .await
                    .map(|i| i.into()))
            })
        })
    }

    /// Get all active SubAgent activities.
    fn get_active_activities(&self, py: Python<'_>) -> PyResult<Vec<(String, PySubAgentActivity)>> {
        let orch = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async move {
                let activities = orch.lock().await.get_active_activities().await;
                Ok(activities
                    .into_iter()
                    .map(|(id, activity)| (id, activity.into()))
                    .collect())
            })
        })
    }

    /// Get all SubAgent states.
    fn get_all_states(&self, py: Python<'_>) -> PyResult<Vec<(String, String)>> {
        let orch = self.inner.clone();
        py.allow_threads(move || {
            get_runtime().block_on(async move {
                let states = orch.lock().await.get_all_states().await;
                Ok(states
                    .into_iter()
                    .map(|(id, state)| (id, format!("{:?}", state)))
                    .collect())
            })
        })
    }

    /// Pause a SubAgent.
    fn pause_subagent(&self, py: Python<'_>, id: String) -> PyResult<()> {
        let orch = self.inner.clone();
        py.allow_threads(move || {
            get_runtime()
                .block_on(async move { orch.lock().await.pause_subagent(&id).await })
                .map_err(|e| PyRuntimeError::new_err(format!("Pause failed: {e}")))
        })
    }

    /// Resume a SubAgent.
    fn resume_subagent(&self, py: Python<'_>, id: String) -> PyResult<()> {
        let orch = self.inner.clone();
        py.allow_threads(move || {
            get_runtime()
                .block_on(async move { orch.lock().await.resume_subagent(&id).await })
                .map_err(|e| PyRuntimeError::new_err(format!("Resume failed: {e}")))
        })
    }

    /// Cancel a SubAgent.
    fn cancel_subagent(&self, py: Python<'_>, id: String) -> PyResult<()> {
        let orch = self.inner.clone();
        py.allow_threads(move || {
            get_runtime()
                .block_on(async move { orch.lock().await.cancel_subagent(&id).await })
                .map_err(|e| PyRuntimeError::new_err(format!("Cancel failed: {e}")))
        })
    }

    /// Wait for all SubAgents to complete.
    fn wait_all(&self, py: Python<'_>) -> PyResult<()> {
        let orch = self.inner.clone();
        py.allow_threads(move || {
            get_runtime()
                .block_on(async move { orch.lock().await.wait_all().await })
                .map_err(|e| PyRuntimeError::new_err(format!("Wait failed: {e}")))
        })
    }

    fn __repr__(&self) -> String {
        "Orchestrator(...)".to_string()
    }
}

// ============================================================================
// Python Module
// ============================================================================

/// A3S Code - Native AI coding agent library for Python.
#[pymodule(name = "_native")]
fn a3s_code_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyAgent>()?;
    m.add_class::<PySession>()?;
    m.add_class::<PyAgentResult>()?;
    m.add_class::<PyAgentEvent>()?;
    m.add_class::<PyToolResult>()?;
    m.add_class::<PyWebSearchParams>()?;
    m.add_class::<PyAgenticSearchScore>()?;
    m.add_class::<PyAgenticSearchMatch>()?;
    m.add_class::<PyAgenticSearchSampledLine>()?;
    m.add_class::<PyAgenticParseLlmBlockLocation>()?;
    m.add_class::<PyAgenticParseLlmBlock>()?;
    m.add_class::<PyAgenticSearchResult>()?;
    m.add_class::<PyBtwResult>()?;
    m.add_class::<PyEventStream>()?;
    m.add_class::<PySkillInfo>()?;
    m.add_class::<PyFileMemoryStore>()?;
    m.add_class::<PyFileSessionStore>()?;
    m.add_class::<PyMemorySessionStore>()?;
    m.add_class::<PyDefaultSecurityProvider>()?;
    m.add_class::<PySkillPlugin>()?;
    m.add_class::<PyStdioTransport>()?;
    m.add_class::<PyHttpTransport>()?;
    m.add_class::<PyWebSocketTransport>()?;
    m.add_class::<PyUnixSocketTransport>()?;
    m.add_class::<PyPermissionPolicy>()?;
    m.add_class::<PySessionOptions>()?;
    m.add_class::<PySessionQueueConfig>()?;
    m.add_class::<PySearchConfig>()?;
    m.add_class::<PySearchEngineConfig>()?;
    m.add_class::<PySearchHealthConfig>()?;
    m.add_class::<PyBrowserBackend>()?;
    m.add_class::<PyHeadlessConfig>()?;
    m.add_class::<PyEventType>()?;
    // Advanced SubAgent control plane
    m.add_class::<PyOrchestrator>()?;
    m.add_class::<PySubAgentConfig>()?;
    m.add_class::<PySubAgentHandle>()?;
    m.add_class::<PySubAgentEventStream>()?;
    m.add_class::<PySubAgentInfo>()?;
    m.add_class::<PySubAgentActivity>()?;
    // AHP types
    m.add_class::<PyAhpEventType>()?;
    m.add_class::<PyFact>()?;
    m.add_class::<PyMemorySummary>()?;
    m.add_class::<PySessionStats>()?;
    m.add_class::<PyIdleDecision>()?;
    m.add_class::<PyAhpEventContext>()?;
    m.add_class::<PyTargetHints>()?;
    m.add_class::<PyIntentDetectionEvent>()?;
    m.add_class::<PyIntentDetectionDecision>()?;
    m.add_function(wrap_pyfunction!(format_verification_summary, m)?)?;
    m.add_function(wrap_pyfunction!(py_builtin_skills, m)?)?;

    Ok(())
}

/// Return a list of built-in skills compiled into the library.
///
/// Each entry has `name`, `description`, and `kind` (instruction, tool, or agent).
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
