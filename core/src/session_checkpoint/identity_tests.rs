#![cfg(test)]

use super::*;
use serde_json::json;

fn checkpoint() -> LoopCheckpoint {
    LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: "source-run".into(),
        session_id: "source-session".into(),
        capability_binding: None,
        standing_context_digest: None,
        durable_source: None,
        turn: 1,
        messages: vec![crate::llm::Message::user("exact source content")],
        total_usage: Default::default(),
        tool_calls_count: 1,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1,
    }
}

#[test]
fn source_content_identity_is_the_portable_v1_canonical_identity() {
    let checkpoint = checkpoint();
    let (size, digest) = loop_checkpoint_content_identity(&checkpoint).unwrap();
    let evidence = SessionLogicalResumeEvidenceV1::from_checkpoint(&checkpoint).unwrap();
    evidence.validate_for(&checkpoint).unwrap();
    assert_eq!(size, evidence.size_bytes);
    assert_eq!(digest, evidence.content_digest);
    let canonical = codec::canonical_json_bytes(&checkpoint).unwrap();
    assert_eq!(size, canonical.len() as u64);
    assert_eq!(digest, format!("sha256:{}", sha256::digest(canonical)));
}

#[test]
fn legacy_identity_only_path_leaves_portable_admission_and_input_unchanged() {
    let mut checkpoint = checkpoint();
    checkpoint.schema_version = 0;
    checkpoint.turn = 0;
    let before = serde_json::to_value(&checkpoint).unwrap();
    let (size, digest) = loop_checkpoint_content_identity(&checkpoint).unwrap();
    assert!(size > 0);
    assert_eq!(
        digest,
        codec::content_digest(&codec::canonical_json_bytes(&checkpoint).unwrap())
    );
    assert!(SessionLogicalResumeEvidenceV1::from_checkpoint(&checkpoint).is_err());
    assert_eq!(checkpoint.schema_version, 0);
    assert_eq!(checkpoint.capability_binding, None);
    assert_eq!(serde_json::to_value(checkpoint).unwrap(), before);
}

#[test]
fn source_identity_refuses_future_schema_and_invalid_existing_identity_fields() {
    let original = checkpoint();
    let mut future = original.clone();
    future.schema_version = LOOP_CHECKPOINT_SCHEMA_VERSION + 1;
    assert!(loop_checkpoint_content_identity(&future).is_err());
    for field in ["session", "run"] {
        for invalid in [String::new(), " ".into(), "x".repeat(257), "bad\0id".into()] {
            let mut value = original.clone();
            if field == "session" {
                value.session_id = invalid;
            } else {
                value.run_id = invalid;
            }
            assert!(loop_checkpoint_content_identity(&value).is_err());
        }
    }
    let mut invalid_binding = original;
    invalid_binding.standing_context_digest = Some("sha256:not-a-digest".into());
    assert!(loop_checkpoint_content_identity(&invalid_binding).is_err());
}

#[test]
fn canonical_digest_reuses_codec_ordering_and_domain_separation() {
    #[derive(Serialize)]
    struct ReverseFieldOrder {
        z: serde_json::Value,
        a: serde_json::Value,
    }
    let left: serde_json::Value = serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":[2,1]}"#).unwrap();
    let right: serde_json::Value =
        serde_json::from_str(r#"{"a":[2,1],"z":{"a":1,"b":2}}"#).unwrap();
    let domain = "a3s.code.recovery-unproved.identity.v1";
    let digest = canonical_identity_digest(domain, &left).unwrap();
    assert_eq!(digest, canonical_identity_digest(domain, &right).unwrap());
    assert_eq!(digest, codec::domain_digest(domain, &left).unwrap());
    // Struct serialization preserves declaration order. This catches using
    // ordinary digest_json instead of the existing canonical codec facade.
    let reverse = ReverseFieldOrder {
        z: json!({"b":2,"a":1}),
        a: json!([2, 1]),
    };
    assert_eq!(digest, canonical_identity_digest(domain, &reverse).unwrap());
    assert_ne!(
        digest,
        crate::content_digest::digest_json(domain, &reverse).unwrap()
    );
    assert_ne!(
        digest,
        canonical_identity_digest("another.domain.v1", &left).unwrap()
    );
    assert_ne!(
        digest,
        canonical_identity_digest(domain, &json!({"a":[1,2],"z":{"a":1,"b":2}})).unwrap()
    );
}

#[test]
fn canonical_source_digest_binds_content_and_does_not_normalize_exact_ids() {
    let original = checkpoint();
    let expected = loop_checkpoint_content_identity(&original).unwrap().1;
    let mut changed = original.clone();
    changed
        .messages
        .push(crate::llm::Message::assistant("changed source content"));
    assert_ne!(
        loop_checkpoint_content_identity(&changed).unwrap().1,
        expected
    );
    let mut spaced = original.clone();
    spaced.session_id = " source-session ".into();
    assert_ne!(
        loop_checkpoint_content_identity(&spaced).unwrap().1,
        expected
    );
    let mut unicode = original;
    unicode.session_id = "源 Session \\ exact".into();
    let identity = loop_checkpoint_content_identity(&unicode).unwrap();
    assert_ne!(identity.1, expected);
    assert_eq!(unicode.session_id, "源 Session \\ exact");
}

#[test]
fn durable_portable_snapshot_and_loop_must_bind_the_same_original_source() {
    let mut loop_state = checkpoint();
    let facts = [a3s_effect::Fact {
        seq: 1,
        kind: "a3s.work-policy.admitted".into(),
        key: "source".into(),
        cause: None,
        payload: json!({"source":"original"}),
    }];
    let source =
        crate::store::SessionFactSourceV1::from_facts(&loop_state.session_id, &facts).unwrap();
    loop_state.durable_source = source.clone();
    loop_state.schema_version = LoopCheckpoint::schema_for(source.as_ref());
    let mut snapshot = super::tests::minimal_snapshot(&loop_state.session_id);
    snapshot.session.durable_source = source;
    snapshot.schema_version = crate::store::SESSION_SNAPSHOT_SCHEMA_VERSION;
    snapshot.run_records.push(crate::run::RunRecord {
        snapshot: crate::run::RunSnapshot {
            id: loop_state.run_id.clone(),
            session_id: loop_state.session_id.clone(),
            status: crate::run::RunStatus::Executing,
            prompt: "continue".into(),
            cognitive_package_binding: None,
            capability_binding: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            result_text: None,
            error: None,
            event_count: 0,
            workspace_change_set: None,
        },
        events: Vec::new(),
    });
    let export =
        SessionCheckpointExportV1::new(snapshot.clone(), Some(loop_state.clone())).unwrap();
    let payload = export.open().unwrap();
    assert_eq!(payload.snapshot.schema_version, 2);
    assert_eq!(payload.logical_resume.unwrap().schema_version, 2);
    let evidence = SessionLogicalResumeEvidenceV1::from_checkpoint(&loop_state).unwrap();
    evidence.validate_for(&loop_state).unwrap();
    let mut unbound = loop_state.clone();
    unbound.durable_source = None;
    unbound.schema_version = 1;
    assert!(SessionCheckpointExportV1::new(snapshot.clone(), Some(unbound)).is_err());
    let mut foreign = loop_state;
    foreign.durable_source.as_mut().unwrap().sequence += 1;
    assert!(SessionCheckpointExportV1::new(snapshot.clone(), Some(foreign)).is_err());
    snapshot.session.durable_source = None;
    snapshot.schema_version = 1;
    assert!(
        SessionCheckpointExportV1::new(snapshot, export.open().unwrap().logical_resume).is_err()
    );
}
