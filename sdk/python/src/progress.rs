//! Progress tracking bindings for Python SDK
//!
//! Exposes a3s-code-core progress tracking types to Python.

use a3s_code_core::task::progress::{
    AgentProgress as RustAgentProgress, ProgressTracker as RustProgressTracker,
    TaskTokenUsage as RustTaskTokenUsage, ToolActivity as RustToolActivity,
};
use pyo3::prelude::*;
use std::collections::HashMap;

use crate::truncate_utf8;

// ============================================================================
// TaskTokenUsage
// ============================================================================

/// Token usage statistics for a task.
#[pyclass(name = "TaskTokenUsage")]
#[derive(Clone, Default)]
pub struct PyTaskTokenUsage {
    #[pyo3(get, set)]
    input_tokens: u64,
    #[pyo3(get, set)]
    output_tokens: u64,
    #[pyo3(get, set)]
    cache_read_tokens: u64,
    #[pyo3(get, set)]
    cache_write_tokens: u64,
}

#[pymethods]
impl PyTaskTokenUsage {
    #[new]
    #[pyo3(signature = (input_tokens=0, output_tokens=0, cache_read_tokens=0, cache_write_tokens=0))]
    fn new(
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "TaskTokenUsage(input={}, output={}, cache_read={}, cache_write={}, total={})",
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
            self.total()
        )
    }

    /// Total tokens used.
    fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

impl From<RustTaskTokenUsage> for PyTaskTokenUsage {
    fn from(usage: RustTaskTokenUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
        }
    }
}

// ============================================================================
// ToolActivity
// ============================================================================

/// Record of a single tool activity.
#[pyclass(name = "ToolActivity")]
#[derive(Clone)]
pub struct PyToolActivity {
    #[pyo3(get)]
    tool_name: String,
    #[pyo3(get)]
    timestamp: String,
    #[pyo3(get)]
    args_summary: String,
    #[pyo3(get)]
    success: bool,
}

#[pymethods]
impl PyToolActivity {
    fn __repr__(&self) -> String {
        format!(
            "ToolActivity(tool='{}', success={}, args={:?})",
            self.tool_name,
            self.success,
            if self.args_summary.len() > 40 {
                format!("{}...", truncate_utf8(&self.args_summary, 40))
            } else {
                self.args_summary.clone()
            }
        )
    }
}

impl From<RustToolActivity> for PyToolActivity {
    fn from(activity: RustToolActivity) -> Self {
        Self {
            tool_name: activity.tool_name,
            timestamp: activity.timestamp.to_rfc3339(),
            args_summary: activity.args_summary,
            success: activity.success,
        }
    }
}

// ============================================================================
// AgentProgress
// ============================================================================

/// Snapshot of agent execution progress.
#[pyclass(name = "AgentProgress")]
#[derive(Clone)]
pub struct PyAgentProgress {
    #[pyo3(get)]
    tool_counts: HashMap<String, usize>,
    #[pyo3(get)]
    total_tool_calls: usize,
    #[pyo3(get)]
    token_usage: PyTaskTokenUsage,
    #[pyo3(get)]
    recent_activities: Vec<PyToolActivity>,
    #[pyo3(get)]
    elapsed_ms: u64,
    #[pyo3(get)]
    running: bool,
}

#[pymethods]
impl PyAgentProgress {
    fn __repr__(&self) -> String {
        format!(
            "AgentProgress(tool_calls={}, tokens={}, elapsed_ms={}, running={})",
            self.total_tool_calls,
            self.token_usage.total(),
            self.elapsed_ms,
            self.running
        )
    }
}

impl From<RustAgentProgress> for PyAgentProgress {
    fn from(progress: RustAgentProgress) -> Self {
        Self {
            tool_counts: progress.tool_counts,
            total_tool_calls: progress.total_tool_calls,
            token_usage: PyTaskTokenUsage::from(progress.token_usage),
            recent_activities: progress
                .recent_activities
                .into_iter()
                .map(PyToolActivity::from)
                .collect(),
            elapsed_ms: progress.elapsed_ms,
            running: progress.running,
        }
    }
}

// ============================================================================
// ProgressTracker
// ============================================================================

/// Real-time progress tracker for agent execution.
///
/// Note: ProgressTracker is owned by the Agent/Session internally.
/// Use `session.get_progress()` to get a progress snapshot.
#[pyclass(name = "ProgressTracker")]
pub struct PyProgressTracker {
    #[pyo3(get)]
    total_tool_calls: usize,
    #[pyo3(get)]
    token_usage: PyTaskTokenUsage,
    #[pyo3(get)]
    elapsed_ms: u64,
}

#[pymethods]
impl PyProgressTracker {
    fn __repr__(&self) -> String {
        format!(
            "ProgressTracker(tool_calls={}, tokens={}, elapsed_ms={})",
            self.total_tool_calls,
            self.token_usage.total(),
            self.elapsed_ms
        )
    }

    /// Get current progress snapshot.
    fn progress(&self) -> PyAgentProgress {
        PyAgentProgress {
            tool_counts: HashMap::new(),
            total_tool_calls: self.total_tool_calls,
            token_usage: self.token_usage.clone(),
            recent_activities: Vec::new(),
            elapsed_ms: self.elapsed_ms,
            running: true,
        }
    }
}
