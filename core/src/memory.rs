//! Memory and learning system for the agent.
//!
//! Core types (`MemoryStore`, `MemoryItem`, `MemoryType`, `MemoryConfig`,
//! `RelevanceConfig`, `FileMemoryStore`, `MemoryStats`) live in `a3s-memory`
//! and are re-exported here for backwards compatibility.
//!
//! This module owns `AgentMemory` (three-tier session memory) and
//! `MemoryContextProvider` (context injection bridge).

use a3s_memory::{MemoryConfig, MemoryItem, MemoryStats, MemoryStore, MemoryType, RelevanceConfig};

use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Agent Memory (three-tier: working / short-term / long-term)
// ============================================================================

/// Three-tier agent memory: working, short-term (session), and long-term (persisted).
#[derive(Clone)]
pub struct AgentMemory {
    /// Long-term memory store
    pub(crate) store: Arc<dyn MemoryStore>,
    /// Short-term memory (current session)
    short_term: Arc<RwLock<VecDeque<MemoryItem>>>,
    /// Working memory (active context)
    working: Arc<RwLock<Vec<MemoryItem>>>,
    pub(crate) max_short_term: usize,
    pub(crate) max_working: usize,
    pub(crate) relevance_config: RelevanceConfig,
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

    pub(crate) fn score(&self, item: &MemoryItem, now: DateTime<Utc>) -> f32 {
        let age_days = (now - item.timestamp).num_seconds() as f32 / 86400.0;
        let decay = (-age_days / self.relevance_config.decay_days).exp();
        item.importance * self.relevance_config.importance_weight
            + decay * self.relevance_config.recency_weight
    }

    /// Store a memory in long-term storage and add to short-term
    pub async fn remember(&self, item: MemoryItem) -> anyhow::Result<()> {
        self.store.store(item.clone()).await?;
        let mut short_term = self.short_term.write().await;
        short_term.push_back(item);
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
            .with_importance(0.9)
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

    /// Add to working memory (auto-trims by relevance if over capacity)
    pub async fn add_to_working(&self, item: MemoryItem) -> anyhow::Result<()> {
        let mut working = self.working.write().await;
        working.push(item);
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
        Ok(MemoryStats {
            long_term_count: self.store.count().await?,
            short_term_count: self.short_term.read().await.len(),
            working_count: self.working.read().await.len(),
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

// ============================================================================
// Memory Context Provider
// ============================================================================

/// Context provider that surfaces past memories as agent context.
pub struct MemoryContextProvider {
    memory: AgentMemory,
}

impl MemoryContextProvider {
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
            let token_count = item.content.len() / 4;
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
        self.memory.remember_success(prompt, &[], response).await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
            let q = query.to_lowercase();
            Ok(items
                .iter()
                .filter(|i| i.content.to_lowercase().contains(&q))
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
            let mut sorted: Vec<_> = self.items.lock().unwrap().iter().cloned().collect();
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

    #[tokio::test]
    async fn test_agent_memory_remember_and_recall() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));
        memory
            .remember_success("create file", &["write".to_string()], "ok")
            .await
            .unwrap();
        memory
            .remember_failure("delete file", "denied", &["bash".to_string()])
            .await
            .unwrap();

        let results = memory.recall_similar("create", 10).await.unwrap();
        assert!(!results.is_empty());

        let stats = memory.stats().await.unwrap();
        assert_eq!(stats.long_term_count, 2);
        assert_eq!(stats.short_term_count, 2);
    }

    #[tokio::test]
    async fn test_agent_memory_working() {
        let memory = AgentMemory::new(Arc::new(TestMemoryStore::new()));
        memory
            .add_to_working(MemoryItem::new("task").with_type(MemoryType::Working))
            .await
            .unwrap();
        assert_eq!(memory.working_count().await, 1);
        memory.clear_working().await;
        assert_eq!(memory.working_count().await, 0);
    }

    #[tokio::test]
    async fn test_agent_memory_working_overflow_trims() {
        let memory = AgentMemory {
            store: Arc::new(TestMemoryStore::new()),
            short_term: Arc::new(RwLock::new(VecDeque::new())),
            working: Arc::new(RwLock::new(Vec::new())),
            max_short_term: 100,
            max_working: 3,
            relevance_config: RelevanceConfig::default(),
        };
        for i in 0..5 {
            memory
                .add_to_working(
                    MemoryItem::new(format!("task {i}")).with_importance(i as f32 * 0.2),
                )
                .await
                .unwrap();
        }
        assert_eq!(memory.get_working().await.len(), 3);
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
    async fn test_agent_memory_short_term_trim() {
        let store = Arc::new(TestMemoryStore::new());
        let memory = AgentMemory {
            store,
            short_term: Arc::new(RwLock::new(VecDeque::new())),
            working: Arc::new(RwLock::new(Vec::new())),
            max_short_term: 3,
            max_working: 10,
            relevance_config: RelevanceConfig::default(),
        };
        for i in 0..5 {
            memory
                .remember(MemoryItem::new(format!("item {i}")))
                .await
                .unwrap();
        }
        assert_eq!(memory.short_term_count().await, 3);
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
        let score = memory.score(&item, Utc::now());
        assert!(score > 0.95, "Score was {score}");
    }
}
