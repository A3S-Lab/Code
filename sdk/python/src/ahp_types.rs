//! AHP (Agent Harness Protocol) type bindings for Python SDK
//!
//! Exposes AHP protocol types to Python.

use a3s_code_core::ahp::{
    EventContext as RustEventContext, EventType as RustEventType, Fact as RustFact,
    IdleDecision as RustIdleDecision, MemorySummary as RustMemorySummary,
    SessionStats as RustSessionStats,
};
use pyo3::prelude::*;
use std::collections::HashMap;

// ============================================================================
// AhpEventType
// ============================================================================

/// AHP event type.
#[pyclass(name = "AhpEventType")]
#[derive(Clone)]
pub struct PyAhpEventType {
    event_type: String,
}

#[pymethods]
impl PyAhpEventType {
    #[new]
    fn new(event_type: String) -> Self {
        Self { event_type }
    }

    fn __repr__(&self) -> String {
        format!("AhpEventType('{}')", self.event_type)
    }

    fn __str__(&self) -> String {
        self.event_type.clone()
    }

    fn __eq__(&self, other: &PyAhpEventType) -> bool {
        self.event_type == other.event_type
    }
}

impl From<RustEventType> for PyAhpEventType {
    fn from(et: RustEventType) -> Self {
        Self {
            event_type: match et {
                RustEventType::Handshake => "handshake".to_string(),
                RustEventType::PreAction => "pre_action".to_string(),
                RustEventType::PostAction => "post_action".to_string(),
                RustEventType::PrePrompt => "pre_prompt".to_string(),
                RustEventType::PostResponse => "post_response".to_string(),
                RustEventType::SessionStart => "session_start".to_string(),
                RustEventType::SessionEnd => "session_end".to_string(),
                RustEventType::Error => "error".to_string(),
                RustEventType::Query => "query".to_string(),
                RustEventType::Heartbeat => "heartbeat".to_string(),
                RustEventType::Idle => "idle".to_string(),
                RustEventType::ContextPerception => "context_perception".to_string(),
                RustEventType::Success => "success".to_string(),
                RustEventType::MemoryRecall => "memory_recall".to_string(),
                RustEventType::Planning => "planning".to_string(),
                RustEventType::Reasoning => "reasoning".to_string(),
                RustEventType::RateLimit => "rate_limit".to_string(),
                RustEventType::Confirmation => "confirmation".to_string(),
            },
        }
    }
}

// ============================================================================
// Fact
// ============================================================================

/// A factual memory item.
#[pyclass(name = "Fact")]
#[derive(Clone)]
pub struct PyFact {
    #[pyo3(get)]
    content: String,
    #[pyo3(get)]
    source: String,
    #[pyo3(get)]
    confidence: f32,
}

#[pymethods]
impl PyFact {
    #[new]
    fn new(content: String, source: String, confidence: f32) -> Self {
        Self { content, source, confidence }
    }

    fn __repr__(&self) -> String {
        format!("Fact(content={:?}, confidence={})", self.content, self.confidence)
    }
}

impl From<RustFact> for PyFact {
    fn from(fact: RustFact) -> Self {
        Self {
            content: fact.content,
            source: fact.source,
            confidence: fact.confidence,
        }
    }
}

// ============================================================================
// MemorySummary
// ============================================================================

/// Memory state summary.
#[pyclass(name = "MemorySummary")]
#[derive(Clone)]
pub struct PyMemorySummary {
    #[pyo3(get)]
    memory_type: String,
    #[pyo3(get)]
    total_items: usize,
    #[pyo3(get)]
    recent_topics: Vec<String>,
}

#[pymethods]
impl PyMemorySummary {
    #[new]
    fn new(memory_type: String, total_items: usize, recent_topics: Vec<String>) -> Self {
        Self { memory_type, total_items, recent_topics }
    }

    fn __repr__(&self) -> String {
        format!(
            "MemorySummary(type={}, items={}, topics={})",
            self.memory_type, self.total_items, self.recent_topics.len()
        )
    }
}

impl From<RustMemorySummary> for PyMemorySummary {
    fn from(ms: RustMemorySummary) -> Self {
        Self {
            memory_type: ms.memory_type,
            total_items: ms.total_items,
            recent_topics: ms.recent_topics,
        }
    }
}

// ============================================================================
// SessionStats
// ============================================================================

/// Session statistics.
#[pyclass(name = "SessionStats")]
#[derive(Clone)]
pub struct PySessionStats {
    #[pyo3(get)]
    total_actions: usize,
    #[pyo3(get)]
    total_tokens: i32,
    #[pyo3(get)]
    duration_ms: u64,
    #[pyo3(get)]
    error_count: usize,
}

#[pymethods]
impl PySessionStats {
    #[new]
    fn new(total_actions: usize, total_tokens: i32, duration_ms: u64, error_count: usize) -> Self {
        Self { total_actions, total_tokens, duration_ms, error_count }
    }

    fn __repr__(&self) -> String {
        format!(
            "SessionStats(actions={}, tokens={}, duration_ms={}, errors={})",
            self.total_actions, self.total_tokens, self.duration_ms, self.error_count
        )
    }
}

impl From<RustSessionStats> for PySessionStats {
    fn from(ss: RustSessionStats) -> Self {
        Self {
            total_actions: ss.total_actions,
            total_tokens: ss.total_tokens,
            duration_ms: ss.duration_ms,
            error_count: ss.error_count,
        }
    }
}

// ============================================================================
// IdleDecision
// ============================================================================

/// Decision from harness for idle events.
#[pyclass(name = "IdleDecision")]
#[derive(Clone)]
pub struct PyIdleDecision {
    decision: String,
    reason: Option<String>,
}

#[pymethods]
impl PyIdleDecision {
    #[new]
    fn new(decision: String, reason: Option<String>) -> Self {
        Self { decision, reason }
    }

    fn __repr__(&self) -> String {
        match &self.reason {
            Some(r) => format!("IdleDecision('{}', reason={:?})", self.decision, r),
            None => format!("IdleDecision('{}')", self.decision),
        }
    }

    fn __str__(&self) -> String {
        self.decision.clone()
    }

    fn __eq__(&self, other: &PyIdleDecision) -> bool {
        self.decision == other.decision
    }

    /// Whether this decision allows idle processing.
    fn is_allow(&self) -> bool {
        self.decision == "allow"
    }

    /// Whether this decision defers idle processing.
    fn is_defer(&self) -> bool {
        self.decision == "defer"
    }

    /// Get the defer reason if any.
    fn get_reason(&self) -> Option<String> {
        self.reason.clone()
    }
}

impl From<RustIdleDecision> for PyIdleDecision {
    fn from(id: RustIdleDecision) -> Self {
        match id {
            RustIdleDecision::Allow => Self {
                decision: "allow".to_string(),
                reason: None,
            },
            RustIdleDecision::Defer { reason } => Self {
                decision: "defer".to_string(),
                reason,
            },
        }
    }
}

// ============================================================================
// AhpEventContext
// ============================================================================

/// Context passed with AHP events.
#[pyclass(name = "AhpEventContext")]
#[derive(Clone)]
pub struct PyAhpEventContext {
    #[pyo3(get)]
    recent_facts: Option<Vec<PyFact>>,
    #[pyo3(get)]
    memory_summary: Option<PyMemorySummary>,
    #[pyo3(get)]
    session_stats: Option<PySessionStats>,
    #[pyo3(get)]
    current_task: Option<String>,
    #[pyo3(get)]
    capabilities: Option<HashMap<String, String>>,
}

#[pymethods]
impl PyAhpEventContext {
    #[new]
    fn new(
        recent_facts: Option<Vec<PyFact>>,
        memory_summary: Option<PyMemorySummary>,
        session_stats: Option<PySessionStats>,
        current_task: Option<String>,
        capabilities: Option<std::collections::HashMap<String, String>>,
    ) -> Self {
        Self { recent_facts, memory_summary, session_stats, current_task, capabilities }
    }

    fn __repr__(&self) -> String {
        format!(
            "AhpEventContext(facts={}, memory={}, stats={}, task={})",
            self.recent_facts.as_ref().map(|f| f.len()).unwrap_or(0),
            self.memory_summary.is_some(),
            self.session_stats.is_some(),
            self.current_task.as_ref().map(|t| t.len().min(30)).unwrap_or(0) > 0
        )
    }
}

impl From<RustEventContext> for PyAhpEventContext {
    fn from(ec: RustEventContext) -> Self {
        Self {
            recent_facts: ec.recent_facts.map(|facts| facts.into_iter().map(PyFact::from).collect()),
            memory_summary: ec.memory_summary.map(PyMemorySummary::from),
            session_stats: ec.session_stats.map(PySessionStats::from),
            current_task: ec.current_task,
            capabilities: ec.capabilities.map(|caps| {
                caps.into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            serde_json::to_string(&v).unwrap_or_default(),
                        )
                    })
                    .collect()
            }),
        }
    }
}
