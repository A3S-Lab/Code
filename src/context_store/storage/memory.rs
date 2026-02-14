//! In-memory storage backend for testing

use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

use super::StorageBackend;
use super::super::config::VectorIndexConfig;
use super::super::digest::Digest;
use super::super::error::{A3SError, Result};
use super::super::pathway::Pathway;
use super::super::types::Node;
use super::vector_index::VectorIndex;

pub struct MemoryStorage {
    nodes: RwLock<HashMap<String, Node>>,
    index: RwLock<VectorIndex>,
}

impl MemoryStorage {
    pub fn new(config: &VectorIndexConfig) -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            index: RwLock::new(VectorIndex::new(1536, config)),
        }
    }
}

#[async_trait]
impl StorageBackend for MemoryStorage {
    async fn put(&self, node: &Node) -> Result<()> {
        let key = node.pathway.as_str().to_string();
        if !node.embedding.is_empty() {
            self.index.write().await.insert(key.clone(), node.embedding.clone());
        }
        self.nodes.write().await.insert(key, node.clone());
        Ok(())
    }

    async fn get(&self, pathway: &Pathway) -> Result<Node> {
        self.nodes.read().await.get(pathway.as_str())
            .cloned()
            .ok_or_else(|| A3SError::Storage(format!("Node not found: {}", pathway)))
    }

    async fn exists(&self, pathway: &Pathway) -> Result<bool> {
        Ok(self.nodes.read().await.contains_key(pathway.as_str()))
    }

    async fn remove(&self, pathway: &Pathway) -> Result<()> {
        let key = pathway.as_str();
        self.nodes.write().await.remove(key);
        self.index.write().await.remove(key);
        Ok(())
    }

    async fn list(&self, namespace: &str) -> Result<Vec<Pathway>> {
        let nodes = self.nodes.read().await;
        let pathways: Vec<Pathway> = nodes.keys()
            .filter(|k| {
                Pathway::parse(k).map(|p| p.namespace() == namespace).unwrap_or(false)
            })
            .filter_map(|k| Pathway::parse(k).ok())
            .collect();
        Ok(pathways)
    }

    async fn search_vector(&self, query: &[f32], limit: usize, threshold: f32) -> Result<Vec<(Pathway, f32)>> {
        let results = self.index.read().await.search(query, limit, threshold);
        let pathways: Vec<(Pathway, f32)> = results.into_iter()
            .filter_map(|(key, score)| Pathway::parse(&key).ok().map(|p| (p, score)))
            .collect();
        Ok(pathways)
    }

    async fn search_text(&self, query: &str, namespace: Option<&str>, limit: usize) -> Result<Vec<(Pathway, f32)>> {
        let query_lower = query.to_lowercase();
        let nodes = self.nodes.read().await;
        let mut results: Vec<(Pathway, f32)> = nodes.values()
            .filter(|n| {
                if let Some(ns) = namespace {
                    if n.pathway.namespace() != ns { return false; }
                }
                n.content.to_lowercase().contains(&query_lower)
                    || n.digest.brief.to_lowercase().contains(&query_lower)
            })
            .map(|n| {
                let score = if n.content.to_lowercase().contains(&query_lower) { 0.8 } else { 0.5 };
                (n.pathway.clone(), score)
            })
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        Ok(results)
    }

    async fn stats(&self) -> Result<(usize, usize)> {
        let node_count = self.nodes.read().await.len();
        let vector_count = self.index.read().await.len();
        Ok((node_count, vector_count))
    }

    async fn flush(&self) -> Result<()> { Ok(()) }

    async fn get_children(&self, pathway: &Pathway) -> Result<Vec<Node>> {
        let prefix = format!("{}/", pathway.as_str());
        let nodes = self.nodes.read().await;
        let children: Vec<Node> = nodes.values()
            .filter(|n| {
                let key = n.pathway.as_str();
                key.starts_with(&prefix) && !key[prefix.len()..].contains('/')
            })
            .cloned()
            .collect();
        Ok(children)
    }

    async fn update_embedding(&self, pathway: &Pathway, embedding: Vec<f32>) -> Result<()> {
        let key = pathway.as_str().to_string();
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(&key) {
            node.embedding = embedding.clone();
            self.index.write().await.insert(key, embedding);
        }
        Ok(())
    }

    async fn update_digest(&self, pathway: &Pathway, digest: Digest) -> Result<()> {
        let key = pathway.as_str().to_string();
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(&key) {
            node.digest = digest;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::super::types::NodeKind;

    fn test_storage() -> MemoryStorage {
        MemoryStorage::new(&VectorIndexConfig::default())
    }

    #[tokio::test]
    async fn test_put_and_get() {
        let storage = test_storage();
        let pathway = Pathway::new("test", "doc.md");
        let node = Node::new(pathway.clone(), NodeKind::Document, "hello".to_string());
        storage.put(&node).await.unwrap();
        let retrieved = storage.get(&pathway).await.unwrap();
        assert_eq!(retrieved.content, "hello");
    }

    #[tokio::test]
    async fn test_exists() {
        let storage = test_storage();
        let pathway = Pathway::new("test", "doc.md");
        assert!(!storage.exists(&pathway).await.unwrap());
        let node = Node::new(pathway.clone(), NodeKind::Document, "hello".to_string());
        storage.put(&node).await.unwrap();
        assert!(storage.exists(&pathway).await.unwrap());
    }

    #[tokio::test]
    async fn test_remove() {
        let storage = test_storage();
        let pathway = Pathway::new("test", "doc.md");
        let node = Node::new(pathway.clone(), NodeKind::Document, "hello".to_string());
        storage.put(&node).await.unwrap();
        storage.remove(&pathway).await.unwrap();
        assert!(!storage.exists(&pathway).await.unwrap());
    }

    #[tokio::test]
    async fn test_list() {
        let storage = test_storage();
        storage.put(&Node::new(Pathway::new("ns1", "a.md"), NodeKind::Document, "a".into())).await.unwrap();
        storage.put(&Node::new(Pathway::new("ns1", "b.md"), NodeKind::Document, "b".into())).await.unwrap();
        storage.put(&Node::new(Pathway::new("ns2", "c.md"), NodeKind::Document, "c".into())).await.unwrap();
        let list = storage.list("ns1").await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_text_search() {
        let storage = test_storage();
        storage.put(&Node::new(Pathway::new("test", "a.md"), NodeKind::Document, "hello world".into())).await.unwrap();
        storage.put(&Node::new(Pathway::new("test", "b.md"), NodeKind::Document, "goodbye world".into())).await.unwrap();
        let results = storage.search_text("hello", None, 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_stats() {
        let storage = test_storage();
        let (nodes, _) = storage.stats().await.unwrap();
        assert_eq!(nodes, 0);
        storage.put(&Node::new(Pathway::new("test", "a.md"), NodeKind::Document, "a".into())).await.unwrap();
        let (nodes, _) = storage.stats().await.unwrap();
        assert_eq!(nodes, 1);
    }
}
