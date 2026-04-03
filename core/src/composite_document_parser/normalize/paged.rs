use crate::document_parser::DocumentBlock;

use super::{
    looks_like_heading, normalize_text,
    text::{
        align_ragged_row_cells, alignment_for_missing_column, infer_multi_column_count,
        infer_reference_column_gaps, is_probable_multi_column_layout, merged_reference_gaps,
        should_dehyphenate_line_join, split_aligned_columns, split_aligned_columns_with_gaps,
        starts_with_lowercase_word, AlignedTextRow,
    },
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct NormalizedPagedText {
    pub(super) text: String,
    pub(super) continued_from_previous_page: bool,
    pub(super) continued_to_next_page: bool,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn normalize_paged_text_boundaries(pages: Vec<String>) -> Vec<String> {
    normalize_paged_text_pages(pages)
        .into_iter()
        .map(|page| page.text)
        .collect()
}

pub(super) fn normalize_paged_text_pages(pages: Vec<String>) -> Vec<NormalizedPagedText> {
    let mut pages = pages
        .into_iter()
        .map(|text| NormalizedPagedText {
            text,
            continued_from_previous_page: false,
            continued_to_next_page: false,
        })
        .collect::<Vec<_>>();

    for idx in 0..pages.len().saturating_sub(1) {
        if let Some(consumed_lines) =
            page_boundary_multi_column_carry(&pages[idx].text, &pages[idx + 1].text)
        {
            let carried = next_non_empty_lines(&pages[idx + 1].text, consumed_lines);
            pages[idx].text = append_page_lines(&pages[idx].text, &carried);
            pages[idx + 1].text =
                remove_first_n_non_empty_lines(&pages[idx + 1].text, consumed_lines);
            pages[idx].continued_to_next_page = true;
            pages[idx + 1].continued_from_previous_page = true;
        }

        let Some((carry, consumed_lines, join_without_space)) =
            page_boundary_carry(&pages[idx].text, &pages[idx + 1].text)
        else {
            continue;
        };

        pages[idx].text = append_to_last_line(&pages[idx].text, &carry, join_without_space);
        pages[idx + 1].text = remove_first_n_non_empty_lines(&pages[idx + 1].text, consumed_lines);
        pages[idx].continued_to_next_page = true;
        pages[idx + 1].continued_from_previous_page = true;
    }

    pages
        .into_iter()
        .filter(|page| !page.text.trim().is_empty())
        .collect()
}

pub(super) fn page_boundary_multi_column_carry(current: &str, next: &str) -> Option<usize> {
    let current_tail_lines = trailing_aligned_lines(current);
    let next_head_lines = leading_aligned_lines(next);
    if current_tail_lines.is_empty() || next_head_lines.is_empty() {
        return None;
    }

    let current_rows = current_tail_lines
        .iter()
        .map(|line| split_aligned_columns_with_gaps(line))
        .collect::<Vec<_>>();
    let next_rows = next_head_lines
        .iter()
        .map(|line| split_aligned_columns_with_gaps(line))
        .collect::<Vec<_>>();
    let combined_rows = current_rows
        .iter()
        .chain(next_rows.iter())
        .map(|row| row.cells.clone())
        .collect::<Vec<_>>();
    let column_count = infer_multi_column_count(&combined_rows)?;
    if !is_probable_multi_column_layout(&combined_rows, column_count) {
        return None;
    }

    let reference_gaps = infer_reference_column_gaps(
        &current_rows
            .iter()
            .chain(next_rows.iter())
            .cloned()
            .collect::<Vec<_>>(),
        column_count,
    );
    let mut open_columns =
        infer_open_columns_from_rows(&current_rows, column_count, reference_gaps.as_deref());
    if open_columns.is_empty() {
        return None;
    }

    let mut consumed = 0usize;
    for row in &next_rows {
        let aligned = align_ragged_row_cells_for_open_columns(
            row,
            column_count,
            reference_gaps.as_deref(),
            &open_columns,
        );
        let mut touched = false;
        open_columns.retain(|idx| {
            let cell = aligned
                .get(*idx)
                .and_then(|cell| cell.as_deref())
                .map(str::trim)
                .unwrap_or_default();
            if cell.is_empty() {
                return true;
            }
            touched = true;
            !ends_with_sentence_boundary(cell)
        });
        if !touched {
            break;
        }
        consumed += 1;
        if open_columns.is_empty() {
            break;
        }
    }

    (consumed > 0).then_some(consumed)
}

pub(super) fn align_ragged_row_cells_for_open_columns(
    row: &AlignedTextRow,
    column_count: usize,
    reference_gaps: Option<&[usize]>,
    open_columns: &[usize],
) -> Vec<Option<String>> {
    let default = align_ragged_row_cells(row, column_count, reference_gaps);
    if open_columns.is_empty()
        || open_columns.iter().any(|idx| {
            default
                .get(*idx)
                .and_then(|cell: &Option<String>| cell.as_deref())
                .is_some_and(|cell: &str| !cell.trim().is_empty())
        })
    {
        return default;
    }

    let Some(reference_gaps) = reference_gaps else {
        return default;
    };
    let Some(best) = best_open_column_alignment(row, column_count, reference_gaps, open_columns)
    else {
        return default;
    };
    best
}

pub(super) fn best_open_column_alignment(
    row: &AlignedTextRow,
    column_count: usize,
    reference_gaps: &[usize],
    open_columns: &[usize],
) -> Option<Vec<Option<String>>> {
    if row.cells.len() + 1 != column_count || row.gaps.len() + 1 != row.cells.len() {
        return None;
    }

    let mut best: Option<(usize, usize, usize, Vec<Option<String>>)> = None;
    for missing in 0..column_count {
        let expected = merged_reference_gaps(column_count, missing, reference_gaps)?;
        if expected.len() != row.gaps.len() {
            continue;
        }

        let alignment = alignment_for_missing_column(row, column_count, missing);
        let open_hits = open_columns
            .iter()
            .filter(|idx| {
                alignment
                    .get(**idx)
                    .and_then(|cell: &Option<String>| cell.as_deref())
                    .is_some_and(|cell: &str| !cell.trim().is_empty())
            })
            .count();
        let rightmost_hit = open_columns
            .iter()
            .copied()
            .filter(|idx| {
                alignment
                    .get(*idx)
                    .and_then(|cell: &Option<String>| cell.as_deref())
                    .is_some_and(|cell: &str| !cell.trim().is_empty())
            })
            .max()
            .unwrap_or(0);
        let gap_score = row
            .gaps
            .iter()
            .zip(expected.iter())
            .map(|(actual, expected): (&usize, &usize)| actual.abs_diff(*expected))
            .sum::<usize>();

        match &best {
            Some((best_hits, best_rightmost, best_gap_score, _))
                if open_hits < *best_hits
                    || (open_hits == *best_hits && rightmost_hit < *best_rightmost)
                    || (open_hits == *best_hits
                        && rightmost_hit == *best_rightmost
                        && gap_score >= *best_gap_score) => {}
            _ => best = Some((open_hits, rightmost_hit, gap_score, alignment)),
        }
    }

    best.map(|(_, _, _, alignment)| alignment)
}

pub(super) fn infer_open_columns_from_rows(
    rows: &[AlignedTextRow],
    column_count: usize,
    reference_gaps: Option<&[usize]>,
) -> Vec<usize> {
    let mut open = Vec::new();
    for column_idx in 0..column_count {
        let latest: Option<String> = rows.iter().rev().find_map(|row| {
            let aligned = align_ragged_row_cells(row, column_count, reference_gaps);
            aligned
                .get(column_idx)
                .and_then(|cell: &Option<String>| cell.as_deref())
                .map(str::trim)
                .filter(|cell: &&str| !cell.is_empty())
                .map(str::to_string)
        });

        let Some(cell) = latest else {
            continue;
        };
        if ends_with_sentence_boundary(&cell) {
            continue;
        }
        if looks_like_heading(&cell)
            && cell.split_whitespace().count() <= 3
            && !starts_with_lowercase_word(&cell)
        {
            continue;
        }
        open.push(column_idx);
    }
    open
}

pub(super) fn page_boundary_carry(current: &str, next: &str) -> Option<(String, usize, bool)> {
    let current_last = current
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())?
        .trim()
        .to_string();
    let next_lines = next
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let next_first = next_lines.first()?.clone();

    if split_aligned_columns(&current_last).len() >= 2
        || split_aligned_columns(&next_first).len() >= 2
    {
        return None;
    }

    if current_last.ends_with('-') && starts_with_lowercase_word(&next_first) {
        let consumed = collect_page_carry_lines(&next_lines);
        let carry = join_carry_lines(&consumed);
        return Some((carry, consumed.len(), true));
    }

    if !ends_with_sentence_boundary(&current_last) && starts_with_lowercase_word(&next_first) {
        let consumed = collect_page_carry_lines(&next_lines);
        let carry = join_carry_lines(&consumed);
        return Some((carry, consumed.len(), false));
    }

    None
}

pub(super) fn append_to_last_line(page: &str, carry: &str, join_without_space: bool) -> String {
    let mut lines = page.lines().map(str::to_string).collect::<Vec<_>>();
    if let Some(last_idx) = lines.iter().rposition(|line| !line.trim().is_empty()) {
        if join_without_space && lines[last_idx].ends_with('-') {
            lines[last_idx].pop();
            lines[last_idx].push_str(carry.trim());
        } else {
            lines[last_idx].push(' ');
            lines[last_idx].push_str(carry.trim());
        }
    }
    lines.join("\n")
}

pub(super) fn append_page_lines(page: &str, extra_lines: &[String]) -> String {
    let mut lines = page.lines().map(str::to_string).collect::<Vec<_>>();
    lines.extend(extra_lines.iter().cloned());
    lines.join("\n")
}

pub(super) fn remove_first_n_non_empty_lines(page: &str, count: usize) -> String {
    let mut removed = 0usize;
    page.lines()
        .filter(|line| {
            if removed < count && !line.trim().is_empty() {
                removed += 1;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn next_non_empty_lines(page: &str, count: usize) -> Vec<String> {
    page.lines()
        .filter(|line| !line.trim().is_empty())
        .take(count)
        .map(str::to_string)
        .collect()
}

pub(super) fn trailing_aligned_lines(page: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in page.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        if split_aligned_columns(trimmed).len() < 2 {
            break;
        }
        lines.push(trimmed.to_string());
    }
    lines.reverse();
    lines
}

pub(super) fn leading_aligned_lines(page: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in page.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !lines.is_empty() {
                break;
            }
            continue;
        }
        if split_aligned_columns(trimmed).len() < 2 {
            break;
        }
        lines.push(trimmed.to_string());
    }
    lines
}

fn collect_page_carry_lines(lines: &[String]) -> Vec<String> {
    let mut consumed = Vec::new();
    for line in lines {
        consumed.push(line.clone());
        let previous = consumed.last().expect("line was just pushed");
        if ends_with_sentence_boundary(previous) {
            break;
        }
    }
    consumed
}

fn join_carry_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for line in lines {
        if !out.is_empty() {
            if should_dehyphenate_line_join(&out, line) {
                out.pop();
            } else {
                out.push(' ');
            }
        }
        out.push_str(line.trim());
    }
    normalize_text(&out)
}

pub(super) fn ends_with_sentence_boundary(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.ends_with('.')
        || trimmed.ends_with('!')
        || trimmed.ends_with('?')
        || trimmed.ends_with(':')
        || trimmed.ends_with(';')
}

pub(super) fn label_paged_block(
    mut block: DocumentBlock,
    page: usize,
    page_block_index: usize,
) -> DocumentBlock {
    if block.label.is_some() {
        let existing = block.label.take().unwrap_or_default();
        block.label = Some(format!("page {page}: {existing}"));
        return block;
    }

    block.label = Some(match block.kind {
        crate::document_parser::DocumentBlockKind::Heading => format!("page {page}: heading"),
        crate::document_parser::DocumentBlockKind::Section => format!("page {page}: section"),
        crate::document_parser::DocumentBlockKind::Table => format!("page {page}: table"),
        _ => format!("page {page}: block {page_block_index}"),
    });
    block
}
