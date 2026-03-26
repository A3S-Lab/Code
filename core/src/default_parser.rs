use anyhow::{Context, Result};
use base64::Engine as _;
use roxmltree::Document;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use zip::ZipArchive;

use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

/// Built-in rich document parser inspired by Kreuzberg's multi-format extraction model.
///
/// This parser handles common binary and containerized document formats and returns
/// plain text suitable for `agentic_parse` and `agentic_search`.
pub trait DefaultParserOcrProvider: Send + Sync {
    fn name(&self) -> &str;

    fn ocr_pdf(
        &self,
        path: &Path,
        config: &crate::config::DefaultParserOcrConfig,
    ) -> Result<Option<String>>;
}

#[derive(Default)]
pub struct DefaultParser {
    config: crate::config::DefaultParserConfig,
    ocr_provider: Option<Arc<dyn DefaultParserOcrProvider>>,
}

impl DefaultParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: crate::config::DefaultParserConfig) -> Self {
        Self {
            config,
            ocr_provider: None,
        }
    }

    pub fn with_config_and_ocr(
        config: crate::config::DefaultParserConfig,
        ocr_provider: Arc<dyn DefaultParserOcrProvider>,
    ) -> Self {
        Self {
            config,
            ocr_provider: Some(ocr_provider),
        }
    }

    pub fn config(&self) -> &crate::config::DefaultParserConfig {
        &self.config
    }

    pub fn ocr_provider(&self) -> Option<&Arc<dyn DefaultParserOcrProvider>> {
        self.ocr_provider.as_ref()
    }
}

impl crate::document_parser::DocumentParser for DefaultParser {
    fn name(&self) -> &str {
        "default-parser"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[
            "pdf", "docx", "xlsx", "xlsm", "pptx", "odt", "ods", "odp", "epub", "rtf", "html",
            "htm", "xhtml", "xml", "eml",
        ]
    }

    fn parse(&self, path: &Path) -> Result<String> {
        Ok(self.parse_document(path)?.to_text())
    }

    fn parse_document(&self, path: &Path) -> Result<ParsedDocument> {
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_default();

        match ext.as_str() {
            "pdf" => parse_pdf_document(path, &self.config, self.ocr_provider.as_deref()),
            "docx" => parse_docx(path),
            "xlsx" | "xlsm" => parse_xlsx(path),
            "pptx" => parse_pptx(path),
            "odt" | "ods" | "odp" => parse_odf(path),
            "epub" => parse_epub(path),
            "eml" => parse_eml(path),
            "rtf" => parsed_text_document(path, parse_rtf(path)?, DocumentBlockKind::Paragraph),
            "html" | "htm" | "xhtml" => parse_html_document(path),
            "xml" => parse_xml_document(path),
            _ => anyhow::bail!("unsupported extension for kreuzberg parser"),
        }
    }

    fn max_file_size(&self) -> u64 {
        self.config.max_file_size_mb * 1024 * 1024
    }
}

fn parse_pdf(path: &Path) -> Result<String> {
    pdf_extract::extract_text(path)
        .with_context(|| format!("failed to extract text from PDF {}", path.display()))
}

fn parse_pdf_document(
    path: &Path,
    config: &crate::config::DefaultParserConfig,
    ocr_provider: Option<&dyn DefaultParserOcrProvider>,
) -> Result<ParsedDocument> {
    let extracted_text = parse_pdf(path).unwrap_or_default();
    let text = maybe_run_pdf_ocr(path, extracted_text, config, ocr_provider)?;
    parsed_text_document(path, text, DocumentBlockKind::Paragraph)
}

fn maybe_run_pdf_ocr(
    path: &Path,
    extracted_text: String,
    config: &crate::config::DefaultParserConfig,
    ocr_provider: Option<&dyn DefaultParserOcrProvider>,
) -> Result<String> {
    if !should_attempt_pdf_ocr(&extracted_text, config) {
        return Ok(extracted_text);
    }

    let Some(ocr_config) = config.ocr.as_ref().filter(|ocr| ocr.enabled) else {
        return Ok(extracted_text);
    };
    let Some(provider) = ocr_provider else {
        tracing::debug!(
            "DefaultParser OCR enabled for {} but no OCR provider was configured",
            path.display()
        );
        return Ok(extracted_text);
    };

    match provider.ocr_pdf(path, ocr_config) {
        Ok(Some(ocr_text)) if !ocr_text.trim().is_empty() => {
            tracing::info!(
                "DefaultParser used OCR provider '{}' for {}",
                provider.name(),
                path.display()
            );
            Ok(ocr_text)
        }
        Ok(_) => Ok(extracted_text),
        Err(err) => {
            tracing::warn!(
                "DefaultParser OCR provider '{}' failed on {}: {}",
                provider.name(),
                path.display(),
                err
            );
            Ok(extracted_text)
        }
    }
}

fn should_attempt_pdf_ocr(text: &str, config: &crate::config::DefaultParserConfig) -> bool {
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

fn parse_html_document(path: &Path) -> Result<ParsedDocument> {
    let html = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read HTML file {}", path.display()))?;
    parse_markup_document(path, &html, true)
}

fn parse_xml_document(path: &Path) -> Result<ParsedDocument> {
    let xml = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read XML file {}", path.display()))?;
    parse_markup_document(path, &xml, false)
}

fn parse_rtf(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read RTF file {}", path.display()))?;
    Ok(strip_rtf(&raw))
}

fn parse_eml(path: &Path) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read EML file {}", path.display()))?;
    let mail = parse_email_part(&raw);

    let mut doc = ParsedDocument::new();
    doc.title = file_title(path);
    if !mail.headers.is_empty() {
        let mut header_lines = Vec::new();
        for key in ["Subject", "From", "To", "Cc", "Date"] {
            if let Some(value) = mail.headers.get(key) {
                header_lines.push(format!("{key}: {value}"));
            }
        }
        if !header_lines.is_empty() {
            doc.push(
                DocumentBlock::new(
                    DocumentBlockKind::EmailHeader,
                    Some("headers"),
                    header_lines.join("\n"),
                )
                .with_source("message")
                .with_ordinal(1),
            );
        }
    }

    let body = collect_best_mail_body(&mail);
    if !body.trim().is_empty() {
        doc.push(
            DocumentBlock::new(DocumentBlockKind::Paragraph, Some("body"), body)
                .with_source("message")
                .with_ordinal(2),
        );
    }

    ensure_document(doc, path)
}

fn parse_epub(path: &Path) -> Result<ParsedDocument> {
    let mut zip = open_zip(path)?;
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();
    let mut doc = ParsedDocument::new();
    doc.title = file_title(path);

    for name in names {
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm")) {
            continue;
        }

        let content = read_zip_entry(&mut zip, &name)?;
        let section_doc = parse_markup_string(&content, true).unwrap_or_else(|| {
            fallback_text_blocks(&render_html_to_text(&content).unwrap_or_default())
        });
        if section_doc.is_empty() {
            continue;
        }

        doc.push(
            DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some(name.clone()),
                format!("source: {}", name),
            )
            .with_source(name.clone()),
        );
        for (idx, block) in section_doc.into_iter().enumerate() {
            let label = block
                .label
                .as_ref()
                .map(|label| format!("{}: {}", name, label))
                .or_else(|| Some(name.clone()));
            doc.push(
                DocumentBlock::new(block.kind, label, block.content)
                    .with_source(name.clone())
                    .with_ordinal(idx + 1),
            );
        }
    }

    ensure_document(doc, path)
}

fn parse_docx(path: &Path) -> Result<ParsedDocument> {
    let mut zip = open_zip(path)?;
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();

    let mut doc = ParsedDocument::new();
    doc.title = file_title(path);
    for name in names {
        if !name.starts_with("word/") || !name.ends_with(".xml") {
            continue;
        }
        if !(name == "word/document.xml"
            || name.starts_with("word/header")
            || name.starts_with("word/footer")
            || name.starts_with("word/footnotes")
            || name.starts_with("word/endnotes"))
        {
            continue;
        }

        let content = read_zip_entry(&mut zip, &name)?;
        let blocks = extract_docx_blocks(&content)?;
        if !blocks.is_empty() {
            for (idx, block) in blocks.into_iter().enumerate() {
                let label = block
                    .label
                    .as_ref()
                    .map(|label| format!("{}: {}", name, label))
                    .or_else(|| Some(name.clone()));
                doc.push(
                    DocumentBlock::new(block.kind, label, block.content)
                        .with_source(name.clone())
                        .with_ordinal(idx + 1),
                );
            }
        }
    }

    ensure_document(doc, path)
}

fn parse_xlsx(path: &Path) -> Result<ParsedDocument> {
    let mut zip = open_zip(path)?;
    let shared_strings = read_shared_strings(&mut zip).unwrap_or_default();
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();

    let mut doc = ParsedDocument::new();
    doc.title = file_title(path);
    for name in names {
        if !name.starts_with("xl/worksheets/") || !name.ends_with(".xml") {
            continue;
        }
        let content = read_zip_entry(&mut zip, &name)?;
        let text = parse_xlsx_sheet(&content, &shared_strings)?;
        if !text.trim().is_empty() {
            doc.push(
                DocumentBlock::new(DocumentBlockKind::Table, Some(name.clone()), text)
                    .with_source(name.clone())
                    .with_ordinal(1),
            );
        }
    }

    ensure_document(doc, path)
}

fn parse_pptx(path: &Path) -> Result<ParsedDocument> {
    let mut zip = open_zip(path)?;
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();

    let mut doc = ParsedDocument::new();
    doc.title = file_title(path);
    for name in names {
        if !name.starts_with("ppt/slides/slide") || !name.ends_with(".xml") {
            continue;
        }
        let content = read_zip_entry(&mut zip, &name)?;
        let text = extract_xml_text(&content)?;
        let blocks = text_blocks(&text, DocumentBlockKind::Paragraph);
        if blocks.is_empty() {
            continue;
        }
        for (idx, block) in blocks.into_iter().enumerate() {
            let kind = if idx == 0 && looks_like_heading(&block.content) {
                DocumentBlockKind::Heading
            } else {
                DocumentBlockKind::Slide
            };
            let label = if idx == 0 {
                Some(name.clone())
            } else {
                Some(format!("{}: block {}", name, idx + 1))
            };
            doc.push(
                DocumentBlock::new(kind, label, block.content)
                    .with_source(name.clone())
                    .with_page(extract_slide_number(&name).unwrap_or(idx + 1))
                    .with_ordinal(idx + 1),
            );
        }
    }

    ensure_document(doc, path)
}

fn parse_odf(path: &Path) -> Result<ParsedDocument> {
    let mut zip = open_zip(path)?;
    let mut doc = ParsedDocument::new();
    doc.title = file_title(path);

    for name in ["meta.xml", "styles.xml", "content.xml"] {
        if let Ok(content) = read_zip_entry(&mut zip, name) {
            let blocks = if name == "content.xml" {
                parse_odf_content_blocks(&content)?
            } else {
                text_blocks(&extract_xml_text(&content)?, DocumentBlockKind::Metadata)
            };
            for (idx, block) in blocks.into_iter().enumerate() {
                let label = block
                    .label
                    .as_ref()
                    .map(|label| format!("{}: {}", name, label))
                    .or_else(|| {
                        if idx == 0 {
                            Some(name.to_string())
                        } else {
                            Some(format!("{}: block {}", name, idx + 1))
                        }
                    });
                doc.push(
                    DocumentBlock::new(block.kind, label, block.content)
                        .with_source(name)
                        .with_ordinal(idx + 1),
                );
            }
        }
    }

    ensure_document(doc, path)
}

fn read_shared_strings(zip: &mut ZipArchive<File>) -> Result<Vec<String>> {
    let content = read_zip_entry(zip, "xl/sharedStrings.xml")?;
    let doc = Document::parse(&content).context("failed to parse xlsx sharedStrings.xml")?;
    let mut values = Vec::new();

    for si in doc.descendants().filter(|n| n.tag_name().name() == "si") {
        let value = si
            .descendants()
            .filter(|n| n.tag_name().name() == "t")
            .filter_map(|n| n.text())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("");
        if !value.is_empty() {
            values.push(value);
        }
    }

    Ok(values)
}

fn parse_xlsx_sheet(xml: &str, shared_strings: &[String]) -> Result<String> {
    let doc = Document::parse(xml).context("failed to parse worksheet xml")?;
    let mut rows = Vec::new();

    for row in doc.descendants().filter(|n| n.tag_name().name() == "row") {
        let mut cells = Vec::new();
        for cell in row.children().filter(|n| n.tag_name().name() == "c") {
            let value = extract_xlsx_cell(cell, shared_strings);
            if !value.is_empty() {
                cells.push(value);
            }
        }
        if !cells.is_empty() {
            rows.push(cells.join("\t"));
        }
    }

    Ok(rows.join("\n"))
}

fn parse_markup_document(path: &Path, input: &str, is_html: bool) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = file_title(path);

    let blocks = parse_markup_string(input, is_html).unwrap_or_else(|| {
        let rendered = if is_html {
            render_html_to_text(input).unwrap_or_default()
        } else {
            extract_xml_text(input).unwrap_or_default()
        };
        fallback_text_blocks(&rendered)
    });

    if doc.title.is_none() {
        doc.title = extract_markup_title(input);
    }
    for block in blocks {
        doc.push(block);
    }

    ensure_document(doc, path)
}

fn parse_markup_string(input: &str, is_html: bool) -> Option<Vec<DocumentBlock>> {
    let doc = Document::parse(input).ok()?;
    let mut blocks = Vec::new();

    for node in doc.descendants().filter(|node| node.is_element()) {
        let tag = node.tag_name().name();
        let kind = match tag {
            "title" => continue,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => DocumentBlockKind::Heading,
            "p" | "li" | "blockquote" => DocumentBlockKind::Paragraph,
            "pre" | "code" => DocumentBlockKind::Code,
            "table" => DocumentBlockKind::Table,
            "meta" if is_html => DocumentBlockKind::Metadata,
            "section" | "article" => DocumentBlockKind::Section,
            _ => continue,
        };

        let content = collect_node_text(node);
        if content.trim().is_empty() {
            continue;
        }

        let label = match tag {
            "meta" => node
                .attribute("name")
                .or_else(|| node.attribute("property"))
                .or_else(|| node.attribute("http-equiv"))
                .map(str::to_string),
            _ => None,
        };
        blocks.push(DocumentBlock::new(kind, label, content));
    }

    if blocks.is_empty() {
        None
    } else {
        Some(dedupe_adjacent_blocks(blocks))
    }
}

fn extract_markup_title(input: &str) -> Option<String> {
    let doc = Document::parse(input).ok()?;
    doc.descendants()
        .find(|node| node.has_tag_name("title"))
        .map(collect_node_text)
        .filter(|title| !title.trim().is_empty())
}

fn render_html_to_text(input: &str) -> Result<String> {
    html2text::from_read(input.as_bytes(), 80).context("failed to render HTML as text")
}

fn collect_node_text(node: roxmltree::Node<'_, '_>) -> String {
    let text = node
        .descendants()
        .filter_map(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    normalize_text(&text)
}

fn dedupe_adjacent_blocks(blocks: Vec<DocumentBlock>) -> Vec<DocumentBlock> {
    let mut deduped = Vec::new();
    for block in blocks {
        let is_duplicate = deduped.last().is_some_and(|last: &DocumentBlock| {
            last.kind == block.kind && last.label == block.label && last.content == block.content
        });
        if !is_duplicate {
            deduped.push(block);
        }
    }
    deduped
}

fn extract_docx_blocks(xml: &str) -> Result<Vec<DocumentBlock>> {
    let doc = Document::parse(xml).context("failed to parse docx xml")?;
    let mut blocks = Vec::new();

    for para in doc
        .descendants()
        .filter(|node| node.tag_name().name() == "p")
    {
        let content = para
            .descendants()
            .filter(|node| node.tag_name().name() == "t")
            .filter_map(|node| node.text())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("");
        let content = normalize_text(&content);
        if content.is_empty() {
            continue;
        }

        let kind = if paragraph_style(para)
            .map(|style| style.to_ascii_lowercase().contains("heading"))
            .unwrap_or(false)
            || looks_like_heading(&content)
        {
            DocumentBlockKind::Heading
        } else {
            DocumentBlockKind::Paragraph
        };

        blocks.push(DocumentBlock::new(kind, None::<String>, content));
    }

    if blocks.is_empty() {
        let fallback = extract_xml_text(xml)?;
        Ok(fallback_text_blocks(&fallback))
    } else {
        Ok(blocks)
    }
}

fn paragraph_style(node: roxmltree::Node<'_, '_>) -> Option<String> {
    node.descendants()
        .find(|child| child.tag_name().name() == "pStyle")
        .and_then(|child| child.attribute("val").or_else(|| child.attribute("w:val")))
        .map(str::to_string)
}

fn parse_odf_content_blocks(xml: &str) -> Result<Vec<DocumentBlock>> {
    let doc = Document::parse(xml).context("failed to parse odf content xml")?;
    let mut blocks = Vec::new();

    for node in doc.descendants().filter(|node| node.is_element()) {
        let tag = node.tag_name().name();
        let kind = match tag {
            "h" => DocumentBlockKind::Heading,
            "p" => DocumentBlockKind::Paragraph,
            "list-item" => DocumentBlockKind::Paragraph,
            _ => continue,
        };
        let content = collect_node_text(node);
        if content.is_empty() {
            continue;
        }
        blocks.push(DocumentBlock::new(kind, None::<String>, content));
    }

    if blocks.is_empty() {
        let fallback = extract_xml_text(xml)?;
        Ok(fallback_text_blocks(&fallback))
    } else {
        Ok(blocks)
    }
}

fn parsed_text_document(
    path: &Path,
    text: String,
    default_kind: DocumentBlockKind,
) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = file_title(path);
    let source = doc
        .title
        .clone()
        .unwrap_or_else(|| path.display().to_string());
    for (idx, block) in text_blocks(&text, default_kind).into_iter().enumerate() {
        doc.push(block.with_source(source.clone()).with_ordinal(idx + 1));
    }
    ensure_document(doc, path)
}

fn fallback_text_blocks(text: &str) -> Vec<DocumentBlock> {
    text_blocks(text, DocumentBlockKind::Paragraph)
}

fn text_blocks(text: &str, default_kind: DocumentBlockKind) -> Vec<DocumentBlock> {
    let normalized = normalize_text(text);
    normalized
        .split("\n\n")
        .filter_map(|chunk| {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                return None;
            }

            let kind = if looks_like_heading(chunk) {
                DocumentBlockKind::Heading
            } else {
                default_kind.clone()
            };
            Some(DocumentBlock::new(kind, None::<String>, chunk))
        })
        .collect()
}

fn looks_like_heading(text: &str) -> bool {
    let line = text.lines().next().unwrap_or("").trim();
    if line.is_empty() || text.lines().count() > 2 {
        return false;
    }
    if line.starts_with('#') {
        return true;
    }
    let char_count = line.chars().count();
    let ends_like_sentence = matches!(line.chars().last(), Some('.' | '!' | '?' | ':' | ';'));
    char_count <= 80 && !ends_like_sentence
}

fn extract_slide_number(name: &str) -> Option<usize> {
    let digits = name
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn extract_xlsx_cell(cell: roxmltree::Node<'_, '_>, shared_strings: &[String]) -> String {
    let cell_type = cell.attribute("t").unwrap_or_default();

    if cell_type == "inlineStr" {
        return cell
            .descendants()
            .filter(|n| n.tag_name().name() == "t")
            .filter_map(|n| n.text())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("");
    }

    let raw = cell
        .children()
        .find(|n| n.tag_name().name() == "v")
        .and_then(|n| n.text())
        .map(str::trim)
        .unwrap_or_default();

    if raw.is_empty() {
        return String::new();
    }

    if cell_type == "s" {
        return raw
            .parse::<usize>()
            .ok()
            .and_then(|idx| shared_strings.get(idx))
            .cloned()
            .unwrap_or_else(|| raw.to_string());
    }

    raw.to_string()
}

fn open_zip(path: &Path) -> Result<ZipArchive<File>> {
    let file = File::open(path)
        .with_context(|| format!("failed to open zip container {}", path.display()))?;
    ZipArchive::new(file)
        .with_context(|| format!("failed to read zip container {}", path.display()))
}

fn read_zip_entry(zip: &mut ZipArchive<File>, name: &str) -> Result<String> {
    let mut file = zip
        .by_name(name)
        .with_context(|| format!("zip entry not found: {name}"))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .with_context(|| format!("failed to read zip entry: {name}"))?;
    Ok(buf)
}

fn extract_xml_text(xml: &str) -> Result<String> {
    let doc = Document::parse(xml).context("failed to parse XML")?;
    let mut out = String::new();
    let mut last_was_space = true;

    for node in doc.descendants() {
        if let Some(text) = node.text() {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !last_was_space && !needs_newline(node.tag_name().name()) {
                out.push(' ');
            }
            out.push_str(trimmed);
            if needs_newline(node.tag_name().name()) {
                out.push('\n');
                last_was_space = true;
            } else {
                last_was_space = false;
            }
        }
    }

    Ok(normalize_text(&out))
}

fn needs_newline(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "br"
            | "section"
            | "li"
            | "tr"
            | "row"
            | "sheetData"
            | "worksheet"
            | "text-box"
    )
}

fn normalize_text(text: &str) -> String {
    let mut out = String::new();
    let mut blank_lines = 0usize;

    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank_lines += 1;
            if blank_lines <= 1 && !out.ends_with("\n\n") {
                out.push('\n');
            }
            continue;
        }

        blank_lines = 0;
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
    }

    out.trim().to_string()
}

fn file_title(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
}

fn ensure_document(doc: ParsedDocument, path: &Path) -> Result<ParsedDocument> {
    if doc.is_empty() {
        anyhow::bail!("no extractable text found in {}", path.display());
    }
    Ok(doc)
}

#[derive(Debug, Default, Clone)]
struct EmailPart {
    headers: std::collections::HashMap<String, String>,
    content_type: String,
    body: String,
    parts: Vec<EmailPart>,
}

fn parse_email_part(raw: &str) -> EmailPart {
    let (header_block, body_block) = split_headers_body(raw);
    let headers = parse_headers(header_block);
    let content_type = headers
        .get("Content-Type")
        .cloned()
        .unwrap_or_else(|| "text/plain; charset=utf-8".to_string());
    let encoding = headers
        .get("Content-Transfer-Encoding")
        .cloned()
        .unwrap_or_default();

    if let Some(boundary) = extract_content_type_param(&content_type, "boundary") {
        let parts = split_multipart_body(body_block, &boundary)
            .into_iter()
            .map(|part| parse_email_part(&part))
            .collect::<Vec<_>>();
        EmailPart {
            headers,
            content_type,
            body: String::new(),
            parts,
        }
    } else {
        let decoded = decode_email_body(body_block, &encoding);
        EmailPart {
            headers,
            content_type,
            body: decoded,
            parts: Vec::new(),
        }
    }
}

fn split_headers_body(raw: &str) -> (&str, &str) {
    if let Some(idx) = raw.find("\r\n\r\n") {
        (&raw[..idx], &raw[idx + 4..])
    } else if let Some(idx) = raw.find("\n\n") {
        (&raw[..idx], &raw[idx + 2..])
    } else {
        ("", raw)
    }
}

fn parse_headers(raw: &str) -> std::collections::HashMap<String, String> {
    let mut headers = std::collections::HashMap::new();
    let mut current_key: Option<String> = None;
    let mut current_val = String::new();

    for line in raw.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if !current_val.is_empty() {
                current_val.push(' ');
            }
            current_val.push_str(line.trim());
            continue;
        }

        if let Some(key) = current_key.take() {
            headers.insert(key, current_val.trim().to_string());
            current_val.clear();
        }

        if let Some((key, value)) = line.split_once(':') {
            current_key = Some(key.trim().to_string());
            current_val.push_str(value.trim());
        }
    }

    if let Some(key) = current_key {
        headers.insert(key, current_val.trim().to_string());
    }

    headers
}

fn extract_content_type_param(content_type: &str, name: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let (key, value) = part.split_once('=')?;
        if key.trim().eq_ignore_ascii_case(name) {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn split_multipart_body(body: &str, boundary: &str) -> Vec<String> {
    let marker = format!("--{boundary}");
    let end_marker = format!("--{boundary}--");
    let normalized = body.replace("\r\n", "\n");
    let mut parts = Vec::new();
    let mut current = Vec::new();
    let mut in_part = false;

    for line in normalized.lines() {
        if line == marker {
            if in_part && !current.is_empty() {
                parts.push(current.join("\n"));
                current.clear();
            }
            in_part = true;
            continue;
        }
        if line == end_marker {
            if in_part && !current.is_empty() {
                parts.push(current.join("\n"));
            }
            break;
        }
        if in_part {
            current.push(line.to_string());
        }
    }

    parts
}

fn decode_email_body(body: &str, encoding: &str) -> String {
    let normalized = body.replace("\r\n", "\n");
    let decoded = if encoding.eq_ignore_ascii_case("base64") {
        decode_base64_text(&normalized).unwrap_or(normalized)
    } else if encoding.eq_ignore_ascii_case("quoted-printable") {
        decode_quoted_printable(&normalized)
    } else {
        normalized
    };

    decoded.trim().to_string()
}

fn decode_base64_text(input: &str) -> Option<String> {
    let compact = input.lines().map(str::trim).collect::<String>();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .ok()?;
    String::from_utf8(bytes).ok()
}

fn decode_quoted_printable(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'=' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 2;
                    continue;
                }
                if i + 2 < bytes.len() && bytes[i + 1] == b'\r' && bytes[i + 2] == b'\n' {
                    i += 3;
                    continue;
                }
                if i + 2 < bytes.len() {
                    let hex = &input[i + 1..i + 3];
                    if let Ok(byte) = u8::from_str_radix(hex, 16) {
                        out.push(byte);
                        i += 3;
                        continue;
                    }
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'_' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn collect_best_mail_body(part: &EmailPart) -> String {
    if !part.parts.is_empty() {
        let preferred_plain = part
            .parts
            .iter()
            .map(collect_best_mail_body)
            .find(|body| !body.trim().is_empty());
        if let Some(body) = preferred_plain {
            return body;
        }
    }

    if part
        .content_type
        .to_ascii_lowercase()
        .starts_with("text/html")
    {
        return html2text::from_read(part.body.as_bytes(), 80)
            .unwrap_or_else(|_| part.body.clone())
            .trim()
            .to_string();
    }

    if part.content_type.is_empty() || part.content_type.to_ascii_lowercase().starts_with("text/") {
        return part.body.trim().to_string();
    }

    String::new()
}

fn strip_rtf(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '{' | '}' => {}
            '\\' => match chars.peek().copied() {
                Some('\\') | Some('{') | Some('}') => {
                    out.push(chars.next().unwrap_or_default());
                }
                Some('\'') => {
                    chars.next();
                    let hi = chars.next();
                    let lo = chars.next();
                    if let (Some(hi), Some(lo)) = (hi, lo) {
                        let hex = format!("{hi}{lo}");
                        if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                            out.push(byte as char);
                        }
                    }
                }
                Some(_) => {
                    let mut word = String::new();
                    while let Some(c) = chars.peek().copied() {
                        if c.is_ascii_alphabetic() {
                            word.push(c);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    while let Some(c) = chars.peek().copied() {
                        if c.is_ascii_digit() || c == '-' {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if chars.peek() == Some(&' ') {
                        chars.next();
                    }
                    if matches!(word.as_str(), "par" | "line") {
                        out.push('\n');
                    }
                }
                None => break,
            },
            '\r' => {}
            '\n' => out.push('\n'),
            _ => out.push(ch),
        }
    }

    normalize_text(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::FileOptions;

    struct MockOcrProvider {
        text: Option<String>,
    }

    impl DefaultParserOcrProvider for MockOcrProvider {
        fn name(&self) -> &str {
            "mock-ocr"
        }

        fn ocr_pdf(
            &self,
            _path: &Path,
            _config: &crate::config::DefaultParserOcrConfig,
        ) -> Result<Option<String>> {
            Ok(self.text.clone())
        }
    }

    fn write_file(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn write_zip(dir: &TempDir, name: &str, entries: &[(&str, &str)]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let file = File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::default();

        for (entry, content) in entries {
            zip.start_file(*entry, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }

        zip.finish().unwrap();
        path
    }

    #[test]
    fn parses_html() {
        let dir = TempDir::new().unwrap();
        let path = write_file(
            &dir,
            "sample.html",
            "<html><body><h1>Hello</h1><p>World</p></body></html>",
        );
        let doc = parse_html_document(&path).unwrap();
        assert!(doc
            .blocks
            .iter()
            .any(|block| block.kind == DocumentBlockKind::Heading));
        assert!(doc.to_text().contains("Hello"));
        assert!(doc.to_text().contains("World"));
    }

    #[test]
    fn parses_docx_like_zip() {
        let dir = TempDir::new().unwrap();
        let path = write_zip(
            &dir,
            "sample.docx",
            &[(
                "word/document.xml",
                r#"<w:document xmlns:w="urn:test"><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p><w:p><w:r><w:t>World</w:t></w:r></w:p></w:body></w:document>"#,
            )],
        );
        let doc = parse_docx(&path).unwrap();
        assert!(doc
            .blocks
            .iter()
            .any(|block| block.kind == DocumentBlockKind::Heading));
        assert!(doc.to_text().contains("Hello"));
        assert!(doc.to_text().contains("World"));
    }

    #[test]
    fn parses_xlsx_shared_strings_and_inline_cells() {
        let dir = TempDir::new().unwrap();
        let path = write_zip(
            &dir,
            "sample.xlsx",
            &[
                (
                    "xl/sharedStrings.xml",
                    r#"<sst xmlns="urn:test"><si><t>Name</t></si><si><t>Alice</t></si></sst>"#,
                ),
                (
                    "xl/worksheets/sheet1.xml",
                    r#"<worksheet xmlns="urn:test"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>Score</t></is></c></row><row r="2"><c r="A2" t="s"><v>1</v></c><c r="B2"><v>42</v></c></row></sheetData></worksheet>"#,
                ),
            ],
        );
        let text = parse_xlsx(&path).unwrap().to_text();
        assert!(text.contains("Name"));
        assert!(text.contains("Score"));
        assert!(text.contains("Alice"));
        assert!(text.contains("42"));
    }

    #[test]
    fn parses_pptx_slides() {
        let dir = TempDir::new().unwrap();
        let path = write_zip(
            &dir,
            "slides.pptx",
            &[(
                "ppt/slides/slide1.xml",
                r#"<p:sld xmlns:p="urn:test" xmlns:a="urn:test-a"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Quarterly Review</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
            )],
        );
        let doc = parse_pptx(&path).unwrap();
        assert!(doc
            .blocks
            .iter()
            .any(|block| block.kind == DocumentBlockKind::Heading));
        assert!(doc
            .blocks
            .iter()
            .any(|block| block.location.as_ref().and_then(|loc| loc.page) == Some(1)));
        assert!(doc.to_text().contains("Quarterly Review"));
    }

    #[test]
    fn parses_odf_content() {
        let dir = TempDir::new().unwrap();
        let path = write_zip(
            &dir,
            "document.odt",
            &[(
                "content.xml",
                r#"<office:document-content xmlns:office="urn:test" xmlns:text="urn:test-text"><office:body><office:text><text:p>Hello ODF</text:p><text:p>Second line</text:p></office:text></office:body></office:document-content>"#,
            )],
        );
        let doc = parse_odf(&path).unwrap();
        assert!(doc
            .blocks
            .iter()
            .any(|block| block.kind == DocumentBlockKind::Paragraph));
        assert!(doc.to_text().contains("Hello ODF"));
        assert!(doc.to_text().contains("Second line"));
    }

    #[test]
    fn parses_epub_html_entries() {
        let dir = TempDir::new().unwrap();
        let path = write_zip(
            &dir,
            "book.epub",
            &[(
                "OPS/ch1.xhtml",
                "<html><body><p>Chapter One</p></body></html>",
            )],
        );
        let doc = parse_epub(&path).unwrap();
        assert!(doc
            .blocks
            .iter()
            .any(|block| block.kind == DocumentBlockKind::Paragraph));
        assert!(doc.to_text().contains("Chapter One"));
    }

    #[test]
    fn parses_plain_eml() {
        let dir = TempDir::new().unwrap();
        let path = write_file(
            &dir,
            "mail.eml",
            "Subject: Hello\nFrom: alice@example.com\nTo: bob@example.com\nContent-Type: text/plain; charset=utf-8\n\nThis is a plain email body.\n",
        );
        let text = parse_eml(&path).unwrap().to_text();
        assert!(text.contains("Subject: Hello"));
        assert!(text.contains("alice@example.com"));
        assert!(text.contains("This is a plain email body."));
    }

    #[test]
    fn parsed_text_document_sets_block_locations() {
        let dir = TempDir::new().unwrap();
        let path = write_file(&dir, "notes.rtf", "{\\rtf1\\ansi Hello \\par World}");
        let doc = parsed_text_document(
            &path,
            parse_rtf(&path).unwrap(),
            DocumentBlockKind::Paragraph,
        )
        .unwrap();
        assert!(doc.blocks.iter().enumerate().all(|(idx, block)| {
            block.location.as_ref().and_then(|loc| loc.ordinal) == Some(idx + 1)
        }));
    }

    #[test]
    fn parses_xml_document_into_structured_blocks() {
        let dir = TempDir::new().unwrap();
        let path = write_file(
            &dir,
            "sample.xml",
            "<root><title>Spec</title><section><p>Intro text</p><p>More text</p></section></root>",
        );
        let doc = parse_xml_document(&path).unwrap();
        assert!(doc.title.as_deref() == Some("sample.xml") || doc.title.as_deref() == Some("Spec"));
        assert!(doc
            .blocks
            .iter()
            .any(|block| block.kind == DocumentBlockKind::Paragraph));
        assert!(doc.to_text().contains("Intro text"));
    }

    #[test]
    fn parses_multipart_eml_with_html_and_quoted_printable() {
        let dir = TempDir::new().unwrap();
        let path = write_file(
            &dir,
            "multipart.eml",
            concat!(
                "Subject: Multipart Test\n",
                "From: sender@example.com\n",
                "To: receiver@example.com\n",
                "Content-Type: multipart/alternative; boundary=\"abc123\"\n",
                "\n",
                "--abc123\n",
                "Content-Type: text/plain; charset=utf-8\n",
                "Content-Transfer-Encoding: quoted-printable\n",
                "\n",
                "Hello=20World=21\n",
                "--abc123\n",
                "Content-Type: text/html; charset=utf-8\n",
                "\n",
                "<html><body><p>Ignored HTML fallback</p></body></html>\n",
                "--abc123--\n"
            ),
        );
        let text = parse_eml(&path).unwrap().to_text();
        assert!(text.contains("Subject: Multipart Test"));
        assert!(text.contains("Hello World!"));
    }

    #[test]
    fn strips_rtf_control_words() {
        let text = strip_rtf(r"{\rtf1\ansi Hello \par World}");
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
    }

    #[test]
    fn pdf_ocr_heuristic_detects_weak_text() {
        let config = crate::config::DefaultParserConfig {
            enabled: true,
            max_file_size_mb: 50,
            ocr: Some(crate::config::DefaultParserOcrConfig {
                enabled: true,
                ..Default::default()
            }),
        };

        assert!(should_attempt_pdf_ocr("", &config));
        assert!(should_attempt_pdf_ocr("%%% ---", &config));
        assert!(!should_attempt_pdf_ocr(
            "This is a reasonably healthy PDF text extraction with enough words and letters to avoid OCR fallback across multiple paragraphs and sections of the document body.",
            &config
        ));
    }

    #[test]
    fn pdf_ocr_fallback_uses_provider_when_text_is_weak() {
        let dir = TempDir::new().unwrap();
        let path = write_file(&dir, "sample.pdf", "not-a-real-pdf");
        let config = crate::config::DefaultParserConfig {
            enabled: true,
            max_file_size_mb: 50,
            ocr: Some(crate::config::DefaultParserOcrConfig {
                enabled: true,
                ..Default::default()
            }),
        };
        let provider = MockOcrProvider {
            text: Some("OCR recovered text".to_string()),
        };

        let text = maybe_run_pdf_ocr(&path, String::new(), &config, Some(&provider)).unwrap();
        assert_eq!(text, "OCR recovered text");
    }

    #[test]
    fn pdf_ocr_fallback_preserves_extracted_text_without_provider() {
        let dir = TempDir::new().unwrap();
        let path = write_file(&dir, "sample.pdf", "not-a-real-pdf");
        let config = crate::config::DefaultParserConfig {
            enabled: true,
            max_file_size_mb: 50,
            ocr: Some(crate::config::DefaultParserOcrConfig {
                enabled: true,
                ..Default::default()
            }),
        };

        let text = maybe_run_pdf_ocr(&path, "weak".to_string(), &config, None).unwrap();
        assert_eq!(text, "weak");
    }

    #[test]
    fn default_parser_can_hold_ocr_provider() {
        let parser = DefaultParser::with_config_and_ocr(
            crate::config::DefaultParserConfig::default(),
            Arc::new(MockOcrProvider { text: None }),
        );
        assert!(parser.ocr_provider().is_some());
    }
}
