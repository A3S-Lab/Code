#![cfg(test)]

use super::*;
use crate::verification::VerificationCheck;
use a3s_effect::{
    ToolEffectObservationV1, ToolExecutionOutcome, ToolExecutionResultV1, ToolFileIdentityV1,
    ToolFileKind, ToolInvocationIdentityV1, ToolPathChangeV1, TOOL_RESULT_SCHEMA,
};
use serde_json::json;

fn file_identity(bytes: &[u8], mode: u32) -> ToolFileIdentityV1 {
    ToolFileIdentityV1 {
        kind: ToolFileKind::File,
        content_digest: Some(format!("sha256:{}", sha256::digest(bytes))),
        mode: Some(mode),
    }
}

fn receipt(id: &str, path: &str, after: ToolFileIdentityV1) -> ToolExecutionResultV1 {
    ToolExecutionResultV1 {
        schema: TOOL_RESULT_SCHEMA.into(),
        identity: ToolInvocationIdentityV1 {
            turn: 1,
            cycle: 0,
            tool_call_id: id.into(),
            batch_index: 0,
            admission: None,
        },
        prepared_digest: format!("sha256:{}", "a".repeat(64)),
        outcome: ToolExecutionOutcome::Succeeded,
        exit_code: Some(0),
        output: json!("observed"),
        effects: ToolEffectObservationV1::complete(vec![ToolPathChangeV1 {
            path: path.into(),
            before: None,
            after,
        }]),
    }
}

#[test]
fn repeated_batch_observation_keeps_a_real_file_verification_bound() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("changed.txt");
    std::fs::write(&file, b"verified file bytes").unwrap();
    let bytes = std::fs::read(&file).unwrap();
    let after = file_identity(&bytes, 0o644);
    let mut ledger = MutationLedger::default();
    ledger.observe_tool_receipt("write", &receipt("write", "changed.txt", after.clone()));
    let digest = ledger.digest().to_string();
    let report = VerificationReport::new(
        "actual file check",
        vec![
            VerificationCheck::required("read-back", "command", "file bytes match")
                .with_status(VerificationStatus::Passed),
        ],
    )
    .with_effect_digest(&digest);
    assert_eq!(std::fs::read(&file).unwrap(), bytes);

    // All calls prepare before the batch runs. The verification watch can
    // report the first call's after-state again; that is one workspace state.
    let observed_again = file_identity(&std::fs::read(&file).unwrap(), 0o644);
    let verification = receipt("verification", "changed.txt", observed_again);
    ledger.observe_tool_receipt("bash", &verification);
    ledger.observe_tool_receipt("bash", &verification);
    assert_eq!(ledger.digest(), digest);
    assert_eq!(ledger.records.len(), 1);
    assert_eq!(ledger.applied_tool_receipts.len(), 2);
    assert!(matches!(decide_completion(&ledger, &[report], &[], false),
        CompletionGate::Allow(CompletionTerminal::Verified { effect_digest }) if effect_digest == digest));
}

#[test]
fn file_type_mode_and_bytes_each_bind_the_effect_identity() {
    let variants = [
        file_identity(b"same bytes", 0o644),
        file_identity(b"same bytes", 0o755),
        file_identity(b"different bytes", 0o644),
        ToolFileIdentityV1 {
            kind: ToolFileKind::Missing,
            content_digest: None,
            mode: None,
        },
        ToolFileIdentityV1 {
            kind: ToolFileKind::Directory,
            content_digest: None,
            mode: Some(0o755),
        },
        ToolFileIdentityV1 {
            kind: ToolFileKind::Symlink,
            content_digest: Some(format!("sha256:{}", sha256::digest(b"same bytes"))),
            mode: Some(0o644),
        },
    ];
    let mut identities = std::collections::BTreeSet::new();
    for (index, after) in variants.into_iter().enumerate() {
        let mut ledger = MutationLedger::default();
        ledger.observe_tool_receipt("tool", &receipt(&index.to_string(), "path", after.clone()));
        assert!(
            identities.insert(ledger.digest().to_string()),
            "kind/mode/content must each affect the digest"
        );
        assert_eq!(ledger.records[0].after_identity.as_ref(), Some(&after));
        if after.kind == ToolFileKind::File {
            assert!(ledger.content_digest_matches(
                "path",
                after
                    .content_digest
                    .as_ref()
                    .unwrap()
                    .strip_prefix("sha256:")
                    .unwrap()
            ));
        }
    }
}

#[test]
fn authoritative_after_state_supersedes_stale_bytes_and_survives_cache_round_trip() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool_receipt(
        "write",
        &receipt("first", "path", file_identity(b"first", 0o644)),
    );
    ledger.observe_tool_receipt(
        "write",
        &receipt("second", "path", file_identity(b"second", 0o644)),
    );
    assert!(!ledger.content_digest_matches("path", &sha256::digest(b"first")));
    assert!(ledger.content_digest_matches("path", &sha256::digest(b"second")));
    let deletion = receipt(
        "delete",
        "path",
        ToolFileIdentityV1 {
            kind: ToolFileKind::Missing,
            content_digest: None,
            mode: None,
        },
    );
    ledger.observe_tool_receipt("delete", &deletion);
    assert!(!ledger.has_content_digest("path"));
    assert_eq!(ledger.records.len(), 1);
    let digest = ledger.digest().to_string();
    let mut restored: MutationLedger =
        serde_json::from_value(serde_json::to_value(&ledger).unwrap()).unwrap();
    restored.observe_tool_receipt("delete", &deletion);
    assert_eq!(restored, ledger);
    assert_eq!(restored.digest(), digest);
}

#[test]
fn observer_provenance_and_receipt_replay_order_do_not_change_workspace_identity() {
    let after = file_identity(b"one after-state", 0o644);
    let write = receipt("write", "path", after.clone());
    let verify = receipt("verify", "path", after);
    let mut first = MutationLedger::default();
    first.observe_tool_receipt("write", &write);
    first.observe_tool_receipt("bash", &verify);
    let mut second = MutationLedger::default();
    second.observe_tool_receipt("bash", &verify);
    second.observe_tool_receipt("write", &write);
    assert_eq!(first.digest(), second.digest());
    assert_eq!(first.records.len(), 1);
    assert_eq!(second.records.len(), 1);
}

#[test]
fn legacy_cache_retains_its_original_digest_until_typed_observation() {
    let bytes = sha256::digest(b"legacy bytes");
    let digest = sha256::digest(format!("write|path|{bytes}"));
    let mut ledger: MutationLedger = serde_json::from_value(json!({
        "records":[{"tool":"write", "path":"path", "content_digest":bytes}], "digest":digest,
    }))
    .unwrap();
    assert!(ledger.records[0].after_identity.is_none());
    ledger.rehash();
    assert_eq!(ledger.digest(), digest);
    ledger.observe_tool_receipt(
        "write",
        &receipt("typed", "path", file_identity(b"legacy bytes", 0o644)),
    );
    assert_eq!(ledger.records.len(), 1);
    assert_ne!(ledger.digest(), digest);
    assert!(ledger.content_digest_matches("path", &sha256::digest(b"legacy bytes")));
}

#[test]
fn typed_projection_preserves_real_space_unicode_control_and_backslash_paths() {
    let root = tempfile::tempdir().unwrap();
    let names = [
        " padded ",
        "padded",
        "数据.txt",
        "line\nbreak",
        "tab\tname",
        #[cfg(unix)]
        r"literal\name",
        #[cfg(unix)]
        r".a3s\legal-file",
    ];
    let mut ledger = MutationLedger::default();
    for (index, path) in names.iter().enumerate() {
        let bytes = format!("distinct actual bytes {index}");
        std::fs::write(root.path().join(path), bytes.as_bytes()).unwrap();
        let actual = std::fs::read(root.path().join(path)).unwrap();
        let result = receipt(
            &format!("exact-{index}"),
            path,
            file_identity(&actual, 0o644),
        );
        result.validate().unwrap();
        ledger.observe_tool_receipt("write", &result);
    }
    assert_eq!(ledger.records.len(), names.len());
    for (index, path) in names.iter().enumerate() {
        let record = ledger
            .records
            .iter()
            .find(|record| record.path == *path)
            .unwrap();
        assert_eq!(record.path.as_bytes(), path.as_bytes());
        assert_eq!(
            record.content_digest,
            sha256::digest(format!("distinct actual bytes {index}"))
        );
    }
    let path = " unknown path\n ";
    let mut unknown = receipt(
        "unknown-exact",
        path,
        ToolFileIdentityV1 {
            kind: ToolFileKind::Unknown,
            content_digest: None,
            mode: None,
        },
    );
    unknown.outcome = ToolExecutionOutcome::Unknown;
    unknown.effects.completeness = a3s_effect::ToolObservationCompleteness::Incomplete;
    unknown.effects.reason = Some("interrupted".into());
    unknown.validate().unwrap();
    ledger.observe_tool_receipt("write", &unknown);
    assert!(ledger
        .records
        .iter()
        .any(|record| record.path.as_bytes() == path.as_bytes()));
    assert!(ledger.has_uncertain_tool_invocations());
}
