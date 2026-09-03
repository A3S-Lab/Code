//! Python request parsing and stable DTO serialization.

use super::*;

pub(super) fn semantic_request(
    request: &Bound<'_, PyDict>,
) -> PyResult<WorkspaceSemanticSearchRequest> {
    let (query, path, include, limit) = parse_search_request(request)?;
    let mut request = WorkspaceSemanticSearchRequest::new(query).with_limit(limit);
    request.path = path;
    request.include = include;
    Ok(request)
}

pub(super) fn hybrid_request(
    request: &Bound<'_, PyDict>,
) -> PyResult<WorkspaceHybridSearchRequest> {
    let (query, path, include, limit) = parse_search_request(request)?;
    let mut request = WorkspaceHybridSearchRequest::new(query).with_limit(limit);
    request.path = path;
    request.include = include;
    Ok(request)
}

fn parse_search_request(
    request: &Bound<'_, PyDict>,
) -> PyResult<(String, Option<String>, Option<String>, usize)> {
    let query = request
        .get_item("query")?
        .ok_or_else(|| PyValueError::new_err("query is required"))?
        .extract::<String>()?;
    let path = request
        .get_item("path")?
        .map(|value| value.extract::<String>())
        .transpose()?;
    let include = request
        .get_item("include")?
        .map(|value| value.extract::<String>())
        .transpose()?;
    let limit = request
        .get_item("limit")?
        .map(|value| value.extract::<usize>())
        .transpose()?
        .unwrap_or(10);
    if !(1..=MAX_SEARCH_LIMIT).contains(&limit) {
        return Err(PyValueError::new_err(format!(
            "limit must be from 1 to {MAX_SEARCH_LIMIT}"
        )));
    }
    Ok((query, path, include, limit))
}

pub(super) fn status_json(status: &WorkspaceRetrievalStatus) -> serde_json::Value {
    serde_json::json!({
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
        "active_vector_engine": status.active_vector_engine.map(|engine| match engine {
            a3s_code_core::WorkspaceVectorEngine::A3sMemory => "a3s_memory",
            a3s_code_core::WorkspaceVectorEngine::A3sVec => "a3s_vec",
        }),
        "vec_shadow": {
            "phase": format!("{:?}", status.vec_shadow.phase).to_ascii_lowercase(),
            "revision": status.vec_shadow.revision,
            "record_count": status.vec_shadow.record_count,
            "accounted_bytes": status.vec_shadow.accounted_bytes,
            "initialization_failures": status.vec_shadow.initialization_failures,
            "successful_mutations": status.vec_shadow.successful_mutations,
            "failed_mutations": status.vec_shadow.failed_mutations,
            "compared_queries": status.vec_shadow.compared_queries,
            "matching_queries": status.vec_shadow.matching_queries,
            "mismatched_queries": status.vec_shadow.mismatched_queries,
            "failed_queries": status.vec_shadow.failed_queries,
        },
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
        "model": status.model.as_ref().map(|model| serde_json::json!({
            "provider": model.provider,
            "model": model.model,
            "revision": model.revision,
            "dimension": model.dimension,
            "normalization": model.normalization,
        })),
    })
}

fn chunk_json(chunk: &a3s_code_core::WorkspaceChunk) -> serde_json::Value {
    serde_json::json!({
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

pub(super) fn semantic_result_json(result: WorkspaceSemanticSearchResult) -> serde_json::Value {
    serde_json::json!({
        "hits": result.hits.iter().map(|hit| serde_json::json!({
            "chunk": chunk_json(&hit.chunk),
            "score": hit.score,
        })).collect::<Vec<_>>(),
        "status": status_json(&result.status),
        "searched_records": result.searched_records,
        "truncated": result.truncated,
        "fallback": result.fallback,
    })
}

pub(super) fn hybrid_result_json(result: WorkspaceHybridSearchResult) -> serde_json::Value {
    serde_json::json!({
        "hits": result.hits.iter().map(|hit| serde_json::json!({
            "chunk": chunk_json(&hit.chunk),
            "fused_score": hit.fused_score,
            "rerank_score": hit.rerank_score,
            "redundancy_score": hit.redundancy_score,
            "exact_identifier": hit.exact_identifier,
            "channels": hit.channels.iter().map(|rank| serde_json::json!({
                "channel": rank.channel,
                "rank": rank.rank,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "semantic_status": status_json(&result.semantic_status),
        "catalog_revision": result.catalog_revision,
        "source_revision": result.source_revision,
        "channels": result.channels.iter().map(|status| serde_json::json!({
            "channel": status.channel,
            "candidate_count": status.candidate_count,
            "truncated": status.truncated,
            "fallback": status.fallback,
        })).collect::<Vec<_>>(),
        "rerank": rerank_json(&result.rerank),
        "truncated": result.truncated,
        "fallback": result.fallback,
    })
}

fn rerank_json(status: &a3s_code_core::WorkspaceRerankStatus) -> serde_json::Value {
    serde_json::json!({
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

pub(super) fn retrieval_error(error: a3s_code_core::WorkspaceRetrievalError) -> PyErr {
    py_error_with_code("WORKSPACE_RETRIEVAL_ERROR", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_migration_status_uses_python_snake_case_fields() {
        let mut status = WorkspaceRetrievalStatus::disabled();
        status.active_vector_engine = Some(a3s_code_core::WorkspaceVectorEngine::A3sMemory);
        status.vec_shadow = a3s_code_core::WorkspaceVecShadowStatus {
            phase: a3s_code_core::WorkspaceVecShadowPhase::Ready,
            revision: 7,
            record_count: 11,
            accounted_bytes: 4_096,
            compared_queries: 3,
            matching_queries: 3,
            ..Default::default()
        };

        let value = status_json(&status);
        assert_eq!(value["active_vector_engine"], "a3s_memory");
        assert_eq!(value["vec_shadow"]["phase"], "ready");
        assert_eq!(value["vec_shadow"]["record_count"], 11);
        assert_eq!(value["vec_shadow"]["matching_queries"], 3);
        assert!(value.get("activeVectorEngine").is_none());
    }

    #[test]
    fn vec_primary_status_uses_the_stable_engine_literal() {
        let mut status = WorkspaceRetrievalStatus::disabled();
        status.active_vector_engine = Some(a3s_code_core::WorkspaceVectorEngine::A3sVec);

        let value = status_json(&status);
        assert_eq!(value["active_vector_engine"], "a3s_vec");
    }
}
