use super::memory_vector_adapter::MemoryVectorIndexAdapter;
use super::semantic_batch::plan_semantic_batches;
use super::semantic_runtime::{BuildState, CatalogPartition, ReadyPartition};
use super::semantic_status::SemanticStatusCell;
use super::vector_contract::{VectorRecord, WorkspaceVectorIndex};
use super::{
    ChunkCatalogSnapshot, WorkspaceChunkCatalog, WorkspaceRetrievalPhase, WorkspaceRetrievalStatus,
};
use crate::embedding::EmbeddingExecutor;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) struct ProjectionContext<'a> {
    pub(super) catalog: &'a WorkspaceChunkCatalog,
    pub(super) index: &'a MemoryVectorIndexAdapter,
    pub(super) executor: &'a EmbeddingExecutor,
    pub(super) status: &'a SemanticStatusCell,
    pub(super) snapshot: &'a ChunkCatalogSnapshot,
    pub(super) partitions: &'a BTreeMap<String, CatalogPartition>,
    pub(super) pending: &'a [String],
    pub(super) cancellation: &'a CancellationToken,
}

pub(super) async fn project_pending_partitions(
    context: ProjectionContext<'_>,
    state: &mut BuildState,
) {
    let ProjectionContext {
        catalog,
        index,
        executor,
        status,
        snapshot,
        partitions,
        pending,
        cancellation,
    } = context;
    let plan = plan_semantic_batches(
        pending.iter().filter_map(|path| {
            partitions
                .get(path)
                .map(|partition| (path.as_str(), partition.chunks.as_slice()))
        }),
        executor.descriptor().dimension,
        executor.config(),
    );
    state.batching.document_inputs = plan.document_inputs;
    state.batching.document_text_bytes = plan.document_text_bytes;
    state.batching.batch_limit_lower_bound = plan.batch_limit_lower_bound;
    publish_progress(status, snapshot, index, state, pending.len(), false);

    let mut remaining = pending.iter().cloned().collect::<BTreeSet<_>>();
    let mut unpublished = BTreeMap::<String, Vec<VectorRecord>>::new();
    for path in pending {
        let Some(partition) = partitions.get(path) else {
            continue;
        };
        if !partition.chunks.is_empty() {
            unpublished.insert(path.clone(), Vec::with_capacity(partition.chunks.len()));
            continue;
        }
        state.ready.insert(
            path.clone(),
            ReadyPartition {
                digest: Arc::clone(&partition.digest),
                chunk_count: 0,
            },
        );
        state.record_first_ready();
        remaining.remove(path);
        publish_progress(status, snapshot, index, state, remaining.len(), false);
    }

    tracing::debug!(
        document_inputs = plan.document_inputs,
        document_text_bytes = plan.document_text_bytes,
        provider_batches = plan.batches.len(),
        batch_limit_lower_bound = plan.batch_limit_lower_bound,
        "planned workspace semantic embedding generation"
    );

    for batch in plan.batches {
        if cancellation.is_cancelled() || !catalog_revision_matches(catalog, snapshot.revision()) {
            return;
        }
        let entries = batch
            .entries
            .into_iter()
            .filter(|entry| remaining.contains(&entry.path))
            .collect::<Vec<_>>();
        if entries.is_empty() {
            continue;
        }
        let active_paths = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<BTreeSet<_>>();
        let inputs = entries.iter().map(|entry| entry.input.clone()).collect();
        state.record_flush(batch.flush_reason);
        let provider_requests = AtomicUsize::new(0);
        let result = executor
            .embed_counted(inputs, cancellation.child_token(), &provider_requests)
            .await;
        state.batching.document_provider_requests = state
            .batching
            .document_provider_requests
            .saturating_add(provider_requests.load(Ordering::Relaxed));
        let execution = match result {
            Ok(execution) => execution,
            Err(_) if cancellation.is_cancelled() => return,
            Err(error) => {
                for path in &active_paths {
                    state.record_failure(path);
                    remaining.remove(path);
                    unpublished.remove(path);
                }
                tracing::warn!(
                    paths = ?active_paths,
                    flush_reason = ?batch.flush_reason,
                    error = %error,
                    "workspace semantic embedding batch failed"
                );
                publish_progress(status, snapshot, index, state, remaining.len(), true);
                continue;
            }
        };
        debug_assert_eq!(execution.batch_count, 1);
        if cancellation.is_cancelled() || !catalog_revision_matches(catalog, snapshot.revision()) {
            return;
        }
        for (entry, vector) in entries.into_iter().zip(execution.vectors) {
            if let Some(records) = unpublished.get_mut(&entry.path) {
                records.push(VectorRecord::new(vector.id.to_string(), vector.values));
            }
        }

        let completed = active_paths
            .into_iter()
            .filter(|path| {
                partitions.get(path).is_some_and(|partition| {
                    unpublished
                        .get(path)
                        .is_some_and(|records| records.len() == partition.chunks.len())
                })
            })
            .collect::<Vec<_>>();
        for path in completed {
            if cancellation.is_cancelled()
                || !catalog_revision_matches(catalog, snapshot.revision())
            {
                return;
            }
            let Some(partition) = partitions.get(&path) else {
                continue;
            };
            let records = unpublished.remove(&path).unwrap_or_default();
            if let Err(error) = index.replace_partition(&path, records).await {
                state.record_failure(&path);
                remaining.remove(&path);
                tracing::warn!(path, %error, "workspace semantic partition publication failed");
                publish_progress(status, snapshot, index, state, remaining.len(), true);
                continue;
            }
            if cancellation.is_cancelled()
                || !catalog_digest_matches(catalog, &path, &partition.digest)
                || !catalog_revision_matches(catalog, snapshot.revision())
            {
                remove_stale_partition(index, state, &path).await;
                return;
            }
            state.ready.insert(
                path.clone(),
                ReadyPartition {
                    digest: Arc::clone(&partition.digest),
                    chunk_count: partition.chunks.len(),
                },
            );
            state.record_first_ready();
            state.failed.remove(&path);
            remaining.remove(&path);
            publish_progress(status, snapshot, index, state, remaining.len(), false);
        }
    }
}

pub(super) async fn remove_stale_partition(
    index: &MemoryVectorIndexAdapter,
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

fn catalog_digest_matches(catalog: &WorkspaceChunkCatalog, path: &str, digest: &str) -> bool {
    catalog.snapshot().ok().is_some_and(|snapshot| {
        snapshot
            .content_digest(&crate::workspace::WorkspacePath::from_normalized(path))
            .is_some_and(|current| current.as_ref() == digest)
    })
}

pub(super) fn catalog_revision_matches(catalog: &WorkspaceChunkCatalog, revision: u64) -> bool {
    catalog
        .snapshot()
        .ok()
        .is_some_and(|snapshot| snapshot.revision() == revision)
}

pub(super) fn publish_progress(
    status: &SemanticStatusCell,
    snapshot: &ChunkCatalogSnapshot,
    index: &MemoryVectorIndexAdapter,
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
    let model = status.load().model;
    status.publish(WorkspaceRetrievalStatus {
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
        lexical_engine: snapshot.lexical_engine(),
        batching: state.batching.clone(),
        model,
    });
}
