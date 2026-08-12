//! Fail-fast admission control for session conversation operations.
//!
//! A session owns one mutable transcript and one current-run slot. Admission is
//! therefore intentionally single-flight: callers receive `SessionBusy`
//! immediately instead of waiting for another operation to release the session.

use crate::error::{CodeError, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::{AbortHandle, JoinHandle};

#[derive(Debug)]
pub(super) struct RunAdmission {
    active: AtomicBool,
    stream_aborts: Mutex<Vec<AbortHandle>>,
    idle: Notify,
}

impl Default for RunAdmission {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(false),
            stream_aborts: Mutex::new(Vec::new()),
            idle: Notify::new(),
        }
    }
}

impl RunAdmission {
    pub(super) fn try_acquire(self: &Arc<Self>, session_id: &str) -> Result<RunAdmissionLease> {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| CodeError::SessionBusy {
                session_id: session_id.to_string(),
            })?;

        Ok(RunAdmissionLease {
            admission: Arc::clone(self),
            task_lease: None,
        })
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
}

impl RunAdmissionLease {
    pub(super) fn attach_task_lease(
        mut self,
        task_lease: crate::task_scheduler::TaskLease,
    ) -> Self {
        self.task_lease = Some(task_lease);
        self
    }
}

impl Drop for RunAdmissionLease {
    fn drop(&mut self) {
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
    lease.admission.register_stream_workers(worker_aborts);
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = handle.await;
        lease.admission.clear_stream_workers();
        drop(lease);
        let _ = finished_tx.send(());
    });

    tokio::spawn(async move {
        let _ = finished_rx.await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_rejects_overlap_and_releases_on_drop() {
        let admission = Arc::new(RunAdmission::default());
        let lease = admission.try_acquire("session-1").unwrap();

        assert!(matches!(
            admission.try_acquire("session-1"),
            Err(CodeError::SessionBusy { ref session_id }) if session_id == "session-1"
        ));

        drop(lease);
        assert!(admission.try_acquire("session-1").is_ok());
    }

    #[tokio::test]
    async fn stream_guard_survives_public_handle_abort() {
        let admission = Arc::new(RunAdmission::default());
        let lease = admission.try_acquire("session-1").unwrap();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let inner = tokio::spawn(async move {
            let _ = release_rx.await;
        });
        let inner_abort = inner.abort_handle();

        let public_handle = guard_stream_handle(inner, vec![inner_abort], lease);
        public_handle.abort();
        tokio::task::yield_now().await;

        assert!(matches!(
            admission.try_acquire("session-1"),
            Err(CodeError::SessionBusy { .. })
        ));

        let _ = release_tx.send(());
        let mut released = None;
        for _ in 0..50 {
            match admission.try_acquire("session-1") {
                Ok(lease) => {
                    released = Some(lease);
                    break;
                }
                Err(CodeError::SessionBusy { .. }) => tokio::task::yield_now().await,
                Err(other) => panic!("unexpected admission error: {other:?}"),
            }
        }
        assert!(
            released.is_some(),
            "guardian must release after inner cleanup"
        );
    }

    #[tokio::test]
    async fn registered_stream_worker_can_be_force_settled() {
        let admission = Arc::new(RunAdmission::default());
        let lease = admission.try_acquire("session-1").unwrap();
        let worker = tokio::spawn(std::future::pending::<()>());
        let worker_abort = worker.abort_handle();
        let (cleanup_tx, cleanup_rx) = tokio::sync::oneshot::channel();
        let supervisor = tokio::spawn(async move {
            let error = worker.await.expect_err("worker must be aborted");
            assert!(error.is_cancelled());
            let _ = cleanup_tx.send(());
        });
        let proxy = guard_stream_handle(supervisor, vec![worker_abort], lease);

        assert!(admission.abort_stream_workers());
        cleanup_rx
            .await
            .expect("the lifecycle supervisor must survive worker abortion");
        assert!(admission.wait_until_idle(Duration::from_secs(1)).await);
        assert!(proxy.await.is_ok());
        assert!(admission.try_acquire("session-1").is_ok());
    }
}
