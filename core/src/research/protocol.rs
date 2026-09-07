//! Versioned, language-neutral transport for native research contracts.
//!
//! Sibling modules own Rust-first research values. This module is the small
//! wire boundary hosts use when projecting those values across a process or
//! SDK. It carries no scientific rubric, publication decision, or Cloud
//! business state.

use super::{
    ResearchCitationV1, ResearchClaimV1, ResearchContractError, ResearchEventV1,
    ResearchEvidenceFactV1, ResearchEvidenceGraphV1, ResearchProvenanceReceiptV1,
    ResearchReproducibilityManifestV1, ResearchRerunLineageV1, ResearchReviewBatchV1,
    ResearchReviewFindingV1, ResearchRunV1, ResearchWorkflowPlanV1, ResearchWorkflowStepV1,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Version carried by the research wire envelope.
pub const RESEARCH_PROTOCOL_VERSION_V1: u16 = 1;

/// Stable schema identifier for the cross-process research envelope.
pub const RESEARCH_PROTOCOL_SCHEMA_V1: &str = "a3s.code.research-wire.v1";

/// Maximum encoded envelope size accepted at a process boundary.
pub const RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

macro_rules! define_research_wire_kinds_v1 {
    ($( $variant:ident => $constant:ident = $wire_name:literal => $payload:ident ),+ $(,)?) => {
        /// Closed top-level payload kinds in research wire version 1.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum ResearchWireKindV1 {
            $( $variant, )+
        }

        /// Canonical string constants for the version-one wire catalog.
        #[derive(Debug, Clone, Copy)]
        pub struct ResearchWireTypeV1;

        impl ResearchWireTypeV1 {
            $( pub const $constant: &'static str = $wire_name; )+
        }

        /// One source-of-truth descriptor used by SDK artifact generation and
        /// parity tests. The payload type is documentation metadata; Rust
        /// validation still uses the concrete type in the match below.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct ResearchWireKindDescriptorV1 {
            pub kind: ResearchWireKindV1,
            pub wire_name: &'static str,
            pub constant_name: &'static str,
            pub payload_type: &'static str,
        }

        /// Ordered catalog known by research wire version 1.
        pub const RESEARCH_WIRE_KIND_DESCRIPTORS_V1: &[ResearchWireKindDescriptorV1] = &[
            $( ResearchWireKindDescriptorV1 {
                kind: ResearchWireKindV1::$variant,
                wire_name: $wire_name,
                constant_name: stringify!($constant),
                payload_type: stringify!($payload),
            }, )+
        ];

        impl ResearchWireKindV1 {
            /// Return the canonical wire spelling for this kind.
            pub const fn wire_name(self) -> &'static str {
                match self {
                    $( Self::$variant => $wire_name, )+
                }
            }

            /// Return the Rust payload type projected by this kind.
            pub const fn payload_type(self) -> &'static str {
                match self {
                    $( Self::$variant => stringify!($payload), )+
                }
            }

            /// Parse one canonical wire spelling without accepting aliases.
            pub fn from_wire_name(value: &str) -> Option<Self> {
                match value {
                    $( $wire_name => Some(Self::$variant), )+
                    _ => None,
                }
            }
        }
    };
}

// Keep this catalog intentionally boring and one-entry-per-line. The SDK
// generator parses these lines, while Rust compiles the same list into the
// enum, constants, and descriptors above.
define_research_wire_kinds_v1! {
    ResearchRun => RESEARCH_RUN = "research_run" => ResearchRunV1,
    ResearchEvidenceFact => RESEARCH_EVIDENCE_FACT = "research_evidence_fact" => ResearchEvidenceFactV1,
    ResearchClaim => RESEARCH_CLAIM = "research_claim" => ResearchClaimV1,
    ResearchCitation => RESEARCH_CITATION = "research_citation" => ResearchCitationV1,
    ResearchEvidenceGraph => RESEARCH_EVIDENCE_GRAPH = "research_evidence_graph" => ResearchEvidenceGraphV1,
    ResearchWorkflowStep => RESEARCH_WORKFLOW_STEP = "research_workflow_step" => ResearchWorkflowStepV1,
    ResearchWorkflowPlan => RESEARCH_WORKFLOW_PLAN = "research_workflow_plan" => ResearchWorkflowPlanV1,
    ResearchRerunLineage => RESEARCH_RERUN_LINEAGE = "research_rerun_lineage" => ResearchRerunLineageV1,
    ResearchProvenanceReceipt => RESEARCH_PROVENANCE_RECEIPT = "research_provenance_receipt" => ResearchProvenanceReceiptV1,
    ResearchReproducibilityManifest => RESEARCH_REPRODUCIBILITY_MANIFEST = "research_reproducibility_manifest" => ResearchReproducibilityManifestV1,
    ResearchReviewFinding => RESEARCH_REVIEW_FINDING = "research_review_finding" => ResearchReviewFindingV1,
    ResearchReviewBatch => RESEARCH_REVIEW_BATCH = "research_review_batch" => ResearchReviewBatchV1,
    ResearchEvent => RESEARCH_EVENT = "research_event" => ResearchEventV1,
}

/// Errors produced while decoding or validating a research wire message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResearchProtocolError {
    #[error("research wire schema is unsupported")]
    UnsupportedSchema,
    #[error("research wire version {0} is unsupported")]
    UnsupportedVersion(u16),
    #[error("research wire kind is unknown")]
    UnknownKind,
    #[error("research wire field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("research wire payload is invalid: {0}")]
    Payload(String),
    #[error("research wire value exceeds its bounded encoding")]
    Encoding,
    #[error("research wire serialization failed: {0}")]
    Serialization(String),
}

impl ResearchProtocolError {
    /// Stable machine-readable error code for SDK and host boundaries.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema => "a3s.code.research_protocol.unsupported_schema",
            Self::UnsupportedVersion(_) => "a3s.code.research_protocol.unsupported_version",
            Self::UnknownKind => "a3s.code.research_protocol.unknown_kind",
            Self::InvalidField(_) => "a3s.code.research_protocol.invalid_field",
            Self::Payload(_) => "a3s.code.research_protocol.payload",
            Self::Encoding => "a3s.code.research_protocol.encoding",
            Self::Serialization(_) => "a3s.code.research_protocol.serialization",
        }
    }
}

impl From<ResearchContractError> for ResearchProtocolError {
    fn from(error: ResearchContractError) -> Self {
        Self::Payload(error.to_string())
    }
}

/// A strict, versioned envelope carrying one research contract value.
///
/// The envelope uses a JSON `Value` for the payload so the same transport can
/// cross Node, Python, and Go without requiring those SDKs to instantiate Rust
/// types. [`Self::validate`] immediately decodes the value into the closed
/// payload type selected by `kind`, preserving Rust-side validation and
/// rejecting unknown payload fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchWireEnvelopeV1 {
    pub schema: String,
    pub version: u16,
    pub kind: ResearchWireKindV1,
    pub payload: Value,
}

impl ResearchWireEnvelopeV1 {
    /// Construct and validate an envelope from an arbitrary JSON payload.
    pub fn new(kind: ResearchWireKindV1, payload: Value) -> Result<Self, ResearchProtocolError> {
        let envelope = Self {
            schema: RESEARCH_PROTOCOL_SCHEMA_V1.to_string(),
            version: RESEARCH_PROTOCOL_VERSION_V1,
            kind,
            payload,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    /// Decode and validate a bounded JSON wire message.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, ResearchProtocolError> {
        if bytes.len() > RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES {
            return Err(ResearchProtocolError::Encoding);
        }
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|error| ResearchProtocolError::Serialization(error.to_string()))?;
        Self::from_value(value)
    }

    /// Decode and validate a JSON value at the process boundary.
    pub fn from_value(value: Value) -> Result<Self, ResearchProtocolError> {
        let encoded = serde_json::to_vec(&value)
            .map_err(|error| ResearchProtocolError::Serialization(error.to_string()))?;
        if encoded.len() > RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES {
            return Err(ResearchProtocolError::Encoding);
        }
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or(ResearchProtocolError::InvalidField("kind"))?;
        if ResearchWireKindV1::from_wire_name(kind).is_none() {
            return Err(ResearchProtocolError::UnknownKind);
        }
        let envelope: Self = serde_json::from_value(value)
            .map_err(|error| ResearchProtocolError::Serialization(error.to_string()))?;
        envelope.validate()?;
        Ok(envelope)
    }

    /// Encode a validated envelope with the process-boundary size limit.
    pub fn to_vec(&self) -> Result<Vec<u8>, ResearchProtocolError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| ResearchProtocolError::Serialization(error.to_string()))?;
        if bytes.len() > RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES {
            return Err(ResearchProtocolError::Encoding);
        }
        Ok(bytes)
    }

    /// Validate envelope identity, bounded encoding, and the selected payload.
    pub fn validate(&self) -> Result<(), ResearchProtocolError> {
        if self.schema != RESEARCH_PROTOCOL_SCHEMA_V1 {
            return Err(ResearchProtocolError::UnsupportedSchema);
        }
        if self.version != RESEARCH_PROTOCOL_VERSION_V1 {
            return Err(ResearchProtocolError::UnsupportedVersion(self.version));
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|error| ResearchProtocolError::Serialization(error.to_string()))?;
        if encoded.len() > RESEARCH_PROTOCOL_MAX_MESSAGE_BYTES {
            return Err(ResearchProtocolError::Encoding);
        }

        match self.kind {
            ResearchWireKindV1::ResearchRun => {
                let payload: ResearchRunV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchEvidenceFact => {
                let payload: ResearchEvidenceFactV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchClaim => {
                let payload: ResearchClaimV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchCitation => {
                let payload: ResearchCitationV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchEvidenceGraph => {
                let payload: ResearchEvidenceGraphV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchWorkflowStep => {
                let payload: ResearchWorkflowStepV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchWorkflowPlan => {
                let payload: ResearchWorkflowPlanV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchRerunLineage => {
                let payload: ResearchRerunLineageV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchProvenanceReceipt => {
                let payload: ResearchProvenanceReceiptV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchReproducibilityManifest => {
                let payload: ResearchReproducibilityManifestV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchReviewFinding => {
                let payload: ResearchReviewFindingV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchReviewBatch => {
                let payload: ResearchReviewBatchV1 = self.decode_payload()?;
                payload.validate()?;
            }
            ResearchWireKindV1::ResearchEvent => {
                let payload: ResearchEventV1 = self.decode_payload()?;
                payload.validate()?;
            }
        }
        Ok(())
    }

    /// Return the payload kind without exposing a second string authority.
    pub const fn kind(&self) -> ResearchWireKindV1 {
        self.kind
    }

    /// Borrow the raw JSON payload for a host transport adapter.
    pub fn payload(&self) -> &Value {
        &self.payload
    }

    /// Construct a research-event envelope.
    pub fn from_research_event(payload: ResearchEventV1) -> Result<Self, ResearchProtocolError> {
        Self::from_typed(ResearchWireKindV1::ResearchEvent, payload)
    }

    /// Construct a reproducibility-manifest envelope.
    pub fn from_reproducibility_manifest(
        payload: ResearchReproducibilityManifestV1,
    ) -> Result<Self, ResearchProtocolError> {
        Self::from_typed(ResearchWireKindV1::ResearchReproducibilityManifest, payload)
    }

    /// Decode a typed payload after checking that the envelope kind matches.
    pub fn payload_as<T>(&self, expected: ResearchWireKindV1) -> Result<T, ResearchProtocolError>
    where
        T: DeserializeOwned,
    {
        self.validate()?;
        if self.kind != expected {
            return Err(ResearchProtocolError::InvalidField("kind"));
        }
        self.decode_payload()
    }

    fn from_typed<T: Serialize>(
        kind: ResearchWireKindV1,
        payload: T,
    ) -> Result<Self, ResearchProtocolError> {
        let value = serde_json::to_value(payload)
            .map_err(|error| ResearchProtocolError::Serialization(error.to_string()))?;
        Self::new(kind, value)
    }

    fn decode_payload<T: DeserializeOwned>(&self) -> Result<T, ResearchProtocolError> {
        serde_json::from_value(self.payload.clone())
            .map_err(|error| ResearchProtocolError::Payload(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }

    fn fixture_event() -> ResearchEventV1 {
        ResearchEventV1::new(
            "project-1",
            1,
            Some("run-1".to_owned()),
            1,
            "research.run.admitted",
            digest('1'),
            1,
        )
        .unwrap()
    }

    #[test]
    fn event_envelope_round_trips_and_rejects_unknown_kind() {
        let event = fixture_event();
        let envelope = ResearchWireEnvelopeV1::from_research_event(event.clone()).unwrap();
        assert_eq!(envelope.kind(), ResearchWireKindV1::ResearchEvent);
        let encoded = envelope.to_vec().unwrap();
        let decoded = ResearchWireEnvelopeV1::from_slice(&encoded).unwrap();
        let restored: ResearchEventV1 = decoded
            .payload_as(ResearchWireKindV1::ResearchEvent)
            .unwrap();
        assert_eq!(restored, event);

        let mut value = serde_json::to_value(&envelope).unwrap();
        value["kind"] = Value::String("future_kind".to_owned());
        assert_eq!(
            ResearchWireEnvelopeV1::from_value(value),
            Err(ResearchProtocolError::UnknownKind)
        );
    }

    #[test]
    fn envelope_rejects_unknown_top_level_and_version_drift() {
        let envelope = ResearchWireEnvelopeV1::from_research_event(fixture_event()).unwrap();
        let mut value = serde_json::to_value(&envelope).unwrap();
        value["future_field"] = Value::Bool(true);
        assert!(matches!(
            ResearchWireEnvelopeV1::from_value(value.clone()),
            Err(ResearchProtocolError::Serialization(_))
        ));
        value = serde_json::to_value(&envelope).unwrap();
        value["version"] = Value::from(RESEARCH_PROTOCOL_VERSION_V1 + 1);
        assert_eq!(
            ResearchWireEnvelopeV1::from_value(value),
            Err(ResearchProtocolError::UnsupportedVersion(
                RESEARCH_PROTOCOL_VERSION_V1 + 1
            ))
        );
    }

    #[test]
    fn generated_fixtures_accept_valid_and_reject_drift() {
        #[derive(Deserialize)]
        struct Fixtures {
            valid: Value,
            unknown_top_level_field: Value,
            unknown_payload_field: Value,
            unsupported_version: Value,
        }
        let fixtures: Fixtures = serde_json::from_str(include_str!(
            "../../../sdk/research/research-wire-v1-fixtures.json"
        ))
        .expect("generated fixtures");
        let valid = ResearchWireEnvelopeV1::from_value(fixtures.valid).unwrap();
        assert_eq!(valid.kind(), ResearchWireKindV1::ResearchEvent);
        assert!(matches!(
            ResearchWireEnvelopeV1::from_value(fixtures.unknown_top_level_field),
            Err(ResearchProtocolError::Serialization(_))
        ));
        assert!(ResearchWireEnvelopeV1::from_value(fixtures.unknown_payload_field).is_err());
        assert_eq!(
            ResearchWireEnvelopeV1::from_value(fixtures.unsupported_version),
            Err(ResearchProtocolError::UnsupportedVersion(
                RESEARCH_PROTOCOL_VERSION_V1 + 1
            ))
        );
    }

    #[test]
    fn catalog_covers_every_research_contract_surface() {
        assert_eq!(RESEARCH_WIRE_KIND_DESCRIPTORS_V1.len(), 13);
        let names: Vec<_> = RESEARCH_WIRE_KIND_DESCRIPTORS_V1
            .iter()
            .map(|item| item.wire_name)
            .collect();
        assert!(names.contains(&"research_reproducibility_manifest"));
        assert!(names.contains(&"research_event"));
        assert_eq!(
            names.len(),
            names
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        );
    }
}
