//! Shared coordination for resource-intensive unit and integration tests.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
#[cfg(any(windows, target_os = "macos"))]
use tokio::sync::{Semaphore, SemaphorePermit};

#[cfg(any(windows, target_os = "macos"))]
const RESOURCE_INTENSIVE_TEST_CONCURRENCY: usize = 1;

/// Bound tests that create filesystem watchers or child language-server
/// processes. Libtest otherwise runs one Tokio runtime per test across every
/// logical CPU, which can exhaust process and watcher capacity on macOS and
/// Windows before the behavior under test receives a scheduling turn.
#[cfg(any(windows, target_os = "macos"))]
pub(crate) async fn resource_intensive_test_permit() -> SemaphorePermit<'static> {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    PERMITS
        .get_or_init(|| Semaphore::new(RESOURCE_INTENSIVE_TEST_CONCURRENCY))
        .acquire()
        .await
        .expect("resource-intensive test semaphore must remain open")
}

/// Other supported hosts do not require the macOS/Windows resource gate.
#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) async fn resource_intensive_test_permit() {}

/// A small non-git directory that stays alive for the test process.
///
/// Hermetic turns must not observe `/tmp`: that walk hits the file cap and the
/// completion gate correctly treats the observation as incomplete.
pub(crate) fn hermetic_workspace() -> PathBuf {
    static KEEP: OnceLock<Mutex<Vec<tempfile::TempDir>>> = OnceLock::new();
    let dir = tempfile::tempdir().expect("hermetic workspace");
    let path = dir.path().to_path_buf();
    KEEP.get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("hermetic workspace lock")
        .push(dir);
    path
}

/// Allow OS-backed test resources to start under a busy shared macOS/Windows
/// host without relaxing behavioral cancellation or shutdown deadlines.
pub(crate) const fn external_resource_start_timeout(default: Duration) -> Duration {
    #[cfg(any(windows, target_os = "macos"))]
    {
        if default.as_secs() < 30 {
            return Duration::from_secs(30);
        }
    }
    default
}
