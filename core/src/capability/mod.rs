//! Immutable, source-owned capability identity and scoped lifecycle kernel.
//!
//! This module deliberately contains no package resolution, runtime
//! activation, or mutable latest-value registry. A capability source submits
//! one complete contribution, and [`CapabilitySet`] freezes the validated
//! descriptors behind an [`std::sync::Arc`]. Typed scopes add monotonic
//! ceilings, borrowed leases, reversible effects, and bounded structured
//! teardown. Later projection gates attach category-specific runtime values
//! and publication transactions to this kernel.

mod ceiling;
mod descriptor;
mod effect;
mod error;
mod id;
mod lease;
mod scope;
mod scope_error;
mod set;
mod source;
mod supervisor;

pub use ceiling::{
    CapabilityCeiling, CapabilityExecutionCeiling, GovernanceCapabilityCeiling,
    WorkspaceCapabilityCeiling, CAPABILITY_CEILING_SCHEMA,
};
pub use descriptor::{CapabilityContribution, CapabilityDescriptor, MAX_CAPABILITY_DEPENDENCIES};
pub use effect::{CapabilityEffect, CapabilityEffectError};
pub use error::CapabilitySetError;
pub use id::{
    CapabilityId, CapabilityKind, CapabilitySourceId, CodeCatalogGeneration, Sha256Digest,
    UseCapabilityGeneration, UsePackageGeneration, MAX_CAPABILITY_IDENTIFIER_BYTES,
    USE_CAPABILITY_SNAPSHOT_CURSOR_SCHEMA,
};
pub use lease::{CapabilityLease, RetainedUseGeneration};
pub use scope::{
    CapabilityScope, CapabilityScopeId, CapabilityScopeKind, Run, ScopeKind, Session, Subtask, Turn,
};
pub use scope_error::CapabilityScopeError;
pub use set::{
    CapabilitySet, CAPABILITY_SET_DIGEST_DOMAIN, CAPABILITY_SET_SCHEMA, MAX_CAPABILITIES,
    MAX_CAPABILITY_CANONICAL_BYTES, MAX_CAPABILITY_DEPENDENCY_EDGES, MAX_CAPABILITY_SOURCES,
};
pub use source::{CapabilitySource, CapabilitySourceClass};
pub use supervisor::{
    ScopeClosePolicy, ScopeCloseReport, SupervisedTaskId, DEFAULT_SCOPE_CLOSE_TIMEOUT,
    MAX_SCOPE_CHILDREN, MAX_SCOPE_CLOSE_TIMEOUT, MAX_SCOPE_EFFECTS, MAX_SCOPE_TASKS,
};
