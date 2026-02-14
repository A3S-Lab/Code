//! Storage backends for context store

pub mod local;
pub mod memory;
pub mod vector_index;

use async_trait::async_trait;
use std::sync::Arc;

use super::config::{StorageBackend as StorageBackendType, StorageConfig};
use super::digest::Digest;
use super::error::Result;
use super::pathway::Pathway;
use super::types::Node;

pub use local::LocalStorage;
pub use memory::MemoryStorage;

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn put(&self, node: &Node) -> Result<()>;
    async fn get(&self, pathway: &Pathway) -> Result<Node>;
    async fn exists(&self, pathway: &Pathway) -> Result<bool>;
    async fn remove(&self, pathway: &Pathway) -> Result<()>;
    async fn list(&self, namespace: &str) -> Result<Vec<Pathway>>;
    async fn search_vector(&self, query: &[f32], limit: usize, threshold: f32) -> Result<Vec<(Pathway, f32)>>;
    async fn search_text(&self, query: &str, namespace: Option<&str>, limit: usize) -> Result<Vec<(Pathway, f32)>>;
    async fn stats(&self) -> Result<(usize, usize)>;
    async fn flush(&self) -> Result<()>;
    async fn get_children(&self, pathway: &Pathway) -> Result<Vec<Node>>;
    async fn update_embedding(&self, pathway: &Pathway, embedding: Vec<f32>) -> Result<()>;
    async fn update_digest(&self, pathway: &Pathway, digest: Digest) -> Result<()>;
}

pub fn create_backend(config: &StorageConfig) -> Result<Arc<dyn StorageBackend>> {
    match config.backend {
        StorageBackendType::Memory => {
            Ok(Arc::new(MemoryStorage::new(&config.vector_index)))
        }
        StorageBackendType::Local => {
            Ok(Arc::new(LocalStorage::new(config.path.clone(), &config.vector_index)?))
        }
        StorageBackendType::Remote => {
            Err(super::error::A3SError::Storage("Remote storage not yet implemented".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::config::StorageConfig;

    #[test]
    fn test_create_memory_backend() {
        let mut config = StorageConfig::default();
        config.backend = StorageBackendType::Memory;
        let backend = create_backend(&config);
        assert!(backend.is_ok());
    }

    #[test]
    fn test_create_remote_backend_fails() {
        let mut config = StorageConfig::default();
        config.backend = StorageBackendType::Remote;
        let backend = create_backend(&config);
        assert!(backend.is_err());
    }
}
