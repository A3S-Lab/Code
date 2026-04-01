use crate::document_parser::{DocumentBlockKind, DocumentParserRegistry, ParsedDocument};
use crate::document_pipeline::DocumentChunk;
use regex::Regex;
use serde_json::json;
use std::path::Path;

fn search_block_kind_label(kind: &DocumentBlockKind) -> &'static str {
    match kind {
        DocumentBlockKind::Paragraph => "paragraph",
        DocumentBlockKind::Heading => "heading",
        DocumentBlockKind::Table => "table",
        DocumentBlockKind::Section => "section",
        DocumentBlockKind::Metadata => "metadata",
        DocumentBlockKind::Slide => "slide",
        DocumentBlockKind::EmailHeader => "email",
        DocumentBlockKind::Code => "code",
        DocumentBlockKind::Raw => "raw",
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn build_search_lines(doc: &ParsedDocument) -> Vec<String> {
    let chunks = crate::document_pipeline_defaults::build_default_document_chunks(doc);
    build_search_lines_from_chunks(doc.title.as_deref(), &chunks)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn build_search_lines_from_chunks(
    title: Option<&str>,
    chunks: &[DocumentChunk],
) -> Vec<String> {
    build_search_line_entries_from_chunks(title, chunks)
        .into_iter()
        .map(|entry| entry.content)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchLineEntry {
    pub content: String,
    pub locator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchDocumentMetadata {
    pub title: Option<String>,
    pub stage: Option<String>,
    pub line_count: usize,
    pub chunk_count: usize,
    pub has_runtime_metadata: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchDocumentSubstrate {
    pub metadata: SearchDocumentMetadata,
    pub search_lines: Vec<String>,
    pub search_line_locators: Vec<Option<String>>,
    pub document_runtime: Option<serde_json::Value>,
}

pub(crate) fn build_search_line_entries_from_chunks(
    title: Option<&str>,
    chunks: &[DocumentChunk],
) -> Vec<SearchLineEntry> {
    let mut lines = Vec::new();

    if let Some(title) = title {
        let title = title.trim();
        if !title.is_empty() {
            lines.push(SearchLineEntry {
                content: format!("# {}", title),
                locator: None,
            });
        }
    }

    for chunk in chunks {
        if let Some(location) = &chunk.location {
            let location = crate::document_render::format_block_location(location);
            if !location.is_empty() {
                lines.push(SearchLineEntry {
                    content: format!("[loc] {}", location),
                    locator: chunk.locator.clone(),
                });
            }
        }

        if let Some(label) = &chunk.label {
            let label = label.trim();
            if !label.is_empty() {
                let kind = chunk.kind.clone().unwrap_or(DocumentBlockKind::Raw);
                lines.push(SearchLineEntry {
                    content: format!("[{}] {}", search_block_kind_label(&kind), label),
                    locator: chunk.locator.clone(),
                });
            }
        }

        // Include heading context (context_label) for better search ranking
        if let Some(context) = &chunk.context_label {
            let context = context.trim();
            if !context.is_empty() {
                lines.push(SearchLineEntry {
                    content: format!("[section] {}", context),
                    locator: chunk.locator.clone(),
                });
            }
        }

        // Include keywords for better search ranking
        if !chunk.keywords.is_empty() {
            let keywords = chunk.keywords.join(", ");
            lines.push(SearchLineEntry {
                content: format!("[keywords] {}", keywords),
                locator: chunk.locator.clone(),
            });
        }

        for line in chunk.content.lines() {
            let line = line.trim_end();
            if !line.is_empty() {
                lines.push(SearchLineEntry {
                    content: line.to_string(),
                    locator: chunk.locator.clone(),
                });
            }
        }
    }

    lines
}

pub(crate) fn build_search_document_substrate(
    path: &Path,
    doc: &ParsedDocument,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
) -> SearchDocumentSubstrate {
    let chunks = crate::document_consume::build_document_chunks(path, doc, None, pipeline_registry);

    let entries = build_search_line_entries_from_chunks(doc.title.as_deref(), &chunks);
    let document_runtime = crate::document_consume::extract_document_runtime_metadata(doc);
    let stage = doc
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.attributes.get("document.stage"))
        .cloned();

    SearchDocumentSubstrate {
        metadata: SearchDocumentMetadata {
            title: doc.title.clone(),
            stage,
            line_count: entries.len(),
            chunk_count: chunks.len(),
            has_runtime_metadata: document_runtime.is_some(),
        },
        search_lines: entries.iter().map(|entry| entry.content.clone()).collect(),
        search_line_locators: entries.iter().map(|entry| entry.locator.clone()).collect(),
        document_runtime,
    }
}

pub(crate) fn prepare_search_document_substrate(
    path: &Path,
    parser_registry: Option<&DocumentParserRegistry>,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
    max_plaintext_bytes: Option<u64>,
) -> anyhow::Result<Option<SearchDocumentSubstrate>> {
    let prepared = match crate::document_consume::prepare_document_from_path(
        path,
        parser_registry,
        pipeline_registry,
        max_plaintext_bytes,
    )? {
        Some(doc) => doc,
        None => return Ok(None),
    };

    Ok(Some(build_search_document_substrate(
        path,
        &prepared,
        pipeline_registry,
    )))
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchScoreMetadataInput {
    pub base_score: f32,
    pub path_signal: f32,
    pub idf_boost: f32,
    pub file_type_boost: f32,
    pub unique_keywords_matched: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchFileType {
    Code,
    Config,
    Documentation,
    Other,
}

impl SearchFileType {
    pub(crate) fn from_path(path: &Path) -> Self {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext {
                "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "java" | "c" | "cpp" | "h"
                | "hpp" => Self::Code,
                "toml" | "yaml" | "yml" | "json" | "ini" | "conf" => Self::Config,
                "md" | "txt" | "rst" | "adoc" => Self::Documentation,
                _ => Self::Other,
            }
        } else {
            Self::Other
        }
    }

    pub(crate) fn relevance_boost(self) -> f32 {
        match self {
            Self::Code => 1.2,
            Self::Config => 1.0,
            Self::Documentation => 0.9,
            Self::Other => 0.8,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Config => "config",
            Self::Documentation => "docs",
            Self::Other => "file",
        }
    }
}

pub(crate) struct MatchLineMetadataInput<'a> {
    pub line_number: usize,
    pub content: &'a str,
    pub locator: Option<&'a str>,
    pub context_before: &'a [String],
    pub context_after: &'a [String],
}

pub(crate) struct SampledLineMetadataInput<'a> {
    pub line_number: usize,
    pub content: &'a str,
    pub locator: Option<&'a str>,
    pub distance: usize,
    pub weight: f32,
}

pub(crate) struct SearchResultMetadataInput {
    pub path_display: String,
    pub file_type: String,
    pub relevance: f32,
    pub score: SearchScoreMetadataInput,
    pub matches: Vec<serde_json::Value>,
    pub document_metadata: Option<serde_json::Value>,
    pub document_runtime: Option<serde_json::Value>,
}

pub(crate) struct DeepSearchResultMetadataInput {
    pub path_display: String,
    pub file_type: String,
    pub evidence_score: f32,
    pub sampled_line_count: usize,
    pub score: SearchScoreMetadataInput,
    pub matches: Vec<serde_json::Value>,
    pub sampled_lines: Vec<serde_json::Value>,
    pub document_metadata: Option<serde_json::Value>,
    pub document_runtime: Option<serde_json::Value>,
}

pub(crate) struct SearchMatchRenderInput {
    pub line_number: usize,
    pub content: String,
    pub locator: Option<String>,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

pub(crate) struct SearchResultRenderInput {
    pub path_display: String,
    pub file_type: String,
    pub relevance: f32,
    pub score: SearchScoreMetadataInput,
    pub matches: Vec<SearchMatchRenderInput>,
}

pub(crate) struct DeepSampledLineRenderInput {
    pub line_number: usize,
    pub content: String,
    pub distance: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchDocumentMatchLine {
    pub line_number: usize,
    pub content: String,
    pub locator: Option<String>,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchDocumentMatch {
    pub path_display: String,
    pub file_type: String,
    pub relevance: f32,
    pub score: SearchScoreMetadataInput,
    pub matches: Vec<SearchDocumentMatchLine>,
    pub document_metadata: Option<SearchDocumentMetadata>,
    pub document_runtime: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DeepSearchSampledLine {
    pub line_number: usize,
    pub content: String,
    pub locator: Option<String>,
    pub distance: usize,
    pub weight: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DeepSearchSamplingDocument {
    pub path_display: String,
    pub file_type: String,
    pub relevance: f32,
    pub score: SearchScoreMetadataInput,
    pub matches: Vec<SearchDocumentMatchLine>,
    pub search_lines: Vec<String>,
    pub search_line_locators: Vec<Option<String>>,
    pub document_metadata: SearchDocumentMetadata,
    pub document_runtime: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DeepSearchDocumentRegion {
    pub path_display: String,
    pub file_type: String,
    pub evidence_score: f32,
    pub score: SearchScoreMetadataInput,
    pub matches: Vec<SearchDocumentMatchLine>,
    pub sampled_lines: Vec<DeepSearchSampledLine>,
    pub document_metadata: SearchDocumentMetadata,
    pub document_runtime: Option<serde_json::Value>,
}

pub(crate) fn build_search_document_metadata_value(
    metadata: &SearchDocumentMetadata,
) -> serde_json::Value {
    json!({
        "title": metadata.title,
        "stage": metadata.stage,
        "line_count": metadata.line_count,
        "chunk_count": metadata.chunk_count,
        "has_runtime_metadata": metadata.has_runtime_metadata,
    })
}

pub(crate) struct DeepEvidenceBlockRenderInput {
    pub lines: Vec<DeepSampledLineRenderInput>,
}

pub(crate) struct DeepSearchResultRenderInput {
    pub path_display: String,
    pub file_type: String,
    pub evidence_score: f32,
    pub score: SearchScoreMetadataInput,
    pub blocks: Vec<DeepEvidenceBlockRenderInput>,
}

pub(crate) struct SearchResponseInput {
    pub keywords: Vec<String>,
    pub result_count: usize,
    pub report: String,
    pub results_metadata: Vec<serde_json::Value>,
}

pub(crate) struct DeepSearchResponseInput {
    pub keywords: Vec<String>,
    pub result_count: usize,
    pub initial_pool_size: usize,
    pub report: String,
    pub results_metadata: Vec<serde_json::Value>,
}

pub(crate) fn build_match_lines_metadata(
    matches: &[MatchLineMetadataInput<'_>],
) -> Vec<serde_json::Value> {
    matches
        .iter()
        .map(|m| {
            json!({
                "line_number": m.line_number,
                "content": m.content,
                "locator": m.locator,
                "context_before": m.context_before,
                "context_after": m.context_after,
            })
        })
        .collect()
}

pub(crate) fn build_sampled_lines_metadata(
    lines: &[SampledLineMetadataInput<'_>],
) -> Vec<serde_json::Value> {
    lines
        .iter()
        .map(|line| {
            json!({
                "line_number": line.line_number,
                "content": line.content,
                "locator": line.locator,
                "distance": line.distance,
                "weight": line.weight,
            })
        })
        .collect()
}

pub(crate) fn build_search_results_metadata(
    matches: &[SearchResultMetadataInput],
) -> Vec<serde_json::Value> {
    matches
        .iter()
        .map(|fm| {
            json!({
                "path": fm.path_display,
                "file_type": fm.file_type,
                "relevance": fm.relevance,
                "match_count": fm.matches.len(),
                "score": {
                    "base": fm.score.base_score,
                    "path_signal": fm.score.path_signal,
                    "idf_boost": fm.score.idf_boost,
                    "file_type_boost": fm.score.file_type_boost,
                    "unique_keywords_matched": fm.score.unique_keywords_matched,
                },
                "matches": fm.matches,
                "document_metadata": fm.document_metadata,
                "document_runtime": fm.document_runtime,
            })
        })
        .collect()
}

pub(crate) fn build_deep_search_results_metadata(
    regions: &[DeepSearchResultMetadataInput],
) -> Vec<serde_json::Value> {
    regions
        .iter()
        .map(|region| {
            json!({
                "path": region.path_display,
                "file_type": region.file_type,
                "evidence_score": region.evidence_score,
                "sampled_line_count": region.sampled_line_count,
                "score": {
                    "base": region.score.base_score,
                    "path_signal": region.score.path_signal,
                    "idf_boost": region.score.idf_boost,
                    "file_type_boost": region.score.file_type_boost,
                    "unique_keywords_matched": region.score.unique_keywords_matched,
                },
                "matches": region.matches,
                "sampled_lines": region.sampled_lines,
                "document_metadata": region.document_metadata,
                "document_runtime": region.document_runtime,
            })
        })
        .collect()
}

pub(crate) fn build_search_results_report(
    results: &[SearchResultRenderInput],
    query: &str,
) -> String {
    let mut out = format!("Found {} file(s) matching \"{}\"\n\n", results.len(), query);

    for (i, result) in results.iter().enumerate() {
        out.push_str(&format!(
            "─── {} ({}, relevance: {:.2}) ───\n",
            result.path_display, result.file_type, result.relevance
        ));
        out.push_str(&format!(
            "    score: base={:.2}, path={:.2}, idf={:.2}, type={:.2}, keywords={}\n",
            result.score.base_score,
            result.score.path_signal,
            result.score.idf_boost,
            result.score.file_type_boost,
            result.score.unique_keywords_matched
        ));

        for m in result.matches.iter().take(5) {
            for ctx in &m.context_before {
                out.push_str(&format!("  {}\n", ctx));
            }
            if let Some(locator) = &m.locator {
                out.push_str(&format!(
                    "▶ L{} [{}]: {}\n",
                    m.line_number, locator, m.content
                ));
            } else {
                out.push_str(&format!("▶ L{}: {}\n", m.line_number, m.content));
            }
            for ctx in &m.context_after {
                out.push_str(&format!("  {}\n", ctx));
            }
            out.push('\n');
        }

        if result.matches.len() > 5 {
            out.push_str(&format!(
                "  ... {} more matches\n",
                result.matches.len() - 5
            ));
        }

        if i < results.len() - 1 {
            out.push('\n');
        }
    }

    out
}

pub(crate) fn build_deep_search_results_report(
    regions: &[DeepSearchResultRenderInput],
    query: &str,
) -> String {
    let mut out = format!(
        "Deep search found {} evidence region(s) for \"{}\"\n\n",
        regions.len(),
        query
    );

    for (i, region) in regions.iter().enumerate() {
        out.push_str(&format!(
            "━━━ {} ({}, evidence: {:.3}) ━━━\n",
            region.path_display, region.file_type, region.evidence_score
        ));
        out.push_str(&format!(
            "    score: base={:.2}, path={:.2}, idf={:.2}, type={:.2}, keywords={}\n",
            region.score.base_score,
            region.score.path_signal,
            region.score.idf_boost,
            region.score.file_type_boost,
            region.score.unique_keywords_matched
        ));

        for block in &region.blocks {
            for line in &block.lines {
                let marker = if line.distance == 0 { "▶" } else { " " };
                out.push_str(&format!(
                    "{} L{:4}: {}\n",
                    marker, line.line_number, line.content
                ));
            }
            out.push('\n');
        }

        if i < regions.len() - 1 {
            out.push('\n');
        }
    }

    out
}

pub(crate) fn build_search_tool_output(input: SearchResponseInput) -> crate::tools::ToolOutput {
    crate::tools::ToolOutput::success(input.report).with_metadata(json!({
        "result_count": input.result_count,
        "keywords": input.keywords,
        "mode": "fast",
        "results": input.results_metadata,
    }))
}

pub(crate) fn build_deep_search_tool_output(
    input: DeepSearchResponseInput,
) -> crate::tools::ToolOutput {
    crate::tools::ToolOutput::success(input.report).with_metadata(json!({
        "result_count": input.result_count,
        "initial_pool_size": input.initial_pool_size,
        "keywords": input.keywords,
        "mode": "deep",
        "results": input.results_metadata,
    }))
}

pub(crate) fn build_search_no_results_output(query: &str) -> crate::tools::ToolOutput {
    crate::tools::ToolOutput::success(format!(
        "No results found for: {}\n\nTry broader keywords or check the search path.",
        query
    ))
}

pub(crate) fn build_filename_search_response(
    paths: &[std::path::PathBuf],
) -> crate::tools::ToolOutput {
    let output = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("\n");

    crate::tools::ToolOutput::success(output).with_metadata(json!({
        "result_count": paths.len(),
        "mode": "filename_only"
    }))
}

pub(crate) fn build_filename_search_no_results_output(query: &str) -> crate::tools::ToolOutput {
    crate::tools::ToolOutput::success(format!("No files found matching: {}", query))
}

pub(crate) fn build_fast_search_response(
    query: &str,
    keywords: Vec<String>,
    matches: &[SearchDocumentMatch],
) -> crate::tools::ToolOutput {
    let render_inputs = matches
        .iter()
        .map(|m| SearchResultRenderInput {
            path_display: m.path_display.clone(),
            file_type: m.file_type.clone(),
            relevance: m.relevance,
            score: clone_search_score_metadata(&m.score),
            matches: m
                .matches
                .iter()
                .map(|line| SearchMatchRenderInput {
                    line_number: line.line_number,
                    content: line.content.clone(),
                    locator: line.locator.clone(),
                    context_before: line.context_before.clone(),
                    context_after: line.context_after.clone(),
                })
                .collect(),
        })
        .collect::<Vec<_>>();

    let metadata_inputs = matches
        .iter()
        .map(|m| SearchResultMetadataInput {
            path_display: m.path_display.clone(),
            file_type: m.file_type.clone(),
            relevance: m.relevance,
            score: clone_search_score_metadata(&m.score),
            matches: build_match_line_metadata_values(&m.matches),
            document_metadata: m
                .document_metadata
                .as_ref()
                .map(build_search_document_metadata_value),
            document_runtime: m.document_runtime.clone(),
        })
        .collect::<Vec<_>>();

    build_search_tool_output(SearchResponseInput {
        keywords,
        result_count: matches.len(),
        report: build_search_results_report(&render_inputs, query),
        results_metadata: build_search_results_metadata(&metadata_inputs),
    })
}

pub(crate) fn build_deep_search_response(
    query: &str,
    keywords: Vec<String>,
    initial_pool_size: usize,
    regions: &[DeepSearchDocumentRegion],
) -> crate::tools::ToolOutput {
    let render_inputs = regions
        .iter()
        .map(|region| DeepSearchResultRenderInput {
            path_display: region.path_display.clone(),
            file_type: region.file_type.clone(),
            evidence_score: region.evidence_score,
            score: clone_search_score_metadata(&region.score),
            blocks: build_deep_evidence_blocks(&region.sampled_lines),
        })
        .collect::<Vec<_>>();

    let metadata_inputs = regions
        .iter()
        .map(|region| DeepSearchResultMetadataInput {
            path_display: region.path_display.clone(),
            file_type: region.file_type.clone(),
            evidence_score: region.evidence_score,
            sampled_line_count: region.sampled_lines.len(),
            score: clone_search_score_metadata(&region.score),
            matches: build_match_line_metadata_values(&region.matches),
            sampled_lines: build_sampled_line_metadata_values(&region.sampled_lines),
            document_metadata: Some(build_search_document_metadata_value(
                &region.document_metadata,
            )),
            document_runtime: region.document_runtime.clone(),
        })
        .collect::<Vec<_>>();

    build_deep_search_tool_output(DeepSearchResponseInput {
        keywords,
        result_count: regions.len(),
        initial_pool_size,
        report: build_deep_search_results_report(&render_inputs, query),
        results_metadata: build_deep_search_results_metadata(&metadata_inputs),
    })
}

pub(crate) fn extract_search_keywords(query: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
        "need", "dare", "ought", "how", "what", "where", "when", "why", "who", "which", "that",
        "this", "these", "those", "it", "its", "in", "on", "at", "to", "for", "of", "with", "by",
        "from", "up", "about", "into", "through", "during", "and", "or", "but", "not", "no", "nor",
        "so", "yet", "both", "either", "work", "works", "working", "find", "get", "use", "make",
        "look",
    ];

    let mut keywords: Vec<String> = Vec::new();
    let words: Vec<&str> = query.split_whitespace().collect();

    if words.len() >= 2 {
        for window in words.windows(2) {
            let phrase = window.join(" ");
            if !phrase.chars().all(|c| c.is_whitespace()) {
                keywords.push(phrase);
            }
        }
    }

    for word in &words {
        let clean = word
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .to_lowercase();
        if clean.len() >= 3 && !STOP_WORDS.contains(&clean.as_str()) {
            keywords.push(clean);
        }

        for variant in split_identifier_variants(word) {
            if variant.len() >= 3 && !STOP_WORDS.contains(&variant.as_str()) {
                keywords.push(variant);
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    keywords.retain(|k| seen.insert(k.clone()));
    keywords.truncate(8);
    keywords
}

pub(crate) fn build_search_score_metadata(
    base_score: f32,
    path_signal: f32,
    idf_boost: f32,
    file_type_boost: f32,
    unique_keywords_matched: usize,
) -> SearchScoreMetadataInput {
    SearchScoreMetadataInput {
        base_score,
        path_signal,
        idf_boost,
        file_type_boost,
        unique_keywords_matched,
    }
}

pub(crate) fn rank_search_sampling_document(
    path: &Path,
    workspace: &Path,
    search_lines: Vec<String>,
    search_line_locators: Vec<Option<String>>,
    document_metadata: SearchDocumentMetadata,
    document_runtime: Option<serde_json::Value>,
    patterns: &[Regex],
    context_lines: usize,
    path_signal: f32,
) -> Option<DeepSearchSamplingDocument> {
    if search_lines.iter().all(|line| line.trim().is_empty()) {
        return None;
    }

    let mut matches: Vec<SearchDocumentMatchLine> = Vec::new();
    let mut keyword_hits: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();

    for pattern in patterns {
        for (idx, line) in search_lines.iter().enumerate() {
            if pattern.is_match(line) {
                *keyword_hits.entry(idx).or_insert(0) += 1;
            }
        }
    }

    if keyword_hits.is_empty() {
        return None;
    }

    let mut seen_lines = std::collections::HashSet::new();
    let mut sorted_hits: Vec<(usize, usize)> = keyword_hits.into_iter().collect();
    sorted_hits.sort_by(|a, b| b.1.cmp(&a.1));

    for (line_idx, _) in sorted_hits.iter().take(20) {
        if !seen_lines.insert(*line_idx) {
            continue;
        }

        let before_start = line_idx.saturating_sub(context_lines);
        let after_end = (line_idx + context_lines + 1).min(search_lines.len());
        matches.push(SearchDocumentMatchLine {
            line_number: line_idx + 1,
            content: search_lines[*line_idx].clone(),
            locator: search_line_locators[*line_idx]
                .clone()
                .or_else(|| crate::document_render::derive_match_locator(&search_lines, *line_idx)),
            context_before: search_lines[before_start..*line_idx].to_vec(),
            context_after: search_lines[line_idx + 1..after_end].to_vec(),
        });
    }

    matches.sort_by_key(|m| m.line_number);

    let unique_keywords_matched = patterns
        .iter()
        .filter(|p| search_lines.iter().any(|l| p.is_match(l)))
        .count();
    let file_type = SearchFileType::from_path(path);
    let base_score = (matches.len() as f32) / (search_lines.len() as f32).sqrt();
    let idf_boost = (unique_keywords_matched as f32) / (patterns.len() as f32);
    let file_type_boost = file_type.relevance_boost();
    let relevance = (base_score + path_signal) * idf_boost * file_type_boost;

    Some(DeepSearchSamplingDocument {
        path_display: path
            .strip_prefix(workspace)
            .ok()
            .map(|p| p.display().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| path.display().to_string()),
        file_type: file_type.label().to_string(),
        relevance,
        score: build_search_score_metadata(
            base_score,
            path_signal,
            idf_boost,
            file_type_boost,
            unique_keywords_matched,
        ),
        matches,
        search_lines,
        search_line_locators,
        document_metadata,
        document_runtime,
    })
}

pub(crate) fn clone_search_score_metadata(
    score: &SearchScoreMetadataInput,
) -> SearchScoreMetadataInput {
    SearchScoreMetadataInput {
        base_score: score.base_score,
        path_signal: score.path_signal,
        idf_boost: score.idf_boost,
        file_type_boost: score.file_type_boost,
        unique_keywords_matched: score.unique_keywords_matched,
    }
}

pub(crate) fn clone_search_match_lines(
    matches: &[SearchDocumentMatchLine],
) -> Vec<SearchDocumentMatchLine> {
    matches
        .iter()
        .map(|line| SearchDocumentMatchLine {
            line_number: line.line_number,
            content: line.content.clone(),
            locator: line.locator.clone(),
            context_before: line.context_before.clone(),
            context_after: line.context_after.clone(),
        })
        .collect()
}

pub(crate) fn build_match_line_metadata_values(
    matches: &[SearchDocumentMatchLine],
) -> Vec<serde_json::Value> {
    let inputs = matches
        .iter()
        .map(|line| MatchLineMetadataInput {
            line_number: line.line_number,
            content: &line.content,
            locator: line.locator.as_deref(),
            context_before: &line.context_before,
            context_after: &line.context_after,
        })
        .collect::<Vec<_>>();
    build_match_lines_metadata(&inputs)
}

pub(crate) fn build_sampled_line_metadata_values(
    lines: &[DeepSearchSampledLine],
) -> Vec<serde_json::Value> {
    let inputs = lines
        .iter()
        .map(|line| SampledLineMetadataInput {
            line_number: line.line_number,
            content: &line.content,
            locator: line.locator.as_deref(),
            distance: line.distance,
            weight: line.weight,
        })
        .collect::<Vec<_>>();
    build_sampled_lines_metadata(&inputs)
}

pub(crate) fn build_deep_evidence_blocks(
    lines: &[DeepSearchSampledLine],
) -> Vec<DeepEvidenceBlockRenderInput> {
    let mut blocks: Vec<Vec<&DeepSearchSampledLine>> = Vec::new();
    let mut current: Vec<&DeepSearchSampledLine> = Vec::new();

    for line in lines {
        if let Some(last) = current.last() {
            if line.line_number > last.line_number + 3 && !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
        }
        current.push(line);
    }
    if !current.is_empty() {
        blocks.push(current);
    }

    blocks
        .into_iter()
        .map(|block| DeepEvidenceBlockRenderInput {
            lines: block
                .into_iter()
                .map(|line| DeepSampledLineRenderInput {
                    line_number: line.line_number,
                    content: line.content.clone(),
                    distance: line.distance,
                })
                .collect(),
        })
        .collect()
}

pub(crate) fn sample_deep_search_regions(
    matches: &[DeepSearchSamplingDocument],
    top_k: usize,
) -> Vec<DeepSearchDocumentRegion> {
    let mut regions: Vec<DeepSearchDocumentRegion> = Vec::new();

    for doc in matches {
        let anchors: Vec<usize> = doc.matches.iter().map(|m| m.line_number).collect();
        if anchors.is_empty() || doc.search_lines.is_empty() {
            continue;
        }

        let match_density = anchors.len() as f32 / doc.search_lines.len() as f32;
        let sigma = (5.0_f32 - match_density * 20.0).max(2.0);
        let window_radius = (sigma * 3.0) as usize;

        let mut sampled: Vec<DeepSearchSampledLine> = Vec::new();
        let mut seen_lines = std::collections::HashSet::new();

        for &anchor in &anchors {
            let start = anchor.saturating_sub(window_radius + 1);
            let end = (anchor + window_radius).min(doc.search_lines.len());

            #[allow(clippy::needless_range_loop)]
            for line_idx in start..end {
                if !seen_lines.insert(line_idx) {
                    continue;
                }

                let distance = anchor.abs_diff(line_idx + 1);
                let weight = (-((distance * distance) as f32) / (2.0 * sigma * sigma)).exp();

                sampled.push(DeepSearchSampledLine {
                    line_number: line_idx + 1,
                    content: doc.search_lines[line_idx].clone(),
                    locator: doc.search_line_locators[line_idx].clone().or_else(|| {
                        crate::document_render::derive_match_locator(&doc.search_lines, line_idx)
                    }),
                    distance,
                    weight,
                });
            }
        }

        sampled.sort_by_key(|line| line.line_number);
        let weight_sum: f32 = sampled.iter().map(|line| line.weight).sum();
        let evidence_score = doc.relevance * weight_sum / (sampled.len() as f32).sqrt().max(1.0);

        regions.push(DeepSearchDocumentRegion {
            path_display: doc.path_display.clone(),
            file_type: doc.file_type.clone(),
            evidence_score,
            score: clone_search_score_metadata(&doc.score),
            matches: clone_search_match_lines(&doc.matches),
            sampled_lines: sampled,
            document_metadata: doc.document_metadata.clone(),
            document_runtime: doc.document_runtime.clone(),
        });
    }

    regions.sort_by(|a, b| {
        b.evidence_score
            .partial_cmp(&a.evidence_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    regions.truncate(top_k);
    regions
}

fn split_identifier_variants(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut prev_is_lower_or_digit = false;

    for ch in value.chars() {
        let is_separator = !ch.is_alphanumeric();
        let is_upper = ch.is_ascii_uppercase();

        if is_separator || (is_upper && prev_is_lower_or_digit && !current.is_empty()) {
            let normalized = current
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_ascii_lowercase();
            if normalized.len() >= 3 {
                parts.push(normalized);
            }
            current.clear();
        }

        if !is_separator {
            current.push(ch);
        }

        prev_is_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }

    if !current.is_empty() {
        let normalized = current
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase();
        if normalized.len() >= 3 {
            parts.push(normalized);
        }
    }

    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

    #[test]
    fn build_search_lines_from_chunks_preserves_locations_labels_and_content() {
        let doc = ParsedDocument {
            title: Some("report.pdf".to_string()),
            blocks: vec![
                DocumentBlock::new(
                    DocumentBlockKind::Section,
                    Some("Overview"),
                    "Line one\nLine two",
                )
                .with_source("report.pdf")
                .with_page(2)
                .with_ordinal(4),
                DocumentBlock::new(DocumentBlockKind::EmailHeader, Some("Subject"), "Hello"),
            ],
            metadata: None,
            ..Default::default()
        };

        let chunks = vec![
            DocumentChunk {
                label: Some("Overview".to_string()),
                context_label: Some("Summary".to_string()),
                content: "Line one\nLine two".to_string(),
                keywords: vec!["overview".to_string(), "summary".to_string()],
                language: Some("en".to_string()),
                location: doc.blocks[0].location.clone(),
                location_display: Some("source=report.pdf, page=2, ordinal=4".to_string()),
                locator: Some("page 2 | Overview".to_string()),
                source: Some("report.pdf".to_string()),
                page: Some(2),
                ordinal: Some(4),
                block_indices: vec![0],
                kind: Some(DocumentBlockKind::Section),
                score: 0,
            },
            DocumentChunk {
                label: Some("Subject".to_string()),
                context_label: None,
                content: "Hello".to_string(),
                keywords: vec!["subject".to_string()],
                language: Some("en".to_string()),
                location: None,
                location_display: None,
                locator: Some("Subject".to_string()),
                source: None,
                page: None,
                ordinal: None,
                block_indices: vec![1],
                kind: Some(DocumentBlockKind::EmailHeader),
                score: 0,
            },
        ];

        let lines = build_search_lines_from_chunks(doc.title.as_deref(), &chunks);
        assert_eq!(lines[0], "# report.pdf");
        assert!(lines
            .iter()
            .any(|line| line == "[loc] source=report.pdf, page=2, ordinal=4"));
        assert!(lines.iter().any(|line| line == "[section] Overview"));
        assert!(lines.iter().any(|line| line == "[email] Subject"));
        assert!(lines.iter().any(|line| line == "Line one"));
        assert!(lines.iter().any(|line| line == "Hello"));
    }

    #[test]
    fn build_search_line_entries_from_chunks_propagates_chunk_locator() {
        let chunks = vec![DocumentChunk {
            label: Some("Overview".to_string()),
            context_label: Some("Summary".to_string()),
            content: "Line one\nLine two".to_string(),
            keywords: vec!["overview".to_string()],
            language: Some("en".to_string()),
            location: None,
            location_display: None,
            locator: Some("page 2 | Overview".to_string()),
            source: Some("report.pdf".to_string()),
            page: Some(2),
            ordinal: Some(4),
            block_indices: vec![0],
            kind: Some(DocumentBlockKind::Section),
            score: 0,
        }];

        let lines = build_search_line_entries_from_chunks(Some("report.pdf"), &chunks);
        assert_eq!(lines[1].locator.as_deref(), Some("page 2 | Overview"));
        assert_eq!(lines[2].locator.as_deref(), Some("page 2 | Overview"));
    }
}
