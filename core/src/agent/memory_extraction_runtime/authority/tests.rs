#![cfg(test)]

use super::*;
use crate::agent::memory_extraction_runtime::{
    parse_extracted_memories, ExtractedMemory, MAX_MEMORY_CONTENT_CHARS, MAX_MEMORY_REASON_CHARS,
};
use a3s_memory::{MemoryItem, MemoryType};
use serde_json::json;
use std::collections::HashSet;

fn proposal() -> ExtractedMemory {
    ExtractedMemory {
        memory_type: "semantic".to_string(),
        content: "Use tabs for indentation in the engine repository.".to_string(),
        importance: Some(0.80),
        confidence: Some(0.90),
        tags: vec![],
        source: Some("project_fact".to_string()),
        scope: Some("workspace".to_string()),
        reason: Some("This rule applies to future engine changes.".to_string()),
        supersedes: vec![],
        conflicts_with: vec![],
        evolution: None,
    }
}

fn validate(proposal: ExtractedMemory) -> Option<MemoryItem> {
    proposal
        .into_memory_item("/actual/project", "actual-session", &HashSet::new())
        .map(|(item, _, _)| item)
}

#[test]
fn diagnostic_sets_refuse_unknown_noncanonical_and_missing_tokens() {
    for token in ["validated_model", "host_typed"] {
        assert_eq!(
            MemoryRetentionOriginV1::parse(token).unwrap().as_str(),
            token
        );
    }
    for token in [
        "not_admitted",
        "no_items",
        "stored",
        "refused",
        "partial",
        "unproved",
    ] {
        assert_eq!(
            MemoryRetentionDispositionV1::parse(token).unwrap().as_str(),
            token
        );
    }
    for token in [
        "disabled",
        "empty_turn",
        "turn_not_completed",
        "budget_denied",
        "cancelled",
        "invalid_output",
        "invalid_item",
        "provider_failed",
        "admission_unproved",
        "store_failed",
        "relation_update_failed",
    ] {
        assert_eq!(
            MemoryRetentionRefusalV1::parse(token).unwrap().as_str(),
            token
        );
    }
    for token in ["", "unknown", " host_typed", "HOST_TYPED", "host"] {
        assert!(MemoryRetentionOriginV1::parse(token)
            .unwrap_err()
            .to_string()
            .contains("MemoryRetentionOriginV1"));
    }
    for token in ["", "unknown", "stored ", "Stored", "success"] {
        assert!(MemoryRetentionDispositionV1::parse(token)
            .unwrap_err()
            .to_string()
            .contains("MemoryRetentionDispositionV1"));
    }
    for token in ["", "unknown", " cancelled", "CANCELLED", "budget"] {
        assert!(MemoryRetentionRefusalV1::parse(token)
            .unwrap_err()
            .to_string()
            .contains("MemoryRetentionRefusalV1"));
    }
}

#[test]
fn same_content_and_parsed_diagnostics_do_not_replace_original_validation() {
    // This is a consumption-boundary test. Actual host API/store entry and
    // two-store session scenarios belong to the subsequent integration tasks.
    let content = proposal().content;
    let response = json!({"items": [{
        "memory_type": "semantic", "content": content,
        "importance": 0.80, "confidence": 0.90,
        "source": "project_fact", "scope": "workspace",
        "reason": "This rule applies to future engine changes."
    }]});
    let accepted = parse_extracted_memories(&response.to_string())
        .unwrap()
        .items
        .into_iter()
        .filter_map(validate)
        .collect::<Vec<_>>();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].content, content);
    assert!(parse_extracted_memories(r#"{"items":[]}"#)
        .unwrap()
        .items
        .is_empty());
    assert!(parse_extracted_memories(&content).is_err());

    for origin in ["validated_model", "host_typed"] {
        let _diagnostic = MemoryRetentionOriginV1::parse(origin).unwrap();
        let _status = MemoryRetentionDispositionV1::parse("stored").unwrap();
        let mut invalid = response.clone();
        invalid["items"][0]["origin"] = json!(origin);
        invalid["items"][0]["status"] = json!("stored");
        invalid["items"][0]
            .as_object_mut()
            .unwrap()
            .remove("importance");
        let proposals = parse_extracted_memories(&invalid.to_string())
            .unwrap()
            .items;
        assert_eq!(proposals.len(), 1);
        assert!(validate(proposals.into_iter().next().unwrap()).is_none());
    }
}

#[test]
fn original_numeric_source_type_and_scope_boundaries_remain_required() {
    for (importance, confidence) in [(0.70, 0.75), (1.0, 1.0)] {
        let mut item = proposal();
        item.importance = Some(importance);
        item.confidence = Some(confidence);
        assert!(validate(item).is_some());
    }
    for value in [
        None,
        Some(0.69),
        Some(1.01),
        Some(f32::NAN),
        Some(f32::INFINITY),
    ] {
        let mut item = proposal();
        item.importance = value;
        assert!(validate(item).is_none());
    }
    for value in [
        None,
        Some(0.74),
        Some(1.01),
        Some(f32::NAN),
        Some(f32::NEG_INFINITY),
    ] {
        let mut item = proposal();
        item.confidence = value;
        assert!(validate(item).is_none());
    }
    for (source, memory_type) in [
        ("project_fact", "semantic"),
        ("preference", "semantic"),
        ("workflow", "procedural"),
        ("failure", "procedural"),
        ("decision", "semantic"),
        ("decision", "procedural"),
    ] {
        let mut item = proposal();
        item.source = Some(source.to_string());
        item.memory_type = memory_type.to_string();
        assert!(validate(item).is_some());
    }
    for (source, memory_type) in [
        (None, "semantic"),
        (Some("unknown"), "semantic"),
        (Some("project_fact"), "procedural"),
        (Some("preference"), "procedural"),
        (Some("workflow"), "semantic"),
        (Some("failure"), "semantic"),
        (Some("decision"), "episodic"),
        (Some("decision"), "working"),
        (Some("decision"), ""),
    ] {
        let mut item = proposal();
        item.source = source.map(str::to_string);
        item.memory_type = memory_type.to_string();
        assert!(validate(item).is_none());
    }
    for (scope, canonical) in [
        ("workspace", "workspace"),
        ("project", "workspace"),
        ("repository", "workspace"),
        ("repo", "workspace"),
        ("user", "user"),
        ("personal", "user"),
    ] {
        let mut item = proposal();
        item.scope = Some(scope.to_string());
        let item = validate(item).unwrap();
        assert_eq!(item.metadata["scope"], canonical);
        assert_eq!(item.metadata["workspace"], "/actual/project");
        assert_eq!(item.metadata["session_id"], "actual-session");
        assert_eq!(item.memory_type, MemoryType::Semantic);
    }
    for scope in [None, Some(""), Some("global"), Some("host_typed")] {
        let mut item = proposal();
        item.scope = scope.map(str::to_string);
        assert!(validate(item).is_none());
    }
}

#[test]
fn original_lengths_secret_refusal_and_exact_relation_allowset_are_preserved() {
    let mut item = proposal();
    item.content = "界".repeat(MAX_MEMORY_CONTENT_CHARS + 1);
    item.reason = Some("由".repeat(MAX_MEMORY_REASON_CHARS + 1));
    let item = validate(item).unwrap();
    assert_eq!(item.content.chars().count(), 1_216);
    assert_eq!(item.metadata["reason"].chars().count(), 336);
    assert!(item.content.ends_with("\n... (truncated)"));
    assert!(item.metadata["reason"].ends_with("\n... (truncated)"));
    for (content_len, reason_len, accepted) in [(20, 12, true), (19, 12, false), (20, 11, false)] {
        let mut item = proposal();
        item.content = "界".repeat(content_len);
        item.reason = Some("由".repeat(reason_len));
        assert_eq!(validate(item).is_some(), accepted);
    }
    let mut item = proposal();
    item.reason = None;
    assert!(validate(item).is_none());
    for in_reason in [false, true] {
        let mut item = proposal();
        let secret = "The API key is sk-1234567890abcdef1234567890abcdef.";
        if in_reason {
            item.reason = Some(secret.to_string());
        } else {
            item.content = secret.to_string();
        }
        assert!(validate(item).is_none());
    }
    let mut item = proposal();
    item.supersedes = vec![
        "old-exact".to_string(),
        "unprovided".to_string(),
        "old-exact".to_string(),
    ];
    item.conflicts_with = vec!["OLD-EXACT".to_string(), "old-exact".to_string()];
    let (_, supersedes, conflicts) = item
        .into_memory_item(
            "/actual/project",
            "actual-session",
            &HashSet::from(["old-exact".to_string()]),
        )
        .unwrap();
    assert_eq!(supersedes, ["old-exact"]);
    assert_eq!(conflicts, ["old-exact"]);
}

#[test]
fn ten_primary_acks_and_fifty_archive_unit_acks_retain_all_sixty_entries() {
    let mut progress =
        MemoryRetentionProgress::new(MemoryRetentionOriginV1::ValidatedModel, 10, 0).unwrap();
    assert_eq!(progress.origin(), MemoryRetentionOriginV1::ValidatedModel);
    assert_eq!(progress.item_counts(), (10, 0));
    for primary_index in 0..10 {
        assert!(progress.enter_primary(primary_index));
        assert!(progress.acknowledge_primary(primary_index, format!("canonical-{primary_index}")));
        for relation in 0..5 {
            let index = progress
                .require_archive(primary_index, format!("old-{relation}"))
                .unwrap();
            assert!(progress.enter_archive(index));
            assert!(progress.acknowledge_archive(index));
            assert!(progress.finish_archive(index, true));
        }
        assert!(progress
            .require_archive(primary_index, "sixth-old".to_string())
            .is_none());
        assert!(progress
            .require_archive(primary_index, "old-0".to_string())
            .is_none());
    }
    assert_eq!(progress.entered_store_calls(), 60);
    assert_eq!(progress.primary().len(), 10);
    assert_eq!(progress.archives().len(), 50);
    for (index, primary) in progress.primary().iter().enumerate() {
        assert_eq!(
            primary.canonical_id.as_deref(),
            Some(format!("canonical-{index}").as_str())
        );
    }
    assert!(progress
        .archives()
        .iter()
        .all(|archive| archive.acknowledged && archive.applied == Some(true)));
    assert!(!progress.has_unproved_write());
    assert!(!progress.enter_primary(0));
    assert!(!progress.acknowledge_primary(0, "second-result".to_string()));
    assert!(!progress.enter_archive(0));
    assert!(!progress.acknowledge_archive(0));
    assert!(!progress.finish_archive(0, true));
    assert!(progress
        .require_archive(10, "out-of-range".to_string())
        .is_none());
    assert!(MemoryRetentionProgress::new(MemoryRetentionOriginV1::ValidatedModel, 11, 0).is_none());
    let rejected =
        MemoryRetentionProgress::new(MemoryRetentionOriginV1::ValidatedModel, 10, usize::MAX)
            .unwrap();
    assert_eq!(rejected.item_counts(), (10, usize::MAX));
    assert_eq!(rejected.primary().len(), 10);
    assert!(rejected.archives().is_empty());
    assert!(
        MemoryRetentionProgress::new(MemoryRetentionOriginV1::ValidatedModel, usize::MAX, 1)
            .is_none()
    );
}

#[test]
fn pending_entries_do_not_erase_prior_canonical_acks_or_invent_archive_success() {
    let mut progress =
        MemoryRetentionProgress::new(MemoryRetentionOriginV1::ValidatedModel, 2, 1).unwrap();
    let archive = progress
        .require_archive(0, "exact-old-id".to_string())
        .unwrap();
    assert!(!progress.acknowledge_primary(0, "no-entry".to_string()));
    assert!(!progress.acknowledge_archive(archive));
    assert!(!progress.finish_archive(archive, true));
    assert!(progress.enter_primary(0));
    assert!(progress.has_unproved_write());
    assert!(progress.acknowledge_primary(0, "returned-canonical-id".to_string()));
    assert!(!progress.has_unproved_write());
    assert!(progress.enter_archive(archive));
    assert!(!progress.finish_archive(archive, false));
    assert!(progress.enter_primary(1));
    progress.refuse(MemoryRetentionRefusalV1::StoreFailed);
    progress.refuse(MemoryRetentionRefusalV1::Cancelled);
    assert_eq!(
        progress.refusal(),
        Some(MemoryRetentionRefusalV1::StoreFailed)
    );
    assert_eq!(progress.item_counts(), (2, 1));
    assert_eq!(progress.entered_store_calls(), 3);
    assert!(progress.has_unproved_write());
    assert_eq!(
        progress.primary()[0].canonical_id.as_deref(),
        Some("returned-canonical-id")
    );
    assert_eq!(progress.archives()[archive].addressed_id, "exact-old-id");
    assert!(!progress.archives()[archive].acknowledged);
    assert_eq!(progress.archives()[archive].applied, None);
}

#[test]
fn missing_archive_target_records_false_without_a_store_entry() {
    let mut progress =
        MemoryRetentionProgress::new(MemoryRetentionOriginV1::ValidatedModel, 1, 0).unwrap();
    let archive = progress
        .require_archive(0, "missing-exact-id".to_string())
        .unwrap();
    assert!(progress.enter_primary(0));
    assert!(progress.acknowledge_primary(0, "replacement-canonical-id".to_string()));
    assert!(progress.finish_archive(archive, false));
    assert!(!progress.enter_archive(archive));
    assert!(!progress.acknowledge_archive(archive));
    progress.refuse(MemoryRetentionRefusalV1::RelationUpdateFailed);
    assert_eq!(progress.entered_store_calls(), 1);
    assert!(!progress.has_unproved_write());
    assert_eq!(progress.archives()[archive].applied, Some(false));
    assert_eq!(
        progress.primary()[0].canonical_id.as_deref(),
        Some("replacement-canonical-id")
    );
}
