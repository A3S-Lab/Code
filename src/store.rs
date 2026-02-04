//! Session persistence layer
//!
//! Provides pluggable session storage via the `SessionStore` trait.
//!
//! ## Default Implementation
//!
//! `FileSessionStore` stores each session as a JSON file:
//! - Session metadata (id, name, timestamps)
//! - Configuration (system prompt, policies)
//! - Conversation history (messages)
//! - Context usage statistics
//!
//! ## Custom Backends
//!
//! Implement `SessionStore` trait for custom backends (Redis, PostgreSQL, etc.):
//!
//! ```ignore
//! use a3s_box_code::store::{SessionStore, SessionData};
//!
//! struct RedisStore { /* ... */ }
//!
//! #[async_trait::async_trait]
//! impl SessionStore for RedisStore {
//!     async fn save(&self, session: &SessionData) -> Result<()> { /* ... */ }
//!     async fn load(&self, id: &str) -> Result<Option<SessionData>> { /* ... */ }
//!     async fn delete(&self, id: &str) -> Result<()> { /* ... */ }
//!     async fn list(&self) -> Result<Vec<String>> { /* ... */ }
//!     async fn exists(&self, id: &str) -> Result<bool> { /* ... */ }
//! }
//! ```

use crate::llm::{Message, TokenUsage, ToolDefinition};
use crate::session::{ContextUsage, SessionConfig, SessionState};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

// ============================================================================
// Serializable Session Data
// ============================================================================

/// Serializable session data for persistence
///
/// Contains only the fields that can be serialized.
/// Non-serializable fields (event_tx, command_queue, etc.) are rebuilt on load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    /// Session ID
    pub id: String,

    /// Session configuration
    pub config: SessionConfig,

    /// Current state
    pub state: SessionState,

    /// Conversation history
    pub messages: Vec<Message>,

    /// Context usage statistics
    pub context_usage: ContextUsage,

    /// Total token usage
    pub total_usage: TokenUsage,

    /// Tool definitions (names only, rebuilt from executor on load)
    pub tool_names: Vec<String>,

    /// Whether thinking mode is enabled
    pub thinking_enabled: bool,

    /// Thinking budget if set
    pub thinking_budget: Option<usize>,

    /// Creation timestamp (Unix epoch seconds)
    pub created_at: i64,

    /// Last update timestamp (Unix epoch seconds)
    pub updated_at: i64,

    /// LLM configuration for per-session client (if set)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_config: Option<LlmConfigData>,
}

/// Serializable LLM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfigData {
    pub provider: String,
    pub model: String,
    /// API key is NOT stored - must be provided on session resume
    #[serde(skip_serializing, default)]
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

impl SessionData {
    /// Extract tool names from definitions
    pub fn tool_names_from_definitions(tools: &[ToolDefinition]) -> Vec<String> {
        tools.iter().map(|t| t.name.clone()).collect()
    }
}

// ============================================================================
// Session Store Trait
// ============================================================================

/// Session storage trait
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Save session data
    async fn save(&self, session: &SessionData) -> Result<()>;

    /// Load session data by ID
    async fn load(&self, id: &str) -> Result<Option<SessionData>>;

    /// Delete session data
    async fn delete(&self, id: &str) -> Result<()>;

    /// List all session IDs
    async fn list(&self) -> Result<Vec<String>>;

    /// Check if session exists
    async fn exists(&self, id: &str) -> Result<bool>;
}

// ============================================================================
// File-based Session Store
// ============================================================================

/// File-based session store
///
/// Stores each session as a JSON file in a directory:
/// ```
/// sessions/
///   session-1.json
///   session-2.json
/// ```
pub struct FileSessionStore {
    /// Directory to store session files
    dir: PathBuf,
}

impl FileSessionStore {
    /// Create a new file session store
    ///
    /// Creates the directory if it doesn't exist.
    pub async fn new<P: AsRef<Path>>(dir: P) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();

        // Create directory if it doesn't exist
        fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("Failed to create session directory: {}", dir.display()))?;

        Ok(Self { dir })
    }

    /// Get the file path for a session
    fn session_path(&self, id: &str) -> PathBuf {
        // Sanitize ID to prevent path traversal
        let safe_id = id.replace(['/', '\\'], "_").replace("..", "_");
        self.dir.join(format!("{}.json", safe_id))
    }
}

#[async_trait::async_trait]
impl SessionStore for FileSessionStore {
    async fn save(&self, session: &SessionData) -> Result<()> {
        let path = self.session_path(&session.id);

        // Serialize to JSON with pretty printing for readability
        let json = serde_json::to_string_pretty(session)
            .with_context(|| format!("Failed to serialize session: {}", session.id))?;

        // Write atomically: write to temp file, then rename
        let temp_path = path.with_extension("json.tmp");

        let mut file = fs::File::create(&temp_path)
            .await
            .with_context(|| format!("Failed to create temp file: {}", temp_path.display()))?;

        file.write_all(json.as_bytes())
            .await
            .with_context(|| format!("Failed to write session data: {}", session.id))?;

        file.sync_all()
            .await
            .with_context(|| format!("Failed to sync session file: {}", session.id))?;

        // Rename temp file to final path (atomic on most filesystems)
        fs::rename(&temp_path, &path)
            .await
            .with_context(|| format!("Failed to rename session file: {}", session.id))?;

        tracing::debug!("Saved session {} to {}", session.id, path.display());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<SessionData>> {
        let path = self.session_path(id);

        if !path.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read session file: {}", path.display()))?;

        let session: SessionData = serde_json::from_str(&json)
            .with_context(|| format!("Failed to parse session file: {}", path.display()))?;

        tracing::debug!("Loaded session {} from {}", id, path.display());
        Ok(Some(session))
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let path = self.session_path(id);

        if path.exists() {
            fs::remove_file(&path)
                .await
                .with_context(|| format!("Failed to delete session file: {}", path.display()))?;

            tracing::debug!("Deleted session {} from {}", id, path.display());
        }

        Ok(())
    }

    async fn list(&self) -> Result<Vec<String>> {
        let mut session_ids = Vec::new();

        let mut entries = fs::read_dir(&self.dir)
            .await
            .with_context(|| format!("Failed to read session directory: {}", self.dir.display()))?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "json") {
                if let Some(stem) = path.file_stem() {
                    if let Some(id) = stem.to_str() {
                        session_ids.push(id.to_string());
                    }
                }
            }
        }

        Ok(session_ids)
    }

    async fn exists(&self, id: &str) -> Result<bool> {
        let path = self.session_path(id);
        Ok(path.exists())
    }
}

// ============================================================================
// In-Memory Session Store (for testing)
// ============================================================================

/// In-memory session store for testing
pub struct MemorySessionStore {
    sessions: tokio::sync::RwLock<HashMap<String, SessionData>>,
}

impl MemorySessionStore {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            sessions: tokio::sync::RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SessionStore for MemorySessionStore {
    async fn save(&self, session: &SessionData) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id.clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<SessionData>> {
        let sessions = self.sessions.read().await;
        Ok(sessions.get(id).cloned())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(id);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<String>> {
        let sessions = self.sessions.read().await;
        Ok(sessions.keys().cloned().collect())
    }

    async fn exists(&self, id: &str) -> Result<bool> {
        let sessions = self.sessions.read().await;
        Ok(sessions.contains_key(id))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hitl::ConfirmationPolicy;
    use crate::permissions::PermissionPolicy;
    use crate::queue::SessionQueueConfig;
    use tempfile::tempdir;

    fn create_test_session_data() -> SessionData {
        SessionData {
            id: "test-session-1".to_string(),
            config: SessionConfig {
                name: "Test Session".to_string(),
                workspace: "/tmp/workspace".to_string(),
                system_prompt: Some("You are helpful.".to_string()),
                max_context_length: 200000,
                auto_compact: false,
                queue_config: None,
                confirmation_policy: None,
                permission_policy: None,
            },
            state: SessionState::Active,
            messages: vec![
                Message::user("Hello"),
                Message {
                    role: "assistant".to_string(),
                    content: vec![crate::llm::ContentBlock::Text {
                        text: "Hi there!".to_string(),
                    }],
                },
            ],
            context_usage: ContextUsage {
                used_tokens: 100,
                max_tokens: 200000,
                percent: 0.0005,
                turns: 2,
            },
            total_usage: TokenUsage {
                prompt_tokens: 50,
                completion_tokens: 50,
                total_tokens: 100,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            tool_names: vec!["bash".to_string(), "read".to_string()],
            thinking_enabled: false,
            thinking_budget: None,
            created_at: 1700000000,
            updated_at: 1700000100,
            llm_config: None,
        }
    }

    // ========================================================================
    // FileSessionStore Tests
    // ========================================================================

    #[tokio::test]
    async fn test_file_store_save_and_load() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        let session = create_test_session_data();

        // Save
        store.save(&session).await.unwrap();

        // Load
        let loaded = store.load(&session.id).await.unwrap();
        assert!(loaded.is_some());

        let loaded = loaded.unwrap();
        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.config.name, session.config.name);
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.state, SessionState::Active);
    }

    #[tokio::test]
    async fn test_file_store_load_nonexistent() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        let loaded = store.load("nonexistent").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_file_store_delete() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        let session = create_test_session_data();
        store.save(&session).await.unwrap();

        // Verify exists
        assert!(store.exists(&session.id).await.unwrap());

        // Delete
        store.delete(&session.id).await.unwrap();

        // Verify gone
        assert!(!store.exists(&session.id).await.unwrap());
        assert!(store.load(&session.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_file_store_list() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        // Initially empty
        let list = store.list().await.unwrap();
        assert!(list.is_empty());

        // Add sessions
        for i in 1..=3 {
            let mut session = create_test_session_data();
            session.id = format!("session-{}", i);
            store.save(&session).await.unwrap();
        }

        // List should have 3 sessions
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
        assert!(list.contains(&"session-1".to_string()));
        assert!(list.contains(&"session-2".to_string()));
        assert!(list.contains(&"session-3".to_string()));
    }

    #[tokio::test]
    async fn test_file_store_overwrite() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        let mut session = create_test_session_data();
        store.save(&session).await.unwrap();

        // Modify and save again
        session.messages.push(Message::user("Another message"));
        session.updated_at = 1700000200;
        store.save(&session).await.unwrap();

        // Load and verify
        let loaded = store.load(&session.id).await.unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 3);
        assert_eq!(loaded.updated_at, 1700000200);
    }

    #[tokio::test]
    async fn test_file_store_path_traversal_prevention() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        // Attempt path traversal - should be sanitized
        let mut session = create_test_session_data();
        session.id = "../../../etc/passwd".to_string();
        store.save(&session).await.unwrap();

        // File should be in the store directory, not /etc/passwd
        let files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1);

        // Should still be loadable with sanitized ID
        let loaded = store.load(&session.id).await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn test_file_store_with_policies() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        let mut session = create_test_session_data();
        session.config.confirmation_policy = Some(ConfirmationPolicy::enabled());
        session.config.permission_policy = Some(PermissionPolicy::new().allow("Bash(cargo:*)"));
        session.config.queue_config = Some(SessionQueueConfig::default());

        store.save(&session).await.unwrap();

        let loaded = store.load(&session.id).await.unwrap().unwrap();
        assert!(loaded.config.confirmation_policy.is_some());
        assert!(loaded.config.permission_policy.is_some());
        assert!(loaded.config.queue_config.is_some());
    }

    #[tokio::test]
    async fn test_file_store_with_llm_config() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        let mut session = create_test_session_data();
        session.llm_config = Some(LlmConfigData {
            provider: "anthropic".to_string(),
            model: "claude-3-5-sonnet-20241022".to_string(),
            api_key: Some("secret".to_string()), // Should NOT be saved
            base_url: None,
        });

        store.save(&session).await.unwrap();

        let loaded = store.load(&session.id).await.unwrap().unwrap();
        let llm_config = loaded.llm_config.unwrap();
        assert_eq!(llm_config.provider, "anthropic");
        assert_eq!(llm_config.model, "claude-3-5-sonnet-20241022");
        // API key should not be persisted
        assert!(llm_config.api_key.is_none());
    }

    // ========================================================================
    // MemorySessionStore Tests
    // ========================================================================

    #[tokio::test]
    async fn test_memory_store_save_and_load() {
        let store = MemorySessionStore::new();
        let session = create_test_session_data();

        store.save(&session).await.unwrap();

        let loaded = store.load(&session.id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id, session.id);
    }

    #[tokio::test]
    async fn test_memory_store_delete() {
        let store = MemorySessionStore::new();
        let session = create_test_session_data();

        store.save(&session).await.unwrap();
        assert!(store.exists(&session.id).await.unwrap());

        store.delete(&session.id).await.unwrap();
        assert!(!store.exists(&session.id).await.unwrap());
    }

    #[tokio::test]
    async fn test_memory_store_list() {
        let store = MemorySessionStore::new();

        for i in 1..=3 {
            let mut session = create_test_session_data();
            session.id = format!("session-{}", i);
            store.save(&session).await.unwrap();
        }

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 3);
    }

    // ========================================================================
    // SessionData Tests
    // ========================================================================

    #[test]
    fn test_session_data_serialization() {
        let session = create_test_session_data();
        let json = serde_json::to_string(&session).unwrap();
        let parsed: SessionData = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, session.id);
        assert_eq!(parsed.messages.len(), session.messages.len());
    }

    #[test]
    fn test_tool_names_from_definitions() {
        let tools = vec![
            crate::llm::ToolDefinition {
                name: "bash".to_string(),
                description: "Execute bash".to_string(),
                parameters: serde_json::json!({}),
            },
            crate::llm::ToolDefinition {
                name: "read".to_string(),
                description: "Read file".to_string(),
                parameters: serde_json::json!({}),
            },
        ];

        let names = SessionData::tool_names_from_definitions(&tools);
        assert_eq!(names, vec!["bash", "read"]);
    }
}
