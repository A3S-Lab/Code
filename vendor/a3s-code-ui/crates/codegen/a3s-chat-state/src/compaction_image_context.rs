//! The `<image_files>` envelope and the image state carried across compaction: the last real user
//! turn's image parts and block, and the on-disk paths of images attached earlier in the session.
//! Pure text and data transforms; the existence check on the paths belongs to the shell.

use std::collections::BTreeSet;
use std::ops::Range;

use a3s_code_sampling_types::{ContentPart, ConversationItem, SyntheticReason};

use crate::compaction_utils::wrap_user_query;

/// Image state of the pre-compaction conversation the compacted history must carry. Pure data.
#[derive(Debug, Clone, Default)]
pub struct CompactionImageContext {
    /// `ContentPart::Image` parts of the item `last_user_query` came from, in order.
    /// `url` is `Arc<str>`, so clones are cheap.
    pub last_turn_image_parts: Vec<ContentPart>,
    /// Verbatim `<image_files>…</image_files>` block from that item's text.
    pub last_turn_image_files: Option<String>,
    /// Paths from every `<image_files>` block in the summarized conversation: chronological, deduped,
    /// excluding the last turn's own block. Harvested text is untrusted; the shell verifies each path
    /// against the session `assets/` dir and caps the list.
    pub attached_paths: Vec<String>,
}

/// Byte range of the first `<tag>…</tag>` block of `text`, tags included.
/// `None` when absent or unclosed.
pub(crate) fn tag_block_range(text: &str, tag: &str) -> Option<Range<usize>> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)?;
    let rel_end = text.get(start..)?.find(&close)?;
    Some(start..start + rel_end + close.len())
}

/// First `<tag>…</tag>` block of `text`, tags included. `None` when absent or unclosed.
fn extract_tag_block<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    text.get(tag_block_range(text, tag)?)
}

/// Image parts and `<image_files>` block of one user item (empty for anything else).
pub(crate) fn image_context_from_item(item: &ConversationItem) -> CompactionImageContext {
    let ConversationItem::User(user) = item else {
        return CompactionImageContext::default();
    };
    CompactionImageContext {
        last_turn_image_parts: user
            .content
            .iter()
            .filter(|part| matches!(part, ContentPart::Image { .. }))
            .cloned()
            .collect(),
        last_turn_image_files: extract_tag_block(&item.text_content(), "image_files")
            .map(str::to_owned),
        attached_paths: Vec::new(),
    }
}

/// The re-added last query: `[<image_files> block + "\n\n"] + wrap_user_query(q)` as one text
/// part, then the carried image parts in order. Byte-identical to
/// `ConversationItem::user(wrap_user_query(q))` when the image context is empty.
pub(crate) fn last_query_item(
    images: &CompactionImageContext,
    last_query: &str,
) -> ConversationItem {
    let query = wrap_user_query(last_query);
    // A query extracted without a `<user_query>` wrapper already carries the block.
    let text = match &images.last_turn_image_files {
        Some(block) if !last_query.contains(block.as_str()) => format!("{block}\n\n{query}"),
        _ => query,
    };
    let text_part = ContentPart::Text {
        text: std::sync::Arc::<str>::from(text),
    };
    ConversationItem::user_with_parts(
        std::iter::once(text_part)
            .chain(images.last_turn_image_parts.iter().cloned())
            .collect(),
    )
}

/// Paths listed as `N. <path>` lines inside every closed `<image_files>` block of `text`.
/// Only the numbered lines count, so the lead sentence of a block can change without breaking the chain.
pub(crate) fn parse_image_files_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut rest = text;
    while let Some(range) = tag_block_range(rest, "image_files") {
        let block = rest.get(range.clone()).unwrap_or_default();
        paths.extend(block.lines().filter_map(numbered_path));
        rest = rest.get(range.end..).unwrap_or_default();
    }
    paths
}

fn numbered_path(line: &str) -> Option<String> {
    let (number, path) = line.trim().split_once(". ")?;
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| path.trim().to_owned())
}

/// Paths from the `<image_files>` blocks of every `User` item, whatever its origin: a prior note is
/// `CompactionMeta`, and skipping it would break the chain across compactions.
/// Chronological, a repeated path takes its latest position (a re-attach refreshes it), `exclude`
/// removed. Uncapped: the shell caps after it has verified which paths are real asset files.
pub(crate) fn collect_attached_image_paths(
    conversation: &[ConversationItem],
    exclude: &[String],
) -> Vec<String> {
    // A compaction note or summary describes only what came before it, so its paths predate the
    // carried query above it; among meta items the later one (the note) lists the older paths.
    let (meta, rest): (Vec<&ConversationItem>, Vec<&ConversationItem>) = conversation
        .iter()
        .filter(|item| matches!(item, ConversationItem::User(_)))
        .partition(|item| {
            matches!(
                item,
                ConversationItem::User(user)
                    if user.synthetic_reason == SyntheticReason::CompactionMeta
            )
        });
    let chronological: Vec<String> = meta
        .into_iter()
        .rev()
        .chain(rest)
        .flat_map(|item| parse_image_files_paths(&item.text_content()))
        .filter(|path| !exclude.contains(path))
        .collect();
    let mut seen = BTreeSet::new();
    let mut newest_first: Vec<String> = chronological
        .into_iter()
        .rev()
        .filter(|path| seen.insert(path.clone()))
        .collect();
    newest_first.reverse();
    newest_first
}

/// The `<image_files>` block a prompt turn leads with, so the model has real on-disk paths for
/// `read_file`. Each path goes through `scrub_for_envelope`, so one containing a literal
/// `</image_files>` cannot close the envelope early.
pub fn render_image_files_block(paths: &[String]) -> Option<String> {
    (!paths.is_empty()).then(|| {
        image_files_block(
            "The following images were provided by the user and saved to the workspace for future use:",
            paths,
            "\nThese images can be copied for use in other locations.\n",
        )
    })
}

/// The note re-added after the summary. It is itself an `<image_files>` block so the same parser
/// harvests it on the next compaction. `paths` must already be the shell-verified asset files.
pub(crate) fn render_attached_image_paths_note(paths: &[String]) -> String {
    image_files_block(
        "Images the user attached earlier in this session (before the summary above) are saved at the absolute paths below. They live under the session directory, not the workspace. If one matters for the current task, open it with read_file; do not ask the user to re-send it.",
        paths,
        "",
    )
}

fn image_files_block(lead: &str, paths: &[String], trailer: &str) -> String {
    let mut out = format!("<image_files>\n{lead}\n");
    for (i, path) in paths.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, scrub_for_envelope(path)));
    }
    out.push_str(trailer);
    out.push_str("</image_files>");
    out
}

/// Sanitize a single-line string before interpolating it into a structured envelope.
/// Replaces `<` / `>` with the typographic look-alikes `‹` / `›` so envelope-close tags cannot be forged.
/// Trade-off: model output sees `‹` instead of `<` in the scrubbed region; these are envelope fillers, not source code.
fn scrub_for_envelope(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push('‹'),
            '>' => out.push('›'),
            c if c.is_ascii_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
#[path = "compaction_image_context_tests.rs"]
mod tests;
