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
pub(super) struct RunControlState {
    session_id: String,
    run_store: Arc<crate::run::InMemoryRunStore>,
    cancel_token: Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    current_run_id: Arc<tokio::sync::Mutex<Option<String>>>,
    active_run_control: Arc<tokio::sync::Mutex<Option<Arc<crate::run_control::RunControlInbox>>>>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    hook_executor: Option<Arc<dyn crate::hooks::HookExecutor>>,
    host_env: Arc<crate::host_env::HostEnv>,
}

impl RunControlState {
    pub(super) fn from_session(session: &AgentSession) -> Self {
        Self {
            session_id: session.session_id.clone(),
            run_store: Arc::clone(&session.run_store),
            cancel_token: Arc::clone(&session.cancel_token),
            current_run_id: Arc::clone(&session.current_run_id),
            active_run_control: Arc::clone(&session.active_run_control),
            closed: Arc::clone(&session.closed),
            hook_executor: session.hook_executor.clone(),
            host_env: Arc::clone(&session.config.host_env),
        }
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
        let run_id = self.current_run_id.lock().await.clone()?;
        let control = self.active_run_control.lock().await.clone()?;
        (control.snapshot_run_id() == run_id).then_some(control)
    }

    #[cfg(test)]
    pub(super) async fn start_run(&self, prompt: &str) -> crate::run::RunHandle {
        let id = format!("run-{}", self.host_env.next_id());
        let snapshot = self
            .run_store
            .create_run_with_id(id, &self.session_id, prompt)
            .await;
        *self.current_run_id.lock().await = Some(snapshot.id.clone());
        self.run_handle(snapshot.id, self.session_id.clone())
    }

    pub(super) async fn start_run_with_bindings(
        &self,
        prompt: &str,
        cognitive_binding: Option<crate::cognitive_context::CognitivePackageBindingV1>,
        capability_binding: crate::capability::RunCapabilityBindingV1,
    ) -> Result<crate::run::RunHandle> {
        // Honor the session's host-provided IdGenerator so deterministic
        // replay tooling can pin run ids alongside session_id.
        let id = format!("run-{}", self.host_env.next_id());
        let reservation = self
            .run_store
            .reserve_run_with_id(id.clone(), &self.session_id, prompt)
            .await;
        if reservation.replayed() {
            return Err(CodeError::RunIdentityConflict { run_id: id });
        }
        let snapshot = reservation.snapshot().clone();
        if let Err(error) = self
            .bind_capability_generation(&snapshot.id, capability_binding)
            .await
        {
            let _ = self
                .run_store
                .mark_failed(&snapshot.id, error.to_string())
                .await;
            return Err(error);
        }
        if let Some(binding) = cognitive_binding {
            if let Err(error) = self.bind_cognitive_package(&snapshot.id, binding).await {
                let _ = self
                    .run_store
                    .mark_failed(&snapshot.id, error.to_string())
                    .await;
                return Err(error);
            }
        }
        *self.current_run_id.lock().await = Some(snapshot.id.clone());
        Ok(self.run_handle(snapshot.id, self.session_id.clone()))
    }

    pub(super) async fn reserve_run_with_id(
        &self,
        run_id: &str,
        prompt: &str,
    ) -> Result<crate::run::RunReservation> {
        if run_id.trim().is_empty() || run_id.contains('\0') || run_id.contains(['\r', '\n']) {
            return Err(CodeError::RunIdentityConflict {
                run_id: run_id.to_string(),
            });
        }
        let reservation = self
            .run_store
            .reserve_run_with_id(run_id.to_string(), &self.session_id, prompt)
            .await;
        let snapshot = reservation.snapshot();
        if snapshot.session_id != self.session_id || snapshot.prompt != prompt {
            return Err(CodeError::RunIdentityConflict {
                run_id: run_id.to_string(),
            });
        }
        if !reservation.replayed() {
            *self.current_run_id.lock().await = Some(run_id.to_string());
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

    /// Settle a newly reserved exact Run when admission fails before its
    /// runtime lifecycle exists. This prevents a failed capability lease from
    /// leaving a permanent `Created` record or a stale current-run pointer.
    pub(super) async fn fail_reserved_run_start(&self, run_id: &str, error: &CodeError) {
        let cancelled = matches!(
            error,
            CodeError::SessionClosed { .. }
                | CodeError::Capability(
                    crate::capability::CapabilityRuntimeError::Cancelled
                        | crate::capability::CapabilityRuntimeError::SessionClosed
                )
        );
        if cancelled {
            let _ = self.run_store.mark_cancelled(run_id).await;
            if let Some(executor) = &self.hook_executor {
                executor
                    .record_run_cancelled(
                        run_id,
                        &self.session_id,
                        Some("cancelled during Run admission"),
                    )
                    .await;
            }
        } else {
            let _ = self.run_store.mark_failed(run_id, error.to_string()).await;
        }

        let mut current = self.current_run_id.lock().await;
        if current.as_deref() == Some(run_id) {
            *current = None;
        }
    }

    pub(super) async fn cancel(&self) -> bool {
        let token = self.cancel_token.lock().await.clone();
        if let Some(token) = token {
            token.cancel();
            if let Some(run_id) = self.current_run_id.lock().await.clone() {
                let _ = self.run_store.mark_cancelled(&run_id).await;
                if let Some(executor) = &self.hook_executor {
                    executor
                        .record_run_cancelled(&run_id, &self.session_id, Some("cancelled by host"))
                        .await;
                }
            }
            tracing::info!(session_id = %self.session_id, "Cancelled ongoing operation");
            true
        } else {
            tracing::debug!(session_id = %self.session_id, "No ongoing operation to cancel");
            false
        }
    }

    pub(super) async fn cancel_run(&self, run_id: &str) -> bool {
        match self.current_run().await {
            Some(run) if run.id() == run_id => run.cancel().await,
            _ => false,
        }
    }

    pub(super) async fn current_run(&self) -> Option<crate::run::RunHandle> {
        let run_id = self.current_run_id.lock().await.clone()?;
        let snapshot = self.run_store.snapshot(&run_id).await?;
        Some(self.run_handle(snapshot.id, snapshot.session_id))
    }

    fn run_handle(&self, run_id: String, session_id: String) -> crate::run::RunHandle {
        crate::run::RunHandle::new(
            run_id,
            session_id,
            Arc::clone(&self.run_store),
            Arc::clone(&self.cancel_token),
            Arc::clone(&self.current_run_id),
            self.hook_executor.clone(),
        )
    }
}

pub(super) struct BlockingRunLifecycle {
    coordinator: ExecutionCoordinator,
    persistence: Option<SessionPersistenceContext>,
    cleanup: RunCleanupState,
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
        }
    }

    pub(super) async fn set_cancel_token(&self, token: tokio_util::sync::CancellationToken) {
        self.cleanup.set_cancel_token(token).await;
    }

    pub(super) async fn complete<E>(
        self,
        runtime_collector: JoinHandle<()>,
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
        let _ = runtime_collector.await;

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
            self.cleanup.clear_cancel_token().await;
            self.cleanup.finish().await;
        });
        (lifecycle, worker_aborts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_control() -> RunControlState {
        RunControlState {
            session_id: "session-1".to_string(),
            run_store: Arc::new(crate::run::InMemoryRunStore::new()),
            cancel_token: Arc::new(tokio::sync::Mutex::new(None)),
            current_run_id: Arc::new(tokio::sync::Mutex::new(None)),
            active_run_control: Arc::new(tokio::sync::Mutex::new(None)),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            hook_executor: None,
            host_env: Arc::new(crate::host_env::HostEnv::system()),
        }
    }

    #[tokio::test]
    async fn start_run_sets_current_run() {
        let control = run_control();
        let run = control.start_run("hello").await;

        assert_eq!(control.current_run().await.unwrap().id(), run.id());
        assert_eq!(
            control.run_store.snapshot(run.id()).await.unwrap().prompt,
            "hello"
        );
    }

    #[tokio::test]
    async fn cancel_without_token_is_noop() {
        let control = run_control();
        let run = control.start_run("hello").await;

        assert!(!control.cancel().await);
        assert_ne!(
            control.run_store.snapshot(run.id()).await.unwrap().status,
            crate::run::RunStatus::Cancelled
        );
    }
}
