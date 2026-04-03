use anyhow::{Context, Result};
use std::path::Path;

use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

pub(super) fn parse_plain_text_document(path: &Path) -> Result<ParsedDocument> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read text document {}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "csv" => {
            super::parsed_structured_text_document(path, parse_delimited_blocks(&content, ','))
        }
        "tsv" => {
            super::parsed_structured_text_document(path, parse_delimited_blocks(&content, '\t'))
        }
        "jsonl" | "ndjson" => {
            super::parsed_structured_text_document(path, parse_json_lines_blocks(&content))
        }
        "json" => {
            super::parsed_structured_text_document(path, parse_json_document_blocks(&content))
        }
        "yaml" | "yml" => {
            super::parsed_structured_text_document(path, parse_yaml_document_blocks(&content))
        }
        "toml" => {
            super::parsed_structured_text_document(path, parse_toml_document_blocks(&content))
        }
        _ => super::parsed_text_document(path, content, DocumentBlockKind::Paragraph),
    }
}

pub(super) fn parse_ipynb(path: &Path) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Jupyter notebook {}", path.display()))?;
    let notebook: serde_json::Value =
        serde_json::from_str(&raw).context("failed to parse ipynb json")?;

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);

    if let Some(metadata) = notebook.get("metadata").and_then(|value| value.as_object()) {
        let mut lines = Vec::new();
        let mut payload = serde_json::Map::new();
        if let Some(kernelspec) = metadata
            .get("kernelspec")
            .and_then(|value| value.as_object())
        {
            if let Some(name) = kernelspec
                .get("display_name")
                .and_then(|value| value.as_str())
            {
                lines.push(format!("kernel={name}"));
                payload.insert(
                    "kernel".to_string(),
                    serde_json::Value::String(name.to_string()),
                );
            }
        }
        if let Some(language_info) = metadata
            .get("language_info")
            .and_then(|value| value.as_object())
        {
            if let Some(name) = language_info.get("name").and_then(|value| value.as_str()) {
                lines.push(format!("language={name}"));
                payload.insert(
                    "language".to_string(),
                    serde_json::Value::String(name.to_string()),
                );
            }
        }
        if !lines.is_empty() {
            let mut block = DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some("notebook"),
                lines.join("\n"),
            )
            .with_source("metadata")
            .with_ordinal(0)
            .with_attribute("record_type", "notebook-metadata");
            if !payload.is_empty() {
                if let Ok(serialized) = serde_json::to_string(&payload) {
                    block = block.with_structured_payload(serialized);
                }
            }
            doc.push(block);
        }
    }

    if let Some(cells) = notebook.get("cells").and_then(|value| value.as_array()) {
        for (idx, cell) in cells.iter().enumerate() {
            let cell_type = cell
                .get("cell_type")
                .and_then(|value| value.as_str())
                .unwrap_or("raw");
            let source = join_json_text_lines(cell.get("source"));
            if source.trim().is_empty() {
                continue;
            }
            let (kind, label) = match cell_type {
                "markdown" => (
                    DocumentBlockKind::Section,
                    format!("markdown cell {}", idx + 1),
                ),
                "code" => (DocumentBlockKind::Code, format!("code cell {}", idx + 1)),
                _ => (DocumentBlockKind::Raw, format!("raw cell {}", idx + 1)),
            };
            let mut payload = serde_json::Map::new();
            payload.insert(
                "cell_type".to_string(),
                serde_json::Value::String(cell_type.to_string()),
            );
            if let Some(execution_count) =
                cell.get("execution_count").and_then(|value| value.as_i64())
            {
                payload.insert(
                    "execution_count".to_string(),
                    serde_json::Value::Number(execution_count.into()),
                );
            }
            if let Some(outputs) = cell.get("outputs").and_then(|value| value.as_array()) {
                payload.insert(
                    "output_count".to_string(),
                    serde_json::Value::Number((outputs.len() as u64).into()),
                );
            }
            if let Some(metadata) = cell.get("metadata").and_then(|value| value.as_object()) {
                payload.insert(
                    "metadata_keys".to_string(),
                    serde_json::Value::Array(
                        metadata
                            .keys()
                            .cloned()
                            .map(serde_json::Value::String)
                            .collect(),
                    ),
                );
            }
            let mut block = DocumentBlock::new(kind, Some(label), super::normalize_text(&source))
                .with_source("cells")
                .with_ordinal(idx + 1)
                .with_attribute("cell_type", cell_type);
            if let Ok(serialized) = serde_json::to_string(&payload) {
                block = block.with_structured_payload(serialized);
            }
            doc.push(block);
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_delimited_blocks(text: &str, delimiter: char) -> Vec<DocumentBlock> {
    let mut rows = Vec::new();
    let delimiter_name = match delimiter {
        ',' => "csv",
        '\t' => "tsv",
        _ => "delimited",
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cells = trimmed
            .split(delimiter)
            .map(|cell| cell.trim())
            .collect::<Vec<_>>();
        if cells.iter().all(|cell| cell.is_empty()) {
            continue;
        }
        rows.push(cells.join("\t"));
    }

    if rows.is_empty() {
        return Vec::new();
    }

    let structured_rows = rows
        .iter()
        .map(|row| row.split('\t').map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let column_count = structured_rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut block = DocumentBlock::new(DocumentBlockKind::Table, Some("table"), rows.join("\n"))
        .with_attribute("row_count", structured_rows.len().to_string())
        .with_attribute("column_count", column_count.to_string())
        .with_attribute("delimiter", delimiter_name);
    if let Some(payload) = super::table_structured_payload(&structured_rows) {
        block = block.with_structured_payload(payload);
    }

    vec![block]
}

pub(super) fn parse_json_lines_blocks(text: &str) -> Vec<DocumentBlock> {
    text.lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }

            let content = serde_json::from_str::<serde_json::Value>(trimmed)
                .ok()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| trimmed.to_string());

            Some(DocumentBlock::new(
                DocumentBlockKind::Code,
                Some(format!("record {}", idx + 1)),
                content,
            ))
        })
        .collect()
}

pub(super) fn parse_json_document_blocks(text: &str) -> Vec<DocumentBlock> {
    let Some(value) = serde_json::from_str::<serde_json::Value>(text).ok() else {
        return super::fallback_text_blocks(text);
    };
    structured_value_blocks("json", &value)
}

pub(super) fn parse_yaml_document_blocks(text: &str) -> Vec<DocumentBlock> {
    let Some(value) = serde_yaml::from_str::<serde_yaml::Value>(text).ok() else {
        return super::fallback_text_blocks(text);
    };
    let Some(json) = serde_json::to_value(value).ok() else {
        return super::fallback_text_blocks(text);
    };
    structured_value_blocks("yaml", &json)
}

pub(super) fn parse_toml_document_blocks(text: &str) -> Vec<DocumentBlock> {
    let Some(value) = text.parse::<toml::Value>().ok() else {
        return super::fallback_text_blocks(text);
    };
    let Some(json) = serde_json::to_value(value).ok() else {
        return super::fallback_text_blocks(text);
    };
    structured_value_blocks("toml", &json)
}

fn structured_value_blocks(format: &str, value: &serde_json::Value) -> Vec<DocumentBlock> {
    let mut blocks = vec![DocumentBlock::new(
        DocumentBlockKind::Metadata,
        Some("structure"),
        format!("format={format}\nroot_type={}", json_value_kind(value)),
    )];
    push_structured_value_blocks(value, "root", &mut blocks);
    blocks
}

fn push_structured_value_blocks(
    value: &serde_json::Value,
    path: &str,
    blocks: &mut Vec<DocumentBlock>,
) {
    match value {
        serde_json::Value::Object(map) => {
            let fields = map.keys().cloned().collect::<Vec<_>>();
            blocks.push(DocumentBlock::new(
                DocumentBlockKind::Section,
                Some(path.to_string()),
                format!("type=object\nfields={}", fields.join(", ")),
            ));
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                push_structured_value_blocks(child, &child_path, blocks);
            }
        }
        serde_json::Value::Array(items) => {
            blocks.push(DocumentBlock::new(
                DocumentBlockKind::Section,
                Some(path.to_string()),
                format!("type=array\nitems={}", items.len()),
            ));
            for (idx, child) in items.iter().enumerate() {
                let child_path = format!("{path}[{}]", idx);
                push_structured_value_blocks(child, &child_path, blocks);
            }
        }
        _ => {
            blocks.push(DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some(path.to_string()),
                scalar_json_value_to_text(value),
            ));
        }
    }
}

fn json_value_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn scalar_json_value_to_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(boolean) => boolean.to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string_pretty(value).unwrap_or_default()
        }
    }
}

fn join_json_text_lines(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(lines)) => lines
            .iter()
            .filter_map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}
