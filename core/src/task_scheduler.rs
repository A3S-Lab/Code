//! Agent-wide admission scheduler for top-level and background work.
//!
//! Session transcript admission remains single-flight. This scheduler adds a
//! shared capacity boundary across every session created by one [`Agent`](crate::Agent),
//! using `a3s-lane`'s stable priority queue for exact priority/FIFO ordering.

use crate::execution_identity::ExecutionIdentityV1;
use a3s_lane::{Priority, PriorityQueue};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_ACTIVE: usize = 4;
const DEFAULT_AGING_INTERVAL_MS: u64 = 30_000;

/// Relative importance of work admitted through an agent's shared scheduler.
///
/// Lower values run first. `Urgent` is reserved for explicit host control
/// actions and never participates in aging. Older work from the other classes
/// can age up to `Interactive`, but never ahead of `Urgent`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord,
)]
#[serde(rename_all = "camelCase")]
#[repr(u8)]
pub enum TaskPriority {
    Urgent = 0,
    #[default]
    Interactive = 1,
    Foreground = 2,
    Background = 3,
    Maintenance = 4,
}

impl TaskPriority {
    const ALL: [Self; 5] = [
        Self::Urgent,
        Self::Interactive,
        Self::Foreground,
        Self::Background,
        Self::Maintenance,
    ];

    fn lane_priority(self) -> Priority {
        self as Priority
    }
}

impl FromStr for TaskPriority {
    type Err = TaskSchedulerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "urgent" => Ok(Self::Urgent),
            "interactive" | "user" => Ok(Self::Interactive),
            "foreground" => Ok(Self::Foreground),
            "background" => Ok(Self::Background),
            "maintenance" => Ok(Self::Maintenance),
            _ => Err(TaskSchedulerError::InvalidConfig(format!(
                "unknown task priority '{value}'; expected urgent, interactive, foreground, background, or maintenance"
            ))),
        }
    }
}

/// Agent-wide task scheduler settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskSchedulerConfig {
    /// Maximum number of independently admitted tasks across all sessions.
    #[serde(default = "default_max_active", alias = "max_active")]
    pub max_active: usize,
    /// Time before queued work is promoted by one priority level.
    #[serde(default = "default_aging_interval_ms", alias = "aging_interval_ms")]
    pub aging_interval_ms: u64,
}

impl Default for TaskSchedulerConfig {
    fn default() -> Self {
        Self {
            max_active: default_max_active(),
            aging_interval_ms: default_aging_interval_ms(),
        }
    }
}

impl TaskSchedulerConfig {
    /// Validate configuration before starting the scheduler actor.
    pub fn validate(&self) -> Result<(), TaskSchedulerError> {
        if self.max_active == 0 {
            return Err(TaskSchedulerError::InvalidConfig(
                "maxActive must be greater than zero".to_string(),
            ));
        }
        if self.aging_interval_ms == 0 {
            return Err(TaskSchedulerError::InvalidConfig(
                "agingIntervalMs must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }
}

const fn default_max_active() -> usize {
    DEFAULT_MAX_ACTIVE
}

const fn default_aging_interval_ms() -> u64 {
    DEFAULT_AGING_INTERVAL_MS
}

/// Scheduler admission failures.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum TaskSchedulerError {
    #[error("task scheduler configuration is invalid: {0}")]
    InvalidConfig(String),
    #[error("task admission was cancelled")]
    Cancelled,
    #[error("task scheduler is closed")]
    Closed,
}

/// Counts grouped by the stable public priority classes.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskPriorityCounts {
    pub urgent: usize,
    pub interactive: usize,
    pub foreground: usize,
    pub background: usize,
    pub maintenance: usize,
}

impl TaskPriorityCounts {
    fn increment(&mut self, priority: TaskPriority) {
        match priority {
            TaskPriority::Urgent => self.urgent += 1,
            TaskPriority::Interactive => self.interactive += 1,
            TaskPriority::Foreground => self.foreground += 1,
            TaskPriority::Background => self.background += 1,
            TaskPriority::Maintenance => self.maintenance += 1,
        }
    }
}

/// Point-in-time scheduler occupancy for hosts and diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskSchedulerStats {
    pub max_active: usize,
    pub active: usize,
    pub pending: usize,
    pub active_by_priority: TaskPriorityCounts,
    pub pending_by_priority: TaskPriorityCounts,
    pub closed: bool,
}

/// Bounded cumulative admission and fairness diagnostics for one scheduler.
///
/// The counters are owned by the scheduler actor and never retain task labels,
/// execution identities, or queue entries.  They therefore remain safe to
/// expose to a host while still making starvation and lifecycle leaks
/// measurable.  Occupancy fields are sampled at the same actor turn as the
/// cumulative counters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskSchedulerHealthSnapshot {
    /// Configured global capacity.
    pub max_active: usize,
    /// Number of leases currently held.
    pub active: usize,
    /// Number of requests waiting for a lease.
    pub pending: usize,
    /// Current occupancy grouped by base priority.
    pub active_by_priority: TaskPriorityCounts,
    /// Current pending work grouped by base priority.
    pub pending_by_priority: TaskPriorityCounts,
    /// Number of requests that acquired a lease since scheduler creation.
    pub admitted: u64,
    /// Number of admitted leases whose ownership was released.
    pub released: u64,
    /// Number of admission requests cancelled before normal release,
    /// including queued requests and active leases cancelled by their caller.
    pub cancelled: u64,
    /// Number of requests rejected because the scheduler was closing.
    pub rejected: u64,
    /// Number of queued requests promoted by the aging policy.
    pub aging_promotions: u64,
    /// Highest number of simultaneously active leases observed.
    pub peak_active: usize,
    /// Sum of admission wait time in microseconds, saturating at `u64::MAX`.
    /// This is useful for host-side rate calculations without retaining a
    /// latency histogram in the execution kernel.
    pub total_wait_micros: u64,
    /// Mean admission wait time in microseconds (`total / admitted`).
    pub average_wait_micros: u64,
    /// Longest observed admission wait in microseconds.
    pub max_wait_micros: u64,
    /// Whether the scheduler is draining or has finished shutdown.
    pub closed: bool,
}

/// Shared actor handle. One instance belongs to each `Agent`.
#[derive(Debug)]
pub struct TaskScheduler {
    tx: mpsc::UnboundedSender<SchedulerMessage>,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
}

impl TaskScheduler {
    /// Start a scheduler on the current Tokio runtime.
    pub fn new(config: TaskSchedulerConfig) -> Result<Self, TaskSchedulerError> {
        config.validate()?;
        let (tx, rx) = mpsc::unbounded_channel();
        let closed = Arc::new(AtomicBool::new(false));
        tokio::spawn(run_scheduler(rx, config, Arc::clone(&closed)));
        Ok(Self {
            tx,
            next_id: AtomicU64::new(1),
            closed,
        })
    }

    /// Wait until this task owns one global execution slot.
    pub async fn acquire(
        &self,
        priority: TaskPriority,
        label: impl Into<String>,
        cancellation: &CancellationToken,
    ) -> Result<TaskLease, TaskSchedulerError> {
        self.acquire_with_identity(priority, label, None, cancellation)
            .await
    }

    /// Wait until this task owns one global execution slot and carry its
    /// semantic execution identity through the admission boundary.
    ///
    /// The identity is optional for backwards compatibility with callers that
    /// only need capacity. When present it is validated before anything is
    /// queued, and the resulting lease retains it for tracing and downstream
    /// adapters.
    pub async fn acquire_with_identity(
        &self,
        priority: TaskPriority,
        label: impl Into<String>,
        identity: Option<ExecutionIdentityV1>,
        cancellation: &CancellationToken,
    ) -> Result<TaskLease, TaskSchedulerError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TaskSchedulerError::Closed);
        }
        if cancellation.is_cancelled() {
            return Err(TaskSchedulerError::Cancelled);
        }
        if let Some(identity) = &identity {
            identity.validate().map_err(|error| {
                TaskSchedulerError::InvalidConfig(format!("execution identity is invalid: {error}"))
            })?;
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (ready_tx, ready_rx) = oneshot::channel();
        self.tx
            .send(SchedulerMessage::Enqueue(QueuedAdmission {
                id,
                priority,
                effective_priority: priority.lane_priority(),
                label: label.into(),
                identity: identity.clone(),
                enqueued_at: Instant::now(),
                ready: ready_tx,
            }))
            .map_err(|_| TaskSchedulerError::Closed)?;

        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                let _ = self.tx.send(SchedulerMessage::Cancel(id));
                Err(TaskSchedulerError::Cancelled)
            }
            ready = ready_rx => {
                ready.map_err(|_| TaskSchedulerError::Closed)??;
                Ok(TaskLease {
                    id,
                    tx: self.tx.clone(),
                    released: false,
                    identity,
                })
            }
        }
    }

    /// Return a consistent actor-owned occupancy snapshot.
    pub async fn stats(&self) -> Result<TaskSchedulerStats, TaskSchedulerError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TaskSchedulerError::Closed);
        }
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(SchedulerMessage::Stats(tx))
            .map_err(|_| TaskSchedulerError::Closed)?;
        rx.await.map_err(|_| TaskSchedulerError::Closed)
    }

    /// Return occupancy plus bounded cumulative admission/fairness counters.
    ///
    /// This is intentionally a separate method from [`Self::stats`] so the
    /// long-lived counters can be added without changing the established
    /// occupancy wire shape consumed by older SDKs.
    pub async fn health(&self) -> Result<TaskSchedulerHealthSnapshot, TaskSchedulerError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TaskSchedulerError::Closed);
        }
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(SchedulerMessage::Health(tx))
            .map_err(|_| TaskSchedulerError::Closed)?;
        rx.await.map_err(|_| TaskSchedulerError::Closed)
    }

    /// Reject pending work and wait for already-admitted leases to finish.
    pub async fn shutdown(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let (tx, rx) = oneshot::channel();
        if self.tx.send(SchedulerMessage::Shutdown(tx)).is_ok() {
            let _ = rx.await;
        }
    }
}

/// RAII ownership of one globally admitted execution slot.
pub struct TaskLease {
    id: u64,
    tx: mpsc::UnboundedSender<SchedulerMessage>,
    released: bool,
    identity: Option<ExecutionIdentityV1>,
}

impl TaskLease {
    /// Stable admission identifier, useful for tracing.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Semantic identity carried by this admission, when one was supplied.
    pub fn identity(&self) -> Option<&ExecutionIdentityV1> {
        self.identity.as_ref()
    }
}

impl Drop for TaskLease {
    fn drop(&mut self) {
        if !self.released {
            self.released = true;
            let _ = self.tx.send(SchedulerMessage::Release(self.id));
        }
    }
}

struct QueuedAdmission {
    id: u64,
    priority: TaskPriority,
    effective_priority: Priority,
    label: String,
    identity: Option<ExecutionIdentityV1>,
    enqueued_at: Instant,
    ready: oneshot::Sender<Result<(), TaskSchedulerError>>,
}

enum SchedulerMessage {
    Enqueue(QueuedAdmission),
    Cancel(u64),
    Release(u64),
    Stats(oneshot::Sender<TaskSchedulerStats>),
    Health(oneshot::Sender<TaskSchedulerHealthSnapshot>),
    Shutdown(oneshot::Sender<()>),
}

#[derive(Default)]
struct SchedulerCounters {
    admitted: u64,
    released: u64,
    cancelled: u64,
    rejected: u64,
    aging_promotions: u64,
    peak_active: usize,
    total_wait_micros: u64,
    max_wait_micros: u64,
}

struct SchedulerState {
    config: TaskSchedulerConfig,
    pending: PriorityQueue<QueuedAdmission>,
    cancelled: HashSet<u64>,
    active: HashMap<u64, TaskPriority>,
    closing: bool,
    shutdown_waiters: Vec<oneshot::Sender<()>>,
    counters: SchedulerCounters,
}

async fn run_scheduler(
    mut rx: mpsc::UnboundedReceiver<SchedulerMessage>,
    config: TaskSchedulerConfig,
    closed: Arc<AtomicBool>,
) {
    let mut state = SchedulerState {
        config,
        pending: PriorityQueue::new(),
        cancelled: HashSet::new(),
        active: HashMap::new(),
        closing: false,
        shutdown_waiters: Vec::new(),
        counters: SchedulerCounters::default(),
    };

    while let Some(message) = rx.recv().await {
        match message {
            SchedulerMessage::Enqueue(item) => {
                if state.closing {
                    state.counters.rejected = state.counters.rejected.saturating_add(1);
                    let _ = item.ready.send(Err(TaskSchedulerError::Closed));
                } else {
                    state.pending.push(item.effective_priority, item);
                    state.dispatch();
                }
            }
            SchedulerMessage::Cancel(id) => {
                if state.active.remove(&id).is_none() {
                    state.cancelled.insert(id);
                    state.purge_cancelled();
                } else {
                    state.counters.cancelled = state.counters.cancelled.saturating_add(1);
                }
                state.dispatch();
                state.finish_shutdown_if_idle();
            }
            SchedulerMessage::Release(id) => {
                if state.active.remove(&id).is_some() {
                    state.counters.released = state.counters.released.saturating_add(1);
                }
                state.dispatch();
                state.finish_shutdown_if_idle();
            }
            SchedulerMessage::Stats(reply) => {
                let _ = reply.send(state.snapshot());
            }
            SchedulerMessage::Health(reply) => {
                // A health read is also a scheduling observation point. Apply
                // elapsed aging before taking the snapshot so operators see
                // promotions that became eligible while capacity was full,
                // even when no new admission or release arrived yet.
                state.apply_aging();
                let _ = reply.send(state.health_snapshot());
            }
            SchedulerMessage::Shutdown(reply) => {
                state.closing = true;
                closed.store(true, Ordering::Release);
                while let Some(item) = state.pending.pop() {
                    let item = item.into_value();
                    state.counters.rejected = state.counters.rejected.saturating_add(1);
                    let _ = item.ready.send(Err(TaskSchedulerError::Closed));
                }
                state.cancelled.clear();
                state.shutdown_waiters.push(reply);
                state.finish_shutdown_if_idle();
            }
        }

        if state.closing && state.active.is_empty() && state.shutdown_waiters.is_empty() {
            break;
        }
    }

    closed.store(true, Ordering::Release);
}

impl SchedulerState {
    fn purge_cancelled(&mut self) {
        if self.cancelled.is_empty() {
            return;
        }
        if self.pending.is_empty() {
            // Admission ids are never reused. A cancellation that arrives
            // after a lease was released cannot match a future queue item;
            // discard the tombstone instead of retaining it indefinitely.
            self.cancelled.clear();
            return;
        }
        let mut retained = Vec::with_capacity(self.pending.len());
        while let Some(item) = self.pending.pop() {
            if self.cancelled.remove(&item.value().id) {
                let item = item.into_value();
                self.counters.cancelled = self.counters.cancelled.saturating_add(1);
                let _ = item.ready.send(Err(TaskSchedulerError::Cancelled));
            } else {
                retained.push(item);
            }
        }
        for item in retained {
            self.pending.restore(item);
        }
        if self.pending.is_empty() {
            self.cancelled.clear();
        }
    }

    fn dispatch(&mut self) {
        if self.closing {
            return;
        }
        self.apply_aging();
        while self.active.len() < self.config.max_active {
            let Some(item) = self.pending.pop() else {
                break;
            };
            let item = item.into_value();
            if self.cancelled.remove(&item.id) {
                continue;
            }

            let id = item.id;
            let priority = item.priority;
            let label = item.label;
            let identity = item.identity;
            let wait_micros = item
                .enqueued_at
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            self.active.insert(id, priority);
            if item.ready.send(Ok(())).is_err() {
                self.active.remove(&id);
                continue;
            }
            self.counters.admitted = self.counters.admitted.saturating_add(1);
            self.counters.total_wait_micros =
                self.counters.total_wait_micros.saturating_add(wait_micros);
            self.counters.max_wait_micros = self.counters.max_wait_micros.max(wait_micros);
            self.counters.peak_active = self.counters.peak_active.max(self.active.len());
            tracing::trace!(
                admission_id = id,
                ?priority,
                %label,
                execution_identity = identity.as_ref().map(ExecutionIdentityV1::key).unwrap_or(""),
                "task admitted"
            );
        }
        if self.pending.is_empty() {
            self.cancelled.clear();
        }
    }

    fn apply_aging(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let now = Instant::now();
        let interval_ms = self.config.aging_interval_ms as u128;
        let mut entries = Vec::with_capacity(self.pending.len());
        while let Some(item) = self.pending.pop() {
            entries.push((item.sequence(), item.into_value()));
        }
        // Re-insertion gives Lane fresh sequence numbers. Insert in original
        // sequence order so work that ages into the same class remains FIFO.
        entries.sort_by_key(|(sequence, _)| *sequence);
        for (_, item) in entries {
            let elapsed_ms = now.duration_since(item.enqueued_at).as_millis();
            let levels = (elapsed_ms / interval_ms).min(u8::MAX as u128) as u8;
            let effective = if item.priority == TaskPriority::Urgent {
                TaskPriority::Urgent.lane_priority()
            } else {
                (item.priority as u8).saturating_sub(levels).max(1) as Priority
            };
            if effective < item.effective_priority {
                self.counters.aging_promotions = self.counters.aging_promotions.saturating_add(1);
            }
            let mut item = item;
            item.effective_priority = effective;
            self.pending.push(item.effective_priority, item);
        }
    }

    fn snapshot(&self) -> TaskSchedulerStats {
        let mut active_by_priority = TaskPriorityCounts::default();
        for priority in self.active.values() {
            active_by_priority.increment(*priority);
        }
        let mut pending_by_priority = TaskPriorityCounts::default();
        for item in self.pending.ordered() {
            pending_by_priority.increment(item.value().priority);
        }
        debug_assert_eq!(
            TaskPriority::ALL
                .iter()
                .map(|priority| match priority {
                    TaskPriority::Urgent => active_by_priority.urgent,
                    TaskPriority::Interactive => active_by_priority.interactive,
                    TaskPriority::Foreground => active_by_priority.foreground,
                    TaskPriority::Background => active_by_priority.background,
                    TaskPriority::Maintenance => active_by_priority.maintenance,
                })
                .sum::<usize>(),
            self.active.len()
        );
        TaskSchedulerStats {
            max_active: self.config.max_active,
            active: self.active.len(),
            pending: self.pending.len(),
            active_by_priority,
            pending_by_priority,
            closed: self.closing,
        }
    }

    fn health_snapshot(&self) -> TaskSchedulerHealthSnapshot {
        let stats = self.snapshot();
        TaskSchedulerHealthSnapshot {
            max_active: stats.max_active,
            active: stats.active,
            pending: stats.pending,
            active_by_priority: stats.active_by_priority,
            pending_by_priority: stats.pending_by_priority,
            admitted: self.counters.admitted,
            released: self.counters.released,
            cancelled: self.counters.cancelled,
            rejected: self.counters.rejected,
            aging_promotions: self.counters.aging_promotions,
            peak_active: self.counters.peak_active,
            total_wait_micros: self.counters.total_wait_micros,
            average_wait_micros: self
                .counters
                .total_wait_micros
                .checked_div(self.counters.admitted)
                .unwrap_or(0),
            max_wait_micros: self.counters.max_wait_micros,
            closed: self.closing,
        }
    }

    fn finish_shutdown_if_idle(&mut self) {
        if self.closing && self.active.is_empty() {
            for waiter in self.shutdown_waiters.drain(..) {
                let _ = waiter.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scheduler(max_active: usize, aging_interval_ms: u64) -> TaskScheduler {
        TaskScheduler::new(TaskSchedulerConfig {
            max_active,
            aging_interval_ms,
        })
        .unwrap()
    }

    #[test]
    fn priority_names_are_stable_and_reject_unknown_values() {
        assert_eq!("user".parse(), Ok(TaskPriority::Interactive));
        assert_eq!("background".parse(), Ok(TaskPriority::Background));
        assert!("eventually".parse::<TaskPriority>().is_err());
    }

    async fn wait_for_pending(scheduler: &TaskScheduler, expected: usize) {
        for _ in 0..100 {
            if scheduler.stats().await.unwrap().pending == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("scheduler never reached {expected} pending tasks");
    }

    #[tokio::test]
    async fn strict_priority_and_fifo_are_enforced_globally() {
        let scheduler = Arc::new(scheduler(1, 60_000));
        let blocker = scheduler
            .acquire(
                TaskPriority::Interactive,
                "blocker",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (order_tx, mut order_rx) = mpsc::unbounded_channel();

        for (name, priority) in [
            ("background", TaskPriority::Background),
            ("interactive-1", TaskPriority::Interactive),
            ("foreground", TaskPriority::Foreground),
            ("interactive-2", TaskPriority::Interactive),
            ("urgent", TaskPriority::Urgent),
        ] {
            let expected = scheduler.stats().await.unwrap().pending + 1;
            let task_scheduler = Arc::clone(&scheduler);
            let order_tx = order_tx.clone();
            tokio::spawn(async move {
                let lease = task_scheduler
                    .acquire(priority, name, &CancellationToken::new())
                    .await
                    .unwrap();
                order_tx.send(name).unwrap();
                drop(lease);
            });
            wait_for_pending(&scheduler, expected).await;
        }

        drop(blocker);
        let mut actual = Vec::new();
        for _ in 0..5 {
            actual.push(order_rx.recv().await.unwrap());
        }
        assert_eq!(
            actual,
            [
                "urgent",
                "interactive-1",
                "interactive-2",
                "foreground",
                "background"
            ]
        );
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn cancellation_does_not_consume_capacity() {
        let scheduler = Arc::new(scheduler(1, 60_000));
        let blocker = scheduler
            .acquire(
                TaskPriority::Interactive,
                "blocker",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let cancellation = CancellationToken::new();
        let cancelled_task = {
            let scheduler = Arc::clone(&scheduler);
            let cancellation = cancellation.clone();
            tokio::spawn(async move {
                scheduler
                    .acquire(TaskPriority::Urgent, "cancelled", &cancellation)
                    .await
            })
        };
        wait_for_pending(&scheduler, 1).await;
        cancellation.cancel();
        assert!(matches!(
            cancelled_task.await.unwrap(),
            Err(TaskSchedulerError::Cancelled)
        ));

        let next = {
            let scheduler = Arc::clone(&scheduler);
            tokio::spawn(async move {
                scheduler
                    .acquire(TaskPriority::Background, "next", &CancellationToken::new())
                    .await
            })
        };
        wait_for_pending(&scheduler, 1).await;
        drop(blocker);
        let lease = next.await.unwrap().unwrap();
        assert_eq!(scheduler.stats().await.unwrap().active, 1);
        drop(lease);
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn aging_prevents_background_starvation() {
        let scheduler = Arc::new(scheduler(1, 2));
        let blocker = scheduler
            .acquire(
                TaskPriority::Interactive,
                "blocker",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (order_tx, mut order_rx) = mpsc::unbounded_channel();
        let background = {
            let scheduler = Arc::clone(&scheduler);
            let order_tx = order_tx.clone();
            tokio::spawn(async move {
                let lease = scheduler
                    .acquire(
                        TaskPriority::Background,
                        "old-background",
                        &CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                order_tx.send("background").unwrap();
                drop(lease);
            })
        };
        wait_for_pending(&scheduler, 1).await;
        tokio::time::sleep(Duration::from_millis(8)).await;
        let interactive = {
            let scheduler = Arc::clone(&scheduler);
            let order_tx = order_tx.clone();
            tokio::spawn(async move {
                let lease = scheduler
                    .acquire(
                        TaskPriority::Interactive,
                        "new-interactive",
                        &CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                order_tx.send("interactive").unwrap();
                drop(lease);
            })
        };
        wait_for_pending(&scheduler, 2).await;
        drop(blocker);

        assert_eq!(order_rx.recv().await.unwrap(), "background");
        assert_eq!(order_rx.recv().await.unwrap(), "interactive");
        background.await.unwrap();
        interactive.await.unwrap();
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_rejects_pending_and_waits_for_active_lease() {
        let scheduler = Arc::new(scheduler(1, 60_000));
        let blocker = scheduler
            .acquire(
                TaskPriority::Interactive,
                "blocker",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let pending = {
            let scheduler = Arc::clone(&scheduler);
            tokio::spawn(async move {
                scheduler
                    .acquire(
                        TaskPriority::Background,
                        "pending",
                        &CancellationToken::new(),
                    )
                    .await
            })
        };
        wait_for_pending(&scheduler, 1).await;
        let shutdown = {
            let scheduler = Arc::clone(&scheduler);
            tokio::spawn(async move { scheduler.shutdown().await })
        };
        assert!(matches!(
            pending.await.unwrap(),
            Err(TaskSchedulerError::Closed)
        ));
        assert!(!shutdown.is_finished());
        drop(blocker);
        shutdown.await.unwrap();
        assert!(matches!(
            scheduler
                .acquire(TaskPriority::Urgent, "late", &CancellationToken::new())
                .await,
            Err(TaskSchedulerError::Closed)
        ));
    }

    #[tokio::test]
    async fn stats_report_base_priority_occupancy() {
        let scheduler = Arc::new(scheduler(1, 60_000));
        let blocker = scheduler
            .acquire(
                TaskPriority::Foreground,
                "blocker",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let waiting = {
            let scheduler = Arc::clone(&scheduler);
            tokio::spawn(async move {
                scheduler
                    .acquire(
                        TaskPriority::Maintenance,
                        "waiting",
                        &CancellationToken::new(),
                    )
                    .await
            })
        };
        wait_for_pending(&scheduler, 1).await;
        let stats = scheduler.stats().await.unwrap();
        assert_eq!(stats.max_active, 1);
        assert_eq!(stats.active_by_priority.foreground, 1);
        assert_eq!(stats.pending_by_priority.maintenance, 1);
        drop(blocker);
        drop(waiting.await.unwrap().unwrap());
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn health_reports_bounded_admission_and_wait_counters() {
        let scheduler = Arc::new(scheduler(1, 2));
        let blocker = scheduler
            .acquire(
                TaskPriority::Interactive,
                "health-blocker",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let cancellation = CancellationToken::new();
        let cancelled = {
            let scheduler = Arc::clone(&scheduler);
            let cancellation = cancellation.clone();
            tokio::spawn(async move {
                scheduler
                    .acquire(TaskPriority::Background, "health-cancelled", &cancellation)
                    .await
            })
        };
        wait_for_pending(&scheduler, 1).await;
        cancellation.cancel();
        assert!(matches!(
            cancelled.await.unwrap(),
            Err(TaskSchedulerError::Cancelled)
        ));

        let waiting = {
            let scheduler = Arc::clone(&scheduler);
            tokio::spawn(async move {
                scheduler
                    .acquire(
                        TaskPriority::Foreground,
                        "health-waiting",
                        &CancellationToken::new(),
                    )
                    .await
            })
        };
        wait_for_pending(&scheduler, 1).await;
        tokio::time::sleep(Duration::from_millis(6)).await;
        drop(blocker);
        let lease = waiting.await.unwrap().unwrap();
        drop(lease);
        let health = scheduler.health().await.unwrap();
        assert_eq!(health.active, 0);
        assert_eq!(health.pending, 0);
        assert_eq!(health.admitted, 2);
        assert_eq!(health.released, 2);
        assert_eq!(health.cancelled, 1);
        assert!(health.aging_promotions >= 1);
        assert!(health.peak_active >= 1);
        assert!(health.total_wait_micros > 0);
        assert!(health.max_wait_micros >= health.average_wait_micros);
        assert!(!health.closed);
        scheduler.shutdown().await;
        assert!(scheduler.health().await.is_err());
    }

    #[tokio::test]
    async fn aging_keeps_a_resumed_workflow_step_from_starving() {
        let scheduler = Arc::new(scheduler(1, 2));
        let blocker = scheduler
            .acquire(
                TaskPriority::Interactive,
                "resumed-run-blocker",
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let (order_tx, mut order_rx) = mpsc::unbounded_channel();
        let resumed = {
            let scheduler = Arc::clone(&scheduler);
            let order_tx = order_tx.clone();
            tokio::spawn(async move {
                let lease = scheduler
                    .acquire(
                        TaskPriority::Background,
                        "flow:resumed-run:step-1",
                        &CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                order_tx.send("resumed").unwrap();
                drop(lease);
            })
        };
        wait_for_pending(&scheduler, 1).await;
        tokio::time::sleep(Duration::from_millis(8)).await;

        // Simulate a stream of newly resumed interactive work arriving while
        // the original run waits. The aged continuation must be admitted
        // before those newer requests once capacity is released.
        let mut interactive = Vec::new();
        for index in 0..8 {
            let scheduler = Arc::clone(&scheduler);
            let wait_scheduler = Arc::clone(&scheduler);
            let order_tx = order_tx.clone();
            interactive.push(tokio::spawn(async move {
                let lease = scheduler
                    .acquire(
                        TaskPriority::Interactive,
                        format!("flow:new-run-{index}:step-1"),
                        &CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                order_tx.send("interactive").unwrap();
                drop(lease);
            }));
            wait_for_pending(&wait_scheduler, index + 2).await;
        }
        let aged = scheduler.health().await.unwrap();
        assert!(aged.aging_promotions >= 1);
        drop(blocker);
        assert_eq!(order_rx.recv().await.unwrap(), "resumed");
        for task in interactive {
            task.await.unwrap();
        }
        resumed.await.unwrap();
        let health = scheduler.health().await.unwrap();
        assert!(health.aging_promotions >= 1);
        assert_eq!(health.pending, 0);
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn identity_is_carried_by_global_admission_lease() {
        let scheduler = scheduler(1, 60_000);
        let identity = ExecutionIdentityV1::derive(
            crate::execution_identity::FLOW_STEP_IDENTITY_DOMAIN_V1,
            &serde_json::json!({
                "run_id": "run-1",
                "step_id": "step-1",
                "step_name": "read",
                "input": {"path": "README.md"},
            }),
        )
        .unwrap();
        let lease = scheduler
            .acquire_with_identity(
                TaskPriority::Foreground,
                "flow:run-1:step-1:read",
                Some(identity.clone()),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(lease.identity(), Some(&identity));
        drop(lease);
        scheduler.shutdown().await;
    }
}
