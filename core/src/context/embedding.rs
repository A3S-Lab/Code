//! Embedding Provider Extension Point
//!
//! Defines the trait for generating vector embeddings from text.
//! Implementations can use LLM APIs (OpenAI, Anthropic) or local models.

use anyhow::Result;
use async_trait::async_trait;

/// Vector embedding (dense float array)
pub type Embedding = Vec<f32>;

/// Trait for generating text embeddings
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Provider name for logging
    fn name(&self) -> &str;

    /// Embedding dimension (e.g., 1536 for OpenAI text-embedding-3-small)
    fn dimension(&self) -> usize;

    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Embedding>;

    /// Generate embeddings for multiple texts (batch)
    ///
    /// Default implementation calls `embed()` sequentially.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
}

/// OpenAI-compatible embedding provider
///
/// Works with OpenAI, Azure OpenAI, and any API that implements
/// the `/v1/embeddings` endpoint.
pub struct OpenAiEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    dimension: usize,
}

impl OpenAiEmbeddingProvider {
    /// Create a new OpenAI embedding provider
    ///
    /// - `api_key`: API key for authentication
    /// - `model`: Model name (e.g., "text-embedding-3-small")
    /// - `dimension`: Embedding dimension (e.g., 1536)
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimension: usize,
    ) -> Result<Self> {
        Self::with_base_url(api_key, model, dimension, "https://api.openai.com/v1")
    }

    /// Create with a custom base URL (for Azure, local proxies, etc.)
    pub fn with_base_url(
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimension: usize,
        base_url: impl Into<String>,
    ) -> Result<Self> {
        let api_key = api_key.into();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", api_key)
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid API key header: {}", e))?,
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            dimension,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    fn name(&self) -> &str {
        "openai-embedding"
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, text: &str) -> Result<Embedding> {
        let mut results = self.embed_batch(&[text]).await?;
        results
            .pop()
            .ok_or_else(|| anyhow::anyhow!("Empty embedding response"))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{}/embeddings", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Embedding API request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Embedding API returned HTTP {}: {}",
                status,
                body
            ));
        }

        let json: serde_json::Value = response.json().await?;
        let data = json["data"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid embedding response: missing 'data' array"))?;

        let mut embeddings = Vec::with_capacity(data.len());
        for item in data {
            let embedding: Vec<f32> = item["embedding"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Invalid embedding item: missing 'embedding'"))?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();

            if embedding.len() != self.dimension {
                return Err(anyhow::anyhow!(
                    "Embedding dimension mismatch: expected {}, got {}",
                    self.dimension,
                    embedding.len()
                ));
            }

            embeddings.push(embedding);
        }

        Ok(embeddings)
    }
}

impl std::fmt::Debug for OpenAiEmbeddingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiEmbeddingProvider")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("dimension", &self.dimension)
            .finish()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- Mock embedding provider for testing --

    /// Simple mock that returns deterministic embeddings based on text hash
    pub(crate) struct MockEmbeddingProvider {
        dim: usize,
    }

    impl MockEmbeddingProvider {
        pub fn new(dim: usize) -> Self {
            Self { dim }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        fn name(&self) -> &str {
            "mock-embedding"
        }

        fn dimension(&self) -> usize {
            self.dim
        }

        async fn embed(&self, text: &str) -> Result<Embedding> {
            // Deterministic pseudo-embedding from text bytes
            let mut embedding = vec![0.0f32; self.dim];
            for (i, byte) in text.bytes().enumerate() {
                embedding[i % self.dim] += (byte as f32) / 255.0;
            }
            // Normalize
            let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for v in &mut embedding {
                    *v /= norm;
                }
            }
            Ok(embedding)
        }
    }

    #[test]
    fn test_embedding_type() {
        let emb: Embedding = vec![0.1, 0.2, 0.3];
        assert_eq!(emb.len(), 3);
    }

    #[tokio::test]
    async fn test_mock_embedding_provider() {
        let provider = MockEmbeddingProvider::new(8);
        assert_eq!(provider.name(), "mock-embedding");
        assert_eq!(provider.dimension(), 8);

        let emb = provider.embed("hello world").await.unwrap();
        assert_eq!(emb.len(), 8);

        // Verify normalization (unit vector)
        let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_mock_embedding_deterministic() {
        let provider = MockEmbeddingProvider::new(8);
        let emb1 = provider.embed("test input").await.unwrap();
        let emb2 = provider.embed("test input").await.unwrap();
        assert_eq!(emb1, emb2);
    }

    #[tokio::test]
    async fn test_mock_embedding_different_texts() {
        let provider = MockEmbeddingProvider::new(8);
        let emb1 = provider.embed("hello").await.unwrap();
        let emb2 = provider.embed("world").await.unwrap();
        assert_ne!(emb1, emb2);
    }

    #[tokio::test]
    async fn test_embed_batch_default() {
        let provider = MockEmbeddingProvider::new(4);
        let results = provider
            .embed_batch(&["hello", "world", "test"])
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        for emb in &results {
            assert_eq!(emb.len(), 4);
        }
    }

    #[tokio::test]
    async fn test_embed_batch_empty() {
        let provider = MockEmbeddingProvider::new(4);
        let results = provider.embed_batch(&[]).await.unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_openai_embedding_provider_debug() {
        let provider = OpenAiEmbeddingProvider {
            client: reqwest::Client::new(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "text-embedding-3-small".to_string(),
            dimension: 1536,
        };
        let debug = format!("{:?}", provider);
        assert!(debug.contains("OpenAiEmbeddingProvider"));
        assert!(debug.contains("text-embedding-3-small"));
        assert!(debug.contains("1536"));
    }
}
