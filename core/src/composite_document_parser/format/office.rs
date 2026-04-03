use anyhow::{Context, Result};
use roxmltree::Document;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use zip::ZipArchive;

use crate::composite_document_parser::{
    attribute_by_local_name, DocumentOcrFormat, DocumentOcrProvider,
};
use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

pub(super) fn parse_legacy_doc(path: &Path) -> Result<ParsedDocument> {
    parse_legacy_compound_document(path, LegacyOfficeKind::Word)
}

pub(super) fn parse_legacy_xls(path: &Path) -> Result<ParsedDocument> {
    parse_legacy_compound_document(path, LegacyOfficeKind::Excel)
}

pub(super) fn parse_legacy_ppt(path: &Path) -> Result<ParsedDocument> {
    parse_legacy_compound_document(path, LegacyOfficeKind::PowerPoint)
}

pub(super) fn parse_hwp(path: &Path) -> Result<ParsedDocument> {
    let mut compound = cfb::open(path)
        .with_context(|| format!("failed to open HWP document {}", path.display()))?;

    let stream_paths = compound
        .walk()
        .filter(|entry| entry.is_stream())
        .map(|entry| entry.path().to_path_buf())
        .collect::<Vec<_>>();

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    doc.push(
        DocumentBlock::new(DocumentBlockKind::Metadata, Some("hwp"), "format=hwp")
            .with_source("hwp")
            .with_ordinal(0),
    );

    let mut ordinal = 1usize;
    let mut seen = BTreeSet::new();
    for stream_path in prioritize_hwp_streams(stream_paths) {
        let Some(stream_name) = stream_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let mut stream = compound.open_stream(&stream_path)?;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes)?;

        let values = extract_hwp_stream_strings(&bytes);
        if values.is_empty() {
            continue;
        }

        for value in values {
            let normalized = normalize_legacy_text(&value, LegacyOfficeKind::Word);
            if normalized.trim().is_empty() || !seen.insert(normalized.clone()) {
                continue;
            }

            let kind = if ordinal == 1 && super::looks_like_heading(&normalized) {
                DocumentBlockKind::Heading
            } else {
                DocumentBlockKind::Paragraph
            };
            if doc.title.is_none() && kind == DocumentBlockKind::Heading {
                doc.title = Some(normalized.clone());
            }
            doc.push(
                DocumentBlock::new(kind, Some(format!("{stream_name}: text")), normalized)
                    .with_source(stream_name)
                    .with_ordinal(ordinal),
            );
            ordinal += 1;
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_archive_office_entry(name: &str, bytes: &[u8]) -> Option<ParsedDocument> {
    let path = Path::new(name);
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())?;

    write_archive_entry_tempfile(name, bytes)
        .ok()
        .and_then(|temp_path| {
            let result = match ext.as_str() {
                "doc" | "dot" => parse_legacy_doc(&temp_path),
                "docx" | "docm" | "dotx" | "dotm" => parse_docx(
                    &temp_path,
                    &crate::config::DocumentParserConfig::default(),
                    None,
                ),
                "xls" | "xlt" => parse_legacy_xls(&temp_path),
                "xlsb" => parse_xlsb(
                    &temp_path,
                    &crate::config::DocumentParserConfig::default(),
                    None,
                ),
                "xlsx" | "xlsm" | "xltx" | "xltm" | "xlam" => parse_xlsx(
                    &temp_path,
                    &crate::config::DocumentParserConfig::default(),
                    None,
                ),
                "ppt" | "pps" => parse_legacy_ppt(&temp_path),
                "pptx" | "pptm" | "ppsx" | "potx" | "potm" => parse_pptx(
                    &temp_path,
                    &crate::config::DocumentParserConfig::default(),
                    None,
                ),
                "pdf" => super::parse_pdf_document(
                    &temp_path,
                    &crate::config::DocumentParserConfig::default(),
                    None,
                ),
                "hwp" => parse_hwp(&temp_path),
                "hwpx" => parse_hwpx(&temp_path),
                "odt" | "ods" | "odp" => parse_odf(
                    &temp_path,
                    &crate::config::DocumentParserConfig::default(),
                    None,
                ),
                _ => return None,
            };

            let _ = fs::remove_file(&temp_path);
            result.ok()
        })
}

pub(super) fn parse_docx(
    path: &Path,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<ParsedDocument> {
    let mut zip = super::open_zip(path)?;
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    if let Some(metadata) = super::read_docx_core_metadata(&mut zip).ok().flatten() {
        if let Some(title) = metadata.title.clone() {
            doc.title = Some(title);
        }
        doc.push(
            DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some("core-properties"),
                metadata.as_block_content(),
            )
            .with_source("docProps/core.xml")
            .with_ordinal(0),
        );
    }
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

        let content = super::read_zip_entry(&mut zip, &name)?;
        let blocks = extract_docx_blocks(&content)?;
        if !blocks.is_empty() {
            for (idx, block) in blocks.into_iter().enumerate() {
                let label = block
                    .label
                    .as_ref()
                    .map(|label| format!("{}: {}", docx_part_label(&name), label))
                    .or_else(|| Some(docx_part_label(&name)));
                doc.push(
                    DocumentBlock::new(block.kind, label, block.content)
                        .with_source(name.clone())
                        .with_ordinal(idx + 1),
                );
            }
        }
    }

    if doc.is_empty() {
        if let Some(ocr_doc) = super::maybe_run_document_ocr_fallback(
            path,
            DocumentOcrFormat::Docx,
            config,
            ocr_provider,
        )? {
            return Ok(ocr_doc);
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_xlsx(
    path: &Path,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<ParsedDocument> {
    let mut zip = super::open_zip(path)?;
    let shared_strings = read_shared_strings(&mut zip).unwrap_or_default();
    let worksheet_names = read_xlsx_sheet_names(&mut zip).unwrap_or_default();
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    for name in names {
        if !name.starts_with("xl/worksheets/") || !name.ends_with(".xml") {
            continue;
        }
        let content = super::read_zip_entry(&mut zip, &name)?;
        let sheet = parse_xlsx_sheet(&content, &shared_strings)?;
        if !sheet.text.trim().is_empty() {
            let label = worksheet_names
                .get(&name)
                .cloned()
                .unwrap_or_else(|| name.clone());
            doc.push(
                DocumentBlock::new(
                    DocumentBlockKind::Metadata,
                    Some(format!("{label}: worksheet")),
                    format!("rows={}\ncolumns={}", sheet.row_count, sheet.column_count),
                )
                .with_attribute("row_count", sheet.row_count.to_string())
                .with_attribute("column_count", sheet.column_count.to_string())
                .with_source(name.clone())
                .with_ordinal(0),
            );
            let mut block = DocumentBlock::new(DocumentBlockKind::Table, Some(label), sheet.text)
                .with_attribute("row_count", sheet.row_count.to_string())
                .with_attribute("column_count", sheet.column_count.to_string())
                .with_source(name.clone())
                .with_ordinal(1);
            if let Some(payload) = super::table_structured_payload(&sheet.rows) {
                block = block.with_structured_payload(payload);
            }
            doc.push(block);
        }
    }

    if doc.is_empty() {
        if let Some(ocr_doc) = super::maybe_run_document_ocr_fallback(
            path,
            DocumentOcrFormat::Xlsx,
            config,
            ocr_provider,
        )? {
            return Ok(ocr_doc);
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_xlsb(
    path: &Path,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<ParsedDocument> {
    let mut zip = super::open_zip(path)?;
    let worksheet_names = read_xlsb_sheet_names(&mut zip).unwrap_or_default();
    let shared_strings = read_xlsb_shared_strings(&mut zip).unwrap_or_default();
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    doc.push(
        DocumentBlock::new(
            DocumentBlockKind::Metadata,
            Some("xlsb"),
            "mode=heuristic-string-recovery\nformat=xlsb",
        )
        .with_source("xl/workbook.bin")
        .with_ordinal(0),
    );

    let mut ordinal = 1usize;
    if !shared_strings.is_empty() {
        let row_count = shared_strings.len();
        let rows = shared_strings
            .iter()
            .cloned()
            .map(|value| vec![value])
            .collect::<Vec<_>>();
        let mut block = DocumentBlock::new(
            DocumentBlockKind::Table,
            Some("shared strings"),
            super::table_text_from_cells(&rows),
        )
        .with_attribute("row_count", row_count.to_string())
        .with_attribute("column_count", "1")
        .with_attribute("extraction", "shared-strings-recovery")
        .with_source("xl/sharedStrings.bin")
        .with_ordinal(ordinal);
        if let Some(payload) = super::table_structured_payload(&rows) {
            block = block.with_structured_payload(payload);
        }
        doc.push(
            DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some("shared strings: worksheet"),
                format!("rows={row_count}\ncolumns=1\nextraction=shared-strings-recovery"),
            )
            .with_attribute("row_count", row_count.to_string())
            .with_attribute("column_count", "1")
            .with_attribute("extraction", "shared-strings-recovery")
            .with_source("xl/sharedStrings.bin")
            .with_ordinal(ordinal),
        );
        ordinal += 1;
        doc.push(block);
        ordinal += 1;
    }

    for name in names {
        if !(name.starts_with("xl/worksheets/") && name.ends_with(".bin")) {
            continue;
        }

        let bytes = super::read_zip_entry_bytes(&mut zip, &name)?;
        let segments = extract_xlsb_segments(&bytes);
        if segments.is_empty() {
            continue;
        }

        let label = worksheet_names
            .get(&name)
            .cloned()
            .unwrap_or_else(|| xlsb_sheet_label(&name));
        let table_segment_count = segments
            .iter()
            .filter(|segment| matches!(segment, XlsbSheetSegment::Table(_, _)))
            .count();
        let total_rows = segments
            .iter()
            .filter_map(|segment| match segment {
                XlsbSheetSegment::Table(rows, _) => Some(rows.len()),
                XlsbSheetSegment::Text(_) => None,
            })
            .sum::<usize>();
        let max_columns = segments
            .iter()
            .filter_map(|segment| match segment {
                XlsbSheetSegment::Table(rows, _) => {
                    Some(rows.iter().map(Vec::len).max().unwrap_or(0))
                }
                XlsbSheetSegment::Text(_) => None,
            })
            .max()
            .unwrap_or(0);
        let text_block_count = segments
            .iter()
            .filter(|segment| matches!(segment, XlsbSheetSegment::Text(_)))
            .count();

        doc.push(
            DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some(format!("{label}: worksheet")),
                format!(
                    "rows={total_rows}\ncolumns={max_columns}\nsegments={}\ntext_blocks={text_block_count}\nextraction=heuristic-string-recovery",
                    segments.len()
                ),
            )
            .with_attribute("row_count", total_rows.to_string())
            .with_attribute("column_count", max_columns.to_string())
            .with_attribute("segment_count", segments.len().to_string())
            .with_attribute("text_block_count", text_block_count.to_string())
            .with_attribute("extraction", "heuristic-string-recovery")
            .with_source(name.clone())
            .with_ordinal(ordinal),
        );
        ordinal += 1;

        let mut table_index = 1usize;
        for segment in segments {
            match segment {
                XlsbSheetSegment::Table(rows, extraction) => {
                    let row_count = rows.len();
                    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
                    let label = if table_segment_count <= 1 {
                        label.clone()
                    } else {
                        format!("{label}: table {table_index}")
                    };
                    let mut block = DocumentBlock::new(
                        DocumentBlockKind::Table,
                        Some(label),
                        super::table_text_from_cells(&rows),
                    )
                    .with_attribute("row_count", row_count.to_string())
                    .with_attribute("column_count", column_count.to_string())
                    .with_attribute("extraction", extraction)
                    .with_source(name.clone())
                    .with_ordinal(ordinal);
                    if let Some(payload) = super::table_structured_payload(&rows) {
                        block = block.with_structured_payload(payload);
                    }
                    doc.push(block);
                    ordinal += 1;
                    table_index += 1;
                }
                XlsbSheetSegment::Text(text) => {
                    for block in super::text_blocks(&text, DocumentBlockKind::Paragraph) {
                        doc.push(
                            DocumentBlock::new(block.kind, block.label, block.content)
                                .with_attribute("extraction", "text-segmentation")
                                .with_source(name.clone())
                                .with_ordinal(ordinal),
                        );
                        ordinal += 1;
                    }
                }
            }
        }
    }

    if doc.non_empty_block_count() <= 1 {
        if let Some(ocr_doc) = super::maybe_run_document_ocr_fallback(
            path,
            DocumentOcrFormat::Xlsx,
            config,
            ocr_provider,
        )? {
            return Ok(ocr_doc);
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_pptx(
    path: &Path,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<ParsedDocument> {
    let mut zip = super::open_zip(path)?;
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    for name in names {
        if !name.starts_with("ppt/slides/slide") || !name.ends_with(".xml") {
            continue;
        }
        let content = super::read_zip_entry(&mut zip, &name)?;
        let text = super::extract_xml_text(&content)?;
        let blocks = super::text_blocks(&text, DocumentBlockKind::Paragraph);
        if blocks.is_empty() {
            continue;
        }
        for (idx, block) in blocks.into_iter().enumerate() {
            let kind = if idx == 0 && super::looks_like_heading(&block.content) {
                DocumentBlockKind::Heading
            } else {
                DocumentBlockKind::Slide
            };
            let slide_number = extract_slide_number(&name).unwrap_or(idx + 1);
            let label = if idx == 0 {
                Some(format!("slide {slide_number}"))
            } else {
                Some(format!("slide {slide_number}: block {}", idx + 1))
            };
            doc.push(
                DocumentBlock::new(kind, label, block.content)
                    .with_source(name.clone())
                    .with_page(slide_number)
                    .with_ordinal(idx + 1),
            );
        }
    }

    if doc.is_empty() {
        if let Some(ocr_doc) = super::maybe_run_document_ocr_fallback(
            path,
            DocumentOcrFormat::Pptx,
            config,
            ocr_provider,
        )? {
            return Ok(ocr_doc);
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_odf(
    path: &Path,
    config: &crate::config::DocumentParserConfig,
    ocr_provider: Option<&dyn DocumentOcrProvider>,
) -> Result<ParsedDocument> {
    let mut zip = super::open_zip(path)?;
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);

    if let Some(metadata) = super::read_odf_metadata(&mut zip).ok().flatten() {
        if let Some(title) = metadata.title.clone() {
            doc.title = Some(title);
        }
        doc.push(
            DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some("document-metadata"),
                metadata.as_block_content(),
            )
            .with_source("meta.xml")
            .with_ordinal(0),
        );
    }

    for name in ["meta.xml", "styles.xml", "content.xml"] {
        if let Ok(content) = super::read_zip_entry(&mut zip, name) {
            let blocks = if name == "content.xml" {
                parse_odf_content_blocks(&content)?
            } else {
                super::text_blocks(
                    &super::extract_xml_text(&content)?,
                    DocumentBlockKind::Metadata,
                )
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

    if doc.is_empty() {
        if let Some(ocr_doc) = super::maybe_run_document_ocr_fallback(
            path,
            DocumentOcrFormat::Odf,
            config,
            ocr_provider,
        )? {
            return Ok(ocr_doc);
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_hwpx(path: &Path) -> Result<ParsedDocument> {
    let mut zip = super::open_zip(path)?;
    let mut names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    names.sort();

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    doc.push(
        DocumentBlock::new(DocumentBlockKind::Metadata, Some("hwpx"), "format=hwpx")
            .with_source("contents")
            .with_ordinal(0),
    );

    let mut ordinal = 1usize;
    for name in names {
        let lower = name.to_ascii_lowercase();
        if !(lower.starts_with("contents/") && lower.ends_with(".xml")) {
            continue;
        }

        let content = match super::read_zip_entry(&mut zip, &name) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let blocks = parse_hwpx_content_blocks(&content)?;
        if blocks.is_empty() {
            continue;
        }

        for (idx, block) in blocks.into_iter().enumerate() {
            let label = block
                .label
                .as_ref()
                .map(|label| format!("{}: {}", hwpx_part_label(&name), label))
                .or_else(|| Some(hwpx_part_label(&name)));
            doc.push(
                DocumentBlock::new(block.kind, label, block.content)
                    .with_source(name.clone())
                    .with_ordinal(ordinal + idx),
            );
        }
        ordinal += 1;
    }

    super::ensure_document(doc, path)
}

#[derive(Clone, Copy)]
enum LegacyOfficeKind {
    Word,
    Excel,
    PowerPoint,
}

impl LegacyOfficeKind {
    fn display_name(self) -> &'static str {
        match self {
            Self::Word => "word",
            Self::Excel => "excel",
            Self::PowerPoint => "powerpoint",
        }
    }

    fn primary_streams(self) -> &'static [&'static str] {
        match self {
            Self::Word => &["WordDocument", "0Table", "1Table"],
            Self::Excel => &["Workbook", "Book"],
            Self::PowerPoint => &["PowerPoint Document", "Current User"],
        }
    }
}

fn parse_legacy_compound_document(path: &Path, kind: LegacyOfficeKind) -> Result<ParsedDocument> {
    let mut compound = cfb::open(path).with_context(|| {
        format!(
            "failed to open legacy {} document {}",
            kind.display_name(),
            path.display()
        )
    })?;

    let stream_paths = compound
        .walk()
        .filter(|entry| entry.is_stream())
        .map(|entry| entry.path().to_path_buf())
        .collect::<Vec<_>>();

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    doc.push(
        DocumentBlock::new(
            DocumentBlockKind::Metadata,
            Some("legacy-office"),
            format!("format={}", kind.display_name()),
        )
        .with_source(kind.display_name())
        .with_ordinal(0),
    );

    let mut seen = BTreeSet::new();
    let mut ordinal = 1usize;
    for stream_path in prioritize_legacy_streams(stream_paths, kind) {
        let Some(stream_name) = stream_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let mut stream = compound.open_stream(&stream_path)?;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes)?;

        let values = extract_legacy_stream_strings(&bytes, kind);
        if values.is_empty() {
            continue;
        }

        for value in values {
            let normalized = normalize_legacy_text(&value, kind);
            if normalized.trim().is_empty() || !seen.insert(normalized.clone()) {
                continue;
            }

            let label = legacy_stream_label(stream_name, ordinal, kind);
            let block_kind = legacy_block_kind(&normalized, kind, ordinal);
            if doc.title.is_none() && matches!(block_kind, DocumentBlockKind::Heading) {
                doc.title = Some(normalized.clone());
            }
            doc.push(legacy_document_block(
                block_kind,
                label,
                normalized,
                stream_name,
                ordinal,
            ));
            ordinal += 1;
        }
    }

    super::ensure_document(doc, path)
}

fn write_archive_entry_tempfile(name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let temp_path = std::env::temp_dir().join(format!(
        "a3s-default-parser-archive-{}-{}",
        Uuid::new_v4(),
        Path::new(name)
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .unwrap_or("entry.bin")
    ));
    fs::write(&temp_path, bytes)
        .with_context(|| format!("failed to materialize archive office entry {name}"))?;
    Ok(temp_path)
}

fn prioritize_legacy_streams(
    mut stream_paths: Vec<std::path::PathBuf>,
    kind: LegacyOfficeKind,
) -> Vec<std::path::PathBuf> {
    stream_paths.sort_by(|left, right| {
        let left_name = left
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let right_name = right
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let left_primary = kind.primary_streams().contains(&left_name);
        let right_primary = kind.primary_streams().contains(&right_name);
        right_primary
            .cmp(&left_primary)
            .then_with(|| left_name.cmp(right_name))
    });
    stream_paths
}

fn prioritize_hwp_streams(mut stream_paths: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    stream_paths.sort_by(|left, right| {
        let left_name = left.to_string_lossy().to_ascii_lowercase();
        let right_name = right.to_string_lossy().to_ascii_lowercase();
        let left_primary = left_name.contains("prvtext") || left_name.contains("bodytext");
        let right_primary = right_name.contains("prvtext") || right_name.contains("bodytext");
        right_primary
            .cmp(&left_primary)
            .then_with(|| left_name.cmp(&right_name))
    });
    stream_paths
}

fn extract_legacy_stream_strings(bytes: &[u8], kind: LegacyOfficeKind) -> Vec<String> {
    let mut values = Vec::new();

    if let Some(text) = decode_utf16le_text(bytes) {
        let normalized = normalize_legacy_text(&text, kind);
        if is_interesting_legacy_text(&normalized) {
            values.push(normalized);
        }
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        let normalized = normalize_legacy_text(text.trim_end_matches('\0'), kind);
        if is_interesting_legacy_text(&normalized) {
            values.push(normalized);
        }
    }

    for text in extract_utf16le_strings(bytes)
        .into_iter()
        .chain(extract_ascii_strings(bytes))
    {
        let normalized = normalize_legacy_text(&text, kind);
        if is_interesting_legacy_text(&normalized) {
            values.push(normalized);
        }
    }

    let mut deduped = Vec::new();
    let mut seen = BTreeSet::new();
    for value in values {
        if seen.insert(value.clone()) {
            deduped.push(value);
        }
    }

    if matches!(kind, LegacyOfficeKind::Excel) {
        return deduped
            .into_iter()
            .map(|value| value.replace('\u{000b}', "\t"))
            .collect();
    }

    deduped
}

fn extract_hwp_stream_strings(bytes: &[u8]) -> Vec<String> {
    let mut values = Vec::new();

    if let Some(text) = decode_utf16le_text(bytes) {
        if is_interesting_legacy_text(&text) {
            values.push(text);
        }
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        let normalized = text.trim_end_matches('\0').to_string();
        if is_interesting_legacy_text(&normalized) {
            values.push(normalized);
        }
    }

    values.extend(extract_utf16le_strings(bytes));
    values.extend(extract_ascii_strings(bytes));

    let mut deduped = Vec::new();
    let mut seen = BTreeSet::new();
    for value in values {
        let normalized = value.trim().to_string();
        if normalized.is_empty() || !is_interesting_legacy_text(&normalized) {
            continue;
        }
        if seen.insert(normalized.clone()) {
            deduped.push(normalized);
        }
    }
    deduped
}

fn normalize_legacy_text(text: &str, kind: LegacyOfficeKind) -> String {
    match kind {
        LegacyOfficeKind::Excel => text
            .lines()
            .map(|line| {
                line.split('\t')
                    .map(|cell| cell.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => super::normalize_text(text),
    }
}

fn decode_utf16le_text(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 4 || !bytes.len().is_multiple_of(2) {
        return None;
    }

    let units = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    String::from_utf16(&units).ok()
}

fn extract_utf16le_strings(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();

    for chunk in bytes.chunks_exact(2) {
        let value = u16::from_le_bytes([chunk[0], chunk[1]]);
        let ch = char::from_u32(value as u32);
        match ch {
            Some(c) if is_legacy_text_char(c) => current.push(c),
            _ => flush_legacy_string(&mut current, &mut out, 4),
        }
    }
    flush_legacy_string(&mut current, &mut out, 4);
    out
}

fn extract_ascii_strings(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();

    for &byte in bytes {
        let ch = byte as char;
        if is_legacy_text_char(ch) && ch.is_ascii() {
            current.push(ch);
        } else {
            flush_legacy_string(&mut current, &mut out, 6);
        }
    }
    flush_legacy_string(&mut current, &mut out, 6);
    out
}

fn flush_legacy_string(current: &mut String, out: &mut Vec<String>, min_len: usize) {
    if current.chars().count() >= min_len {
        out.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

fn is_legacy_text_char(ch: char) -> bool {
    ch == '\n' || ch == '\r' || ch == '\t' || (!ch.is_control() && !ch.is_ascii_control())
}

fn is_interesting_legacy_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 3 {
        return false;
    }
    if trimmed.starts_with("Root Entry") || trimmed.starts_with("ObjectPool") {
        return false;
    }
    let alnum = trimmed.chars().filter(|ch| ch.is_alphanumeric()).count();
    let total = trimmed.chars().count().max(1);
    (alnum as f32 / total as f32) > 0.2
}

fn legacy_stream_label(stream_name: &str, ordinal: usize, kind: LegacyOfficeKind) -> String {
    match kind {
        LegacyOfficeKind::Word => {
            if ordinal == 1 {
                format!("{stream_name}: body")
            } else {
                format!("{stream_name}: section {ordinal}")
            }
        }
        LegacyOfficeKind::Excel => format!("{stream_name}: worksheet"),
        LegacyOfficeKind::PowerPoint => format!("slide {ordinal}"),
    }
}

fn legacy_block_kind(text: &str, kind: LegacyOfficeKind, ordinal: usize) -> DocumentBlockKind {
    match kind {
        LegacyOfficeKind::Word => {
            if ordinal == 1 && super::looks_like_heading(text) {
                DocumentBlockKind::Heading
            } else {
                DocumentBlockKind::Paragraph
            }
        }
        LegacyOfficeKind::Excel => {
            if text.lines().count() >= 2 && text.contains('\t') {
                DocumentBlockKind::Table
            } else {
                DocumentBlockKind::Paragraph
            }
        }
        LegacyOfficeKind::PowerPoint => {
            if ordinal == 1 && super::looks_like_heading(text) {
                DocumentBlockKind::Heading
            } else {
                DocumentBlockKind::Slide
            }
        }
    }
}

pub(super) fn xlsx_cell_reference_to_index(reference: &str) -> Option<usize> {
    let letters: String = reference
        .chars()
        .take_while(|ch| ch.is_ascii_alphabetic())
        .collect();
    if letters.is_empty() {
        return None;
    }

    let mut value = 0usize;
    for ch in letters.chars() {
        value = value * 26 + ((ch.to_ascii_uppercase() as u8 - b'A') as usize + 1);
    }
    value.checked_sub(1)
}

fn read_shared_strings(zip: &mut ZipArchive<File>) -> Result<Vec<String>> {
    let content = super::read_zip_entry(zip, "xl/sharedStrings.xml")?;
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

fn read_xlsx_sheet_names(zip: &mut ZipArchive<File>) -> Result<HashMap<String, String>> {
    let workbook = super::read_zip_entry(zip, "xl/workbook.xml")?;
    let rels = super::read_zip_entry(zip, "xl/_rels/workbook.xml.rels")?;
    let workbook_doc = Document::parse(&workbook).context("failed to parse workbook.xml")?;
    let rels_doc = Document::parse(&rels).context("failed to parse workbook.xml.rels")?;

    let mut rel_targets = HashMap::new();
    for rel in rels_doc
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "Relationship")
    {
        let Some(id) = rel.attribute("Id").or_else(|| rel.attribute("r:Id")) else {
            continue;
        };
        let Some(target) = rel.attribute("Target") else {
            continue;
        };

        let resolved = if target.starts_with('/') {
            target.trim_start_matches('/').to_string()
        } else if target.starts_with("xl/") {
            target.to_string()
        } else {
            format!("xl/{target}")
        };
        rel_targets.insert(id.to_string(), resolved);
    }

    let mut sheet_names = HashMap::new();
    for sheet in workbook_doc
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "sheet")
    {
        let Some(name) = sheet.attribute("name") else {
            continue;
        };
        let Some(rel_id) = attribute_by_local_name(sheet, "id") else {
            continue;
        };
        if let Some(target) = rel_targets.get(rel_id) {
            sheet_names.insert(target.clone(), name.to_string());
        }
    }

    Ok(sheet_names)
}

fn read_xlsb_sheet_names(zip: &mut ZipArchive<File>) -> Result<HashMap<String, String>> {
    let rels = super::read_zip_entry(zip, "xl/_rels/workbook.bin.rels")?;
    let rels_doc = Document::parse(&rels).context("failed to parse workbook.bin.rels")?;
    let workbook_bytes = super::read_zip_entry_bytes(zip, "xl/workbook.bin")?;

    let mut worksheet_targets = Vec::new();
    for rel in rels_doc
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "Relationship")
    {
        let Some(target) = rel.attribute("Target") else {
            continue;
        };
        let resolved = if target.starts_with('/') {
            target.trim_start_matches('/').to_string()
        } else if target.starts_with("xl/") {
            target.to_string()
        } else {
            format!("xl/{target}")
        };
        if resolved.starts_with("xl/worksheets/") && resolved.ends_with(".bin") {
            worksheet_targets.push(resolved);
        }
    }

    if worksheet_targets.is_empty() {
        return Ok(HashMap::new());
    }

    let candidate_names =
        extract_xlsb_workbook_sheet_names(&workbook_bytes, worksheet_targets.len());
    let mut sheet_names = HashMap::new();
    for (target, name) in worksheet_targets.into_iter().zip(candidate_names) {
        sheet_names.insert(target, name);
    }
    Ok(sheet_names)
}

struct XlsxSheetData {
    text: String,
    row_count: usize,
    column_count: usize,
    rows: Vec<Vec<String>>,
}

fn parse_xlsx_sheet(xml: &str, shared_strings: &[String]) -> Result<XlsxSheetData> {
    let doc = Document::parse(xml).context("failed to parse worksheet xml")?;
    let mut rows = Vec::new();
    let mut max_columns = 0usize;

    for row in doc.descendants().filter(|n| n.tag_name().name() == "row") {
        let mut cells: Vec<String> = Vec::new();
        for cell in row.children().filter(|n| n.tag_name().name() == "c") {
            let col_index = cell
                .attribute("r")
                .and_then(xlsx_cell_reference_to_index)
                .unwrap_or(cells.len());
            while cells.len() < col_index {
                cells.push(String::new());
            }
            let value = extract_xlsx_cell(cell, shared_strings);
            if cells.len() == col_index {
                cells.push(value);
            } else {
                cells[col_index] = value;
            }
        }
        if cells.iter().any(|cell| !cell.is_empty()) {
            max_columns = max_columns.max(cells.len());
            rows.push(cells);
        }
    }

    let text = super::table_text_from_cells(&rows);
    Ok(XlsxSheetData {
        text,
        row_count: rows.len(),
        column_count: max_columns,
        rows,
    })
}

fn xlsb_sheet_label(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| name.to_string())
}

fn read_xlsb_shared_strings(zip: &mut ZipArchive<File>) -> Result<Vec<String>> {
    let bytes = super::read_zip_entry_bytes(zip, "xl/sharedStrings.bin")?;
    Ok(extract_distinct_xlsb_strings(&bytes))
}

fn extract_xlsb_workbook_sheet_names(bytes: &[u8], limit: usize) -> Vec<String> {
    extract_distinct_xlsb_strings(bytes)
        .into_iter()
        .filter(|value| is_probable_xlsb_sheet_name(value))
        .take(limit)
        .collect()
}

fn extract_distinct_xlsb_strings(bytes: &[u8]) -> Vec<String> {
    let mut cleaned = Vec::new();
    let mut seen = BTreeSet::new();
    for value in extract_xlsb_string_candidates(bytes) {
        let normalized = super::normalize_text(&value);
        if normalized.is_empty() || !seen.insert(normalized.clone()) {
            continue;
        }
        cleaned.push(normalized);
    }
    cleaned
}

fn extract_raw_xlsb_strings(bytes: &[u8]) -> Vec<String> {
    let mut cleaned = Vec::new();
    for value in extract_xlsb_string_candidates(bytes) {
        let normalized = value
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .trim()
            .to_string();
        if normalized.is_empty() {
            continue;
        }
        cleaned.push(normalized);
    }
    cleaned
}

fn extract_xlsb_string_candidates(bytes: &[u8]) -> Vec<String> {
    let mut values = extract_utf16le_strings(bytes);
    values.extend(extract_ascii_strings(bytes));
    values
}

fn is_probable_xlsb_sheet_name(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "workbook" | "sheet" | "styles" | "theme" | "sharedstrings"
    ) {
        return false;
    }
    trimmed
        .chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, ' ' | '_' | '-' | '.' | '(' | ')'))
}

fn extract_xlsb_rows(bytes: &[u8]) -> Vec<Vec<String>> {
    let raw_values = extract_raw_xlsb_strings(bytes);
    if let Some(rows) = infer_xlsb_embedded_table_rows(&raw_values) {
        return rows;
    }

    let values = extract_distinct_xlsb_strings(bytes)
        .into_iter()
        .flat_map(|value| split_xlsb_cell_candidates(&value))
        .filter_map(|value| {
            let normalized = normalize_legacy_text(&value, LegacyOfficeKind::Excel);
            let trimmed = normalized.trim();
            if trimmed.is_empty() || !is_interesting_xlsb_cell(trimmed) {
                return None;
            }
            Some(vec![trimmed.to_string()])
        })
        .collect::<Vec<_>>();

    infer_xlsb_table_rows(values)
}

enum XlsbSheetSegment {
    Table(Vec<Vec<String>>, &'static str),
    Text(String),
}

fn extract_xlsb_segments(bytes: &[u8]) -> Vec<XlsbSheetSegment> {
    let raw_values = extract_raw_xlsb_strings(bytes);
    if raw_values.is_empty() {
        return Vec::new();
    }

    if let Some(segments) = infer_xlsb_direct_segments(&raw_values) {
        return segments;
    }

    let rows = extract_xlsb_rows(bytes);
    if rows.is_empty() {
        return Vec::new();
    }
    vec![XlsbSheetSegment::Table(rows, "heuristic-string-recovery")]
}

fn infer_xlsb_direct_segments(values: &[String]) -> Option<Vec<XlsbSheetSegment>> {
    let mut segments = Vec::new();
    let mut pending_text = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut table_extraction: Option<&'static str> = None;

    for value in values {
        if let Some((rows, extraction)) = parse_xlsb_delimited_fragment(value) {
            flush_xlsb_text_segment(&mut pending_text, &mut segments);
            if table_rows.is_empty() {
                table_rows = rows;
                table_extraction = Some(extraction);
            } else if table_rows.first().map(Vec::len) == rows.first().map(Vec::len)
                && table_extraction == Some(extraction)
            {
                table_rows.extend(rows);
            } else {
                flush_xlsb_table_segment(&mut table_rows, &mut table_extraction, &mut segments);
                table_rows = rows;
                table_extraction = Some(extraction);
            }
        } else {
            flush_xlsb_table_segment(&mut table_rows, &mut table_extraction, &mut segments);
            let normalized = super::normalize_text(value);
            if !normalized.is_empty() {
                pending_text.push(normalized);
            }
        }
    }

    flush_xlsb_table_segment(&mut table_rows, &mut table_extraction, &mut segments);
    flush_xlsb_text_segment(&mut pending_text, &mut segments);

    segments
        .iter()
        .any(|segment| matches!(segment, XlsbSheetSegment::Table(_, _)))
        .then_some(segments)
}

fn parse_xlsb_delimited_fragment(value: &str) -> Option<(Vec<Vec<String>>, &'static str)> {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    for (delimiter, extraction) in [
        ('\t', "embedded-tsv-recovery"),
        (',', "embedded-csv-recovery"),
        (';', "embedded-csv-recovery"),
    ] {
        let lines = normalized
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if lines.is_empty() || !lines.iter().any(|line| line.contains(delimiter)) {
            continue;
        }

        let rows = lines
            .iter()
            .map(|line| {
                line.split(delimiter)
                    .map(|cell| normalize_legacy_text(cell.trim(), LegacyOfficeKind::Excel))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let column_count = rows.first().map(Vec::len).unwrap_or(0);
        if column_count < 2 || rows.iter().any(|row| row.len() != column_count) {
            continue;
        }
        if rows.len() >= 2 && score_inferred_xlsb_rows(&rows) == 0 {
            continue;
        }
        return Some((rows, extraction));
    }

    None
}

fn flush_xlsb_table_segment(
    table_rows: &mut Vec<Vec<String>>,
    table_extraction: &mut Option<&'static str>,
    segments: &mut Vec<XlsbSheetSegment>,
) {
    if table_rows.is_empty() {
        return;
    }
    let rows = std::mem::take(table_rows);
    let extraction = table_extraction
        .take()
        .unwrap_or("heuristic-string-recovery");
    segments.push(XlsbSheetSegment::Table(rows, extraction));
}

fn flush_xlsb_text_segment(pending_text: &mut Vec<String>, segments: &mut Vec<XlsbSheetSegment>) {
    if pending_text.is_empty() {
        return;
    }
    segments.push(XlsbSheetSegment::Text(pending_text.join("\n\n")));
    pending_text.clear();
}

fn infer_xlsb_embedded_table_rows(values: &[String]) -> Option<Vec<Vec<String>>> {
    if let Some(rows) = infer_xlsb_delimited_row_sequence(values) {
        return Some(rows);
    }

    for value in values {
        let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
        let delimiter = if normalized.lines().any(|line| line.contains('\t')) {
            '\t'
        } else if normalized.lines().any(|line| line.contains(',')) {
            ','
        } else if normalized.lines().any(|line| line.contains(';')) {
            ';'
        } else {
            continue;
        };

        let rows = normalized
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                line.split(delimiter)
                    .map(|cell| normalize_legacy_text(cell.trim(), LegacyOfficeKind::Excel))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        if rows.len() < 2 {
            continue;
        }
        let column_count = rows.first().map(Vec::len).unwrap_or(0);
        if column_count < 2 || rows.iter().any(|row| row.len() != column_count) {
            continue;
        }
        if score_inferred_xlsb_rows(&rows) == 0 {
            continue;
        }

        return Some(rows);
    }

    None
}

fn infer_xlsb_delimited_row_sequence(values: &[String]) -> Option<Vec<Vec<String>>> {
    for delimiter in ['\t', ',', ';'] {
        let rows = values
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty() && value.contains(delimiter))
            .map(|value| {
                value
                    .split(delimiter)
                    .map(|cell| normalize_legacy_text(cell.trim(), LegacyOfficeKind::Excel))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        if rows.len() < 2 {
            continue;
        }
        let column_count = rows.first().map(Vec::len).unwrap_or(0);
        if column_count < 2 || rows.iter().any(|row| row.len() != column_count) {
            continue;
        }
        if score_inferred_xlsb_rows(&rows) == 0 {
            continue;
        }
        return Some(rows);
    }

    None
}

fn split_xlsb_cell_candidates(value: &str) -> Vec<String> {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.contains('\n') {
        return normalized
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
    }
    vec![normalized]
}

fn infer_xlsb_table_rows(rows: Vec<Vec<String>>) -> Vec<Vec<String>> {
    if rows.len() < 4 || rows.iter().any(|row| row.len() != 1) {
        return rows;
    }

    let values = rows
        .iter()
        .filter_map(|row| row.first().cloned())
        .collect::<Vec<_>>();

    let row_major = infer_xlsb_row_major_rows(&values);
    let column_major = infer_xlsb_column_major_rows(&values);

    match (row_major, column_major) {
        (Some(row_major), Some(column_major)) => {
            let row_score = score_inferred_xlsb_rows(&row_major);
            let column_score = score_inferred_xlsb_rows(&column_major);
            if column_score >= row_score {
                return column_major;
            }
            return row_major;
        }
        (Some(row_major), None) => return row_major,
        (None, Some(column_major)) => return column_major,
        (None, None) => {}
    }

    rows
}

pub(super) fn infer_xlsb_row_major_rows(values: &[String]) -> Option<Vec<Vec<String>>> {
    let max_columns = values.len().min(6);
    let mut best: Option<(usize, Vec<Vec<String>>)> = None;

    for column_count in 2..=max_columns {
        if !values.len().is_multiple_of(column_count) {
            continue;
        }

        let rows = values
            .chunks(column_count)
            .map(|chunk| chunk.to_vec())
            .collect::<Vec<_>>();
        if rows.len() < 2 || rows.iter().any(|row| row.len() != column_count) {
            continue;
        }

        let score = score_inferred_xlsb_rows(&rows);
        if score == 0 {
            continue;
        }

        match &best {
            Some((best_score, _)) if *best_score >= score => {}
            _ => best = Some((score, rows)),
        }
    }

    best.map(|(_, rows)| rows)
}

pub(super) fn infer_xlsb_column_major_rows(values: &[String]) -> Option<Vec<Vec<String>>> {
    let max_columns = values.len().min(6);
    let mut best: Option<(usize, Vec<Vec<String>>)> = None;

    for column_count in 2..=max_columns {
        if !values.len().is_multiple_of(column_count) {
            continue;
        }
        let rows_per_column = values.len() / column_count;
        if rows_per_column < 2 {
            continue;
        }

        let columns = values
            .chunks(rows_per_column)
            .map(|chunk| chunk.to_vec())
            .collect::<Vec<_>>();
        if columns.len() != column_count
            || columns.iter().any(|column| column.len() != rows_per_column)
        {
            continue;
        }

        let rows = transpose_columns_to_rows(&columns);
        let score = score_inferred_xlsb_rows(&rows);
        if score == 0 {
            continue;
        }

        match &best {
            Some((best_score, _)) if *best_score >= score => {}
            _ => best = Some((score, rows)),
        }
    }

    best.map(|(_, rows)| rows)
}

fn transpose_columns_to_rows(columns: &[Vec<String>]) -> Vec<Vec<String>> {
    let row_count = columns.first().map(Vec::len).unwrap_or(0);
    let mut rows = Vec::with_capacity(row_count);

    for row_idx in 0..row_count {
        let mut row = Vec::with_capacity(columns.len());
        for column in columns {
            row.push(column.get(row_idx).cloned().unwrap_or_default());
        }
        rows.push(row);
    }

    rows
}

fn score_inferred_xlsb_rows(rows: &[Vec<String>]) -> usize {
    if rows.len() < 2 {
        return 0;
    }
    let column_count = rows[0].len();
    if column_count < 2 || rows.iter().any(|row| row.len() != column_count) {
        return 0;
    }

    let header = &rows[0];
    let header_text_cells = header
        .iter()
        .filter(|cell| is_probable_header_cell(cell))
        .count();
    if header_text_cells < column_count.saturating_sub(1).max(1) {
        return 0;
    }

    let data_rows = &rows[1..];
    if data_rows.len() < 2 && column_count > 2 {
        return 0;
    }
    let populated_cells = data_rows
        .iter()
        .flatten()
        .filter(|cell| !cell.trim().is_empty())
        .count();
    if populated_cells < column_count {
        return 0;
    }

    let data_like_cells = data_rows
        .iter()
        .flatten()
        .filter(|cell| is_probable_data_cell(cell))
        .count();
    if data_like_cells == 0 {
        return 0;
    }

    header_text_cells * 2 + data_like_cells + data_rows.len() * 3 - column_count
}

fn is_probable_header_cell(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 48
        && !looks_like_numeric_cell(trimmed)
        && trimmed
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, ' ' | '_' | '-' | '/' | '.' | '%'))
}

fn is_probable_data_cell(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && (looks_like_numeric_cell(trimmed)
            || matches!(trimmed, "TRUE" | "FALSE")
            || trimmed.len() <= 64)
}

fn looks_like_numeric_cell(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.parse::<f64>().is_ok()
}

fn is_interesting_xlsb_cell(value: &str) -> bool {
    let trimmed = value.trim();
    is_interesting_legacy_text(trimmed)
        || looks_like_numeric_cell(trimmed)
        || matches!(trimmed, "TRUE" | "FALSE")
        || trimmed.starts_with('=')
}

fn legacy_document_block(
    kind: DocumentBlockKind,
    label: String,
    content: String,
    source: &str,
    ordinal: usize,
) -> DocumentBlock {
    let mut block = DocumentBlock::new(kind.clone(), Some(label), content.clone())
        .with_source(source)
        .with_ordinal(ordinal);

    if kind == DocumentBlockKind::Table {
        let rows = content
            .lines()
            .map(|row| row.split('\t').map(str::to_string).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let row_count = rows.len();
        let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        block = block
            .with_attribute("row_count", row_count.to_string())
            .with_attribute("column_count", column_count.to_string());
        if let Some(payload) = super::table_structured_payload(&rows) {
            block = block.with_structured_payload(payload);
        }
    }

    block
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
        let content = super::normalize_text(&content);
        if content.is_empty() {
            continue;
        }

        let kind = if paragraph_style(para)
            .map(|style| style.to_ascii_lowercase().contains("heading"))
            .unwrap_or(false)
            || super::looks_like_heading(&content)
        {
            DocumentBlockKind::Heading
        } else {
            DocumentBlockKind::Paragraph
        };

        blocks.push(DocumentBlock::new(kind, None::<String>, content));
    }

    if blocks.is_empty() {
        let fallback = super::extract_xml_text(xml)?;
        Ok(super::fallback_text_blocks(&fallback))
    } else {
        Ok(blocks)
    }
}

fn paragraph_style(node: roxmltree::Node<'_, '_>) -> Option<String> {
    node.descendants()
        .find(|child| child.tag_name().name() == "pStyle")
        .and_then(|child| attribute_by_local_name(child, "val"))
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
        let content = super::collect_node_text(node);
        if content.is_empty() {
            continue;
        }
        blocks.push(DocumentBlock::new(kind, None::<String>, content));
    }

    if blocks.is_empty() {
        let fallback = super::extract_xml_text(xml)?;
        Ok(super::fallback_text_blocks(&fallback))
    } else {
        Ok(blocks)
    }
}

fn parse_hwpx_content_blocks(xml: &str) -> Result<Vec<DocumentBlock>> {
    let doc = Document::parse(xml).context("failed to parse hwpx content xml")?;
    let mut blocks = Vec::new();

    for node in doc.descendants().filter(|node| node.is_element()) {
        let tag = node.tag_name().name();
        let kind = match tag {
            "title" | "subTitle" | "header" => DocumentBlockKind::Heading,
            "p" | "t" | "text" | "run" | "lineSeg" | "sec" | "subList" | "hp:p" => {
                DocumentBlockKind::Paragraph
            }
            _ => continue,
        };
        let content = super::collect_node_text(node);
        if content.is_empty() {
            continue;
        }
        blocks.push(DocumentBlock::new(kind, None::<String>, content));
    }

    if blocks.is_empty() {
        let fallback = super::extract_xml_text(xml)?;
        Ok(super::fallback_text_blocks(&fallback))
    } else {
        Ok(blocks)
    }
}

fn hwpx_part_label(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| name.to_string())
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

    if cell_type == "b" {
        let raw = cell
            .children()
            .find(|n| n.tag_name().name() == "v")
            .and_then(|n| n.text())
            .map(str::trim)
            .unwrap_or_default();
        return match raw {
            "1" => "TRUE".to_string(),
            "0" => "FALSE".to_string(),
            _ => raw.to_string(),
        };
    }

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
        if let Some(formula) = cell
            .children()
            .find(|n| n.tag_name().name() == "f")
            .and_then(|n| n.text())
            .map(str::trim)
            .filter(|formula| !formula.is_empty())
        {
            return format!("={formula}");
        }
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

fn docx_part_label(name: &str) -> String {
    if name == "word/document.xml" {
        "document".to_string()
    } else if let Some(index) = name
        .strip_prefix("word/header")
        .and_then(|rest| rest.strip_suffix(".xml"))
    {
        format!("header {}", index)
    } else if let Some(index) = name
        .strip_prefix("word/footer")
        .and_then(|rest| rest.strip_suffix(".xml"))
    {
        format!("footer {}", index)
    } else if name == "word/footnotes.xml" {
        "footnotes".to_string()
    } else if name == "word/endnotes.xml" {
        "endnotes".to_string()
    } else {
        name.to_string()
    }
}
