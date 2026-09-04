use a3s_code_core::evaluation::{
    digest_bytes, AuxiliaryCapabilityProfileV1, AuxiliaryModeV1, AuxiliaryRunOutputV1,
    AuxiliaryRunSnapshotV1, AuxiliaryRunSpecV1, AuxiliaryRunStateV1, EvaluationProtocolError,
    EvaluationRecordV1, EvaluationResultV1, EvaluationWireEnvelopeV1, EvaluationWireKindV1,
    EvidenceReadRequestV1, ExecutionFrameV1, ExecutionTargetV1, RunEvidenceReader,
    EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES, EVALUATION_PROTOCOL_SCHEMA_V1,
    EVALUATION_PROTOCOL_VERSION_V1, EVALUATION_WIRE_KIND_DESCRIPTORS_V1,
};
use a3s_code_core::InMemoryRunStore;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct Manifest {
    schema: String,
    version: u16,
    max_message_bytes: usize,
    kinds: Vec<ManifestKind>,
}

#[derive(Debug, Deserialize)]
struct ManifestKind {
    variant: String,
    constant: String,
    wire_name: String,
    payload_type: String,
}

async fn fixture() -> (
    a3s_code_core::evaluation::EvidenceSnapshotV1,
    ExecutionTargetV1,
    AuxiliaryRunSpecV1,
    AuxiliaryRunOutputV1,
    AuxiliaryRunSnapshotV1,
    EvaluationResultV1,
    EvaluationRecordV1,
) {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id(
            "run-protocol-fixture".to_string(),
            "session-protocol-fixture",
            "protocol prompt",
        )
        .await;
    let target = ExecutionTargetV1::new("session-protocol-fixture", &run.id);
    let evidence = RunEvidenceReader::new(runs)
        .read(EvidenceReadRequestV1::new(target.clone()))
        .await
        .expect("evidence fixture");
    let frame = ExecutionFrameV1::root(target.clone());
    let spec = AuxiliaryRunSpecV1::new(
        frame.clone(),
        "protocol-fixture",
        "return a bounded object",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-protocol-fixture")
    .with_mode(AuxiliaryModeV1::Advisory)
    .with_capabilities(AuxiliaryCapabilityProfileV1::tool_free());
    let value = serde_json::json!({"ok": true});
    let encoded = serde_json::to_vec(&value).expect("output encoding");
    let output = AuxiliaryRunOutputV1 {
        schema: "a3s.code.auxiliary-output.v1".to_string(),
        value,
        output_bytes: encoded.len() as u64,
        output_digest: digest_bytes("a3s.code.auxiliary-output.value.v1", &encoded),
    };
    let snapshot = AuxiliaryRunSnapshotV1 {
        schema: "a3s.code.auxiliary-run-snapshot.v1".to_string(),
        id: spec.id.clone(),
        parent: frame,
        mode: spec.mode,
        state: AuxiliaryRunStateV1::Queued,
        spec_digest: spec.digest().expect("spec digest"),
        created_at_ms: 1,
        updated_at_ms: 1,
        output_digest: None,
        error: None,
    };
    let result = EvaluationResultV1::new(
        "protocol-evaluator",
        target.clone(),
        spec.id.clone(),
        "host-token",
        output.value.clone(),
        evidence.snapshot_digest.clone(),
    )
    .expect("result fixture");
    let record = EvaluationRecordV1::new(result.clone(), 1).expect("record fixture");
    (evidence, target, spec, output, snapshot, result, record)
}

#[tokio::test]
async fn every_catalog_kind_round_trips_through_the_strict_envelope() {
    let (evidence, _target, spec, output, snapshot, result, record) = fixture().await;
    let envelopes = vec![
        EvaluationWireEnvelopeV1::from_evidence_read_request(EvidenceReadRequestV1::new(
            evidence.target.clone(),
        ))
        .expect("request envelope"),
        EvaluationWireEnvelopeV1::from_evidence_snapshot(evidence).expect("snapshot envelope"),
        EvaluationWireEnvelopeV1::from_auxiliary_run_spec(spec).expect("spec envelope"),
        EvaluationWireEnvelopeV1::from_auxiliary_run_snapshot(snapshot)
            .expect("auxiliary snapshot envelope"),
        EvaluationWireEnvelopeV1::from_auxiliary_run_output(output).expect("output envelope"),
        EvaluationWireEnvelopeV1::from_evaluation_result(result).expect("result envelope"),
        EvaluationWireEnvelopeV1::from_evaluation_record(record).expect("record envelope"),
    ];
    let expected = [
        EvaluationWireKindV1::EvidenceReadRequest,
        EvaluationWireKindV1::EvidenceSnapshot,
        EvaluationWireKindV1::AuxiliaryRunSpec,
        EvaluationWireKindV1::AuxiliaryRunSnapshot,
        EvaluationWireKindV1::AuxiliaryRunOutput,
        EvaluationWireKindV1::EvaluationResult,
        EvaluationWireKindV1::EvaluationRecord,
    ];
    assert_eq!(envelopes.len(), EVALUATION_WIRE_KIND_DESCRIPTORS_V1.len());
    for (envelope, expected_kind) in envelopes.into_iter().zip(expected) {
        assert_eq!(envelope.kind(), expected_kind);
        let bytes = envelope.to_vec().expect("wire encoding");
        let decoded = EvaluationWireEnvelopeV1::from_slice(&bytes).expect("wire decoding");
        assert_eq!(decoded, envelope);
    }
}

#[tokio::test]
async fn unknown_fields_and_version_changes_are_rejected_at_the_boundary() {
    let (evidence, ..) = fixture().await;
    let envelope = EvaluationWireEnvelopeV1::from_evidence_snapshot(evidence).unwrap();
    let mut value = serde_json::to_value(&envelope).unwrap();
    value
        .get_mut("payload")
        .and_then(Value::as_object_mut)
        .expect("payload object")
        .insert("future_field".to_string(), Value::Bool(true));
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(EvaluationWireEnvelopeV1::from_slice(&bytes).is_err());

    let mut versioned = serde_json::to_value(&envelope).unwrap();
    versioned
        .as_object_mut()
        .expect("envelope object")
        .insert("version".to_string(), Value::from(2));
    let decoded: EvaluationWireEnvelopeV1 = serde_json::from_value(versioned).unwrap();
    assert!(matches!(
        decoded.validate(),
        Err(EvaluationProtocolError::UnsupportedVersion(2))
    ));
}

#[test]
fn generated_manifest_matches_rust_catalog() {
    let manifest: Manifest =
        serde_json::from_str(include_str!("../../sdk/evaluation/evaluation-wire-v1.json"))
            .expect("generated manifest");
    assert_eq!(manifest.schema, EVALUATION_PROTOCOL_SCHEMA_V1);
    assert_eq!(manifest.version, EVALUATION_PROTOCOL_VERSION_V1);
    assert_eq!(
        manifest.max_message_bytes,
        EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES
    );
    assert_eq!(
        manifest.kinds.len(),
        EVALUATION_WIRE_KIND_DESCRIPTORS_V1.len()
    );
    for (manifest_kind, descriptor) in manifest
        .kinds
        .iter()
        .zip(EVALUATION_WIRE_KIND_DESCRIPTORS_V1)
    {
        assert_eq!(manifest_kind.variant, format!("{:?}", descriptor.kind));
        assert_eq!(manifest_kind.constant, descriptor.constant_name);
        assert_eq!(manifest_kind.wire_name, descriptor.wire_name);
        assert_eq!(manifest_kind.payload_type, descriptor.payload_type);
    }
}

#[test]
fn generated_boundary_fixtures_are_enforced_by_rust() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../sdk/evaluation/evaluation-wire-v1-fixtures.json"
    ))
    .expect("generated fixtures");
    let valid = serde_json::to_vec(fixtures.get("valid").expect("valid fixture")).unwrap();
    let decoded = EvaluationWireEnvelopeV1::from_slice(&valid).expect("valid fixture decodes");
    assert_eq!(decoded.kind(), EvaluationWireKindV1::EvidenceReadRequest);

    for name in ["unknown_top_level_field", "unknown_payload_field"] {
        let bytes = serde_json::to_vec(fixtures.get(name).expect("negative fixture")).unwrap();
        assert!(
            EvaluationWireEnvelopeV1::from_slice(&bytes).is_err(),
            "fixture {name} must be rejected"
        );
    }
    let versioned = serde_json::to_vec(
        fixtures
            .get("unsupported_version")
            .expect("version fixture"),
    )
    .unwrap();
    assert!(matches!(
        EvaluationWireEnvelopeV1::from_slice(&versioned),
        Err(EvaluationProtocolError::UnsupportedVersion(2))
    ));
}
