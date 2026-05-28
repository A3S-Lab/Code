//! Run lifecycle control.
//!
//! This module owns how runs are started, cancelled, completed, failed, and
//! cleaned up. Execution contexts can call a small lifecycle interface without
//! knowing how run handles, current-run state, persistence, and cleanup interact.

use super::{
    runtime_events::RunCleanupState, session_persistence::SessionPersistenceContext, AgentSession,
};
use crate::agent::AgentResult;
use crate::error::{CodeError, Result};
use std::sync::Arc;
use tokio::task::JoinHandle;

#[derive(Clone)]
pub(super) struct StreamRunWorkerState {
    run_store: Arc<crate::run::InMemoryRunStore>,
    run_id: String,
    persistence: Option<SessionPersistenceContext>,
    should_auto_save: Arc<std::sync::atomic::AtomicBool>,
    /// Shared per-run cancel token slot (populated by lifecycle's
    /// `set_cancel_token`). Used to classify a failed run as `Cancelled`
    /// when the token was fired (e.g., by `session_cancel.cancel()`).
    cancel_token: Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
}

impl StreamRunWorkerState {
    pub(super) async fn complete<E>(&self, result: std::result::Result<AgentResult, E>)
    where
        E: std::fmt::Display,
    {
        match result {
            Ok(result) => {
                if let Some(persistence) = &self.persistence {
                    persistence.record_result(&result);
                    self.should_auto_save
                        .store(true, std::sync::atomic::Ordering::Release);
                }
            }
            Err(error) => {
                let cancelled = self
                    .cancel_token
                    .lock()
                    .await
                    .as_ref()
                    .map(|t| t.is_cancelled())
                    .unwrap_or(false);
                if cancelled {
                    let _ = self.run_store.mark_cancelled(&self.run_id).await;
                } else {
                    let error_message = error.to_string();
                    let _ = self
                        .run_store
                        .mark_failed(&self.run_id, error_message)
                        .await;
                }
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct RunControlState {
    session_id: String,
    run_store: Arc<crate::run::InMemoryRunStore>,
    cancel_token: Arc<tokio::sync::Mutex<Option<tokio_util::sync::CancellationToken>>>,
    current_run_id: Arc<tokio::sync::Mutex<Option<String>>>,
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
            hook_executor: session.ahp_executor.clone(),
            host_env: Arc::clone(&session.config.host_env),
        }
    }

    pub(super) async fn start_run(&self, prompt: &str) -> crate::run::RunHandle {
        // Honor the session's host-provided IdGenerator so deterministic
        // replay tooling can pin run ids alongside session_id.
        let id = format!("run-{}", self.host_env.next_id());
        let snapshot = self
            .run_store
            .create_run_with_id(id, &self.session_id, prompt)
            .await;
        *self.current_run_id.lock().await = Some(snapshot.id.clone());
        self.run_handle(snapshot.id, self.session_id.clone())
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
    run_store: Arc<crate::run::InMemoryRunStore>,
    persistence: Option<SessionPersistenceContext>,
    cleanup: RunCleanupState,
}

impl BlockingRunLifecycle {
    pub(super) fn from_session(
        session: &AgentSession,
        run_id: &str,
        persistence: Option<SessionPersistenceContext>,
    ) -> Self {
        Self {
            run_store: Arc::clone(&session.run_store),
            persistence,
            cleanup: RunCleanupState::from_session(session, run_id),
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
        // Sample the cancellation flag *before* clearing the token so we can
        // distinguish cancellation-driven errors from genuine failures.
        let cancelled = self.cleanup.was_cancelled().await;
        self.cleanup.clear_cancel_token().await;
        let _ = runtime_collector.await;

        match result {
            Ok(result) => {
                if let Some(persistence) = &self.persistence {
                    persistence.record_result(&result);
                    persistence.auto_save_if_enabled().await;
                }
                self.cleanup.finish().await;
                Ok(result)
            }
            Err(error) => {
                if cancelled {
                    let _ = self.run_store.mark_cancelled(self.cleanup.run_id()).await;
                } else {
                    let error_message = error.to_string();
                    let _ = self
                        .run_store
                        .mark_failed(self.cleanup.run_id(), error_message)
                        .await;
                }
                self.cleanup.finish().await;
                Err(error.into())
            }
        }
    }
}

pub(super) struct StreamRunLifecycle {
    run_store: Arc<crate::run::InMemoryRunStore>,
    persistence: Option<SessionPersistenceContext>,
    should_auto_save: Arc<std::sync::atomic::AtomicBool>,
    cleanup: RunCleanupState,
}

impl StreamRunLifecycle {
    pub(super) fn from_session(
        session: &AgentSession,
        run_id: &str,
        persistence: Option<SessionPersistenceContext>,
    ) -> Self {
        Self {
            run_store: Arc::clone(&session.run_store),
            persistence,
            should_auto_save: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cleanup: RunCleanupState::from_session(session, run_id),
        }
    }

    pub(super) async fn set_cancel_token(&self, token: tokio_util::sync::CancellationToken) {
        self.cleanup.set_cancel_token(token).await;
    }

    pub(super) fn worker_state(&self) -> StreamRunWorkerState {
        StreamRunWorkerState {
            run_store: Arc::clone(&self.run_store),
            run_id: self.cleanup.run_id().to_string(),
            persistence: self.persistence.clone(),
            should_auto_save: Arc::clone(&self.should_auto_save),
            cancel_token: self.cleanup.cancel_token_slot(),
        }
    }

    pub(super) fn wrap(self, worker: JoinHandle<()>, forwarder: JoinHandle<()>) -> JoinHandle<()> {
        tokio::spawn(async move {
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
            self.cleanup.clear_cancel_token().await;
            self.cleanup.finish().await;
        })
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
