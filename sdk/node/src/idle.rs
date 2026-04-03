//! Idle (memory consolidation) bindings for Node.js SDK
//!
//! Exposes a3s-code-core idle types to Node.js via napi-rs.

use a3s_code_core::task::idle::{
    IdlePhase as RustIdlePhase, IdleTask as RustIdleTask, IdleToolCall as RustIdleToolCall,
    IdleTurn as RustIdleTurn, EpisodicEntry as RustEpisodicEntry,
    MemoryUpdate as RustMemoryUpdate,
};

// ============================================================================
// IdlePhase
// ============================================================================

/// Phase of an idle task.
#[napi(object)]
#[derive(Clone)]
pub struct IdlePhase {
    /// Phase string: "starting", "consolidating", "updating", "completed"
    pub phase: String,
}

impl From<RustIdlePhase> for IdlePhase {
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
#[napi(object)]
#[derive(Clone)]
pub struct IdleToolCall {
    pub name: String,
    pub args_summary: String,
    pub success: bool,
}

impl From<RustIdleToolCall> for IdleToolCall {
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
#[napi(object)]
#[derive(Clone)]
pub struct IdleTurn {
    pub text: String,
    pub tool_calls: Vec<IdleToolCall>,
    pub touched_files: Vec<String>,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl From<RustIdleTurn> for IdleTurn {
    fn from(turn: RustIdleTurn) -> Self {
        Self {
            text: turn.text,
            tool_calls: turn.tool_calls.into_iter().map(IdleToolCall::from).collect(),
            touched_files: turn.touched_files.into_iter().map(|p| p.display().to_string()).collect(),
            input_tokens: turn.input_tokens as u32,
            output_tokens: turn.output_tokens as u32,
        }
    }
}

// ============================================================================
// MemoryUpdate
// ============================================================================

/// Memory update produced by idle completion.
#[napi(object)]
#[derive(Clone)]
pub struct MemoryUpdate {
    pub semantic_facts: Vec<String>,
    pub episodic_entries: Vec<EpisodicEntry>,
    pub procedural_updates: Vec<String>,
    pub total_tokens: u32,
    pub duration_ms: u32,
}

impl From<RustMemoryUpdate> for MemoryUpdate {
    fn from(update: RustMemoryUpdate) -> Self {
        Self {
            semantic_facts: update.semantic_facts,
            episodic_entries: update
                .episodic_entries
                .into_iter()
                .map(EpisodicEntry::from)
                .collect(),
            procedural_updates: update.procedural_updates,
            total_tokens: update.total_tokens as u32,
            duration_ms: update.duration_ms as u32,
        }
    }
}

// ============================================================================
// EpisodicEntry
// ============================================================================

/// Episodic memory entry from idle.
#[napi(object)]
#[derive(Clone)]
pub struct EpisodicEntry {
    pub timestamp: String,
    pub description: String,
    pub related_files: Vec<String>,
    pub importance: f64,
}

impl From<RustEpisodicEntry> for EpisodicEntry {
    fn from(entry: RustEpisodicEntry) -> Self {
        Self {
            timestamp: entry.timestamp.to_rfc3339(),
            description: entry.description,
            related_files: entry
                .related_files
                .into_iter()
                .map(|p| p.display().to_string())
                .collect(),
            importance: entry.importance as f64,
        }
    }
}

// ============================================================================
// IdleTask
// ============================================================================

/// Idle (memory consolidation) task state.
#[napi(object)]
#[derive(Clone)]
pub struct IdleTask {
    pub id: String,
    pub phase: IdlePhase,
    pub reason: String,
    pub turns: Vec<IdleTurn>,
    pub touched_files: Vec<String>,
    pub error: Option<String>,
}

impl From<RustIdleTask> for IdleTask {
    fn from(idle: RustIdleTask) -> Self {
        Self {
            id: idle.id.to_string(),
            phase: IdlePhase::from(idle.phase),
            reason: idle.reason,
            turns: idle.turns.into_iter().map(IdleTurn::from).collect(),
            touched_files: idle
                .touched_files
                .into_iter()
                .map(|p| p.display().to_string())
                .collect(),
            error: idle.error,
        }
    }
}
