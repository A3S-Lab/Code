use super::catalog::WorkspaceChunkCatalog;
use super::eligibility::WorkspaceEligibilityPolicy;
use super::reconcile::{CatalogReconcileReport, WorkspaceCatalogReconciler};
use super::types::WorkspaceIndexError;
use crate::workspace::{LocalWorkspaceManifest, WorkspaceFileChange, WorkspaceFileSystem};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

// Test-only force switches. Always compiled so `--no-cfg-coverage` does not
// leave dead `#[cfg(not(test))]` stubs diluting line coverage; production
// never flips these atomics.
static TEST_FORCE_PUBLISH_SPAWN_FAILURE: AtomicBool = AtomicBool::new(false);
static TEST_FORCE_PUBLISH_DROP_RESULT: AtomicBool = AtomicBool::new(false);
static TEST_FORCE_SYNC_SPAWN_FAILURE: AtomicBool = AtomicBool::new(false);
/// Serializes tests that flip the process-global `TEST_FORCE_*` atomics so
/// parallel `multi_thread` suites cannot steal each other's forced failures.
#[allow(dead_code)] // Referenced only from `#[cfg(test)]` helpers; keep linked in lib builds.
static TEST_FORCE_LOCK: Mutex<()> = Mutex::new(());

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
    index: Arc<super::persistent::WorkspacePersistentIndex>,
    updates: mpsc::UnboundedSender<Arc<super::catalog::ChunkCatalogSnapshot>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl PersistentIndexCoordinator {
    fn start(
        persistent: Arc<super::persistent::WorkspacePersistentIndex>,
        lifetime: CancellationToken,
    ) -> Arc<Self> {
        let (updates, mut pending_updates) =
            mpsc::unbounded_channel::<Arc<super::catalog::ChunkCatalogSnapshot>>();
        let task_index = Arc::clone(&persistent);
        let task = tokio::spawn(async move {
            loop {
                let mut pending = tokio::select! {
                    _ = lifetime.cancelled() => return,
                    next = pending_updates.recv() => match next {
                        Some(snapshot) => snapshot,
                        None => return,
                    },
                };

                // Coalesce a short burst of saves. Keep the newest source
                // revision even if notifications arrive out of order.
                let settle = tokio::time::sleep(PERSISTENT_INDEX_SETTLE_DELAY);
                tokio::pin!(settle);
                loop {
                    tokio::select! {
                        _ = lifetime.cancelled() => return,
                        _ = &mut settle => break,
                        next = pending_updates.recv() => match next {
                            Some(update) => {
                                if is_newer_snapshot(&update, &pending) {
                                    pending = update;
                                }
                            }
                            None => break,
                        },
                    }
                }

                sync_snapshot_with_retry(
                    Arc::clone(&task_index),
                    pending,
                    &lifetime,
                    &mut pending_updates,
                )
                .await;
            }
        });
        Arc::new(Self {
            index: persistent,
            updates,
            task: Mutex::new(Some(task)),
        })
    }

    fn submit(&self, snapshot: super::catalog::ChunkCatalogSnapshot) {
        let _ = self.updates.send(Arc::new(snapshot));
    }

    /// Publish a catalog snapshot into the durable projection.
    ///
    /// When the index is still absent, sync this snapshot on a detached thread
    /// *before* waking the coalescing worker so the first generation cannot
    /// race two writers on the same staging directory (Windows serial CI).
    async fn publish(&self, snapshot: super::catalog::ChunkCatalogSnapshot) {
        if self.index.is_ready() {
            self.submit(snapshot);
            return;
        }
        let index = Arc::clone(&self.index);
        let pending = Arc::new(snapshot.clone());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let drop_result = TEST_FORCE_PUBLISH_DROP_RESULT.swap(false, Ordering::SeqCst);
        let force_spawn_failure = TEST_FORCE_PUBLISH_SPAWN_FAILURE.swap(false, Ordering::SeqCst);
        let spawn_result = if force_spawn_failure {
            Err(std::io::Error::other("forced publish spawn failure"))
        } else {
            std::thread::Builder::new()
                .name("a3s-persistent-publish".to_owned())
                .spawn(move || {
                    if drop_result {
                        drop(tx);
                        return;
                    }
                    let _ = tx.send(index.sync_snapshot(pending.as_ref()));
                })
        };
        if spawn_result.is_err() {
            tracing::warn!("failed to spawn inline persistent publish worker");
            self.submit(snapshot);
            return;
        }
        match rx.await {
            Ok(Ok(())) => {
                self.submit(snapshot);
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, "inline persistent publish failed; queueing for retry");
                self.submit(snapshot);
            }
            Err(_) => {
                tracing::warn!("inline persistent publish worker dropped its result");
                self.submit(snapshot);
            }
        }
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
    pending_updates: &mut mpsc::UnboundedReceiver<Arc<super::catalog::ChunkCatalogSnapshot>>,
) {
    let mut retries = 0usize;
    loop {
        let snapshot = Arc::clone(&pending);
        let persistent = Arc::clone(&persistent);
        // Keep durable sync off Tokio's blocking pool. Under Windows
        // `--test-threads=1` suites the pool can stay saturated by other
        // crate work, so `spawn_blocking` never runs while the catalog is
        // already at revision 1 and the index stays Absent.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let force_spawn_failure = TEST_FORCE_SYNC_SPAWN_FAILURE.swap(false, Ordering::SeqCst);
        let spawn_result = if force_spawn_failure {
            Err(std::io::Error::other("forced sync spawn failure"))
        } else {
            std::thread::Builder::new()
                .name("a3s-persistent-sync".to_owned())
                .spawn(move || {
                    let _ = tx.send(persistent.sync_snapshot(snapshot.as_ref()));
                })
                .map(|_| ())
        };
        let result = match spawn_result {
            Ok(_) => rx.await.map_err(|_| ()),
            Err(_) => Err(()),
        };
        match result {
            Ok(Ok(())) => return,
            Ok(Err(error)) => {
                let retryable = retryable_index_error(&error);
                tracing::warn!(%error, retryable, retries, "workspace persistent index update failed");
                if !retryable {
                    // The same snapshot stays failed. A newer catalog revision
                    // is the only reason to build again.
                    let Some(true) = wait_for_index_retry(
                        Duration::from_secs(1),
                        lifetime,
                        pending_updates,
                        &mut pending,
                    )
                    .await
                    else {
                        return;
                    };
                    retries = 0;
                    continue;
                }
                let delay = PERSISTENT_INDEX_RETRY_DELAYS
                    .get(retries.min(PERSISTENT_INDEX_RETRY_DELAYS.len().saturating_sub(1)))
                    .copied()
                    .unwrap_or(Duration::from_secs(1));
                retries = retries.saturating_add(1);
                let Some(newer_snapshot) =
                    wait_for_index_retry(delay, lifetime, pending_updates, &mut pending).await
                else {
                    return;
                };
                if newer_snapshot {
                    retries = 0;
                }
            }
            Err(()) => {
                tracing::warn!(retries, "workspace persistent index sync worker failed");
                retries = retries.saturating_add(1);
                let Some(newer_snapshot) = wait_for_index_retry(
                    Duration::from_millis(250),
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
    pending_updates: &mut mpsc::UnboundedReceiver<Arc<super::catalog::ChunkCatalogSnapshot>>,
    pending: &mut Arc<super::catalog::ChunkCatalogSnapshot>,
) -> Option<bool> {
    let retry = tokio::time::sleep(delay);
    tokio::pin!(retry);
    tokio::select! {
        _ = lifetime.cancelled() => None,
        _ = &mut retry => {
            Some(drain_newer_snapshots(pending_updates, pending))
        }
        next = pending_updates.recv() => match next {
            Some(update) => {
                let mut newer_snapshot = false;
                if is_newer_snapshot(&update, pending) {
                    *pending = update;
                    newer_snapshot = true;
                }
                if drain_newer_snapshots(pending_updates, pending) {
                    newer_snapshot = true;
                }
                Some(newer_snapshot)
            }
            None => None,
        }
    }
}

fn drain_newer_snapshots(
    pending_updates: &mut mpsc::UnboundedReceiver<Arc<super::catalog::ChunkCatalogSnapshot>>,
    pending: &mut Arc<super::catalog::ChunkCatalogSnapshot>,
) -> bool {
    let mut newer_snapshot = false;
    while let Ok(update) = pending_updates.try_recv() {
        if is_newer_snapshot(&update, pending) {
            *pending = update;
            newer_snapshot = true;
        }
    }
    newer_snapshot
}

fn retryable_index_error(error: &WorkspaceIndexError) -> bool {
    match error {
        // The native adapter classifies build failures at the source:
        // environmental failures (io, engine open) stay bounded-retryable,
        // deterministic rejections fail fast instead of burning rebuilds.
        WorkspaceIndexError::IndexBuildFailed { retryable, .. } => *retryable,
        WorkspaceIndexError::InvalidConfig(_)
        | WorkspaceIndexError::InvalidQuery(_)
        | WorkspaceIndexError::StaleRevision { .. } => false,
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
    // Catalog runtime can start while the first scan is still in flight, or
    // after it already published (broadcast fans out only to live subscribers).
    // Poll live manifest state until the first non-empty revision so Windows
    // serial CI does not sit idle waiting only on a missed channel message.
    let mut seeded = false;
    while !seeded {
        let initial = manifest.snapshot();
        if initial.version > 0 {
            report_reconciliation(
                reconciler.reconcile_snapshot(&initial).await,
                &reconciler,
                persistent.as_ref(),
            )
            .await;
            seeded = true;
            break;
        }
        tokio::select! {
            _ = lifetime.cancelled() => return,
            update = snapshots.recv() => match update {
                Ok(snapshot) => {
                    if snapshot.version > 0 {
                        report_reconciliation(
                            reconciler.reconcile_snapshot(&snapshot).await,
                            &reconciler,
                            persistent.as_ref(),
                        )
                        .await;
                        seeded = true;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let snapshot = manifest.snapshot();
                    if snapshot.version > 0 {
                        report_reconciliation(
                            reconciler.reconcile_after_lag(&snapshot).await,
                            &reconciler,
                            persistent.as_ref(),
                        )
                        .await;
                        seeded = true;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
    let _ = seeded;

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
                        persistent.publish(snapshot).await;
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

#[cfg(all(test, feature = "a3s-vec-fts"))]
mod tests;
