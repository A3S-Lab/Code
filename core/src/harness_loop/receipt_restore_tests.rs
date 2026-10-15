#![cfg(test)]

use super::*;
use std::collections::BTreeSet;

fn keys(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).into()).collect()
}

#[test]
fn absent_original_log_cannot_forgive_cached_unknown_or_verified_paths() {
    let mut cached = MutationLedger::default();
    cached.observe_unseen_paths(&["changed.txt".into()]);
    cached.set_unresolved_tool_effects(keys(&["invocation:old-unknown"]));
    let cached: MutationLedger =
        serde_json::from_value(serde_json::to_value(cached).unwrap()).unwrap();
    let mut restored = MutationLedger::default();
    restored.restore_checkpoint_cache(&cached, false);
    restored.restore_unresolved_tool_effects(BTreeSet::new(), &BTreeSet::new());
    assert!(restored
        .unresolved_tool_effects
        .contains("invocation:old-unknown"));
    assert!(restored
        .unresolved_tool_effects
        .contains("tool_effect_cache_evidence_missing"));
    assert_eq!(restored.paths().collect::<Vec<_>>(), ["changed.txt"]);
    assert!(matches!(
        decide_completion(&restored, &[], &[], false),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn later_log_uncertainty_and_current_state_survive_a_stale_checkpoint() {
    let mut cached = MutationLedger::default();
    cached.observe_unseen_paths(&["changed.txt".into()]);
    cached.set_unresolved_tool_effects(keys(&["old-unknown"]));
    let mut live = MutationLedger::default();
    let after = a3s_effect::ToolFileIdentityV1 {
        kind: a3s_effect::ToolFileKind::File,
        content_digest: Some(format!("sha256:{}", sha256::digest(b"later bytes"))),
        mode: Some(0o644),
    };
    live.observe_receipt_path("bash", "changed.txt", &after);
    live.set_unresolved_tool_effects(keys(&["later-unknown"]));
    let current_digest = live.digest().to_string();
    live.restore_checkpoint_cache(&cached, true);
    live.restore_unresolved_tool_effects(keys(&["latest-unknown"]), &BTreeSet::new());
    assert_eq!(live.digest(), current_digest);
    assert_eq!(live.records[0].after_identity.as_ref(), Some(&after));
    assert_eq!(
        live.unresolved_tool_effects,
        keys(&["old-unknown", "later-unknown", "latest-unknown"])
    );
    assert!(live.applied_tool_receipts.is_empty());
}

#[test]
fn child_debt_requires_exact_committed_terminal_and_never_a_local_scan_alone() {
    let mut ledger = MutationLedger::default();
    ledger.set_unresolved_tool_effects(keys(&[
        "workspace-child:one",
        "workspace-child:two",
        "invocation:unknown",
    ]));
    ledger.open_children.push(OpenWorkspaceChild {
        task_id: "one".into(),
        porcelain: None,
        head: None,
        nongit: true,
    });
    // Bare child ids, unrelated keys and local observations have no authority.
    ledger.restore_unresolved_tool_effects(BTreeSet::new(), &keys(&["one", "invocation:unknown"]));
    assert!(ledger.has_open_children());
    ledger.record_child_paths("one", &[]);
    assert!(ledger
        .unresolved_tool_effects
        .contains("workspace-child:one"));
    ledger.open_children.push(OpenWorkspaceChild {
        task_id: "one".into(),
        porcelain: None,
        head: None,
        nongit: true,
    });
    ledger.restore_unresolved_tool_effects(
        BTreeSet::new(),
        &keys(&["workspace-child:one", "invocation:unknown"]),
    );
    assert!(!ledger.has_open_children());
    assert_eq!(
        ledger.unresolved_tool_effects,
        keys(&["workspace-child:two", "invocation:unknown"])
    );
}

#[test]
fn exact_durable_terminal_settles_a_cached_nongit_child_without_a_local_registry() {
    let id = "cached-terminal-child-no-local-slot";
    assert!(!crate::porcelain::workspace_child_pending(id));
    let mut cached = MutationLedger::default();
    cached.open_children.push(OpenWorkspaceChild {
        task_id: id.into(),
        porcelain: None,
        head: None,
        nongit: true,
    });
    let key = format!("workspace-child:{id}");
    cached.set_unresolved_tool_effects(BTreeSet::from([key.clone()]));
    let mut restored = MutationLedger::default();
    restored.restore_checkpoint_cache(&cached, true);
    assert!(restored.has_open_children());
    restored.restore_unresolved_tool_effects(BTreeSet::new(), &BTreeSet::from([key]));
    assert!(!restored.has_open_children());
    assert!(restored.unresolved_tool_effects.is_empty());
    assert!(matches!(
        decide_completion(&restored, &[], &[], false),
        CompletionGate::Allow(CompletionTerminal::Narrative)
    ));
}

#[test]
fn an_empty_cache_does_not_invent_debt_or_change_default_projection() {
    let mut ledger = MutationLedger::default();
    ledger.restore_checkpoint_cache(&MutationLedger::default(), false);
    assert_eq!(ledger, MutationLedger::default());
}

fn coverage_receipt(id: &str, path: &str, bytes: &[u8]) -> a3s_effect::ToolExecutionResultV1 {
    a3s_effect::ToolExecutionResultV1 {
        schema: a3s_effect::TOOL_RESULT_SCHEMA.into(),
        identity: a3s_effect::ToolInvocationIdentityV1 {
            turn: 1,
            cycle: 0,
            tool_call_id: id.into(),
            batch_index: 0,
            admission: None,
        },
        prepared_digest: format!("sha256:{}", "a".repeat(64)),
        outcome: a3s_effect::ToolExecutionOutcome::Succeeded,
        exit_code: Some(0),
        output: serde_json::json!("observed"),
        effects: a3s_effect::ToolEffectObservationV1::complete(vec![
            a3s_effect::ToolPathChangeV1 {
                path: path.into(),
                before: None,
                after: a3s_effect::ToolFileIdentityV1 {
                    kind: a3s_effect::ToolFileKind::File,
                    content_digest: Some(format!("sha256:{}", sha256::digest(bytes))),
                    mode: Some(0o644),
                },
            },
        ]),
    }
}

#[test]
fn cache_coverage_requires_every_receipt_and_exact_observed_state() {
    let first = coverage_receipt("first-write", "first.txt", b"first bytes");
    let second = coverage_receipt("second-write", "second.txt", b"second bytes");
    let mut cached = MutationLedger::default();
    cached.observe_tool_receipt("write", &first);
    cached.observe_tool_receipt("write", &second);
    let round_trip: MutationLedger =
        serde_json::from_value(serde_json::to_value(&cached).unwrap()).unwrap();
    assert!(round_trip.checkpoint_cache_covered_by(&cached));

    let mut partial = MutationLedger::default();
    partial.observe_tool_receipt("write", &second);
    assert!(!cached.checkpoint_cache_covered_by(&partial));
    let mut read = coverage_receipt("unrelated-read", "unused", b"");
    read.effects = a3s_effect::ToolEffectObservationV1::complete(Vec::new());
    let mut unrelated = MutationLedger::default();
    unrelated.observe_tool_receipt("read", &read);
    assert!(!cached.checkpoint_cache_covered_by(&unrelated));

    for mismatched in [
        coverage_receipt("first-write", "first.txt", b"wrong bytes"),
        coverage_receipt("first-write", "wrong-path.txt", b"first bytes"),
    ] {
        let mut original = MutationLedger::default();
        original.observe_tool_receipt("write", &mismatched);
        original.observe_tool_receipt("write", &second);
        assert!(!cached.checkpoint_cache_covered_by(&original));
    }
    let mut different_mode = first.clone();
    different_mode.effects.changes[0].after.mode = Some(0o755);
    let mut original = MutationLedger::default();
    original.observe_tool_receipt("write", &different_mode);
    original.observe_tool_receipt("write", &second);
    assert!(!cached.checkpoint_cache_covered_by(&original));
    let mut different_kind = first.clone();
    different_kind.effects.changes[0].after.kind = a3s_effect::ToolFileKind::Symlink;
    let mut original = MutationLedger::default();
    original.observe_tool_receipt("write", &different_kind);
    original.observe_tool_receipt("write", &second);
    assert!(!cached.checkpoint_cache_covered_by(&original));
}

#[test]
fn cache_coverage_binds_background_fact_identity_and_rejects_legacy_or_bad_digest() {
    let receipt = coverage_receipt("write", "background.txt", b"background bytes");
    let mut cached = MutationLedger::default();
    cached.observe_background_receipt("background:exact-child:exit", &receipt.effects);
    let mut original = MutationLedger::default();
    original.observe_background_receipt("background:exact-child:exit", &receipt.effects);
    assert!(cached.checkpoint_cache_covered_by(&original));
    let mut unrelated = MutationLedger::default();
    unrelated.observe_background_receipt("background:other-child:exit", &receipt.effects);
    assert!(!cached.checkpoint_cache_covered_by(&unrelated));

    let mut legacy = MutationLedger::default();
    legacy.observe_unseen_paths(&["background.txt".into()]);
    assert!(!legacy.checkpoint_cache_covered_by(&original));
    cached.digest = sha256::digest(b"invented cached digest");
    assert!(!cached.checkpoint_cache_covered_by(&original));
}

#[test]
fn exact_original_unknown_covers_cached_debt_without_claiming_completion() {
    let mut unknown = coverage_receipt("original-unknown", "changed.txt", b"partial actual bytes");
    unknown.outcome = a3s_effect::ToolExecutionOutcome::Unknown;
    unknown.effects.completeness = a3s_effect::ToolObservationCompleteness::Incomplete;
    unknown.effects.reason = Some("original observation was incomplete".into());
    unknown.effects.unresolved_external_operations = vec!["opaque exact external operation".into()];
    let mut cached = MutationLedger::default();
    cached.observe_tool_receipt("bash", &unknown);
    let mut original = MutationLedger::default();
    original.observe_tool_receipt("bash", &unknown);
    assert!(cached.checkpoint_cache_covered_by(&original));
    let mut restored = MutationLedger::default();
    restored.restore_checkpoint_cache(&cached, true);
    assert_eq!(
        restored.unresolved_tool_effects,
        cached.unresolved_tool_effects
    );
    assert!(!restored
        .unresolved_tool_effects
        .contains("tool_effect_cache_evidence_missing"));
    assert!(matches!(
        decide_completion(&restored, &[], &[], false),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn cache_cannot_borrow_unrelated_uncertainty_or_invent_a_missing_claim() {
    let write = coverage_receipt("exact-write", "changed.txt", b"exact bytes");
    let mut cached = MutationLedger::default();
    cached.observe_tool_receipt("bash", &write);
    let mut original = cached.clone();
    original.set_unresolved_tool_effects(keys(&["unrelated actual C"]));
    assert!(cached.checkpoint_cache_covered_by(&original));
    cached.set_unresolved_tool_effects(keys(&["invented cached C"]));
    assert!(!cached.checkpoint_cache_covered_by(&original));
    cached.unresolved_tool_effects.clear();
    cached.set_unresolved_tool_effects(keys(&["unrelated actual C"]));
    assert!(cached.checkpoint_cache_covered_by(&original));
    assert_eq!(
        cached.checkpoint_uncertainty_keys(),
        &keys(&["unrelated actual C"])
    );
}

#[test]
fn cached_child_snapshots_and_local_incomplete_flags_need_their_own_provenance() {
    let write = coverage_receipt("exact-write", "changed.txt", b"exact bytes");
    let mut cached = MutationLedger::default();
    cached.observe_tool_receipt("bash", &write);
    let original = cached.clone();
    cached.set_observation_incomplete(true);
    assert!(!cached.checkpoint_cache_covered_by(&original));
    cached.set_observation_incomplete(false);
    cached.open_children.push(OpenWorkspaceChild {
        task_id: "unproved child".into(),
        porcelain: None,
        head: None,
        nongit: true,
    });
    assert!(!cached.checkpoint_cache_covered_by(&original));
    assert!(!cached.checkpoint_cache_covered_by(&cached));
}
