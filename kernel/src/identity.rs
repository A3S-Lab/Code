//! Typed identity primitives shared by Code runtime projections.
//!
//! These values are not an event store. Agent event adaptation stays in
//! `a3s-code-core`, which re-exports this module.

use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use std::fmt;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
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

    #[test]
    fn retention_bounds_are_the_published_ceilings() {
        assert_eq!(CORE_IDENTITY_MAX_ID_BYTES, 256);
        assert_eq!(CORE_IDENTITY_MAX_EVENT_TYPE_BYTES, 128);
        assert_eq!(CORE_IDENTITY_MAX_PAYLOAD_BYTES, 4 * 1024 * 1024);
        assert_eq!(CORE_IDENTITY_MAX_MEDIA_TYPE_BYTES, 256);
        assert_eq!(CORE_IDENTITY_MAX_ARTIFACT_BYTES, 4 * 1024 * 1024 * 1024);
    }
}
