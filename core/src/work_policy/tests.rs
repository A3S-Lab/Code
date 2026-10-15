#![cfg(test)]

use super::*;

#[test]
fn original_instruction_bytes_round_trip_without_normalization() {
    let body = "  Prefer reuse.\n保留验证。\n";
    let policy = WorkPolicyV1::new("工程", 7, body).unwrap();
    let decoded: WorkPolicyV1 =
        serde_json::from_slice(&serde_json::to_vec(&policy).unwrap()).unwrap();
    assert_eq!(decoded, policy);
    assert_eq!(decoded.id(), "工程");
    assert_eq!(decoded.revision(), 7);
    assert_eq!(decoded.instructions(), body);
}

#[test]
fn digest_has_a_fixed_cross_language_reference() {
    let policy = WorkPolicyV1::new("engineering", 1, "Prefer reuse.").unwrap();
    assert_eq!(
        policy.digest().unwrap(),
        "sha256:438a80b73efc8defe9aacad62edc2a22ce881923c5bf76638ce5f8bc80380404"
    );
}

#[test]
fn every_bound_field_changes_identity() {
    let policy = WorkPolicyV1::new("engineering", 1, "Prefer reuse.").unwrap();
    let digest = policy.digest().unwrap();
    for changed in [
        WorkPolicyV1::new("other", 1, "Prefer reuse.").unwrap(),
        WorkPolicyV1::new("engineering", 2, "Prefer reuse.").unwrap(),
        WorkPolicyV1::new("engineering", 1, "Prefer reuse. ").unwrap(),
    ] {
        assert_ne!(changed.digest().unwrap(), digest);
    }
    let other_domain =
        crate::content_digest::digest_json("other.v1", &(WORK_POLICY_SCHEMA, &policy)).unwrap();
    assert_ne!(other_domain, digest);
}

#[test]
fn id_limit_counts_utf8_bytes_and_refuses_control_characters() {
    assert!(WorkPolicyV1::new("x".repeat(128), 1, "guide").is_ok());
    assert!(WorkPolicyV1::new("中".repeat(42), 1, "guide").is_ok());
    for id in [
        String::new(),
        "x".repeat(129),
        "中".repeat(43),
        "line\nbreak".into(),
        "nul\0id".into(),
    ] {
        assert_eq!(
            WorkPolicyV1::new(id, 1, "guide").unwrap_err().refusal(),
            WorkPolicyRefusal::Invalid
        );
    }
}

#[test]
fn instruction_limit_counts_utf8_bytes_without_truncation() {
    assert!(WorkPolicyV1::new("p", 1, "x".repeat(4096)).is_ok());
    assert!(WorkPolicyV1::new("p", 1, "中".repeat(1365)).is_ok());
    for body in [
        "x".repeat(4097),
        "中".repeat(1366),
        String::new(),
        " \n\t\u{2003}".into(),
        "text\0tail".into(),
    ] {
        assert!(WorkPolicyV1::new("p", 1, body).is_err());
    }
    assert!(WorkPolicyV1::new("p", 0, "guide").is_err());
    assert!(WorkPolicyV1::new("p", u32::MAX, "guide").is_ok());
}

#[test]
fn untrusted_wire_cannot_bypass_validation_or_add_authority() {
    for input in [
        serde_json::json!({"id":"p","revision":0,"instructions":"guide"}),
        serde_json::json!({"id":"p","revision":1,"instructions":" "}),
        serde_json::json!({"id":"p","revision":1,"instructions":"text\0tail"}),
        serde_json::json!({"id":"p","revision":1,"instructions":"guide","permissions":"allow"}),
        serde_json::json!({"id":"p","revision":1,"instructions":"guide","provider":"other"}),
        serde_json::json!({"id":"p","revision":1}),
        serde_json::json!({"id":"p","revision":-1,"instructions":"guide"}),
    ] {
        assert!(serde_json::from_value::<WorkPolicyV1>(input).is_err());
    }
}

#[test]
fn debug_does_not_disclose_instruction_body() {
    let policy = WorkPolicyV1::new("p", 1, "private-host-guidance").unwrap();
    let debug = format!("{policy:?}");
    assert!(!debug.contains(policy.instructions()));
    assert!(debug.contains("instructions_bytes"));
}

#[test]
fn refusal_closed_set_round_trips_and_names_unknown_values() {
    for code in [
        WorkPolicyRefusal::Invalid,
        WorkPolicyRefusal::Conflict,
        WorkPolicyRefusal::Missing,
        WorkPolicyRefusal::Corrupt,
        WorkPolicyRefusal::LegacyRebind,
    ] {
        assert_eq!(WorkPolicyRefusal::parse(code.as_str()).unwrap(), code);
    }
    for unknown in ["", "invalid", "WORK_POLICY.INVALID", "work_policy.allow"] {
        let error = WorkPolicyRefusal::parse(unknown).unwrap_err();
        assert!(error.to_string().contains("WorkPolicyRefusal"));
    }
}
