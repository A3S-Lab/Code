//! AHP (Agent Harness Protocol) type bindings for Node.js SDK
//!
//! Exposes AHP protocol types to Node.js via napi-rs.

#![allow(dead_code)]

use a3s_code_core::ahp::{
    EventContext as RustEventContext, EventType as RustEventType, Fact as RustFact,
    IdleDecision as RustIdleDecision, MemorySummary as RustMemorySummary,
    SessionStats as RustSessionStats,
};
use std::collections::HashMap;

// ============================================================================
// AhpEventType
// ============================================================================

/// AHP event type.
#[napi(object)]
#[derive(Clone)]
pub struct AhpEventType {
    /// Event type string: "handshake", "pre_action", "post_action", "pre_prompt", "post_response", "session_start", "session_end", "error", "query", "heartbeat", "idle"
    pub event_type: String,
}

impl From<RustEventType> for AhpEventType {
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
            },
        }
    }
}

// ============================================================================
// AhpFact
// ============================================================================

/// A factual memory item.
#[napi(object)]
#[derive(Clone)]
pub struct AhpFact {
    pub content: String,
    pub source: String,
    pub confidence: f64,
}

impl From<RustFact> for AhpFact {
    fn from(fact: RustFact) -> Self {
        Self {
            content: fact.content,
            source: fact.source,
            confidence: fact.confidence as f64,
        }
    }
}

// ============================================================================
// AhpMemorySummary
// ============================================================================

/// Memory state summary.
#[napi(object)]
#[derive(Clone)]
pub struct AhpMemorySummary {
    pub memory_type: String,
    pub total_items: u32,
    pub recent_topics: Vec<String>,
}

impl From<RustMemorySummary> for AhpMemorySummary {
    fn from(ms: RustMemorySummary) -> Self {
        Self {
            memory_type: ms.memory_type,
            total_items: ms.total_items as u32,
            recent_topics: ms.recent_topics,
        }
    }
}

// ============================================================================
// AhpSessionStats
// ============================================================================

/// Session statistics.
#[napi(object)]
#[derive(Clone)]
pub struct AhpSessionStats {
    pub total_actions: u32,
    pub total_tokens: u32,
    pub duration_ms: u32,
    pub error_count: u32,
}

impl From<RustSessionStats> for AhpSessionStats {
    fn from(ss: RustSessionStats) -> Self {
        Self {
            total_actions: ss.total_actions as u32,
            total_tokens: ss.total_tokens as u32,
            duration_ms: ss.duration_ms as u32,
            error_count: ss.error_count as u32,
        }
    }
}

// ============================================================================
// AhpIdleDecision
// ============================================================================

/// Decision from harness for idle events.
#[napi(object)]
#[derive(Clone)]
pub struct AhpIdleDecision {
    /// Decision string: "allow" or "defer"
    pub decision: String,
    /// Reason if deferred
    pub reason: Option<String>,
}

impl From<RustIdleDecision> for AhpIdleDecision {
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
#[napi(object)]
#[derive(Clone)]
pub struct AhpEventContext {
    pub recent_facts: Option<Vec<AhpFact>>,
    pub memory_summary: Option<AhpMemorySummary>,
    pub session_stats: Option<AhpSessionStats>,
    pub current_task: Option<String>,
    pub capabilities: Option<HashMap<String, String>>,
}

impl From<RustEventContext> for AhpEventContext {
    fn from(ec: RustEventContext) -> Self {
        Self {
            recent_facts: ec
                .recent_facts
                .map(|facts| facts.into_iter().map(AhpFact::from).collect()),
            memory_summary: ec.memory_summary.map(AhpMemorySummary::from),
            session_stats: ec.session_stats.map(AhpSessionStats::from),
            current_task: ec.current_task,
            capabilities: ec.capabilities.map(|caps| {
                caps.into_iter()
                    .map(|(k, v)| (k, serde_json::to_string(&v).unwrap_or_default()))
                    .collect()
            }),
        }
    }
}
