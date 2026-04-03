//! Idle (memory consolidation) bindings for Python SDK
//!
//! Exposes a3s-code-core idle types to Python.

use a3s_code_core::task::idle::{
    IdlePhase as RustIdlePhase, IdleTask as RustIdleTask, IdleToolCall as RustIdleToolCall,
    IdleTurn as RustIdleTurn, EpisodicEntry as RustEpisodicEntry,
    MemoryUpdate as RustMemoryUpdate,
};
use pyo3::prelude::*;

// ============================================================================
// IdlePhase
// ============================================================================

/// Phase of an idle task.
#[pyclass(name = "IdlePhase")]
#[derive(Clone)]
pub struct PyIdlePhase {
    phase: String,
}

#[pymethods]
impl PyIdlePhase {
    fn __repr__(&self) -> String {
        format!("IdlePhase('{}')", self.phase)
    }

    fn __str__(&self) -> String {
        self.phase.clone()
    }

    fn __eq__(&self, other: &PyIdlePhase) -> bool {
        self.phase == other.phase
    }
}

impl From<RustIdlePhase> for PyIdlePhase {
    fn from(phase: RustIdlePhase) -> Self {
        Self {
            phase: match phase {
                RustIdlePhase::Starting => "starting".to_string(),
                RustIdlePhase::Consolidating => "consolidating".to_string(),
                RustIdlePhase::Updating => "updating".to_string(),
                RustIdlePhase::Completed => "completed".to_string(),
            },
        }
    }
}

// ============================================================================
// IdleToolCall
// ============================================================================

/// Tool call recorded during an idle turn.
#[pyclass(name = "IdleToolCall")]
#[derive(Clone)]
pub struct PyIdleToolCall {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    args_summary: String,
    #[pyo3(get)]
    success: bool,
}

#[pymethods]
impl PyIdleToolCall {
    fn __repr__(&self) -> String {
        format!(
            "IdleToolCall(name='{}', success={})",
            self.name, self.success
        )
    }
}

impl From<RustIdleToolCall> for PyIdleToolCall {
    fn from(call: RustIdleToolCall) -> Self {
        Self {
            name: call.name,
            args_summary: call.args_summary,
            success: call.success,
        }
    }
}

// ============================================================================
// IdleTurn
// ============================================================================

/// A single turn in idle execution.
#[pyclass(name = "IdleTurn")]
#[derive(Clone)]
pub struct PyIdleTurn {
    #[pyo3(get)]
    text: String,
    #[pyo3(get)]
    tool_calls: Vec<PyIdleToolCall>,
    #[pyo3(get)]
    touched_files: Vec<String>,
    #[pyo3(get)]
    input_tokens: u64,
    #[pyo3(get)]
    output_tokens: u64,
}

#[pymethods]
impl PyIdleTurn {
    fn __repr__(&self) -> String {
        format!(
            "IdleTurn(text={:?}, tool_calls={}, files={})",
            if self.text.len() > 30 {
                format!("{}...", &self.text[..30])
            } else {
                self.text.clone()
            },
            self.tool_calls.len(),
            self.touched_files.len()
        )
    }
}

impl From<RustIdleTurn> for PyIdleTurn {
    fn from(turn: RustIdleTurn) -> Self {
        Self {
            text: turn.text,
            tool_calls: turn.tool_calls.into_iter().map(PyIdleToolCall::from).collect(),
            touched_files: turn.touched_files.into_iter().map(|p| p.display().to_string()).collect(),
            input_tokens: turn.input_tokens,
            output_tokens: turn.output_tokens,
        }
    }
}

// ============================================================================
// MemoryUpdate
// ============================================================================

/// Memory update produced by idle completion.
#[pyclass(name = "MemoryUpdate")]
#[derive(Clone)]
pub struct PyMemoryUpdate {
    #[pyo3(get)]
    semantic_facts: Vec<String>,
    #[pyo3(get)]
    episodic_entries: Vec<PyEpisodicEntry>,
    #[pyo3(get)]
    procedural_updates: Vec<String>,
    #[pyo3(get)]
    total_tokens: u64,
    #[pyo3(get)]
    duration_ms: u64,
}

#[pymethods]
impl PyMemoryUpdate {
    fn __repr__(&self) -> String {
        format!(
            "MemoryUpdate(facts={}, episodes={}, tokens={})",
            self.semantic_facts.len(),
            self.episodic_entries.len(),
            self.total_tokens
        )
    }
}

impl From<RustMemoryUpdate> for PyMemoryUpdate {
    fn from(update: RustMemoryUpdate) -> Self {
        Self {
            semantic_facts: update.semantic_facts,
            episodic_entries: update
                .episodic_entries
                .into_iter()
                .map(PyEpisodicEntry::from)
                .collect(),
            procedural_updates: update.procedural_updates,
            total_tokens: update.total_tokens,
            duration_ms: update.duration_ms,
        }
    }
}

// ============================================================================
// EpisodicEntry
// ============================================================================

/// Episodic memory entry from idle.
#[pyclass(name = "EpisodicEntry")]
#[derive(Clone)]
pub struct PyEpisodicEntry {
    #[pyo3(get)]
    timestamp: String,
    #[pyo3(get)]
    description: String,
    #[pyo3(get)]
    related_files: Vec<String>,
    #[pyo3(get)]
    importance: f32,
}

#[pymethods]
impl PyEpisodicEntry {
    fn __repr__(&self) -> String {
        format!(
            "EpisodicEntry(description={:?}, importance={})",
            if self.description.len() > 40 {
                format!("{}...", &self.description[..40])
            } else {
                self.description.clone()
            },
            self.importance
        )
    }
}

impl From<RustEpisodicEntry> for PyEpisodicEntry {
    fn from(entry: RustEpisodicEntry) -> Self {
        Self {
            timestamp: entry.timestamp.to_rfc3339(),
            description: entry.description,
            related_files: entry
                .related_files
                .into_iter()
                .map(|p| p.display().to_string())
                .collect(),
            importance: entry.importance,
        }
    }
}

// ============================================================================
// IdleTask
// ============================================================================

/// Idle (memory consolidation) task state.
#[pyclass(name = "IdleTask")]
#[derive(Clone)]
pub struct PyIdleTask {
    #[pyo3(get)]
    id: String,
    #[pyo3(get)]
    phase: PyIdlePhase,
    #[pyo3(get)]
    reason: String,
    #[pyo3(get)]
    turns: Vec<PyIdleTurn>,
    #[pyo3(get)]
    touched_files: Vec<String>,
    #[pyo3(get)]
    error: Option<String>,
}

#[pymethods]
impl PyIdleTask {
    fn __repr__(&self) -> String {
        format!(
            "IdleTask(id='{}', phase={}, reason={:?})",
            self.id,
            self.phase.phase,
            if self.reason.len() > 30 {
                format!("{}...", &self.reason[..30])
            } else {
                self.reason.clone()
            }
        )
    }

    /// Get recent turns (most recent last).
    fn recent_turns(&self, count: usize) -> Vec<PyIdleTurn> {
        let start = self.turns.len().saturating_sub(count);
        self.turns[start..].to_vec()
    }
}

impl From<RustIdleTask> for PyIdleTask {
    fn from(idle: RustIdleTask) -> Self {
        Self {
            id: idle.id.to_string(),
            phase: PyIdlePhase::from(idle.phase),
            reason: idle.reason,
            turns: idle.turns.into_iter().map(PyIdleTurn::from).collect(),
            touched_files: idle
                .touched_files
                .into_iter()
                .map(|p| p.display().to_string())
                .collect(),
            error: idle.error,
        }
    }
}
