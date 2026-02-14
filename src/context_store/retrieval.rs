//! Context retrieval with vector search and optional reranking

use std::sync::Arc;

use super::config::RetrievalConfig;
use super::embedding::Embedder;
use super::error::Result;
use super::pathway::Pathway;
use super::rerank::Reranker;
use super::storage::StorageBackend;

pub struct Retriever {
    storage: Arc<dyn StorageBackend>,
    embedder: Arc<dyn Embedder>,
    config: RetrievalConfig,
    reranker: Option<Arc<dyn Reranker>>,
}

impl Retriever {
    pub fn new(
        storage: Arc<dyn StorageBackend>,
        embedder: Arc<dyn Embedder>,
        config: &RetrievalConfig,
        reranker: Option<Arc<dyn Reranker>>,
    ) -> Self {
        Self { storage, embedder, config: config.clone(), reranker }
    }

    pub async fn search(&self, query: &str, namespace: Option<&str>, limit: Option<usize>) -> Result<Vec<(Pathway, String, f32)>> {
        let limit = limit.unwrap_or(self.config.default_limit);
        let embedding = self.embedder.embed(query).await?;

        // Vector search
        let vector_results = self.storage.search_vector(&embedding, limit * 2, self.config.score_threshold).await?;

        // Text search as fallback
        let text_results = self.storage.search_text(query, namespace, limit).await?;

        // Merge results, preferring vector matches
        let mut seen = std::collections::HashSet::new();
        let mut merged = Vec::new();

        for (pathway, score) in vector_results {
            if seen.insert(pathway.as_str().to_string()) {
                merged.push((pathway, score));
            }
        }
        for (pathway, score) in text_results {
            if seen.insert(pathway.as_str().to_string()) {
                merged.push((pathway, score));
            }
        }

        // Fetch content for results
        let mut results = Vec::new();
        for (pathway, score) in merged {
            if let Ok(node) = self.storage.get(&pathway).await {
                results.push((pathway, node.content, score));
            }
        }

        // Rerank if available
        if let Some(ref reranker) = self.reranker {
            if self.config.rerank {
                let reranked = reranker.rerank(query, results.iter().map(|(p, c, s)| (p.as_str().to_string(), c.clone(), *s)).collect()).await?;
                results = reranked.into_iter()
                    .filter_map(|(key, content, score)| Pathway::parse(&key).ok().map(|p| (p, content, score)))
                    .collect();
            }
        }

        results.truncate(limit);
        Ok(results)
    }

    pub async fn search_hierarchical(&self, query: &str, namespace: Option<&str>, limit: Option<usize>) -> Result<Vec<(Pathway, String, f32)>> {
        if !self.config.hierarchical {
            return self.search(query, namespace, limit).await;
        }

        let mut results = self.search(query, namespace, limit).await?;

        // Explore parent directories for additional context
        let mut explored = std::collections::HashSet::new();
        let mut additional = Vec::new();

        for (pathway, _, _) in &results {
            if let Some(parent) = pathway.parent() {
                if explored.insert(parent.as_str().to_string()) {
                    if let Ok(children) = self.storage.get_children(&parent).await {
                        for child in children {
                            let key = child.pathway.as_str().to_string();
                            if !explored.contains(&key) {
                                explored.insert(key);
                                additional.push((child.pathway, child.content, 0.3));
                            }
                        }
                    }
                }
            }
        }

        results.extend(additional);
        let limit = limit.unwrap_or(self.config.default_limit);
        results.truncate(limit);
        Ok(results)
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { return 0.0; }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 1.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }
}
