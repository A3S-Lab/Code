//! S-PS-01: admission stays inside capacity across a thousand mixed-priority
//! cycles, and a queued low-priority job is not starved past the aging window.

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::queue::{LaneHandlerConfig, SessionLane, SessionQueueConfig, TaskHandlerMode};
use a3s_code_core::{Agent, SessionOptions, TaskPriority, TaskScheduler, TaskSchedulerConfig};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// One aging step. Maintenance is four levels from Urgent and stops at
/// Interactive, so it needs three steps to enter the interactive class.
const AGING_INTERVAL_MS: u64 = 20;
const MAINTENANCE_TO_INTERACTIVE_LEVELS: u64 = 3;
const FAIRNESS_WINDOW: Duration =
    Duration::from_millis(AGING_INTERVAL_MS * MAINTENANCE_TO_INTERACTIVE_LEVELS);

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
        ..Default::default()
    }
}

fn priority_for(index: usize) -> TaskPriority {
    match index % 4 {
        0 => TaskPriority::Interactive,
        1 => TaskPriority::Foreground,
        2 => TaskPriority::Background,
        _ => TaskPriority::Maintenance,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "S-PS-01 soak: 1000 admissions and the aging fairness window"]
async fn soak_priority_admission_stays_within_capacity() {
    let scheduler = Arc::new(
        TaskScheduler::new(TaskSchedulerConfig {
            max_active: 2,
            aging_interval_ms: AGING_INTERVAL_MS,
        })
        .expect("scheduler"),
    );
    let cancel = CancellationToken::new();
    let workspace = tempfile::tempdir().expect("workspace");
    let agent = Agent::from_config(offline_config()).await.expect("agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("s-ps-01")
                    .with_model("anthropic/claude-sonnet-4-20250514")
                    .with_queue_config(SessionQueueConfig {
                        execute_max_concurrency: 2,
                        ..SessionQueueConfig::default()
                    }),
            ),
        )
        .await
        .expect("session");

    let started = Arc::new(AtomicUsize::new(0));
    let mut joins = Vec::with_capacity(1000);
    for index in 0..1000 {
        let scheduler = Arc::clone(&scheduler);
        let cancel = cancel.clone();
        let started = Arc::clone(&started);
        joins.push(tokio::spawn(async move {
            started.fetch_add(1, Ordering::SeqCst);
            let lease = scheduler
                .acquire(priority_for(index), format!("cycle-{index}"), &cancel)
                .await
                .expect("admit");
            let stats = scheduler.stats().await.expect("stats");
            assert!(stats.active <= stats.max_active);
            assert!(stats.active <= 2);
            assert!(stats.active + stats.pending <= started.load(Ordering::SeqCst));
            drop(lease);
        }));
        if index % 20 == 0 {
            let mode = if (index / 20) % 2 == 0 {
                TaskHandlerMode::External
            } else {
                TaskHandlerMode::Internal
            };
            session
                .set_lane_handler(
                    SessionLane::Query,
                    LaneHandlerConfig {
                        mode,
                        timeout_ms: 30_000,
                    },
                )
                .await
                .expect("handler replacement");
        }
    }
    for join in joins {
        join.await.expect("admission task");
    }
    let health = scheduler.health().await.expect("health");
    assert_eq!(health.active, 0);
    assert_eq!(health.pending, 0);
    assert_eq!(health.admitted, 1000);
    assert_eq!(health.released, 1000);

    let hold_a = scheduler
        .acquire(TaskPriority::Foreground, "hold-a", &cancel)
        .await
        .expect("hold");
    let hold_b = scheduler
        .acquire(TaskPriority::Foreground, "hold-b", &cancel)
        .await
        .expect("hold");
    let (order_tx, mut order_rx) = mpsc::channel(2);
    let maintenance = {
        let scheduler = Arc::clone(&scheduler);
        let cancel = cancel.clone();
        let order_tx = order_tx.clone();
        tokio::spawn(async move {
            let lease = scheduler
                .acquire(TaskPriority::Maintenance, "aged-maintenance", &cancel)
                .await
                .expect("maintenance");
            order_tx.send("maintenance").await.expect("order");
            drop(lease);
        })
    };
    tokio::time::sleep(FAIRNESS_WINDOW + Duration::from_millis(AGING_INTERVAL_MS)).await;
    let interactive = {
        let scheduler = Arc::clone(&scheduler);
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let lease = scheduler
                .acquire(TaskPriority::Interactive, "fresh-interactive", &cancel)
                .await
                .expect("interactive");
            order_tx.send("interactive").await.expect("order");
            drop(lease);
        })
    };
    tokio::time::sleep(Duration::from_millis(AGING_INTERVAL_MS)).await;
    drop(hold_a);
    let first = tokio::time::timeout(Duration::from_secs(2), order_rx.recv())
        .await
        .expect("fairness window must admit someone")
        .expect("order");
    assert_eq!(
        first,
        "maintenance",
        "maintenance waited past {}ms while a newer interactive job was queued",
        FAIRNESS_WINDOW.as_millis()
    );
    drop(hold_b);
    maintenance.await.expect("maintenance join");
    interactive.await.expect("interactive join");
}
