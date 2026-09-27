use super::*;

/// Scopes a phase to a region of work: entered on creation, closed on drop, so no failure return can leave the phase open across a retry wait.
#[must_use = "the phase closes when this guard drops"]
pub struct PhaseScope(());

impl Drop for PhaseScope {
    fn drop(&mut self) {
        if let Some(timer) = current() {
            timer.close_open_phase();
        }
    }
}

/// Enter `phase` for the lifetime of the returned guard.
pub fn phase_scope(phase: StartupPhase) -> PhaseScope {
    enter(phase);
    PhaseScope(())
}

/// The obligation to end startup exactly once; a dropped token ends startup itself and logs a warning, so forgotten paths are visible.
#[must_use = "startup must be finished or abandoned"]
pub struct PendingStartup {
    ended: bool,
}

impl PendingStartup {
    /// One per interactive or headless process; utility commands call [`mark_utility_process`] instead.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        PendingStartup { ended: false }
    }

    /// Records the startup total with `outcome` and ends recording.
    pub fn finish(mut self, outcome: StartupOutcome) {
        report_total(outcome);
        self.ended = true;
    }

    /// Ends recording without a total, for a run the user cancelled or one that never was a startup.
    pub fn abandon(mut self) {
        clear();
        self.ended = true;
    }

    /// Finishes a token still held in an `Option`; does nothing once taken.
    pub fn finish_held(token: &mut Option<Self>, outcome: StartupOutcome) {
        if let Some(pending) = token.take() {
            pending.finish(outcome);
        }
    }
}

impl Drop for PendingStartup {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        tracing::warn!("startup was never finished; ending recording");
        crate::unified_log::warn("startup never finished", None, None);
        clear();
    }
}

/// A deadline for a readiness-path network step.
/// Naming the phase and bounding the wait are one call, so neither can be forgotten.
pub struct ReadinessBudget {
    limit: Duration,
}

impl ReadinessBudget {
    pub const fn new(limit: Duration) -> Self {
        Self { limit }
    }

    /// Run `fut` under the budget, attributed to `phase` for exactly the run's duration.
    /// Returns `None` on timeout, after logging, instead of blocking readiness.
    pub async fn run<T>(
        &self,
        phase: StartupPhase,
        fut: impl std::future::Future<Output = T>,
    ) -> Option<T> {
        let _scope = phase_scope(phase);
        match tokio::time::timeout(self.limit, fut).await {
            Ok(value) => Some(value),
            Err(_) => {
                tracing::warn!(
                    phase = phase.label(),
                    limit_secs = self.limit.as_secs(),
                    "readiness step hit its budget"
                );
                crate::unified_log::warn(
                    "readiness step hit its budget",
                    None,
                    Some(
                        serde_json::json!({ "phase": phase.label(), "limit_secs": self.limit.as_secs() }),
                    ),
                );
                None
            }
        }
    }
}
