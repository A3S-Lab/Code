use anyhow::{Context, Result};
use roxmltree::Document;
use std::path::Path;

use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

pub(super) fn parse_html_document(path: &Path) -> Result<ParsedDocument> {
    let html = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read HTML file {}", path.display()))?;
    parse_markup_document(path, &html, true)
}

pub(super) fn parse_xml_document(path: &Path) -> Result<ParsedDocument> {
    let xml = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read XML file {}", path.display()))?;
    parse_markup_document(path, &xml, false)
}

pub(super) fn parse_markup_document(
    path: &Path,
    input: &str,
    is_html: bool,
) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = extract_markup_title(input).or_else(|| super::file_title(path));
    let source = doc
        .title
        .clone()
        .unwrap_or_else(|| path.display().to_string());

    let blocks = parse_markup_string(input, is_html).unwrap_or_else(|| {
        let rendered = if is_html {
            render_html_to_text(input).unwrap_or_default()
        } else {
            super::extract_xml_text(input).unwrap_or_default()
        };
        super::fallback_text_blocks(&rendered)
    });

    for (idx, block) in blocks.into_iter().enumerate() {
        doc.push(block.with_source(source.clone()).with_ordinal(idx + 1));
    }

    super::ensure_document(doc, path)
}

pub(super) fn parse_markup_string(input: &str, is_html: bool) -> Option<Vec<DocumentBlock>> {
    let doc = Document::parse(input).ok()?;
    let mut blocks = Vec::new();

    for node in doc.descendants().filter(|node| node.is_element()) {
        let tag = node.tag_name().name();
        let kind = match tag {
            "title" => continue,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => DocumentBlockKind::Heading,
            "p" | "blockquote" | "outline" => DocumentBlockKind::Paragraph,
            "pre" | "code" => DocumentBlockKind::Code,
            "table" => DocumentBlockKind::Table,
            "meta" if is_html => DocumentBlockKind::Metadata,
            "section" | "article" => DocumentBlockKind::Section,
            _ => continue,
        };

        let (content, structured_payload, row_count, column_count) = match tag {
            "meta" if is_html => (collect_meta_content(node), None, None, None),
            "table" => collect_table_content(node),
            "outline" => (collect_outline_text(node), None, None, None),
            _ => (collect_node_text(node), None, None, None),
        };
        if content.trim().is_empty() {
            continue;
        }

        let label = match tag {
            "meta" => node
                .attribute("name")
                .or_else(|| node.attribute("property"))
                .or_else(|| node.attribute("http-equiv"))
                .map(str::to_string),
            "table" => collect_table_label(node),
            "section" | "article" => collect_section_label(node),
            _ => None,
        };
        let mut block = DocumentBlock::new(kind, label, content);
        if let Some(row_count) = row_count {
            block = block.with_attribute("row_count", row_count.to_string());
        }
        if let Some(column_count) = column_count {
            block = block.with_attribute("column_count", column_count.to_string());
        }
        if let Some(payload) = structured_payload {
            block = block.with_structured_payload(payload);
        }
        blocks.push(block);
    }

    for node in doc.descendants().filter(|node| node.is_element()) {
        let tag = node.tag_name().name();
        match tag {
            "img" if is_html => {
                let content = collect_image_alt_text(node);
                if !content.is_empty() {
                    let label = node
                        .attribute("src")
                        .or_else(|| node.attribute("data-src"))
                        .map(|src| format!("image-alt: {src}"))
                        .or_else(|| Some("image-alt".to_string()));
                    blocks.push(DocumentBlock::new(
                        DocumentBlockKind::Metadata,
                        label,
                        content,
                    ));
                }
            }
            "ul" | "ol" => {
                let content = collect_list_text(node);
                if !content.is_empty() {
                    blocks.push(DocumentBlock::new(
                        DocumentBlockKind::Section,
                        Some(match tag {
                            "ol" => "ordered-list",
                            _ => "list",
                        }),
                        content,
                    ));
                }
            }
            "dl" => {
                let content = collect_definition_list_text(node);
                if !content.is_empty() {
                    blocks.push(DocumentBlock::new(
                        DocumentBlockKind::Section,
                        Some("definitions"),
                        content,
                    ));
                }
            }
            _ => {}
        }
    }

    if blocks.is_empty() {
        None
    } else {
        Some(dedupe_adjacent_blocks(blocks))
    }
}

pub(super) fn extract_markup_title(input: &str) -> Option<String> {
    let doc = Document::parse(input).ok()?;
    doc.descendants()
        .find(|node| node.has_tag_name("title"))
        .and_then(|node| {
            let text = node
                .children()
                .filter_map(|child| child.text())
                .collect::<Vec<_>>()
                .join(" ");
            let text = super::normalize_text(&text);
            (!text.trim().is_empty()).then_some(text)
        })
}

pub(super) fn render_html_to_text(input: &str) -> Result<String> {
    html2text::from_read(input.as_bytes(), 80).context("failed to render HTML as text")
}

pub(super) fn collect_node_text(node: roxmltree::Node<'_, '_>) -> String {
    let text = node
        .descendants()
        .filter(|child| child.is_text())
        .filter_map(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    super::normalize_text(&text)
}

fn collect_meta_content(node: roxmltree::Node<'_, '_>) -> String {
    let value = node
        .attribute("content")
        .or_else(|| node.attribute("value"))
        .unwrap_or_default();
    super::normalize_text(value)
}

fn collect_table_content(
    node: roxmltree::Node<'_, '_>,
) -> (String, Option<String>, Option<usize>, Option<usize>) {
    let mut rows = Vec::new();
    for row in node
        .descendants()
        .filter(|child| child.is_element() && child.tag_name().name() == "tr")
    {
        let cells = row
            .children()
            .filter(|child| child.is_element() && matches!(child.tag_name().name(), "th" | "td"))
            .map(collect_node_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>();
        if !cells.is_empty() {
            rows.push(cells);
        }
    }

    let content = super::table_text_from_cells(&rows);
    let row_count = (!rows.is_empty()).then_some(rows.len());
    let column_count = rows.iter().map(Vec::len).max();
    let payload = super::table_structured_payload(&rows);
    (content, payload, row_count, column_count)
}

fn collect_table_label(node: roxmltree::Node<'_, '_>) -> Option<String> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == "caption")
        .map(collect_node_text)
        .filter(|text| !text.is_empty())
}

fn collect_section_label(node: roxmltree::Node<'_, '_>) -> Option<String> {
    node.children()
        .find(|child| {
            child.is_element()
                && matches!(
                    child.tag_name().name(),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                )
        })
        .map(collect_node_text)
        .filter(|text| !text.is_empty())
}

fn collect_list_text(node: roxmltree::Node<'_, '_>) -> String {
    let ordered = node.tag_name().name() == "ol";
    let items = node
        .children()
        .filter(|child| child.is_element() && child.tag_name().name() == "li")
        .enumerate()
        .filter_map(|(idx, child)| {
            let text = collect_node_text(child);
            if text.is_empty() {
                None
            } else if ordered {
                Some(format!("{}. {}", idx + 1, text))
            } else {
                Some(format!("- {}", text))
            }
        })
        .collect::<Vec<_>>();
    super::normalize_text(&items.join("\n"))
}

fn collect_definition_list_text(node: roxmltree::Node<'_, '_>) -> String {
    let mut entries = Vec::new();
    let mut current_term: Option<String> = None;
    for child in node.children().filter(|child| child.is_element()) {
        match child.tag_name().name() {
            "dt" => {
                current_term = Some(collect_node_text(child));
            }
            "dd" => {
                let value = collect_node_text(child);
                if value.is_empty() {
                    continue;
                }
                if let Some(term) = current_term.take().filter(|term| !term.is_empty()) {
                    entries.push(format!("{term}: {value}"));
                } else {
                    entries.push(value);
                }
            }
            _ => {}
        }
    }
    super::normalize_text(&entries.join("\n"))
}

fn collect_outline_text(node: roxmltree::Node<'_, '_>) -> String {
    let value = node
        .attribute("text")
        .or_else(|| node.attribute("title"))
        .or_else(|| node.attribute("label"))
        .unwrap_or_default();
    let text = super::normalize_text(value);
    if !text.is_empty() {
        return text;
    }
    collect_node_text(node)
}

fn collect_image_alt_text(node: roxmltree::Node<'_, '_>) -> String {
    let text = node
        .attribute("alt")
        .or_else(|| node.attribute("title"))
        .unwrap_or_default();
    super::normalize_text(text)
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
