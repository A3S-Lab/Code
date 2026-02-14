//! Local file-based storage backend

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::RwLock;

use super::StorageBackend;
use super::super::config::VectorIndexConfig;
use super::super::digest::Digest;
use super::super::error::{A3SError, Result};
use super::super::pathway::Pathway;
use super::super::types::Node;
use super::vector_index::VectorIndex;

pub struct LocalStorage {
    base_path: PathBuf,
    cache: RwLock<HashMap<String, Node>>,
    index: RwLock<VectorIndex>,
}

impl LocalStorage {
    pub fn new(base_path: PathBuf, config: &VectorIndexConfig) -> Result<Self> {
        std::fs::create_dir_all(&base_path).map_err(|e| {
            A3SError::Storage(format!("Failed to create storage directory {}: {}", base_path.display(), e))
        })?;
        Ok(Self {
            base_path,
            cache: RwLock::new(HashMap::new()),
            index: RwLock::new(VectorIndex::new(1536, config)),
        })
    }

    fn node_path(&self, pathway: &Pathway) -> PathBuf {
        let safe_name = pathway.as_str().replace("://", "_").replace('/', "_");
        self.base_path.join(format!("{}.json", safe_name))
    }
}

#[async_trait]
impl StorageBackend for LocalStorage {
    async fn put(&self, node: &Node) -> Result<()> {
        let path = self.node_path(&node.pathway);
        let json = serde_json::to_string_pretty(node)?;
        tokio::fs::write(&path, json).await.map_err(|e| {
            A3SError::Storage(format!("Failed to write node {}: {}", path.display(), e))
        })?;
        let key = node.pathway.as_str().to_string();
        if !node.embedding.is_empty() {
            self.index.write().await.insert(key.clone(), node.embedding.clone());
        }
        self.cache.write().await.insert(key, node.clone());
        Ok(())
    }

    async fn get(&self, pathway: &Pathway) -> Result<Node> {
        let key = pathway.as_str();
        if let Some(node) = self.cache.read().await.get(key) {
            return Ok(node.clone());
        }
        let path = self.node_path(pathway);
        if !path.exists() {
            return Err(A3SError::Storage(format!("Node not found: {}", pathway)));
        }
        let json = tokio::fs::read_to_string(&path).await.map_err(|e| {
            A3SError::Storage(format!("Failed to read node {}: {}", path.display(), e))
        })?;
        let node: Node = serde_json::from_str(&json)?;
        self.cache.write().await.insert(key.to_string(), node.clone());
        Ok(node)
    }

    async fn exists(&self, pathway: &Pathway) -> Result<bool> {
        if self.cache.read().await.contains_key(pathway.as_str()) {
            return Ok(true);
        }
        Ok(self.node_path(pathway).exists())
    }

    async fn remove(&self, pathway: &Pathway) -> Result<()> {
        let key = pathway.as_str();
        self.cache.write().await.remove(key);
        self.index.write().await.remove(key);
        let path = self.node_path(pathway);
        if path.exists() {
            tokio::fs::remove_file(&path).await.map_err(|e| {
                A3SError::Storage(format!("Failed to remove {}: {}", path.display(), e))
            })?;
        }
        Ok(())
    }

    async fn list(&self, namespace: &str) -> Result<Vec<Pathway>> {
        let cache = self.cache.read().await;
        let pathways: Vec<Pathway> = cache.keys()
            .filter_map(|k| Pathway::parse(k).ok())
            .filter(|p| p.namespace() == namespace)
            .collect();
        Ok(pathways)
    }

    async fn search_vector(&self, query: &[f32], limit: usize, threshold: f32) -> Result<Vec<(Pathway, f32)>> {
        let results = self.index.read().await.search(query, limit, threshold);
        Ok(results.into_iter().filter_map(|(k, s)| Pathway::parse(&k).ok().map(|p| (p, s))).collect())
    }

    async fn search_text(&self, query: &str, namespace: Option<&str>, limit: usize) -> Result<Vec<(Pathway, f32)>> {
        let query_lower = query.to_lowercase();
        let cache = self.cache.read().await;
        let mut results: Vec<(Pathway, f32)> = cache.values()
            .filter(|n| {
                if let Some(ns) = namespace { if n.pathway.namespace() != ns { return false; } }
                n.content.to_lowercase().contains(&query_lower)
            })
            .map(|n| (n.pathway.clone(), 0.8))
            .collect();
        results.truncate(limit);
        Ok(results)
    }

    async fn stats(&self) -> Result<(usize, usize)> {
        Ok((self.cache.read().await.len(), self.index.read().await.len()))
    }

    async fn flush(&self) -> Result<()> { Ok(()) }

    async fn get_children(&self, pathway: &Pathway) -> Result<Vec<Node>> {
        let prefix = format!("{}/", pathway.as_str());
        let cache = self.cache.read().await;
        Ok(cache.values()
            .filter(|n| {
                let key = n.pathway.as_str();
                key.starts_with(&prefix) && !key[prefix.len()..].contains('/')
            })
            .cloned()
            .collect())
    }

    async fn update_embedding(&self, pathway: &Pathway, embedding: Vec<f32>) -> Result<()> {
        let key = pathway.as_str().to_string();
        let mut cache = self.cache.write().await;
        if let Some(node) = cache.get_mut(&key) {
            node.embedding = embedding.clone();
            self.index.write().await.insert(key, embedding);
        }
        Ok(())
    }

    async fn update_digest(&self, pathway: &Pathway, digest: Digest) -> Result<()> {
        let key = pathway.as_str().to_string();
        let mut cache = self.cache.write().await;
        if let Some(node) = cache.get_mut(&key) {
            node.digest = digest;
        }
        Ok(())
    }
}
