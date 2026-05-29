//! Out-of-band session close handle.
//!
//! `SessionCloseHandle` is an `Arc`-shareable substruct that owns just the
//! fields needed to terminate an `AgentSession` from outside (typically from
//! the parent [`Agent`](super::Agent)'s session registry).
//!
//! `AgentSession` carries one of these via `Arc<SessionCloseHandle>`; the
//! parent `Agent` stores a `Weak<SessionCloseHandle>` in its registry. When
//! the user drops the session, the handle drops too and the registry's
//! `Weak` becomes dangling — pruned on the next `list_sessions()` /
//! `close_session()` call.
//!
//! Sharing the close mechanics through a single `close()` method on this
//! struct guarantees `AgentSession::close()` and `Agent::close_session(id)`
//! perform exactly the same cleanup.

use crate::hitl::ConfirmationProvider;
use crate::hooks::HookExecutor;
use crate::run::InMemoryRunStore;
use crate::subagent_task_tracker::{InMemorySubagentTaskTracker, SubagentStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Bundle of `Arc`-shared session state needed to perform a graceful close
/// from anywhere holding (a clone of) the handle.
pub(crate) struct SessionCloseHandle {
    pub(crate) session_id: String,
    /// Tripped on first `close()` call; subsequent calls become no-ops and
    /// `AgentSession::send`/`stream` fast-fail.
    pub(crate) closed: Arc<AtomicBool>,
    /// Session-level parent token. All in-flight run/subagent tokens are
    /// `child_token()` of this.
    pub(crate) session_cancel: CancellationToken,
    /// Per-run cancel-token slot (currently active run's token, if any).
    /// Populated by the run lifecycle.
    pub(crate) cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    /// Current run id (matches `cancel_token` when set).
    pub(crate) current_run_id: Arc<Mutex<Option<String>>>,
    pub(crate) run_store: Arc<InMemoryRunStore>,
    pub(crate) subagent_tasks: Arc<InMemorySubagentTaskTracker>,
    pub(crate) confirmation_manager: Option<Arc<dyn ConfirmationProvider>>,
    pub(crate) hook_executor: Option<Arc<dyn HookExecutor>>,
}

impl SessionCloseHandle {
    /// Return whether `close()` has already been called.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Perform the full session close sequence. Idempotent: subsequent calls
    /// are no-ops and are guaranteed not to panic.
    ///
    /// Sequence (see [`AgentSession::close`](super::AgentSession::close)
    /// for the public-facing contract):
    /// 1. Flip the `closed` flag so further `send`/`stream` fast-fail;
    /// 2. Fire the session-level cancellation token so every derived run
    ///    and subagent task token fires;
    /// 3. Mark the active run as `Cancelled` in the run store and emit the
    ///    AHP `record_run_cancelled` hook;
    /// 4. Mark every still-running delegated subagent task as `Cancelled`
    ///    in the tracker;
    /// 5. Cancel pending HITL tool confirmations so blocked tool callers
    ///    receive a rejection instead of hanging.
    pub(crate) async fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }

        // 1. Fire the session-level token so children cascade.
        self.session_cancel.cancel();

        // 2. Mark the active run cancelled and fire AHP hook bookkeeping.
        //    The per-run token has already fired via step 1; this loop
        //    just updates the run store and emits the hook event.
        let had_active_token = self.cancel_token.lock().await.is_some();
        if had_active_token {
            if let Some(run_id) = self.current_run_id.lock().await.clone() {
                let _ = self.run_store.mark_cancelled(&run_id).await;
                if let Some(hook) = &self.hook_executor {
                    hook.record_run_cancelled(&run_id, &self.session_id, Some("cancelled by host"))
                        .await;
                }
            }
        }

        // 3. Mark every still-running subagent task cancelled.
        let pending: Vec<String> = self
            .subagent_tasks
            .list_for_parent(&self.session_id)
            .await
            .into_iter()
            .filter(|task| task.status == SubagentStatus::Running)
            .map(|task| task.task_id)
            .collect();
        for task_id in pending {
            let _ = self.subagent_tasks.cancel(&task_id).await;
        }

        // 4. Cancel pending HITL confirmations.
        if let Some(manager) = &self.confirmation_manager {
            let _ = manager.cancel_all().await;
        }

        tracing::info!(session_id = %self.session_id, "AgentSession closed");
    }
}
