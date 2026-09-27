//! The attempt loop. Each attempt takes the slot, re-opens the target, tries the flock once, and
//! releases the slot before any sleep, so the slot is never held across a wait or past return.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{LockError, Result};
use crate::locked_file::LockedFile;
use crate::options::{LockOptions, Wait};
use crate::slot::{SlotAttempt, SlotHandle};

/// Floor for `Wait::Poll`'s interval so a caller cannot spin.
const MIN_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Acquire an exclusive advisory lock on `path`, creating the file if missing and never
/// truncating it. Every attempt re-opens the path, so a lock file unlinked and recreated by an
/// older holder is picked up on the next attempt.
///
/// # Errors
/// `Contended` (`Wait::NoWait`: held by another process), `Timeout` (`Wait::Poll`: still held when
/// the budget ran out), `AcquireInProgress` (another process is wedged inside its own open+flock
/// of `path`; the path was not touched), `Open` (the lock file could not be opened), `Lock`
/// (`flock` failed for a reason other than contention).
pub fn lock_file(path: &Path, options: &LockOptions) -> Result<LockedFile> {
    lock_file_with(path, options, &mut open_target)
}

fn open_target(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// `open_fn` stands in for the real open so a test can park one attempt inside "open" and prove
/// that a second caller gets `AcquireInProgress` without touching the path.
pub(crate) fn lock_file_with(
    path: &Path,
    options: &LockOptions,
    open_fn: &mut dyn FnMut(&Path) -> io::Result<File>,
) -> Result<LockedFile> {
    let started = Instant::now();
    let poll = match options.wait() {
        Wait::NoWait => None,
        Wait::Poll { timeout, interval } => {
            Some((started + timeout, interval.max(MIN_POLL_INTERVAL)))
        }
    };
    let deadline = poll.map(|(deadline, _)| deadline);
    let slot = SlotHandle::resolve(options.slot(), path);
    let mut slowest_guarded = Duration::ZERO;
    let result = loop {
        let (file, outcome, guarded_elapsed) = {
            let guard = match slot.acquire(deadline) {
                Ok(SlotAttempt::Ready(guard)) => guard,
                #[cfg(unix)]
                Ok(SlotAttempt::DeadlineReached) => {
                    break Err(LockError::Timeout {
                        path: path.to_path_buf(),
                        waited: started.elapsed(),
                    });
                }
                Err(e) => break Err(e),
            };
            let attempt_started = Instant::now();
            let file = match open_fn(path) {
                Ok(file) => file,
                Err(source) => {
                    break Err(LockError::Open {
                        path: path.to_path_buf(),
                        source,
                    });
                }
            };
            let outcome = file.try_lock();
            (
                file,
                outcome,
                guard.is_some().then(|| attempt_started.elapsed()),
            )
        };
        if let Some(elapsed) = guarded_elapsed {
            slowest_guarded = slowest_guarded.max(elapsed);
        }
        match outcome {
            Ok(()) => break Ok(LockedFile::new(file, path.to_path_buf())),
            Err(TryLockError::WouldBlock) => drop(file),
            Err(TryLockError::Error(source)) => {
                break Err(LockError::Lock {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
        let Some((deadline, interval)) = poll else {
            break Err(LockError::Contended {
                path: path.to_path_buf(),
            });
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break Err(LockError::Timeout {
                path: path.to_path_buf(),
                waited: started.elapsed(),
            });
        }
        std::thread::sleep(interval.min(remaining));
    };
    // A healthy attempt this slow is what the grace is tuned against.
    if let Some(grace) = slot.grace()
        && slowest_guarded > grace
    {
        tracing::warn!(
            path = %path.display(),
            elapsed_ms = slowest_guarded.as_millis(),
            "lock attempt exceeded the slot grace"
        );
    }
    result
}

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
