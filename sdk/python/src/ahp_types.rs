//! AHP (Agent Harness Protocol) type bindings for Python SDK
//!
//! Exposes AHP protocol types to Python.

use a3s_code_core::ahp::{
    EventContext as RustEventContext, EventType as RustEventType, Fact as RustFact,
    IdleDecision as RustIdleDecision, IntentDetectionDecision as RustIntentDetectionDecision,
    IntentDetectionEvent as RustIntentDetectionEvent, MemorySummary as RustMemorySummary,
    SessionStats as RustSessionStats, TargetHints as RustTargetHints,
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
                RustEventType::IntentDetection => "intent_detection".to_string(),
                RustEventType::RunLifecycle => "run_lifecycle".to_string(),
                RustEventType::TaskList => "task_list".to_string(),
                RustEventType::Verification => "verification".to_string(),
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
        Self {
            content,
            source,
            confidence,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "Fact(content={:?}, confidence={})",
            self.content, self.confidence
        )
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
        Self {
            memory_type,
            total_items,
            recent_topics,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "MemorySummary(type={}, items={}, topics={})",
            self.memory_type,
            self.total_items,
            self.recent_topics.len()
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
        Self {
            total_actions,
            total_tokens,
            duration_ms,
            error_count,
        }
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
    #[pyo3(signature = (decision, reason=None))]
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
    #[pyo3(signature = (recent_facts=None, memory_summary=None, session_stats=None, current_task=None, capabilities=None))]
    fn new(
        recent_facts: Option<Vec<PyFact>>,
        memory_summary: Option<PyMemorySummary>,
        session_stats: Option<PySessionStats>,
        current_task: Option<String>,
        capabilities: Option<std::collections::HashMap<String, String>>,
    ) -> Self {
        Self {
            recent_facts,
            memory_summary,
            session_stats,
            current_task,
            capabilities,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "AhpEventContext(facts={}, memory={}, stats={}, task={})",
            self.recent_facts.as_ref().map(|f| f.len()).unwrap_or(0),
            self.memory_summary.is_some(),
            self.session_stats.is_some(),
            self.current_task
                .as_ref()
                .map(|t| t.len().min(30))
                .unwrap_or(0)
                > 0
        )
    }
}

impl From<RustEventContext> for PyAhpEventContext {
    fn from(ec: RustEventContext) -> Self {
        Self {
            recent_facts: ec
                .recent_facts
                .map(|facts| facts.into_iter().map(PyFact::from).collect()),
            memory_summary: ec.memory_summary.map(PyMemorySummary::from),
            session_stats: ec.session_stats.map(PySessionStats::from),
            current_task: ec.current_task,
            capabilities: ec.capabilities.map(|caps| {
                caps.into_iter()
                    .map(|(k, v)| (k, serde_json::to_string(&v).unwrap_or_default()))
                    .collect()
            }),
        }
    }
}

// ============================================================================
// TargetHints
// ============================================================================

/// Optional hints about the detected intent target.
#[pyclass(name = "TargetHints")]
#[derive(Clone)]
pub struct PyTargetHints {
    #[pyo3(get)]
    target_type: Option<String>,
    #[pyo3(get)]
    target_name: Option<String>,
    #[pyo3(get)]
    domain: Option<String>,
}

#[pymethods]
impl PyTargetHints {
    #[new]
    #[pyo3(signature = (target_type=None, target_name=None, domain=None))]
    fn new(
        target_type: Option<String>,
        target_name: Option<String>,
        domain: Option<String>,
    ) -> Self {
        Self {
            target_type,
            target_name,
            domain,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "TargetHints(type={:?}, name={:?}, domain={:?})",
            self.target_type, self.target_name, self.domain
        )
    }
}

impl From<RustTargetHints> for PyTargetHints {
    fn from(th: RustTargetHints) -> Self {
        Self {
            target_type: th.target_type,
            target_name: th.target_name,
            domain: th.domain,
        }
    }
}

// ============================================================================
// IntentDetectionEvent
// ============================================================================

/// Intent detection event payload.
#[pyclass(name = "IntentDetectionEvent")]
#[derive(Clone)]
pub struct PyIntentDetectionEvent {
    #[pyo3(get)]
    session_id: String,
    #[pyo3(get)]
    prompt: String,
    #[pyo3(get)]
    workspace: String,
    #[pyo3(get)]
    language_hint: Option<String>,
}

#[pymethods]
impl PyIntentDetectionEvent {
    #[new]
    #[pyo3(signature = (session_id, prompt, workspace, language_hint=None))]
    fn new(
        session_id: String,
        prompt: String,
        workspace: String,
        language_hint: Option<String>,
    ) -> Self {
        Self {
            session_id,
            prompt,
            workspace,
            language_hint,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "IntentDetectionEvent(session_id={}, prompt={}, workspace={}, language_hint={:?})",
            self.session_id,
            &self.prompt[..self.prompt.len().min(30)],
            self.workspace,
            self.language_hint
        )
    }
}

impl From<RustIntentDetectionEvent> for PyIntentDetectionEvent {
    fn from(e: RustIntentDetectionEvent) -> Self {
        Self {
            session_id: e.session_id,
            prompt: e.prompt,
            workspace: e.workspace,
            language_hint: e.language_hint,
        }
    }
}

// ============================================================================
// IntentDetectionDecision
// ============================================================================

/// Decision from harness for intent detection events.
#[pyclass(name = "IntentDetectionDecision")]
#[derive(Clone)]
pub struct PyIntentDetectionDecision {
    decision: String,
    detected_intent: Option<String>,
    confidence: Option<f32>,
    target_hints: Option<PyTargetHints>,
    block_reason: Option<String>,
}

#[pymethods]
impl PyIntentDetectionDecision {
    #[new]
    #[pyo3(signature = (decision, detected_intent=None, confidence=None, target_hints=None, block_reason=None))]
    fn new(
        decision: String,
        detected_intent: Option<String>,
        confidence: Option<f32>,
        target_hints: Option<PyTargetHints>,
        block_reason: Option<String>,
    ) -> Self {
        Self {
            decision,
            detected_intent,
            confidence,
            target_hints,
            block_reason,
        }
    }

    fn __repr__(&self) -> String {
        match &self.block_reason {
            Some(r) => format!("IntentDetectionDecision('block', reason={:?})", r),
            None => format!(
                "IntentDetectionDecision('allow', intent={:?}, confidence={})",
                self.detected_intent,
                self.confidence.unwrap_or(0.0)
            ),
        }
    }

    fn __str__(&self) -> String {
        self.decision.clone()
    }

    fn __eq__(&self, other: &PyIntentDetectionDecision) -> bool {
        self.decision == other.decision
    }

    /// Whether this decision allows (intent detected).
    fn is_allow(&self) -> bool {
        self.decision == "allow"
    }

    /// Whether this decision blocks.
    fn is_block(&self) -> bool {
        self.decision == "block"
    }

    /// Get the detected intent (if allowed).
    fn get_detected_intent(&self) -> Option<String> {
        self.detected_intent.clone()
    }

    /// Get the confidence score (if allowed).
    fn get_confidence(&self) -> Option<f32> {
        self.confidence
    }

    /// Get target hints (if allowed).
    fn get_target_hints(&self) -> Option<PyTargetHints> {
        self.target_hints.clone()
    }

    /// Get the block reason (if blocked).
    fn get_block_reason(&self) -> Option<String> {
        self.block_reason.clone()
    }
}

impl From<RustIntentDetectionDecision> for PyIntentDetectionDecision {
    fn from(id: RustIntentDetectionDecision) -> Self {
        match id {
            RustIntentDetectionDecision::Allow {
                detected_intent,
                confidence,
                target_hints,
            } => Self {
                decision: "allow".to_string(),
                detected_intent: Some(detected_intent),
                confidence: Some(confidence),
                target_hints: target_hints.map(PyTargetHints::from),
                block_reason: None,
            },
            RustIntentDetectionDecision::Block { reason } => Self {
                decision: "block".to_string(),
                detected_intent: None,
                confidence: None,
                target_hints: None,
                block_reason: Some(reason),
            },
        }
    }
}
