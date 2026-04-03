//! Progress tracking bindings for Node.js SDK
//!
//! Exposes a3s-code-core progress tracking types to Node.js via napi-rs.

use a3s_code_core::task::progress::{
    AgentProgress as RustAgentProgress, TaskTokenUsage as RustTaskTokenUsage,
    ToolActivity as RustToolActivity,
};
use std::collections::HashMap;

// ============================================================================
// TaskTokenUsage
// ============================================================================

/// Token usage statistics for a task.
#[napi(object)]
#[derive(Clone)]
pub struct TaskTokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_write_tokens: u32,
}

impl From<RustTaskTokenUsage> for TaskTokenUsage {
    fn from(usage: RustTaskTokenUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens as u32,
            output_tokens: usage.output_tokens as u32,
            cache_read_tokens: usage.cache_read_tokens as u32,
            cache_write_tokens: usage.cache_write_tokens as u32,
        }
    }
}

// ============================================================================
// ToolActivity
// ============================================================================

/// Record of a single tool activity.
#[napi(object)]
#[derive(Clone)]
pub struct ToolActivity {
    pub tool_name: String,
    pub timestamp: String,
    pub args_summary: String,
    pub success: bool,
}

impl From<RustToolActivity> for ToolActivity {
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
#[napi(object)]
#[derive(Clone)]
pub struct AgentProgress {
    pub tool_counts: HashMap<String, u32>,
    pub total_tool_calls: u32,
    pub token_usage: TaskTokenUsage,
    pub recent_activities: Vec<ToolActivity>,
    pub elapsed_ms: u32,
    pub running: bool,
}

impl From<RustAgentProgress> for AgentProgress {
    fn from(progress: RustAgentProgress) -> Self {
        Self {
            tool_counts: progress
                .tool_counts
                .into_iter()
                .map(|(k, v)| (k, v as u32))
                .collect(),
            total_tool_calls: progress.total_tool_calls as u32,
            token_usage: TaskTokenUsage::from(progress.token_usage),
            recent_activities: progress
                .recent_activities
                .into_iter()
                .map(ToolActivity::from)
                .collect(),
            elapsed_ms: progress.elapsed_ms as u32,
            running: progress.running,
        }
    }
}
