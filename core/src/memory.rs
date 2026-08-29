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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{oneshot, Notify, RwLock};

const MEMORY_STATUS_METADATA: &str = "a3s.memory.status";
const MEMORY_STATUS_SUPERSEDED: &str = "superseded";

/// One durable-memory write as observed by a host integration.
///
/// `incoming` preserves the identity and per-turn metadata of this observation,
/// while `stored` is the canonical item returned by the memory backend. They
/// differ when a backend consolidates a duplicate into an existing item.
#[derive(Debug, Clone)]
pub struct MemoryObservation {
    pub incoming: MemoryItem,
    pub stored: MemoryItem,
    pub merged: bool,
}

/// Host extension point invoked after a durable memory has been persisted.
///
/// Observer failures are logged but never roll back the memory write. This is
/// intended for derived, auditable projections such as preference and workflow
/// learning; the memory store remains the source of truth.
#[async_trait::async_trait]
pub trait MemoryObserver: Send + Sync {
    async fn on_memory_stored(&self, observation: MemoryObservation) -> anyhow::Result<()>;
}

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
    /// Use an LLM after every completed, non-empty turn to judge whether the
    /// turn contains durable memories and, when it does, distill them from the
    /// transcript.
    ///
    /// Enabled by default when memory is configured. Semantic value decisions
    /// belong to the LLM; the runtime does not use content-keyword gates.
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
    extraction_queue: Arc<MemoryExtractionQueue>,
    observers: Arc<Vec<Arc<dyn MemoryObserver>>>,
    durable_memory: Option<crate::durable_memory::DurableMemorySession>,
}

#[derive(Default)]
struct MemoryExtractionQueue {
    state: std::sync::Mutex<MemoryExtractionQueueState>,
    pending: AtomicUsize,
    idle: Notify,
}

#[derive(Default)]
struct MemoryExtractionQueueState {
    tail: Option<oneshot::Receiver<()>>,
}

/// A FIFO ticket for one completed-turn extraction.
///
/// Registration happens before a background task is spawned, so session close
/// can observe every accepted extraction even if the task has not been polled
/// yet. Chaining each ticket to its predecessor preserves completed-turn order
/// without blocking streaming callers.
pub(crate) struct MemoryExtractionTicket {
    predecessor: Option<oneshot::Receiver<()>>,
    completion: Option<oneshot::Sender<()>>,
    queue: Arc<MemoryExtractionQueue>,
}

impl MemoryExtractionTicket {
    pub(crate) async fn wait_for_turn(&mut self) {
        if let Some(predecessor) = self.predecessor.take() {
            let _ = predecessor.await;
        }
    }
}

impl Drop for MemoryExtractionTicket {
    fn drop(&mut self) {
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(());
        }
        if self.queue.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.queue.idle.notify_waiters();
        }
    }
}

impl std::fmt::Debug for AgentMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentMemory")
            .field("max_short_term", &self.max_short_term)
            .field("max_working", &self.max_working)
            .field("observers", &self.observers.len())
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
        Self::with_config_and_observers(store, config, Vec::new())
    }

    /// Create a memory system with host observers for successful durable
    /// writes. Observers receive both the incoming observation and the
    /// canonical stored item so duplicate consolidation remains auditable.
    pub fn with_config_and_observers(
        store: Arc<dyn MemoryStore>,
        config: MemoryConfig,
        observers: Vec<Arc<dyn MemoryObserver>>,
    ) -> Self {
        Self::with_config_observers_and_durable(store, config, observers, None)
    }

    /// Create a memory system with host observers and an optional exact V2
    /// durable-memory binding.
    pub fn with_config_observers_and_durable(
        store: Arc<dyn MemoryStore>,
        config: MemoryConfig,
        observers: Vec<Arc<dyn MemoryObserver>>,
        durable_memory: Option<crate::durable_memory::DurableMemorySession>,
    ) -> Self {
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
            extraction_queue: Arc::new(MemoryExtractionQueue::default()),
            observers: Arc::new(observers),
            durable_memory,
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
        let incoming = item.clone();
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
        drop(short_term);

        if !self.observers.is_empty() {
            let observation = MemoryObservation {
                merged: item.id != incoming.id,
                incoming,
                stored: item.clone(),
            };
            for observer in self.observers.iter() {
                if let Err(error) = observer.on_memory_stored(observation.clone()).await {
                    tracing::warn!(%error, "memory observer failed after persistence");
                }
            }
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

    /// Preserve a superseded V1 item for audit while removing it from recall.
    pub(crate) async fn mark_superseded(
        &self,
        id: &str,
        replacement_id: &str,
    ) -> anyhow::Result<bool> {
        if id == replacement_id {
            anyhow::bail!("a memory cannot supersede itself");
        }
        let Some(mut item) = self.store.retrieve(id).await? else {
            return Ok(false);
        };
        item.metadata.insert(
            MEMORY_STATUS_METADATA.to_string(),
            MEMORY_STATUS_SUPERSEDED.to_string(),
        );
        item.metadata
            .insert("superseded_by".to_string(), replacement_id.to_string());
        item.metadata
            .insert("protected".to_string(), "true".to_string());
        if !item.tags.iter().any(|tag| tag == "superseded") {
            item.tags.push("superseded".to_string());
        }
        self.store.store(item).await?;
        self.short_term.write().await.retain(|item| item.id != id);
        self.working.write().await.retain(|item| item.id != id);
        Ok(true)
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
        let items = self.store.search(prompt, recall_scan_limit(limit)).await?;
        Ok(recallable_items(items, limit))
    }

    /// Recall by tags
    pub async fn recall_by_tags(
        &self,
        tags: &[String],
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryItem>> {
        let items = self
            .store
            .search_by_tags(tags, recall_scan_limit(limit))
            .await?;
        Ok(recallable_items(items, limit))
    }

    /// Get recent memories
    pub async fn get_recent(&self, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        let items = self.store.get_recent(recall_scan_limit(limit)).await?;
        Ok(recallable_items(items, limit))
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
        self.working
            .read()
            .await
            .iter()
            .filter(|item| is_recallable(item))
            .cloned()
            .collect()
    }

    /// Clear working memory
    pub async fn clear_working(&self) {
        self.working.write().await.clear();
    }

    /// Get short-term memory
    pub async fn get_short_term(&self) -> Vec<MemoryItem> {
        self.short_term
            .read()
            .await
            .iter()
            .filter(|item| is_recallable(item))
            .cloned()
            .collect()
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

    pub(crate) fn durable_memory(&self) -> Option<&crate::durable_memory::DurableMemorySession> {
        self.durable_memory.as_ref()
    }

    pub(crate) fn llm_extraction_max_items(&self) -> usize {
        self.llm_extraction_max_items
    }

    pub(crate) fn llm_extraction_max_input_chars(&self) -> usize {
        self.llm_extraction_max_input_chars
    }

    pub(crate) fn enqueue_llm_extraction(&self) -> MemoryExtractionTicket {
        let (completion, receiver) = oneshot::channel();
        let predecessor = {
            let mut state = self
                .extraction_queue
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.tail.replace(receiver)
        };
        self.extraction_queue.pending.fetch_add(1, Ordering::AcqRel);
        MemoryExtractionTicket {
            predecessor,
            completion: Some(completion),
            queue: Arc::clone(&self.extraction_queue),
        }
    }

    /// Wait until every extraction registered before this call has settled.
    /// Returns `false` when the bounded close-time wait expires.
    pub(crate) async fn drain_llm_extractions(&self, timeout: std::time::Duration) -> bool {
        let wait_until_idle = async {
            loop {
                let notified = self.extraction_queue.idle.notified();
                if self.extraction_queue.pending.load(Ordering::Acquire) == 0 {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, wait_until_idle).await.is_ok()
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
    let items = items.into_iter().filter(is_recallable).collect::<Vec<_>>();
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

fn recall_scan_limit(limit: usize) -> usize {
    limit.saturating_mul(4).max(limit)
}

fn recallable_items(items: Vec<MemoryItem>, limit: usize) -> Vec<MemoryItem> {
    items
        .into_iter()
        .filter(is_recallable)
        .take(limit)
        .collect()
}

fn is_recallable(item: &MemoryItem) -> bool {
    !matches!(
        item.metadata
            .get(MEMORY_STATUS_METADATA)
            .map(|status| status.trim().to_ascii_lowercase()),
        Some(status) if matches!(status.as_str(), "superseded" | "tombstoned")
    )
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
        // Memory extraction is owned by the agent loop's LLM value judge.
        // This provider only contributes recalled memories as prompt context.
        Ok(())
    }
}
#[cfg(test)]
#[path = "memory/tests.rs"]
mod tests;
