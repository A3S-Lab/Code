use super::{
    WorkspaceChunk, WorkspaceHybridChannelRank, WorkspaceHybridSearchHit, WorkspaceRetrievalChannel,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub(super) const RRF_K: usize = 60;
pub(super) const MAX_RESULTS_PER_FILE: usize = 2;

pub(super) struct RankedCandidate {
    pub chunk: Arc<WorkspaceChunk>,
    pub channel: WorkspaceRetrievalChannel,
    pub rank: usize,
    pub exact_identifier: bool,
}

struct Accumulator {
    chunk: Arc<WorkspaceChunk>,
    fused_score: f64,
    exact_identifier: bool,
    ranks: BTreeMap<WorkspaceRetrievalChannel, usize>,
}

pub(super) fn fuse_candidates(
    candidates: Vec<RankedCandidate>,
    limit: usize,
) -> Vec<WorkspaceHybridSearchHit> {
    let mut fused = HashMap::<String, Accumulator>::new();
    for candidate in candidates {
        if candidate.rank == 0 {
            continue;
        }
        let entry = fused
            .entry(candidate.chunk.id.as_str().to_owned())
            .or_insert_with(|| Accumulator {
                chunk: Arc::clone(&candidate.chunk),
                fused_score: 0.0,
                exact_identifier: false,
                ranks: BTreeMap::new(),
            });
        if entry.ranks.contains_key(&candidate.channel) {
            continue;
        }
        entry.fused_score += 1.0 / (RRF_K.saturating_add(candidate.rank) as f64);
        entry.exact_identifier |= candidate.exact_identifier;
        entry.ranks.insert(candidate.channel, candidate.rank);
    }

    let mut fused = fused
        .into_values()
        .map(|candidate| WorkspaceHybridSearchHit {
            chunk: candidate.chunk,
            fused_score: candidate.fused_score,
            exact_identifier: candidate.exact_identifier,
            channels: candidate
                .ranks
                .into_iter()
                .map(|(channel, rank)| WorkspaceHybridChannelRank { channel, rank })
                .collect(),
        })
        .collect::<Vec<_>>();
    fused.sort_by(|left, right| {
        right
            .exact_identifier
            .cmp(&left.exact_identifier)
            .then_with(|| right.fused_score.total_cmp(&left.fused_score))
            .then_with(|| left.chunk.path.cmp(&right.chunk.path))
            .then_with(|| left.chunk.start_byte.cmp(&right.chunk.start_byte))
            .then_with(|| left.chunk.id.cmp(&right.chunk.id))
    });

    let mut per_file = HashMap::<Arc<str>, usize>::new();
    fused
        .into_iter()
        .filter(|candidate| {
            let count = per_file
                .entry(Arc::clone(&candidate.chunk.path))
                .or_default();
            if *count >= MAX_RESULTS_PER_FILE {
                return false;
            }
            *count += 1;
            true
        })
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::retrieval::{ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkCatalog};
    use crate::workspace::WorkspacePath;

    #[test]
    fn rrf_uses_channel_ranks_and_deduplicates_each_channel() {
        let chunks = chunks(&[("a.rs", "alpha\n"), ("b.rs", "beta\n")]);
        let hits = fuse_candidates(
            vec![
                candidate(&chunks[0], WorkspaceRetrievalChannel::Lexical, 1, false),
                candidate(&chunks[0], WorkspaceRetrievalChannel::Lexical, 2, false),
                candidate(&chunks[0], WorkspaceRetrievalChannel::Semantic, 2, false),
                candidate(&chunks[1], WorkspaceRetrievalChannel::Semantic, 1, false),
            ],
            10,
        );

        assert_eq!(hits[0].chunk.path.as_ref(), "a.rs");
        assert_eq!(hits[0].channels.len(), 2);
        assert_eq!(
            hits[0].channels[0].channel,
            WorkspaceRetrievalChannel::Lexical
        );
        assert_eq!(hits[0].channels[0].rank, 1);
    }

    #[test]
    fn exact_identifier_tier_cannot_be_displaced_by_semantic_only_hits() {
        let chunks = chunks(&[("exact.rs", "ExactType\n"), ("semantic.rs", "concept\n")]);
        let hits = fuse_candidates(
            vec![
                candidate(&chunks[0], WorkspaceRetrievalChannel::Exact, 25, true),
                candidate(&chunks[1], WorkspaceRetrievalChannel::Semantic, 1, false),
                candidate(&chunks[1], WorkspaceRetrievalChannel::Lexical, 1, false),
                candidate(&chunks[1], WorkspaceRetrievalChannel::Structural, 1, false),
            ],
            10,
        );

        assert_eq!(hits[0].chunk.path.as_ref(), "exact.rs");
        assert!(hits[0].exact_identifier);
    }

    #[test]
    fn fusion_is_deterministic_and_limits_per_file_diversity() {
        let catalog = WorkspaceChunkCatalog::new(
            ChunkingConfig {
                max_lines: 1,
                ..ChunkingConfig::default()
            },
            ChunkCatalogLimits::default(),
        )
        .unwrap();
        catalog
            .replace_file(
                &WorkspacePath::from_normalized("a.rs"),
                None,
                1,
                "one\ntwo\nthree\n",
            )
            .unwrap();
        catalog
            .replace_file(&WorkspacePath::from_normalized("b.rs"), None, 2, "other\n")
            .unwrap();
        let chunks = catalog.snapshot().unwrap().chunks().to_vec();
        let a_chunks = chunks
            .iter()
            .filter(|chunk| chunk.path.as_ref() == "a.rs")
            .collect::<Vec<_>>();
        let b_chunk = chunks
            .iter()
            .find(|chunk| chunk.path.as_ref() == "b.rs")
            .unwrap();
        let candidates = vec![
            candidate(a_chunks[0], WorkspaceRetrievalChannel::Semantic, 1, false),
            candidate(a_chunks[1], WorkspaceRetrievalChannel::Semantic, 2, false),
            candidate(a_chunks[2], WorkspaceRetrievalChannel::Semantic, 3, false),
            candidate(b_chunk, WorkspaceRetrievalChannel::Semantic, 4, false),
        ];
        let first = fuse_candidates(candidates, 10);
        assert_eq!(
            first
                .iter()
                .filter(|hit| hit.chunk.path.as_ref() == "a.rs")
                .count(),
            MAX_RESULTS_PER_FILE
        );
        assert_eq!(first[0].chunk.path.as_ref(), "a.rs");
        assert_eq!(first.last().unwrap().chunk.path.as_ref(), "b.rs");
    }

    fn candidate(
        chunk: &Arc<WorkspaceChunk>,
        channel: WorkspaceRetrievalChannel,
        rank: usize,
        exact_identifier: bool,
    ) -> RankedCandidate {
        RankedCandidate {
            chunk: Arc::clone(chunk),
            channel,
            rank,
            exact_identifier,
        }
    }

    fn chunks(files: &[(&str, &str)]) -> Vec<Arc<WorkspaceChunk>> {
        let catalog =
            WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
                .unwrap();
        for (revision, (path, content)) in files.iter().enumerate() {
            catalog
                .replace_file(
                    &WorkspacePath::from_normalized(*path),
                    None,
                    revision as u64 + 1,
                    content,
                )
                .unwrap();
        }
        catalog.snapshot().unwrap().chunks().to_vec()
    }
}
