use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
#[cfg(test)]
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

pub(super) fn parse_msg(path: &Path, normalize_text: fn(&str) -> String) -> Result<ParsedDocument> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read Outlook MSG file {}", path.display()))?;
    let compound = parse_msg_compound(path, normalize_text).ok();

    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);

    let mut strings = extract_candidate_strings(&bytes, normalize_text);
    if let Some(compound) = &compound {
        for value in &compound.strings {
            if !strings.iter().any(|existing| existing == value) {
                strings.push(value.clone());
            }
        }
    }
    let emails = compound
        .as_ref()
        .and_then(|compound| (!compound.emails.is_empty()).then_some(compound.emails.clone()))
        .unwrap_or_else(|| extract_emails(&strings));
    let subject = compound
        .as_ref()
        .and_then(|compound| compound.subject.clone())
        .or_else(|| select_subject(&strings));
    let body = compound
        .as_ref()
        .and_then(select_msg_body)
        .or_else(|| select_body(&strings, subject.as_deref()));
    let header_blob = compound
        .as_ref()
        .and_then(|compound| compound.headers.clone());
    let compound_from = compound.as_ref().and_then(|compound| compound.from.clone());
    let compound_sender_name = compound
        .as_ref()
        .and_then(|compound| compound.sender_name.clone());
    let compound_to = compound.as_ref().and_then(|compound| compound.to.clone());
    let compound_cc = compound.as_ref().and_then(|compound| compound.cc.clone());
    let compound_bcc = compound.as_ref().and_then(|compound| compound.bcc.clone());
    let compound_date = compound.as_ref().and_then(|compound| compound.date.clone());
    let attachments = compound
        .as_ref()
        .map(|compound| compound.attachments.clone())
        .unwrap_or_default();

    if let Some(subject) = subject.clone() {
        doc.title = Some(subject.clone());
    }

    let mut header_lines = Vec::new();
    if let Some(headers) = header_blob {
        for line in headers
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            if matches!(line.split_once(':'), Some((key, _)) if matches!(key, "Subject" | "From" | "To" | "Cc" | "Date"))
            {
                header_lines.push(line.to_string());
            }
        }
    }
    if let Some(from) = compound_from {
        let from_line = if let Some(sender_name) = compound_sender_name {
            format!("From: {sender_name} <{from}>")
        } else {
            format!("From: {from}")
        };
        upsert_header_line(&mut header_lines, "From:", from_line);
    }
    if let Some(to) = compound_to {
        upsert_header_line(&mut header_lines, "To:", format!("To: {to}"));
    }
    if let Some(cc) = compound_cc {
        upsert_header_line(&mut header_lines, "Cc:", format!("Cc: {cc}"));
    }
    if let Some(bcc) = compound_bcc {
        upsert_header_line(&mut header_lines, "Bcc:", format!("Bcc: {bcc}"));
    }
    if let Some(date) = compound_date {
        upsert_header_line(&mut header_lines, "Date:", format!("Date: {date}"));
    }
    if let Some(subject) = subject {
        upsert_header_line(&mut header_lines, "Subject:", format!("Subject: {subject}"));
    }
    if !emails.is_empty() {
        header_lines.push(format!("Emails: {}", emails.join(", ")));
    }
    if !header_lines.is_empty() {
        let mut payload = serde_json::Map::new();
        for line in &header_lines {
            if let Some((key, value)) = line.split_once(':') {
                payload.insert(
                    key.trim().to_ascii_lowercase(),
                    serde_json::Value::String(value.trim().to_string()),
                );
            }
        }
        if !emails.is_empty() {
            payload.insert(
                "emails".to_string(),
                serde_json::Value::Array(
                    emails
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }
        let mut block = DocumentBlock::new(
            DocumentBlockKind::EmailHeader,
            Some("headers"),
            header_lines.join("\n"),
        )
        .with_source("msg")
        .with_ordinal(1)
        .with_attribute("record_type", "email-headers");
        if let Ok(serialized) = serde_json::to_string(&payload) {
            block = block.with_structured_payload(serialized);
        }
        doc.push(block);
    }

    if let Some(body) = body {
        doc.push(
            DocumentBlock::new(DocumentBlockKind::Paragraph, Some("body"), body)
                .with_source("msg")
                .with_ordinal(2)
                .with_attribute("record_type", "email-body"),
        );
    } else {
        let fallback = strings.into_iter().take(8).collect::<Vec<_>>().join("\n");
        if !fallback.trim().is_empty() {
            doc.push(
                DocumentBlock::new(DocumentBlockKind::Raw, Some("strings"), fallback)
                    .with_source("msg")
                    .with_ordinal(2),
            );
        }
    }

    if !attachments.is_empty() {
        let mut block = DocumentBlock::new(
            DocumentBlockKind::Metadata,
            Some("attachments"),
            attachments.join("\n"),
        )
        .with_source("msg")
        .with_ordinal(3)
        .with_attribute("record_type", "attachments")
        .with_attribute("attachment_count", attachments.len().to_string());
        if let Ok(payload) = serde_json::to_string(&attachments) {
            block = block.with_structured_payload(payload);
        }
        doc.push(block);
    }

    super::ensure_document(doc, path)
}

#[derive(Debug, Default, Clone)]
struct MsgExtraction {
    subject: Option<String>,
    body: Option<String>,
    html_body: Option<String>,
    rtf_body: Option<String>,
    headers: Option<String>,
    from: Option<String>,
    sender_name: Option<String>,
    to: Option<String>,
    cc: Option<String>,
    bcc: Option<String>,
    date: Option<String>,
    attachments: Vec<String>,
    emails: Vec<String>,
    strings: Vec<String>,
}

#[derive(Debug, Default, Clone)]
struct MsgRecipient {
    display_name: Option<String>,
    email: Option<String>,
    smtp: Option<String>,
    recipient_type: Option<u32>,
}

#[derive(Debug, Default, Clone)]
struct MsgAttachment {
    filename: Option<String>,
    long_filename: Option<String>,
    extension: Option<String>,
    mime_tag: Option<String>,
    content_id: Option<String>,
    attach_method: Option<u32>,
    size: Option<u32>,
}

fn parse_msg_compound(path: &Path, normalize_text: fn(&str) -> String) -> Result<MsgExtraction> {
    let mut cfb = cfb::open(path)
        .with_context(|| format!("failed to open compound msg {}", path.display()))?;
    let mut extraction = MsgExtraction::default();
    let mut collected = Vec::new();
    let mut recipients: BTreeMap<String, MsgRecipient> = BTreeMap::new();
    let mut attachments: BTreeMap<String, MsgAttachment> = BTreeMap::new();
    let stream_paths = cfb
        .walk()
        .filter(|entry| entry.is_stream())
        .filter_map(|entry| {
            let stream_name = entry.path().file_name()?.to_str()?;
            parse_msg_stream_name(stream_name)?;
            Some(entry.path().to_path_buf())
        })
        .collect::<Vec<_>>();

    for stream_path in stream_paths {
        let Some(stream_name) = stream_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some((property, ty)) = parse_msg_stream_name(stream_name) else {
            continue;
        };
        let parent = stream_path.parent().and_then(|parent| parent.to_str());

        let mut stream = cfb.open_stream(&stream_path)?;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes)?;
        if let Some(parent) = parent {
            if parent.contains("__recip_version1.0_") && property == "0C15" && ty == "0003" {
                let recipient = recipients.entry(parent.to_string()).or_default();
                recipient.recipient_type = decode_u32_msg(&bytes);
                continue;
            } else if parent.contains("__attach_version1.0_") {
                let attachment = attachments.entry(parent.to_string()).or_default();
                match (property, ty) {
                    ("3705", "0003") => {
                        attachment.attach_method = decode_u32_msg(&bytes);
                        continue;
                    }
                    ("0E20", "0003") => {
                        attachment.size = decode_u32_msg(&bytes);
                        continue;
                    }
                    _ => {}
                }
            }
        }
        if parent.is_none_or(|parent| !parent.contains("__recip_version1.0_")) {
            match (property, ty) {
                ("1013", "001F") | ("1013", "001E") | ("1013", "0102") => {
                    if let Some(html) = decode_msg_html_body(&bytes, ty, normalize_text) {
                        extraction.html_body = Some(html.clone());
                        collected.push(html);
                    }
                    continue;
                }
                ("1009", "001F") | ("1009", "001E") | ("1009", "0102") => {
                    if let Some(rtf) = decode_msg_rtf_body(&bytes, ty, normalize_text) {
                        extraction.rtf_body = Some(rtf.clone());
                        collected.push(rtf);
                    }
                    continue;
                }
                _ => {}
            }
        }
        let Some(text) = decode_msg_stream(&bytes, ty, normalize_text) else {
            continue;
        };
        let text = sanitize_msg_string(&text);
        if !is_interesting_msg_string(&text) {
            continue;
        }
        collected.push(text.clone());

        if let Some(parent) = parent {
            if parent.contains("__recip_version1.0_") {
                let recipient = recipients.entry(parent.to_string()).or_default();
                match property {
                    "3001" => recipient.display_name = Some(text.clone()),
                    "39FE" => recipient.smtp = Some(text.clone()),
                    "3003" | "3002" | "3A39" => recipient.email = Some(text.clone()),
                    _ => {}
                }
            } else if parent.contains("__attach_version1.0_") {
                let attachment = attachments.entry(parent.to_string()).or_default();
                match property {
                    "3703" => attachment.extension = Some(text.clone()),
                    "3704" => attachment.filename = Some(text.clone()),
                    "3707" => attachment.long_filename = Some(text.clone()),
                    "370E" => attachment.mime_tag = Some(text.clone()),
                    "3712" => attachment.content_id = Some(text.clone()),
                    _ => {}
                }
            }
        }

        match property {
            "0037" => extraction.subject = Some(text),
            "1000" => extraction.body = Some(text),
            "007D" => extraction.headers = Some(text),
            "0C1A" | "0042" => extraction.sender_name = Some(text),
            "0C1F" | "0065" | "5D02" => extraction.from = Some(text),
            "0E04" => extraction.to = Some(text),
            "0E03" => extraction.cc = Some(text),
            "0039" => extraction.date = Some(text),
            _ => {}
        }
    }

    merge_recipients_into_extraction(&mut extraction, recipients.into_values().collect());
    extraction.attachments = attachments
        .into_values()
        .filter_map(render_msg_attachment_summary)
        .collect();
    extraction.emails = extract_emails(&collected);
    extraction.strings = collected;
    if extraction.subject.is_none() {
        extraction.subject = select_subject(&extraction.strings);
    }
    if extraction.body.is_none() {
        extraction.body = select_body(&extraction.strings, extraction.subject.as_deref());
    }
    Ok(extraction)
}

fn select_msg_body(extraction: &MsgExtraction) -> Option<String> {
    extraction
        .body
        .clone()
        .or_else(|| {
            extraction
                .html_body
                .as_ref()
                .map(|html| render_msg_html_body(html))
        })
        .or_else(|| {
            extraction
                .rtf_body
                .as_ref()
                .map(|rtf| render_msg_rtf_body(rtf))
        })
}

fn parse_msg_stream_name(name: &str) -> Option<(&str, &str)> {
    let suffix = name.strip_prefix("__substg1.0_")?;
    if suffix.len() != 8 || !suffix.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    Some((&suffix[..4], &suffix[4..]))
}

fn decode_msg_stream(bytes: &[u8], ty: &str, normalize_text: fn(&str) -> String) -> Option<String> {
    match ty {
        "001F" => decode_utf16le_msg(bytes).map(|text| normalize_text(&text)),
        "001E" => std::str::from_utf8(bytes)
            .ok()
            .map(|text| normalize_text(text.trim_end_matches('\0'))),
        "0040" => decode_filetime_msg(bytes),
        _ => None,
    }
}

fn decode_msg_html_body(
    bytes: &[u8],
    ty: &str,
    normalize_text: fn(&str) -> String,
) -> Option<String> {
    match ty {
        "001F" | "001E" => decode_msg_stream(bytes, ty, normalize_text),
        "0102" => decode_binary_msg_text(bytes, normalize_text),
        _ => None,
    }
}

fn decode_msg_rtf_body(
    bytes: &[u8],
    ty: &str,
    normalize_text: fn(&str) -> String,
) -> Option<String> {
    let raw = match ty {
        "001F" | "001E" => decode_msg_stream(bytes, ty, normalize_text),
        "0102" => decode_binary_msg_text(bytes, normalize_text),
        _ => None,
    }?;
    raw.contains("{\\rtf").then_some(raw)
}

fn decode_binary_msg_text(bytes: &[u8], normalize_text: fn(&str) -> String) -> Option<String> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Some(normalize_text(text.trim_end_matches('\0')));
    }
    decode_utf16le_msg(bytes).map(|text| normalize_text(&text))
}

fn decode_utf16le_msg(bytes: &[u8]) -> Option<String> {
    let units = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    String::from_utf16(&units).ok()
}

fn decode_filetime_msg(bytes: &[u8]) -> Option<String> {
    let raw = bytes.get(0..8)?;
    let value = u64::from_le_bytes(raw.try_into().ok()?);
    if value == 0 {
        return None;
    }

    const WINDOWS_TO_UNIX_100NS: u64 = 116_444_736_000_000_000;
    let unix_100ns = value.checked_sub(WINDOWS_TO_UNIX_100NS)?;
    let secs = (unix_100ns / 10_000_000) as i64;
    let nanos = ((unix_100ns % 10_000_000) * 100) as u32;
    let datetime = DateTime::<Utc>::from_timestamp(secs, nanos)?;
    Some(datetime.format("%Y-%m-%d %H:%M:%S UTC").to_string())
}

fn decode_u32_msg(bytes: &[u8]) -> Option<u32> {
    let raw = bytes.get(0..4)?;
    Some(u32::from_le_bytes(raw.try_into().ok()?))
}

fn extract_candidate_strings(bytes: &[u8], normalize_text: fn(&str) -> String) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();

    for text in extract_utf16le_strings(bytes)
        .into_iter()
        .chain(extract_ascii_strings(bytes))
    {
        let normalized = sanitize_msg_string(&normalize_text(&text));
        if !is_interesting_msg_string(&normalized) {
            continue;
        }
        if seen.insert(normalized.clone()) {
            out.push(normalized);
        }
    }

    out
}

fn extract_utf16le_strings(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();

    for chunk in bytes.chunks_exact(2) {
        let value = u16::from_le_bytes([chunk[0], chunk[1]]);
        let ch = char::from_u32(value as u32);
        match ch {
            Some(c) if is_msg_text_char(c) => current.push(c),
            _ => flush_msg_string(&mut current, &mut out, 4),
        }
    }
    flush_msg_string(&mut current, &mut out, 4);
    out
}

fn extract_ascii_strings(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();

    for &byte in bytes {
        let ch = byte as char;
        if is_msg_text_char(ch) && ch.is_ascii() {
            current.push(ch);
        } else {
            flush_msg_string(&mut current, &mut out, 6);
        }
    }
    flush_msg_string(&mut current, &mut out, 6);
    out
}

fn flush_msg_string(current: &mut String, out: &mut Vec<String>, min_len: usize) {
    if current.chars().count() >= min_len {
        out.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

fn is_msg_text_char(ch: char) -> bool {
    ch == '\n' || ch == '\r' || ch == '\t' || (!ch.is_control() && !ch.is_ascii_control())
}

fn is_interesting_msg_string(text: &str) -> bool {
    if text.trim().len() < 3 {
        return false;
    }
    if text.starts_with("__substg1.0_") || text.starts_with("Root Entry") {
        return false;
    }
    if text.contains("Ole10Native") || text.contains("Properties") {
        return false;
    }
    let alnum = text.chars().filter(|ch| ch.is_alphanumeric()).count();
    let chars = text.chars().count().max(1);
    let ratio = alnum as f32 / chars as f32;
    ratio > 0.25
}

fn sanitize_msg_string(text: &str) -> String {
    let trimmed = text.trim_matches(|ch: char| {
        !(ch.is_ascii_alphanumeric()
            || ch.is_ascii_whitespace()
            || matches!(
                ch,
                '@' | '.' | ',' | ':' | ';' | '-' | '_' | '/' | '(' | ')'
            ))
    });
    trimmed.to_string()
}

fn email_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}\b").unwrap())
}

fn extract_emails(strings: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();

    for text in strings {
        for m in email_regex().find_iter(text) {
            let email = m.as_str().to_ascii_lowercase();
            if seen.insert(email.clone()) {
                out.push(email);
            }
        }
    }

    out
}

fn select_subject(strings: &[String]) -> Option<String> {
    strings
        .iter()
        .find(|text| {
            let len = text.chars().count();
            (4..=120).contains(&len)
                && text
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphanumeric())
                && !text.contains('@')
                && !text.contains("http://")
                && !text.contains("https://")
                && text.chars().any(|ch| ch.is_alphabetic())
                && text.split_whitespace().count() <= 16
        })
        .cloned()
}

fn select_body(strings: &[String], subject: Option<&str>) -> Option<String> {
    strings
        .iter()
        .filter(|text| Some(text.as_str()) != subject)
        .filter(|text| {
            let len = text.chars().count();
            len >= 40 && text.split_whitespace().count() >= 6 && !text.contains("__substg1.0_")
        })
        .max_by_key(|text| text.len())
        .cloned()
}

fn upsert_header_line(lines: &mut Vec<String>, prefix: &str, value: String) {
    if let Some(existing) = lines.iter_mut().find(|line| line.starts_with(prefix)) {
        *existing = value;
    } else {
        lines.push(value);
    }
}

fn merge_recipients_into_extraction(extraction: &mut MsgExtraction, recipients: Vec<MsgRecipient>) {
    let mut to = Vec::new();
    let mut cc = Vec::new();
    let mut bcc = Vec::new();

    for recipient in recipients {
        let display = recipient
            .display_name
            .or(recipient.smtp.clone())
            .or(recipient.email.clone());
        let email = recipient.smtp.or(recipient.email);
        let rendered = match (display, email) {
            (Some(display), Some(email)) if display != email => format!("{display} <{email}>"),
            (_, Some(email)) => email,
            (Some(display), None) => display,
            (None, None) => continue,
        };

        match recipient.recipient_type.unwrap_or(1) {
            2 => cc.push(rendered),
            3 => bcc.push(rendered),
            _ => to.push(rendered),
        }
    }

    if extraction.to.is_none() && !to.is_empty() {
        extraction.to = Some(to.join(", "));
    }
    if extraction.cc.is_none() && !cc.is_empty() {
        extraction.cc = Some(cc.join(", "));
    }
    if extraction.bcc.is_none() && !bcc.is_empty() {
        extraction.bcc = Some(bcc.join(", "));
    }
}

fn render_msg_html_body(html: &str) -> String {
    super::parse_markup_string(html, true)
        .map(|blocks| {
            let mut doc = ParsedDocument::new();
            for block in blocks {
                doc.push(block);
            }
            doc.to_text()
        })
        .or_else(|| super::render_html_to_text(html).ok())
        .map(|text| super::normalize_text(&text))
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| super::normalize_text(html))
}

fn render_msg_rtf_body(rtf: &str) -> String {
    super::strip_rtf(rtf)
}

fn render_msg_attachment_summary(attachment: MsgAttachment) -> Option<String> {
    let name = attachment
        .long_filename
        .or(attachment.filename)
        .or(attachment.content_id.clone())
        .or(attachment.mime_tag.clone())?;
    let mut fields = vec![format!("name={name}")];
    if let Some(extension) = attachment.extension {
        fields.push(format!("ext={extension}"));
    }
    if let Some(mime_tag) = attachment.mime_tag {
        fields.push(format!("mime={mime_tag}"));
    }
    if let Some(content_id) = attachment.content_id {
        fields.push(format!("content_id={content_id}"));
    }
    if let Some(size) = attachment.size {
        fields.push(format!("size={size}"));
    }
    if let Some(method) = attachment.attach_method {
        fields.push(format!("method={}", render_msg_attachment_method(method)));
    }
    Some(fields.join("\n"))
}

fn render_msg_attachment_method(method: u32) -> &'static str {
    match method {
        0 => "none",
        1 => "by_value",
        2 => "by_reference",
        3 => "by_reference_resolve",
        4 => "by_reference_only",
        5 => "embedded_message",
        6 => "ole",
        _ => "unknown",
    }
}

#[cfg(test)]
pub(super) fn write_msg_utf16_stream(
    compound: &mut cfb::CompoundFile<std::fs::File>,
    path: &str,
    value: &str,
) {
    let mut stream = compound.create_stream(path).unwrap();
    for unit in value.encode_utf16() {
        stream.write_all(&unit.to_le_bytes()).unwrap();
    }
    stream.write_all(&0u16.to_le_bytes()).unwrap();
}

#[cfg(test)]
pub(super) fn write_msg_time_stream(
    compound: &mut cfb::CompoundFile<std::fs::File>,
    path: &str,
    unix_seconds: i64,
) {
    let mut stream = compound.create_stream(path).unwrap();
    const WINDOWS_TO_UNIX_100NS: u64 = 116_444_736_000_000_000;
    let filetime = WINDOWS_TO_UNIX_100NS + (unix_seconds as u64) * 10_000_000;
    stream.write_all(&filetime.to_le_bytes()).unwrap();
}
