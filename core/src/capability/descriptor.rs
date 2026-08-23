use std::collections::BTreeMap;

use serde::Serialize;

use super::{
    CapabilityId, CapabilityKind, CapabilitySetError, CapabilitySource, Sha256Digest,
    MAX_CAPABILITIES,
};

pub const MAX_CAPABILITY_DEPENDENCIES: usize = 128;
const MAX_CAPABILITY_PUBLIC_NAME_BYTES: usize = 256;

/// Serializable identity plane for one projected capability surface.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    id: CapabilityId,
    public_name: Box<str>,
    surface_digest: Sha256Digest,
    dependencies: Vec<CapabilityId>,
}

impl CapabilityDescriptor {
    pub fn new(
        source: &CapabilitySource,
        kind: CapabilityKind,
        local_id: impl Into<String>,
        public_name: impl Into<String>,
        surface_digest: Sha256Digest,
        dependencies: impl IntoIterator<Item = CapabilityId>,
    ) -> Result<Self, CapabilitySetError> {
        let id = CapabilityId::new(source, kind, local_id)?;
        let public_name = public_name.into();
        validate_public_name(&public_name)?;
        let mut dependencies = dependencies.into_iter().collect::<Vec<_>>();
        if dependencies.len() > MAX_CAPABILITY_DEPENDENCIES {
            return Err(CapabilitySetError::BoundExceeded {
                field: "dependencies",
                max: MAX_CAPABILITY_DEPENDENCIES,
            });
        }
        dependencies.sort();
        for pair in dependencies.windows(2) {
            if pair[0] == pair[1] {
                return Err(CapabilitySetError::DuplicateDependency {
                    capability: id.to_string(),
                    dependency: pair[0].to_string(),
                });
            }
        }
        if dependencies.binary_search(&id).is_ok() {
            return Err(CapabilitySetError::SelfDependency {
                capability: id.to_string(),
            });
        }
        Ok(Self {
            id,
            public_name: public_name.into_boxed_str(),
            surface_digest,
            dependencies,
        })
    }

    pub fn id(&self) -> &CapabilityId {
        &self.id
    }

    pub fn public_name(&self) -> &str {
        &self.public_name
    }

    pub fn surface_digest(&self) -> &Sha256Digest {
        &self.surface_digest
    }

    pub fn dependencies(&self) -> &[CapabilityId] {
        &self.dependencies
    }
}

/// Complete descriptor batch owned by one exact source generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityContribution {
    source: CapabilitySource,
    descriptors: BTreeMap<CapabilityId, CapabilityDescriptor>,
}

impl CapabilityContribution {
    pub fn new(
        source: CapabilitySource,
        descriptors: impl IntoIterator<Item = CapabilityDescriptor>,
    ) -> Result<Self, CapabilitySetError> {
        let mut canonical = BTreeMap::new();
        for descriptor in descriptors {
            if canonical.len() >= MAX_CAPABILITIES {
                return Err(CapabilitySetError::BoundExceeded {
                    field: "capabilities",
                    max: MAX_CAPABILITIES,
                });
            }
            if descriptor.id().source() != source.id() {
                return Err(CapabilitySetError::SourceMismatch {
                    capability: descriptor.id().to_string(),
                    expected_source: source.id().to_string(),
                    actual_source: descriptor.id().source().to_string(),
                });
            }
            let id = descriptor.id().clone();
            if canonical.insert(id.clone(), descriptor).is_some() {
                return Err(CapabilitySetError::DuplicateCapability {
                    capability: id.to_string(),
                });
            }
        }
        if canonical.is_empty() {
            return Err(CapabilitySetError::EmptyContribution {
                source_id: source.id().to_string(),
            });
        }
        Ok(Self {
            source,
            descriptors: canonical,
        })
    }

    pub fn source(&self) -> &CapabilitySource {
        &self.source
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&CapabilityId, &CapabilityDescriptor)> {
        self.descriptors.iter()
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        CapabilitySource,
        BTreeMap<CapabilityId, CapabilityDescriptor>,
    ) {
        (self.source, self.descriptors)
    }
}

fn validate_public_name(value: &str) -> Result<(), CapabilitySetError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(CapabilitySetError::InvalidIdentifier {
            field: "public_name",
            reason: "it is empty, padded, or contains control characters",
        });
    }
    if value.len() > MAX_CAPABILITY_PUBLIC_NAME_BYTES {
        return Err(CapabilitySetError::BoundExceeded {
            field: "public_name",
            max: MAX_CAPABILITY_PUBLIC_NAME_BYTES,
        });
    }
    Ok(())
}
