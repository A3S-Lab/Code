use super::catalog::WorkspaceChunkCatalog;
use super::eligibility::WorkspaceEligibilityPolicy;
use super::reconcile::{CatalogReconcileReport, WorkspaceCatalogReconciler};
use super::types::WorkspaceIndexError;
use crate::workspace::{LocalWorkspaceManifest, WorkspaceFileChange, WorkspaceFileSystem};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const SNAPSHOT_SETTLE_DELAY: Duration = Duration::from_millis(10);
const PERSISTENT_INDEX_SETTLE_DELAY: Duration = Duration::from_millis(50);
const PERSISTENT_INDEX_RETRY_DELAYS: &[Duration] = &[
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
];

/// Schedules durable generation updates independently from catalog admission.
///
/// The catalog is the live query authority, so an index update never needs to
/// block reconciliation. A bounded settle window also collapses editor save
/// bursts into one build of the newest snapshot instead of rebuilding every
/// intermediate revision.
struct PersistentIndexCoordinator {
    updates: watch::Sender<Option<Arc<super::catalog::ChunkCatalogSnapshot>>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl PersistentIndexCoordinator {
    fn start(
        persistent: Arc<super::persistent::WorkspacePersistentIndex>,
        lifetime: CancellationToken,
    ) -> Arc<Self> {
        let (updates, mut pending_updates) =
            watch::channel::<Option<Arc<super::catalog::ChunkCatalogSnapshot>>>(None);
        let task = tokio::spawn(async move {
            loop {
                let Some(mut pending) = (tokio::select! {
                    _ = lifetime.cancelled() => None,
                    changed = pending_updates.changed() => {
                        if changed.is_err() {
                            None
                        } else {
                            pending_updates.borrow_and_update().clone()
                        }
                    },
                }) else {
                    break;
                };

                // Coalesce a short burst of saves. Keep the newest source
                // revision even if notifications arrive out of order.
                let settle = tokio::time::sleep(PERSISTENT_INDEX_SETTLE_DELAY);
                tokio::pin!(settle);
                loop {
                    tokio::select! {
                        _ = lifetime.cancelled() => return,
                        _ = &mut settle => break,
                        changed = pending_updates.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            if let Some(update) = pending_updates.borrow_and_update().clone() {
                                if is_newer_snapshot(&update, &pending) {
                                    pending = update;
                                }
                            }
                        },
                    }
                }

                sync_snapshot_with_retry(
                    Arc::clone(&persistent),
                    pending,
                    &lifetime,
                    &mut pending_updates,
                )
                .await;
            }
        });
        Arc::new(Self {
            updates,
            task: Mutex::new(Some(task)),
        })
    }

    fn submit(&self, snapshot: super::catalog::ChunkCatalogSnapshot) {
        self.updates.send_replace(Some(Arc::new(snapshot)));
    }

    fn shutdown(&self) {
        if let Some(task) = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

async fn sync_snapshot_with_retry(
    persistent: Arc<super::persistent::WorkspacePersistentIndex>,
    mut pending: Arc<super::catalog::ChunkCatalogSnapshot>,
    lifetime: &CancellationToken,
    pending_updates: &mut watch::Receiver<Option<Arc<super::catalog::ChunkCatalogSnapshot>>>,
) {
    let mut retries = 0usize;
    loop {
        let snapshot = Arc::clone(&pending);
        let persistent = Arc::clone(&persistent);
        let result =
            tokio::task::spawn_blocking(move || persistent.sync_snapshot(snapshot.as_ref())).await;
        match result {
            Ok(Ok(())) => return,
            Ok(Err(error)) => {
                if !retryable_index_error(&error) || retries >= PERSISTENT_INDEX_RETRY_DELAYS.len()
                {
                    tracing::warn!(%error, "workspace persistent index update failed");
                    return;
                }
                retries += 1;
                let Some(newer_snapshot) = wait_for_index_retry(
                    PERSISTENT_INDEX_RETRY_DELAYS[retries - 1],
                    lifetime,
                    pending_updates,
                    &mut pending,
                )
                .await
                else {
                    return;
                };
                if newer_snapshot {
                    retries = 0;
                }
            }
            Err(error) => {
                if retries >= PERSISTENT_INDEX_RETRY_DELAYS.len() {
                    tracing::warn!(%error, "workspace persistent index task failed");
                    return;
                }
                retries += 1;
                let Some(newer_snapshot) = wait_for_index_retry(
                    PERSISTENT_INDEX_RETRY_DELAYS[retries - 1],
                    lifetime,
                    pending_updates,
                    &mut pending,
                )
                .await
                else {
                    return;
                };
                if newer_snapshot {
                    retries = 0;
                }
            }
        }
    }
}

async fn wait_for_index_retry(
    delay: Duration,
    lifetime: &CancellationToken,
    pending_updates: &mut watch::Receiver<Option<Arc<super::catalog::ChunkCatalogSnapshot>>>,
    pending: &mut Arc<super::catalog::ChunkCatalogSnapshot>,
) -> Option<bool> {
    let retry = tokio::time::sleep(delay);
    tokio::pin!(retry);
    tokio::select! {
        _ = lifetime.cancelled() => None,
        _ = &mut retry => Some(false),
        changed = pending_updates.changed() => {
            if changed.is_err() {
                return None;
            }
            let mut newer_snapshot = false;
            if let Some(update) = pending_updates.borrow_and_update().clone() {
                if is_newer_snapshot(&update, pending) {
                    *pending = update;
                    newer_snapshot = true;
                }
            }
            Some(newer_snapshot)
        }
    }
}

fn retryable_index_error(error: &WorkspaceIndexError) -> bool {
    match error {
        WorkspaceIndexError::InvalidConfig(message) => {
            // The native adapter currently reports its FFI/open failures as
            // InvalidConfig. Keep those bounded-retryable while leaving
            // actual schema/configuration errors fail-fast.
            message.starts_with("persistent zvec index failed:")
        }
        WorkspaceIndexError::InvalidQuery(_) | WorkspaceIndexError::StaleRevision { .. } => false,
        _ => true,
    }
}

impl Drop for PersistentIndexCoordinator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn is_newer_snapshot(
    candidate: &super::catalog::ChunkCatalogSnapshot,
    current: &super::catalog::ChunkCatalogSnapshot,
) -> bool {
    (candidate.source_revision(), candidate.revision())
        > (current.source_revision(), current.revision())
}

#[cfg(all(test, feature = "zvec-rust-fts"))]
mod tests {
    use super::PersistentIndexCoordinator;
    use crate::workspace::{
        ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkCatalog, WorkspaceIndexError,
        WorkspaceLexicalEngine, WorkspacePath, WorkspacePersistentIndex,
    };
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn persistent_coordinator_coalesces_a_save_burst_to_the_newest_snapshot() {
        let temp = tempfile::tempdir().expect("temporary workspace");
        let catalog = WorkspaceChunkCatalog::new_with_engine(
            ChunkingConfig::default(),
            ChunkCatalogLimits::default(),
            WorkspaceLexicalEngine::ZvecRust,
        )
        .expect("catalog");
        let path = WorkspacePath::from_normalized("src/burst.rs");
        let index = WorkspacePersistentIndex::open(
            temp.path().join(".a3s-code/index"),
            WorkspaceLexicalEngine::ZvecRust,
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
            WorkspaceLexicalEngine::ZvecRust,
        )
        .expect("catalog");
        let path = WorkspacePath::from_normalized("src/retry.rs");
        catalog
            .replace_file(&path, Some("rust"), 1, "pub fn retry_marker() {}\n")
            .expect("catalog replacement");
        let index = WorkspacePersistentIndex::open(
            temp.path().join(".a3s-code/index"),
            WorkspaceLexicalEngine::ZvecRust,
        )
        .expect("persistent index");
        let destination = temp.path().join(".a3s-code/index/generation-1");
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
                    && index.status().phase
                        == crate::workspace::WorkspacePersistentIndexPhase::Ready
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
            &WorkspaceIndexError::InvalidConfig(
                "persistent zvec index failed: temporary native lock".to_owned()
            )
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
}

/// Owns asynchronous manifest-to-catalog reconciliation for one local backend.
pub(crate) struct LocalWorkspaceCatalogRuntime {
    catalog: Arc<WorkspaceChunkCatalog>,
    lifetime: CancellationToken,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    persistent: Option<Arc<PersistentIndexCoordinator>>,
}

impl LocalWorkspaceCatalogRuntime {
    pub(crate) fn start(
        manifest: Arc<LocalWorkspaceManifest>,
        file_system: Arc<dyn WorkspaceFileSystem>,
    ) -> Arc<Self> {
        Self::start_with_catalog_and_persistent(
            manifest,
            file_system,
            WorkspaceChunkCatalog::default_catalog(),
            None,
        )
    }

    pub(crate) fn start_with_catalog_and_persistent(
        manifest: Arc<LocalWorkspaceManifest>,
        file_system: Arc<dyn WorkspaceFileSystem>,
        catalog: Arc<WorkspaceChunkCatalog>,
        persistent: Option<Arc<super::persistent::WorkspacePersistentIndex>>,
    ) -> Arc<Self> {
        let snapshots = manifest.subscribe();
        let changes = manifest.subscribe_changes();
        let lifetime = CancellationToken::new();
        let persistent_coordinator = persistent
            .map(|persistent| PersistentIndexCoordinator::start(persistent, lifetime.clone()));
        let runtime = Arc::new(Self {
            catalog: Arc::clone(&catalog),
            lifetime: lifetime.clone(),
            task: Mutex::new(None),
            persistent: persistent_coordinator.clone(),
        });
        let task = tokio::spawn(run_catalog_updates(
            manifest,
            WorkspaceCatalogReconciler::new(
                catalog,
                WorkspaceEligibilityPolicy::default(),
                file_system,
            ),
            snapshots,
            changes,
            lifetime,
            persistent_coordinator,
        ));
        *runtime
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(task);
        runtime
    }

    pub(crate) fn catalog(&self) -> Arc<WorkspaceChunkCatalog> {
        Arc::clone(&self.catalog)
    }

    pub(crate) fn shutdown(&self) {
        self.lifetime.cancel();
        if let Some(task) = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
        if let Some(persistent) = &self.persistent {
            persistent.shutdown();
        }
    }
}

impl Drop for LocalWorkspaceCatalogRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn run_catalog_updates(
    manifest: Arc<LocalWorkspaceManifest>,
    reconciler: WorkspaceCatalogReconciler,
    mut snapshots: broadcast::Receiver<crate::workspace::LocalWorkspaceManifestSnapshot>,
    mut changes: broadcast::Receiver<WorkspaceFileChange>,
    lifetime: CancellationToken,
    persistent: Option<Arc<PersistentIndexCoordinator>>,
) {
    let initial = manifest.snapshot();
    if initial.version > 0 {
        report_reconciliation(
            reconciler.reconcile_snapshot(&initial).await,
            &reconciler,
            persistent.as_ref(),
        )
        .await;
    }

    loop {
        tokio::select! {
            _ = lifetime.cancelled() => break,
            update = snapshots.recv() => match update {
                Ok(snapshot) => {
                    tokio::time::sleep(SNAPSHOT_SETTLE_DELAY).await;
                    let batch = drain_changes(&mut changes);
                    if batch.lagged {
                        report_reconciliation(reconciler.reconcile_after_lag(&manifest.snapshot()).await, &reconciler, persistent.as_ref()).await;
                    } else if batch.changes.is_empty() {
                        report_reconciliation(reconciler.reconcile_snapshot(&snapshot).await, &reconciler, persistent.as_ref()).await;
                    } else {
                        report_reconciliation(reconciler.reconcile_changes(&snapshot, &batch.changes).await, &reconciler, persistent.as_ref()).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "workspace retrieval snapshot stream lagged; rebuilding admitted files");
                    report_reconciliation(reconciler.reconcile_after_lag(&manifest.snapshot()).await, &reconciler, persistent.as_ref()).await;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            update = changes.recv() => match update {
                Ok(change) => {
                    let mut batch = drain_changes(&mut changes);
                    batch.changes.insert(0, change);
                    if batch.lagged {
                        report_reconciliation(reconciler.reconcile_after_lag(&manifest.snapshot()).await, &reconciler, persistent.as_ref()).await;
                    } else {
                        report_reconciliation(
                            reconciler
                                .reconcile_changes(&manifest.snapshot(), &batch.changes)
                                .await,
                            &reconciler,
                            persistent.as_ref(),
                        ).await;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "workspace retrieval change stream lagged; rebuilding admitted files");
                    report_reconciliation(reconciler.reconcile_after_lag(&manifest.snapshot()).await, &reconciler, persistent.as_ref()).await;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

fn drain_changes(changes: &mut broadcast::Receiver<WorkspaceFileChange>) -> DrainedChanges {
    let mut batch = DrainedChanges::default();
    loop {
        match changes.try_recv() {
            Ok(change) => batch.changes.push(change),
            Err(broadcast::error::TryRecvError::Lagged(_)) => batch.lagged = true,
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                break;
            }
        }
    }
    batch
}

#[derive(Default)]
struct DrainedChanges {
    changes: Vec<WorkspaceFileChange>,
    lagged: bool,
}

async fn report_reconciliation(
    result: Result<CatalogReconcileReport, super::types::WorkspaceIndexError>,
    reconciler: &WorkspaceCatalogReconciler,
    persistent: Option<&Arc<PersistentIndexCoordinator>>,
) {
    match result {
        Ok(report) => {
            if let Some(persistent) = persistent {
                match reconciler.catalog_snapshot() {
                    Ok(snapshot) => {
                        persistent.submit(snapshot);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "workspace persistent index snapshot failed")
                    }
                }
            }
            if !report.failures.is_empty() {
                tracing::warn!(
                    source_revision = report.source_revision,
                    failed_files = report.failures.len(),
                    indexed_files = report.indexed_files,
                    "workspace retrieval catalog is partially indexed"
                );
            }
            tracing::debug!(
                source_revision = report.source_revision,
                catalog_revision = report.catalog_revision,
                indexed_files = report.indexed_files,
                indexed_chunks = report.indexed_chunks,
                eligible_files = report.eligible_files,
                read_files = report.read_paths.len(),
                removed_files = report.removed_paths.len(),
                full_rebuild = report.full_rebuild,
                "workspace retrieval catalog reconciled"
            );
        }
        Err(error) => tracing::warn!(%error, "workspace retrieval catalog reconciliation failed"),
    }
}
