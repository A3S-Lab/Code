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
    stream_abort: Mutex<Option<AbortHandle>>,
    idle: Notify,
}

impl Default for RunAdmission {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(false),
            stream_abort: Mutex::new(None),
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
        })
    }

    fn register_stream_worker(&self, abort: AbortHandle) {
        *self
            .stream_abort
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(abort);
    }

    fn clear_stream_worker(&self) {
        self.stream_abort
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
    }

    /// Abort only the real streaming worker registered for the current lease.
    /// Blocking sends have no registered worker and remain cooperative-only.
    pub(super) fn abort_stream_worker(&self) -> bool {
        let abort = self
            .stream_abort
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        if let Some(abort) = abort {
            abort.abort();
            true
        } else {
            false
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
    lease: RunAdmissionLease,
) -> JoinHandle<()> {
    lease
        .admission
        .register_stream_worker(handle.abort_handle());
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = handle.await;
        lease.admission.clear_stream_worker();
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

        let public_handle = guard_stream_handle(inner, lease);
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
        let inner = tokio::spawn(std::future::pending::<()>());
        let proxy = guard_stream_handle(inner, lease);

        assert!(admission.abort_stream_worker());
        assert!(admission.wait_until_idle(Duration::from_secs(1)).await);
        assert!(proxy.await.is_ok());
        assert!(admission.try_acquire("session-1").is_ok());
    }
}
