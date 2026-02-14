//! Configuration for A3S Context Store

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default = "default_embedding_dimension")]
    pub embedding_dimension: usize,
    #[serde(default)]
    pub retrieval: RetrievalConfig,
    #[serde(default)]
    pub ingest: IngestConfig,
    #[serde(default = "default_auto_digest")]
    pub auto_digest: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            storage: StorageConfig::default(),
            embedding_dimension: default_embedding_dimension(),
            retrieval: RetrievalConfig::default(),
            ingest: IngestConfig::default(),
            auto_digest: default_auto_digest(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_storage_backend")]
    pub backend: StorageBackend,
    #[serde(default = "default_storage_path")]
    pub path: PathBuf,
    pub url: Option<String>,
    #[serde(default)]
    pub vector_index: VectorIndexConfig,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            path: default_storage_path(),
            url: None,
            vector_index: VectorIndexConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    Local,
    Remote,
    Memory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorIndexConfig {
    #[serde(default = "default_index_type")]
    pub index_type: String,
    #[serde(default = "default_hnsw_m")]
    pub hnsw_m: usize,
    #[serde(default = "default_hnsw_ef_construction")]
    pub hnsw_ef_construction: usize,
}

impl Default for VectorIndexConfig {
    fn default() -> Self {
        Self {
            index_type: default_index_type(),
            hnsw_m: default_hnsw_m(),
            hnsw_ef_construction: default_hnsw_ef_construction(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    #[serde(default = "default_limit")]
    pub default_limit: usize,
    #[serde(default = "default_threshold")]
    pub score_threshold: f32,
    #[serde(default = "default_hierarchical")]
    pub hierarchical: bool,
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    #[serde(default)]
    pub rerank: bool,
    pub rerank_top_n: Option<usize>,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            default_limit: default_limit(),
            score_threshold: default_threshold(),
            hierarchical: default_hierarchical(),
            max_depth: default_max_depth(),
            rerank: false,
            rerank_top_n: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestConfig {
    #[serde(default = "default_extensions")]
    pub extensions: Vec<String>,
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,
    #[serde(default = "default_ignore_patterns")]
    pub ignore_patterns: Vec<String>,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            extensions: default_extensions(),
            max_file_size: default_max_file_size(),
            chunk_size: default_chunk_size(),
            chunk_overlap: default_chunk_overlap(),
            ignore_patterns: default_ignore_patterns(),
        }
    }
}

fn default_storage_backend() -> StorageBackend { StorageBackend::Local }
fn default_storage_path() -> PathBuf { PathBuf::from("./a3s_data") }
fn default_index_type() -> String { "hnsw".to_string() }
fn default_hnsw_m() -> usize { 16 }
fn default_hnsw_ef_construction() -> usize { 200 }
fn default_embedding_dimension() -> usize { 1536 }
fn default_auto_digest() -> bool { true }
fn default_limit() -> usize { 10 }
fn default_threshold() -> f32 { 0.5 }
fn default_hierarchical() -> bool { true }
fn default_max_depth() -> usize { 3 }
fn default_extensions() -> Vec<String> {
    vec!["md","txt","rs","py","js","ts","go","java","c","cpp","h","json","yaml","toml"]
        .into_iter().map(String::from).collect()
}
fn default_max_file_size() -> u64 { 10 * 1024 * 1024 }
fn default_chunk_size() -> usize { 1000 }
fn default_chunk_overlap() -> usize { 200 }
fn default_ignore_patterns() -> Vec<String> {
    vec![".git","node_modules","target","__pycache__",".venv","*.pyc","*.pyo",".DS_Store"]
        .into_iter().map(String::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.storage.backend, StorageBackend::Local);
        assert_eq!(config.embedding_dimension, 1536);
        assert!(config.auto_digest);
    }

    #[test]
    fn test_retrieval_config_default() {
        let config = RetrievalConfig::default();
        assert_eq!(config.default_limit, 10);
        assert!(!config.rerank);
    }

    #[test]
    fn test_ingest_config_default() {
        let config = IngestConfig::default();
        assert!(config.extensions.contains(&"rs".to_string()));
        assert_eq!(config.max_file_size, 10 * 1024 * 1024);
    }
}
