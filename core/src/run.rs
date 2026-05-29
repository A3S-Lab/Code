//! Durable run primitives for agent executions.
//!
//! This module is intentionally small: it records runtime events and maintains a
//! stable run status snapshot that can be persisted by session stores.

use crate::agent::AgentEvent;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Created,
    Planning,
    Executing,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEventRecord {
    pub sequence: usize,
    pub timestamp_ms: u64,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveToolSnapshot {
    pub id: String,
    pub name: String,
    pub started_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub id: String,
    pub session_id: String,
    pub status: RunStatus,
    pub prompt: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub event_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub snapshot: RunSnapshot,
    pub events: Vec<RunEventRecord>,
}

impl RunSnapshot {
    fn new(id: String, session_id: String, prompt: String) -> Self {
        let now = now_ms();
        Self {
            id,
            session_id,
            status: RunStatus::Created,
            prompt,
            created_at_ms: now,
            updated_at_ms: now,
            result_text: None,
            error: None,
            event_count: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct InMemoryRunStore {
    runs: RwLock<HashMap<String, RunSnapshot>>,
    events: RwLock<HashMap<String, Vec<RunEventRecord>>>,
    /// Insertion order of run ids — used to FIFO-evict the oldest run
    /// when `max_runs` is set and exceeded.
    insertion_order: RwLock<VecDeque<String>>,
    /// Maximum number of runs retained. When exceeded, oldest run is
    /// dropped along with its events. `None` = unlimited (default).
    max_runs: Option<usize>,
    /// Maximum number of events retained per run. When exceeded, the
    /// oldest events are FIFO-dropped from that run's buffer. The
    /// run's `event_count` field is **not** decremented — it stays as
    /// the cumulative total ever recorded. `None` = unlimited.
    max_events_per_run: Option<usize>,
}

impl InMemoryRunStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a store with optional FIFO retention caps. `None`
    /// fields keep the unbounded default.
    pub fn with_retention(max_runs: Option<usize>, max_events_per_run: Option<usize>) -> Self {
        Self {
            runs: RwLock::new(HashMap::new()),
            events: RwLock::new(HashMap::new()),
            insertion_order: RwLock::new(VecDeque::new()),
            max_runs,
            max_events_per_run,
        }
    }

    pub async fn create_run(&self, session_id: &str, prompt: &str) -> RunSnapshot {
        // Default ID generation when the caller has no host_env handy.
        // Production callers reach `create_run_with_id` via
        // `RunControlState::start_run` so the host's IdGenerator is honored.
        let id = format!("run-{}", uuid::Uuid::new_v4());
        self.create_run_with_id(id, session_id, prompt).await
    }

    /// Create a run with a caller-supplied id. Used by the session
    /// orchestration layer so the parent session's host-provided
    /// [`IdGenerator`](crate::host_env::IdGenerator) governs run ids.
    pub async fn create_run_with_id(
        &self,
        id: String,
        session_id: &str,
        prompt: &str,
    ) -> RunSnapshot {
        let snapshot = RunSnapshot::new(id.clone(), session_id.to_string(), prompt.to_string());
        // Hold all three structures together for the insert + FIFO-evict so
        // `runs`, `events`, and `insertion_order` never diverge under
        // concurrent access (previously the maps were locked separately,
        // leaving a window where a run existed in one map but not the
        // other). Canonical acquisition order: order -> events -> runs.
        // Other methods (record_event, records, mark_*) only ever hold ONE
        // of {events, runs} at a time — they never nest — so holding both
        // here cannot ABBA-deadlock against them.
        {
            let mut order = self.insertion_order.write().await;
            let mut events = self.events.write().await;
            let mut runs = self.runs.write().await;
            runs.insert(id.clone(), snapshot.clone());
            events.insert(id.clone(), Vec::new());
            order.push_back(id);
            if let Some(cap) = self.max_runs {
                while order.len() > cap {
                    if let Some(victim) = order.pop_front() {
                        runs.remove(&victim);
                        events.remove(&victim);
                    }
                }
            }
        }
        snapshot
    }

    pub async fn record_event(&self, run_id: &str, event: AgentEvent) -> Option<RunSnapshot> {
        let mut events = self.events.write().await;
        let run_events = events.get_mut(run_id)?;
        let sequence = run_events.len();
        run_events.push(RunEventRecord {
            sequence,
            timestamp_ms: now_ms(),
            event: event.clone(),
        });
        // FIFO-trim event buffer past per-run cap.
        if let Some(cap) = self.max_events_per_run {
            if run_events.len() > cap {
                let excess = run_events.len() - cap;
                run_events.drain(..excess);
            }
        }
        drop(events);

        let mut runs = self.runs.write().await;
        let run = runs.get_mut(run_id)?;
        apply_event_to_snapshot(run, &event);
        run.event_count += 1;
        run.updated_at_ms = now_ms();
        Some(run.clone())
    }

    pub async fn mark_failed(&self, run_id: &str, error: impl Into<String>) -> Option<RunSnapshot> {
        let mut runs = self.runs.write().await;
        let run = runs.get_mut(run_id)?;
        if run.status == RunStatus::Cancelled {
            return Some(run.clone());
        }
        run.status = RunStatus::Failed;
        run.error = Some(error.into());
        run.updated_at_ms = now_ms();
        Some(run.clone())
    }

    pub async fn mark_cancelled(&self, run_id: &str) -> Option<RunSnapshot> {
        let mut runs = self.runs.write().await;
        let run = runs.get_mut(run_id)?;
        run.status = RunStatus::Cancelled;
        run.updated_at_ms = now_ms();
        Some(run.clone())
    }

    pub async fn snapshot(&self, run_id: &str) -> Option<RunSnapshot> {
        self.runs.read().await.get(run_id).cloned()
    }

    pub async fn events(&self, run_id: &str) -> Vec<RunEventRecord> {
        self.events
            .read()
            .await
            .get(run_id)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn list(&self) -> Vec<RunSnapshot> {
        let mut runs = self.runs.read().await.values().cloned().collect::<Vec<_>>();
        runs.sort_by_key(|run| run.created_at_ms);
        runs
    }

    pub async fn records(&self) -> Vec<RunRecord> {
        let snapshots = self.runs.read().await.values().cloned().collect::<Vec<_>>();
        let events = self.events.read().await;
        let mut records = snapshots
            .into_iter()
            .map(|snapshot| RunRecord {
                events: events.get(&snapshot.id).cloned().unwrap_or_default(),
                snapshot,
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.snapshot.created_at_ms);
        records
    }

    pub async fn replace_records(&self, records: Vec<RunRecord>) {
        // Preserve creation-order in the FIFO eviction queue so a
        // restored session honours its `max_runs` cap consistently
        // with newly-created runs.
        let mut sorted = records;
        sorted.sort_by_key(|r| r.snapshot.created_at_ms);
        let mut run_map = HashMap::new();
        let mut event_map = HashMap::new();
        let mut order = VecDeque::with_capacity(sorted.len());
        for record in sorted {
            let id = record.snapshot.id.clone();
            // Trust the persisted `event_count` — it is the CUMULATIVE total
            // ever recorded and is deliberately not decremented when the
            // per-run event buffer is FIFO-trimmed by `max_events_per_run`.
            // Overwriting it with `record.events.len()` here would corrupt
            // the cumulative count for any restored run whose buffer was
            // trimmed (restoring a 100-event run with a 50-cap buffer as
            // event_count=50).
            event_map.insert(id.clone(), record.events);
            run_map.insert(id.clone(), record.snapshot);
            order.push_back(id);
        }
        *self.runs.write().await = run_map;
        *self.events.write().await = event_map;
        *self.insertion_order.write().await = order;
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_create_and_record_under_cap_does_not_deadlock() {
        // Guards the canonical lock-ordering change in create_run_with_id
        // (order -> events -> runs held together). A bad ordering would
        // ABBA-deadlock against concurrent record_event and hang this test.
        let store = std::sync::Arc::new(InMemoryRunStore::with_retention(Some(10), None));
        let mut handles = Vec::new();
        for i in 0..100 {
            let s = std::sync::Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let r = s.create_run("sess", &format!("p{i}")).await;
                for _ in 0..5 {
                    s.record_event(
                        &r.id,
                        AgentEvent::TextDelta {
                            text: "x".to_string(),
                        },
                    )
                    .await;
                }
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        // Cap honored under concurrent load, and the store is still usable
        // (no deadlock, no poisoned locks).
        assert!(store.list().await.len() <= 10);
    }

    #[tokio::test]
    async fn replace_records_preserves_cumulative_event_count_after_trim() {
        // Source store with a small per-run event cap.
        let src = InMemoryRunStore::with_retention(None, Some(3));
        let run = src.create_run("s", "p").await;
        for _ in 0..10 {
            src.record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: "x".to_string(),
                },
            )
            .await;
        }
        let records = src.records().await;
        // Buffer trimmed to cap, but cumulative event_count is the total.
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].events.len(), 3, "buffer trimmed to cap");
        assert_eq!(records[0].snapshot.event_count, 10, "cumulative preserved");

        // Round-trip into a fresh store via replace_records.
        let dst = InMemoryRunStore::new();
        dst.replace_records(records).await;
        let restored = dst.snapshot(&run.id).await.unwrap();
        assert_eq!(
            restored.event_count, 10,
            "replace_records must NOT reset event_count to the trimmed buffer length"
        );
        // The (trimmed) event buffer still round-trips at cap size.
        assert_eq!(dst.events(&run.id).await.len(), 3);
    }

    #[tokio::test]
    async fn max_runs_evicts_oldest() {
        let store = InMemoryRunStore::with_retention(Some(2), None);
        let _ = store.create_run("session-1", "prompt-1").await;
        let r2 = store.create_run("session-1", "prompt-2").await;
        let r3 = store.create_run("session-1", "prompt-3").await;

        // Oldest run (prompt-1) must have been evicted.
        assert_eq!(store.list().await.len(), 2);
        let ids: Vec<String> = store.list().await.into_iter().map(|r| r.id).collect();
        assert!(ids.contains(&r2.id));
        assert!(ids.contains(&r3.id));
        assert!(store.events(&r2.id).await.is_empty());
        // The evicted run's events are gone too.
        let surviving_event_count: usize =
            store.events(&r2.id).await.len() + store.events(&r3.id).await.len();
        assert_eq!(surviving_event_count, 0);
    }

    #[tokio::test]
    async fn max_events_per_run_caps_event_buffer() {
        let store = InMemoryRunStore::with_retention(None, Some(3));
        let run = store.create_run("session-1", "prompt").await;
        for _ in 0..10 {
            store
                .record_event(
                    &run.id,
                    AgentEvent::TextDelta {
                        text: "x".to_string(),
                    },
                )
                .await;
        }
        let events = store.events(&run.id).await;
        assert_eq!(
            events.len(),
            3,
            "buffer must be capped at max_events_per_run"
        );
        // Snapshot `event_count` reflects the cumulative total, not the
        // surviving buffer length.
        let snap = store.snapshot(&run.id).await.unwrap();
        assert_eq!(snap.event_count, 10);
    }

    #[tokio::test]
    async fn unlimited_retention_is_the_default() {
        let store = InMemoryRunStore::new();
        for i in 0..50 {
            let r = store.create_run("s", &format!("p{i}")).await;
            for _ in 0..20 {
                store
                    .record_event(
                        &r.id,
                        AgentEvent::TextDelta {
                            text: "y".to_string(),
                        },
                    )
                    .await;
            }
        }
        assert_eq!(store.list().await.len(), 50);
    }
}

#[derive(Clone)]
pub struct RunHandle {
    id: String,
    session_id: String,
    store: Arc<InMemoryRunStore>,
    cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    current_run_id: Arc<Mutex<Option<String>>>,
    hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
}

impl RunHandle {
    pub(crate) fn new(
        id: String,
        session_id: String,
        store: Arc<InMemoryRunStore>,
        cancel_token: Arc<Mutex<Option<CancellationToken>>>,
        current_run_id: Arc<Mutex<Option<String>>>,
        hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
    ) -> Self {
        Self {
            id,
            session_id,
            store,
            cancel_token,
            current_run_id,
            hook_executor,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub async fn snapshot(&self) -> Option<RunSnapshot> {
        self.store.snapshot(&self.id).await
    }

    pub async fn events(&self) -> Vec<RunEventRecord> {
        self.store.events(&self.id).await
    }

    pub async fn status(&self) -> Option<RunStatus> {
        self.snapshot().await.map(|snapshot| snapshot.status)
    }

    pub async fn cancel(&self) -> bool {
        let current_run_id = self.current_run_id.lock().await.clone();
        if current_run_id.as_deref() != Some(self.id.as_str()) {
            return false;
        }

        let token = self.cancel_token.lock().await.clone();
        if let Some(token) = token {
            token.cancel();
            let _ = self.store.mark_cancelled(&self.id).await;
            if let Some(executor) = &self.hook_executor {
                executor
                    .record_run_cancelled(&self.id, &self.session_id, Some("cancelled by host"))
                    .await;
            }
            true
        } else {
            false
        }
    }
}

fn apply_event_to_snapshot(run: &mut RunSnapshot, event: &AgentEvent) {
    match event {
        AgentEvent::Start { prompt } => {
            run.status = RunStatus::Executing;
            if run.prompt.is_empty() {
                run.prompt = prompt.clone();
            }
        }
        AgentEvent::PlanningStart { .. } => {
            run.status = RunStatus::Planning;
        }
        AgentEvent::StepStart { .. }
        | AgentEvent::ToolStart { .. }
        | AgentEvent::TurnStart { .. }
            if !matches!(run.status, RunStatus::Planning) =>
        {
            run.status = RunStatus::Executing;
        }
        AgentEvent::End { text, .. } => {
            if run.status == RunStatus::Cancelled {
                return;
            }
            run.status = RunStatus::Completed;
            run.result_text = Some(text.clone());
            run.error = None;
        }
        AgentEvent::Error { message } => {
            if run.status == RunStatus::Cancelled {
                return;
            }
            run.status = RunStatus::Failed;
            run.error = Some(message.clone());
        }
        _ => {}
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_store_tracks_status_and_events() {
        let store = InMemoryRunStore::new();
        let run = store.create_run("session-1", "fix tests").await;

        store
            .record_event(
                &run.id,
                AgentEvent::Start {
                    prompt: "fix tests".to_string(),
                },
            )
            .await;
        store
            .record_event(
                &run.id,
                AgentEvent::End {
                    text: "done".to_string(),
                    usage: Default::default(),
                    verification_summary: Box::new(
                        crate::verification::VerificationSummary::from_reports(&[]),
                    ),
                    meta: None,
                },
            )
            .await;

        let snapshot = store.snapshot(&run.id).await.unwrap();
        assert_eq!(snapshot.status, RunStatus::Completed);
        assert_eq!(snapshot.result_text.as_deref(), Some("done"));
        assert_eq!(snapshot.event_count, 2);
        assert_eq!(store.events(&run.id).await.len(), 2);
    }

    #[tokio::test]
    async fn run_store_replaces_persisted_records() {
        let source = InMemoryRunStore::new();
        let run = source.create_run("session-1", "persist").await;
        source
            .record_event(
                &run.id,
                AgentEvent::Start {
                    prompt: "persist".to_string(),
                },
            )
            .await;

        let target = InMemoryRunStore::new();
        target.replace_records(source.records().await).await;

        assert_eq!(target.list().await.len(), 1);
        assert_eq!(target.events(&run.id).await.len(), 1);
        assert_eq!(target.snapshot(&run.id).await.unwrap().event_count, 1);
    }

    #[tokio::test]
    async fn run_handle_only_cancels_current_run() {
        let store = Arc::new(InMemoryRunStore::new());
        let run = store.create_run("session-1", "fix tests").await;
        let cancel_token = Arc::new(Mutex::new(Some(CancellationToken::new())));
        let current_run_id = Arc::new(Mutex::new(Some(run.id.clone())));
        let handle = RunHandle::new(
            run.id.clone(),
            run.session_id.clone(),
            store.clone(),
            cancel_token,
            current_run_id.clone(),
            None,
        );

        assert!(handle.cancel().await);
        assert_eq!(handle.status().await, Some(RunStatus::Cancelled));

        *current_run_id.lock().await = Some("other-run".to_string());
        assert!(!handle.cancel().await);
    }
}
