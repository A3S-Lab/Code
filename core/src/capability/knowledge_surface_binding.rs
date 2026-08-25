use std::fmt;

use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{CapabilitySetError, Sha256Digest};

pub const KNOWLEDGE_SURFACE_BINDING_SCHEMA: &str = "a3s.code.knowledge-surface-binding.v1";
pub const MAX_KNOWLEDGE_SURFACE_PROJECTIONS: usize = 256;

const MAX_KNOWLEDGE_PUBLIC_NAME_BYTES: usize = 256;
const MAX_KNOWLEDGE_FORMAT_VERSION_BYTES: usize = 64;
const KNOWLEDGE_SURFACE_DIGEST_PREFIX: &[u8] = b"a3s-code-knowledge-surface\0";

/// Host-reviewed, path-free readiness evidence for one Knowledge surface.
///
/// This value is deliberately not a cognitive context provider. Multiple
/// package Knowledge surfaces may coexist in one catalog and satisfy exact
/// readiness edges for host capabilities such as Flow. A separately selected
/// [`crate::CognitiveContextSession`] remains the singular Run-visible
/// cognitive authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgeSurfaceBindingSpec {
    pub public_name: String,
    pub format_version: String,
    pub content_digest: Sha256Digest,
    pub projection_digests: Vec<Sha256Digest>,
}

/// Immutable multi-instance Knowledge readiness value accepted by the Code
/// capability projection kernel.
///
/// Projection digests are opaque host evidence. Code canonicalizes and binds
/// them but does not interpret scopes, open package files, query an index, or
/// select a cognitive package.
#[derive(Clone, Eq, PartialEq)]
pub struct KnowledgeSurfaceBinding {
    public_name: Box<str>,
    format_version: Box<str>,
    content_digest: Sha256Digest,
    projection_digests: Box<[Sha256Digest]>,
    surface_digest: Sha256Digest,
}

impl KnowledgeSurfaceBinding {
    pub fn new(
        mut spec: KnowledgeSurfaceBindingSpec,
    ) -> Result<Self, KnowledgeSurfaceBindingError> {
        validate_required_text(
            "public_name",
            &spec.public_name,
            MAX_KNOWLEDGE_PUBLIC_NAME_BYTES,
        )?;
        validate_required_text(
            "format_version",
            &spec.format_version,
            MAX_KNOWLEDGE_FORMAT_VERSION_BYTES,
        )?;
        if spec.projection_digests.is_empty() {
            return Err(KnowledgeSurfaceBindingError::MissingProjectionEvidence);
        }
        if spec.projection_digests.len() > MAX_KNOWLEDGE_SURFACE_PROJECTIONS {
            return Err(KnowledgeSurfaceBindingError::ProjectionCountExceeded {
                max: MAX_KNOWLEDGE_SURFACE_PROJECTIONS,
            });
        }
        spec.projection_digests.sort();
        if spec
            .projection_digests
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        {
            return Err(KnowledgeSurfaceBindingError::DuplicateProjectionDigest);
        }
        let surface_digest = binding_digest(&spec)?;
        Ok(Self {
            public_name: spec.public_name.into_boxed_str(),
            format_version: spec.format_version.into_boxed_str(),
            content_digest: spec.content_digest,
            projection_digests: spec.projection_digests.into_boxed_slice(),
            surface_digest,
        })
    }

    pub fn public_name(&self) -> &str {
        &self.public_name
    }

    pub fn format_version(&self) -> &str {
        &self.format_version
    }

    pub fn content_digest(&self) -> &Sha256Digest {
        &self.content_digest
    }

    pub fn projection_digests(&self) -> &[Sha256Digest] {
        &self.projection_digests
    }

    pub fn surface_digest(&self) -> &Sha256Digest {
        &self.surface_digest
    }
}

impl fmt::Debug for KnowledgeSurfaceBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KnowledgeSurfaceBinding")
            .field("public_name", &self.public_name)
            .field("format_version", &self.format_version)
            .field("content_digest", &self.content_digest)
            .field("projection_count", &self.projection_digests.len())
            .field("surface_digest", &self.surface_digest)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum KnowledgeSurfaceBindingError {
    #[error("Knowledge surface field '{field}' is invalid: {reason}")]
    InvalidText {
        field: &'static str,
        reason: &'static str,
    },
    #[error("Knowledge surface field '{field}' exceeds its byte bound of {max}")]
    TextTooLarge { field: &'static str, max: usize },
    #[error("Knowledge surface readiness requires at least one exact projection digest")]
    MissingProjectionEvidence,
    #[error("Knowledge surface readiness contains more than {max} projection digests")]
    ProjectionCountExceeded { max: usize },
    #[error("Knowledge surface readiness repeats one exact projection digest")]
    DuplicateProjectionDigest,
    #[error("Knowledge surface digest construction violated the canonical SHA-256 invariant")]
    DigestInvariant,
}

fn validate_required_text(
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), KnowledgeSurfaceBindingError> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(KnowledgeSurfaceBindingError::InvalidText {
            field,
            reason: "it is empty, padded, or contains control characters",
        });
    }
    if value.len() > max {
        return Err(KnowledgeSurfaceBindingError::TextTooLarge { field, max });
    }
    Ok(())
}

fn binding_digest(
    spec: &KnowledgeSurfaceBindingSpec,
) -> Result<Sha256Digest, KnowledgeSurfaceBindingError> {
    let mut hasher = Sha256::new();
    hasher.update(KNOWLEDGE_SURFACE_DIGEST_PREFIX);
    hash_field(&mut hasher, KNOWLEDGE_SURFACE_BINDING_SCHEMA.as_bytes());
    hash_field(&mut hasher, spec.public_name.as_bytes());
    hash_field(&mut hasher, spec.format_version.as_bytes());
    hash_field(&mut hasher, spec.content_digest.as_str().as_bytes());
    hash_field(
        &mut hasher,
        &(spec.projection_digests.len() as u64).to_be_bytes(),
    );
    for digest in &spec.projection_digests {
        hash_field(&mut hasher, digest.as_str().as_bytes());
    }
    Sha256Digest::new(format!("sha256:{:x}", hasher.finalize())).map_err(map_digest_error)
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn map_digest_error(_error: CapabilitySetError) -> KnowledgeSurfaceBindingError {
    KnowledgeSurfaceBindingError::DigestInvariant
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: char) -> Sha256Digest {
        Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
    }

    #[test]
    fn projection_evidence_is_canonical_and_bounded() {
        let binding = KnowledgeSurfaceBinding::new(KnowledgeSurfaceBindingSpec {
            public_name: "research:domain".to_owned(),
            format_version: "0.2".to_owned(),
            content_digest: digest('a'),
            projection_digests: vec![digest('c'), digest('b')],
        })
        .unwrap();
        assert_eq!(binding.projection_digests(), &[digest('b'), digest('c')]);

        assert!(matches!(
            KnowledgeSurfaceBinding::new(KnowledgeSurfaceBindingSpec {
                public_name: "research:domain".to_owned(),
                format_version: "0.2".to_owned(),
                content_digest: digest('a'),
                projection_digests: vec![digest('b'), digest('b')],
            }),
            Err(KnowledgeSurfaceBindingError::DuplicateProjectionDigest)
        ));
        assert!(matches!(
            KnowledgeSurfaceBinding::new(KnowledgeSurfaceBindingSpec {
                public_name: "research:domain".to_owned(),
                format_version: "0.2".to_owned(),
                content_digest: digest('a'),
                projection_digests: Vec::new(),
            }),
            Err(KnowledgeSurfaceBindingError::MissingProjectionEvidence)
        ));
    }
}
