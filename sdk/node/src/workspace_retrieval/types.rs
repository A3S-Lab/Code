use super::*;

const MAX_SEARCH_LIMIT: usize = 25;

#[napi(object)]
#[derive(Clone)]
pub struct EmbeddingProviderDescriptorObject {
    pub provider: String,
    pub model: String,
    pub revision: Option<String>,
    pub dimension: f64,
    /// `"none"` or `"unit"`.
    pub normalization: Option<String>,
}

impl EmbeddingProviderDescriptorObject {
    pub(super) fn to_core(&self) -> napi::Result<EmbeddingProviderDescriptor> {
        let dimension = js_optional_usize(
            Some(self.dimension),
            "embeddingProvider.descriptor.dimension",
            0,
        )?;
        let normalization = match self.normalization.as_deref().unwrap_or("none") {
            "none" => EmbeddingNormalization::None,
            "unit" => EmbeddingNormalization::Unit,
            other => {
                return Err(napi::Error::from_reason(format!(
                    "embeddingProvider.descriptor.normalization must be 'none' or 'unit', got '{other}'"
                )))
            }
        };
        Ok(EmbeddingProviderDescriptor {
            provider: self.provider.clone(),
            model: self.model.clone(),
            revision: self.revision.clone(),
            dimension,
            normalization,
        })
    }
}

#[napi(object)]
#[derive(Clone)]
pub struct WorkspaceSearchRequest {
    pub query: String,
    pub path: Option<String>,
    pub include: Option<String>,
    pub limit: Option<u32>,
}

fn search_limit(limit: Option<u32>) -> napi::Result<usize> {
    let limit = limit.unwrap_or(10) as usize;
    if !(1..=MAX_SEARCH_LIMIT).contains(&limit) {
        return Err(napi::Error::from_reason(format!(
            "workspace retrieval limit must be from 1 to {MAX_SEARCH_LIMIT}"
        )));
    }
    Ok(limit)
}

pub(super) fn semantic_request(
    request: WorkspaceSearchRequest,
) -> napi::Result<RustSemanticSearchRequest> {
    let mut core =
        RustSemanticSearchRequest::new(request.query).with_limit(search_limit(request.limit)?);
    core.path = request.path;
    core.include = request.include;
    Ok(core)
}

pub(super) fn hybrid_request(
    request: WorkspaceSearchRequest,
) -> napi::Result<RustHybridSearchRequest> {
    let mut core =
        RustHybridSearchRequest::new(request.query).with_limit(search_limit(request.limit)?);
    core.path = request.path;
    core.include = request.include;
    Ok(core)
}

#[napi(object)]
#[derive(Clone)]
pub struct WorkspaceChunkObject {
    pub id: String,
    pub path: String,
    pub language: Option<String>,
    pub start_line: f64,
    pub end_line: f64,
    pub start_byte: f64,
    pub end_byte: f64,
    pub source_revision: f64,
    pub text: String,
    pub digest_verified: bool,
}

fn chunk_object(chunk: &a3s_code_core::WorkspaceChunk) -> WorkspaceChunkObject {
    WorkspaceChunkObject {
        id: chunk.id.as_str().to_owned(),
        path: chunk.path.to_string(),
        language: chunk.language.as_ref().map(ToString::to_string),
        start_line: chunk.start_line as f64,
        end_line: chunk.end_line as f64,
        start_byte: chunk.start_byte as f64,
        end_byte: chunk.end_byte as f64,
        source_revision: chunk.source_revision as f64,
        text: chunk.text.to_string(),
        digest_verified: true,
    }
}

#[napi(object)]
#[derive(Clone)]
pub struct WorkspaceEmbeddingBatchMetricsObject {
    pub document_inputs: f64,
    pub document_text_bytes: f64,
    pub document_batches: f64,
    pub document_provider_requests: f64,
    pub batch_limit_lower_bound: f64,
    pub input_limit_flushes: f64,
    pub text_byte_limit_flushes: f64,
    pub vector_byte_limit_flushes: f64,
    pub generation_complete_flushes: f64,
    pub time_to_first_ready_ms: Option<f64>,
    pub non_text_inputs: f64,
}

impl From<a3s_code_core::WorkspaceEmbeddingBatchMetrics> for WorkspaceEmbeddingBatchMetricsObject {
    fn from(metrics: a3s_code_core::WorkspaceEmbeddingBatchMetrics) -> Self {
        Self {
            document_inputs: metrics.document_inputs as f64,
            document_text_bytes: metrics.document_text_bytes as f64,
            document_batches: metrics.document_batches as f64,
            document_provider_requests: metrics.document_provider_requests as f64,
            batch_limit_lower_bound: metrics.batch_limit_lower_bound as f64,
            input_limit_flushes: metrics.input_limit_flushes as f64,
            text_byte_limit_flushes: metrics.text_byte_limit_flushes as f64,
            vector_byte_limit_flushes: metrics.vector_byte_limit_flushes as f64,
            generation_complete_flushes: metrics.generation_complete_flushes as f64,
            time_to_first_ready_ms: metrics.time_to_first_ready_ms.map(|value| value as f64),
            non_text_inputs: metrics.non_text_inputs as f64,
        }
    }
}

#[napi(object)]
#[derive(Clone)]
pub struct WorkspaceVecShadowStatusObject {
    pub phase: String,
    pub revision: f64,
    pub record_count: f64,
    pub accounted_bytes: f64,
    pub initialization_failures: f64,
    pub successful_mutations: f64,
    pub failed_mutations: f64,
    pub compared_queries: f64,
    pub matching_queries: f64,
    pub mismatched_queries: f64,
    pub failed_queries: f64,
}

impl From<a3s_code_core::WorkspaceVecShadowStatus> for WorkspaceVecShadowStatusObject {
    fn from(status: a3s_code_core::WorkspaceVecShadowStatus) -> Self {
        Self {
            phase: format!("{:?}", status.phase).to_ascii_lowercase(),
            revision: status.revision as f64,
            record_count: status.record_count as f64,
            accounted_bytes: status.accounted_bytes as f64,
            initialization_failures: status.initialization_failures as f64,
            successful_mutations: status.successful_mutations as f64,
            failed_mutations: status.failed_mutations as f64,
            compared_queries: status.compared_queries as f64,
            matching_queries: status.matching_queries as f64,
            mismatched_queries: status.mismatched_queries as f64,
            failed_queries: status.failed_queries as f64,
        }
    }
}

#[napi(object)]
#[derive(Clone)]
pub struct WorkspaceRetrievalStatusObject {
    pub phase: String,
    pub catalog_revision: f64,
    pub source_revision: f64,
    pub vector_revision: f64,
    pub eligible_files: f64,
    pub catalog_files: f64,
    pub catalog_chunks: f64,
    pub indexed_files: f64,
    pub indexed_chunks: f64,
    pub coverage_bps: u32,
    pub queue_depth: f64,
    pub failed_files: f64,
    pub total_failures: f64,
    pub vector_records: f64,
    pub vector_bytes: f64,
    pub active_vector_engine: Option<String>,
    pub vec_shadow: WorkspaceVecShadowStatusObject,
    pub batching: WorkspaceEmbeddingBatchMetricsObject,
    pub model: Option<EmbeddingProviderDescriptorObject>,
}

impl From<WorkspaceRetrievalStatus> for WorkspaceRetrievalStatusObject {
    fn from(status: WorkspaceRetrievalStatus) -> Self {
        Self {
            phase: format!("{:?}", status.phase).to_ascii_lowercase(),
            catalog_revision: status.catalog_revision as f64,
            source_revision: status.source_revision as f64,
            vector_revision: status.vector_revision as f64,
            eligible_files: status.eligible_files as f64,
            catalog_files: status.catalog_files as f64,
            catalog_chunks: status.catalog_chunks as f64,
            indexed_files: status.indexed_files as f64,
            indexed_chunks: status.indexed_chunks as f64,
            coverage_bps: u32::from(status.coverage_bps),
            queue_depth: status.queue_depth as f64,
            failed_files: status.failed_files as f64,
            total_failures: status.total_failures as f64,
            vector_records: status.vector_records as f64,
            vector_bytes: status.vector_bytes as f64,
            active_vector_engine: status.active_vector_engine.map(|engine| match engine {
                WorkspaceVectorEngine::A3sMemory => "a3s_memory".to_owned(),
            }),
            vec_shadow: status.vec_shadow.into(),
            batching: status.batching.into(),
            model: status.model.map(|model| EmbeddingProviderDescriptorObject {
                provider: model.provider,
                model: model.model,
                revision: model.revision,
                dimension: model.dimension as f64,
                normalization: Some(match model.normalization {
                    EmbeddingNormalization::None => "none".to_owned(),
                    EmbeddingNormalization::Unit => "unit".to_owned(),
                }),
            }),
        }
    }
}

#[napi(object)]
#[derive(Clone)]
pub struct WorkspaceSemanticSearchHitObject {
    pub chunk: WorkspaceChunkObject,
    pub score: f64,
}

impl From<&WorkspaceSemanticSearchHit> for WorkspaceSemanticSearchHitObject {
    fn from(hit: &WorkspaceSemanticSearchHit) -> Self {
        Self {
            chunk: chunk_object(&hit.chunk),
            score: f64::from(hit.score),
        }
    }
}

#[napi(object)]
pub struct WorkspaceSemanticSearchResultObject {
    pub hits: Vec<WorkspaceSemanticSearchHitObject>,
    pub status: WorkspaceRetrievalStatusObject,
    pub searched_records: f64,
    pub truncated: bool,
    pub fallback: Option<String>,
}

impl From<RustSemanticSearchResult> for WorkspaceSemanticSearchResultObject {
    fn from(result: RustSemanticSearchResult) -> Self {
        Self {
            hits: result.hits.iter().map(Into::into).collect(),
            status: result.status.into(),
            searched_records: result.searched_records as f64,
            truncated: result.truncated,
            fallback: result
                .fallback
                .map(|value| format!("{value:?}").to_ascii_lowercase()),
        }
    }
}

#[napi(object)]
#[derive(Clone)]
pub struct WorkspaceHybridChannelRankObject {
    pub channel: String,
    pub rank: u32,
}

impl From<&WorkspaceHybridChannelRank> for WorkspaceHybridChannelRankObject {
    fn from(rank: &WorkspaceHybridChannelRank) -> Self {
        Self {
            channel: format!("{:?}", rank.channel).to_ascii_lowercase(),
            rank: rank.rank as u32,
        }
    }
}

#[napi(object)]
#[derive(Clone)]
pub struct WorkspaceHybridChannelStatusObject {
    pub channel: String,
    pub candidate_count: f64,
    pub truncated: bool,
    pub fallback: Option<String>,
}

impl From<&WorkspaceHybridChannelStatus> for WorkspaceHybridChannelStatusObject {
    fn from(status: &WorkspaceHybridChannelStatus) -> Self {
        Self {
            channel: format!("{:?}", status.channel).to_ascii_lowercase(),
            candidate_count: status.candidate_count as f64,
            truncated: status.truncated,
            fallback: status
                .fallback
                .map(|value| format!("{value:?}").to_ascii_lowercase()),
        }
    }
}

#[napi(object)]
#[derive(Clone)]
pub struct WorkspaceRerankStatusObject {
    pub requested_mode: String,
    pub applied_mode: String,
    pub algorithm: String,
    pub input_candidates: f64,
    pub evaluated_candidates: f64,
    pub selected_candidates: f64,
    pub near_duplicate_candidates: f64,
    pub selected_near_duplicates: f64,
    pub feature_bytes: f64,
    pub accounted_scratch_bytes: f64,
    pub candidate_truncated: bool,
    pub fallback: Option<String>,
}

impl From<&WorkspaceRerankStatus> for WorkspaceRerankStatusObject {
    fn from(status: &WorkspaceRerankStatus) -> Self {
        Self {
            requested_mode: rerank_mode_name(status.requested_mode).to_owned(),
            applied_mode: rerank_mode_name(status.applied_mode).to_owned(),
            algorithm: status.algorithm.as_str().to_owned(),
            input_candidates: status.input_candidates as f64,
            evaluated_candidates: status.evaluated_candidates as f64,
            selected_candidates: status.selected_candidates as f64,
            near_duplicate_candidates: status.near_duplicate_candidates as f64,
            selected_near_duplicates: status.selected_near_duplicates as f64,
            feature_bytes: status.feature_bytes as f64,
            accounted_scratch_bytes: status.accounted_scratch_bytes as f64,
            candidate_truncated: status.candidate_truncated,
            fallback: status.fallback.map(rerank_fallback_name).map(str::to_owned),
        }
    }
}

const fn rerank_mode_name(mode: WorkspaceRerankMode) -> &'static str {
    match mode {
        WorkspaceRerankMode::RrfOnly => "rrf_only",
        WorkspaceRerankMode::Deterministic => "deterministic",
    }
}

const fn rerank_fallback_name(reason: WorkspaceRerankFallbackReason) -> &'static str {
    match reason {
        WorkspaceRerankFallbackReason::ScratchBudgetExceeded => "scratch_budget_exceeded",
        WorkspaceRerankFallbackReason::InvalidConfiguration => "invalid_configuration",
    }
}

#[napi(object)]
#[derive(Clone)]
pub struct WorkspaceHybridSearchHitObject {
    pub chunk: WorkspaceChunkObject,
    pub fused_score: f64,
    pub rerank_score: f64,
    pub redundancy_score: f64,
    pub exact_identifier: bool,
    pub channels: Vec<WorkspaceHybridChannelRankObject>,
}

impl From<&WorkspaceHybridSearchHit> for WorkspaceHybridSearchHitObject {
    fn from(hit: &WorkspaceHybridSearchHit) -> Self {
        Self {
            chunk: chunk_object(&hit.chunk),
            fused_score: hit.fused_score,
            rerank_score: hit.rerank_score,
            redundancy_score: hit.redundancy_score,
            exact_identifier: hit.exact_identifier,
            channels: hit.channels.iter().map(Into::into).collect(),
        }
    }
}

#[napi(object)]
pub struct WorkspaceHybridSearchResultObject {
    pub hits: Vec<WorkspaceHybridSearchHitObject>,
    pub semantic_status: WorkspaceRetrievalStatusObject,
    pub catalog_revision: f64,
    pub source_revision: f64,
    pub channels: Vec<WorkspaceHybridChannelStatusObject>,
    pub rerank: WorkspaceRerankStatusObject,
    pub truncated: bool,
    pub fallback: Option<String>,
}

impl From<RustHybridSearchResult> for WorkspaceHybridSearchResultObject {
    fn from(result: RustHybridSearchResult) -> Self {
        Self {
            hits: result.hits.iter().map(Into::into).collect(),
            semantic_status: result.semantic_status.into(),
            catalog_revision: result.catalog_revision as f64,
            source_revision: result.source_revision as f64,
            channels: result.channels.iter().map(Into::into).collect(),
            rerank: (&result.rerank).into(),
            truncated: result.truncated,
            fallback: result
                .fallback
                .map(|value| format!("{value:?}").to_ascii_lowercase()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_descriptor_rejects_unknown_normalization() {
        let error = EmbeddingProviderDescriptorObject {
            provider: "test".to_owned(),
            model: "fixture".to_owned(),
            revision: None,
            dimension: 4.0,
            normalization: Some("mystery".to_owned()),
        }
        .to_core()
        .unwrap_err();

        assert!(error.to_string().contains("normalization"));
    }

    #[test]
    fn search_request_rejects_unbounded_limits() {
        assert!(search_limit(Some(0)).is_err());
        assert!(search_limit(Some(26)).is_err());
        assert_eq!(search_limit(None).unwrap(), 10);
    }

    #[test]
    fn vector_migration_status_maps_to_node_fields() {
        let mut status = WorkspaceRetrievalStatus::disabled();
        status.active_vector_engine = Some(WorkspaceVectorEngine::A3sMemory);
        status.vec_shadow = a3s_code_core::WorkspaceVecShadowStatus {
            phase: a3s_code_core::WorkspaceVecShadowPhase::Ready,
            revision: 7,
            record_count: 11,
            accounted_bytes: 4_096,
            compared_queries: 3,
            matching_queries: 3,
            ..Default::default()
        };

        let mapped: WorkspaceRetrievalStatusObject = status.into();
        assert_eq!(mapped.active_vector_engine.as_deref(), Some("a3s_memory"));
        assert_eq!(mapped.vec_shadow.phase, "ready");
        assert_eq!(mapped.vec_shadow.record_count, 11.0);
        assert_eq!(mapped.vec_shadow.matching_queries, 3.0);
    }
}
