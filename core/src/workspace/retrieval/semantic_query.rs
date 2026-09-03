use super::vector_contract::{VectorSearchRequest, WorkspaceVectorIndex};
use super::{
    retain_verified, WorkspaceChunk, WorkspaceRetrievalError, WorkspaceRetrievalPhase,
    WorkspaceRetrievalRuntime, WorkspaceSemanticFallbackReason, WorkspaceSemanticSearchHit,
    WorkspaceSemanticSearchRequest, WorkspaceSemanticSearchResult,
};
use crate::embedding::EmbeddingInput;
use crate::workspace::WorkspaceFileSystem;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const MAX_QUERY_BYTES: usize = 16 * 1024;
const MAX_RESULTS: usize = 100;
const QUERY_ID: &str = "workspace-query";

impl WorkspaceRetrievalRuntime {
    /// Search the ready vector partitions and return only catalog-verified
    /// chunks from the exact observed revision.
    pub(crate) async fn search(
        &self,
        mut request: WorkspaceSemanticSearchRequest,
        file_system: Arc<dyn WorkspaceFileSystem>,
        operation_timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<WorkspaceSemanticSearchResult, WorkspaceRetrievalError> {
        let requested_limit = request.limit;
        request.limit = requested_limit.saturating_mul(4).min(MAX_RESULTS);
        let runtime_cancellation = self.child_lifetime();
        let mut result = self
            .search_candidates(request, cancellation.clone())
            .await?;
        let (hits, verification_filtered, verification_truncated) = retain_verified(
            result.hits,
            requested_limit,
            |hit: &WorkspaceSemanticSearchHit| hit.chunk.as_ref(),
            file_system.as_ref(),
            operation_timeout,
            &runtime_cancellation,
            &cancellation,
        )
        .await?;
        result.hits = hits;
        result.truncated |= verification_truncated;
        if verification_filtered {
            result.fallback = Some(WorkspaceSemanticFallbackReason::FilteredStaleHits);
        }
        if !self.result_revision_matches(&result.status, &runtime_cancellation, &cancellation)? {
            return Ok(empty_result(
                self.status(),
                WorkspaceSemanticFallbackReason::RevisionChanged,
            ));
        }
        Ok(result)
    }

    pub(super) async fn search_candidates(
        &self,
        request: WorkspaceSemanticSearchRequest,
        cancellation: CancellationToken,
    ) -> Result<WorkspaceSemanticSearchResult, WorkspaceRetrievalError> {
        let request = ValidatedSemanticRequest::new(request)?;
        if cancellation.is_cancelled() {
            return Err(WorkspaceRetrievalError::Cancelled);
        }
        let runtime_cancellation = self.child_lifetime();
        let observed_status = self
            .wait_for_semantic_readiness(&runtime_cancellation, &cancellation)
            .await?;
        if observed_status.phase == WorkspaceRetrievalPhase::Closed {
            return Ok(empty_result(
                observed_status,
                WorkspaceSemanticFallbackReason::Closed,
            ));
        }
        if observed_status.indexed_chunks == 0 {
            return Ok(match observed_status.phase {
                WorkspaceRetrievalPhase::Ready | WorkspaceRetrievalPhase::Disabled => {
                    WorkspaceSemanticSearchResult {
                        hits: Vec::new(),
                        status: observed_status,
                        searched_records: 0,
                        truncated: false,
                        fallback: None,
                    }
                }
                WorkspaceRetrievalPhase::Degraded => {
                    empty_result(observed_status, WorkspaceSemanticFallbackReason::Degraded)
                }
                WorkspaceRetrievalPhase::Building => {
                    empty_result(observed_status, WorkspaceSemanticFallbackReason::Building)
                }
                WorkspaceRetrievalPhase::Closed => {
                    empty_result(observed_status, WorkspaceSemanticFallbackReason::Closed)
                }
            });
        }
        let Some(index) = self.index() else {
            return Ok(empty_result(
                observed_status,
                WorkspaceSemanticFallbackReason::Closed,
            ));
        };

        let query_cancellation = runtime_cancellation.child_token();
        let _query_cancellation_guard = query_cancellation.clone().drop_guard();
        let query_call = self.executor.embed(
            vec![EmbeddingInput::new(QUERY_ID, request.query.clone())],
            query_cancellation.clone(),
        );
        tokio::pin!(query_call);
        let query_result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                query_cancellation.cancel();
                return Err(WorkspaceRetrievalError::Cancelled);
            }
            result = &mut query_call => result,
        };
        let query = match query_result {
            Ok(execution) => match execution.vectors.into_iter().next() {
                Some(vector) => vector,
                None => {
                    return Ok(empty_result(
                        self.status(),
                        WorkspaceSemanticFallbackReason::QueryEmbeddingFailed,
                    ))
                }
            },
            Err(crate::embedding::EmbeddingError::Cancelled)
                if cancellation.is_cancelled() || runtime_cancellation.is_cancelled() =>
            {
                return Err(WorkspaceRetrievalError::Cancelled)
            }
            Err(_) => {
                return Ok(empty_result(
                    self.status(),
                    WorkspaceSemanticFallbackReason::QueryEmbeddingFailed,
                ))
            }
        };

        let catalog_snapshot = self.catalog.snapshot()?;
        let status_before_search = self.status();
        if status_before_search.catalog_revision != catalog_snapshot.revision()
            || status_before_search.source_revision != catalog_snapshot.source_revision()
        {
            return Ok(empty_result(
                status_before_search,
                WorkspaceSemanticFallbackReason::RevisionChanged,
            ));
        }
        let chunks = eligible_chunks(&catalog_snapshot, &request);
        let mut partitions = chunks
            .values()
            .map(|chunk| chunk.path.to_string())
            .collect::<std::collections::BTreeSet<_>>();
        if partitions.is_empty() {
            let fallback = semantic_coverage_fallback(status_before_search.phase);
            return Ok(WorkspaceSemanticSearchResult {
                hits: Vec::new(),
                status: status_before_search,
                searched_records: 0,
                truncated: false,
                fallback,
            });
        }

        let candidate_limit = request
            .limit
            .saturating_mul(4)
            .max(request.limit)
            .min(MAX_RESULTS.saturating_mul(4));
        let mut vector_request = VectorSearchRequest::new(query.values, candidate_limit);
        vector_request.partitions.append(&mut partitions);
        let vector_search = index.search(vector_request);
        tokio::pin!(vector_search);
        let vector_result = match tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(WorkspaceRetrievalError::Cancelled),
            _ = runtime_cancellation.cancelled() => return Err(WorkspaceRetrievalError::Cancelled),
            result = &mut vector_search => result,
        } {
            Ok(result) => result,
            Err(_) => {
                return Ok(empty_result(
                    self.status(),
                    WorkspaceSemanticFallbackReason::VectorSearchFailed,
                ))
            }
        };
        let after_search = self.status();
        let current_catalog_revision = self.catalog.snapshot()?.revision();
        if after_search.phase == WorkspaceRetrievalPhase::Closed {
            return Ok(empty_result(
                after_search,
                WorkspaceSemanticFallbackReason::Closed,
            ));
        }
        if after_search.catalog_revision != status_before_search.catalog_revision
            || after_search.source_revision != status_before_search.source_revision
            || after_search.vector_revision != status_before_search.vector_revision
            || vector_result.status.revision.value() != status_before_search.vector_revision
            || current_catalog_revision != catalog_snapshot.revision()
        {
            return Ok(empty_result(
                after_search,
                WorkspaceSemanticFallbackReason::RevisionChanged,
            ));
        }

        let searched_records = vector_result.searched_records;
        let mut truncated = vector_result.truncated;
        let mut filtered_stale = false;
        let candidates = vector_result
            .hits
            .into_iter()
            .filter_map(|hit| {
                let Some(chunk) = chunks.get(hit.id.as_str()) else {
                    filtered_stale = true;
                    return None;
                };
                if hit.partition != chunk.path.as_ref() {
                    filtered_stale = true;
                    return None;
                }
                Some(WorkspaceSemanticSearchHit {
                    chunk: Arc::clone(chunk),
                    score: hit.score,
                })
            })
            .collect::<Vec<_>>();
        truncated |= candidates.len() > request.limit;
        let hits = candidates.into_iter().take(request.limit).collect();
        let final_status = self.status();
        let final_catalog_revision = self.catalog.snapshot()?.revision();
        if final_status.phase == WorkspaceRetrievalPhase::Closed {
            return Ok(empty_result(
                final_status,
                WorkspaceSemanticFallbackReason::Closed,
            ));
        }
        if final_status.catalog_revision != status_before_search.catalog_revision
            || final_status.source_revision != status_before_search.source_revision
            || final_status.vector_revision != status_before_search.vector_revision
            || final_catalog_revision != catalog_snapshot.revision()
        {
            return Ok(empty_result(
                final_status,
                WorkspaceSemanticFallbackReason::RevisionChanged,
            ));
        }
        let fallback = if filtered_stale {
            Some(WorkspaceSemanticFallbackReason::FilteredStaleHits)
        } else {
            semantic_coverage_fallback(final_status.phase)
        };
        Ok(WorkspaceSemanticSearchResult {
            hits,
            status: final_status,
            searched_records,
            truncated,
            fallback,
        })
    }

    pub(super) fn result_revision_matches(
        &self,
        expected: &super::WorkspaceRetrievalStatus,
        runtime_cancellation: &CancellationToken,
        caller_cancellation: &CancellationToken,
    ) -> Result<bool, WorkspaceRetrievalError> {
        if caller_cancellation.is_cancelled() || runtime_cancellation.is_cancelled() {
            return Err(WorkspaceRetrievalError::Cancelled);
        }
        let current = self.status();
        let catalog_revision = self.catalog.snapshot()?.revision();
        if current.phase == WorkspaceRetrievalPhase::Closed
            || current.catalog_revision != expected.catalog_revision
            || current.source_revision != expected.source_revision
            || current.vector_revision != expected.vector_revision
            || catalog_revision != expected.catalog_revision
        {
            return Ok(false);
        }
        Ok(true)
    }
}

struct ValidatedSemanticRequest {
    query: String,
    path: Option<crate::workspace::WorkspacePath>,
    include: Option<glob::Pattern>,
    limit: usize,
}

impl ValidatedSemanticRequest {
    fn new(request: WorkspaceSemanticSearchRequest) -> Result<Self, WorkspaceRetrievalError> {
        let query = request.query.trim();
        if query.is_empty() || query.len() > MAX_QUERY_BYTES {
            return Err(WorkspaceRetrievalError::InvalidQuery(format!(
                "query must contain 1..={MAX_QUERY_BYTES} UTF-8 bytes"
            )));
        }
        if request.limit == 0 || request.limit > MAX_RESULTS {
            return Err(WorkspaceRetrievalError::InvalidQuery(format!(
                "limit must be between 1 and {MAX_RESULTS}"
            )));
        }
        // WorkspaceServices already applied the host-provided resolver. The
        // runtime must not reinterpret a custom backend's normalized path with
        // local-filesystem rules.
        let path = request
            .path
            .map(crate::workspace::WorkspacePath::from_normalized);
        let include = request
            .include
            .map(|pattern| {
                crate::workspace::validate_relative_pattern(&pattern, "semantic include pattern")
                    .map_err(|error| WorkspaceRetrievalError::InvalidQuery(error.to_string()))?;
                glob::Pattern::new(&pattern)
                    .map_err(|error| WorkspaceRetrievalError::InvalidQuery(error.to_string()))
            })
            .transpose()?;
        Ok(Self {
            query: query.to_owned(),
            path,
            include,
            limit: request.limit,
        })
    }
}

fn eligible_chunks(
    snapshot: &super::ChunkCatalogSnapshot,
    request: &ValidatedSemanticRequest,
) -> HashMap<String, Arc<WorkspaceChunk>> {
    let mut chunks = HashMap::new();
    for chunk in snapshot.chunks().iter() {
        if !path_matches(chunk.path.as_ref(), request.path.as_ref())
            || !include_matches(chunk.path.as_ref(), request.include.as_ref())
        {
            continue;
        }
        chunks.insert(chunk.id.as_str().to_owned(), Arc::clone(chunk));
    }
    chunks
}

fn path_matches(path: &str, base: Option<&crate::workspace::WorkspacePath>) -> bool {
    base.is_none_or(|base| {
        base.is_root()
            || path == base.as_str()
            || path
                .strip_prefix(base.as_str())
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn include_matches(path: &str, include: Option<&glob::Pattern>) -> bool {
    include.is_none_or(|include| {
        let path = std::path::Path::new(path);
        include.matches_path(path)
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| include.matches(name))
    })
}

fn empty_result(
    status: super::WorkspaceRetrievalStatus,
    fallback: WorkspaceSemanticFallbackReason,
) -> WorkspaceSemanticSearchResult {
    WorkspaceSemanticSearchResult {
        hits: Vec::new(),
        status,
        searched_records: 0,
        truncated: false,
        fallback: Some(fallback),
    }
}

fn semantic_coverage_fallback(
    phase: WorkspaceRetrievalPhase,
) -> Option<WorkspaceSemanticFallbackReason> {
    match phase {
        WorkspaceRetrievalPhase::Building => Some(WorkspaceSemanticFallbackReason::Building),
        WorkspaceRetrievalPhase::Degraded => Some(WorkspaceSemanticFallbackReason::Degraded),
        WorkspaceRetrievalPhase::Closed => Some(WorkspaceSemanticFallbackReason::Closed),
        WorkspaceRetrievalPhase::Disabled | WorkspaceRetrievalPhase::Ready => None,
    }
}
