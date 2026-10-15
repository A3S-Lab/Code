#![cfg(test)]

use super::*;
use a3s_effect::MemoryLog;

fn standing() -> NewFact {
    NewFact {
        kind: crate::standing_context::STANDING_CONTEXT_FACT_KIND.into(),
        key: "standing-context".into(),
        payload: serde_json::json!({}),
    }
}

fn policy() -> WorkPolicyV1 {
    WorkPolicyV1::new("engineering", 1, "  Reuse existing code.\n保留检查。\n").unwrap()
}

fn prove_policy_transition(log: &dyn LogStore) {
    let policy = policy();
    let initial = prepare_policy(&log.read("session").unwrap(), "session", Some(&policy)).unwrap();
    assert_eq!(initial.policy.as_ref(), Some(&policy));
    log.append("session", &[standing()], None).unwrap();
    log.append("session", &[initial.pending.unwrap()], None)
        .unwrap();
    let admitted = log.read("session").unwrap();
    assert_eq!(admitted.len(), 2);
    let restored = prepare_policy(&admitted, "session", None).unwrap();
    assert_eq!(restored.policy.unwrap(), policy);
    assert!(restored.pending.is_none());
    let replay = prepare_policy(&admitted, "session", Some(&policy)).unwrap();
    assert!(replay.pending.is_none());
    let changed = WorkPolicyV1::new("engineering", 2, policy.instructions()).unwrap();
    assert!(matches!(
        prepare_policy(&admitted, "session", Some(&changed)),
        Err(WorkPolicyError::Conflict)
    ));
    assert!(prepare_policy(&admitted, "foreign", None).is_err());
    assert_eq!(log.read("session").unwrap(), admitted);
}

#[test]
fn pure_policy_transitions_share_memory_and_file_scenarios_and_actual_reopen() {
    prove_policy_transition(&MemoryLog::new());
    let directory = tempfile::tempdir().unwrap();
    let retained = {
        let log = FileLog::open(directory.path()).unwrap();
        prove_policy_transition(&log);
        log.read("session").unwrap()
    };
    assert_eq!(
        FileLog::open(directory.path())
            .unwrap()
            .read("session")
            .unwrap(),
        retained
    );
}

#[test]
fn corrupt_future_unknown_duplicate_or_out_of_order_facts_refuse() {
    let pending = prepare_policy(&[], "session", Some(&policy()))
        .unwrap()
        .pending
        .unwrap();
    let base = vec![
        Fact {
            seq: 1,
            kind: standing().kind,
            key: standing().key,
            cause: None,
            payload: serde_json::json!({}),
        },
        Fact {
            seq: 2,
            kind: pending.kind,
            key: pending.key,
            cause: None,
            payload: pending.payload,
        },
    ];
    let mut cases = Vec::new();
    let mut bad = base.clone();
    bad[1].payload["schema"] = serde_json::json!("future");
    cases.push(bad);
    let mut bad = base.clone();
    bad[1].payload["digest"] = serde_json::json!("sha256:bad");
    cases.push(bad);
    let mut bad = base.clone();
    bad[1].payload["permissions"] = serde_json::json!("allow");
    cases.push(bad);
    let mut bad = base.clone();
    bad[1].payload["policy"]["provider"] = serde_json::json!("other");
    cases.push(bad);
    let mut bad = base.clone();
    bad[1].kind = "a3s.work-policy.future".into();
    cases.push(bad);
    let mut bad = base.clone();
    bad[1].key = "other-key".into();
    cases.push(bad);
    let mut bad = base.clone();
    bad.push(bad[1].clone());
    cases.push(bad);
    let mut bad = base.clone();
    bad.remove(0);
    cases.push(bad);
    let mut bad = base;
    bad[0].kind = "model.turn".into();
    cases.push(bad);
    for bad in cases {
        assert!(prepare_policy(&bad, "session", None).is_err());
    }
}

#[test]
fn new_policy_cannot_rebind_legacy_history_or_an_existing_goal() {
    for kind in [
        "user.message",
        "model.turn",
        "tool.result",
        "a3s.goal.created",
    ] {
        let log = MemoryLog::new();
        log.append(
            "session",
            &[NewFact {
                kind: kind.into(),
                key: "history".into(),
                payload: serde_json::json!({}),
            }],
            None,
        )
        .unwrap();
        let before = log.read("session").unwrap();
        assert!(matches!(
            prepare_policy(&before, "session", Some(&policy())),
            Err(WorkPolicyError::LegacyRebind)
        ));
        assert!(prepare_policy(&before, "session", None)
            .unwrap()
            .policy
            .is_none());
        assert_eq!(log.read("session").unwrap(), before);
    }
}

#[test]
fn commit_uses_the_exact_original_writer_and_preserves_admission_order() {
    let directory = tempfile::tempdir().unwrap();
    let original = FileLog::open(directory.path()).unwrap();
    let foreign_root = tempfile::tempdir().unwrap();
    let foreign = FileLog::open(foreign_root.path()).unwrap();
    let writer = original.try_writer_fence().unwrap();
    let prepared = prepare_policy(&[], "session", Some(&policy())).unwrap();
    assert!(commit_policy(&original, &writer, "session", &prepared).is_err());
    assert!(original.read("session").unwrap().is_empty());
    assert!(commit_policy(&original, &writer, "foreign", &prepared).is_err());
    assert!(commit_policy(&foreign, &writer, "session", &prepared).is_err());
    assert!(foreign.read("session").unwrap().is_empty());
    original.append("session", &[standing()], None).unwrap();
    commit_policy(&original, &writer, "session", &prepared).unwrap();
    commit_policy(&original, &writer, "session", &prepared).unwrap();
    let facts = original.read("session").unwrap();
    assert_eq!(facts.len(), 2);
    assert_eq!(
        facts[0].kind,
        crate::standing_context::STANDING_CONTEXT_FACT_KIND
    );
    assert_eq!(facts[1].kind, POLICY_KIND);
}
