//! Fail-fast admission control for session conversation operations.
//!
//! A session owns one mutable transcript and one current-run slot. Admission is
//! therefore intentionally single-flight: callers receive `SessionBusy`
//! immediately instead of waiting for another operation to release the session.

use crate::error::{CodeError, Result};
use futures::FutureExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::sync::{oneshot, Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard, Notify};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;

mod publication;
use publication::*;

pub(super) struct RunAdmission {
    active: AtomicBool,
    stream_aborts: Mutex<Vec<AbortHandle>>,
    original: Mutex<Option<Arc<OriginalExecutionContext>>>,
    idle: Notify,
    log_source: Option<(std::path::PathBuf, String)>,
    fact_log: Arc<Mutex<Option<Arc<a3s_effect::FileLog>>>>,
}

impl std::fmt::Debug for RunAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunAdmission")
            .field("active", &self.active.load(Ordering::Acquire))
            .field("has_original_log_source", &self.log_source.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for RunAdmission {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(false),
            stream_aborts: Mutex::new(Vec::new()),
            original: Mutex::new(None),
            idle: Notify::new(),
            log_source: None,
            fact_log: Arc::new(Mutex::new(None)),
        }
    }
}

impl RunAdmission {
    pub(super) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(super) fn for_session(workspace: &std::path::Path, session_id: &str) -> Self {
        Self {
            log_source: Some((
                crate::session_folder::effect_log_dir(workspace, session_id),
                session_id.to_owned(),
            )),
            ..Self::default()
        }
    }

    pub(super) fn fact_log_slot(&self) -> Arc<Mutex<Option<Arc<a3s_effect::FileLog>>>> {
        Arc::clone(&self.fact_log)
    }

    pub(super) fn try_acquire(self: &Arc<Self>, session_id: &str) -> Result<RunAdmissionLease> {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| CodeError::SessionBusy {
                session_id: session_id.to_string(),
            })?;

        let mut lease = RunAdmissionLease {
            admission: Arc::clone(self),
            task_lease: None,
            session_id: session_id.to_owned(),
            original: OnceLock::new(),
            retirement_handled: false,
            log_owner: None,
        };
        if let Some((directory, source_session)) = &self.log_source {
            if source_session != session_id {
                return Err(owner_error("original log admission identity mismatch"));
            }
            let log = {
                let mut shared = self
                    .fact_log
                    .lock()
                    .map_err(|_| owner_error("poisoned original log admission"))?;
                match shared.as_ref() {
                    Some(log) => Arc::clone(log),
                    None => {
                        let log =
                            Arc::new(a3s_effect::FileLog::open(directory).map_err(|error| {
                                CodeError::Session(format!("original log admission: {error}"))
                            })?);
                        *shared = Some(Arc::clone(&log));
                        log
                    }
                }
            };
            lease.log_owner = Some(
                log.try_run_owner(&crate::fact_control::thread_for_session(session_id))
                    .map_err(|error| match error {
                        a3s_effect::ActorError::LogBusy => CodeError::SessionBusy {
                            session_id: session_id.to_owned(),
                        },
                        other => CodeError::Session(format!("original log ownership: {other}")),
                    })?,
            );
        }
        Ok(lease)
    }

    pub(super) async fn wait_for_original_release(
        &self,
        original: &Arc<OriginalExecutionContext>,
        timeout: Duration,
    ) -> Option<OwnerDisposition> {
        let wait = async {
            loop {
                let notified = self.idle.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                {
                    let state = original.state.lock().ok()?;
                    if let Some(actual) = state.actual_retired {
                        if matches!(
                            actual,
                            OwnerDisposition::Released | OwnerDisposition::NeverStarted
                        ) && state.disposition == Some(actual)
                        {
                            return Some(actual);
                        }
                        return None;
                    }
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, wait).await.ok().flatten()
    }

    pub(super) fn abort_original_stream_workers(
        &self,
        original: &Arc<OriginalExecutionContext>,
    ) -> bool {
        let handles = {
            let Ok(registered) = self.original.lock() else {
                return false;
            };
            if original.mode == ExecutionMode::Blocking
                || !registered
                    .as_ref()
                    .is_some_and(|actual| Arc::ptr_eq(actual, original))
            {
                return false;
            }
            let Ok(aborts) = self.stream_aborts.lock() else {
                return false;
            };
            aborts.clone()
        };
        if handles.is_empty() {
            return false;
        }
        for actual in handles {
            actual.abort();
        }
        true
    }

    pub(super) fn current_original(&self) -> Option<Arc<OriginalExecutionContext>> {
        self.original.lock().ok()?.clone()
    }

    fn register_stream_workers(&self, aborts: Vec<AbortHandle>) {
        *self
            .stream_aborts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = aborts;
    }

    fn clear_stream_workers(&self) {
        self.stream_aborts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
    }

    /// Abort the real execution and event-forwarding tasks registered for the
    /// current streaming lease. The lifecycle supervisor remains alive so it
    /// can clear run state and release the lease after both tasks settle.
    /// Blocking sends have no registered workers and remain cooperative-only.
    #[cfg(test)]
    pub(super) fn abort_stream_workers(&self) -> bool {
        let aborts = self
            .stream_aborts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        if aborts.is_empty() {
            false
        } else {
            for abort in aborts {
                abort.abort();
            }
            true
        }
    }

    pub(super) async fn wait_until_idle(&self, timeout: Duration) -> bool {
        let idle = async {
            loop {
                let notified = self.idle.notified();
                if !self.active.load(Ordering::Acquire) {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, idle).await.is_ok()
    }
}

/// RAII lease for one admitted session operation.
pub(super) struct RunAdmissionLease {
    admission: Arc<RunAdmission>,
    task_lease: Option<crate::task_scheduler::TaskLease>,
    session_id: String,
    original: OnceLock<Arc<OriginalExecutionContext>>,
    retirement_handled: bool,
    log_owner: Option<a3s_effect::FileLogRunOwnerFence>,
}

impl RunAdmissionLease {
    pub(super) fn nonexecuting_fact_log(
        &self,
        actual: &Arc<RunAdmission>,
        session_id: &str,
    ) -> Result<Arc<a3s_effect::FileLog>> {
        if !self.is_nonexecuting_fence_for(actual)
            || self.log_owner.is_none()
            || self.session_id != session_id
            || !self
                .admission
                .log_source
                .as_ref()
                .is_some_and(|(_, source)| source == session_id)
        {
            return Err(owner_error(
                "goal.writer_unavailable: original nonexecuting admission is unproved",
            ));
        }
        self.admission
            .fact_log
            .lock()
            .map_err(|_| owner_error("goal.writer_unavailable: poisoned original log"))?
            .clone()
            .ok_or_else(|| owner_error("goal.writer_unavailable: original log is absent"))
    }

    pub(super) fn attach_task_lease(
        mut self,
        task_lease: crate::task_scheduler::TaskLease,
    ) -> Self {
        self.task_lease = Some(task_lease);
        self
    }

    pub(super) fn is_nonexecuting_fence_for(&self, actual: &Arc<RunAdmission>) -> bool {
        Arc::ptr_eq(&self.admission, actual)
            && self.task_lease.is_none()
            && self.original.get().is_none()
            && self
                .admission
                .original
                .lock()
                .is_ok_and(|registered| registered.is_none())
    }

    fn retire_original(
        &mut self,
        original: &Arc<OriginalExecutionContext>,
    ) -> Result<OwnerDisposition> {
        // Lock order is registration → original state → original abort list.
        let mut registered = self
            .admission
            .original
            .lock()
            .map_err(|_| owner_error("poisoned retirement registration"))?;
        let mut state = original
            .state
            .lock()
            .map_err(|_| owner_error("poisoned retirement owner"))?;
        let mut aborts = self
            .admission
            .stream_aborts
            .lock()
            .map_err(|_| owner_error("poisoned retirement aborts"))?;
        let actual = state
            .disposition
            .ok_or_else(|| owner_error("missing positive retirement disposition"))?;
        if self.retirement_handled
            || !registered
                .as_ref()
                .is_some_and(|actual| Arc::ptr_eq(actual, original))
            || !self
                .original
                .get()
                .is_some_and(|actual| Arc::ptr_eq(actual, original))
            || !matches!(
                actual,
                OwnerDisposition::Released | OwnerDisposition::NeverStarted
            )
            || state.actual_retired.is_some()
            || !self
                .task_lease
                .as_ref()
                .is_some_and(crate::task_scheduler::TaskLease::consumes_global_slot)
        {
            return Err(owner_error(
                "foreign or unproved original physical retirement",
            ));
        }
        if let Some(owner) = self.log_owner.as_mut() {
            owner.release().map_err(|error| {
                CodeError::Session(format!("original log owner release: {error}"))
            })?;
        }
        drop(self.log_owner.take());
        // All guards and predicates precede mutation. TaskLease Drop only enqueues
        // the original scheduler release; this is NOT a processed-capacity ack.
        drop(self.task_lease.take());
        aborts.clear();
        *registered = None;
        state.actual_retired = Some(actual);
        state.release.retired = true;
        self.retirement_handled = true;
        self.admission.active.store(false, Ordering::Release);
        self.admission.idle.notify_waiters();
        Ok(actual)
    }

    pub(super) fn belongs_to(&self, original: &Arc<RunAdmission>) -> bool {
        Arc::ptr_eq(&self.admission, original)
    }

    pub(super) fn register_original(
        &self,
        session_id: String,
        mode: ExecutionMode,
        original_cancel: CancellationToken,
        active_slot: Arc<AsyncMutex<Option<Arc<crate::run_control::RunControlInbox>>>>,
        current_slot: Arc<AsyncMutex<Option<String>>>,
        cancel_slot: Arc<AsyncMutex<Option<CancellationToken>>>,
    ) -> Result<Arc<OriginalExecutionContext>> {
        if session_id != self.session_id
            || !self
                .task_lease
                .as_ref()
                .is_some_and(crate::task_scheduler::TaskLease::consumes_global_slot)
        {
            return Err(owner_error(
                "foreign session or missing original task lease",
            ));
        }
        let mut registered = self
            .admission
            .original
            .lock()
            .map_err(|_| owner_error("poisoned original registration"))?;
        if registered.is_some() || self.original.get().is_some() {
            return Err(owner_error("original context already registered"));
        }
        let original_fact_log = if self.log_owner.is_some() {
            Some(
                self.admission
                    .fact_log
                    .lock()
                    .map_err(|_| owner_error("poisoned original admitted log"))?
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| owner_error("missing original admitted log"))?,
            )
        } else {
            None
        };
        let original = Arc::new(OriginalExecutionContext {
            session_id,
            mode,
            original_cancel,
            active_slot,
            current_slot,
            cancel_slot,
            state: Mutex::new(OriginalOwnerState::default()),
            committer: OriginalCommitterCell::default(),
            model_call: OriginalModelCell::default(),
            original_fact_log,
        });
        self.original
            .set(Arc::clone(&original))
            .map_err(|_| owner_error("original lease already bound"))?;
        *registered = Some(Arc::clone(&original));
        Ok(original)
    }
}

impl Drop for RunAdmissionLease {
    fn drop(&mut self) {
        if self.retirement_handled {
            return;
        }
        if let Some(original) = self.original.get() {
            if let Ok(mut registered) = self.admission.original.lock() {
                if registered
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, original))
                {
                    *registered = None;
                }
            }
        }
        drop(self.log_owner.take());
        self.admission.active.store(false, Ordering::Release);
        self.admission.idle.notify_waiters();
    }
}

/// Keep a streaming operation's lease until its worker/forwarder lifecycle
/// handle has completed.
///
/// A detached guardian owns the lease, so dropping or aborting the public proxy
/// handle does not admit a second run while the original background work is
/// still active.
pub(super) fn guard_stream_handle(
    handle: JoinHandle<()>,
    worker_aborts: Vec<AbortHandle>,
    lease: RunAdmissionLease,
) -> JoinHandle<()> {
    spawn_guardian(
        None,
        handle,
        worker_aborts,
        lease,
        GuardianRegistration::LegacyUnqualified,
    )
}

fn owner_error(message: &str) -> CodeError {
    CodeError::Internal(anyhow::anyhow!("original execution owner: {message}"))
}

macro_rules! diagnostic_set {
    ($name:ident, $label:literal, {$($variant:ident => $wire:literal),+ $(,)?}) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(super) enum $name { $($variant),+ }
        impl $name {
            #[cfg(test)]
            pub(super) fn parse(value: &str) -> Result<Self> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    _ => Err(owner_error(&format!("unknown {} '{value}'", $label))),
                }
            }
            #[cfg(test)]
            pub(super) fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire,)+ }
            }
        }
    };
}

diagnostic_set!(ExecutionMode, "execution mode", {
    Blocking => "blocking", Streaming => "streaming", Detached => "detached",
});
diagnostic_set!(EffectPhase, "effect phase", {
    NeverStarted => "never_started", MayHaveEffects => "may_have_effects",
    WorkerFinished => "worker_finished",
});
diagnostic_set!(OwnerDisposition, "owner disposition", {
    NeverStarted => "never_started", Released => "released",
    RetainedUnproved => "retained_unproved",
});

mod joined_failure;
pub(super) use joined_failure::OriginalFailurePublisher;

/// Opaque weak identity only. The request operation and actual handle minting
/// belong to T005; this component grants neither mutation nor publication.
#[derive(Debug, Clone)]
pub(crate) struct RunCancelPin {
    original: Weak<OriginalExecutionContext>,
}

impl RunCancelPin {
    pub(super) fn from_original(original: &Arc<OriginalExecutionContext>) -> Self {
        Self {
            original: Arc::downgrade(original),
        }
    }
}

#[derive(Debug)]
pub(super) struct OriginalExecutionContext {
    session_id: String,
    mode: ExecutionMode,
    original_cancel: CancellationToken,
    active_slot: Arc<AsyncMutex<Option<Arc<crate::run_control::RunControlInbox>>>>,
    current_slot: Arc<AsyncMutex<Option<String>>>,
    cancel_slot: Arc<AsyncMutex<Option<CancellationToken>>>,
    state: Mutex<OriginalOwnerState>,
    committer: OriginalCommitterCell,
    model_call: OriginalModelCell,
    original_fact_log: Option<Arc<a3s_effect::FileLog>>,
}

/// Lifetime-only port to the SAME original execution. Only its actual canonical
/// worker may reserve a model call; the provider child never receives this port.
#[derive(Clone)]
pub(crate) struct OriginalModelCallPort {
    original: Weak<OriginalExecutionContext>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelCallJoin {
    NeverEntered,
    Returned(crate::agent::FactAccountingDisposition),
    Unproved,
}

#[derive(Debug)]
struct ModelReady {
    actual: std::result::Result<
        std::result::Result<crate::agent::FactModelCallOutcome, a3s_effect::ActorError>,
        tokio::task::JoinError,
    >,
    accounting: Arc<crate::agent::FactModelAccountingObservation>,
    observed: bool,
    attempt_cancelled: bool,
}

#[derive(Debug)]
enum OriginalModelState {
    Vacant,
    Reserved {
        previous: Option<ModelReady>,
    },
    Running {
        actual: JoinHandle<
            std::result::Result<crate::agent::FactModelCallOutcome, a3s_effect::ActorError>,
        >,
        attempt: CancellationToken,
        accounting: Arc<crate::agent::FactModelAccountingObservation>,
    },
    Ready(ModelReady),
}

#[derive(Debug)]
struct OriginalModelCell {
    state: Arc<AsyncMutex<OriginalModelState>>,
}

impl Default for OriginalModelCell {
    fn default() -> Self {
        Self {
            state: Arc::new(AsyncMutex::new(OriginalModelState::Vacant)),
        }
    }
}

pub(crate) struct OriginalModelCallReservation {
    state: tokio::sync::OwnedMutexGuard<OriginalModelState>,
}

impl Drop for OriginalModelCallReservation {
    fn drop(&mut self) {
        if matches!(&*self.state, OriginalModelState::Reserved { .. }) {
            // No spawn/handle was installed. Restore its actual prior completed
            // observation, or positive initial non-entry; never erase Running.
            let previous = std::mem::replace(&mut *self.state, OriginalModelState::Vacant);
            if let OriginalModelState::Reserved {
                previous: Some(previous),
            } = previous
            {
                *self.state = OriginalModelState::Ready(previous);
            }
        }
    }
}

impl OriginalModelCallReservation {
    pub(crate) fn install_and_start(
        mut self,
        actual: JoinHandle<
            std::result::Result<crate::agent::FactModelCallOutcome, a3s_effect::ActorError>,
        >,
        attempt: CancellationToken,
        accounting: Arc<crate::agent::FactModelAccountingObservation>,
        start: oneshot::Sender<()>,
    ) {
        // The exclusive validated reservation exists before spawn. This
        // synchronous install cannot return Err or detach the actual handle.
        *self.state = OriginalModelState::Running {
            actual,
            attempt,
            accounting,
        };
        let _ = start.send(());
    }
}

impl OriginalExecutionContext {
    pub(super) fn model_call_port(self: &Arc<Self>) -> OriginalModelCallPort {
        OriginalModelCallPort {
            original: Arc::downgrade(self),
        }
    }
}

impl OriginalModelCallPort {
    pub(crate) fn admits_original_run(
        &self,
        log: &Arc<a3s_effect::FileLog>,
        session_id: &str,
        run_id: &str,
    ) -> bool {
        self.admits_original_log(log, session_id)
            && self.original.upgrade().is_some_and(|original| {
                original
                    .created_handle()
                    .is_ok_and(|run| run.id() == run_id)
            })
    }

    pub(crate) fn admits_original_log(
        &self,
        log: &Arc<a3s_effect::FileLog>,
        session_id: &str,
    ) -> bool {
        let Some(original) = self.original.upgrade() else {
            return false;
        };
        original.session_id == session_id
            && original
                .original_fact_log
                .as_ref()
                .is_some_and(|actual| Arc::ptr_eq(actual, log))
            && original
                .worker_state()
                .is_ok_and(|state| !state.release.sealed && !state.release.retired)
    }

    pub(crate) async fn reserve_same_original_call(
        &self,
        session_id: &str,
    ) -> Result<OriginalModelCallReservation> {
        let original = self
            .original
            .upgrade()
            .ok_or_else(|| owner_error("original model owner missing"))?;
        {
            let state = original
                .state
                .lock()
                .map_err(|_| owner_error("poisoned original model owner"))?;
            if original.session_id != session_id
                || state.phase != EffectPhase::MayHaveEffects
                || state.worker_id != tokio::task::try_id()
            {
                return Err(owner_error(
                    "model call requires the same actual entered original worker",
                ));
            }
        }
        let mut state = Arc::clone(&original.model_call.state)
            .try_lock_owned()
            .map_err(|_| owner_error("original model call already borrowed"))?;
        if matches!(*state, OriginalModelState::Running { .. }) {
            // A response deadline ends observation, not the actual original
            // child. Reuse waits for that same handle and owed accounting.
            OriginalModelCell::observe_join(&mut state).await;
        }
        let reusable = match &*state {
            OriginalModelState::Vacant => true,
            OriginalModelState::Ready(ready) => {
                ready.observed && model_ready_observation(ready) != ModelCallJoin::Unproved
            }
            _ => false,
        };
        if !reusable {
            return Err(owner_error("original model call unresolved"));
        }
        let previous = match std::mem::replace(&mut *state, OriginalModelState::Vacant) {
            OriginalModelState::Ready(ready) => Some(ready),
            _ => None,
        };
        *state = OriginalModelState::Reserved { previous };
        Ok(OriginalModelCallReservation { state })
    }

    pub(crate) async fn wait_same_original_call(
        &self,
    ) -> std::result::Result<crate::agent::FactAccountingDisposition, a3s_effect::ActorError> {
        let original = self
            .original
            .upgrade()
            .ok_or_else(|| a3s_effect::ActorError::Defect("original model owner missing".into()))?;
        let observed = original.model_call.wait().await;
        let state = original.model_call.state.lock().await;
        if let OriginalModelState::Ready(ready) = &*state {
            if let Ok(Err(error)) = &ready.actual {
                return Err(error.clone());
            }
            if let Err(error) = &ready.actual {
                return Err(a3s_effect::ActorError::Defect(format!(
                    "model task failed: {error}"
                )));
            }
        }
        match observed {
            ModelCallJoin::Returned(disposition) => Ok(disposition),
            _ => Err(a3s_effect::ActorError::Defect(
                "original model join/accounting unproved".into(),
            )),
        }
    }
}

fn model_ready_observation(ready: &ModelReady) -> ModelCallJoin {
    let disposition = ready.accounting.disposition();
    if ready.accounting.projection_has_missing_return() {
        return ModelCallJoin::Unproved;
    }
    tracing::trace!(
        original_attempt_cancelled = ready.attempt_cancelled,
        accounting = disposition.as_str(),
        "observed original model settlement"
    );
    match &ready.actual {
        Ok(Ok(outcome))
            if outcome.accounting == disposition
                && disposition != crate::agent::FactAccountingDisposition::Unproved =>
        {
            ModelCallJoin::Returned(disposition)
        }
        Ok(Err(_)) if disposition == crate::agent::FactAccountingDisposition::NoSuccessObserved => {
            ModelCallJoin::Returned(disposition)
        }
        _ => ModelCallJoin::Unproved,
    }
}

impl OriginalModelCell {
    fn has_terminal_observation(&self) -> bool {
        self.state.try_lock().is_ok_and(|state| match &*state {
            OriginalModelState::Vacant => true,
            OriginalModelState::Ready(ready) => {
                ready.observed && model_ready_observation(ready) != ModelCallJoin::Unproved
            }
            _ => false,
        })
    }

    fn is_vacant(&self) -> bool {
        self.state
            .try_lock()
            .is_ok_and(|state| matches!(*state, OriginalModelState::Vacant))
    }

    async fn wait(&self) -> ModelCallJoin {
        let mut state = self.state.lock().await;
        Self::observe_join(&mut state).await
    }

    async fn observe_join(state: &mut OriginalModelState) -> ModelCallJoin {
        match state {
            OriginalModelState::Vacant => return ModelCallJoin::NeverEntered,
            OriginalModelState::Reserved { .. } => return ModelCallJoin::Unproved,
            OriginalModelState::Running {
                actual,
                accounting,
                attempt,
            } => {
                // Borrow the actual handle. Cancellation drops only this lock
                // guard; it neither takes nor aborts the installed handle.
                let returned = (&mut *actual).await;
                let accounting = Arc::clone(accounting);
                let attempt_cancelled = attempt.is_cancelled();
                *state = OriginalModelState::Ready(ModelReady {
                    actual: returned,
                    accounting,
                    observed: false,
                    attempt_cancelled,
                });
                // No await between the winning actual Ready poll and retention.
            }
            OriginalModelState::Ready(_) => {}
        }
        if let OriginalModelState::Ready(ready) = state {
            let disposition = model_ready_observation(ready);
            ready.observed = disposition != ModelCallJoin::Unproved;
            disposition
        } else {
            ModelCallJoin::Unproved
        }
    }
}

#[derive(Debug, Default)]
enum StartObservation {
    #[default]
    NotObserved,
    ReceiverClosed,
    Received,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActualJoinObservation {
    Completed,
    Cancelled,
    Panicked,
}

impl ActualJoinObservation {
    fn from_result(actual: &std::result::Result<(), tokio::task::JoinError>) -> Self {
        match actual {
            Ok(()) => Self::Completed,
            Err(error) if error.is_cancelled() => Self::Cancelled,
            Err(_) => Self::Panicked,
        }
    }
}

#[derive(Debug)]
struct OriginalOwnerState {
    phase: EffectPhase,
    start_gate_created: bool,
    start: StartObservation,
    worker_id: Option<tokio::task::Id>,
    worker_join: Option<std::result::Result<(), tokio::task::JoinError>>,
    guardian_id: Option<tokio::task::Id>,
    failure_continuation: Option<joined_failure::OriginalFailureContinuation>,
    disposition: Option<OwnerDisposition>,
    actual_retired: Option<OwnerDisposition>,
    run: OriginalRunBinding,
    release: OriginalReleaseState,
}

impl Default for OriginalOwnerState {
    fn default() -> Self {
        Self {
            phase: EffectPhase::NeverStarted,
            start_gate_created: false,
            start: StartObservation::NotObserved,
            worker_id: None,
            worker_join: None,
            guardian_id: None,
            failure_continuation: None,
            disposition: None,
            actual_retired: None,
            run: OriginalRunBinding::Unobserved,
            release: OriginalReleaseState::default(),
        }
    }
}

pub(super) struct OwnedWorkerStart {
    original: Arc<OriginalExecutionContext>,
    receiver: oneshot::Receiver<()>,
}

pub(super) struct OwnedStartSignal {
    original: Weak<OriginalExecutionContext>,
    sender: oneshot::Sender<()>,
}

impl OriginalExecutionContext {
    pub(super) fn start_gate(self: &Arc<Self>) -> Result<(OwnedStartSignal, OwnedWorkerStart)> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| owner_error("poisoned start state"))?;
        if state.start_gate_created {
            return Err(owner_error("original start gate already constructed"));
        }
        state.start_gate_created = true;
        let (sender, receiver) = oneshot::channel();
        Ok((
            OwnedStartSignal {
                original: Arc::downgrade(self),
                sender,
            },
            OwnedWorkerStart {
                original: Arc::clone(self),
                receiver,
            },
        ))
    }

    #[cfg(test)]
    pub(super) fn observed_worker_join(&self) -> Option<ActualJoinObservation> {
        self.state
            .lock()
            .ok()?
            .worker_join
            .as_ref()
            .map(ActualJoinObservation::from_result)
    }

    fn observe_worker_join(
        &self,
        actual_id: tokio::task::Id,
        actual: std::result::Result<(), tokio::task::JoinError>,
    ) -> OwnerDisposition {
        let observed = ActualJoinObservation::from_result(&actual);
        let Ok(mut state) = self.state.lock() else {
            return OwnerDisposition::RetainedUnproved;
        };
        let never_started = state.worker_id == Some(actual_id)
            && matches!(state.start, StartObservation::ReceiverClosed)
            && state.phase == EffectPhase::NeverStarted
            && observed == ActualJoinObservation::Completed
            && self.committer.is_unconstructed()
            && self.model_call.is_vacant();
        if state.worker_id == Some(actual_id) {
            state.guardian_id = tokio::task::try_id();
        }
        state.worker_join = Some(actual);
        state.phase = EffectPhase::WorkerFinished;
        // No entered-path release witness exists in this non-activating slice.
        let disposition = if never_started {
            OwnerDisposition::NeverStarted
        } else {
            OwnerDisposition::RetainedUnproved
        };
        state.disposition = Some(disposition);
        disposition
    }
}

impl OwnedWorkerStart {
    pub(super) async fn wait(self) -> Result<bool> {
        {
            let mut state = self
                .original
                .state
                .lock()
                .map_err(|_| owner_error("poisoned original worker state"))?;
            if state.worker_id.is_some() {
                return Err(owner_error("original worker start already observed"));
            }
            state.worker_id = Some(
                tokio::task::try_id()
                    .ok_or_else(|| owner_error("original start requires an actual worker task"))?,
            );
        }
        let received = self.receiver.await.is_ok();
        let mut state = self
            .original
            .state
            .lock()
            .map_err(|_| owner_error("poisoned original start observation"))?;
        state.start = if received {
            StartObservation::Received
        } else {
            StartObservation::ReceiverClosed
        };
        if received {
            state.phase = EffectPhase::MayHaveEffects;
        }
        Ok(received)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActualCommitterJoin {
    Joined(ActualJoinObservation),
    PositivelyDisabled,
}

#[derive(Debug)]
enum OriginalCommitterState {
    NotConstructedYet,
    PositivelyDisabled,
    Running(JoinHandle<()>),
    Joined(std::result::Result<(), tokio::task::JoinError>),
}

#[derive(Debug)]
struct OriginalCommitterCell {
    state: AsyncMutex<OriginalCommitterState>,
}

impl Default for OriginalCommitterCell {
    fn default() -> Self {
        Self {
            state: AsyncMutex::new(OriginalCommitterState::NotConstructedYet),
        }
    }
}

pub(super) struct OriginalCommitterInstall<'a> {
    state: AsyncMutexGuard<'a, OriginalCommitterState>,
}

impl OriginalCommitterCell {
    fn has_completed_observation(&self) -> bool {
        self.state.try_lock().is_ok_and(|state| {
            matches!(
                &*state,
                OriginalCommitterState::PositivelyDisabled | OriginalCommitterState::Joined(Ok(()))
            )
        })
    }

    fn reserve_install(&self) -> Result<OriginalCommitterInstall<'_>> {
        let state = self
            .state
            .try_lock()
            .map_err(|_| owner_error("committer cell busy"))?;
        if !matches!(*state, OriginalCommitterState::NotConstructedYet) {
            return Err(owner_error("original committer already constructed"));
        }
        Ok(OriginalCommitterInstall { state })
    }

    fn positively_disabled(&self) -> Result<()> {
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| owner_error("committer cell busy"))?;
        if !matches!(*state, OriginalCommitterState::NotConstructedYet) {
            return Err(owner_error("original committer already constructed"));
        }
        *state = OriginalCommitterState::PositivelyDisabled;
        Ok(())
    }

    fn is_unconstructed(&self) -> bool {
        self.state
            .try_lock()
            .is_ok_and(|state| matches!(*state, OriginalCommitterState::NotConstructedYet))
    }

    async fn wait(&self) -> Result<ActualCommitterJoin> {
        let mut state = self.state.lock().await;
        match &mut *state {
            OriginalCommitterState::Running(actual) => {
                let returned = (&mut *actual).await;
                let observed = ActualJoinObservation::from_result(&returned);
                // No await or cancelled window between actual return and retention.
                *state = OriginalCommitterState::Joined(returned);
                Ok(ActualCommitterJoin::Joined(observed))
            }
            OriginalCommitterState::Joined(actual) => Ok(ActualCommitterJoin::Joined(
                ActualJoinObservation::from_result(actual),
            )),
            OriginalCommitterState::PositivelyDisabled => {
                Ok(ActualCommitterJoin::PositivelyDisabled)
            }
            OriginalCommitterState::NotConstructedYet => Err(owner_error("committer unproved")),
        }
    }
}

impl OriginalCommitterInstall<'_> {
    pub(super) fn install(mut self, actual: JoinHandle<()>) {
        *self.state = OriginalCommitterState::Running(actual);
    }
}

diagnostic_set!(OriginalScopeKind, "original scope kind", {
    Command => "command", Fact => "fact",
});
diagnostic_set!(OriginalCancelOrigin, "original cancel origin", {
    Host => "host", Close => "close", Admission => "admission",
});
diagnostic_set!(OriginalPublicationKind, "original publication kind", {
    End => "end", Error => "error", Cancelled => "cancelled",
    AdmissionFailed => "admission_failed", NoRun => "no_run",
});

#[derive(Debug)]
enum OriginalRunBinding {
    Unobserved,
    NoRun,
    Created {
        snapshot: Box<crate::run::RunSnapshot>,
        handle: crate::run::RunHandle,
    },
}

#[derive(Debug, Default)]
enum OriginalScopeObservation {
    #[default]
    Unobserved,
    NotAttempted,
    Attempted,
    Returned {
        instance: uuid::Uuid,
        close: Option<
            std::result::Result<
                crate::capability::ScopeCloseReport,
                crate::capability::CapabilityRuntimeError,
            >,
        >,
    },
}

impl OriginalScopeObservation {
    fn clean(&self) -> bool {
        matches!(self, Self::NotAttempted)
            || matches!(self, Self::Returned { close: Some(Ok(report)), .. } if report.is_clean())
    }
}

#[derive(Debug, Clone)]
pub(super) enum OriginalStoreObservation {
    NotApplicable,
    NotAttempted,
    Entered,
    Returned(Box<Option<crate::run::RunSnapshot>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OriginalHookObservation {
    NotApplicable,
    NotAttempted,
    Entered,
    Returned,
}

#[derive(Debug, Clone)]
pub(super) struct OriginalPublicationObservation {
    pub(super) kind: OriginalPublicationKind,
    pub(super) terminal: OriginalStoreObservation,
    pub(super) diagnostic: OriginalStoreObservation,
    pub(super) cancellation_hook: OriginalHookObservation,
    pub(super) event_hook: OriginalHookObservation,
}

#[derive(Debug)]
pub(super) struct OriginalPublicationTarget {
    pub(super) session_id: String,
    pub(super) run_id: Option<String>,
    pub(super) cancelled: bool,
    pub(super) cancel_origin: Option<OriginalCancelOrigin>,
}

/// Unique worker-local ticket. It grants no writer and never owns its context.
pub(super) struct SealedPublication {
    original: Weak<OriginalExecutionContext>,
}

#[derive(Debug, Default)]
struct OriginalReleaseState {
    sealed: bool,
    retired: bool,
    cancelled_at_seal: bool,
    hooks_configured: Option<bool>,
    cancel_origin: Option<OriginalCancelOrigin>,
    inbox: Option<Arc<crate::run_control::RunControlInbox>>,
    no_inbox: bool,
    installed: bool,
    slots_retired: bool,
    tools_cleaned: bool,
    controls: Option<crate::run_control::OriginalPendingObservation>,
    scopes: [OriginalScopeObservation; 2],
    publication: Option<OriginalPublicationObservation>,
    publication_complete: bool,
    failure_wire: Option<joined_failure::OriginalFailureWire>,
}

impl RunCancelPin {
    pub(crate) fn same_original(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.original, &other.original)
    }
    pub(crate) async fn request_cancel(&self, session_id: &str, run_id: &str) -> bool {
        match self.original.upgrade() {
            Some(original) => {
                original
                    .request(session_id, Some(run_id), OriginalCancelOrigin::Host)
                    .await
            }
            None => false,
        }
    }
}

enum GuardianRegistration {
    LegacyUnqualified,
    Owned {
        pin: super::session_close::SessionIdentityPin,
        original: Arc<OriginalExecutionContext>,
        disposition: oneshot::Sender<OwnerDisposition>,
        failure: Option<OriginalFailurePublisher>,
    },
}

/// Validated before actual worker spawn; all ownership fields stay unique.
pub(super) struct OriginalWorkerHandoff {
    runtime: tokio::runtime::Handle,
    lease: RunAdmissionLease,
    pin: super::session_close::SessionIdentityPin,
    original: Arc<OriginalExecutionContext>,
    start: OwnedStartSignal,
    failure: Option<OriginalFailurePublisher>,
}

impl OriginalWorkerHandoff {
    pub(super) fn prepare(
        session: &super::AgentSession,
        lease: RunAdmissionLease,
        mode: ExecutionMode,
        original_cancel: CancellationToken,
    ) -> Result<(Self, OwnedWorkerStart, Arc<OriginalExecutionContext>)> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| owner_error("original worker requires a runtime"))?;
        if !lease.belongs_to(&session.run_admission) {
            return Err(owner_error("foreign original admission lease"));
        }
        let original = lease.register_original(
            session.session_id.clone(),
            mode,
            original_cancel,
            Arc::clone(&session.active_run_control),
            Arc::clone(&session.current_run_id),
            Arc::clone(&session.cancel_token),
        )?;
        {
            let mut state = original
                .state
                .lock()
                .map_err(|_| owner_error("poisoned hook configuration"))?;
            state.release.hooks_configured = Some(session.hook_executor.is_some());
        }
        let (start, worker_start) = original.start_gate()?;
        let handoff = Self {
            runtime,
            lease,
            pin: session.close_handle.identity_pin(),
            original: Arc::clone(&original),
            start,
            failure: None,
        };
        Ok((handoff, worker_start, original))
    }

    pub(super) fn with_failure_publisher(mut self, failure: OriginalFailurePublisher) -> Self {
        self.failure = Some(failure);
        self
    }

    pub(super) fn supervise(
        self,
        actual_worker: JoinHandle<()>,
        mode_aborts: Vec<AbortHandle>,
        disposition: oneshot::Sender<OwnerDisposition>,
    ) -> JoinHandle<()> {
        let Self {
            runtime,
            lease,
            pin,
            original,
            start,
            failure,
        } = self;
        let mode_aborts = if original.mode == ExecutionMode::Blocking {
            Vec::new()
        } else {
            mode_aborts
        };
        let same_original = std::ptr::eq(start.original.as_ptr(), Arc::as_ptr(&original));
        let proxy = spawn_guardian(
            Some(&runtime),
            actual_worker,
            mode_aborts,
            lease,
            GuardianRegistration::Owned {
                pin,
                original,
                disposition,
                failure,
            },
        );
        // The original guardian already owns the actual worker and lease.
        if same_original {
            let _ = start.sender.send(());
        }
        proxy
    }
}

fn spawn_guardian(
    runtime: Option<&tokio::runtime::Handle>,
    handle: JoinHandle<()>,
    worker_aborts: Vec<AbortHandle>,
    mut lease: RunAdmissionLease,
    registration: GuardianRegistration,
) -> JoinHandle<()> {
    lease.admission.register_stream_workers(worker_aborts);
    let (finished_tx, finished_rx) = oneshot::channel();
    let guardian = async move {
        let actual_id = handle.id();
        let actual_join = handle.await;
        match registration {
            GuardianRegistration::LegacyUnqualified => {
                // Preserve the old join-result drop before lease retirement.
                // No new owner/proof is minted by this compatibility branch.
                drop(actual_join);
            }
            GuardianRegistration::Owned {
                pin,
                original,
                disposition,
                failure,
            } => {
                let observed = original.observe_worker_join(actual_id, actual_join);
                let mut failure_continuation = if observed == OwnerDisposition::NeverStarted {
                    drop(failure); // Positive actual non-entry; no failure target.
                    None
                } else if let Some(failure) = failure {
                    // Host observational hooks may panic. Keep that unwind in
                    // this SAME guardian, so it cannot Drop the original lease.
                    Some(joined_failure::OriginalFailureContinuation {
                        actual: std::panic::AssertUnwindSafe(
                            original.continue_joined_failure(failure),
                        )
                        .catch_unwind()
                        .await,
                    })
                } else {
                    None // Compatibility callers acquire no failure authority.
                };
                let mut continuation_unproved = failure_continuation
                    .as_ref()
                    .is_some_and(joined_failure::OriginalFailureContinuation::unproved);
                if continuation_unproved {
                    tracing::error!(observation = ?failure_continuation, "original joined failure continuation unproved");
                }
                if let Ok(mut state) = original.state.lock() {
                    state.failure_continuation = failure_continuation.take();
                } else {
                    continuation_unproved = true;
                    // A poisoned context cannot consume the real outcome; it
                    // remains owned by this guardian's retained tuple below.
                }
                // The transient publisher and its sender are gone before either
                // original child join, including normal and retained branches.
                // The SAME guardian joins canonical worker FIRST, then only
                // observes the original model cell. No callback/writer grant.
                let model = original.model_call.wait().await;
                let observed = if observed == OwnerDisposition::NeverStarted {
                    if model == ModelCallJoin::NeverEntered {
                        observed
                    } else {
                        OwnerDisposition::RetainedUnproved
                    }
                } else {
                    // Borrow the SAME unique handle only after actual worker and model.
                    let committer = original.committer.wait().await;
                    if continuation_unproved {
                        OwnerDisposition::RetainedUnproved
                    } else {
                        original.qualify_release(actual_id, model, committer)
                    }
                };
                let observed = if matches!(
                    observed,
                    OwnerDisposition::NeverStarted | OwnerDisposition::Released
                ) {
                    lease
                        .retire_original(&original)
                        .unwrap_or(OwnerDisposition::RetainedUnproved)
                } else {
                    observed
                };
                let _ = disposition.send(observed);
                if !matches!(
                    observed,
                    OwnerDisposition::NeverStarted | OwnerDisposition::Released
                ) {
                    let retained = (lease, pin, original, finished_tx, failure_continuation);
                    std::future::pending::<()>().await;
                    drop(retained); // Runtime disposal is not qualified release.
                    return;
                }
                drop(pin);
            }
        }
        if !lease.retirement_handled {
            lease.admission.clear_stream_workers();
        }
        drop(lease);
        let _ = finished_tx.send(());
    };
    let proxy = async move {
        let _ = finished_rx.await;
    };
    match runtime {
        Some(runtime) => {
            runtime.spawn(guardian);
            runtime.spawn(proxy)
        }
        None => {
            tokio::spawn(guardian);
            tokio::spawn(proxy)
        }
    }
}

fn retain_publication_progress(
    state: &mut OriginalOwnerState,
    actual: &OriginalPublicationObservation,
) -> Result<()> {
    if !state.release.sealed || state.release.publication_complete {
        return Err(owner_error("publication already consumed"));
    }
    if let Some(previous) = &state.release.publication {
        validate_publication_progress(previous, actual)?;
    } else if matches!(actual.terminal, OriginalStoreObservation::Returned(_))
        || matches!(actual.diagnostic, OriginalStoreObservation::Returned(_))
        || actual.cancellation_hook == OriginalHookObservation::Returned
        || actual.event_hook == OriginalHookObservation::Returned
    {
        return Err(owner_error(
            "publication return without prior attempted operation",
        ));
    }
    let mut retained = actual.clone();
    if let Some(previous) = &state.release.publication {
        if matches!(previous.terminal, OriginalStoreObservation::Returned(_)) {
            retained.terminal = previous.terminal.clone();
        }
        if matches!(previous.diagnostic, OriginalStoreObservation::Returned(_)) {
            retained.diagnostic = previous.diagnostic.clone();
        }
    }
    state.release.publication = Some(retained);
    Ok(())
}

#[cfg(test)]
mod owner_lifetime_tests;

#[cfg(test)]
mod log_owner_tests;

#[cfg(test)]
mod tests;
