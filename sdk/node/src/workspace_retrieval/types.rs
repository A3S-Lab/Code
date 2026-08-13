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
pub struct WorkspaceHybridSearchHitObject {
    pub chunk: WorkspaceChunkObject,
    pub fused_score: f64,
    pub exact_identifier: bool,
    pub channels: Vec<WorkspaceHybridChannelRankObject>,
}

impl From<&WorkspaceHybridSearchHit> for WorkspaceHybridSearchHitObject {
    fn from(hit: &WorkspaceHybridSearchHit) -> Self {
        Self {
            chunk: chunk_object(&hit.chunk),
            fused_score: hit.fused_score,
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
}
