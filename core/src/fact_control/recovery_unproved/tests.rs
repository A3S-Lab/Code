#![cfg(test)]

use super::*;
use a3s_effect::{FileLog, LogStore, MemoryLog};
use serde_json::{json, Value};

fn checkpoint(session_id: &str) -> LoopCheckpoint {
    LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: "original-source-run".into(),
        session_id: session_id.into(),
        capability_binding: None,
        standing_context_digest: None,
        durable_source: None,
        turn: 1,
        messages: vec![crate::llm::Message::user("compacted source content")],
        total_usage: Default::default(),
        tool_calls_count: 1,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1,
    }
}

fn input(checkpoint: &LoopCheckpoint) -> RecoveryUnprovedV1 {
    RecoveryUnprovedV1::from_checkpoint(
        checkpoint,
        &checkpoint.session_id,
        &super::super::thread_for_session(&checkpoint.session_id),
    )
    .unwrap()
}

fn stored(new: NewFact) -> Fact {
    Fact {
        seq: 7,
        kind: new.kind,
        key: new.key,
        cause: None,
        payload: new.payload,
    }
}

fn invalid_fact(payload: Value) -> RecoveryUnprovedError {
    let mut fact = stored(input(&checkpoint("source-session")).into_fact().unwrap());
    fact.payload = payload;
    RecoveryUnprovedV1::validate_stored_fact(&fact).unwrap_err()
}

fn binding() -> crate::capability::RunCapabilityBindingV1 {
    use crate::capability::{
        CapabilityCeiling, CapabilityExecutionCeiling, CapabilitySet, GovernanceCapabilityCeiling,
        RunCapabilityBindingV1, WorkspaceCapabilityCeiling,
    };
    let set = CapabilitySet::empty().unwrap();
    let ceiling = CapabilityCeiling::all(
        &set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        CapabilityExecutionCeiling::new(8, 4, None, None, None).unwrap(),
    )
    .unwrap();
    RunCapabilityBindingV1::from_set_and_ceiling(&set, &ceiling).unwrap()
}

#[test]
fn exact_payload_round_trips_without_execution_or_verification_claims() {
    let checkpoint = checkpoint("source-session");
    let value = input(&checkpoint);
    let expected = SessionLogicalResumeEvidenceV1::from_checkpoint(&checkpoint).unwrap();
    assert_eq!(
        value.source_checkpoint_content_digest,
        expected.content_digest
    );
    let new = value.clone().into_fact().unwrap();
    assert_eq!(new.kind, "recovery.unproved");
    assert_eq!(new.key, value.key().unwrap());
    assert_eq!(
        new.key,
        format!(
            "recovery-unproved:{}",
            canonical_identity_digest(RECOVERY_UNPROVED_IDENTITY_DOMAIN, &new.payload).unwrap()
        )
    );
    assert_eq!(
        new.payload,
        json!({
            "schema":"a3s.code.recovery-unproved.v1",
            "session_id":"source-session",
            "thread_id":"source-session",
            "source_run_id":"original-source-run",
            "source_checkpoint_schema_version":1,
            "source_checkpoint_content_digest":expected.content_digest,
            "source_capability_binding_digest":null,
            "reason":"cache_evidence_missing"
        })
    );
    assert_eq!(
        RecoveryUnprovedV1::validate_stored_fact(&stored(new)).unwrap(),
        value
    );
}

#[test]
fn every_payload_field_is_required_including_nullable_binding() {
    let payload = input(&checkpoint("source-session"))
        .into_fact()
        .unwrap()
        .payload;
    let fields = payload
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 8);
    for field in fields {
        let mut missing = payload.clone();
        missing.as_object_mut().unwrap().remove(&field);
        let error = invalid_fact(missing);
        assert_eq!(
            error.code(),
            "tool_effect_recovery_unproved_invalid",
            "{field}"
        );
        assert!(
            error.to_string().contains("missing field"),
            "{field}: {error}"
        );
    }
}

#[test]
fn unknown_field_and_incorrect_wire_types_refuse() {
    let payload = input(&checkpoint("source-session"))
        .into_fact()
        .unwrap()
        .payload;
    let mut extra = payload.clone();
    extra["target_run_id"] = json!("not-part-of-source-identity");
    assert!(invalid_fact(extra).to_string().contains("unknown field"));
    for (field, wrong) in [
        ("schema", Value::Null),
        ("session_id", json!(42)),
        ("thread_id", json!([])),
        ("source_run_id", Value::Null),
        ("source_checkpoint_schema_version", json!("1")),
        ("source_checkpoint_schema_version", json!(1.5)),
        ("source_checkpoint_content_digest", json!({})),
        ("source_capability_binding_digest", json!(false)),
        ("reason", Value::Null),
    ] {
        let mut wrong_payload = payload.clone();
        wrong_payload[field] = wrong;
        assert_eq!(
            invalid_fact(wrong_payload).code(),
            "tool_effect_recovery_unproved_invalid"
        );
    }
}

#[test]
fn unsupported_schema_and_closed_reason_have_named_refusals() {
    let payload = input(&checkpoint("source-session"))
        .into_fact()
        .unwrap()
        .payload;
    let mut future = payload.clone();
    future["schema"] = json!("a3s.code.recovery-unproved.v99");
    assert_eq!(
        invalid_fact(future).code(),
        "tool_effect_recovery_unproved_schema_unsupported"
    );
    let reason = RecoveryUnprovedReasonV1::CacheEvidenceMissing;
    assert_eq!(
        RecoveryUnprovedReasonV1::parse(reason.as_str()).unwrap(),
        reason
    );
    for wrong in [
        "unknown",
        "CacheEvidenceMissing",
        " cache_evidence_missing",
        "",
    ] {
        assert!(RecoveryUnprovedReasonV1::parse(wrong)
            .unwrap_err()
            .to_string()
            .contains("RecoveryUnprovedReasonV1"));
        let mut wrong_payload = payload.clone();
        wrong_payload["reason"] = json!(wrong);
        assert!(invalid_fact(wrong_payload)
            .to_string()
            .contains("RecoveryUnprovedReasonV1"));
    }
}

#[test]
fn construction_checks_exact_source_session_and_derived_thread() {
    let checkpoint = checkpoint("source-session");
    for (session, thread) in [
        ("another-session", "another-session"),
        ("source-session", "another-thread"),
        (" source-session", " source-session"),
        ("", ""),
    ] {
        assert!(RecoveryUnprovedV1::from_checkpoint(&checkpoint, session, thread).is_err());
    }
    let exact = " 源 Session \\ exact ";
    let unicode_checkpoint = self::checkpoint(exact);
    let exact_thread = super::super::thread_for_session(exact);
    let value =
        RecoveryUnprovedV1::from_checkpoint(&unicode_checkpoint, exact, &exact_thread).unwrap();
    assert_eq!(value.session_id, exact);
    assert_eq!(value.thread_id, exact_thread);
    assert!(
        RecoveryUnprovedV1::from_checkpoint(&unicode_checkpoint, exact.trim(), &exact_thread)
            .is_err()
    );
    assert!(RecoveryUnprovedV1::from_checkpoint(
        &unicode_checkpoint,
        "源 Session / exact",
        &exact_thread
    )
    .is_err());
}

#[test]
fn empty_unbounded_control_and_mismatched_stored_identities_refuse() {
    let original = input(&checkpoint("source-session"));
    for field in ["session_id", "thread_id", "source_run_id"] {
        for invalid in [
            String::new(),
            " ".into(),
            "x".repeat(257),
            "bad\nidentity".into(),
        ] {
            let mut value = original.clone();
            match field {
                "session_id" => value.session_id = invalid,
                "thread_id" => value.thread_id = invalid,
                _ => value.source_run_id = invalid,
            }
            assert!(value.into_fact().is_err(), "{field}");
        }
    }
    let mut mismatched = original;
    mismatched.thread_id = "other-exact-thread".into();
    assert!(mismatched.into_fact().is_err());
}

#[test]
fn stored_kind_key_and_transition_cause_cannot_be_reassigned() {
    let original = stored(input(&checkpoint("source-session")).into_fact().unwrap());
    let mut wrong_kind = original.clone();
    wrong_kind.kind = "tool.result".into();
    assert!(RecoveryUnprovedV1::validate_stored_fact(&wrong_kind).is_err());
    let mut wrong_key = original.clone();
    wrong_key.key.push('0');
    assert!(RecoveryUnprovedV1::validate_stored_fact(&wrong_key).is_err());
    let mut transition = original.clone();
    transition.cause = Some("infer:1:0".into());
    assert!(RecoveryUnprovedV1::validate_stored_fact(&transition).is_err());
    let mut rewritten_payload = original;
    rewritten_payload.payload["source_run_id"] = json!("another-source-run");
    assert!(RecoveryUnprovedV1::validate_stored_fact(&rewritten_payload).is_err());
}

#[test]
fn invalid_digest_and_unloadable_source_schema_refuse() {
    let original = input(&checkpoint("source-session"));
    for digest in [
        "sha256:UPPER".into(),
        format!("sha256:{}", "A".repeat(64)),
        "f".repeat(64),
    ] {
        let mut content = original.clone();
        content.source_checkpoint_content_digest = digest.clone();
        assert!(content.into_fact().is_err());
        let mut binding = original.clone();
        binding.source_capability_binding_digest = Some(digest);
        assert!(binding.into_fact().is_err());
    }
    let mut future = original;
    future.source_checkpoint_schema_version = LOOP_CHECKPOINT_SCHEMA_VERSION + 1;
    assert!(future.into_fact().is_err());
    let mut future_checkpoint = checkpoint("source-session");
    future_checkpoint.schema_version += 1;
    assert!(RecoveryUnprovedV1::from_checkpoint(
        &future_checkpoint,
        "source-session",
        "source-session"
    )
    .is_err());
}

#[test]
fn actual_frozen_binding_is_retained_and_never_inferred() {
    let mut checkpoint = checkpoint("source-session");
    let absent = input(&checkpoint);
    assert_eq!(absent.source_capability_binding_digest, None);
    checkpoint.capability_binding = Some(binding());
    let present = input(&checkpoint);
    assert_eq!(
        present.source_capability_binding_digest,
        Some(
            crate::content_digest::digest_json(
                SOURCE_BINDING_DIGEST_DOMAIN,
                checkpoint.capability_binding.as_ref().unwrap()
            )
            .unwrap()
        )
    );
    assert_ne!(present.key().unwrap(), absent.key().unwrap());
    let mut malformed =
        serde_json::to_value(checkpoint.capability_binding.as_ref().unwrap()).unwrap();
    malformed["catalogDigest"] = json!("sha256:bad");
    checkpoint.capability_binding = Some(serde_json::from_value(malformed).unwrap());
    assert!(
        RecoveryUnprovedV1::from_checkpoint(&checkpoint, "source-session", "source-session")
            .is_err()
    );
}

#[test]
fn legacy_content_identity_does_not_upgrade_schema_binding_or_portable_admission() {
    let mut checkpoint = checkpoint("source-session");
    checkpoint.schema_version = 0;
    checkpoint.turn = 0;
    let original = serde_json::to_value(&checkpoint).unwrap();
    let value = input(&checkpoint);
    assert_eq!(value.source_checkpoint_schema_version, 0);
    assert_eq!(value.source_capability_binding_digest, None);
    assert_eq!(
        value.source_checkpoint_content_digest,
        loop_checkpoint_content_identity(&checkpoint).unwrap().1
    );
    assert!(SessionLogicalResumeEvidenceV1::from_checkpoint(&checkpoint).is_err());
    assert_eq!(serde_json::to_value(checkpoint).unwrap(), original);
}

#[test]
fn portable_v1_still_requires_a_real_completed_tool_round_boundary() {
    let mut checkpoint = checkpoint("source-session");
    checkpoint.turn = 0;
    assert!(
        RecoveryUnprovedV1::from_checkpoint(&checkpoint, "source-session", "source-session")
            .is_err()
    );
}

#[test]
fn source_content_changes_identity_but_object_order_does_not() {
    let checkpoint = checkpoint("source-session");
    let original = input(&checkpoint);
    let fact = original.clone().into_fact().unwrap();
    let mut reordered = fact.payload.as_object().unwrap().iter().collect::<Vec<_>>();
    reordered.reverse();
    let text = format!(
        "{{{}}}",
        reordered
            .iter()
            .map(|(key, value)| format!("{}:{}", serde_json::to_string(key).unwrap(), value))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut stored_fact = stored(fact);
    stored_fact.payload = serde_json::from_str(&text).unwrap();
    assert_eq!(
        RecoveryUnprovedV1::validate_stored_fact(&stored_fact)
            .unwrap()
            .key()
            .unwrap(),
        original.key().unwrap()
    );
    let mut changed = checkpoint;
    changed
        .messages
        .push(crate::llm::Message::assistant("different source content"));
    assert_ne!(input(&changed).key().unwrap(), original.key().unwrap());
    changed.run_id = "another-source-run".into();
    assert_ne!(input(&changed).key().unwrap(), original.key().unwrap());
}

fn log_round_trip(log: &dyn LogStore) {
    let checkpoint = checkpoint("source-session");
    let first = input(&checkpoint).into_fact().unwrap();
    let same_source = input(&checkpoint).into_fact().unwrap();
    assert_eq!(first, same_source);
    let committed = log
        .append("source-session", std::slice::from_ref(&first), None)
        .unwrap();
    assert_eq!(committed.len(), 1);
    assert_eq!(
        log.append("source-session", &[same_source], None).unwrap(),
        committed
    );
    assert_eq!(
        RecoveryUnprovedV1::validate_stored_fact(&committed[0])
            .unwrap()
            .key()
            .unwrap(),
        first.key
    );
    let mut incompatible = first;
    incompatible.payload["source_run_id"] = json!("conflicting-source");
    assert!(log.append("source-session", &[incompatible], None).is_err());
    assert_eq!(log.read("source-session").unwrap(), committed);
}

#[test]
fn negative_contract_is_idempotent_on_memory_and_reopened_original_file_log() {
    log_round_trip(&MemoryLog::new());
    let directory = tempfile::tempdir().unwrap();
    log_round_trip(&FileLog::open(directory.path()).unwrap());
    let reopened = FileLog::open(directory.path()).unwrap();
    let facts = reopened.read("source-session").unwrap();
    assert_eq!(facts.len(), 1);
    let value = RecoveryUnprovedV1::validate_stored_fact(&facts[0]).unwrap();
    assert_eq!(value, input(&checkpoint("source-session")));
    assert_eq!(facts[0].kind, RECOVERY_UNPROVED_FACT_KIND);
    assert!(facts
        .iter()
        .all(|fact| fact.kind != "tool.prepared" && fact.kind != "tool.result"));
}
