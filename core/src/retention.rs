//! In-memory retention limits for long-running sessions.
//!
//! The framework's in-memory stores
//! ([`InMemoryRunStore`](crate::run::InMemoryRunStore),
//! [`InMemoryTraceSink`](crate::trace::InMemoryTraceSink),
//! [`InMemorySubagentTaskTracker`](crate::subagent_task_tracker::InMemorySubagentTaskTracker))
//! accumulate unboundedly by default — fine for short-lived runs, a
//! memory leak for sessions that live for hours or days under cluster
//! workloads.
//!
//! `SessionRetentionLimits` lets the host cap each store with a FIFO
//! policy. `None` for any field keeps the unbounded default, so
//! callers that don't set anything see no behaviour change.
//!
//! All caps are **soft**: when a store hits its cap, the oldest entry
//! is dropped on insert. The framework never returns errors from cap
//! enforcement.

/// Per-session in-memory retention caps. Built via
/// [`SessionOptions::with_retention_limits`](crate::agent_api::SessionOptions::with_retention_limits)
/// or by constructing the struct directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionRetentionLimits {
    /// Maximum number of runs retained in
    /// [`InMemoryRunStore`](crate::run::InMemoryRunStore).
    ///
    /// When a new run is created past this cap, the **oldest** run
    /// (by insertion order) is dropped along with its events.
    /// `None` (default) keeps all runs.
    pub max_runs_retained: Option<usize>,

    /// Maximum number of event records retained per run in
    /// [`InMemoryRunStore`](crate::run::InMemoryRunStore).
    ///
    /// When a run accumulates more events than this, the oldest
    /// events are FIFO-dropped. The run snapshot's `event_count`
    /// is **not** decremented — it remains the total ever recorded.
    /// `None` (default) keeps all events.
    pub max_events_per_run: Option<usize>,

    /// Maximum number of events retained in
    /// [`InMemoryTraceSink`](crate::trace::InMemoryTraceSink).
    ///
    /// When the sink reaches this cap, the oldest event is dropped
    /// on each new write. `None` (default) keeps all events.
    pub max_trace_events: Option<usize>,

    /// Maximum number of **terminal** (Completed / Failed / Cancelled)
    /// subagent task snapshots retained in
    /// [`InMemorySubagentTaskTracker`](crate::subagent_task_tracker::InMemorySubagentTaskTracker).
    /// Running tasks are never dropped.
    ///
    /// When the count of terminal entries exceeds this cap, the
    /// oldest terminal entry (by completion time) is dropped.
    /// `None` (default) keeps all terminal entries.
    pub max_terminal_subagent_tasks: Option<usize>,
}

impl SessionRetentionLimits {
    /// Convenience builder.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_runs(mut self, n: usize) -> Self {
        self.max_runs_retained = Some(n);
        self
    }

    pub fn with_max_events_per_run(mut self, n: usize) -> Self {
        self.max_events_per_run = Some(n);
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
