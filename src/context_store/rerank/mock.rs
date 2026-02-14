//! Mock reranker for testing

use async_trait::async_trait;

pub struct MockReranker;

impl MockReranker {
    pub fn new() -> Self { Self }
}

impl Default for MockReranker {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl super::Reranker for MockReranker {
    async fn rerank(&self, _query: &str, mut documents: Vec<(String, String, f32)>) -> super::super::error::Result<Vec<(String, String, f32)>> {
        // Assign deterministic scores based on content hash
        for (i, doc) in documents.iter_mut().enumerate() {
            let hash = doc.1.bytes().fold(0u64, |acc, b| acc.wrapping_add(b as u64));
            doc.2 = ((hash % 100) as f32 / 100.0) * 0.5 + 0.5 - (i as f32 * 0.01);
        }
        documents.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        Ok(documents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::Reranker;

    #[tokio::test]
    async fn test_mock_reranker() {
        let reranker = MockReranker::new();
        let docs = vec![
            ("a".to_string(), "first doc".to_string(), 0.5),
            ("b".to_string(), "second doc".to_string(), 0.3),
        ];
        let result = reranker.rerank("query", docs).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result[0].2 >= result[1].2);
    }

    #[tokio::test]
    async fn test_mock_reranker_empty() {
        let reranker = MockReranker::new();
        let result = reranker.rerank("query", vec![]).await.unwrap();
        assert!(result.is_empty());
    }
}
