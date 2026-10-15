//! Durable run primitives for agent executions.
//!
//! This module is intentionally small: it records runtime events and maintains a
//! stable run status snapshot that can be persisted by session stores.

use crate::agent::AgentEvent;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

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

/// One terminal outcome applied to an admitted Run.
///
/// `Completed` is normally materialized by the authoritative `End` event,
/// while cancellation and failure are applied by control or lifecycle
/// boundaries that may finish before an event arrives. Keeping the transition
/// type beside the Run store gives every caller one typed write primitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunTerminalTransition {
    #[allow(dead_code)]
    Completed,
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEventRecord {
    pub sequence: usize,
    pub timestamp_ms: u64,
    pub event: AgentEvent,
}

impl RunEventRecord {
    /// Project this retained run event into the shared Core identity plane.
    ///
    /// The projection is intentionally computed on demand so the existing
    /// persisted run/event wire shape remains unchanged during migration.
    pub fn core_identity(
        &self,
        operation_id: crate::core_identity::OperationId,
        source_revision: crate::core_identity::SourceRevision,
        capability_stamp: Option<crate::core_identity::CapabilityStamp>,
    ) -> Result<crate::core_identity::CoreEventIdentity, crate::core_identity::CoreIdentityError>
    {
        crate::core_identity::CoreEventIdentity::from_run_event(
            operation_id,
            source_revision,
            capability_stamp,
            self,
        )
    }
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
    /// Exact cognitive Knowledge binding frozen at Run admission.
    ///
    /// The non-serializable provider and query lease stay in the Run-owned
    /// capability projection. This identity remains durable even when old
    /// events are FIFO-trimmed or the Session catalog advances.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cognitive_package_binding: Option<crate::cognitive_context::CognitivePackageBindingV1>,
    /// Complete scoped capability identity frozen at Run admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_binding: Option<crate::capability::RunCapabilityBindingV1>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub event_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_change_set: Option<RunWorkspaceChangeSet>,
}

/// Immutable workspace evidence captured around one exact run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunWorkspaceChangeSet {
    pub base_tree: String,
    pub result_tree: String,
    pub patch_digest: String,
    pub patch_bytes: u64,
    pub patch_base64: String,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunWorkspaceChangeSetError {
    #[error("run was not found")]
    RunNotFound,
    #[error("run is not terminal")]
    RunNotTerminal,
    #[error("run workspace change set conflicts with immutable evidence")]
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub snapshot: RunSnapshot,
    pub events: Vec<RunEventRecord>,
}

/// Outcome of atomically reserving one host-selected run identity.
#[derive(Debug, Clone)]
pub enum RunReservation {
    Created(RunSnapshot),
    Existing(RunSnapshot),
}

impl RunReservation {
    pub fn snapshot(&self) -> &RunSnapshot {
        match self {
            Self::Created(snapshot) | Self::Existing(snapshot) => snapshot,
        }
    }

    pub const fn replayed(&self) -> bool {
        matches!(self, Self::Existing(_))
    }
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunCognitiveBindingError {
    #[error("run was not found")]
    RunNotFound,
    #[error("cognitive binding is invalid: {0}")]
    InvalidBinding(String),
    #[error("run has already crossed its cognitive binding admission boundary")]
    AlreadyObserved,
    #[error("run cognitive binding conflicts with immutable admission evidence")]
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunCapabilityAdmissionError {
    #[error("run was not found")]
    RunNotFound,
    #[error("capability binding is invalid: {0}")]
    InvalidBinding(String),
    #[error("run has already crossed its capability binding admission boundary")]
    AlreadyObserved,
    #[error("run capability binding conflicts with immutable admission evidence")]
    Conflict,
}

/// One atomic read generation of a run snapshot and its retained event page.
///
/// The protocol host uses this internal projection so a concurrent event
/// cannot be observed in the page while its state and logical timestamp still
/// come from an older snapshot.
#[derive(Debug, Clone)]
pub(crate) struct RunEventObservation {
    pub(crate) snapshot: RunSnapshot,
    pub(crate) page: RunEventPage,
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
            cognitive_package_binding: None,
            capability_binding: None,
            created_at_ms: now,
            updated_at_ms: now,
            result_text: None,
            error: None,
            event_count: 0,
            workspace_change_set: None,
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
        // Default ID generation for callers that do not need a host-selected
        // identity. Session orchestration uses `reserve_run_with_id` so a
        // repeated host ID cannot replace retained Run history.
        let id = format!("run-{}", uuid::Uuid::new_v4());
        self.create_run_with_id(id, session_id, prompt).await
    }

    /// Unconditionally insert a run with a caller-supplied id.
    ///
    /// This compatibility primitive replaces an existing record with the same
    /// id. New orchestration paths that consume host- or externally-selected
    /// identities must use [`Self::reserve_run_with_id`] instead.
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
            // This compatibility API intentionally replaces an existing
            // record. Remove its old position first so the FIFO index remains
            // one-to-one with the run map instead of retaining duplicate IDs.
            order.retain(|run_id| run_id != &id);
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

    /// Atomically reserve a caller-supplied run id without replacing an
    /// existing run. Normal session orchestration uses it to reject retained
    /// host-ID collisions; headless hosts additionally use the returned
    /// reservation as the Code-owned idempotency boundary for external replay.
    pub async fn reserve_run_with_id(
        &self,
        id: String,
        session_id: &str,
        prompt: &str,
    ) -> RunReservation {
        // Keep the same canonical lock order as create/read paths. Checking
        // and inserting while all three guards are held prevents concurrent
        // command replays from both claiming the same exact run id.
        let mut order = self.insertion_order.write().await;
        let mut events = self.events.write().await;
        let mut runs = self.runs.write().await;
        if let Some(existing) = runs.get(&id) {
            return RunReservation::Existing(existing.clone());
        }

        let snapshot = RunSnapshot::new(id.clone(), session_id.to_string(), prompt.to_string());
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
        RunReservation::Created(snapshot)
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
        let next_event_count = sequence.checked_add(1)?;
        // Wall clocks may move backwards and persisted runs may come from a
        // host whose clock was ahead. Keep Code's run-local observation time
        // monotonic so event-page validation and replay never regress.
        let timestamp_ms = now_ms().max(run.updated_at_ms);
        let record = RunEventRecord {
            sequence,
            timestamp_ms,
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
        run.event_count = next_event_count;
        run.updated_at_ms = timestamp_ms;
        Some(run.clone())
    }

    pub async fn mark_failed(&self, run_id: &str, error: impl Into<String>) -> Option<RunSnapshot> {
        let mut runs = self.runs.write().await;
        let run = runs.get_mut(run_id)?;
        // Terminal state is monotonic. A late worker error must not rewrite a
        // completed or cancelled run (the event stream can legitimately
        // deliver cleanup notifications after the terminal event).
        if run.status.is_terminal() {
            return Some(run.clone());
        }
        run.status = RunStatus::Failed;
        run.error = Some(error.into());
        run.updated_at_ms = now_ms().max(run.updated_at_ms);
        Some(run.clone())
    }

    pub async fn mark_cancelled(&self, run_id: &str) -> Option<RunSnapshot> {
        let mut runs = self.runs.write().await;
        let run = runs.get_mut(run_id)?;
        // Cancellation is a request, not a retroactive rewrite of an already
        // terminal outcome. This preserves the first terminal observation
        // under races between host close and the final End/Error event.
        if run.status.is_terminal() {
            return Some(run.clone());
        }
        run.status = RunStatus::Cancelled;
        run.updated_at_ms = now_ms().max(run.updated_at_ms);
        Some(run.clone())
    }

    /// Apply one terminal transition without allowing a late outcome to
    /// rewrite an already terminal Run.
    ///
    /// Successful Runs are finalized by `AgentEvent::End`, so the
    /// `Completed` transition is intentionally a read-only acknowledgement.
    /// Cancellation and failure share the same monotonic primitives used by
    /// the rest of the Run API, which keeps host cancellation and lifecycle
    /// cleanup on one typed storage boundary.
    pub(crate) async fn settle_terminal(
        &self,
        run_id: &str,
        transition: RunTerminalTransition,
    ) -> Option<RunSnapshot> {
        match transition {
            RunTerminalTransition::Completed => self.snapshot(run_id).await,
            RunTerminalTransition::Cancelled => self.mark_cancelled(run_id).await,
            RunTerminalTransition::Failed(error) => self.mark_failed(run_id, error).await,
        }
    }

    pub async fn snapshot(&self, run_id: &str) -> Option<RunSnapshot> {
        self.runs.read().await.get(run_id).cloned()
    }

    /// Bind the exact cognitive generation before the Run can emit events.
    /// Exact replay is idempotent; late or conflicting writes fail closed.
    pub async fn bind_cognitive_package(
        &self,
        run_id: &str,
        binding: crate::cognitive_context::CognitivePackageBindingV1,
    ) -> Result<RunSnapshot, RunCognitiveBindingError> {
        binding
            .validate()
            .map_err(|error| RunCognitiveBindingError::InvalidBinding(error.to_string()))?;
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or(RunCognitiveBindingError::RunNotFound)?;
        match &run.cognitive_package_binding {
            Some(existing) if existing == &binding => return Ok(run.clone()),
            Some(_) => return Err(RunCognitiveBindingError::Conflict),
            None => {}
        }
        if run.event_count != 0 || run.status != RunStatus::Created {
            return Err(RunCognitiveBindingError::AlreadyObserved);
        }
        run.cognitive_package_binding = Some(binding);
        run.updated_at_ms = now_ms().max(run.updated_at_ms);
        Ok(run.clone())
    }

    /// Bind the complete scoped capability identity before the Run can emit
    /// events. Exact replay is idempotent; late or conflicting writes fail
    /// closed.
    pub async fn bind_capability_generation(
        &self,
        run_id: &str,
        binding: crate::capability::RunCapabilityBindingV1,
    ) -> Result<RunSnapshot, RunCapabilityAdmissionError> {
        binding
            .validate()
            .map_err(|error| RunCapabilityAdmissionError::InvalidBinding(error.to_string()))?;
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or(RunCapabilityAdmissionError::RunNotFound)?;
        match &run.capability_binding {
            Some(existing) if existing == &binding => return Ok(run.clone()),
            Some(_) => return Err(RunCapabilityAdmissionError::Conflict),
            None => {}
        }
        if run.event_count != 0 || run.status != RunStatus::Created {
            return Err(RunCapabilityAdmissionError::AlreadyObserved);
        }
        run.capability_binding = Some(binding);
        run.updated_at_ms = now_ms().max(run.updated_at_ms);
        Ok(run.clone())
    }

    /// Bind one immutable workspace change set to an already terminal run.
    /// Exact replay is accepted; a different second write fails closed.
    pub async fn record_workspace_change_set(
        &self,
        run_id: &str,
        change_set: RunWorkspaceChangeSet,
    ) -> Result<RunSnapshot, RunWorkspaceChangeSetError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or(RunWorkspaceChangeSetError::RunNotFound)?;
        if !run.status.is_terminal() {
            return Err(RunWorkspaceChangeSetError::RunNotTerminal);
        }
        match &run.workspace_change_set {
            Some(existing) if existing == &change_set => return Ok(run.clone()),
            Some(_) => return Err(RunWorkspaceChangeSetError::Conflict),
            None => {}
        }
        run.workspace_change_set = Some(change_set);
        Ok(run.clone())
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
        self.event_observation(run_id, after_sequence, limit)
            .await
            .map(|observation| observation.page)
    }

    /// Read the run snapshot and retained event page under one lock generation.
    pub(crate) async fn event_observation(
        &self,
        run_id: &str,
        after_sequence: Option<usize>,
        limit: usize,
    ) -> Option<RunEventObservation> {
        // Match the canonical events -> runs lock order used by record_event.
        let events = self.events.read().await;
        let runs = self.runs.read().await;
        let retained = events.get(run_id)?;
        let run = runs.get(run_id)?;
        Some(RunEventObservation {
            snapshot: run.clone(),
            page: retained_event_page(retained, run, after_sequence, limit),
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

fn retained_event_page(
    retained: &RetainedRunEvents,
    run: &RunSnapshot,
    after_sequence: Option<usize>,
    limit: usize,
) -> RunEventPage {
    let first_available_sequence = retained.records.first().map(|event| event.sequence);
    let requested_start = after_sequence
        .map(|sequence| sequence.saturating_add(1))
        .unwrap_or(0);
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
    RunEventPage {
        events: page_events,
        first_available_sequence,
        latest_sequence_exclusive: run.event_count,
        next_after_sequence,
        retention_gap,
        has_more,
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

#[derive(Debug, Clone)]
pub struct RunHandle {
    id: String,
    session_id: String,
    store: Arc<InMemoryRunStore>,
    cancel_pin: crate::agent_api::RunCancelPin,
}

impl RunHandle {
    pub(crate) fn new(
        id: String,
        session_id: String,
        store: Arc<InMemoryRunStore>,
        cancel_pin: crate::agent_api::RunCancelPin,
    ) -> Self {
        Self {
            id,
            session_id,
            store,
            cancel_pin,
        }
    }

    pub(crate) fn has_cancel_pin(&self, actual: &crate::agent_api::RunCancelPin) -> bool {
        self.cancel_pin.same_original(actual)
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

    /// True acknowledges a request to this exact original worker, not terminal publication.
    pub async fn cancel(&self) -> bool {
        self.cancel_pin
            .request_cancel(&self.session_id, &self.id)
            .await
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
mod retention_tests;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod owner_cancel_tests;
