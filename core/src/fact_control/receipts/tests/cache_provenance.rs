#![cfg(test)]

use super::*;
use crate::harness_loop::{CompletionGate, MutationLedger};
use std::collections::BTreeSet;

fn project_receipts(debt: &ReceiptDebt) -> MutationLedger {
    let mut ledger = MutationLedger::default();
    for projection in &debt.projection_order {
        match projection {
            ReceiptProjection::Tool(index) => {
                let (tool, receipt) = &debt.results[*index];
                ledger.observe_tool_receipt(tool, receipt);
            }
            ReceiptProjection::Background(index) => {
                let (key, effects) = &debt.background_results[*index];
                ledger.observe_background_receipt(key, effects);
            }
        }
    }
    ledger
}

fn assert_incomplete_without_cache_gap(ledger: &MutationLedger) {
    let encoded = serde_json::to_value(ledger).unwrap();
    let unresolved = encoded["unresolved_tool_effects"].as_array().unwrap();
    assert!(!unresolved.contains(&json!("tool_effect_cache_evidence_missing")));
    assert!(matches!(
        crate::harness_loop::decide_completion(ledger, &[], &[], false),
        CompletionGate::Incomplete { message } if message.contains("tool_effect_unresolved")
    ));
}

#[test]
fn actual_write_and_host_verifier_provenance_survive_unrelated_prepared_uncertainty() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("changed.txt"), b"actual bytes").unwrap();
    let write = preparation("write-A");
    let mut written = result(&write, ToolExecutionOutcome::Succeeded);
    written.exit_code = Some(0);
    written.effects = crate::porcelain::observe_path_effects(
        workspace.path(),
        crate::porcelain::PathDelta {
            paths: vec!["changed.txt".into()],
            incomplete: false,
        },
    );
    assert_eq!(
        std::fs::read(workspace.path().join("changed.txt")).unwrap(),
        b"actual bytes"
    );
    let mut cached = MutationLedger::default();
    cached.observe_tool_receipt("bash", &written);
    let report = crate::verification::VerificationReport::new(
        "actual exact-byte check",
        vec![crate::verification::VerificationCheck::required(
            "exact-bytes",
            "file",
            "read changed.txt and compared its actual bytes",
        )
        .with_status(crate::verification::VerificationStatus::Passed)],
    )
    .with_effect_digest(cached.digest());
    let verify = preparation("verifier-V");
    let mut verified = result_without_execution(
        &verify,
        ToolExecutionOutcome::Succeeded,
        "actual byte check passed".into(),
    )
    .unwrap();
    verified.output = runner_result_value(
        "actual byte check passed",
        false,
        &[],
        0,
        Some(&json!({"verification_report":report,"verification_author":"host"})),
    );
    cached.observe_tool_receipt("bash", &verified);
    let mut accepted = Vec::new();
    let envelope = verified
        .output
        .as_str()
        .and_then(decode_fact_tool_result)
        .unwrap();
    crate::agent::AgentLoop::collect_verification_report(&mut accepted, &envelope.metadata);
    assert_eq!(accepted, [report]);

    for before in [false, true] {
        let pending = preparation("unrelated-C");
        let mut facts = vec![
            prepared_fact(&write),
            result_fact(&written),
            prepared_fact(&verify),
            result_fact(&verified),
        ];
        facts.insert(
            if before { 0 } else { facts.len() },
            prepared_fact(&pending),
        );
        let debt = fold_cache_evidence(&facts);
        assert!(!debt.integrity_invalid);
        let key = pending.identity.key().unwrap();
        assert_eq!(debt.validated_uncertainty, BTreeSet::from([key.clone()]));
        let boundary = debt.projection_order.last().unwrap();
        assert_eq!(
            debt.validated_uncertainty_for(boundary).contains(&key),
            before
        );
        let mut original = project_receipts(&debt);
        original.restore_unresolved_tool_effects(
            debt.validated_uncertainty_for(boundary),
            &BTreeSet::new(),
        );
        assert!(cached.checkpoint_cache_covered_by(&original));
        let mut restored = MutationLedger::default();
        restored.restore_checkpoint_cache(&cached, true);
        restored.restore_unresolved_tool_effects(debt.unresolved, &BTreeSet::new());
        assert_incomplete_without_cache_gap(&restored);
    }
}

#[test]
fn exact_original_unknown_and_incomplete_receipts_prove_cached_uncertainty() {
    let prepared = preparation("original-unknown");
    let unknown = ToolExecutionResultV1::unknown(&prepared, "original process died").unwrap();
    let mut incomplete = result(&prepared, ToolExecutionOutcome::Failed);
    incomplete.effects.completeness = ToolObservationCompleteness::Incomplete;
    incomplete.effects.reason = Some("original observation was partial".into());
    incomplete.effects.unresolved_external_operations = vec!["opaque exact operation".into()];
    for receipt in [unknown, incomplete] {
        let serialized = serde_json::to_value(&receipt).unwrap();
        let debt = fold_cache_evidence(&[prepared_fact(&prepared), result_fact(&receipt)]);
        assert!(!debt.integrity_invalid);
        assert!(debt
            .validated_uncertainty
            .contains(&prepared.identity.key().unwrap()));
        assert_eq!(
            serde_json::to_value(&debt.results[0].1).unwrap(),
            serialized
        );
        let original = project_receipts(&debt);
        let mut cached = MutationLedger::default();
        cached.observe_tool_receipt("bash", &receipt);
        let cached: MutationLedger =
            serde_json::from_value(serde_json::to_value(cached).unwrap()).unwrap();
        assert!(cached.checkpoint_cache_covered_by(&original));
        let mut restored = MutationLedger::default();
        restored.restore_checkpoint_cache(&cached, true);
        restored.restore_unresolved_tool_effects(debt.unresolved, &BTreeSet::new());
        assert_incomplete_without_cache_gap(&restored);
    }
}

#[test]
fn selected_prefix_never_imports_future_or_untyped_uncertainty() {
    let write = preparation("write-before-boundary");
    let written = result(&write, ToolExecutionOutcome::Succeeded);
    let pending = preparation("prepared-before-boundary");
    let verify = preparation("selected-boundary");
    let verified =
        result_without_execution(&verify, ToolExecutionOutcome::Succeeded, "ok".into()).unwrap();
    let future = preparation("future-unknown");
    let unknown = ToolExecutionResultV1::unknown(&future, "future process died").unwrap();
    let facts = vec![
        prepared_fact(&write),
        result_fact(&written),
        prepared_fact(&pending),
        prepared_fact(&verify),
        result_fact(&verified),
        prepared_fact(&future),
        result_fact(&unknown),
        fact(
            "tool.result",
            "legacy",
            json!({"ok":false,"output":"untyped"}),
        ),
    ];
    let debt = fold_cache_evidence(&facts);
    assert!(
        debt.integrity_invalid,
        "untyped provenance still fails closed"
    );
    let selected = &debt.projection_order[1];
    assert_eq!(
        debt.validated_uncertainty_for(selected),
        BTreeSet::from([pending.identity.key().unwrap()])
    );
    assert!(debt
        .validated_uncertainty
        .contains(&future.identity.key().unwrap()));
    assert!(debt.unresolved.len() > debt.validated_uncertainty.len());
}

#[test]
fn cached_preparation_after_last_receipt_requires_the_actual_final_log_boundary() {
    let write = preparation("cached-write");
    let written = result(&write, ToolExecutionOutcome::Succeeded);
    let verify = preparation("cached-verifier");
    let verified =
        result_without_execution(&verify, ToolExecutionOutcome::Succeeded, "ok".into()).unwrap();
    let pending = preparation("cached-C-after-verifier");
    let debt = fold_cache_evidence(&[
        prepared_fact(&write),
        result_fact(&written),
        prepared_fact(&verify),
        result_fact(&verified),
        prepared_fact(&pending),
    ]);
    assert!(!debt.integrity_invalid);
    let mut cached = project_receipts(&debt);
    cached.set_unresolved_tool_effects(BTreeSet::from([pending.identity.key().unwrap()]));
    let receipt_boundary = debt.projection_order.last().unwrap();
    assert!(!cached
        .checkpoint_uncertainty_keys()
        .is_subset(&debt.validated_uncertainty_for(receipt_boundary)));
    assert!(cached
        .checkpoint_uncertainty_keys()
        .is_subset(&debt.validated_uncertainty));
    let mut original = project_receipts(&debt);
    assert!(!cached.checkpoint_cache_covered_by(&original));
    original.restore_unresolved_tool_effects(debt.validated_uncertainty, &BTreeSet::new());
    assert!(cached.checkpoint_cache_covered_by(&original));
    let mut restored = MutationLedger::default();
    restored.restore_checkpoint_cache(&cached, true);
    assert_incomplete_without_cache_gap(&restored);

    cached.set_unresolved_tool_effects(BTreeSet::from(["invented opaque C".into()]));
    assert!(!cached.checkpoint_cache_covered_by(&original));
}

#[test]
fn purely_negative_cache_uses_original_typed_preparation_without_inventing_receipts() {
    let prepared = preparation("original-prepared-only");
    let debt = fold_cache_evidence(&[prepared_fact(&prepared)]);
    assert!(!debt.integrity_invalid);
    assert!(debt.results.is_empty());
    assert!(debt.projection_order.is_empty());
    let mut cached = MutationLedger::default();
    cached.set_unresolved_tool_effects(BTreeSet::from([prepared.identity.key().unwrap()]));
    assert!(!cached.checkpoint_cache_covered_by(&MutationLedger::default()));
    assert!(cached.checkpoint_negative_cache_covered_by(&debt.validated_uncertainty));
    assert!(!cached.checkpoint_negative_cache_covered_by(&BTreeSet::new()));
    let mut restored = MutationLedger::default();
    restored.restore_checkpoint_cache(&cached, true);
    assert_incomplete_without_cache_gap(&restored);
    assert_eq!(
        cached.checkpoint_receipt_keys(),
        (&BTreeSet::new(), &BTreeSet::new())
    );

    let mut invented = cached.clone();
    invented.set_unresolved_tool_effects(BTreeSet::from(["invented uncertainty".into()]));
    assert!(!invented.checkpoint_negative_cache_covered_by(&debt.validated_uncertainty));
    cached.set_observation_incomplete(true);
    assert!(!cached.checkpoint_negative_cache_covered_by(&debt.validated_uncertainty));
    assert!(!MutationLedger::default()
        .checkpoint_negative_cache_covered_by(&debt.validated_uncertainty));

    let mut positive_claim = MutationLedger::default();
    positive_claim
        .observe_tool_receipt("bash", &result(&prepared, ToolExecutionOutcome::Succeeded));
    positive_claim.set_unresolved_tool_effects(debt.validated_uncertainty.clone());
    assert!(!positive_claim.checkpoint_negative_cache_covered_by(&debt.validated_uncertainty));
}

#[test]
fn later_terminal_cannot_erase_the_original_pending_interval_at_a_verified_byte_prefix() {
    let pending = preparation("C-prepared-before-A");
    let write = preparation("original-A");
    let written = result(&write, ToolExecutionOutcome::Succeeded);
    let verify = preparation("original-V");
    let mut verified =
        result_without_execution(&verify, ToolExecutionOutcome::Succeeded, "verified".into())
            .unwrap();
    let mut cached = MutationLedger::default();
    cached.observe_tool_receipt("bash", &written);
    let report = crate::verification::VerificationReport::new(
        "exact original byte prefix",
        vec![crate::verification::VerificationCheck::required(
            "original-bytes",
            "file",
            "exact observed bytes at A before the later C write",
        )
        .with_status(crate::verification::VerificationStatus::Passed)],
    )
    .with_effect_digest(cached.digest());
    verified.output = runner_result_value(
        "verified original bytes",
        false,
        &[],
        0,
        Some(&json!({"verification_report":report,"verification_author":"host"})),
    );
    cached.observe_tool_receipt("bash", &verified);
    cached.set_unresolved_tool_effects(BTreeSet::from([pending.identity.key().unwrap()]));
    let mut later = result(&pending, ToolExecutionOutcome::Succeeded);
    later.effects.changes[0].after.content_digest =
        Some(format!("sha256:{}", sha256::digest(b"later C bytes")));
    let debt = fold_cache_evidence(&[
        prepared_fact(&pending),
        prepared_fact(&write),
        result_fact(&written),
        prepared_fact(&verify),
        result_fact(&verified),
        result_fact(&later),
    ]);
    assert!(!debt.integrity_invalid);
    assert!(
        debt.validated_uncertainty.is_empty(),
        "the final C is known"
    );
    let boundary = &debt.projection_order[1];
    assert_eq!(
        debt.validated_uncertainty_for(boundary),
        BTreeSet::from([pending.identity.key().unwrap()])
    );
    assert!(debt
        .validated_uncertainty_for(debt.projection_order.last().unwrap())
        .is_empty());
    let mut original_at_v = MutationLedger::default();
    original_at_v.observe_tool_receipt("bash", &written);
    original_at_v.observe_tool_receipt("bash", &verified);
    original_at_v.restore_unresolved_tool_effects(
        debt.validated_uncertainty_for(boundary),
        &BTreeSet::new(),
    );
    assert!(cached.checkpoint_cache_covered_by(&original_at_v));
    assert!(!cached.checkpoint_cache_covered_by(&project_receipts(&debt)));
    let mut restored = MutationLedger::default();
    restored.restore_checkpoint_cache(&cached, true);
    assert_incomplete_without_cache_gap(&restored);
}

#[test]
fn background_terminal_cannot_borrow_a_later_exact_spawn_binding_at_its_earlier_boundary() {
    let facts = background_debt_fixture(false);
    let early_background = vec![facts[2].clone(), facts[0].clone(), facts[1].clone()];
    let debt = fold_cache_evidence(&early_background);
    assert!(
        !debt.integrity_invalid,
        "the full log has a valid exact binding"
    );
    assert!(matches!(
        debt.projection_order[0],
        ReceiptProjection::Background(0)
    ));
    assert!(matches!(
        debt.projection_order[1],
        ReceiptProjection::Tool(0)
    ));
    let claimed = BTreeSet::from([facts[2].key.clone()]);
    assert!(!debt.background_proof_ready_for(&debt.projection_order[0], &claimed));
    assert!(debt.background_proof_ready_for(&debt.projection_order[1], &claimed));
    assert!(!debt.background_proof_ready_for(
        &debt.projection_order[1],
        &BTreeSet::from(["invented background identity".into()]),
    ));

    let original_order = fold_cache_evidence(&facts);
    assert!(!original_order.integrity_invalid);
    assert!(original_order
        .validated_uncertainty_for(&original_order.projection_order[0])
        .contains("workspace-child:one"));
    assert!(!original_order
        .validated_uncertainty_for(original_order.projection_order.last().unwrap())
        .contains("workspace-child:one"));
    assert!(original_order
        .background_proof_ready_for(original_order.projection_order.last().unwrap(), &claimed,));
    let mut incomplete_early = early_background;
    incomplete_early[0].payload["process_settled"] = json!(false);
    let incomplete = fold_cache_evidence(&incomplete_early);
    assert!(!incomplete.integrity_invalid);
    assert!(!incomplete.background_proof_ready_for(&incomplete.projection_order[0], &claimed));
    assert!(incomplete.background_proof_ready_for(&incomplete.projection_order[1], &claimed));
    // A pure-negative cache has no background receipt keys for the parent
    // readiness check, so its uncertainty needs the same proof boundary.
    assert!(
        incomplete.background_proof_ready_for(&incomplete.projection_order[0], &BTreeSet::new())
    );
    assert!(!incomplete
        .validated_uncertainty_for(&incomplete.projection_order[0])
        .contains("background-evidence:one"));
    assert!(incomplete
        .validated_uncertainty_for(&incomplete.projection_order[1])
        .contains("background-evidence:one"));
    assert!(incomplete
        .validated_uncertainty
        .contains("background-evidence:one"));

    let mut wrong_binding = facts;
    wrong_binding[2].payload["source_run"] = json!("wrong-original-source");
    let invalid = fold_cache_evidence(&wrong_binding);
    assert!(invalid.integrity_invalid);
    assert!(
        !invalid.background_proof_ready_for(invalid.projection_order.last().unwrap(), &claimed,)
    );
}

#[test]
fn malformed_and_conflicting_originals_are_integrity_errors_not_honest_unknown() {
    let prepared = preparation("original");
    let terminal = result(&prepared, ToolExecutionOutcome::Succeeded);
    let mut mismatched = terminal.clone();
    mismatched.prepared_digest = format!("sha256:{}", "f".repeat(64));
    let mut conflicting_prepared = prepared.clone();
    conflicting_prepared.original_args_digest =
        a3s_effect::tool_args_digest(&json!({"other":1})).unwrap();
    let mut conflicting_terminal = terminal.clone();
    conflicting_terminal.outcome = ToolExecutionOutcome::Failed;
    for facts in [
        vec![prepared_fact(&prepared), result_fact(&mismatched)],
        vec![result_fact(&terminal)],
        vec![
            prepared_fact(&prepared),
            fact("tool.result", "bad", json!({"receipt":null})),
        ],
        vec![
            prepared_fact(&prepared),
            prepared_fact(&conflicting_prepared),
        ],
        vec![
            prepared_fact(&prepared),
            result_fact(&terminal),
            result_fact(&conflicting_terminal),
        ],
        vec![fact(
            "recovery.unproved",
            "bad",
            json!({"schema":"unknown"}),
        )],
    ] {
        let debt = fold_cache_evidence(&facts);
        assert!(debt.integrity_invalid);
        assert!(!debt.unresolved.is_empty());
    }
}

#[test]
fn typed_background_uncertainty_is_distinct_from_malformed_fields_and_bindings() {
    for (field, value) in [
        ("process_settled", json!(false)),
        ("observation_incomplete", json!(true)),
        ("disposition", json!("unknown")),
        ("process_settlement_basis", json!("unproven")),
    ] {
        let mut facts = background_debt_fixture(false);
        facts[2].payload[field] = value;
        let debt = fold_cache_evidence(&facts);
        assert!(
            !debt.integrity_invalid,
            "valid typed uncertainty in {field}"
        );
        assert!(debt
            .validated_uncertainty
            .contains("background-evidence:one"));
        assert!(debt.unresolved.contains("workspace-child:one"));
    }
    for (field, value) in [
        ("process_settled", json!("false")),
        ("disposition", json!("undefined")),
        ("process_settlement_basis", json!(null)),
        ("source_run", json!("wrong-source")),
        ("run", json!("wrong-effect")),
        ("task_id", json!("wrong-child")),
        ("effects", json!(null)),
        ("changed_paths", json!(["wrong.txt"])),
    ] {
        let mut facts = background_debt_fixture(false);
        facts[2].payload[field] = value;
        let debt = fold_cache_evidence(&facts);
        assert!(
            debt.integrity_invalid,
            "malformed/conflicting evidence in {field}"
        );
        assert!(debt.unresolved.contains("workspace-child:one"));
    }
}
