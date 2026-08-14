//! Typed Node.js bridge for session-bound workspace retrieval.

use super::*;
use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingNormalization, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::{
    WorkspaceHybridChannelRank, WorkspaceHybridChannelStatus, WorkspaceHybridSearchHit,
    WorkspaceHybridSearchRequest as RustHybridSearchRequest,
    WorkspaceHybridSearchResult as RustHybridSearchResult, WorkspaceRerankFallbackReason,
    WorkspaceRerankMode, WorkspaceRerankStatus, WorkspaceRetrievalStatus,
    WorkspaceSemanticSearchHit, WorkspaceSemanticSearchRequest as RustSemanticSearchRequest,
    WorkspaceSemanticSearchResult as RustSemanticSearchResult,
};
use async_trait::async_trait;
use napi::bindgen_prelude::Promise;
use napi::threadsafe_function::{
    ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction, ThreadsafeFunctionCallMode,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

mod chunking;
mod options;
mod provider;
mod rerank;
mod session_api;
mod types;

pub use chunking::{
    FixedWindowWorkspaceChunkingStrategy, LineWorkspaceChunkingStrategy,
    RecursiveWorkspaceChunkingStrategy,
};
pub use options::{WorkspaceRetrievalOptions, WorkspaceRetrievalOptionsObject};
pub use provider::CallbackEmbeddingProvider;
pub use rerank::DeterministicWorkspaceReranker;
pub use types::*;

use chunking::{
    bind_workspace_chunking_strategy, resolve_workspace_chunking_strategy,
    unregister_workspace_chunking_strategy, NodeWorkspaceChunkingConfiguration,
    WorkspaceChunkingStrategyInput,
};
use options::embedding_provider_registry;
pub(super) use options::js_workspace_retrieval_to_rust;
use provider::NodeEmbeddingProvider;
use rerank::{
    bind_deterministic_reranker, resolve_deterministic_reranker, unregister_deterministic_reranker,
    NodeDeterministicRerankerConfiguration,
};
use types::{hybrid_request, semantic_request};
