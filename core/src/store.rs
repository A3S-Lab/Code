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
//! use a3s_code::store::{SessionStore, SessionData};
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
use crate::planning::Task;
use crate::prompts::PlanningMode;
use crate::queue::SessionQueueConfig;
use crate::tools::ArtifactStore;
use crate::trace::TraceEvent;
use crate::verification::VerificationReport;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

// ============================================================================
// Serializable Session Data
// ============================================================================

/// Session state persisted with saved sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SessionState {
    #[default]
    Unknown = 0,
    Active = 1,
    Paused = 2,
    Completed = 3,
    Error = 4,
}

/// Context usage statistics persisted with saved sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextUsage {
    pub used_tokens: usize,
    pub max_tokens: usize,
    pub percent: f32,
    pub turns: usize,
}

impl Default for ContextUsage {
    fn default() -> Self {
        Self {
            used_tokens: 0,
            max_tokens: 200_000,
            percent: 0.0,
            turns: 0,
        }
    }
}

/// Default auto-compact threshold (80% of context window).
pub const DEFAULT_AUTO_COMPACT_THRESHOLD: f32 = 0.80;

fn default_auto_compact_threshold() -> f32 {
    DEFAULT_AUTO_COMPACT_THRESHOLD
}

/// Serializable session configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub name: String,
    pub workspace: String,
    pub system_prompt: Option<String>,
    pub max_context_length: u32,
    pub auto_compact: bool,
    /// Context usage percentage threshold to trigger auto-compaction (0.0 - 1.0).
    /// Only used when `auto_compact` is true. Default: 0.80 (80%).
    #[serde(default = "default_auto_compact_threshold")]
    pub auto_compact_threshold: f32,
    /// Storage type for this session.
    #[serde(default)]
    pub storage_type: crate::config::StorageBackend,
    /// Optional advanced queue configuration.
    ///
    /// Queue infrastructure is initialized only when this is set. Ordinary
    /// sessions stay queue-free.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_config: Option<SessionQueueConfig>,
    /// Confirmation policy (optional, uses defaults if None).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_policy: Option<crate::hitl::ConfirmationPolicy>,
    /// Permission policy (optional, uses defaults if None).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_policy: Option<crate::permissions::PermissionPolicy>,
    /// Parent session ID (for subagent sessions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Security configuration (optional, enables security features).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_config: Option<crate::security::SecurityConfig>,
    /// Shared hook engine for lifecycle events.
    #[serde(skip)]
    pub hook_engine: Option<std::sync::Arc<dyn crate::hooks::HookExecutor>>,
    /// Enable planning phase before execution.
    #[serde(default)]
    pub planning_mode: PlanningMode,
    /// Enable goal tracking.
    #[serde(default)]
    pub goal_tracking: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            workspace: String::new(),
            system_prompt: None,
            max_context_length: 0,
            auto_compact: false,
            auto_compact_threshold: DEFAULT_AUTO_COMPACT_THRESHOLD,
            storage_type: crate::config::StorageBackend::default(),
            queue_config: None,
            confirmation_policy: None,
            permission_policy: None,
            parent_id: None,
            security_config: None,
            hook_engine: None,
            planning_mode: PlanningMode::default(),
            goal_tracking: false,
        }
    }
}

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

    /// Cumulative dollar cost for this session
    #[serde(default)]
    pub total_cost: f64,

    /// Model name for cost calculation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,

    /// LLM cost records for this session
    #[serde(default)]
    pub cost_records: Vec<crate::telemetry::LlmCostRecord>,

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

    /// Task list for tracking
    #[serde(default, alias = "todos")]
    pub tasks: Vec<Task>,

    /// Parent session ID (for subagent sessions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
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

    /// Save artifacts associated with a session.
    async fn save_artifacts(&self, _id: &str, _artifacts: &ArtifactStore) -> Result<()> {
        Ok(())
    }

    /// Load artifacts associated with a session.
    async fn load_artifacts(&self, _id: &str) -> Result<Option<ArtifactStore>> {
        Ok(None)
    }

    /// Save compact trace events associated with a session.
    async fn save_trace_events(&self, _id: &str, _events: &[TraceEvent]) -> Result<()> {
        Ok(())
    }

    /// Load compact trace events associated with a session.
    async fn load_trace_events(&self, _id: &str) -> Result<Option<Vec<TraceEvent>>> {
        Ok(None)
    }

    /// Save structured verification reports associated with a session.
    async fn save_verification_reports(
        &self,
        _id: &str,
        _reports: &[VerificationReport],
    ) -> Result<()> {
        Ok(())
    }

    /// Load structured verification reports associated with a session.
    async fn load_verification_reports(
        &self,
        _id: &str,
    ) -> Result<Option<Vec<VerificationReport>>> {
        Ok(None)
    }

    /// Health check — verify the store backend is reachable and operational
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }

    /// Backend name for diagnostics
    fn backend_name(&self) -> &str {
        "unknown"
    }
}

// ============================================================================
// File-based Session Store
// ============================================================================

/// File-based session store
///
/// Stores each session as a JSON file in a directory:
/// ```text
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
        self.dir.join(format!("{}.json", safe_session_id(id)))
    }

    fn artifact_dir(&self, id: &str) -> PathBuf {
        self.dir.join("artifacts").join(safe_session_id(id))
    }

    fn trace_path(&self, id: &str) -> PathBuf {
        self.dir
            .join("traces")
            .join(format!("{}.json", safe_session_id(id)))
    }

    fn verification_path(&self, id: &str) -> PathBuf {
        self.dir
            .join("verification")
            .join(format!("{}.json", safe_session_id(id)))
    }
}

fn safe_session_id(id: &str) -> String {
    id.replace(['/', '\\'], "_").replace("..", "_")
}

#[async_trait::async_trait]
impl SessionStore for FileSessionStore {
    async fn save(&self, session: &SessionData) -> Result<()> {
        let path = self.session_path(&session.id);

        // Serialize to JSON with pretty printing for readability
        let json = serde_json::to_string_pretty(session)
            .with_context(|| format!("Failed to serialize session: {}", session.id))?;

        // Write atomically: write to temp file with unique name, then rename
        // Use timestamp + process ID to ensure uniqueness for concurrent saves
        let unique_suffix = format!(
            "{}.{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            std::process::id()
        );
        let temp_path = path.with_extension(format!("json.{}.tmp", unique_suffix));

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

        let artifact_dir = self.artifact_dir(id);
        if artifact_dir.exists() {
            fs::remove_dir_all(&artifact_dir).await.with_context(|| {
                format!(
                    "Failed to delete artifact directory for session {}: {}",
                    id,
                    artifact_dir.display()
                )
            })?;
        }

        let trace_path = self.trace_path(id);
        if trace_path.exists() {
            fs::remove_file(&trace_path).await.with_context(|| {
                format!(
                    "Failed to delete trace file for session {}: {}",
                    id,
                    trace_path.display()
                )
            })?;
        }

        let verification_path = self.verification_path(id);
        if verification_path.exists() {
            fs::remove_file(&verification_path).await.with_context(|| {
                format!(
                    "Failed to delete verification report file for session {}: {}",
                    id,
                    verification_path.display()
                )
            })?;
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

    async fn save_artifacts(&self, id: &str, artifacts: &ArtifactStore) -> Result<()> {
        let artifact_dir = self.artifact_dir(id);
        artifacts.save_to_dir(&artifact_dir).with_context(|| {
            format!(
                "Failed to save artifacts for session {} to {}",
                id,
                artifact_dir.display()
            )
        })
    }

    async fn load_artifacts(&self, id: &str) -> Result<Option<ArtifactStore>> {
        let artifact_dir = self.artifact_dir(id);
        if !artifact_dir.exists() {
            return Ok(None);
        }

        let artifacts = ArtifactStore::load_from_dir(&artifact_dir).with_context(|| {
            format!(
                "Failed to load artifacts for session {} from {}",
                id,
                artifact_dir.display()
            )
        })?;
        Ok(Some(artifacts))
    }

    async fn save_trace_events(&self, id: &str, events: &[TraceEvent]) -> Result<()> {
        let path = self.trace_path(id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.with_context(|| {
                format!("Failed to create trace directory: {}", parent.display())
            })?;
        }

        let json = serde_json::to_string_pretty(events)
            .with_context(|| format!("Failed to serialize trace events for session {id}"))?;
        fs::write(&path, json)
            .await
            .with_context(|| format!("Failed to write trace events to {}", path.display()))?;
        Ok(())
    }

    async fn load_trace_events(&self, id: &str) -> Result<Option<Vec<TraceEvent>>> {
        let path = self.trace_path(id);
        if !path.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read trace events from {}", path.display()))?;
        let events = serde_json::from_str(&json)
            .with_context(|| format!("Failed to parse trace events from {}", path.display()))?;
        Ok(Some(events))
    }

    async fn save_verification_reports(
        &self,
        id: &str,
        reports: &[VerificationReport],
    ) -> Result<()> {
        let path = self.verification_path(id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.with_context(|| {
                format!(
                    "Failed to create verification report directory: {}",
                    parent.display()
                )
            })?;
        }

        let json = serde_json::to_string_pretty(reports).with_context(|| {
            format!("Failed to serialize verification reports for session {id}")
        })?;
        fs::write(&path, json).await.with_context(|| {
            format!("Failed to write verification reports to {}", path.display())
        })?;
        Ok(())
    }

    async fn load_verification_reports(&self, id: &str) -> Result<Option<Vec<VerificationReport>>> {
        let path = self.verification_path(id);
        if !path.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(&path).await.with_context(|| {
            format!(
                "Failed to read verification reports from {}",
                path.display()
            )
        })?;
        let reports = serde_json::from_str(&json).with_context(|| {
            format!(
                "Failed to parse verification reports from {}",
                path.display()
            )
        })?;
        Ok(Some(reports))
    }

    async fn health_check(&self) -> Result<()> {
        // Verify directory exists and is writable
        let probe = self.dir.join(".health_check");
        fs::write(&probe, b"ok")
            .await
            .with_context(|| format!("Store directory not writable: {}", self.dir.display()))?;
        let _ = fs::remove_file(&probe).await;
        Ok(())
    }

    fn backend_name(&self) -> &str {
        "file"
    }
}

// ============================================================================
// In-Memory Session Store (for testing)
// ============================================================================

/// In-memory session store for testing
pub struct MemorySessionStore {
    sessions: tokio::sync::RwLock<HashMap<String, SessionData>>,
    artifacts: tokio::sync::RwLock<HashMap<String, ArtifactStore>>,
    trace_events: tokio::sync::RwLock<HashMap<String, Vec<TraceEvent>>>,
    verification_reports: tokio::sync::RwLock<HashMap<String, Vec<VerificationReport>>>,
}

impl MemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: tokio::sync::RwLock::new(HashMap::new()),
            artifacts: tokio::sync::RwLock::new(HashMap::new()),
            trace_events: tokio::sync::RwLock::new(HashMap::new()),
            verification_reports: tokio::sync::RwLock::new(HashMap::new()),
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
        self.artifacts.write().await.remove(id);
        self.trace_events.write().await.remove(id);
        self.verification_reports.write().await.remove(id);
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

    async fn save_artifacts(&self, id: &str, artifacts: &ArtifactStore) -> Result<()> {
        self.artifacts
            .write()
            .await
            .insert(id.to_string(), artifacts.clone());
        Ok(())
    }

    async fn load_artifacts(&self, id: &str) -> Result<Option<ArtifactStore>> {
        Ok(self.artifacts.read().await.get(id).cloned())
    }

    async fn save_trace_events(&self, id: &str, events: &[TraceEvent]) -> Result<()> {
        self.trace_events
            .write()
            .await
            .insert(id.to_string(), events.to_vec());
        Ok(())
    }

    async fn load_trace_events(&self, id: &str) -> Result<Option<Vec<TraceEvent>>> {
        Ok(self.trace_events.read().await.get(id).cloned())
    }

    async fn save_verification_reports(
        &self,
        id: &str,
        reports: &[VerificationReport],
    ) -> Result<()> {
        self.verification_reports
            .write()
            .await
            .insert(id.to_string(), reports.to_vec());
        Ok(())
    }

    async fn load_verification_reports(&self, id: &str) -> Result<Option<Vec<VerificationReport>>> {
        Ok(self.verification_reports.read().await.get(id).cloned())
    }

    fn backend_name(&self) -> &str {
        "memory"
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
    use crate::prompts::PlanningMode;
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
                auto_compact_threshold: DEFAULT_AUTO_COMPACT_THRESHOLD,
                storage_type: crate::config::StorageBackend::File,
                queue_config: None,
                confirmation_policy: None,
                permission_policy: None,
                parent_id: None,
                security_config: None,
                hook_engine: None,
                planning_mode: PlanningMode::default(),
                goal_tracking: false,
            },
            state: SessionState::Active,
            messages: vec![
                Message::user("Hello"),
                Message {
                    role: "assistant".to_string(),
                    content: vec![crate::llm::ContentBlock::Text {
                        text: "Hi there!".to_string(),
                    }],
                    reasoning_content: None,
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
            tasks: vec![],
            parent_id: None,
            total_cost: 0.0,
            model_name: None,
            cost_records: Vec::new(),
        }
    }

    fn create_test_verification_report() -> VerificationReport {
        VerificationReport::new(
            "program:test",
            vec![crate::verification::VerificationCheck::required(
                "check:test",
                "test",
                "Run tests",
            )
            .with_status(crate::verification::VerificationStatus::Passed)],
        )
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
    async fn test_file_store_save_and_load_artifacts() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();
        let artifacts = ArtifactStore::new();
        artifacts.put(crate::tools::ToolArtifact {
            artifact_id: "tool-output:test:a".to_string(),
            artifact_uri: "a3s://tool-output/test/a".to_string(),
            tool_name: "test".to_string(),
            content: "artifact content".to_string(),
            original_bytes: 16,
            shown_bytes: 4,
        });

        store.save_artifacts("session/a", &artifacts).await.unwrap();
        let loaded = store
            .load_artifacts("session/a")
            .await
            .unwrap()
            .expect("artifacts");

        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded
                .get("a3s://tool-output/test/a")
                .expect("artifact")
                .content,
            "artifact content"
        );
    }

    #[tokio::test]
    async fn test_file_store_save_and_load_trace_events() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();
        let event = TraceEvent::tool_execution(
            "read",
            true,
            0,
            std::time::Duration::from_millis(9),
            12,
            Some(&serde_json::json!({
                "artifact": {
                    "artifact_uri": "a3s://tool-output/read/abc"
                }
            })),
        );

        store
            .save_trace_events("session/a", std::slice::from_ref(&event))
            .await
            .unwrap();
        let loaded = store
            .load_trace_events("session/a")
            .await
            .unwrap()
            .expect("trace events");

        assert_eq!(loaded, vec![event]);
    }

    #[tokio::test]
    async fn test_file_store_save_and_load_verification_reports() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();
        let report = create_test_verification_report();

        store
            .save_verification_reports("session/a", std::slice::from_ref(&report))
            .await
            .unwrap();
        let loaded = store
            .load_verification_reports("session/a")
            .await
            .unwrap()
            .expect("verification reports");

        assert_eq!(loaded, vec![report]);
    }

    #[tokio::test]
    async fn test_memory_store_save_load_and_delete_artifacts() {
        let store = MemorySessionStore::new();
        let session = create_test_session_data();
        store.save(&session).await.unwrap();
        let artifacts = ArtifactStore::new();
        artifacts.put(crate::tools::ToolArtifact {
            artifact_id: "tool-output:test:a".to_string(),
            artifact_uri: "a3s://tool-output/test/a".to_string(),
            tool_name: "test".to_string(),
            content: "artifact content".to_string(),
            original_bytes: 16,
            shown_bytes: 4,
        });

        store.save_artifacts(&session.id, &artifacts).await.unwrap();
        assert!(store
            .load_artifacts(&session.id)
            .await
            .unwrap()
            .expect("artifacts")
            .get("a3s://tool-output/test/a")
            .is_some());

        store.delete(&session.id).await.unwrap();
        assert!(store.load_artifacts(&session.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_memory_store_save_load_and_delete_trace_events() {
        let store = MemorySessionStore::new();
        let session = create_test_session_data();
        let event = TraceEvent::tool_execution(
            "grep",
            false,
            1,
            std::time::Duration::from_millis(2),
            24,
            None,
        );

        store.save(&session).await.unwrap();
        store
            .save_trace_events(&session.id, std::slice::from_ref(&event))
            .await
            .unwrap();
        let loaded = store
            .load_trace_events(&session.id)
            .await
            .unwrap()
            .expect("trace events");
        assert_eq!(loaded, vec![event]);

        store.delete(&session.id).await.unwrap();
        assert!(store
            .load_trace_events(&session.id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_memory_store_save_load_and_delete_verification_reports() {
        let store = MemorySessionStore::new();
        let session = create_test_session_data();
        let report = create_test_verification_report();

        store.save(&session).await.unwrap();
        store
            .save_verification_reports(&session.id, std::slice::from_ref(&report))
            .await
            .unwrap();
        let loaded = store
            .load_verification_reports(&session.id)
            .await
            .unwrap()
            .expect("verification reports");
        assert_eq!(loaded, vec![report]);

        store.delete(&session.id).await.unwrap();
        assert!(store
            .load_verification_reports(&session.id)
            .await
            .unwrap()
            .is_none());
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

    // ========================================================================
    // Sanitization Tests
    // ========================================================================

    #[tokio::test]
    async fn test_file_store_backslash_sanitization() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        let mut session = create_test_session_data();
        session.id = r"foo\bar\baz".to_string();
        store.save(&session).await.unwrap();

        let loaded = store.load(&session.id).await.unwrap();
        assert!(loaded.is_some());

        let loaded = loaded.unwrap();
        assert_eq!(loaded.id, session.id);

        // Verify the file on disk uses sanitized name
        let expected_path = dir.path().join("foo_bar_baz.json");
        assert!(expected_path.exists());
    }

    #[tokio::test]
    async fn test_file_store_mixed_separator_sanitization() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        let mut session = create_test_session_data();
        session.id = r"foo/bar\baz..qux".to_string();
        store.save(&session).await.unwrap();

        let loaded = store.load(&session.id).await.unwrap();
        assert!(loaded.is_some());

        let loaded = loaded.unwrap();
        assert_eq!(loaded.id, session.id);

        // / -> _, \ -> _, .. -> _
        let expected_path = dir.path().join("foo_bar_baz_qux.json");
        assert!(expected_path.exists());
    }

    // ========================================================================
    // Error Recovery Tests
    // ========================================================================

    #[tokio::test]
    async fn test_file_store_corrupted_json_recovery() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        // Manually write invalid JSON to a session file
        let corrupted_path = dir.path().join("test-id.json");
        tokio::fs::write(&corrupted_path, b"not valid json {{{")
            .await
            .unwrap();

        // Loading should return an error, not panic
        let result = store.load("test-id").await;
        assert!(result.is_err());
    }

    // ========================================================================
    // Exists Tests
    // ========================================================================

    #[tokio::test]
    async fn test_file_store_exists() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        let session = create_test_session_data();

        // Not yet saved
        assert!(!store.exists(&session.id).await.unwrap());

        // Save and verify exists
        store.save(&session).await.unwrap();
        assert!(store.exists(&session.id).await.unwrap());

        // Delete and verify gone
        store.delete(&session.id).await.unwrap();
        assert!(!store.exists(&session.id).await.unwrap());
    }

    #[tokio::test]
    async fn test_memory_store_exists() {
        let store = MemorySessionStore::new();

        // Unknown id
        assert!(!store.exists("unknown-id").await.unwrap());

        // Save and verify exists
        let session = create_test_session_data();
        store.save(&session).await.unwrap();
        assert!(store.exists(&session.id).await.unwrap());
    }

    #[tokio::test]
    async fn test_file_store_health_check() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();
        assert!(store.health_check().await.is_ok());
        assert_eq!(store.backend_name(), "file");
    }

    #[tokio::test]
    async fn test_file_store_health_check_bad_dir() {
        let store = FileSessionStore {
            dir: std::path::PathBuf::from("/nonexistent/path/that/does/not/exist"),
        };
        assert!(store.health_check().await.is_err());
    }

    #[tokio::test]
    async fn test_memory_store_health_check() {
        let store = MemorySessionStore::new();
        assert!(store.health_check().await.is_ok());
        assert_eq!(store.backend_name(), "memory");
    }

    // ========================================================================
    // Session Resume Boundary Tests
    // ========================================================================

    #[tokio::test]
    async fn test_file_store_load_empty_file() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        // Write an empty file — JSON parse must fail gracefully, not panic
        let empty_path = dir.path().join("empty-session.json");
        tokio::fs::write(&empty_path, b"").await.unwrap();

        let result = store.load("empty-session").await;
        assert!(
            result.is_err(),
            "Empty file must return error, not Ok(None)"
        );
    }

    #[tokio::test]
    async fn test_file_store_load_partial_json() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        // Truncated JSON — simulates a crash mid-write
        let partial_path = dir.path().join("partial-session.json");
        tokio::fs::write(&partial_path, b"{\"id\":\"partial-session\",\"message")
            .await
            .unwrap();

        let result = store.load("partial-session").await;
        assert!(result.is_err(), "Partial JSON must return error");
    }

    #[tokio::test]
    async fn test_file_store_concurrent_save() {
        let dir = tempdir().unwrap();
        let store = std::sync::Arc::new(FileSessionStore::new(dir.path()).await.unwrap());

        let session = create_test_session_data();
        let id = session.id.clone();

        // First save to create the file
        store.save(&session).await.unwrap();

        // Spawn multiple concurrent saves — last write wins, no corruption
        let mut handles = Vec::new();
        for _ in 0..5 {
            let s = store.clone();
            let sess = session.clone();
            handles.push(tokio::spawn(async move { s.save(&sess).await }));
        }
        for h in handles {
            h.await.unwrap().unwrap();
        }

        // File must be loadable after concurrent writes
        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id, id);
    }

    #[tokio::test]
    async fn test_file_store_load_nonexistent_returns_none() {
        let dir = tempdir().unwrap();
        let store = FileSessionStore::new(dir.path()).await.unwrap();

        let result = store.load("does-not-exist-at-all").await.unwrap();
        assert!(result.is_none(), "Missing session must return Ok(None)");
    }
}
