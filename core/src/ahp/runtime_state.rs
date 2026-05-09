//! Runtime counters observed by the AHP executor.
//!
//! AHP remains a harness-facing control plane. This module only tracks facts
//! emitted by the main agent so heartbeats and event contexts can describe the
//! current run without starting an additional advisory runtime in-core.

use crate::agent::AgentEvent;
use crate::error::{read_or_recover, write_or_recover};
use crate::hooks::HookEvent;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AhpRuntimeSnapshot {
    pub active_tools: usize,
    pub pending_actions: usize,
    pub queue_depth: usize,
    pub tokens_used: i32,
    pub total_events_processed: u64,
    pub error_count: usize,
    pub uptime_ms: u64,
    pub current_state: String,
}

#[derive(Debug)]
pub(super) struct AhpRuntimeState {
    active_tools: RwLock<HashSet<String>>,
    pending_confirmations: RwLock<HashSet<String>>,
    external_tasks: RwLock<HashSet<String>>,
    runtime_run_tokens: RwLock<HashMap<String, i32>>,
    hook_token_fallback: AtomicI32,
    runtime_tokens_seen: AtomicBool,
}

impl Default for AhpRuntimeState {
    fn default() -> Self {
        Self {
            active_tools: RwLock::new(HashSet::new()),
            pending_confirmations: RwLock::new(HashSet::new()),
            external_tasks: RwLock::new(HashSet::new()),
            runtime_run_tokens: RwLock::new(HashMap::new()),
            hook_token_fallback: AtomicI32::new(0),
            runtime_tokens_seen: AtomicBool::new(false),
        }
    }
}

impl AhpRuntimeState {
    pub(super) fn observe_agent_event(&self, event: &AgentEvent, run_id: &str) {
        match event {
            AgentEvent::ToolStart { id, .. } => {
                write_or_recover(&self.active_tools).insert(id.clone());
            }
            AgentEvent::ToolEnd { id, .. } | AgentEvent::PermissionDenied { tool_id: id, .. } => {
                write_or_recover(&self.active_tools).remove(id);
                write_or_recover(&self.pending_confirmations).remove(id);
            }
            AgentEvent::ConfirmationRequired { tool_id, .. } => {
                write_or_recover(&self.active_tools).remove(tool_id);
                write_or_recover(&self.pending_confirmations).insert(tool_id.clone());
            }
            AgentEvent::ConfirmationReceived { tool_id, .. }
            | AgentEvent::ConfirmationTimeout { tool_id, .. } => {
                write_or_recover(&self.pending_confirmations).remove(tool_id);
            }
            AgentEvent::ExternalTaskPending { task_id, .. } => {
                write_or_recover(&self.external_tasks).insert(task_id.clone());
            }
            AgentEvent::ExternalTaskCompleted { task_id, .. } => {
                write_or_recover(&self.external_tasks).remove(task_id);
            }
            AgentEvent::TurnEnd { usage, .. } => {
                self.add_runtime_turn_tokens(run_id, tokens_to_i32(usage.total_tokens));
            }
            AgentEvent::End { usage, .. } => {
                self.set_runtime_run_tokens_at_least(run_id, tokens_to_i32(usage.total_tokens));
                self.clear_runtime_actions();
            }
            AgentEvent::Error { .. } => {
                self.clear_runtime_actions();
            }
            _ => {}
        }
    }

    pub(super) fn observe_hook_event(&self, event: &HookEvent) {
        if self.runtime_tokens_seen.load(Ordering::Relaxed) {
            return;
        }

        match event {
            HookEvent::GenerateEnd(e) => {
                let tokens = e.usage.total_tokens.max(0);
                let _ = self.hook_token_fallback.fetch_update(
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                    |current| Some(current.saturating_add(tokens)),
                );
            }
            HookEvent::PostResponse(e) => {
                let tokens = e.usage.total_tokens.max(0);
                let _ = self.hook_token_fallback.fetch_update(
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                    |current| Some(current.max(tokens)),
                );
            }
            _ => {}
        }
    }

    pub(super) fn clear_runtime_actions(&self) {
        write_or_recover(&self.active_tools).clear();
        write_or_recover(&self.pending_confirmations).clear();
        write_or_recover(&self.external_tasks).clear();
    }

    pub(super) fn active_tools(&self) -> usize {
        read_or_recover(&self.active_tools).len()
    }

    pub(super) fn pending_actions(&self) -> usize {
        read_or_recover(&self.pending_confirmations).len()
            + read_or_recover(&self.external_tasks).len()
    }

    pub(super) fn queue_depth(&self) -> usize {
        read_or_recover(&self.external_tasks).len()
    }

    pub(super) fn tokens_used(&self) -> i32 {
        if self.runtime_tokens_seen.load(Ordering::Relaxed) {
            read_or_recover(&self.runtime_run_tokens)
                .values()
                .fold(0i32, |sum, tokens| sum.saturating_add(*tokens))
        } else {
            self.hook_token_fallback.load(Ordering::Relaxed)
        }
    }

    fn add_runtime_turn_tokens(&self, run_id: &str, tokens: i32) {
        self.mark_runtime_tokens_seen();
        let mut run_tokens = write_or_recover(&self.runtime_run_tokens);
        let entry = run_tokens.entry(run_id.to_string()).or_insert(0);
        *entry = entry.saturating_add(tokens);
    }

    fn set_runtime_run_tokens_at_least(&self, run_id: &str, tokens: i32) {
        self.mark_runtime_tokens_seen();
        let mut run_tokens = write_or_recover(&self.runtime_run_tokens);
        let entry = run_tokens.entry(run_id.to_string()).or_insert(0);
        *entry = (*entry).max(tokens);
    }

    fn mark_runtime_tokens_seen(&self) {
        if !self.runtime_tokens_seen.swap(true, Ordering::Relaxed) {
            self.hook_token_fallback.store(0, Ordering::Relaxed);
        }
    }
}

pub(super) fn handshake_capabilities() -> Vec<String> {
    [
        "pre_action",
        "post_action",
        "pre_prompt",
        "post_response",
        "session_start",
        "session_end",
        "error",
        "context_perception",
        "success",
        "memory_recall",
        "planning",
        "reasoning",
        "rate_limit",
        "confirmation",
        "idle",
        "heartbeat",
        "query",
        "batch",
        "skill_load",
        "skill_unload",
        "run_lifecycle",
        "task_list",
        "verification",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn tokens_to_i32(tokens: usize) -> i32 {
    i32::try_from(tokens).unwrap_or(i32::MAX)
}
