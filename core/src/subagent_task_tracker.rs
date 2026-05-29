//! In-memory tracker for delegated subagent tasks.
//!
//! Materializes a queryable view of subagent task lifecycle from the
//! `AgentEvent` stream. The event stream remains the authoritative record;
//! this module exists so callers can ask "what is task X doing right now?"
//! without scanning `run_events()`.

use crate::agent::AgentEvent;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentProgressEntry {
    pub timestamp_ms: u64,
    pub status: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTaskSnapshot {
    pub task_id: String,
    pub parent_session_id: String,
    pub child_session_id: String,
    pub agent: String,
    pub description: String,
    pub status: SubagentStatus,
    pub started_ms: u64,
    pub updated_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    pub progress: Vec<SubagentProgressEntry>,
}

#[derive(Debug, Default)]
pub struct InMemorySubagentTaskTracker {
    tasks: RwLock<HashMap<String, SubagentTaskSnapshot>>,
    cancellers: RwLock<HashMap<String, CancellationToken>>,
    /// FIFO queue of task_ids that have transitioned to a terminal
    /// state (Completed / Failed / Cancelled). Used to evict the
    /// oldest terminal entry when `max_terminal_tasks` is configured.
    /// Running tasks are never in this queue.
    terminal_order: RwLock<VecDeque<String>>,
    /// FIFO cap on terminal-state snapshots. `None` keeps the
    /// unbounded default.
    max_terminal_tasks: Option<usize>,
}

impl InMemorySubagentTaskTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a tracker with an optional FIFO cap on terminal-state
    /// snapshots. Running tasks are never dropped.
    pub fn with_max_terminal_tasks(max: usize) -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            cancellers: RwLock::new(HashMap::new()),
            terminal_order: RwLock::new(VecDeque::new()),
            max_terminal_tasks: Some(max),
        }
    }

    /// Internal helper: mark a task_id as terminal in the FIFO queue
    /// and evict oldest entries past the cap. Idempotent for tasks
    /// that are already in the terminal queue (a SubagentEnd arriving
    /// after a cancel won't double-push).
    async fn mark_terminal_and_evict(&self, task_id: &str) {
        let cap = match self.max_terminal_tasks {
            Some(n) => n,
            None => return,
        };
        // Hold all three structures together for the push + eviction so a
        // concurrent `record_event` (which takes only `tasks`) cannot
        // re-insert a victim into `tasks` in the window between its removal
        // from `tasks` and `cancellers`. Canonical order:
        // terminal_order -> tasks -> cancellers. Callers (`cancel`,
        // `record_event`) always drop their `tasks`/`cancellers` guards
        // before invoking this, so holding all three here cannot deadlock.
        let mut order = self.terminal_order.write().await;
        let mut tasks = self.tasks.write().await;
        let mut cancellers = self.cancellers.write().await;
        if !order.iter().any(|id| id == task_id) {
            order.push_back(task_id.to_string());
        }
        while order.len() > cap {
            if let Some(victim) = order.pop_front() {
                tasks.remove(&victim);
                cancellers.remove(&victim);
            }
        }
    }

    /// Register a `CancellationToken` for a running task so callers can
    /// trigger cancellation through `cancel(task_id)`. The task executor
    /// is expected to remove the entry on exit via `clear_canceller`.
    pub async fn register_canceller(&self, task_id: &str, token: CancellationToken) {
        self.cancellers
            .write()
            .await
            .insert(task_id.to_string(), token);
    }

    pub async fn clear_canceller(&self, task_id: &str) {
        self.cancellers.write().await.remove(task_id);
    }

    /// Fire the registered token and mark the snapshot as `Cancelled`.
    /// Returns `true` if a token was found (caller can interpret as
    /// "cancellation initiated"), `false` if the task id was unknown or
    /// the task already finished. The eventual `SubagentEnd` event won't
    /// overwrite the Cancelled status — see `record_event`.
    pub async fn cancel(&self, task_id: &str) -> bool {
        let token = self.cancellers.write().await.remove(task_id);
        match token {
            Some(token) => {
                token.cancel();
                let now = now_ms();
                let transitioned = {
                    let mut tasks = self.tasks.write().await;
                    if let Some(entry) = tasks.get_mut(task_id) {
                        if entry.status == SubagentStatus::Running {
                            entry.status = SubagentStatus::Cancelled;
                            entry.updated_ms = now;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if transitioned {
                    self.mark_terminal_and_evict(task_id).await;
                }
                true
            }
            None => false,
        }
    }

    /// Apply a single agent event to the tracker. Non-subagent events are ignored.
    pub async fn record_event(&self, event: &AgentEvent) {
        match event {
            AgentEvent::SubagentStart {
                task_id,
                session_id,
                parent_session_id,
                agent,
                description,
            } => {
                let now = now_ms();
                let mut tasks = self.tasks.write().await;
                tasks
                    .entry(task_id.clone())
                    .and_modify(|task| {
                        // Late start (e.g. background path) — keep the first-seen
                        // started_ms but refresh fields we now know.
                        task.parent_session_id = parent_session_id.clone();
                        task.child_session_id = session_id.clone();
                        task.agent = agent.clone();
                        task.description = description.clone();
                        task.updated_ms = now;
                    })
                    .or_insert_with(|| SubagentTaskSnapshot {
                        task_id: task_id.clone(),
                        parent_session_id: parent_session_id.clone(),
                        child_session_id: session_id.clone(),
                        agent: agent.clone(),
                        description: description.clone(),
                        status: SubagentStatus::Running,
                        started_ms: now,
                        updated_ms: now,
                        finished_ms: None,
                        output: None,
                        success: None,
                        progress: Vec::new(),
                    });
            }
            AgentEvent::SubagentProgress {
                task_id,
                session_id,
                status,
                metadata,
            } => {
                let now = now_ms();
                let mut tasks = self.tasks.write().await;
                let entry = tasks
                    .entry(task_id.clone())
                    .or_insert_with(|| SubagentTaskSnapshot {
                        task_id: task_id.clone(),
                        parent_session_id: String::new(),
                        child_session_id: session_id.clone(),
                        agent: String::new(),
                        description: String::new(),
                        status: SubagentStatus::Running,
                        started_ms: now,
                        updated_ms: now,
                        finished_ms: None,
                        output: None,
                        success: None,
                        progress: Vec::new(),
                    });
                entry.updated_ms = now;
                entry.progress.push(SubagentProgressEntry {
                    timestamp_ms: now,
                    status: status.clone(),
                    metadata: metadata.clone(),
                });
            }
            AgentEvent::SubagentEnd {
                task_id,
                session_id,
                agent,
                output,
                success,
            } => {
                let now = now_ms();
                let was_running = {
                    let mut tasks = self.tasks.write().await;
                    let entry =
                        tasks
                            .entry(task_id.clone())
                            .or_insert_with(|| SubagentTaskSnapshot {
                                task_id: task_id.clone(),
                                parent_session_id: String::new(),
                                child_session_id: session_id.clone(),
                                agent: agent.clone(),
                                description: String::new(),
                                status: SubagentStatus::Running,
                                started_ms: now,
                                updated_ms: now,
                                finished_ms: None,
                                output: None,
                                success: None,
                                progress: Vec::new(),
                            });
                    let was_running = entry.status == SubagentStatus::Running;
                    // Preserve a pre-set Cancelled status (set by `cancel()`)
                    // — a late SubagentEnd from the cancelled child loop is
                    // expected and must not downgrade the terminal state.
                    if entry.status != SubagentStatus::Cancelled {
                        entry.status = if *success {
                            SubagentStatus::Completed
                        } else {
                            SubagentStatus::Failed
                        };
                    }
                    entry.updated_ms = now;
                    entry.finished_ms = Some(now);
                    entry.output = Some(output.clone());
                    entry.success = Some(*success);
                    was_running
                };
                if was_running {
                    self.mark_terminal_and_evict(task_id).await;
                }
            }
            _ => {}
        }
    }

    pub async fn get(&self, task_id: &str) -> Option<SubagentTaskSnapshot> {
        self.tasks.read().await.get(task_id).cloned()
    }

    pub async fn list(&self) -> Vec<SubagentTaskSnapshot> {
        let mut tasks = self
            .tasks
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by_key(|task| task.started_ms);
        tasks
    }

    pub async fn list_pending(&self) -> Vec<SubagentTaskSnapshot> {
        self.list()
            .await
            .into_iter()
            .filter(|task| task.status == SubagentStatus::Running)
            .collect()
    }

    pub async fn list_for_parent(&self, parent_session_id: &str) -> Vec<SubagentTaskSnapshot> {
        self.list()
            .await
            .into_iter()
            .filter(|task| task.parent_session_id == parent_session_id)
            .collect()
    }

    /// Replace the tracker's task snapshots with the given set. Cancellers
    /// are **not** restored (they are runtime-only channels tied to live
    /// child loops). After `replace_snapshots`, any task whose status was
    /// `Running` at checkpoint time will appear `Running` in the tracker
    /// but `cancel(task_id)` will return `false` because no canceller is
    /// registered — callers should normally checkpoint at a quiescent
    /// point so no tasks are `Running`.
    ///
    /// Used by [`SessionStore`](crate::store::SessionStore) rehydration to
    /// restore the materialized subagent view of a previously-saved
    /// session.
    pub async fn replace_snapshots(&self, snapshots: Vec<SubagentTaskSnapshot>) {
        let mut map = HashMap::with_capacity(snapshots.len());
        for snap in snapshots {
            map.insert(snap.task_id.clone(), snap);
        }
        *self.tasks.write().await = map;
        // Cancellers reference live tokens — invalidate the lot.
        self.cancellers.write().await.clear();
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start_event(task_id: &str, parent: &str, child: &str) -> AgentEvent {
        AgentEvent::SubagentStart {
            task_id: task_id.to_string(),
            session_id: child.to_string(),
            parent_session_id: parent.to_string(),
            agent: "explore".to_string(),
            description: "find things".to_string(),
        }
    }

    fn progress_event(task_id: &str, child: &str, status: &str) -> AgentEvent {
        AgentEvent::SubagentProgress {
            task_id: task_id.to_string(),
            session_id: child.to_string(),
            status: status.to_string(),
            metadata: serde_json::json!({}),
        }
    }

    fn end_event(task_id: &str, child: &str, success: bool) -> AgentEvent {
        AgentEvent::SubagentEnd {
            task_id: task_id.to_string(),
            session_id: child.to_string(),
            agent: "explore".to_string(),
            output: "done".to_string(),
            success,
        }
    }

    #[tokio::test]
    async fn lifecycle_start_progress_end_transitions_status() {
        let tracker = InMemorySubagentTaskTracker::new();

        tracker
            .record_event(&start_event("task-1", "parent", "child"))
            .await;
        let snap = tracker.get("task-1").await.unwrap();
        assert_eq!(snap.status, SubagentStatus::Running);
        assert_eq!(snap.parent_session_id, "parent");
        assert_eq!(snap.child_session_id, "child");
        assert!(snap.finished_ms.is_none());

        tracker
            .record_event(&progress_event("task-1", "child", "tool_completed: bash"))
            .await;
        let snap = tracker.get("task-1").await.unwrap();
        assert_eq!(snap.status, SubagentStatus::Running);
        assert_eq!(snap.progress.len(), 1);

        tracker
            .record_event(&end_event("task-1", "child", true))
            .await;
        let snap = tracker.get("task-1").await.unwrap();
        assert_eq!(snap.status, SubagentStatus::Completed);
        assert_eq!(snap.success, Some(true));
        assert_eq!(snap.output.as_deref(), Some("done"));
        assert!(snap.finished_ms.is_some());
    }

    #[tokio::test]
    async fn failed_end_event_marks_status_failed() {
        let tracker = InMemorySubagentTaskTracker::new();
        tracker
            .record_event(&start_event("task-2", "parent", "child"))
            .await;
        tracker
            .record_event(&end_event("task-2", "child", false))
            .await;
        let snap = tracker.get("task-2").await.unwrap();
        assert_eq!(snap.status, SubagentStatus::Failed);
        assert_eq!(snap.success, Some(false));
    }

    #[tokio::test]
    async fn pending_list_excludes_completed_tasks() {
        let tracker = InMemorySubagentTaskTracker::new();
        tracker
            .record_event(&start_event("task-a", "parent", "child-a"))
            .await;
        tracker
            .record_event(&start_event("task-b", "parent", "child-b"))
            .await;
        tracker
            .record_event(&end_event("task-a", "child-a", true))
            .await;

        let pending = tracker.list_pending().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].task_id, "task-b");
    }

    #[tokio::test]
    async fn list_for_parent_filters_by_session() {
        let tracker = InMemorySubagentTaskTracker::new();
        tracker
            .record_event(&start_event("task-a", "session-1", "child-a"))
            .await;
        tracker
            .record_event(&start_event("task-b", "session-2", "child-b"))
            .await;

        let mine = tracker.list_for_parent("session-1").await;
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].task_id, "task-a");
    }

    #[tokio::test]
    async fn end_before_start_still_records_terminal_state() {
        let tracker = InMemorySubagentTaskTracker::new();
        tracker
            .record_event(&end_event("task-late", "child", true))
            .await;
        let snap = tracker.get("task-late").await.unwrap();
        assert_eq!(snap.status, SubagentStatus::Completed);
    }

    #[tokio::test]
    async fn non_subagent_events_are_ignored() {
        let tracker = InMemorySubagentTaskTracker::new();
        tracker
            .record_event(&AgentEvent::TextDelta {
                text: "ignore me".to_string(),
            })
            .await;
        assert!(tracker.list().await.is_empty());
    }

    #[tokio::test]
    async fn cancel_fires_token_and_marks_snapshot_cancelled() {
        let tracker = InMemorySubagentTaskTracker::new();
        tracker
            .record_event(&start_event("task-c", "parent", "child"))
            .await;

        let token = CancellationToken::new();
        tracker.register_canceller("task-c", token.clone()).await;
        assert!(!token.is_cancelled());

        let fired = tracker.cancel("task-c").await;
        assert!(fired, "cancel should report success");
        assert!(token.is_cancelled(), "registered token should be triggered");

        let snap = tracker.get("task-c").await.unwrap();
        assert_eq!(snap.status, SubagentStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_returns_false_for_unknown_task() {
        let tracker = InMemorySubagentTaskTracker::new();
        assert!(!tracker.cancel("task-does-not-exist").await);
    }

    #[tokio::test]
    async fn late_subagent_end_does_not_downgrade_cancelled_status() {
        let tracker = InMemorySubagentTaskTracker::new();
        tracker
            .record_event(&start_event("task-d", "parent", "child"))
            .await;
        let token = CancellationToken::new();
        tracker.register_canceller("task-d", token).await;
        assert!(tracker.cancel("task-d").await);

        // The cancelled child loop will still emit a (likely failed)
        // SubagentEnd. The terminal status should remain Cancelled.
        tracker
            .record_event(&end_event("task-d", "child", false))
            .await;
        let snap = tracker.get("task-d").await.unwrap();
        assert_eq!(snap.status, SubagentStatus::Cancelled);
        assert!(snap.finished_ms.is_some());
        assert_eq!(snap.success, Some(false));
    }

    #[tokio::test]
    async fn clear_canceller_disarms_future_cancel_calls() {
        let tracker = InMemorySubagentTaskTracker::new();
        tracker
            .record_event(&start_event("task-e", "parent", "child"))
            .await;
        let token = CancellationToken::new();
        tracker.register_canceller("task-e", token.clone()).await;
        tracker.clear_canceller("task-e").await;

        assert!(!tracker.cancel("task-e").await);
        assert!(!token.is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_record_and_cancel_under_terminal_cap_does_not_deadlock() {
        // Guards the canonical lock-ordering change in mark_terminal_and_evict
        // (terminal_order -> tasks -> cancellers held together). A bad ordering
        // would ABBA-deadlock against concurrent cancel()/record_event and hang.
        let tracker = std::sync::Arc::new(InMemorySubagentTaskTracker::with_max_terminal_tasks(8));
        let mut handles = Vec::new();
        for i in 0..60 {
            let t = std::sync::Arc::clone(&tracker);
            handles.push(tokio::spawn(async move {
                let task_id = format!("t-{i}");
                let child = format!("c-{i}");
                t.record_event(&start_event(&task_id, "parent", &child))
                    .await;
                if i % 2 == 0 {
                    t.register_canceller(&task_id, CancellationToken::new())
                        .await;
                    let _ = t.cancel(&task_id).await;
                } else {
                    t.record_event(&end_event(&task_id, &child, true)).await;
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        // Terminal cap honored; tracker still usable.
        let terminal = tracker
            .list()
            .await
            .into_iter()
            .filter(|t| t.status != SubagentStatus::Running)
            .count();
        assert!(
            terminal <= 8,
            "terminal cap must hold under load, got {terminal}"
        );
    }

    #[tokio::test]
    async fn max_terminal_tasks_evicts_oldest_completed_only() {
        let tracker = InMemorySubagentTaskTracker::with_max_terminal_tasks(2);

        // Three fully terminal tasks; oldest must be evicted.
        for i in 0..3 {
            let task_id = format!("done-{i}");
            tracker
                .record_event(&start_event(&task_id, "parent", "child"))
                .await;
            tracker
                .record_event(&end_event(&task_id, "child", true))
                .await;
        }

        // Only the two most-recent terminal tasks survive.
        let list = tracker.list().await;
        let ids: Vec<&str> = list.iter().map(|t| t.task_id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"done-1"));
        assert!(ids.contains(&"done-2"));
        assert!(
            !ids.contains(&"done-0"),
            "oldest terminal entry must be evicted"
        );
    }

    #[tokio::test]
    async fn max_terminal_tasks_never_evicts_running_tasks() {
        let tracker = InMemorySubagentTaskTracker::with_max_terminal_tasks(1);

        // One running, two terminal — the cap applies only to terminal
        // entries, so the running task survives even if it would be
        // the "oldest".
        tracker
            .record_event(&start_event("running", "parent", "child"))
            .await;
        for i in 0..3 {
            let task_id = format!("done-{i}");
            tracker
                .record_event(&start_event(&task_id, "parent", "child"))
                .await;
            tracker
                .record_event(&end_event(&task_id, "child", true))
                .await;
        }

        let list = tracker.list().await;
        let ids: Vec<&str> = list.iter().map(|t| t.task_id.as_str()).collect();
        assert!(
            ids.contains(&"running"),
            "running task must never be evicted"
        );
        // Only the most recent terminal task survives.
        assert!(ids.contains(&"done-2"));
        assert!(!ids.contains(&"done-0"));
        assert!(!ids.contains(&"done-1"));
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn cancel_path_also_participates_in_terminal_cap() {
        let tracker = InMemorySubagentTaskTracker::with_max_terminal_tasks(1);

        // Two cancellations — second one should evict the first.
        for i in 0..2 {
            let task_id = format!("c-{i}");
            tracker
                .record_event(&start_event(&task_id, "parent", "child"))
                .await;
            tracker
                .register_canceller(&task_id, CancellationToken::new())
                .await;
            assert!(tracker.cancel(&task_id).await);
        }

        let list = tracker.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].task_id, "c-1");
    }
}
