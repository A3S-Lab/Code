//! Session-local workspace text catalog and incremental lexical index.
//!
//! The catalog is intentionally independent from semantic embedding and model
//! providers. It is the single source of chunk boundaries for lexical and
//! future semantic retrieval.

mod catalog;
mod chunk;
mod chunking_strategy;
mod eligibility;
mod hybrid_candidates;
mod hybrid_query;
mod hybrid_rank;
mod hybrid_types;
pub(crate) mod lexical;
mod reconcile;
mod runtime;
mod semantic_query;
mod semantic_runtime;
mod semantic_types;
mod types;
mod verification;

pub use catalog::{ChunkCatalogSnapshot, WorkspaceChunkCatalog};
pub(crate) use chunk::digest_content;
pub use chunking_strategy::{
    CustomWorkspaceChunkingStrategy, FixedWindowChunkingOptions, RecursiveChunkingOptions,
    WorkspaceChunkRange, WorkspaceChunkingError, WorkspaceChunkingInput, WorkspaceChunkingStrategy,
};
pub use eligibility::WorkspaceEligibilityPolicy;
pub use hybrid_types::{
    WorkspaceHybridChannelRank, WorkspaceHybridChannelStatus, WorkspaceHybridFallbackReason,
    WorkspaceHybridSearchHit, WorkspaceHybridSearchRequest, WorkspaceHybridSearchResult,
    WorkspaceRetrievalChannel,
};
pub use lexical::{LexicalSearchHit, LexicalSearchRequest, LexicalSearchResult};
pub(crate) use runtime::LocalWorkspaceCatalogRuntime;
pub use semantic_runtime::WorkspaceRetrievalRuntime;
pub use semantic_types::{
    WorkspaceRetrievalError, WorkspaceRetrievalOptions, WorkspaceRetrievalPhase,
    WorkspaceRetrievalResult, WorkspaceRetrievalStatus, WorkspaceSemanticFallbackReason,
    WorkspaceSemanticIndexLimits, WorkspaceSemanticSearchHit, WorkspaceSemanticSearchRequest,
    WorkspaceSemanticSearchResult,
};
pub use types::{
    ChunkCatalogLimits, ChunkingConfig, WorkspaceChunk, WorkspaceChunkId, WorkspaceIndexError,
};
pub(crate) use verification::retain_verified;

#[cfg(test)]
mod tests;
