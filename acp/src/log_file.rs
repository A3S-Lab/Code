//! Best-effort daily log file for the ACP agent.
//!
//! The agent's stderr is inherited by the launching terminal and scrolls
//! away; every hang/failure investigation so far had no on-disk trail. This
//! module adds a date-stamped file under the a3s state directory (default
//! `~/.a3s/logs/a3s-code-acp.YYYY-MM-DD.log`, keeping the newest seven).
//! Opening or writing failures degrade to stderr-only logging — diagnostics
//! must never block or kill the agent.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;

const KEEP_DAYS: usize = 7;

/// `A3S_CODE_LOG_FILE=0` disables the file layer entirely.
pub fn file_logging_enabled() -> bool {
    !matches!(
        std::env::var("A3S_CODE_LOG_FILE").as_deref(),
        Ok("0") | Ok("off") | Ok("false")
    )
}

/// Directory holding the log files; `A3S_CODE_LOG_DIR` overrides the
/// default `<home>/.a3s/logs`.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("A3S_CODE_LOG_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".a3s").join("logs")
}

fn today() -> String {
    // Unix-days stamp: no chrono dependency, no timezone ambiguity for the
    // rotation key (a local-midnight rollover nuance is acceptable).
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (secs / 86_400).to_string()
}

/// One open log file plus its rotation stamp.
pub struct DailyLog {
    dir: PathBuf,
    day: String,
    file: Option<File>,
    size_capped: bool,
}

impl DailyLog {
    pub fn create(dir: PathBuf) -> Self {
        let day = today();
        let file = open_today(&dir, &day);
        Self {
            dir,
            day,
            file,
            size_capped: false,
        }
    }

    #[cfg(test)]
    fn path_for(&self, day: &str) -> PathBuf {
        self.dir.join(format!("a3s-code-acp.{day}.log"))
    }

    /// Roll to today's file when the day changed; prune old files.
    fn rotate_if_stale(&mut self) {
        let day = today();
        if day == self.day {
            return;
        }
        self.file = open_today(&self.dir, &day);
        self.day = day;
        self.size_capped = false;
        prune_old_logs(&self.dir);
    }

    pub fn write_line(&mut self, line: &str) {
        self.rotate_if_stale();
        self.enforce_size_cap();
        if self.size_capped {
            return;
        }
        if let Some(file) = self.file.as_mut() {
            let _ = writeln!(file, "{line}");
        }
    }

    /// Bound in-day growth: past the cap the file takes one final note and
    /// then receives nothing until the next rotation.
    fn enforce_size_cap(&mut self) {
        const MAX_BYTES: u64 = 16 * 1024 * 1024;
        if self.size_capped {
            return;
        }
        let over = self
            .file
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .is_some_and(|meta| meta.len() >= MAX_BYTES);
        if over {
            self.size_capped = true;
            if let Some(file) = self.file.as_mut() {
                let _ = writeln!(file, "-- log size cap reached; stopping for today --");
            }
            tracing::warn!("agent log file reached the size cap; stopping writes for today");
        }
    }
}

fn open_today(dir: &std::path::Path, day: &str) -> Option<File> {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("a3s-code-acp.{day}.log"));
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Keep only the newest `KEEP_DAYS` dated files; failures are ignored.
fn prune_old_logs(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dated: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_prefix("a3s-code-acp.")?.strip_suffix(".log")?;
            Some((stem.to_string(), entry.path()))
        })
        .collect();
    dated.sort();
    while dated.len() > KEEP_DAYS.saturating_sub(1) {
        let (_, path) = dated.remove(0);
        let _ = std::fs::remove_file(path);
    }
}

/// A `tracing_subscriber` writer that appends each formatted event to the
/// daily file. Cloned cheaply into the subscriber.
#[derive(Clone)]
pub struct DailyLogWriter {
    inner: std::sync::Arc<Mutex<DailyLog>>,
}

impl DailyLogWriter {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(DailyLog::create(log_dir()))),
        }
    }
}

impl Default for DailyLogWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for DailyLogWriter {
    type Writer = DailyLogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl std::io::Write for DailyLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut log) = self.inner.lock() {
            log.write_line(String::from_utf8_lossy(buf).trim_end());
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Ok(mut log) = self.inner.lock() {
            if let Some(file) = log.file.as_mut() {
                let _ = file.flush();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HOME/A3S_CODE_LOG_DIR are process-global; serialise and restore.
    fn env_locked<T>(test: impl FnOnce() -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let log_dir = std::env::var_os("A3S_CODE_LOG_DIR");
        std::env::set_var("A3S_CODE_LOG_DIR", "/nonexistent-logs");
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(test));
        match log_dir {
            Some(value) => std::env::set_var("A3S_CODE_LOG_DIR", value),
            None => std::env::remove_var("A3S_CODE_LOG_DIR"),
        }
        match outcome {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    #[test]
    fn daily_log_writes_and_prunes() {
        env_locked(|| {
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var("A3S_CODE_LOG_DIR", dir.path());
            let mut log = DailyLog::create(log_dir());
            let day = today();
            log.write_line("first line");
            log.write_line("second line");
            let path = log.path_for(&day);
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            assert_eq!(body, "first line\nsecond line\n");
            // Rotation pruning keeps the bound.
            for stale in ["1", "2", "3", "4", "5", "6", "7", "8", "9"] {
                let _ = std::fs::write(dir.path().join(format!("a3s-code-acp.{stale}.log")), "x");
            }
            prune_old_logs(&log_dir());
            let remaining = std::fs::read_dir(dir.path()).unwrap().count();
            assert!(remaining <= KEEP_DAYS, "kept {remaining} files");
        })
    }

    #[test]
    fn daily_log_writer_routes_to_the_env_dir() {
        env_locked(|| {
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var("A3S_CODE_LOG_DIR", dir.path());
            let mut writer = DailyLogWriter::new();
            use std::io::Write as _;
            writer.write_all(b"routed line\n").unwrap();
            writer.flush().unwrap();
            let day = today();
            let body = std::fs::read_to_string(dir.path().join(format!("a3s-code-acp.{day}.log")))
                .unwrap_or_default();
            assert!(body.contains("routed line"), "got {body:?}");
        })
    }

    #[test]
    fn writer_reports_full_writes_even_without_a_file() {
        let mut writer = DailyLogWriter::new();
        let n = writer.write(b"event line\n").unwrap();
        assert_eq!(n, "event line\n".len());
        writer.flush().unwrap();
    }
}
