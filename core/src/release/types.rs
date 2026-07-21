use super::validation::{unique_capabilities, validate_dotted_name, validate_protocol};
use super::{AgentReleaseError, AgentReleaseField, MAX_CAPABILITY_LEVEL};

/// Immutable artifact metadata covered by the Agent release identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReleaseArtifact {
    pub(crate) digest: String,
    pub(crate) media_type: String,
}

impl AgentReleaseArtifact {
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }
}

/// Static container entrypoint covered by the Agent release identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReleaseEntrypoint {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
}

impl AgentReleaseEntrypoint {
    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }
}

/// Bounded health and shutdown contract declared by one release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReleaseHealth {
    pub(crate) transport: String,
    pub(crate) port: u16,
    pub(crate) readiness_path: String,
    pub(crate) liveness_path: String,
    pub(crate) shutdown_grace_seconds: u32,
}

impl AgentReleaseHealth {
    pub fn transport(&self) -> &str {
        &self.transport
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn readiness_path(&self) -> &str {
        &self.readiness_path
    }

    pub fn liveness_path(&self) -> &str {
        &self.liveness_path
    }

    pub fn shutdown_grace_seconds(&self) -> u32 {
        self.shutdown_grace_seconds
    }
}

/// Writable workspace boundary declared by one release.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentReleaseWorkspaceMode {
    ReadOnly,
    Ephemeral,
}

impl AgentReleaseWorkspaceMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Ephemeral => "ephemeral",
        }
    }
}

/// Cache lifetime declared by one release.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentReleaseCacheMode {
    None,
    Ephemeral,
}

impl AgentReleaseCacheMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ephemeral => "ephemeral",
        }
    }
}

/// Persistent-data ownership declared by one release.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentReleasePersistentDataMode {
    None,
    External,
}

impl AgentReleasePersistentDataMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::External => "external",
        }
    }
}

/// Explicit workspace, cache, and persistent-data boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReleaseStorage {
    pub(crate) workspace: AgentReleaseWorkspaceMode,
    pub(crate) cache: AgentReleaseCacheMode,
    pub(crate) persistent_data: AgentReleasePersistentDataMode,
}

impl AgentReleaseStorage {
    pub const fn workspace(&self) -> AgentReleaseWorkspaceMode {
        self.workspace
    }

    pub const fn cache(&self) -> AgentReleaseCacheMode {
        self.cache
    }

    pub const fn persistent_data(&self) -> AgentReleasePersistentDataMode {
        self.persistent_data
    }
}

/// Injection surface for a secret requirement.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentReleaseSecretTarget {
    Environment,
    File,
}

impl AgentReleaseSecretTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::File => "file",
        }
    }
}

/// Named secret slot. It declares no external secret identifier or value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReleaseSecretRequirement {
    pub(crate) name: String,
    pub(crate) target: AgentReleaseSecretTarget,
    pub(crate) destination: String,
}

impl AgentReleaseSecretRequirement {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn target(&self) -> AgentReleaseSecretTarget {
        self.target
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }
}

/// One versioned capability required or provided by an Agent runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReleaseCapability {
    pub(crate) name: String,
    pub(crate) level: u32,
}

impl AgentReleaseCapability {
    pub fn new(name: impl Into<String>, level: u32) -> Result<Self, AgentReleaseError> {
        let name = name.into();
        validate_dotted_name(&name, AgentReleaseField::CapabilityName)?;
        if !(1..=MAX_CAPABILITY_LEVEL).contains(&level) {
            return Err(AgentReleaseError::InvalidField(
                AgentReleaseField::CapabilityLevel,
            ));
        }
        Ok(Self { name, level })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn level(&self) -> u32 {
        self.level
    }
}

/// One immutable, digest-bound provenance reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReleaseProvenance {
    pub(crate) kind: String,
    pub(crate) uri: String,
    pub(crate) digest: String,
}

impl AgentReleaseProvenance {
    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Runtime protocol and capability levels available before activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReleaseCompatibility {
    pub(crate) protocol: String,
    pub(crate) capabilities: Vec<AgentReleaseCapability>,
}

impl AgentReleaseCompatibility {
    pub fn new(
        protocol: impl Into<String>,
        capabilities: impl IntoIterator<Item = AgentReleaseCapability>,
    ) -> Result<Self, AgentReleaseError> {
        let protocol = protocol.into();
        validate_protocol(&protocol)?;
        let capabilities = unique_capabilities(capabilities)?;
        Ok(Self {
            protocol,
            capabilities,
        })
    }

    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    pub fn capabilities(&self) -> &[AgentReleaseCapability] {
        &self.capabilities
    }
}
