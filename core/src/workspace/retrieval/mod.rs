//! Session-local workspace text catalog and incremental lexical index.
//!
//! The catalog is intentionally independent from semantic embedding and model
//! providers. It is the single source of chunk boundaries for lexical and
//! future semantic retrieval.

#[cfg(feature = "zvec-rust-fts")]
use std::sync::{OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Coordinates native workspace resources with host process creation.
///
/// The zvec C++ library opens descriptors without `O_CLOEXEC`. Native
/// indexing therefore takes the write side of this gate for its complete
/// open/query/close scope, while every Code-owned child-process spawn takes
/// the read side for the fork/exec boundary. This also covers macOS
/// `posix_spawn`, for which `pthread_atfork` handlers are not guaranteed to
/// run.
#[cfg(feature = "zvec-rust-fts")]
static NATIVE_RESOURCE_GATE: OnceLock<RwLock<()>> = OnceLock::new();

#[cfg(feature = "zvec-rust-fts")]
pub(crate) struct NativeResourceOperationGuard {
    _guard: RwLockWriteGuard<'static, ()>,
}

#[cfg(feature = "zvec-rust-fts")]
pub(crate) struct NativeProcessSpawnGuard {
    _guard: RwLockReadGuard<'static, ()>,
}

#[cfg(feature = "zvec-rust-fts")]
pub(crate) fn native_resource_operation() -> Result<NativeResourceOperationGuard, String> {
    NATIVE_RESOURCE_GATE
        .get_or_init(|| RwLock::new(()))
        .write()
        .map(|guard| NativeResourceOperationGuard { _guard: guard })
        .map_err(|_| "native workspace resource gate poisoned".to_owned())
}

#[cfg(feature = "zvec-rust-fts")]
pub(crate) fn native_process_spawn() -> NativeProcessSpawnGuard {
    let guard = NATIVE_RESOURCE_GATE
        .get_or_init(|| RwLock::new(()))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    NativeProcessSpawnGuard { _guard: guard }
}

#[cfg(not(feature = "zvec-rust-fts"))]
pub(crate) struct NativeProcessSpawnGuard;

#[cfg(not(feature = "zvec-rust-fts"))]
pub(crate) fn native_process_spawn() -> NativeProcessSpawnGuard {
    NativeProcessSpawnGuard
}

mod catalog;
mod chunk;
mod chunking_strategy;
mod eligibility;
mod hybrid_candidates;
mod hybrid_query;
mod hybrid_rank;
mod hybrid_types;
pub(crate) mod lexical;
mod memory_vector_adapter;
mod persistent;
mod reconcile;
mod rerank;
mod runtime;
mod semantic_batch;
mod semantic_projection;
mod semantic_query;
mod semantic_runtime;
mod semantic_status;
mod semantic_types;
mod types;
mod vector_contract;
mod verification;
#[cfg(feature = "zvec-rust-fts")]
mod zvec_rust;

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
    WorkspaceRerankAlgorithm, WorkspaceRerankFallbackReason, WorkspaceRerankMode,
    WorkspaceRerankOptions, WorkspaceRerankStatus, WorkspaceRetrievalChannel,
};
pub use lexical::{LexicalSearchHit, LexicalSearchRequest, LexicalSearchResult};
pub use persistent::{
    WorkspacePersistentIndex, WorkspacePersistentIndexPhase, WorkspacePersistentIndexStatus,
};
pub(crate) use runtime::LocalWorkspaceCatalogRuntime;
pub use semantic_runtime::WorkspaceRetrievalRuntime;
pub use semantic_types::{
    WorkspaceEmbeddingBatchMetrics, WorkspaceRetrievalError, WorkspaceRetrievalOptions,
    WorkspaceRetrievalPhase, WorkspaceRetrievalResult, WorkspaceRetrievalStatus,
    WorkspaceSemanticFallbackReason, WorkspaceSemanticIndexLimits, WorkspaceSemanticSearchHit,
    WorkspaceSemanticSearchRequest, WorkspaceSemanticSearchResult,
};
pub use types::{
    ChunkCatalogLimits, ChunkingConfig, WorkspaceChunk, WorkspaceChunkId, WorkspaceIndexError,
    WorkspaceLexicalEngine,
};
pub(crate) use verification::retain_verified;

#[cfg(test)]
mod tests;
