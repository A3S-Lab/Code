//! Bounded advisory file locks for files under the a3s home. Every attempt runs behind a
//! machine-local acquire slot, so a stalled network filesystem wedges at most one process per
//! lock path; a sibling that finds the slot held past a short grace gets
//! [`LockError::AcquireInProgress`] without naming the target path.
//!
//! Invariants:
//! - No blocking lock: every wait is a bounded poll driven by a deadline computed once.
//! - Slot before open: under a guarded policy no syscall names the target until the slot is held.
//! - The slot spans one attempt; it is released before any sleep and before returning.
//! - Slot storage is local by construction: `GROK_FILE_LOCK_SLOT_DIR` or
//!   `/tmp/a3s-file-lock-<euid>`, never `$HOME`, `$TMPDIR`, or `$XDG_RUNTIME_DIR`.
//! - The slot name is a pure function of the path string; the target is never canonicalized
//!   or stat'ed.
//! - Slot dir and file are owned by the effective uid and are never symlinks; anything else is
//!   skipped, never used. Slot files are never unlinked. There is no slot on Windows.
//! - Exclusion comes from the target flock only; an unusable slot dir degrades to an unguarded
//!   attempt.
//! - Contention is `TryLockError::WouldBlock` only, so old and new binaries keep excluding each
//!   other. Every attempt re-opens the target path.

#![deny(clippy::indexing_slicing)]

mod error;
mod lock;
mod locked_file;
mod options;
mod slot;

pub use error::{LockError, Result};
pub use lock::lock_file;
pub use locked_file::LockedFile;
pub use options::{DEFAULT_SLOT_GRACE, LockOptions, SlotPolicy, Wait};
pub use slot::{SLOT_DIR_ENV, slot_path_in};
