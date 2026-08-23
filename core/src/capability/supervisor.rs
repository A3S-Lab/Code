use std::collections::BTreeMap;
use std::future::Future;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio::task::{JoinError, JoinHandle, JoinSet};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{CapabilityEffect, CapabilityEffectError, CapabilityScopeError, RetainedUseGeneration};

pub const MAX_SCOPE_EFFECTS: usize = 1_024;
pub const MAX_SCOPE_TASKS: usize = 1_024;
pub const MAX_SCOPE_CHILDREN: usize = 1_024;
pub const DEFAULT_SCOPE_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_SCOPE_CLOSE_TIMEOUT: Duration = Duration::from_secs(60);

const MAX_LIFECYCLE_NAME_BYTES: usize = 128;
const ABORT_SETTLE_GRACE: Duration = Duration::from_millis(100);

/// Bounded close policy inherited by every child scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopeClosePolicy {
    timeout: Duration,
}

impl ScopeClosePolicy {
    pub fn new(timeout: Duration) -> Result<Self, CapabilityScopeError> {
        if timeout.is_zero() {
            return Err(CapabilityScopeError::InvalidExecutionLimit {
                field: "scope_close_timeout",
            });
        }
        if timeout > MAX_SCOPE_CLOSE_TIMEOUT {
            return Err(CapabilityScopeError::BoundExceeded {
                field: "scope_close_timeout_ms",
                max: MAX_SCOPE_CLOSE_TIMEOUT.as_millis() as usize,
            });
        }
        Ok(Self { timeout })
    }

    pub const fn timeout(self) -> Duration {
        self.timeout
    }
}

impl Default for ScopeClosePolicy {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_SCOPE_CLOSE_TIMEOUT,
        }
    }
}

/// Opaque identity of one task owned by a capability scope supervisor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SupervisedTaskId(NonZeroU64);

impl SupervisedTaskId {
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Deterministic, bounded outcome of one idempotent scope close.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScopeCloseReport {
    pub tasks_completed: usize,
    pub tasks_failed: usize,
    pub tasks_cancelled: usize,
    pub tasks_timed_out: usize,
    pub child_scopes_closed: usize,
    pub child_scopes_failed: usize,
    pub child_scopes_timed_out: usize,
    pub effects_closed: usize,
    pub effects_failed: usize,
    pub effects_timed_out: usize,
    pub generation_leases_released: usize,
}

impl ScopeCloseReport {
    pub const fn is_clean(&self) -> bool {
        self.tasks_failed == 0
            && self.tasks_timed_out == 0
            && self.child_scopes_failed == 0
            && self.child_scopes_timed_out == 0
            && self.effects_failed == 0
            && self.effects_timed_out == 0
    }
}

struct SupervisedTaskOutcome {
    name: Box<str>,
    result: Result<(), CapabilityEffectError>,
}

struct RegisteredEffect {
    name: Box<str>,
    effect: Box<dyn CapabilityEffect>,
}

#[async_trait]
pub(super) trait SupervisedChild: Send {
    fn name(&self) -> &str;

    fn cancel(&self);

    async fn close(self: Box<Self>) -> Result<ScopeCloseReport, CapabilityScopeError>;
}

struct OpenSupervisor {
    tasks: JoinSet<SupervisedTaskOutcome>,
    children: BTreeMap<u64, Box<dyn SupervisedChild>>,
    effects: Vec<RegisteredEffect>,
    generation_leases: Vec<Box<dyn RetainedUseGeneration>>,
    next_task_id: u64,
    next_child_id: u64,
}

impl Default for OpenSupervisor {
    fn default() -> Self {
        Self {
            tasks: JoinSet::new(),
            children: BTreeMap::new(),
            effects: Vec::new(),
            generation_leases: Vec::new(),
            next_task_id: 1,
            next_child_id: 1,
        }
    }
}

enum SupervisorState {
    Open(OpenSupervisor),
    Closing { driver: Option<JoinHandle<()>> },
    Closed(ScopeCloseReport),
}

pub(super) struct SupervisorInner {
    scope_id: Box<str>,
    cancellation: CancellationToken,
    policy: ScopeClosePolicy,
    state: Mutex<SupervisorState>,
    closed: Notify,
}

impl SupervisorInner {
    fn lock_state(&self) -> MutexGuard<'_, SupervisorState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for SupervisorInner {
    fn drop(&mut self) {
        self.cancellation.cancel();
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let SupervisorState::Open(open) = state {
            open.tasks.abort_all();
        }
    }
}

/// Owns every asynchronous task, child scope, reversible effect, and exact
/// upstream generation lease for one capability scope.
pub(super) struct EffectSupervisor {
    inner: Arc<SupervisorInner>,
}

impl EffectSupervisor {
    pub(super) fn new(
        scope_id: impl Into<String>,
        cancellation: CancellationToken,
        policy: ScopeClosePolicy,
    ) -> Self {
        Self {
            inner: Arc::new(SupervisorInner {
                scope_id: scope_id.into().into_boxed_str(),
                cancellation,
                policy,
                state: Mutex::new(SupervisorState::Open(OpenSupervisor::default())),
                closed: Notify::new(),
            }),
        }
    }

    pub(super) fn policy(&self) -> ScopeClosePolicy {
        self.inner.policy
    }

    pub(super) fn cancellation(&self) -> CancellationToken {
        self.inner.cancellation.clone()
    }

    pub(super) fn is_open(&self) -> bool {
        matches!(*self.inner.lock_state(), SupervisorState::Open(_))
            && !self.inner.cancellation.is_cancelled()
    }

    pub(super) fn register_effect(
        &self,
        effect: Box<dyn CapabilityEffect>,
    ) -> Result<(), CapabilityScopeError> {
        let name = effect.name().to_owned();
        validate_lifecycle_name(&name)?;
        let mut state = self.inner.lock_state();
        let SupervisorState::Open(open) = &mut *state else {
            return Err(self.closed_error());
        };
        if self.inner.cancellation.is_cancelled() {
            return Err(self.closed_error());
        }
        if open.effects.len() >= MAX_SCOPE_EFFECTS {
            return Err(CapabilityScopeError::BoundExceeded {
                field: "scope_effects",
                max: MAX_SCOPE_EFFECTS,
            });
        }
        open.effects.push(RegisteredEffect {
            name: name.into_boxed_str(),
            effect,
        });
        Ok(())
    }

    pub(super) fn register_generation_lease(
        &self,
        lease: Box<dyn RetainedUseGeneration>,
    ) -> Result<(), CapabilityScopeError> {
        let mut state = self.inner.lock_state();
        let SupervisorState::Open(open) = &mut *state else {
            return Err(self.closed_error());
        };
        if self.inner.cancellation.is_cancelled() {
            return Err(self.closed_error());
        }
        if !open.generation_leases.is_empty() {
            return Err(CapabilityScopeError::BoundExceeded {
                field: "use_generation_leases",
                max: 1,
            });
        }
        open.generation_leases.push(lease);
        Ok(())
    }

    pub(super) fn spawn_task<F>(
        &self,
        name: impl Into<String>,
        task: F,
    ) -> Result<SupervisedTaskId, CapabilityScopeError>
    where
        F: Future<Output = Result<(), CapabilityEffectError>> + Send + 'static,
    {
        let name = name.into();
        validate_lifecycle_name(&name)?;
        tokio::runtime::Handle::try_current()
            .map_err(|_| CapabilityScopeError::TokioRuntimeUnavailable)?;

        let mut state = self.inner.lock_state();
        let SupervisorState::Open(open) = &mut *state else {
            return Err(self.closed_error());
        };
        if self.inner.cancellation.is_cancelled() {
            return Err(self.closed_error());
        }
        if open.tasks.len() >= MAX_SCOPE_TASKS {
            return Err(CapabilityScopeError::BoundExceeded {
                field: "scope_tasks",
                max: MAX_SCOPE_TASKS,
            });
        }
        let raw_id = open.next_task_id;
        let id = NonZeroU64::new(raw_id).ok_or(CapabilityScopeError::TaskIdentityExhausted)?;
        open.next_task_id = raw_id
            .checked_add(1)
            .ok_or(CapabilityScopeError::TaskIdentityExhausted)?;
        let task_name = name.into_boxed_str();
        let _abort_handle = open.tasks.spawn(async move {
            SupervisedTaskOutcome {
                name: task_name,
                result: task.await,
            }
        });
        Ok(SupervisedTaskId(id))
    }

    pub(super) fn register_child(
        &self,
        child: Box<dyn SupervisedChild>,
    ) -> Result<u64, CapabilityScopeError> {
        let mut state = self.inner.lock_state();
        let SupervisorState::Open(open) = &mut *state else {
            return Err(self.closed_error());
        };
        if self.inner.cancellation.is_cancelled() {
            return Err(self.closed_error());
        }
        if open
            .children
            .values()
            .any(|existing| existing.name() == child.name())
        {
            return Err(CapabilityScopeError::DuplicateChildScope {
                scope_id: child.name().to_owned(),
            });
        }
        if open.children.len() >= MAX_SCOPE_CHILDREN {
            return Err(CapabilityScopeError::BoundExceeded {
                field: "scope_children",
                max: MAX_SCOPE_CHILDREN,
            });
        }
        let id = open.next_child_id;
        open.next_child_id = id
            .checked_add(1)
            .ok_or(CapabilityScopeError::ChildIdentityExhausted)?;
        open.children.insert(id, child);
        Ok(id)
    }

    pub(super) fn downgrade(&self) -> Weak<SupervisorInner> {
        Arc::downgrade(&self.inner)
    }

    pub(super) fn cancel(&self) {
        self.inner.cancellation.cancel();
        let mut state = self.inner.lock_state();
        match &mut *state {
            SupervisorState::Open(open) => {
                open.tasks.abort_all();
                for child in open.children.values() {
                    child.cancel();
                }
            }
            SupervisorState::Closing { driver } => {
                let _close_driver_started = driver.is_some();
            }
            SupervisorState::Closed(_) => {}
        }
    }

    pub(super) async fn close(&self) -> Result<ScopeCloseReport, CapabilityScopeError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| CapabilityScopeError::TokioRuntimeUnavailable)?;

        loop {
            let notified = self.inner.closed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let start = {
                let mut state = self.inner.lock_state();
                match &*state {
                    SupervisorState::Closed(report) => return Ok(report.clone()),
                    SupervisorState::Closing { driver } => {
                        let _close_driver_started = driver.is_some();
                        None
                    }
                    SupervisorState::Open(_) => {
                        let previous = std::mem::replace(
                            &mut *state,
                            SupervisorState::Closing { driver: None },
                        );
                        match previous {
                            SupervisorState::Open(open) => Some(open),
                            SupervisorState::Closing { .. } | SupervisorState::Closed(_) => None,
                        }
                    }
                }
            };

            if let Some(open) = start {
                let inner = Arc::clone(&self.inner);
                let driver = runtime.spawn(async move {
                    drive_close(inner, open).await;
                });
                let mut state = self.inner.lock_state();
                if let SupervisorState::Closing { driver: slot } = &mut *state {
                    *slot = Some(driver);
                }
            }

            notified.await;
        }
    }

    fn closed_error(&self) -> CapabilityScopeError {
        CapabilityScopeError::SupervisorClosed {
            scope_id: self.inner.scope_id.to_string(),
        }
    }
}

impl Drop for EffectSupervisor {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(super) fn remove_registered_child(parent: &Weak<SupervisorInner>, id: u64) {
    let Some(parent) = parent.upgrade() else {
        return;
    };
    let mut state = parent.lock_state();
    if let SupervisorState::Open(open) = &mut *state {
        open.children.remove(&id);
    }
}

async fn drive_close(inner: Arc<SupervisorInner>, mut open: OpenSupervisor) {
    inner.cancellation.cancel();
    let deadline = Instant::now() + inner.policy.timeout();
    let mut report = ScopeCloseReport::default();

    settle_tasks(&mut open.tasks, deadline, &mut report).await;
    close_children(open.children, deadline, &mut report).await;
    close_effects(open.effects, deadline, &mut report).await;
    while let Some(lease) = open.generation_leases.pop() {
        drop(lease);
        report.generation_leases_released += 1;
    }

    {
        let mut state = inner.lock_state();
        *state = SupervisorState::Closed(report);
    }
    inner.closed.notify_waiters();
}

async fn settle_tasks(
    tasks: &mut JoinSet<SupervisedTaskOutcome>,
    deadline: Instant,
    report: &mut ScopeCloseReport,
) {
    loop {
        if tasks.is_empty() {
            return;
        }
        match tokio::time::timeout_at(deadline, tasks.join_next()).await {
            Ok(Some(result)) => record_task_result(result, report),
            Ok(None) => return,
            Err(_) => {
                let remaining = tasks.len();
                report.tasks_timed_out += remaining;
                tasks.abort_all();
                let _ = tokio::time::timeout(ABORT_SETTLE_GRACE, tasks.shutdown()).await;
                return;
            }
        }
    }
}

fn record_task_result(
    result: Result<SupervisedTaskOutcome, JoinError>,
    report: &mut ScopeCloseReport,
) {
    match result {
        Ok(outcome) => match outcome.result {
            Ok(()) => report.tasks_completed += 1,
            Err(error) => {
                report.tasks_failed += 1;
                tracing::warn!(task = %outcome.name, error = %error, "Capability scope task failed");
            }
        },
        Err(error) if error.is_cancelled() => report.tasks_cancelled += 1,
        Err(error) => {
            report.tasks_failed += 1;
            tracing::warn!(error = %error, "Capability scope task panicked");
        }
    }
}

async fn close_children(
    children: BTreeMap<u64, Box<dyn SupervisedChild>>,
    deadline: Instant,
    report: &mut ScopeCloseReport,
) {
    for (_, child) in children.into_iter().rev() {
        let name = child.name().to_owned();
        if Instant::now() >= deadline {
            drop(child);
            report.child_scopes_timed_out += 1;
            continue;
        }
        let mut close = tokio::spawn(async move { child.close().await });
        match tokio::time::timeout_at(deadline, &mut close).await {
            Ok(Ok(Ok(child_report))) => {
                report.child_scopes_closed += 1;
                if !child_report.is_clean() {
                    report.child_scopes_failed += 1;
                }
            }
            Ok(Ok(Err(error))) => {
                report.child_scopes_failed += 1;
                tracing::warn!(scope = %name, error = %error, "Child capability scope close failed");
            }
            Ok(Err(error)) => {
                report.child_scopes_failed += 1;
                tracing::warn!(scope = %name, error = %error, "Child capability scope close panicked");
            }
            Err(_) => {
                close.abort();
                let _ = tokio::time::timeout(ABORT_SETTLE_GRACE, &mut close).await;
                report.child_scopes_timed_out += 1;
            }
        }
    }
}

async fn close_effects(
    effects: Vec<RegisteredEffect>,
    deadline: Instant,
    report: &mut ScopeCloseReport,
) {
    for registered in effects.into_iter().rev() {
        let name = registered.name;
        let effect = registered.effect;
        if Instant::now() >= deadline {
            drop(effect);
            report.effects_timed_out += 1;
            continue;
        }
        let mut close = tokio::spawn(async move { effect.close().await });
        match tokio::time::timeout_at(deadline, &mut close).await {
            Ok(Ok(Ok(()))) => report.effects_closed += 1,
            Ok(Ok(Err(error))) => {
                report.effects_failed += 1;
                tracing::warn!(effect = %name, error = %error, "Capability effect close failed");
            }
            Ok(Err(error)) => {
                report.effects_failed += 1;
                tracing::warn!(effect = %name, error = %error, "Capability effect close panicked");
            }
            Err(_) => {
                close.abort();
                let _ = tokio::time::timeout(ABORT_SETTLE_GRACE, &mut close).await;
                report.effects_timed_out += 1;
            }
        }
    }
}

fn validate_lifecycle_name(value: &str) -> Result<(), CapabilityScopeError> {
    if value.is_empty() {
        return Err(CapabilityScopeError::InvalidLifecycleName {
            reason: "it is empty",
        });
    }
    if value.len() > MAX_LIFECYCLE_NAME_BYTES {
        return Err(CapabilityScopeError::BoundExceeded {
            field: "lifecycle_name",
            max: MAX_LIFECYCLE_NAME_BYTES,
        });
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
    }) || !value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        || !value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(CapabilityScopeError::InvalidLifecycleName {
            reason: "it contains non-canonical characters or boundaries",
        });
    }
    Ok(())
}
