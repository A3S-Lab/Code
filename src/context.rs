//! Context Provider Extension Point
//!
//! This module provides the extension point for integrating context databases
//! like OpenViking into the agent loop. Context providers can supply memory,
//! resources, and skills to augment the LLM's context.
//!
//! ## Usage
//!
//! Implement the `ContextProvider` trait and register it with a session:
//!
//! ```ignore
//! use a3s_code::context::{ContextProvider, ContextQuery, ContextResult};
//!
//! struct MyProvider { /* ... */ }
//!
//! #[async_trait::async_trait]
//! impl ContextProvider for MyProvider {
//!     fn name(&self) -> &str { "my-provider" }
//!
//!     async fn query(&self, query: &ContextQuery) -> anyhow::Result<ContextResult> {
//!         // Retrieve relevant context...
//!     }
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Type of context being queried
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ContextType {
    /// Session/user history, extracted insights
    Memory,
    /// Documentation, code, knowledge base
    #[default]
    Resource,
    /// Agent capabilities, behavior instructions
    Skill,
}

/// Retrieval depth for tiered context (L0/L1/L2 pattern)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ContextDepth {
    /// ~100 tokens - high-level summary
    Abstract,
    /// ~2k tokens - key details (default)
    #[default]
    Overview,
    /// Variable - complete content
    Full,
}

/// Query to a context provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextQuery {
    /// The query string to search for relevant context
    pub query: String,

    /// Types of context to retrieve
    #[serde(default)]
    pub context_types: Vec<ContextType>,

    /// Desired retrieval depth
    #[serde(default)]
    pub depth: ContextDepth,

    /// Maximum number of results to return
    #[serde(default = "default_max_results")]
    pub max_results: usize,

    /// Maximum total tokens across all results
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,

    /// Optional session ID for session-specific context
    #[serde(default)]
    pub session_id: Option<String>,

    /// Additional provider-specific parameters
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

fn default_max_results() -> usize {
    10
}

fn default_max_tokens() -> usize {
    4000
}

impl ContextQuery {
    /// Create a new context query with defaults
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            context_types: vec![ContextType::Resource],
            depth: ContextDepth::default(),
            max_results: default_max_results(),
            max_tokens: default_max_tokens(),
            session_id: None,
            params: HashMap::new(),
        }
    }

    /// Set the context types to retrieve
    pub fn with_types(mut self, types: impl IntoIterator<Item = ContextType>) -> Self {
        self.context_types = types.into_iter().collect();
        self
    }

    /// Set the retrieval depth
    pub fn with_depth(mut self, depth: ContextDepth) -> Self {
        self.depth = depth;
        self
    }

    /// Set the maximum number of results
    pub fn with_max_results(mut self, max: usize) -> Self {
        self.max_results = max;
        self
    }

    /// Set the maximum total tokens
    pub fn with_max_tokens(mut self, max: usize) -> Self {
        self.max_tokens = max;
        self
    }

    /// Set the session ID
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Add a custom parameter
    pub fn with_param(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.params.insert(key.into(), value);
        self
    }
}

/// A single piece of retrieved context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    /// Unique identifier for this context item
    pub id: String,

    /// Type of context
    pub context_type: ContextType,

    /// The actual content
    pub content: String,

    /// Estimated token count (informational)
    #[serde(default)]
    pub token_count: usize,

    /// Relevance score (0.0 to 1.0)
    #[serde(default)]
    pub relevance: f32,

    /// Optional source URI (e.g., "viking://docs/auth")
    #[serde(default)]
    pub source: Option<String>,

    /// Additional metadata
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl ContextItem {
    /// Create a new context item
    pub fn new(
        id: impl Into<String>,
        context_type: ContextType,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            context_type,
            content: content.into(),
            token_count: 0,
            relevance: 0.0,
            source: None,
            metadata: HashMap::new(),
        }
    }

    /// Set the token count
    pub fn with_token_count(mut self, count: usize) -> Self {
        self.token_count = count;
        self
    }

    /// Set the relevance score
    pub fn with_relevance(mut self, score: f32) -> Self {
        self.relevance = score.clamp(0.0, 1.0);
        self
    }

    /// Set the source URI
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Format as XML tag for system prompt injection
    pub fn to_xml(&self) -> String {
        let source_attr = self
            .source
            .as_ref()
            .map(|s| format!(" source=\"{}\"", s))
            .unwrap_or_default();
        let type_str = match self.context_type {
            ContextType::Memory => "Memory",
            ContextType::Resource => "Resource",
            ContextType::Skill => "Skill",
        };
        format!(
            "<context{} type=\"{}\">\n{}\n</context>",
            source_attr, type_str, self.content
        )
    }
}

/// Result from a context provider query
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextResult {
    /// Retrieved context items
    pub items: Vec<ContextItem>,

    /// Total tokens across all items
    pub total_tokens: usize,

    /// Name of the provider that returned these results
    pub provider: String,

    /// Whether results were truncated due to limits
    pub truncated: bool,
}

impl ContextResult {
    /// Create a new empty result
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            items: Vec::new(),
            total_tokens: 0,
            provider: provider.into(),
            truncated: false,
        }
    }

    /// Add an item to the result
    pub fn add_item(&mut self, item: ContextItem) {
        self.total_tokens += item.token_count;
        self.items.push(item);
    }

    /// Check if the result is empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Format all items as XML for system prompt injection
    pub fn to_xml(&self) -> String {
        self.items
            .iter()
            .map(|item| item.to_xml())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Context provider trait - implement this for OpenViking, RAG systems, etc.
#[async_trait::async_trait]
pub trait ContextProvider: Send + Sync {
    /// Provider name (used for identification and logging)
    fn name(&self) -> &str;

    /// Query the provider for relevant context
    async fn query(&self, query: &ContextQuery) -> anyhow::Result<ContextResult>;

    /// Called after each turn for memory extraction (optional)
    ///
    /// Providers can implement this to extract and store memories from
    /// the conversation.
    async fn on_turn_complete(
        &self,
        _session_id: &str,
        _prompt: &str,
        _response: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // ContextType Tests
    // ========================================================================

    #[test]
    fn test_context_type_default() {
        let ct: ContextType = Default::default();
        assert_eq!(ct, ContextType::Resource);
    }

    #[test]
    fn test_context_type_serialization() {
        let ct = ContextType::Memory;
        let json = serde_json::to_string(&ct).unwrap();
        assert_eq!(json, "\"Memory\"");

        let parsed: ContextType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ContextType::Memory);
    }

    #[test]
    fn test_context_type_all_variants() {
        let types = vec![
            ContextType::Memory,
            ContextType::Resource,
            ContextType::Skill,
        ];
        for ct in types {
            let json = serde_json::to_string(&ct).unwrap();
            let parsed: ContextType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, ct);
        }
    }

    // ========================================================================
    // ContextDepth Tests
    // ========================================================================

    #[test]
    fn test_context_depth_default() {
        let cd: ContextDepth = Default::default();
        assert_eq!(cd, ContextDepth::Overview);
    }

    #[test]
    fn test_context_depth_serialization() {
        let cd = ContextDepth::Full;
        let json = serde_json::to_string(&cd).unwrap();
        assert_eq!(json, "\"Full\"");

        let parsed: ContextDepth = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ContextDepth::Full);
    }

    #[test]
    fn test_context_depth_all_variants() {
        let depths = vec![
            ContextDepth::Abstract,
            ContextDepth::Overview,
            ContextDepth::Full,
        ];
        for cd in depths {
            let json = serde_json::to_string(&cd).unwrap();
            let parsed: ContextDepth = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, cd);
        }
    }

    // ========================================================================
    // ContextQuery Tests
    // ========================================================================

    #[test]
    fn test_context_query_new() {
        let query = ContextQuery::new("test query");
        assert_eq!(query.query, "test query");
        assert_eq!(query.context_types, vec![ContextType::Resource]);
        assert_eq!(query.depth, ContextDepth::Overview);
        assert_eq!(query.max_results, 10);
        assert_eq!(query.max_tokens, 4000);
        assert!(query.session_id.is_none());
        assert!(query.params.is_empty());
    }

    #[test]
    fn test_context_query_builder() {
        let query = ContextQuery::new("test")
            .with_types([ContextType::Memory, ContextType::Skill])
            .with_depth(ContextDepth::Full)
            .with_max_results(5)
            .with_max_tokens(2000)
            .with_session_id("sess-123")
            .with_param("custom", serde_json::json!("value"));

        assert_eq!(query.context_types.len(), 2);
        assert!(query.context_types.contains(&ContextType::Memory));
        assert!(query.context_types.contains(&ContextType::Skill));
        assert_eq!(query.depth, ContextDepth::Full);
        assert_eq!(query.max_results, 5);
        assert_eq!(query.max_tokens, 2000);
        assert_eq!(query.session_id, Some("sess-123".to_string()));
        assert_eq!(
            query.params.get("custom"),
            Some(&serde_json::json!("value"))
        );
    }

    #[test]
    fn test_context_query_serialization() {
        let query = ContextQuery::new("search term")
            .with_types([ContextType::Resource])
            .with_session_id("sess-456");

        let json = serde_json::to_string(&query).unwrap();
        let parsed: ContextQuery = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.query, "search term");
        assert_eq!(parsed.session_id, Some("sess-456".to_string()));
    }

    #[test]
    fn test_context_query_deserialization_with_defaults() {
        let json = r#"{"query": "minimal query"}"#;
        let query: ContextQuery = serde_json::from_str(json).unwrap();

        assert_eq!(query.query, "minimal query");
        assert!(query.context_types.is_empty()); // Default from serde is empty vec
        assert_eq!(query.depth, ContextDepth::Overview);
        assert_eq!(query.max_results, 10);
        assert_eq!(query.max_tokens, 4000);
    }

    // ========================================================================
    // ContextItem Tests
    // ========================================================================

    #[test]
    fn test_context_item_new() {
        let item = ContextItem::new("item-1", ContextType::Resource, "Some content");
        assert_eq!(item.id, "item-1");
        assert_eq!(item.context_type, ContextType::Resource);
        assert_eq!(item.content, "Some content");
        assert_eq!(item.token_count, 0);
        assert_eq!(item.relevance, 0.0);
        assert!(item.source.is_none());
        assert!(item.metadata.is_empty());
    }

    #[test]
    fn test_context_item_builder() {
        let item = ContextItem::new("item-2", ContextType::Memory, "Memory content")
            .with_token_count(150)
            .with_relevance(0.85)
            .with_source("viking://memory/session-123")
            .with_metadata("key", serde_json::json!("value"));

        assert_eq!(item.token_count, 150);
        assert!((item.relevance - 0.85).abs() < f32::EPSILON);
        assert_eq!(item.source, Some("viking://memory/session-123".to_string()));
        assert_eq!(item.metadata.get("key"), Some(&serde_json::json!("value")));
    }

    #[test]
    fn test_context_item_relevance_clamping() {
        let item1 = ContextItem::new("id", ContextType::Resource, "").with_relevance(1.5);
        assert!((item1.relevance - 1.0).abs() < f32::EPSILON);

        let item2 = ContextItem::new("id", ContextType::Resource, "").with_relevance(-0.5);
        assert!(item2.relevance.abs() < f32::EPSILON);
    }

    #[test]
    fn test_context_item_to_xml_without_source() {
        let item = ContextItem::new("id", ContextType::Resource, "Content here");
        let xml = item.to_xml();
        assert_eq!(xml, "<context type=\"Resource\">\nContent here\n</context>");
    }

    #[test]
    fn test_context_item_to_xml_with_source() {
        let item = ContextItem::new("id", ContextType::Memory, "Memory content")
            .with_source("viking://docs/auth");
        let xml = item.to_xml();
        assert_eq!(
            xml,
            "<context source=\"viking://docs/auth\" type=\"Memory\">\nMemory content\n</context>"
        );
    }

    #[test]
    fn test_context_item_to_xml_all_types() {
        let memory = ContextItem::new("m", ContextType::Memory, "m").to_xml();
        assert!(memory.contains("type=\"Memory\""));

        let resource = ContextItem::new("r", ContextType::Resource, "r").to_xml();
        assert!(resource.contains("type=\"Resource\""));

        let skill = ContextItem::new("s", ContextType::Skill, "s").to_xml();
        assert!(skill.contains("type=\"Skill\""));
    }

    #[test]
    fn test_context_item_serialization() {
        let item = ContextItem::new("item-3", ContextType::Skill, "Skill instructions")
            .with_token_count(200)
            .with_relevance(0.9)
            .with_source("viking://skills/code-review");

        let json = serde_json::to_string(&item).unwrap();
        let parsed: ContextItem = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, "item-3");
        assert_eq!(parsed.context_type, ContextType::Skill);
        assert_eq!(parsed.content, "Skill instructions");
        assert_eq!(parsed.token_count, 200);
    }

    // ========================================================================
    // ContextResult Tests
    // ========================================================================

    #[test]
    fn test_context_result_new() {
        let result = ContextResult::new("test-provider");
        assert!(result.items.is_empty());
        assert_eq!(result.total_tokens, 0);
        assert_eq!(result.provider, "test-provider");
        assert!(!result.truncated);
    }

    #[test]
    fn test_context_result_add_item() {
        let mut result = ContextResult::new("provider");
        let item = ContextItem::new("id", ContextType::Resource, "content").with_token_count(100);
        result.add_item(item);

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.total_tokens, 100);
    }

    #[test]
    fn test_context_result_add_multiple_items() {
        let mut result = ContextResult::new("provider");
        result.add_item(ContextItem::new("1", ContextType::Resource, "a").with_token_count(50));
        result.add_item(ContextItem::new("2", ContextType::Memory, "b").with_token_count(75));
        result.add_item(ContextItem::new("3", ContextType::Skill, "c").with_token_count(25));

        assert_eq!(result.items.len(), 3);
        assert_eq!(result.total_tokens, 150);
    }

    #[test]
    fn test_context_result_is_empty() {
        let empty = ContextResult::new("provider");
        assert!(empty.is_empty());

        let mut non_empty = ContextResult::new("provider");
        non_empty.add_item(ContextItem::new("id", ContextType::Resource, "content"));
        assert!(!non_empty.is_empty());
    }

    #[test]
    fn test_context_result_to_xml() {
        let mut result = ContextResult::new("provider");
        result.add_item(
            ContextItem::new("1", ContextType::Resource, "First content").with_source("source://1"),
        );
        result.add_item(ContextItem::new("2", ContextType::Memory, "Second content"));

        let xml = result.to_xml();
        assert!(xml.contains("<context source=\"source://1\" type=\"Resource\">"));
        assert!(xml.contains("First content"));
        assert!(xml.contains("<context type=\"Memory\">"));
        assert!(xml.contains("Second content"));
    }

    #[test]
    fn test_context_result_to_xml_empty() {
        let result = ContextResult::new("provider");
        let xml = result.to_xml();
        assert!(xml.is_empty());
    }

    #[test]
    fn test_context_result_serialization() {
        let mut result = ContextResult::new("test-provider");
        result.truncated = true;
        result.add_item(ContextItem::new("id", ContextType::Resource, "content"));

        let json = serde_json::to_string(&result).unwrap();
        let parsed: ContextResult = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.provider, "test-provider");
        assert!(parsed.truncated);
        assert_eq!(parsed.items.len(), 1);
    }

    #[test]
    fn test_context_result_default() {
        let result: ContextResult = Default::default();
        assert!(result.items.is_empty());
        assert_eq!(result.total_tokens, 0);
        assert!(result.provider.is_empty());
        assert!(!result.truncated);
    }

    // ========================================================================
    // ContextProvider Trait Tests (with Mock)
    // ========================================================================

    struct MockContextProvider {
        name: String,
        items: Vec<ContextItem>,
    }

    impl MockContextProvider {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                items: Vec::new(),
            }
        }

        fn with_items(mut self, items: Vec<ContextItem>) -> Self {
            self.items = items;
            self
        }
    }

    #[async_trait::async_trait]
    impl ContextProvider for MockContextProvider {
        fn name(&self) -> &str {
            &self.name
        }

        async fn query(&self, _query: &ContextQuery) -> anyhow::Result<ContextResult> {
            let mut result = ContextResult::new(&self.name);
            for item in &self.items {
                result.add_item(item.clone());
            }
            Ok(result)
        }
    }

    #[tokio::test]
    async fn test_mock_context_provider() {
        let provider = MockContextProvider::new("mock").with_items(vec![ContextItem::new(
            "1",
            ContextType::Resource,
            "content",
        )]);

        assert_eq!(provider.name(), "mock");

        let query = ContextQuery::new("test");
        let result = provider.query(&query).await.unwrap();

        assert_eq!(result.provider, "mock");
        assert_eq!(result.items.len(), 1);
    }

    #[tokio::test]
    async fn test_context_provider_on_turn_complete_default() {
        let provider = MockContextProvider::new("mock");

        // Default implementation should succeed
        let result = provider
            .on_turn_complete("session-1", "prompt", "response")
            .await;
        assert!(result.is_ok());
    }

    struct MockMemoryProvider {
        memories: std::sync::Arc<tokio::sync::RwLock<Vec<(String, String, String)>>>,
    }

    impl MockMemoryProvider {
        fn new() -> Self {
            Self {
                memories: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            }
        }
    }

    #[async_trait::async_trait]
    impl ContextProvider for MockMemoryProvider {
        fn name(&self) -> &str {
            "memory-provider"
        }

        async fn query(&self, _query: &ContextQuery) -> anyhow::Result<ContextResult> {
            Ok(ContextResult::new("memory-provider"))
        }

        async fn on_turn_complete(
            &self,
            session_id: &str,
            prompt: &str,
            response: &str,
        ) -> anyhow::Result<()> {
            let mut memories = self.memories.write().await;
            memories.push((
                session_id.to_string(),
                prompt.to_string(),
                response.to_string(),
            ));
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_context_provider_on_turn_complete_custom() {
        let provider = MockMemoryProvider::new();

        provider
            .on_turn_complete("sess-1", "What is Rust?", "Rust is a systems language.")
            .await
            .unwrap();

        let memories = provider.memories.read().await;
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].0, "sess-1");
        assert_eq!(memories[0].1, "What is Rust?");
        assert_eq!(memories[0].2, "Rust is a systems language.");
    }

    // ========================================================================
    // Integration-style Tests
    // ========================================================================

    #[tokio::test]
    async fn test_multiple_providers_query() {
        let provider1 = MockContextProvider::new("provider-1").with_items(vec![ContextItem::new(
            "p1-1",
            ContextType::Resource,
            "Resource from P1",
        )]);

        let provider2 = MockContextProvider::new("provider-2").with_items(vec![
            ContextItem::new("p2-1", ContextType::Memory, "Memory from P2"),
            ContextItem::new("p2-2", ContextType::Skill, "Skill from P2"),
        ]);

        let providers: Vec<&dyn ContextProvider> = vec![&provider1, &provider2];
        let query = ContextQuery::new("test");

        let mut all_items = Vec::new();
        for provider in providers {
            let result = provider.query(&query).await.unwrap();
            all_items.extend(result.items);
        }

        assert_eq!(all_items.len(), 3);
        assert!(all_items.iter().any(|i| i.id == "p1-1"));
        assert!(all_items.iter().any(|i| i.id == "p2-1"));
        assert!(all_items.iter().any(|i| i.id == "p2-2"));
    }

    #[test]
    fn test_context_result_xml_formatting_complex() {
        let mut result = ContextResult::new("openviking");
        result.add_item(
            ContextItem::new(
                "doc-1",
                ContextType::Resource,
                "Authentication uses JWT tokens stored in httpOnly cookies.",
            )
            .with_source("viking://docs/auth")
            .with_token_count(50),
        );
        result.add_item(
            ContextItem::new(
                "mem-1",
                ContextType::Memory,
                "User prefers TypeScript over JavaScript.",
            )
            .with_token_count(30),
        );

        let xml = result.to_xml();

        // Verify structure
        assert!(xml.contains("<context source=\"viking://docs/auth\" type=\"Resource\">"));
        assert!(xml.contains("Authentication uses JWT tokens"));
        assert!(xml.contains("<context type=\"Memory\">"));
        assert!(xml.contains("User prefers TypeScript"));

        // Verify items are separated
        assert!(xml.contains("</context>\n\n<context"));
    }
}
