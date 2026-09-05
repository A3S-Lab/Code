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
async fn global_capacity_is_recomputed_after_each_admission() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let blocker = scheduler
        .acquire(
            TaskPriority::Interactive,
            "capacity-blocker",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let (admitted_tx, mut admitted_rx) = mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let mut tasks = Vec::new();
    for index in 0..3 {
        let scheduler = Arc::clone(&scheduler);
        let admitted_tx = admitted_tx.clone();
        let release = Arc::clone(&release);
        tasks.push(tokio::spawn(async move {
            let lease = scheduler
                .acquire(
                    TaskPriority::Foreground,
                    format!("capacity-{index}"),
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            admitted_tx.send(index).unwrap();
            release.acquire().await.unwrap().forget();
            drop(lease);
        }));
    }
    wait_for_pending(&scheduler, 3).await;
    drop(blocker);
    assert!(admitted_rx.recv().await.is_some());
    assert_eq!(scheduler.stats().await.unwrap().active, 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), admitted_rx.recv())
            .await
            .is_err(),
        "a second global lease must not be admitted while capacity is full"
    );
    release.add_permits(3);
    for task in tasks {
        task.await.unwrap();
    }
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

#[tokio::test]
async fn multiple_quota_dimensions_are_atomic_and_release_together() {
    let scheduler = Arc::new(scheduler(2, 60_000));
    let owner = TaskSchedulerQuota::for_scope("run:multi", 2).unwrap();
    let provider = TaskSchedulerQuota::for_scope("provider:shared", 1).unwrap();
    let first = scheduler
        .acquire_with_quotas(
            TaskPriority::Foreground,
            "multi:first",
            &[owner.clone(), provider.clone()],
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        first.quota_identities(),
        &[owner.identity.clone(), provider.identity.clone()]
    );

    // The provider dimension blocks this request even though the owner still
    // has one free slot. No partial owner reservation is visible.
    let cancellation = CancellationToken::new();
    let waiting = {
        let scheduler = Arc::clone(&scheduler);
        let owner = owner.clone();
        let provider = provider.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            scheduler
                .acquire_with_quotas(
                    TaskPriority::Foreground,
                    "multi:waiting",
                    &[owner, provider],
                    None,
                    &cancellation,
                )
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    assert_eq!(scheduler.quota_snapshot(&owner).await.unwrap().active, 1);
    assert_eq!(scheduler.quota_snapshot(&owner).await.unwrap().pending, 1);
    assert!(scheduler.quota_snapshot(&provider).await.unwrap().blocked);

    cancellation.cancel();
    assert!(matches!(
        waiting.await.unwrap(),
        Err(TaskSchedulerError::Cancelled)
    ));
    assert_eq!(scheduler.quota_snapshot(&owner).await.unwrap().pending, 0);
    assert_eq!(scheduler.quota_snapshot(&provider).await.unwrap().active, 1);
    drop(first);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn independent_provider_quota_can_progress_under_global_capacity() {
    let scheduler = Arc::new(scheduler(2, 60_000));
    let provider_a = TaskSchedulerQuota::for_scope("provider:a", 1).unwrap();
    let provider_b = TaskSchedulerQuota::for_scope("provider:b", 1).unwrap();
    let holder = scheduler
        .acquire_with_quota(
            TaskPriority::Foreground,
            "provider-a:holder",
            &provider_a,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let blocked_a = {
        let scheduler = Arc::clone(&scheduler);
        let provider_a = provider_a.clone();
        tokio::spawn(async move {
            scheduler
                .acquire_with_quota(
                    TaskPriority::Foreground,
                    "provider-a:blocked",
                    &provider_a,
                    None,
                    &CancellationToken::new(),
                )
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    let b = scheduler
        .acquire_with_quota(
            TaskPriority::Foreground,
            "provider-b:independent",
            &provider_b,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        scheduler.quota_snapshot(&provider_b).await.unwrap().active,
        1
    );
    drop(b);
    drop(holder);
    let a = blocked_a.await.unwrap().unwrap();
    drop(a);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn duplicate_quota_dimensions_are_rejected_before_enqueue() {
    let scheduler = scheduler(1, 60_000);
    let quota = TaskSchedulerQuota::for_scope("duplicate", 1).unwrap();
    let result = scheduler
        .acquire_with_quotas(
            TaskPriority::Foreground,
            "duplicate",
            &[quota.clone(), quota],
            None,
            &CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        result,
        Err(TaskSchedulerError::InvalidConfig(message))
            if message.contains("duplicate quota")
    ));
    scheduler.shutdown().await;
}

#[tokio::test]
async fn quota_only_admission_does_not_consume_global_slot() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let provider = TaskSchedulerQuota::for_scope("provider:leaf", 1).unwrap();
    let global = scheduler
        .acquire(
            TaskPriority::Interactive,
            "global",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let leaf = scheduler
        .acquire_quota(
            TaskPriority::Foreground,
            "provider:leaf-generation",
            &provider,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!leaf.consumes_global_slot());
    assert!(global.consumes_global_slot());
    assert_eq!(scheduler.stats().await.unwrap().active, 1);
    assert_eq!(scheduler.quota_snapshot(&provider).await.unwrap().active, 1);
    drop(leaf);
    drop(global);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn quota_only_cancellation_releases_provider_state_and_allows_retry() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let provider = TaskSchedulerQuota::for_scope("provider:cancel", 1).unwrap();
    let holder = scheduler
        .acquire_quota(
            TaskPriority::Foreground,
            "provider:holder",
            &provider,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let cancellation = CancellationToken::new();
    let waiting = {
        let scheduler = Arc::clone(&scheduler);
        let provider = provider.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            scheduler
                .acquire_quota(
                    TaskPriority::Foreground,
                    "provider:cancelled",
                    &provider,
                    &cancellation,
                )
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    cancellation.cancel();
    assert!(matches!(
        waiting.await.unwrap(),
        Err(TaskSchedulerError::Cancelled)
    ));
    assert_eq!(
        scheduler.quota_snapshot(&provider).await.unwrap().pending,
        0
    );
    drop(holder);
    let retry = scheduler
        .acquire_quota(
            TaskPriority::Foreground,
            "provider:retry",
            &provider,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    drop(retry);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn independent_quota_only_provider_progress_ignores_full_global_budget() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let global = scheduler
        .acquire(
            TaskPriority::Interactive,
            "global-holder",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let provider_a = TaskSchedulerQuota::for_scope("provider:quota-a", 1).unwrap();
    let provider_b = TaskSchedulerQuota::for_scope("provider:quota-b", 1).unwrap();
    let a = scheduler
        .acquire_quota(
            TaskPriority::Foreground,
            "provider-a-generation",
            &provider_a,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let b = scheduler
        .acquire_quota(
            TaskPriority::Foreground,
            "provider-b-generation",
            &provider_b,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(!a.consumes_global_slot());
    assert!(!b.consumes_global_slot());
    assert_eq!(scheduler.stats().await.unwrap().active, 1);
    assert_eq!(
        scheduler.quota_snapshot(&provider_a).await.unwrap().active,
        1
    );
    assert_eq!(
        scheduler.quota_snapshot(&provider_b).await.unwrap().active,
        1
    );
    drop(b);
    drop(a);
    drop(global);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn quota_health_retains_bounded_counters_after_the_pool_becomes_idle() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let provider = TaskSchedulerQuota::for_scope("provider:health", 1).unwrap();
    let first = scheduler
        .acquire_quota(
            TaskPriority::Foreground,
            "provider-health:first",
            &provider,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let waiting = {
        let scheduler = Arc::clone(&scheduler);
        let provider = provider.clone();
        tokio::spawn(async move {
            scheduler
                .acquire_quota(
                    TaskPriority::Background,
                    "provider-health:waiting",
                    &provider,
                    &CancellationToken::new(),
                )
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;
    tokio::time::sleep(Duration::from_millis(2)).await;

    let blocked = scheduler.quota_health(&provider).await.unwrap();
    assert!(blocked.observed);
    assert!(blocked.live);
    assert_eq!(blocked.active, 1);
    assert_eq!(blocked.pending, 1);
    assert!(blocked.blocked);
    assert_eq!(blocked.admitted, 1);
    assert_eq!(blocked.released, 0);

    drop(first);
    let second = waiting.await.unwrap().unwrap();
    let admitted = scheduler.quota_health(&provider).await.unwrap();
    assert_eq!(admitted.active, 1);
    assert_eq!(admitted.pending, 0);
    assert_eq!(admitted.admitted, 2);
    assert_eq!(admitted.released, 1);
    assert_eq!(admitted.peak_active, 1);
    assert!(admitted.total_wait_micros > 0);
    assert!(admitted.max_wait_micros >= admitted.average_wait_micros);

    drop(second);
    let retained = scheduler.quota_health(&provider).await.unwrap();
    assert!(retained.observed);
    assert!(!retained.live);
    assert_eq!(retained.active, 0);
    assert_eq!(retained.pending, 0);
    assert_eq!(retained.admitted, 2);
    assert_eq!(retained.released, 2);
    let encoded = serde_json::to_string(&retained).unwrap();
    assert!(!encoded.contains("provider:health"));
    assert!(!encoded.contains("provider-health:first"));
    assert!(!encoded.contains("provider-health:waiting"));
    scheduler.shutdown().await;
}

#[tokio::test]
async fn quota_health_records_cancellation_and_isolation_between_provider_pools() {
    let scheduler = Arc::new(scheduler(1, 60_000));
    let provider_a = TaskSchedulerQuota::for_scope("provider:health-a", 1).unwrap();
    let provider_b = TaskSchedulerQuota::for_scope("provider:health-b", 1).unwrap();
    let holder = scheduler
        .acquire_quota(
            TaskPriority::Foreground,
            "provider-health-a:holder",
            &provider_a,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let cancellation = CancellationToken::new();
    let waiting = {
        let scheduler = Arc::clone(&scheduler);
        let provider_a = provider_a.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            scheduler
                .acquire_quota(
                    TaskPriority::Urgent,
                    "provider-health-a:cancelled",
                    &provider_a,
                    &cancellation,
                )
                .await
        })
    };
    wait_for_pending(&scheduler, 1).await;

    let independent = scheduler
        .acquire_quota(
            TaskPriority::Maintenance,
            "provider-health-b:independent",
            &provider_b,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    cancellation.cancel();
    assert!(matches!(
        waiting.await.unwrap(),
        Err(TaskSchedulerError::Cancelled)
    ));

    let health_a = scheduler.quota_health(&provider_a).await.unwrap();
    let health_b = scheduler.quota_health(&provider_b).await.unwrap();
    assert_eq!(health_a.cancelled, 1);
    assert_eq!(health_a.admitted, 1);
    assert_eq!(health_b.cancelled, 0);
    assert_eq!(health_b.admitted, 1);
    assert_eq!(health_b.active, 1);

    drop(independent);
    drop(holder);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn retained_quota_health_has_a_hard_identity_bound() {
    let scheduler = scheduler(1, 60_000);
    let mut quotas = Vec::new();
    for index in 0..=TASK_SCHEDULER_QUOTA_HEALTH_RETENTION {
        let quota = TaskSchedulerQuota::for_scope(&format!("retained:{index}"), 1).unwrap();
        let lease = scheduler
            .acquire_quota(
                TaskPriority::Maintenance,
                format!("retained:{index}"),
                &quota,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        drop(lease);
        // Actor-order the release before the next identity is inserted.
        let _ = scheduler.quota_health(&quota).await.unwrap();
        quotas.push(quota);
    }

    assert!(!scheduler.quota_health(&quotas[0]).await.unwrap().observed);
    assert!(
        scheduler
            .quota_health(quotas.last().unwrap())
            .await
            .unwrap()
            .observed
    );
    scheduler.shutdown().await;
}
