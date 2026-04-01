use anyhow::{Context, Result};
use roxmltree::Document;
use std::path::Path;

use crate::document_parser::{DocumentMetadata, DocumentProvenance, ParsedDocument};

fn normalize_table_text(rows: &[String]) -> String {
    rows.iter()
        .map(|row| {
            row.split('\t')
                .map(|cell| cell.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .filter(|row| !row.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_table_cells(rows: &[Vec<String>]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|cell| normalize_text(cell))
                .collect::<Vec<_>>()
        })
        .filter(|row| row.iter().any(|cell| !cell.is_empty()))
        .collect()
}

pub(super) fn table_text_from_cells(rows: &[Vec<String>]) -> String {
    let normalized_rows = normalize_table_cells(rows)
        .into_iter()
        .map(|row| row.join("\t"))
        .collect::<Vec<_>>();
    normalize_table_text(&normalized_rows)
}

pub(super) fn table_structured_payload(rows: &[Vec<String>]) -> Option<String> {
    let normalized_rows = normalize_table_cells(rows);
    if normalized_rows.is_empty() {
        return None;
    }

    let column_count = normalized_rows.iter().map(Vec::len).max().unwrap_or(0);
    let row_count = normalized_rows.len();

    // First row is typically the header
    let headers = normalized_rows.first().cloned().unwrap_or_default();
    let data_rows = if normalized_rows.len() > 1 {
        &normalized_rows[1..]
    } else {
        &normalized_rows[..0]
    };

    serde_json::to_string(&serde_json::json!({
        "schema": "table/v1",
        "schema_url": "https://docs.a3s.dev/schemas/table/v1",
        "row_count": row_count,
        "data_row_count": data_rows.len(),
        "column_count": column_count,
        "headers": headers,
        "rows": normalized_rows,
        "description": format!("{}x{} table with {} data rows", column_count, row_count, data_rows.len()),
    }))
    .ok()
}

pub(super) fn attribute_by_local_name<'a>(
    node: roxmltree::Node<'a, 'a>,
    local_name: &str,
) -> Option<&'a str> {
    node.attributes()
        .find(|attr| attr.name() == local_name || attr.name().ends_with(&format!(":{local_name}")))
        .map(|attr| attr.value())
}

pub(super) fn looks_like_heading(text: &str) -> bool {
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

pub(super) fn extract_xml_text(xml: &str) -> Result<String> {
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

pub(super) fn normalize_text(text: &str) -> String {
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

pub(super) fn file_title(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string())
}

pub(super) fn enrich_document_metadata(
    doc: &mut ParsedDocument,
    path: &Path,
    detected_file_type: Option<&str>,
    extractor: Option<&str>,
) {
    let metadata = doc.metadata.get_or_insert_with(DocumentMetadata::default);
    metadata
        .attributes
        .entry("document.stage".to_string())
        .or_insert_with(|| "extracted".to_string());
    metadata.detected_file_type.get_or_insert_with(|| {
        detected_file_type
            .map(str::to_string)
            .or_else(|| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.to_ascii_lowercase())
            })
            .unwrap_or_else(|| "unknown".to_string())
    });

    let provenance = metadata
        .provenance
        .get_or_insert_with(DocumentProvenance::default);
    provenance
        .parser
        .get_or_insert_with(|| "composite-document-parser".to_string());
    if provenance.extractor.is_none() {
        provenance.extractor = extractor.map(str::to_string);
    }
}

pub(super) fn ensure_document(mut doc: ParsedDocument, path: &Path) -> Result<ParsedDocument> {
    if doc.is_empty() {
        anyhow::bail!("no extractable text found in {}", path.display());
    }
    enrich_document_metadata(&mut doc, path, None, None);
    Ok(doc)
}
