use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    CapabilityContribution, CapabilityDescriptor, CapabilityId, CapabilityKind, CapabilitySetError,
    CapabilitySource, CapabilitySourceClass, CapabilitySourceId, CodeCatalogGeneration,
    Sha256Digest,
};

pub const CAPABILITY_SET_SCHEMA: &str = "a3s.code.capability-set.v1";
pub const CAPABILITY_SET_DIGEST_DOMAIN: &str = "a3s.code.capability-set.v1";
pub const MAX_CAPABILITY_SOURCES: usize = 1_024;
pub const MAX_CAPABILITIES: usize = 4_096;
pub const MAX_CAPABILITY_DEPENDENCY_EDGES: usize = 32_768;
pub const MAX_CAPABILITY_CANONICAL_BYTES: u64 = 16 * 1024 * 1024;

const CAPABILITY_DIGEST_PREFIX: &[u8] = b"a3s-code-capability\0";

/// Canonically ordered immutable capability identity set.
///
/// Construction returns an [`Arc`] so readers retain the exact generation
/// without consulting a mutable latest-value registry.
#[derive(Debug)]
pub struct CapabilitySet {
    generation: CodeCatalogGeneration,
    digest: Sha256Digest,
    sources: BTreeMap<CapabilitySourceId, CapabilitySource>,
    descriptors: BTreeMap<CapabilityId, CapabilityDescriptor>,
    use_generation: Option<super::UseCapabilityGeneration>,
}

impl CapabilitySet {
    pub const fn schema(&self) -> &'static str {
        CAPABILITY_SET_SCHEMA
    }

    pub fn empty() -> Result<Arc<Self>, CapabilitySetError> {
        Self::from_contributions(
            CodeCatalogGeneration::INITIAL,
            Vec::<CapabilityContribution>::new(),
        )
    }

    pub fn from_contributions(
        generation: CodeCatalogGeneration,
        contributions: impl IntoIterator<Item = CapabilityContribution>,
    ) -> Result<Arc<Self>, CapabilitySetError> {
        Self::build(generation, None, contributions)
    }

    /// Freeze a complete product projection while retaining its upstream Use
    /// cursor even when product filtering yields no package descriptors.
    pub fn from_use_projection(
        generation: CodeCatalogGeneration,
        use_generation: super::UseCapabilityGeneration,
        contributions: impl IntoIterator<Item = CapabilityContribution>,
    ) -> Result<Arc<Self>, CapabilitySetError> {
        Self::build(generation, Some(use_generation), contributions)
    }

    fn build(
        generation: CodeCatalogGeneration,
        expected_use_generation: Option<super::UseCapabilityGeneration>,
        contributions: impl IntoIterator<Item = CapabilityContribution>,
    ) -> Result<Arc<Self>, CapabilitySetError> {
        let mut sources = BTreeMap::new();
        let mut descriptors = BTreeMap::new();
        let mut public_names = BTreeMap::<(CapabilityKind, Box<str>), CapabilitySourceClass>::new();
        let mut dependency_edges = 0_usize;
        let mut use_generation = expected_use_generation;

        for contribution in contributions {
            if sources.len() >= MAX_CAPABILITY_SOURCES {
                return Err(CapabilitySetError::BoundExceeded {
                    field: "sources",
                    max: MAX_CAPABILITY_SOURCES,
                });
            }
            let (source, contributed) = contribution.into_parts();
            let source_id = source.id().clone();
            if sources.contains_key(&source_id) {
                return Err(CapabilitySetError::DuplicateSource {
                    source_id: source_id.to_string(),
                });
            }
            if let Some(observed) = source.use_capability_generation() {
                match &use_generation {
                    Some(expected) if expected != observed => {
                        return Err(CapabilitySetError::MixedUseGeneration {
                            expected_generation: expected.generation(),
                            actual_generation: observed.generation(),
                            revision_mismatch: expected.revision() != observed.revision(),
                            registry_revision_mismatch: expected.registry_revision()
                                != observed.registry_revision(),
                        });
                    }
                    None => use_generation = Some(observed.clone()),
                    Some(_) => {}
                }
            }

            for (id, descriptor) in contributed {
                if descriptors.len() >= MAX_CAPABILITIES {
                    return Err(CapabilitySetError::BoundExceeded {
                        field: "capabilities",
                        max: MAX_CAPABILITIES,
                    });
                }
                dependency_edges = dependency_edges
                    .checked_add(descriptor.dependencies().len())
                    .ok_or(CapabilitySetError::BoundExceeded {
                        field: "dependency_edges",
                        max: MAX_CAPABILITY_DEPENDENCY_EDGES,
                    })?;
                if dependency_edges > MAX_CAPABILITY_DEPENDENCY_EDGES {
                    return Err(CapabilitySetError::BoundExceeded {
                        field: "dependency_edges",
                        max: MAX_CAPABILITY_DEPENDENCY_EDGES,
                    });
                }
                let public_key = (
                    id.kind(),
                    descriptor.public_name().to_owned().into_boxed_str(),
                );
                if let Some(existing_class) = public_names.get(&public_key) {
                    let error = if *existing_class == CapabilitySourceClass::BuiltIn
                        || source.class() == CapabilitySourceClass::BuiltIn
                    {
                        CapabilitySetError::BuiltinShadow {
                            kind: id.kind(),
                            public_name: descriptor.public_name().to_owned(),
                        }
                    } else {
                        CapabilitySetError::PublicNameConflict {
                            kind: id.kind(),
                            public_name: descriptor.public_name().to_owned(),
                        }
                    };
                    return Err(error);
                }
                public_names.insert(public_key, source.class());
                if descriptors.insert(id.clone(), descriptor).is_some() {
                    return Err(CapabilitySetError::DuplicateCapability {
                        capability: id.to_string(),
                    });
                }
            }
            sources.insert(source_id, source);
        }

        for (id, descriptor) in &descriptors {
            for dependency in descriptor.dependencies() {
                if !descriptors.contains_key(dependency) {
                    return Err(CapabilitySetError::MissingDependency {
                        capability: id.to_string(),
                        dependency: dependency.to_string(),
                    });
                }
            }
        }

        let digest = canonical_digest(generation, use_generation.as_ref(), &sources, &descriptors)?;
        Ok(Arc::new(Self {
            generation,
            digest,
            sources,
            descriptors,
            use_generation,
        }))
    }

    pub const fn generation(&self) -> CodeCatalogGeneration {
        self.generation
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    pub fn use_capability_generation(&self) -> Option<&super::UseCapabilityGeneration> {
        self.use_generation.as_ref()
    }

    pub fn source(&self, id: &CapabilitySourceId) -> Option<&CapabilitySource> {
        self.sources.get(id)
    }

    pub fn get(&self, id: &CapabilityId) -> Option<&CapabilityDescriptor> {
        self.descriptors.get(id)
    }

    pub fn contains(&self, id: &CapabilityId) -> bool {
        self.descriptors.contains_key(id)
    }

    pub fn sources(
        &self,
    ) -> impl ExactSizeIterator<Item = (&CapabilitySourceId, &CapabilitySource)> {
        self.sources.iter()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&CapabilityId, &CapabilityDescriptor)> {
        self.descriptors.iter()
    }
}

fn canonical_digest(
    generation: CodeCatalogGeneration,
    use_generation: Option<&super::UseCapabilityGeneration>,
    sources: &BTreeMap<CapabilitySourceId, CapabilitySource>,
    descriptors: &BTreeMap<CapabilityId, CapabilityDescriptor>,
) -> Result<Sha256Digest, CapabilitySetError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Identity<'a> {
        schema: &'static str,
        generation: CodeCatalogGeneration,
        use_generation: Option<&'a super::UseCapabilityGeneration>,
        sources: Vec<&'a CapabilitySource>,
        capabilities: Vec<&'a CapabilityDescriptor>,
    }

    let identity = Identity {
        schema: CAPABILITY_SET_SCHEMA,
        generation,
        use_generation,
        sources: sources.values().collect(),
        capabilities: descriptors.values().collect(),
    };
    let mut writer = DigestWriter::new(CAPABILITY_SET_DIGEST_DOMAIN);
    let encoded = serde_json::to_writer(&mut writer, &identity);
    if writer.exceeded {
        return Err(CapabilitySetError::BoundExceeded {
            field: "canonical_bytes",
            max: usize::try_from(MAX_CAPABILITY_CANONICAL_BYTES).unwrap_or(usize::MAX),
        });
    }
    encoded.map_err(|error| CapabilitySetError::CanonicalEncoding(error.to_string()))?;
    Sha256Digest::new(writer.finish())
}

struct DigestWriter {
    hasher: Sha256,
    bytes: u64,
    exceeded: bool,
}

impl DigestWriter {
    fn new(domain: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(CAPABILITY_DIGEST_PREFIX);
        hasher.update((domain.len() as u64).to_be_bytes());
        hasher.update(domain.as_bytes());
        Self {
            hasher,
            bytes: 0,
            exceeded: false,
        }
    }

    fn finish(mut self) -> String {
        self.hasher.update(self.bytes.to_be_bytes());
        format!("sha256:{:x}", self.hasher.finalize())
    }
}

impl Write for DigestWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let buffer_len = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if self.bytes.saturating_add(buffer_len) > MAX_CAPABILITY_CANONICAL_BYTES {
            self.exceeded = true;
            return Err(std::io::Error::other(
                "canonical capability set exceeds its byte bound",
            ));
        }
        self.hasher.update(buffer);
        self.bytes += buffer_len;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: char) -> Sha256Digest {
        Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
    }

    #[test]
    fn external_sources_cannot_shadow_a_sealed_builtin() {
        let builtin = CapabilitySource::builtin("a3s-code", digest('a')).unwrap();
        let external = CapabilitySource::host("desktop", digest('b')).unwrap();
        let builtin_read = CapabilityDescriptor::new(
            &builtin,
            CapabilityKind::Tool,
            "read",
            "read",
            digest('c'),
            [],
        )
        .unwrap();
        let external_read = CapabilityDescriptor::new(
            &external,
            CapabilityKind::Tool,
            "workspace-read",
            "read",
            digest('d'),
            [],
        )
        .unwrap();

        for contributions in [
            vec![
                CapabilityContribution::new(builtin.clone(), vec![builtin_read.clone()]).unwrap(),
                CapabilityContribution::new(external.clone(), vec![external_read.clone()]).unwrap(),
            ],
            vec![
                CapabilityContribution::new(external.clone(), vec![external_read.clone()]).unwrap(),
                CapabilityContribution::new(builtin.clone(), vec![builtin_read.clone()]).unwrap(),
            ],
        ] {
            let error =
                CapabilitySet::from_contributions(CodeCatalogGeneration::new(1), contributions)
                    .unwrap_err();
            assert!(matches!(error, CapabilitySetError::BuiltinShadow { .. }));
        }
    }
}
