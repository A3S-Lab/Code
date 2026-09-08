#![cfg(feature = "research")]
use a3s_code_core::research::{
    ResearchEventV1, ResearchProtocolError, ResearchWireEnvelopeV1, ResearchWireKindV1,
    RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES, RESEARCH_PROTOCOL_SCHEMA_V1, RESEARCH_PROTOCOL_VERSION_V1,
    RESEARCH_WIRE_KIND_DESCRIPTORS_V1,
};
use serde::Deserialize;
use serde_json::Value;

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

fn fixture_event() -> ResearchEventV1 {
    ResearchEventV1::new(
        "fixture-project",
        1,
        Some("fixture-run".to_owned()),
        1,
        "research.run.admitted",
        format!("sha256:{}", "1".repeat(64)),
        1,
    )
    .expect("fixture event")
}

#[test]
fn research_event_round_trips_through_the_strict_envelope() {
    let event = fixture_event();
    let envelope = ResearchWireEnvelopeV1::from_research_event(event.clone()).expect("envelope");
    assert_eq!(envelope.kind(), ResearchWireKindV1::ResearchEvent);
    let bytes = envelope.to_vec().expect("encode");
    let decoded = ResearchWireEnvelopeV1::from_slice(&bytes).expect("decode");
    let restored: ResearchEventV1 = decoded
        .payload_as(ResearchWireKindV1::ResearchEvent)
        .expect("payload");
    assert_eq!(restored, event);
}

#[test]
fn generated_manifest_matches_rust_catalog() {
    let manifest: Manifest =
        serde_json::from_str(include_str!("../../sdk/research/research-wire-v1.json"))
            .expect("generated manifest");
    assert_eq!(manifest.schema, RESEARCH_PROTOCOL_SCHEMA_V1);
    assert_eq!(manifest.version, RESEARCH_PROTOCOL_VERSION_V1);
    assert_eq!(
        manifest.max_message_bytes,
        RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES
    );
    assert_eq!(
        manifest.kinds.len(),
        RESEARCH_WIRE_KIND_DESCRIPTORS_V1.len()
    );
    for (manifest_kind, descriptor) in manifest.kinds.iter().zip(RESEARCH_WIRE_KIND_DESCRIPTORS_V1)
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
        "../../sdk/research/research-wire-v1-fixtures.json"
    ))
    .expect("generated fixtures");
    let valid = serde_json::to_vec(fixtures.get("valid").expect("valid fixture")).unwrap();
    let decoded = ResearchWireEnvelopeV1::from_slice(&valid).expect("valid fixture decodes");
    assert_eq!(decoded.kind(), ResearchWireKindV1::ResearchEvent);

    for name in ["unknown_top_level_field", "unknown_payload_field"] {
        let bytes = serde_json::to_vec(fixtures.get(name).expect("negative fixture")).unwrap();
        assert!(
            ResearchWireEnvelopeV1::from_slice(&bytes).is_err(),
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
        ResearchWireEnvelopeV1::from_slice(&versioned),
        Err(ResearchProtocolError::UnsupportedVersion(2))
    ));
}
