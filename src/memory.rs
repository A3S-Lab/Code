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
}

impl MemoryItem {
    /// Create a new memory item
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content: content.into(),
            timestamp: Utc::now(),
            importance: 0.5,
            tags: Vec::new(),
            memory_type: MemoryType::Episodic,
            metadata: HashMap::new(),
            access_count: 0,
            last_accessed: None,
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

    /// Calculate relevance score based on recency and importance
    pub fn relevance_score(&self) -> f32 {
        let now = Utc::now();
        let age_seconds = (now - self.timestamp).num_seconds() as f32;
        let age_days = age_seconds / 86400.0;

        // Decay factor: memories lose relevance over time
        let decay = (-age_days / 30.0).exp(); // 30-day half-life

        // Combine importance and recency
        self.importance * 0.7 + decay * 0.3
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

/// Search memories by content substring, sorted by relevance
fn search_memories(memories: &[MemoryItem], query: &str, limit: usize) -> Vec<MemoryItem> {
    let query_lower = query.to_lowercase();
    let mut results: Vec<_> = memories
        .iter()
        .filter(|m| m.content.to_lowercase().contains(&query_lower))
        .cloned()
        .collect();
    sort_by_relevance(&mut results);
    results.truncate(limit);
    results
}

/// Search memories by tags, sorted by relevance
fn search_memories_by_tags(
    memories: &[MemoryItem],
    tags: &[String],
    limit: usize,
) -> Vec<MemoryItem> {
    let mut results: Vec<_> = memories
        .iter()
        .filter(|m| tags.iter().any(|tag| m.tags.contains(tag)))
        .cloned()
        .collect();
    sort_by_relevance(&mut results);
    results.truncate(limit);
    results
}

/// Get recent memories sorted by timestamp (newest first)
fn recent_memories(memories: &[MemoryItem], limit: usize) -> Vec<MemoryItem> {
    let mut results: Vec<_> = memories.iter().cloned().collect();
    results.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    results.truncate(limit);
    results
}

/// Get important memories above threshold, sorted by importance
fn important_memories(memories: &[MemoryItem], threshold: f32, limit: usize) -> Vec<MemoryItem> {
    let mut results: Vec<_> = memories
        .iter()
        .filter(|m| m.importance >= threshold)
        .cloned()
        .collect();
    results.sort_by(|a, b| {
        b.importance
            .partial_cmp(&a.importance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    results
}

/// Sort memory items by relevance score (highest first)
fn sort_by_relevance(items: &mut [MemoryItem]) {
    items.sort_by(|a, b| {
        b.relevance_score()
            .partial_cmp(&a.relevance_score())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

// ============================================================================
// In-Memory Store
// ============================================================================

/// Simple in-memory storage (for testing and development)
#[derive(Debug, Clone)]
pub struct InMemoryStore {
    memories: Arc<RwLock<Vec<MemoryItem>>>,
}

impl InMemoryStore {
    /// Create a new in-memory store
    pub fn new() -> Self {
        Self {
            memories: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl MemoryStore for InMemoryStore {
    async fn store(&self, item: MemoryItem) -> anyhow::Result<()> {
        let mut memories = self.memories.write().await;
        memories.push(item);
        Ok(())
    }

    async fn retrieve(&self, id: &str) -> anyhow::Result<Option<MemoryItem>> {
        let memories = self.memories.read().await;
        Ok(memories.iter().find(|m| m.id == id).cloned())
    }

    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        let memories = self.memories.read().await;
        Ok(search_memories(&memories, query, limit))
    }

    async fn search_by_tags(
        &self,
        tags: &[String],
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryItem>> {
        let memories = self.memories.read().await;
        Ok(search_memories_by_tags(&memories, tags, limit))
    }

    async fn get_recent(&self, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        let memories = self.memories.read().await;
        Ok(recent_memories(&memories, limit))
    }

    async fn get_important(&self, threshold: f32, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        let memories = self.memories.read().await;
        Ok(important_memories(&memories, threshold, limit))
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let mut memories = self.memories.write().await;
        memories.retain(|m| m.id != id);
        Ok(())
    }

    async fn clear(&self) -> anyhow::Result<()> {
        let mut memories = self.memories.write().await;
        memories.clear();
        Ok(())
    }

    async fn count(&self) -> anyhow::Result<usize> {
        let memories = self.memories.read().await;
        Ok(memories.len())
    }
}

// ============================================================================
// File-Based Store
// ============================================================================

/// File-based persistent storage using JSONL format
#[derive(Debug, Clone)]
pub struct FileStore {
    file_path: std::path::PathBuf,
    memories: Arc<RwLock<Vec<MemoryItem>>>,
}

impl FileStore {
    /// Create a new file-based store
    ///
    /// Note: This constructor performs blocking I/O to load existing memories.
    /// For async contexts, consider using `FileStore::open()` instead.
    pub fn new(file_path: impl Into<std::path::PathBuf>) -> anyhow::Result<Self> {
        let file_path = file_path.into();

        // Create parent directory if it doesn't exist
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Load existing memories from file
        let memories = if file_path.exists() {
            Self::load_from_file(&file_path)?
        } else {
            Vec::new()
        };

        Ok(Self {
            file_path,
            memories: Arc::new(RwLock::new(memories)),
        })
    }

    /// Create a new file-based store asynchronously
    pub async fn open(file_path: impl Into<std::path::PathBuf>) -> anyhow::Result<Self> {
        let file_path = file_path.into();

        // Create parent directory if it doesn't exist
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Load existing memories from file
        let memories = if file_path.exists() {
            let content = tokio::fs::read_to_string(&file_path).await?;
            Self::parse_jsonl(&content)?
        } else {
            Vec::new()
        };

        Ok(Self {
            file_path,
            memories: Arc::new(RwLock::new(memories)),
        })
    }

    /// Load memories from JSONL file (blocking)
    fn load_from_file(path: &std::path::Path) -> anyhow::Result<Vec<MemoryItem>> {
        let content = std::fs::read_to_string(path)?;
        Self::parse_jsonl(&content)
    }

    /// Parse JSONL content into memory items
    fn parse_jsonl(content: &str) -> anyhow::Result<Vec<MemoryItem>> {
        let mut memories = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let item: MemoryItem = serde_json::from_str(line)?;
            memories.push(item);
        }

        Ok(memories)
    }

    /// Save all memories to JSONL file
    async fn save_to_file(&self) -> anyhow::Result<()> {
        let memories = self.memories.read().await;
        let mut content = String::new();

        for memory in memories.iter() {
            let json = serde_json::to_string(memory)?;
            content.push_str(&json);
            content.push('\n');
        }

        // Write atomically using a temporary file
        let temp_path = self.file_path.with_extension("tmp");
        tokio::fs::write(&temp_path, content).await?;
        tokio::fs::rename(&temp_path, &self.file_path).await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl MemoryStore for FileStore {
    async fn store(&self, item: MemoryItem) -> anyhow::Result<()> {
        {
            let mut memories = self.memories.write().await;
            memories.push(item);
        }
        self.save_to_file().await
    }

    async fn retrieve(&self, id: &str) -> anyhow::Result<Option<MemoryItem>> {
        let memories = self.memories.read().await;
        Ok(memories.iter().find(|m| m.id == id).cloned())
    }

    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        let memories = self.memories.read().await;
        Ok(search_memories(&memories, query, limit))
    }

    async fn search_by_tags(
        &self,
        tags: &[String],
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryItem>> {
        let memories = self.memories.read().await;
        Ok(search_memories_by_tags(&memories, tags, limit))
    }

    async fn get_recent(&self, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        let memories = self.memories.read().await;
        Ok(recent_memories(&memories, limit))
    }

    async fn get_important(&self, threshold: f32, limit: usize) -> anyhow::Result<Vec<MemoryItem>> {
        let memories = self.memories.read().await;
        Ok(important_memories(&memories, threshold, limit))
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        {
            let mut memories = self.memories.write().await;
            memories.retain(|m| m.id != id);
        }
        self.save_to_file().await
    }

    async fn clear(&self) -> anyhow::Result<()> {
        {
            let mut memories = self.memories.write().await;
            memories.clear();
        }
        self.save_to_file().await
    }

    async fn count(&self) -> anyhow::Result<usize> {
        let memories = self.memories.read().await;
        Ok(memories.len())
    }
}

// ============================================================================
// Agent Memory
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
}

impl AgentMemory {
    /// Create a new agent memory system
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self {
            store,
            short_term: Arc::new(RwLock::new(VecDeque::new())),
            working: Arc::new(RwLock::new(Vec::new())),
            max_short_term: 100,
            max_working: 10,
        }
    }

    /// Create with in-memory store (for testing)
    pub fn in_memory() -> Self {
        Self::new(Arc::new(InMemoryStore::new()))
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
            working.sort_by(|a, b| {
                b.relevance_score()
                    .partial_cmp(&a.relevance_score())
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
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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

    #[tokio::test]
    async fn test_in_memory_store() {
        let store = InMemoryStore::new();

        let item = MemoryItem::new("Test memory").with_tag("test");
        store.store(item.clone()).await.unwrap();

        let retrieved = store.retrieve(&item.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().content, "Test memory");
    }

    #[tokio::test]
    async fn test_memory_search() {
        let store = InMemoryStore::new();

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
        let memory = AgentMemory::in_memory();

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
        let memory = AgentMemory::in_memory();

        let item = MemoryItem::new("Active task").with_type(MemoryType::Working);
        memory.add_to_working(item).await.unwrap();

        let working = memory.get_working().await;
        assert_eq!(working.len(), 1);

        memory.clear_working().await;
        let working = memory.get_working().await;
        assert_eq!(working.len(), 0);
    }

    #[tokio::test]
    async fn test_file_store_basic() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("test_memory_{}.jsonl", uuid::Uuid::new_v4()));

        // Create store
        let store = FileStore::new(&test_file).unwrap();

        // Store items
        let item1 = MemoryItem::new("Test memory 1").with_tag("test");
        let item2 = MemoryItem::new("Test memory 2").with_tag("test");

        store.store(item1.clone()).await.unwrap();
        store.store(item2.clone()).await.unwrap();

        // Verify count
        assert_eq!(store.count().await.unwrap(), 2);

        // Retrieve
        let retrieved = store.retrieve(&item1.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().content, "Test memory 1");

        // Clean up
        let _ = std::fs::remove_file(&test_file);
    }

    #[tokio::test]
    async fn test_file_store_persistence() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!(
            "test_memory_persist_{}.jsonl",
            uuid::Uuid::new_v4()
        ));

        let item_id = {
            // Create store and add item
            let store = FileStore::new(&test_file).unwrap();
            let item = MemoryItem::new("Persistent memory").with_importance(0.9);
            let id = item.id.clone();
            store.store(item).await.unwrap();
            id
        };

        // Create new store instance (simulating restart)
        let store2 = FileStore::new(&test_file).unwrap();

        // Verify data persisted
        assert_eq!(store2.count().await.unwrap(), 1);
        let retrieved = store2.retrieve(&item_id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().content, "Persistent memory");

        // Clean up
        let _ = std::fs::remove_file(&test_file);
    }

    #[tokio::test]
    async fn test_file_store_search() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("test_memory_search_{}.jsonl", uuid::Uuid::new_v4()));

        let store = FileStore::new(&test_file).unwrap();

        // Store multiple items
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

        // Search by content
        let results = store.search("create", 10).await.unwrap();
        assert_eq!(results.len(), 2);

        // Search by tags
        let results = store
            .search_by_tags(&["file".to_string()], 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        // Clean up
        let _ = std::fs::remove_file(&test_file);
    }

    #[tokio::test]
    async fn test_file_store_delete() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("test_memory_delete_{}.jsonl", uuid::Uuid::new_v4()));

        let store = FileStore::new(&test_file).unwrap();

        let item = MemoryItem::new("To be deleted");
        let item_id = item.id.clone();
        store.store(item).await.unwrap();

        assert_eq!(store.count().await.unwrap(), 1);

        // Delete
        store.delete(&item_id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);

        // Verify persistence
        let store2 = FileStore::new(&test_file).unwrap();
        assert_eq!(store2.count().await.unwrap(), 0);

        // Clean up
        let _ = std::fs::remove_file(&test_file);
    }

    #[tokio::test]
    async fn test_file_store_clear() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("test_memory_clear_{}.jsonl", uuid::Uuid::new_v4()));

        let store = FileStore::new(&test_file).unwrap();

        // Store multiple items
        for i in 0..5 {
            store
                .store(MemoryItem::new(format!("Memory {}", i)))
                .await
                .unwrap();
        }

        assert_eq!(store.count().await.unwrap(), 5);

        // Clear
        store.clear().await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);

        // Verify persistence
        let store2 = FileStore::new(&test_file).unwrap();
        assert_eq!(store2.count().await.unwrap(), 0);

        // Clean up
        let _ = std::fs::remove_file(&test_file);
    }
}

#[cfg(test)]
mod extra_memory_tests {
    use super::*;

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
    // InMemoryStore
    // ========================================================================

    #[tokio::test]
    async fn test_in_memory_store_retrieve_nonexistent() {
        let store = InMemoryStore::new();
        let result = store.retrieve("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_store_delete() {
        let store = InMemoryStore::new();
        let item = MemoryItem::new("to delete");
        let id = item.id.clone();
        store.store(item).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);

        store.delete(&id).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_in_memory_store_clear() {
        let store = InMemoryStore::new();
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
        let store = InMemoryStore::new();
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
        let store = InMemoryStore::new();
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
        let store = InMemoryStore::new();
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
        let memory = AgentMemory::in_memory();
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
        let memory = AgentMemory::in_memory();
        assert_eq!(memory.short_term_count().await, 0);
        memory.remember(MemoryItem::new("item")).await.unwrap();
        assert_eq!(memory.short_term_count().await, 1);
    }

    #[tokio::test]
    async fn test_agent_memory_working_count() {
        let memory = AgentMemory::in_memory();
        assert_eq!(memory.working_count().await, 0);
        memory
            .add_to_working(MemoryItem::new("task"))
            .await
            .unwrap();
        assert_eq!(memory.working_count().await, 1);
    }

    #[tokio::test]
    async fn test_agent_memory_recall_by_tags() {
        let memory = AgentMemory::in_memory();
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
        let memory = AgentMemory::in_memory();
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
        let memory = AgentMemory::in_memory();
        memory.remember(MemoryItem::new("test")).await.unwrap();
        let count = memory.store().count().await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_agent_memory_stats_all_fields() {
        let memory = AgentMemory::in_memory();
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
        let store = Arc::new(InMemoryStore::new());
        let memory = AgentMemory {
            store,
            short_term: Arc::new(RwLock::new(VecDeque::new())),
            working: Arc::new(RwLock::new(Vec::new())),
            max_short_term: 100,
            max_working: 3, // Small limit
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

#[cfg(test)]
mod extra_memory_tests2 {
    use super::*;

    #[tokio::test]
    async fn test_file_store_open_creates_parent_dirs() {
        // Use a nested path that doesn't exist yet
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("nested")
            .join("deep")
            .join("memories.jsonl");
        let store = FileStore::open(&path).await.unwrap();
        // Should create the parent dirs and start with empty memories
        let all = store.search("", 100).await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_file_store_open_loads_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memories.jsonl");
        // Create a store, add a memory, which saves to file
        {
            let store = FileStore::open(&path).await.unwrap();
            let item = MemoryItem::new("test memory".to_string());
            store.store(item).await.unwrap();
        }
        // Re-open and verify the memory persists
        let store = FileStore::open(&path).await.unwrap();
        let results = store.search("test", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("test memory"));
    }

    #[tokio::test]
    async fn test_file_store_open_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.jsonl");
        let store = FileStore::open(&path).await.unwrap();
        let all = store.search("", 100).await.unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn test_parse_jsonl_empty_string() {
        let result = FileStore::parse_jsonl("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_jsonl_empty_lines_skipped() {
        // Create valid JSONL with empty lines interspersed
        let item = MemoryItem::new("hello".to_string());
        let json = serde_json::to_string(&item).unwrap();
        let content = format!("\n{}\n\n{}\n\n", json, json);
        let result = FileStore::parse_jsonl(&content).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_parse_jsonl_invalid_json_returns_error() {
        let result = FileStore::parse_jsonl("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_jsonl_valid_single_line() {
        let item = MemoryItem::new("single".to_string());
        let json = serde_json::to_string(&item).unwrap();
        let result = FileStore::parse_jsonl(&json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "single");
    }
}
