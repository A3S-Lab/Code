use a3s_acl::{CanonicalError, ParseError, SchemaDiagnosticCode};
use std::fmt;

/// Typed Agent release field used by value-redacting admission errors.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentReleaseField {
    Contract,
    Protocol,
    ArtifactDigest,
    ArtifactMediaType,
    EntrypointCommand,
    EntrypointArgument,
    HealthTransport,
    HealthPort,
    ReadinessPath,
    LivenessPath,
    ShutdownGraceSeconds,
    WorkspaceMode,
    CacheMode,
    PersistentDataMode,
    CapabilityName,
    CapabilityLevel,
    SecretName,
    SecretTarget,
    SecretDestination,
    ProvenanceKind,
    ProvenanceUri,
    ProvenanceDigest,
}

impl fmt::Display for AgentReleaseField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Contract => "agent_release.schema",
            Self::Protocol => "agent_release.protocol",
            Self::ArtifactDigest => "agent_release.artifact.digest",
            Self::ArtifactMediaType => "agent_release.artifact.media_type",
            Self::EntrypointCommand => "agent_release.entrypoint.command",
            Self::EntrypointArgument => "agent_release.entrypoint.args",
            Self::HealthTransport => "agent_release.health.transport",
            Self::HealthPort => "agent_release.health.port",
            Self::ReadinessPath => "agent_release.health.readiness_path",
            Self::LivenessPath => "agent_release.health.liveness_path",
            Self::ShutdownGraceSeconds => "agent_release.health.shutdown_grace_seconds",
            Self::WorkspaceMode => "agent_release.storage.workspace",
            Self::CacheMode => "agent_release.storage.cache",
            Self::PersistentDataMode => "agent_release.storage.persistent_data",
            Self::CapabilityName => "agent_release.capability.name",
            Self::CapabilityLevel => "agent_release.capability.level",
            Self::SecretName => "agent_release.secret.name",
            Self::SecretTarget => "agent_release.secret.target",
            Self::SecretDestination => "agent_release.secret.destination",
            Self::ProvenanceKind => "agent_release.provenance.kind",
            Self::ProvenanceUri => "agent_release.provenance.uri",
            Self::ProvenanceDigest => "agent_release.provenance.digest",
        })
    }
}

/// Stable, value-redacting failure returned by Agent release admission.
#[non_exhaustive]
#[derive(Debug)]
pub enum AgentReleaseError {
    Io(std::io::Error),
    InputTooLarge,
    InvalidEncoding,
    Parse(ParseError),
    Schema {
        diagnostic: SchemaDiagnosticCode,
        truncated: bool,
    },
    SchemaBudgetExceeded,
    Canonical(CanonicalError),
    CanonicalEncoding,
    UnsupportedContract,
    InvalidField(AgentReleaseField),
    DuplicateCapability,
    DuplicateSecret,
    DuplicateProvenance,
    IncompatibleProtocol,
    UnsupportedCapability {
        required_index: usize,
    },
}

impl AgentReleaseError {
    /// Stable machine-readable error code.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "a3s.code.agent_release.io",
            Self::InputTooLarge | Self::InvalidEncoding | Self::Parse(_) => {
                "a3s.code.agent_release.parse"
            }
            Self::Schema { .. } | Self::SchemaBudgetExceeded => "a3s.code.agent_release.schema",
            Self::Canonical(_) | Self::CanonicalEncoding => "a3s.code.agent_release.canonical",
            Self::UnsupportedContract => "a3s.code.agent_release.unsupported_contract",
            Self::InvalidField(_) => "a3s.code.agent_release.invalid_field",
            Self::DuplicateCapability => "a3s.code.agent_release.duplicate_capability",
            Self::DuplicateSecret => "a3s.code.agent_release.duplicate_secret",
            Self::DuplicateProvenance => "a3s.code.agent_release.duplicate_provenance",
            Self::IncompatibleProtocol => "a3s.code.agent_release.incompatible_protocol",
            Self::UnsupportedCapability { .. } => "a3s.code.agent_release.unsupported_capability",
        }
    }

    /// Invalid semantic field, when this is an `invalid_field` error.
    pub const fn field(&self) -> Option<AgentReleaseField> {
        match self {
            Self::InvalidField(field) => Some(*field),
            _ => None,
        }
    }
}

impl fmt::Display for AgentReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "could not read Agent release manifest: {error}"),
            Self::InputTooLarge => {
                formatter.write_str("Agent release manifest exceeds its input budget")
            }
            Self::InvalidEncoding => {
                formatter.write_str("Agent release manifest must be valid UTF-8")
            }
            Self::Parse(error) => write!(formatter, "Agent release manifest parse failed: {error}"),
            Self::Schema {
                diagnostic,
                truncated,
            } => {
                write!(
                    formatter,
                    "Agent release manifest schema rejected the document ({diagnostic})"
                )?;
                if *truncated {
                    formatter.write_str("; additional diagnostics were truncated")?;
                }
                Ok(())
            }
            Self::SchemaBudgetExceeded => {
                formatter.write_str("Agent release schema diagnostics exceeded their budget")
            }
            Self::Canonical(error) => {
                write!(formatter, "Agent release canonicalization failed: {error}")
            }
            Self::CanonicalEncoding => {
                formatter.write_str("Agent release canonical bytes are not UTF-8")
            }
            Self::UnsupportedContract => {
                formatter.write_str("Agent release contract version is not supported")
            }
            Self::InvalidField(field) => {
                write!(formatter, "Agent release field {field} is invalid")
            }
            Self::DuplicateCapability => {
                formatter.write_str("Agent release capability requirements must be unique")
            }
            Self::DuplicateSecret => {
                formatter.write_str("Agent release secret requirements must be unique")
            }
            Self::DuplicateProvenance => {
                formatter.write_str("Agent release provenance kinds must be unique")
            }
            Self::IncompatibleProtocol => {
                formatter.write_str("Agent release protocol is not supported by the runtime")
            }
            Self::UnsupportedCapability { required_index } => write!(
                formatter,
                "Agent release capability requirement at index {required_index} is unavailable"
            ),
        }
    }
}

impl std::error::Error for AgentReleaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::Canonical(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AgentReleaseError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ParseError> for AgentReleaseError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

impl From<CanonicalError> for AgentReleaseError {
    fn from(error: CanonicalError) -> Self {
        Self::Canonical(error)
    }
}
