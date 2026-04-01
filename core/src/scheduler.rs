//! Session-scoped prompt scheduler — backs `/loop`, `/cron-list`, `/cron-cancel`.
//!
//! Users schedule recurring prompts with `/loop 5m check the deployment`.
//! The scheduler fires them via a tokio channel; `AgentSession` drains the
//! channel after each `send()` and runs the fired prompts.
//!
//! ## Design
//!
//! - `CronScheduler` holds a `std::sync::Mutex<CronSchedulerInner>` so it can
//!   be accessed from synchronous slash command handlers.
//! - A background tokio task holds a `Weak<CronScheduler>` and ticks every
//!   second. It exits automatically when the session is dropped.
//! - Fired prompts are delivered via `tokio::sync::mpsc::UnboundedSender`.
//! - Deterministic jitter (0–10% of period, capped at 15 min) is derived
//!   from the task ID so the same task always fires with the same offset.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Maximum number of tasks a session may have at one time.
const MAX_TASKS: usize = 50;
/// Default interval when none is specified.
const DEFAULT_INTERVAL_SECS: u64 = 600; // 10 minutes
/// Recurring tasks auto-expire after this duration.
const MAX_RECURRING_AGE: Duration = Duration::from_secs(3 * 24 * 3600); // 3 days
/// Jitter cap.
const MAX_JITTER: Duration = Duration::from_secs(15 * 60); // 15 minutes

// ─── Public Types ────────────────────────────────────────────────────────────

/// A pending scheduled task.
pub struct ScheduledTask {
    pub id: String,
    pub prompt: String,
    pub interval: Duration,
    pub recurring: bool,
    pub created_at: Instant,
    pub next_fire: Instant,
    pub fire_count: usize,
}

/// A task fire event delivered to `AgentSession`.
pub struct ScheduledFire {
    pub task_id: String,
    pub prompt: String,
}

/// Snapshot of a task used for display (avoids exposing `Instant` in public API).
pub struct ScheduledTaskInfo {
    pub id: String,
    pub prompt: String,
    pub interval_secs: u64,
    pub recurring: bool,
    pub fire_count: usize,
    /// Seconds until the next fire (0 if overdue).
    pub next_fire_in_secs: u64,
}

// ─── Scheduler ───────────────────────────────────────────────────────────────

struct CronSchedulerInner {
    tasks: HashMap<String, ScheduledTask>,
}

/// Session-scoped prompt scheduler.
///
/// Create with [`CronScheduler::new`], then call [`CronScheduler::start`] to
/// launch the background ticker.
pub struct CronScheduler {
    inner: Mutex<CronSchedulerInner>,
    prompt_tx: mpsc::UnboundedSender<ScheduledFire>,
    stopped: AtomicBool,
}

impl CronScheduler {
    /// Create a new scheduler and its receiver channel.
    ///
    /// Call [`CronScheduler::start`] after wrapping in `Arc` to start the ticker.
    pub fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<ScheduledFire>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let scheduler = Arc::new(Self {
            inner: Mutex::new(CronSchedulerInner {
                tasks: HashMap::new(),
            }),
            prompt_tx: tx,
            stopped: AtomicBool::new(false),
        });
        (scheduler, rx)
    }

    /// Spawn the background 1-second ticker.
    ///
    /// The task holds a `Weak` reference so it exits automatically when the
    /// `Arc<CronScheduler>` is dropped (i.e., when the session is dropped).
    pub fn start(scheduler: Arc<Self>) {
        let weak = Arc::downgrade(&scheduler);
        drop(scheduler);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.tick().await; // consume the immediate first tick
            loop {
                interval.tick().await;
                match weak.upgrade() {
                    Some(s) => {
                        if s.stopped.load(Ordering::Relaxed) {
                            break;
                        }
                        s.tick();
                    }
                    None => break, // session dropped — exit cleanly
                }
            }
        });
    }

    /// Stop the background ticker and clear all scheduled tasks.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Ok(mut inner) = self.inner.lock() {
            inner.tasks.clear();
        }
    }

    /// Check all tasks and fire any that are due.
    fn tick(&self) {
        let now = Instant::now();
        let mut to_fire: Vec<(String, String)> = Vec::new();
        let mut to_remove: Vec<String> = Vec::new();

        {
            let inner = match self.inner.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            for (id, task) in &inner.tasks {
                if now >= task.next_fire {
                    to_fire.push((id.clone(), task.prompt.clone()));
                    let age = now - task.created_at;
                    if !task.recurring || age >= MAX_RECURRING_AGE {
                        to_remove.push(id.clone());
                    }
                }
            }
        }

        if to_fire.is_empty() {
            return;
        }

        {
            let mut inner = match self.inner.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            for (id, prompt) in &to_fire {
                if let Some(task) = inner.tasks.get_mut(id) {
                    task.fire_count += 1;
                    if task.recurring && !to_remove.contains(id) {
                        let jitter = compute_jitter(id, task.interval);
                        task.next_fire = Instant::now() + task.interval + jitter;
                    }
                }
                let _ = self.prompt_tx.send(ScheduledFire {
                    task_id: id.clone(),
                    prompt: prompt.clone(),
                });
            }
            for id in &to_remove {
                inner.tasks.remove(id);
            }
        }
    }

    /// Schedule a new task. Returns the task ID on success.
    pub fn create_task(
        &self,
        prompt: String,
        interval: Duration,
        recurring: bool,
    ) -> Result<String, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "scheduler lock poisoned".to_string())?;
        if inner.tasks.len() >= MAX_TASKS {
            return Err(format!(
                "maximum of {MAX_TASKS} scheduled tasks reached; cancel one with /cron-cancel"
            ));
        }
        let id = new_task_id();
        let jitter = compute_jitter(&id, interval);
        let now = Instant::now();
        inner.tasks.insert(
            id.clone(),
            ScheduledTask {
                id: id.clone(),
                prompt,
                interval,
                recurring,
                created_at: now,
                next_fire: now + interval + jitter,
                fire_count: 0,
            },
        );
        Ok(id)
    }

    /// List all active tasks sorted by ID.
    pub fn list_tasks(&self) -> Vec<ScheduledTaskInfo> {
        let inner = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return vec![],
        };
        let now = Instant::now();
        let mut tasks: Vec<_> = inner
            .tasks
            .values()
            .map(|t| ScheduledTaskInfo {
                id: t.id.clone(),
                prompt: t.prompt.clone(),
                interval_secs: t.interval.as_secs(),
                recurring: t.recurring,
                fire_count: t.fire_count,
                next_fire_in_secs: if t.next_fire > now {
                    (t.next_fire - now).as_secs()
                } else {
                    0
                },
            })
            .collect();
        tasks.sort_by(|a, b| a.id.cmp(&b.id));
        tasks
    }

    /// Cancel a task by ID. Returns `true` if it existed.
    pub fn cancel_task(&self, id: &str) -> bool {
        self.inner
            .lock()
            .ok()
            .map(|mut g| g.tasks.remove(id).is_some())
            .unwrap_or(false)
    }

    /// Number of active tasks.
    pub fn task_count(&self) -> usize {
        self.inner.lock().map(|g| g.tasks.len()).unwrap_or(0)
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Generate a short random task ID (8 hex chars via UUID v4).
fn new_task_id() -> String {
    let id = uuid::Uuid::new_v4().to_string();
    id[..8].to_string()
}

/// Deterministic jitter: 0–10% of `interval`, capped at 15 min.
///
/// Derived from the task ID hash so the same task always fires with the
/// same relative offset (avoids thundering herd when many tasks share the
/// same interval).
fn compute_jitter(id: &str, interval: Duration) -> Duration {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    id.hash(&mut h);
    let fraction = (h.finish() % 1000) as f64 / 10000.0; // 0.000 .. 0.099
    let raw_secs = (interval.as_secs_f64() * fraction) as u64;
    Duration::from_secs(raw_secs.min(MAX_JITTER.as_secs()))
}

// ─── Interval / Arg Parsing ──────────────────────────────────────────────────

/// Parse a duration string like `"5m"`, `"30s"`, `"2h"`, `"1d"`.
///
/// Returns `Some(Duration)` on success, `None` if the format is not recognized.
pub fn parse_interval(s: &str) -> Option<Duration> {
    if s.len() < 2 {
        return None;
    }
    let (num_part, unit) = s.split_at(s.len() - 1);
    let n: u64 = num_part.parse().ok()?;
    match unit {
        "s" => Some(Duration::from_secs(n)),
        "m" => Some(Duration::from_secs(n * 60)),
        "h" => Some(Duration::from_secs(n * 3600)),
        "d" => Some(Duration::from_secs(n * 86400)),
        _ => None,
    }
}

/// Parse the argument string from `/loop <args>`.
///
/// Supports three forms:
/// - Leading interval: `/loop 5m check the build` → (5m, "check the build")
/// - Trailing clause:  `/loop check the build every 2h` → (2h, "check the build")
/// - No interval:      `/loop check the build` → (10m default, "check the build")
///
/// Returns `(interval, prompt)`.
pub fn parse_loop_args(args: &str) -> (Duration, String) {
    let args = args.trim();

    // Try leading interval: first whitespace-separated token is a valid interval
    if let Some(space) = args.find(char::is_whitespace) {
        let first = &args[..space];
        let rest = args[space..].trim();
        if let Some(interval) = parse_interval(first) {
            if !rest.is_empty() {
                return (interval, rest.to_string());
            }
        }
    }

    // Try trailing "every <interval>": last " every <token>" in the string.
    // `find_every_clause` returns the byte position of the leading space before "every".
    const EVERY_NEEDLE: &str = " every ";
    if let Some(every_pos) = find_every_clause(args) {
        let prompt_part = args[..every_pos].trim();
        let interval_token = args[every_pos + EVERY_NEEDLE.len()..]
            .split_whitespace()
            .next()
            .unwrap_or("");
        if let Some(interval) = parse_interval(interval_token) {
            if !prompt_part.is_empty() {
                return (interval, prompt_part.to_string());
            }
        }
    }

    // Default: 10 minutes, full args as prompt
    (Duration::from_secs(DEFAULT_INTERVAL_SECS), args.to_string())
}

/// Find the position of the last ` every <valid-interval>` clause in `s`.
fn find_every_clause(s: &str) -> Option<usize> {
    let needle = " every ";
    let mut best: Option<usize> = None;
    let mut search_from = 0;
    while let Some(rel) = s[search_from..].find(needle) {
        let abs = search_from + rel;
        let after = s[abs + needle.len()..]
            .split_whitespace()
            .next()
            .unwrap_or("");
        if parse_interval(after).is_some() {
            best = Some(abs);
        }
        search_from = abs + 1;
    }
    best
}

/// Format a duration in seconds to a human-readable string (`"5m"`, `"2h 30m"`, etc.).
pub fn format_duration(secs: u64) -> String {
    if secs == 0 {
        return "now".to_string();
    }
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let mut parts: Vec<String> = Vec::new();
    if d > 0 {
        parts.push(format!("{d}d"));
    }
    if h > 0 {
        parts.push(format!("{h}h"));
    }
    if m > 0 {
        parts.push(format!("{m}m"));
    }
    if s > 0 && parts.is_empty() {
        parts.push(format!("{s}s"));
    }
    parts.join(" ")
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_interval() {
        assert_eq!(parse_interval("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_interval("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_interval("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_interval("1d"), Some(Duration::from_secs(86400)));
        assert_eq!(parse_interval("bad"), None);
        assert_eq!(parse_interval(""), None);
        assert_eq!(parse_interval("m"), None);
        assert_eq!(parse_interval("0m"), Some(Duration::from_secs(0)));
    }

    #[test]
    fn test_parse_loop_args_leading_interval() {
        let (interval, prompt) = parse_loop_args("5m check the deployment");
        assert_eq!(interval, Duration::from_secs(300));
        assert_eq!(prompt, "check the deployment");
    }

    #[test]
    fn test_parse_loop_args_trailing_every() {
        let (interval, prompt) = parse_loop_args("monitor memory usage every 2h");
        assert_eq!(interval, Duration::from_secs(7200));
        assert_eq!(prompt, "monitor memory usage");
    }

    #[test]
    fn test_parse_loop_args_default_interval() {
        let (interval, prompt) = parse_loop_args("check the build");
        assert_eq!(interval, Duration::from_secs(600));
        assert_eq!(prompt, "check the build");
    }

    #[test]
    fn test_parse_loop_args_leading_seconds() {
        let (interval, prompt) = parse_loop_args("30s ping the server");
        assert_eq!(interval, Duration::from_secs(30));
        assert_eq!(prompt, "ping the server");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(0), "now");
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(300), "5m");
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(3660), "1h 1m");
        assert_eq!(format_duration(86400), "1d");
        assert_eq!(format_duration(90000), "1d 1h");
    }

    #[test]
    fn test_create_and_list_tasks() {
        let (scheduler, _rx) = CronScheduler::new();
        let id = scheduler
            .create_task("hello".to_string(), Duration::from_secs(60), true)
            .unwrap();
        assert_eq!(id.len(), 8);
        let tasks = scheduler.list_tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].prompt, "hello");
        assert_eq!(tasks[0].interval_secs, 60);
        assert!(tasks[0].recurring);
        assert_eq!(tasks[0].fire_count, 0);
    }

    #[test]
    fn test_cancel_task() {
        let (scheduler, _rx) = CronScheduler::new();
        let id = scheduler
            .create_task("test".to_string(), Duration::from_secs(60), true)
            .unwrap();
        assert!(scheduler.cancel_task(&id));
        assert!(!scheduler.cancel_task(&id)); // already gone
        assert_eq!(scheduler.list_tasks().len(), 0);
    }

    #[test]
    fn test_max_tasks_limit() {
        let (scheduler, _rx) = CronScheduler::new();
        for i in 0..MAX_TASKS {
            scheduler
                .create_task(format!("task {i}"), Duration::from_secs(60), true)
                .unwrap();
        }
        let err = scheduler
            .create_task("overflow".to_string(), Duration::from_secs(60), true)
            .unwrap_err();
        assert!(err.contains("maximum"));
    }

    #[test]
    fn test_compute_jitter_deterministic() {
        let j1 = compute_jitter("abc123", Duration::from_secs(600));
        let j2 = compute_jitter("abc123", Duration::from_secs(600));
        assert_eq!(j1, j2);
    }

    #[test]
    fn test_compute_jitter_bounded() {
        for id in &["aaa", "bbb", "ccc", "ddd"] {
            let jitter = compute_jitter(id, Duration::from_secs(600));
            assert!(jitter <= Duration::from_secs(60)); // 10% of 600s
        }
        // With a very large interval, jitter is capped at 15 min
        let jitter = compute_jitter("aaa", Duration::from_secs(10 * 3600));
        assert!(jitter <= MAX_JITTER);
    }
}
