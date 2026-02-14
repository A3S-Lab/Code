//! A3S Context Store - unified context storage and retrieval
//!
//! Provides semantic search, digest generation, and hierarchical
//! context management using the session's LLM for all AI operations.

pub mod config;
pub mod digest;
pub mod embedding;
pub mod error;
pub mod ingest;
pub mod pathway;
pub mod rerank;
pub mod retrieval;
pub mod session;
pub mod storage;
pub mod telemetry;
pub mod types;

use std::sync::Arc;

use crate::llm::LlmClient;
use config::Config;
use embedding::{create_embedder, Embedder};
use error::Result;
use ingest::Processor;
use pathway::Pathway;
use rerank::create_reranker;
use retrieval::Retriever;
use storage::{create_backend, StorageBackend};
use telemetry::Metrics;

/// Provider info extracted from session config
#[derive(Debug, Clone, Default)]
pub struct ProviderInfo {
    pub api_base: Option<String>,
    pub api_key: Option<String>,
    pub embedding_model: Option<String>,
}

/// Result of an ingest operation
#[derive(Debug, Clone)]
pub struct IngestResult {
    pub pathway: Pathway,
    pub nodes_created: usize,
    pub nodes_updated: usize,
    pub errors: Vec<String>,
}

/// Options for querying context
#[derive(Debug, Clone, Default)]
pub struct QueryOptions {
    pub namespace: Option<String>,
    pub limit: Option<usize>,
    pub hierarchical: bool,
}

/// Result of a context query
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub matches: Vec<MatchedNode>,
    pub total: usize,
}

/// A matched node from a query
#[derive(Debug, Clone)]
pub struct MatchedNode {
    pub pathway: Pathway,
    pub content: String,
    pub score: f32,
}

/// Storage statistics
#[derive(Debug, Clone)]
pub struct StorageStats {
    pub total_nodes: usize,
    pub total_vectors: usize,
    pub namespaces: Vec<NamespaceStats>,
}

/// Per-namespace statistics
#[derive(Debug, Clone)]
pub struct NamespaceStats {
    pub name: String,
    pub node_count: usize,
}

/// Main context store client
pub struct A3SContextClient {
    storage: Arc<dyn StorageBackend>,
    embedder: Arc<dyn Embedder>,
    processor: Processor,
    retriever: Retriever,
    metrics: Arc<Metrics>,
    config: Config,
}

impl A3SContextClient {
    /// Create a new context client using the session's LLM
    pub fn new(
        config: Config,
        llm_client: Option<Arc<dyn LlmClient>>,
        provider: ProviderInfo,
    ) -> Result<Self> {
        let storage = create_backend(&config.storage)?;
        let embedder = create_embedder(
            provider.api_base.as_deref(),
            provider.api_key.as_deref(),
            provider.embedding_model.as_deref(),
            config.embedding_dimension,
        )?;
        let reranker = create_reranker(llm_client.clone());
        let processor = Processor::new(
            storage.clone(),
            embedder.clone(),
            llm_client,
            &config,
        );
        let retriever = Retriever::new(
            storage.clone(),
            embedder.clone(),
            &config.retrieval,
            reranker,
        );
        Ok(Self {
            storage,
            embedder,
            processor,
            retriever,
            metrics: Arc::new(Metrics::new()),
            config,
        })
    }

    /// Ingest content from a source path
    pub async fn ingest(&self, source: &str, namespace: &str) -> Result<IngestResult> {
        let target = Pathway::new(namespace, "");
        let result = self.processor.process(source, &target).await?;
        for _ in 0..result.nodes_created {
            self.metrics.record_ingest();
        }
        Ok(result)
    }

    /// Query for relevant context
    pub async fn query(&self, query: &str, options: QueryOptions) -> Result<QueryResult> {
        self.metrics.record_query();
        let results = if options.hierarchical {
            self.retriever.search_hierarchical(query, options.namespace.as_deref(), options.limit).await?
        } else {
            self.retriever.search(query, options.namespace.as_deref(), options.limit).await?
        };
        let matches: Vec<MatchedNode> = results.into_iter()
            .map(|(pathway, content, score)| MatchedNode { pathway, content, score })
            .collect();
        let total = matches.len();
        Ok(QueryResult { matches, total })
    }

    /// Get storage statistics
    pub async fn stats(&self) -> Result<StorageStats> {
        let (total_nodes, total_vectors) = self.storage.stats().await?;
        Ok(StorageStats {
            total_nodes,
            total_vectors,
            namespaces: Vec::new(),
        })
    }

    /// Get metrics snapshot
    pub fn metrics(&self) -> telemetry::MetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Get the embedder
    pub fn embedder(&self) -> &Arc<dyn Embedder> {
        &self.embedder
    }

    /// Get the storage backend
    pub fn storage(&self) -> &Arc<dyn StorageBackend> {
        &self.storage
    }

    /// Get the config
    pub fn config(&self) -> &Config {
        &self.config
    }
}

/// Context provider that bridges context_store to the agent's ContextProvider trait
pub struct A3SContextProvider {
    client: Arc<A3SContextClient>,
}

impl A3SContextProvider {
    pub fn new(client: Arc<A3SContextClient>) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl crate::context::ContextProvider for A3SContextProvider {
    fn name(&self) -> &str {
        "a3s-context"
    }

    async fn query(&self, query: &crate::context::ContextQuery) -> anyhow::Result<crate::context::ContextResult> {
        let options = QueryOptions {
            namespace: None,
            limit: Some(query.max_results),
            hierarchical: true,
        };
        let result = self.client.query(&query.query, options).await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        let mut ctx_result = crate::context::ContextResult::new("a3s-context");
        for m in result.matches {
            let item = crate::context::ContextItem::new(
                m.pathway.as_str(),
                crate::context::ContextType::Resource,
                m.content,
            )
            .with_relevance(m.score)
            .with_source(m.pathway.as_str());
            ctx_result.add_item(item);
        }
        Ok(ctx_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_info_default() {
        let info = ProviderInfo::default();
        assert!(info.api_base.is_none());
        assert!(info.api_key.is_none());
        assert!(info.embedding_model.is_none());
    }

    #[test]
    fn test_query_options_default() {
        let opts = QueryOptions::default();
        assert!(opts.namespace.is_none());
        assert!(opts.limit.is_none());
        assert!(!opts.hierarchical);
    }

    #[test]
    fn test_create_client_memory_backend() {
        let mut config = Config::default();
        config.storage.backend = config::StorageBackend::Memory;
        let client = A3SContextClient::new(config, None, ProviderInfo::default());
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_client_stats() {
        let mut config = Config::default();
        config.storage.backend = config::StorageBackend::Memory;
        let client = A3SContextClient::new(config, None, ProviderInfo::default()).unwrap();
        let stats = client.stats().await.unwrap();
        assert_eq!(stats.total_nodes, 0);
    }

    #[test]
    fn test_client_metrics() {
        let mut config = Config::default();
        config.storage.backend = config::StorageBackend::Memory;
        let client = A3SContextClient::new(config, None, ProviderInfo::default()).unwrap();
        let metrics = client.metrics();
        assert_eq!(metrics.nodes_ingested, 0);
    }
}
