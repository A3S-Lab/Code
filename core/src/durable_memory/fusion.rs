use super::context::RecallCandidate;
use super::semantic::SemanticRecallCandidate;
use super::DurableMemoryRecallChannel;
use std::collections::BTreeMap;

const RRF_K: f32 = 60.0;

pub(super) fn fuse_lexical_semantic(
    lexical: Vec<RecallCandidate>,
    semantic: Vec<SemanticRecallCandidate>,
) -> Vec<RecallCandidate> {
    if semantic.is_empty() {
        return lexical;
    }
    if lexical.is_empty() {
        return semantic
            .into_iter()
            .map(|candidate| RecallCandidate {
                node: candidate.node,
                score: candidate.score,
                channel: DurableMemoryRecallChannel::Semantic,
                related_from: None,
            })
            .collect();
    }

    struct FusionEntry {
        candidate: RecallCandidate,
        reciprocal_rank: f32,
        lexical: bool,
        semantic: bool,
    }

    let mut entries = BTreeMap::<String, FusionEntry>::new();
    for (index, candidate) in lexical.into_iter().enumerate() {
        let contribution = reciprocal_rank(index);
        entries.insert(
            candidate.node.id.clone(),
            FusionEntry {
                candidate,
                reciprocal_rank: contribution,
                lexical: true,
                semantic: false,
            },
        );
    }
    for (index, semantic) in semantic.into_iter().enumerate() {
        let contribution = reciprocal_rank(index);
        match entries.get_mut(&semantic.node.id) {
            Some(entry) => {
                entry.reciprocal_rank += contribution;
                entry.semantic = true;
                // Semantic candidates were re-read from the repository after
                // lexical search, so prefer that later verified revision.
                entry.candidate.node = semantic.node;
            }
            None => {
                entries.insert(
                    semantic.node.id.clone(),
                    FusionEntry {
                        candidate: RecallCandidate {
                            node: semantic.node,
                            score: semantic.score,
                            channel: DurableMemoryRecallChannel::Semantic,
                            related_from: None,
                        },
                        reciprocal_rank: contribution,
                        lexical: false,
                        semantic: true,
                    },
                );
            }
        }
    }
    let maximum = entries
        .values()
        .map(|entry| entry.reciprocal_rank)
        .max_by(f32::total_cmp)
        .unwrap_or(1.0);
    let mut fused = entries
        .into_values()
        .map(|entry| {
            let mut candidate = entry.candidate;
            candidate.score = (entry.reciprocal_rank / maximum).clamp(0.0, 1.0);
            candidate.channel = if entry.lexical && entry.semantic {
                DurableMemoryRecallChannel::Hybrid
            } else if entry.lexical {
                DurableMemoryRecallChannel::Lexical
            } else {
                DurableMemoryRecallChannel::Semantic
            };
            candidate
        })
        .collect::<Vec<_>>();
    fused.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| {
                super::context::channel_rank(left.channel)
                    .cmp(&super::context::channel_rank(right.channel))
            })
            .then_with(|| right.node.updated_at.cmp(&left.node.updated_at))
            .then_with(|| left.node.id.cmp(&right.node.id))
    });
    fused
}

fn reciprocal_rank(index: usize) -> f32 {
    1.0 / (RRF_K + index as f32 + 1.0)
}
