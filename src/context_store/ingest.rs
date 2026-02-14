//! Content ingestion and processing

use std::path::Path;
use std::sync::Arc;
use walkdir::WalkDir;

use super::config::Config;
use super::types::{Node, NodeKind};
use super::digest::DigestGenerator;
use super::embedding::Embedder;
use super::error::Result;
use super::pathway::Pathway;
use super::storage::StorageBackend;
use super::IngestResult;
use crate::llm::LlmClient;

pub struct Processor {
    storage: Arc<dyn StorageBackend>,
    embedder: Arc<dyn Embedder>,
    digest_generator: DigestGenerator,
    config: Config,
}

impl Processor {
    pub fn new(storage: Arc<dyn StorageBackend>, embedder: Arc<dyn Embedder>, llm_client: Option<Arc<dyn LlmClient>>, config: &Config) -> Self {
        let digest_llm = if config.auto_digest { llm_client } else { None };
        Self { storage, embedder, digest_generator: DigestGenerator::new(digest_llm), config: config.clone() }
    }

    pub async fn process(&self, source: &str, target: &Pathway) -> Result<IngestResult> {
        let path = Path::new(source);
        if !path.exists() {
            return Err(super::error::A3SError::Ingest(format!("Source path does not exist: {}", source)));
        }
        let mut nodes_created = 0;
        let mut nodes_updated = 0;
        let mut errors = Vec::new();

        if path.is_file() {
            match self.process_file(path, target).await {
                Ok(created) => { if created { nodes_created += 1; } else { nodes_updated += 1; } }
                Err(e) => errors.push(format!("{}: {}", source, e)),
            }
        } else if path.is_dir() {
            for entry in WalkDir::new(path).follow_links(false).into_iter().filter_entry(|e| !self.should_ignore(e.path())) {
                let entry = match entry { Ok(e) => e, Err(e) => { errors.push(format!("Walk error: {}", e)); continue; } };
                if entry.file_type().is_file() {
                    let rel_path = entry.path().strip_prefix(path).unwrap().to_string_lossy().to_string();
                    let file_pathway = target.join(&rel_path);
                    match self.process_file(entry.path(), &file_pathway).await {
                        Ok(created) => { if created { nodes_created += 1; } else { nodes_updated += 1; } }
                        Err(e) => errors.push(format!("{}: {}", rel_path, e)),
                    }
                }
            }
        }
        Ok(IngestResult { pathway: target.clone(), nodes_created, nodes_updated, errors })
    }

    async fn process_file(&self, path: &Path, pathway: &Pathway) -> Result<bool> {
        let metadata = std::fs::metadata(path)?;
        if metadata.len() > self.config.ingest.max_file_size {
            return Err(super::error::A3SError::Ingest(format!("File too large: {} bytes", metadata.len())));
        }
        let content = std::fs::read_to_string(path)?;
        let kind = self.detect_kind(path);
        let exists = self.storage.exists(pathway).await?;
        let mut node = if exists {
            let mut existing = self.storage.get(pathway).await?;
            existing.update_content(content);
            existing
        } else {
            Node::new(pathway.clone(), kind, content)
        };
        if self.config.auto_digest {
            node.digest = self.digest_generator.generate(&node.content, node.kind).await?;
        }
        let embedding = self.embedder.embed(&node.content).await?;
        node.embedding = embedding;
        self.storage.put(&node).await?;
        Ok(!exists)
    }

    fn detect_kind(&self, path: &Path) -> NodeKind {
        match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
            "md" => NodeKind::Markdown,
            "rs" | "py" | "js" | "ts" | "go" | "java" | "c" | "cpp" | "h" => NodeKind::Code,
            _ => NodeKind::Document,
        }
    }

    fn should_ignore(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        self.config.ingest.ignore_patterns.iter().any(|p| path_str.contains(p))
    }
}
