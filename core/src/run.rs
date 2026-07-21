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

impl RunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
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

/// Cursor-based view over the retained event window for one run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEventPage {
    pub events: Vec<RunEventRecord>,
    /// Oldest sequence still available, or `None` when no events are retained.
    pub first_available_sequence: Option<usize>,
    /// Exclusive upper bound for every event ever recorded by this run.
    pub latest_sequence_exclusive: usize,
    /// Cursor to pass as `after_sequence` for the next page.
    pub next_after_sequence: Option<usize>,
    /// True when events requested by the cursor have already been evicted.
    pub retention_gap: bool,
    pub has_more: bool,
}

#[derive(Debug, Default)]
struct RetainedRunEvents {
    records: Vec<RunEventRecord>,
    serialized_bytes: usize,
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
    events: RwLock<HashMap<String, RetainedRunEvents>>,
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
    /// Maximum serialized size of retained event records per run. This is
    /// independent of the count cap and uses the same FIFO policy.
    max_event_bytes_per_run: Option<usize>,
}

impl InMemoryRunStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a store with optional FIFO retention caps. `None`
    /// fields keep the unbounded default.
    pub fn with_retention(max_runs: Option<usize>, max_events_per_run: Option<usize>) -> Self {
        Self::with_retention_limits(max_runs, max_events_per_run, None)
    }

    /// Construct a store with count and serialized-byte FIFO retention caps.
    pub fn with_retention_limits(
        max_runs: Option<usize>,
        max_events_per_run: Option<usize>,
        max_event_bytes_per_run: Option<usize>,
    ) -> Self {
        Self {
            runs: RwLock::new(HashMap::new()),
            events: RwLock::new(HashMap::new()),
            insertion_order: RwLock::new(VecDeque::new()),
            max_runs,
            max_events_per_run,
            max_event_bytes_per_run,
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
        // `record_event` uses the same events -> runs order. Other methods
        // hold at most one of those locks, so holding both here cannot
        // ABBA-deadlock against them.
        {
            let mut order = self.insertion_order.write().await;
            let mut events = self.events.write().await;
            let mut runs = self.runs.write().await;
            runs.insert(id.clone(), snapshot.clone());
            events.insert(id.clone(), RetainedRunEvents::default());
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
        let mut runs = self.runs.write().await;
        let run_events = events.get_mut(run_id)?;
        let run = runs.get_mut(run_id)?;

        // `event_count` is cumulative and survives FIFO retention and
        // persisted snapshot restoration, so it is the stable cursor for
        // event sequencing. The retained buffer length is not: once a
        // capped buffer is full it remains constant and would reuse the
        // same sequence for every subsequent event.
        let sequence = run.event_count;
        let record = RunEventRecord {
            sequence,
            timestamp_ms: now_ms(),
            event: event.clone(),
        };
        run_events.serialized_bytes = run_events
            .serialized_bytes
            .saturating_add(serialized_event_record_len(&record));
        run_events.records.push(record);
        trim_retained_events(
            run_events,
            self.max_events_per_run,
            self.max_event_bytes_per_run,
        );
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
            .map(|events| events.records.clone())
            .unwrap_or_default()
    }

    /// Return retained events strictly after `after_sequence`, bounded by
    /// `limit`. The page reports when the requested cursor predates the
    /// retained FIFO window. `None` distinguishes an unknown run from a known
    /// run whose event window is empty.
    pub async fn event_page(
        &self,
        run_id: &str,
        after_sequence: Option<usize>,
        limit: usize,
    ) -> Option<RunEventPage> {
        // Match the canonical events -> runs lock order used by record_event.
        let events = self.events.read().await;
        let runs = self.runs.read().await;
        let retained = events.get(run_id)?;
        let run = runs.get(run_id)?;
        let first_available_sequence = retained.records.first().map(|event| event.sequence);
        let requested_start = match after_sequence {
            Some(sequence) => sequence.saturating_add(1),
            None => 0,
        };
        let retention_gap = if requested_start >= run.event_count {
            false
        } else {
            first_available_sequence
                .map(|first| requested_start < first)
                .unwrap_or(true)
        };
        let mut matching = retained
            .records
            .iter()
            .filter(|event| after_sequence.is_none_or(|cursor| event.sequence > cursor));
        let page_events = matching.by_ref().take(limit).cloned().collect::<Vec<_>>();
        let has_more = matching.next().is_some();
        let next_after_sequence = page_events
            .last()
            .map(|event| event.sequence)
            .or(after_sequence);
        Some(RunEventPage {
            events: page_events,
            first_available_sequence,
            latest_sequence_exclusive: run.event_count,
            next_after_sequence,
            retention_gap,
            has_more,
        })
    }

    pub async fn list(&self) -> Vec<RunSnapshot> {
        let order = self.insertion_order.read().await;
        let runs = self.runs.read().await;
        order
            .iter()
            .filter_map(|run_id| runs.get(run_id).cloned())
            .collect()
    }

    pub async fn records(&self) -> Vec<RunRecord> {
        // Preserve insertion order explicitly. Millisecond timestamps can tie,
        // and sorting snapshots from a HashMap would then make FIFO restore
        // nondeterministic. `create_run_with_id` uses the same
        // order -> events -> runs acquisition order; `record_event` never
        // acquires `insertion_order`, so this cannot form an ABBA cycle.
        let order = self.insertion_order.read().await;
        let events = self.events.read().await;
        let runs = self.runs.read().await;
        order
            .iter()
            .filter_map(|run_id| {
                let snapshot = runs.get(run_id)?.clone();
                Some(RunRecord {
                    events: events
                        .get(run_id)
                        .map(|events| events.records.clone())
                        .unwrap_or_default(),
                    snapshot,
                })
            })
            .collect()
    }

    pub async fn replace_records(&self, records: Vec<RunRecord>) {
        // Preserve creation-order in the FIFO eviction queue so a
        // restored session honours its `max_runs` cap consistently
        // with newly-created runs.
        let mut sorted = records;
        sorted.sort_by_key(|r| r.snapshot.created_at_ms);
        if let Some(cap) = self.max_runs {
            let excess = sorted.len().saturating_sub(cap);
            if excess > 0 {
                sorted.drain(..excess);
            }
        }
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
            let mut retained = RetainedRunEvents {
                serialized_bytes: record
                    .events
                    .iter()
                    .map(serialized_event_record_len)
                    .fold(0usize, usize::saturating_add),
                records: record.events,
            };
            trim_retained_events(
                &mut retained,
                self.max_events_per_run,
                self.max_event_bytes_per_run,
            );
            event_map.insert(id.clone(), retained);
            run_map.insert(id.clone(), record.snapshot);
            order.push_back(id);
        }
        // Publish the restored generation under the same canonical lock order
        // used by create/read paths so concurrent observers cannot see a run
        // map from one generation and event/order state from another.
        let mut stored_order = self.insertion_order.write().await;
        let mut stored_events = self.events.write().await;
        let mut stored_runs = self.runs.write().await;
        *stored_runs = run_map;
        *stored_events = event_map;
        *stored_order = order;
    }
}

fn serialized_event_record_len(record: &RunEventRecord) -> usize {
    serde_json::to_vec(record)
        .map(|encoded| encoded.len())
        .unwrap_or(usize::MAX)
}

fn trim_retained_events(
    events: &mut RetainedRunEvents,
    max_events: Option<usize>,
    max_bytes: Option<usize>,
) {
    let count_excess = max_events
        .map(|cap| events.records.len().saturating_sub(cap))
        .unwrap_or(0);
    let mut remove_count = count_excess;
    let mut remaining_bytes = events.serialized_bytes;
    for record in events.records.iter().take(remove_count) {
        remaining_bytes = remaining_bytes.saturating_sub(serialized_event_record_len(record));
    }
    while max_bytes.is_some_and(|cap| remaining_bytes > cap) && remove_count < events.records.len()
    {
        remaining_bytes = remaining_bytes
            .saturating_sub(serialized_event_record_len(&events.records[remove_count]));
        remove_count += 1;
    }
    for record in events.records.iter().take(remove_count) {
        events.serialized_bytes = events
            .serialized_bytes
            .saturating_sub(serialized_event_record_len(record));
    }
    if remove_count > 0 {
        events.records.drain(..remove_count);
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
    async fn replace_records_enforces_run_and_event_caps() {
        let source = InMemoryRunStore::new();
        for run_index in 0..4 {
            let run = source
                .create_run_with_id(
                    format!("run-{run_index}"),
                    "session-1",
                    &format!("prompt-{run_index}"),
                )
                .await;
            for event_index in 0..5 {
                source
                    .record_event(
                        &run.id,
                        AgentEvent::TextDelta {
                            text: format!("{run_index}:{event_index}"),
                        },
                    )
                    .await;
            }
        }

        let restored = InMemoryRunStore::with_retention(Some(2), Some(2));
        restored.replace_records(source.records().await).await;

        let records = restored.records().await;
        assert_eq!(
            records
                .iter()
                .map(|record| record.snapshot.id.as_str())
                .collect::<Vec<_>>(),
            vec!["run-2", "run-3"],
            "restore must keep the newest runs under the same FIFO policy as live writes"
        );
        for (run_index, record) in records.iter().enumerate() {
            assert_eq!(record.snapshot.event_count, 5);
            assert_eq!(record.events.len(), 2);
            assert_eq!(record.events[0].sequence, 3);
            assert_eq!(record.events[1].sequence, 4);
            assert_eq!(record.snapshot.id, format!("run-{}", run_index + 2));
        }
    }

    #[tokio::test]
    async fn replace_records_honors_zero_caps() {
        let source = InMemoryRunStore::new();
        let run = source.create_run("session-1", "prompt").await;
        source
            .record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: "event".to_string(),
                },
            )
            .await;

        let no_runs = InMemoryRunStore::with_retention(Some(0), Some(0));
        no_runs.replace_records(source.records().await).await;
        assert!(no_runs.records().await.is_empty());

        let no_events = InMemoryRunStore::with_retention(None, Some(0));
        no_events.replace_records(source.records().await).await;
        let records = no_events.records().await;
        assert_eq!(records.len(), 1);
        assert!(records[0].events.is_empty());
        assert_eq!(records[0].snapshot.event_count, 1);
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
    async fn max_event_bytes_per_run_drops_oversized_live_event_but_advances_cursor() {
        let store = InMemoryRunStore::with_retention_limits(None, None, Some(0));
        let run = store.create_run("session-1", "prompt").await;

        store
            .record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: "oversized".to_string(),
                },
            )
            .await;

        assert!(store.events(&run.id).await.is_empty());
        let snapshot = store.snapshot(&run.id).await.unwrap();
        assert_eq!(snapshot.event_count, 1);
    }

    #[tokio::test]
    async fn replace_records_enforces_serialized_event_byte_cap_fifo() {
        let source = InMemoryRunStore::new();
        let run = source.create_run("session-1", "prompt").await;
        for text in ["old", "middle", "new"] {
            source
                .record_event(
                    &run.id,
                    AgentEvent::TextDelta {
                        text: text.to_string(),
                    },
                )
                .await;
        }
        let source_records = source.records().await;
        let retained_bytes = source_records[0].events[1..]
            .iter()
            .map(serialized_event_record_len)
            .sum();

        let restored = InMemoryRunStore::with_retention_limits(None, None, Some(retained_bytes));
        restored.replace_records(source_records).await;

        let records = restored.records().await;
        assert_eq!(records[0].snapshot.event_count, 3);
        assert_eq!(
            records[0]
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[tokio::test]
    async fn retained_event_sequences_remain_monotonic_after_fifo_trim() {
        let store = InMemoryRunStore::with_retention(None, Some(3));
        let run = store.create_run("session-1", "prompt").await;

        for index in 0..10 {
            store
                .record_event(
                    &run.id,
                    AgentEvent::TextDelta {
                        text: index.to_string(),
                    },
                )
                .await;
        }

        let sequences = store
            .events(&run.id)
            .await
            .into_iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>();
        assert_eq!(sequences, vec![7, 8, 9]);
        assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[tokio::test]
    async fn restored_run_continues_sequence_from_cumulative_event_count() {
        let source = InMemoryRunStore::with_retention(None, Some(3));
        let run = source.create_run("session-1", "prompt").await;
        for index in 0..10 {
            source
                .record_event(
                    &run.id,
                    AgentEvent::TextDelta {
                        text: index.to_string(),
                    },
                )
                .await;
        }

        let restored = InMemoryRunStore::with_retention(None, Some(3));
        restored.replace_records(source.records().await).await;
        restored
            .record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: "after restore".to_string(),
                },
            )
            .await;

        let sequences = restored
            .events(&run.id)
            .await
            .into_iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>();
        assert_eq!(sequences, vec![8, 9, 10]);
        assert_eq!(restored.snapshot(&run.id).await.unwrap().event_count, 11);
    }

    #[tokio::test]
    async fn event_page_reports_retention_gap_and_paginates_from_cursor() {
        let store = InMemoryRunStore::with_retention(None, Some(3));
        let run = store.create_run("session-1", "prompt").await;
        for index in 0..6 {
            store
                .record_event(
                    &run.id,
                    AgentEvent::TextDelta {
                        text: index.to_string(),
                    },
                )
                .await;
        }

        let first = store.event_page(&run.id, None, 2).await.unwrap();
        assert_eq!(first.first_available_sequence, Some(3));
        assert_eq!(first.latest_sequence_exclusive, 6);
        assert!(first.retention_gap);
        assert!(first.has_more);
        assert_eq!(first.next_after_sequence, Some(4));
        assert_eq!(
            first
                .events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );

        let second = store
            .event_page(&run.id, first.next_after_sequence, 2)
            .await
            .unwrap();
        assert!(!second.retention_gap);
        assert!(!second.has_more);
        assert_eq!(second.next_after_sequence, Some(5));
        assert_eq!(second.events[0].sequence, 5);
        assert!(store.event_page("missing", None, 10).await.is_none());
    }

    #[tokio::test]
    async fn event_page_reports_gap_when_retention_keeps_no_events() {
        let store = InMemoryRunStore::with_retention(None, Some(0));
        let run = store.create_run("session-1", "prompt").await;
        store
            .record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: "gone".to_string(),
                },
            )
            .await;

        let page = store.event_page(&run.id, None, 10).await.unwrap();
        assert!(page.events.is_empty());
        assert_eq!(page.first_available_sequence, None);
        assert_eq!(page.latest_sequence_exclusive, 1);
        assert!(page.retention_gap);
        assert!(!page.has_more);
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
    // Events can arrive through independent runtime and high-level channels.
    // Keep recording late events for replay, but never let their delivery
    // order regress a terminal run back to Planning or Executing.
    if run.status.is_terminal() {
        return;
    }

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
        | AgentEvent::ToolExecutionStart { .. }
        | AgentEvent::TurnStart { .. }
            if !matches!(run.status, RunStatus::Planning) =>
        {
            run.status = RunStatus::Executing;
        }
        AgentEvent::End { text, .. } => {
            run.status = RunStatus::Completed;
            run.result_text = Some(text.clone());
            run.error = None;
        }
        AgentEvent::Error { message } => {
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

    #[tokio::test]
    async fn late_events_cannot_regress_a_terminal_run_status() {
        let store = InMemoryRunStore::new();
        let cancelled = store.create_run("session-1", "cancelled").await;
        store.mark_cancelled(&cancelled.id).await;
        store
            .record_event(&cancelled.id, AgentEvent::TurnStart { turn: 2 })
            .await;
        assert_eq!(
            store.snapshot(&cancelled.id).await.unwrap().status,
            RunStatus::Cancelled
        );

        let completed = store.create_run("session-1", "completed").await;
        store
            .record_event(
                &completed.id,
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
        store
            .record_event(
                &completed.id,
                AgentEvent::ToolExecutionStart {
                    id: "late-tool".to_string(),
                    name: "bash".to_string(),
                    args: serde_json::json!({}),
                },
            )
            .await;
        assert_eq!(
            store.snapshot(&completed.id).await.unwrap().status,
            RunStatus::Completed
        );
    }
}
