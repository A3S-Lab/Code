//! Progress Tracking for Task Execution
//!
//! Provides real-time progress tracking for agent and tool execution,
//! similar to claw-codes's ProgressTracker.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

/// Tool activity record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolActivity {
    /// Tool name
    pub tool_name: String,
    /// When the activity occurred
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Arguments summary (truncated)
    pub args_summary: String,
    /// Whether the tool succeeded
    pub success: bool,
}

impl ToolActivity {
    /// Create a new tool activity
    pub fn new(
        tool_name: impl Into<String>,
        args_summary: impl Into<String>,
        success: bool,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            timestamp: chrono::Utc::now(),
            args_summary: args_summary.into(),
            success,
        }
    }
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskTokenUsage {
    /// Input tokens used
    pub input_tokens: u64,
    /// Output tokens generated
    pub output_tokens: u64,
    /// Cache read tokens
    pub cache_read_tokens: u64,
    /// Cache write tokens
    pub cache_write_tokens: u64,
}

impl TaskTokenUsage {
    /// Total tokens used
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }

    /// Add token usage
    pub fn add(&mut self, other: &TaskTokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
    }
}

/// Agent execution progress
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProgress {
    /// Tool call counts by tool name
    pub tool_counts: HashMap<String, usize>,
    /// Total tool calls
    pub total_tool_calls: usize,
    /// Token usage
    pub token_usage: TaskTokenUsage,
    /// Recent activities (most recent first)
    pub recent_activities: Vec<ToolActivity>,
    /// Time since task started in milliseconds
    pub elapsed_ms: u64,
    /// Whether task is still running
    pub running: bool,
}

impl Default for AgentProgress {
    fn default() -> Self {
        Self {
            tool_counts: HashMap::new(),
            total_tool_calls: 0,
            token_usage: TaskTokenUsage::default(),
            recent_activities: Vec::new(),
            elapsed_ms: 0,
            running: true,
        }
    }
}

/// Progress tracker for real-time progress monitoring
///
/// Tracks tool calls, token usage, and recent activities in real-time.
/// Similar to claw-codes's ProgressTracker.
#[derive(Debug)]
pub struct ProgressTracker {
    /// When tracking started
    start_time: Instant,
    /// Tool call counts
    tool_counts: HashMap<String, usize>,
    /// Token usage
    token_usage: TaskTokenUsage,
    /// Recent activities (sliding window)
    recent_activities: VecDeque<ToolActivity>,
    /// Maximum recent activities to keep
    max_recent: usize,
    /// Whether tracking is active
    active: bool,
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self::new(30) // Default: keep last 30 activities
    }
}

impl ProgressTracker {
    /// Create a new progress tracker
    ///
    /// # Arguments
    ///
    /// * `max_recent` - Maximum number of recent activities to keep
    pub fn new(max_recent: usize) -> Self {
        Self {
            start_time: Instant::now(),
            tool_counts: HashMap::new(),
            token_usage: TaskTokenUsage::default(),
            recent_activities: VecDeque::with_capacity(max_recent),
            max_recent,
            active: true,
        }
    }

    /// Track a tool call
    pub fn track_tool_call(
        &mut self,
        tool_name: impl Into<String>,
        args_summary: impl Into<String>,
        success: bool,
    ) {
        if !self.active {
            return;
        }

        let tool_name = tool_name.into();
        *self.tool_counts.entry(tool_name.clone()).or_insert(0) += 1;

        let activity = ToolActivity::new(&tool_name, args_summary, success);
        self.recent_activities.push_front(activity);

        // Trim to max size
        while self.recent_activities.len() > self.max_recent {
            self.recent_activities.pop_back();
        }
    }

    /// Track token usage
    pub fn track_tokens(&mut self, usage: TaskTokenUsage) {
        if !self.active {
            return;
        }
        self.token_usage.add(&usage);
    }

    /// Get current progress snapshot
    pub fn progress(&self) -> AgentProgress {
        AgentProgress {
            tool_counts: self.tool_counts.clone(),
            total_tool_calls: self.tool_counts.values().sum(),
            token_usage: self.token_usage.clone(),
            recent_activities: self.recent_activities.iter().cloned().collect(),
            elapsed_ms: self.start_time.elapsed().as_millis() as u64,
            running: self.active,
        }
    }

    /// Stop tracking
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Check if tracking is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Reset the tracker
    pub fn reset(&mut self) {
        self.start_time = Instant::now();
        self.tool_counts.clear();
        self.token_usage = TaskTokenUsage::default();
        self.recent_activities.clear();
        self.active = true;
    }

    /// Get tool count for a specific tool
    pub fn get_tool_count(&self, tool_name: &str) -> usize {
        self.tool_counts.get(tool_name).copied().unwrap_or(0)
    }

    /// Get total tool calls
    pub fn total_tool_calls(&self) -> usize {
        self.tool_counts.values().sum()
    }

    /// Get elapsed time
    pub fn elapsed(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_tracker() {
        let mut tracker = ProgressTracker::new(5);

        tracker.track_tool_call("read", "file_path: test.txt", true);
        tracker.track_tool_call("bash", "command: ls", true);
        tracker.track_tool_call("read", "file_path: main.rs", true);

        let progress = tracker.progress();

        assert_eq!(progress.total_tool_calls, 3);
        assert_eq!(tracker.get_tool_count("read"), 2);
        assert_eq!(tracker.get_tool_count("bash"), 1);
        assert!(progress.running);
    }

    #[test]
    fn test_token_usage() {
        let mut usage = TaskTokenUsage::default();
        assert_eq!(usage.total(), 0);

        usage.input_tokens = 100;
        usage.output_tokens = 50;
        assert_eq!(usage.total(), 150);

        let other = TaskTokenUsage {
            input_tokens: 50,
            output_tokens: 25,
            cache_read_tokens: 10,
            cache_write_tokens: 5,
        };
        usage.add(&other);
        assert_eq!(usage.total(), 240);
    }

    #[test]
    fn test_progress_sliding_window() {
        let mut tracker = ProgressTracker::new(3);

        for i in 0..5 {
            tracker.track_tool_call("tool", format!("arg_{}", i), true);
        }

        let progress = tracker.progress();
        assert_eq!(progress.recent_activities.len(), 3);
        // Most recent should be first
        assert_eq!(progress.recent_activities[0].args_summary, "arg_4");
        assert_eq!(progress.recent_activities[2].args_summary, "arg_2");
    }
}
