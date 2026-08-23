use thiserror::Error;

/// Failure to construct, narrow, admit, or operate a capability scope.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CapabilityScopeError {
    #[error("Capability scope identifier is invalid: {reason}")]
    InvalidScopeId { reason: &'static str },
    #[error("Capability lifecycle name is invalid: {reason}")]
    InvalidLifecycleName { reason: &'static str },
    #[error("Capability execution limit '{field}' must be positive")]
    InvalidExecutionLimit { field: &'static str },
    #[error("Capability scope field '{field}' exceeds its bound of {max}")]
    BoundExceeded { field: &'static str, max: usize },
    #[error("Capability '{capability}' is repeated in one ceiling")]
    DuplicateCeilingCapability { capability: String },
    #[error("Capability '{capability}' is not present in catalog '{catalog_digest}'")]
    CapabilityOutsideCatalog {
        capability: String,
        catalog_digest: String,
    },
    #[error("Capability ceiling belongs to another immutable catalog")]
    CeilingCatalogMismatch,
    #[error("Child capability scope broadens parent ceiling dimension '{dimension}'")]
    CeilingExpansion { dimension: &'static str },
    #[error("Catalog requires an exact A3S Use generation lease for Run admission")]
    MissingUseGenerationLease,
    #[error("Catalog has no A3S Use generation but Run admission supplied a Use lease")]
    UnexpectedUseGenerationLease,
    #[error(
        "A3S Use Run lease does not match the catalog cursor (generation {expected_generation} vs {actual_generation}, capability revision mismatch: {revision_mismatch}, Registry revision mismatch: {registry_revision_mismatch})"
    )]
    UseGenerationLeaseMismatch {
        expected_generation: u64,
        actual_generation: u64,
        revision_mismatch: bool,
        registry_revision_mismatch: bool,
    },
    #[error("Capability scope '{scope_id}' is no longer active")]
    ScopeInactive { scope_id: String },
    #[error("Capability scope '{scope_id}' no longer accepts lifecycle registrations")]
    SupervisorClosed { scope_id: String },
    #[error("Capability scope requires an active Tokio runtime")]
    TokioRuntimeUnavailable,
    #[error("Capability supervisor task identity is exhausted")]
    TaskIdentityExhausted,
    #[error("Capability supervisor child identity is exhausted")]
    ChildIdentityExhausted,
    #[error("Capability child scope '{scope_id}' is already active")]
    DuplicateChildScope { scope_id: String },
}
