use super::{
    ChunkCatalogSnapshot, WorkspaceChunk, WorkspaceChunkCatalog, WorkspaceRetrievalOptions,
    WorkspaceRetrievalPhase, WorkspaceRetrievalResult, WorkspaceRetrievalStatus,
};
use crate::embedding::{EmbeddingExecutor, EmbeddingInput};
use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorRecord};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Session-owned asynchronous projection from workspace chunks to vectors.
///
/// The background projection embeds only text already admitted and retained by
/// the shared [`WorkspaceChunkCatalog`]. Query-time source verification reads
/// through the caller's [`WorkspaceFileSystem`](crate::workspace::WorkspaceFileSystem)
/// capability before any chunk text is returned.
pub struct WorkspaceRetrievalRuntime {
    pub(super) catalog: Arc<WorkspaceChunkCatalog>,
    pub(super) executor: EmbeddingExecutor,
    index: Mutex<Option<Arc<InMemoryVectorIndex>>>,
    status: Arc<RwLock<WorkspaceRetrievalStatus>>,
    lifetime: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
    close_gate: tokio::sync::Mutex<()>,
    shutdown_timeout: std::time::Duration,
    pub(super) rerank_options: super::WorkspaceRerankOptions,
}

impl std::fmt::Debug for WorkspaceRetrievalRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceRetrievalRuntime")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl WorkspaceRetrievalRuntime {
    pub(crate) fn start(
        catalog: Arc<WorkspaceChunkCatalog>,
        options: WorkspaceRetrievalOptions,
        parent_lifetime: CancellationToken,
    ) -> WorkspaceRetrievalResult<Arc<Self>> {
        let limits = options.index_limits.validate()?;
        let rerank_options = options.rerank.validate()?;
        let executor = EmbeddingExecutor::new(options.provider, options.embedding)?;
        let descriptor = VectorIndexDescriptor::new(executor.descriptor().dimension)
            .with_max_records(limits.max_records)
            .with_max_bytes(limits.max_bytes);
        let index = Arc::new(InMemoryVectorIndex::new(descriptor)?);
        let status = Arc::new(RwLock::new(WorkspaceRetrievalStatus::building(
            executor.descriptor().clone(),
        )));
        let lifetime = parent_lifetime.child_token();
        let runtime = Arc::new(Self {
            catalog: Arc::clone(&catalog),
            executor: executor.clone(),
            index: Mutex::new(Some(Arc::clone(&index))),
            status: Arc::clone(&status),
            lifetime: lifetime.clone(),
            task: Mutex::new(None),
            close_gate: tokio::sync::Mutex::new(()),
            shutdown_timeout: limits.shutdown_timeout,
            rerank_options,
        });
        let task = tokio::spawn(run_semantic_updates(
            catalog, index, executor, status, lifetime,
        ));
        *lock_unpoisoned(&runtime.task) = Some(task);
        Ok(runtime)
    }

    /// Return a lock-free observation of current partial readiness.
    pub fn status(&self) -> WorkspaceRetrievalStatus {
        read_unpoisoned(&self.status).clone()
    }

    pub(super) fn index(&self) -> Option<Arc<InMemoryVectorIndex>> {
        lock_unpoisoned(&self.index).clone()
    }

    pub(super) fn child_lifetime(&self) -> CancellationToken {
        self.lifetime.child_token()
    }

    /// Cancel indexing, join its owned task within the configured deadline,
    /// and release the in-memory vector index. This operation is idempotent.
    pub async fn close(&self) {
        let _close = self.close_gate.lock().await;
        self.lifetime.cancel();
        let task = lock_unpoisoned(&self.task).take();
        if let Some(mut task) = task {
            if tokio::time::timeout(self.shutdown_timeout, &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
                tracing::warn!(
                    timeout_ms = self.shutdown_timeout.as_millis() as u64,
                    "workspace semantic index task exceeded its close deadline"
                );
            }
        }
        lock_unpoisoned(&self.index).take();
        publish_closed(&self.status);
    }
}

impl Drop for WorkspaceRetrievalRuntime {
    fn drop(&mut self) {
        self.lifetime.cancel();
        if let Some(task) = lock_unpoisoned(&self.task).take() {
            task.abort();
        }
        lock_unpoisoned(&self.index).take();
    }
}

#[derive(Clone)]
struct ReadyPartition {
    digest: Arc<str>,
    chunk_count: usize,
}

#[derive(Clone)]
struct CatalogPartition {
    digest: Arc<str>,
    chunks: Vec<Arc<WorkspaceChunk>>,
}

struct BuildState {
    ready: BTreeMap<String, ReadyPartition>,
    failed: BTreeSet<String>,
    total_failures: u64,
    observed_catalog_revision: u64,
}

impl BuildState {
    fn new() -> Self {
        Self {
            ready: BTreeMap::new(),
            failed: BTreeSet::new(),
            total_failures: 0,
            observed_catalog_revision: 0,
        }
    }

    fn record_failure(&mut self, path: &str) {
        self.failed.insert(path.to_owned());
        self.total_failures = self.total_failures.saturating_add(1);
    }
}

async fn run_semantic_updates(
    catalog: Arc<WorkspaceChunkCatalog>,
    index: Arc<InMemoryVectorIndex>,
    executor: EmbeddingExecutor,
    status: Arc<RwLock<WorkspaceRetrievalStatus>>,
    lifetime: CancellationToken,
) {
    let mut updates = catalog.subscribe();
    let mut state = BuildState::new();

    loop {
        if lifetime.is_cancelled() {
            break;
        }
        let snapshot = updates.borrow_and_update().clone();
        if snapshot.source_revision() == 0 {
            tokio::select! {
                biased;
                _ = lifetime.cancelled() => break,
                changed = updates.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
            }
            continue;
        }

        let generation = lifetime.child_token();
        let reconcile = reconcile_semantic_snapshot(
            &catalog,
            &index,
            &executor,
            &status,
            &mut state,
            snapshot,
            generation.clone(),
        );
        tokio::pin!(reconcile);
        let changed_during_reconcile = tokio::select! {
            biased;
            _ = lifetime.cancelled() => {
                generation.cancel();
                reconcile.await;
                break;
            }
            changed = updates.changed() => {
                generation.cancel();
                reconcile.await;
                Some(changed.is_ok())
            }
            _ = &mut reconcile => None,
        };
        if let Some(channel_open) = changed_during_reconcile {
            if !channel_open {
                break;
            }
            continue;
        }
        tokio::select! {
            biased;
            _ = lifetime.cancelled() => break,
            changed = updates.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }
    }

    if let Err(error) = index.clear().await {
        tracing::warn!(%error, "failed to clear workspace semantic index during shutdown");
    }
    publish_closed(&status);
}

async fn reconcile_semantic_snapshot(
    catalog: &WorkspaceChunkCatalog,
    index: &InMemoryVectorIndex,
    executor: &EmbeddingExecutor,
    status: &RwLock<WorkspaceRetrievalStatus>,
    state: &mut BuildState,
    snapshot: ChunkCatalogSnapshot,
    cancellation: CancellationToken,
) {
    if state.observed_catalog_revision != snapshot.revision() {
        state.total_failures = state
            .total_failures
            .saturating_add(snapshot.failed_file_count() as u64);
        state.observed_catalog_revision = snapshot.revision();
    }
    let partitions = catalog_partitions(&snapshot);
    state.failed.clear();
    let stale_count = state
        .ready
        .iter()
        .filter(|(path, ready)| {
            !partitions
                .get(*path)
                .is_some_and(|partition| partition.digest == ready.digest)
        })
        .count();
    let missing_count = partitions
        .iter()
        .filter(|(path, partition)| {
            !state
                .ready
                .get(*path)
                .is_some_and(|ready| ready.digest == partition.digest)
        })
        .count();
    publish_progress(
        status,
        &snapshot,
        index,
        state,
        stale_count.saturating_add(missing_count),
        false,
    );
    invalidate_stale_partitions(index, state, &partitions).await;
    if cancellation.is_cancelled() {
        return;
    }
    // Only publish readiness for the exact catalog revision that is still
    // current after stale partitions have been removed. A newer catalog
    // update may already be waiting on the watch receiver.
    if catalog
        .snapshot()
        .map(|current| current.revision() != snapshot.revision())
        .unwrap_or(true)
    {
        return;
    }

    let pending = partitions
        .iter()
        .filter(|(path, partition)| {
            !state
                .ready
                .get(*path)
                .is_some_and(|ready| ready.digest == partition.digest)
        })
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    publish_progress(status, &snapshot, index, state, pending.len(), false);

    for (offset, path) in pending.iter().enumerate() {
        if cancellation.is_cancelled() {
            return;
        }
        let Some(partition) = partitions.get(path) else {
            continue;
        };
        if partition.chunks.is_empty() {
            state.ready.insert(
                path.clone(),
                ReadyPartition {
                    digest: Arc::clone(&partition.digest),
                    chunk_count: 0,
                },
            );
            publish_progress(
                status,
                &snapshot,
                index,
                state,
                pending.len().saturating_sub(offset + 1),
                false,
            );
            continue;
        }

        let inputs = partition
            .chunks
            .iter()
            .map(|chunk| {
                EmbeddingInput::new(Arc::<str>::from(chunk.id.as_str()), Arc::clone(&chunk.text))
            })
            .collect();
        let execution = match executor.embed(inputs, cancellation.child_token()).await {
            Ok(execution) => execution,
            Err(_) if cancellation.is_cancelled() => return,
            Err(error) => {
                state.record_failure(path);
                tracing::warn!(
                    path,
                    error = %error,
                    "workspace semantic partition embedding failed"
                );
                publish_progress(
                    status,
                    &snapshot,
                    index,
                    state,
                    pending.len().saturating_sub(offset + 1),
                    true,
                );
                continue;
            }
        };

        if !catalog_digest_matches(catalog, path, &partition.digest) {
            continue;
        }
        let records = execution
            .vectors
            .into_iter()
            .map(|vector| VectorRecord::new(vector.id.to_string(), vector.values))
            .collect();
        if let Err(error) = index.replace_partition(path, records).await {
            state.record_failure(path);
            tracing::warn!(path, %error, "workspace semantic partition publication failed");
            publish_progress(
                status,
                &snapshot,
                index,
                state,
                pending.len().saturating_sub(offset + 1),
                true,
            );
            continue;
        }
        if !catalog_digest_matches(catalog, path, &partition.digest) {
            remove_stale_partition(index, state, path).await;
            continue;
        }
        state.ready.insert(
            path.clone(),
            ReadyPartition {
                digest: Arc::clone(&partition.digest),
                chunk_count: partition.chunks.len(),
            },
        );
        state.failed.remove(path);
        publish_progress(
            status,
            &snapshot,
            index,
            state,
            pending.len().saturating_sub(offset + 1),
            false,
        );
    }
}

async fn invalidate_stale_partitions(
    index: &InMemoryVectorIndex,
    state: &mut BuildState,
    current: &BTreeMap<String, CatalogPartition>,
) {
    let stale = state
        .ready
        .iter()
        .filter(|(path, ready)| {
            !current
                .get(*path)
                .is_some_and(|partition| partition.digest == ready.digest)
        })
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    for path in stale {
        if !remove_stale_partition(index, state, &path).await {
            return;
        }
    }
}

async fn remove_stale_partition(
    index: &InMemoryVectorIndex,
    state: &mut BuildState,
    path: &str,
) -> bool {
    if let Err(error) = index.remove_partition(path).await {
        state.record_failure(path);
        tracing::warn!(path, %error, "workspace semantic partition invalidation failed");
        if let Err(clear_error) = index.clear().await {
            tracing::warn!(%clear_error, "workspace semantic index fallback clear failed");
        }
        state.ready.clear();
        return false;
    }
    state.ready.remove(path);
    state.failed.remove(path);
    true
}

fn catalog_partitions(snapshot: &ChunkCatalogSnapshot) -> BTreeMap<String, CatalogPartition> {
    let mut partitions = snapshot
        .paths()
        .into_iter()
        .filter_map(|path| {
            let digest = snapshot.content_digest(
                &crate::workspace::WorkspacePath::from_normalized(path.clone()),
            )?;
            Some((
                path,
                CatalogPartition {
                    digest,
                    chunks: Vec::new(),
                },
            ))
        })
        .collect::<BTreeMap<_, _>>();
    for chunk in snapshot.chunks().iter() {
        if let Some(partition) = partitions.get_mut(chunk.path.as_ref()) {
            partition.chunks.push(Arc::clone(chunk));
        }
    }
    partitions
}

fn catalog_digest_matches(catalog: &WorkspaceChunkCatalog, path: &str, digest: &str) -> bool {
    catalog.snapshot().ok().is_some_and(|snapshot| {
        snapshot
            .content_digest(&crate::workspace::WorkspacePath::from_normalized(path))
            .is_some_and(|current| current.as_ref() == digest)
    })
}

fn publish_progress(
    status: &RwLock<WorkspaceRetrievalStatus>,
    snapshot: &ChunkCatalogSnapshot,
    index: &InMemoryVectorIndex,
    state: &BuildState,
    queue_depth: usize,
    semantic_failure: bool,
) {
    let vector = index.status();
    let indexed_files = state
        .ready
        .iter()
        .filter(|(path, ready)| {
            snapshot
                .content_digest(&crate::workspace::WorkspacePath::from_normalized(*path))
                .is_some_and(|digest| digest == ready.digest)
        })
        .count();
    let indexed_chunks = state
        .ready
        .iter()
        .filter(|(path, ready)| {
            snapshot
                .content_digest(&crate::workspace::WorkspacePath::from_normalized(*path))
                .is_some_and(|digest| digest == ready.digest)
        })
        .map(|(_, ready)| ready.chunk_count)
        .sum();
    let eligible_files = snapshot.eligible_file_count();
    let failed_files = snapshot
        .failed_file_count()
        .saturating_add(state.failed.len());
    let coverage_bps = if eligible_files == 0 {
        10_000
    } else {
        indexed_files
            .saturating_mul(10_000)
            .checked_div(eligible_files)
            .unwrap_or_default()
            .min(10_000) as u16
    };
    let degraded = semantic_failure || failed_files > 0;
    let phase = if degraded {
        WorkspaceRetrievalPhase::Degraded
    } else if queue_depth == 0 && indexed_files == eligible_files {
        WorkspaceRetrievalPhase::Ready
    } else {
        WorkspaceRetrievalPhase::Building
    };
    let model = read_unpoisoned(status).model.clone();
    *write_unpoisoned(status) = WorkspaceRetrievalStatus {
        phase,
        catalog_revision: snapshot.revision(),
        source_revision: snapshot.source_revision(),
        vector_revision: vector.revision.value(),
        eligible_files,
        catalog_files: snapshot.file_count(),
        catalog_chunks: snapshot.chunk_count(),
        indexed_files,
        indexed_chunks,
        coverage_bps,
        queue_depth,
        failed_files,
        total_failures: state.total_failures,
        vector_records: vector.record_count,
        vector_bytes: vector.byte_count,
        model,
    };
}

fn publish_closed(status: &RwLock<WorkspaceRetrievalStatus>) {
    let mut closed = read_unpoisoned(status).clone();
    closed.phase = WorkspaceRetrievalPhase::Closed;
    closed.queue_depth = 0;
    closed.indexed_files = 0;
    closed.indexed_chunks = 0;
    closed.coverage_bps = 0;
    closed.vector_records = 0;
    closed.vector_bytes = 0;
    *write_unpoisoned(status) = closed;
}

fn lock_unpoisoned<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
