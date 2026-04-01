use anyhow::{Context, Result};
use base64::Engine as _;
use std::path::Path;

use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

#[derive(Debug, Default, Clone)]
pub(super) struct EmailPart {
    pub headers: std::collections::HashMap<String, String>,
    pub content_type: String,
    pub body: String,
    pub parts: Vec<EmailPart>,
}

pub(super) fn parse_email_part(raw: &str) -> EmailPart {
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

pub(super) fn collect_best_mail_body(part: &EmailPart) -> String {
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

pub(super) fn push_mail_blocks(
    doc: &mut ParsedDocument,
    mail: &EmailPart,
    message_index: Option<usize>,
) {
    if !mail.headers.is_empty() {
        let mut header_lines = Vec::new();
        for key in ["Subject", "From", "To", "Cc", "Date"] {
            if let Some(value) = mail.headers.get(key) {
                header_lines.push(format!("{key}: {value}"));
            }
        }
        if !header_lines.is_empty() {
            let label = message_index
                .map(|index| format!("message {index}: headers"))
                .unwrap_or_else(|| "headers".to_string());
            let ordinal = message_index.map(|index| index * 2 - 1).unwrap_or(1);
            let mut payload = serde_json::Map::new();
            for key in ["Subject", "From", "To", "Cc", "Date"] {
                if let Some(value) = mail.headers.get(key) {
                    payload.insert(
                        key.to_ascii_lowercase(),
                        serde_json::Value::String(value.clone()),
                    );
                }
            }
            let mut block = DocumentBlock::new(
                DocumentBlockKind::EmailHeader,
                Some(label),
                header_lines.join("\n"),
            )
            .with_source("message")
            .with_ordinal(ordinal)
            .with_attribute("record_type", "email-headers");
            if let Some(serialized) = serde_json::to_string(&payload).ok() {
                block = block.with_structured_payload(serialized);
            }
            doc.push(block);
        }
    }

    let body = collect_best_mail_body(mail);
    if !body.trim().is_empty() {
        let label = message_index
            .map(|index| format!("message {index}: body"))
            .unwrap_or_else(|| "body".to_string());
        let ordinal = message_index.map(|index| index * 2).unwrap_or(2);
        doc.push(
            DocumentBlock::new(DocumentBlockKind::Paragraph, Some(label), body)
                .with_source("message")
                .with_ordinal(ordinal)
                .with_attribute("record_type", "email-body")
                .with_attribute("content_type", &mail.content_type),
        );
    }
}

pub(super) fn strip_emlx_wrapper(raw: &str) -> &str {
    let normalized = raw.trim_start_matches('\u{feff}');
    let Some((first_line, rest)) = normalized.split_once('\n') else {
        return normalized;
    };
    if first_line.trim().chars().all(|ch| ch.is_ascii_digit()) {
        rest
    } else {
        normalized
    }
}

pub(super) fn split_mbox_messages(raw: &str) -> Vec<String> {
    let normalized = raw.replace("\r\n", "\n");
    let mut messages = Vec::new();
    let mut current = Vec::new();

    for line in normalized.lines() {
        if line.starts_with("From ") && !current.is_empty() {
            messages.push(current.join("\n"));
            current.clear();
            continue;
        }
        current.push(line.to_string());
    }

    if !current.is_empty() {
        messages.push(current.join("\n"));
    }

    messages
        .into_iter()
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
        .collect()
}

pub(super) fn parse_eml(path: &Path) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read EML file {}", path.display()))?;
    parse_eml_string(path, &raw)
}

pub(super) fn parse_emlx(path: &Path) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read EMLX file {}", path.display()))?;
    let mail_source = strip_emlx_wrapper(&raw);
    parse_eml_string(path, mail_source)
}

pub(super) fn parse_mbox(path: &Path) -> Result<ParsedDocument> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read mbox file {}", path.display()))?;
    parse_mbox_string(path, &raw)
}

pub(super) fn parse_mbox_string(path: &Path, raw: &str) -> Result<ParsedDocument> {
    let messages = split_mbox_messages(raw);
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);

    for (idx, message) in messages.iter().enumerate() {
        let mail = parse_email_part(message);
        if idx == 0 {
            if let Some(subject) = mail
                .headers
                .get("Subject")
                .filter(|value| !value.trim().is_empty())
            {
                doc.title = Some(subject.clone());
            }
        }
        push_mail_blocks(&mut doc, &mail, Some(idx + 1));
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_eml_string(path: &Path, raw: &str) -> Result<ParsedDocument> {
    let mail = parse_email_part(raw);

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    if let Some(subject) = mail
        .headers
        .get("Subject")
        .filter(|value| !value.trim().is_empty())
    {
        doc.title = Some(subject.clone());
    }
    push_mail_blocks(&mut doc, &mail, None);
    super::ensure_document(doc, path)
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
