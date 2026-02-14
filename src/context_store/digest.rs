//! Multi-level digest generation for efficient context retrieval

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::llm::{ContentBlock, LlmClient, Message};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Digest {
    pub brief: String,
    pub summary: String,
    pub generated: bool,
}

impl Digest {
    pub fn new() -> Self { Self::default() }

    pub fn with_content(brief: String, summary: String) -> Self {
        Self { brief, summary, generated: true }
    }

    pub fn is_generated(&self) -> bool { self.generated }

    pub fn get_level(&self, max_tokens: usize) -> DigestLevel {
        if max_tokens < 100 { DigestLevel::Brief }
        else if max_tokens < 1000 { DigestLevel::Summary }
        else { DigestLevel::Full }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestLevel { Brief, Summary, Full }

pub struct DigestGenerator {
    llm_client: Option<Arc<dyn LlmClient>>,
}

impl DigestGenerator {
    pub fn new(llm_client: Option<Arc<dyn LlmClient>>) -> Self {
        Self { llm_client }
    }

    pub async fn generate(&self, content: &str, kind: super::types::NodeKind) -> super::error::Result<Digest> {
        if self.llm_client.is_none() {
            return Ok(self.generate_simple(content));
        }
        let llm = self.llm_client.as_ref().unwrap();
        let brief_prompt = format!("Summarize the following {} in one concise sentence (max 50 tokens):\n\n{}", kind_to_str(kind), truncate(content, 4000));
        let brief = self.complete_simple(llm, &brief_prompt).await?;
        let summary_prompt = format!("Provide a comprehensive summary of the following {} (max 500 tokens). Include key points, main concepts, and important details:\n\n{}", kind_to_str(kind), truncate(content, 8000));
        let summary = self.complete_simple(llm, &summary_prompt).await?;
        Ok(Digest::with_content(brief, summary))
    }

    async fn complete_simple(&self, llm: &Arc<dyn LlmClient>, prompt: &str) -> super::error::Result<String> {
        let messages = vec![Message::user(prompt)];
        let response = llm.complete(&messages, None, &[]).await
            .map_err(|e| super::error::A3SError::DigestGeneration(e.to_string()))?;
        let text = response.message.content.iter()
            .filter_map(|block| if let ContentBlock::Text { text } = block { Some(text.as_str()) } else { None })
            .collect::<Vec<_>>().join("");
        Ok(text)
    }

    fn generate_simple(&self, content: &str) -> Digest {
        let brief = extract_first_sentence(content);
        let summary = truncate(content, 2000).to_string();
        Digest::with_content(brief, summary)
    }
}

fn kind_to_str(kind: super::types::NodeKind) -> &'static str {
    match kind {
        super::types::NodeKind::Document => "document",
        super::types::NodeKind::Code => "code",
        super::types::NodeKind::Markdown => "markdown document",
        super::types::NodeKind::Memory => "memory",
        super::types::NodeKind::Capability => "capability",
        super::types::NodeKind::Message => "message",
        super::types::NodeKind::Data => "data",
        super::types::NodeKind::Directory => "directory",
    }
}

fn truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars { s } else { &s[..max_chars] }
}

fn extract_first_sentence(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() { return String::new(); }
    let endings = [". ", ".\n", "! ", "!\n", "? ", "?\n"];
    let mut min_pos = s.len();
    for ending in &endings {
        if let Some(pos) = s.find(ending) { min_pos = min_pos.min(pos + 1); }
    }
    let end = min_pos.min(200);
    s[..end].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_first_sentence() {
        assert_eq!(extract_first_sentence("This is first. This is second."), "This is first.");
    }

    #[test]
    fn test_extract_first_sentence_empty() {
        assert_eq!(extract_first_sentence(""), "");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("Hello, world!", 5), "Hello");
        assert_eq!(truncate("Hello", 100), "Hello");
    }

    #[test]
    fn test_digest_new() {
        let d = Digest::new();
        assert!(!d.is_generated());
    }

    #[test]
    fn test_digest_with_content() {
        let d = Digest::with_content("brief".into(), "summary".into());
        assert!(d.is_generated());
        assert_eq!(d.brief, "brief");
    }

    #[test]
    fn test_digest_get_level() {
        let d = Digest::new();
        assert_eq!(d.get_level(50), DigestLevel::Brief);
        assert_eq!(d.get_level(500), DigestLevel::Summary);
        assert_eq!(d.get_level(2000), DigestLevel::Full);
    }

    #[test]
    fn test_kind_to_str() {
        use crate::context_store::types::NodeKind;
        assert_eq!(kind_to_str(NodeKind::Code), "code");
        assert_eq!(kind_to_str(NodeKind::Document), "document");
    }
}
