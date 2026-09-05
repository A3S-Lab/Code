//! Typed identity primitives shared by Code runtime projections.
//!
//! This module is deliberately an identity and adaptation layer, not another
//! event store. The existing run/evaluation journal remains the append-only
//! authority; these values let Agent, evaluation, research, and SDK adapters
//! refer to the same operation, source, capability, and evidence identity.

use crate::event_protocol::{EventEnvelopeV1, EventProtocolError};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const CORE_IDENTITY_SCHEMA_V1: &str = "a3s.code.core-identity.v1";
pub const CORE_EVENT_IDENTITY_SCHEMA_V1: &str = "a3s.code.core-event-identity.v1";
pub const CORE_EVENT_PAYLOAD_DIGEST_DOMAIN_V1: &str = "a3s.code.core-event.payload.v1";
pub const CORE_EVENT_IDENTITY_DIGEST_DOMAIN_V1: &str = "a3s.code.core-event.identity.v1";
pub const CORE_IDENTITY_MAX_ID_BYTES: usize = 256;
pub const CORE_IDENTITY_MAX_EVENT_TYPE_BYTES: usize = 128;
pub const CORE_IDENTITY_MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const CORE_IDENTITY_MAX_MEDIA_TYPE_BYTES: usize = 256;
pub const CORE_IDENTITY_MAX_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreIdentityError {
    #[error("unsupported core identity schema")]
    UnsupportedSchema,
    #[error("core identity field `{0}` is invalid")]
    InvalidField(&'static str),
    #[error("core identity digest `{0}` is invalid")]
    InvalidDigest(&'static str),
    #[error("core identity sequence overflow")]
    SequenceOverflow,
    #[error("core identity serialization failed: {0}")]
    Serialization(String),
    #[error("agent event cannot be adapted to a core identity: {0}")]
    EventProtocol(String),
}

/// Stable operation identity shared by all projections of one execution.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct OperationId(Box<str>);

impl OperationId {
    pub fn new(value: impl Into<String>) -> Result<Self, CoreIdentityError> {
        let value = value.into();
        validate_id("operation_id", &value)?;
        Ok(Self(value.into_boxed_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for OperationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Monotonic source revision used to prevent derived data crossing a source
/// snapshot boundary. Zero means that the caller has not supplied a source
/// revision yet; it is retained for backwards-compatible adapters.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SourceRevision(u64);

impl SourceRevision {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn unknown() -> Self {
        Self(0)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn is_known(self) -> bool {
        self.0 != 0
    }

    pub fn next(self) -> Result<Self, CoreIdentityError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(CoreIdentityError::SequenceOverflow)
    }
}

/// Exact capability publication admitted for an operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityStamp {
    generation: u64,
    digest: String,
}

impl CapabilityStamp {
    pub fn new(generation: u64, digest: impl Into<String>) -> Result<Self, CoreIdentityError> {
        if generation == 0 {
            return Err(CoreIdentityError::InvalidField("generation"));
        }
        let digest = digest.into();
        validate_digest("digest", &digest)?;
        Ok(Self { generation, digest })
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }
}

impl<'de> Deserialize<'de> for CapabilityStamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            generation: u64,
            digest: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.generation, wire.digest).map_err(D::Error::custom)
    }
}

/// Cursor into the evidence stream for one operation.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct EvidenceCursor(u64);

impl EvidenceCursor {
    pub const fn new(sequence: u64) -> Self {
        Self(sequence)
    }

    pub const fn sequence(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Result<Self, CoreIdentityError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(CoreIdentityError::SequenceOverflow)
    }
}

/// Content-addressed artifact identity. Content remains in the authorized
/// artifact store; this value is safe to carry through projections.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactRef {
    digest: String,
    media_type: String,
    size_bytes: u64,
}

impl ArtifactRef {
    pub fn new(
        digest: impl Into<String>,
        media_type: impl Into<String>,
        size_bytes: u64,
    ) -> Result<Self, CoreIdentityError> {
        let digest = digest.into();
        validate_digest("digest", &digest)?;
        let media_type = media_type.into();
        if media_type.is_empty()
            || media_type.len() > CORE_IDENTITY_MAX_MEDIA_TYPE_BYTES
            || media_type.contains('\0')
            || media_type.lines().count() != 1
        {
            return Err(CoreIdentityError::InvalidField("media_type"));
        }
        if size_bytes > CORE_IDENTITY_MAX_ARTIFACT_BYTES {
            return Err(CoreIdentityError::InvalidField("size_bytes"));
        }
        Ok(Self {
            digest,
            media_type,
            size_bytes,
        })
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn validate(&self) -> Result<(), CoreIdentityError> {
        Self::new(
            self.digest.clone(),
            self.media_type.clone(),
            self.size_bytes,
        )
        .map(|_| ())
    }
}

impl<'de> Deserialize<'de> for ArtifactRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            digest: String,
            media_type: String,
            size_bytes: u64,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.digest, wire.media_type, wire.size_bytes).map_err(D::Error::custom)
    }
}

/// Identity shared by one operation's Agent/evaluation/research projections.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreIdentity {
    pub schema: String,
    pub operation_id: OperationId,
    pub source_revision: SourceRevision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_stamp: Option<CapabilityStamp>,
    pub evidence_cursor: EvidenceCursor,
}

impl CoreIdentity {
    pub fn new(
        operation_id: OperationId,
        source_revision: SourceRevision,
        capability_stamp: Option<CapabilityStamp>,
        evidence_cursor: EvidenceCursor,
    ) -> Self {
        Self {
            schema: CORE_IDENTITY_SCHEMA_V1.to_owned(),
            operation_id,
            source_revision,
            capability_stamp,
            evidence_cursor,
        }
    }

    pub fn validate(&self) -> Result<(), CoreIdentityError> {
        if self.schema != CORE_IDENTITY_SCHEMA_V1 {
            return Err(CoreIdentityError::UnsupportedSchema);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for CoreIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            schema: String,
            operation_id: OperationId,
            source_revision: SourceRevision,
            #[serde(default)]
            capability_stamp: Option<CapabilityStamp>,
            evidence_cursor: EvidenceCursor,
        }

        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            schema: wire.schema,
            operation_id: wire.operation_id,
            source_revision: wire.source_revision,
            capability_stamp: wire.capability_stamp,
            evidence_cursor: wire.evidence_cursor,
        };
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

/// Canonical digest-only identity for one runtime event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreEventIdentity {
    pub schema: String,
    pub identity: CoreIdentity,
    pub event_type: String,
    pub payload_digest: String,
    pub payload_bytes: u64,
    pub observed_at_ms: u64,
    pub event_digest: String,
}

/// Canonical event encoding shared by the Core identity and evaluation
/// adapters. `payload` is the type-free envelope payload; `wire` preserves the
/// legacy runtime encoding used by existing evaluation fact digests.
pub(crate) struct CanonicalEventPayload {
    pub(crate) event_type: String,
    pub(crate) payload: Vec<u8>,
    pub(crate) wire: Vec<u8>,
}

pub(crate) fn canonical_event_payload(
    event: &crate::AgentEvent,
) -> Result<CanonicalEventPayload, CoreIdentityError> {
    let envelope = EventEnvelopeV1::try_from(event).map_err(map_event_protocol_error)?;
    let payload = serde_json::to_vec(&envelope.payload)
        .map_err(|error| CoreIdentityError::Serialization(error.to_string()))?;
    let wire = serde_json::to_vec(event)
        .map_err(|error| CoreIdentityError::Serialization(error.to_string()))?;
    Ok(CanonicalEventPayload {
        event_type: envelope.event_type,
        payload,
        wire,
    })
}

impl CoreEventIdentity {
    pub fn from_agent_event(
        identity: CoreIdentity,
        observed_at_ms: u64,
        event: &crate::AgentEvent,
    ) -> Result<Self, CoreIdentityError> {
        identity.validate()?;
        let canonical = canonical_event_payload(event)?;
        let payload = canonical.payload;
        if payload.is_empty() || payload.len() > CORE_IDENTITY_MAX_PAYLOAD_BYTES {
            return Err(CoreIdentityError::InvalidField("payload_bytes"));
        }
        let payload_bytes = u64::try_from(payload.len())
            .map_err(|_| CoreIdentityError::InvalidField("payload_bytes"))?;
        let mut value = Self {
            schema: CORE_EVENT_IDENTITY_SCHEMA_V1.to_owned(),
            identity,
            event_type: canonical.event_type,
            payload_digest: digest_bytes(CORE_EVENT_PAYLOAD_DIGEST_DOMAIN_V1, &payload),
            payload_bytes,
            observed_at_ms,
            event_digest: String::new(),
        };
        value.validate_without_digest()?;
        value.event_digest = value.expected_digest()?;
        Ok(value)
    }

    /// Adapt an event using an injected logical clock instead of reading wall
    /// time in the caller. This keeps deterministic replay and tests separate
    /// from the system clock implementation.
    pub fn from_agent_event_at(
        identity: CoreIdentity,
        clock: &dyn LogicalClock,
        event: &crate::AgentEvent,
    ) -> Result<Self, CoreIdentityError> {
        Self::from_agent_event(identity, clock.now_ms(), event)
    }

    /// Adapt the event representation already retained by a Code run.
    pub fn from_run_event(
        operation_id: OperationId,
        source_revision: SourceRevision,
        capability_stamp: Option<CapabilityStamp>,
        record: &crate::run::RunEventRecord,
    ) -> Result<Self, CoreIdentityError> {
        let sequence = u64::try_from(record.sequence)
            .map_err(|_| CoreIdentityError::InvalidField("sequence"))?;
        Self::from_agent_event(
            CoreIdentity::new(
                operation_id,
                source_revision,
                capability_stamp,
                EvidenceCursor::new(sequence),
            ),
            record.timestamp_ms,
            &record.event,
        )
    }

    pub fn validate(&self) -> Result<(), CoreIdentityError> {
        self.validate_without_digest()?;
        validate_digest("event_digest", &self.event_digest)?;
        if self.event_digest != self.expected_digest()? {
            return Err(CoreIdentityError::InvalidField("event_digest"));
        }
        Ok(())
    }

    pub fn expected_digest(&self) -> Result<String, CoreIdentityError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            identity: &'a CoreIdentity,
            event_type: &'a str,
            payload_digest: &'a str,
            payload_bytes: u64,
            observed_at_ms: u64,
        }
        digest_json(
            CORE_EVENT_IDENTITY_DIGEST_DOMAIN_V1,
            &Identity {
                schema: &self.schema,
                identity: &self.identity,
                event_type: &self.event_type,
                payload_digest: &self.payload_digest,
                payload_bytes: self.payload_bytes,
                observed_at_ms: self.observed_at_ms,
            },
        )
    }

    fn validate_without_digest(&self) -> Result<(), CoreIdentityError> {
        if self.schema != CORE_EVENT_IDENTITY_SCHEMA_V1 {
            return Err(CoreIdentityError::UnsupportedSchema);
        }
        self.identity.validate()?;
        if self.event_type.is_empty()
            || self.event_type.len() > CORE_IDENTITY_MAX_EVENT_TYPE_BYTES
            || self.event_type.starts_with('.')
            || self.event_type.ends_with('.')
            || !self.event_type.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
        {
            return Err(CoreIdentityError::InvalidField("event_type"));
        }
        validate_digest("payload_digest", &self.payload_digest)?;
        if self.payload_bytes == 0 || self.payload_bytes > CORE_IDENTITY_MAX_PAYLOAD_BYTES as u64 {
            return Err(CoreIdentityError::InvalidField("payload_bytes"));
        }
        if self.observed_at_ms == 0 {
            return Err(CoreIdentityError::InvalidField("observed_at_ms"));
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for CoreEventIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            schema: String,
            identity: CoreIdentity,
            event_type: String,
            payload_digest: String,
            payload_bytes: u64,
            observed_at_ms: u64,
            event_digest: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            schema: wire.schema,
            identity: wire.identity,
            event_type: wire.event_type,
            payload_digest: wire.payload_digest,
            payload_bytes: wire.payload_bytes,
            observed_at_ms: wire.observed_at_ms,
            event_digest: wire.event_digest,
        };
        value.validate().map_err(D::Error::custom)?;
        Ok(value)
    }
}

/// Injectable logical time source for event adapters and deterministic tests.
pub trait LogicalClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLogicalClock;

impl LogicalClock for SystemLogicalClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0)
    }
}

#[derive(Debug)]
pub struct ManualLogicalClock {
    now_ms: AtomicU64,
}

impl ManualLogicalClock {
    pub const fn new(initial_ms: u64) -> Self {
        Self {
            now_ms: AtomicU64::new(initial_ms),
        }
    }

    pub fn set(&self, value: u64) {
        self.now_ms.store(value, Ordering::SeqCst);
    }

    pub fn advance(&self, delta_ms: u64) -> Result<u64, CoreIdentityError> {
        let mut current = self.now_ms.load(Ordering::SeqCst);
        loop {
            let next = current
                .checked_add(delta_ms)
                .ok_or(CoreIdentityError::SequenceOverflow)?;
            match self
                .now_ms
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Ok(next),
                Err(actual) => current = actual,
            }
        }
    }
}

impl LogicalClock for ManualLogicalClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

fn map_event_protocol_error(error: EventProtocolError) -> CoreIdentityError {
    CoreIdentityError::EventProtocol(error.to_string())
}

fn validate_id(field: &'static str, value: &str) -> Result<(), CoreIdentityError> {
    if value.is_empty()
        || value.len() > CORE_IDENTITY_MAX_ID_BYTES
        || value.contains('\0')
        || value.lines().count() != 1
    {
        return Err(CoreIdentityError::InvalidField(field));
    }
    Ok(())
}

fn validate_digest(field: &'static str, value: &str) -> Result<(), CoreIdentityError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(CoreIdentityError::InvalidDigest(field));
    }
    Ok(())
}

fn digest_bytes(domain: &str, bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn digest_json<T: Serialize>(domain: &str, value: &T) -> Result<String, CoreIdentityError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| CoreIdentityError::Serialization(error.to_string()))?;
    Ok(digest_bytes(domain, &bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentEvent;
    use serde_json::Value;

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }

    fn identity(cursor: u64) -> CoreIdentity {
        CoreIdentity::new(
            OperationId::new("session-1/run-1").unwrap(),
            SourceRevision::new(7),
            Some(CapabilityStamp::new(3, digest('a')).unwrap()),
            EvidenceCursor::new(cursor),
        )
    }

    #[test]
    fn event_adapter_binds_typed_identity_and_is_replay_stable() {
        let first = CoreEventIdentity::from_agent_event(
            identity(4),
            42,
            &AgentEvent::TextDelta {
                text: "evidence".to_owned(),
            },
        )
        .unwrap();
        let second = CoreEventIdentity::from_agent_event(
            identity(4),
            42,
            &AgentEvent::TextDelta {
                text: "evidence".to_owned(),
            },
        )
        .unwrap();

        assert_eq!(first, second);
        assert_eq!(first.identity.evidence_cursor.sequence(), 4);
        assert_eq!(first.event_type, "text_delta");
        assert!(first.validate().is_ok());
        assert!(
            serde_json::from_value::<CoreEventIdentity>(serde_json::to_value(&first).unwrap())
                .is_ok()
        );
    }

    #[test]
    fn retained_run_event_uses_its_cursor_and_observation_time() {
        let record = crate::run::RunEventRecord {
            sequence: 9,
            timestamp_ms: 77,
            event: AgentEvent::TextDelta {
                text: "done".to_owned(),
            },
        };
        let projected = record
            .core_identity(
                OperationId::new("session-1/run-1").unwrap(),
                SourceRevision::new(8),
                None,
            )
            .unwrap();
        assert_eq!(projected.identity.evidence_cursor.sequence(), 9);
        assert_eq!(projected.observed_at_ms, 77);
        assert_eq!(projected.event_type, "text_delta");
    }

    #[test]
    fn deserialization_rejects_tampered_digest_and_unknown_fields() {
        let event = CoreEventIdentity::from_agent_event(
            identity(0),
            42,
            &AgentEvent::Start {
                prompt: "run".to_owned(),
            },
        )
        .unwrap();
        let mut value = serde_json::to_value(&event).unwrap();
        value["eventDigest"] = Value::String(digest('b'));
        assert!(serde_json::from_value::<CoreEventIdentity>(value).is_err());

        let mut value = serde_json::to_value(&event).unwrap();
        value["unexpected"] = Value::Bool(true);
        assert!(serde_json::from_value::<CoreEventIdentity>(value).is_err());

        let artifact = ArtifactRef::new(digest('a'), "text/plain", 3).unwrap();
        let mut value = serde_json::to_value(&artifact).unwrap();
        value["sizeBytes"] = Value::from(CORE_IDENTITY_MAX_ARTIFACT_BYTES + 1);
        assert!(serde_json::from_value::<ArtifactRef>(value).is_err());
    }

    #[test]
    fn manual_clock_is_injectable_and_overflow_is_explicit() {
        let clock = ManualLogicalClock::new(10);
        assert_eq!(clock.now_ms(), 10);
        assert_eq!(clock.advance(5).unwrap(), 15);
        clock.set(u64::MAX);
        assert_eq!(clock.advance(1), Err(CoreIdentityError::SequenceOverflow));
    }

    #[test]
    fn event_adapter_can_use_an_injected_clock() {
        let clock = ManualLogicalClock::new(123);
        let event = CoreEventIdentity::from_agent_event_at(
            identity(0),
            &clock,
            &AgentEvent::Start {
                prompt: "run".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(event.observed_at_ms, 123);
        clock.advance(7).unwrap();
        let next = CoreEventIdentity::from_agent_event_at(
            identity(1),
            &clock,
            &AgentEvent::TextDelta {
                text: "step".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(next.observed_at_ms, 130);
    }

    #[test]
    fn typed_values_reject_invalid_wire_data() {
        assert!(OperationId::new("bad\noperation").is_err());
        assert!(CapabilityStamp::new(0, digest('a')).is_err());
        assert!(ArtifactRef::new(digest('a'), "", 0).is_err());
        assert_eq!(
            EvidenceCursor::new(u64::MAX).next(),
            Err(CoreIdentityError::SequenceOverflow)
        );
    }
}
