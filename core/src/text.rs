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
}
