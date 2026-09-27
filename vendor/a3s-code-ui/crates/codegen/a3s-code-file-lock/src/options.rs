use std::path::PathBuf;
use std::time::Duration;

/// How long a caller waits behind a held acquire slot before `AcquireInProgress`. A healthy
/// sibling holds the slot for microseconds; only one wedged in the kernel holds it this long.
pub const DEFAULT_SLOT_GRACE: Duration = Duration::from_secs(2);

/// How long to wait for a contended target lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// One open+flock attempt; contention is `LockError::Contended`. Behind a wedged sibling this
    /// still blocks up to the slot grace before returning `AcquireInProgress`.
    NoWait,
    /// Re-open and retry every `interval` (clamped to at least 1 ms) until acquired or `timeout`
    /// elapses (`LockError::Timeout`). A held slot is `AcquireInProgress` only once it has been
    /// watched for a full grace; a wait the deadline cuts short is `Timeout`, so a `timeout`
    /// shorter than the grace never reports `AcquireInProgress`.
    Poll {
        timeout: Duration,
        interval: Duration,
    },
}

/// Whether an attempt runs behind the machine-local acquire slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotPolicy {
    /// Default. The slot directory is resolved per call; a sibling's in-flight attempt is waited
    /// for up to `grace`, then `AcquireInProgress`.
    Guarded { grace: Duration },
    /// As `Guarded`, rooted at an explicit directory (tests, diagnostics).
    GuardedIn { dir: PathBuf, grace: Duration },
    /// No slot. Windows always behaves as if this were set.
    Unguarded,
}

/// Options for `lock_file`. Fields are private so later additions stay non-breaking.
#[derive(Debug, Clone)]
pub struct LockOptions {
    wait: Wait,
    slot: SlotPolicy,
}

impl LockOptions {
    /// `Wait::NoWait` and `SlotPolicy::Guarded` with [`DEFAULT_SLOT_GRACE`]. The lock file is
    /// opened read+write, created if missing, and never truncated.
    pub fn new() -> Self {
        LockOptions {
            wait: Wait::NoWait,
            slot: SlotPolicy::Guarded {
                grace: DEFAULT_SLOT_GRACE,
            },
        }
    }

    pub fn with_wait(mut self, wait: Wait) -> Self {
        self.wait = wait;
        self
    }

    /// Shorthand for `with_wait(Wait::Poll { timeout, interval })`.
    pub fn with_poll(self, timeout: Duration, interval: Duration) -> Self {
        self.with_wait(Wait::Poll { timeout, interval })
    }

    pub fn with_slot(mut self, slot: SlotPolicy) -> Self {
        self.slot = slot;
        self
    }

    pub fn wait(&self) -> Wait {
        self.wait
    }

    pub fn slot(&self) -> &SlotPolicy {
        &self.slot
    }
}

impl Default for LockOptions {
    fn default() -> Self {
        LockOptions::new()
    }
}
