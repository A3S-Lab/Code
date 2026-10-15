//! Non-secret usage notes for installed connectors.
//!
//! The Studio `reconcile_at` consumer lands with its slice; kept linked.
#![allow(dead_code)]
//! Studio `reconcile_at` and the session tests both call [`sync_connector_notes`].
//! A note is `<root>/<slug>.md`. Command, env, headers, and URLs are not inputs.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

const DESCRIPTION_LIMIT: usize = 160;
const BODY_LIMIT: usize = 8_000;

/// Replace `<root>/<slug>.md` with `notes`. Slugs absent from `notes` lose their file.
pub fn sync_connector_notes(root: &Path, notes: &[(&str, &str)]) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let mut keep = BTreeSet::new();
    for (name, usage) in notes {
        if !slug_ok(name) {
            continue;
        }
        let file_name = format!("{name}.md");
        let path = root.join(&file_name);
        if path.parent() != Some(root) {
            continue;
        }
        fs::write(&path, render_note(name, usage))?;
        keep.insert(file_name);
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.ends_with(".md") || keep.contains(name) {
            continue;
        }
        let stem = &name[..name.len() - 3];
        if !slug_ok(stem) {
            continue;
        }
        let path = entry.path();
        if fs::symlink_metadata(&path)
            .is_ok_and(|meta| meta.file_type().is_file() || meta.file_type().is_symlink())
        {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

/// Catalog description, capped at 160 scalars. `None` when the note has no description.
pub fn note_summary(prefix: &str) -> Option<String> {
    let mut lines = prefix.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != "description" {
            continue;
        }
        let line = one_line(&unquote(value.trim()));
        if line.is_empty() {
            return None;
        }
        return Some(line);
    }
    None
}

/// Text after the closing frontmatter fence.
///
/// `Some` is the whole string when there is no opening fence, or the body
/// when the closing fence is its own line. `None` means the opening fence
/// has no closer: that document fails closed instead of treating the
/// frontmatter as the body. The closer may end in LF, CRLF, or CR. One line
/// ending after the closer is skipped, so a CRLF body starts at the content
/// character.
pub fn note_body(text: &str) -> Option<&str> {
    let stripped = text.trim_start_matches('\u{feff}');
    let Some((opening, mut offset)) = next_logical_line(stripped, 0) else {
        return Some(text);
    };
    if opening.trim() != "---" {
        return Some(text);
    }
    loop {
        let Some((line, next)) = next_logical_line(stripped, offset) else {
            return None;
        };
        offset = next;
        if line.trim() == "---" {
            return Some(stripped[offset..].trim_start_matches(['\r', '\n']));
        }
    }
}

/// One line with its ending removed. The returned index is the next line.
fn next_logical_line(text: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    if start >= bytes.len() {
        return None;
    }
    let mut index = start;
    while index < bytes.len() && bytes[index] != b'\n' && bytes[index] != b'\r' {
        index += 1;
    }
    let line = &text[start..index];
    if index == bytes.len() {
        return Some((line, index));
    }
    let next = if bytes[index] == b'\r' && index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
        index + 2
    } else {
        index + 1
    };
    Some((line, next))
}

/// Label for a connector that has no catalog description. The kind is `stdio`, `http`, or `sse`.
pub fn transport_label(kind: Option<&str>) -> String {
    match kind {
        Some("http") => "Streamable HTTP connector".to_owned(),
        Some("sse") => "SSE connector".to_owned(),
        Some("stdio") => "Local stdio connector".to_owned(),
        _ => "Installed connector".to_owned(),
    }
}

/// Drop NUL and cap the catalog description a connector may carry.
pub fn bounded_usage(raw: &str) -> String {
    raw.chars()
        .filter(|ch| *ch != '\0')
        .take(BODY_LIMIT)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn render_note(name: &str, usage: &str) -> String {
    let body = bounded_usage(usage);
    let description = one_line(&body);
    format!(
        "---\nname: {}\ndescription: {}\n---\n{body}\n",
        yaml_quote(name),
        yaml_quote(&description),
    )
}

fn one_line(raw: &str) -> String {
    let collapsed: String = raw
        .chars()
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .filter(|ch| *ch != '\0')
        .collect();
    collapsed.trim().chars().take(DESCRIPTION_LIMIT).collect()
}

fn yaml_quote(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value
        .chars()
        .filter(|ch| *ch != '\n' && *ch != '\r' && *ch != '"' && *ch != '\\')
    {
        out.push(ch);
    }
    out.push('"');
    out
}

fn unquote(value: &str) -> String {
    let quoted = value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')));
    if quoted {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

fn slug_ok(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}
