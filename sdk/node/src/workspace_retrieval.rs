//! Typed Node.js bridge for session-bound workspace retrieval.

use super::*;
use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingNormalization, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::{
    WorkspaceHybridChannelRank, WorkspaceHybridChannelStatus, WorkspaceHybridSearchHit,
    WorkspaceHybridSearchRequest as RustHybridSearchRequest,
    WorkspaceHybridSearchResult as RustHybridSearchResult, WorkspaceRetrievalStatus,
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

mod options;
mod provider;
mod session_api;
mod types;

pub use options::{WorkspaceRetrievalOptions, WorkspaceRetrievalOptionsObject};
pub use provider::CallbackEmbeddingProvider;
pub use types::*;

use options::embedding_provider_registry;
pub(super) use options::js_workspace_retrieval_to_rust;
use provider::NodeEmbeddingProvider;
use types::{hybrid_request, semantic_request};
