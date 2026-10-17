//! Literal grep widening used only after an exact search returns no hits.
//!
//! Case folding stays an exact `(?i)` search. Fuzzy matching is a separate
//! scorer: one transposition costs one typo, one to two characters allow no
//! typos, and a score under half of a contiguous match is discarded.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MATCH_POINT: i32 = 16;
const GAP_COST: i32 = 4;
const TYPO_COST: i32 = 16;
const MAX_FUZZY_HITS: usize = 20;
const MAX_FUZZY_FILES: usize = 8_000;
const MAX_FUZZY_FILE_BYTES: u64 = 1024 * 1024;
const MAX_LINE_CHARS: usize = 2_000;
pub(super) const FUZZY_TIME_BUDGET: Duration = Duration::from_millis(200);
const FUZZY_SCAN_STOPPED: &str =
    "fuzzy scan stopped before the workspace was finished; later files were not read.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FuzzyHit {
    pub score: i32,
}

pub(super) fn is_literal_query(query: &str) -> bool {
    !query.chars().any(|ch| {
        matches!(
            ch,
            '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        )
    })
}

pub(super) fn grep_is_empty(content: &str, matching_lines: Option<u64>) -> bool {
    if let Some(lines) = matching_lines {
        return lines == 0;
    }
    content.starts_with("No matches found")
}

/// Best in-limit fuzzy hit of `needle` inside `line`.
pub(super) fn find_fuzzy(needle: &str, line: &str, case_insensitive: bool) -> Option<FuzzyHit> {
    if !case_insensitive {
        let needle_lower = needle.to_ascii_lowercase();
        let line_lower = line.to_ascii_lowercase();
        // A case variant is not a typo. Explicit case-sensitive search stays exact.
        if line_lower.contains(&needle_lower) && !line.contains(needle) {
            return None;
        }
    }
    let needle: Vec<char> = needle.chars().collect();
    let line: Vec<char> = line.chars().take(MAX_LINE_CHARS).collect();
    let count = needle.len();
    if count == 0 || line.is_empty() {
        return None;
    }
    let max_typos = if count <= 2 { 0 } else { (count / 3).min(2) };
    let perfect = (count as i32).saturating_mul(MATCH_POINT);
    let floor = perfect / 2;
    // One alignment. A free start keeps a late hit; the score floor drops a span past 3× the query.
    align(&needle, &line, case_insensitive, max_typos, floor).map(|score| FuzzyHit { score })
}

fn align(
    needle: &[char],
    window: &[char],
    case_insensitive: bool,
    max_typos: usize,
    floor: i32,
) -> Option<i32> {
    let rows = needle.len() + 1;
    let cols = window.len() + 1;
    let mut score = vec![vec![i32::MIN / 4; cols]; rows];
    let mut typos = vec![vec![usize::MAX / 4; cols]; rows];
    for col in 0..cols {
        score[0][col] = 0;
        typos[0][col] = 0;
    }
    for row in 1..rows {
        score[row][0] = score[row - 1][0] - TYPO_COST;
        typos[row][0] = typos[row - 1][0].saturating_add(1);
    }
    for row in 1..rows {
        for col in 1..cols {
            let mut next_score = i32::MIN / 4;
            let mut next_typos = usize::MAX / 4;
            let consider = |next_score: &mut i32,
                            next_typos: &mut usize,
                            add: i32,
                            from_score: i32,
                            from_typos: usize,
                            extra_typos: usize| {
                if from_score <= i32::MIN / 8 {
                    return;
                }
                let typos = from_typos.saturating_add(extra_typos);
                let value = from_score.saturating_add(add);
                if typos < *next_typos || (typos == *next_typos && value > *next_score) {
                    *next_score = value;
                    *next_typos = typos;
                }
            };
            if same_char(needle[row - 1], window[col - 1], case_insensitive) {
                consider(
                    &mut next_score,
                    &mut next_typos,
                    MATCH_POINT,
                    score[row - 1][col - 1],
                    typos[row - 1][col - 1],
                    0,
                );
            } else {
                consider(
                    &mut next_score,
                    &mut next_typos,
                    -TYPO_COST,
                    score[row - 1][col - 1],
                    typos[row - 1][col - 1],
                    1,
                );
            }
            consider(
                &mut next_score,
                &mut next_typos,
                -TYPO_COST,
                score[row - 1][col],
                typos[row - 1][col],
                1,
            );
            consider(
                &mut next_score,
                &mut next_typos,
                -GAP_COST,
                score[row][col - 1],
                typos[row][col - 1],
                0,
            );
            if row > 1
                && col > 1
                && same_char(needle[row - 1], window[col - 2], case_insensitive)
                && same_char(needle[row - 2], window[col - 1], case_insensitive)
                && !same_char(needle[row - 1], window[col - 1], case_insensitive)
            {
                consider(
                    &mut next_score,
                    &mut next_typos,
                    MATCH_POINT,
                    score[row - 2][col - 2],
                    typos[row - 2][col - 2],
                    1,
                );
            }
            score[row][col] = next_score;
            typos[row][col] = next_typos;
        }
    }
    let mut best: Option<i32> = None;
    for col in 0..cols {
        if typos[rows - 1][col] <= max_typos
            && score[rows - 1][col] >= floor
            && best.is_none_or(|current| score[rows - 1][col] > current)
        {
            best = Some(score[rows - 1][col]);
        }
    }
    best
}

fn same_char(left: char, right: char, case_insensitive: bool) -> bool {
    if left == right {
        return true;
    }
    case_insensitive && left.eq_ignore_ascii_case(&right)
}

pub(super) fn fuzzy_scan(
    root: &Path,
    base: &Path,
    glob: Option<&str>,
    needle: &str,
    case_insensitive: bool,
    budget: Duration,
) -> Option<String> {
    fuzzy_scan_within(root, base, glob, needle, case_insensitive, budget)
}

fn fuzzy_scan_within(
    root: &Path,
    base: &Path,
    glob: Option<&str>,
    needle: &str,
    case_insensitive: bool,
    budget: Duration,
) -> Option<String> {
    let started = Instant::now();
    let mut builder = ignore::WalkBuilder::new(base);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .follow_links(false);
    if let Some(pattern) = glob {
        let mut types = ignore::types::TypesBuilder::new();
        if types.add("custom", pattern).is_ok() {
            types.select("custom");
            if let Ok(built) = types.build() {
                builder.types(built);
            }
        }
    }
    let mut rendered = String::new();
    let mut hits = 0usize;
    let mut files = 0usize;
    let mut seen_files = 0usize;
    let mut stopped_for_budget = false;
    for entry in builder.build().flatten() {
        if seen_files >= MAX_FUZZY_FILES || hits >= MAX_FUZZY_HITS {
            break;
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        // shortcut: the first file always runs; later files stop when the budget is spent and keep hits already found. A full scan belongs on an index.
        if seen_files > 0 && started.elapsed() >= budget {
            stopped_for_budget = true;
            break;
        }
        seen_files += 1;
        let path = entry.path();
        let Ok(metadata) = path.metadata() else {
            continue;
        };
        if metadata.len() > MAX_FUZZY_FILE_BYTES {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if text.contains('\0') {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(path);
        let mut file_hit = false;
        for (index, line) in text.lines().enumerate() {
            if hits >= MAX_FUZZY_HITS {
                break;
            }
            if find_fuzzy(needle, line, case_insensitive).is_none() {
                continue;
            }
            file_hit = true;
            hits += 1;
            rendered.push_str(&format!(
                ">{}:{}: {}\n",
                relative.display(),
                index + 1,
                clipped_line(line)
            ));
        }
        if file_hit {
            files += 1;
        }
    }
    if hits == 0 && !stopped_for_budget {
        return None;
    }
    if hits > 0 {
        rendered.push_str(&format!("{hits} fuzzy match(es) in {files} file(s)\n"));
    }
    if stopped_for_budget {
        rendered.push_str(FUZZY_SCAN_STOPPED);
        rendered.push('\n');
    }
    Some(rendered)
}

fn clipped_line(line: &str) -> &str {
    if line.len() <= MAX_LINE_CHARS {
        return line;
    }
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((byte, _)) => &line[..byte],
        None => line,
    }
}

pub(super) fn search_base(root: &Path, path: Option<&str>) -> Option<PathBuf> {
    let relative = path.unwrap_or(".");
    if relative.split(['/', '\\']).any(|part| part == "..") {
        return None;
    }
    let joined = root.join(relative);
    joined.is_dir().then_some(joined)
}

#[cfg(test)]
#[path = "search_fallback/tests.rs"]
mod tests;
