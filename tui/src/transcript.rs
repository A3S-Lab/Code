//! Scrollback: user and assistant blocks above the prompt.

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

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// The newest lines that fit in the scrollback pane.
    pub fn window(&self, rows: usize) -> &[String] {
        let start = self.lines.len().saturating_sub(rows);
        &self.lines[start..]
    }
}
