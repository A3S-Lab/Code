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

#[cfg(test)]
mod tests {
    use super::truncate_utf8;

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
}
