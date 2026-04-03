use anyhow::{Context, Result};
use std::path::Path;

use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

pub(super) fn parse_ics(
    path: &Path,
    normalize_text: fn(&str) -> String,
    tagged_fields_to_text: fn(&std::collections::BTreeMap<String, String>, &[&str]) -> String,
) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read iCalendar file {}", path.display()))?;
    parse_ics_string(path, &raw, normalize_text, tagged_fields_to_text)
}

pub(super) fn parse_ics_string(
    path: &Path,
    raw: &str,
    normalize_text: fn(&str) -> String,
    tagged_fields_to_text: fn(&std::collections::BTreeMap<String, String>, &[&str]) -> String,
) -> Result<ParsedDocument> {
    let lines = unfold_structured_text_lines(raw);
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);

    let calendar = collect_block_properties(&lines, "VCALENDAR", normalize_text);
    if let Some(title) = calendar
        .get("X-WR-CALNAME")
        .or_else(|| calendar.get("NAME"))
        .filter(|value| !value.trim().is_empty())
    {
        doc.title = Some(title.clone());
    }

    let calendar_content = tagged_fields_to_text(
        &calendar,
        &[
            "PRODID",
            "VERSION",
            "CALSCALE",
            "METHOD",
            "X-WR-CALNAME",
            "NAME",
        ],
    );
    if !calendar_content.is_empty() {
        doc.push(
            DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some("calendar"),
                calendar_content,
            )
            .with_source("ical")
            .with_ordinal(0),
        );
    }

    let mut ordinal = 1usize;
    for component in ["VEVENT", "VTODO", "VJOURNAL", "VFREEBUSY"] {
        for fields in collect_nested_block_properties(&lines, component, normalize_text) {
            if doc.title.is_none() {
                if let Some(title) = fields
                    .get("SUMMARY")
                    .filter(|value| !value.trim().is_empty())
                {
                    doc.title = Some(title.clone());
                }
            }
            let content = tagged_fields_to_text(
                &fields,
                &[
                    "UID",
                    "SUMMARY",
                    "DTSTART",
                    "DTEND",
                    "DUE",
                    "STATUS",
                    "LOCATION",
                    "ORGANIZER",
                    "ATTENDEE",
                    "DESCRIPTION",
                    "COMMENT",
                    "URL",
                ],
            );
            if content.is_empty() {
                continue;
            }
            doc.push(
                DocumentBlock::new(
                    DocumentBlockKind::Metadata,
                    Some(format!("{} {}", component.to_ascii_lowercase(), ordinal)),
                    content,
                )
                .with_source("ical")
                .with_ordinal(ordinal),
            );
            ordinal += 1;
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_vcf(
    path: &Path,
    normalize_text: fn(&str) -> String,
    tagged_fields_to_text: fn(&std::collections::BTreeMap<String, String>, &[&str]) -> String,
) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read vCard file {}", path.display()))?;
    parse_vcf_string(path, &raw, normalize_text, tagged_fields_to_text)
}

pub(super) fn parse_vcf_string(
    path: &Path,
    raw: &str,
    normalize_text: fn(&str) -> String,
    tagged_fields_to_text: fn(&std::collections::BTreeMap<String, String>, &[&str]) -> String,
) -> Result<ParsedDocument> {
    let lines = unfold_structured_text_lines(raw);
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);

    for (idx, fields) in collect_nested_block_properties(&lines, "VCARD", normalize_text)
        .into_iter()
        .enumerate()
    {
        if idx == 0 {
            if let Some(title) = fields
                .get("FN")
                .or_else(|| fields.get("N"))
                .filter(|value| !value.trim().is_empty())
            {
                doc.title = Some(title.clone());
            }
        }

        let content = tagged_fields_to_text(
            &fields,
            &[
                "FN", "N", "ORG", "TITLE", "ROLE", "TEL", "EMAIL", "ADR", "URL", "NOTE",
            ],
        );
        if content.is_empty() {
            continue;
        }
        doc.push(
            DocumentBlock::new(
                DocumentBlockKind::Metadata,
                Some(format!("contact {}", idx + 1)),
                content,
            )
            .with_source("vcard")
            .with_ordinal(idx + 1),
        );
    }

    super::ensure_document(doc, path)
}

fn unfold_structured_text_lines(raw: &str) -> Vec<String> {
    let mut unfolded: Vec<String> = Vec::new();
    for line in raw.replace("\r\n", "\n").replace('\r', "\n").lines() {
        if (line.starts_with(' ') || line.starts_with('\t')) && !unfolded.is_empty() {
            unfolded
                .last_mut()
                .expect("non-empty")
                .push_str(line.trim_start());
        } else {
            unfolded.push(line.to_string());
        }
    }
    unfolded
}

fn collect_block_properties(
    lines: &[String],
    block_name: &str,
    normalize_text: fn(&str) -> String,
) -> std::collections::BTreeMap<String, String> {
    let mut fields = std::collections::BTreeMap::new();
    let mut depth = 0usize;
    let block_name_upper = block_name.to_ascii_uppercase();

    for line in lines {
        let trimmed = line.trim();
        let upper = trimmed.to_ascii_uppercase();
        if upper == format!("BEGIN:{block_name_upper}") {
            depth += 1;
            continue;
        }
        if upper == format!("END:{block_name_upper}") {
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth == 1 {
            if upper.starts_with("BEGIN:") {
                depth += 1;
                continue;
            }
            if upper.starts_with("END:") {
                depth = depth.saturating_sub(1);
                continue;
            }
            if let Some((key, value)) = parse_structured_property_line(trimmed, normalize_text) {
                append_structured_field(&mut fields, key, value);
            }
        } else if depth > 1 {
            if upper.starts_with("BEGIN:") {
                depth += 1;
            } else if upper.starts_with("END:") {
                depth = depth.saturating_sub(1);
            }
        }
    }

    fields
}

fn collect_nested_block_properties(
    lines: &[String],
    block_name: &str,
    normalize_text: fn(&str) -> String,
) -> Vec<std::collections::BTreeMap<String, String>> {
    let mut entries = Vec::new();
    let mut current: Option<std::collections::BTreeMap<String, String>> = None;
    let mut depth = 0usize;
    let block_name_upper = block_name.to_ascii_uppercase();

    for line in lines {
        let trimmed = line.trim();
        let upper = trimmed.to_ascii_uppercase();
        if upper == format!("BEGIN:{block_name_upper}") {
            depth += 1;
            if depth == 1 {
                current = Some(std::collections::BTreeMap::new());
            }
            continue;
        }
        if upper == format!("END:{block_name_upper}") {
            if depth == 1 {
                if let Some(fields) = current.take() {
                    entries.push(fields);
                }
            }
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth == 1 {
            if upper.starts_with("BEGIN:") {
                depth += 1;
                continue;
            }
            if upper.starts_with("END:") {
                depth = depth.saturating_sub(1);
                continue;
            }
            if let Some((key, value)) = parse_structured_property_line(trimmed, normalize_text) {
                if let Some(fields) = current.as_mut() {
                    append_structured_field(fields, key, value);
                }
            }
        } else if depth > 1 {
            if upper.starts_with("BEGIN:") {
                depth += 1;
            } else if upper.starts_with("END:") {
                depth = depth.saturating_sub(1);
            }
        }
    }

    entries
}

fn parse_structured_property_line(
    line: &str,
    normalize_text: fn(&str) -> String,
) -> Option<(String, String)> {
    let (raw_key, raw_value) = line.split_once(':')?;
    let key = raw_key
        .split(';')
        .next()
        .map(str::trim)
        .filter(|key| !key.is_empty())?
        .to_ascii_uppercase();
    let value = decode_structured_text_value(raw_value, normalize_text);
    if value.is_empty() {
        return None;
    }
    Some((key, value))
}

fn decode_structured_text_value(value: &str, normalize_text: fn(&str) -> String) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some(next) => out.push(next),
                None => {}
            }
        } else {
            out.push(ch);
        }
    }
    normalize_text(&out.replace(';', ", "))
}

fn append_structured_field(
    fields: &mut std::collections::BTreeMap<String, String>,
    key: String,
    value: String,
) {
    fields
        .entry(key)
        .and_modify(|existing| {
            if !existing.is_empty() {
                existing.push_str("; ");
            }
            existing.push_str(&value);
        })
        .or_insert(value);
}
