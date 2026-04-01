use anyhow::{Context, Result};

use crate::document_parser::{DocumentBlock, ParsedDocument};

pub(super) fn parse_html_document(path: &std::path::Path) -> Result<ParsedDocument> {
    super::markup::parse_html_document(path)
}

pub(super) fn parse_xml_document(path: &std::path::Path) -> Result<ParsedDocument> {
    super::markup::parse_xml_document(path)
}

pub(super) fn parse_rtf(path: &std::path::Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read RTF file {}", path.display()))?;
    Ok(strip_rtf(&raw))
}

#[allow(dead_code)]
pub(super) fn parse_markup_document(
    path: &std::path::Path,
    input: &str,
    is_html: bool,
) -> Result<ParsedDocument> {
    super::markup::parse_markup_document(path, input, is_html)
}

pub(super) fn parse_markup_string(input: &str, is_html: bool) -> Option<Vec<DocumentBlock>> {
    super::markup::parse_markup_string(input, is_html)
}

pub(super) fn extract_markup_title(input: &str) -> Option<String> {
    super::markup::extract_markup_title(input)
}

pub(super) fn render_html_to_text(input: &str) -> Result<String> {
    super::markup::render_html_to_text(input)
}

pub(super) fn collect_node_text(node: roxmltree::Node<'_, '_>) -> String {
    super::markup::collect_node_text(node)
}

pub(super) fn strip_rtf(input: &str) -> String {
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

    super::normalize_text(&out)
}
