//! Vector Store Extension Point
//!
//! Provides trait and in-memory implementation for storing and querying
//! vector embeddings with cosine similarity search.

use super::embedding::Embedding;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metadata attached to a stored vector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorMetadata {
    /// Source file path or URI
    pub source: String,
    /// Content type (e.g., "code", "markdown", "comment")
    pub content_type: String,
    /// The original text content
    pub content: String,
    /// Estimated token count
    pub token_count: usize,
    /// Additional key-value metadata
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// A stored vector entry (embedding + metadata)
#[derive(Debug, Clone)]
pub struct VectorEntry {
    /// Unique identifier
    pub id: String,
    /// The embedding vector
    pub embedding: Embedding,
    /// Associated metadata
    pub metadata: VectorMetadata,
}

/// Search result from vector store
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    /// The matched entry ID
    pub id: String,
    /// Cosine similarity score (0.0 to 1.0)
    pub score: f32,
    /// Associated metadata
    pub metadata: VectorMetadata,
}

/// Trait for vector storage backends
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Store name for logging
    fn name(&self) -> &str;

    /// Number of stored vectors
    async fn len(&self) -> usize;

    /// Check if store is empty
    async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Insert a single vector entry
    async fn insert(&self, entry: VectorEntry) -> Result<()>;

    /// Insert multiple vector entries (batch)
    async fn insert_batch(&self, entries: Vec<VectorEntry>) -> Result<()> {
        for entry in entries {
            self.insert(entry).await?;
        }
        Ok(())
    }

    /// Search for the top-k most similar vectors
    async fn search(&self, query: &Embedding, top_k: usize) -> Result<Vec<VectorSearchResult>>;

    /// Delete a vector by ID
    async fn delete(&self, id: &str) -> Result<bool>;

    /// Clear all stored vectors
    async fn clear(&self) -> Result<()>;
}

/// Compute cosine similarity between two vectors
///
/// Returns a value between -1.0 and 1.0 (1.0 = identical direction).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

/// In-memory vector store using brute-force cosine similarity
///
/// Suitable for small-to-medium codebases (< 100k chunks).
/// For larger datasets, use an external vector DB (Qdrant, Milvus, etc.).
pub struct InMemoryVectorStore {
    entries: tokio::sync::RwLock<Vec<VectorEntry>>,
}

impl InMemoryVectorStore {
    /// Create a new empty in-memory vector store
    pub fn new() -> Self {
        Self {
            entries: tokio::sync::RwLock::new(Vec::new()),
        }
    }
}

impl Default for InMemoryVectorStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VectorStore for InMemoryVectorStore {
    fn name(&self) -> &str {
        "in-memory"
    }

    async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    async fn insert(&self, entry: VectorEntry) -> Result<()> {
        let mut entries = self.entries.write().await;
        // Upsert: replace if ID exists
        if let Some(pos) = entries.iter().position(|e| e.id == entry.id) {
            entries[pos] = entry;
        } else {
            entries.push(entry);
        }
        Ok(())
    }

    async fn insert_batch(&self, new_entries: Vec<VectorEntry>) -> Result<()> {
        let mut entries = self.entries.write().await;
        for entry in new_entries {
            if let Some(pos) = entries.iter().position(|e| e.id == entry.id) {
                entries[pos] = entry;
            } else {
                entries.push(entry);
            }
        }
        Ok(())
    }

    async fn search(&self, query: &Embedding, top_k: usize) -> Result<Vec<VectorSearchResult>> {
        let entries = self.entries.read().await;

        let mut scored: Vec<(usize, f32)> = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| (i, cosine_similarity(query, &entry.embedding)))
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        Ok(scored
            .into_iter()
            .map(|(i, score)| VectorSearchResult {
                id: entries[i].id.clone(),
                score,
                metadata: entries[i].metadata.clone(),
            })
            .collect())
    }

    async fn delete(&self, id: &str) -> Result<bool> {
        let mut entries = self.entries.write().await;
        let initial_len = entries.len();
        entries.retain(|e| e.id != id);
        Ok(entries.len() < initial_len)
    }

    async fn clear(&self) -> Result<()> {
        let mut entries = self.entries.write().await;
        entries.clear();
        Ok(())
    }
}

impl std::fmt::Debug for InMemoryVectorStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryVectorStore")
            .field("entries", &"<locked>")
            .finish()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, embedding: Vec<f32>, content: &str) -> VectorEntry {
        VectorEntry {
            id: id.to_string(),
            embedding,
            metadata: VectorMetadata {
                source: format!("test://{}", id),
                content_type: "test".to_string(),
                content: content.to_string(),
                token_count: content.split_whitespace().count(),
                extra: HashMap::new(),
            },
        }
    }

    // -- cosine_similarity tests --

    #[test]
    fn test_cosine_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_different_lengths() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_empty() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_zero_vector() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similar_vectors() {
        let a = vec![1.0, 1.0, 0.0];
        let b = vec![1.0, 0.9, 0.1];
        let sim = cosine_similarity(&a, &b);
        assert!(sim > 0.95); // Very similar
    }

    // -- InMemoryVectorStore tests --

    #[tokio::test]
    async fn test_store_new_empty() {
        let store = InMemoryVectorStore::new();
        assert_eq!(store.name(), "in-memory");
        assert_eq!(store.len().await, 0);
        assert!(store.is_empty().await);
    }

    #[tokio::test]
    async fn test_store_insert_and_len() {
        let store = InMemoryVectorStore::new();
        store
            .insert(make_entry("e1", vec![1.0, 0.0], "hello"))
            .await
            .unwrap();
        assert_eq!(store.len().await, 1);
        assert!(!store.is_empty().await);
    }

    #[tokio::test]
    async fn test_store_upsert() {
        let store = InMemoryVectorStore::new();
        store
            .insert(make_entry("e1", vec![1.0, 0.0], "original"))
            .await
            .unwrap();
        store
            .insert(make_entry("e1", vec![0.0, 1.0], "updated"))
            .await
            .unwrap();
        assert_eq!(store.len().await, 1);

        let results = store.search(&vec![0.0, 1.0], 1).await.unwrap();
        assert_eq!(results[0].metadata.content, "updated");
    }

    #[tokio::test]
    async fn test_store_insert_batch() {
        let store = InMemoryVectorStore::new();
        let entries = vec![
            make_entry("e1", vec![1.0, 0.0], "first"),
            make_entry("e2", vec![0.0, 1.0], "second"),
            make_entry("e3", vec![0.7, 0.7], "third"),
        ];
        store.insert_batch(entries).await.unwrap();
        assert_eq!(store.len().await, 3);
    }

    #[tokio::test]
    async fn test_store_search_top_k() {
        let store = InMemoryVectorStore::new();
        store
            .insert_batch(vec![
                make_entry("e1", vec![1.0, 0.0, 0.0], "rust code"),
                make_entry("e2", vec![0.0, 1.0, 0.0], "python code"),
                make_entry("e3", vec![0.9, 0.1, 0.0], "rust similar"),
            ])
            .await
            .unwrap();

        let query = vec![1.0, 0.0, 0.0];
        let results = store.search(&query, 2).await.unwrap();

        assert_eq!(results.len(), 2);
        // e1 should be first (exact match), e3 second (similar)
        assert_eq!(results[0].id, "e1");
        assert!((results[0].score - 1.0).abs() < 1e-6);
        assert_eq!(results[1].id, "e3");
        assert!(results[1].score > 0.9);
    }

    #[tokio::test]
    async fn test_store_search_empty() {
        let store = InMemoryVectorStore::new();
        let results = store.search(&vec![1.0, 0.0], 5).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_store_search_top_k_larger_than_entries() {
        let store = InMemoryVectorStore::new();
        store
            .insert(make_entry("e1", vec![1.0, 0.0], "only one"))
            .await
            .unwrap();

        let results = store.search(&vec![1.0, 0.0], 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_store_delete() {
        let store = InMemoryVectorStore::new();
        store
            .insert(make_entry("e1", vec![1.0, 0.0], "hello"))
            .await
            .unwrap();

        let deleted = store.delete("e1").await.unwrap();
        assert!(deleted);
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn test_store_delete_nonexistent() {
        let store = InMemoryVectorStore::new();
        let deleted = store.delete("nope").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_store_clear() {
        let store = InMemoryVectorStore::new();
        store
            .insert_batch(vec![
                make_entry("e1", vec![1.0, 0.0], "a"),
                make_entry("e2", vec![0.0, 1.0], "b"),
            ])
            .await
            .unwrap();

        store.clear().await.unwrap();
        assert!(store.is_empty().await);
    }

    #[tokio::test]
    async fn test_store_search_ordering() {
        let store = InMemoryVectorStore::new();
        store
            .insert_batch(vec![
                make_entry("far", vec![0.0, 0.0, 1.0], "far away"),
                make_entry("close", vec![0.95, 0.05, 0.0], "very close"),
                make_entry("exact", vec![1.0, 0.0, 0.0], "exact match"),
                make_entry("medium", vec![0.7, 0.3, 0.0], "medium distance"),
            ])
            .await
            .unwrap();

        let results = store.search(&vec![1.0, 0.0, 0.0], 4).await.unwrap();
        assert_eq!(results[0].id, "exact");
        assert_eq!(results[1].id, "close");
        assert_eq!(results[2].id, "medium");
        assert_eq!(results[3].id, "far");
    }

    #[test]
    fn test_vector_metadata_serialization() {
        let meta = VectorMetadata {
            source: "file:src/main.rs".to_string(),
            content_type: "code".to_string(),
            content: "fn main() {}".to_string(),
            token_count: 3,
            extra: HashMap::new(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: VectorMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.source, "file:src/main.rs");
        assert_eq!(parsed.content_type, "code");
        assert_eq!(parsed.token_count, 3);
    }

    #[test]
    fn test_store_debug() {
        let store = InMemoryVectorStore::new();
        let debug = format!("{:?}", store);
        assert!(debug.contains("InMemoryVectorStore"));
    }

    #[test]
    fn test_store_default() {
        let store = InMemoryVectorStore::default();
        let debug = format!("{:?}", store);
        assert!(debug.contains("InMemoryVectorStore"));
    }
}
