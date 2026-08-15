use super::semantic_batch::SemanticBatchFlushReason;
use super::semantic_projection::{
    project_pending_partitions, publish_progress, remove_stale_partition, ProjectionContext,
};
use super::semantic_status::SemanticStatusCell;
use super::{
    ChunkCatalogSnapshot, WorkspaceChunk, WorkspaceChunkCatalog, WorkspaceEmbeddingBatchMetrics,
    WorkspaceRetrievalOptions, WorkspaceRetrievalPhase, WorkspaceRetrievalResult,
    WorkspaceRetrievalStatus,
};
use crate::embedding::EmbeddingExecutor;
use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;
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
    status: Arc<SemanticStatusCell>,
    lifetime: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
    close_gate: tokio::sync::Mutex<()>,
    shutdown_timeout: std::time::Duration,
    pub(super) rerank_options: super::WorkspaceRerankOptions,
    pub(super) semantic_readiness_timeout: std::time::Duration,
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
        options.validate_semantic_readiness_timeout()?;
        let limits = options.index_limits.validate()?;
        let rerank_options = options.rerank.validate()?;
        let semantic_readiness_timeout = options.semantic_readiness_timeout;
        let executor = EmbeddingExecutor::new(options.provider, options.embedding)?;
        let descriptor = VectorIndexDescriptor::new(executor.descriptor().dimension)
            .with_max_records(limits.max_records)
            .with_max_bytes(limits.max_bytes);
        let index = Arc::new(InMemoryVectorIndex::new(descriptor)?);
        let status = Arc::new(SemanticStatusCell::new(WorkspaceRetrievalStatus::building(
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
            semantic_readiness_timeout,
        });
        let task = tokio::spawn(run_semantic_updates(
            catalog, index, executor, status, lifetime,
        ));
        *lock_unpoisoned(&runtime.task) = Some(task);
        Ok(runtime)
    }

    /// Return a lock-free observation of current partial readiness.
    pub fn status(&self) -> WorkspaceRetrievalStatus {
        self.status.load()
    }

    pub(super) fn index(&self) -> Option<Arc<InMemoryVectorIndex>> {
        lock_unpoisoned(&self.index).clone()
    }

    pub(super) fn child_lifetime(&self) -> CancellationToken {
        self.lifetime.child_token()
    }

    pub(super) async fn wait_for_semantic_readiness(
        &self,
        runtime_cancellation: &CancellationToken,
        caller_cancellation: &CancellationToken,
    ) -> WorkspaceRetrievalResult<WorkspaceRetrievalStatus> {
        self.status
            .wait_for_readiness(
                self.semantic_readiness_timeout,
                runtime_cancellation,
                caller_cancellation,
            )
            .await
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
pub(super) struct ReadyPartition {
    pub(super) digest: Arc<str>,
    pub(super) chunk_count: usize,
}

#[derive(Clone)]
pub(super) struct CatalogPartition {
    pub(super) digest: Arc<str>,
    pub(super) chunks: Vec<Arc<WorkspaceChunk>>,
}

pub(super) struct BuildState {
    pub(super) ready: BTreeMap<String, ReadyPartition>,
    pub(super) failed: BTreeSet<String>,
    pub(super) total_failures: u64,
    observed_catalog_revision: u64,
    pub(super) batching: WorkspaceEmbeddingBatchMetrics,
    generation_started: Option<Instant>,
}

impl BuildState {
    fn new() -> Self {
        Self {
            ready: BTreeMap::new(),
            failed: BTreeSet::new(),
            total_failures: 0,
            observed_catalog_revision: 0,
            batching: WorkspaceEmbeddingBatchMetrics::default(),
            generation_started: None,
        }
    }

    pub(super) fn record_failure(&mut self, path: &str) {
        self.failed.insert(path.to_owned());
        self.total_failures = self.total_failures.saturating_add(1);
    }

    pub(super) fn record_flush(&mut self, reason: SemanticBatchFlushReason) {
        self.batching.document_batches = self.batching.document_batches.saturating_add(1);
        let counter = match reason {
            SemanticBatchFlushReason::InputLimit => &mut self.batching.input_limit_flushes,
            SemanticBatchFlushReason::TextByteLimit => &mut self.batching.text_byte_limit_flushes,
            SemanticBatchFlushReason::VectorByteLimit => {
                &mut self.batching.vector_byte_limit_flushes
            }
            SemanticBatchFlushReason::GenerationComplete => {
                &mut self.batching.generation_complete_flushes
            }
        };
        *counter = counter.saturating_add(1);
    }

    pub(super) fn record_first_ready(&mut self) {
        if self.batching.time_to_first_ready_ms.is_some() {
            return;
        }
        self.batching.time_to_first_ready_ms = self
            .generation_started
            .map(|started| started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
    }
}

async fn run_semantic_updates(
    catalog: Arc<WorkspaceChunkCatalog>,
    index: Arc<InMemoryVectorIndex>,
    executor: EmbeddingExecutor,
    status: Arc<SemanticStatusCell>,
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
    status: &SemanticStatusCell,
    state: &mut BuildState,
    snapshot: ChunkCatalogSnapshot,
    cancellation: CancellationToken,
) {
    if state.observed_catalog_revision != snapshot.revision() {
        state.total_failures = state
            .total_failures
            .saturating_add(snapshot.failed_file_count() as u64);
        state.observed_catalog_revision = snapshot.revision();
        state.batching = WorkspaceEmbeddingBatchMetrics::default();
        state.generation_started = Some(Instant::now());
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
    project_pending_partitions(
        ProjectionContext {
            catalog,
            index,
            executor,
            status,
            snapshot: &snapshot,
            partitions: &partitions,
            pending: &pending,
            cancellation: &cancellation,
        },
        state,
    )
    .await;
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

fn publish_closed(status: &SemanticStatusCell) {
    let mut closed = status.load();
    closed.phase = WorkspaceRetrievalPhase::Closed;
    closed.queue_depth = 0;
    closed.indexed_files = 0;
    closed.indexed_chunks = 0;
    closed.coverage_bps = 0;
    closed.vector_records = 0;
    closed.vector_bytes = 0;
    status.publish(closed);
}

fn lock_unpoisoned<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
