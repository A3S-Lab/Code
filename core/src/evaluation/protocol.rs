//! Versioned, language-neutral transport for the evaluation substrate.
//!
//! The runtime contracts in the sibling modules are Rust-first traits and
//! stores.  This module is the deliberately small wire boundary used by a
//! host when it projects evidence, auxiliary-run lifecycle, and immutable
//! result records to another process or SDK.  It contains no reviewer rubric,
//! finding vocabulary, authorization, or Cloud business state.

use super::auxiliary_run::{
    AuxiliaryRunOutputV1, AuxiliaryRunSnapshotV1, AuxiliaryRunSpecV1, AUXILIARY_MAX_OUTPUT_BYTES,
};
use super::evidence::{EvidenceReadRequestV1, EvidenceSnapshotV1};
use super::result::{EvaluationRecordV1, EvaluationResultV1};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Version carried by the evaluation wire envelope.
pub const EVALUATION_PROTOCOL_VERSION_V1: u16 = 1;

/// Stable schema identifier for the cross-process evaluation envelope.
pub const EVALUATION_PROTOCOL_SCHEMA_V1: &str = "a3s.code.evaluation-wire.v1";

/// Maximum encoded envelope size accepted at a process boundary.
pub const EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

macro_rules! define_evaluation_wire_kinds_v1 {
    ($( $variant:ident => $constant:ident = $wire_name:literal => $payload:ident ),+ $(,)?) => {
        /// Closed top-level payload kinds in evaluation wire version 1.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum EvaluationWireKindV1 {
            $( $variant, )+
        }

        /// Canonical string constants for the version-one wire catalog.
        #[derive(Debug, Clone, Copy)]
        pub struct EvaluationWireTypeV1;

        impl EvaluationWireTypeV1 {
            $( pub const $constant: &'static str = $wire_name; )+
        }

        /// One source-of-truth descriptor used by SDK artifact generation and
        /// parity tests.  The payload type is documentation metadata; Rust
        /// validation still uses the concrete type in the match below.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct EvaluationWireKindDescriptorV1 {
            pub kind: EvaluationWireKindV1,
            pub wire_name: &'static str,
            pub constant_name: &'static str,
            pub payload_type: &'static str,
        }

        /// Ordered catalog known by evaluation wire version 1.
        pub const EVALUATION_WIRE_KIND_DESCRIPTORS_V1: &[EvaluationWireKindDescriptorV1] = &[
            $( EvaluationWireKindDescriptorV1 {
                kind: EvaluationWireKindV1::$variant,
                wire_name: $wire_name,
                constant_name: stringify!($constant),
                payload_type: stringify!($payload),
            }, )+
        ];

        impl EvaluationWireKindV1 {
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

// Keep this catalog intentionally boring and one-entry-per-line.  The SDK
// generator parses these lines, while Rust compiles the same list into the
// enum, constants, and descriptors above.
define_evaluation_wire_kinds_v1! {
    EvidenceReadRequest => EVIDENCE_READ_REQUEST = "evidence_read_request" => EvidenceReadRequestV1,
    EvidenceSnapshot => EVIDENCE_SNAPSHOT = "evidence_snapshot" => EvidenceSnapshotV1,
    AuxiliaryRunSpec => AUXILIARY_RUN_SPEC = "auxiliary_run_spec" => AuxiliaryRunSpecV1,
    AuxiliaryRunSnapshot => AUXILIARY_RUN_SNAPSHOT = "auxiliary_run_snapshot" => AuxiliaryRunSnapshotV1,
    AuxiliaryRunOutput => AUXILIARY_RUN_OUTPUT = "auxiliary_run_output" => AuxiliaryRunOutputV1,
    EvaluationResult => EVALUATION_RESULT = "evaluation_result" => EvaluationResultV1,
    EvaluationRecord => EVALUATION_RECORD = "evaluation_record" => EvaluationRecordV1,
}

/// Errors produced while decoding or validating an evaluation wire message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvaluationProtocolError {
    #[error("evaluation wire schema is unsupported")]
    UnsupportedSchema,
    #[error("evaluation wire version {0} is unsupported")]
    UnsupportedVersion(u16),
    #[error("evaluation wire kind is unknown")]
    UnknownKind,
    #[error("evaluation wire field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("evaluation wire payload is invalid: {0}")]
    Payload(String),
    #[error("evaluation wire value exceeds its bounded encoding")]
    Encoding,
    #[error("evaluation wire serialization failed: {0}")]
    Serialization(String),
}

impl EvaluationProtocolError {
    /// Stable machine-readable error code for SDK and host boundaries.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema => "a3s.code.evaluation_protocol.unsupported_schema",
            Self::UnsupportedVersion(_) => "a3s.code.evaluation_protocol.unsupported_version",
            Self::UnknownKind => "a3s.code.evaluation_protocol.unknown_kind",
            Self::InvalidField(_) => "a3s.code.evaluation_protocol.invalid_field",
            Self::Payload(_) => "a3s.code.evaluation_protocol.payload",
            Self::Encoding => "a3s.code.evaluation_protocol.encoding",
            Self::Serialization(_) => "a3s.code.evaluation_protocol.serialization",
        }
    }
}

/// A strict, versioned envelope carrying one evaluation substrate value.
///
/// The envelope uses a JSON `Value` for the payload so the same transport can
/// cross Node, Python, and Go without requiring those SDKs to instantiate Rust
/// types.  [`Self::validate`] immediately decodes the value into the closed
/// payload type selected by `kind`, preserving Rust-side validation and
/// rejecting unknown payload fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationWireEnvelopeV1 {
    pub schema: String,
    pub version: u16,
    pub kind: EvaluationWireKindV1,
    pub payload: Value,
}

impl EvaluationWireEnvelopeV1 {
    /// Construct and validate an envelope from an arbitrary JSON payload.
    pub fn new(
        kind: EvaluationWireKindV1,
        payload: Value,
    ) -> Result<Self, EvaluationProtocolError> {
        let envelope = Self {
            schema: EVALUATION_PROTOCOL_SCHEMA_V1.to_string(),
            version: EVALUATION_PROTOCOL_VERSION_V1,
            kind,
            payload,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    /// Decode and validate a bounded JSON wire message.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, EvaluationProtocolError> {
        if bytes.len() > EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES {
            return Err(EvaluationProtocolError::Encoding);
        }
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|error| EvaluationProtocolError::Serialization(error.to_string()))?;
        Self::from_value(value)
    }

    /// Decode and validate a JSON value at the process boundary.
    pub fn from_value(value: Value) -> Result<Self, EvaluationProtocolError> {
        let encoded = serde_json::to_vec(&value)
            .map_err(|error| EvaluationProtocolError::Serialization(error.to_string()))?;
        if encoded.len() > EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES {
            return Err(EvaluationProtocolError::Encoding);
        }
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or(EvaluationProtocolError::InvalidField("kind"))?;
        if EvaluationWireKindV1::from_wire_name(kind).is_none() {
            return Err(EvaluationProtocolError::UnknownKind);
        }
        let envelope: Self = serde_json::from_value(value)
            .map_err(|error| EvaluationProtocolError::Serialization(error.to_string()))?;
        envelope.validate()?;
        Ok(envelope)
    }

    /// Encode a validated envelope with the process-boundary size limit.
    pub fn to_vec(&self) -> Result<Vec<u8>, EvaluationProtocolError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| EvaluationProtocolError::Serialization(error.to_string()))?;
        if bytes.len() > EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES {
            return Err(EvaluationProtocolError::Encoding);
        }
        Ok(bytes)
    }

    /// Validate envelope identity, bounded encoding, and the selected payload.
    pub fn validate(&self) -> Result<(), EvaluationProtocolError> {
        if self.schema != EVALUATION_PROTOCOL_SCHEMA_V1 {
            return Err(EvaluationProtocolError::UnsupportedSchema);
        }
        if self.version != EVALUATION_PROTOCOL_VERSION_V1 {
            return Err(EvaluationProtocolError::UnsupportedVersion(self.version));
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|error| EvaluationProtocolError::Serialization(error.to_string()))?;
        if encoded.len() > EVALUATION_PROTOCOL_MAX_MESSAGE_BYTES {
            return Err(EvaluationProtocolError::Encoding);
        }

        match self.kind {
            EvaluationWireKindV1::EvidenceReadRequest => {
                let payload: EvidenceReadRequestV1 = self.decode_payload()?;
                payload
                    .validate()
                    .map_err(|error| EvaluationProtocolError::Payload(error.to_string()))?;
            }
            EvaluationWireKindV1::EvidenceSnapshot => {
                let payload: EvidenceSnapshotV1 = self.decode_payload()?;
                payload
                    .validate()
                    .map_err(|error| EvaluationProtocolError::Payload(error.to_string()))?;
            }
            EvaluationWireKindV1::AuxiliaryRunSpec => {
                let payload: AuxiliaryRunSpecV1 = self.decode_payload()?;
                // The envelope can prove the spec's internal digest and shape;
                // the host still binds it to an actual evidence snapshot at
                // admission time.
                payload
                    .validate(&payload.evidence_digest)
                    .map_err(|error| EvaluationProtocolError::Payload(error.to_string()))?;
            }
            EvaluationWireKindV1::AuxiliaryRunSnapshot => {
                let payload: AuxiliaryRunSnapshotV1 = self.decode_payload()?;
                payload
                    .validate()
                    .map_err(|error| EvaluationProtocolError::Payload(error.to_string()))?;
            }
            EvaluationWireKindV1::AuxiliaryRunOutput => {
                let payload: AuxiliaryRunOutputV1 = self.decode_payload()?;
                payload
                    .validate(AUXILIARY_MAX_OUTPUT_BYTES, None)
                    .map_err(|error| EvaluationProtocolError::Payload(error.to_string()))?;
            }
            EvaluationWireKindV1::EvaluationResult => {
                let payload: EvaluationResultV1 = self.decode_payload()?;
                payload
                    .validate()
                    .map_err(|error| EvaluationProtocolError::Payload(error.to_string()))?;
            }
            EvaluationWireKindV1::EvaluationRecord => {
                let payload: EvaluationRecordV1 = self.decode_payload()?;
                payload
                    .validate()
                    .map_err(|error| EvaluationProtocolError::Payload(error.to_string()))?;
            }
        }
        Ok(())
    }

    /// Return the payload kind without exposing a second string authority.
    pub const fn kind(&self) -> EvaluationWireKindV1 {
        self.kind
    }

    /// Borrow the raw JSON payload for a host transport adapter.
    pub fn payload(&self) -> &Value {
        &self.payload
    }

    /// Construct an evidence read request envelope.
    pub fn from_evidence_read_request(
        payload: EvidenceReadRequestV1,
    ) -> Result<Self, EvaluationProtocolError> {
        Self::from_typed(EvaluationWireKindV1::EvidenceReadRequest, payload)
    }

    /// Construct an evidence snapshot envelope.
    pub fn from_evidence_snapshot(
        payload: EvidenceSnapshotV1,
    ) -> Result<Self, EvaluationProtocolError> {
        Self::from_typed(EvaluationWireKindV1::EvidenceSnapshot, payload)
    }

    /// Construct an auxiliary specification envelope.
    pub fn from_auxiliary_run_spec(
        payload: AuxiliaryRunSpecV1,
    ) -> Result<Self, EvaluationProtocolError> {
        Self::from_typed(EvaluationWireKindV1::AuxiliaryRunSpec, payload)
    }

    /// Construct an auxiliary lifecycle snapshot envelope.
    pub fn from_auxiliary_run_snapshot(
        payload: AuxiliaryRunSnapshotV1,
    ) -> Result<Self, EvaluationProtocolError> {
        Self::from_typed(EvaluationWireKindV1::AuxiliaryRunSnapshot, payload)
    }

    /// Construct an auxiliary output envelope.
    pub fn from_auxiliary_run_output(
        payload: AuxiliaryRunOutputV1,
    ) -> Result<Self, EvaluationProtocolError> {
        Self::from_typed(EvaluationWireKindV1::AuxiliaryRunOutput, payload)
    }

    /// Construct an evaluation result envelope.
    pub fn from_evaluation_result(
        payload: EvaluationResultV1,
    ) -> Result<Self, EvaluationProtocolError> {
        Self::from_typed(EvaluationWireKindV1::EvaluationResult, payload)
    }

    /// Construct an immutable evaluation record envelope.
    pub fn from_evaluation_record(
        payload: EvaluationRecordV1,
    ) -> Result<Self, EvaluationProtocolError> {
        Self::from_typed(EvaluationWireKindV1::EvaluationRecord, payload)
    }

    /// Decode a typed payload after checking that the envelope kind matches.
    pub fn payload_as<T>(
        &self,
        expected: EvaluationWireKindV1,
    ) -> Result<T, EvaluationProtocolError>
    where
        T: DeserializeOwned,
    {
        self.validate()?;
        if self.kind != expected {
            return Err(EvaluationProtocolError::InvalidField("kind"));
        }
        self.decode_payload()
    }

    fn from_typed<T: Serialize>(
        kind: EvaluationWireKindV1,
        payload: T,
    ) -> Result<Self, EvaluationProtocolError> {
        let value = serde_json::to_value(payload)
            .map_err(|error| EvaluationProtocolError::Serialization(error.to_string()))?;
        Self::new(kind, value)
    }

    fn decode_payload<T: DeserializeOwned>(&self) -> Result<T, EvaluationProtocolError> {
        serde_json::from_value(self.payload.clone())
            .map_err(|error| EvaluationProtocolError::Payload(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluation::{
        AuxiliaryCapabilityProfileV1, AuxiliaryModeV1, EvidenceContentModeV1, EvidenceLimitsV1,
        ExecutionFrameV1, ExecutionTargetV1,
    };

    fn target() -> ExecutionTargetV1 {
        ExecutionTargetV1::new("session-protocol", "run-protocol")
    }

    fn auxiliary_spec() -> AuxiliaryRunSpecV1 {
        AuxiliaryRunSpecV1::new(
            ExecutionFrameV1::root(target()),
            "protocol-fixture",
            "return JSON",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .with_id("aux-protocol")
        .with_mode(AuxiliaryModeV1::Advisory)
        .with_capabilities(AuxiliaryCapabilityProfileV1::tool_free())
    }

    #[test]
    fn catalog_is_ordered_and_self_consistent() {
        assert_eq!(EVALUATION_WIRE_KIND_DESCRIPTORS_V1.len(), 7);
        for descriptor in EVALUATION_WIRE_KIND_DESCRIPTORS_V1 {
            assert_eq!(descriptor.kind.wire_name(), descriptor.wire_name);
            assert!(!descriptor.payload_type.is_empty());
        }
        assert_eq!(
            EvaluationWireTypeV1::EVIDENCE_SNAPSHOT,
            EvaluationWireKindV1::EvidenceSnapshot.wire_name()
        );
    }

    #[test]
    fn strict_envelope_round_trip_and_typed_projection() {
        let mut request = EvidenceReadRequestV1::new(target());
        request.content_mode = EvidenceContentModeV1::BoundedPayload;
        request.limits = EvidenceLimitsV1::default();
        let envelope = EvaluationWireEnvelopeV1::from_evidence_read_request(request.clone())
            .expect("valid request envelope");
        let bytes = envelope.to_vec().expect("encode");
        let decoded = EvaluationWireEnvelopeV1::from_slice(&bytes).expect("decode");
        let projected: EvidenceReadRequestV1 = decoded
            .payload_as(EvaluationWireKindV1::EvidenceReadRequest)
            .expect("typed payload");
        assert_eq!(projected, request);
        assert_eq!(decoded.kind().wire_name(), "evidence_read_request");
    }

    #[test]
    fn unknown_fields_and_versions_fail_closed() {
        let envelope = EvaluationWireEnvelopeV1::from_auxiliary_run_spec(auxiliary_spec())
            .expect("valid spec envelope");
        let mut value = serde_json::to_value(&envelope).expect("serialize");
        value
            .as_object_mut()
            .expect("object")
            .insert("future_field".to_string(), Value::Bool(true));
        assert!(EvaluationWireEnvelopeV1::from_slice(
            &serde_json::to_vec(&value).expect("serialize unknown")
        )
        .is_err());

        let mut versioned = serde_json::to_value(&envelope).expect("serialize");
        versioned
            .as_object_mut()
            .expect("object")
            .insert("version".to_string(), Value::from(2));
        let decoded: EvaluationWireEnvelopeV1 =
            serde_json::from_value(versioned).expect("shape remains valid");
        assert!(matches!(
            decoded.validate(),
            Err(EvaluationProtocolError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn mismatched_payload_kind_is_rejected() {
        let envelope = EvaluationWireEnvelopeV1::from_auxiliary_run_spec(auxiliary_spec())
            .expect("valid spec envelope");
        assert!(matches!(
            envelope.payload_as::<EvidenceReadRequestV1>(EvaluationWireKindV1::EvidenceReadRequest),
            Err(EvaluationProtocolError::InvalidField("kind"))
        ));
    }

    #[test]
    fn unknown_kind_has_a_stable_boundary_error() {
        let value = serde_json::json!({
            "schema": EVALUATION_PROTOCOL_SCHEMA_V1,
            "version": EVALUATION_PROTOCOL_VERSION_V1,
            "kind": "future_kind",
            "payload": {}
        });
        assert!(matches!(
            EvaluationWireEnvelopeV1::from_value(value),
            Err(EvaluationProtocolError::UnknownKind)
        ));
    }
}
