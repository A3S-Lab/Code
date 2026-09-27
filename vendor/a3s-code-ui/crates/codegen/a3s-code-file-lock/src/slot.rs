//! The machine-local acquire slot: an exclusive flock on a small file under a local directory, held
//! for exactly one open+flock attempt on the target. The slot name is a pure function of the target
//! path string, so nothing here canonicalizes or stats the target. Slot files are never unlinked:
//! unlinking races a sibling that already opened the old inode and leaves two holders of one slot.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[cfg(not(unix))]
pub(crate) use stub::{SlotGuard, SlotHandle};
#[cfg(unix)]
pub(crate) use unix::{SlotGuard, SlotHandle};

/// Environment variable naming an absolute directory that replaces the default slot directory.
pub const SLOT_DIR_ENV: &str = "GROK_FILE_LOCK_SLOT_DIR";

/// Outcome of taking the slot for one attempt.
pub(crate) enum SlotAttempt {
    /// Proceed with the attempt: the slot is held (`Some`) or this call runs unguarded (`None`).
    Ready(Option<SlotGuard>),
    /// The caller's deadline arrived less than a full grace into the wait with the slot still held:
    /// indistinguishable from ordinary contention, so the target stays untouched and the caller
    /// reports `Timeout`.
    #[cfg(unix)]
    DeadlineReached,
}

const FNV1A_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV1A_PRIME: u64 = 0x0000_0100_0000_01b3;
const MAX_HINT_LEN: usize = 48;

/// Pure: `<dir>/<hint>.<fnv1a64 of the lexically normalized target>.slot`. Normalization is
/// `Path::components()` only (collapses `.` and repeated separators, resolves neither `..` nor
/// symlinks), so two spellings of one path share a slot without any filesystem access.
pub fn slot_path_in(dir: &Path, target: &Path) -> PathBuf {
    let mut normalized = Vec::new();
    for component in target.components() {
        normalized.extend_from_slice(component.as_os_str().as_encoded_bytes());
        normalized.push(b'/');
    }
    let hash = fnv1a64(&normalized);
    dir.join(format!("{}.{hash:016x}.slot", hint_for(target)))
}

/// Target file name restricted to `[A-Za-z0-9._-]` and truncated, for a human-readable prefix.
fn hint_for(target: &Path) -> String {
    let name = target
        .file_name()
        .map(OsStr::to_string_lossy)
        .unwrap_or_default();
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_HINT_LEN)
        .collect();
    if sanitized.is_empty() {
        "lock".to_owned()
    } else {
        sanitized
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV1A_OFFSET, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV1A_PRIME)
    })
}

#[cfg(unix)]
mod unix {
    use std::fs::{self, File, OpenOptions, TryLockError};
    use std::io::{self, Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use crate::error::{LockError, Result};
    use crate::options::SlotPolicy;
    use crate::slot::{SLOT_DIR_ENV, SlotAttempt, slot_path_in};

    const SLOT_POLL_INTERVAL: Duration = Duration::from_millis(10);
    const MAX_SLOT_READ: u64 = 4096;

    static SLOT_UNAVAILABLE_WARNED: AtomicBool = AtomicBool::new(false);
    static RELATIVE_OVERRIDE_WARNED: AtomicBool = AtomicBool::new(false);

    /// What to do with a slot directory we own whose mode lets group or others in.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum LooseMode {
        /// The default directory is ours by construction: chmod it to 0700.
        Tighten,
        /// A caller-supplied directory may be shared on purpose: refuse it (the caller fails open).
        Refuse,
    }

    /// Pure: an absolute `override_dir` (refused when loose), else `/tmp/a3s-file-lock-<euid>`
    /// (tightened when loose). Never `$TMPDIR` or `$HOME` (either may sit on a network filesystem)
    /// and never `$XDG_RUNTIME_DIR` (one directory per login session would mean one wedged process
    /// per session).
    pub(crate) fn resolve_slot_dir(override_dir: Option<&Path>, euid: u32) -> (PathBuf, LooseMode) {
        match override_dir {
            Some(dir) if dir.is_absolute() => (dir.to_path_buf(), LooseMode::Refuse),
            Some(_) | None => (
                PathBuf::from(format!("/tmp/grok-file-lock-{euid}")),
                LooseMode::Tighten,
            ),
        }
    }

    /// Create `dir` with mode 0700 if missing, then require a non-symlink directory owned by `euid`
    /// whose mode admits nobody else (see [`LooseMode`] for a looser one).
    pub(crate) fn verify_slot_dir(dir: &Path, euid: u32, loose: LooseMode) -> io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

        if let Err(e) = fs::DirBuilder::new().mode(0o700).create(dir)
            && e.kind() != io::ErrorKind::AlreadyExists
        {
            return Err(e);
        }
        let meta = fs::symlink_metadata(dir)?;
        if !meta.is_dir() {
            return Err(io::Error::other("not a directory (symlinks are refused)"));
        }
        if meta.uid() != euid {
            return Err(io::Error::other(format!(
                "owned by uid {}, expected {euid}",
                meta.uid()
            )));
        }
        if meta.mode() & 0o077 != 0 {
            match loose {
                LooseMode::Tighten => {
                    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
                }
                LooseMode::Refuse => {
                    return Err(io::Error::other(format!(
                        "mode {:o} admits other users; expected 0700",
                        meta.mode() & 0o777
                    )));
                }
            }
        }
        Ok(())
    }

    /// Resolve, create (mode 0700), and verify the slot directory: `$GROK_FILE_LOCK_SLOT_DIR` when
    /// set and absolute, else `/tmp/a3s-file-lock-<euid>`. `None` means no usable directory; the
    /// caller then acquires unguarded, since exclusion comes from the target lock alone.
    pub(crate) fn slot_dir() -> Option<PathBuf> {
        let euid = current_euid();
        let override_dir = std::env::var_os(SLOT_DIR_ENV).map(PathBuf::from);
        let (dir, loose) = resolve_slot_dir(override_dir.as_deref(), euid);
        // An override that resolved to the default was relative and got discarded.
        if let (Some(value), LooseMode::Tighten) = (&override_dir, loose)
            && !RELATIVE_OVERRIDE_WARNED.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(
                value = %value.display(),
                "{SLOT_DIR_ENV} is not an absolute path; using the default slot directory"
            );
        }
        verify_usable_slot_dir(dir, euid, loose)
    }

    fn verify_usable_slot_dir(dir: PathBuf, euid: u32, loose: LooseMode) -> Option<PathBuf> {
        match verify_slot_dir(&dir, euid, loose) {
            Ok(()) => Some(dir),
            Err(e) => {
                warn_unavailable(&dir, &e);
                None
            }
        }
    }

    /// Fail-open is logged once per process: the slot limits the hazard, it is not the lock.
    fn warn_unavailable(path: &Path, error: &io::Error) {
        if !SLOT_UNAVAILABLE_WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "lock slot unusable; acquiring without the local acquire slot"
            );
        }
    }

    fn current_euid() -> u32 {
        // SAFETY: `geteuid` takes no arguments, cannot fail, and only reads process credentials.
        unsafe { libc::geteuid() }
    }

    /// The slot for one `lock_file` call: resolved once, then taken once per attempt.
    pub(crate) struct SlotHandle {
        /// `None`: acquire unguarded.
        path: Option<PathBuf>,
        target: PathBuf,
        grace: Duration,
        euid: u32,
    }

    impl SlotHandle {
        pub(crate) fn resolve(policy: &SlotPolicy, target: &Path) -> Self {
            let euid = current_euid();
            let (dir, grace) = match policy {
                SlotPolicy::Guarded { grace } => (slot_dir(), *grace),
                SlotPolicy::GuardedIn { dir, grace } => (
                    verify_usable_slot_dir(dir.clone(), euid, LooseMode::Refuse),
                    *grace,
                ),
                SlotPolicy::Unguarded => (None, Duration::ZERO),
            };
            SlotHandle {
                path: dir.map(|root| slot_path_in(&root, target)),
                target: target.to_path_buf(),
                grace,
                euid,
            }
        }

        /// The grace a sibling gets before `AcquireInProgress`; `None` when acquiring unguarded.
        pub(crate) fn grace(&self) -> Option<Duration> {
            self.path.is_some().then_some(self.grace)
        }

        /// Take the slot for one attempt, polling until it is free or the grace elapses. Only a
        /// slot held for the full grace is a wedge (`AcquireInProgress`, target untouched); a wait
        /// cut short by `deadline` is `DeadlineReached`. `Ready(None)` means unguarded, including a
        /// slot file that turned out unusable.
        pub(crate) fn acquire(&self, deadline: Option<Instant>) -> Result<SlotAttempt> {
            let Some(slot_path) = &self.path else {
                return Ok(SlotAttempt::Ready(None));
            };
            let mut file = match open_slot_file(slot_path, self.euid) {
                Ok(file) => file,
                Err(e) => {
                    warn_unavailable(slot_path, &e);
                    return Ok(SlotAttempt::Ready(None));
                }
            };
            let now = Instant::now();
            let budget = deadline.map_or(self.grace, |outer| {
                self.grace.min(outer.saturating_duration_since(now))
            });
            let clipped = budget < self.grace;
            let slot_deadline = now + budget;
            loop {
                match file.try_lock() {
                    Ok(()) => break,
                    Err(TryLockError::WouldBlock) => {}
                    Err(TryLockError::Error(e)) => {
                        warn_unavailable(slot_path, &e);
                        return Ok(SlotAttempt::Ready(None));
                    }
                }
                let remaining = slot_deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    if clipped {
                        return Ok(SlotAttempt::DeadlineReached);
                    }
                    return Err(LockError::AcquireInProgress {
                        path: self.target.clone(),
                        holder_pid: read_holder_pid(&mut file),
                    });
                }
                std::thread::sleep(SLOT_POLL_INTERVAL.min(remaining));
            }
            // Best effort: the stamp only names the holder for a waiter's error message.
            let stamp = format!("{}\n{}\n", std::process::id(), self.target.display());
            if let Err(e) = file
                .set_len(0)
                .and_then(|()| file.write_all(stamp.as_bytes()))
            {
                tracing::debug!(
                    path = %slot_path.display(),
                    error = %e,
                    "failed to stamp lock slot"
                );
            }
            Ok(SlotAttempt::Ready(Some(SlotGuard { _file: file })))
        }
    }

    /// Holds the slot flock for one attempt. Closing the only descriptor releases it.
    pub(crate) struct SlotGuard {
        _file: File,
    }

    /// `O_RDWR|O_CREAT|O_NOFOLLOW`, mode 0600, never truncated. The opened inode must be a regular
    /// file owned by `euid`; anything else means the slot directory was tampered with.
    fn open_slot_file(path: &Path, euid: u32) -> io::Result<File> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        if meta.uid() != euid {
            return Err(io::Error::other(format!(
                "owned by uid {}, expected {euid}",
                meta.uid()
            )));
        }
        Ok(file)
    }

    /// First line of the slot file as a pid. `None`: empty or unparsable, because the holder has
    /// not stamped yet or an external `flock(1)` holds the slot.
    fn read_holder_pid(file: &mut File) -> Option<u32> {
        let mut contents = String::new();
        file.take(MAX_SLOT_READ)
            .read_to_string(&mut contents)
            .ok()?;
        contents.lines().next()?.trim().parse().ok()
    }
}

/// No acquire slot on this platform: every attempt runs unguarded.
#[cfg(not(unix))]
mod stub {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use crate::error::Result;
    use crate::options::SlotPolicy;
    use crate::slot::SlotAttempt;

    pub(crate) struct SlotHandle;

    pub(crate) enum SlotGuard {}

    impl SlotHandle {
        pub(crate) fn resolve(_policy: &SlotPolicy, _target: &Path) -> Self {
            SlotHandle
        }

        pub(crate) fn grace(&self) -> Option<Duration> {
            None
        }

        pub(crate) fn acquire(&self, _deadline: Option<Instant>) -> Result<SlotAttempt> {
            Ok(SlotAttempt::Ready(None))
        }
    }
}

#[cfg(test)]
#[path = "slot_tests.rs"]
mod tests;
