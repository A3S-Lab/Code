//! Cron-driven schedules for filesystem-first agents (P1).
//!
//! A `schedules/<name>.md` file (parsed into a [`ScheduleSpec`] by the agent-dir
//! convention) fires a markdown prompt into the agent on a cron cadence. The
//! firing itself goes through a [`ScheduleSink`] — the serve daemon implements it
//! to route the prompt through `AgentSession::send`, so a scheduled turn is a
//! FULL harness turn (context, safety gate, verification), never a raw model call.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use cron::Schedule;
use tokio_util::sync::CancellationToken;

use crate::config::ScheduleSpec;
use crate::error::{CodeError, Result};

/// A parsed, fireable schedule: a [`ScheduleSpec`] with its compiled cron.
pub struct ScheduledJob {
    pub spec: ScheduleSpec,
    schedule: Schedule,
}

impl ScheduledJob {
    /// Parse a spec's cron. Accepts standard 5-field cron
    /// (`min hour dom mon dow`) and the 6-field form (`sec min hour dom mon dow`).
    pub fn parse(spec: ScheduleSpec) -> Result<Self> {
        let schedule = Schedule::from_str(&normalize_cron(&spec.cron)).map_err(|e| {
            CodeError::Context(format!(
                "schedule '{}' has an invalid cron '{}': {e}",
                spec.name, spec.cron
            ))
        })?;
        Ok(Self { spec, schedule })
    }

    /// The next fire time strictly after `now` (None if it never fires again).
    pub fn next_fire_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.schedule.after(&now).next()
    }
}

/// Normalize a cron expression to the 6-field form the `cron` crate expects:
/// standard 5-field cron gets a leading `0` seconds field.
fn normalize_cron(expr: &str) -> String {
    let expr = expr.trim();
    match expr.split_whitespace().count() {
        5 => format!("0 {expr}"),
        _ => expr.to_string(),
    }
}

/// What to do when a schedule fires. The serve daemon implements this to drive
/// the schedule's markdown prompt into `AgentSession::send` (a full harness turn).
#[async_trait::async_trait]
pub trait ScheduleSink: Send + Sync {
    async fn fire(&self, spec: &ScheduleSpec);
}

/// Drives a set of cron schedules: for each enabled job, sleep until its next
/// fire, invoke the sink, repeat — until cancelled.
pub struct Scheduler {
    jobs: Vec<ScheduledJob>,
}

impl Scheduler {
    /// Build from specs, skipping disabled ones. Errors on any invalid cron.
    pub fn new(specs: impl IntoIterator<Item = ScheduleSpec>) -> Result<Self> {
        let mut jobs = Vec::new();
        let mut names = std::collections::HashSet::new();
        for spec in specs.into_iter().filter(|spec| spec.enabled) {
            if !names.insert(spec.name.clone()) {
                return Err(CodeError::Context(format!(
                    "duplicate enabled schedule name: {}",
                    spec.name
                )));
            }
            jobs.push(ScheduledJob::parse(spec)?);
        }
        Ok(Self { jobs })
    }

    /// Number of active (enabled, valid) jobs.
    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Run until `cancel` fires; one independent loop per job.
    ///
    /// This compatibility entry point preserves the original fire-and-forget
    /// error semantics and returns immediately when there are no jobs. Daemon
    /// hosts that need observable failures should call [`Self::run_observed`].
    pub async fn run(self, sink: Arc<dyn ScheduleSink>, cancel: CancellationToken) {
        if self.jobs.is_empty() {
            return;
        }
        let _ = self.run_jobs(sink, cancel).await;
    }

    /// Run until cancellation while preserving worker failures for the host.
    ///
    /// An empty scheduler remains alive until cancellation so a successfully
    /// prepared daemon does not exit merely because it has no cron jobs.
    pub async fn run_observed(
        self,
        sink: Arc<dyn ScheduleSink>,
        cancel: CancellationToken,
    ) -> Result<()> {
        if self.jobs.is_empty() {
            cancel.cancelled().await;
            return Ok(());
        }
        self.run_jobs(sink, cancel).await
    }

    async fn run_jobs(self, sink: Arc<dyn ScheduleSink>, cancel: CancellationToken) -> Result<()> {
        let mut workers = tokio::task::JoinSet::new();
        for job in self.jobs {
            let sink = Arc::clone(&sink);
            let cancel = cancel.clone();
            workers.spawn(run_job(job, sink, cancel));
        }
        while let Some(joined) = workers.join_next().await {
            joined.map_err(|error| {
                CodeError::Context(format!("schedule worker terminated unexpectedly: {error}"))
            })?;
        }
        Ok(())
    }
}

async fn run_job(job: ScheduledJob, sink: Arc<dyn ScheduleSink>, cancel: CancellationToken) {
    loop {
        let now = Utc::now();
        let Some(next) = job.next_fire_after(now) else {
            return; // never fires again
        };
        let wait = (next - now)
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(0));
        tokio::select! {
            _ = tokio::time::sleep(wait) => {
                tokio::select! {
                    _ = sink.fire(&job.spec) => {}
                    _ = cancel.cancelled() => return,
                }
            }
            _ = cancel.cancelled() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    fn spec(name: &str, cron: &str, enabled: bool) -> ScheduleSpec {
        ScheduleSpec {
            name: name.to_string(),
            cron: cron.to_string(),
            prompt: "do the thing".to_string(),
            enabled,
        }
    }

    #[test]
    fn parses_5_field_cron_and_computes_next_fire() {
        let job = ScheduledJob::parse(spec("daily", "0 9 * * *", true)).unwrap();
        // Before 09:00 → fires at 09:00 the same day.
        let before = DateTime::parse_from_rfc3339("2026-01-01T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            job.next_fire_after(before).unwrap().to_rfc3339(),
            "2026-01-01T09:00:00+00:00"
        );
        // After 09:00 → next is 09:00 the following day.
        let after = DateTime::parse_from_rfc3339("2026-01-01T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            job.next_fire_after(after).unwrap().to_rfc3339(),
            "2026-01-02T09:00:00+00:00"
        );
    }

    #[test]
    fn invalid_cron_is_an_error() {
        assert!(ScheduledJob::parse(spec("bad", "not a cron", true)).is_err());
    }

    #[test]
    fn parses_6_field_cron_with_seconds() {
        // 6-field form is passed through verbatim (the 5-field form gets a leading
        // `0` seconds field). "30 0 9 * * *" → 09:00:30 daily.
        let job = ScheduledJob::parse(spec("sec", "30 0 9 * * *", true)).unwrap();
        let before = DateTime::parse_from_rfc3339("2026-01-01T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            job.next_fire_after(before).unwrap().to_rfc3339(),
            "2026-01-01T09:00:30+00:00"
        );
    }

    #[test]
    fn scheduler_skips_disabled_and_rejects_bad_cron() {
        let s = Scheduler::new([
            spec("a", "0 9 * * *", true),
            spec("b", "*/5 * * * *", false), // disabled → skipped
        ])
        .unwrap();
        assert_eq!(s.job_count(), 1);

        assert!(Scheduler::new([spec("bad", "xyz", true)]).is_err());
    }

    #[test]
    fn scheduler_rejects_duplicate_enabled_names() {
        let error = Scheduler::new([
            spec("daily", "0 9 * * *", true),
            spec("daily", "0 10 * * *", true),
        ])
        .err()
        .expect("duplicate enabled schedule names must fail startup");
        assert!(error
            .to_string()
            .contains("duplicate enabled schedule name: daily"));

        let scheduler = Scheduler::new([
            spec("daily", "0 9 * * *", true),
            spec("daily", "0 10 * * *", false),
        ])
        .unwrap();
        assert_eq!(scheduler.job_count(), 1);
    }

    #[tokio::test]
    async fn compatibility_run_with_no_jobs_returns_immediately() {
        struct NoopSink;

        #[async_trait::async_trait]
        impl ScheduleSink for NoopSink {
            async fn fire(&self, _spec: &ScheduleSpec) {}
        }

        let scheduler = Scheduler::new(Vec::<ScheduleSpec>::new()).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            scheduler.run(Arc::new(NoopSink), CancellationToken::new()),
        )
        .await
        .expect("the compatibility API must keep its empty-scheduler behavior");
    }

    #[tokio::test]
    async fn fires_at_least_once_then_stops_on_cancel() {
        struct CountSink(Arc<AtomicUsize>);
        #[async_trait::async_trait]
        impl ScheduleSink for CountSink {
            async fn fire(&self, _spec: &ScheduleSpec) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let fires = Arc::new(AtomicUsize::new(0));
        // 6-field cron = every second; a short real wait yields >= 1 fire without
        // paused-clock plumbing (run_job mixes wall-clock next-fire with the timer).
        let s = Scheduler::new([spec("sec", "* * * * * *", true)]).unwrap();
        let cancel = CancellationToken::new();
        let sink = Arc::new(CountSink(Arc::clone(&fires)));
        let handle = tokio::spawn(s.run_observed(sink, cancel.clone()));
        tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
        cancel.cancel();
        handle.await.unwrap().unwrap();
        assert!(
            fires.load(Ordering::SeqCst) >= 1,
            "expected at least one fire in ~2.2s, got {}",
            fires.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_an_in_flight_schedule_fire() {
        struct BlockingSink(Arc<Notify>);

        #[async_trait::async_trait]
        impl ScheduleSink for BlockingSink {
            async fn fire(&self, _spec: &ScheduleSpec) {
                self.0.notify_one();
                std::future::pending::<()>().await;
            }
        }

        let started = Arc::new(Notify::new());
        let scheduler = Scheduler::new([spec("blocked", "* * * * * *", true)]).unwrap();
        let cancel = CancellationToken::new();
        let run_cancel = cancel.clone();
        let sink: Arc<dyn ScheduleSink> = Arc::new(BlockingSink(Arc::clone(&started)));
        let run = tokio::spawn(scheduler.run_observed(sink, run_cancel));

        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .expect("schedule fire must start");
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_millis(250), run)
            .await
            .expect("cancellation must interrupt the in-flight fire")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn observed_run_surfaces_any_worker_panic_without_waiting_for_earlier_workers() {
        struct PanicSink;

        #[async_trait::async_trait]
        impl ScheduleSink for PanicSink {
            async fn fire(&self, spec: &ScheduleSpec) {
                if spec.name == "blocked" {
                    std::future::pending::<()>().await;
                }
                panic!("worker panic");
            }
        }

        let scheduler = Scheduler::new([
            spec("blocked", "* * * * * *", true),
            spec("panics", "* * * * * *", true),
        ])
        .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            scheduler.run_observed(Arc::new(PanicSink), CancellationToken::new()),
        )
        .await
        .expect("a later worker panic must not be hidden by an earlier worker")
        .unwrap_err();
        assert!(result
            .to_string()
            .contains("schedule worker terminated unexpectedly"));
    }
}
