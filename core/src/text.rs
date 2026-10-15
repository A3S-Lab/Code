pub(crate) fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }

    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }

    &s[..end]
}

/// A file or snippet whose every newline is the same ASCII sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NewlineStyle {
    Lf,
    Crlf,
}

/// Classification of the newline bytes in one string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NewlineClass {
    /// No `\n` and no `\r`.
    None,
    Uniform(NewlineStyle),
    /// A bare CR, or both LF and CRLF in the same string.
    Unsafe,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LineEndingEditError {
    NotFound,
    Ambiguous(usize),
    /// Mixed endings, a bare CR, or a replacement that would split a CRLF pair.
    Unsafe,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LineEndingEdit {
    pub content: String,
    pub count: usize,
}

pub(crate) fn classify_newlines(text: &str) -> NewlineClass {
    let bytes = text.as_bytes();
    let mut saw_lf = false;
    let mut saw_crlf = false;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' {
            if index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
                saw_crlf = true;
                index += 2;
                continue;
            }
            return NewlineClass::Unsafe;
        }
        if bytes[index] == b'\n' {
            saw_lf = true;
        }
        index += 1;
    }
    match (saw_lf, saw_crlf) {
        (false, false) => NewlineClass::None,
        (true, false) => NewlineClass::Uniform(NewlineStyle::Lf),
        (false, true) => NewlineClass::Uniform(NewlineStyle::Crlf),
        (true, true) => NewlineClass::Unsafe,
    }
}

/// Rewrite `incoming` into `existing`'s newline style when both are uniform
/// and they differ. Any other combination, including mixed endings, is copied
/// through unchanged.
pub(crate) fn preserve_newline_style<'a>(
    existing: &str,
    incoming: &'a str,
) -> std::borrow::Cow<'a, str> {
    match (classify_newlines(existing), classify_newlines(incoming)) {
        (NewlineClass::Uniform(target), NewlineClass::Uniform(source)) if target != source => {
            std::borrow::Cow::Owned(transcode_newlines(incoming, target))
        }
        _ => std::borrow::Cow::Borrowed(incoming),
    }
}

/// Apply `old_string` -> `new_string`, treating LF and CRLF as the same break.
///
/// An exact byte match wins, except a match that starts or ends inside a
/// `\r\n` pair. Those matches fall through to a logical match so a Windows
/// file is not left with a bare CR. Untouched bytes keep the file's endings.
/// Mixed or CR-only text is refused when a logical match exists.
pub(crate) fn apply_line_ending_edit(
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<LineEndingEdit, LineEndingEditError> {
    if old_string.is_empty() {
        return Err(LineEndingEditError::NotFound);
    }

    let exact = exact_spans(content, old_string);
    let splitting = exact
        .iter()
        .filter(|(start, end)| span_splits_crlf(content, *start, *end))
        .count();
    if !exact.is_empty() && splitting == 0 {
        let count = exact.len();
        if count > 1 && !replace_all {
            return Err(LineEndingEditError::Ambiguous(count));
        }
        let edited = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };
        return Ok(LineEndingEdit {
            content: edited,
            count: if replace_all { count } else { 1 },
        });
    }
    if splitting > 0 && splitting != exact.len() {
        return Err(LineEndingEditError::Unsafe);
    }

    if classify_newlines(content) == NewlineClass::Unsafe
        || classify_newlines(old_string) == NewlineClass::Unsafe
    {
        return if cr_stripped_contains(content, old_string) {
            Err(LineEndingEditError::Unsafe)
        } else {
            Err(LineEndingEditError::NotFound)
        };
    }

    let lf_content = to_lf(content);
    let lf_old = to_lf(old_string);
    if lf_old.is_empty() {
        return Err(LineEndingEditError::NotFound);
    }
    let logical = exact_spans(lf_content.as_ref(), lf_old.as_ref());
    if logical.is_empty() {
        return Err(LineEndingEditError::NotFound);
    }
    let count = logical.len();
    if count > 1 && !replace_all {
        return Err(LineEndingEditError::Ambiguous(count));
    }
    if classify_newlines(new_string) == NewlineClass::Unsafe {
        return Err(LineEndingEditError::Unsafe);
    }
    let file_style = match classify_newlines(content) {
        NewlineClass::Uniform(style) => style,
        NewlineClass::None | NewlineClass::Unsafe => NewlineStyle::Lf,
    };
    let replacement = transcode_newlines(new_string, file_style);
    let spans = if classify_newlines(content) == NewlineClass::Uniform(NewlineStyle::Crlf) {
        match map_lf_spans_to_crlf(lf_content.as_ref(), &logical) {
            Some(spans) => spans,
            None => return Err(LineEndingEditError::Unsafe),
        }
    } else {
        logical
    };
    let chosen = if replace_all {
        spans.as_slice()
    } else {
        &spans[..1]
    };
    Ok(LineEndingEdit {
        content: splice(content, chosen, &replacement),
        count: if replace_all { count } else { 1 },
    })
}

fn transcode_newlines(text: &str, style: NewlineStyle) -> String {
    match (style, classify_newlines(text)) {
        (NewlineStyle::Crlf, NewlineClass::Uniform(NewlineStyle::Lf) | NewlineClass::None) => {
            text.replace('\n', "\r\n")
        }
        (NewlineStyle::Lf, NewlineClass::Uniform(NewlineStyle::Crlf)) => text.replace("\r\n", "\n"),
        _ => text.to_string(),
    }
}

fn to_lf(text: &str) -> std::borrow::Cow<'_, str> {
    if classify_newlines(text) == NewlineClass::Uniform(NewlineStyle::Crlf) {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

fn cr_stripped_contains(haystack: &str, needle: &str) -> bool {
    let needle = needle.replace('\r', "");
    !needle.is_empty() && haystack.replace('\r', "").contains(&needle)
}

fn exact_spans(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut start = 0;
    while let Some(found) = haystack[start..].find(needle) {
        let abs = start + found;
        spans.push((abs, abs + needle.len()));
        start = abs + needle.len();
    }
    spans
}

fn span_splits_crlf(haystack: &str, start: usize, end: usize) -> bool {
    let bytes = haystack.as_bytes();
    if start > 0 && bytes.get(start) == Some(&b'\n') && bytes[start - 1] == b'\r' {
        return true;
    }
    end > start && end < bytes.len() && bytes[end - 1] == b'\r' && bytes[end] == b'\n'
}

fn map_lf_spans_to_crlf(lf: &str, spans: &[(usize, usize)]) -> Option<Vec<(usize, usize)>> {
    let mut needed = Vec::with_capacity(spans.len() * 2);
    for (start, end) in spans {
        needed.push(*start);
        needed.push(*end);
    }
    needed.sort_unstable();
    needed.dedup();
    let mut mapped = std::collections::BTreeMap::new();
    let mut extra = 0usize;
    let mut next = 0usize;
    for (index, byte) in lf.as_bytes().iter().enumerate() {
        while next < needed.len() && needed[next] == index {
            mapped.insert(needed[next], index + extra);
            next += 1;
        }
        if *byte == b'\n' {
            extra += 1;
        }
    }
    while next < needed.len() && needed[next] == lf.len() {
        mapped.insert(needed[next], lf.len() + extra);
        next += 1;
    }
    spans
        .iter()
        .map(|(start, end)| Some((*mapped.get(start)?, *mapped.get(end)?)))
        .collect()
}

fn splice(haystack: &str, spans: &[(usize, usize)], replacement: &str) -> String {
    let mut out = String::with_capacity(haystack.len() + replacement.len());
    let mut cursor = 0usize;
    for (start, end) in spans {
        out.push_str(&haystack[cursor..*start]);
        out.push_str(replacement);
        cursor = *end;
    }
    out.push_str(&haystack[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::{
        apply_line_ending_edit, classify_newlines, preserve_newline_style, truncate_utf8,
        LineEndingEditError, NewlineClass, NewlineStyle,
    };

    #[test]
    fn truncate_utf8_keeps_ascii_prefix() {
        assert_eq!(truncate_utf8("abcdef", 3), "abc");
    }

    #[test]
    fn truncate_utf8_does_not_split_multibyte_characters() {
        let s = "执行视频分析任务";
        let truncated = truncate_utf8(s, 5);
        assert_eq!(truncated, "执");
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn truncate_utf8_issue_23_real_payload() {
        // Exact scenario from GitHub issue #23: byte 180 falls inside '费' (bytes 178..181)
        let raw = "# Issue Summary\n\n# Issue Source\n- issue_id: 297936\n- org_id: 848\n- create_time: 2025-10-29 03:16:16\n- item_id: 11089\n\n## issue_name\n用户请求处理视频分析任务，涉及计费(费用:100元)\n";
        let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        // After joining with spaces, byte 180 splits a Chinese char
        assert!(
            !compact.is_char_boundary(180),
            "compact len={}",
            compact.len()
        );
        // The original code did: &compact[..180] — this panicked
        let truncated = truncate_utf8(&compact, 180);
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn truncate_utf8_chinese_only() {
        let s = "用户请求处理视频分析任务，涉及计费";
        let truncated = truncate_utf8(s, 15);
        assert!(truncated.is_char_boundary(truncated.len()));
        // Should not split: 费 is 3 bytes (bytes 12-14), so 15 should stop before it
    }

    #[test]
    fn truncate_utf8_mixed_ascii_chinese() {
        let s = "hello你好world世界test";
        let truncated = truncate_utf8(s, 10);
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn classify_newlines_rejects_mixed_and_bare_cr() {
        assert_eq!(
            classify_newlines("a\nb\n"),
            NewlineClass::Uniform(NewlineStyle::Lf)
        );
        assert_eq!(
            classify_newlines("a\r\nb\r\n"),
            NewlineClass::Uniform(NewlineStyle::Crlf)
        );
        assert_eq!(classify_newlines("no-break"), NewlineClass::None);
        assert_eq!(classify_newlines("a\nb\r\n"), NewlineClass::Unsafe);
        assert_eq!(classify_newlines("a\rb"), NewlineClass::Unsafe);
    }

    #[test]
    fn logical_edit_preserves_crlf_outside_the_match() {
        let edited = apply_line_ending_edit(
            "keep\r\nalpha\r\nbeta\r\n",
            "alpha\nbeta",
            "alpha\ngamma",
            false,
        )
        .unwrap();
        assert_eq!(edited.count, 1);
        assert_eq!(edited.content, "keep\r\nalpha\r\ngamma\r\n");
    }

    #[test]
    fn lf_needle_does_not_split_a_crlf_pair() {
        let edited = apply_line_ending_edit("a\r\nb\r\n", "\n", "X", true).unwrap();
        assert_eq!(edited.count, 2);
        assert_eq!(edited.content, "aXbX");
    }

    #[test]
    fn mixed_line_endings_are_refused_when_the_text_matches() {
        let error = apply_line_ending_edit("a\r\nb\n", "a\nb", "a\nc", false).unwrap_err();
        assert_eq!(error, LineEndingEditError::Unsafe);
    }

    #[test]
    fn preserve_newline_style_converts_only_uniform_opposites() {
        assert_eq!(preserve_newline_style("a\r\nb\r\n", "a\nc\n"), "a\r\nc\r\n");
        assert_eq!(preserve_newline_style("a\nb\n", "a\r\nc\r\n"), "a\nc\n");
        assert_eq!(
            preserve_newline_style("a\r\nb\r\n", "a\r\nc\r\n"),
            "a\r\nc\r\n"
        );
        assert_eq!(preserve_newline_style("a\nb\r\n", "a\nc\n"), "a\nc\n");
        assert_eq!(preserve_newline_style("a\r\nb\r\n", "hello"), "hello");
    }
}
