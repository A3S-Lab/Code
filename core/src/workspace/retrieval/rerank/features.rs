//! Bounded lexical features and conservative scratch accounting.

use super::super::{WorkspaceHybridChannelRank, WorkspaceHybridSearchHit, WorkspaceRerankOptions};
use super::{FNV_OFFSET, FNV_PRIME, MAX_CHANNELS};
use std::mem::size_of;

pub(super) struct CandidateFeatures {
    pub fingerprints: Vec<u64>,
    pub feature_bytes: usize,
}

pub(super) fn candidate_features(text: &str, options: WorkspaceRerankOptions) -> CandidateFeatures {
    let mut fingerprints = Vec::with_capacity(options.max_fingerprints_per_candidate);
    let mut feature_bytes = 0usize;
    for segment in sample_segments(text, options.max_feature_bytes_per_candidate) {
        feature_bytes = feature_bytes.saturating_add(segment.len());
        fingerprint_segment(
            segment,
            &mut fingerprints,
            options.max_fingerprints_per_candidate,
        );
    }
    CandidateFeatures {
        fingerprints,
        feature_bytes,
    }
}

fn sample_segments(text: &str, max_bytes: usize) -> Vec<&str> {
    if text.len() <= max_bytes {
        return vec![text];
    }
    let head_target = max_bytes / 2;
    let mut head_end = head_target;
    while head_end > 0 && !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let tail_target = max_bytes.saturating_sub(head_end);
    let mut tail_start = text.len().saturating_sub(tail_target);
    while tail_start < text.len() && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    vec![&text[..head_end], &text[tail_start..]]
}

fn fingerprint_segment(segment: &str, fingerprints: &mut Vec<u64>, maximum: usize) {
    let mut ascii_token = Vec::<u8>::with_capacity(32);
    let mut previous = None;
    for character in segment.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            ascii_token.push(character.to_ascii_lowercase() as u8);
            continue;
        }
        flush_ascii_token(&mut ascii_token, &mut previous, fingerprints, maximum);
        if character.is_alphanumeric() {
            let mut encoded = [0u8; 4];
            let normalized = character.to_lowercase().next().unwrap_or(character);
            emit_token(
                stable_hash(normalized.encode_utf8(&mut encoded).as_bytes()),
                &mut previous,
                fingerprints,
                maximum,
            );
        }
    }
    flush_ascii_token(&mut ascii_token, &mut previous, fingerprints, maximum);
}

fn flush_ascii_token(
    token: &mut Vec<u8>,
    previous: &mut Option<u64>,
    fingerprints: &mut Vec<u64>,
    maximum: usize,
) {
    if token.is_empty() {
        return;
    }
    emit_token(stable_hash(token), previous, fingerprints, maximum);
    token.clear();
}

fn emit_token(token: u64, previous: &mut Option<u64>, fingerprints: &mut Vec<u64>, maximum: usize) {
    insert_bottom_k(fingerprints, token, maximum);
    if let Some(previous) = previous {
        insert_bottom_k(
            fingerprints,
            previous.rotate_left(17) ^ token.wrapping_mul(FNV_PRIME),
            maximum,
        );
    }
    *previous = Some(token);
}

fn insert_bottom_k(values: &mut Vec<u64>, value: u64, maximum: usize) {
    match values.binary_search(&value) {
        Ok(_) => {}
        Err(index) if values.len() < maximum => values.insert(index, value),
        Err(index) if index < maximum => {
            values.insert(index, value);
            values.pop();
        }
        Err(_) => {}
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

pub(super) fn scratch_bytes(candidate_count: usize, options: WorkspaceRerankOptions) -> usize {
    let fingerprints = options
        .max_fingerprints_per_candidate
        .saturating_mul(size_of::<u64>());
    let candidate_vectors = size_of::<WorkspaceHybridSearchHit>().saturating_mul(2);
    let cloned_channels = size_of::<WorkspaceHybridChannelRank>().saturating_mul(MAX_CHANNELS);
    let selection_state = size_of::<(f64, f64)>()
        .saturating_add(size_of::<bool>())
        .saturating_add(size_of::<usize>().saturating_mul(4));
    let per_candidate = size_of::<CandidateFeatures>()
        .saturating_add(fingerprints)
        // The bounded-pool and selected-output vectors can coexist with their
        // source allocations while elements are moved or cloned.
        .saturating_add(candidate_vectors)
        .saturating_add(cloned_channels)
        .saturating_add(selection_state)
        // Conservatively cover per-file map buckets, Arc keys, and allocator
        // bookkeeping without depending on HashMap's private representation.
        .saturating_add(128);
    candidate_count
        .saturating_mul(per_candidate)
        // Token normalization is performed one candidate at a time. Account for
        // its largest transient buffer once instead of multiplying it by the
        // whole candidate pool.
        .saturating_add(options.max_feature_bytes_per_candidate)
}
