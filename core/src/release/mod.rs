//! Immutable, bounded Agent release manifests.
//!
//! This module owns admission and identity for `.a3s/asset.acl`. It does not
//! start an Agent daemon or claim that a Runtime Service is ready.

mod error;
mod manifest;
mod schema;
mod types;
mod validation;

pub use error::{AgentReleaseError, AgentReleaseField};
pub use manifest::AgentReleaseManifest;
pub use types::{
    AgentReleaseArtifact, AgentReleaseCacheMode, AgentReleaseCapability, AgentReleaseCompatibility,
    AgentReleaseEntrypoint, AgentReleaseHealth, AgentReleasePersistentDataMode,
    AgentReleaseProvenance, AgentReleaseSecretRequirement, AgentReleaseSecretTarget,
    AgentReleaseStorage, AgentReleaseWorkspaceMode,
};

use a3s_acl::ParseLimits;

/// Versioned ACL schema identifier accepted by this release contract.
pub const AGENT_RELEASE_CONTRACT_V1: &str = "a3s.code.agent-release.v1";

/// Version-one headless Agent request protocol identifier.
pub const AGENT_PROTOCOL_V1: &str = "a3s.code.agent.v1";

/// Capabilities supplied by the native `a3s code harness` process.
pub const AGENT_HARNESS_CAPABILITIES_V1: [(&str, u32); 3] = [
    ("runtime.service", 1),
    ("secrets.external", 1),
    ("workspace.local", 1),
];

/// Sole executable admitted by the version-one Agent release contract.
pub const AGENT_RELEASE_ENTRYPOINT_COMMAND_V1: &str = "/usr/bin/a3s";

/// Arguments that select the A3S Code Harness in a version-one Agent release.
pub const AGENT_RELEASE_ENTRYPOINT_ARGS_V1: [&str; 4] =
    ["code", "harness", "--manifest", "/app/.a3s/asset.acl"];

/// Exact protocol and capability surface supplied by the native v1 Harness.
///
/// Keeping this in Core prevents executable hosts from independently copying
/// capability names or levels when they admit an Agent release.
pub fn agent_harness_compatibility_v1() -> AgentReleaseCompatibility {
    AgentReleaseCompatibility {
        protocol: AGENT_PROTOCOL_V1.to_string(),
        capabilities: AGENT_HARNESS_CAPABILITIES_V1
            .into_iter()
            .map(|(name, level)| AgentReleaseCapability {
                name: name.to_string(),
                level,
            })
            .collect(),
    }
}

/// OCI image-manifest media type accepted by the version-one artifact contract.
pub const AGENT_RELEASE_OCI_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// Limits applied before an untrusted Agent release manifest is admitted.
pub const AGENT_RELEASE_LIMITS: ParseLimits = ParseLimits {
    max_document_bytes: 64 * 1024,
    max_nesting_depth: 8,
    max_collection_items: 256,
    max_token_bytes: 8 * 1024,
    max_diagnostics: 20,
};

pub(crate) const MAX_CAPABILITY_LEVEL: u32 = 65_535;
pub(crate) const MAX_ENTRYPOINT_ARGS: usize = 64;
pub(crate) const MAX_SHUTDOWN_GRACE_SECONDS: u32 = 3_600;
