use a3s_code_core::memory::{
    AgentMemory, MemoryConfig, MemoryMaintenanceContext, MemoryMaintenanceError,
    MemoryMaintenanceJob, MemoryMaintenanceOptions, MemoryMaintenanceOutcome,
    MemoryMaintenancePhase, MemoryMaintenanceRuntime, ScheduledMemoryMaintenance,
};
use a3s_code_core::{
    Agent, CodeConfig, CodeError, ModelConfig, ModelModalities, ProviderConfig,
    SessionBuildResource, SessionOptions,
};
use a3s_memory::{InMemoryStore, MemoryItem, MemoryStore, PrunePolicy};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct CountingConsolidator {
    runs: AtomicUsize,
}

#[derive(Default)]
struct RecoveringConsolidator {
    runs: AtomicUsize,
}

#[async_trait]
impl MemoryMaintenanceJob for RecoveringConsolidator {
    async fn run(
        &self,
        _context: &MemoryMaintenanceContext,
        _cancellation: CancellationToken,
    ) -> anyhow::Result<MemoryMaintenanceOutcome> {
        let run = self.runs.fetch_add(1, Ordering::SeqCst);
        if run == 0 {
            anyhow::bail!("verification service unavailable");
        }
        Ok(MemoryMaintenanceOutcome::new(2))
    }
}

#[async_trait]
impl MemoryMaintenanceJob for CountingConsolidator {
    async fn run(
        &self,
        context: &MemoryMaintenanceContext,
        cancellation: CancellationToken,
    ) -> anyhow::Result<MemoryMaintenanceOutcome> {
        assert_eq!(context.owner_id(), "memory-maintenance-session");
        assert!(!cancellation.is_cancelled());
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(MemoryMaintenanceOutcome::new(1))
    }
}

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                family: "claude-sonnet".to_string(),
                api_key: None,
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        memory: Some(MemoryConfig {
            prune_policy: Some(PrunePolicy {
                max_age_days: 90,
                min_importance_to_keep: 0.5,
                max_items: 0,
            }),
            prune_interval_secs: 10,
            llm_extraction: false,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn maintenance_public_types_are_thread_safe_and_validate_bounds() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MemoryMaintenanceContext>();
    assert_send_sync::<MemoryMaintenanceOptions>();
    assert_send_sync::<MemoryMaintenanceOutcome>();
    assert_send_sync::<ScheduledMemoryMaintenance>();

    let job = Arc::new(CountingConsolidator::default());
    assert!(ScheduledMemoryMaintenance::try_new("", Duration::from_secs(1), job.clone()).is_err());
    assert!(ScheduledMemoryMaintenance::try_new("job", Duration::ZERO, job).is_err());
    assert!(MemoryMaintenanceOptions::new()
        .try_with_shutdown_timeout(Duration::ZERO)
        .is_err());
}

#[tokio::test(start_paused = true)]
async fn session_owns_pruning_and_host_consolidation_until_bounded_close() {
    let store = Arc::new(InMemoryStore::new());
    let mut old = MemoryItem::new("obsolete low-value memory").with_importance(0.1);
    old.timestamp = chrono::Utc::now() - chrono::Duration::days(120);
    store.store(old).await.unwrap();

    let consolidator = Arc::new(CountingConsolidator::default());
    let consolidation = ScheduledMemoryMaintenance::try_new(
        "verified_consolidation",
        Duration::from_secs(10),
        consolidator.clone(),
    )
    .unwrap();
    let options = SessionOptions::new()
        .with_session_id("memory-maintenance-session")
        .with_memory(store.clone())
        .with_memory_maintenance(MemoryMaintenanceOptions::new().with_job(consolidation));
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .unwrap();

    let initial = session.memory_maintenance_health();
    assert_eq!(initial.phase, MemoryMaintenancePhase::Running);
    assert_eq!(initial.jobs.len(), 2);
    assert_eq!(store.count().await.unwrap(), 1);
    assert_eq!(consolidator.runs.load(Ordering::SeqCst), 0);

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(store.count().await.unwrap(), 0);
    assert_eq!(consolidator.runs.load(Ordering::SeqCst), 1);
    let running = session.memory_maintenance_health();
    assert_eq!(running.phase, MemoryMaintenancePhase::Running);
    assert!(running.jobs.iter().all(|job| job.successful_runs == 1));
    assert!(running.jobs.iter().all(|job| job.last_error.is_none()));

    session.close().await;
    let closed = session.memory_maintenance_health();
    assert_eq!(closed.phase, MemoryMaintenancePhase::Closed);
    assert!(closed.jobs.iter().all(|job| !job.worker_alive));

    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::task::yield_now().await;
    assert_eq!(consolidator.runs.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn agent_memory_construction_is_inert_and_runtime_ownership_is_exclusive() {
    let store = Arc::new(InMemoryStore::new());
    let mut old = MemoryItem::new("old unowned memory").with_importance(0.1);
    old.timestamp = chrono::Utc::now() - chrono::Duration::days(120);
    store.store(old).await.unwrap();
    let memory = Arc::new(AgentMemory::with_config(
        store.clone(),
        MemoryConfig {
            prune_policy: Some(PrunePolicy {
                max_age_days: 90,
                min_importance_to_keep: 0.5,
                max_items: 0,
            }),
            prune_interval_secs: 10,
            ..Default::default()
        },
    ));

    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::task::yield_now().await;
    assert_eq!(store.count().await.unwrap(), 1);

    let runtime = MemoryMaintenanceRuntime::start(
        "explicit-owner",
        memory.clone(),
        MemoryMaintenanceOptions::new(),
    )
    .unwrap();
    assert!(matches!(
        MemoryMaintenanceRuntime::start(
            "competing-owner",
            memory.clone(),
            MemoryMaintenanceOptions::new(),
        ),
        Err(MemoryMaintenanceError::AlreadyOwned)
    ));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(store.count().await.unwrap(), 0);

    let first = runtime.close().await;
    let second = runtime.close().await;
    assert_eq!(first, second);
    assert!(first.is_clean());

    let restarted = MemoryMaintenanceRuntime::start(
        "replacement-owner",
        memory.clone(),
        MemoryMaintenanceOptions::new(),
    )
    .unwrap();
    drop(runtime);
    assert!(matches!(
        MemoryMaintenanceRuntime::start("third-owner", memory, MemoryMaintenanceOptions::new(),),
        Err(MemoryMaintenanceError::AlreadyOwned)
    ));
    assert!(restarted.close().await.is_clean());
}

#[tokio::test(start_paused = true)]
async fn failed_job_degrades_health_and_later_success_recovers_it() {
    let memory = Arc::new(AgentMemory::new(Arc::new(InMemoryStore::new())));
    let consolidator = Arc::new(RecoveringConsolidator::default());
    let job = ScheduledMemoryMaintenance::try_new(
        "recovering_consolidation",
        Duration::from_secs(5),
        consolidator,
    )
    .unwrap();
    let runtime = MemoryMaintenanceRuntime::start(
        "health-owner",
        memory,
        MemoryMaintenanceOptions::new().with_job(job),
    )
    .unwrap();

    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    let degraded = runtime.health();
    assert_eq!(degraded.phase, MemoryMaintenancePhase::Degraded);
    assert_eq!(degraded.jobs[0].failed_runs, 1);
    assert!(degraded.jobs[0]
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("verification service")));

    tokio::time::advance(Duration::from_secs(5)).await;
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    let recovered = runtime.health();
    assert_eq!(recovered.phase, MemoryMaintenancePhase::Running);
    assert_eq!(recovered.jobs[0].successful_runs, 1);
    assert_eq!(recovered.jobs[0].total_affected_items, 2);
    assert!(recovered.jobs[0].last_error.is_none());
    assert!(runtime.close().await.is_clean());
}

#[tokio::test]
async fn maintenance_requires_async_session_build_and_rejects_zero_prune_interval() {
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let sync_error = agent
        .session(
            workspace.path().display().to_string(),
            Some(SessionOptions::new().with_memory(Arc::new(InMemoryStore::new()))),
        )
        .expect_err("owned maintenance must not be silently skipped by sync construction");
    assert!(matches!(
        sync_error,
        CodeError::AsyncSessionBuildRequired {
            resource: SessionBuildResource::MemoryMaintenance
        }
    ));

    let mut invalid = offline_config();
    invalid.memory.as_mut().unwrap().prune_interval_secs = 0;
    let agent = Agent::from_config(invalid).await.unwrap();
    let error = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(SessionOptions::new().with_memory(Arc::new(InMemoryStore::new()))),
        )
        .await
        .expect_err("zero interval must fail before spawning maintenance");
    assert!(matches!(
        error,
        CodeError::SessionInitialization {
            resource: SessionBuildResource::MemoryMaintenance,
            ..
        }
    ));
}
