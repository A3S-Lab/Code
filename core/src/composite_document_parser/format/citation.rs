use anyhow::{Context, Result};
use std::path::Path;

use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

pub(super) fn parse_ris(path: &Path, normalize_text: fn(&str) -> String) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read RIS file {}", path.display()))?;
    parse_ris_string(path, &raw, normalize_text)
}

pub(super) fn parse_ris_string(
    path: &Path,
    raw: &str,
    normalize_text: fn(&str) -> String,
) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    let mut entries = Vec::new();
    let mut current = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        current.push(line.trim_end().to_string());
        if line.starts_with("ER  -") {
            entries.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        entries.push(current);
    }

    for (idx, entry) in entries.into_iter().enumerate() {
        let fields = parse_tagged_lines(&entry, "  -", normalize_text);
        if idx == 0 {
            if let Some(title) = fields
                .get("TI")
                .or_else(|| fields.get("T1"))
                .or_else(|| fields.get("BT"))
            {
                doc.title = Some(title.clone());
            }
        }
        let content = tagged_fields_to_text(
            &fields,
            &[
                "TY", "TI", "T1", "BT", "AU", "PY", "JO", "T2", "AB", "KW", "DO",
            ],
        );
        if !content.is_empty() {
            doc.push(citation_block(
                format!("reference {}", idx + 1),
                content,
                "ris",
                idx + 1,
                fields,
            ));
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_enw(path: &Path, normalize_text: fn(&str) -> String) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read EndNote file {}", path.display()))?;
    parse_enw_string(path, &raw, normalize_text)
}

pub(super) fn parse_enw_string(
    path: &Path,
    raw: &str,
    normalize_text: fn(&str) -> String,
) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    let mut entries = Vec::new();
    let mut current = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            if !current.is_empty() {
                entries.push(std::mem::take(&mut current));
            }
            continue;
        }
        if trimmed.starts_with("%0") && !current.is_empty() {
            entries.push(std::mem::take(&mut current));
        }
        current.push(trimmed.to_string());
    }
    if !current.is_empty() {
        entries.push(current);
    }

    for (idx, entry) in entries.into_iter().enumerate() {
        let fields = parse_tagged_lines(&entry, " ", normalize_text);
        if idx == 0 {
            if let Some(title) = fields.get("%T").filter(|value| !value.trim().is_empty()) {
                doc.title = Some(title.clone());
            }
        }
        let content = tagged_fields_to_text(
            &fields,
            &["%0", "%T", "%A", "%D", "%J", "%B", "%X", "%K", "%R"],
        );
        if !content.is_empty() {
            doc.push(citation_block(
                format!("reference {}", idx + 1),
                content,
                "enw",
                idx + 1,
                fields,
            ));
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_nbib(
    path: &Path,
    normalize_text: fn(&str) -> String,
) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read NBIB file {}", path.display()))?;
    parse_nbib_string(path, &raw, normalize_text)
}

pub(super) fn parse_nbib_string(
    path: &Path,
    raw: &str,
    normalize_text: fn(&str) -> String,
) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    let fields = parse_tagged_lines(
        &raw.lines().map(|line| line.to_string()).collect::<Vec<_>>(),
        "- ",
        normalize_text,
    );
    if let Some(title) = fields.get("TI").or_else(|| fields.get("BTI")) {
        doc.title = Some(title.clone());
    }
    let content = tagged_fields_to_text(
        &fields,
        &[
            "PMID", "TI", "BTI", "FAU", "AU", "JT", "DP", "AB", "MH", "LID",
        ],
    );
    if !content.is_empty() {
        doc.push(citation_block(
            "citation".to_string(),
            content,
            "nbib",
            1,
            fields,
        ));
    }
    super::ensure_document(doc, path)
}

pub(super) fn parse_bib(path: &Path, normalize_text: fn(&str) -> String) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read BibTeX file {}", path.display()))?;
    parse_bib_string(path, &raw, normalize_text)
}

pub(super) fn parse_bib_string(
    path: &Path,
    raw: &str,
    normalize_text: fn(&str) -> String,
) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    for (idx, chunk) in raw
        .split('@')
        .filter(|chunk| !chunk.trim().is_empty())
        .enumerate()
    {
        let entry = format!("@{}", chunk.trim());
        let fields = parse_bib_fields(&entry, normalize_text);
        if idx == 0 {
            if let Some(title) = fields.get("title") {
                doc.title = Some(title.clone());
            }
        }
        let content = tagged_fields_to_text(
            &fields,
            &[
                "type",
                "key",
                "title",
                "author",
                "year",
                "journal",
                "booktitle",
                "abstract",
                "doi",
            ],
        );
        if !content.is_empty() {
            doc.push(citation_block(
                format!("reference {}", idx + 1),
                content,
                "bib",
                idx + 1,
                fields,
            ));
        }
    }
    super::ensure_document(doc, path)
}

pub(super) fn parse_csl(path: &Path, normalize_text: fn(&str) -> String) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read CSL JSON file {}", path.display()))?;
    parse_csl_string(path, &raw, normalize_text)
}

pub(super) fn parse_csl_string(
    path: &Path,
    raw: &str,
    normalize_text: fn(&str) -> String,
) -> Result<ParsedDocument> {
    let value: serde_json::Value =
        serde_json::from_str(&raw).context("failed to parse csl json")?;

    let entries = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(_) => vec![value],
        _ => anyhow::bail!("unsupported CSL JSON payload"),
    };

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);

    for (idx, entry) in entries.iter().enumerate() {
        let Some(object) = entry.as_object() else {
            continue;
        };

        if idx == 0 {
            if let Some(title) = object
                .get("title")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
            {
                doc.title = Some(normalize_text(title));
            }
        }

        let mut fields = std::collections::BTreeMap::new();
        for key in [
            "id",
            "type",
            "title",
            "container-title",
            "DOI",
            "URL",
            "abstract",
        ] {
            if let Some(value) = object.get(key) {
                let rendered = json_scalar_or_string_list(value, normalize_text);
                if !rendered.is_empty() {
                    fields.insert(key.to_string(), rendered);
                }
            }
        }

        if let Some(authors) = object
            .get("author")
            .and_then(|v| render_csl_names(v, normalize_text))
        {
            fields.insert("author".to_string(), authors);
        }
        if let Some(issued) = object.get("issued").and_then(render_csl_issued_date) {
            fields.insert("issued".to_string(), issued);
        }
        if let Some(keywords) = object
            .get("keyword")
            .map(|v| json_scalar_or_string_list(v, normalize_text))
        {
            if !keywords.is_empty() {
                fields.insert("keyword".to_string(), keywords);
            }
        }

        let content = tagged_fields_to_text(
            &fields,
            &[
                "id",
                "type",
                "title",
                "author",
                "container-title",
                "issued",
                "abstract",
                "keyword",
                "DOI",
                "URL",
            ],
        );
        if !content.is_empty() {
            doc.push(citation_block(
                format!("reference {}", idx + 1),
                content,
                "csl",
                idx + 1,
                fields,
            ));
        }
    }

    super::ensure_document(doc, path)
}

fn parse_tagged_lines(
    lines: &[String],
    delimiter: &str,
    normalize_text: fn(&str) -> String,
) -> std::collections::BTreeMap<String, String> {
    let mut fields = std::collections::BTreeMap::new();
    let mut current_key: Option<String> = None;

    for line in lines {
        if let Some((key, value)) = line.split_once(delimiter) {
            let key = key.trim().to_string();
            let value = normalize_text(value);
            if !value.is_empty() {
                fields
                    .entry(key.clone())
                    .and_modify(|existing: &mut String| {
                        if !existing.is_empty() {
                            existing.push_str("; ");
                        }
                        existing.push_str(&value);
                    })
                    .or_insert_with(|| value.clone());
            }
            current_key = Some(key);
        } else if line.starts_with(' ') {
            if let Some(key) = &current_key {
                let value = normalize_text(line);
                if !value.is_empty() {
                    fields
                        .entry(key.clone())
                        .and_modify(|existing: &mut String| {
                            if !existing.is_empty() {
                                existing.push(' ');
                            }
                            existing.push_str(&value);
                        })
                        .or_insert(value);
                }
            }
        }
    }

    fields
}

pub(super) fn tagged_fields_to_text(
    fields: &std::collections::BTreeMap<String, String>,
    ordered_keys: &[&str],
) -> String {
    ordered_keys
        .iter()
        .filter_map(|key| fields.get(*key).map(|value| format!("{key}={value}")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_bib_fields(
    entry: &str,
    normalize_text: fn(&str) -> String,
) -> std::collections::BTreeMap<String, String> {
    let mut fields = std::collections::BTreeMap::new();
    let trimmed = entry.trim();
    let Some((entry_type, rest)) = trimmed
        .strip_prefix('@')
        .and_then(|value| value.split_once('{'))
    else {
        return fields;
    };
    fields.insert("type".to_string(), entry_type.trim().to_string());

    let body = rest.trim_end().trim_end_matches('}').trim();
    let mut lines = body.lines();
    if let Some(first_line) = lines.next() {
        if let Some((key, _)) = first_line.split_once(',') {
            fields.insert("key".to_string(), key.trim().to_string());
        }
    }

    for line in lines {
        let line = line.trim().trim_end_matches(',');
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .trim()
            .trim_matches('{')
            .trim_matches('}')
            .trim_matches('"')
            .trim();
        if !value.is_empty() {
            fields.insert(key.trim().to_ascii_lowercase(), normalize_text(value));
        }
    }

    fields
}

fn json_scalar_or_string_list(
    value: &serde_json::Value,
    normalize_text: fn(&str) -> String,
) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(text) => normalize_text(text),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Bool(flag) => flag.to_string(),
        serde_json::Value::Array(items) => normalize_text(
            &items
                .iter()
                .filter_map(|item| match item {
                    serde_json::Value::String(text) => Some(text.clone()),
                    serde_json::Value::Number(number) => Some(number.to_string()),
                    serde_json::Value::Bool(flag) => Some(flag.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("; "),
        ),
        _ => String::new(),
    }
}

fn render_csl_names(
    value: &serde_json::Value,
    normalize_text: fn(&str) -> String,
) -> Option<String> {
    let authors = value.as_array()?;
    let rendered = authors
        .iter()
        .filter_map(|author| {
            let object = author.as_object()?;
            let family = object
                .get("family")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let given = object
                .get("given")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let literal = object
                .get("literal")
                .and_then(|value| value.as_str())
                .unwrap_or_default();

            let name = if !literal.trim().is_empty() {
                literal.to_string()
            } else if !family.trim().is_empty() && !given.trim().is_empty() {
                format!("{given} {family}")
            } else if !family.trim().is_empty() {
                family.to_string()
            } else if !given.trim().is_empty() {
                given.to_string()
            } else {
                String::new()
            };

            (!name.trim().is_empty()).then_some(normalize_text(&name))
        })
        .collect::<Vec<_>>();

    if rendered.is_empty() {
        None
    } else {
        Some(rendered.join("; "))
    }
}

fn render_csl_issued_date(value: &serde_json::Value) -> Option<String> {
    let object = value.as_object()?;
    let date_parts = object.get("date-parts")?.as_array()?;
    let first = date_parts.first()?.as_array()?;
    let parts = first
        .iter()
        .filter_map(|part| match part {
            serde_json::Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("-"))
    }
}

fn citation_block(
    label: String,
    content: String,
    source: &str,
    ordinal: usize,
    fields: std::collections::BTreeMap<String, String>,
) -> DocumentBlock {
    let mut block = DocumentBlock::new(DocumentBlockKind::Metadata, Some(label), content)
        .with_source(source)
        .with_ordinal(ordinal)
        .with_attribute("record_type", "citation")
        .with_attribute("citation_format", source);
    if let Some(payload) = serde_json::to_string(&fields).ok() {
        block = block.with_structured_payload(payload);
    }
    block
}
