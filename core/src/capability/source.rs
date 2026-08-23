use serde::Serialize;

use super::{
    CapabilitySetError, CapabilitySourceId, Sha256Digest, UseCapabilityGeneration,
    UsePackageGeneration,
};

/// Trusted precedence class assigned by the host adapter, never by package
/// content.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilitySourceClass {
    BuiltIn,
    Host,
    UsePackage,
    Session,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
enum CapabilitySourceEvidence {
    // CAP-SET1 seals construction inside Core. Surface projection will become
    // the first production constructor consumer in CAP-PROJ1.
    #[allow(dead_code)]
    BuiltIn {
        revision: Sha256Digest,
    },
    Host {
        revision: Sha256Digest,
    },
    UsePackage {
        capability: UseCapabilityGeneration,
        package: UsePackageGeneration,
    },
    Session {
        revision: Sha256Digest,
    },
}

/// Immutable identity and authority class for one complete contribution.
/// Fields are private so untrusted package data cannot manufacture built-in
/// precedence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySource {
    id: CapabilitySourceId,
    class: CapabilitySourceClass,
    evidence: CapabilitySourceEvidence,
}

impl CapabilitySource {
    #[allow(dead_code)]
    pub(crate) fn builtin(
        component: impl Into<String>,
        revision: Sha256Digest,
    ) -> Result<Self, CapabilitySetError> {
        Ok(Self {
            id: CapabilitySourceId::scoped("builtin", component)?,
            class: CapabilitySourceClass::BuiltIn,
            evidence: CapabilitySourceEvidence::BuiltIn { revision },
        })
    }

    pub fn host(
        component: impl Into<String>,
        revision: Sha256Digest,
    ) -> Result<Self, CapabilitySetError> {
        Ok(Self {
            id: CapabilitySourceId::scoped("host", component)?,
            class: CapabilitySourceClass::Host,
            evidence: CapabilitySourceEvidence::Host { revision },
        })
    }

    pub fn session(
        registration: impl Into<String>,
        revision: Sha256Digest,
    ) -> Result<Self, CapabilitySetError> {
        Ok(Self {
            id: CapabilitySourceId::scoped("session", registration)?,
            class: CapabilitySourceClass::Session,
            evidence: CapabilitySourceEvidence::Session { revision },
        })
    }

    pub fn use_package(
        capability: UseCapabilityGeneration,
        package: UsePackageGeneration,
    ) -> Result<Self, CapabilitySetError> {
        if capability.generation() == 0 {
            return Err(CapabilitySetError::InvalidGeneration {
                field: "use_capability_generation",
            });
        }
        let id = CapabilitySourceId::scoped("use", package.package_id())?;
        Ok(Self {
            id,
            class: CapabilitySourceClass::UsePackage,
            evidence: CapabilitySourceEvidence::UsePackage {
                capability,
                package,
            },
        })
    }

    pub fn id(&self) -> &CapabilitySourceId {
        &self.id
    }

    pub const fn class(&self) -> CapabilitySourceClass {
        self.class
    }

    pub fn revision(&self) -> Option<&Sha256Digest> {
        match &self.evidence {
            CapabilitySourceEvidence::BuiltIn { revision }
            | CapabilitySourceEvidence::Host { revision }
            | CapabilitySourceEvidence::Session { revision } => Some(revision),
            CapabilitySourceEvidence::UsePackage { .. } => None,
        }
    }

    pub fn use_capability_generation(&self) -> Option<&UseCapabilityGeneration> {
        match &self.evidence {
            CapabilitySourceEvidence::UsePackage { capability, .. } => Some(capability),
            _ => None,
        }
    }

    pub fn use_package_generation(&self) -> Option<&UsePackageGeneration> {
        match &self.evidence {
            CapabilitySourceEvidence::UsePackage { package, .. } => Some(package),
            _ => None,
        }
    }
}
