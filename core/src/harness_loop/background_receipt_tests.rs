#![cfg(test)]

use super::*;
use a3s_effect::{ToolEffectObservationV1, ToolFileIdentityV1, ToolFileKind, ToolPathChangeV1};

fn observed(bytes: &[u8], mode: u32) -> ToolEffectObservationV1 {
    ToolEffectObservationV1::complete(vec![ToolPathChangeV1 {
        path: "background.txt".into(),
        before: None,
        after: ToolFileIdentityV1 {
            kind: ToolFileKind::File,
            content_digest: Some(format!("sha256:{}", sha256::digest(bytes))),
            mode: Some(mode),
        },
    }])
}

#[test]
fn background_fact_projection_binds_real_bytes_mode_and_exact_fact_key_once() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("background.txt");
    std::fs::write(&file, b"child wrote bytes").unwrap();
    let effects = observed(&std::fs::read(file).unwrap(), 0o644);
    let mut ledger = MutationLedger::default();
    ledger.observe_background_receipt("background:child:exited", &effects);
    let digest = ledger.digest().to_string();
    ledger.observe_background_receipt("background:child:exited", &effects);
    ledger.observe_background_receipt("background:other:exited", &effects);
    assert_eq!(ledger.digest(), digest);
    assert_eq!(ledger.records.len(), 1);
    assert_eq!(ledger.applied_background_receipts.len(), 2);
    assert!(ledger.applied_tool_receipts.is_empty());
    assert!(ledger.content_digest_matches("background.txt", &sha256::digest(b"child wrote bytes")));
    ledger.observe_background_receipt(
        "background:chmod:exited",
        &observed(b"child wrote bytes", 0o755),
    );
    assert_ne!(ledger.digest(), digest);
}

#[test]
fn incomplete_invalid_and_external_background_observations_remain_debt() {
    let mut ledger = MutationLedger::default();
    let mut incomplete = observed(b"partial bytes", 0o644);
    incomplete.completeness = a3s_effect::ToolObservationCompleteness::Incomplete;
    incomplete.reason = Some("workspace read budget exceeded".into());
    incomplete
        .unresolved_external_operations
        .push("remote-operation:one".into());
    ledger.observe_background_receipt("background:partial", &incomplete);
    let mut invalid = observed(b"bytes", 0o644);
    invalid.changes[0].after.content_digest = Some("invalid-digest".into());
    ledger.observe_background_receipt("background:invalid", &invalid);
    ledger.observe_background_receipt("", &observed(b"bytes", 0o644));
    ledger.restore_unresolved_tool_effects(Default::default(), &Default::default());
    assert!(ledger
        .unresolved_tool_effects
        .contains("background-receipt-incomplete:background:partial"));
    assert!(ledger
        .unresolved_tool_effects
        .contains("background-receipt-invalid:background:invalid"));
    assert!(ledger
        .unresolved_tool_effects
        .contains("background-receipt-identity-missing"));
    assert!(ledger
        .unresolved_tool_effects
        .contains("remote-operation:one"));
    assert!(matches!(
        decide_completion(&ledger, &[], &[], false),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn background_log_can_reproject_after_conservative_checkpoint_restore() {
    let mut cached = MutationLedger::default();
    cached.observe_background_receipt("background:child:exited", &observed(b"cached bytes", 0o644));
    let mut restored = MutationLedger::default();
    restored.restore_checkpoint_cache(&cached, true);
    assert!(restored.applied_background_receipts.is_empty());
    restored.observe_background_receipt(
        "background:child:exited",
        &observed(b"actual committed bytes", 0o644),
    );
    assert!(restored
        .content_digest_matches("background.txt", &sha256::digest(b"actual committed bytes")));
    assert_eq!(restored.records.len(), 1);
}

#[test]
fn full_log_replay_preserves_later_foreground_after_an_unprojected_background() {
    let mut ledger = MutationLedger::default();
    let background = observed(b"older background bytes", 0o644);
    let foreground_effects = observed(b"newer foreground bytes", 0o644);
    let foreground = a3s_effect::ToolExecutionResultV1 {
        schema: a3s_effect::TOOL_RESULT_SCHEMA.into(),
        identity: a3s_effect::ToolInvocationIdentityV1 {
            turn: 1,
            cycle: 0,
            tool_call_id: "later-foreground".into(),
            batch_index: 0,
            admission: None,
        },
        prepared_digest: format!("sha256:{}", "a".repeat(64)),
        outcome: a3s_effect::ToolExecutionOutcome::Succeeded,
        exit_code: Some(0),
        output: serde_json::json!("ok"),
        effects: foreground_effects,
    };
    // The live foreground adapter projected B before the log fold has seen
    // an already committed background terminal A.
    ledger.observe_tool_receipt("write", &foreground);
    let expected = ledger.digest().to_string();
    for _ in 0..2 {
        ledger.begin_receipt_replay();
        ledger.observe_background_receipt("background:older:exited", &background);
        ledger.observe_tool_receipt("write", &foreground);
        assert_eq!(ledger.digest(), expected);
        assert!(ledger
            .content_digest_matches("background.txt", &sha256::digest(b"newer foreground bytes")));
    }
}
