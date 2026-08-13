//! Session-local workspace text catalog and incremental lexical index.
//!
//! The catalog is intentionally independent from semantic embedding and model
//! providers. It is the single source of chunk boundaries for lexical and
//! future semantic retrieval.

mod catalog;
mod chunk;
mod eligibility;
pub(crate) mod lexical;
mod reconcile;
mod runtime;
mod types;

pub use catalog::{ChunkCatalogSnapshot, WorkspaceChunkCatalog};
pub use eligibility::WorkspaceEligibilityPolicy;
pub use lexical::{LexicalSearchHit, LexicalSearchRequest, LexicalSearchResult};
pub(crate) use runtime::LocalWorkspaceCatalogRuntime;
pub use types::{
    ChunkCatalogLimits, ChunkingConfig, WorkspaceChunk, WorkspaceChunkId, WorkspaceIndexError,
};

#[cfg(test)]
mod tests;
