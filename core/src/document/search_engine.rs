use crate::document_consume::{self};
use crate::document_parser::DocumentParserRegistry;
use crate::document_search_scan;
use crate::document_service_types::{
    search_path_signal_score, ResolvedSearchRequest, SearchExecutionMode,
};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct CollectedSearchDocuments {
    keywords: Vec<String>,
    matches: Vec<document_consume::DeepSearchSamplingDocument>,
}

pub(crate) async fn execute_fast_search(
    query: &str,
    workspace: PathBuf,
    parser_registry: Option<Arc<DocumentParserRegistry>>,
    pipeline_registry: Option<Arc<crate::document_pipeline::DocumentPipelineRegistry>>,
    max_results: usize,
    include_glob: Option<String>,
    context_lines: usize,
) -> Result<crate::tools::ToolOutput> {
    let CollectedSearchDocuments { keywords, matches } = collect_search_documents(
        query,
        workspace,
        parser_registry,
        pipeline_registry,
        max_results,
        include_glob,
        context_lines,
    )
    .await?;

    if matches.is_empty() {
        return Ok(document_consume::build_search_no_results_output(query));
    }

    let response_matches = matches
        .iter()
        .map(|m| document_consume::SearchDocumentMatch {
            path_display: m.path_display.clone(),
            file_type: m.file_type.clone(),
            relevance: m.relevance,
            score: document_consume::clone_search_score_metadata(&m.score),
            matches: document_consume::clone_search_match_lines(&m.matches),
            document_metadata: Some(m.document_metadata.clone()),
            document_runtime: m.document_runtime.clone(),
        })
        .collect::<Vec<_>>();

    Ok(document_consume::build_fast_search_response(
        query,
        keywords,
        &response_matches,
    ))
}

pub(crate) async fn execute_search_request(
    query: &str,
    workspace: PathBuf,
    parser_registry: Option<Arc<DocumentParserRegistry>>,
    pipeline_registry: Option<Arc<crate::document_pipeline::DocumentPipelineRegistry>>,
    request: ResolvedSearchRequest,
) -> Result<crate::tools::ToolOutput> {
    match request.mode {
        SearchExecutionMode::Fast => {
            execute_fast_search(
                query,
                workspace,
                parser_registry,
                pipeline_registry,
                request.max_results,
                request.include_glob,
                request.context_lines,
            )
            .await
        }
        SearchExecutionMode::Deep => {
            execute_deep_search(
                query,
                workspace,
                parser_registry,
                pipeline_registry,
                request.max_results,
                request.include_glob,
                request.context_lines,
            )
            .await
        }
        SearchExecutionMode::FilenameOnly => {
            execute_filename_search(query, workspace, request.max_results, request.include_glob)
                .await
        }
    }
}

pub(crate) async fn execute_deep_search(
    query: &str,
    workspace: PathBuf,
    parser_registry: Option<Arc<DocumentParserRegistry>>,
    pipeline_registry: Option<Arc<crate::document_pipeline::DocumentPipelineRegistry>>,
    max_results: usize,
    include_glob: Option<String>,
    context_lines: usize,
) -> Result<crate::tools::ToolOutput> {
    let initial_limit = max_results.saturating_mul(2).max(max_results);
    let CollectedSearchDocuments {
        keywords,
        matches: initial_matches,
    } = collect_search_documents(
        query,
        workspace,
        parser_registry,
        pipeline_registry,
        initial_limit,
        include_glob,
        context_lines,
    )
    .await?;

    if initial_matches.is_empty() {
        return Ok(document_consume::build_search_no_results_output(query));
    }

    let regions = document_consume::sample_deep_search_regions(&initial_matches, max_results);
    Ok(document_consume::build_deep_search_response(
        query,
        keywords,
        initial_matches.len(),
        &regions,
    ))
}

pub(crate) async fn execute_filename_search(
    query: &str,
    workspace: PathBuf,
    max_results: usize,
    include_glob: Option<String>,
) -> Result<crate::tools::ToolOutput> {
    let keywords = document_consume::extract_search_keywords(query);
    let files = tokio::task::spawn_blocking(move || {
        document_search_scan::find_matching_paths(
            &workspace,
            &keywords,
            max_results,
            include_glob.as_deref(),
        )
    })
    .await
    .map_err(|e| anyhow::anyhow!("File search task failed: {}", e))??;

    if files.is_empty() {
        return Ok(document_consume::build_filename_search_no_results_output(
            query,
        ));
    }

    Ok(document_consume::build_filename_search_response(&files))
}

async fn collect_search_documents(
    query: &str,
    workspace: PathBuf,
    parser_registry: Option<Arc<DocumentParserRegistry>>,
    pipeline_registry: Option<Arc<crate::document_pipeline::DocumentPipelineRegistry>>,
    max_results: usize,
    include_glob: Option<String>,
    context_lines: usize,
) -> Result<CollectedSearchDocuments> {
    let keywords = document_consume::extract_search_keywords(query);
    let search_keywords = keywords.clone();
    let matches = tokio::task::spawn_blocking(move || {
        search_workspace_documents(
            &workspace,
            &search_keywords,
            max_results,
            include_glob.as_deref(),
            context_lines,
            parser_registry.as_deref(),
            pipeline_registry.as_deref(),
        )
    })
    .await
    .map_err(|e| anyhow::anyhow!("Search task failed: {}", e))??;

    Ok(CollectedSearchDocuments { keywords, matches })
}

pub(crate) fn search_workspace_documents(
    workspace: &Path,
    keywords: &[String],
    max_results: usize,
    include_glob: Option<&str>,
    context_lines: usize,
    parser_registry: Option<&DocumentParserRegistry>,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
) -> Result<Vec<document_consume::DeepSearchSamplingDocument>> {
    if keywords.is_empty() {
        return Ok(Vec::new());
    }

    let patterns = document_search_scan::compile_search_patterns(keywords);
    if patterns.is_empty() {
        return Ok(Vec::new());
    }

    let builder = document_search_scan::build_walk_builder(workspace, include_glob, true);
    let mut file_matches: Vec<document_consume::DeepSearchSamplingDocument> = Vec::new();

    for entry in builder.build().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let substrate = match document_consume::prepare_search_document_substrate(
            path,
            parser_registry,
            pipeline_registry,
            Some(1024 * 1024),
        ) {
            Ok(Some(substrate)) => substrate,
            Ok(None) => continue,
            Err(_) => continue,
        };

        let path_signal = search_path_signal_score(workspace, path, &patterns);
        let Some(match_doc) = document_consume::rank_search_sampling_document(
            path,
            workspace,
            substrate.search_lines,
            substrate.search_line_locators,
            substrate.metadata,
            substrate.document_runtime,
            &patterns,
            context_lines,
            path_signal,
        ) else {
            continue;
        };

        file_matches.push(match_doc);
    }

    file_matches.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    file_matches.truncate(max_results);
    Ok(file_matches)
}
