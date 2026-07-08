//! Memory and learning system for the agent.
//!
//! Core types (`MemoryStore`, `MemoryItem`, `MemoryType`, `RelevanceConfig`,
//! `FileMemoryStore`, `InMemoryStore`) live in `a3s-memory`.
//!
//! This module owns `MemoryConfig`, `MemoryStats`, `AgentMemory` (three-tier
//! session memory), and `MemoryContextProvider` (context injection bridge).

use a3s_memory::{MemoryItem, MemoryStore, MemoryType, PrunePolicy, RelevanceConfig};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the agent memory system (three-tier: working/short-term/long-term)
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
    /// Automatic pruning policy for long-term storage. `None` disables background pruning.
    #[serde(default)]
    pub prune_policy: Option<PrunePolicy>,
    /// How often the background pruning task runs, in seconds (default: 3600).
    #[serde(default = "MemoryConfig::default_prune_interval_secs")]
    pub prune_interval_secs: u64,
    /// Use an LLM after significant completed turns to distill durable memories
    /// from the turn transcript.
    ///
    /// Enabled by default when memory is configured. The runtime still applies
    /// a significance gate so trivial turns do not trigger an extraction call.
    #[serde(
        default = "MemoryConfig::default_llm_extraction",
        alias = "llm_extraction"
    )]
    pub llm_extraction: bool,
    /// Maximum durable memories the LLM extractor may write per turn.
    #[serde(default = "MemoryConfig::default_llm_extraction_max_items")]
    pub llm_extraction_max_items: usize,
    /// Maximum transcript characters passed into the LLM memory extractor.
    #[serde(default = "MemoryConfig::default_llm_extraction_max_input_chars")]
    pub llm_extraction_max_input_chars: usize,
}

impl MemoryConfig {
    fn default_max_short_term() -> usize {
        100
    }
    fn default_max_working() -> usize {
        10
    }
    fn default_prune_interval_secs() -> u64 {
        3600
    }
    fn default_llm_extraction() -> bool {
        true
    }
    fn default_llm_extraction_max_items() -> usize {
        5
    }
    fn default_llm_extraction_max_input_chars() -> usize {
        8_000
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            relevance: RelevanceConfig::default(),
            max_short_term: 100,
            max_working: 10,
            prune_policy: None,
            prune_interval_secs: 3600,
            llm_extraction: true,
            llm_extraction_max_items: 5,
            llm_extraction_max_input_chars: 8_000,
        }
    }
}

// ============================================================================
// Memory Stats
// ============================================================================

/// Statistics for the three-tier agent memory system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    pub long_term_count: usize,
    pub short_term_count: usize,
    pub working_count: usize,
}

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
    pub(crate) llm_extraction: bool,
    pub(crate) llm_extraction_max_items: usize,
    pub(crate) llm_extraction_max_input_chars: usize,
    llm_extraction_in_flight: Arc<AtomicBool>,
}

pub(crate) struct MemoryExtractionPermit {
    in_flight: Arc<AtomicBool>,
}

impl Drop for MemoryExtractionPermit {
    fn drop(&mut self) {
        self.in_flight.store(false, Ordering::Release);
    }
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

    /// Create a new agent memory system with custom configuration.
    ///
    /// If `config.prune_policy` is `Some`, a background Tokio task is spawned
    /// that periodically calls `store.prune()` at the configured interval.
    pub fn with_config(store: Arc<dyn MemoryStore>, config: MemoryConfig) -> Self {
        if let Some(policy) = config.prune_policy.clone() {
            let store_for_task = Arc::clone(&store);
            let interval_secs = config.prune_interval_secs;
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    handle.spawn(async move {
                        let mut ticker =
                            tokio::time::interval(std::time::Duration::from_secs(interval_secs));
                        ticker.tick().await; // skip the immediate first tick
                        loop {
                            ticker.tick().await;
                            if let Err(e) = store_for_task.prune(&policy).await {
                                tracing::warn!("memory prune failed: {e}");
                            }
                        }
                    });
                }
                Err(_) => {
                    tracing::warn!(
                        "memory prune policy configured but no async runtime is available"
                    );
                }
            }
        }

        Self {
            store,
            short_term: Arc::new(RwLock::new(VecDeque::new())),
            working: Arc::new(RwLock::new(Vec::new())),
            max_short_term: config.max_short_term,
            max_working: config.max_working,
            relevance_config: config.relevance,
            llm_extraction: config.llm_extraction,
            llm_extraction_max_items: config.llm_extraction_max_items,
            llm_extraction_max_input_chars: config.llm_extraction_max_input_chars,
            llm_extraction_in_flight: Arc::new(AtomicBool::new(false)),
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
        self.remember_item(item).await.map(|_| ())
    }

    /// Store a memory and return the normalized item that was sent to storage.
    pub async fn remember_item(&self, item: MemoryItem) -> anyhow::Result<MemoryItem> {
        let item = self.store.store_and_return(item).await?;
        let mut short_term = self.short_term.write().await;
        if let Some(existing) = short_term
            .iter_mut()
            .find(|existing| existing.id == item.id)
        {
            *existing = item.clone();
        } else {
            short_term.push_back(item.clone());
        }
        if short_term.len() > self.max_short_term {
            short_term.pop_front();
        }
        Ok(item)
    }

    /// Remove a memory from long-term storage and session-local memory tiers.
    pub async fn forget(&self, id: &str) -> anyhow::Result<()> {
        self.store.delete(id).await?;
        self.short_term.write().await.retain(|item| item.id != id);
        self.working.write().await.retain(|item| item.id != id);
        Ok(())
    }

    /// Remember a successful pattern
    pub async fn remember_success(
        &self,
        prompt: &str,
        tools_used: &[String],
        result: &str,
    ) -> anyhow::Result<()> {
        self.remember_success_item(prompt, tools_used, result)
            .await
            .map(|_| ())
    }

    /// Remember a successful pattern and return the stored memory item.
    pub async fn remember_success_item(
        &self,
        prompt: &str,
        tools_used: &[String],
        result: &str,
    ) -> anyhow::Result<MemoryItem> {
        let content = format!(
            "Success: {}\nTools: {}\nResult: {}",
            prompt,
            tools_used.join(", "),
            result
        );
        let mut item = MemoryItem::new(content)
            .with_importance(0.8)
            .with_tag("success")
            .with_tag("pattern")
            .with_type(MemoryType::Procedural)
            .with_metadata("prompt", prompt)
            .with_metadata("tools", tools_used.join(","));
        for tool in tools_used {
            item = item.with_tag(tool.clone());
        }
        self.remember_item(item).await
    }

    /// Remember a failure to avoid repeating
    pub async fn remember_failure(
        &self,
        prompt: &str,
        error: &str,
        attempted_tools: &[String],
    ) -> anyhow::Result<()> {
        self.remember_failure_item(prompt, error, attempted_tools)
            .await
            .map(|_| ())
    }

    /// Remember a failed pattern and return the stored memory item.
    pub async fn remember_failure_item(
        &self,
        prompt: &str,
        error: &str,
        attempted_tools: &[String],
    ) -> anyhow::Result<MemoryItem> {
        let content = format!(
            "Failure: {}\nError: {}\nAttempted tools: {}",
            prompt,
            error,
            attempted_tools.join(", ")
        );
        let mut item = MemoryItem::new(content)
            .with_importance(0.9)
            .with_tag("failure")
            .with_tag("avoid")
            .with_type(MemoryType::Episodic)
            .with_metadata("prompt", prompt)
            .with_metadata("error", error);
        for tool in attempted_tools {
            item = item.with_tag(tool.clone());
        }
        self.remember_item(item).await
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

    pub(crate) fn llm_extraction_enabled(&self) -> bool {
        self.llm_extraction
    }

    pub(crate) fn llm_extraction_max_items(&self) -> usize {
        self.llm_extraction_max_items
    }

    pub(crate) fn llm_extraction_max_input_chars(&self) -> usize {
        self.llm_extraction_max_input_chars
    }

    pub(crate) fn try_begin_llm_extraction(&self) -> Option<MemoryExtractionPermit> {
        self.llm_extraction_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| MemoryExtractionPermit {
                in_flight: Arc::clone(&self.llm_extraction_in_flight),
            })
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

pub(crate) fn memory_items_to_context_result(
    provider: impl Into<String>,
    items: Vec<MemoryItem>,
) -> crate::context::ContextResult {
    let mut result = crate::context::ContextResult::new(provider);
    let total = items.len().max(1);
    for (index, item) in items.into_iter().enumerate() {
        let supersedes = relation_ids(&item, "supersedes");
        let conflicts_with = relation_ids(&item, "conflicts_with");
        let content = memory_context_content(&item, &supersedes, &conflicts_with);
        let token_count = (content.len() / 4).max(1);
        let recall_rank_score = 1.0 - (index as f32 / total as f32);
        let relevance = (item.relevance_score() * 0.35 + recall_rank_score * 0.65).clamp(0.0, 1.0);
        let context_item = crate::context::ContextItem::new(
            &item.id,
            crate::context::ContextType::Memory,
            content,
        )
        .with_relevance(relevance)
        .with_token_count(token_count)
        .with_source(format!("memory://{}", item.id))
        .with_metadata("memory_id", serde_json::json!(item.id))
        .with_metadata(
            "memory_type",
            serde_json::json!(memory_type_label(item.memory_type)),
        )
        .with_metadata("tags", serde_json::json!(item.tags))
        .with_metadata("importance", serde_json::json!(item.importance))
        .with_provenance("long_term_memory")
        .with_priority(0.35)
        .with_trust(0.7)
        .with_freshness(0.5);
        let context_item = add_relation_metadata(context_item, "supersedes", supersedes);
        let context_item = add_relation_metadata(context_item, "conflicts_with", conflicts_with);
        result.add_item(context_item);
    }
    result
}

fn relation_ids(item: &MemoryItem, key: &str) -> Vec<String> {
    item.metadata
        .get(key)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn memory_context_content(
    item: &MemoryItem,
    supersedes: &[String],
    conflicts_with: &[String],
) -> String {
    let mut content = item.content.clone();
    if supersedes.is_empty() && conflicts_with.is_empty() {
        return content;
    }

    content.push_str("\n\nMemory relations:");
    if !supersedes.is_empty() {
        content.push_str("\n- supersedes: ");
        content.push_str(&relation_sources(supersedes));
    }
    if !conflicts_with.is_empty() {
        content.push_str("\n- conflicts_with: ");
        content.push_str(&relation_sources(conflicts_with));
    }
    content
}

fn relation_sources(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("memory://{id}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn add_relation_metadata(
    item: crate::context::ContextItem,
    key: &str,
    ids: Vec<String>,
) -> crate::context::ContextItem {
    if ids.is_empty() {
        item
    } else {
        item.with_metadata(key, serde_json::json!(ids))
    }
}

fn memory_type_label(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Episodic => "episodic",
        MemoryType::Semantic => "semantic",
        MemoryType::Procedural => "procedural",
        MemoryType::Working => "working",
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

        Ok(memory_items_to_context_result("memory", items))
    }

    async fn on_turn_complete(
        &self,
        _session_id: &str,
        _prompt: &str,
        _response: &str,
    ) -> anyhow::Result<()> {
        // Memory extraction is owned by the agent loop's gated LLM extractor.
        // This provider only contributes recalled memories as prompt context.
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextProvider;
    use a3s_memory::InMemoryStore;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_agent_memory_remember_and_recall() {
        let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
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
    async fn test_agent_memory_forget_removes_all_tiers() {
        let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
        let item = memory
            .remember_item(MemoryItem::new("superseded memory"))
            .await
            .unwrap();
        memory.add_to_working(item.clone()).await.unwrap();

        memory.forget(&item.id).await.unwrap();

        assert_eq!(memory.stats().await.unwrap().long_term_count, 0);
        assert!(memory.get_short_term().await.is_empty());
        assert!(memory.get_working().await.is_empty());
    }

    #[tokio::test]
    async fn test_agent_memory_uses_canonical_store_item_for_duplicates() {
        let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
        let first = memory
            .remember_item(
                MemoryItem::new("Run focused memory extraction tests after parser changes.")
                    .with_importance(0.3)
                    .with_tag("memory"),
            )
            .await
            .unwrap();

        let duplicate = memory
            .remember_item(
                MemoryItem::new("run focused MEMORY extraction tests after parser changes!")
                    .with_importance(0.9)
                    .with_tag("tests"),
            )
            .await
            .unwrap();

        assert_eq!(duplicate.id, first.id);
        assert_eq!(memory.stats().await.unwrap().long_term_count, 1);
        let short_term = memory.get_short_term().await;
        assert_eq!(short_term.len(), 1);
        assert_eq!(short_term[0].id, first.id);
        assert_eq!(short_term[0].importance, 0.9);
        assert!(short_term[0].tags.contains(&"memory".to_string()));
        assert!(short_term[0].tags.contains(&"tests".to_string()));
    }

    #[tokio::test]
    async fn test_agent_memory_working() {
        let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
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
            store: Arc::new(InMemoryStore::new()),
            short_term: Arc::new(RwLock::new(VecDeque::new())),
            working: Arc::new(RwLock::new(Vec::new())),
            max_short_term: 100,
            max_working: 3,
            relevance_config: RelevanceConfig::default(),
            llm_extraction: false,
            llm_extraction_max_items: 5,
            llm_extraction_max_input_chars: 8_000,
            llm_extraction_in_flight: Arc::new(AtomicBool::new(false)),
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
        let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
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
        let store = Arc::new(InMemoryStore::new());
        let memory = AgentMemory {
            store,
            short_term: Arc::new(RwLock::new(VecDeque::new())),
            working: Arc::new(RwLock::new(Vec::new())),
            max_short_term: 3,
            max_working: 10,
            relevance_config: RelevanceConfig::default(),
            llm_extraction: false,
            llm_extraction_max_items: 5,
            llm_extraction_max_input_chars: 8_000,
            llm_extraction_in_flight: Arc::new(AtomicBool::new(false)),
        };
        for i in 0..5 {
            memory
                .remember(MemoryItem::new(format!("item {i}")))
                .await
                .unwrap();
        }
        assert_eq!(memory.short_term_count().await, 3);
    }

    #[tokio::test]
    async fn test_agent_memory_prune_delegates() {
        use a3s_memory::PrunePolicy;

        let store = Arc::new(InMemoryStore::new());
        let memory = AgentMemory::new(store.clone());

        // Insert one old low-importance item directly into the store.
        let mut old_item = a3s_memory::MemoryItem::new("stale").with_importance(0.2);
        old_item.timestamp = chrono::Utc::now() - chrono::Duration::days(100);
        store.store(old_item).await.unwrap();

        assert_eq!(store.count().await.unwrap(), 1);

        // Calling prune on the underlying store via the public accessor works.
        let policy = PrunePolicy {
            max_age_days: 90,
            min_importance_to_keep: 0.5,
            max_items: 0,
        };
        let deleted = memory.store().prune(&policy).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(store.count().await.unwrap(), 0);
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
        let memory = AgentMemory::with_config(Arc::new(InMemoryStore::new()), config);
        let item = MemoryItem::new("Test").with_importance(1.0);
        let score = memory.score(&item, Utc::now());
        assert!(score > 0.95, "Score was {score}");
    }

    #[test]
    fn test_memory_config_partial_deserialize_keeps_llm_extraction_enabled() {
        let config: MemoryConfig = serde_json::from_str(r#"{"maxShortTerm": 12}"#).unwrap();
        assert!(config.llm_extraction);
        assert_eq!(config.max_short_term, 12);
    }

    #[test]
    fn test_memory_config_allows_explicit_llm_extraction_disable() {
        let config: MemoryConfig =
            serde_json::from_str(r#"{"llmExtraction": false, "maxShortTerm": 12}"#).unwrap();
        assert!(!config.llm_extraction);
        assert_eq!(config.max_short_term, 12);
    }

    #[test]
    fn test_memory_context_result_includes_relation_context() {
        let item = MemoryItem::new("Use the file memory store for local sessions.")
            .with_type(MemoryType::Procedural)
            .with_tag("consolidated")
            .with_tag("conflict")
            .with_metadata("supersedes", "old-preference, old-workflow")
            .with_metadata("conflicts_with", "legacy-default");

        let result = memory_items_to_context_result("memory", vec![item.clone()]);

        assert_eq!(result.items.len(), 1);
        let context_item = &result.items[0];
        assert!(context_item
            .content
            .contains("Use the file memory store for local sessions."));
        assert!(context_item.content.contains("Memory relations:"));
        assert!(context_item
            .content
            .contains("supersedes: memory://old-preference, memory://old-workflow"));
        assert!(context_item
            .content
            .contains("conflicts_with: memory://legacy-default"));
        assert_eq!(
            context_item.metadata.get("memory_id"),
            Some(&serde_json::json!(item.id))
        );
        assert_eq!(
            context_item.metadata.get("memory_type"),
            Some(&serde_json::json!("procedural"))
        );
        assert_eq!(
            context_item.metadata.get("tags"),
            Some(&serde_json::json!(["consolidated", "conflict"]))
        );
        assert_eq!(
            context_item.metadata.get("supersedes"),
            Some(&serde_json::json!(["old-preference", "old-workflow"]))
        );
        assert_eq!(
            context_item.metadata.get("conflicts_with"),
            Some(&serde_json::json!(["legacy-default"]))
        );
        assert_eq!(
            context_item.token_count,
            (context_item.content.len() / 4).max(1)
        );
    }

    #[test]
    fn test_memory_context_relevance_preserves_recall_order() {
        let top_match =
            MemoryItem::new("Run focused memory extraction tests after parser changes.")
                .with_importance(0.2)
                .with_type(MemoryType::Procedural);
        let generic_high_importance = MemoryItem::new("Remember general memory behavior.")
            .with_importance(1.0)
            .with_type(MemoryType::Semantic);

        let result =
            memory_items_to_context_result("memory", vec![top_match, generic_high_importance]);

        assert_eq!(result.items.len(), 2);
        assert!(
            result.items[0].relevance > result.items[1].relevance,
            "search recall order should remain a strong memory context ranking signal"
        );
    }

    #[tokio::test]
    async fn test_memory_context_provider_does_not_mechanically_store_turns() {
        let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
        let provider = MemoryContextProvider::new(memory.clone());

        provider
            .on_turn_complete("session-1", "remember nothing", "ok")
            .await
            .unwrap();

        assert_eq!(memory.stats().await.unwrap().long_term_count, 0);
    }
}
