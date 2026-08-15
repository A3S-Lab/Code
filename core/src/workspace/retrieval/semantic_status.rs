use super::{WorkspaceRetrievalError, WorkspaceRetrievalPhase, WorkspaceRetrievalStatus};
use std::sync::RwLock;
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Event-driven status cell for one session-owned semantic projection.
pub(super) struct SemanticStatusCell {
    value: RwLock<WorkspaceRetrievalStatus>,
    changed: Notify,
}

impl SemanticStatusCell {
    pub(super) fn new(status: WorkspaceRetrievalStatus) -> Self {
        Self {
            value: RwLock::new(status),
            changed: Notify::new(),
        }
    }

    pub(super) fn load(&self) -> WorkspaceRetrievalStatus {
        self.value
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(super) fn publish(&self, status: WorkspaceRetrievalStatus) {
        *self
            .value
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = status;
        self.changed.notify_waiters();
    }

    /// Wait for the current catalog generation to become complete or degraded.
    /// A zero timeout preserves the non-blocking partial-result behavior.
    pub(super) async fn wait_for_readiness(
        &self,
        timeout: Duration,
        runtime_cancellation: &CancellationToken,
        caller_cancellation: &CancellationToken,
    ) -> Result<WorkspaceRetrievalStatus, WorkspaceRetrievalError> {
        if timeout.is_zero() {
            return Ok(self.load());
        }
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        loop {
            // Register before observing the value so a publication between the
            // observation and select cannot be lost.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let status = self.load();
            if status.phase != WorkspaceRetrievalPhase::Building {
                return Ok(status);
            }
            tokio::select! {
                biased;
                _ = caller_cancellation.cancelled() => {
                    return Err(WorkspaceRetrievalError::Cancelled);
                }
                _ = runtime_cancellation.cancelled() => {
                    return Err(WorkspaceRetrievalError::Cancelled);
                }
                _ = &mut deadline => return Ok(self.load()),
                _ = &mut changed => {}
            }
        }
    }
}
