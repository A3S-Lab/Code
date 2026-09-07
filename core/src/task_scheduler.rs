//! Agent-wide admission scheduler for top-level and background work.
//!
//! Session transcript admission remains single-flight. This scheduler adds a
//! shared capacity boundary across every session created by one [`Agent`](crate::Agent),
//! using `a3s-lane`'s stable priority queue for exact priority/FIFO ordering.

use a3s_lane::{Priority, PriorityQueue};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_ACTIVE: usize = 4;
const DEFAULT_AGING_INTERVAL_MS: u64 = 30_000;
// Admission is backpressured before this many messages can be retained by the
// scheduler actor. Release notifications use a separate control channel and
// therefore cannot be starved by a full admission queue.
const MAX_PENDING_ADMISSIONS: usize = 4_096;

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

/// Shared actor handle. One instance belongs to each `Agent`.
#[derive(Debug)]
pub struct TaskScheduler {
    tx: mpsc::Sender<SchedulerMessage>,
    release_tx: mpsc::UnboundedSender<u64>,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
}

impl TaskScheduler {
    /// Start a scheduler on the current Tokio runtime.
    pub fn new(config: TaskSchedulerConfig) -> Result<Self, TaskSchedulerError> {
        config.validate()?;
        let (tx, rx) = mpsc::channel(MAX_PENDING_ADMISSIONS);
        let (release_tx, release_rx) = mpsc::unbounded_channel();
        let closed = Arc::new(AtomicBool::new(false));
        tokio::spawn(run_scheduler(rx, release_rx, config, Arc::clone(&closed)));
        Ok(Self {
            tx,
            release_tx,
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
        if self.closed.load(Ordering::Acquire) {
            return Err(TaskSchedulerError::Closed);
        }
        if cancellation.is_cancelled() {
            return Err(TaskSchedulerError::Cancelled);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (ready_tx, ready_rx) = oneshot::channel();
        let lease = TaskLease {
            id,
            release_tx: self.release_tx.clone(),
            released: false,
        };

        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                // Dropping the lease emits Release. Because Enqueue was sent
                // first on this channel, the actor removes either the queued
                // item or the just-admitted slot without a tombstone set.
                Err(TaskSchedulerError::Cancelled)
            }
            sent = self.tx.send(SchedulerMessage::Enqueue(QueuedAdmission {
                id,
                priority,
                label: label.into(),
                enqueued_at: Instant::now(),
                ready: ready_tx,
            })) => {
                sent.map_err(|_| TaskSchedulerError::Closed)?;
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => Err(TaskSchedulerError::Cancelled),
                    ready = ready_rx => {
                        ready.map_err(|_| TaskSchedulerError::Closed)??;
                        Ok(lease)
                    }
                }
            }
        }
    }

    /// Return a consistent actor-owned occupancy snapshot.
    pub async fn stats(&self) -> Result<TaskSchedulerStats, TaskSchedulerError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(SchedulerMessage::Stats(tx))
            .await
            .map_err(|_| TaskSchedulerError::Closed)?;
        rx.await.map_err(|_| TaskSchedulerError::Closed)
    }

    /// Reject pending work and wait for already-admitted leases to finish.
    pub async fn shutdown(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let (tx, rx) = oneshot::channel();
        if self.tx.send(SchedulerMessage::Shutdown(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }
}

/// RAII ownership of one globally admitted execution slot.
pub struct TaskLease {
    id: u64,
    release_tx: mpsc::UnboundedSender<u64>,
    released: bool,
}

impl TaskLease {
    /// Stable admission identifier, useful for tracing.
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for TaskLease {
    fn drop(&mut self) {
        if !self.released {
            self.released = true;
            let _ = self.release_tx.send(self.id);
        }
    }
}

struct QueuedAdmission {
    id: u64,
    priority: TaskPriority,
    label: String,
    enqueued_at: Instant,
    ready: oneshot::Sender<Result<(), TaskSchedulerError>>,
}

enum SchedulerMessage {
    Enqueue(QueuedAdmission),
    Release(u64),
    Stats(oneshot::Sender<TaskSchedulerStats>),
    Shutdown(oneshot::Sender<()>),
}

struct SchedulerState {
    config: TaskSchedulerConfig,
    pending: PriorityQueue<QueuedAdmission>,
    active: HashMap<u64, TaskPriority>,
    closing: bool,
    shutdown_waiters: Vec<oneshot::Sender<()>>,
}

async fn run_scheduler(
    mut rx: mpsc::Receiver<SchedulerMessage>,
    mut release_rx: mpsc::UnboundedReceiver<u64>,
    config: TaskSchedulerConfig,
    closed: Arc<AtomicBool>,
) {
    let mut state = SchedulerState {
        config,
        pending: PriorityQueue::new(),
        active: HashMap::new(),
        closing: false,
        shutdown_waiters: Vec::new(),
    };

    loop {
        let message = tokio::select! {
            biased;
            Some(id) = release_rx.recv() => SchedulerMessage::Release(id),
            Some(message) = rx.recv() => message,
            else => break,
        };
        match message {
            SchedulerMessage::Enqueue(item) => {
                if state.closing {
                    let _ = item.ready.send(Err(TaskSchedulerError::Closed));
                } else {
                    state.pending.push(item.priority.lane_priority(), item);
                    state.dispatch();
                }
            }
            SchedulerMessage::Release(id) => {
                if state.active.remove(&id).is_none() {
                    state.remove_pending(id);
                }
                state.dispatch();
                state.finish_shutdown_if_idle();
            }
            SchedulerMessage::Stats(reply) => {
                let _ = reply.send(state.snapshot());
            }
            SchedulerMessage::Shutdown(reply) => {
                state.closing = true;
                closed.store(true, Ordering::Release);
                while let Some(item) = state.pending.pop() {
                    let item = item.into_value();
                    let _ = item.ready.send(Err(TaskSchedulerError::Closed));
                }
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
    fn remove_pending(&mut self, id: u64) {
        if self.pending.is_empty() {
            return;
        }
        let mut retained = Vec::with_capacity(self.pending.len());
        while let Some(item) = self.pending.pop() {
            if item.value().id == id {
                let item = item.into_value();
                let _ = item.ready.send(Err(TaskSchedulerError::Cancelled));
            } else {
                retained.push(item);
            }
        }
        for item in retained {
            self.pending.restore(item);
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
            let id = item.id;
            let priority = item.priority;
            let label = item.label;
            self.active.insert(id, priority);
            if item.ready.send(Ok(())).is_err() {
                self.active.remove(&id);
                continue;
            }
            tracing::trace!(admission_id = id, ?priority, %label, "task admitted");
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
            self.pending.push(effective, item);
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
    use std::future::{poll_fn, Future};
    use std::pin::Pin;
    use std::task::Poll;
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

    async fn poll_once_pending(mut future: Pin<&mut impl Future>) {
        poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    async fn dropped_acquire_releases_unclaimed_slot() {
        let scheduler = scheduler(1, 60_000);
        let cancellation = CancellationToken::new();
        let mut admission =
            Box::pin(scheduler.acquire(TaskPriority::Interactive, "unclaimed", &cancellation));
        poll_once_pending(admission.as_mut()).await;
        // The actor has sent readiness, but acquire has not consumed it yet.
        assert_eq!(scheduler.stats().await.unwrap().active, 1);
        drop(admission);
        assert_eq!(scheduler.stats().await.unwrap().active, 0);
        tokio::time::timeout(Duration::from_secs(1), scheduler.shutdown())
            .await
            .expect("an abandoned admission must not prevent shutdown");
    }

    #[tokio::test]
    async fn dropped_acquire_removes_pending_without_free_capacity() {
        let scheduler = scheduler(1, 60_000);
        let cancellation = CancellationToken::new();
        let blocker = scheduler
            .acquire(TaskPriority::Interactive, "blocker", &cancellation)
            .await
            .unwrap();
        let mut admission =
            Box::pin(scheduler.acquire(TaskPriority::Background, "abandoned", &cancellation));
        poll_once_pending(admission.as_mut()).await;
        assert_eq!(scheduler.stats().await.unwrap().pending, 1);
        drop(admission);
        let stats = scheduler.stats().await.unwrap();
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.active, 1);
        drop(blocker);
        scheduler.shutdown().await;
    }

    #[tokio::test]
    async fn cancellation_after_readiness_releases_unclaimed_slot() {
        let scheduler = scheduler(1, 60_000);
        let cancellation = CancellationToken::new();
        let mut admission = Box::pin(scheduler.acquire(
            TaskPriority::Interactive,
            "cancelled-after-readiness",
            &cancellation,
        ));
        poll_once_pending(admission.as_mut()).await;
        assert_eq!(scheduler.stats().await.unwrap().active, 1);
        cancellation.cancel();
        assert!(matches!(
            admission.await,
            Err(TaskSchedulerError::Cancelled)
        ));
        assert_eq!(scheduler.stats().await.unwrap().active, 0);
        scheduler.shutdown().await;
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
}
