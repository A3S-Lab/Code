//! Bounded deterministic second-stage ranking for fused workspace evidence.

use super::hybrid_rank::compare_fused;
use super::{
    WorkspaceHybridSearchHit, WorkspaceRerankAlgorithm, WorkspaceRerankFallbackReason,
    WorkspaceRerankMode, WorkspaceRerankOptions, WorkspaceRerankStatus, WorkspaceRetrievalError,
    WorkspaceRetrievalResult,
};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

mod features;
use features::{candidate_features, scratch_bytes, CandidateFeatures};

const MAX_CANDIDATES: usize = 100;
const MAX_FEATURE_BYTES_PER_CANDIDATE: usize = 4 * 1024;
const MAX_FINGERPRINTS_PER_CANDIDATE: usize = 128;
const MAX_SCRATCH_BYTES: usize = 4 * 1024 * 1024;
const MAX_RESULTS_PER_FILE: usize = 2;
const NEAR_DUPLICATE_THRESHOLD: f64 = 0.85;
const RELEVANCE_WEIGHT: f64 = 0.70;
const CHANNEL_AGREEMENT_WEIGHT: f64 = 0.10;
const DIVERSITY_WEIGHT: f64 = 0.20;
const NEAR_DUPLICATE_PENALTY: f64 = 0.50;
const MAX_CHANNELS: usize = 4;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

pub(super) struct RerankOutcome {
    pub hits: Vec<WorkspaceHybridSearchHit>,
    pub status: WorkspaceRerankStatus,
}

pub(super) fn rerank_status_not_run(requested_mode: WorkspaceRerankMode) -> WorkspaceRerankStatus {
    WorkspaceRerankStatus {
        requested_mode,
        applied_mode: WorkspaceRerankMode::RrfOnly,
        algorithm: WorkspaceRerankAlgorithm::RrfK60,
        input_candidates: 0,
        evaluated_candidates: 0,
        selected_candidates: 0,
        near_duplicate_candidates: 0,
        selected_near_duplicates: 0,
        feature_bytes: 0,
        accounted_scratch_bytes: 0,
        candidate_truncated: false,
        fallback: None,
    }
}

impl WorkspaceRerankOptions {
    pub(crate) fn validate(self) -> WorkspaceRetrievalResult<Self> {
        validate_range(
            self.max_candidates,
            1,
            MAX_CANDIDATES,
            "rerank.max_candidates",
        )?;
        validate_range(
            self.max_feature_bytes_per_candidate,
            4,
            MAX_FEATURE_BYTES_PER_CANDIDATE,
            "rerank.max_feature_bytes_per_candidate",
        )?;
        validate_range(
            self.max_fingerprints_per_candidate,
            1,
            MAX_FINGERPRINTS_PER_CANDIDATE,
            "rerank.max_fingerprints_per_candidate",
        )?;
        validate_range(
            self.max_scratch_bytes,
            1,
            MAX_SCRATCH_BYTES,
            "rerank.max_scratch_bytes",
        )?;
        Ok(self)
    }
}

fn validate_range(
    value: usize,
    minimum: usize,
    maximum: usize,
    field: &'static str,
) -> WorkspaceRetrievalResult<()> {
    if !(minimum..=maximum).contains(&value) {
        return Err(WorkspaceRetrievalError::InvalidConfiguration {
            field,
            reason: "is outside the supported bounded range",
        });
    }
    Ok(())
}

pub(super) fn rerank_fused_candidates(
    mut candidates: Vec<WorkspaceHybridSearchHit>,
    limit: usize,
    options: WorkspaceRerankOptions,
) -> RerankOutcome {
    candidates.sort_by(compare_fused);
    let input_candidates = candidates.len();
    if options.mode == WorkspaceRerankMode::RrfOnly {
        return rrf_only(
            candidates,
            limit,
            input_candidates,
            options.mode,
            0,
            false,
            None,
        );
    }
    if options.validate().is_err() {
        return rrf_only(
            candidates,
            limit,
            input_candidates,
            options.mode,
            0,
            false,
            Some(WorkspaceRerankFallbackReason::InvalidConfiguration),
        );
    }

    let candidate_truncated = candidates.len() > options.max_candidates;
    let evaluated_candidates = candidates.len().min(options.max_candidates);
    let accounted_scratch_bytes = scratch_bytes(evaluated_candidates, options);
    if accounted_scratch_bytes > options.max_scratch_bytes {
        return rrf_only(
            candidates,
            limit,
            input_candidates,
            options.mode,
            accounted_scratch_bytes,
            candidate_truncated,
            Some(WorkspaceRerankFallbackReason::ScratchBudgetExceeded),
        );
    }
    candidates = bounded_candidate_pool(candidates, options.max_candidates);

    let features = candidates
        .iter()
        .map(|candidate| candidate_features(&candidate.chunk.text, options))
        .collect::<Vec<_>>();
    let feature_bytes = features.iter().fold(0usize, |total, features| {
        total.saturating_add(features.feature_bytes)
    });
    let near_duplicate_candidates = count_near_duplicates(&candidates, &features);
    let (hits, selected_near_duplicates) = select_mmr(candidates, &features, limit);
    let selected_candidates = hits.len();

    RerankOutcome {
        hits,
        status: WorkspaceRerankStatus {
            requested_mode: options.mode,
            applied_mode: WorkspaceRerankMode::Deterministic,
            algorithm: WorkspaceRerankAlgorithm::RrfK60DeterministicMmrV1,
            input_candidates,
            evaluated_candidates: features.len(),
            selected_candidates,
            near_duplicate_candidates,
            selected_near_duplicates,
            feature_bytes,
            accounted_scratch_bytes,
            candidate_truncated,
            fallback: None,
        },
    }
}

fn bounded_candidate_pool(
    candidates: Vec<WorkspaceHybridSearchHit>,
    maximum: usize,
) -> Vec<WorkspaceHybridSearchHit> {
    if candidates.len() <= maximum {
        return candidates;
    }

    let mut selected = Vec::<usize>::with_capacity(maximum);
    for per_file_quota in 1..=MAX_RESULTS_PER_FILE {
        for (index, candidate) in candidates.iter().enumerate() {
            if selected.len() == maximum {
                break;
            }
            if selected.contains(&index) {
                continue;
            }
            let selected_for_file = selected
                .iter()
                .filter(|selected_index| {
                    candidates[**selected_index].chunk.path == candidate.chunk.path
                })
                .count();
            if selected_for_file < per_file_quota {
                selected.push(index);
            }
        }
    }
    for index in 0..candidates.len() {
        if selected.len() == maximum {
            break;
        }
        if !selected.contains(&index) {
            selected.push(index);
        }
    }
    selected.sort_unstable();

    let mut selected = selected.into_iter().peekable();
    candidates
        .into_iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            if selected.next_if_eq(&index).is_some() {
                Some(candidate)
            } else {
                None
            }
        })
        .collect()
}

fn rrf_only(
    candidates: Vec<WorkspaceHybridSearchHit>,
    limit: usize,
    input_candidates: usize,
    requested_mode: WorkspaceRerankMode,
    accounted_scratch_bytes: usize,
    candidate_truncated: bool,
    fallback: Option<WorkspaceRerankFallbackReason>,
) -> RerankOutcome {
    let mut per_file = HashMap::<Arc<str>, usize>::new();
    let hits = candidates
        .into_iter()
        .filter_map(|mut candidate| {
            let count = per_file
                .entry(Arc::clone(&candidate.chunk.path))
                .or_default();
            if *count >= MAX_RESULTS_PER_FILE {
                return None;
            }
            *count += 1;
            candidate.rerank_score = candidate.fused_score;
            candidate.redundancy_score = 0.0;
            Some(candidate)
        })
        .take(limit)
        .collect::<Vec<_>>();
    let selected_candidates = hits.len();
    RerankOutcome {
        hits,
        status: WorkspaceRerankStatus {
            requested_mode,
            applied_mode: WorkspaceRerankMode::RrfOnly,
            algorithm: WorkspaceRerankAlgorithm::RrfK60,
            input_candidates,
            evaluated_candidates: 0,
            selected_candidates,
            near_duplicate_candidates: 0,
            selected_near_duplicates: 0,
            feature_bytes: 0,
            accounted_scratch_bytes,
            candidate_truncated,
            fallback,
        },
    }
}

fn select_mmr(
    candidates: Vec<WorkspaceHybridSearchHit>,
    features: &[CandidateFeatures],
    limit: usize,
) -> (Vec<WorkspaceHybridSearchHit>, usize) {
    let exact_max = maximum_fused_score(&candidates, true);
    let non_exact_max = maximum_fused_score(&candidates, false);
    let mut selected = Vec::<usize>::with_capacity(limit.min(candidates.len()));
    let mut selected_scores = Vec::<(f64, f64)>::with_capacity(selected.capacity());
    let mut selected_flags = vec![false; candidates.len()];
    let mut per_file = HashMap::<Arc<str>, usize>::new();
    let mut selected_near_duplicates = 0usize;

    while selected.len() < limit {
        let select_exact = candidates.iter().enumerate().any(|(index, candidate)| {
            !selected_flags[index]
                && candidate.exact_identifier
                && per_file.get(&candidate.chunk.path).copied().unwrap_or(0) < MAX_RESULTS_PER_FILE
        });
        let maximum = if select_exact {
            exact_max
        } else {
            non_exact_max
        };
        let mut best = None::<(usize, f64, f64)>;
        for (index, candidate) in candidates.iter().enumerate() {
            if selected_flags[index]
                || candidate.exact_identifier != select_exact
                || per_file.get(&candidate.chunk.path).copied().unwrap_or(0) >= MAX_RESULTS_PER_FILE
            {
                continue;
            }
            let redundancy = selected.iter().fold(0.0_f64, |maximum, selected_index| {
                maximum.max(candidate_similarity(
                    candidate,
                    &features[index],
                    &candidates[*selected_index],
                    &features[*selected_index],
                ))
            });
            let score = selection_score(candidate, maximum, redundancy);
            let replace = best.is_none_or(|(best_index, best_score, _)| {
                score
                    .total_cmp(&best_score)
                    .then_with(|| best_index.cmp(&index))
                    == Ordering::Greater
            });
            if replace {
                best = Some((index, score, redundancy));
            }
        }
        let Some((index, score, redundancy)) = best else {
            break;
        };
        selected_flags[index] = true;
        *per_file
            .entry(Arc::clone(&candidates[index].chunk.path))
            .or_default() += 1;
        if redundancy >= NEAR_DUPLICATE_THRESHOLD {
            selected_near_duplicates = selected_near_duplicates.saturating_add(1);
        }
        selected.push(index);
        selected_scores.push((score, redundancy));
    }

    let hits = selected
        .into_iter()
        .zip(selected_scores)
        .map(|(index, (score, redundancy))| {
            let mut candidate = candidates[index].clone();
            candidate.rerank_score = score;
            candidate.redundancy_score = redundancy;
            candidate
        })
        .collect();
    (hits, selected_near_duplicates)
}

fn maximum_fused_score(candidates: &[WorkspaceHybridSearchHit], exact: bool) -> f64 {
    candidates
        .iter()
        .filter(|candidate| candidate.exact_identifier == exact)
        .map(|candidate| candidate.fused_score)
        .filter(|score| score.is_finite() && *score > 0.0)
        .max_by(f64::total_cmp)
        .unwrap_or(1.0)
}

fn selection_score(
    candidate: &WorkspaceHybridSearchHit,
    maximum_fused_score: f64,
    redundancy: f64,
) -> f64 {
    let relevance = (candidate.fused_score / maximum_fused_score).clamp(0.0, 1.0);
    let agreement = (candidate.channels.len() as f64 / MAX_CHANNELS as f64).clamp(0.0, 1.0);
    let duplicate_penalty = if redundancy >= NEAR_DUPLICATE_THRESHOLD {
        NEAR_DUPLICATE_PENALTY * redundancy
    } else {
        0.0
    };
    RELEVANCE_WEIGHT * relevance
        + CHANNEL_AGREEMENT_WEIGHT * agreement
        + DIVERSITY_WEIGHT * (1.0 - redundancy)
        - duplicate_penalty
}

fn count_near_duplicates(
    candidates: &[WorkspaceHybridSearchHit],
    features: &[CandidateFeatures],
) -> usize {
    candidates
        .iter()
        .enumerate()
        .filter(|(index, candidate)| {
            (0..*index).any(|prior| {
                candidate_similarity(
                    candidate,
                    &features[*index],
                    &candidates[prior],
                    &features[prior],
                ) >= NEAR_DUPLICATE_THRESHOLD
            })
        })
        .count()
}

fn candidate_similarity(
    left: &WorkspaceHybridSearchHit,
    left_features: &CandidateFeatures,
    right: &WorkspaceHybridSearchHit,
    right_features: &CandidateFeatures,
) -> f64 {
    interval_overlap(left, right).max(fingerprint_jaccard(
        &left_features.fingerprints,
        &right_features.fingerprints,
    ))
}

fn interval_overlap(left: &WorkspaceHybridSearchHit, right: &WorkspaceHybridSearchHit) -> f64 {
    if left.chunk.path != right.chunk.path {
        return 0.0;
    }
    let intersection = left
        .chunk
        .end_byte
        .min(right.chunk.end_byte)
        .saturating_sub(left.chunk.start_byte.max(right.chunk.start_byte));
    let denominator = left
        .chunk
        .end_byte
        .saturating_sub(left.chunk.start_byte)
        .min(right.chunk.end_byte.saturating_sub(right.chunk.start_byte));
    if denominator == 0 {
        0.0
    } else {
        intersection as f64 / denominator as f64
    }
}

fn fingerprint_jaccard(left: &[u64], right: &[u64]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let mut left_index = 0usize;
    let mut right_index = 0usize;
    let mut intersection = 0usize;
    while left_index < left.len() && right_index < right.len() {
        match left[left_index].cmp(&right[right_index]) {
            Ordering::Less => left_index += 1,
            Ordering::Greater => right_index += 1,
            Ordering::Equal => {
                intersection += 1;
                left_index += 1;
                right_index += 1;
            }
        }
    }
    let union = left
        .len()
        .saturating_add(right.len())
        .saturating_sub(intersection);
    intersection as f64 / union as f64
}
