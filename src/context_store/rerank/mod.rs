//! Reranking for search results using session's LLM

pub mod mock;

use async_trait::async_trait;
use std::sync::Arc;
use crate::llm::{ContentBlock, LlmClient, Message};

pub use mock::MockReranker;

#[async_trait]
pub trait Reranker: Send + Sync {
    async fn rerank(&self, query: &str, documents: Vec<(String, String, f32)>) -> super::error::Result<Vec<(String, String, f32)>>;
}

pub fn create_reranker(llm_client: Option<Arc<dyn LlmClient>>) -> Option<Arc<dyn Reranker>> {
    llm_client.map(|llm| Arc::new(LlmReranker::new(llm)) as Arc<dyn Reranker>)
}

pub struct LlmReranker {
    llm_client: Arc<dyn LlmClient>,
}

impl LlmReranker {
    pub fn new(llm_client: Arc<dyn LlmClient>) -> Self {
        Self { llm_client }
    }
}

#[async_trait]
impl Reranker for LlmReranker {
    async fn rerank(&self, query: &str, documents: Vec<(String, String, f32)>) -> super::error::Result<Vec<(String, String, f32)>> {
        let mut scored = Vec::with_capacity(documents.len());
        for (id, content, original_score) in documents {
            let prompt = format!(
                "Rate the relevance of this document to the query on a scale of 0.0 to 1.0.\n\nQuery: {}\n\nDocument:\n{}\n\nRespond with ONLY a number between 0.0 and 1.0.",
                query,
                if content.len() > 2000 { &content[..2000] } else { &content }
            );
            let messages = vec![Message::user(&prompt)];
            match self.llm_client.complete(&messages, None, &[]).await {
                Ok(response) => {
                    let text: String = response.message.content.iter()
                        .filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None })
                        .collect();
                    let score = text.trim().parse::<f32>().unwrap_or(original_score);
                    scored.push((id, content, score.clamp(0.0, 1.0)));
                }
                Err(_) => scored.push((id, content, original_score)),
            }
        }
        scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_reranker_none() {
        let reranker = create_reranker(None);
        assert!(reranker.is_none());
    }
}
