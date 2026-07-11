//! Fail-fast admission control for session conversation operations.
//!
//! A session owns one mutable transcript and one current-run slot. Admission is
//! therefore intentionally single-flight: callers receive `SessionBusy`
//! immediately instead of waiting for another operation to release the session.

use crate::error::{CodeError, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::task::JoinHandle;

#[derive(Debug, Default)]
pub(super) struct RunAdmission {
    active: AtomicBool,
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
}

/// RAII lease for one admitted session operation.
pub(super) struct RunAdmissionLease {
    admission: Arc<RunAdmission>,
}

impl Drop for RunAdmissionLease {
    fn drop(&mut self) {
        self.admission.active.store(false, Ordering::Release);
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
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = handle.await;
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
}
