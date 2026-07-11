//! In-memory retention limits for long-running sessions.
//!
//! The framework's in-memory stores
//! ([`InMemoryRunStore`](crate::run::InMemoryRunStore),
//! [`InMemoryTraceSink`](crate::trace::InMemoryTraceSink),
//! [`InMemorySubagentTaskTracker`](crate::subagent_task_tracker::InMemorySubagentTaskTracker))
//! use conservative finite defaults so sessions that live for hours or days
//! cannot grow these collections without bound.
//!
//! `SessionRetentionLimits` lets the host cap each store with a FIFO
//! policy. `None` for any field keeps that collection unbounded. Hosts that
//! deliberately need the legacy behavior can use
//! [`SessionRetentionLimits::unbounded`].
//!
//! All caps are **soft**: when a store hits its cap, the oldest entry
//! is dropped on insert. The framework never returns errors from cap
//! enforcement.

/// Per-session in-memory retention caps. Built via
/// [`SessionOptions::with_retention_limits`](crate::agent_api::SessionOptions::with_retention_limits)
/// or by constructing the struct directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionRetentionLimits {
    /// Maximum number of runs retained in
    /// [`InMemoryRunStore`](crate::run::InMemoryRunStore).
    ///
    /// When a new run is created past this cap, the **oldest** run
    /// (by insertion order) is dropped along with its events.
    /// `None` keeps all runs.
    pub max_runs_retained: Option<usize>,

    /// Maximum number of event records retained per run in
    /// [`InMemoryRunStore`](crate::run::InMemoryRunStore).
    ///
    /// When a run accumulates more events than this, the oldest
    /// events are FIFO-dropped. The run snapshot's `event_count`
    /// is **not** decremented — it remains the total ever recorded.
    /// `None` keeps all events.
    pub max_events_per_run: Option<usize>,

    /// Maximum serialized bytes retained for event records in each run.
    ///
    /// This cap is enforced together with [`Self::max_events_per_run`].
    /// Oldest records are FIFO-dropped until both limits are satisfied. A
    /// single record larger than the cap is therefore not retained, while
    /// the run's cumulative `event_count` and state transition still advance.
    /// `None` does not impose a byte limit.
    pub max_event_bytes_per_run: Option<usize>,

    /// Maximum number of events retained in
    /// [`InMemoryTraceSink`](crate::trace::InMemoryTraceSink).
    ///
    /// When the sink reaches this cap, the oldest event is dropped
    /// on each new write. `None` keeps all events.
    pub max_trace_events: Option<usize>,

    /// Maximum number of **terminal** (Completed / Failed / Cancelled)
    /// subagent task snapshots retained in
    /// [`InMemorySubagentTaskTracker`](crate::subagent_task_tracker::InMemorySubagentTaskTracker).
    /// Running tasks are never dropped.
    ///
    /// When the count of terminal entries exceeds this cap, the
    /// oldest terminal entry (by completion time) is dropped.
    /// `None` keeps all terminal entries.
    pub max_terminal_subagent_tasks: Option<usize>,
}

impl SessionRetentionLimits {
    /// Convenience builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Explicitly opt into unbounded in-memory retention for every store.
    pub const fn unbounded() -> Self {
        Self {
            max_runs_retained: None,
            max_events_per_run: None,
            max_event_bytes_per_run: None,
            max_trace_events: None,
            max_terminal_subagent_tasks: None,
        }
    }

    pub fn with_max_runs(mut self, n: usize) -> Self {
        self.max_runs_retained = Some(n);
        self
    }

    pub fn with_max_events_per_run(mut self, n: usize) -> Self {
        self.max_events_per_run = Some(n);
        self
    }

    pub fn with_max_event_bytes_per_run(mut self, n: usize) -> Self {
        self.max_event_bytes_per_run = Some(n);
        self
    }

    pub fn with_max_trace_events(mut self, n: usize) -> Self {
        self.max_trace_events = Some(n);
        self
    }

    pub fn with_max_terminal_subagent_tasks(mut self, n: usize) -> Self {
        self.max_terminal_subagent_tasks = Some(n);
        self
    }
}

impl Default for SessionRetentionLimits {
    fn default() -> Self {
        Self {
            max_runs_retained: Some(64),
            max_events_per_run: Some(2_048),
            max_event_bytes_per_run: Some(8 * 1024 * 1024),
            max_trace_events: Some(8_192),
            max_terminal_subagent_tasks: Some(512),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_finite_and_unbounded_is_explicit() {
        let defaults = SessionRetentionLimits::default();
        assert!(defaults.max_runs_retained.is_some());
        assert!(defaults.max_events_per_run.is_some());
        assert!(defaults.max_event_bytes_per_run.is_some());
        assert!(defaults.max_trace_events.is_some());
        assert!(defaults.max_terminal_subagent_tasks.is_some());
        assert_eq!(
            SessionRetentionLimits::unbounded(),
            SessionRetentionLimits {
                max_runs_retained: None,
                max_events_per_run: None,
                max_event_bytes_per_run: None,
                max_trace_events: None,
                max_terminal_subagent_tasks: None,
            }
        );
    }
}
