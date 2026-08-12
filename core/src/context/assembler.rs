//! Budgeted assembly for context items.

use super::{ContextItem, ContextResult};
use std::collections::HashMap;

/// Budget limits for prompt-bound context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub max_items: usize,
    pub max_tokens: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_items: 12,
            max_tokens: 4_000,
        }
    }
}

/// Source-aware limits that prevent one context source from dominating the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextSourcePolicy {
    pub max_items_per_source: Option<usize>,
    pub max_tokens_per_source: Option<usize>,
}

impl Default for ContextSourcePolicy {
    fn default() -> Self {
        Self {
            max_items_per_source: Some(6),
            max_tokens_per_source: Some(2_500),
        }
    }
}

/// Named budget policy for context assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextAssemblyPolicy {
    pub budget: ContextBudget,
    pub source_policy: ContextSourcePolicy,
}

impl ContextAssemblyPolicy {
    pub fn balanced() -> Self {
        Self {
            budget: ContextBudget {
                max_items: 12,
                max_tokens: 4_000,
            },
            source_policy: ContextSourcePolicy {
                max_items_per_source: Some(6),
                max_tokens_per_source: Some(2_500),
            },
        }
    }

    pub fn compact() -> Self {
        Self {
            budget: ContextBudget {
                max_items: 8,
                max_tokens: 2_500,
            },
            source_policy: ContextSourcePolicy {
                max_items_per_source: Some(4),
                max_tokens_per_source: Some(1_200),
            },
        }
    }

    pub fn expansive() -> Self {
        Self {
            budget: ContextBudget {
                max_items: 20,
                max_tokens: 8_000,
            },
            source_policy: ContextSourcePolicy {
                max_items_per_source: Some(8),
                max_tokens_per_source: Some(3_500),
            },
        }
    }
}

impl Default for ContextAssemblyPolicy {
    fn default() -> Self {
        Self::balanced()
    }
}

/// Final context selected for prompt rendering.
#[derive(Debug, Clone, Default)]
pub struct ContextAssembly {
    pub items: Vec<ContextItem>,
    pub total_tokens: usize,
    pub truncated: bool,
}

impl ContextAssembly {
    pub fn to_xml(&self) -> String {
        self.items
            .iter()
            .map(ContextItem::to_xml)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Assembles raw provider results into a ranked, deduplicated, budgeted set.
#[derive(Debug, Clone)]
pub struct ContextAssembler {
    budget: ContextBudget,
    source_policy: ContextSourcePolicy,
}

impl ContextAssembler {
    pub fn new(budget: ContextBudget) -> Self {
        Self::from_policy(ContextAssemblyPolicy {
            budget,
            source_policy: ContextSourcePolicy::default(),
        })
    }

    pub fn from_policy(policy: ContextAssemblyPolicy) -> Self {
        Self {
            budget: policy.budget,
            source_policy: policy.source_policy,
        }
    }

    pub fn with_source_policy(mut self, policy: ContextSourcePolicy) -> Self {
        self.source_policy = policy;
        self
    }

    pub fn with_default_budget() -> Self {
        Self::from_policy(ContextAssemblyPolicy::balanced())
    }

    pub fn assemble(&self, results: &[ContextResult]) -> ContextAssembly {
        let mut deduped: HashMap<String, ContextItem> = HashMap::new();
        let mut source_count = 0usize;

        for result in results {
            for item in &result.items {
                source_count += 1;
                let key = dedupe_key(item);
                match deduped.get(&key) {
                    Some(existing)
                        if ranking_score(existing)
                            .total_cmp(&ranking_score(item))
                            .then_with(|| existing.relevance.total_cmp(&item.relevance))
                            .is_ge() => {}
                    _ => {
                        deduped.insert(key, item.clone());
                    }
                }
            }
        }

        let mut items = deduped.into_values().collect::<Vec<_>>();
        items.sort_by(|a, b| {
            b.is_required()
                .cmp(&a.is_required())
                .then_with(|| ranking_score(b).total_cmp(&ranking_score(a)))
                .then_with(|| b.relevance.total_cmp(&a.relevance))
                .then_with(|| estimated_tokens(a).cmp(&estimated_tokens(b)))
                .then_with(|| a.id.cmp(&b.id))
        });

        let mut selected = Vec::new();
        let mut total_tokens = 0usize;
        let mut truncated = source_count > items.len();
        let mut source_item_counts: HashMap<String, usize> = HashMap::new();
        let mut source_token_counts: HashMap<String, usize> = HashMap::new();

        for item in items {
            if item.is_required() {
                total_tokens = total_tokens.saturating_add(estimated_tokens(&item));
                selected.push(item);
                continue;
            }
            if selected.len() >= self.budget.max_items {
                truncated = true;
                break;
            }

            let item_tokens = estimated_tokens(&item);
            if total_tokens + item_tokens > self.budget.max_tokens {
                truncated = true;
                continue;
            }

            let source_key = source_policy_key(&item);
            if let Some(max_items) = self.source_policy.max_items_per_source {
                let count = source_item_counts.get(&source_key).copied().unwrap_or(0);
                if count >= max_items {
                    truncated = true;
                    continue;
                }
            }
            if let Some(max_tokens) = self.source_policy.max_tokens_per_source {
                let source_tokens = source_token_counts.get(&source_key).copied().unwrap_or(0);
                if source_tokens + item_tokens > max_tokens {
                    truncated = true;
                    continue;
                }
            }

            total_tokens += item_tokens;
            *source_item_counts.entry(source_key.clone()).or_insert(0) += 1;
            *source_token_counts.entry(source_key).or_insert(0) += item_tokens;
            selected.push(item);
        }

        ContextAssembly {
            items: selected,
            total_tokens,
            truncated,
        }
    }
}

fn dedupe_key(item: &ContextItem) -> String {
    item.source.clone().unwrap_or_else(|| item.id.clone())
}

fn source_policy_key(item: &ContextItem) -> String {
    if let Some(provenance) = item.provenance() {
        return format!("provenance:{provenance}");
    }

    if let Some(source) = &item.source {
        let family = source
            .split_once(':')
            .map(|(family, _)| family)
            .unwrap_or(source);
        return format!("source:{family}");
    }

    format!("type:{:?}", item.context_type)
}

fn estimated_tokens(item: &ContextItem) -> usize {
    if item.token_count > 0 {
        item.token_count
    } else {
        item.content.split_whitespace().count().max(1)
    }
}

fn ranking_score(item: &ContextItem) -> f32 {
    item.relevance + item.priority() * 0.25 + item.trust() * 0.15 + item.freshness() * 0.10
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ContextItem, ContextResult, ContextType};

    fn result(provider: &str, items: Vec<ContextItem>) -> ContextResult {
        let mut result = ContextResult::new(provider);
        for item in items {
            result.add_item(item);
        }
        result
    }

    #[test]
    fn balanced_policy_matches_default_budget_and_source_caps() {
        let policy = ContextAssemblyPolicy::balanced();

        assert_eq!(policy.budget, ContextBudget::default());
        assert_eq!(policy.source_policy, ContextSourcePolicy::default());
    }

    #[test]
    fn compact_policy_applies_tighter_caps() {
        let assembler = ContextAssembler::from_policy(ContextAssemblyPolicy::compact());
        let assembly = assembler.assemble(&[result(
            "test",
            (0..10)
                .map(|index| {
                    ContextItem::new(
                        format!("file-{index}"),
                        ContextType::Resource,
                        format!("file {index}"),
                    )
                    .with_source(format!("file://{index}"))
                    .with_relevance(1.0 - index as f32 * 0.01)
                    .with_token_count(1)
                })
                .collect(),
        )]);

        assert_eq!(assembly.items.len(), 4);
        assert!(assembly.truncated);
    }

    #[test]
    fn expansive_policy_allows_broader_context() {
        let assembler = ContextAssembler::from_policy(ContextAssemblyPolicy::expansive());
        let assembly = assembler.assemble(&[result(
            "test",
            (0..8)
                .map(|index| {
                    ContextItem::new(
                        format!("file-{index}"),
                        ContextType::Resource,
                        format!("file {index}"),
                    )
                    .with_source(format!("file://{index}"))
                    .with_relevance(1.0 - index as f32 * 0.01)
                    .with_token_count(1)
                })
                .collect(),
        )]);

        assert_eq!(assembly.items.len(), 8);
        assert!(!assembly.truncated);
    }

    #[test]
    fn assemble_ranks_by_relevance() {
        let assembler = ContextAssembler::new(ContextBudget {
            max_items: 10,
            max_tokens: 100,
        });
        let assembly = assembler.assemble(&[result(
            "test",
            vec![
                ContextItem::new("low", ContextType::Resource, "low")
                    .with_relevance(0.1)
                    .with_token_count(1),
                ContextItem::new("high", ContextType::Resource, "high")
                    .with_relevance(0.9)
                    .with_token_count(1),
            ],
        )]);

        assert_eq!(assembly.items[0].id, "high");
        assert_eq!(assembly.items[1].id, "low");
        assert!(!assembly.truncated);
    }

    #[test]
    fn assemble_uses_priority_trust_and_freshness_as_ranking_signals() {
        let assembler = ContextAssembler::new(ContextBudget {
            max_items: 10,
            max_tokens: 100,
        });
        let assembly = assembler.assemble(&[result(
            "test",
            vec![
                ContextItem::new("plain", ContextType::Resource, "plain")
                    .with_relevance(0.7)
                    .with_token_count(1),
                ContextItem::new("boosted", ContextType::Resource, "boosted")
                    .with_relevance(0.6)
                    .with_priority(1.0)
                    .with_trust(1.0)
                    .with_freshness(1.0)
                    .with_token_count(1),
            ],
        )]);

        assert_eq!(assembly.items[0].id, "boosted");
        assert_eq!(assembly.items[1].id, "plain");
    }

    #[test]
    fn assemble_dedupes_by_source_and_keeps_more_relevant_item() {
        let assembler = ContextAssembler::with_default_budget();
        let assembly = assembler.assemble(&[result(
            "test",
            vec![
                ContextItem::new("old", ContextType::Resource, "old")
                    .with_source("file://auth.rs")
                    .with_relevance(0.2),
                ContextItem::new("new", ContextType::Resource, "new")
                    .with_source("file://auth.rs")
                    .with_relevance(0.8),
            ],
        )]);

        assert_eq!(assembly.items.len(), 1);
        assert_eq!(assembly.items[0].id, "new");
        assert!(assembly.truncated);
    }

    #[test]
    fn assemble_dedupes_by_ranking_score() {
        let assembler = ContextAssembler::with_default_budget();
        let assembly = assembler.assemble(&[result(
            "test",
            vec![
                ContextItem::new("plain", ContextType::Resource, "plain")
                    .with_source("file://auth.rs")
                    .with_relevance(0.7),
                ContextItem::new("boosted", ContextType::Resource, "boosted")
                    .with_source("file://auth.rs")
                    .with_relevance(0.6)
                    .with_priority(1.0),
            ],
        )]);

        assert_eq!(assembly.items.len(), 1);
        assert_eq!(assembly.items[0].id, "boosted");
        assert!(assembly.truncated);
    }

    #[test]
    fn assemble_respects_item_and_token_budget() {
        let assembler = ContextAssembler::new(ContextBudget {
            max_items: 1,
            max_tokens: 5,
        });
        let assembly = assembler.assemble(&[result(
            "test",
            vec![
                ContextItem::new("a", ContextType::Resource, "one two")
                    .with_relevance(0.9)
                    .with_token_count(2),
                ContextItem::new("b", ContextType::Resource, "three four")
                    .with_relevance(0.8)
                    .with_token_count(2),
            ],
        )]);

        assert_eq!(assembly.items.len(), 1);
        assert_eq!(assembly.total_tokens, 2);
        assert!(assembly.truncated);
    }

    #[test]
    fn assemble_caps_items_per_source() {
        let assembler = ContextAssembler::new(ContextBudget {
            max_items: 10,
            max_tokens: 100,
        })
        .with_source_policy(ContextSourcePolicy {
            max_items_per_source: Some(2),
            max_tokens_per_source: None,
        });
        let assembly = assembler.assemble(&[result(
            "test",
            vec![
                ContextItem::new("a", ContextType::Resource, "a")
                    .with_source("file://a")
                    .with_relevance(0.9)
                    .with_token_count(1),
                ContextItem::new("b", ContextType::Resource, "b")
                    .with_source("file://b")
                    .with_relevance(0.8)
                    .with_token_count(1),
                ContextItem::new("c", ContextType::Resource, "c")
                    .with_source("file://c")
                    .with_relevance(0.7)
                    .with_token_count(1),
            ],
        )]);

        assert_eq!(assembly.items.len(), 2);
        assert_eq!(assembly.items[0].id, "a");
        assert_eq!(assembly.items[1].id, "b");
        assert!(assembly.truncated);
    }

    #[test]
    fn assemble_caps_tokens_per_source_but_keeps_other_sources() {
        let assembler = ContextAssembler::new(ContextBudget {
            max_items: 10,
            max_tokens: 100,
        })
        .with_source_policy(ContextSourcePolicy {
            max_items_per_source: None,
            max_tokens_per_source: Some(3),
        });
        let assembly = assembler.assemble(&[result(
            "test",
            vec![
                ContextItem::new("file-a", ContextType::Resource, "file a")
                    .with_source("file://a")
                    .with_relevance(0.9)
                    .with_token_count(2),
                ContextItem::new("file-b", ContextType::Resource, "file b")
                    .with_source("file://b")
                    .with_relevance(0.8)
                    .with_token_count(2),
                ContextItem::new("memory", ContextType::Memory, "memory")
                    .with_source("memory://a")
                    .with_relevance(0.7)
                    .with_token_count(2),
            ],
        )]);

        assert_eq!(
            assembly
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["file-a", "memory"]
        );
        assert_eq!(assembly.total_tokens, 4);
        assert!(assembly.truncated);
    }

    #[test]
    fn required_workspace_instructions_bypass_generic_context_budgets() {
        let assembler = ContextAssembler::new(ContextBudget {
            max_items: 1,
            max_tokens: 2,
        })
        .with_source_policy(ContextSourcePolicy {
            max_items_per_source: Some(1),
            max_tokens_per_source: Some(1),
        });
        let instructions = ContextItem::new(
            "agents_md",
            ContextType::Resource,
            "one two three four five six",
        )
        .with_source("a3s://workspace-instructions")
        .with_provenance("workspace_instructions")
        .with_token_count(6)
        .with_required();
        let ordinary = ContextItem::new("ordinary", ContextType::Resource, "ordinary")
            .with_token_count(1)
            .with_relevance(1.0);

        let assembly = assembler.assemble(&[result("test", vec![ordinary, instructions])]);

        assert_eq!(assembly.items.len(), 1);
        assert_eq!(assembly.items[0].id, "agents_md");
        assert_eq!(assembly.total_tokens, 6);
        assert!(assembly.truncated);
    }
}
