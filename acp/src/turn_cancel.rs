//! Stop returns the prompt while a tool poll is still on a pool thread.
//!
//! The public stream join is only a waiter. Aborting it does not stop the
//! fact worker. The turn lock stays with that join until the worker settles
//! or [`CANCEL_JOIN_HOLD`] elapses.

use std::time::Duration;

use tokio::sync::OwnedMutexGuard;
use tokio::task::JoinHandle;

/// How long a cancelled turn keeps reading events so a waiting tool can
/// append `tool.result` `cancelled` before the worker is aborted.
pub(crate) const CANCEL_EVENT_DRAIN: Duration = Duration::from_millis(150);

/// Cooperative wait inside `cancel_and_settle` before the inner worker is aborted.
pub(crate) const CANCEL_SETTLE_GRACE: Duration = Duration::from_millis(50);

/// How long the inner worker may take to drop its lease after abort.
pub(crate) const CANCEL_ABORT_GRACE: Duration = Duration::from_millis(50);

/// Upper bound on holding the turn lock for a public join that does not finish.
pub(crate) const CANCEL_JOIN_HOLD: Duration = Duration::from_secs(30);

/// `cancel` records one generation. A later turn uses the next number.
pub(crate) fn generation_was_cancelled(recorded: Option<u64>, generation: u64) -> bool {
    recorded == Some(generation)
}

/// Hold `permit` until `join` finishes, or until [`CANCEL_JOIN_HOLD`].
///
/// Dropping the public join does not abort the fact worker. The lock is what
/// keeps the next prompt from writing the same fact log.
pub(crate) fn detach_turn_permit(permit: OwnedMutexGuard<()>, mut join: JoinHandle<()>) {
    tokio::spawn(async move {
        let _permit = permit;
        tokio::select! {
            biased;
            result = &mut join => {
                let _ = result;
            }
            _ = tokio::time::sleep(CANCEL_JOIN_HOLD) => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn cancel_matches_only_the_recorded_generation() {
        assert!(generation_was_cancelled(Some(4), 4));
        assert!(!generation_was_cancelled(Some(3), 4));
        assert!(!generation_was_cancelled(None, 4));
    }

    #[tokio::test]
    async fn detached_permit_blocks_until_the_join_ends() {
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let permit = Arc::clone(&lock).lock_owned().await;
        let (release, wait) = tokio::sync::oneshot::channel();
        let join = tokio::spawn(async move {
            let _ = wait.await;
        });
        detach_turn_permit(permit, join);
        tokio::task::yield_now().await;
        let held = tokio::time::timeout(Duration::from_millis(40), lock.lock()).await;
        assert!(
            held.is_err(),
            "turn lock must stay held while the join runs"
        );
        let _ = release.send(());
        let _guard = tokio::time::timeout(Duration::from_secs(2), lock.lock())
            .await
            .expect("turn lock releases when the join finishes");
        drop(_guard);
    }

    /// A `tokio::spawn` poll that never yields must not freeze the `LocalSet`
    /// that reads `session/cancel`.
    #[test]
    fn multi_thread_local_set_observes_cancel_while_spawned_work_blocks() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let local = tokio::task::LocalSet::new();
        let elapsed = local.block_on(&runtime, async {
            tokio::spawn(async {
                std::thread::sleep(Duration::from_millis(800));
            });
            let notify = Arc::new(tokio::sync::Notify::new());
            let waiter = notify.notified();
            let wake = Arc::clone(&notify);
            tokio::task::spawn_local(async move {
                tokio::time::sleep(Duration::from_millis(40)).await;
                wake.notify_waiters();
            });
            let started = Instant::now();
            waiter.await;
            started.elapsed()
        });
        assert!(
            elapsed < Duration::from_millis(400),
            "local set stayed blocked for {elapsed:?}"
        );
    }
}
