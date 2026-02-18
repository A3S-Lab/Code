//! Memory and learning system for the agent
//!
//! This module provides memory storage, recall, and learning capabilities
//! to enable the agent to learn from past experiences and improve over time.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for relevance scoring
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelevanceConfig {
    /// Exponential decay half-life in days (default: 30.0)
    #[serde(default = "RelevanceConfig::default_decay_days")]
    pub decay_days: f32,
    /// Weight for importance factor (default: 0.7)
    #[serde(default = "RelevanceConfig::default_importance_weight")]
    pub importance_weight: f32,
    /// Weight for recency factor (default: 0.3)
    #[serde(default = "RelevanceConfig::default_recency_weight")]
    pub recency_weight: f32,
}

impl RelevanceConfig {
    fn default_decay_days() -> f32 {
        30.0
    }
    fn default_importance_weight() -> f32 {
        0.7
    }
    fn default_recency_weight() -> f32 {
        0.3
    }
}

impl Default for RelevanceConfig {
    fn default() -> Self {
        Self {
            decay_days: 30.0,
            importance_weight: 0.7,
            recency_weight: 0.3,
        }
    }
}

/// Configuration for the agent memory system
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConfig {
    /// Relevance scoring parameters
    #[serde(default)]
    pub relevance: RelevanceConfig,
    /// Maximum short-term memory items (default: 100)
    #[serde(default = "MemoryConfig::default_max_short_term")]
    pub max_short_term: usize,
    /// Maximum working memory items (default: 10)
    #[serde(default = "MemoryConfig::default_max_working")]
    pub max_working: usize,
}

impl MemoryConfig {
    fn default_max_short_term() -> usize {
        100
    }
    fn default_max_working() -> usize {
        10
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            relevance: RelevanceConfig::default(),
            max_short_term: 100,
            max_working: 10,
        }
    }
}

// ============================================================================
// Memory Item
// ============================================================================

/// A single memory item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    /// Unique identifier
    pub id: String,
    /// Memory content
    pub content: String,
    /// When this memory was created
    pub timestamp: DateTime<Utc>,
    /// Importance score (0.0 - 1.0)
    pub importance: f32,
    /// Tags for categorization
    pub tags: Vec<String>,
    /// Memory type
    pub memory_type: MemoryType,
    /// Associated metadata
    pub metadata: HashMap<String, String>,
    /// Number of times this memory was accessed
    pub access_count: u32,
    /// Last access time
    pub last_accessed: Option<DateTime<Utc>>,
    /// Cached lowercase content for fast substring search
    #[serde(skip)]
    pub content_lower: String,
}

impl MemoryItem {
    /// Create a new memory item
    pub fn new(content: impl Into<String>) -> Self {
        let content = content.into();
        let content_lower = content.to_lowercase();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content,
            timestamp: Utc::now(),
            importance: 0.5,
            tags: Vec::new(),
            memory_type: MemoryType::Episodic,
            metadata: HashMap::new(),
            access_count: 0,
            last_accessed: None,
            content_lower,
        }
    }

    /// Set importance
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance.clamp(0.0, 1.0);
        self
    }

    /// Add tags
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Add a single tag
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Set memory type
    pub fn with_type(mut self, memory_type: MemoryType) -> Self {
        self.memory_type = memory_type;
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Record access
    pub fn record_access(&mut self) {
        self.access_count += 1;
        self.last_accessed = Some(Utc::now());
    }

    /// Calculate relevance score at a given timestamp
    ///
    /// Use this variant in sort comparators to avoid repeated `Utc::now()` syscalls.
    pub fn relevance_score_at(&self, now: DateTime<Utc>) -> f32 {
        let age_seconds = (now - self.timestamp).num_seconds() as f32;
        let age_days = age_seconds / 86400.0;

        // Decay factor: memories lose relevance over time
        let decay = (-age_days / 30.0).exp(); // 30-day half-life

        // Combine importance and recency
        self.importance * 0.7 + decay * 0.3
    }

    /// Calculate relevance score based on recency and importance
    pub fn relevance_score(&self) -> f32 {
        self.relevance_score_at(Utc::now())
    }
}

/// Type of memory
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// Episodic memory (specific events)
    Episodic,
    /// Semantic memory (facts and knowledge)
    Semantic,
    /// Procedural memory (how to do things)
    Procedural,
    /// Working memory (temporary, active)
    Working,
}

// ============================================================================
// Memory Store Trait
// ============================================================================

/// Trait for memory storage backends
#[async_trait::async_trait]
pub trait MemoryStore: Send + Sync {
    /// Store a memory item
    async fn store(&self, item: MemoryItem) -> anyhow::Result<()>;

    /// Retrieve a memory by ID
    async fn retrieve(&self, id: &str) -> anyhow::Result<Option<MemoryItem>>;

    /// Search memories by query
    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryItem>>;

    /// Search memories by tags
    async fn search_by_tags(
        &self,
        tags: &[String],
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryItem>>;

    /// Get recent memories
    async fn get_recent(&self, limit: usize) -> anyhow::Result<Vec<MemoryItem>>;

    /// Get important memories
    async fn get_important(&self, threshold: f32, limit: usize) -> anyhow::Result<Vec<MemoryItem>>;

    /// Delete a memory
    async fn delete(&self, id: &str) -> anyhow::Result<()>;

    /// Clear all memories
    async fn clear(&self) -> anyhow::Result<()>;

    /// Get total memory count
    async fn count(&self) -> anyhow::Result<usize>;
}

// ============================================================================
// Shared Search/Sort Helpers (DRY)
// ============================================================================

/// Sort memory items by relevance score (highest first)
fn sort_by_relevance(items: &mut [MemoryItem]) {
    let now = Utc::now();
    items.sort_by(|a, b| {
        b.relevance_score_at(now)
            .partial_cmp(&a.relevance_score_at(now))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

// ============================================================================
// In-Memory Store
// ============================================================================

/// Agent memory system
#[derive(Clone)]
pub struct AgentMemory {
    /// Long-term memory store
    store: Arc<dyn MemoryStore>,
    /// Short-term memory (current session)
    short_term: Arc<RwLock<VecDeque<MemoryItem>>>,
    /// Working memory (active context)
    working: Arc<RwLock<Vec<MemoryItem>>>,
    /// Maximum short-term memory size
    max_short_term: usize,
    /// Maximum working memory size
    max_working: usize,
    /// Relevance scoring configuration
    relevance_config: RelevanceConfig,
}

impl std::fmt::Debug for AgentMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentMemory")
            .field("max_short_term", &self.max_short_term)
            .field("max_working", &self.max_working)
            .finish()
    }
}

impl AgentMemory {
    /// Create a new agent memory system with default configuration
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self::with_config(store, MemoryConfig::default())
    }

    /// Create a new agent memory system with custom configuration
    pub fn with_config(store: Arc<dyn MemoryStore>, config: MemoryConfig) -> Self {
        Self {
            store,
            short_term: Arc::new(RwLock::new(VecDeque::new())),
            working: Arc::new(RwLock::new(Vec::new())),
            max_short_term: config.max_short_term,
            max_working: config.max_working,
            relevance_config: config.relevance,
        }
    }

    /// Calculate relevance score using this memory system's configuration
    fn score(&self, item: &MemoryItem, now: DateTime<Utc>) -> f32 {
        let age_seconds = (now - item.timestamp).num_seconds() as f32;
        let age_days = age_seconds / 86400.0;
        let decay = (-age_days / self.relevance_config.decay_days).exp();
        item.importance * self.relevance_config.importance_weight
            + decay * self.relevance_config.recency_weight
    }

    /// Store a memory in long-term storage
    pub async fn remember(&self, item: MemoryItem) -> anyhow::Result<()> {
        // Store in long-term
        self.store.store(item.clone()).await?;

        // Add to short-term
        let mut short_term = self.short_term.write().await;
        short_term.push_back(item);

        // Trim if needed
        if short_term.len() > self.max_short_term {
            short_term.pop_front();
        }

        Ok(())
    }

    /// Remember a successful pattern
    pub async fn remember_success(
        &self,
        prompt: &str,
        tools_used: &[String],
        result: &str,
    ) -> anyhow::Result<()> {
        let content = format!(
            "Success: {}\nTools: {}\nResult: {}",
            prompt,
            tools_used.join(", "),
            result
        );

        let item = MemoryItem::new(content)
            .with_importance(0.8)
            .with_tag("success")
            .with_tag("pattern")
            .with_type(MemoryType::Procedural)
            .with_metadata("prompt", prompt)
            .with_metadata("tools", tools_used.join(","));

        self.remember(item).await
    }

    /// Remember a failure to avoid repeating
    pub async fn remember_failure(
        &self,
        prompt: &str,
        error: &str,
        attempted_tools: &[String],
    ) -> anyhow::Result<()> {
        let content = format!(
            "Failure: {}\nError: {}\nAttempted tools: {}",
            prompt,
            error,
            attempted_tools.join(", ")
        );

        let item = MemoryItem::new(content)
            .with_importance(0.9) // Failures are important to remember
            .with_tag("failure")
            .with_tag("avoid")
            .with_type(MemoryType::Episodic)
            .with_metadata("prompt", prompt)
            .with_metadata("error", error);

        self.remember(item).await
    }

    /// Recall similar past experiences
    pub async fn recall_similar(
        &self,
        prompt: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryItem>> {
        self.store.search(prompt, limit).await
    }

    /// Recall by tags
    pub async fn recall_by_tags(
        &self,
        tags: &[String],
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryItem>> {
        self.store.search_by_tags(tags, limit).await
    }

    /// Get recent memories
    pub async fn get_recent(&self, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        self.store.get_recent(limit).await
    }

    /// Add to working memory
    pub async fn add_to_working(&self, item: MemoryItem) -> anyhow::Result<()> {
        let mut working = self.working.write().await;
        working.push(item);

        // Trim if needed (keep most relevant)
        if working.len() > self.max_working {
            let now = Utc::now();
            working.sort_by(|a, b| {
                self.score(b, now)
                    .partial_cmp(&self.score(a, now))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            working.truncate(self.max_working);
        }

        Ok(())
    }

    /// Get working memory
    pub async fn get_working(&self) -> Vec<MemoryItem> {
        self.working.read().await.clone()
    }

    /// Clear working memory
    pub async fn clear_working(&self) {
        self.working.write().await.clear();
    }

    /// Get short-term memory
    pub async fn get_short_term(&self) -> Vec<MemoryItem> {
        self.short_term.read().await.iter().cloned().collect()
    }

    /// Clear short-term memory
    pub async fn clear_short_term(&self) {
        self.short_term.write().await.clear();
    }

    /// Get memory statistics
    pub async fn stats(&self) -> anyhow::Result<MemoryStats> {
        let long_term_count = self.store.count().await?;
        let short_term_count = self.short_term.read().await.len();
        let working_count = self.working.read().await.len();

        Ok(MemoryStats {
            long_term_count,
            short_term_count,
            working_count,
        })
    }

    /// Get access to the underlying store
    pub fn store(&self) -> &Arc<dyn MemoryStore> {
        &self.store
    }

    /// Get working memory count
    pub async fn working_count(&self) -> usize {
        self.working.read().await.len()
    }

    /// Get short-term memory count
    pub async fn short_term_count(&self) -> usize {
        self.short_term.read().await.len()
    }
}

/// Memory statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    /// Number of long-term memories
    pub long_term_count: usize,
    /// Number of short-term memories
    pub short_term_count: usize,
    /// Number of working memories
    pub working_count: usize,
}

// ============================================================================
// Memory Context Provider
// ============================================================================

/// Context provider that surfaces past memories (successes/failures) as context.
///
/// Wraps `AgentMemory` and implements the `ContextProvider` trait so that
/// session memory is automatically injected into the agent's system prompt.
pub struct MemoryContextProvider {
    memory: AgentMemory,
}

impl MemoryContextProvider {
    /// Create a new memory context provider
    pub fn new(memory: AgentMemory) -> Self {
        Self { memory }
    }
}

#[async_trait::async_trait]
impl crate::context::ContextProvider for MemoryContextProvider {
    fn name(&self) -> &str {
        "memory"
    }

    async fn query(
        &self,
        query: &crate::context::ContextQuery,
    ) -> anyhow::Result<crate::context::ContextResult> {
        let limit = query.max_results.min(5);
        let items = self.memory.recall_similar(&query.query, limit).await?;

        let mut result = crate::context::ContextResult::new("memory");
        for item in items {
            let relevance = item.relevance_score();
            let token_count = item.content.len() / 4; // rough estimate
            let context_item = crate::context::ContextItem::new(
                &item.id,
                crate::context::ContextType::Memory,
                &item.content,
            )
            .with_relevance(relevance)
            .with_token_count(token_count)
            .with_source("memory");
            result.add_item(context_item);
        }

        Ok(result)
    }

    async fn on_turn_complete(
        &self,
        _session_id: &str,
        prompt: &str,
        response: &str,
    ) -> anyhow::Result<()> {
        // Store the successful interaction as a memory
        self.memory.remember_success(prompt, &[], response).await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Simple in-memory store for testing
    struct TestMemoryStore {
        items: std::sync::Mutex<Vec<MemoryItem>>,
    }

    impl TestMemoryStore {
        fn new() -> Self {
            Self {
                items: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl MemoryStore for TestMemoryStore {
        async fn store(&self, item: MemoryItem) -> anyhow::Result<()> {
            self.items.lock().unwrap().push(item);
            Ok(())
        }
        async fn retrieve(&self, id: &str) -> anyhow::Result<Option<MemoryItem>> {
            Ok(self
                .items
                .lock()
                .unwrap()
                .iter()
                .find(|i| i.id == id)
                .cloned())
        }
        async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
            let items = self.items.lock().unwrap();
            let query_lower = query.to_lowercase();
            Ok(items
                .iter()
                .filter(|i| i.content.to_lowercase().contains(&query_lower))
                .take(limit)
                .cloned()
                .collect())
        }
        async fn search_by_tags(
            &self,
            tags: &[String],
            limit: usize,
        ) -> anyhow::Result<Vec<MemoryItem>> {
            let items = self.items.lock().unwrap();
            Ok(items
                .iter()
                .filter(|i| tags.iter().any(|t| i.tags.contains(t)))
                .take(limit)
                .cloned()
                .collect())
        }
        async fn get_recent(&self, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
            let items = self.items.lock().unwrap();
            let mut sorted: Vec<_> = items.iter().cloned().collect();
            sorted.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            sorted.truncate(limit);
            Ok(sorted)
        }
        async fn get_important(
            &self,
            threshold: f32,
            limit: usize,
        ) -> anyhow::Result<Vec<MemoryItem>> {
            let items = self.items.lock().unwrap();
            Ok(items
                .iter()
                .filter(|i| i.importance >= threshold)
                .take(limit)
                .cloned()
                .collect())
        }
        async fn delete(&self, id: &str) -> anyhow::Result<()> {
            self.items.lock().unwrap().retain(|i| i.id != id);
            Ok(())
        }
        async fn clear(&self) -> anyhow::Result<()> {
            self.items.lock().unwrap().clear();
            Ok(())
        }
        async fn count(&self) -> anyhow::Result<usize> {
            Ok(self.items.lock().unwrap().len())
        }
    }

    #[test]
    fn test_memory_item_creation() {
        let item = MemoryItem::new("Test memory")
            .with_importance(0.8)
            .with_tag("test")
            .with_type(MemoryType::Semantic);

        assert_eq!(item.content, "Test memory");
        assert_eq!(item.importance, 0.8);
        assert_eq!(item.tags, vec!["test"]);
        assert_eq!(item.memory_type, MemoryType::Semantic);
    }

    #[test]
    fn test_memory_item_relevance() {
        let item = MemoryItem::new("Test").with_importance(0.9);
        let score = item.relevance_score();

        // Should be high for recent, important memory
        assert!(score > 0.6);
    }

    #[test]
    fn test_relevance_config_defaults() {
        let config = RelevanceConfig::default();
        assert_eq!(config.decay_days, 30.0);
        assert_eq!(config.importance_weight, 0.7);
        assert_eq!(config.recency_weight, 0.3);
    }

    #[test]
    fn test_memory_config_defaults() {
        let config = MemoryConfig::default();
        assert_eq!(config.max_short_term, 100);
        assert_eq!(config.max_working, 10);
        assert_eq!(config.relevance.decay_days, 30.0);
    }

    #[test]
    fn test_memory_config_serde_roundtrip() {
        let config = MemoryConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: MemoryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.max_short_term, config.max_short_term);
        assert_eq!(parsed.max_working, config.max_working);
        assert_eq!(parsed.relevance.decay_days, config.relevance.decay_days);
    }

    #[test]
    fn test_agent_memory_with_config() {
        let config = MemoryConfig {
            relevance: RelevanceConfig {
                decay_days: 7.0,
                importance_weight: 0.5,
                recency_weight: 0.5,
            },
            max_short_term: 50,
            max_working: 5,
        };
        let memory = AgentMemory::with_config(Arc::new(TestMemoryStore::new()), config);
        assert_eq!(memory.max_short_term, 50);
        assert_eq!(memory.max_working, 5);
        assert_eq!(memory.relevance_config.decay_days, 7.0);
    }

    #[test]
    fn test_agent_memory_score_uses_config() {
        let config = MemoryConfig {
            relevance: RelevanceConfig {
                decay_days: 7.0,
                importance_weight: 0.9,
                recency_weight: 0.1,
            },
            ..Default::default()
        };
        let memory = AgentMemory::with_config(Arc::new(TestMemoryStore::new()), config);

        let item = MemoryItem::new("Test").with_importance(1.0);
        let now = Utc::now();
        let score = memory.score(&item, now);

        // With importance_weight=0.9, a brand new item with importance=1.0
        // should score close to 0.9 + 0.1 = 1.0 (decay ~1.0 for recent items)
        assert!(score > 0.95, "Score was {}", score);
    }

    #[tokio::test]
    async fn test_in_memory_store() {
        let store = TestMemoryStore::new();

        let item = MemoryItem::new("Test memory").with_tag("test");
        store.store(item.clone()).await.unwrap();

        let retrieved = store.retrieve(&item.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().content, "Test memory");
    }

    #[tokio::test]
    async fn test_memory_search() {
        let store = TestMemoryStore::new();

        store
            .store(MemoryItem::new("How to create a file").with_tag("file"))
            .await
            .unwrap();
        store
            .store(MemoryItem::new("How to delete a file").with_tag("file"))
            .await
            .unwrap();
        store
            .store(MemoryItem::new("How to create a directory").with_tag("dir"))
            .await
            .unwrap();

        let results = store.search("create", 10).await.unwrap();
        assert_eq!(results.len(), 2);

        let results = store
            .search_by_tags(&["file".to_string()], 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_agent_memory() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));

        // Remember success
        memory
            .remember_success("Create a file", &["write".to_string()], "File created")
            .await
            .unwrap();

        // Remember failure
        memory
            .remember_failure("Delete file", "Permission denied", &["bash".to_string()])
            .await
            .unwrap();

        // Recall
        let results = memory.recall_similar("create", 10).await.unwrap();
        assert!(!results.is_empty());

        let stats = memory.stats().await.unwrap();
        assert_eq!(stats.long_term_count, 2);
    }

    #[tokio::test]
    async fn test_working_memory() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));

        let item = MemoryItem::new("Active task").with_type(MemoryType::Working);
        memory.add_to_working(item).await.unwrap();

        let working = memory.get_working().await;
        assert_eq!(working.len(), 1);

        memory.clear_working().await;
        let working = memory.get_working().await;
        assert_eq!(working.len(), 0);
    }
}

#[cfg(test)]
mod extra_memory_tests {
    use super::*;

    /// Simple in-memory store for testing
    struct TestMemoryStore {
        items: std::sync::Mutex<Vec<MemoryItem>>,
    }

    impl TestMemoryStore {
        fn new() -> Self {
            Self {
                items: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl MemoryStore for TestMemoryStore {
        async fn store(&self, item: MemoryItem) -> anyhow::Result<()> {
            self.items.lock().unwrap().push(item);
            Ok(())
        }
        async fn retrieve(&self, id: &str) -> anyhow::Result<Option<MemoryItem>> {
            Ok(self
                .items
                .lock()
                .unwrap()
                .iter()
                .find(|i| i.id == id)
                .cloned())
        }
        async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
            let items = self.items.lock().unwrap();
            let query_lower = query.to_lowercase();
            Ok(items
                .iter()
                .filter(|i| i.content.to_lowercase().contains(&query_lower))
                .take(limit)
                .cloned()
                .collect())
        }
        async fn search_by_tags(
            &self,
            tags: &[String],
            limit: usize,
        ) -> anyhow::Result<Vec<MemoryItem>> {
            let items = self.items.lock().unwrap();
            Ok(items
                .iter()
                .filter(|i| tags.iter().any(|t| i.tags.contains(t)))
                .take(limit)
                .cloned()
                .collect())
        }
        async fn get_recent(&self, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
            let items = self.items.lock().unwrap();
            let mut sorted: Vec<_> = items.iter().cloned().collect();
            sorted.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            sorted.truncate(limit);
            Ok(sorted)
        }
        async fn get_important(
            &self,
            threshold: f32,
            limit: usize,
        ) -> anyhow::Result<Vec<MemoryItem>> {
            let items = self.items.lock().unwrap();
            Ok(items
                .iter()
                .filter(|i| i.importance >= threshold)
                .take(limit)
                .cloned()
                .collect())
        }
        async fn delete(&self, id: &str) -> anyhow::Result<()> {
            self.items.lock().unwrap().retain(|i| i.id != id);
            Ok(())
        }
        async fn clear(&self) -> anyhow::Result<()> {
            self.items.lock().unwrap().clear();
            Ok(())
        }
        async fn count(&self) -> anyhow::Result<usize> {
            Ok(self.items.lock().unwrap().len())
        }
    }

    // ========================================================================
    // MemoryItem builder methods
    // ========================================================================

    #[test]
    fn test_memory_item_with_metadata() {
        let item = MemoryItem::new("test")
            .with_metadata("key1", "value1")
            .with_metadata("key2", "value2");
        assert_eq!(item.metadata.get("key1").unwrap(), "value1");
        assert_eq!(item.metadata.get("key2").unwrap(), "value2");
    }

    #[test]
    fn test_memory_item_with_tags_vec() {
        let item = MemoryItem::new("test").with_tags(vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
        ]);
        assert_eq!(item.tags.len(), 3);
    }

    #[test]
    fn test_memory_item_importance_clamped() {
        let item_high = MemoryItem::new("test").with_importance(1.5);
        assert_eq!(item_high.importance, 1.0);

        let item_low = MemoryItem::new("test").with_importance(-0.5);
        assert_eq!(item_low.importance, 0.0);
    }

    #[test]
    fn test_memory_item_record_access() {
        let mut item = MemoryItem::new("test");
        assert_eq!(item.access_count, 0);
        assert!(item.last_accessed.is_none());

        item.record_access();
        assert_eq!(item.access_count, 1);
        assert!(item.last_accessed.is_some());

        item.record_access();
        assert_eq!(item.access_count, 2);
    }

    #[test]
    fn test_memory_item_all_types() {
        let episodic = MemoryItem::new("e").with_type(MemoryType::Episodic);
        assert_eq!(episodic.memory_type, MemoryType::Episodic);

        let semantic = MemoryItem::new("s").with_type(MemoryType::Semantic);
        assert_eq!(semantic.memory_type, MemoryType::Semantic);

        let procedural = MemoryItem::new("p").with_type(MemoryType::Procedural);
        assert_eq!(procedural.memory_type, MemoryType::Procedural);

        let working = MemoryItem::new("w").with_type(MemoryType::Working);
        assert_eq!(working.memory_type, MemoryType::Working);
    }

    #[test]
    fn test_memory_item_default_type_is_episodic() {
        let item = MemoryItem::new("test");
        assert_eq!(item.memory_type, MemoryType::Episodic);
    }

    // ========================================================================
    // TestMemoryStore
    // ========================================================================

    #[tokio::test]
    async fn test_in_memory_store_retrieve_nonexistent() {
        let store = TestMemoryStore::new();
        let result = store.retrieve("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_store_delete() {
        let store = TestMemoryStore::new();
        let item = MemoryItem::new("to delete");
        let id = item.id.clone();
        store.store(item).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);

        store.delete(&id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_in_memory_store_clear() {
        let store = TestMemoryStore::new();
        for i in 0..5 {
            store
                .store(MemoryItem::new(format!("item {}", i)))
                .await
                .unwrap();
        }
        assert_eq!(store.count().await.unwrap(), 5);

        store.clear().await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_in_memory_store_get_recent() {
        let store = TestMemoryStore::new();
        for i in 0..5 {
            store
                .store(MemoryItem::new(format!("item {}", i)))
                .await
                .unwrap();
        }
        let recent = store.get_recent(3).await.unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[tokio::test]
    async fn test_in_memory_store_get_important() {
        let store = TestMemoryStore::new();
        store
            .store(MemoryItem::new("low").with_importance(0.2))
            .await
            .unwrap();
        store
            .store(MemoryItem::new("medium").with_importance(0.5))
            .await
            .unwrap();
        store
            .store(MemoryItem::new("high").with_importance(0.9))
            .await
            .unwrap();

        let important = store.get_important(0.7, 10).await.unwrap();
        assert_eq!(important.len(), 1);
        assert_eq!(important[0].content, "high");
    }

    #[tokio::test]
    async fn test_in_memory_store_search_case_insensitive() {
        let store = TestMemoryStore::new();
        store
            .store(MemoryItem::new("How to CREATE a file"))
            .await
            .unwrap();
        let results = store.search("create", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    // ========================================================================
    // AgentMemory
    // ========================================================================

    #[tokio::test]
    async fn test_agent_memory_short_term() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));
        memory.remember(MemoryItem::new("item 1")).await.unwrap();
        memory.remember(MemoryItem::new("item 2")).await.unwrap();

        let short_term = memory.get_short_term().await;
        assert_eq!(short_term.len(), 2);

        memory.clear_short_term().await;
        let short_term = memory.get_short_term().await;
        assert_eq!(short_term.len(), 0);
    }

    #[tokio::test]
    async fn test_agent_memory_short_term_count() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));
        assert_eq!(memory.short_term_count().await, 0);
        memory.remember(MemoryItem::new("item")).await.unwrap();
        assert_eq!(memory.short_term_count().await, 1);
    }

    #[tokio::test]
    async fn test_agent_memory_working_count() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));
        assert_eq!(memory.working_count().await, 0);
        memory
            .add_to_working(MemoryItem::new("task"))
            .await
            .unwrap();
        assert_eq!(memory.working_count().await, 1);
    }

    #[tokio::test]
    async fn test_agent_memory_recall_by_tags() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));
        memory
            .remember_success("create file", &["write".to_string()], "ok")
            .await
            .unwrap();
        memory
            .remember_failure("delete file", "denied", &["bash".to_string()])
            .await
            .unwrap();

        let successes = memory
            .recall_by_tags(&["success".to_string()], 10)
            .await
            .unwrap();
        assert_eq!(successes.len(), 1);

        let failures = memory
            .recall_by_tags(&["failure".to_string()], 10)
            .await
            .unwrap();
        assert_eq!(failures.len(), 1);
    }

    #[tokio::test]
    async fn test_agent_memory_get_recent() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));
        for i in 0..5 {
            memory
                .remember(MemoryItem::new(format!("item {}", i)))
                .await
                .unwrap();
        }
        let recent = memory.get_recent(3).await.unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[tokio::test]
    async fn test_agent_memory_store_accessor() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));
        memory.remember(MemoryItem::new("test")).await.unwrap();
        let count = memory.store().count().await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_agent_memory_stats_all_fields() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));
        memory.remember(MemoryItem::new("long term")).await.unwrap();
        memory
            .add_to_working(MemoryItem::new("working"))
            .await
            .unwrap();

        let stats = memory.stats().await.unwrap();
        assert_eq!(stats.long_term_count, 1);
        assert_eq!(stats.short_term_count, 1); // remember also adds to short_term
        assert_eq!(stats.working_count, 1);
    }

    #[tokio::test]
    async fn test_agent_memory_working_overflow_trims() {
        let store = Arc::new(TestMemoryStore::new());
        let memory = AgentMemory {
            store,
            short_term: Arc::new(RwLock::new(VecDeque::new())),
            working: Arc::new(RwLock::new(Vec::new())),
            max_short_term: 100,
            max_working: 3, // Small limit
            relevance_config: RelevanceConfig::default(),
        };

        for i in 0..5 {
            memory
                .add_to_working(
                    MemoryItem::new(format!("task {}", i)).with_importance(i as f32 * 0.2),
                )
                .await
                .unwrap();
        }

        let working = memory.get_working().await;
        assert_eq!(working.len(), 3); // Trimmed to max_working
    }
}
