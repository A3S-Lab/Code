use super::catalog::WorkspaceChunkCatalog;
use super::eligibility::WorkspaceEligibilityPolicy;
use super::reconcile::{CatalogReconcileReport, WorkspaceCatalogReconciler};
use crate::workspace::{LocalWorkspaceManifest, WorkspaceFileChange, WorkspaceFileSystem};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

const SNAPSHOT_SETTLE_DELAY: Duration = Duration::from_millis(10);

/// Owns asynchronous manifest-to-catalog reconciliation for one local backend.
pub(crate) struct LocalWorkspaceCatalogRuntime {
    catalog: Arc<WorkspaceChunkCatalog>,
    lifetime: CancellationToken,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl LocalWorkspaceCatalogRuntime {
    pub(crate) fn start(
        manifest: Arc<LocalWorkspaceManifest>,
        file_system: Arc<dyn WorkspaceFileSystem>,
    ) -> Arc<Self> {
        let snapshots = manifest.subscribe();
        let changes = manifest.subscribe_changes();
        let catalog = WorkspaceChunkCatalog::default_catalog();
        let lifetime = CancellationToken::new();
        let runtime = Arc::new(Self {
            catalog: Arc::clone(&catalog),
            lifetime: lifetime.clone(),
            task: Mutex::new(None),
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
) {
    let initial = manifest.snapshot();
    if initial.version > 0 {
        report_reconciliation(reconciler.reconcile_snapshot(&initial).await);
    }

    loop {
        tokio::select! {
            _ = lifetime.cancelled() => break,
            update = snapshots.recv() => match update {
                Ok(snapshot) => {
                    tokio::time::sleep(SNAPSHOT_SETTLE_DELAY).await;
                    let batch = drain_changes(&mut changes);
                    if batch.lagged {
                        report_reconciliation(reconciler.reconcile_after_lag(&manifest.snapshot()).await);
                    } else if batch.changes.is_empty() {
                        report_reconciliation(reconciler.reconcile_snapshot(&snapshot).await);
                    } else {
                        report_reconciliation(reconciler.reconcile_changes(&snapshot, &batch.changes).await);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "workspace retrieval snapshot stream lagged; rebuilding admitted files");
                    report_reconciliation(reconciler.reconcile_after_lag(&manifest.snapshot()).await);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            update = changes.recv() => match update {
                Ok(change) => {
                    let mut batch = drain_changes(&mut changes);
                    batch.changes.insert(0, change);
                    if batch.lagged {
                        report_reconciliation(reconciler.reconcile_after_lag(&manifest.snapshot()).await);
                    } else {
                        report_reconciliation(
                            reconciler
                                .reconcile_changes(&manifest.snapshot(), &batch.changes)
                                .await,
                        );
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "workspace retrieval change stream lagged; rebuilding admitted files");
                    report_reconciliation(reconciler.reconcile_after_lag(&manifest.snapshot()).await);
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

fn report_reconciliation(
    result: Result<CatalogReconcileReport, super::types::WorkspaceIndexError>,
) {
    match result {
        Ok(report) => {
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
