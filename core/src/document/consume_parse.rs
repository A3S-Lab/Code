use crate::document_parser::{
    DocumentBlockKind, DocumentBlockLocation, DocumentParserRegistry, ParsedDocument,
};
use anyhow::Result;
use serde_json::json;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructuralSummaryStyle {
    Narrative,
    Tabular,
    Code,
}

pub(crate) fn build_structural_summary(
    doc: &ParsedDocument,
    style: StructuralSummaryStyle,
) -> String {
    let mut out = String::from("\n## Structural Summary\n\n");
    if doc.blocks.is_empty() {
        out.push_str("(no structure detected)\n");
        return out;
    }

    match style {
        StructuralSummaryStyle::Code => append_code_summary(&mut out, doc),
        StructuralSummaryStyle::Tabular => append_tabular_summary(&mut out, doc),
        StructuralSummaryStyle::Narrative => append_block_summary(&mut out, doc),
    }
    out
}

fn append_code_summary(out: &mut String, doc: &ParsedDocument) {
    let symbols = detect_code_symbols(&doc.to_text());
    if symbols.is_empty() {
        out.push_str("(no symbols detected)\n");
        return;
    }

    out.push_str("### Symbols\n\n");
    for symbol in symbols.iter().take(50) {
        out.push_str(&format!("- `{}`\n", symbol));
    }
    if symbols.len() > 50 {
        out.push_str(&format!("… {} more symbols\n", symbols.len() - 50));
    }
}

fn append_tabular_summary(out: &mut String, doc: &ParsedDocument) {
    let mut wrote_any = false;
    for block in &doc.blocks {
        if !matches!(block.kind, DocumentBlockKind::Table) {
            continue;
        }

        wrote_any = true;
        if let Some(label) = &block.label {
            out.push_str(&format!("### {}\n\n", label));
        }

        let mut lines = block.content.lines().filter(|line| !line.trim().is_empty());
        if let Some(header) = lines.next() {
            let row_count = lines.count();
            out.push_str(&format!("Header row: `{}`\n", header.trim()));
            out.push_str(&format!("Data rows: {}\n\n", row_count));
        }
    }

    if !wrote_any {
        append_block_summary(out, doc);
    }
}

fn append_block_summary(out: &mut String, doc: &ParsedDocument) {
    if let Some(title) = &doc.title {
        out.push_str(&format!("### {}\n\n", title));
    }

    for block in doc.blocks.iter().take(12) {
        let heading = block
            .label
            .as_deref()
            .filter(|label| !label.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| block_kind_heading(&block.kind));
        out.push_str(&format!("### {}\n\n", heading));

        if let Some(location) = &block.location {
            let location = crate::document_render::format_block_location(location);
            if !location.is_empty() {
                out.push_str(&format!("_Location: {}_\n\n", location));
            }
        }

        let preview_lines: Vec<&str> = block
            .content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect();

        if preview_lines.is_empty() {
            out.push_str("(empty block)\n\n");
            continue;
        }

        for line in preview_lines {
            out.push_str(&format!("> {}\n", line));
        }

        let total_lines = block
            .content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .count();
        if total_lines > 3 {
            out.push_str(&format!("_… {} more lines_\n", total_lines - 3));
        }
        out.push('\n');
    }

    if doc.blocks.len() > 12 {
        out.push_str(&format!("_… {} more blocks_\n", doc.blocks.len() - 12));
    }
}

fn block_kind_heading(kind: &DocumentBlockKind) -> String {
    match kind {
        DocumentBlockKind::Paragraph => "Paragraph".to_string(),
        DocumentBlockKind::Heading => "Heading".to_string(),
        DocumentBlockKind::Table => "Table".to_string(),
        DocumentBlockKind::Section => "Section".to_string(),
        DocumentBlockKind::Metadata => "Metadata".to_string(),
        DocumentBlockKind::Slide => "Slide".to_string(),
        DocumentBlockKind::EmailHeader => "Email Header".to_string(),
        DocumentBlockKind::Code => "Code".to_string(),
        DocumentBlockKind::Raw => "Raw Content".to_string(),
    }
}

fn detect_code_symbols(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|l| l.trim())
        .filter(|l| {
            l.starts_with("pub fn ")
                || l.starts_with("fn ")
                || l.starts_with("async fn ")
                || l.starts_with("pub async fn ")
                || l.starts_with("pub struct ")
                || l.starts_with("struct ")
                || l.starts_with("pub enum ")
                || l.starts_with("enum ")
                || l.starts_with("pub trait ")
                || l.starts_with("trait ")
                || l.starts_with("impl ")
                || l.starts_with("def ")
                || l.starts_with("class ")
                || l.starts_with("func ")
                || l.starts_with("function ")
        })
        .map(|l| l.to_string())
        .collect()
}

pub(crate) struct RenderedForLlm {
    pub content: String,
    pub truncated: bool,
    pub included_blocks: usize,
    pub included_indices: Vec<usize>,
}

pub(crate) struct ParseLlmRequest<'a> {
    pub path_display: &'a str,
    pub strategy_label: &'a str,
    pub rendered_content: &'a str,
    pub query: &'a str,
    pub max_chars: usize,
    pub llm_input_truncated: bool,
    pub llm_blocks_included: usize,
}

pub(crate) struct BuiltParseLlmRequest {
    pub system_prompt: String,
    pub user_prompt: String,
}

pub(crate) struct PreparedParseDocument {
    pub path_display: String,
    pub strategy_label: String,
    pub line_count: usize,
    pub word_count: usize,
    pub block_count: usize,
    pub non_empty_block_count: usize,
    pub document_language: Option<String>,
    pub document_keywords: Vec<String>,
    pub document_provenance: Option<serde_json::Value>,
    pub document_confidence: Option<serde_json::Value>,
    pub chunk_highlights: Vec<serde_json::Value>,
    pub structured_payloads: Vec<serde_json::Value>,
    pub tables: Vec<serde_json::Value>,
    pub pages: Vec<serde_json::Value>,
    pub elements: Vec<serde_json::Value>,
    pub structural_summary: String,
    pub document_runtime: Option<serde_json::Value>,
    pub document_quality: Option<crate::document_pipeline::DocumentQualityReport>,
    pub llm_rendered_content: String,
    pub llm_input_truncated: bool,
    pub llm_blocks_included: usize,
    pub llm_block_details: Vec<serde_json::Value>,
}

pub(crate) fn build_parse_file_not_found_output(path: &Path) -> crate::tools::ToolOutput {
    crate::tools::ToolOutput::error(format!("File not found: {}", path.display()))
}

pub(crate) fn build_parse_unreadable_output(path: &Path) -> crate::tools::ToolOutput {
    crate::tools::ToolOutput::error(format!(
        "Cannot read `{}` — it appears to be a binary file with no registered parser. \
         Register a DocumentParser for this format via SessionOptions.",
        path.display()
    ))
}

pub(crate) fn build_parse_llm_failure_message(err: &anyhow::Error) -> String {
    format!("[LLM extraction failed: {}]", err)
}

pub(crate) fn load_extracted_document_from_path(
    path: &Path,
    parser_registry: Option<&DocumentParserRegistry>,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
    max_plaintext_bytes: Option<u64>,
) -> Result<Option<crate::document_pipeline::ExtractedDocument>> {
    let extraction_cache_key = build_document_extraction_cache_key(path, parser_registry);

    if let Some(registry) = parser_registry {
        let cached_extracted_document = if let (Some(pipeline), Some(extraction_cache_key)) =
            (pipeline_registry, extraction_cache_key.as_ref())
        {
            if let Some(cache_store) = pipeline.cache_store() {
                cache_store.get_extracted_document(extraction_cache_key)?
            } else {
                None
            }
        } else {
            None
        };

        let extracted = if let Some(document) = cached_extracted_document {
            Ok(Some(document))
        } else {
            registry.parse_file_extracted(path)
        };

        match extracted {
            Ok(Some(extracted)) => {
                if let (Some(cache_store), Some(extraction_cache_key)) = (
                    pipeline_registry.and_then(|pipeline| pipeline.cache_store()),
                    extraction_cache_key.as_ref(),
                ) {
                    cache_store.put_extracted_document(extraction_cache_key, &extracted)?;
                }
                return Ok(Some(extracted));
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    "document_parsers failed on {}: {} — falling back to text read",
                    path.display(),
                    e
                );
            }
        }
    }

    if let Some(limit) = max_plaintext_bytes {
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > limit {
                return Ok(None);
            }
        }
    }

    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(crate::document_pipeline::ExtractedDocument::new(
            ParsedDocument::from_text(text),
        ))),
        Err(_) => Ok(None),
    }
}

pub(crate) fn prepare_document_from_path(
    path: &Path,
    parser_registry: Option<&DocumentParserRegistry>,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
    max_plaintext_bytes: Option<u64>,
) -> Result<Option<ParsedDocument>> {
    let Some(prepared) = prepare_document_artifacts_from_path(
        path,
        parser_registry,
        pipeline_registry,
        max_plaintext_bytes,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(prepared.document))
}

struct PreparedDocumentArtifacts {
    document: ParsedDocument,
    validation_report: crate::document_pipeline::DocumentValidationReport,
}

fn prepare_document_artifacts_from_path(
    path: &Path,
    parser_registry: Option<&DocumentParserRegistry>,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
    max_plaintext_bytes: Option<u64>,
) -> Result<Option<PreparedDocumentArtifacts>> {
    let cache_key = build_document_cache_key(path, parser_registry, pipeline_registry);
    if let (Some(pipeline), Some(cache_key)) = (pipeline_registry, cache_key.as_ref()) {
        if let Some(cache_store) = pipeline.cache_store() {
            if let Some(document) = cache_store.get_document(cache_key)? {
                let validation_report = validate_document_with_pipeline(path, &document, pipeline)?;
                return Ok(Some(PreparedDocumentArtifacts {
                    document,
                    validation_report,
                }));
            }
        }
    }

    let Some(extracted) = load_extracted_document_from_path(
        path,
        parser_registry,
        pipeline_registry,
        max_plaintext_bytes,
    )?
    else {
        return Ok(None);
    };

    let mut doc = extracted.into_parsed_document();
    let mut validation_report = crate::document_pipeline::DocumentValidationReport::default();
    if let Some(pipeline) = pipeline_registry {
        validation_report = pipeline.process_document(path, &mut doc)?;
        if validation_report.has_errors() {
            tracing::warn!(
                "document context extraction rejected {}: {}",
                path.display(),
                format_validation_issues(&validation_report)
            );
            return Ok(None);
        }
        if let (Some(cache_store), Some(cache_key)) = (pipeline.cache_store(), cache_key.as_ref()) {
            cache_store.put_document(cache_key, &doc)?;
        }
    }
    Ok(Some(PreparedDocumentArtifacts {
        document: doc,
        validation_report,
    }))
}

pub(crate) fn prepare_parse_document_from_path(
    path: &Path,
    parser_registry: Option<&DocumentParserRegistry>,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
    strategy_hint: &str,
    max_chars: usize,
    query: Option<&str>,
    block_kind_label: fn(&DocumentBlockKind) -> &'static str,
) -> Result<Option<PreparedParseDocument>> {
    let prepared_document =
        match prepare_document_artifacts_from_path(path, parser_registry, pipeline_registry, None)?
        {
            Some(doc) => doc,
            None => return Ok(None),
        };
    let raw_document = prepared_document.document;

    let raw_text = raw_document.to_text();
    let line_count = raw_text.lines().count();
    let word_count = raw_text.split_whitespace().count();
    let block_count = raw_document.block_count();
    let non_empty_block_count = raw_document.non_empty_block_count();
    let path_display = path.display().to_string();
    let document_language = raw_document
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.language.clone());
    let document_provenance = raw_document
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.provenance.as_ref())
        .and_then(|provenance| serde_json::to_value(provenance).ok());
    let document_confidence = raw_document
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.confidence.as_ref())
        .and_then(|confidence| serde_json::to_value(confidence).ok());
    let document_keywords = raw_document
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.attributes.get("document.keywords"))
        .map(|value| split_attribute_list(value, 12))
        .unwrap_or_default();
    let strategy_label =
        crate::document_service_types::detect_parse_strategy_label(path, &raw_text, strategy_hint);
    let ranked_chunks = build_document_chunks(path, &raw_document, query, pipeline_registry);
    let chunk_highlights = summarize_chunks_for_parse_report(&ranked_chunks, 3);
    let structured_payloads = summarize_structured_payloads(&raw_document, block_kind_label);
    let tables = extract_tables_for_output(&raw_document);
    let pages = summarize_pages_for_parse_report(&raw_document);
    let elements = extract_elements_for_output(&raw_document);
    let structural_summary = build_structural_summary(
        &raw_document,
        crate::document_service_types::structural_summary_style_for_strategy_label(&strategy_label),
    );
    let document_runtime = extract_document_runtime_metadata(&raw_document);
    let document_quality = match pipeline_registry {
        Some(pipeline) => pipeline.evaluate_document_quality(
            path,
            &raw_document,
            &prepared_document.validation_report,
        )?,
        None => None,
    };
    let rendered_for_llm = render_document_for_llm(
        &raw_document,
        max_chars,
        query,
        pipeline_registry,
        block_kind_label,
    );
    let llm_block_details = llm_block_metadata(
        &raw_document,
        &rendered_for_llm.included_indices,
        block_kind_label,
    );

    Ok(Some(PreparedParseDocument {
        path_display,
        strategy_label,
        line_count,
        word_count,
        block_count,
        non_empty_block_count,
        document_language,
        document_keywords,
        document_provenance,
        document_confidence,
        chunk_highlights,
        structured_payloads,
        tables,
        pages,
        elements,
        structural_summary,
        document_runtime,
        document_quality,
        llm_rendered_content: rendered_for_llm.content,
        llm_input_truncated: rendered_for_llm.truncated,
        llm_blocks_included: rendered_for_llm.included_blocks,
        llm_block_details,
    }))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn parse_document_from_path(
    path: &Path,
    parser_registry: Option<&DocumentParserRegistry>,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
    max_plaintext_bytes: Option<u64>,
) -> Result<Option<ParsedDocument>> {
    prepare_document_from_path(
        path,
        parser_registry,
        pipeline_registry,
        max_plaintext_bytes,
    )
}

fn build_document_cache_key(
    path: &Path,
    parser_registry: Option<&DocumentParserRegistry>,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
) -> Option<crate::document_pipeline::DocumentCacheKey> {
    let pipeline = pipeline_registry?;
    let extraction_key = build_document_extraction_cache_key(path, parser_registry)?;

    Some(crate::document_pipeline::DocumentCacheKey {
        path: extraction_key.path,
        file_hash: extraction_key.file_hash,
        parser: extraction_key.parser,
        pipeline_signature: pipeline.signature(),
    })
}

fn build_document_extraction_cache_key(
    path: &Path,
    parser_registry: Option<&DocumentParserRegistry>,
) -> Option<crate::document_pipeline::DocumentExtractionCacheKey> {
    let bytes = std::fs::read(path).ok()?;
    let parser = parser_registry
        .and_then(|registry| registry.find_parser(path))
        .map(|parser| parser.signature())
        .unwrap_or_else(|| "plain-text-fallback".to_string());

    Some(crate::document_pipeline::DocumentExtractionCacheKey {
        path: path.display().to_string(),
        file_hash: sha256::digest(bytes),
        parser,
    })
}

fn format_validation_issues(report: &crate::document_pipeline::DocumentValidationReport) -> String {
    report
        .issues
        .iter()
        .map(|issue| format!("{}: {}", issue.validator, issue.message))
        .collect::<Vec<_>>()
        .join("; ")
}

fn validate_document_with_pipeline(
    path: &Path,
    document: &ParsedDocument,
    pipeline: &crate::document_pipeline::DocumentPipelineRegistry,
) -> Result<crate::document_pipeline::DocumentValidationReport> {
    let mut report = crate::document_pipeline::DocumentValidationReport::default();
    for validator in pipeline.validators() {
        report.issues.extend(validator.validate(path, document)?);
    }
    Ok(report)
}

pub(crate) fn build_document_chunks(
    path: &Path,
    doc: &ParsedDocument,
    query: Option<&str>,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
) -> Vec<crate::document_pipeline::DocumentChunk> {
    match pipeline_registry {
        Some(pipeline) if !pipeline.chunkers().is_empty() => pipeline
            .chunk_document(path, doc, query)
            .unwrap_or_else(|_| default_chunks(doc)),
        _ => crate::document_pipeline_defaults::chunk_document_with_default_pipeline(
            path, doc, query,
        ),
    }
}

pub(crate) fn render_document_for_llm(
    doc: &ParsedDocument,
    max_chars: usize,
    query: Option<&str>,
    pipeline_registry: Option<&crate::document_pipeline::DocumentPipelineRegistry>,
    block_kind_label: fn(&DocumentBlockKind) -> &'static str,
) -> RenderedForLlm {
    if max_chars == 0 {
        return RenderedForLlm {
            content: String::new(),
            truncated: true,
            included_blocks: 0,
            included_indices: Vec::new(),
        };
    }

    let mut out = String::new();
    let mut included_blocks = 0usize;
    let mut included_indices = Vec::new();
    let mut truncated = false;

    if let Some(title) = &doc.title {
        let header = format!("# Document: {}\n\n", title.trim());
        if header.chars().count() <= max_chars {
            out.push_str(&header);
        } else {
            return RenderedForLlm {
                content: header.chars().take(max_chars).collect(),
                truncated: true,
                included_blocks: 0,
                included_indices: Vec::new(),
            };
        }
    }

    let mut chunks = build_document_chunks(
        Path::new(doc.title.as_deref().unwrap_or("document")),
        doc,
        query,
        pipeline_registry,
    );
    rerank_chunks_for_llm_query(&mut chunks, query);

    for chunk in &chunks {
        let section = render_chunk_for_llm(chunk, block_kind_label);

        let current_len = out.chars().count();
        let section_len = section.chars().count();
        if current_len + section_len <= max_chars {
            out.push_str(&section);
            included_blocks += chunk.block_indices.len();
            for idx in &chunk.block_indices {
                if !included_indices.contains(idx) {
                    included_indices.push(*idx);
                }
            }
            continue;
        }

        let remaining = max_chars.saturating_sub(current_len);
        if remaining > 0 {
            out.push_str(&render_chunk_preview_for_llm(
                chunk,
                remaining,
                block_kind_label,
            ));
        }
        truncated = true;
        break;
    }

    if included_indices.len() < doc.blocks.len() {
        truncated = true;
        let omitted = doc.blocks.len() - included_indices.len();
        if omitted > 0 && !out.contains("[truncated]") {
            out.push_str(&format!("\n… [truncated: omitted {} block(s)]", omitted));
        }
    }

    RenderedForLlm {
        content: out,
        truncated,
        included_blocks,
        included_indices,
    }
}

pub(crate) fn build_parse_llm_request(input: &ParseLlmRequest<'_>) -> BuiltParseLlmRequest {
    let truncation_note = if input.llm_input_truncated {
        format!(
            "\nLLM input was truncated to {} chars across {} block(s).",
            input.max_chars, input.llm_blocks_included
        )
    } else {
        String::new()
    };

    let system_prompt = "You are a document analysis assistant. \
         The user will provide document content and ask you to extract information from it. \
         Answer based solely on the provided content. Be concise."
        .to_string();

    let user_prompt = format!(
        "Document: `{}`\nParse strategy: {}\n\n\
         --- DOCUMENT ---\n{}\n--- END DOCUMENT ---{}\n\n\
         Query: {}",
        input.path_display,
        input.strategy_label,
        input.rendered_content,
        truncation_note,
        input.query
    );

    BuiltParseLlmRequest {
        system_prompt,
        user_prompt,
    }
}

pub(crate) fn build_parse_llm_request_for_prepared(
    prepared: &PreparedParseDocument,
    query: &str,
    max_chars: usize,
) -> BuiltParseLlmRequest {
    build_parse_llm_request(&ParseLlmRequest {
        path_display: &prepared.path_display,
        strategy_label: &prepared.strategy_label,
        rendered_content: &prepared.llm_rendered_content,
        query,
        max_chars,
        llm_input_truncated: prepared.llm_input_truncated,
        llm_blocks_included: prepared.llm_blocks_included,
    })
}

fn default_chunks(doc: &ParsedDocument) -> Vec<crate::document_pipeline::DocumentChunk> {
    crate::document_pipeline_defaults::build_default_document_chunks(doc)
}

fn rerank_chunks_for_llm_query(
    chunks: &mut [crate::document_pipeline::DocumentChunk],
    query: Option<&str>,
) {
    let keywords = llm_query_keywords(query);
    if keywords.is_empty() {
        return;
    }

    chunks.sort_by(|a, b| {
        score_chunk_for_llm_query(b, &keywords)
            .cmp(&score_chunk_for_llm_query(a, &keywords))
            .then_with(|| {
                a.block_indices
                    .first()
                    .copied()
                    .unwrap_or(usize::MAX)
                    .cmp(&b.block_indices.first().copied().unwrap_or(usize::MAX))
            })
    });
}

fn llm_query_keywords(query: Option<&str>) -> Vec<String> {
    let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) else {
        return Vec::new();
    };

    let mut keywords = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for token in query.split_whitespace() {
        let normalized = token
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
            .to_ascii_lowercase();
        if normalized.len() >= 2 && seen.insert(normalized.clone()) {
            keywords.push(normalized);
        }
    }
    keywords
}

fn score_chunk_for_llm_query(
    chunk: &crate::document_pipeline::DocumentChunk,
    keywords: &[String],
) -> usize {
    let label = chunk
        .label
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let content = chunk.content.to_ascii_lowercase();
    let location = chunk
        .location_display
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    keywords.iter().fold(0usize, |score, keyword| {
        let mut next = score;
        if label.contains(keyword) {
            next += 6;
        }
        if location.contains(keyword) {
            next += 4;
        }
        if content.contains(keyword) {
            next += 2;
        }
        next
    })
}

fn render_chunk_for_llm(
    chunk: &crate::document_pipeline::DocumentChunk,
    block_kind_label: fn(&DocumentBlockKind) -> &'static str,
) -> String {
    let ordinal = chunk.block_indices.first().map(|idx| idx + 1).unwrap_or(1);
    let kind = chunk.kind.clone().unwrap_or(DocumentBlockKind::Raw);
    let mut section = format!("## Block {}: {}", ordinal, block_kind_label(&kind));
    if let Some(label) = &chunk.label {
        let label = label.trim();
        if !label.is_empty() {
            section.push_str(&format!(" ({})", label));
        }
    }
    section.push('\n');
    if let Some(location) = &chunk.location {
        let location = crate::document_render::format_block_location(location);
        if !location.is_empty() {
            section.push_str(&format!("Location: {}\n", location));
        }
    }
    section.push_str(chunk.content.trim());
    section.push_str("\n\n");
    section
}

fn render_chunk_preview_for_llm(
    chunk: &crate::document_pipeline::DocumentChunk,
    remaining_chars: usize,
    block_kind_label: fn(&DocumentBlockKind) -> &'static str,
) -> String {
    if remaining_chars == 0 {
        return String::new();
    }

    let mut header_chunk = chunk.clone();
    header_chunk.content.clear();
    let header = render_chunk_for_llm(&header_chunk, block_kind_label);
    let header = header.trim_end().to_string();
    let header_len = header.chars().count();
    if header_len >= remaining_chars {
        let mut clipped: String = header.chars().take(remaining_chars).collect();
        if !clipped.ends_with('\n') {
            clipped.push('\n');
        }
        clipped.push_str("… [truncated]");
        return clipped;
    }

    let mut preview = String::new();
    let available = remaining_chars.saturating_sub(header_len);
    let excerpt = summarize_block_content(&chunk.content, available.saturating_sub(16));
    preview.push_str(&header);
    if !preview.ends_with('\n') {
        preview.push('\n');
    }
    preview.push_str(&excerpt);
    if !preview.ends_with('\n') {
        preview.push('\n');
    }
    preview.push_str("… [truncated]");
    preview
}

fn summarize_block_content(content: &str, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }

    let mut lines = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");

    if lines.chars().count() > budget {
        lines = lines.chars().take(budget).collect();
    }

    lines
}

pub(crate) fn llm_block_metadata(
    doc: &ParsedDocument,
    indices: &[usize],
    block_kind_label: fn(&DocumentBlockKind) -> &'static str,
) -> Vec<serde_json::Value> {
    indices
        .iter()
        .filter_map(|idx| {
            let block = doc.blocks.get(*idx)?;
            Some(json!({
                "index": idx + 1,
                "kind": block_kind_label(&block.kind),
                "label": block.label,
                "structured_payload": block
                    .structured_payload
                    .as_deref()
                    .map(parse_structured_payload_value)
                    .unwrap_or(serde_json::Value::Null),
                "location": block.location.as_ref().map(|location: &DocumentBlockLocation| json!({
                    "source": location.source,
                    "page": location.page,
                    "ordinal": location.ordinal,
                    "continued_from_previous_page": location.continued_from_previous_page,
                    "continued_to_next_page": location.continued_to_next_page,
                    "display": crate::document_render::format_block_location(location),
                })),
            }))
        })
        .collect()
}

pub(crate) fn extract_document_runtime_metadata(doc: &ParsedDocument) -> Option<serde_json::Value> {
    crate::document_ocr::extract_document_ocr_runtime_metadata(doc)
        .and_then(|metadata| serde_json::to_value(metadata).ok())
}

pub(crate) fn summarize_document_runtime(metadata: Option<&serde_json::Value>) -> Option<String> {
    let ocr = metadata?.get("ocr")?;
    if !ocr
        .get("used")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return None;
    }

    let provider = ocr
        .get("provider")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let format = ocr
        .get("format")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let model = ocr
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or("unset");
    let max_images = ocr
        .get("max_images")
        .and_then(|value| value.as_u64())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unset".to_string());
    let dpi = ocr
        .get("dpi")
        .and_then(|value| value.as_u64())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unset".to_string());

    Some(format!(
        "OCR used for `{format}` via `{provider}` (model=`{model}`, max_images={max_images}, dpi={dpi})"
    ))
}

pub(crate) fn describe_document_parser_config(
    config: Option<&crate::config::DocumentParserConfig>,
) -> String {
    match config {
        Some(config) => {
            let mut line = format!(
                "enabled={}, max_file_size_mb={}",
                config.enabled, config.max_file_size_mb
            );
            if let Some(ocr) = &config.ocr {
                line.push_str(&format!(
                    ", ocr.enabled={}, ocr.model={}, ocr.prompt={}, ocr.max_images={}, ocr.dpi={}",
                    ocr.enabled,
                    ocr.model.as_deref().unwrap_or("unset"),
                    if ocr
                        .prompt
                        .as_deref()
                        .is_some_and(|prompt| !prompt.trim().is_empty())
                    {
                        "set"
                    } else {
                        "unset"
                    },
                    ocr.max_images,
                    ocr.dpi
                ));
            } else {
                line.push_str(", ocr=unset");
            }
            line
        }
        None => "unset".to_string(),
    }
}

pub(crate) struct ParseOutputHeader<'a> {
    pub path_display: &'a str,
    pub strategy_label: &'a str,
    pub document_parser_summary: &'a str,
    pub block_count: usize,
    pub non_empty_block_count: usize,
    pub line_count: usize,
    pub word_count: usize,
    pub document_language: Option<&'a str>,
    pub document_keywords: &'a [String],
    pub provenance_summary: Option<&'a str>,
    pub confidence_summary: Option<&'a str>,
    pub runtime_summary: Option<&'a str>,
    pub quality_summary: Option<&'a str>,
}

pub(crate) fn build_parse_output_header(header: &ParseOutputHeader<'_>) -> String {
    let mut output = format!(
        "# Agentic Parse: `{}`\n\n\
         - **Strategy**: `{}`\n\
         - **Document Parser**: `{}`\n\
         - **Blocks**: {} (non-empty: {})\n\
         - **Lines**: {}\n\
         - **Words**: {}\n",
        header.path_display,
        header.strategy_label,
        header.document_parser_summary,
        header.block_count,
        header.non_empty_block_count,
        header.line_count,
        header.word_count,
    );

    if let Some(language) = header.document_language {
        output.push_str(&format!("- **Language**: `{language}`\n"));
    }
    if !header.document_keywords.is_empty() {
        output.push_str(&format!(
            "- **Keywords**: {}\n",
            header
                .document_keywords
                .iter()
                .take(8)
                .map(|keyword| format!("`{keyword}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(provenance_summary) = header.provenance_summary {
        output.push_str(&format!("- **Provenance**: {}\n", provenance_summary));
    }
    if let Some(confidence_summary) = header.confidence_summary {
        output.push_str(&format!("- **Confidence**: {}\n", confidence_summary));
    }
    if let Some(runtime_summary) = header.runtime_summary {
        output.push_str(&format!("- **Runtime**: {}\n", runtime_summary));
    }
    if let Some(quality_summary) = header.quality_summary {
        output.push_str(&format!("- **Quality**: {}\n", quality_summary));
    }

    output
}

pub(crate) struct ParseReportSections<'a> {
    pub header: ParseOutputHeader<'a>,
    pub query_used: bool,
    pub llm_block_details: &'a [serde_json::Value],
    pub chunk_highlights: &'a [serde_json::Value],
    pub structured_payloads: &'a [serde_json::Value],
    pub pages: &'a [serde_json::Value],
    pub structural_summary: &'a str,
    pub llm_answer: Option<&'a str>,
}

pub(crate) fn build_parse_report(sections: &ParseReportSections<'_>) -> String {
    let mut output = build_parse_output_header(&sections.header);

    if sections.query_used && !sections.llm_block_details.is_empty() {
        let locators = sections
            .llm_block_details
            .iter()
            .take(3)
            .map(|block| {
                block
                    .get("location")
                    .and_then(|location| location.get("display"))
                    .and_then(|value| value.as_str())
                    .or_else(|| block.get("label").and_then(|value| value.as_str()))
                    .unwrap_or("unlabeled")
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("; ");
        output.push_str(&format!("- **LLM Blocks**: {}\n", locators));
    }

    if !sections.chunk_highlights.is_empty() {
        output.push_str("\n## Key Chunks\n\n");
        for chunk in sections.chunk_highlights {
            let label = chunk
                .get("label")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("unlabeled");
            output.push_str(&format!("### {label}\n\n"));

            let mut details = Vec::new();
            if let Some(locator) = chunk.get("locator").and_then(|value| value.as_str()) {
                details.push(format!("locator=`{locator}`"));
            }
            if let Some(language) = chunk.get("language").and_then(|value| value.as_str()) {
                details.push(format!("language=`{language}`"));
            }
            if let Some(score) = chunk.get("score").and_then(|value| value.as_u64()) {
                details.push(format!("score={score}"));
            }
            if !details.is_empty() {
                output.push_str(&format!("_{}_\n\n", details.join(", ")));
            }

            if let Some(keywords) = chunk.get("keywords").and_then(|value| value.as_array()) {
                let rendered = keywords
                    .iter()
                    .filter_map(|value| value.as_str())
                    .take(6)
                    .map(|keyword| format!("`{keyword}`"))
                    .collect::<Vec<_>>();
                if !rendered.is_empty() {
                    output.push_str(&format!("Keywords: {}\n\n", rendered.join(", ")));
                }
            }

            if let Some(preview) = chunk.get("preview").and_then(|value| value.as_str()) {
                for line in preview
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .take(3)
                {
                    output.push_str(&format!("> {}\n", line.trim()));
                }
                output.push('\n');
            }
        }
    }

    if !sections.structured_payloads.is_empty() {
        output.push_str("\n## Structured Payloads\n\n");
        for payload in sections.structured_payloads.iter().take(3) {
            let label = payload
                .get("label")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    payload
                        .get("location")
                        .and_then(|location| location.get("display"))
                        .and_then(|value| value.as_str())
                })
                .unwrap_or("unlabeled");
            output.push_str(&format!("### {label}\n\n"));

            let mut details = Vec::new();
            if let Some(kind) = payload.get("kind").and_then(|value| value.as_str()) {
                details.push(format!("kind=`{kind}`"));
            }
            if let Some(summary) = payload
                .get("payload_summary")
                .and_then(|value| value.as_str())
            {
                details.push(summary.to_string());
            }
            if !details.is_empty() {
                output.push_str(&format!("_{}_\n\n", details.join(", ")));
            }

            if let Some(preview) = payload
                .get("payload_preview")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
            {
                output.push_str("```json\n");
                output.push_str(preview);
                output.push_str("\n```\n\n");
            }
        }
        if sections.structured_payloads.len() > 3 {
            output.push_str(&format!(
                "_… {} more structured payloads_\n",
                sections.structured_payloads.len() - 3
            ));
        }
    }

    if !sections.pages.is_empty() {
        output.push_str("\n## Pages\n\n");
        for page in sections.pages.iter().take(5) {
            let page_number = page
                .get("page")
                .and_then(|value| value.as_u64())
                .unwrap_or_default();
            output.push_str(&format!("### Page {page_number}\n\n"));

            let mut details = Vec::new();
            if let Some(block_count) = page.get("block_count").and_then(|value| value.as_u64()) {
                details.push(format!("blocks={block_count}"));
            }
            if page
                .get("continued_from_previous_page")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                details.push("continued_from_previous_page=true".to_string());
            }
            if page
                .get("continued_to_next_page")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                details.push("continued_to_next_page=true".to_string());
            }
            if !details.is_empty() {
                output.push_str(&format!("_{}_\n\n", details.join(", ")));
            }

            if let Some(labels) = page.get("labels").and_then(|value| value.as_array()) {
                let rendered = labels
                    .iter()
                    .filter_map(|value| value.as_str())
                    .take(4)
                    .map(|label| format!("`{label}`"))
                    .collect::<Vec<_>>();
                if !rendered.is_empty() {
                    output.push_str(&format!("Labels: {}\n\n", rendered.join(", ")));
                }
            }

            if let Some(preview) = page
                .get("preview")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
            {
                for line in preview
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .take(3)
                {
                    output.push_str(&format!("> {}\n", line.trim()));
                }
                output.push('\n');
            }
        }
        if sections.pages.len() > 5 {
            output.push_str(&format!("_… {} more pages_\n", sections.pages.len() - 5));
        }
    }

    output.push_str(sections.structural_summary);

    if let Some(answer) = sections.llm_answer {
        output.push_str("\n## Query Answer\n\n");
        output.push_str(answer);
        output.push('\n');
    }

    output
}

pub(crate) struct ParseResultInput<'a> {
    pub path_display: &'a str,
    pub strategy_label: &'a str,
    pub document_parser_config: Option<&'a crate::config::DocumentParserConfig>,
    pub document_runtime: Option<&'a serde_json::Value>,
    pub document_quality: Option<&'a crate::document_pipeline::DocumentQualityReport>,
    pub block_count: usize,
    pub non_empty_block_count: usize,
    pub line_count: usize,
    pub word_count: usize,
    pub document_language: Option<&'a str>,
    pub document_keywords: &'a [String],
    pub document_provenance: Option<&'a serde_json::Value>,
    pub document_confidence: Option<&'a serde_json::Value>,
    pub query_used: bool,
    pub llm_input_truncated: bool,
    pub llm_blocks_included: usize,
    pub llm_block_details: &'a [serde_json::Value],
    pub chunk_highlights: &'a [serde_json::Value],
    pub structured_payloads: &'a [serde_json::Value],
    pub tables: &'a [serde_json::Value],
    pub pages: &'a [serde_json::Value],
    pub elements: &'a [serde_json::Value],
    pub max_chars: usize,
    pub structural_summary: &'a str,
    pub llm_answer: Option<&'a str>,
}

pub(crate) struct BuiltParseResult {
    pub content: String,
    pub metadata: serde_json::Value,
}

pub(crate) fn build_parse_result(input: &ParseResultInput<'_>) -> BuiltParseResult {
    let document_parser_summary = describe_document_parser_config(input.document_parser_config);
    let runtime_summary = summarize_document_runtime(input.document_runtime);
    let quality_summary = summarize_document_quality(input.document_quality);
    let provenance_summary = summarize_document_provenance(input.document_provenance);
    let confidence_summary = summarize_document_confidence(input.document_confidence);
    let content = build_parse_report(&ParseReportSections {
        header: ParseOutputHeader {
            path_display: input.path_display,
            strategy_label: input.strategy_label,
            document_parser_summary: &document_parser_summary,
            block_count: input.block_count,
            non_empty_block_count: input.non_empty_block_count,
            line_count: input.line_count,
            word_count: input.word_count,
            document_language: input.document_language,
            document_keywords: input.document_keywords,
            provenance_summary: provenance_summary.as_deref(),
            confidence_summary: confidence_summary.as_deref(),
            runtime_summary: runtime_summary.as_deref(),
            quality_summary: quality_summary.as_deref(),
        },
        query_used: input.query_used,
        llm_block_details: input.llm_block_details,
        chunk_highlights: input.chunk_highlights,
        structured_payloads: input.structured_payloads,
        pages: input.pages,
        structural_summary: input.structural_summary,
        llm_answer: input.llm_answer,
    });

    let metadata = json!({
        "file": input.path_display,
        "strategy": input.strategy_label,
        "document_parser": input.document_parser_config.map(|cfg| json!({
            "enabled": cfg.enabled,
            "max_file_size_mb": cfg.max_file_size_mb,
            "ocr": cfg.ocr.as_ref().map(|ocr| json!({
                "enabled": ocr.enabled,
                "model": ocr.model,
                "prompt": ocr.prompt,
                "max_images": ocr.max_images,
                "dpi": ocr.dpi,
            })),
        })),
        "document_runtime": input.document_runtime,
        "document_quality": input.document_quality,
        "document_language": input.document_language,
        "document_keywords": input.document_keywords,
        "document_provenance": input.document_provenance,
        "document_confidence": input.document_confidence,
        "blocks": input.block_count,
        "non_empty_blocks": input.non_empty_block_count,
        "lines": input.line_count,
        "words": input.word_count,
        "llm_used": input.query_used,
        "llm_input_truncated": input.llm_input_truncated,
        "llm_blocks_included": input.llm_blocks_included,
        "llm_blocks": input.llm_block_details,
        "chunk_highlights": input.chunk_highlights,
        "structured_payloads": input.structured_payloads,
        "tables": input.tables,
        "pages": input.pages,
        "elements": input.elements,
        "max_chars": input.max_chars,
        "query_aware_selection": input.query_used,
    });

    BuiltParseResult { content, metadata }
}

pub(crate) fn build_parse_tool_output(input: &ParseResultInput<'_>) -> crate::tools::ToolOutput {
    let built = build_parse_result(input);
    crate::tools::ToolOutput::success(built.content).with_metadata(built.metadata)
}

pub(crate) fn build_parse_tool_output_from_prepared(
    prepared: &PreparedParseDocument,
    document_parser_config: Option<&crate::config::DocumentParserConfig>,
    query_used: bool,
    max_chars: usize,
    llm_answer: Option<&str>,
) -> crate::tools::ToolOutput {
    build_parse_tool_output(&ParseResultInput {
        path_display: &prepared.path_display,
        strategy_label: &prepared.strategy_label,
        document_parser_config,
        document_runtime: prepared.document_runtime.as_ref(),
        document_quality: prepared.document_quality.as_ref(),
        block_count: prepared.block_count,
        non_empty_block_count: prepared.non_empty_block_count,
        line_count: prepared.line_count,
        word_count: prepared.word_count,
        document_language: prepared.document_language.as_deref(),
        document_keywords: &prepared.document_keywords,
        document_provenance: prepared.document_provenance.as_ref(),
        document_confidence: prepared.document_confidence.as_ref(),
        query_used,
        llm_input_truncated: prepared.llm_input_truncated,
        llm_blocks_included: prepared.llm_blocks_included,
        llm_block_details: &prepared.llm_block_details,
        chunk_highlights: &prepared.chunk_highlights,
        structured_payloads: &prepared.structured_payloads,
        tables: &prepared.tables,
        pages: &prepared.pages,
        elements: &prepared.elements,
        max_chars,
        structural_summary: &prepared.structural_summary,
        llm_answer,
    })
}

fn parse_structured_payload_value(payload: &str) -> serde_json::Value {
    serde_json::from_str(payload).unwrap_or_else(|_| serde_json::Value::String(payload.to_string()))
}

fn summarize_structured_payloads(
    doc: &ParsedDocument,
    block_kind_label: fn(&DocumentBlockKind) -> &'static str,
) -> Vec<serde_json::Value> {
    doc.blocks
        .iter()
        .enumerate()
        .filter_map(|(idx, block)| {
            let payload = block.structured_payload.as_deref()?;
            let parsed = parse_structured_payload_value(payload);
            let payload_preview = match &parsed {
                serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                    serde_json::to_string_pretty(&parsed).ok()
                }
                serde_json::Value::String(value) => Some(value.clone()),
                other => Some(other.to_string()),
            }
            .map(|preview| {
                let mut clipped = preview.chars().take(600).collect::<String>();
                if preview.chars().count() > 600 {
                    clipped.push('…');
                }
                clipped
            });

            Some(json!({
                "index": idx + 1,
                "kind": block_kind_label(&block.kind),
                "label": block.label,
                "location": block.location.as_ref().map(|location: &DocumentBlockLocation| json!({
                    "source": location.source,
                    "page": location.page,
                    "ordinal": location.ordinal,
                    "continued_from_previous_page": location.continued_from_previous_page,
                    "continued_to_next_page": location.continued_to_next_page,
                    "display": crate::document_render::format_block_location(location),
                })),
                "payload_summary": structured_payload_summary(&parsed),
                "payload_preview": payload_preview,
                "structured_payload": parsed,
            }))
        })
        .collect()
}

fn structured_payload_summary(payload: &serde_json::Value) -> String {
    match payload {
        serde_json::Value::Object(map) => {
            let keys = map.keys().take(6).cloned().collect::<Vec<_>>();
            format!("object keys={}", keys.join(", "))
        }
        serde_json::Value::Array(items) => format!("array items={}", items.len()),
        serde_json::Value::String(value) => {
            format!("string chars={}", value.chars().count())
        }
        serde_json::Value::Number(_) => "number".to_string(),
        serde_json::Value::Bool(_) => "boolean".to_string(),
        serde_json::Value::Null => "null".to_string(),
    }
}

/// Extract dedicated table data for stable machine-readable `tables[]` output.
/// This filters table blocks and formats them in a consistent structure.
fn extract_tables_for_output(doc: &ParsedDocument) -> Vec<serde_json::Value> {
    doc.blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| matches!(block.kind, DocumentBlockKind::Table))
        .filter_map(|(idx, block)| {
            let payload = block.structured_payload.as_deref()?;
            let parsed = parse_structured_payload_value(payload);

            // Extract table-specific fields from the structured payload
            let (row_count, column_count, headers, rows) = match &parsed {
                serde_json::Value::Object(map) => {
                    let row_count = map
                        .get("row_count")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(0);
                    let data_row_count = map
                        .get("data_row_count")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(row_count.saturating_sub(1));
                    let column_count = map
                        .get("column_count")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(0);
                    let headers: Vec<String> = map
                        .get("headers")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let rows: Vec<Vec<String>> = map
                        .get("rows")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|row| {
                                    row.as_array().map(|cells| {
                                        cells
                                            .iter()
                                            .filter_map(|c| c.as_str().map(String::from))
                                            .collect::<Vec<String>>()
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    (data_row_count, column_count, headers, rows)
                }
                _ => return None,
            };

            Some(json!({
                "index": idx + 1,
                "label": block.label,
                "location": block.location.as_ref().map(|location: &DocumentBlockLocation| json!({
                    "source": location.source,
                    "page": location.page,
                    "ordinal": location.ordinal,
                    "continued_from_previous_page": location.continued_from_previous_page,
                    "continued_to_next_page": location.continued_to_next_page,
                    "display": crate::document_render::format_block_location(location),
                })),
                "row_count": row_count,
                "column_count": column_count,
                "headers": headers,
                "rows": rows,
                "structured_payload": parsed,
            }))
        })
        .collect()
}

/// Extract unified elements for stable machine-readable `elements[]` output.
/// Combines blocks, tables, and pages into a single indexed array.
fn extract_elements_for_output(doc: &ParsedDocument) -> Vec<serde_json::Value> {
    let mut elements = Vec::new();
    let mut index = 0;

    // Add blocks as elements
    for block in &doc.blocks {
        let location_display = block
            .location
            .as_ref()
            .map(crate::document_render::format_block_location)
            .unwrap_or_default();

        elements.push(json!({
            "index": index,
            "kind": "block",
            "kind_detail": match block.kind {
                DocumentBlockKind::Paragraph => "paragraph",
                DocumentBlockKind::Heading => "heading",
                DocumentBlockKind::Table => "table",
                DocumentBlockKind::Section => "section",
                DocumentBlockKind::Metadata => "metadata",
                DocumentBlockKind::Slide => "slide",
                DocumentBlockKind::EmailHeader => "email_header",
                DocumentBlockKind::Code => "code",
                DocumentBlockKind::Raw => "raw",
            },
            "label": block.label,
            "content": block.content,
            "page": block.location.as_ref().and_then(|l| l.page),
            "source": block.location.as_ref().and_then(|l| l.source.clone()),
            "location": block.location.as_ref().map(|location: &DocumentBlockLocation| json!({
                "source": location.source,
                "page": location.page,
                "ordinal": location.ordinal,
                "continued_from_previous_page": location.continued_from_previous_page,
                "continued_to_next_page": location.continued_to_next_page,
                "display": location_display,
            })),
            "attributes": block.attributes,
            "structured_payload": block.structured_payload,
        }));
        index += 1;
    }

    // Add tables as elements
    for table in &doc.tables {
        elements.push(json!({
            "index": index,
            "kind": "table",
            "kind_detail": "table",
            "label": table.label,
            "content": if table.rows.is_empty() {
                String::new()
            } else {
                table.rows.iter().map(|row| row.join("\t")).collect::<Vec<_>>().join("\n")
            },
            "page": table.page,
            "source": table.source,
            "location": table.location.as_ref().map(|location: &DocumentBlockLocation| json!({
                "source": location.source,
                "page": location.page,
                "ordinal": location.ordinal,
                "continued_from_previous_page": location.continued_from_previous_page,
                "continued_to_next_page": location.continued_to_next_page,
                "display": crate::document_render::format_block_location(location),
            })),
            "attributes": {
                "row_count": table.row_count,
                "column_count": table.column_count,
            },
            "structured_payload": null,
        }));
        index += 1;
    }

    // Add pages as elements
    for page in &doc.pages {
        elements.push(json!({
            "index": index,
            "kind": "page",
            "kind_detail": "page",
            "label": null,
            "content": page.preview.clone().unwrap_or_default(),
            "page": page.page,
            "source": page.source,
            "location": null,
            "attributes": {
                "block_count": page.block_count,
                "continued_from_previous_page": page.continued_from_previous_page,
                "continued_to_next_page": page.continued_to_next_page,
            },
            "structured_payload": null,
        }));
        index += 1;
    }

    elements
}

fn summarize_pages_for_parse_report(doc: &ParsedDocument) -> Vec<serde_json::Value> {
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct PageSummary {
        source: Option<String>,
        block_count: usize,
        labels: Vec<String>,
        preview: Option<String>,
        continued_from_previous_page: bool,
        continued_to_next_page: bool,
    }

    let mut pages: BTreeMap<usize, PageSummary> = BTreeMap::new();

    for block in &doc.blocks {
        let Some(location) = block.location.as_ref() else {
            continue;
        };
        let Some(page) = location.page else {
            continue;
        };

        let summary = pages.entry(page).or_default();
        summary.block_count += 1;
        if summary.source.is_none() {
            summary.source = location.source.clone();
        }
        if let Some(label) = block
            .label
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if !summary.labels.iter().any(|existing| existing == label) {
                summary.labels.push(label.to_string());
            }
        }
        if summary.preview.is_none() {
            let preview = summarize_block_content(&block.content, 240);
            if !preview.trim().is_empty() {
                summary.preview = Some(preview);
            }
        }
        summary.continued_from_previous_page |= location.continued_from_previous_page;
        summary.continued_to_next_page |= location.continued_to_next_page;
    }

    pages
        .into_iter()
        .map(|(page, summary)| {
            json!({
                "page": page,
                "source": summary.source,
                "block_count": summary.block_count,
                "labels": summary.labels,
                "preview": summary.preview,
                "continued_from_previous_page": summary.continued_from_previous_page,
                "continued_to_next_page": summary.continued_to_next_page,
            })
        })
        .collect()
}

fn summarize_document_quality(
    quality: Option<&crate::document_pipeline::DocumentQualityReport>,
) -> Option<String> {
    let quality = quality?;
    let grade = match quality.grade {
        crate::document_pipeline::DocumentQualityGrade::Excellent => "excellent",
        crate::document_pipeline::DocumentQualityGrade::Good => "good",
        crate::document_pipeline::DocumentQualityGrade::Fair => "fair",
        crate::document_pipeline::DocumentQualityGrade::Poor => "poor",
    };
    let mut summary = format!("score={}, grade={}", quality.score, grade);
    if !quality.issues.is_empty() {
        let issue_preview = quality
            .issues
            .iter()
            .take(2)
            .map(|issue| issue.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        summary.push_str(&format!(", issues={}", issue_preview));
        if quality.issues.len() > 2 {
            summary.push_str(&format!(" (+{} more)", quality.issues.len() - 2));
        }
    }
    Some(summary)
}

fn summarize_document_provenance(metadata: Option<&serde_json::Value>) -> Option<String> {
    let metadata = metadata?;
    let mut parts = Vec::new();
    if let Some(parser) = metadata.get("parser").and_then(|value| value.as_str()) {
        parts.push(format!("parser=`{parser}`"));
    }
    if let Some(extractor) = metadata.get("extractor").and_then(|value| value.as_str()) {
        parts.push(format!("extractor=`{extractor}`"));
    }
    if let Some(provider) = metadata.get("provider").and_then(|value| value.as_str()) {
        parts.push(format!("provider=`{provider}`"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

fn summarize_document_confidence(metadata: Option<&serde_json::Value>) -> Option<String> {
    let metadata = metadata?;
    let mut parts = Vec::new();
    if let Some(score_percent) = metadata
        .get("score_percent")
        .and_then(|value| value.as_u64())
    {
        parts.push(format!("score_percent={score_percent}"));
    }
    if let Some(label) = metadata.get("label").and_then(|value| value.as_str()) {
        parts.push(format!("label=`{label}`"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

fn split_attribute_list(value: &str, limit: usize) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .take(limit)
        .map(str::to_string)
        .collect()
}

fn summarize_chunks_for_parse_report(
    chunks: &[crate::document_pipeline::DocumentChunk],
    limit: usize,
) -> Vec<serde_json::Value> {
    chunks
        .iter()
        .filter(|chunk| !chunk.content.trim().is_empty())
        .take(limit)
        .map(|chunk| {
            json!({
                "label": chunk.label.as_deref().or(chunk.context_label.as_deref()).unwrap_or("unlabeled"),
                "context_label": chunk.context_label,
                "locator": chunk.locator,
                "language": chunk.language,
                "keywords": chunk.keywords,
                "score": chunk.score,
                "preview": summarize_block_content(&chunk.content, 240),
            })
        })
        .collect()
}
