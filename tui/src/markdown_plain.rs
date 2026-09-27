//! Thin markdown → plain text for scrollback (uses vendored a3s markdown-core).

use a3s_markdown_core::offset_events;
use pulldown_cmark::{Event, TagEnd};

/// Flatten markdown to plain text suitable for the monochrome scrollback pane.
pub fn markdown_to_plain(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for (event, _) in offset_events(input) {
        match event {
            Event::Text(text) | Event::Code(text) => out.push_str(&text),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::Rule => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("---\n");
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::CodeBlock) => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::markdown_to_plain;

    #[test]
    fn flattens_emphasis_and_code() {
        let plain = markdown_to_plain("hello **world** and `code`");
        assert!(plain.contains("hello"));
        assert!(plain.contains("world"));
        assert!(plain.contains("code"));
        assert!(!plain.contains("**"));
    }
}
