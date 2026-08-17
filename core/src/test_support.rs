//! Shared coordination for resource-intensive unit and integration tests.

#[cfg(windows)]
use std::sync::OnceLock;
use std::time::Duration;
#[cfg(windows)]
use tokio::sync::{Semaphore, SemaphorePermit};

#[cfg(windows)]
const RESOURCE_INTENSIVE_TEST_CONCURRENCY: usize = 1;

/// Bound tests that create filesystem watchers or child language-server
/// processes. Libtest otherwise runs one Tokio runtime per test across every
/// logical CPU, which can exhaust Windows process and watcher capacity before
/// the behavior under test receives a scheduling turn.
#[cfg(windows)]
pub(crate) async fn resource_intensive_test_permit() -> SemaphorePermit<'static> {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    PERMITS
        .get_or_init(|| Semaphore::new(RESOURCE_INTENSIVE_TEST_CONCURRENCY))
        .acquire()
        .await
        .expect("resource-intensive test semaphore must remain open")
}

/// Other supported hosts do not require the Windows resource gate.
#[cfg(not(windows))]
pub(crate) async fn resource_intensive_test_permit() {}

/// Allow OS-backed test resources to start under a busy shared Windows host
/// without relaxing behavioral cancellation or shutdown deadlines.
pub(crate) const fn external_resource_start_timeout(default: Duration) -> Duration {
    #[cfg(windows)]
    {
        if default.as_secs() < 30 {
            return Duration::from_secs(30);
        }
    }
    default
}
