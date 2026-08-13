//! Deterministic orchestration for exact, lexical, structural, and semantic retrieval.

use super::hybrid_candidates::{
    exact_candidates, structural_candidates, StructuralCandidates, ValidatedHybridRequest,
    CHANNEL_CANDIDATE_LIMIT,
};
use super::hybrid_rank::{fuse_candidates, RankedCandidate};
use super::{
    retain_verified, ChunkCatalogSnapshot, WorkspaceHybridChannelStatus,
    WorkspaceHybridFallbackReason, WorkspaceHybridSearchRequest, WorkspaceHybridSearchResult,
    WorkspaceRetrievalChannel, WorkspaceRetrievalError, WorkspaceRetrievalRuntime,
    WorkspaceRetrievalStatus, WorkspaceSemanticFallbackReason,
};
use crate::code_intelligence::WorkspaceCodeIntelligence;
use crate::workspace::WorkspaceFileSystem;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const MAX_RESULTS: usize = 25;
const VERIFICATION_OVERFETCH: usize = 4;

impl WorkspaceRetrievalRuntime {
    /// Fuse bounded retrieval channels, then reread every selected source file
    /// once before returning any chunk text.
    pub(crate) async fn hybrid_search(
        &self,
        request: WorkspaceHybridSearchRequest,
        file_system: Arc<dyn WorkspaceFileSystem>,
        code_intelligence: Option<Arc<dyn WorkspaceCodeIntelligence>>,
        operation_timeout: Option<Duration>,
        cancellation: CancellationToken,
    ) -> Result<WorkspaceHybridSearchResult, WorkspaceRetrievalError> {
        let request = ValidatedHybridRequest::new(request)?;
        ensure_active(&self.child_lifetime(), &cancellation)?;
        let snapshot = self.catalog.snapshot()?;

        let lexical_request = request.lexical_request();
        let lexical = match snapshot.lexical_search(&lexical_request) {
            Ok(result) => Some(result),
            Err(super::WorkspaceIndexError::InvalidQuery(_)) => None,
            Err(error) => return Err(error.into()),
        };

        let semantic_request = request.semantic_request();
        let query_lifetime = self.child_lifetime();
        let exact_call = exact_candidates(&snapshot, &request, &cancellation);
        let semantic_call = self.search_candidates(semantic_request, query_lifetime.child_token());
        let structural_call = structural_candidates(
            code_intelligence,
            &snapshot,
            &request,
            query_lifetime.child_token(),
        );
        let operations = async { tokio::join!(exact_call, semantic_call, structural_call) };
        tokio::pin!(operations);
        let (exact, semantic, structural) = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                query_lifetime.cancel();
                return Err(WorkspaceRetrievalError::Cancelled);
            }
            _ = query_lifetime.cancelled() => {
                return Err(WorkspaceRetrievalError::Cancelled);
            }
            result = &mut operations => result,
        };
        let (exact, exact_truncated) = exact?;
        let exact_count = exact.len();
        let semantic = semantic?;
        let structural = structural?;

        if !catalog_revision_matches(&self.catalog.snapshot()?, &snapshot) {
            return Ok(revision_changed_result(
                semantic.status,
                &snapshot,
                channel_statuses(
                    exact_count,
                    exact_truncated,
                    lexical.as_ref(),
                    &structural,
                    0,
                    false,
                    Some(WorkspaceHybridFallbackReason::RevisionChanged),
                ),
            ));
        }

        let mut semantic_fallback = semantic.fallback.map(map_semantic_fallback);
        let semantic_compatible = semantic.hits.is_empty()
            || (semantic.status.catalog_revision == snapshot.revision()
                && semantic.status.source_revision == snapshot.source_revision());
        let semantic_hits = if semantic_compatible {
            semantic
                .hits
                .into_iter()
                .filter(|hit| hit.score.is_finite() && hit.score > 0.0)
                .collect()
        } else {
            semantic_fallback = Some(WorkspaceHybridFallbackReason::RevisionChanged);
            Vec::new()
        };
        let semantic_count = semantic_hits.len();
        let semantic_truncated = semantic.truncated;

        let mut candidates = Vec::with_capacity(
            exact.len()
                + lexical.as_ref().map_or(0, |result| result.hits.len())
                + structural.candidates.len()
                + semantic_count,
        );
        candidates.extend(exact);
        if let Some(lexical) = lexical.as_ref() {
            candidates.extend(
                lexical
                    .hits
                    .iter()
                    .enumerate()
                    .map(|(rank, hit)| RankedCandidate {
                        chunk: Arc::clone(&hit.chunk),
                        channel: WorkspaceRetrievalChannel::Lexical,
                        rank: rank + 1,
                        exact_identifier: false,
                    }),
            );
        }
        candidates.extend(
            structural
                .candidates
                .iter()
                .map(|candidate| RankedCandidate {
                    chunk: Arc::clone(&candidate.chunk),
                    channel: WorkspaceRetrievalChannel::Structural,
                    rank: candidate.rank,
                    exact_identifier: candidate.exact_identifier,
                }),
        );
        candidates.extend(semantic_hits.into_iter().enumerate().map(|(rank, hit)| {
            RankedCandidate {
                chunk: hit.chunk,
                channel: WorkspaceRetrievalChannel::Semantic,
                rank: rank + 1,
                exact_identifier: false,
            }
        }));

        let verification_limit = request
            .limit
            .saturating_mul(VERIFICATION_OVERFETCH)
            .min(MAX_RESULTS.saturating_mul(VERIFICATION_OVERFETCH));
        let fused = fuse_candidates(candidates, verification_limit);
        let runtime_cancellation = self.child_lifetime();
        let (hits, verification_filtered, verification_truncated) = retain_verified(
            fused,
            request.limit,
            |hit| hit.chunk.as_ref(),
            file_system.as_ref(),
            operation_timeout,
            &runtime_cancellation,
            &cancellation,
        )
        .await?;

        if !catalog_revision_matches(&self.catalog.snapshot()?, &snapshot) {
            return Ok(revision_changed_result(
                self.status(),
                &snapshot,
                channel_statuses(
                    exact_count,
                    exact_truncated,
                    lexical.as_ref(),
                    &structural,
                    semantic_count,
                    semantic_truncated,
                    semantic_fallback,
                ),
            ));
        }

        let channels = channel_statuses(
            exact_count,
            exact_truncated,
            lexical.as_ref(),
            &structural,
            semantic_count,
            semantic_truncated,
            semantic_fallback,
        );
        let channel_truncated = channels.iter().any(|channel| channel.truncated);
        let semantic_fallback = semantic_fallback.filter(|fallback| {
            !matches!(fallback, WorkspaceHybridFallbackReason::FilteredStaleHits)
        });
        let fallback = if verification_filtered {
            Some(WorkspaceHybridFallbackReason::FilteredStaleHits)
        } else {
            semantic_fallback.or_else(|| structural.global_fallback())
        };
        Ok(WorkspaceHybridSearchResult {
            hits,
            semantic_status: semantic.status,
            catalog_revision: snapshot.revision(),
            source_revision: snapshot.source_revision(),
            channels,
            truncated: channel_truncated || verification_truncated,
            fallback,
        })
    }
}

fn channel_statuses(
    exact_count: usize,
    exact_truncated: bool,
    lexical: Option<&super::LexicalSearchResult>,
    structural: &StructuralCandidates,
    semantic_count: usize,
    semantic_truncated: bool,
    semantic_fallback: Option<WorkspaceHybridFallbackReason>,
) -> Vec<WorkspaceHybridChannelStatus> {
    vec![
        WorkspaceHybridChannelStatus {
            channel: WorkspaceRetrievalChannel::Exact,
            candidate_count: exact_count,
            truncated: exact_truncated,
            fallback: None,
        },
        WorkspaceHybridChannelStatus {
            channel: WorkspaceRetrievalChannel::Lexical,
            candidate_count: lexical.map_or(0, |result| result.hits.len()),
            truncated: lexical.is_some_and(|result| {
                result.candidate_truncated || result.hits.len() == CHANNEL_CANDIDATE_LIMIT
            }),
            fallback: None,
        },
        WorkspaceHybridChannelStatus {
            channel: WorkspaceRetrievalChannel::Structural,
            candidate_count: structural.candidates.len(),
            truncated: structural.truncated,
            fallback: structural.fallback,
        },
        WorkspaceHybridChannelStatus {
            channel: WorkspaceRetrievalChannel::Semantic,
            candidate_count: semantic_count,
            truncated: semantic_truncated,
            fallback: semantic_fallback,
        },
    ]
}

fn map_semantic_fallback(
    fallback: WorkspaceSemanticFallbackReason,
) -> WorkspaceHybridFallbackReason {
    match fallback {
        WorkspaceSemanticFallbackReason::Building => WorkspaceHybridFallbackReason::Building,
        WorkspaceSemanticFallbackReason::Degraded => WorkspaceHybridFallbackReason::Degraded,
        WorkspaceSemanticFallbackReason::Closed => WorkspaceHybridFallbackReason::Unavailable,
        WorkspaceSemanticFallbackReason::QueryEmbeddingFailed => {
            WorkspaceHybridFallbackReason::QueryEmbeddingFailed
        }
        WorkspaceSemanticFallbackReason::VectorSearchFailed => {
            WorkspaceHybridFallbackReason::VectorSearchFailed
        }
        WorkspaceSemanticFallbackReason::RevisionChanged => {
            WorkspaceHybridFallbackReason::RevisionChanged
        }
        WorkspaceSemanticFallbackReason::FilteredStaleHits => {
            WorkspaceHybridFallbackReason::FilteredStaleHits
        }
    }
}

fn revision_changed_result(
    semantic_status: WorkspaceRetrievalStatus,
    snapshot: &ChunkCatalogSnapshot,
    channels: Vec<WorkspaceHybridChannelStatus>,
) -> WorkspaceHybridSearchResult {
    WorkspaceHybridSearchResult {
        hits: Vec::new(),
        semantic_status,
        catalog_revision: snapshot.revision(),
        source_revision: snapshot.source_revision(),
        channels,
        truncated: false,
        fallback: Some(WorkspaceHybridFallbackReason::RevisionChanged),
    }
}

fn catalog_revision_matches(
    current: &ChunkCatalogSnapshot,
    expected: &ChunkCatalogSnapshot,
) -> bool {
    current.revision() == expected.revision()
        && current.source_revision() == expected.source_revision()
}

fn ensure_active(
    runtime: &CancellationToken,
    caller: &CancellationToken,
) -> Result<(), WorkspaceRetrievalError> {
    if runtime.is_cancelled() || caller.is_cancelled() {
        return Err(WorkspaceRetrievalError::Cancelled);
    }
    Ok(())
}
