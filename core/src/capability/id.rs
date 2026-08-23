use std::fmt;
use std::num::NonZeroU64;

use serde::Serialize;

use super::CapabilitySetError;

pub const MAX_CAPABILITY_IDENTIFIER_BYTES: usize = 256;
pub const USE_CAPABILITY_SNAPSHOT_CURSOR_SCHEMA: &str = "a3s.use.capability-snapshot-cursor.v1";

/// Canonical lowercase SHA-256 value used by capability identity contracts.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Sha256Digest(Box<str>);

impl Sha256Digest {
    pub fn new(value: impl Into<String>) -> Result<Self, CapabilitySetError> {
        let value = value.into();
        let valid = value.len() == 71
            && value.starts_with("sha256:")
            && value[7..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
        if !valid {
            return Err(CapabilitySetError::InvalidDigest { field: "digest" });
        }
        Ok(Self(value.into_boxed_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Closed product capability categories. Implementations inside a category
/// remain open; arbitrary runtime categories do not enter the set.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityKind {
    Tool,
    Skill,
    Agent,
    Command,
    Hook,
    Mcp,
    Flow,
    Knowledge,
    Ui,
    Context,
}

impl CapabilityKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Skill => "skill",
            Self::Agent => "agent",
            Self::Command => "command",
            Self::Hook => "hook",
            Self::Mcp => "mcp",
            Self::Flow => "flow",
            Self::Knowledge => "knowledge",
            Self::Ui => "ui",
            Self::Context => "context",
        }
    }
}

impl fmt::Display for CapabilityKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stable identity of one trusted contribution source.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CapabilitySourceId(Box<str>);

impl CapabilitySourceId {
    pub(super) fn scoped(
        prefix: &'static str,
        local: impl Into<String>,
    ) -> Result<Self, CapabilitySetError> {
        let local = local.into();
        validate_identifier("source", &local)?;
        let value = format!("{prefix}/{local}");
        if value.len() > MAX_CAPABILITY_IDENTIFIER_BYTES {
            return Err(CapabilitySetError::BoundExceeded {
                field: "source",
                max: MAX_CAPABILITY_IDENTIFIER_BYTES,
            });
        }
        Ok(Self(value.into_boxed_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CapabilitySourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Stable source, category, and local-surface identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityId {
    source: CapabilitySourceId,
    kind: CapabilityKind,
    local_id: Box<str>,
}

impl CapabilityId {
    pub fn new(
        source: &super::CapabilitySource,
        kind: CapabilityKind,
        local_id: impl Into<String>,
    ) -> Result<Self, CapabilitySetError> {
        let local_id = local_id.into();
        validate_identifier("local_id", &local_id)?;
        Ok(Self {
            source: source.id().clone(),
            kind,
            local_id: local_id.into_boxed_str(),
        })
    }

    pub fn source(&self) -> &CapabilitySourceId {
        &self.source
    }

    pub const fn kind(&self) -> CapabilityKind {
        self.kind
    }

    pub fn local_id(&self) -> &str {
        &self.local_id
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}:{}", self.source, self.kind, self.local_id)
    }
}

/// Exact immutable A3S Use capability publication identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UseCapabilityGeneration {
    schema: &'static str,
    generation: u64,
    revision: Sha256Digest,
    registry_revision: Sha256Digest,
}

impl UseCapabilityGeneration {
    pub fn new(generation: u64, revision: Sha256Digest, registry_revision: Sha256Digest) -> Self {
        Self {
            schema: USE_CAPABILITY_SNAPSHOT_CURSOR_SCHEMA,
            generation,
            revision,
            registry_revision,
        }
    }

    pub const fn schema(&self) -> &'static str {
        self.schema
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn revision(&self) -> &Sha256Digest {
        &self.revision
    }

    pub fn registry_revision(&self) -> &Sha256Digest {
        &self.registry_revision
    }
}

/// Exact immutable A3S Use package lifecycle generation identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsePackageGeneration {
    package_id: Box<str>,
    component_id: Box<str>,
    route: Box<str>,
    version: Box<str>,
    lifecycle_generation: NonZeroU64,
    package_digest: Sha256Digest,
    manifest_digest: Sha256Digest,
}

impl UsePackageGeneration {
    pub fn new(
        package_id: impl Into<String>,
        component_id: impl Into<String>,
        route: impl Into<String>,
        version: impl Into<String>,
        lifecycle_generation: u64,
        package_digest: Sha256Digest,
        manifest_digest: Sha256Digest,
    ) -> Result<Self, CapabilitySetError> {
        let package_id = package_id.into();
        let component_id = component_id.into();
        let route = route.into();
        let version = version.into();
        validate_identifier("package_id", &package_id)?;
        validate_identifier("component_id", &component_id)?;
        validate_identifier("route", &route)?;
        validate_bounded_text("version", &version, 128)?;
        let lifecycle_generation =
            NonZeroU64::new(lifecycle_generation).ok_or(CapabilitySetError::InvalidGeneration {
                field: "lifecycle_generation",
            })?;
        Ok(Self {
            package_id: package_id.into_boxed_str(),
            component_id: component_id.into_boxed_str(),
            route: route.into_boxed_str(),
            version: version.into_boxed_str(),
            lifecycle_generation,
            package_digest,
            manifest_digest,
        })
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn component_id(&self) -> &str {
        &self.component_id
    }

    pub fn route(&self) -> &str {
        &self.route
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub const fn lifecycle_generation(&self) -> u64 {
        self.lifecycle_generation.get()
    }

    pub fn package_digest(&self) -> &Sha256Digest {
        &self.package_digest
    }

    pub fn manifest_digest(&self) -> &Sha256Digest {
        &self.manifest_digest
    }
}

/// Session-local immutable Code catalog generation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CodeCatalogGeneration(u64);

impl CodeCatalogGeneration {
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

pub(super) fn validate_identifier(
    field: &'static str,
    value: &str,
) -> Result<(), CapabilitySetError> {
    if value.is_empty() {
        return Err(CapabilitySetError::InvalidIdentifier {
            field,
            reason: "it is empty",
        });
    }
    if value.len() > MAX_CAPABILITY_IDENTIFIER_BYTES {
        return Err(CapabilitySetError::BoundExceeded {
            field,
            max: MAX_CAPABILITY_IDENTIFIER_BYTES,
        });
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'-' | b'_' | b'.' | b'/')
    }) {
        return Err(CapabilitySetError::InvalidIdentifier {
            field,
            reason: "it contains non-canonical characters",
        });
    }
    if !value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        || !value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        || value
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(CapabilitySetError::InvalidIdentifier {
            field,
            reason: "it has an unsafe boundary or path segment",
        });
    }
    Ok(())
}

fn validate_bounded_text(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), CapabilitySetError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(CapabilitySetError::InvalidIdentifier {
            field,
            reason: "it is empty, padded, or contains control characters",
        });
    }
    if value.len() > max {
        return Err(CapabilitySetError::BoundExceeded { field, max });
    }
    Ok(())
}
