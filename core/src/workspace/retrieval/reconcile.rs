use super::catalog::{CatalogFile, CatalogUsage, WorkspaceChunkCatalog};
use super::chunk::digest_content;
use super::eligibility::WorkspaceEligibilityPolicy;
use super::types::{WorkspaceIndexError, WorkspaceIndexResult};
use crate::workspace::{
    LocalWorkspaceManifestSnapshot, WorkspaceFileChange, WorkspaceFileSystem, WorkspacePath,
};
use futures::stream::{self, StreamExt};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

const DEFAULT_READ_CONCURRENCY: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogReconcileFailure {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogReconcileReport {
    pub source_revision: u64,
    pub catalog_revision: u64,
    pub eligible_files: usize,
    pub indexed_files: usize,
    pub indexed_chunks: usize,
    pub read_paths: Vec<String>,
    pub removed_paths: Vec<String>,
    pub failures: Vec<CatalogReconcileFailure>,
    pub full_rebuild: bool,
}

/// Reconciles immutable manifest snapshots into one bounded chunk catalog.
///
/// Normal snapshots reuse files whose manifest identity has not changed.
/// Explicit change paths are always reread, covering timestamp-resolution
/// collisions. Lag recovery uses [`Self::reconcile_after_lag`] and rereads the
/// full admitted corpus because missing events cannot otherwise be proven safe.
pub(crate) struct WorkspaceCatalogReconciler {
    catalog: Arc<WorkspaceChunkCatalog>,
    policy: WorkspaceEligibilityPolicy,
    file_system: Arc<dyn WorkspaceFileSystem>,
    read_concurrency: usize,
}

impl WorkspaceCatalogReconciler {
    pub(crate) fn new(
        catalog: Arc<WorkspaceChunkCatalog>,
        policy: WorkspaceEligibilityPolicy,
        file_system: Arc<dyn WorkspaceFileSystem>,
    ) -> Self {
        Self {
            catalog,
            policy,
            file_system,
            read_concurrency: DEFAULT_READ_CONCURRENCY,
        }
    }

    pub(crate) fn catalog_snapshot(
        &self,
    ) -> WorkspaceIndexResult<super::catalog::ChunkCatalogSnapshot> {
        self.catalog.snapshot()
    }

    pub(crate) async fn reconcile_snapshot(
        &self,
        snapshot: &LocalWorkspaceManifestSnapshot,
    ) -> WorkspaceIndexResult<CatalogReconcileReport> {
        self.reconcile(snapshot, &HashSet::new(), false).await
    }

    pub(crate) async fn reconcile_changes(
        &self,
        snapshot: &LocalWorkspaceManifestSnapshot,
        changes: &[WorkspaceFileChange],
    ) -> WorkspaceIndexResult<CatalogReconcileReport> {
        let invalidated = changes
            .iter()
            .map(|change| change.path.as_str().to_owned())
            .collect();
        self.reconcile(snapshot, &invalidated, false).await
    }

    pub(crate) async fn reconcile_after_lag(
        &self,
        snapshot: &LocalWorkspaceManifestSnapshot,
    ) -> WorkspaceIndexResult<CatalogReconcileReport> {
        self.reconcile(snapshot, &HashSet::new(), true).await
    }

    async fn reconcile(
        &self,
        snapshot: &LocalWorkspaceManifestSnapshot,
        invalidated: &HashSet<String>,
        full_rebuild: bool,
    ) -> WorkspaceIndexResult<CatalogReconcileReport> {
        let previous = self.catalog.snapshot()?;
        if snapshot.version < previous.source_revision() {
            return Err(WorkspaceIndexError::StaleRevision {
                requested: snapshot.version,
                current: previous.source_revision(),
            });
        }

        let eligible = snapshot
            .files
            .iter()
            .filter(|file| self.policy.admits(file))
            .cloned()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        let eligible_paths = eligible.keys().cloned().collect::<HashSet<_>>();
        let removed_paths = previous
            .state
            .files
            .keys()
            .filter(|path| !eligible_paths.contains(*path))
            .cloned()
            .collect::<Vec<_>>();

        let mut next_files = BTreeMap::new();
        let mut reads = Vec::new();
        for (path, file) in eligible {
            let existing = previous.state.files.get(&path);
            if !full_rebuild
                && !invalidated.contains(&path)
                && existing.is_some_and(|existing| existing.matches_manifest(&file))
            {
                next_files.insert(path, Arc::clone(existing.expect("checked above")));
            } else {
                reads.push((file, existing.cloned()));
            }
        }

        let catalog_changed = !reads.is_empty() || !removed_paths.is_empty();
        if !catalog_changed && snapshot.version == previous.source_revision() {
            return Ok(CatalogReconcileReport {
                source_revision: snapshot.version,
                catalog_revision: previous.revision(),
                eligible_files: eligible_paths.len(),
                indexed_files: previous.file_count(),
                indexed_chunks: previous.chunk_count(),
                read_paths: Vec::new(),
                removed_paths,
                failures: Vec::new(),
                full_rebuild,
            });
        }

        if !catalog_changed {
            let published = self.catalog.publish_reconciliation(
                previous.revision(),
                snapshot.version,
                next_files,
                eligible_paths.len(),
                0,
            )?;
            return Ok(CatalogReconcileReport {
                source_revision: snapshot.version,
                catalog_revision: published.revision(),
                eligible_files: eligible_paths.len(),
                indexed_files: published.file_count(),
                indexed_chunks: published.chunk_count(),
                read_paths: Vec::new(),
                removed_paths,
                failures: Vec::new(),
                full_rebuild,
            });
        }

        // There is no stale content during the initial build. Keep source
        // revision zero until that build publishes so BM25 can use its
        // compatible query-time scanner meanwhile. Later updates fence stale
        // content before any replacement read or CPU work.
        let publish_revision = if previous.source_revision() == 0 {
            previous.revision()
        } else {
            self.catalog
                .publish_reconciliation(
                    previous.revision(),
                    snapshot.version,
                    next_files.clone(),
                    eligible_paths.len(),
                    0,
                )?
                .revision()
        };

        let calls = reads.into_iter().map(|(manifest, previous)| {
            let file_system = Arc::clone(&self.file_system);
            let chunking = self.catalog.chunking();
            let chunking_strategy = self.catalog.chunking_strategy();
            let lexical_engine = self.catalog.lexical_engine();
            let source_revision = snapshot.version;
            async move {
                let path = WorkspacePath::from_normalized(manifest.path.clone());
                let result = file_system.read_text(&path).await;
                match result {
                    Ok(content) => {
                        if let Some(previous) = previous
                            .filter(|previous| previous.content_digest == digest_content(&content))
                        {
                            return (
                                manifest.clone(),
                                Ok(Arc::new(previous.with_manifest(manifest))),
                            );
                        }
                        let build_manifest = manifest.clone();
                        let built = tokio::task::spawn_blocking(move || {
                            CatalogFile::build(
                                build_manifest,
                                source_revision,
                                &content,
                                chunking,
                                &chunking_strategy,
                                lexical_engine,
                            )
                        })
                        .await;
                        let result = match built {
                            Ok(result) => result.map(Arc::new),
                            Err(error) => Err(WorkspaceIndexError::ReadFailed {
                                path: manifest.path.clone(),
                                message: format!("chunking task failed: {error}"),
                            }),
                        };
                        (manifest, result)
                    }
                    Err(error) => {
                        let path = manifest.path.clone();
                        (
                            manifest,
                            Err(WorkspaceIndexError::ReadFailed {
                                path,
                                message: error.to_string(),
                            }),
                        )
                    }
                }
            }
        });
        let mut outcomes = stream::iter(calls).buffered(self.read_concurrency);
        let mut read_paths = Vec::new();
        let mut failures = Vec::new();
        let limits = self.catalog.limits();
        let mut usage = CatalogUsage::from_files(next_files.values(), limits)?;
        while let Some((manifest, outcome)) = outcomes.next().await {
            read_paths.push(manifest.path.clone());
            match outcome {
                Ok(built) => {
                    if let Err(error) = usage.try_add(&built, limits) {
                        failures.push(CatalogReconcileFailure {
                            path: built.manifest.path.clone(),
                            message: error.to_string(),
                        });
                    } else {
                        next_files.insert(built.manifest.path.clone(), built);
                    }
                }
                Err(error) => failures.push(CatalogReconcileFailure {
                    path: manifest.path,
                    message: error.to_string(),
                }),
            }
        }
        read_paths.sort();
        failures.sort_by(|left, right| left.path.cmp(&right.path));

        let published = self.catalog.publish_reconciliation(
            publish_revision,
            snapshot.version,
            next_files,
            eligible_paths.len(),
            failures.len(),
        )?;
        Ok(CatalogReconcileReport {
            source_revision: snapshot.version,
            catalog_revision: published.revision(),
            eligible_files: eligible_paths.len(),
            indexed_files: published.file_count(),
            indexed_chunks: published.chunk_count(),
            read_paths,
            removed_paths,
            failures,
            full_rebuild,
        })
    }
}
