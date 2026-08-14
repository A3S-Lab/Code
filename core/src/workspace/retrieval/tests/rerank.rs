use super::super::rerank::rerank_fused_candidates;
use super::super::{
    WorkspaceChunk, WorkspaceChunkId, WorkspaceHybridChannelRank, WorkspaceHybridSearchHit,
    WorkspaceRerankFallbackReason, WorkspaceRerankMode, WorkspaceRerankOptions,
    WorkspaceRetrievalChannel,
};
use std::sync::Arc;

#[test]
fn deterministic_rerank_defers_overlapping_ranges_and_cross_file_boilerplate() {
    let overlap = hit(
        chunk(
            "src/a.rs",
            0,
            100,
            "shared retry boilerplate reconnect delivery guard alpha",
            "a-1",
        ),
        0.060,
        false,
    );
    let overlapping_copy = hit(
        chunk(
            "src/a.rs",
            5,
            105,
            "shared retry boilerplate reconnect delivery guard alpha",
            "a-2",
        ),
        0.059,
        false,
    );
    let cross_file_copy = hit(
        chunk(
            "src/b.rs",
            0,
            100,
            "shared retry boilerplate reconnect delivery guard alpha",
            "b-1",
        ),
        0.058,
        false,
    );
    let diverse = hit(
        chunk(
            "src/c.rs",
            0,
            80,
            "generation fence cancels superseded vector publication",
            "c-1",
        ),
        0.040,
        false,
    );

    let outcome = rerank_fused_candidates(
        vec![overlap, overlapping_copy, cross_file_copy, diverse],
        4,
        WorkspaceRerankOptions::deterministic(),
    );
    let ids = ids(&outcome.hits);

    assert_eq!(ids[0], "a-1");
    assert_eq!(ids[1], "c-1");
    assert_eq!(&ids[2..], ["a-2", "b-1"]);
    assert_eq!(
        outcome.status.requested_mode,
        WorkspaceRerankMode::Deterministic
    );
    assert_eq!(
        outcome.status.applied_mode,
        WorkspaceRerankMode::Deterministic
    );
    assert_eq!(outcome.status.near_duplicate_candidates, 2);
    assert_eq!(outcome.status.selected_near_duplicates, 2);
    assert!(outcome.status.feature_bytes > 0);
    assert!(outcome.status.accounted_scratch_bytes > 0);
    assert!(outcome.status.fallback.is_none());
}

#[test]
fn exact_identifier_tier_remains_first_during_rerank() {
    let semantic = hit(
        chunk(
            "src/semantic.rs",
            0,
            64,
            "high semantic concept",
            "semantic",
        ),
        0.070,
        false,
    );
    let exact = hit(
        chunk("src/exact.rs", 0, 64, "ExactWorkspaceType", "exact"),
        0.010,
        true,
    );

    let outcome = rerank_fused_candidates(
        vec![semantic, exact],
        2,
        WorkspaceRerankOptions::deterministic(),
    );

    assert_eq!(ids(&outcome.hits), ["exact", "semantic"]);
    assert!(outcome.hits[0].exact_identifier);
}

#[test]
fn rerank_is_deterministic_and_bounds_its_candidate_pool() {
    let candidates = vec![
        hit(chunk("a.rs", 0, 20, "alpha one", "a"), 0.050, false),
        hit(chunk("b.rs", 0, 20, "beta two", "b"), 0.040, false),
        hit(chunk("c.rs", 0, 20, "gamma three", "c"), 0.030, false),
        hit(chunk("d.rs", 0, 20, "delta four", "d"), 0.020, false),
    ];
    let mut options = WorkspaceRerankOptions::deterministic();
    options.max_candidates = 3;
    let first = rerank_fused_candidates(candidates.clone(), 4, options);
    let second = rerank_fused_candidates(candidates, 4, options);

    assert_eq!(ids(&first.hits), ids(&second.hits));
    assert_eq!(first.status, second.status);
    assert_eq!(first.status.input_candidates, 4);
    assert_eq!(first.status.evaluated_candidates, 3);
    assert_eq!(first.status.selected_candidates, 3);
    assert!(first.status.candidate_truncated);
}

#[test]
fn bounded_pool_cannot_be_monopolized_by_one_file() {
    let mut candidates = (0..10)
        .map(|index| {
            hit(
                chunk(
                    "dominant.rs",
                    index * 20,
                    index * 20 + 20,
                    &format!("dominant evidence {index}"),
                    &format!("dominant-{index}"),
                ),
                0.100 - index as f64 * 0.001,
                false,
            )
        })
        .collect::<Vec<_>>();
    candidates.push(hit(
        chunk("diverse.rs", 0, 20, "independent evidence", "diverse"),
        0.010,
        false,
    ));
    let mut options = WorkspaceRerankOptions::deterministic();
    options.max_candidates = 4;

    let outcome = rerank_fused_candidates(candidates, 3, options);

    assert_eq!(outcome.status.input_candidates, 11);
    assert_eq!(outcome.status.evaluated_candidates, 4);
    assert!(outcome.status.candidate_truncated);
    assert!(
        outcome
            .hits
            .iter()
            .any(|hit| hit.chunk.path.as_ref() == "diverse.rs"),
        "a lower-ranked file must survive bounded candidate admission"
    );
}

#[test]
fn scratch_budget_failure_returns_the_original_rrf_order() {
    let candidates = vec![
        hit(chunk("a.rs", 0, 20, "same boilerplate", "a"), 0.050, false),
        hit(chunk("b.rs", 0, 20, "same boilerplate", "b"), 0.040, false),
        hit(
            chunk("c.rs", 0, 20, "different evidence", "c"),
            0.030,
            false,
        ),
    ];
    let baseline =
        rerank_fused_candidates(candidates.clone(), 3, WorkspaceRerankOptions::default());
    let mut constrained = WorkspaceRerankOptions::deterministic();
    constrained.max_scratch_bytes = 1;
    let fallback = rerank_fused_candidates(candidates, 3, constrained);

    assert_eq!(ids(&fallback.hits), ids(&baseline.hits));
    assert_eq!(
        fallback.status.requested_mode,
        WorkspaceRerankMode::Deterministic
    );
    assert_eq!(fallback.status.applied_mode, WorkspaceRerankMode::RrfOnly);
    assert_eq!(
        fallback.status.fallback,
        Some(WorkspaceRerankFallbackReason::ScratchBudgetExceeded)
    );
    assert_eq!(fallback.status.evaluated_candidates, 0);
    assert!(
        fallback.status.accounted_scratch_bytes > constrained.max_scratch_bytes,
        "fallback must report the attempted bounded allocation"
    );
    assert!(!fallback.status.candidate_truncated);
}

#[test]
fn rerank_validation_rejects_every_hard_bound() {
    let invalid = [
        WorkspaceRerankOptions::deterministic().with_max_candidates(0),
        WorkspaceRerankOptions::deterministic().with_max_candidates(101),
        WorkspaceRerankOptions::deterministic().with_max_feature_bytes_per_candidate(3),
        WorkspaceRerankOptions::deterministic().with_max_feature_bytes_per_candidate(4_097),
        WorkspaceRerankOptions::deterministic().with_max_fingerprints_per_candidate(0),
        WorkspaceRerankOptions::deterministic().with_max_fingerprints_per_candidate(129),
        WorkspaceRerankOptions::deterministic().with_max_scratch_bytes(0),
        WorkspaceRerankOptions::deterministic().with_max_scratch_bytes(4 * 1024 * 1024 + 1),
    ];

    for options in invalid {
        assert!(
            options.validate().is_err(),
            "options must be rejected: {options:?}"
        );
    }

    WorkspaceRerankOptions::deterministic()
        .with_max_candidates(1)
        .with_max_feature_bytes_per_candidate(4)
        .with_max_fingerprints_per_candidate(1)
        .with_max_scratch_bytes(1)
        .validate()
        .expect("inclusive lower bounds");
}

fn chunk(
    path: &str,
    start_byte: usize,
    end_byte: usize,
    text: &str,
    id: &str,
) -> Arc<WorkspaceChunk> {
    Arc::new(WorkspaceChunk {
        id: WorkspaceChunkId::new(id.to_owned()),
        path: Arc::from(path),
        language: Some(Arc::from("rust")),
        start_line: 1,
        end_line: 1,
        start_byte,
        end_byte,
        content_digest: Arc::from("sha256:fixture"),
        source_revision: 1,
        text: Arc::from(text),
    })
}

fn hit(
    chunk: Arc<WorkspaceChunk>,
    fused_score: f64,
    exact_identifier: bool,
) -> WorkspaceHybridSearchHit {
    WorkspaceHybridSearchHit {
        chunk,
        fused_score,
        rerank_score: fused_score,
        redundancy_score: 0.0,
        exact_identifier,
        channels: vec![WorkspaceHybridChannelRank {
            channel: WorkspaceRetrievalChannel::Lexical,
            rank: 1,
        }],
    }
}

fn ids(hits: &[WorkspaceHybridSearchHit]) -> Vec<&str> {
    hits.iter().map(|hit| hit.chunk.id.as_str()).collect()
}
