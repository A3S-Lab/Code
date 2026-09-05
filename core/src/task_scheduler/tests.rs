use super::*;
use std::time::Duration;

fn scheduler(max_active: usize, aging_interval_ms: u64) -> TaskScheduler {
    TaskScheduler::new(TaskSchedulerConfig {
        max_active,
        aging_interval_ms,
    })
    .unwrap()
}

#[test]
fn priority_names_are_stable_and_reject_unknown_values() {
    assert_eq!("user".parse(), Ok(TaskPriority::Interactive));
    assert_eq!("background".parse(), Ok(TaskPriority::Background));
    assert!("eventually".parse::<TaskPriority>().is_err());
}

#[test]
fn quota_scope_and_descriptor_validation_are_bounded() {
    assert!(TaskSchedulerQuota::for_scope("", 1).is_err());
    assert!(TaskSchedulerQuota::for_scope("run:test\n", 1).is_err());
    assert!(TaskSchedulerQuota::for_scope("run:test\r", 1).is_err());
    assert!(TaskSchedulerQuota::for_scope("run:\u{2028}test", 1).is_err());
    assert!(
        TaskSchedulerQuota::for_scope(&"x".repeat(TASK_SCHEDULER_MAX_SCOPE_BYTES + 1), 1).is_err()
    );

    let quota = TaskSchedulerQuota::for_scope("run:test", 2).unwrap();
    let encoded = serde_json::to_string(&quota).unwrap();
    assert_eq!(
        serde_json::from_str::<TaskSchedulerQuota>(&encoded).unwrap(),
        quota
    );

    let mut invalid = quota;
    invalid.max_active = 0;
    assert!(invalid.validate().is_err());
}

async fn wait_for_pending(scheduler: &TaskScheduler, expected: usize) {
    for _ in 0..100 {
        if scheduler.stats().await.unwrap().pending == expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("scheduler never reached {expected} pending tasks");
}

#[tokio::test]
async fn strict_priority_and_fifo_are_enforced_globally() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let blocker = scheduler
        .acquire(
            TaskPriority::Interactive,
            "blocker",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let (order_tx, mut order_rx) = mpsc::unbounded_channel();

    for (name, priority) in [
        ("background", TaskPriority::Background),
        ("interactive-1", TaskPriority::Interactive),
        ("foreground", TaskPriority::Foreground),
        ("interactive-2", TaskPriority::Interactive),
        ("urgent", TaskPriority::Urgent),
    ] {
        let expected = scheduler.stats().await.unwrap().pending + 1;
        let task_scheduler = Arc::clone(&scheduler);
        let order_tx = order_tx.clone();
        tokio::spawn(async move {
            let lease = task_scheduler
                .acquire(priority, name, &CancellationToken::new())
                .await
                .unwrap();
            order_tx.send(name).unwrap();
            drop(lease);
        });
        wait_for_pending(&scheduler, expected).await;
    }

    drop(blocker);
    let mut actual = Vec::new();
    for _ in 0..5 {
        actual.push(order_rx.recv().await.unwrap());
    }
    assert_eq!(
        actual,
        [
            "urgent",
            "interactive-1",
            "interactive-2",
            "foreground",
            "background"
        ]
    );
    scheduler.shutdown().await;
}

#[tokio::test]
async fn cancellation_does_not_consume_capacity() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let blocker = scheduler
        .acquire(
            TaskPriority::Interactive,
            "blocker",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let cancellation = CancellationToken::new();
    let cancelled_task = {
        let scheduler = Arc::clone(&scheduler);
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            scheduler
                .acquire(TaskPriority::Urgent, "cancelled", &cancellation)
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    cancellation.cancel();
    assert!(matches!(
        cancelled_task.await.unwrap(),
        Err(TaskSchedulerError::Cancelled)
    ));

    let next = {
        let scheduler = Arc::clone(&scheduler);
        tokio::spawn(async move {
            scheduler
                .acquire(TaskPriority::Background, "next", &CancellationToken::new())
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    drop(blocker);
    let lease = next.await.unwrap().unwrap();
    assert_eq!(scheduler.stats().await.unwrap().active, 1);
    drop(lease);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn aging_prevents_background_starvation() {
    let scheduler = Arc::new(scheduler(1, 2));
    let blocker = scheduler
        .acquire(
            TaskPriority::Interactive,
            "blocker",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let (order_tx, mut order_rx) = mpsc::unbounded_channel();
    let background = {
        let scheduler = Arc::clone(&scheduler);
        let order_tx = order_tx.clone();
        tokio::spawn(async move {
            let lease = scheduler
                .acquire(
                    TaskPriority::Background,
                    "old-background",
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            order_tx.send("background").unwrap();
            drop(lease);
        })
    };
    wait_for_pending(&scheduler, 1).await;
    tokio::time::sleep(Duration::from_millis(8)).await;
    let interactive = {
        let scheduler = Arc::clone(&scheduler);
        let order_tx = order_tx.clone();
        tokio::spawn(async move {
            let lease = scheduler
                .acquire(
                    TaskPriority::Interactive,
                    "new-interactive",
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            order_tx.send("interactive").unwrap();
            drop(lease);
        })
    };
    wait_for_pending(&scheduler, 2).await;
    drop(blocker);

    assert_eq!(order_rx.recv().await.unwrap(), "background");
    assert_eq!(order_rx.recv().await.unwrap(), "interactive");
    background.await.unwrap();
    interactive.await.unwrap();
    scheduler.shutdown().await;
}

#[tokio::test]
async fn shutdown_rejects_pending_and_waits_for_active_lease() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let blocker = scheduler
        .acquire(
            TaskPriority::Interactive,
            "blocker",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let pending = {
        let scheduler = Arc::clone(&scheduler);
        tokio::spawn(async move {
            scheduler
                .acquire(
                    TaskPriority::Background,
                    "pending",
                    &CancellationToken::new(),
                )
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    let shutdown = {
        let scheduler = Arc::clone(&scheduler);
        tokio::spawn(async move { scheduler.shutdown().await })
    };
    assert!(matches!(
        pending.await.unwrap(),
        Err(TaskSchedulerError::Closed)
    ));
    assert!(!shutdown.is_finished());
    drop(blocker);
    shutdown.await.unwrap();
    assert!(matches!(
        scheduler
            .acquire(TaskPriority::Urgent, "late", &CancellationToken::new())
            .await,
        Err(TaskSchedulerError::Closed)
    ));
}

#[tokio::test]
async fn stats_report_base_priority_occupancy() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let blocker = scheduler
        .acquire(
            TaskPriority::Foreground,
            "blocker",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let waiting = {
        let scheduler = Arc::clone(&scheduler);
        tokio::spawn(async move {
            scheduler
                .acquire(
                    TaskPriority::Maintenance,
                    "waiting",
                    &CancellationToken::new(),
                )
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    let stats = scheduler.stats().await.unwrap();
    assert_eq!(stats.max_active, 1);
    assert_eq!(stats.active_by_priority.foreground, 1);
    assert_eq!(stats.pending_by_priority.maintenance, 1);
    drop(blocker);
    drop(waiting.await.unwrap().unwrap());
    scheduler.shutdown().await;
}

#[tokio::test]
async fn health_reports_bounded_admission_and_wait_counters() {
    let scheduler = Arc::new(scheduler(1, 2));
    let blocker = scheduler
        .acquire(
            TaskPriority::Interactive,
            "health-blocker",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let cancellation = CancellationToken::new();
    let cancelled = {
        let scheduler = Arc::clone(&scheduler);
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            scheduler
                .acquire(TaskPriority::Background, "health-cancelled", &cancellation)
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    cancellation.cancel();
    assert!(matches!(
        cancelled.await.unwrap(),
        Err(TaskSchedulerError::Cancelled)
    ));

    let waiting = {
        let scheduler = Arc::clone(&scheduler);
        tokio::spawn(async move {
            scheduler
                .acquire(
                    TaskPriority::Foreground,
                    "health-waiting",
                    &CancellationToken::new(),
                )
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    tokio::time::sleep(Duration::from_millis(6)).await;
    drop(blocker);
    let lease = waiting.await.unwrap().unwrap();
    drop(lease);
    let health = scheduler.health().await.unwrap();
    assert_eq!(health.active, 0);
    assert_eq!(health.pending, 0);
    assert_eq!(health.admitted, 2);
    assert_eq!(health.released, 2);
    assert_eq!(health.cancelled, 1);
    assert!(health.aging_promotions >= 1);
    assert!(health.peak_active >= 1);
    assert!(health.total_wait_micros > 0);
    assert!(health.max_wait_micros >= health.average_wait_micros);
    assert!(!health.closed);
    scheduler.shutdown().await;
    assert!(scheduler.health().await.is_err());
}

#[tokio::test]
async fn aging_keeps_a_resumed_workflow_step_from_starving() {
    let scheduler = Arc::new(scheduler(1, 2));
    let blocker = scheduler
        .acquire(
            TaskPriority::Interactive,
            "resumed-run-blocker",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let (order_tx, mut order_rx) = mpsc::unbounded_channel();
    let resumed = {
        let scheduler = Arc::clone(&scheduler);
        let order_tx = order_tx.clone();
        tokio::spawn(async move {
            let lease = scheduler
                .acquire(
                    TaskPriority::Background,
                    "flow:resumed-run:step-1",
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            order_tx.send("resumed").unwrap();
            drop(lease);
        })
    };
    wait_for_pending(&scheduler, 1).await;
    tokio::time::sleep(Duration::from_millis(8)).await;

    // Simulate a stream of newly resumed interactive work arriving while
    // the original run waits. The aged continuation must be admitted
    // before those newer requests once capacity is released.
    let mut interactive = Vec::new();
    for index in 0..8 {
        let scheduler = Arc::clone(&scheduler);
        let wait_scheduler = Arc::clone(&scheduler);
        let order_tx = order_tx.clone();
        interactive.push(tokio::spawn(async move {
            let lease = scheduler
                .acquire(
                    TaskPriority::Interactive,
                    format!("flow:new-run-{index}:step-1"),
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            order_tx.send("interactive").unwrap();
            drop(lease);
        }));
        wait_for_pending(&wait_scheduler, index + 2).await;
    }
    let aged = scheduler.health().await.unwrap();
    assert!(aged.aging_promotions >= 1);
    drop(blocker);
    assert_eq!(order_rx.recv().await.unwrap(), "resumed");
    for task in interactive {
        task.await.unwrap();
    }
    resumed.await.unwrap();
    let health = scheduler.health().await.unwrap();
    assert!(health.aging_promotions >= 1);
    assert_eq!(health.pending, 0);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn identity_is_carried_by_global_admission_lease() {
    let scheduler = scheduler(1, 60_000);
    let identity = ExecutionIdentityV1::derive(
        crate::execution_identity::FLOW_STEP_IDENTITY_DOMAIN_V1,
        &serde_json::json!({
            "run_id": "run-1",
            "step_id": "step-1",
            "step_name": "read",
            "input": {"path": "README.md"},
        }),
    )
    .unwrap();
    let lease = scheduler
        .acquire_with_identity(
            TaskPriority::Foreground,
            "flow:run-1:step-1:read",
            Some(identity.clone()),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(lease.identity(), Some(&identity));
    drop(lease);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn owner_quota_bounds_one_fanout_but_allows_another_owner() {
    let scheduler = Arc::new(scheduler(2, 60_000));
    let owner_a = TaskSchedulerQuota::for_scope("run:a", 1).unwrap();
    let owner_b = TaskSchedulerQuota::for_scope("run:b", 1).unwrap();
    let blocker = scheduler
        .acquire_with_quota(
            TaskPriority::Foreground,
            "owner-a:blocker",
            &owner_a,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(blocker.quota_identity(), Some(owner_a.identity()));

    let waiting_a = {
        let scheduler = Arc::clone(&scheduler);
        let owner_a = owner_a.clone();
        tokio::spawn(async move {
            scheduler
                .acquire_with_quota(
                    TaskPriority::Urgent,
                    "owner-a:waiting",
                    &owner_a,
                    None,
                    &CancellationToken::new(),
                )
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;

    // The second slot is available to an independent owner even though
    // owner A has already consumed its entire quota.
    let owner_b_lease = scheduler
        .acquire_with_quota(
            TaskPriority::Background,
            "owner-b:independent",
            &owner_b,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let owner_a_snapshot = scheduler.quota_snapshot(&owner_a).await.unwrap();
    assert_eq!(owner_a_snapshot.max_active, 1);
    assert_eq!(owner_a_snapshot.active, 1);
    assert_eq!(owner_a_snapshot.pending, 1);
    assert!(owner_a_snapshot.blocked);

    drop(owner_b_lease);
    // Releasing the independent owner must not bypass owner A's quota.
    let still_blocked = scheduler.quota_snapshot(&owner_a).await.unwrap();
    assert_eq!(still_blocked.active, 1);
    assert_eq!(still_blocked.pending, 1);
    assert!(still_blocked.blocked);

    drop(blocker);
    let waiting_a = waiting_a.await.unwrap().unwrap();
    let admitted = scheduler.quota_snapshot(&owner_a).await.unwrap();
    assert_eq!(admitted.active, 1);
    assert_eq!(admitted.pending, 0);
    assert!(!admitted.blocked);
    drop(waiting_a);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn cancelled_owner_quota_request_releases_its_reservation() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let owner = TaskSchedulerQuota::for_scope("run:cancel", 1).unwrap();
    let blocker = scheduler
        .acquire_with_quota(
            TaskPriority::Interactive,
            "cancel:blocker",
            &owner,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let cancellation = CancellationToken::new();
    let waiting = {
        let scheduler = Arc::clone(&scheduler);
        let owner = owner.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            scheduler
                .acquire_with_quota(
                    TaskPriority::Background,
                    "cancel:waiting",
                    &owner,
                    None,
                    &cancellation,
                )
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    assert!(scheduler.quota_snapshot(&owner).await.unwrap().blocked);
    cancellation.cancel();
    assert!(matches!(
        waiting.await.unwrap(),
        Err(TaskSchedulerError::Cancelled)
    ));
    let snapshot = scheduler.quota_snapshot(&owner).await.unwrap();
    assert_eq!(snapshot.active, 1);
    assert_eq!(snapshot.pending, 0);
    assert!(!snapshot.blocked);
    drop(blocker);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn quota_identity_cannot_silently_change_its_limit() {
    let scheduler = Arc::new(scheduler(2, 60_000));
    let owner_one = TaskSchedulerQuota::for_scope("run:conflict", 1).unwrap();
    let owner_two = TaskSchedulerQuota::for_scope("run:conflict", 2).unwrap();
    let first = scheduler
        .acquire_with_quota(
            TaskPriority::Foreground,
            "conflict:first",
            &owner_one,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let result = scheduler
        .acquire_with_quota(
            TaskPriority::Foreground,
            "conflict:second",
            &owner_two,
            None,
            &CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        result,
        Err(TaskSchedulerError::InvalidConfig(message))
            if message.contains("different limit")
    ));
    drop(first);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn idle_quota_state_is_pruned_before_a_new_limit_is_registered() {
    let scheduler = scheduler(1, 60_000);
    let first_quota = TaskSchedulerQuota::for_scope("run:pruned", 1).unwrap();
    let first = scheduler
        .acquire_with_quota(
            TaskPriority::Foreground,
            "pruned:first",
            &first_quota,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    drop(first);

    // The release is actor-ordered before this observation. A zero
    // projection and successful re-registration prove that idle owner
    // state does not accumulate or pin an old limit.
    let snapshot = scheduler.quota_snapshot(&first_quota).await.unwrap();
    assert_eq!(snapshot.active, 0);
    assert_eq!(snapshot.pending, 0);
    let replacement = TaskSchedulerQuota::for_scope("run:pruned", 2).unwrap();
    let second = scheduler
        .acquire_with_quota(
            TaskPriority::Foreground,
            "pruned:replacement",
            &replacement,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(second.quota_identity(), Some(replacement.identity()));
    drop(second);
    scheduler.shutdown().await;
}
