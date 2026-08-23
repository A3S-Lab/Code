//! Immutable, source-owned capability identity and scoped lifecycle kernel.
//!
//! This module deliberately contains no package resolution, runtime
//! activation, or mutable latest-value registry. A capability source submits
//! one complete contribution, and [`CapabilitySet`] freezes the validated
//! descriptors behind an [`std::sync::Arc`]. Typed scopes add monotonic
//! ceilings, borrowed leases, reversible effects, and bounded structured
//! teardown. Closed runtime values and typestate transactions publish one
//! complete projected generation through a short catalog CAS.

mod ceiling;
mod descriptor;
mod effect;
mod error;
mod id;
mod lease;
mod projection;
mod projection_error;
mod scope;
mod scope_error;
mod set;
mod source;
mod supervisor;
mod transaction;
mod value;

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
pub use projection::{
    CapabilityCatalog, CapabilityCatalogStamp, CapabilityCleanupReport, CapabilityCommitReceipt,
    CapabilityProjection, CapabilityProjectionLease,
};
pub use projection_error::{CapabilityAdapterError, CapabilityProjectionError};
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
pub use transaction::{
    CapabilityProjectionAdapter, CapabilityTxn, Prepared, PreparedCapability, Staged, Validated,
    MAX_CAPABILITY_TRANSACTION_EFFECTS,
};
pub use value::{CapabilityValue, McpBinding};
