//! Scrollback: user and assistant blocks above the prompt.

use std::path::Path;

/// Conversation transcript the full-screen view scrolls.
#[derive(Debug, Default, Clone)]
pub struct Scrollback {
    lines: Vec<String>,
}

impl Scrollback {
    pub fn push_user(&mut self, text: &str) {
        self.lines.push(format!("you  {text}"));
    }

    pub fn push_assistant(&mut self, text: &str) {
        self.lines.push(format!("a3s  {text}"));
    }

    /// Number of stored lines.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Replace a line by index (streaming text deltas).
    pub fn set_assistant_line(&mut self, index: usize, text: &str) {
        let rendered = format!("a3s  {text}");
        if let Some(line) = self.lines.get_mut(index) {
            *line = rendered;
        } else {
            self.lines.push(rendered);
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// The newest lines that fit in the scrollback pane.
    pub fn window(&self, rows: usize) -> &[String] {
        let start = self.lines.len().saturating_sub(rows);
        &self.lines[start..]
    }

    /// User prompts matching `query` (substring, case-insensitive). Newest first.
    pub fn history_matches(&self, query: &str, limit: usize) -> Vec<String> {
        let needle = query.trim().to_ascii_lowercase();
        self.lines
            .iter()
            .rev()
            .filter_map(|line| line.strip_prefix("you  "))
            .filter(|text| {
                needle.is_empty() || text.to_ascii_lowercase().contains(&needle)
            })
            .take(limit)
            .map(|text| text.to_string())
            .collect()
    }

    /// Write the transcript as a simple Markdown session file.
    pub fn export_markdown(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut body = String::from("# A3S session\n\n");
        for line in &self.lines {
            if let Some(text) = line.strip_prefix("you  ") {
                body.push_str("## You\n\n");
                body.push_str(text);
                body.push_str("\n\n");
            } else if let Some(text) = line.strip_prefix("a3s  ") {
                body.push_str("## A3S\n\n");
                body.push_str(text);
                body.push_str("\n\n");
            }
        }
        std::fs::write(path, body)
    }
}
