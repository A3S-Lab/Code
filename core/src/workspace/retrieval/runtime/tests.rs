use super::PersistentIndexCoordinator;
use super::{
    CatalogReconcileReport, WorkspaceCatalogReconciler, WorkspaceEligibilityPolicy,
    WorkspaceFileChange,
};
use crate::workspace::{
    ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkCatalog, WorkspaceIndexError,
    WorkspaceLexicalEngine, WorkspacePath, WorkspacePersistentIndex,
};
use std::sync::atomic::Ordering;
use std::sync::{Arc, MutexGuard};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn lock_test_force_switches() -> MutexGuard<'static, ()> {
    let guard = super::TEST_FORCE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    super::TEST_FORCE_PUBLISH_SPAWN_FAILURE.store(false, Ordering::SeqCst);
    super::TEST_FORCE_PUBLISH_DROP_RESULT.store(false, Ordering::SeqCst);
    super::TEST_FORCE_SYNC_SPAWN_FAILURE.store(false, Ordering::SeqCst);
    guard
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_coordinator_survives_initial_none_before_first_submit() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/late.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn late_marker() {}\n")
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("persistent index");
    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());

    // Let the coordinator observe the channel's initial None before any
    // durable snapshot is submitted — the Windows CI failure mode.
    tokio::time::sleep(Duration::from_millis(50)).await;
    coordinator.submit(catalog.snapshot().expect("catalog snapshot"));

    tokio::time::timeout(Duration::from_secs(15), async {
        while !index.is_ready() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("persistent coordinator exited after the initial None watch notification");
    assert_eq!(index.status().source_revision, 1);
    lifetime.cancel();
    coordinator.shutdown();
}

#[tokio::test]
async fn persistent_coordinator_coalesces_a_save_burst_to_the_newest_snapshot() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/burst.rs");
    let index = WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("persistent index");
    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());

    for revision in 1..=4 {
        catalog
            .replace_file(
                &path,
                Some("rust"),
                revision,
                &format!("pub fn burst_marker_{revision}() {{}}\n"),
            )
            .expect("catalog replacement");
        coordinator.submit(catalog.snapshot().expect("catalog snapshot"));
    }
    let latest_revision = catalog
        .snapshot()
        .expect("latest snapshot")
        .source_revision();

    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if index.status().source_revision == latest_revision {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("coalesced persistent index did not catch up");

    let generations = std::fs::read_dir(temp.path().join(".a3s-code/index"))
        .expect("persistent index directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("generation-"))
        })
        .count();
    assert_eq!(generations, 1, "save burst built intermediate generations");
    lifetime.cancel();
    coordinator.shutdown();
}

#[tokio::test]
async fn persistent_coordinator_retries_a_transient_publish_failure() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/retry.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn retry_marker() {}\n")
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open_with_pinned_epoch(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
        "test",
    )
    .expect("persistent index");
    let destination = temp.path().join(".a3s-code/index/generation-1-test");
    std::fs::create_dir_all(destination.parent().expect("index parent")).expect("index parent");
    std::fs::write(&destination, "temporary publish blocker").expect("publish blocker");

    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());
    let unblock = destination.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        std::fs::remove_file(unblock).expect("remove publish blocker");
    });
    coordinator.submit(catalog.snapshot().expect("catalog snapshot"));

    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if index.status().source_revision == 1
                && index.status().phase == crate::workspace::WorkspacePersistentIndexPhase::Ready
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("coordinator did not recover after transient publish failure");
    lifetime.cancel();
    coordinator.shutdown();
}

#[test]
fn retries_only_recoverable_persistent_index_errors() {
    assert!(!super::retryable_index_error(
        &WorkspaceIndexError::InvalidConfig("schema".to_owned())
    ));
    assert!(super::retryable_index_error(
        &WorkspaceIndexError::IndexBuildFailed {
            retryable: true,
            message: "temporary native lock".to_owned(),
        }
    ));
    assert!(!super::retryable_index_error(
        &WorkspaceIndexError::IndexBuildFailed {
            retryable: false,
            message: "duplicate lexical document keys".to_owned(),
        }
    ));
    assert!(!super::retryable_index_error(
        &WorkspaceIndexError::InvalidQuery("query".to_owned())
    ));
    assert!(!super::retryable_index_error(
        &WorkspaceIndexError::StaleRevision {
            requested: 1,
            current: 2,
        }
    ));
    assert!(super::retryable_index_error(
        &WorkspaceIndexError::ReadFailed {
            path: "index".to_owned(),
            message: "temporarily unavailable".to_owned(),
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_submits_immediately_when_the_index_is_already_ready() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/ready.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn ready_marker() {}\n")
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("persistent index");
    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());
    let first = catalog.snapshot().expect("first snapshot");
    coordinator.publish(first).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        while !index.is_ready() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("first publish must make the index ready");

    catalog
        .replace_file(&path, Some("rust"), 2, "pub fn ready_marker_v2() {}\n")
        .expect("second catalog replacement");
    let second = catalog.snapshot().expect("second snapshot");
    assert!(index.is_ready());
    coordinator.publish(second).await;

    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if index.status().source_revision == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("ready-path publish must advance the durable generation");
    lifetime.cancel();
    coordinator.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_queues_retry_when_inline_sync_fails() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/publish_fail.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn publish_fail_marker() {}\n")
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open_with_pinned_epoch(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
        "test",
    )
    .expect("persistent index");
    let destination = temp.path().join(".a3s-code/index/generation-1-test");
    std::fs::create_dir_all(destination.parent().expect("index parent")).expect("index parent");
    std::fs::write(&destination, "temporary publish blocker").expect("publish blocker");

    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());
    let unblock = destination.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        std::fs::remove_file(unblock).expect("remove publish blocker");
    });
    assert!(!index.is_ready());
    coordinator
        .publish(catalog.snapshot().expect("catalog snapshot"))
        .await;

    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if index.status().source_revision == 1
                && index.status().phase == crate::workspace::WorkspacePersistentIndexPhase::Ready
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("publish failure must queue a recoverable worker retry");
    lifetime.cancel();
    coordinator.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_wait_drains_newer_snapshots_submitted_during_backoff() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/drain.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn drain_marker_v1() {}\n")
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open_with_pinned_epoch(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
        "test",
    )
    .expect("persistent index");
    let destination = temp.path().join(".a3s-code/index/generation-1-test");
    std::fs::create_dir_all(destination.parent().expect("index parent")).expect("index parent");
    std::fs::write(&destination, "temporary publish blocker").expect("publish blocker");

    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());
    coordinator.submit(catalog.snapshot().expect("first snapshot"));

    tokio::time::sleep(Duration::from_millis(30)).await;
    catalog
        .replace_file(&path, Some("rust"), 2, "pub fn drain_marker_v2() {}\n")
        .expect("second replacement");
    coordinator.submit(catalog.snapshot().expect("newer snapshot during backoff"));
    catalog
        .replace_file(&path, Some("rust"), 3, "pub fn drain_marker_v3() {}\n")
        .expect("third replacement");
    coordinator.submit(catalog.snapshot().expect("newest snapshot during backoff"));

    let unblock = destination.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        let _ = std::fs::remove_file(unblock);
    });

    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if index.status().source_revision == 3
                && index.status().phase == crate::workspace::WorkspacePersistentIndexPhase::Ready
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("retry wait must adopt the newest drained snapshot");
    lifetime.cancel();
    coordinator.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_runtime_seeds_from_an_already_scanned_manifest_with_persistent() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    std::fs::write(temp.path().join("seed.rs"), "pub fn seed_marker() {}\n").expect("seed file");
    let backend = crate::workspace::ManifestWorkspaceBackend::new(temp.path());
    let mut snapshots = backend.manifest().subscribe();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if backend.manifest().snapshot().version > 0 {
                break;
            }
            let _ = snapshots.recv().await;
        }
    })
    .await
    .expect("manifest must publish a non-empty scan");

    let index_root = temp.path().join(".a3s-code/index");
    let persistent = WorkspacePersistentIndex::open(index_root, WorkspaceLexicalEngine::A3sVec)
        .expect("persistent index");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let file_system: std::sync::Arc<dyn crate::workspace::WorkspaceFileSystem> =
        std::sync::Arc::new(crate::workspace::LocalWorkspaceBackend::new(
            temp.path().to_path_buf(),
        ));
    let runtime = super::LocalWorkspaceCatalogRuntime::start_with_catalog_and_persistent(
        backend.manifest(),
        file_system,
        std::sync::Arc::clone(&catalog),
        Some(std::sync::Arc::clone(&persistent)),
    );

    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if catalog
                .snapshot()
                .map(|snapshot| snapshot.source_revision() > 0)
                .unwrap_or(false)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("seeded catalog runtime must reconcile the live manifest");

    tokio::time::timeout(Duration::from_secs(15), async {
        while !persistent.is_ready() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("seeded runtime must publish the durable projection");

    runtime.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coordinator_exits_cleanly_when_the_update_channel_closes() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let index = WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("persistent index");
    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index, lifetime.clone());
    // Dropping the coordinator closes the update sender while the worker
    // may be waiting on recv / settle — covering the channel-None exits.
    drop(coordinator);
    lifetime.cancel();
}
#[allow(clippy::await_holding_lock)] // test critical section; blocking is the point
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_queues_when_inline_worker_spawn_fails() {
    let _force_guard = lock_test_force_switches();
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/spawn_fail.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn spawn_fail_marker() {}\n")
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("persistent index");
    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());
    super::TEST_FORCE_PUBLISH_SPAWN_FAILURE.store(true, Ordering::SeqCst);
    assert!(!index.is_ready());
    coordinator
        .publish(catalog.snapshot().expect("catalog snapshot"))
        .await;

    tokio::time::timeout(Duration::from_secs(15), async {
        while !index.is_ready() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("spawn-failure publish must still queue a worker retry");
    lifetime.cancel();
    coordinator.shutdown();
}
#[allow(clippy::await_holding_lock)] // test critical section; blocking is the point
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_queues_when_inline_worker_drops_its_result() {
    let _force_guard = lock_test_force_switches();
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/drop_result.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn drop_result_marker() {}\n")
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("persistent index");
    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());
    super::TEST_FORCE_PUBLISH_DROP_RESULT.store(true, Ordering::SeqCst);
    assert!(!index.is_ready());
    coordinator
        .publish(catalog.snapshot().expect("catalog snapshot"))
        .await;

    tokio::time::timeout(Duration::from_secs(15), async {
        while !index.is_ready() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("dropped publish result must still queue a worker retry");
    lifetime.cancel();
    coordinator.shutdown();
}
#[allow(clippy::await_holding_lock)] // test critical section; blocking is the point
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_worker_retries_after_forced_spawn_failure_then_cancels() {
    let _force_guard = lock_test_force_switches();
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/sync_spawn.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn sync_spawn_marker() {}\n")
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("persistent index");
    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());
    super::TEST_FORCE_SYNC_SPAWN_FAILURE.store(true, Ordering::SeqCst);
    coordinator.submit(catalog.snapshot().expect("catalog snapshot"));

    tokio::time::sleep(Duration::from_millis(80)).await;
    lifetime.cancel();
    coordinator.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_worker_observes_non_retryable_failure_then_cancels() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/sync_non_retryable.rs");
    catalog
        .replace_file(
            &path,
            Some("rust"),
            1,
            "pub fn sync_non_retryable_marker() {}\n",
        )
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("persistent index");
    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());
    index.fail_next_sync_for_test();
    coordinator.submit(catalog.snapshot().expect("catalog snapshot"));

    tokio::time::timeout(Duration::from_secs(5), async {
        while index.non_retryable_sync_failure_is_armed() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("sync worker must consume the forced non-retryable failure");
    tokio::time::sleep(Duration::from_millis(20)).await;
    lifetime.cancel();
    coordinator.shutdown();
    assert!(
        !index.is_ready(),
        "non-retryable sync failure must not publish a ready generation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_worker_resets_retries_when_a_newer_snapshot_arrives_after_non_retryable_failure() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/sync_reset.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn sync_reset_v1() {}\n")
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("persistent index");
    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());
    index.fail_next_sync_for_test();
    coordinator.submit(catalog.snapshot().expect("first snapshot"));

    tokio::time::sleep(Duration::from_millis(30)).await;
    catalog
        .replace_file(&path, Some("rust"), 2, "pub fn sync_reset_v2() {}\n")
        .expect("catalog replacement");
    coordinator.submit(catalog.snapshot().expect("newer snapshot"));

    tokio::time::sleep(Duration::from_millis(120)).await;
    lifetime.cancel();
    coordinator.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_wait_exits_when_lifetime_is_cancelled_during_backoff() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/cancel_backoff.rs");
    catalog
        .replace_file(
            &path,
            Some("rust"),
            1,
            "pub fn cancel_backoff_marker() {}\n",
        )
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open_with_pinned_epoch(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
        "test",
    )
    .expect("persistent index");
    let destination = temp.path().join(".a3s-code/index/generation-1-test");
    std::fs::create_dir_all(destination.parent().expect("index parent")).expect("index parent");
    std::fs::write(&destination, "permanent publish blocker").expect("publish blocker");

    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index.clone(), lifetime.clone());
    coordinator.submit(catalog.snapshot().expect("catalog snapshot"));
    tokio::time::sleep(Duration::from_millis(80)).await;
    lifetime.cancel();
    coordinator.shutdown();
    assert!(
        !index.is_ready(),
        "cancelled lifetime must not leave a ready durable generation behind a blocker"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settle_loop_breaks_when_update_channel_closes_mid_burst() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/mid_burst.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn mid_burst_marker() {}\n")
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("persistent index");
    let lifetime = CancellationToken::new();
    let coordinator = PersistentIndexCoordinator::start(index, lifetime.clone());
    coordinator.submit(catalog.snapshot().expect("first snapshot"));
    // Close the update channel during the settle window so the worker
    // observes None while coalescing.
    tokio::time::sleep(Duration::from_millis(5)).await;
    drop(coordinator);
    lifetime.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_index_retry_returns_none_when_update_channel_closes() {
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let mut pending = Arc::new(catalog.snapshot().expect("snapshot"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    drop(tx);
    let lifetime = CancellationToken::new();
    let result =
        super::wait_for_index_retry(Duration::from_secs(2), &lifetime, &mut rx, &mut pending).await;
    assert_eq!(result, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_index_retry_observes_newer_snapshot_before_backoff() {
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/retry_newer.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pub fn first() {}\n")
        .expect("catalog replacement");
    let mut pending = Arc::new(catalog.snapshot().expect("first snapshot"));
    catalog
        .replace_file(&path, Some("rust"), 2, "pub fn second() {}\n")
        .expect("catalog replacement");
    let newer = Arc::new(catalog.snapshot().expect("newer snapshot"));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tx.send(Arc::clone(&newer)).expect("send newer");
    let lifetime = CancellationToken::new();
    let result =
        super::wait_for_index_retry(Duration::from_secs(2), &lifetime, &mut rx, &mut pending).await;
    assert_eq!(result, Some(true));
    assert_eq!(pending.revision(), newer.revision());
}

#[test]
fn drain_changes_marks_lagged_batches() {
    use crate::workspace::WorkspaceFileChangeKind;
    use tokio::sync::broadcast;

    let (tx, mut rx) = broadcast::channel(1);
    let _ = tx.send(WorkspaceFileChange {
        path: WorkspacePath::from_normalized("a.rs"),
        kind: WorkspaceFileChangeKind::Changed,
    });
    // Overflow the capacity-1 channel so the next receiver observes Lagged.
    let _ = tx.send(WorkspaceFileChange {
        path: WorkspacePath::from_normalized("b.rs"),
        kind: WorkspaceFileChangeKind::Changed,
    });
    let batch = super::drain_changes(&mut rx);
    assert!(batch.lagged || !batch.changes.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_reconciliation_warns_on_catalog_error() {
    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let reconciler = WorkspaceCatalogReconciler::new(
        Arc::clone(&catalog),
        WorkspaceEligibilityPolicy::default(),
        Arc::new(crate::workspace::LocalWorkspaceBackend::new(
            temp.path().to_path_buf(),
        )),
    );
    super::report_reconciliation(
        Err(WorkspaceIndexError::InvalidQuery("forced".into())),
        &reconciler,
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_reconciliation_warns_on_partial_failures() {
    use super::super::reconcile::CatalogReconcileFailure;

    let temp = tempfile::tempdir().expect("temporary workspace");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let reconciler = WorkspaceCatalogReconciler::new(
        Arc::clone(&catalog),
        WorkspaceEligibilityPolicy::default(),
        Arc::new(crate::workspace::LocalWorkspaceBackend::new(
            temp.path().to_path_buf(),
        )),
    );
    let report = CatalogReconcileReport {
        source_revision: 1,
        catalog_revision: 1,
        indexed_files: 0,
        indexed_chunks: 0,
        eligible_files: 1,
        read_paths: Vec::new(),
        removed_paths: Vec::new(),
        full_rebuild: false,
        failures: vec![CatalogReconcileFailure {
            path: "bad.rs".to_string(),
            message: "forced partial failure".to_string(),
        }],
    };
    super::report_reconciliation(Ok(report), &reconciler, None).await;
}
