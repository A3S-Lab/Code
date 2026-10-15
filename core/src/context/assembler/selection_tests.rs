#![cfg(test)]

use super::*;
use crate::context::ContextType;

fn contribution(provider: &str, item: ContextItem) -> ContextResult {
    let mut result = ContextResult::new(provider);
    result.add_item(item);
    result
}

fn required() -> ContextItem {
    ContextItem::new(
        "instructions",
        ContextType::Resource,
        "Keep the required instruction.",
    )
    .with_source("a3s://workspace-instructions")
    .with_relevance(0.0)
    .with_token_count(8)
    .with_required()
}

fn optional() -> ContextItem {
    ContextItem::new("optional", ContextType::Resource, "Optional search result.")
        .with_source("a3s://workspace-instructions")
        .with_relevance(1.0)
        .with_priority(1.0)
        .with_trust(1.0)
        .with_freshness(1.0)
        .with_token_count(1)
}

#[test]
fn required_instruction_survives_higher_scored_source_duplicate_in_both_orders() {
    for reverse in [false, true] {
        for cap in [0, 1, 16] {
            let mut results = vec![
                contribution("instructions", required()),
                contribution("search", optional()),
            ];
            if reverse {
                results.reverse();
            }
            let assembly = ContextAssembler::new(ContextBudget {
                max_items: cap,
                max_tokens: cap,
            })
            .assemble(&results);
            assert_eq!(assembly.items.len(), 1, "order={reverse} cap={cap}");
            assert!(assembly.items[0].is_required());
            assert_eq!(assembly.items[0].id, "instructions");
            assert_eq!(assembly.total_tokens, 8);
            assert!(assembly.truncated);
            assert!(!assembly.to_xml().contains("Optional search result."));
        }
    }
}

#[test]
fn required_instruction_survives_duplicate_id_without_source() {
    for reverse in [false, true] {
        let mut required = required();
        required.source = None;
        let mut optional = optional();
        optional.source = None;
        optional.id.clone_from(&required.id);
        let mut results = vec![
            contribution("instructions", required),
            contribution("search", optional),
        ];
        if reverse {
            results.reverse();
        }
        let assembly = ContextAssembler::with_default_budget().assemble(&results);
        assert_eq!(assembly.items.len(), 1);
        assert!(assembly.items[0].is_required());
        assert_eq!(assembly.items[0].content, "Keep the required instruction.");
    }
}

#[test]
fn latest_provider_contribution_can_withdraw_a_required_instruction() {
    let results = vec![
        contribution("instructions", required()),
        contribution("instructions", optional()),
    ];
    let assembly = ContextAssembler::with_default_budget().assemble(&results);
    assert_eq!(assembly.items.len(), 1);
    assert!(!assembly.items[0].is_required());
    assert_eq!(assembly.items[0].id, "optional");
    assert!(!assembly.to_xml().contains("Keep the required instruction."));
}

#[test]
fn equal_rank_same_id_sources_have_total_order_and_stable_budget_selection() {
    let sources = ["file://a path", "file://back\\slash", "file://资料"];
    let items = sources.map(|source| {
        ContextItem::new("same-local-id", ContextType::Resource, source)
            .with_source(source)
            .with_relevance(0.5)
            .with_token_count(1)
    });
    for first in 0..items.len() {
        for second in 0..items.len() {
            assert_eq!(
                compare_ranked_items(&items[first], &items[second]),
                first.cmp(&second)
            );
        }
    }
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let results = order.map(|index| contribution(sources[index], items[index].clone()));
        for cap in 0..=items.len() {
            let assembly = ContextAssembler::new(ContextBudget {
                max_items: cap,
                max_tokens: cap,
            })
            .assemble(&results);
            let actual = assembly
                .items
                .iter()
                .map(|item| item.source.as_deref().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(actual, sources[..cap], "order={order:?} cap={cap}");
            assert_eq!(assembly.total_tokens, cap);
            assert_eq!(assembly.truncated, cap < items.len());
            let expected = items[..cap]
                .iter()
                .map(ContextItem::to_xml)
                .collect::<Vec<_>>()
                .join("\n\n");
            assert_eq!(assembly.to_xml(), expected);
        }
    }
}
