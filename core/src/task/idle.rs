//! Idle Task - Explicit Memory Consolidation
//!
//! Provides explicit representation of idle-time memory consolidation tasks.
//!
//! ## Idle Phases
//!
//! 1. `Starting` - Idle task initialized
//! 2. `Consolidating` - Memory consolidation in progress
//! 3. `Updating` - Updating semantic/ episodic memory
//! 4. `Completed` - Idle finished, changes committed
//!
//! ## Example
//!
//! ```rust,ignore
//! use a3s_code_core::task::idle::{IdleTask, IdlePhase, IdleTurn};
//!
//! let mut idle = IdleTask::new("no_activity".to_string());
//! idle.add_turn(IdleTurn::default());
//! idle.transition(IdlePhase::Updating);
//! let update = idle.complete();
//! ```

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Idle task phase
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdlePhase {
    /// Idle task just started
    Starting,
    /// Active memory consolidation
    Consolidating,
    /// Updating memory stores
    Updating,
    /// Idle completed
    Completed,
}

impl IdlePhase {
    /// Check if phase is terminal
    pub fn is_terminal(&self) -> bool {
        matches!(self, IdlePhase::Completed)
    }
}

impl Default for IdlePhase {
    fn default() -> Self {
        IdlePhase::Starting
    }
}

impl std::fmt::Display for IdlePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdlePhase::Starting => write!(f, "starting"),
            IdlePhase::Consolidating => write!(f, "consolidating"),
            IdlePhase::Updating => write!(f, "updating"),
            IdlePhase::Completed => write!(f, "completed"),
        }
    }
}

/// A single turn in the idle execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdleTurn {
    /// Turn text (assistant message)
    pub text: String,
    /// Tool calls made during this turn
    pub tool_calls: Vec<IdleToolCall>,
    /// Files touched during this turn
    pub touched_files: Vec<PathBuf>,
    /// Token usage for this turn
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl IdleTurn {
    /// Create a new idle turn
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    /// Add a tool call
    pub fn add_tool_call(&mut self, call: IdleToolCall) {
        self.tool_calls.push(call);
    }

    /// Add a touched file
    pub fn add_touched_file(&mut self, path: impl Into<PathBuf>) {
        self.touched_files.push(path.into());
    }
}

/// Tool call in idle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleToolCall {
    /// Tool name
    pub name: String,
    /// Arguments (truncated)
    pub args_summary: String,
    /// Success/failure
    pub success: bool,
}

/// Memory update produced by idle completion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryUpdate {
    /// Semantic memory updates
    pub semantic_facts: Vec<String>,
    /// Episodic memory entries
    pub episodic_entries: Vec<EpisodicEntry>,
    /// Procedural memory updates
    pub procedural_updates: Vec<String>,
    /// Total tokens consumed
    pub total_tokens: u64,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Episodic memory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicEntry {
    /// When the event occurred
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Event description
    pub description: String,
    /// Related files
    pub related_files: Vec<PathBuf>,
    /// Importance score (0-1)
    pub importance: f32,
}

/// Idle task state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleTask {
    /// Unique task ID
    pub id: uuid::Uuid,
    /// Current phase
    pub phase: IdlePhase,
    /// Reason for idle
    pub reason: String,
    /// Idle execution turns
    pub turns: Vec<IdleTurn>,
    /// All files touched during idle
    pub touched_files: Vec<PathBuf>,
    /// When idle started
    pub start_time: std::time::SystemTime,
    /// When idle ended
    pub end_time: Option<std::time::SystemTime>,
    /// Memory update produced (set on completion)
    pub memory_update: Option<MemoryUpdate>,
    /// Error message if failed
    pub error: Option<String>,
    /// Maximum turns to keep (sliding window)
    max_turns: usize,
}

impl IdleTask {
    /// Create a new idle task
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            phase: IdlePhase::Starting,
            reason: reason.into(),
            turns: Vec::new(),
            touched_files: Vec::new(),
            start_time: std::time::SystemTime::now(),
            end_time: None,
            memory_update: None,
            error: None,
            max_turns: 30,
        }
    }

    /// Add a turn to the idle
    pub fn add_turn(&mut self, mut turn: IdleTurn) {
        // Update touched files
        for file in turn.touched_files.drain(..) {
            if !self.touched_files.contains(&file) {
                self.touched_files.push(file);
            }
        }

        self.turns.push(turn);

        // Trim to max turns (sliding window)
        while self.turns.len() > self.max_turns {
            self.turns.remove(0);
        }
    }

    /// Transition to a new phase
    pub fn transition(&mut self, new_phase: IdlePhase) {
        if new_phase.is_terminal() && !self.phase.is_terminal() {
            self.end_time = Some(std::time::SystemTime::now());
        }
        self.phase = new_phase;
    }

    /// Start consolidation phase
    pub fn start_consolidation(&mut self) {
        self.transition(IdlePhase::Consolidating);
    }

    /// Start update phase
    pub fn start_update(&mut self) {
        self.transition(IdlePhase::Updating);
    }

    /// Complete the idle and produce memory update
    ///
    /// Returns the memory update that should be applied to memory stores.
    pub fn complete(mut self) -> MemoryUpdate {
        self.transition(IdlePhase::Completed);

        let duration_ms = self
            .end_time
            .unwrap_or_else(std::time::SystemTime::now)
            .duration_since(self.start_time)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let total_tokens: u64 = self
            .turns
            .iter()
            .map(|t| t.input_tokens + t.output_tokens)
            .sum();

        // Extract semantic facts from turns
        let semantic_facts: Vec<String> = self
            .turns
            .iter()
            .filter_map(|t| {
                if t.text.len() > 10 {
                    Some(t.text.clone())
                } else {
                    None
                }
            })
            .take(10)
            .collect();

        // Create episodic entries
        let episodic_entries: Vec<EpisodicEntry> = self
            .touched_files
            .iter()
            .map(|f| EpisodicEntry {
                timestamp: chrono::Utc::now(),
                description: format!("File accessed: {}", f.display()),
                related_files: vec![f.clone()],
                importance: 0.5,
            })
            .collect();

        let update = MemoryUpdate {
            semantic_facts,
            episodic_entries,
            procedural_updates: Vec::new(),
            total_tokens,
            duration_ms,
        };

        self.memory_update = Some(update.clone());
        update
    }

    /// Mark idle as failed
    pub fn fail(mut self, error: impl Into<String>) -> Self {
        self.transition(IdlePhase::Completed);
        self.error = Some(error.into());
        self
    }

    /// Check if idle is completed
    pub fn is_completed(&self) -> bool {
        self.phase.is_terminal()
    }

    /// Get recent turns (for UI display)
    pub fn recent_turns(&self, count: usize) -> &[IdleTurn] {
        let start = self.turns.len().saturating_sub(count);
        &self.turns[start..]
    }

    /// Get duration in milliseconds
    pub fn duration_ms(&self) -> Option<u64> {
        self.end_time
            .or_else(|| {
                if self.phase.is_terminal() {
                    Some(std::time::SystemTime::now())
                } else {
                    None
                }
            })
            .and_then(|end| end.duration_since(self.start_time).ok())
            .map(|d| d.as_millis() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idle_lifecycle() {
        let mut idle = IdleTask::new("idle_timeout");

        assert_eq!(idle.phase, IdlePhase::Starting);

        idle.start_consolidation();
        assert_eq!(idle.phase, IdlePhase::Consolidating);

        idle.add_turn(IdleTurn::new("Consolidating memory..."));
        assert_eq!(idle.turns.len(), 1);

        idle.start_update();
        assert_eq!(idle.phase, IdlePhase::Updating);

        idle.add_turn(IdleTurn::new("Updating semantic memory"));
        assert_eq!(idle.turns.len(), 2);

        let update = idle.complete();
        assert!(!update.semantic_facts.is_empty() || !update.episodic_entries.is_empty());
    }

    #[test]
    fn test_idle_sliding_window() {
        let mut idle = IdleTask::new("test");
        idle.max_turns = 3;

        for i in 0..5 {
            idle.add_turn(IdleTurn::new(format!("Turn {}", i)));
        }

        assert_eq!(idle.turns.len(), 3);
        assert_eq!(idle.turns[0].text, "Turn 2");
        assert_eq!(idle.turns[2].text, "Turn 4");
    }

    #[test]
    fn test_idle_touched_files() {
        let mut idle = IdleTask::new("test");

        let mut turn1 = IdleTurn::new("First turn");
        turn1.add_touched_file("/path/to/file1.rs");
        turn1.add_touched_file("/path/to/file2.rs");

        let mut turn2 = IdleTurn::new("Second turn");
        turn2.add_touched_file("/path/to/file1.rs"); // duplicate
        turn2.add_touched_file("/path/to/file3.rs");

        idle.add_turn(turn1);
        idle.add_turn(turn2);

        // Should have only unique files
        assert_eq!(idle.touched_files.len(), 3);
        assert!(idle
            .touched_files
            .contains(&PathBuf::from("/path/to/file1.rs")));
        assert!(idle
            .touched_files
            .contains(&PathBuf::from("/path/to/file2.rs")));
        assert!(idle
            .touched_files
            .contains(&PathBuf::from("/path/to/file3.rs")));
    }
}
