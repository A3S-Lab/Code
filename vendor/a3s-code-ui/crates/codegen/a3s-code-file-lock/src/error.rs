use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::slot::SLOT_DIR_ENV;

/// Why a lock could not be acquired.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// `Wait::NoWait` only: another process holds the lock right now.
    #[error("lock {} is held by another process", .path.display())]
    Contended { path: PathBuf },
    /// The machine-local acquire slot for `path` stayed held past the grace: another process
    /// (`holder_pid`, `None` if it had not stamped the slot yet) is still inside its own
    /// `open()`/`flock()` of this path, which almost always means a stalled network filesystem.
    /// `path` was not touched.
    #[error(
        "another process ({}) is still acquiring {}; the grok home looks stalled (network \
         filesystem?). Retry later; if the holder is not stuck, point {} at a new private \
         directory (mode 0700) so this process uses its own slot",
        .holder_pid.map_or_else(|| "an unknown pid".to_owned(), |pid| format!("pid {pid}")),
        .path.display(),
        SLOT_DIR_ENV
    )]
    AcquireInProgress {
        path: PathBuf,
        holder_pid: Option<u32>,
    },
    /// `Wait::Poll` only: still contended when the budget ran out. `waited` is the elapsed time.
    #[error("timed out after {waited:?} waiting for lock {}", .path.display())]
    Timeout { path: PathBuf, waited: Duration },
    /// Opening the lock file failed.
    #[error("failed to open lock file {}: {source}", .path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// `flock` failed for a reason other than contention (ENOLCK, ENOTSUP, ...): locking is
    /// unavailable on this filesystem, not busy.
    #[error("failed to lock {}: {source}", .path.display())]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl LockError {
    /// `Contended`, `Timeout`, or `AcquireInProgress`: nothing is broken; skip or retry later.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            LockError::Contended { .. }
                | LockError::Timeout { .. }
                | LockError::AcquireInProgress { .. }
        )
    }

    /// The lock file the error is about.
    pub fn path(&self) -> &Path {
        match self {
            LockError::Contended { path }
            | LockError::AcquireInProgress { path, .. }
            | LockError::Timeout { path, .. }
            | LockError::Open { path, .. }
            | LockError::Lock { path, .. } => path,
        }
    }
}

/// Result of a lock operation.
pub type Result<T> = std::result::Result<T, LockError>;
