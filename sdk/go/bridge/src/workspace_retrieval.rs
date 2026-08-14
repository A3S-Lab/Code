//! Go callback-backed embedding provider and workspace retrieval DTO bridge.

use super::*;
use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingNormalization, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::{
    WorkspaceHybridSearchRequest, WorkspaceHybridSearchResult, WorkspaceRetrievalStatus,
    WorkspaceSemanticSearchRequest, WorkspaceSemanticSearchResult,
};
use async_trait::async_trait;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const DEFAULT_PROVIDER_TIMEOUT_MS: u64 = 30_000;
const MAX_PROVIDER_TIMEOUT_MS: u64 = 300_000;
const MAX_SEARCH_LIMIT: usize = 25;

mod chunking;
mod rerank;
use chunking::BridgeWorkspaceChunkingStrategy;
use rerank::BridgeDeterministicWorkspaceReranker;

#[derive(Debug, Deserialize)]
pub(super) struct BridgeWorkspaceRetrievalOptions {
    handler_id: String,
    provider: String,
    model: String,
    revision: Option<String>,
    dimension: usize,
    #[serde(default)]
    normalization: BridgeEmbeddingNormalization,
    #[serde(default = "default_provider_timeout_ms")]
    provider_timeout_ms: u64,
    #[serde(default = "default_max_records")]
    max_records: usize,
    #[serde(default = "default_max_bytes")]
    max_bytes: usize,
    #[serde(default = "default_shutdown_timeout_ms")]
    shutdown_timeout_ms: u64,
    deterministic_reranker: Option<BridgeDeterministicWorkspaceReranker>,
    chunking_strategy: Option<BridgeWorkspaceChunkingStrategy>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BridgeEmbeddingNormalization {
    #[default]
    None,
    Unit,
}

impl BridgeWorkspaceRetrievalOptions {
    pub(super) fn into_core(
        self,
        client: Arc<CallbackClient>,
    ) -> Result<a3s_code_core::WorkspaceRetrievalOptions, BridgeFailure> {
        let chunking_strategy = self
            .chunking_strategy
            .map(BridgeWorkspaceChunkingStrategy::into_core)
            .transpose()?;
        let reranker = self
            .deterministic_reranker
            .map(BridgeDeterministicWorkspaceReranker::into_core)
            .transpose()?;
        if self.handler_id.trim().is_empty() {
            return Err(invalid_retrieval("handler_id must not be empty"));
        }
        if self.provider_timeout_ms == 0 || self.provider_timeout_ms > MAX_PROVIDER_TIMEOUT_MS {
            return Err(invalid_retrieval(format!(
                "provider_timeout_ms must be from 1 to {MAX_PROVIDER_TIMEOUT_MS}"
            )));
        }
        if self.max_records == 0 || self.max_bytes == 0 {
            return Err(invalid_retrieval(
                "max_records and max_bytes must be greater than zero",
            ));
        }
        if self.shutdown_timeout_ms == 0 || self.shutdown_timeout_ms > 30_000 {
            return Err(invalid_retrieval(
                "shutdown_timeout_ms must be from 1 to 30000",
            ));
        }
        let descriptor = EmbeddingProviderDescriptor {
            provider: self.provider,
            model: self.model,
            revision: self.revision,
            dimension: self.dimension,
            normalization: match self.normalization {
                BridgeEmbeddingNormalization::None => EmbeddingNormalization::None,
                BridgeEmbeddingNormalization::Unit => EmbeddingNormalization::Unit,
            },
        };
        let provider: Arc<dyn EmbeddingProvider> = Arc::new(BridgeEmbeddingProvider {
            client,
            handler_id: self.handler_id,
            descriptor,
            timeout_ms: self.provider_timeout_ms,
        });
        let mut retrieval = a3s_code_core::WorkspaceRetrievalOptions::new(provider)
            .with_index_limits(a3s_code_core::WorkspaceSemanticIndexLimits {
                max_records: self.max_records,
                max_bytes: self.max_bytes,
                shutdown_timeout: Duration::from_millis(self.shutdown_timeout_ms),
            });
        if let Some(reranker) = reranker {
            retrieval = retrieval.with_rerank_options(reranker);
        }
        if let Some(chunking_strategy) = chunking_strategy {
            retrieval = retrieval.with_chunking_strategy(chunking_strategy);
        }
        Ok(retrieval)
    }
}

fn invalid_retrieval(message: impl Into<String>) -> BridgeFailure {
    BridgeFailure::new(
        "INVALID_REQUEST",
        format!("workspace_retrieval: {}", message.into()),
    )
}

struct BridgeEmbeddingProvider {
    client: Arc<CallbackClient>,
    handler_id: String,
    descriptor: EmbeddingProviderDescriptor,
    timeout_ms: u64,
}

#[derive(Serialize)]
struct BridgeEmbeddingInput<'a> {
    id: &'a str,
    text: &'a str,
}

#[derive(Serialize)]
struct BridgeEmbeddingRequest<'a> {
    inputs: Vec<BridgeEmbeddingInput<'a>>,
    text_bytes: usize,
}

#[derive(Deserialize)]
struct BridgeEmbeddingVector {
    id: String,
    values: Vec<f32>,
}

#[derive(Deserialize)]
struct BridgeEmbeddingSuccess {
    vectors: Vec<BridgeEmbeddingVector>,
}

#[derive(Deserialize)]
struct BridgeEmbeddingFailure {
    kind: String,
    retry_after_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BridgeEmbeddingValue {
    Success(BridgeEmbeddingSuccess),
    Failure(BridgeEmbeddingFailure),
}

#[async_trait]
impl EmbeddingProvider for BridgeEmbeddingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if cancellation.is_cancelled() {
            return Err(EmbeddingProviderError::Cancelled);
        }
        let payload = serde_json::to_value(BridgeEmbeddingRequest {
            inputs: request
                .inputs()
                .iter()
                .map(|input| BridgeEmbeddingInput {
                    id: input.id(),
                    text: input.text(),
                })
                .collect(),
            text_bytes: request.text_bytes(),
        })
        .map_err(|_| EmbeddingProviderError::InvalidRequest)?;
        let invocation =
            self.client
                .invoke(&self.handler_id, "embedding", payload, self.timeout_ms);
        tokio::pin!(invocation);
        let value = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(EmbeddingProviderError::Cancelled),
            result = &mut invocation => result.map_err(map_callback_failure)?,
        };
        match serde_json::from_value::<BridgeEmbeddingValue>(value)
            .map_err(|_| EmbeddingProviderError::InvalidRequest)?
        {
            BridgeEmbeddingValue::Success(response) => Ok(EmbeddingBatchResponse::new(
                self.descriptor.clone(),
                response
                    .vectors
                    .into_iter()
                    .map(|vector| EmbeddingVector::new(vector.id, vector.values))
                    .collect(),
            )),
            BridgeEmbeddingValue::Failure(failure) => Err(provider_failure(failure)),
        }
    }
}

fn map_callback_failure(error: BridgeFailure) -> EmbeddingProviderError {
    match error.code.as_str() {
        "CALLBACK_TIMEOUT" => EmbeddingProviderError::Timeout,
        "BRIDGE_CLOSED" => EmbeddingProviderError::Unavailable { retry_after: None },
        _ => EmbeddingProviderError::Other,
    }
}

fn provider_failure(failure: BridgeEmbeddingFailure) -> EmbeddingProviderError {
    let retry_after = failure.retry_after_ms.map(Duration::from_millis);
    match failure.kind.as_str() {
        "cancelled" => EmbeddingProviderError::Cancelled,
        "timeout" => EmbeddingProviderError::Timeout,
        "rate_limited" => EmbeddingProviderError::RateLimited { retry_after },
        "unavailable" => EmbeddingProviderError::Unavailable { retry_after },
        "authentication" => EmbeddingProviderError::Authentication,
        "invalid_request" => EmbeddingProviderError::InvalidRequest,
        _ => EmbeddingProviderError::Other,
    }
}

#[derive(Deserialize)]
pub(super) struct BridgeWorkspaceSearchRequest {
    query: String,
    path: Option<String>,
    include: Option<String>,
    #[serde(default = "default_search_limit")]
    limit: usize,
}

impl BridgeWorkspaceSearchRequest {
    pub(super) fn semantic(self) -> Result<WorkspaceSemanticSearchRequest, BridgeFailure> {
        self.validate()?;
        let mut request = WorkspaceSemanticSearchRequest::new(self.query).with_limit(self.limit);
        request.path = self.path;
        request.include = self.include;
        Ok(request)
    }

    pub(super) fn hybrid(self) -> Result<WorkspaceHybridSearchRequest, BridgeFailure> {
        self.validate()?;
        let mut request = WorkspaceHybridSearchRequest::new(self.query).with_limit(self.limit);
        request.path = self.path;
        request.include = self.include;
        Ok(request)
    }

    fn validate(&self) -> Result<(), BridgeFailure> {
        if !(1..=MAX_SEARCH_LIMIT).contains(&self.limit) {
            return Err(BridgeFailure::new(
                "INVALID_REQUEST",
                format!("workspace retrieval limit must be from 1 to {MAX_SEARCH_LIMIT}"),
            ));
        }
        Ok(())
    }
}

pub(super) fn status_value(status: &WorkspaceRetrievalStatus) -> Value {
    json!({
        "phase": format!("{:?}", status.phase).to_ascii_lowercase(),
        "catalog_revision": status.catalog_revision,
        "source_revision": status.source_revision,
        "vector_revision": status.vector_revision,
        "eligible_files": status.eligible_files,
        "catalog_files": status.catalog_files,
        "catalog_chunks": status.catalog_chunks,
        "indexed_files": status.indexed_files,
        "indexed_chunks": status.indexed_chunks,
        "coverage_bps": status.coverage_bps,
        "queue_depth": status.queue_depth,
        "failed_files": status.failed_files,
        "total_failures": status.total_failures,
        "vector_records": status.vector_records,
        "vector_bytes": status.vector_bytes,
        "batching": {
            "document_inputs": status.batching.document_inputs,
            "document_text_bytes": status.batching.document_text_bytes,
            "document_batches": status.batching.document_batches,
            "document_provider_requests": status.batching.document_provider_requests,
            "batch_limit_lower_bound": status.batching.batch_limit_lower_bound,
            "input_limit_flushes": status.batching.input_limit_flushes,
            "text_byte_limit_flushes": status.batching.text_byte_limit_flushes,
            "vector_byte_limit_flushes": status.batching.vector_byte_limit_flushes,
            "generation_complete_flushes": status.batching.generation_complete_flushes,
            "time_to_first_ready_ms": status.batching.time_to_first_ready_ms,
            "non_text_inputs": status.batching.non_text_inputs,
        },
        "model": status.model,
    })
}

fn chunk_value(chunk: &a3s_code_core::WorkspaceChunk) -> Value {
    json!({
        "id": chunk.id.as_str(),
        "path": chunk.path.as_ref(),
        "language": chunk.language.as_deref(),
        "start_line": chunk.start_line,
        "end_line": chunk.end_line,
        "start_byte": chunk.start_byte,
        "end_byte": chunk.end_byte,
        "source_revision": chunk.source_revision,
        "text": chunk.text.as_ref(),
        "digest_verified": true,
    })
}

pub(super) fn semantic_result_value(result: WorkspaceSemanticSearchResult) -> Value {
    json!({
        "hits": result.hits.iter().map(|hit| json!({
            "chunk": chunk_value(&hit.chunk),
            "score": hit.score,
        })).collect::<Vec<_>>(),
        "status": status_value(&result.status),
        "searched_records": result.searched_records,
        "truncated": result.truncated,
        "fallback": result.fallback,
    })
}

pub(super) fn hybrid_result_value(result: WorkspaceHybridSearchResult) -> Value {
    json!({
        "hits": result.hits.iter().map(|hit| json!({
            "chunk": chunk_value(&hit.chunk),
            "fused_score": hit.fused_score,
            "rerank_score": hit.rerank_score,
            "redundancy_score": hit.redundancy_score,
            "exact_identifier": hit.exact_identifier,
            "channels": hit.channels.iter().map(|rank| json!({
                "channel": rank.channel,
                "rank": rank.rank,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "semantic_status": status_value(&result.semantic_status),
        "catalog_revision": result.catalog_revision,
        "source_revision": result.source_revision,
        "channels": result.channels.iter().map(|status| json!({
            "channel": status.channel,
            "candidate_count": status.candidate_count,
            "truncated": status.truncated,
            "fallback": status.fallback,
        })).collect::<Vec<_>>(),
        "rerank": rerank_value(&result.rerank),
        "truncated": result.truncated,
        "fallback": result.fallback,
    })
}

fn rerank_value(status: &a3s_code_core::WorkspaceRerankStatus) -> Value {
    json!({
        "requested_mode": status.requested_mode,
        "applied_mode": status.applied_mode,
        "algorithm": status.algorithm,
        "input_candidates": status.input_candidates,
        "evaluated_candidates": status.evaluated_candidates,
        "selected_candidates": status.selected_candidates,
        "near_duplicate_candidates": status.near_duplicate_candidates,
        "selected_near_duplicates": status.selected_near_duplicates,
        "feature_bytes": status.feature_bytes,
        "accounted_scratch_bytes": status.accounted_scratch_bytes,
        "candidate_truncated": status.candidate_truncated,
        "fallback": status.fallback,
    })
}

pub(super) fn retrieval_failure(error: a3s_code_core::WorkspaceRetrievalError) -> BridgeFailure {
    BridgeFailure::new("WORKSPACE_RETRIEVAL_ERROR", error.to_string())
}

const fn default_provider_timeout_ms() -> u64 {
    DEFAULT_PROVIDER_TIMEOUT_MS
}

const fn default_max_records() -> usize {
    100_000
}

const fn default_max_bytes() -> usize {
    128 * 1024 * 1024
}

const fn default_shutdown_timeout_ms() -> u64 {
    5_000
}

const fn default_search_limit() -> usize {
    10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_limit_is_bounded() {
        for limit in [0, MAX_SEARCH_LIMIT + 1] {
            let request = BridgeWorkspaceSearchRequest {
                query: "query".to_owned(),
                path: None,
                include: None,
                limit,
            };
            assert!(request.semantic().is_err());
        }
    }

    #[test]
    fn provider_failures_preserve_retry_categories() {
        assert!(matches!(
            provider_failure(BridgeEmbeddingFailure {
                kind: "rate_limited".to_owned(),
                retry_after_ms: Some(25),
            }),
            EmbeddingProviderError::RateLimited {
                retry_after: Some(delay)
            } if delay == Duration::from_millis(25)
        ));
    }

    #[test]
    fn rerank_wire_status_uses_go_snake_case_fields() {
        let status = a3s_code_core::WorkspaceRerankStatus {
            requested_mode: a3s_code_core::WorkspaceRerankMode::Deterministic,
            applied_mode: a3s_code_core::WorkspaceRerankMode::Deterministic,
            algorithm: a3s_code_core::WorkspaceRerankAlgorithm::RrfK60DeterministicMmrV1,
            input_candidates: 10,
            evaluated_candidates: 8,
            selected_candidates: 3,
            near_duplicate_candidates: 2,
            selected_near_duplicates: 0,
            feature_bytes: 512,
            accounted_scratch_bytes: 4_096,
            candidate_truncated: true,
            fallback: None,
        };

        let value = rerank_value(&status);
        assert_eq!(value["requested_mode"], "deterministic");
        assert_eq!(value["algorithm"], "rrf_k60+deterministic_mmr_v1");
        assert_eq!(value["accounted_scratch_bytes"], 4_096);
        assert!(value.get("requestedMode").is_none());
    }
}
