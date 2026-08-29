//! Non-sensitive session-owned memory maintenance diagnostics.

use super::*;
use a3s_code_core::memory::{
    MemoryMaintenanceHealth as RustMemoryMaintenanceHealth,
    MemoryMaintenanceJobHealth as RustMemoryMaintenanceJobHealth,
};

/// Current health of one scheduled memory maintenance job.
#[napi(object)]
#[derive(Clone)]
pub struct MemoryMaintenanceJobHealth {
    pub name: String,
    pub interval_ms: f64,
    pub worker_alive: bool,
    pub run_in_progress: bool,
    pub successful_runs: f64,
    pub failed_runs: f64,
    pub total_affected_items: f64,
    pub last_affected_items: Option<f64>,
    pub last_error: Option<String>,
}

impl From<RustMemoryMaintenanceJobHealth> for MemoryMaintenanceJobHealth {
    fn from(value: RustMemoryMaintenanceJobHealth) -> Self {
        Self {
            name: value.name,
            interval_ms: value.interval_ms as f64,
            worker_alive: value.worker_alive,
            run_in_progress: value.run_in_progress,
            successful_runs: value.successful_runs as f64,
            failed_runs: value.failed_runs as f64,
            total_affected_items: value.total_affected_items as f64,
            last_affected_items: value.last_affected_items.map(|count| count as f64),
            last_error: value.last_error,
        }
    }
}

/// Non-sensitive point-in-time snapshot of session-owned maintenance.
#[napi(object)]
#[derive(Clone)]
pub struct MemoryMaintenanceHealth {
    pub phase: String,
    pub jobs: Vec<MemoryMaintenanceJobHealth>,
}

impl From<RustMemoryMaintenanceHealth> for MemoryMaintenanceHealth {
    fn from(value: RustMemoryMaintenanceHealth) -> Self {
        Self {
            phase: format!("{:?}", value.phase).to_ascii_lowercase(),
            jobs: value.jobs.into_iter().map(Into::into).collect(),
        }
    }
}

#[napi]
impl Session {
    /// Observe periodic pruning and host-owned consolidation for this session.
    #[napi]
    pub fn memory_maintenance_health(&self) -> MemoryMaintenanceHealth {
        self.inner.memory_maintenance_health().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use a3s_code_core::memory::{
        MemoryMaintenanceJobHealth as RustJobHealth, MemoryMaintenancePhase,
    };

    #[test]
    fn health_conversion_preserves_bounded_diagnostics() {
        let converted = MemoryMaintenanceHealth::from(RustMemoryMaintenanceHealth {
            phase: MemoryMaintenancePhase::Degraded,
            jobs: vec![RustJobHealth {
                name: "prune_v1".into(),
                interval_ms: 1_000,
                worker_alive: true,
                run_in_progress: false,
                successful_runs: 2,
                failed_runs: 1,
                total_affected_items: 7,
                last_affected_items: Some(3),
                last_error: Some("bounded failure".into()),
            }],
        });
        assert_eq!(converted.phase, "degraded");
        assert_eq!(converted.jobs.len(), 1);
        assert_eq!(converted.jobs[0].total_affected_items, 7.0);
        assert_eq!(
            converted.jobs[0].last_error.as_deref(),
            Some("bounded failure")
        );
    }
}
