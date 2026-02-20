//! CodeSearch tool - Semantic code search via vector embeddings
//!
//! Wraps the VectorContextProvider to expose semantic code search
//! as an LLM-callable tool. The LLM can search for code by meaning
//! rather than exact text patterns.

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::context::embedding::EmbeddingProvider;
use crate::context::vector_provider::{VectorContextConfig, VectorContextProvider};
use crate::context::vector_store::VectorStore;
use crate::context::{ContextProvider, ContextQuery};

/// Semantic code search tool backed by vector embeddings.
///
/// Indexes workspace files on first use and searches by cosine similarity.
/// Unlike `grep` (exact pattern matching), this finds semantically related
/// code even when the exact terms don't appear.
pub struct CodeSearchTool<E: EmbeddingProvider + 'static, S: VectorStore + 'static> {
    provider: Arc<VectorContextProvider<E, S>>,
    /// Track whether we've indexed (separate from provider's internal flag
    /// because the tool may be created before the workspace is known)
    initialized: RwLock<bool>,
}

impl<E: EmbeddingProvider + 'static, S: VectorStore + 'static> CodeSearchTool<E, S> {
    /// Create a new CodeSearch tool with a pre-configured provider.
    pub fn new(provider: Arc<VectorContextProvider<E, S>>) -> Self {
        Self {
            provider,
            initialized: RwLock::new(false),
        }
    }
}

#[async_trait]
impl<E: EmbeddingProvider + 'static, S: VectorStore + 'static> Tool for CodeSearchTool<E, S> {
    fn name(&self) -> &str {
        "codesearch"
    }

    fn description(&self) -> &str {
        "Search code semantically using vector embeddings. Finds code by meaning, \
         not just exact text. Use this when you need to find code related to a concept, \
         function, or feature — even if you don't know the exact names or patterns."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language description of the code you're looking for (e.g., 'authentication middleware', 'database connection pooling', 'error handling for HTTP requests')"
                },
                "top_k": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 5, max: 20)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let query_text = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return Ok(ToolOutput::error("query parameter is required")),
        };

        let top_k = args
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|k| (k as usize).min(20))
            .unwrap_or(5);

        // Query the vector provider
        let query = ContextQuery::new(query_text).with_max_results(top_k);
        let result = match self.provider.query(&query).await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolOutput::error(format!(
                    "Code search failed: {}",
                    e
                )));
            }
        };

        if result.items.is_empty() {
            return Ok(ToolOutput::success(
                "No results found. Try a different query or check that the workspace has been indexed.",
            ));
        }

        // Format results
        let mut output = String::new();
        output.push_str(&format!(
            "Found {} result(s) for: \"{}\"\n\n",
            result.items.len(),
            query_text
        ));

        for (i, item) in result.items.iter().enumerate() {
            let source = item.source.as_deref().unwrap_or("unknown");
            let relevance = format!("{:.2}", item.relevance);

            output.push_str(&format!(
                "--- Result {} (relevance: {}) ---\n",
                i + 1,
                relevance
            ));
            output.push_str(&format!("Source: {}\n", source));
            output.push_str(&item.content);
            output.push_str("\n\n");
        }

        Ok(ToolOutput::success(output))
    }
}

// ============================================================================
// Factory function for easy creation
// ============================================================================

/// Create a CodeSearch tool with an in-memory vector store.
///
/// This is the simplest way to add semantic code search to a session.
/// The workspace is indexed on first query (auto-index).
pub fn create_codesearch_tool<E: EmbeddingProvider + 'static>(
    workspace_path: impl Into<std::path::PathBuf>,
    embedder: E,
) -> CodeSearchTool<E, crate::context::vector_store::InMemoryVectorStore> {
    let config = VectorContextConfig::new(workspace_path);
    let store = crate::context::vector_store::InMemoryVectorStore::new();
    let provider = Arc::new(VectorContextProvider::new(config, embedder, store));
    CodeSearchTool::new(provider)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::vector_store::InMemoryVectorStore;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    /// Simple mock embedding provider
    struct MockEmbedder {
        dim: usize,
    }

    impl MockEmbedder {
        fn new(dim: usize) -> Self {
            Self { dim }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for MockEmbedder {
        fn name(&self) -> &str {
            "mock"
        }
        fn dimension(&self) -> usize {
            self.dim
        }
        async fn embed(&self, text: &str) -> Result<Vec<f32>> {
            let mut emb = vec![0.0f32; self.dim];
            for (i, byte) in text.bytes().enumerate() {
                emb[i % self.dim] += (byte as f32) / 255.0;
            }
            let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for v in &mut emb {
                    *v /= norm;
                }
            }
            Ok(emb)
        }
    }

    fn setup_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        let mut f1 = File::create(root.join("auth.rs")).unwrap();
        writeln!(f1, "pub fn verify_jwt(token: &str) -> Result<Claims> {{\n    // JWT verification\n    todo!()\n}}").unwrap();

        let mut f2 = File::create(root.join("db.rs")).unwrap();
        writeln!(f2, "pub fn connect_pool(url: &str) -> Pool {{\n    // Database connection pooling\n    todo!()\n}}").unwrap();

        let mut f3 = File::create(root.join("handler.rs")).unwrap();
        writeln!(f3, "pub async fn handle_request(req: Request) -> Response {{\n    // HTTP request handler\n    todo!()\n}}").unwrap();

        dir
    }

    fn make_ctx(workspace: &str) -> ToolContext {
        ToolContext {
            workspace: std::path::PathBuf::from(workspace),
            session_id: None,
            event_tx: None,
            search_config: None,
            sandbox: None,
        }
    }

    #[test]
    fn test_tool_name() {
        let tool = create_codesearch_tool("/tmp", MockEmbedder::new(8));
        assert_eq!(tool.name(), "codesearch");
    }

    #[test]
    fn test_tool_description() {
        let tool = create_codesearch_tool("/tmp", MockEmbedder::new(8));
        assert!(!tool.description().is_empty());
        assert!(tool.description().contains("semantic"));
    }

    #[test]
    fn test_tool_parameters() {
        let tool = create_codesearch_tool("/tmp", MockEmbedder::new(8));
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["query"].is_object());
        assert!(params["properties"]["top_k"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("query")));
    }

    #[tokio::test]
    async fn test_execute_missing_query() {
        let tool = create_codesearch_tool("/tmp", MockEmbedder::new(8));
        let ctx = make_ctx("/tmp");
        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_execute_search() {
        let dir = setup_workspace();
        let config = VectorContextConfig::new(dir.path()).with_min_relevance(0.0);
        let provider = Arc::new(VectorContextProvider::new(
            config,
            MockEmbedder::new(8),
            InMemoryVectorStore::new(),
        ));
        provider.index().await.unwrap();

        let tool = CodeSearchTool::new(provider);
        let ctx = make_ctx(&dir.path().to_string_lossy());

        let result = tool
            .execute(&serde_json::json!({"query": "authentication JWT"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("Found"));
        assert!(result.content.contains("result"));
    }

    #[tokio::test]
    async fn test_execute_with_top_k() {
        let dir = setup_workspace();
        let config = VectorContextConfig::new(dir.path()).with_min_relevance(0.0);
        let provider = Arc::new(VectorContextProvider::new(
            config,
            MockEmbedder::new(8),
            InMemoryVectorStore::new(),
        ));
        provider.index().await.unwrap();

        let tool = CodeSearchTool::new(provider);
        let ctx = make_ctx(&dir.path().to_string_lossy());

        let result = tool
            .execute(
                &serde_json::json!({"query": "database", "top_k": 1}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        // Should have at most 1 result section
        let result_count = result.content.matches("--- Result").count();
        assert!(result_count <= 1);
    }

    #[tokio::test]
    async fn test_execute_empty_workspace() {
        let dir = TempDir::new().unwrap();
        let tool = create_codesearch_tool(dir.path(), MockEmbedder::new(8));
        let ctx = make_ctx(&dir.path().to_string_lossy());

        let result = tool
            .execute(&serde_json::json!({"query": "anything"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("No results"));
    }

    #[tokio::test]
    async fn test_execute_top_k_clamped() {
        let dir = setup_workspace();
        let config = VectorContextConfig::new(dir.path()).with_min_relevance(0.0);
        let provider = Arc::new(VectorContextProvider::new(
            config,
            MockEmbedder::new(8),
            InMemoryVectorStore::new(),
        ));
        provider.index().await.unwrap();

        let tool = CodeSearchTool::new(provider);
        let ctx = make_ctx(&dir.path().to_string_lossy());

        // top_k=100 should be clamped to 20
        let result = tool
            .execute(
                &serde_json::json!({"query": "test", "top_k": 100}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_create_codesearch_tool_factory() {
        let tool = create_codesearch_tool("/tmp/test", MockEmbedder::new(16));
        assert_eq!(tool.name(), "codesearch");
    }
}
