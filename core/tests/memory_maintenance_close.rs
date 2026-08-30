use a3s_code_core::memory::{
    AgentMemory, MemoryMaintenanceContext, MemoryMaintenanceJob, MemoryMaintenanceOptions,
    MemoryMaintenanceOutcome, MemoryMaintenancePhase, MemoryMaintenanceRuntime,
    ScheduledMemoryMaintenance,
};
use a3s_memory::InMemoryStore;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct NonCooperativeJob {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl MemoryMaintenanceJob for NonCooperativeJob {
    async fn run(
        &self,
        _context: &MemoryMaintenanceContext,
        _cancellation: CancellationToken,
    ) -> anyhow::Result<MemoryMaintenanceOutcome> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(MemoryMaintenanceOutcome::default())
    }
}

async fn settle_workers() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn close_cancels_then_boundedly_aborts_a_non_cooperative_job() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let job = ScheduledMemoryMaintenance::try_new(
        "non_cooperative",
        Duration::from_secs(1),
        Arc::new(NonCooperativeJob {
            started: started.clone(),
            release: release.clone(),
        }),
    )
    .unwrap();
    let options = MemoryMaintenanceOptions::new()
        .with_job(job)
        .try_with_shutdown_timeout(Duration::from_secs(2))
        .unwrap();
    let runtime = MemoryMaintenanceRuntime::start(
        "bounded-close-owner",
        Arc::new(AgentMemory::new(Arc::new(InMemoryStore::new()))),
        options,
    )
    .unwrap();

    settle_workers().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    started.notified().await;
    assert!(runtime.health().jobs[0].run_in_progress);

    let closing_runtime = runtime.clone();
    let close = tokio::spawn(async move { closing_runtime.close().await });
    settle_workers().await;
    assert!(
        !close.is_finished(),
        "close dropped the job instead of awaiting cancellation settlement"
    );

    tokio::time::sleep(Duration::from_secs(2)).await;
    settle_workers().await;
    let report = close.await.unwrap();
    assert_eq!(report.jobs_joined, 0);
    assert_eq!(report.jobs_aborted, 1);
    assert_eq!(report.join_failures, 0);
    assert!(!report.is_clean());
    assert_eq!(runtime.health().phase, MemoryMaintenancePhase::Closed);
    release.notify_one();
}
