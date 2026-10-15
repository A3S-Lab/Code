#![cfg(test)]

use super::*;
use crate::harness_loop::{CompletionGate, MutationLedger};
use std::collections::BTreeSet;

#[test]
fn large_late_pure_negative_proof_visits_original_events_once_and_waits_for_actual_tail() {
    const KNOWN_RECEIPTS: usize = 2_048;
    let pending = preparation("exact pending after all known receipts");
    let pending_key = pending.identity.key().unwrap();
    let mut facts = Vec::with_capacity(KNOWN_RECEIPTS * 2 + 1);
    for index in 0..KNOWN_RECEIPTS {
        let prepared = preparation(&format!("known-complete-{index}"));
        let terminal = result_without_execution(
            &prepared,
            ToolExecutionOutcome::Succeeded,
            "known complete zero-effect result".into(),
        )
        .unwrap();
        facts.extend([prepared_fact(&prepared), result_fact(&terminal)]);
    }
    facts.push(prepared_fact(&pending));
    let debt = fold_cache_evidence(&facts);
    assert!(!debt.integrity_invalid);
    assert_eq!(debt.projection_order.len(), KNOWN_RECEIPTS);
    let claimed = BTreeSet::from([pending_key.clone()]);
    let mut proof = debt.cache_proof_cursor(&claimed, &BTreeSet::new());
    for (index, projection) in debt.projection_order.iter().enumerate() {
        assert!(!proof.advance_to(projection), "tail proof has not appeared");
        assert_eq!(proof.processed_events(), (index + 1) * 2);
        let visited = proof.processed_events();
        assert!(!proof.advance_to(projection));
        assert_eq!(proof.processed_events(), visited, "no event is revisited");
    }
    assert!(
        proof.finish(),
        "only the actual end boundary includes the tail"
    );
    assert_eq!(proof.processed_events(), KNOWN_RECEIPTS * 2 + 1);
    assert_eq!(proof.processed_events(), debt.cache_proof_events.len());
    let proven = proof.into_proven_uncertainty();
    assert_eq!(proven, claimed);
    let mut cached = MutationLedger::default();
    cached.set_unresolved_tool_effects(claimed.clone());
    assert!(cached.checkpoint_negative_cache_covered_by(&proven));
    let mut restored = MutationLedger::default();
    restored.restore_checkpoint_cache(&cached, true);
    assert_eq!(restored.checkpoint_uncertainty_keys(), &claimed);
    assert!(!restored
        .checkpoint_uncertainty_keys()
        .contains("tool_effect_cache_evidence_missing"));
    assert!(matches!(
        crate::harness_loop::decide_completion(&restored, &[], &[], false),
        CompletionGate::Incomplete { .. }
    ));

    let mut invented_claim = claimed;
    invented_claim.insert("not asserted by any original fact".into());
    let mut missing = debt.cache_proof_cursor(&invented_claim, &BTreeSet::new());
    assert!(!missing.finish());
    assert_eq!(missing.into_proven_uncertainty(), proven);
}

#[test]
fn proof_cursor_refuses_invalid_originals_and_regressing_projection_boundaries() {
    let prepared = preparation("first");
    let terminal =
        result_without_execution(&prepared, ToolExecutionOutcome::Succeeded, "ok".into()).unwrap();
    let second = preparation("second");
    let next =
        result_without_execution(&second, ToolExecutionOutcome::Succeeded, "ok".into()).unwrap();
    let debt = fold_cache_evidence(&[
        prepared_fact(&prepared),
        result_fact(&terminal),
        prepared_fact(&second),
        result_fact(&next),
    ]);
    let mut proof = debt.cache_proof_cursor(&BTreeSet::new(), &BTreeSet::new());
    assert!(proof.advance_to(&debt.projection_order[1]));
    assert!(!proof.advance_to(&debt.projection_order[0]));
    assert!(!proof.finish(), "invalid traversal cannot regain readiness");

    let invalid = fold_cache_evidence(&[fact("tool.result", "bad", json!({"receipt":null}))]);
    let mut proof = invalid.cache_proof_cursor(&BTreeSet::new(), &BTreeSet::new());
    assert!(
        !proof.finish(),
        "malformed originals never prove even empty claims"
    );
}
