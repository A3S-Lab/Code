//! Model-facing pages over retained `a3s://tool-output/...` artifacts.
//!
//! A lossy tool-result transform keeps the original bytes in the session
//! artifact store. `read` and `search` page that exact content so the model
//! can recover an omitted region instead of re-running the command.

use super::types::{ToolContext, ToolOutput};
use super::{MAX_LINE_LENGTH, MAX_READ_LINES};
use crate::text::truncate_utf8;

pub(crate) const TOOL_OUTPUT_URI_PREFIX: &str = "a3s://tool-output/";
const MAX_GREP_MATCHES: usize = 1_000;
pub(crate) const DEFAULT_GREP_MATCHES: usize = 200;
const MAX_GREP_CONTEXT: usize = 8;
const MAX_GREP_OUTPUT_BYTES: usize = 64 * 1_024;

pub(crate) fn is_tool_output_uri(path: &str) -> bool {
    path.starts_with(TOOL_OUTPUT_URI_PREFIX)
}

fn load(ctx: &ToolContext, uri: &str) -> Result<String, String> {
    let store = ctx.artifact_store.as_ref().ok_or_else(|| {
        format!("{uri} is not readable here: this session retains tool output outside the local artifact store")
    })?;
    store.get(uri).map(|artifact| artifact.content).ok_or_else(|| {
        format!("{uri} is not retained in this session (evicted by the artifact limit or never produced here)")
    })
}

fn display_line(line: &str) -> &str {
    truncate_utf8(line, MAX_LINE_LENGTH)
}

/// One `read` page of an artifact, numbered like a workspace file read.
pub(crate) fn read_page(
    ctx: &ToolContext,
    uri: &str,
    offset: usize,
    limit: usize,
    byte_offset: usize,
) -> ToolOutput {
    let content = match load(ctx, uri) {
        Ok(content) => content,
        Err(error) => return ToolOutput::error(error),
    };
    let lines = content.lines().collect::<Vec<_>>();
    let limit = limit.min(MAX_READ_LINES);
    if offset > lines.len() {
        return ToolOutput::error(format!(
            "Offset {offset} exceeds artifact length ({} lines)",
            lines.len()
        ));
    }
    let end = offset.saturating_add(limit).min(lines.len());
    let mut output = String::new();
    for (index, line) in lines[offset..end].iter().enumerate() {
        output.push_str(&format!(
            "{:>6}\t{}\n",
            offset + index + 1,
            super::builtin::read::display_read_line(line, byte_offset)
        ));
    }
    let next_offset = (end < lines.len()).then_some(end);
    if let Some(next) = next_offset {
        output.push_str(&format!(
            "\n... (more lines available; continue with offset={next})\n"
        ));
    } else if output.is_empty() {
        output.push_str("(end of artifact)\n");
    }
    ToolOutput::success(output).with_metadata(serde_json::json!({
        "source_anchors": [uri],
        "range": {
            "offset": offset,
            "returned_lines": end - offset,
            "next_offset": next_offset,
            "eof": next_offset.is_none(),
            "total_lines": lines.len(),
        }
    }))
}

/// Regular-expression line search inside one artifact.
pub(crate) fn grep(
    ctx: &ToolContext,
    uri: &str,
    pattern: &str,
    case_insensitive: bool,
    context: usize,
    limit: usize,
) -> ToolOutput {
    let content = match load(ctx, uri) {
        Ok(content) => content,
        Err(error) => return ToolOutput::error(error),
    };
    let regex = match regex::RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .build()
    {
        Ok(regex) => regex,
        Err(error) => return ToolOutput::error(format!("invalid regular expression: {error}")),
    };
    let lines = content.lines().collect::<Vec<_>>();
    let context = context.min(MAX_GREP_CONTEXT);
    let limit = limit.clamp(1, MAX_GREP_MATCHES);
    let matches = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| regex.is_match(line).then_some(index))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return ToolOutput::success(format!("No matches in {uri} ({} lines)", lines.len()))
            .with_metadata(serde_json::json!({ "source_anchors": [uri], "matches": 0 }));
    }
    let mut output = String::new();
    let mut shown = 0usize;
    let mut printed_until = 0usize;
    let mut budget_hit = false;
    for &index in matches.iter().take(limit) {
        let start = index.saturating_sub(context).max(printed_until);
        let end = (index + context + 1).min(lines.len());
        if printed_until > 0 && start > printed_until {
            output.push_str("--\n");
        }
        for (line_index, line) in lines.iter().enumerate().take(end).skip(start) {
            let separator = if line_index == index { ':' } else { '-' };
            output.push_str(&format!(
                "{}{separator}{}\n",
                line_index + 1,
                display_line(line)
            ));
        }
        printed_until = end;
        shown += 1;
        if output.len() >= MAX_GREP_OUTPUT_BYTES {
            budget_hit = true;
            break;
        }
    }
    if shown < matches.len() {
        let reason = if budget_hit { "output budget" } else { "limit" };
        output.push_str(&format!(
            "\n... ({} of {} matching lines shown; stopped at the {reason}. Read a range with read file_path={uri} offset=<line-1>.)\n",
            shown,
            matches.len()
        ));
    }
    ToolOutput::success(output).with_metadata(serde_json::json!({
        "source_anchors": [uri],
        "matches": matches.len(),
        "shown_matches": shown,
    }))
}

#[cfg(test)]
#[path = "artifact_view_tests.rs"]
mod tests;
