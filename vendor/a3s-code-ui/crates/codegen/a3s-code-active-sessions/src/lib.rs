//! Tracks open TUI sessions in `~/.a3s/active_sessions.json`. A clean exit removes the entry,
//! a crash leaves it behind, and the next [`register`] prunes entries whose PID is dead.

#![deny(clippy::indexing_slicing)]

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use agent_client_protocol as acp;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveSession {
    pub session_id: acp::SessionId,
    pub pid: u32,
    pub cwd: String,
    pub opened_at: DateTime<Utc>,
}

const DATA_FILENAME: &str = "active_sessions.json";
const LOCK_FILENAME: &str = "active_sessions.lock";
const TMP_FILENAME: &str = "active_sessions.json.tmp";

/// On an NFS home `flock` is a network-lock-manager lock with no lease: a holder killed at the
/// wrong moment strands it forever, so never wait unbounded. A live holder only does one small
/// read-modify-write, so anything held this long is stranded.
const LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Register a session as active (idempotent by session_id) and prune entries whose PID is dead.
/// Fails with [`io::ErrorKind::TimedOut`] if the lock is still held after `LOCK_ACQUIRE_TIMEOUT`.
pub fn register(session: ActiveSession) -> io::Result<()> {
    register_in(&a3s_code_config::grok_home(), session)
}

/// Non-blocking unregister for signal handlers.
/// Returns `Ok(false)` on lock contention; the orphan is pruned by the next `register`.
pub fn try_unregister(session_id: &acp::SessionId) -> io::Result<bool> {
    try_unregister_in(&a3s_code_config::grok_home(), session_id)
}

pub fn register_in(root: &Path, session: ActiveSession) -> io::Result<()> {
    with_locked_state(root, LOCK_ACQUIRE_TIMEOUT, |sessions| {
        sessions.retain(|s| s.session_id != session.session_id && is_pid_alive(s.pid));
        sessions.push(session);
    })
}

pub fn try_unregister_in(root: &Path, session_id: &acp::SessionId) -> io::Result<bool> {
    let outcome = with_locked_state(root, Duration::ZERO, |sessions| {
        sessions.retain(|s| s.session_id != *session_id);
    });
    match outcome {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn list_in(root: &Path) -> io::Result<Vec<ActiveSession>> {
    let data_path = root.join(DATA_FILENAME);
    read_data_file(&data_path)
}

fn with_locked_state<F, R>(root: &Path, timeout: Duration, mutate: F) -> io::Result<R>
where
    F: FnOnce(&mut Vec<ActiveSession>) -> R,
{
    let lock_path = root.join(LOCK_FILENAME);
    let data_path = root.join(DATA_FILENAME);
    let tmp_path = root.join(TMP_FILENAME);

    fs::create_dir_all(root)?;
    // Closing the file releases the lock, so every return path unlocks by dropping it.
    let lock_file = open_lock_file(&lock_path)?;
    let deadline = Instant::now() + timeout;
    loop {
        match lock_file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(e)) => return Err(e),
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("active-session registry lock still held after {timeout:?}"),
            ));
        }
        std::thread::sleep(LOCK_POLL_INTERVAL.min(remaining));
    }

    let mut sessions = read_data_file(&data_path)?;
    let result = mutate(&mut sessions);
    write_data_file_atomic(&tmp_path, &data_path, &sessions)?;
    Ok(result)
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn read_data_file(path: &Path) -> io::Result<Vec<ActiveSession>> {
    match fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(Vec::new()),
        Ok(bytes) => match serde_json::from_slice::<Vec<ActiveSession>>(&bytes) {
            Ok(sessions) => Ok(sessions),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "active_sessions.json is corrupted, starting with empty list"
                );
                Ok(Vec::new())
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn write_data_file_atomic(
    tmp_path: &Path,
    data_path: &Path,
    sessions: &[ActiveSession],
) -> io::Result<()> {
    let json = serde_json::to_string_pretty(sessions)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(tmp_path, json.as_bytes())?;
    fs::rename(tmp_path, data_path).inspect_err(|_| {
        let _ = fs::remove_file(tmp_path);
    })
}

fn is_pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let pid_i = match i32::try_from(pid) {
            Ok(p) if p > 0 => p,
            _ => return false,
        };
        let ret = unsafe { libc::kill(pid_i as libc::pid_t, 0) };
        if ret == 0 {
            return true;
        }
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) };
        match handle {
            Ok(h) => {
                let _ = unsafe { CloseHandle(h) };
                true
            }
            Err(_) => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        // Conservative: assume alive if we can't check.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_session(id: &str, pid: u32) -> ActiveSession {
        ActiveSession {
            session_id: acp::SessionId::new(id),
            pid,
            cwd: "/tmp/test".into(),
            opened_at: Utc::now(),
        }
    }

    #[test]
    fn register_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let s = make_session("s1", std::process::id());
        register_in(dir.path(), s.clone()).unwrap();
        register_in(dir.path(), s).unwrap();
        assert_eq!(1, list_in(dir.path()).unwrap().len());
    }

    #[test]
    fn concurrent_registers_no_corruption() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        std::thread::scope(|s| {
            for i in 0..10 {
                let p = path.clone();
                s.spawn(move || {
                    register_in(&p, make_session(&format!("s{i}"), std::process::id())).unwrap()
                });
            }
        });
        assert_eq!(10, list_in(dir.path()).unwrap().len());
    }

    #[test]
    fn try_unregister_skips_if_locked() {
        let dir = TempDir::new().unwrap();
        let s = make_session("s1", std::process::id());
        register_in(dir.path(), s.clone()).unwrap();

        let lock_file = open_lock_file(&dir.path().join(LOCK_FILENAME)).unwrap();
        lock_file.lock().unwrap();
        assert!(!try_unregister_in(dir.path(), &s.session_id).unwrap());
        lock_file.unlock().unwrap();
        assert_eq!(1, list_in(dir.path()).unwrap().len());
    }

    #[test]
    fn locked_update_times_out_when_lock_stays_held() {
        let dir = TempDir::new().unwrap();
        register_in(dir.path(), make_session("s1", std::process::id())).unwrap();
        let holder = open_lock_file(&dir.path().join(LOCK_FILENAME)).unwrap();
        holder.lock().unwrap();

        let outcome = with_locked_state(dir.path(), Duration::from_millis(50), Vec::clear);

        assert_eq!(io::ErrorKind::TimedOut, outcome.unwrap_err().kind());
        assert_eq!(1, list_in(dir.path()).unwrap().len());
    }

    #[test]
    fn locked_update_retries_until_holder_releases() {
        let dir = TempDir::new().unwrap();
        let holder = open_lock_file(&dir.path().join(LOCK_FILENAME)).unwrap();
        holder.lock().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            holder.unlock().unwrap();
        });

        with_locked_state(dir.path(), LOCK_ACQUIRE_TIMEOUT, |sessions| {
            sessions.push(make_session("s1", std::process::id()));
        })
        .unwrap();
        release.join().unwrap();
        assert_eq!(1, list_in(dir.path()).unwrap().len());
    }

    #[test]
    fn corrupt_file_recovers() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(DATA_FILENAME), "garbage{{{").unwrap();
        assert!(list_in(dir.path()).unwrap().is_empty());
        register_in(dir.path(), make_session("s1", std::process::id())).unwrap();
        assert_eq!(1, list_in(dir.path()).unwrap().len());
    }
}
