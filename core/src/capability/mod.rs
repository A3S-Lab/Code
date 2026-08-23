//! Immutable, source-owned capability identity plane.
//!
//! This module deliberately contains no package resolution, runtime
//! activation, or mutable latest-value registry. A capability source submits
//! one complete contribution, and [`CapabilitySet`] freezes the validated
//! descriptors behind an [`std::sync::Arc`]. Later projection gates attach
//! category-specific runtime values and publication transactions to this
//! identity plane.

mod descriptor;
mod error;
mod id;
mod set;
mod source;

pub use descriptor::{CapabilityContribution, CapabilityDescriptor, MAX_CAPABILITY_DEPENDENCIES};
pub use error::CapabilitySetError;
pub use id::{
    CapabilityId, CapabilityKind, CapabilitySourceId, CodeCatalogGeneration, Sha256Digest,
    UseCapabilityGeneration, UsePackageGeneration, MAX_CAPABILITY_IDENTIFIER_BYTES,
    USE_CAPABILITY_SNAPSHOT_CURSOR_SCHEMA,
};
pub use set::{
    CapabilitySet, CAPABILITY_SET_DIGEST_DOMAIN, CAPABILITY_SET_SCHEMA, MAX_CAPABILITIES,
    MAX_CAPABILITY_CANONICAL_BYTES, MAX_CAPABILITY_DEPENDENCY_EDGES, MAX_CAPABILITY_SOURCES,
};
pub use source::{CapabilitySource, CapabilitySourceClass};
