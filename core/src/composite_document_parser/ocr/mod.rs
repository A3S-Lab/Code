#[path = "builtin.rs"]
mod builtin;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::document_parser::{
    DocumentBlock, DocumentBlockKind, DocumentConfidence, DocumentMetadata, DocumentProvenance,
    ParsedDocument,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentOcrCapabilities {
    pub formats: Vec<String>,
    pub model: Option<String>,
    pub prompt_configurable: bool,
    pub page_level_results: bool,
    pub confidence_scores: bool,
    pub language_detection: bool,
    pub layout_boxes: bool,
}

impl DocumentOcrCapabilities {
    pub fn new(formats: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            formats: formats.into_iter().map(Into::into).collect(),
            model: None,
            prompt_configurable: true,
            page_level_results: false,
            confidence_scores: false,
            language_detection: false,
            layout_boxes: false,
        }
    }

    pub fn supports_format(&self, format: DocumentOcrFormat) -> bool {
        self.formats
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(format.as_str()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentOcrFormat {
    Pdf,
    Docx,
    Xlsx,
    Pptx,
    Odf,
    Image,
}

impl DocumentOcrFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Pptx => "pptx",
            Self::Odf => "odf",
            Self::Image => "image",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DocumentOcrRequest<'a> {
    pub path: &'a Path,
    pub format: DocumentOcrFormat,
    pub config: &'a crate::config::DocumentOcrConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentOcrPageResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_score_percent: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentOcrOutput {
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub pages: Vec<DocumentOcrPageResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_score_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl DocumentOcrOutput {
    pub fn from_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            pages: Vec::new(),
            language: None,
            confidence_score_percent: None,
            model: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentOcrRuntimeInfo {
    pub used: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_images: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_score_percent: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentRuntimeMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr: Option<DocumentOcrRuntimeInfo>,
}

pub trait DocumentOcrProvider: Send + Sync {
    fn name(&self) -> &str;

    fn capabilities(&self) -> DocumentOcrCapabilities {
        DocumentOcrCapabilities::new(["pdf"])
    }

    fn ocr_document(&self, request: &DocumentOcrRequest<'_>) -> Result<Option<String>> {
        match request.format {
            DocumentOcrFormat::Pdf => self.ocr_pdf(request.path, request.config),
            _ => Ok(None),
        }
    }

    fn ocr_document_result(
        &self,
        request: &DocumentOcrRequest<'_>,
    ) -> Result<Option<DocumentOcrOutput>> {
        self.ocr_document(request)
            .map(|output| output.map(DocumentOcrOutput::from_text))
    }

    fn ocr_pdf(
        &self,
        _path: &Path,
        _config: &crate::config::DocumentOcrConfig,
    ) -> Result<Option<String>> {
        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum PdfOcrMode {
    Skipped,
    Used,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct OcrResult {
    pub(super) text: String,
    pub(super) mode: PdfOcrMode,
    pub(super) provider_name: Option<String>,
    pub(super) pages: Vec<DocumentOcrPageResult>,
    pub(super) page_count: Option<usize>,
    pub(super) language: Option<String>,
    pub(super) confidence_score_percent: Option<u8>,
    pub(super) model: Option<String>,
    pub(super) structured_payload: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CachedOcrEntry {
    provider_name: String,
    output: DocumentOcrOutput,
}

pub(super) fn parse_pdf_document(
    path: &Path,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<ParsedDocument> {
    let extracted_text = parse_pdf(path).unwrap_or_default();
    let ocr = maybe_run_pdf_ocr(path, extracted_text, config, ocr_provider)?;
    let metadata = build_ocr_metadata_block(config, DocumentOcrFormat::Pdf, &ocr);
    let mut doc = if ocr.mode == PdfOcrMode::Used && !ocr.pages.is_empty() {
        parsed_ocr_document(path, &ocr, DocumentBlockKind::Paragraph)?
    } else {
        super::parsed_paged_text_document(path, ocr.text, DocumentBlockKind::Paragraph)?
    };
    if let Some(metadata) = metadata {
        doc.blocks.insert(0, metadata);
    }
    Ok(doc)
}

pub(super) fn parse_image_document(
    path: &Path,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<ParsedDocument> {
    let ocr = maybe_run_image_ocr(path, config, ocr_provider)?;
    let metadata = build_ocr_metadata_block(config, DocumentOcrFormat::Image, &ocr);
    let mut doc = if ocr.mode == PdfOcrMode::Used && !ocr.pages.is_empty() {
        parsed_ocr_document(path, &ocr, DocumentBlockKind::Paragraph)?
    } else {
        super::parsed_text_document(path, ocr.text, DocumentBlockKind::Paragraph)?
    };
    if let Some(metadata) = metadata {
        doc.blocks.insert(0, metadata);
    }
    Ok(doc)
}

pub(super) fn maybe_run_document_ocr_fallback(
    path: &Path,
    format: DocumentOcrFormat,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<Option<ParsedDocument>> {
    if config.ocr.as_ref().filter(|ocr| ocr.enabled).is_none() {
        return Ok(None);
    }
    let builtin_provider;
    let provider = if let Some(provider) = ocr_provider {
        provider
    } else {
        builtin_provider = builtin::BuiltinOcrProvider::discover();
        let Some(provider) = builtin_provider
            .as_ref()
            .map(|provider| provider as &dyn DocumentOcrProvider)
        else {
            return Ok(None);
        };
        provider
    };

    let ocr = run_ocr_request(path, format, String::new(), config, provider)?;
    if ocr.mode != PdfOcrMode::Used || ocr.text.trim().is_empty() {
        return Ok(None);
    }

    let metadata = build_ocr_metadata_block(config, format, &ocr);
    let mut doc = if !ocr.pages.is_empty() {
        parsed_ocr_document(path, &ocr, DocumentBlockKind::Paragraph)?
    } else {
        super::parsed_text_document(path, ocr.text, DocumentBlockKind::Paragraph)?
    };
    if let Some(metadata) = metadata {
        doc.blocks.insert(0, metadata);
    }
    Ok(Some(doc))
}

fn parsed_ocr_document(
    path: &Path,
    ocr: &OcrResult,
    default_kind: DocumentBlockKind,
) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    let source = doc
        .title
        .clone()
        .unwrap_or_else(|| path.display().to_string());

    let mut ordinal = 1usize;
    for (index, page_result) in ocr.pages.iter().enumerate() {
        let page_number = page_result.page.unwrap_or(index + 1);
        let blocks = super::paged_text_blocks(&page_result.text, default_kind.clone());
        for (page_block_index, block) in blocks.into_iter().enumerate() {
            let mut block = super::label_paged_block(block, page_number, page_block_index + 1)
                .with_source(source.clone())
                .with_page(page_number)
                .with_ordinal(ordinal)
                .with_attribute("ocr_page", page_number.to_string());
            if let Some(language) = &page_result.language {
                block = block.with_attribute("ocr_language", language);
            }
            if let Some(confidence) = page_result.confidence_score_percent {
                block = block
                    .with_attribute("ocr_confidence_score_percent", u8::to_string(&confidence));
            }
            doc.push(block);
            ordinal += 1;
        }
    }

    if doc.blocks.is_empty() {
        return super::parsed_text_document(path, ocr.text.clone(), default_kind);
    }

    super::ensure_document(doc, path)
}

pub fn extract_document_runtime_metadata(doc: &ParsedDocument) -> Option<DocumentRuntimeMetadata> {
    let block = doc.blocks.iter().find(|block| {
        block.kind == DocumentBlockKind::Metadata && block.label.as_deref() == Some("ocr")
    })?;

    let mut runtime = DocumentOcrRuntimeInfo {
        used: true,
        mode: None,
        format: None,
        provider: None,
        model: None,
        prompt: None,
        max_images: None,
        dpi: None,
        page_count: None,
        language: None,
        confidence_score_percent: None,
    };

    for line in block
        .content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "mode" => runtime.mode = Some(value.to_string()),
            "format" => runtime.format = Some(value.to_string()),
            "provider" => runtime.provider = Some(value.to_string()),
            "model" => runtime.model = Some(value.to_string()),
            "prompt" => runtime.prompt = Some(value.to_string()),
            "max_images" => runtime.max_images = value.parse::<usize>().ok(),
            "dpi" => runtime.dpi = value.parse::<u32>().ok(),
            "page_count" => runtime.page_count = value.parse::<usize>().ok(),
            "language" if value != "unknown" => runtime.language = Some(value.to_string()),
            "confidence_score_percent" => {
                runtime.confidence_score_percent = value.parse::<u8>().ok()
            }
            _ => {}
        }
    }

    Some(DocumentRuntimeMetadata { ocr: Some(runtime) })
}

fn parse_pdf(path: &Path) -> Result<String> {
    // Try lopdf first for better text extraction with position info
    let lopdf_result = parse_pdf_with_lopdf(path);
    let lopdf_text = lopdf_result.unwrap_or_default();

    // Try pdf-extract for comparison
    let pdf_extract_text = pdf_extract::extract_text(path)
        .with_context(|| format!("failed to extract text from PDF {}", path.display()))?;

    // Use the better extraction result based on content quality
    // Prefer lopdf if it has meaningful content (position-aware extraction)
    // But use pdf-extract if it has significantly more content (5x threshold)
    let lopdf_trimmed = lopdf_text.trim();
    let pdf_extract_trimmed = pdf_extract_text.trim();

    if lopdf_trimmed.is_empty() {
        return Ok(pdf_extract_trimmed.to_string());
    }

    if pdf_extract_trimmed.is_empty() {
        return Ok(lopdf_trimmed.to_string());
    }

    // If pdf-extract has significantly more content, prefer it
    // This handles cases where lopdf misses embedded fonts or complex content
    if pdf_extract_trimmed.len() > lopdf_trimmed.len() * 5 {
        return Ok(pdf_extract_trimmed.to_string());
    }

    // Otherwise prefer lopdf for better position info
    Ok(lopdf_trimmed.to_string())
}

/// Text item with position for table detection.
#[derive(Debug, Clone)]
struct PositionedTextItem {
    page: usize,
    y: f32,
    x: f32,
    text: String,
    /// Y coordinate scaled to integer for grouping
    y_scaled: i32,
}

/// Extract text from PDF using lopdf for better position-aware extraction.
///
/// This provides improved text ordering (top-to-bottom, left-to-right) compared to
/// pdf-extract which may return text in random order.
fn parse_pdf_with_lopdf(path: &Path) -> Result<String> {
    use lopdf::Document;
    use std::collections::BTreeMap;

    let doc =
        Document::load(path).with_context(|| format!("failed to load PDF {}", path.display()))?;

    let pages = doc.get_pages();
    if pages.is_empty() {
        anyhow::bail!("PDF has no pages: {}", path.display());
    }

    // Collect all text items with positions
    let mut all_items: Vec<PositionedTextItem> = Vec::new();

    // Collect all pages and sort by page number
    let mut page_list: Vec<(u32, lopdf::ObjectId)> = pages.iter().map(|(&k, &v)| (k, v)).collect();
    page_list.sort_by_key(|(num, _)| *num);

    for (page_num, page_id) in page_list {
        // Get page content stream
        if let Ok(contents) = doc.get_page_content(page_id) {
            // Parse content stream to extract text with positions
            let text_items = extract_text_from_content_stream(&contents, page_num as usize);
            for (y, x, text) in text_items {
                if !text.trim().is_empty() {
                    // Scale y by 1000 to group rows (tolerance of ~1 pixel for 72dpi)
                    let y_scaled = (y * 1000.0) as i32;
                    all_items.push(PositionedTextItem {
                        page: page_num as usize,
                        y,
                        x,
                        text,
                        y_scaled,
                    });
                }
            }
        }
    }

    if all_items.is_empty() {
        anyhow::bail!("PDF has no extractable text: {}", path.display());
    }

    // Build output with page markers and row structure preserved
    let mut output = String::new();
    let mut current_page = 0usize;

    // Group items by page and y position (row grouping)
    let mut page_groups: BTreeMap<usize, BTreeMap<i32, Vec<PositionedTextItem>>> = BTreeMap::new();
    for item in all_items {
        page_groups
            .entry(item.page)
            .or_default()
            .entry(item.y_scaled)
            .or_default()
            .push(item);
    }

    for (page_num, y_groups) in page_groups {
        // Add page break marker when page changes
        if page_num != current_page {
            if current_page > 0 {
                output.push_str("\n\n[_PAGE_BREAK_]\n\n");
            }
            current_page = page_num;
        }

        // Process rows in order (BTreeMap is sorted by y_scaled)
        for (_y_key, mut items) in y_groups {
            // Sort items within row by x position
            items.sort_by(|a, b| {
                let a_x = (a.x * 100.0) as i32;
                let b_x = (b.x * 100.0) as i32;
                let x_cmp = a_x.cmp(&b_x);
                x_cmp.then_with(|| a.text.cmp(&b.text))
            });

            // Join items on same row
            let row_text = items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");

            // Detect potential table rows by checking:
            // 1. Row has multiple cell-like fragments (separated by tabs or consistent spacing)
            // 2. Row looks like it could be a table row (multiple aligned segments)
            let is_potential_table_row = detect_table_row(&items);

            if is_potential_table_row {
                // Mark as potential table row for downstream table detection
                output.push_str(&format!("[_TABLE_ROW_]{}[_TABLE_ROW_]", row_text));
            } else {
                output.push_str(&row_text);
            }
            output.push('\n');
        }
    }

    Ok(output)
}

/// Detect if a row of text items might be part of a table.
///
/// Looks for patterns like:
/// - Multiple tab-separated cells
/// - Multiple fragments with consistent horizontal spacing (column alignment)
/// - Fragments that look like table cells (short text, consistent width)
fn detect_table_row(items: &[PositionedTextItem]) -> bool {
    if items.len() < 2 {
        return false;
    }

    // Check for explicit tab separators
    let tab_count = items.iter().filter(|i| i.text.contains('\t')).count();
    if tab_count > 0 {
        return true;
    }

    // Check for consistent spacing between items (column alignment indicator)
    if items.len() >= 2 {
        let mut gaps: Vec<i32> = Vec::new();
        for i in 1..items.len() {
            let gap = ((items[i].x - items[i - 1].x) * 100.0) as i32;
            gaps.push(gap);
        }

        // If we have 3+ items and gaps are somewhat consistent, might be a table
        if items.len() >= 3 {
            let avg_gap: i32 = gaps.iter().sum::<i32>() / gaps.len() as i32;
            let variance: i32 =
                gaps.iter().map(|g| (g - avg_gap).abs()).sum::<i32>() / gaps.len() as i32;
            // Low variance indicates column alignment
            if variance < avg_gap / 4 && avg_gap > 50 {
                return true;
            }
        }
    }

    // Check if items look like table cells (short, similar length)
    let all_short = items
        .iter()
        .all(|i| i.text.len() <= 30 && !i.text.contains(' '));
    let similar_length = items.len() >= 2 && {
        let avg_len = items.iter().map(|i| i.text.len() as i32).sum::<i32>() / items.len() as i32;
        items.iter().all(|i| {
            let len = i.text.len() as i32;
            (len - avg_len).abs() < avg_len / 2
        })
    };

    all_short && similar_length && items.len() >= 3
}

/// Extract text items from PDF content stream with position information.
///
/// Returns vector of (y_position, x_position, text) tuples.
fn extract_text_from_content_stream(contents: &[u8], _page_num: usize) -> Vec<(f32, f32, String)> {
    let mut items = Vec::new();
    let content_str = String::from_utf8_lossy(contents);

    // Track current text position
    let mut current_x = 0f32;
    let mut current_y = 0f32;
    let mut in_text_block = false;

    for line in content_str.lines() {
        let line = line.trim();
        if line == "BT" {
            in_text_block = true;
            continue;
        }
        if line == "ET" {
            in_text_block = false;
            continue;
        }
        if !in_text_block {
            continue;
        }

        // Parse text positioning: Tm (text matrix), Td (text position), TD (text position)
        if line.contains("Tm") || line.contains("Td") || line.contains("TD") {
            if let Some(coords) = parse_text_position(line) {
                current_x = coords.0;
                current_y = coords.1;
            }
        }

        // Parse text showing: Tj (show text), TJ (show text with spacing)
        if line.contains("Tj") {
            if let Some(text) = extract_text_from_tj(line) {
                items.push((current_y, current_x, text));
            }
        } else if line.contains("TJ") {
            if let Some(texts) = extract_text_from_tj_array(line) {
                for text in texts {
                    items.push((current_y, current_x, text));
                    current_x += 5.0; // approximate advancement
                }
            }
        }
    }

    // If we couldn't extract with operator parsing, try simple text extraction
    if items.is_empty() {
        // Fallback: just extract readable text from content
        let text = content_str
            .chars()
            .filter(|c| c.is_ascii_graphic() || c.is_whitespace())
            .collect::<String>();
        if !text.is_empty() {
            items.push((0.0, 0.0, text));
        }
    }

    items
}

/// Parse text positioning from Tm, Td, or TD operators.
/// Returns (x, y) coordinates.
fn parse_text_position(line: &str) -> Option<(f32, f32)> {
    // Tm format: a b c d e f Tm
    // Td format: tx ty Td
    // TD format: tx ty TD
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    // Find the numeric values
    let nums: Vec<f32> = parts.iter().filter_map(|s| s.parse::<f32>().ok()).collect();

    if line.contains("Tm") && nums.len() >= 6 {
        // Tm: matrix is [a b c d e f], position is e (x) f (y)
        Some((nums[4], nums[5]))
    } else if (line.contains("Td") || line.contains("TD")) && nums.len() >= 2 {
        // Td/TD: tx ty
        Some((nums[0], nums[1]))
    } else {
        None
    }
}

/// Extract text from Tj operator.
fn extract_text_from_tj(line: &str) -> Option<String> {
    // Tj format: (text) Tj
    if let Some(start) = line.find("(") {
        if let Some(end) = line[start + 1..].find(")") {
            let text = &line[start + 1..start + 1 + end];
            if !text.is_empty() {
                return Some(decode_pdf_string(text));
            }
        }
    }
    None
}

/// Extract text from TJ operator (text array).
fn extract_text_from_tj_array(line: &str) -> Option<Vec<String>> {
    // TJ format: [(text1) (text2) ...] TJ
    if !line.contains("[") || !line.contains("]") {
        return None;
    }

    let mut texts = Vec::new();
    let between_brackets = line.split('[').nth(1)?.split(']').next()?;

    for part in between_brackets.split('(') {
        if let Some(end) = part.find(')') {
            let text = &part[..end];
            if !text.is_empty() {
                texts.push(decode_pdf_string(text));
            }
        }
    }

    if texts.is_empty() {
        None
    } else {
        Some(texts)
    }
}

/// Decode PDF string escape sequences.
fn decode_pdf_string(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    'n' => result.push('\n'),
                    'r' => result.push('\r'),
                    't' => result.push('\t'),
                    '\\' => result.push('\\'),
                    '(' => result.push('('),
                    ')' => result.push(')'),
                    c if c.is_ascii_digit() => {
                        // Octal escape
                        let mut octal = String::from(c);
                        for _ in 0..2 {
                            if let Some(&peek) = chars.peek() {
                                if peek.is_ascii_digit() && peek != '8' && peek != '9' {
                                    octal.push(chars.next().unwrap());
                                } else {
                                    break;
                                }
                            }
                        }
                        if let Ok(val) = u8::from_str_radix(&octal, 8) {
                            result.push(val as char);
                        }
                    }
                    _ => result.push(next),
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

pub(super) fn maybe_run_pdf_ocr(
    path: &Path,
    extracted_text: String,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<OcrResult> {
    if !should_attempt_pdf_ocr(&extracted_text, config) {
        return Ok(OcrResult {
            text: extracted_text,
            mode: PdfOcrMode::Skipped,
            provider_name: None,
            pages: Vec::new(),
            page_count: None,
            language: None,
            confidence_score_percent: None,
            model: None,
            structured_payload: None,
        });
    }

    if config.ocr.as_ref().filter(|ocr| ocr.enabled).is_none() {
        return Ok(OcrResult {
            text: extracted_text,
            mode: PdfOcrMode::Skipped,
            provider_name: None,
            pages: Vec::new(),
            page_count: None,
            language: None,
            confidence_score_percent: None,
            model: None,
            structured_payload: None,
        });
    }
    let builtin_provider;
    let provider = if let Some(provider) = ocr_provider {
        provider
    } else {
        builtin_provider = builtin::BuiltinOcrProvider::discover();
        let Some(provider) = builtin_provider
            .as_ref()
            .map(|provider| provider as &dyn DocumentOcrProvider)
        else {
            tracing::debug!(
                "CompositeDocumentParser OCR fallback enabled for {} but no configured or built-in OCR backend was available",
                path.display()
            );
            return Ok(OcrResult {
                text: extracted_text,
                mode: PdfOcrMode::Fallback,
                provider_name: None,
                pages: Vec::new(),
                page_count: None,
                language: None,
                confidence_score_percent: None,
                model: None,
                structured_payload: None,
            });
        };
        provider
    };
    run_ocr_request(
        path,
        DocumentOcrFormat::Pdf,
        extracted_text,
        config,
        provider,
    )
}

fn maybe_run_image_ocr(
    path: &Path,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<OcrResult> {
    if config.ocr.as_ref().filter(|ocr| ocr.enabled).is_none() {
        anyhow::bail!("no extractable text found in {}", path.display());
    }
    let builtin_provider;
    let provider = if let Some(provider) = ocr_provider {
        provider
    } else {
        builtin_provider = builtin::BuiltinOcrProvider::discover();
        builtin_provider
            .as_ref()
            .map(|provider| provider as &dyn DocumentOcrProvider)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "image context extraction requires a configured OCR backend or local tesseract when document_parser.ocr.enabled=true"
                )
            })?
    };

    run_ocr_request(
        path,
        DocumentOcrFormat::Image,
        String::new(),
        config,
        provider,
    )
}

fn run_ocr_request(
    path: &Path,
    format: DocumentOcrFormat,
    fallback_text: String,
    config: &crate::config::DocumentParserConfig,
    provider: &dyn DocumentOcrProvider,
) -> Result<OcrResult> {
    let ocr_config = config
        .ocr
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("OCR config missing for {}", path.display()))?;
    let capabilities = provider.capabilities();
    if !capabilities.supports_format(format) {
        tracing::debug!(
            "CompositeDocumentParser OCR backend '{}' does not declare support for {} on {}",
            provider.name(),
            format.as_str(),
            path.display()
        );
        return Ok(OcrResult {
            text: fallback_text,
            mode: PdfOcrMode::Fallback,
            provider_name: Some(provider.name().to_string()),
            pages: Vec::new(),
            page_count: None,
            language: None,
            confidence_score_percent: None,
            model: None,
            structured_payload: None,
        });
    }
    let request = DocumentOcrRequest {
        path,
        format,
        config: ocr_config,
    };

    if let Some(cached) = load_cached_ocr_output(path, format, config, provider.name())? {
        tracing::debug!(
            "CompositeDocumentParser reused cached OCR output for {} via provider '{}'",
            path.display(),
            provider.name()
        );
        return Ok(ocr_result_from_output(cached.output, cached.provider_name));
    }

    match provider.ocr_document_result(&request) {
        Ok(Some(ocr_output)) if !ocr_output.text.trim().is_empty() => {
            tracing::info!(
                "CompositeDocumentParser used OCR backend '{}' for {}",
                provider.name(),
                path.display()
            );
            let _ = store_cached_ocr_output(
                path,
                format,
                config,
                &CachedOcrEntry {
                    provider_name: provider.name().to_string(),
                    output: ocr_output.clone(),
                },
            );
            Ok(ocr_result_from_output(
                ocr_output,
                provider.name().to_string(),
            ))
        }
        Ok(_) => Ok(OcrResult {
            text: fallback_text,
            mode: PdfOcrMode::Fallback,
            provider_name: Some(provider.name().to_string()),
            pages: Vec::new(),
            page_count: None,
            language: None,
            confidence_score_percent: None,
            model: None,
            structured_payload: None,
        }),
        Err(err) => {
            tracing::warn!(
                "CompositeDocumentParser OCR backend '{}' failed on {}: {}",
                provider.name(),
                path.display(),
                err
            );
            Ok(OcrResult {
                text: fallback_text,
                mode: PdfOcrMode::Fallback,
                provider_name: Some(provider.name().to_string()),
                pages: Vec::new(),
                page_count: None,
                language: None,
                confidence_score_percent: None,
                model: None,
                structured_payload: None,
            })
        }
    }
}

fn ocr_result_from_output(output: DocumentOcrOutput, provider_name: String) -> OcrResult {
    let page_count = (!output.pages.is_empty()).then_some(output.pages.len());
    let structured_payload = serde_json::to_string(&output).ok();
    OcrResult {
        text: output.text,
        mode: PdfOcrMode::Used,
        provider_name: Some(provider_name),
        pages: output.pages,
        page_count,
        language: output.language,
        confidence_score_percent: output.confidence_score_percent,
        model: output.model,
        structured_payload,
    }
}

fn load_cached_ocr_output(
    path: &Path,
    format: DocumentOcrFormat,
    config: &crate::config::DocumentParserConfig,
    provider_name: &str,
) -> Result<Option<CachedOcrEntry>> {
    let Some(cache_key) = build_ocr_cache_key(path, format, config, provider_name)? else {
        return Ok(None);
    };
    let cache_store = crate::document_pipeline_defaults::build_default_document_cache_store(config);
    let Some(payload) = cache_store.get_ocr_payload(&cache_key)? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&payload)?))
}

fn store_cached_ocr_output(
    path: &Path,
    format: DocumentOcrFormat,
    config: &crate::config::DocumentParserConfig,
    entry: &CachedOcrEntry,
) -> Result<()> {
    let Some(cache_key) = build_ocr_cache_key(path, format, config, &entry.provider_name)? else {
        return Ok(());
    };
    let cache_store = crate::document_pipeline_defaults::build_default_document_cache_store(config);
    cache_store.put_ocr_payload(&cache_key, &serde_json::to_string(entry)?)?;
    Ok(())
}

fn build_ocr_cache_key(
    path: &Path,
    format: DocumentOcrFormat,
    config: &crate::config::DocumentParserConfig,
    provider_name: &str,
) -> Result<Option<crate::document_pipeline::DocumentOcrCacheKey>> {
    let Some(cache_config) = config.cache.as_ref().filter(|cache| cache.enabled) else {
        return Ok(None);
    };
    let _ = cache_config.directory.as_deref();
    let file_hash = sha256::digest(std::fs::read(path)?);
    let ocr = config.ocr.as_ref();
    let signature = format!(
        "format={};provider={};model={};prompt={};max_images={};dpi={}",
        format.as_str(),
        provider_name,
        ocr.and_then(|cfg| cfg.model.as_deref()).unwrap_or("unset"),
        if ocr
            .and_then(|cfg| cfg.prompt.as_deref())
            .is_some_and(|prompt| !prompt.trim().is_empty())
        {
            "set"
        } else {
            "unset"
        },
        ocr.map(|cfg| cfg.max_images).unwrap_or(0),
        ocr.map(|cfg| cfg.dpi).unwrap_or(0),
    );
    Ok(Some(crate::document_pipeline::DocumentOcrCacheKey {
        path: path.display().to_string(),
        file_hash,
        format: format.as_str().to_string(),
        provider: provider_name.to_string(),
        ocr_signature: signature,
    }))
}

pub(super) fn build_ocr_metadata_block(
    config: &crate::config::DocumentParserConfig,
    format: DocumentOcrFormat,
    ocr: &OcrResult,
) -> Option<DocumentBlock> {
    if ocr.mode != PdfOcrMode::Used {
        return None;
    }

    let ocr_config = config.ocr.as_ref()?;
    let lines = vec![
        "mode=ocr".to_string(),
        format!("format={}", format.as_str()),
        format!(
            "provider={}",
            ocr.provider_name.as_deref().unwrap_or("unknown")
        ),
        format!(
            "model={}",
            ocr.model
                .as_deref()
                .or(ocr_config.model.as_deref())
                .unwrap_or("unset")
        ),
        format!(
            "prompt={}",
            if ocr_config
                .prompt
                .as_deref()
                .is_some_and(|prompt| !prompt.trim().is_empty())
            {
                "set"
            } else {
                "unset"
            }
        ),
        format!("max_images={}", ocr_config.max_images),
        format!("dpi={}", ocr_config.dpi),
        format!("page_count={}", ocr.page_count.unwrap_or(0)),
        format!("language={}", ocr.language.as_deref().unwrap_or("unknown")),
        format!(
            "confidence_score_percent={}",
            ocr.confidence_score_percent.unwrap_or(0)
        ),
    ];

    let mut block = DocumentBlock::new(DocumentBlockKind::Metadata, Some("ocr"), lines.join("\n"))
        .with_source("document_parser")
        .with_ordinal(0)
        .with_attribute("mode", "ocr")
        .with_attribute("format", format.as_str())
        .with_attribute(
            "provider",
            ocr.provider_name.as_deref().unwrap_or("unknown"),
        )
        .with_attribute(
            "model",
            ocr.model
                .as_deref()
                .or(ocr_config.model.as_deref())
                .unwrap_or("unset"),
        )
        .with_attribute(
            "prompt",
            if ocr_config
                .prompt
                .as_deref()
                .is_some_and(|prompt| !prompt.trim().is_empty())
            {
                "set"
            } else {
                "unset"
            },
        )
        .with_attribute("max_images", ocr_config.max_images.to_string())
        .with_attribute("dpi", ocr_config.dpi.to_string())
        .with_attribute("page_count", ocr.page_count.unwrap_or(0).to_string())
        .with_attribute("language", ocr.language.as_deref().unwrap_or("unknown"))
        .with_attribute(
            "confidence_score_percent",
            ocr.confidence_score_percent.unwrap_or(0).to_string(),
        )
        .with_metadata(DocumentMetadata {
            detected_file_type: Some(format.as_str().to_string()),
            provenance: Some(DocumentProvenance {
                parser: Some("composite-document-parser".to_string()),
                extractor: Some("ocr".to_string()),
                provider: ocr.provider_name.clone(),
            }),
            confidence: Some(DocumentConfidence {
                score_percent: ocr.confidence_score_percent.or(Some(90)),
                label: Some("high".to_string()),
            }),
            language: ocr.language.clone(),
            ..DocumentMetadata::default()
        });
    if let Some(payload) = &ocr.structured_payload {
        block = block.with_structured_payload(payload.clone());
    }
    Some(block)
}

pub(super) fn should_attempt_pdf_ocr(
    text: &str,
    config: &crate::config::DocumentParserConfig,
) -> bool {
    let Some(ocr) = config.ocr.as_ref() else {
        return false;
    };
    if !ocr.enabled {
        return false;
    }

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }

    let char_count = trimmed.chars().count();
    let word_count = trimmed.split_whitespace().count();
    let alnum_count = trimmed.chars().filter(|ch| ch.is_alphanumeric()).count();
    let alnum_ratio = alnum_count as f32 / char_count.max(1) as f32;

    char_count < 80 || word_count < 20 || alnum_ratio < 0.45
}
