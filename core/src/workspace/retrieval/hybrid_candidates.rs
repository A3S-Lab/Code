//! Bounded candidate generation for hybrid workspace retrieval.

use super::hybrid_rank::RankedCandidate;
use super::{
    ChunkCatalogSnapshot, LexicalSearchRequest, WorkspaceChunk, WorkspaceHybridFallbackReason,
    WorkspaceHybridSearchRequest, WorkspaceRetrievalChannel, WorkspaceRetrievalError,
    WorkspaceSemanticSearchRequest,
};
use crate::code_intelligence::{
    CodeIntelligenceState, SymbolInformation, WorkspaceCodeIntelligence,
};
use crate::workspace::WorkspacePath;
use std::cmp::Ordering;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const MAX_QUERY_BYTES: usize = 2_048;
const MAX_RESULTS: usize = 25;
pub(super) const CHANNEL_CANDIDATE_LIMIT: usize = 25;
const STRUCTURAL_FETCH_LIMIT: usize = 100;

pub(super) struct ValidatedHybridRequest {
    query: String,
    path: Option<WorkspacePath>,
    include_source: Option<String>,
    include: Option<glob::Pattern>,
    pub(super) limit: usize,
}

impl ValidatedHybridRequest {
    pub(super) fn new(
        request: WorkspaceHybridSearchRequest,
    ) -> Result<Self, WorkspaceRetrievalError> {
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
        let path = request.path.map(WorkspacePath::from_normalized);
        let include_source = request.include;
        let include = include_source
            .as_deref()
            .map(|pattern| {
                crate::workspace::validate_relative_pattern(pattern, "hybrid include pattern")
                    .map_err(|error| WorkspaceRetrievalError::InvalidQuery(error.to_string()))?;
                glob::Pattern::new(pattern)
                    .map_err(|error| WorkspaceRetrievalError::InvalidQuery(error.to_string()))
            })
            .transpose()?;
        Ok(Self {
            query: query.to_owned(),
            path,
            include_source,
            include,
            limit: request.limit,
        })
    }

    fn matches(&self, path: &str) -> bool {
        path_matches(path, self.path.as_ref()) && include_matches(path, self.include.as_ref())
    }

    pub(super) fn lexical_request(&self) -> LexicalSearchRequest {
        let mut request = LexicalSearchRequest::new(&self.query);
        request.path = self.path.clone().unwrap_or_else(WorkspacePath::root);
        request.glob = self.include_source.clone();
        request.limit = CHANNEL_CANDIDATE_LIMIT;
        request.max_results_per_file = CHANNEL_CANDIDATE_LIMIT;
        request
    }

    pub(super) fn semantic_request(&self) -> WorkspaceSemanticSearchRequest {
        WorkspaceSemanticSearchRequest {
            query: self.query.clone(),
            path: self.path.as_ref().map(|path| path.as_str().to_owned()),
            include: self.include_source.clone(),
            limit: CHANNEL_CANDIDATE_LIMIT,
        }
    }
}

pub(super) async fn exact_candidates(
    snapshot: &ChunkCatalogSnapshot,
    request: &ValidatedHybridRequest,
    cancellation: &CancellationToken,
) -> Result<(Vec<RankedCandidate>, bool), WorkspaceRetrievalError> {
    let identifier_query = is_ascii_identifier(&request.query);
    let lowered_query = request.query.to_lowercase();
    let mut matches = Vec::<ExactCandidate>::with_capacity(CHANNEL_CANDIDATE_LIMIT);
    let mut match_count = 0usize;
    for (index, chunk) in snapshot.chunks().iter().enumerate() {
        if index % 256 == 0 {
            if cancellation.is_cancelled() {
                return Err(WorkspaceRetrievalError::Cancelled);
            }
            tokio::task::yield_now().await;
        }
        if !request.matches(chunk.path.as_ref()) {
            continue;
        }
        let exact_identifier = identifier_query && contains_identifier(&chunk.text, &request.query);
        let tier = if exact_identifier {
            3
        } else if chunk.text.contains(&request.query) {
            2
        } else if contains_ascii_case_insensitive(&chunk.text, &lowered_query) {
            1
        } else {
            continue;
        };
        match_count = match_count.saturating_add(1);
        matches.push(ExactCandidate {
            chunk: Arc::clone(chunk),
            tier,
            exact_identifier,
        });
        matches.sort_by(compare_exact);
        matches.truncate(CHANNEL_CANDIDATE_LIMIT);
    }
    Ok((
        matches
            .into_iter()
            .enumerate()
            .map(|(rank, candidate)| RankedCandidate {
                chunk: candidate.chunk,
                channel: WorkspaceRetrievalChannel::Exact,
                rank: rank + 1,
                exact_identifier: candidate.exact_identifier,
            })
            .collect(),
        match_count > CHANNEL_CANDIDATE_LIMIT,
    ))
}

struct ExactCandidate {
    chunk: Arc<WorkspaceChunk>,
    tier: u8,
    exact_identifier: bool,
}

fn compare_exact(left: &ExactCandidate, right: &ExactCandidate) -> Ordering {
    right
        .tier
        .cmp(&left.tier)
        .then_with(|| left.chunk.path.cmp(&right.chunk.path))
        .then_with(|| left.chunk.start_byte.cmp(&right.chunk.start_byte))
        .then_with(|| left.chunk.id.cmp(&right.chunk.id))
}

fn is_ascii_identifier(query: &str) -> bool {
    let mut chars = query.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn contains_identifier(text: &str, identifier: &str) -> bool {
    text.match_indices(identifier).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let end = start + identifier.len();
        let after = text[end..].chars().next();
        !before.is_some_and(is_identifier_continue) && !after.is_some_and(is_identifier_continue)
    })
}

fn contains_ascii_case_insensitive(text: &str, lowered_query: &str) -> bool {
    text.as_bytes()
        .windows(lowered_query.len())
        .any(|window| window.eq_ignore_ascii_case(lowered_query.as_bytes()))
}

fn is_identifier_continue(character: char) -> bool {
    character == '_' || character.is_ascii_alphanumeric()
}

pub(super) struct StructuralCandidate {
    pub(super) chunk: Arc<WorkspaceChunk>,
    pub(super) rank: usize,
    pub(super) exact_identifier: bool,
}

pub(super) struct StructuralCandidates {
    pub(super) candidates: Vec<StructuralCandidate>,
    pub(super) truncated: bool,
    pub(super) fallback: Option<WorkspaceHybridFallbackReason>,
}

impl StructuralCandidates {
    fn unavailable(fallback: WorkspaceHybridFallbackReason) -> Self {
        Self {
            candidates: Vec::new(),
            truncated: false,
            fallback: Some(fallback),
        }
    }

    pub(super) fn global_fallback(&self) -> Option<WorkspaceHybridFallbackReason> {
        self.fallback
            .filter(|fallback| !matches!(fallback, WorkspaceHybridFallbackReason::Unavailable))
    }
}

pub(super) async fn structural_candidates(
    provider: Option<Arc<dyn WorkspaceCodeIntelligence>>,
    snapshot: &ChunkCatalogSnapshot,
    request: &ValidatedHybridRequest,
    cancellation: CancellationToken,
) -> Result<StructuralCandidates, WorkspaceRetrievalError> {
    let Some(provider) = provider else {
        return Ok(StructuralCandidates::unavailable(
            WorkspaceHybridFallbackReason::Unavailable,
        ));
    };
    let status = provider.status();
    if status.state == CodeIntelligenceState::Unavailable || !status.capabilities.workspace_symbols
    {
        return Ok(StructuralCandidates::unavailable(match status.state {
            CodeIntelligenceState::Starting => WorkspaceHybridFallbackReason::Building,
            CodeIntelligenceState::Degraded => WorkspaceHybridFallbackReason::Degraded,
            CodeIntelligenceState::Ready | CodeIntelligenceState::Unavailable => {
                WorkspaceHybridFallbackReason::Unavailable
            }
        }));
    }
    let fallback = match status.state {
        CodeIntelligenceState::Starting => Some(WorkspaceHybridFallbackReason::Building),
        CodeIntelligenceState::Degraded => Some(WorkspaceHybridFallbackReason::Degraded),
        CodeIntelligenceState::Unavailable => Some(WorkspaceHybridFallbackReason::Unavailable),
        CodeIntelligenceState::Ready => None,
    };
    let result = match provider
        .search_symbols(&request.query, STRUCTURAL_FETCH_LIMIT, cancellation.clone())
        .await
    {
        Ok(result) => result,
        Err(crate::code_intelligence::CodeIntelligenceError::Cancelled)
            if cancellation.is_cancelled() =>
        {
            return Err(WorkspaceRetrievalError::Cancelled)
        }
        Err(_) => {
            return Ok(StructuralCandidates::unavailable(
                WorkspaceHybridFallbackReason::StructuralQueryFailed,
            ))
        }
    };
    let identifier_query = is_ascii_identifier(&request.query);
    let mut candidates = Vec::new();
    for (rank, symbol) in result.items.iter().enumerate() {
        if !request.matches(symbol.location.path.as_str()) {
            continue;
        }
        let Some(chunk) = chunk_for_symbol(snapshot, symbol) else {
            continue;
        };
        candidates.push(StructuralCandidate {
            chunk,
            rank: rank + 1,
            exact_identifier: identifier_query && symbol.name == request.query,
        });
    }
    let truncated = result.truncated || candidates.len() > CHANNEL_CANDIDATE_LIMIT;
    candidates.truncate(CHANNEL_CANDIDATE_LIMIT);
    Ok(StructuralCandidates {
        truncated,
        candidates,
        fallback,
    })
}

fn chunk_for_symbol(
    snapshot: &ChunkCatalogSnapshot,
    symbol: &SymbolInformation,
) -> Option<Arc<WorkspaceChunk>> {
    let line = symbol.location.range.start.line as usize + 1;
    snapshot
        .chunks()
        .iter()
        .find(|chunk| {
            chunk.path.as_ref() == symbol.location.path.as_str()
                && (chunk.start_line..=chunk.end_line).contains(&line)
        })
        .cloned()
}

fn path_matches(path: &str, base: Option<&WorkspacePath>) -> bool {
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
