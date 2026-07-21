use super::schema::release_schema;
use super::types::{
    AgentReleaseArtifact, AgentReleaseCapability, AgentReleaseCompatibility,
    AgentReleaseEntrypoint, AgentReleaseHealth, AgentReleaseProvenance,
    AgentReleaseSecretRequirement, AgentReleaseStorage,
};
use super::validation::{
    parse_artifact, parse_capability, parse_entrypoint, parse_health, parse_provenance,
    parse_secret, parse_storage, required_block, required_string, unique_capabilities,
    unique_provenance, unique_secrets, validate_protocol,
};
use super::{
    AgentReleaseError, AgentReleaseField, AGENT_RELEASE_CONTRACT_V1, AGENT_RELEASE_LIMITS,
};
use a3s_acl::{
    canonical_bytes_with_schema, canonical_digest_with_schema, parse_with_limits,
    validate_document_with_limits,
};
use std::io::Read;
use std::path::Path;

/// Admitted, canonical Agent release manifest.
#[derive(Debug, Clone)]
pub struct AgentReleaseManifest {
    contract: String,
    protocol: String,
    artifact: AgentReleaseArtifact,
    entrypoint: AgentReleaseEntrypoint,
    health: AgentReleaseHealth,
    storage: AgentReleaseStorage,
    required_capabilities: Vec<AgentReleaseCapability>,
    required_secrets: Vec<AgentReleaseSecretRequirement>,
    provenance: Vec<AgentReleaseProvenance>,
    identity: String,
    canonical_acl: String,
}

impl AgentReleaseManifest {
    /// Parse, admit, and canonicalize one untrusted `.a3s/asset.acl` document.
    pub fn parse(source: &str) -> Result<Self, AgentReleaseError> {
        let document = parse_with_limits(source, AGENT_RELEASE_LIMITS)?;
        let schema = release_schema();
        let report = validate_document_with_limits(&document, &schema, AGENT_RELEASE_LIMITS);
        if !report.is_empty() {
            let Some(diagnostic) = report.diagnostics.first() else {
                return Err(AgentReleaseError::SchemaBudgetExceeded);
            };
            return Err(AgentReleaseError::Schema {
                diagnostic: diagnostic.code,
                truncated: report.truncated,
            });
        }

        let root = required_block(
            &document.blocks,
            "agent_release",
            AgentReleaseField::Contract,
        )?;
        let contract = required_string(root, "schema", AgentReleaseField::Contract)?;
        if contract != AGENT_RELEASE_CONTRACT_V1 {
            return Err(AgentReleaseError::UnsupportedContract);
        }

        let protocol = required_string(root, "protocol", AgentReleaseField::Protocol)?;
        validate_protocol(&protocol)?;

        let artifact = parse_artifact(required_block(
            &root.blocks,
            "artifact",
            AgentReleaseField::ArtifactDigest,
        )?)?;
        let entrypoint = parse_entrypoint(required_block(
            &root.blocks,
            "entrypoint",
            AgentReleaseField::EntrypointCommand,
        )?)?;
        let health = parse_health(required_block(
            &root.blocks,
            "health",
            AgentReleaseField::HealthTransport,
        )?)?;
        let storage = parse_storage(required_block(
            &root.blocks,
            "storage",
            AgentReleaseField::WorkspaceMode,
        )?)?;

        let required_capabilities = unique_capabilities(
            root.blocks
                .iter()
                .filter(|block| block.name == "capability")
                .map(parse_capability)
                .collect::<Result<Vec<_>, _>>()?,
        )?;
        let required_secrets = unique_secrets(
            root.blocks
                .iter()
                .filter(|block| block.name == "secret")
                .map(parse_secret)
                .collect::<Result<Vec<_>, _>>()?,
        )?;
        if !required_secrets.is_empty()
            && !required_capabilities
                .iter()
                .any(|capability| capability.name == "secrets.external")
        {
            return Err(AgentReleaseError::InvalidField(
                AgentReleaseField::CapabilityName,
            ));
        }
        let provenance = unique_provenance(
            root.blocks
                .iter()
                .filter(|block| block.name == "provenance")
                .map(parse_provenance)
                .collect::<Result<Vec<_>, _>>()?,
        )?;

        let canonical_acl = String::from_utf8(canonical_bytes_with_schema(&document, &schema)?)
            .map_err(|_| AgentReleaseError::CanonicalEncoding)?;
        let identity = canonical_digest_with_schema(&document, &schema)?;

        Ok(Self {
            contract,
            protocol,
            artifact,
            entrypoint,
            health,
            storage,
            required_capabilities,
            required_secrets,
            provenance,
            identity,
            canonical_acl,
        })
    }

    /// Read at most the manifest budget plus one byte before parsing.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, AgentReleaseError> {
        let file = std::fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take(AGENT_RELEASE_LIMITS.max_document_bytes as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > AGENT_RELEASE_LIMITS.max_document_bytes {
            return Err(AgentReleaseError::InputTooLarge);
        }
        let source = std::str::from_utf8(&bytes).map_err(|_| AgentReleaseError::InvalidEncoding)?;
        Self::parse(source)
    }

    pub fn contract(&self) -> &str {
        &self.contract
    }

    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    pub fn artifact(&self) -> &AgentReleaseArtifact {
        &self.artifact
    }

    pub fn entrypoint(&self) -> &AgentReleaseEntrypoint {
        &self.entrypoint
    }

    pub fn health(&self) -> &AgentReleaseHealth {
        &self.health
    }

    pub fn storage(&self) -> &AgentReleaseStorage {
        &self.storage
    }

    pub fn required_capabilities(&self) -> &[AgentReleaseCapability] {
        &self.required_capabilities
    }

    pub fn required_secrets(&self) -> &[AgentReleaseSecretRequirement] {
        &self.required_secrets
    }

    pub fn provenance(&self) -> &[AgentReleaseProvenance] {
        &self.provenance
    }

    /// Lowercase SHA-256 digest of the schema-aware canonical ACL bytes.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Schema-aware canonical ACL bytes as UTF-8 with one final newline.
    pub fn canonical_acl(&self) -> &str {
        &self.canonical_acl
    }

    /// Fail before activation unless protocol and every required capability match.
    pub fn verify_compatibility(
        &self,
        available: &AgentReleaseCompatibility,
    ) -> Result<(), AgentReleaseError> {
        if self.protocol != available.protocol {
            return Err(AgentReleaseError::IncompatibleProtocol);
        }
        for (required_index, required) in self.required_capabilities.iter().enumerate() {
            let supported = available
                .capabilities
                .iter()
                .find(|capability| capability.name == required.name);
            if supported.is_none_or(|capability| capability.level < required.level) {
                return Err(AgentReleaseError::UnsupportedCapability { required_index });
            }
        }
        Ok(())
    }
}
