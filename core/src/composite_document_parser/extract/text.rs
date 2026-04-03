use anyhow::Result;

use crate::document_parser::{DocumentBlock, DocumentBlockKind, ParsedDocument};

pub(super) fn parse_plain_text_document(path: &std::path::Path) -> Result<ParsedDocument> {
    super::extract_structured::parse_plain_text_document(path)
}

pub(super) fn parse_ipynb(path: &std::path::Path) -> Result<ParsedDocument> {
    super::extract_structured::parse_ipynb(path)
}

pub(super) fn parsed_text_document(
    path: &std::path::Path,
    text: String,
    default_kind: DocumentBlockKind,
) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    let source = doc
        .title
        .clone()
        .unwrap_or_else(|| path.display().to_string());
    for (idx, block) in text_blocks(&text, default_kind).into_iter().enumerate() {
        doc.push(block.with_source(source.clone()).with_ordinal(idx + 1));
    }
    super::ensure_document(doc, path)
}

pub(super) fn parsed_paged_text_document(
    path: &std::path::Path,
    text: String,
    default_kind: DocumentBlockKind,
) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    let source = doc
        .title
        .clone()
        .unwrap_or_else(|| path.display().to_string());

    let pages = super::normalize_paged_text_pages(split_paged_text(&text));
    let has_explicit_pages = pages.len() > 1;
    let mut ordinal = 1usize;
    for (page_index, page_text) in pages.into_iter().enumerate() {
        let page = page_index + 1;
        let blocks = paged_text_blocks(&page_text.text, default_kind.clone());
        let block_count = blocks.len();
        for (page_block_index, block) in blocks.into_iter().enumerate() {
            let is_first = page_block_index == 0;
            let is_last = page_block_index + 1 == block_count;
            let mut block = super::label_paged_block(block, page, page_block_index + 1)
                .with_source(source.clone())
                .with_ordinal(ordinal);
            if has_explicit_pages || !page_text.text.trim().is_empty() {
                block = block.with_page(page);
            }
            if is_first && page_text.continued_from_previous_page {
                block = block.with_continued_from_previous_page(true);
            }
            if is_last && page_text.continued_to_next_page {
                block = block.with_continued_to_next_page(true);
            }
            doc.push(block);
            ordinal += 1;
        }
    }

    super::ensure_document(doc, path)
}

pub(super) fn parsed_structured_text_document(
    path: &std::path::Path,
    blocks: Vec<DocumentBlock>,
) -> Result<ParsedDocument> {
    let mut doc = ParsedDocument::new();
    doc.title = super::file_title(path);
    let source = doc
        .title
        .clone()
        .unwrap_or_else(|| path.display().to_string());
    for (idx, block) in blocks.into_iter().enumerate() {
        doc.push(block.with_source(source.clone()).with_ordinal(idx + 1));
    }
    super::ensure_document(doc, path)
}

pub(super) fn fallback_text_blocks(text: &str) -> Vec<DocumentBlock> {
    text_blocks(text, DocumentBlockKind::Paragraph)
}

pub(super) fn paged_text_blocks(text: &str, default_kind: DocumentBlockKind) -> Vec<DocumentBlock> {
    // First, handle page breaks from lopdf extraction
    let pages = text
        .split("[_PAGE_BREAK_]")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();

    if pages.len() > 1 {
        // Multi-page document: process each page separately
        return pages
            .into_iter()
            .flat_map(|page| paged_text_blocks(page, default_kind.clone()))
            .collect();
    }

    // Single page or after page split: handle table row markers
    let blocks = process_table_row_markers(text, default_kind.clone());
    if !blocks.is_empty() {
        return blocks;
    }

    // Fall back to regular chunk processing
    split_paged_chunks(text)
        .into_iter()
        .flat_map(|chunk| {
            if let Some(block) = parse_table_block(&chunk) {
                vec![block]
            } else if let Some(blocks) =
                parse_multi_column_layout_blocks(&chunk, default_kind.clone())
            {
                blocks
            } else {
                chunk_to_blocks(&chunk, default_kind.clone())
            }
        })
        .collect()
}

/// Process text with [_TABLE_ROW_] markers from lopdf position-aware extraction.
///
/// Groups consecutive table rows into a single table block.
/// Returns empty Vec if no [_TABLE_ROW_] markers were found, allowing
/// normal multi-column layout detection to proceed.
fn process_table_row_markers(text: &str, default_kind: DocumentBlockKind) -> Vec<DocumentBlock> {
    // Fast path: if no table row markers exist, return empty to use normal processing
    if !text.contains("[_TABLE_ROW_]") {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut current_text = String::new();
    let mut table_rows: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[_TABLE_ROW_]") && trimmed.ends_with("[_TABLE_ROW_]") {
            // Extract the actual row content
            let row_content = trimmed
                .strip_prefix("[_TABLE_ROW_]")
                .and_then(|s| s.strip_suffix("[_TABLE_ROW_]"))
                .unwrap_or(trimmed)
                .trim();

            if !row_content.is_empty() {
                table_rows.push(row_content.to_string());
            }
        } else if !table_rows.is_empty() {
            // End of table rows - flush the accumulated table
            if let Some(table_block) = build_table_from_rows(&table_rows) {
                result.push(table_block);
            }
            table_rows.clear();
            // Also flush any pending text
            if !current_text.trim().is_empty() {
                result.extend(chunk_to_blocks(current_text.trim(), default_kind.clone()));
                current_text.clear();
            }
            // Process this non-table line
            if !trimmed.is_empty() {
                current_text.push_str(trimmed);
                current_text.push('\n');
            }
        } else {
            // Normal text, accumulate
            if !trimmed.is_empty() {
                current_text.push_str(trimmed);
                current_text.push('\n');
            }
        }
    }

    // Flush remaining table rows
    if !table_rows.is_empty() {
        if let Some(table_block) = build_table_from_rows(&table_rows) {
            result.push(table_block);
        }
    }

    // Flush remaining text
    if !current_text.trim().is_empty() {
        result.extend(chunk_to_blocks(current_text.trim(), default_kind));
    }

    result
}

pub(super) fn text_blocks(text: &str, default_kind: DocumentBlockKind) -> Vec<DocumentBlock> {
    let normalized = super::normalize_text(text);
    normalized
        .split("\n\n")
        .filter_map(|chunk| {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                return None;
            }

            let kind = if super::looks_like_heading(chunk) {
                DocumentBlockKind::Heading
            } else {
                default_kind.clone()
            };
            Some(DocumentBlock::new(kind, None::<String>, chunk))
        })
        .collect()
}

pub(super) fn parse_delimited_blocks(text: &str, delimiter: char) -> Vec<DocumentBlock> {
    super::extract_structured::parse_delimited_blocks(text, delimiter)
}

pub(super) fn parse_json_lines_blocks(text: &str) -> Vec<DocumentBlock> {
    super::extract_structured::parse_json_lines_blocks(text)
}

pub(super) fn parse_json_document_blocks(text: &str) -> Vec<DocumentBlock> {
    super::extract_structured::parse_json_document_blocks(text)
}

pub(super) fn parse_yaml_document_blocks(text: &str) -> Vec<DocumentBlock> {
    super::extract_structured::parse_yaml_document_blocks(text)
}

pub(super) fn parse_toml_document_blocks(text: &str) -> Vec<DocumentBlock> {
    super::extract_structured::parse_toml_document_blocks(text)
}

pub(super) fn split_paged_text(text: &str) -> Vec<String> {
    let pages = text
        .replace("\r\n", "\n")
        .split('\u{000c}')
        .map(|page| page.trim().to_string())
        .filter(|page| !page.trim().is_empty())
        .collect::<Vec<_>>();

    if pages.is_empty() {
        vec![text.replace("\r\n", "\n").trim().to_string()]
    } else {
        pages
    }
}

fn split_paged_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = Vec::new();

    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                chunks.push(current.join("\n"));
                current.clear();
            }
            continue;
        }
        current.push(line.to_string());
    }

    if !current.is_empty() {
        chunks.push(current.join("\n"));
    }

    chunks
}

fn chunk_to_blocks(chunk: &str, default_kind: DocumentBlockKind) -> Vec<DocumentBlock> {
    let paragraph_chunks = chunk
        .split("\n\n")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if paragraph_chunks.len() > 1 {
        return paragraph_chunks
            .into_iter()
            .flat_map(|part| chunk_to_blocks(part, default_kind.clone()))
            .collect();
    }

    let normalized = super::normalize_text(chunk);
    if normalized.trim().is_empty() {
        return Vec::new();
    }

    let Some((heading, body)) = split_heading_and_body(&normalized) else {
        let kind = if super::looks_like_heading(&normalized) {
            DocumentBlockKind::Heading
        } else {
            default_kind
        };
        return vec![DocumentBlock::new(kind, None::<String>, normalized)];
    };

    let mut blocks = vec![DocumentBlock::new(
        DocumentBlockKind::Heading,
        None::<String>,
        heading.clone(),
    )];
    if !body.trim().is_empty() {
        blocks.push(DocumentBlock::new(
            DocumentBlockKind::Section,
            Some(heading),
            body,
        ));
    }
    blocks
}

fn split_heading_and_body(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    let heading = lines.next()?.trim().to_string();
    if heading.is_empty() || !super::looks_like_heading(&heading) {
        return None;
    }

    let rest = lines.collect::<Vec<_>>().join("\n");
    let body = rest.trim().to_string();
    if body.is_empty() || super::looks_like_heading(&body) {
        return None;
    }

    Some((heading, body))
}

fn parse_table_block(text: &str) -> Option<DocumentBlock> {
    parse_pipe_delimited_table_block(text).or_else(|| parse_aligned_table_block(text))
}

fn parse_pipe_delimited_table_block(text: &str) -> Option<DocumentBlock> {
    let mut rows = Vec::new();
    let mut saw_pipe_layout = false;

    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if !(line.contains('|') || line.contains('¦')) {
            return None;
        }

        let normalized_line = line.replace('¦', "|");
        saw_pipe_layout |= line.starts_with('|')
            || line.ends_with('|')
            || normalized_line.matches('|').count() >= 2;
        let cells = normalized_line
            .split('|')
            .map(str::trim)
            .filter(|cell| !cell.is_empty())
            .map(super::normalize_text)
            .filter(|cell| !cell.is_empty())
            .collect::<Vec<_>>();

        if cells.len() < 2 {
            return None;
        }
        if is_pipe_table_separator_row(&cells) {
            continue;
        }
        rows.push(cells);
    }

    if rows.len() < 2 || !saw_pipe_layout {
        return None;
    }

    let expected_columns = rows[0].len();
    if expected_columns < 2 || rows.iter().any(|row| row.len() != expected_columns) {
        return None;
    }

    let row_count = rows.len();
    let column_count = expected_columns;
    let content = super::table_text_from_cells(&rows);
    let payload = super::table_structured_payload(&rows)?;

    Some(
        DocumentBlock::new(DocumentBlockKind::Table, Some("table"), content)
            .with_attribute("row_count", row_count.to_string())
            .with_attribute("column_count", column_count.to_string())
            .with_structured_payload(payload),
    )
}

fn is_pipe_table_separator_row(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let trimmed = cell.trim();
            !trimmed.is_empty()
                && trimmed
                    .chars()
                    .all(|ch| matches!(ch, '-' | ':' | '=' | '+' | ' '))
        })
}

fn parse_aligned_table_block(text: &str) -> Option<DocumentBlock> {
    let rows = text.lines().map(split_aligned_columns).collect::<Vec<_>>();

    if rows.len() < 2 {
        return None;
    }
    if rows.iter().any(|row| row.len() < 2) {
        return None;
    }

    let expected_columns = rows[0].len();
    if expected_columns < 2 || rows.iter().any(|row| row.len() != expected_columns) {
        return None;
    }
    if is_probable_multi_column_layout(&rows, expected_columns) {
        return None;
    }

    let row_count = rows.len();
    let column_count = expected_columns;
    let content = super::table_text_from_cells(&rows);
    let payload = super::table_structured_payload(&rows)?;

    Some(
        DocumentBlock::new(DocumentBlockKind::Table, Some("table"), content)
            .with_attribute("row_count", row_count.to_string())
            .with_attribute("column_count", column_count.to_string())
            .with_structured_payload(payload),
    )
}

fn parse_multi_column_layout_blocks(
    text: &str,
    default_kind: DocumentBlockKind,
) -> Option<Vec<DocumentBlock>> {
    let aligned_rows = text
        .lines()
        .map(split_aligned_columns_with_gaps)
        .collect::<Vec<_>>();
    let rows = aligned_rows
        .iter()
        .map(|row| row.cells.clone())
        .collect::<Vec<_>>();
    let column_count = infer_multi_column_count(&rows)?;
    if !is_probable_multi_column_layout(&rows, column_count) {
        return None;
    }

    let mut columns = vec![Vec::new(); column_count];
    let reference_gaps = infer_reference_column_gaps(&aligned_rows, column_count);
    for row in aligned_rows {
        let aligned = align_ragged_row_cells(&row, column_count, reference_gaps.as_deref());
        for (idx, cell) in aligned.into_iter().enumerate() {
            if let Some(column) = columns.get_mut(idx) {
                column.push(cell.unwrap_or_default());
            }
        }
    }

    Some(
        columns
            .into_iter()
            .flat_map(|column| {
                let reflowed = reflow_column_text(&column);
                if reflowed.is_empty() {
                    Vec::new()
                } else {
                    chunk_to_blocks(&reflowed, default_kind.clone())
                }
            })
            .collect::<Vec<_>>(),
    )
    .filter(|blocks| !blocks.is_empty())
}

#[derive(Debug, Clone)]
pub(super) struct AlignedTextRow {
    pub(super) cells: Vec<String>,
    pub(super) gaps: Vec<usize>,
}

pub(super) fn infer_multi_column_count(rows: &[Vec<String>]) -> Option<usize> {
    let mut counts = [0usize; 5];
    for row in rows {
        let len = row.len();
        if (2..=4).contains(&len) {
            counts[len] += 1;
        }
    }

    let (column_count, seen) = counts
        .iter()
        .enumerate()
        .skip(2)
        .max_by(|(idx_a, count_a), (idx_b, count_b)| count_a.cmp(count_b).then(idx_a.cmp(idx_b)))
        .map(|(idx, count)| (idx, *count))?;
    if seen >= 2 {
        return Some(column_count);
    }

    let first_len = rows.first().map(Vec::len)?;
    (rows.len() >= 2 && first_len == column_count && (2..=4).contains(&column_count))
        .then_some(column_count)
}

pub(super) fn infer_reference_column_gaps(
    rows: &[AlignedTextRow],
    column_count: usize,
) -> Option<Vec<usize>> {
    let complete_rows = rows
        .iter()
        .filter(|row| row.cells.len() == column_count && row.gaps.len() + 1 == column_count)
        .collect::<Vec<_>>();
    if complete_rows.is_empty() {
        return None;
    }

    let mut totals = vec![0usize; column_count.saturating_sub(1)];
    for row in &complete_rows {
        for (idx, gap) in row.gaps.iter().enumerate() {
            totals[idx] += *gap;
        }
    }

    Some(
        totals
            .into_iter()
            .map(|total| total / complete_rows.len())
            .collect(),
    )
}

pub(super) fn align_ragged_row_cells(
    row: &AlignedTextRow,
    column_count: usize,
    reference_gaps: Option<&[usize]>,
) -> Vec<Option<String>> {
    if row.cells.len() >= column_count {
        return row
            .cells
            .iter()
            .take(column_count)
            .cloned()
            .map(Some)
            .collect();
    }

    if row.cells.len() + 1 == column_count && column_count >= 3 {
        if let Some(reference_gaps) = reference_gaps {
            if let Some(aligned) =
                align_single_missing_column_row(row, column_count, reference_gaps)
            {
                return aligned;
            }
        }
    }

    let mut aligned = row.cells.iter().cloned().map(Some).collect::<Vec<_>>();
    aligned.resize(column_count, None);
    aligned
}

pub(super) fn align_single_missing_column_row(
    row: &AlignedTextRow,
    column_count: usize,
    reference_gaps: &[usize],
) -> Option<Vec<Option<String>>> {
    if row.cells.len() + 1 != column_count || row.gaps.len() + 1 != row.cells.len() {
        return None;
    }

    let mut best: Option<(usize, usize)> = None;
    for missing in 0..column_count {
        let expected = merged_reference_gaps(column_count, missing, reference_gaps)?;
        if expected.len() != row.gaps.len() {
            continue;
        }
        let score = row
            .gaps
            .iter()
            .zip(expected.iter())
            .map(|(actual, expected)| actual.abs_diff(*expected))
            .sum::<usize>();
        match best {
            Some((best_score, _)) if score >= best_score => {}
            _ => best = Some((score, missing)),
        }
    }

    let (_, missing) = best?;
    Some(alignment_for_missing_column(row, column_count, missing))
}

pub(super) fn alignment_for_missing_column(
    row: &AlignedTextRow,
    column_count: usize,
    missing: usize,
) -> Vec<Option<String>> {
    let mut aligned = Vec::with_capacity(column_count);
    let mut cell_iter = row.cells.iter().cloned();
    for idx in 0..column_count {
        if idx == missing {
            aligned.push(None);
        } else {
            aligned.push(cell_iter.next().map(Some).unwrap_or(None));
        }
    }
    aligned
}

pub(super) fn merged_reference_gaps(
    column_count: usize,
    missing: usize,
    reference_gaps: &[usize],
) -> Option<Vec<usize>> {
    if reference_gaps.len() + 1 != column_count {
        return None;
    }
    if column_count < 3 {
        return None;
    }

    let surviving = (0..column_count)
        .filter(|idx| *idx != missing)
        .collect::<Vec<_>>();
    if surviving.len() < 2 {
        return None;
    }

    let mut merged = Vec::with_capacity(surviving.len() - 1);
    for pair in surviving.windows(2) {
        let start = pair[0];
        let end = pair[1];
        let gap = reference_gaps[start..end].iter().sum::<usize>();
        merged.push(gap);
    }
    Some(merged)
}

pub(super) fn is_probable_multi_column_layout(rows: &[Vec<String>], column_count: usize) -> bool {
    if rows.len() < 2 || !(2..=4).contains(&column_count) {
        return false;
    }

    let compatible_rows = rows
        .iter()
        .filter(|row| row.len() >= 2 && row.len() <= column_count)
        .count();
    if compatible_rows < 2 || compatible_rows * 2 < rows.len() {
        return false;
    }

    let non_empty_cells = rows
        .iter()
        .filter(|row| row.len() >= 2 && row.len() <= column_count)
        .flat_map(|row| row.iter().take(column_count))
        .map(|cell| cell.trim())
        .filter(|cell| !cell.is_empty())
        .collect::<Vec<_>>();
    if non_empty_cells.len() < (column_count + 2).max(4) {
        return false;
    }

    let total_chars: usize = non_empty_cells
        .iter()
        .map(|cell| cell.chars().count())
        .sum();
    if total_chars < 60 {
        return false;
    }

    let total_words: usize = non_empty_cells
        .iter()
        .map(|cell| cell.split_whitespace().count())
        .sum();
    let avg_chars = total_chars as f64 / non_empty_cells.len() as f64;
    let avg_words = total_words as f64 / non_empty_cells.len() as f64;
    if avg_chars < 12.0 || avg_words < 3.0 {
        return false;
    }

    let digit_count = non_empty_cells
        .iter()
        .flat_map(|cell| cell.chars())
        .filter(|ch| ch.is_ascii_digit())
        .count();
    let alpha_count = non_empty_cells
        .iter()
        .flat_map(|cell| cell.chars())
        .filter(|ch| ch.is_alphabetic())
        .count();
    if alpha_count == 0 {
        return false;
    }

    let digit_ratio = digit_count as f64 / (digit_count + alpha_count) as f64;
    if digit_ratio > 0.12 {
        return false;
    }

    let punctuated_cells = non_empty_cells
        .iter()
        .filter(|cell| {
            cell.ends_with('.')
                || cell.ends_with(',')
                || cell.ends_with(';')
                || cell.ends_with(':')
                || cell.ends_with(')')
        })
        .count();

    punctuated_cells > 0 || non_empty_cells.iter().any(|cell| cell.contains(" the "))
}

fn reflow_column_text(lines: &[String]) -> String {
    let lines = lines
        .iter()
        .map(|line| super::normalize_text(line))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return String::new();
    }

    if lines.len() >= 2
        && super::looks_like_heading(&lines[0])
        && !starts_with_lowercase_word(&lines[1])
    {
        let body = reflow_column_paragraphs(&lines[1..]).join("\n\n");
        if body.trim().is_empty() {
            return lines[0].clone();
        }
        return format!("{}\n{}", lines[0], body);
    }

    reflow_column_paragraphs(&lines).join("\n\n")
}

pub(super) fn starts_with_lowercase_word(text: &str) -> bool {
    text.trim()
        .chars()
        .find(|ch| ch.is_alphabetic())
        .is_some_and(|ch| ch.is_lowercase())
}

fn reflow_column_paragraphs(lines: &[String]) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();

    for (idx, line) in lines.iter().enumerate() {
        if !current.is_empty() {
            if should_dehyphenate_line_join(&current, line) {
                current.pop();
            } else {
                current.push(' ');
            }
        }
        current.push_str(line);

        let next = lines.get(idx + 1).map(String::as_str);
        if should_break_column_paragraph(line, next) {
            let paragraph = super::normalize_text(&current);
            if !paragraph.is_empty() {
                paragraphs.push(paragraph);
            }
            current.clear();
        }
    }

    if !current.trim().is_empty() {
        let paragraph = super::normalize_text(&current);
        if !paragraph.is_empty() {
            paragraphs.push(paragraph);
        }
    }

    paragraphs
}

pub(super) fn should_dehyphenate_line_join(current: &str, next: &str) -> bool {
    current.ends_with('-')
        && next
            .trim()
            .chars()
            .find(|ch| ch.is_alphabetic())
            .is_some_and(|ch| ch.is_lowercase())
}

fn should_break_column_paragraph(current: &str, next: Option<&str>) -> bool {
    let Some(next) = next else {
        return true;
    };

    let current = current.trim();
    let next = next.trim();
    if current.is_empty() || next.is_empty() {
        return true;
    }

    let next_starts_upper = next
        .chars()
        .find(|ch| ch.is_alphabetic())
        .is_some_and(|ch| ch.is_uppercase());

    (current.ends_with('.') || current.ends_with('!') || current.ends_with('?'))
        && next_starts_upper
}

pub(super) fn split_aligned_columns(line: &str) -> Vec<String> {
    split_aligned_columns_with_gaps(line).cells
}

pub(super) fn split_aligned_columns_with_gaps(line: &str) -> AlignedTextRow {
    let mut columns = Vec::new();
    let mut gaps = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut saw_separator = false;

    while let Some(ch) = chars.next() {
        if ch == '\t' {
            saw_separator = true;
            if !current.trim().is_empty() {
                columns.push(current.trim().to_string());
                gaps.push(1);
            }
            current.clear();
            while chars.peek() == Some(&'\t') {
                chars.next();
            }
            continue;
        }

        if ch == ' ' {
            let mut space_count = 1usize;
            while chars.peek() == Some(&' ') {
                chars.next();
                space_count += 1;
            }
            if space_count >= 2 {
                saw_separator = true;
                if !current.trim().is_empty() {
                    columns.push(current.trim().to_string());
                    gaps.push(space_count);
                }
                current.clear();
                continue;
            }
            current.push(' ');
            continue;
        }

        current.push(ch);
    }

    if !current.trim().is_empty() {
        columns.push(current.trim().to_string());
    }

    if !columns.is_empty() && gaps.len() >= columns.len() {
        gaps.truncate(columns.len().saturating_sub(1));
    }

    if saw_separator {
        AlignedTextRow {
            cells: columns,
            gaps,
        }
    } else {
        AlignedTextRow {
            cells: vec![line.trim().to_string()],
            gaps: Vec::new(),
        }
    }
}

/// Build a table DocumentBlock from parsed table rows.
fn build_table_from_rows(rows: &[String]) -> Option<DocumentBlock> {
    if rows.len() < 2 {
        return None;
    }

    // Try to parse each row as tab or comma-separated
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    for row in rows {
        let cells: Vec<String> = if row.contains('\t') {
            row.split('\t')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        } else {
            // Try comma separation
            row.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        };

        if cells.len() >= 2 {
            table_rows.push(cells);
        }
    }

    if table_rows.len() < 2 {
        return None;
    }

    // Verify consistent column count
    let col_count = table_rows[0].len();
    if !table_rows.iter().all(|r| r.len() == col_count) {
        return None;
    }

    let row_count = table_rows.len();
    let column_count = col_count;
    let content = super::table_text_from_cells(&table_rows);
    let payload = super::table_structured_payload(&table_rows)?;

    Some(
        DocumentBlock::new(DocumentBlockKind::Table, Some("pdf-table"), content)
            .with_attribute("row_count", row_count.to_string())
            .with_attribute("column_count", column_count.to_string())
            .with_attribute("extraction", "lopdf-position-aware")
            .with_structured_payload(payload),
    )
}
