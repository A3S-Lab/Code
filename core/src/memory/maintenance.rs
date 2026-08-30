//! Explicitly owned maintenance for long-lived memory work.
//!
//! Storage construction is intentionally side-effect free. A host starts this
//! runtime, observes its health, and closes it at the same lifecycle boundary
//! that owns the associated [`AgentMemory`]. Verified semantic index refresh is
//! available as an opt-in built-in schedule; consolidation remains a host policy
//! supplied through [`MemoryMaintenanceJob`].

use super::semantic_refresh::ScheduledSemanticRefreshClaim;
use super::{AgentMemory, ScheduledSemanticRefresh, SEMANTIC_REFRESH_JOB_NAME};
use async_trait::async_trait;
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const PRUNE_JOB_NAME: &str = "v1_prune";
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const MIN_JOB_INTERVAL: Duration = Duration::from_secs(1);
const MAX_JOB_INTERVAL: Duration = Duration::from_secs(365 * 24 * 60 * 60);
const MAX_JOB_NAME_BYTES: usize = 64;
const MAX_OWNER_ID_BYTES: usize = 256;
const MAX_JOBS: usize = 32;
const MAX_ERROR_BYTES: usize = 1_024;

/// One policy-owned memory maintenance operation.
///
/// Implementations may perform verified consolidation, retention projection,
/// or other bounded host work. The storage kernel never constructs one. Runs
/// for the same scheduled job are serialized, and `cancellation` fires when
/// the owning runtime closes.
#[async_trait]
pub trait MemoryMaintenanceJob: Send + Sync {
    async fn run(
        &self,
        context: &MemoryMaintenanceContext,
        cancellation: CancellationToken,
    ) -> anyhow::Result<MemoryMaintenanceOutcome>;
}

/// Exact runtime context passed to a host-owned maintenance job.
#[derive(Clone)]
pub struct MemoryMaintenanceContext {
    owner_id: Arc<str>,
    memory: Arc<AgentMemory>,
}

impl MemoryMaintenanceContext {
    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    pub fn memory(&self) -> &Arc<AgentMemory> {
        &self.memory
    }

    /// Return the exact V2 binding supplied to this memory instance, if any.
    pub fn durable_memory(&self) -> Option<&crate::durable_memory::DurableMemorySession> {
        self.memory.durable_memory()
    }
}

impl std::fmt::Debug for MemoryMaintenanceContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryMaintenanceContext")
            .field("owner_id", &self.owner_id)
            .field("durable_memory", &self.durable_memory())
            .finish_non_exhaustive()
    }
}

/// Machine-readable result of one maintenance run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMaintenanceOutcome {
    pub affected_items: usize,
}

impl MemoryMaintenanceOutcome {
    pub const fn new(affected_items: usize) -> Self {
        Self { affected_items }
    }
}

/// A typed job plus its non-overlapping periodic schedule.
#[derive(Clone)]
#[must_use = "a scheduled job does nothing until installed in maintenance options"]
pub struct ScheduledMemoryMaintenance {
    name: String,
    interval: Duration,
    job: Arc<dyn MemoryMaintenanceJob>,
}

impl ScheduledMemoryMaintenance {
    pub fn try_new(
        name: impl Into<String>,
        interval: Duration,
        job: Arc<dyn MemoryMaintenanceJob>,
    ) -> Result<Self, MemoryMaintenanceError> {
        let name = name.into();
        validate_job_name(&name)?;
        validate_interval(interval)?;
        Ok(Self {
            name,
            interval,
            job,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }
}

impl std::fmt::Debug for ScheduledMemoryMaintenance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScheduledMemoryMaintenance")
            .field("name", &self.name)
            .field("interval", &self.interval)
            .field("job", &"<host-injected>")
            .finish()
    }
}

/// Typed, session-owned memory maintenance schedules and shutdown policy.
#[derive(Clone, Debug)]
#[must_use = "maintenance options do nothing until installed on a session or runtime"]
pub struct MemoryMaintenanceOptions {
    jobs: Vec<ScheduledMemoryMaintenance>,
    semantic_refresh: Option<ScheduledSemanticRefresh>,
    shutdown_timeout: Duration,
}

impl Default for MemoryMaintenanceOptions {
    fn default() -> Self {
        Self {
            jobs: Vec::new(),
            semantic_refresh: None,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }
}

impl MemoryMaintenanceOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_job(mut self, job: ScheduledMemoryMaintenance) -> Self {
        self.jobs.push(job);
        self
    }

    /// Install or replace the single built-in verified semantic refresh
    /// schedule.
    pub fn with_semantic_refresh(mut self, schedule: ScheduledSemanticRefresh) -> Self {
        self.semantic_refresh = Some(schedule);
        self
    }

    pub fn try_with_shutdown_timeout(
        mut self,
        timeout: Duration,
    ) -> Result<Self, MemoryMaintenanceError> {
        validate_shutdown_timeout(timeout)?;
        self.shutdown_timeout = timeout;
        Ok(self)
    }

    pub fn jobs(&self) -> &[ScheduledMemoryMaintenance] {
        &self.jobs
    }

    pub fn semantic_refresh(&self) -> Option<&ScheduledSemanticRefresh> {
        self.semantic_refresh.as_ref()
    }

    pub fn shutdown_timeout(&self) -> Duration {
        self.shutdown_timeout
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.jobs.is_empty() && self.semantic_refresh.is_none()
    }
}

/// Lifecycle state of one owned maintenance runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMaintenancePhase {
    Disabled,
    Running,
    Degraded,
    Closing,
    Closed,
}

/// Current health of one scheduled job.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMaintenanceJobHealth {
    pub name: String,
    pub interval_ms: u64,
    pub worker_alive: bool,
    pub run_in_progress: bool,
    pub successful_runs: u64,
    pub failed_runs: u64,
    pub total_affected_items: u64,
    pub last_affected_items: Option<usize>,
    pub last_error: Option<String>,
}

impl MemoryMaintenanceJobHealth {
    fn new(schedule: &ScheduledMemoryMaintenance) -> Self {
        Self {
            name: schedule.name.clone(),
            interval_ms: duration_ms(schedule.interval),
            worker_alive: true,
            run_in_progress: false,
            successful_runs: 0,
            failed_runs: 0,
            total_affected_items: 0,
            last_affected_items: None,
            last_error: None,
        }
    }
}

/// Non-sensitive snapshot for readiness and operational diagnostics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMaintenanceHealth {
    pub phase: MemoryMaintenancePhase,
    pub jobs: Vec<MemoryMaintenanceJobHealth>,
}

impl MemoryMaintenanceHealth {
    pub fn disabled() -> Self {
        Self {
            phase: MemoryMaintenancePhase::Disabled,
            jobs: Vec::new(),
        }
    }

    pub fn is_healthy(&self) -> bool {
        matches!(
            self.phase,
            MemoryMaintenancePhase::Disabled
                | MemoryMaintenancePhase::Running
                | MemoryMaintenancePhase::Closed
        )
    }
}

/// Bounded shutdown evidence for one maintenance runtime.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMaintenanceCloseReport {
    pub jobs_joined: usize,
    pub jobs_aborted: usize,
    pub join_failures: usize,
}

impl MemoryMaintenanceCloseReport {
    pub fn is_clean(&self) -> bool {
        self.jobs_aborted == 0 && self.join_failures == 0
    }
}

/// Invalid configuration or ownership failure when starting maintenance.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MemoryMaintenanceError {
    #[error("invalid memory maintenance configuration for {field}: {reason}")]
    InvalidConfiguration { field: &'static str, reason: String },
    #[error("memory maintenance requires a Tokio runtime")]
    AsyncRuntimeRequired,
    #[error("this AgentMemory already has an active maintenance owner")]
    AlreadyOwned,
    #[error("this semantic refresh schedule already has an active maintenance owner")]
    SemanticRefreshAlreadyOwned,
    #[error("no memory maintenance jobs are configured")]
    NoJobsConfigured,
}

/// Owner of every periodic task associated with one [`AgentMemory`].
#[must_use = "the owner must be retained and closed to govern its maintenance tasks"]
pub struct MemoryMaintenanceRuntime {
    memory: Arc<AgentMemory>,
    semantic_refresh_claim: Mutex<Option<ScheduledSemanticRefreshClaim>>,
    lifetime: CancellationToken,
    health: Arc<RwLock<MemoryMaintenanceHealth>>,
    tasks: Mutex<Option<Vec<JoinHandle<()>>>>,
    close_gate: tokio::sync::Mutex<()>,
    close_report: Mutex<Option<MemoryMaintenanceCloseReport>>,
    shutdown_timeout: Duration,
    claim_released: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for MemoryMaintenanceRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryMaintenanceRuntime")
            .field("health", &self.health())
            .field("shutdown_timeout", &self.shutdown_timeout)
            .finish_non_exhaustive()
    }
}

impl MemoryMaintenanceRuntime {
    /// Start an explicitly owned runtime. No task is started by
    /// [`AgentMemory`] construction itself.
    pub fn start(
        owner_id: impl Into<String>,
        memory: Arc<AgentMemory>,
        options: MemoryMaintenanceOptions,
    ) -> Result<Arc<Self>, MemoryMaintenanceError> {
        let owner_id = owner_id.into();
        let schedules = Self::validated_schedules(&owner_id, &memory, &options)?;
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| MemoryMaintenanceError::AsyncRuntimeRequired)?;
        if memory
            .maintenance_claimed
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err()
        {
            return Err(MemoryMaintenanceError::AlreadyOwned);
        }
        let semantic_refresh_claim = match &options.semantic_refresh {
            Some(schedule) => match schedule.try_claim() {
                Ok(claim) => Some(claim),
                Err(error) => {
                    memory
                        .maintenance_claimed
                        .store(false, std::sync::atomic::Ordering::Release);
                    return Err(error);
                }
            },
            None => None,
        };

        let context = MemoryMaintenanceContext {
            owner_id: Arc::from(owner_id),
            memory: Arc::clone(&memory),
        };
        let health = Arc::new(RwLock::new(MemoryMaintenanceHealth {
            phase: MemoryMaintenancePhase::Running,
            jobs: schedules
                .iter()
                .map(MemoryMaintenanceJobHealth::new)
                .collect(),
        }));
        let lifetime = CancellationToken::new();
        let worker_semantic_refresh_claim = semantic_refresh_claim.clone();
        let runtime = Arc::new(Self {
            memory,
            semantic_refresh_claim: Mutex::new(semantic_refresh_claim),
            lifetime: lifetime.clone(),
            health: Arc::clone(&health),
            tasks: Mutex::new(None),
            close_gate: tokio::sync::Mutex::new(()),
            close_report: Mutex::new(None),
            shutdown_timeout: options.shutdown_timeout,
            claim_released: std::sync::atomic::AtomicBool::new(false),
        });
        let tasks = schedules
            .into_iter()
            .enumerate()
            .map(|(index, schedule)| {
                let context = context.clone();
                let health = Arc::clone(&health);
                let cancellation = lifetime.child_token();
                let semantic_refresh_claim = worker_semantic_refresh_claim.clone();
                handle.spawn(async move {
                    let observe_cancellation = cancellation.clone();
                    let result = AssertUnwindSafe(run_schedule(
                        schedule,
                        index,
                        context,
                        Arc::clone(&health),
                        cancellation,
                    ))
                    .catch_unwind()
                    .await;
                    let mut snapshot = write_unpoisoned(&health);
                    let job = &mut snapshot.jobs[index];
                    job.run_in_progress = false;
                    job.worker_alive = false;
                    if result.is_err() && !observe_cancellation.is_cancelled() {
                        job.failed_runs = job.failed_runs.saturating_add(1);
                        job.last_error = Some("maintenance worker panicked".to_string());
                    }
                    drop(semantic_refresh_claim);
                })
            })
            .collect();
        *lock_unpoisoned(&runtime.tasks) = Some(tasks);
        Ok(runtime)
    }

    pub(crate) fn validate_configuration(
        owner_id: &str,
        memory: &AgentMemory,
        options: &MemoryMaintenanceOptions,
    ) -> Result<bool, MemoryMaintenanceError> {
        match Self::validated_schedules(owner_id, memory, options) {
            Ok(_) => Ok(true),
            Err(MemoryMaintenanceError::NoJobsConfigured) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn validated_schedules(
        owner_id: &str,
        memory: &AgentMemory,
        options: &MemoryMaintenanceOptions,
    ) -> Result<Vec<ScheduledMemoryMaintenance>, MemoryMaintenanceError> {
        validate_owner_id(owner_id)?;
        validate_shutdown_timeout(options.shutdown_timeout)?;
        let mut schedules = Vec::new();
        if let Some((policy, interval)) = memory.maintenance_prune_schedule() {
            schedules.push(ScheduledMemoryMaintenance::try_new(
                PRUNE_JOB_NAME,
                interval,
                Arc::new(PruneMemoryJob {
                    store: Arc::clone(memory.store()),
                    policy,
                }),
            )?);
        }
        if let Some(semantic_refresh) = &options.semantic_refresh {
            semantic_refresh.validate_for(memory)?;
            schedules.push(semantic_refresh.as_maintenance()?);
        }
        for schedule in &options.jobs {
            if matches!(
                schedule.name.as_str(),
                PRUNE_JOB_NAME | SEMANTIC_REFRESH_JOB_NAME
            ) {
                return Err(invalid(
                    "jobs.name",
                    format!("'{}' is reserved for built-in maintenance", schedule.name),
                ));
            }
            schedules.push(schedule.clone());
        }
        if schedules.is_empty() {
            return Err(MemoryMaintenanceError::NoJobsConfigured);
        }
        if schedules.len() > MAX_JOBS {
            return Err(invalid(
                "jobs",
                format!("must not contain more than {MAX_JOBS} jobs"),
            ));
        }
        let mut names = HashSet::with_capacity(schedules.len());
        for schedule in &schedules {
            validate_job_name(&schedule.name)?;
            validate_interval(schedule.interval)?;
            if !names.insert(schedule.name.clone()) {
                return Err(invalid(
                    "jobs.name",
                    format!("duplicate job name '{}'", schedule.name),
                ));
            }
        }
        Ok(schedules)
    }

    pub fn health(&self) -> MemoryMaintenanceHealth {
        let mut health = read_unpoisoned(&self.health).clone();
        if health.phase == MemoryMaintenancePhase::Running
            && health
                .jobs
                .iter()
                .any(|job| !job.worker_alive || job.last_error.is_some())
        {
            health.phase = MemoryMaintenancePhase::Degraded;
        }
        health
    }

    /// Cancel all jobs, join them within one total deadline, and abort any
    /// worker that exceeds it. Repeated calls return the first close report.
    pub async fn close(&self) -> MemoryMaintenanceCloseReport {
        let _close = self.close_gate.lock().await;
        if let Some(report) = lock_unpoisoned(&self.close_report).clone() {
            return report;
        }
        write_unpoisoned(&self.health).phase = MemoryMaintenancePhase::Closing;
        self.lifetime.cancel();
        let tasks = lock_unpoisoned(&self.tasks).take().unwrap_or_default();
        let deadline = tokio::time::Instant::now() + self.shutdown_timeout;
        let mut report = MemoryMaintenanceCloseReport::default();
        for mut task in tasks {
            match tokio::time::timeout_at(deadline, &mut task).await {
                Ok(Ok(())) => report.jobs_joined += 1,
                Ok(Err(_)) => report.join_failures += 1,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    report.jobs_aborted += 1;
                }
            }
        }
        {
            let mut health = write_unpoisoned(&self.health);
            health.phase = MemoryMaintenancePhase::Closed;
            for job in &mut health.jobs {
                job.worker_alive = false;
                job.run_in_progress = false;
            }
        }
        self.release_claim();
        *lock_unpoisoned(&self.close_report) = Some(report.clone());
        report
    }

    fn release_claim(&self) {
        if self
            .claim_released
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            self.memory
                .maintenance_claimed
                .store(false, std::sync::atomic::Ordering::Release);
            drop(lock_unpoisoned(&self.semantic_refresh_claim).take());
        }
    }
}

impl Drop for MemoryMaintenanceRuntime {
    fn drop(&mut self) {
        self.lifetime.cancel();
        if let Some(tasks) = lock_unpoisoned(&self.tasks).take() {
            for task in tasks {
                task.abort();
            }
        }
        self.release_claim();
        let mut health = write_unpoisoned(&self.health);
        health.phase = MemoryMaintenancePhase::Closed;
        for job in &mut health.jobs {
            job.worker_alive = false;
            job.run_in_progress = false;
        }
    }
}

struct PruneMemoryJob {
    store: Arc<dyn a3s_memory::MemoryStore>,
    policy: a3s_memory::PrunePolicy,
}

#[async_trait]
impl MemoryMaintenanceJob for PruneMemoryJob {
    async fn run(
        &self,
        _context: &MemoryMaintenanceContext,
        _cancellation: CancellationToken,
    ) -> anyhow::Result<MemoryMaintenanceOutcome> {
        self.store
            .prune(&self.policy)
            .await
            .map(MemoryMaintenanceOutcome::new)
    }
}

async fn run_schedule(
    schedule: ScheduledMemoryMaintenance,
    health_index: usize,
    context: MemoryMaintenanceContext,
    health: Arc<RwLock<MemoryMaintenanceHealth>>,
    cancellation: CancellationToken,
) {
    let first_tick = tokio::time::Instant::now() + schedule.interval;
    let mut ticker = tokio::time::interval_at(first_tick, schedule.interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => break,
            _ = ticker.tick() => {}
        }
        {
            let mut snapshot = write_unpoisoned(&health);
            snapshot.jobs[health_index].run_in_progress = true;
        }
        let run_cancellation = cancellation.child_token();
        let run = schedule.job.run(&context, run_cancellation.clone());
        tokio::pin!(run);
        let (result, close_after_run) = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                run_cancellation.cancel();
                (run.await, true)
            }
            result = &mut run => (result, false),
        };
        let mut snapshot = write_unpoisoned(&health);
        let job = &mut snapshot.jobs[health_index];
        job.run_in_progress = false;
        match result {
            Ok(outcome) => {
                job.successful_runs = job.successful_runs.saturating_add(1);
                job.total_affected_items = job
                    .total_affected_items
                    .saturating_add(u64::try_from(outcome.affected_items).unwrap_or(u64::MAX));
                job.last_affected_items = Some(outcome.affected_items);
                job.last_error = None;
            }
            Err(error) => {
                let message = bounded_error(error.to_string());
                job.failed_runs = job.failed_runs.saturating_add(1);
                job.last_error = Some(message.clone());
                tracing::warn!(
                    owner_id = %context.owner_id,
                    job = %schedule.name,
                    error = %message,
                    "Memory maintenance job failed"
                );
            }
        }
        drop(snapshot);
        if close_after_run {
            break;
        }
    }
}

fn validate_job_name(name: &str) -> Result<(), MemoryMaintenanceError> {
    if name.is_empty() || name.trim() != name {
        return Err(invalid(
            "job.name",
            "must not be empty or contain surrounding whitespace",
        ));
    }
    if name.len() > MAX_JOB_NAME_BYTES {
        return Err(invalid(
            "job.name",
            format!("must not exceed {MAX_JOB_NAME_BYTES} bytes"),
        ));
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
    {
        return Err(invalid(
            "job.name",
            "must contain only ASCII letters, digits, '.', '_' or '-'",
        ));
    }
    Ok(())
}

pub(super) fn validate_interval(interval: Duration) -> Result<(), MemoryMaintenanceError> {
    if interval < MIN_JOB_INTERVAL || interval > MAX_JOB_INTERVAL {
        return Err(invalid(
            "job.interval",
            "must be between one second and 365 days",
        ));
    }
    Ok(())
}

fn validate_shutdown_timeout(timeout: Duration) -> Result<(), MemoryMaintenanceError> {
    if timeout.is_zero() || timeout > MAX_SHUTDOWN_TIMEOUT {
        return Err(invalid(
            "shutdownTimeout",
            "must be greater than zero and no longer than 30 seconds",
        ));
    }
    Ok(())
}

fn validate_owner_id(owner_id: &str) -> Result<(), MemoryMaintenanceError> {
    if owner_id.trim().is_empty() {
        return Err(invalid("ownerId", "must not be empty or whitespace"));
    }
    if owner_id.len() > MAX_OWNER_ID_BYTES {
        return Err(invalid(
            "ownerId",
            format!("must not exceed {MAX_OWNER_ID_BYTES} bytes"),
        ));
    }
    Ok(())
}

fn invalid(field: &'static str, reason: impl Into<String>) -> MemoryMaintenanceError {
    MemoryMaintenanceError::InvalidConfiguration {
        field,
        reason: reason.into(),
    }
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn bounded_error(mut message: String) -> String {
    if message.len() <= MAX_ERROR_BYTES {
        return message;
    }
    let mut boundary = MAX_ERROR_BYTES;
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message
}

fn lock_unpoisoned<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
