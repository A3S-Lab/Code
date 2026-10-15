#![allow(dead_code)]

//! Run lifecycle control.
//!
//! This module owns how runs are started, cancelled, completed, failed, and
//! cleaned up. Execution contexts can call a small lifecycle interface without
//! knowing how run handles, current-run state, persistence, and cleanup interact.

use super::{
    execution_coordinator::ExecutionCoordinator, runtime_events::RunCleanupState,
    session_persistence::SessionPersistenceContext, AgentSession,
};
use crate::agent::AgentResult;
use crate::error::{CodeError, Result};
use std::sync::Arc;
use tokio::task::{AbortHandle, JoinHandle};

#[derive(Clone)]
pub(super) struct StreamRunWorkerState {
    coordinator: ExecutionCoordinator,
    persistence: Option<SessionPersistenceContext>,
    should_auto_save: Arc<std::sync::atomic::AtomicBool>,
}

impl StreamRunWorkerState {
    pub(super) async fn complete<E>(&self, result: std::result::Result<AgentResult, E>)
    where
        E: std::fmt::Display,
    {
        let terminal = self.coordinator.terminal_for(
            result.is_ok(),
            result.as_ref().err().map(ToString::to_string),
        );
        if let Ok(result) = result {
            if let Some(persistence) = &self.persistence {
                persistence.record_result(&result);
                self.should_auto_save
                    .store(true, std::sync::atomic::Ordering::Release);
            }
        }
        self.coordinator.settle_terminal(terminal).await;
    }
}

#[derive(Clone)]
pub(crate) struct RunControlState {
    session_id: String,
    run_store: Arc<crate::run::InMemoryRunStore>,
    admission: Arc<super::run_admission::RunAdmission>,
    original: Option<Arc<super::run_admission::OriginalExecutionContext>>,
    cancel_token: Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    current_run_id: Arc<tokio::sync::Mutex<Option<String>>>,
    active_run_control: Arc<tokio::sync::Mutex<Option<Arc<crate::run_control::RunControlInbox>>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
    host_env: Arc<crate::host_env::HostEnv>,
}

impl RunControlState {
    pub(crate) fn from_session(session: &AgentSession) -> Self {
        Self {
            session_id: session.session_id.clone(),
            run_store: Arc::clone(&session.run_store),
            admission: Arc::clone(&session.run_admission),
            original: session.run_admission.current_original(),
            cancel_token: Arc::clone(&session.cancel_token),
            current_run_id: Arc::clone(&session.current_run_id),
            active_run_control: Arc::clone(&session.active_run_control),
            closed: Arc::clone(&session.closed),
            hook_executor: session.hook_executor.clone(),
            host_env: Arc::clone(&session.config.host_env),
        }
    }

    pub(super) fn bind_original(
        mut self,
        original: Arc<super::run_admission::OriginalExecutionContext>,
    ) -> Result<Self> {
        original.validate_original_lineage(
            &self.session_id,
            &self.admission,
            &self.active_run_control,
            &self.current_run_id,
            &self.cancel_token,
        )?;
        self.original = Some(original);
        Ok(self)
    }

    fn owned_original(&self) -> Result<Arc<super::run_admission::OriginalExecutionContext>> {
        let original = self.original.clone().ok_or_else(|| {
            CodeError::Internal(anyhow::anyhow!("original execution binding required"))
        })?;
        original.validate_original_lineage(
            &self.session_id,
            &self.admission,
            &self.active_run_control,
            &self.current_run_id,
            &self.cancel_token,
        )?;
        Ok(original)
    }

    pub(super) async fn attach_owned_run_control(
        &self,
        run_id: &str,
        actual: Arc<crate::run_control::RunControlInbox>,
    ) -> Result<()> {
        let original = self.owned_original()?;
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(CodeError::SessionClosed {
                session_id: self.session_id.clone(),
            });
        }
        original
            .attach_original_slots(
                &self.session_id,
                &self.admission,
                run_id,
                actual,
                &self.closed,
            )
            .await
    }

    pub(super) async fn attach_run_control(
        &self,
        run_id: &str,
        control: Arc<crate::run_control::RunControlInbox>,
    ) {
        // Admission and session close are separate async paths. Recheck the
        // atomic close bit while holding the active-control slot so a runtime
        // created concurrently with `SessionCloseHandle::close` cannot become
        // controllable after the close boundary.
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            control.close(self.host_env.now_ms()).await;
            return;
        }
        // Never await while holding the slot mutex. A stale stream can still
        // be finishing its cleanup, and waiting here would otherwise block
        // the public control facade from observing the new run.
        let previous = {
            let mut slot = self.active_run_control.lock().await;
            if self.closed.load(std::sync::atomic::Ordering::Acquire) {
                None
            } else if slot
                .as_ref()
                .is_some_and(|current| current.snapshot_run_id() != run_id)
            {
                slot.take()
            } else {
                *slot = Some(control.clone());
                return;
            }
        };
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            if let Some(previous) = previous {
                previous.deactivate(self.host_env.now_ms()).await;
            }
            control.close(self.host_env.now_ms()).await;
            return;
        }
        if let Some(previous) = previous {
            previous.deactivate(self.host_env.now_ms()).await;
        }
        let mut slot = self.active_run_control.lock().await;
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            drop(slot);
            control.close(self.host_env.now_ms()).await;
        } else {
            *slot = Some(control);
        }
    }

    pub(super) async fn current_run_control(
        &self,
    ) -> Option<Arc<crate::run_control::RunControlInbox>> {
        // Read-only inbox correlation also serves retained exact retries in
        // the original slot-removal/physical-retirement window. Execution and
        // cancellation continue to require owned_original's live authority.
        let original = self.original.as_ref()?;
        original
            .correlated_current_control(&self.session_id, &self.admission)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn start_run(&self, prompt: &str) -> crate::run::RunHandle {
        let id = format!("run-{}", self.host_env.next_id());
        let reservation = self
            .reserve_run_with_id(&id, prompt)
            .await
            .expect("actual owned Created fixture");
        assert!(!reservation.replayed());
        let original = self.owned_original().unwrap();
        original.created_handle().unwrap()
    }

    pub(super) async fn start_run_with_bindings(
        &self,
        prompt: &str,
        cognitive_binding: Option<crate::cognitive_context::CognitivePackageBindingV1>,
        capability_binding: crate::capability::RunCapabilityBindingV1,
    ) -> Result<crate::run::RunHandle> {
        let id = format!("run-{}", self.host_env.next_id());
        match self.reserve_run_with_id(&id, prompt).await {
            Ok(reservation) if !reservation.replayed() => {}
            // A retained or foreign id constructed no Run, so the owner can
            // publish NoRun and keep the typed conflict.
            Ok(_) | Err(CodeError::RunIdentityConflict { .. }) => {
                self.owned_original()?.record_no_run_branch()?;
                return Err(CodeError::RunIdentityConflict { run_id: id });
            }
            Err(error) => return Err(error),
        }
        let original = self.owned_original()?;
        self.bind_capability_generation(&id, capability_binding)
            .await?;
        if let Some(binding) = cognitive_binding {
            self.bind_cognitive_package(&id, binding).await?;
        }
        original.created_handle()
    }

    pub(super) async fn reserve_run_with_id(
        &self,
        run_id: &str,
        prompt: &str,
    ) -> Result<crate::run::RunReservation> {
        let original = self.owned_original()?;
        original.validate_original_worker()?;
        if run_id.trim().is_empty() || run_id.contains('\0') || run_id.contains(['\r', '\n']) {
            return Err(CodeError::RunIdentityConflict {
                run_id: run_id.to_string(),
            });
        }
        let reservation = self
            .run_store
            .reserve_run_with_id(run_id.to_owned(), &self.session_id, prompt)
            .await;
        let snapshot = reservation.snapshot();
        if snapshot.session_id != self.session_id || snapshot.prompt != prompt {
            return Err(CodeError::RunIdentityConflict {
                run_id: run_id.to_string(),
            });
        }
        if let crate::run::RunReservation::Created(actual) = &reservation {
            // Actual Created response is bound synchronously BEFORE any next await.
            let handle = crate::run::RunHandle::new(
                actual.id.clone(),
                actual.session_id.clone(),
                Arc::clone(&self.run_store),
                super::run_admission::RunCancelPin::from_original(&original),
            );
            original.record_created_run(actual.clone(), handle)?;
        }
        Ok(reservation)
    }

    pub(super) async fn bind_cognitive_package(
        &self,
        run_id: &str,
        binding: crate::cognitive_context::CognitivePackageBindingV1,
    ) -> Result<crate::run::RunSnapshot> {
        self.run_store
            .bind_cognitive_package(run_id, binding)
            .await
            .map_err(|error| {
                CodeError::Session(format!(
                    "could not bind exact cognitive generation to Run '{run_id}': {error}"
                ))
            })
    }

    pub(super) async fn bind_capability_generation(
        &self,
        run_id: &str,
        binding: crate::capability::RunCapabilityBindingV1,
    ) -> Result<crate::run::RunSnapshot> {
        self.run_store
            .bind_capability_generation(run_id, binding)
            .await
            .map_err(|error| {
                CodeError::Session(format!(
                    "could not bind exact capability generation to Run '{run_id}': {error}"
                ))
            })
    }

    pub(super) async fn snapshot(&self, run_id: &str) -> Option<crate::run::RunSnapshot> {
        self.run_store.snapshot(run_id).await
    }

    /// The original worker alone publishes a typed admission failure.
    pub(super) async fn fail_reserved_run_start(
        &self,
        original: &Arc<super::run_admission::OriginalExecutionContext>,
        error: &CodeError,
    ) -> Result<super::run_admission::OriginalPublicationObservation> {
        use super::run_admission::{
            OriginalHookObservation as Hook, OriginalPublicationKind as Kind,
            OriginalPublicationObservation, OriginalStoreObservation as Store,
        };
        let bound = self.owned_original()?;
        if !Arc::ptr_eq(&bound, original) {
            return Err(CodeError::Internal(anyhow::anyhow!(
                "foreign original admission failure"
            )));
        }
        let cancelled = matches!(
            error,
            CodeError::SessionClosed { .. }
                | CodeError::Capability(
                    crate::capability::CapabilityRuntimeError::Cancelled
                        | crate::capability::CapabilityRuntimeError::SessionClosed
                )
        );
        if cancelled {
            original.record_admission_cancel()?;
        }
        let sealed = original.seal_publication()?;
        let target = original.publication_target(&sealed)?;
        let mut actual = OriginalPublicationObservation {
            kind: if target.run_id.is_none() {
                Kind::NoRun
            } else if target.cancelled {
                Kind::Cancelled
            } else {
                Kind::AdmissionFailed
            },
            terminal: Store::NotApplicable,
            diagnostic: Store::NotApplicable,
            cancellation_hook: Hook::NotApplicable,
            event_hook: Hook::NotApplicable,
        };
        if let Some(run_id) = target.run_id.as_deref() {
            actual.terminal = Store::Entered;
            if target.cancel_origin.is_some() && self.hook_executor.is_some() {
                actual.cancellation_hook = Hook::NotAttempted;
            }
            original.record_publication_progress(&sealed, &actual)?;
            let transition = if target.cancelled {
                crate::run::RunTerminalTransition::Cancelled
            } else {
                crate::run::RunTerminalTransition::Failed(error.to_string())
            };
            actual.terminal = Store::Returned(Box::new(
                self.run_store.settle_terminal(run_id, transition).await,
            ));
            original.record_publication_progress(&sealed, &actual)?;
            if target.cancel_origin.is_some() {
                if let Some(hook) = &self.hook_executor {
                    actual.cancellation_hook = Hook::Entered;
                    original.record_publication_progress(&sealed, &actual)?;
                    hook.record_run_cancelled(
                        run_id,
                        &self.session_id,
                        Some("cancelled during Run admission"),
                    )
                    .await;
                    actual.cancellation_hook = Hook::Returned;
                    original.record_publication_progress(&sealed, &actual)?;
                }
            }
        }
        original.record_publication(sealed, actual.clone())?;
        Ok(actual)
    }

    pub(super) async fn cancel(&self) -> bool {
        let Ok(original) = self.owned_original() else {
            return false;
        };
        let Some(handle) = original
            .correlated_current_handle(&self.session_id, &self.admission)
            .await
        else {
            return false;
        };
        // No store/hook tail. The pin rechecks exact ordered slots before acceptance.
        handle.cancel().await
    }

    pub(super) async fn cancel_run(&self, run_id: &str) -> bool {
        match self.current_run().await {
            Some(run) if run.id() == run_id => run.cancel().await,
            _ => false,
        }
    }

    pub(super) async fn current_run(&self) -> Option<crate::run::RunHandle> {
        let original = self.owned_original().ok()?;
        original
            .correlated_current_handle(&self.session_id, &self.admission)
            .await
    }
}

pub(super) struct BlockingRunLifecycle {
    coordinator: ExecutionCoordinator,
    persistence: Option<SessionPersistenceContext>,
    cleanup: RunCleanupState,
    /// Owned runtime event collector. Owning the join handle here makes it a
    /// compile error to settle a blocking Run without joining its collector;
    /// a dropped-by-accident handle is exactly the detached task the
    /// coordinator is meant to prevent.
    collector: Option<JoinHandle<()>>,
}

impl BlockingRunLifecycle {
    pub(super) fn from_session(
        session: &AgentSession,
        coordinator: ExecutionCoordinator,
        persistence: Option<SessionPersistenceContext>,
    ) -> Self {
        Self {
            cleanup: RunCleanupState::from_session(session, coordinator.run_id()),
            coordinator,
            persistence,
            collector: None,
        }
    }

    pub(super) fn with_collector(mut self, collector: JoinHandle<()>) -> Self {
        self.collector = Some(collector);
        self
    }

    pub(super) async fn set_cancel_token(&self, token: tokio_util::sync::CancellationToken) {
        self.cleanup.set_cancel_token(token).await;
    }

    pub(super) async fn complete<E>(
        mut self,
        result: std::result::Result<AgentResult, E>,
    ) -> Result<AgentResult>
    where
        E: std::fmt::Display + Into<CodeError>,
    {
        let terminal = self.coordinator.terminal_for(
            result.is_ok(),
            result.as_ref().err().map(ToString::to_string),
        );
        self.cleanup.clear_cancel_token().await;
        if let Some(collector) = self.collector.as_mut() {
            let _ = collector.await;
        }

        // The run reached a terminal state in-process — its loop checkpoint
        // is dead weight. Only a process crash (this code never runs) should
        // leave a checkpoint for crash-recovery resume.
        if let Some(persistence) = &self.persistence {
            persistence
                .clear_loop_checkpoint(self.cleanup.run_id())
                .await;
        }

        match result {
            Ok(result) => {
                if let Some(persistence) = &self.persistence {
                    persistence.record_result(&result);
                    persistence.auto_save_if_enabled().await;
                }
                self.coordinator.settle_terminal(terminal).await;
                self.cleanup.finish().await;
                Ok(result)
            }
            Err(error) => {
                self.coordinator.settle_terminal(terminal).await;
                self.cleanup.finish().await;
                Err(error.into())
            }
        }
    }
}

pub(super) struct StreamRunLifecycle {
    coordinator: ExecutionCoordinator,
    persistence: Option<SessionPersistenceContext>,
    should_auto_save: Arc<std::sync::atomic::AtomicBool>,
    cleanup: RunCleanupState,
}

impl StreamRunLifecycle {
    pub(super) fn from_session(
        session: &AgentSession,
        coordinator: ExecutionCoordinator,
        persistence: Option<SessionPersistenceContext>,
    ) -> Self {
        Self {
            cleanup: RunCleanupState::from_session(session, coordinator.run_id()),
            coordinator,
            persistence,
            should_auto_save: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub(super) async fn set_cancel_token(&self, token: tokio_util::sync::CancellationToken) {
        self.cleanup.set_cancel_token(token).await;
    }

    pub(super) fn worker_state(&self) -> StreamRunWorkerState {
        StreamRunWorkerState {
            coordinator: self.coordinator.clone(),
            persistence: self.persistence.clone(),
            should_auto_save: Arc::clone(&self.should_auto_save),
        }
    }

    pub(super) fn wrap(
        self,
        worker: JoinHandle<()>,
        forwarder: JoinHandle<()>,
    ) -> (JoinHandle<()>, Vec<AbortHandle>) {
        let worker_aborts = vec![worker.abort_handle(), forwarder.abort_handle()];
        let lifecycle = tokio::spawn(async move {
            let _ = worker.await;
            let _ = forwarder.await;
            if self
                .should_auto_save
                .load(std::sync::atomic::Ordering::Acquire)
            {
                if let Some(persistence) = &self.persistence {
                    persistence.auto_save_if_enabled().await;
                }
            }
            // Stream run reached a terminal state in-process (worker +
            // forwarder both joined) — drop its loop checkpoint. Only a
            // crash (this task never completes) leaves one for resume.
            if let Some(persistence) = &self.persistence {
                persistence
                    .clear_loop_checkpoint(self.cleanup.run_id())
                    .await;
            }
            // A worker that panicked or aborted before settling must not
            // leave a non-terminal Run behind released cleanup.
            self.coordinator.ensure_settled_after_join().await;
            self.cleanup.clear_cancel_token().await;
            self.cleanup.finish().await;
        });
        (lifecycle, worker_aborts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn prepared() -> (
        tempfile::TempDir,
        super::super::Agent,
        RunControlState,
        super::super::run_admission::OriginalWorkerHandoff,
        super::super::run_admission::OwnedWorkerStart,
        Arc<super::super::run_admission::OriginalExecutionContext>,
        tokio_util::sync::CancellationToken,
    ) {
        let workspace = tempfile::tempdir().unwrap();
        let agent = super::super::Agent::from_config(super::super::test_config())
            .await
            .unwrap();
        let options = super::super::SessionOptions::new().with_session_id("session-1");
        let client = Arc::new(super::super::tests::StaticStreamingClient::new(
            "unused actual fixture",
        ));
        let session = agent
            .build_session(workspace.path().display().to_string(), client, &options)
            .unwrap();
        let lease = ExecutionCoordinator::admit(&session, "actual lifecycle fixture")
            .await
            .unwrap();
        let token = session.session_cancel.child_token();
        let (handoff, start, original) = ExecutionCoordinator::prepare_original_handoff(
            &session,
            lease,
            super::super::run_admission::ExecutionMode::Blocking,
            token.clone(),
        )
        .unwrap();
        let control = RunControlState::from_session(&session)
            .bind_original(original.clone())
            .unwrap();
        (workspace, agent, control, handoff, start, original, token)
    }

    #[tokio::test]
    async fn start_run_sets_current_run() {
        let (_workspace, _agent, control, handoff, start, original, token) = prepared().await;
        let worker = tokio::spawn(async move {
            assert!(start.wait().await.unwrap());
            let run = control.start_run("hello").await;
            original.validate_original_token(&token).unwrap();
            let inbox = crate::run_control::RunControlInbox::new("session-1", run.id(), token);
            original.bind_inbox_once(inbox.clone()).unwrap();
            control
                .attach_owned_run_control(run.id(), inbox)
                .await
                .unwrap();
            assert_eq!(control.current_run().await.unwrap().id(), run.id());
            assert_eq!(
                control.run_store.snapshot(run.id()).await.unwrap().prompt,
                "hello"
            );
        });
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _proxy = handoff.supervise(worker, Vec::new(), tx);
        assert_eq!(
            rx.await.unwrap(),
            super::super::run_admission::OwnerDisposition::RetainedUnproved
        );
    }

    #[tokio::test]
    async fn cancel_without_token_is_noop() {
        let (_workspace, _agent, control, handoff, start, _original, _token) = prepared().await;
        let worker = tokio::spawn(async move {
            assert!(start.wait().await.unwrap());
            let run = control.start_run("hello").await;
            // Actual Created precedes inbox/slot install: no current cancellation token.
            assert!(!control.cancel().await);
            assert_ne!(
                control.run_store.snapshot(run.id()).await.unwrap().status,
                crate::run::RunStatus::Cancelled
            );
        });
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _proxy = handoff.supervise(worker, Vec::new(), tx);
        assert_eq!(
            rx.await.unwrap(),
            super::super::run_admission::OwnerDisposition::RetainedUnproved
        );
    }
}
