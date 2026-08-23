use thiserror::Error;

use super::CapabilityKind;

/// Validation failure that prevents an immutable capability set from being
/// constructed.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CapabilitySetError {
    #[error("Capability field '{field}' is not a canonical identifier: {reason}")]
    InvalidIdentifier {
        field: &'static str,
        reason: &'static str,
    },
    #[error("Capability field '{field}' is not a canonical SHA-256 digest")]
    InvalidDigest { field: &'static str },
    #[error("Capability generation field '{field}' is invalid")]
    InvalidGeneration { field: &'static str },
    #[error("Capability field '{field}' exceeds its bound of {max}")]
    BoundExceeded { field: &'static str, max: usize },
    #[error("Capability source '{source_id}' submitted an empty contribution")]
    EmptyContribution { source_id: String },
    #[error(
        "Capability '{capability}' belongs to source '{actual_source}', not contribution source '{expected_source}'"
    )]
    SourceMismatch {
        capability: String,
        expected_source: String,
        actual_source: String,
    },
    #[error("Capability source '{source_id}' was contributed more than once")]
    DuplicateSource { source_id: String },
    #[error(
        "A capability set mixes A3S Use cursor identities (generation {expected_generation} vs {actual_generation}, capability revision mismatch: {revision_mismatch}, Registry revision mismatch: {registry_revision_mismatch})"
    )]
    MixedUseGeneration {
        expected_generation: u64,
        actual_generation: u64,
        revision_mismatch: bool,
        registry_revision_mismatch: bool,
    },
    #[error("Capability '{capability}' was contributed more than once")]
    DuplicateCapability { capability: String },
    #[error("Capability '{capability}' repeats dependency '{dependency}'")]
    DuplicateDependency {
        capability: String,
        dependency: String,
    },
    #[error("Capability '{capability}' depends on itself")]
    SelfDependency { capability: String },
    #[error("Capability '{capability}' depends on missing capability '{dependency}'")]
    MissingDependency {
        capability: String,
        dependency: String,
    },
    #[error("External {kind} capability '{public_name}' cannot shadow a built-in")]
    BuiltinShadow {
        kind: CapabilityKind,
        public_name: String,
    },
    #[error("Multiple {kind} capabilities publish the same name '{public_name}'")]
    PublicNameConflict {
        kind: CapabilityKind,
        public_name: String,
    },
    #[error("The canonical capability set could not be encoded: {0}")]
    CanonicalEncoding(String),
}
