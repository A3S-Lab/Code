//! Agent-wide admission scheduler for top-level and background work.
//!
//! Session transcript admission remains single-flight. This scheduler adds a
//! shared capacity boundary across every session created by one [`Agent`](crate::Agent),
//! using `a3s-lane`'s stable priority queue for exact priority/FIFO ordering.

use crate::execution_identity::ExecutionIdentityV1;
use a3s_lane::{Priority, PriorityItem, PriorityQueue};
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
/// Maximum bytes accepted while deriving a scheduler owner scope.
///
/// The raw scope is never sent to the scheduler actor or included in a
/// snapshot; the bound only prevents an untrusted host from forcing an
/// unbounded identity-derivation allocation.
pub const TASK_SCHEDULER_MAX_SCOPE_BYTES: usize = 512;

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

/// Immutable owner quota carried by one scheduler admission request.
///
/// The quota is deliberately a descriptor rather than a second queue or
/// semaphore. The scheduler actor remains the only authority that decides
/// whether work owns a global slot; it additionally refuses to admit more than
/// `max_active` requests for this digest-only owner identity at once.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskSchedulerQuota {
    /// Digest-only identity of the run/host scope consuming capacity.
    pub identity: ExecutionIdentityV1,
    /// Maximum global scheduler slots this owner may hold concurrently.
    pub max_active: usize,
}

impl TaskSchedulerQuota {
    /// Build and validate an owner quota descriptor.
    pub fn new(
        identity: ExecutionIdentityV1,
        max_active: usize,
    ) -> Result<Self, TaskSchedulerError> {
        let quota = Self {
            identity,
            max_active,
        };
        quota.validate()?;
        Ok(quota)
    }

    /// Validate a quota received from a host or a deserialized boundary.
    pub fn validate(&self) -> Result<(), TaskSchedulerError> {
        self.identity.validate().map_err(|error| {
            TaskSchedulerError::InvalidConfig(format!(
                "scheduler quota identity is invalid: {error}"
            ))
        })?;
        if self.max_active == 0 {
            return Err(TaskSchedulerError::InvalidConfig(
                "scheduler quota maxActive must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }

    /// Derive a digest-only quota identity from a bounded host/run scope.
    ///
    /// The scope is used only during derivation and is never retained or
    /// emitted by scheduler diagnostics. Callers should use a stable run or
    /// host identifier, not a prompt or tool payload.
    pub fn for_scope(scope: &str, max_active: usize) -> Result<Self, TaskSchedulerError> {
        if scope.is_empty()
            || scope.len() > TASK_SCHEDULER_MAX_SCOPE_BYTES
            || scope.chars().any(|character| {
                character.is_control() || matches!(character, '\u{2028}' | '\u{2029}')
            })
        {
            return Err(TaskSchedulerError::InvalidConfig(
                format!(
                    "scheduler quota scope must be one non-empty line of at most {TASK_SCHEDULER_MAX_SCOPE_BYTES} bytes"
                ),
            ));
        }
        let identity = ExecutionIdentityV1::derive(
            crate::execution_identity::TASK_ADMISSION_SCOPE_IDENTITY_DOMAIN_V1,
            &serde_json::json!({ "scope": scope }),
        )
        .map_err(|error| {
            TaskSchedulerError::InvalidConfig(format!("derive scheduler quota identity: {error}"))
        })?;
        Self::new(identity, max_active)
    }

    /// Return the immutable owner identity.
    pub fn identity(&self) -> &ExecutionIdentityV1 {
        &self.identity
    }
}

/// Live, digest-only occupancy projection for one scheduler quota.
///
/// Counters are intentionally point-in-time. Idle owner state is discarded by
/// the scheduler actor, so an unbounded history of ephemeral run identities
/// cannot accumulate in the process. Global cumulative admission/fairness
/// counters remain available through [`TaskScheduler::health`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskSchedulerQuotaSnapshot {
    /// Digest-only owner identity requested by the caller.
    pub identity: ExecutionIdentityV1,
    /// Immutable owner limit used for this live projection.
    pub max_active: usize,
    /// Global scheduler slots currently owned by this quota identity.
    pub active: usize,
    /// Requests from this owner waiting in the global queue.
    pub pending: usize,
    /// Whether pending work is currently blocked by the owner quota.
    pub blocked: bool,
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
        self.acquire_inner(priority, label.into(), None, identity, cancellation)
            .await
    }

    /// Wait until this task owns a global execution slot subject to an
    /// owner-level quota. The quota reservation is made in the same scheduler
    /// actor as global admission, so a caller cannot bypass it by creating a
    /// fresh local semaphore or executor handle.
    pub async fn acquire_with_quota(
        &self,
        priority: TaskPriority,
        label: impl Into<String>,
        quota: &TaskSchedulerQuota,
        identity: Option<ExecutionIdentityV1>,
        cancellation: &CancellationToken,
    ) -> Result<TaskLease, TaskSchedulerError> {
        self.acquire_inner(
            priority,
            label.into(),
            Some(quota.clone()),
            identity,
            cancellation,
        )
        .await
    }

    async fn acquire_inner(
        &self,
        priority: TaskPriority,
        label: String,
        quota: Option<TaskSchedulerQuota>,
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
        if let Some(quota) = &quota {
            quota.validate()?;
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let quota_identity = quota.as_ref().map(|quota| quota.identity.clone());
        let (ready_tx, ready_rx) = oneshot::channel();
        self.tx
            .send(SchedulerMessage::Enqueue(QueuedAdmission {
                id,
                priority,
                effective_priority: priority.lane_priority(),
                label,
                identity: identity.clone(),
                quota,
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
                    quota_identity,
                })
            }
        }
    }

    /// Return the live occupancy projection for one owner quota.
    pub async fn quota_snapshot(
        &self,
        quota: &TaskSchedulerQuota,
    ) -> Result<TaskSchedulerQuotaSnapshot, TaskSchedulerError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TaskSchedulerError::Closed);
        }
        quota.validate()?;
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(SchedulerMessage::QuotaStats {
                quota: quota.clone(),
                reply: tx,
            })
            .map_err(|_| TaskSchedulerError::Closed)?;
        rx.await.map_err(|_| TaskSchedulerError::Closed)?
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
    quota_identity: Option<ExecutionIdentityV1>,
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

    /// Digest-only owner quota identity applied to this admission, when any.
    pub fn quota_identity(&self) -> Option<&ExecutionIdentityV1> {
        self.quota_identity.as_ref()
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
    quota: Option<TaskSchedulerQuota>,
    enqueued_at: Instant,
    ready: oneshot::Sender<Result<(), TaskSchedulerError>>,
}

enum SchedulerMessage {
    Enqueue(QueuedAdmission),
    Cancel(u64),
    Release(u64),
    Stats(oneshot::Sender<TaskSchedulerStats>),
    Health(oneshot::Sender<TaskSchedulerHealthSnapshot>),
    QuotaStats {
        quota: TaskSchedulerQuota,
        reply: oneshot::Sender<Result<TaskSchedulerQuotaSnapshot, TaskSchedulerError>>,
    },
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
    active: HashMap<u64, ActiveAdmission>,
    quotas: HashMap<String, QuotaState>,
    closing: bool,
    shutdown_waiters: Vec<oneshot::Sender<()>>,
    counters: SchedulerCounters,
}

struct ActiveAdmission {
    priority: TaskPriority,
    quota_identity: Option<String>,
}

struct QuotaState {
    identity: ExecutionIdentityV1,
    max_active: usize,
    active: usize,
    pending: usize,
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
        quotas: HashMap::new(),
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
                } else if let Err(error) = state.register_pending_quota(item.quota.as_ref()) {
                    state.counters.rejected = state.counters.rejected.saturating_add(1);
                    let _ = item.ready.send(Err(error));
                } else {
                    if let Some(quota) = item.quota.as_ref() {
                        if let Some(quota_state) = state.quotas.get_mut(&quota.identity.digest) {
                            quota_state.pending = quota_state.pending.saturating_add(1);
                        }
                    }
                    state.pending.push(item.effective_priority, item);
                    state.dispatch();
                }
            }
            SchedulerMessage::Cancel(id) => {
                if let Some(active) = state.active.remove(&id) {
                    state.counters.cancelled = state.counters.cancelled.saturating_add(1);
                    state.release_active_quota(active.quota_identity.as_deref());
                } else {
                    state.cancelled.insert(id);
                    state.purge_cancelled();
                }
                state.dispatch();
                state.finish_shutdown_if_idle();
            }
            SchedulerMessage::Release(id) => {
                if let Some(active) = state.active.remove(&id) {
                    state.counters.released = state.counters.released.saturating_add(1);
                    state.release_active_quota(active.quota_identity.as_deref());
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
            SchedulerMessage::QuotaStats { quota, reply } => {
                let result = state.quota_snapshot(&quota);
                let _ = reply.send(result);
            }
            SchedulerMessage::Shutdown(reply) => {
                state.closing = true;
                closed.store(true, Ordering::Release);
                while let Some(item) = state.pending.pop() {
                    let item = item.into_value();
                    state.counters.rejected = state.counters.rejected.saturating_add(1);
                    state.reject_pending_quota(item.quota.as_ref());
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
    fn register_pending_quota(
        &mut self,
        quota: Option<&TaskSchedulerQuota>,
    ) -> Result<(), TaskSchedulerError> {
        let Some(quota) = quota else {
            return Ok(());
        };
        let key = quota.identity.digest.clone();
        match self.quotas.get(&key) {
            Some(existing)
                if existing.identity != quota.identity
                    || existing.max_active != quota.max_active =>
            {
                Err(TaskSchedulerError::InvalidConfig(
                    "scheduler quota identity is already registered with a different limit"
                        .to_string(),
                ))
            }
            Some(_) => Ok(()),
            None => {
                self.quotas.insert(
                    key,
                    QuotaState {
                        identity: quota.identity.clone(),
                        max_active: quota.max_active,
                        active: 0,
                        pending: 0,
                    },
                );
                Ok(())
            }
        }
    }

    fn reject_pending_quota(&mut self, quota: Option<&TaskSchedulerQuota>) {
        let Some(quota) = quota else {
            return;
        };
        let key = quota.identity.digest.as_str();
        if let Some(state) = self.quotas.get_mut(key) {
            state.pending = state.pending.saturating_sub(1);
        }
        self.prune_idle_quota(key);
    }

    fn cancel_pending(&mut self, item: QueuedAdmission) {
        self.counters.cancelled = self.counters.cancelled.saturating_add(1);
        self.reject_pending_quota(item.quota.as_ref());
        let _ = item.ready.send(Err(TaskSchedulerError::Cancelled));
    }

    fn release_active_quota(&mut self, key: Option<&str>) {
        let Some(key) = key else {
            return;
        };
        if let Some(state) = self.quotas.get_mut(key) {
            state.active = state.active.saturating_sub(1);
        }
        self.prune_idle_quota(key);
    }

    fn prune_idle_quota(&mut self, key: &str) {
        let remove = self
            .quotas
            .get(key)
            .is_some_and(|state| state.active == 0 && state.pending == 0);
        if remove {
            self.quotas.remove(key);
        }
    }

    fn quota_allows(&self, quota: Option<&TaskSchedulerQuota>) -> bool {
        let Some(quota) = quota else {
            return true;
        };
        self.quotas
            .get(&quota.identity.digest)
            .is_some_and(|state| state.active < state.max_active)
    }

    fn quota_snapshot(
        &self,
        quota: &TaskSchedulerQuota,
    ) -> Result<TaskSchedulerQuotaSnapshot, TaskSchedulerError> {
        quota.validate()?;
        if let Some(state) = self.quotas.get(&quota.identity.digest) {
            if state.identity != quota.identity || state.max_active != quota.max_active {
                return Err(TaskSchedulerError::InvalidConfig(
                    "scheduler quota identity is already registered with a different limit"
                        .to_string(),
                ));
            }
            return Ok(TaskSchedulerQuotaSnapshot {
                identity: state.identity.clone(),
                max_active: state.max_active,
                active: state.active,
                pending: state.pending,
                blocked: state.pending > 0 && state.active >= state.max_active,
            });
        }
        Ok(TaskSchedulerQuotaSnapshot {
            identity: quota.identity.clone(),
            max_active: quota.max_active,
            active: 0,
            pending: 0,
            blocked: false,
        })
    }

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
                self.cancel_pending(item);
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
            let Some(item) = self.pop_admissible() else {
                break;
            };
            let id = item.id;
            let priority = item.priority;
            let label = item.label;
            let identity = item.identity;
            let quota_identity = item
                .quota
                .as_ref()
                .map(|quota| quota.identity.digest.clone());
            if let Some(quota_key) = quota_identity.as_deref() {
                if let Some(quota_state) = self.quotas.get_mut(quota_key) {
                    quota_state.pending = quota_state.pending.saturating_sub(1);
                    quota_state.active = quota_state.active.saturating_add(1);
                }
            }
            let wait_micros = item
                .enqueued_at
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            self.active.insert(
                id,
                ActiveAdmission {
                    priority,
                    quota_identity: quota_identity.clone(),
                },
            );
            if item.ready.send(Ok(())).is_err() {
                self.active.remove(&id);
                self.counters.cancelled = self.counters.cancelled.saturating_add(1);
                self.release_active_quota(quota_identity.as_deref());
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

    /// Claim the first queued item that is eligible under both global capacity
    /// and its owner quota. Items blocked by one owner remain queued while
    /// independent owners can make progress, preventing a single fan-out from
    /// monopolizing the shared scheduler.
    fn pop_admissible(&mut self) -> Option<QueuedAdmission> {
        let mut retained: Vec<PriorityItem<QueuedAdmission>> = Vec::new();
        let mut selected = None;
        while let Some(item) = self.pending.pop() {
            if self.cancelled.remove(&item.value().id) {
                self.cancel_pending(item.into_value());
                continue;
            }
            if selected.is_none() && self.quota_allows(item.value().quota.as_ref()) {
                selected = Some(item.into_value());
            } else {
                retained.push(item);
            }
        }
        for item in retained {
            self.pending.restore(item);
        }
        selected
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
        for active in self.active.values() {
            active_by_priority.increment(active.priority);
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
mod tests;
