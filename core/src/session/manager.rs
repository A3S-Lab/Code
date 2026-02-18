//! SessionManager — manages multiple concurrent sessions

use super::{ContextUsage, Session, SessionConfig, SessionState};
use crate::agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
use crate::hitl::ConfirmationPolicy;
use crate::llm::{self, ContentBlock, LlmClient, LlmConfig, Message};
use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::planning::Task;
use crate::store::{FileSessionStore, LlmConfigData, SessionData, SessionStore};
use crate::tools::ToolExecutor;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Session manager handles multiple concurrent sessions
#[derive(Clone)]
pub struct SessionManager {
    pub(crate) sessions: Arc<RwLock<HashMap<String, Arc<RwLock<Session>>>>>,
    pub(crate) llm_client: Option<Arc<dyn LlmClient>>,
    pub(crate) tool_executor: Arc<ToolExecutor>,
    /// Session stores by storage type
    pub(crate) stores: Arc<RwLock<HashMap<crate::config::StorageBackend, Arc<dyn SessionStore>>>>,
    /// Track which storage type each session uses
    pub(crate) session_storage_types: Arc<RwLock<HashMap<String, crate::config::StorageBackend>>>,
    /// LLM configurations for sessions (stored separately for persistence)
    pub(crate) llm_configs: Arc<RwLock<HashMap<String, LlmConfigData>>>,
    /// Ongoing operations (session_id -> JoinHandle)
    pub(crate) ongoing_operations: Arc<RwLock<HashMap<String, tokio::task::AbortHandle>>>,
}

impl SessionManager {
    /// Create a new session manager without persistence
    pub fn new(llm_client: Option<Arc<dyn LlmClient>>, tool_executor: Arc<ToolExecutor>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            llm_client,
            tool_executor,
            stores: Arc::new(RwLock::new(HashMap::new())),
            session_storage_types: Arc::new(RwLock::new(HashMap::new())),
            llm_configs: Arc::new(RwLock::new(HashMap::new())),
            ongoing_operations: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a session manager with file-based persistence
    ///
    /// Sessions will be automatically saved to disk and restored on startup.
    pub async fn with_persistence<P: AsRef<std::path::Path>>(
        llm_client: Option<Arc<dyn LlmClient>>,
        tool_executor: Arc<ToolExecutor>,
        sessions_dir: P,
    ) -> Result<Self> {
        let store = FileSessionStore::new(sessions_dir).await?;
        let mut stores = HashMap::new();
        stores.insert(
            crate::config::StorageBackend::File,
            Arc::new(store) as Arc<dyn SessionStore>,
        );

        let manager = Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            llm_client,
            tool_executor,
            stores: Arc::new(RwLock::new(stores)),
            session_storage_types: Arc::new(RwLock::new(HashMap::new())),
            llm_configs: Arc::new(RwLock::new(HashMap::new())),
            ongoing_operations: Arc::new(RwLock::new(HashMap::new())),
        };

        Ok(manager)
    }

    /// Create a session manager with a custom store
    ///
    /// The `backend` parameter determines which `StorageBackend` key the store is registered under.
    /// Sessions created with a matching `storage_type` will use this store.
    pub fn with_store(
        llm_client: Option<Arc<dyn LlmClient>>,
        tool_executor: Arc<ToolExecutor>,
        store: Arc<dyn SessionStore>,
        backend: crate::config::StorageBackend,
    ) -> Self {
        let mut stores = HashMap::new();
        stores.insert(backend, store);

        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            llm_client,
            tool_executor,
            stores: Arc::new(RwLock::new(stores)),
            session_storage_types: Arc::new(RwLock::new(HashMap::new())),
            llm_configs: Arc::new(RwLock::new(HashMap::new())),
            ongoing_operations: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Restore a single session by ID from the store
    ///
    /// Searches all registered stores for the given session ID and restores it
    /// into the in-memory session map. Returns an error if not found.
    pub async fn restore_session_by_id(&self, session_id: &str) -> Result<()> {
        // Check if already loaded
        {
            let sessions = self.sessions.read().await;
            if sessions.contains_key(session_id) {
                return Ok(());
            }
        }

        let stores = self.stores.read().await;
        for (backend, store) in stores.iter() {
            match store.load(session_id).await {
                Ok(Some(data)) => {
                    {
                        let mut storage_types = self.session_storage_types.write().await;
                        storage_types.insert(data.id.clone(), backend.clone());
                    }
                    self.restore_session(data).await?;
                    return Ok(());
                }
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(
                        "Failed to load session {} from {:?}: {}",
                        session_id,
                        backend,
                        e
                    );
                    continue;
                }
            }
        }

        Err(anyhow::anyhow!(
            "Session {} not found in any store",
            session_id
        ))
    }

    /// Load all sessions from all registered stores
    pub async fn load_all_sessions(&mut self) -> Result<usize> {
        let stores = self.stores.read().await;
        let mut loaded = 0;

        for (backend, store) in stores.iter() {
            let session_ids = match store.list().await {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::warn!("Failed to list sessions from {:?} store: {}", backend, e);
                    continue;
                }
            };

            for id in session_ids {
                match store.load(&id).await {
                    Ok(Some(data)) => {
                        // Record the storage type for this session
                        {
                            let mut storage_types = self.session_storage_types.write().await;
                            storage_types.insert(data.id.clone(), backend.clone());
                        }

                        if let Err(e) = self.restore_session(data).await {
                            tracing::warn!("Failed to restore session {}: {}", id, e);
                        } else {
                            loaded += 1;
                        }
                    }
                    Ok(None) => {
                        tracing::warn!("Session {} not found in store", id);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load session {}: {}", id, e);
                    }
                }
            }
        }

        tracing::info!("Loaded {} sessions from store", loaded);
        Ok(loaded)
    }

    /// Restore a session from SessionData
    async fn restore_session(&self, data: SessionData) -> Result<()> {
        let tools = self.tool_executor.definitions();
        let mut session = Session::new(data.id.clone(), data.config.clone(), tools).await?;

        // Restore serializable state
        session.restore_from_data(&data);

        // Restore LLM config if present (without API key - must be reconfigured)
        if let Some(llm_config) = &data.llm_config {
            let mut configs = self.llm_configs.write().await;
            configs.insert(data.id.clone(), llm_config.clone());
        }

        let mut sessions = self.sessions.write().await;
        sessions.insert(data.id.clone(), Arc::new(RwLock::new(session)));

        tracing::info!("Restored session: {}", data.id);
        Ok(())
    }

    /// Save a session to the store
    async fn save_session(&self, session_id: &str) -> Result<()> {
        // Get the storage type for this session
        let storage_type = {
            let storage_types = self.session_storage_types.read().await;
            storage_types.get(session_id).cloned()
        };

        let Some(storage_type) = storage_type else {
            // No storage type means memory-only session
            return Ok(());
        };

        // Skip saving for memory storage
        if storage_type == crate::config::StorageBackend::Memory {
            return Ok(());
        }

        // Get the appropriate store
        let stores = self.stores.read().await;
        let Some(store) = stores.get(&storage_type) else {
            tracing::warn!("No store available for storage type: {:?}", storage_type);
            return Ok(());
        };

        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;

        // Get LLM config if set
        let llm_config = {
            let configs = self.llm_configs.read().await;
            configs.get(session_id).cloned()
        };

        let data = session.to_session_data(llm_config);
        store.save(&data).await?;

        tracing::debug!("Saved session: {}", session_id);
        Ok(())
    }

    /// Persist a session to store, logging and emitting an event on failure.
    /// This is a non-fatal wrapper around `save_session` — the operation
    /// succeeds in memory even if persistence fails.
    async fn persist_or_warn(&self, session_id: &str, operation: &str) {
        if let Err(e) = self.save_session(session_id).await {
            tracing::warn!(
                "Failed to persist session {} after {}: {}",
                session_id,
                operation,
                e
            );
            // Emit event so SDK clients can react
            if let Ok(session_lock) = self.get_session(session_id).await {
                let session = session_lock.read().await;
                let _ = session.event_tx().send(AgentEvent::PersistenceFailed {
                    session_id: session_id.to_string(),
                    operation: operation.to_string(),
                    error: e.to_string(),
                });
            }
        }
    }

    /// Spawn persistence as a background task (non-blocking).
    ///
    /// All `SessionManager` fields are `Arc`-wrapped, so `clone()` is a
    /// cheap pointer-copy. This keeps file I/O off the response path.
    fn persist_in_background(&self, session_id: &str, operation: &str) {
        let mgr = self.clone();
        let sid = session_id.to_string();
        let op = operation.to_string();
        tokio::spawn(async move {
            mgr.persist_or_warn(&sid, &op).await;
        });
    }

    /// Create a new session
    pub async fn create_session(&self, id: String, config: SessionConfig) -> Result<String> {
        tracing::info!(name: "a3s.session.create", session_id = %id, "Creating session");

        // Record the storage type for this session
        {
            let mut storage_types = self.session_storage_types.write().await;
            storage_types.insert(id.clone(), config.storage_type.clone());
        }

        // Get tool definitions from the executor
        let tools = self.tool_executor.definitions();
        let mut session = Session::new(id.clone(), config, tools).await?;

        // Start the command queue
        session.start_queue().await?;

        // Set max context length if provided
        if session.config.max_context_length > 0 {
            session.context_usage.max_tokens = session.config.max_context_length as usize;
        }

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(id.clone(), Arc::new(RwLock::new(session)));
        }

        // Persist to store
        self.persist_in_background(&id, "create");

        tracing::info!("Created session: {}", id);
        Ok(id)
    }

    /// Destroy a session
    pub async fn destroy_session(&self, id: &str) -> Result<()> {
        tracing::info!(name: "a3s.session.destroy", session_id = %id, "Destroying session");

        // Get the storage type before removing the session
        let storage_type = {
            let storage_types = self.session_storage_types.read().await;
            storage_types.get(id).cloned()
        };

        {
            let mut sessions = self.sessions.write().await;
            sessions.remove(id);
        }

        // Remove LLM config
        {
            let mut configs = self.llm_configs.write().await;
            configs.remove(id);
        }

        // Remove storage type tracking
        {
            let mut storage_types = self.session_storage_types.write().await;
            storage_types.remove(id);
        }

        // Delete from store if applicable
        if let Some(storage_type) = storage_type {
            if storage_type != crate::config::StorageBackend::Memory {
                let stores = self.stores.read().await;
                if let Some(store) = stores.get(&storage_type) {
                    if let Err(e) = store.delete(id).await {
                        tracing::warn!("Failed to delete session {} from store: {}", id, e);
                    }
                }
            }
        }

        tracing::info!("Destroyed session: {}", id);
        Ok(())
    }

    /// Get a session by ID
    pub async fn get_session(&self, id: &str) -> Result<Arc<RwLock<Session>>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(id)
            .cloned()
            .context(format!("Session not found: {}", id))
    }

    /// Create a child session for a subagent
    ///
    /// Child sessions inherit the parent's LLM client but have their own
    /// permission policy and configuration.
    pub async fn create_child_session(
        &self,
        parent_id: &str,
        child_id: String,
        mut config: SessionConfig,
    ) -> Result<String> {
        // Verify parent exists and inherit HITL policy
        let parent_lock = self.get_session(parent_id).await?;
        let parent_llm_client = {
            let parent = parent_lock.read().await;

            // Inherit parent's confirmation policy if child doesn't have one
            if config.confirmation_policy.is_none() {
                let parent_policy = parent.confirmation_manager.policy().await;
                config.confirmation_policy = Some(parent_policy);
            }

            parent.llm_client.clone()
        };

        // Set parent_id in config
        config.parent_id = Some(parent_id.to_string());

        // Get tool definitions from the executor
        let tools = self.tool_executor.definitions();
        let mut session = Session::new(child_id.clone(), config, tools).await?;

        // Inherit LLM client from parent if not set
        if session.llm_client.is_none() {
            session.llm_client = parent_llm_client.or_else(|| self.llm_client.clone());
        }

        // Start the command queue
        session.start_queue().await?;

        // Set max context length if provided
        if session.config.max_context_length > 0 {
            session.context_usage.max_tokens = session.config.max_context_length as usize;
        }

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(child_id.clone(), Arc::new(RwLock::new(session)));
        }

        // Persist to store
        self.persist_in_background(&child_id, "create_child");

        tracing::info!(
            "Created child session: {} (parent: {})",
            child_id,
            parent_id
        );
        Ok(child_id)
    }

    /// Get all child sessions for a parent session
    pub async fn get_child_sessions(&self, parent_id: &str) -> Vec<String> {
        let sessions = self.sessions.read().await;
        let mut children = Vec::new();

        for (id, session_lock) in sessions.iter() {
            let session = session_lock.read().await;
            if session.parent_id.as_deref() == Some(parent_id) {
                children.push(id.clone());
            }
        }

        children
    }

    /// Check if a session is a child session
    pub async fn is_child_session(&self, session_id: &str) -> Result<bool> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.is_child_session())
    }

    /// Generate response for a prompt
    pub async fn generate(&self, session_id: &str, prompt: &str) -> Result<AgentResult> {
        let session_lock = self.get_session(session_id).await?;

        // Check if session is paused
        {
            let session = session_lock.read().await;
            if session.state == SessionState::Paused {
                anyhow::bail!(
                    "Session {} is paused. Call Resume before generating.",
                    session_id
                );
            }
        }

        // Get session state and LLM client
        let (
            history,
            system,
            tools,
            session_llm_client,
            permission_policy,
            confirmation_manager,
            context_providers,
            session_workspace,
            tool_metrics,
            hook_engine,
            planning_enabled,
            goal_tracking,
        ) = {
            let session = session_lock.read().await;
            (
                session.messages.clone(),
                session.system().map(String::from),
                session.tools.clone(),
                session.llm_client.clone(),
                session.permission_policy.clone(),
                session.confirmation_manager.clone(),
                session.context_providers.clone(),
                session.config.workspace.clone(),
                session.tool_metrics.clone(),
                session.config.hook_engine.clone(),
                session.config.planning_enabled,
                session.config.goal_tracking,
            )
        };

        // Use session's LLM client if configured, otherwise use default
        let llm_client = if let Some(client) = session_llm_client {
            client
        } else if let Some(client) = &self.llm_client {
            client.clone()
        } else {
            anyhow::bail!(
                "LLM client not configured for session {}. Please call Configure RPC with model configuration first.",
                session_id
            );
        };

        // Construct per-session ToolContext from session workspace, falling back to server default
        let tool_context = if session_workspace.is_empty() {
            crate::tools::ToolContext::new(self.tool_executor.workspace().clone())
                .with_session_id(session_id)
        } else {
            crate::tools::ToolContext::new(std::path::PathBuf::from(&session_workspace))
                .with_session_id(session_id)
        };

        // Create agent loop with permission policy, confirmation manager, and context providers
        let config = AgentConfig {
            system_prompt: system,
            tools,
            max_tool_rounds: 50,
            permission_policy: Some(permission_policy),
            confirmation_manager: Some(confirmation_manager),
            context_providers,
            planning_enabled,
            goal_tracking,
            hook_engine,
        };

        let agent = AgentLoop::new(llm_client, self.tool_executor.clone(), tool_context, config)
            .with_tool_metrics(tool_metrics);

        // Execute with session context
        let result = agent
            .execute_with_session(&history, prompt, Some(session_id), None)
            .await?;

        // Update session
        {
            let mut session = session_lock.write().await;
            session.messages = result.messages.clone();
            session.update_usage(&result.usage);
        }

        // Persist to store
        self.persist_in_background(session_id, "generate");

        // Auto-compact if context usage exceeds threshold
        if let Err(e) = self.maybe_auto_compact(session_id).await {
            tracing::warn!("Auto-compact failed for session {}: {}", session_id, e);
        }

        Ok(result)
    }

    /// Generate response with streaming events
    pub async fn generate_streaming(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<(
        mpsc::Receiver<AgentEvent>,
        tokio::task::JoinHandle<Result<AgentResult>>,
    )> {
        let session_lock = self.get_session(session_id).await?;

        // Check if session is paused
        {
            let session = session_lock.read().await;
            if session.state == SessionState::Paused {
                anyhow::bail!(
                    "Session {} is paused. Call Resume before generating.",
                    session_id
                );
            }
        }

        // Get session state and LLM client
        let (
            history,
            system,
            tools,
            session_llm_client,
            permission_policy,
            confirmation_manager,
            context_providers,
            session_workspace,
            tool_metrics,
            hook_engine,
            planning_enabled,
            goal_tracking,
        ) = {
            let session = session_lock.read().await;
            (
                session.messages.clone(),
                session.system().map(String::from),
                session.tools.clone(),
                session.llm_client.clone(),
                session.permission_policy.clone(),
                session.confirmation_manager.clone(),
                session.context_providers.clone(),
                session.config.workspace.clone(),
                session.tool_metrics.clone(),
                session.config.hook_engine.clone(),
                session.config.planning_enabled,
                session.config.goal_tracking,
            )
        };

        // Use session's LLM client if configured, otherwise use default
        let llm_client = if let Some(client) = session_llm_client {
            client
        } else if let Some(client) = &self.llm_client {
            client.clone()
        } else {
            anyhow::bail!(
                "LLM client not configured for session {}. Please call Configure RPC with model configuration first.",
                session_id
            );
        };

        // Construct per-session ToolContext from session workspace, falling back to server default
        let tool_context = if session_workspace.is_empty() {
            crate::tools::ToolContext::new(self.tool_executor.workspace().clone())
                .with_session_id(session_id)
        } else {
            crate::tools::ToolContext::new(std::path::PathBuf::from(&session_workspace))
                .with_session_id(session_id)
        };

        // Create agent loop with permission policy, confirmation manager, and context providers
        let config = AgentConfig {
            system_prompt: system,
            tools,
            max_tool_rounds: 50,
            permission_policy: Some(permission_policy),
            confirmation_manager: Some(confirmation_manager),
            context_providers,
            planning_enabled,
            goal_tracking,
            hook_engine,
        };

        let agent = AgentLoop::new(llm_client, self.tool_executor.clone(), tool_context, config)
            .with_tool_metrics(tool_metrics);

        // Execute with streaming
        let (rx, handle) = agent.execute_streaming(&history, prompt).await?;

        // Store the abort handle for cancellation support
        let abort_handle = handle.abort_handle();
        {
            let mut ops = self.ongoing_operations.write().await;
            ops.insert(session_id.to_string(), abort_handle);
        }

        // Spawn task to update session after completion
        let session_lock_clone = session_lock.clone();
        let original_handle = handle;
        let stores = self.stores.clone();
        let session_storage_types = self.session_storage_types.clone();
        let llm_configs = self.llm_configs.clone();
        let session_id_owned = session_id.to_string();
        let ongoing_operations = self.ongoing_operations.clone();
        let session_manager = self.clone();

        let wrapped_handle = tokio::spawn(async move {
            let result = original_handle.await??;

            // Remove from ongoing operations
            {
                let mut ops = ongoing_operations.write().await;
                ops.remove(&session_id_owned);
            }

            // Update session
            {
                let mut session = session_lock_clone.write().await;
                session.messages = result.messages.clone();
                session.update_usage(&result.usage);
            }

            // Persist to store
            let storage_type = {
                let storage_types = session_storage_types.read().await;
                storage_types.get(&session_id_owned).cloned()
            };

            if let Some(storage_type) = storage_type {
                if storage_type != crate::config::StorageBackend::Memory {
                    let stores_guard = stores.read().await;
                    if let Some(store) = stores_guard.get(&storage_type) {
                        let session = session_lock_clone.read().await;
                        let llm_config = {
                            let configs = llm_configs.read().await;
                            configs.get(&session_id_owned).cloned()
                        };
                        let data = session.to_session_data(llm_config);
                        if let Err(e) = store.save(&data).await {
                            tracing::warn!(
                                "Failed to persist session {} after streaming: {}",
                                session_id_owned,
                                e
                            );
                        }
                    }
                }
            }

            // Auto-compact if context usage exceeds threshold
            if let Err(e) = session_manager.maybe_auto_compact(&session_id_owned).await {
                tracing::warn!(
                    "Auto-compact failed for session {}: {}",
                    session_id_owned,
                    e
                );
            }

            Ok(result)
        });

        Ok((rx, wrapped_handle))
    }

    /// Get context usage for a session
    pub async fn context_usage(&self, session_id: &str) -> Result<ContextUsage> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.context_usage.clone())
    }

    /// Get conversation history for a session
    pub async fn history(&self, session_id: &str) -> Result<Vec<Message>> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.messages.clone())
    }

    /// Clear session history
    pub async fn clear(&self, session_id: &str) -> Result<()> {
        {
            let session_lock = self.get_session(session_id).await?;
            let mut session = session_lock.write().await;
            session.clear();
        }

        // Persist to store
        self.persist_in_background(session_id, "clear");

        Ok(())
    }

    /// Compact session context
    pub async fn compact(&self, session_id: &str) -> Result<()> {
        tracing::info!(name: "a3s.session.compact", session_id = %session_id, "Compacting session context");

        {
            let session_lock = self.get_session(session_id).await?;
            let mut session = session_lock.write().await;

            // Get LLM client for compaction (if available)
            let llm_client = if let Some(client) = &session.llm_client {
                client.clone()
            } else if let Some(client) = &self.llm_client {
                client.clone()
            } else {
                // If no LLM client available, just do simple truncation
                tracing::warn!("No LLM client configured for compaction, using simple truncation");
                let keep_messages = 20;
                if session.messages.len() > keep_messages {
                    let len = session.messages.len();
                    session.messages = session.messages.split_off(len - keep_messages);
                }
                // Persist after truncation
                drop(session);
                self.persist_in_background(session_id, "compact");
                return Ok(());
            };

            session.compact(&llm_client).await?;
        }

        // Persist to store
        self.persist_in_background(session_id, "compact");

        Ok(())
    }

    /// Check if auto-compaction should be triggered and perform it if needed.
    ///
    /// Called after `generate()` / `generate_streaming()` updates session usage.
    /// Triggers compaction when:
    /// - `auto_compact` is enabled in session config
    /// - `context_usage.percent` exceeds `auto_compact_threshold`
    pub async fn maybe_auto_compact(&self, session_id: &str) -> Result<bool> {
        let (should_compact, percent_before, messages_before) = {
            let session_lock = self.get_session(session_id).await?;
            let session = session_lock.read().await;

            if !session.config.auto_compact {
                return Ok(false);
            }

            let threshold = session.config.auto_compact_threshold;
            let percent = session.context_usage.percent;
            let msg_count = session.messages.len();

            tracing::debug!(
                "Auto-compact check for session {}: percent={:.2}%, threshold={:.2}%, messages={}",
                session_id,
                percent * 100.0,
                threshold * 100.0,
                msg_count,
            );

            (percent >= threshold, percent, msg_count)
        };

        if !should_compact {
            return Ok(false);
        }

        tracing::info!(
            name: "a3s.session.auto_compact",
            session_id = %session_id,
            percent_before = %format!("{:.1}%", percent_before * 100.0),
            messages_before = %messages_before,
            "Auto-compacting session due to high context usage"
        );

        // Perform compaction (reuses existing compact logic)
        self.compact(session_id).await?;

        // Get post-compaction message count
        let messages_after = {
            let session_lock = self.get_session(session_id).await?;
            let session = session_lock.read().await;
            session.messages.len()
        };

        // Broadcast event to notify clients
        let event = AgentEvent::ContextCompacted {
            session_id: session_id.to_string(),
            before_messages: messages_before,
            after_messages: messages_after,
            percent_before,
        };

        // Try to send via session's event broadcaster
        if let Ok(session_lock) = self.get_session(session_id).await {
            let session = session_lock.read().await;
            let _ = session.event_tx.send(event);
        }

        tracing::info!(
            name: "a3s.session.auto_compact.done",
            session_id = %session_id,
            messages_before = %messages_before,
            messages_after = %messages_after,
            "Auto-compaction complete"
        );

        Ok(true)
    }

    /// Resolve the LLM client for a session (session-level -> default fallback)
    ///
    /// Returns `None` if no LLM client is configured at either level.
    pub async fn get_llm_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<Arc<dyn LlmClient>>> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;

        if let Some(client) = &session.llm_client {
            return Ok(Some(client.clone()));
        }

        Ok(self.llm_client.clone())
    }

    /// Configure session
    pub async fn configure(
        &self,
        session_id: &str,
        thinking: Option<bool>,
        budget: Option<usize>,
        model_config: Option<LlmConfig>,
    ) -> Result<()> {
        {
            let session_lock = self.get_session(session_id).await?;
            let mut session = session_lock.write().await;

            if let Some(t) = thinking {
                session.thinking_enabled = t;
            }
            if let Some(b) = budget {
                session.thinking_budget = Some(b);
            }
            if let Some(ref config) = model_config {
                tracing::info!(
                    "Configuring session {} with LLM: provider={}, model={}",
                    session_id,
                    config.provider,
                    config.model
                );
                session.model_name = Some(config.model.clone());
                session.llm_client = Some(llm::create_client_with_config(config.clone()));
            }
        }

        // Store LLM config for persistence (without API key)
        if let Some(config) = model_config {
            let llm_config_data = LlmConfigData {
                provider: config.provider,
                model: config.model,
                api_key: None, // Don't persist API key
                base_url: config.base_url,
            };
            let mut configs = self.llm_configs.write().await;
            configs.insert(session_id.to_string(), llm_config_data);
        }

        // Persist to store
        self.persist_in_background(session_id, "configure");

        Ok(())
    }

    /// Get session count
    pub async fn session_count(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }

    /// Check health of all registered stores
    pub async fn store_health(&self) -> Vec<(String, Result<()>)> {
        let stores = self.stores.read().await;
        let mut results = Vec::new();
        for (_, store) in stores.iter() {
            let name = store.backend_name().to_string();
            let result = store.health_check().await;
            results.push((name, result));
        }
        results
    }

    /// List all loaded tools (built-in tools)
    pub fn list_tools(&self) -> Vec<crate::llm::ToolDefinition> {
        self.tool_executor.definitions()
    }

    /// Pause a session
    pub async fn pause_session(&self, session_id: &str) -> Result<bool> {
        let paused = {
            let session_lock = self.get_session(session_id).await?;
            let mut session = session_lock.write().await;
            session.pause()
        };

        if paused {
            self.persist_in_background(session_id, "pause");
        }

        Ok(paused)
    }

    /// Resume a session
    pub async fn resume_session(&self, session_id: &str) -> Result<bool> {
        let resumed = {
            let session_lock = self.get_session(session_id).await?;
            let mut session = session_lock.write().await;
            session.resume()
        };

        if resumed {
            self.persist_in_background(session_id, "resume");
        }

        Ok(resumed)
    }

    /// Cancel an ongoing operation for a session
    ///
    /// Returns true if an operation was cancelled, false if no operation was running.
    pub async fn cancel_operation(&self, session_id: &str) -> Result<bool> {
        // First, cancel any pending HITL confirmations
        let session_lock = self.get_session(session_id).await?;
        let cancelled_confirmations = {
            let session = session_lock.read().await;
            session.confirmation_manager.cancel_all().await
        };

        if cancelled_confirmations > 0 {
            tracing::info!(
                "Cancelled {} pending confirmations for session {}",
                cancelled_confirmations,
                session_id
            );
        }

        // Then, abort the ongoing operation if any
        let abort_handle = {
            let mut ops = self.ongoing_operations.write().await;
            ops.remove(session_id)
        };

        if let Some(handle) = abort_handle {
            handle.abort();
            tracing::info!("Cancelled ongoing operation for session {}", session_id);
            Ok(true)
        } else if cancelled_confirmations > 0 {
            // We cancelled confirmations but no main operation
            Ok(true)
        } else {
            tracing::debug!("No ongoing operation to cancel for session {}", session_id);
            Ok(false)
        }
    }

    /// Get all sessions (returns session locks for iteration)
    pub async fn get_all_sessions(&self) -> Vec<Arc<RwLock<Session>>> {
        let sessions = self.sessions.read().await;
        sessions.values().cloned().collect()
    }

    /// Get tool executor reference
    pub fn tool_executor(&self) -> &Arc<ToolExecutor> {
        &self.tool_executor
    }

    /// Confirm a tool execution (HITL)
    pub async fn confirm_tool(
        &self,
        session_id: &str,
        tool_id: &str,
        approved: bool,
        reason: Option<String>,
    ) -> Result<bool> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        session
            .confirmation_manager
            .confirm(tool_id, approved, reason)
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    /// Set confirmation policy for a session (HITL)
    pub async fn set_confirmation_policy(
        &self,
        session_id: &str,
        policy: ConfirmationPolicy,
    ) -> Result<ConfirmationPolicy> {
        {
            let session_lock = self.get_session(session_id).await?;
            let session = session_lock.read().await;
            session.set_confirmation_policy(policy.clone()).await;
        }

        // Update config for persistence
        {
            let session_lock = self.get_session(session_id).await?;
            let mut session = session_lock.write().await;
            session.config.confirmation_policy = Some(policy.clone());
        }

        // Persist to store
        self.persist_in_background(session_id, "set_confirmation_policy");

        Ok(policy)
    }

    /// Get confirmation policy for a session (HITL)
    pub async fn get_confirmation_policy(&self, session_id: &str) -> Result<ConfirmationPolicy> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.confirmation_policy().await)
    }

    /// Set permission policy for a session
    pub async fn set_permission_policy(
        &self,
        session_id: &str,
        policy: PermissionPolicy,
    ) -> Result<PermissionPolicy> {
        {
            let session_lock = self.get_session(session_id).await?;
            let session = session_lock.read().await;
            session.set_permission_policy(policy.clone()).await;
        }

        // Update config for persistence
        {
            let session_lock = self.get_session(session_id).await?;
            let mut session = session_lock.write().await;
            session.config.permission_policy = Some(policy.clone());
        }

        // Persist to store
        self.persist_in_background(session_id, "set_permission_policy");

        Ok(policy)
    }

    /// Get permission policy for a session
    pub async fn get_permission_policy(&self, session_id: &str) -> Result<PermissionPolicy> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.permission_policy().await)
    }

    /// Check permission for a tool invocation
    pub async fn check_permission(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Result<PermissionDecision> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.check_permission(tool_name, args).await)
    }

    /// Add a permission rule
    pub async fn add_permission_rule(
        &self,
        session_id: &str,
        rule_type: &str,
        rule: &str,
    ) -> Result<()> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        match rule_type {
            "allow" => session.add_allow_rule(rule).await,
            "deny" => session.add_deny_rule(rule).await,
            "ask" => session.add_ask_rule(rule).await,
            _ => anyhow::bail!("Unknown rule type: {}", rule_type),
        }
        Ok(())
    }

    /// Add a context provider to a session
    pub async fn add_context_provider(
        &self,
        session_id: &str,
        provider: Arc<dyn crate::context::ContextProvider>,
    ) -> Result<()> {
        let session_lock = self.get_session(session_id).await?;
        let mut session = session_lock.write().await;
        session.add_context_provider(provider);
        Ok(())
    }

    /// Remove a context provider from a session by name
    pub async fn remove_context_provider(&self, session_id: &str, name: &str) -> Result<bool> {
        let session_lock = self.get_session(session_id).await?;
        let mut session = session_lock.write().await;
        Ok(session.remove_context_provider(name))
    }

    /// List context provider names for a session
    pub async fn list_context_providers(&self, session_id: &str) -> Result<Vec<String>> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.context_provider_names())
    }

    /// Set lane handler configuration
    pub async fn set_lane_handler(
        &self,
        session_id: &str,
        lane: crate::hitl::SessionLane,
        config: crate::queue::LaneHandlerConfig,
    ) -> Result<()> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        session.set_lane_handler(lane, config).await;
        Ok(())
    }

    /// Get lane handler configuration
    pub async fn get_lane_handler(
        &self,
        session_id: &str,
        lane: crate::hitl::SessionLane,
    ) -> Result<crate::queue::LaneHandlerConfig> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.get_lane_handler(lane).await)
    }

    /// Complete an external task
    pub async fn complete_external_task(
        &self,
        session_id: &str,
        task_id: &str,
        result: crate::queue::ExternalTaskResult,
    ) -> Result<bool> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.complete_external_task(task_id, result).await)
    }

    /// Get pending external tasks for a session
    pub async fn pending_external_tasks(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::queue::ExternalTask>> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.pending_external_tasks().await)
    }

    // ========================================================================
    // Task Management
    // ========================================================================

    /// Get tasks for a session
    pub async fn get_tasks(&self, session_id: &str) -> Result<Vec<Task>> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.get_tasks().to_vec())
    }

    /// Set tasks for a session
    pub async fn set_tasks(&self, session_id: &str, tasks: Vec<Task>) -> Result<Vec<Task>> {
        {
            let session_lock = self.get_session(session_id).await?;
            let mut session = session_lock.write().await;
            session.set_tasks(tasks);
        }

        // Save session after updating tasks
        self.persist_in_background(session_id, "task_update");

        // Return updated tasks
        self.get_tasks(session_id).await
    }

    /// Fork a session, creating a new session with copied history and configuration
    ///
    /// The forked session gets:
    /// - A new unique ID
    /// - Copied conversation history (messages)
    /// - Copied configuration (with optional name override)
    /// - Copied usage statistics and cost
    /// - Copied tasks
    /// - `parent_id` set to the source session ID
    /// - Fresh timestamps (created_at = now)
    ///
    /// Non-serializable state (queue, HITL, permissions) is freshly initialized.
    pub async fn fork_session(
        &self,
        source_id: &str,
        new_id: String,
        new_name: Option<String>,
    ) -> Result<String> {
        tracing::info!(
            name: "a3s.session.fork",
            source_id = %source_id,
            new_id = %new_id,
            "Forking session"
        );

        // Read source session data
        let (
            source_config,
            source_messages,
            source_usage,
            source_cost,
            source_model_name,
            source_thinking_enabled,
            source_thinking_budget,
            source_tasks,
            source_context_usage,
        ) = {
            let session_lock = self
                .get_session(source_id)
                .await
                .context(format!("Source session '{}' not found for fork", source_id))?;
            let session = session_lock.read().await;
            (
                session.config.clone(),
                session.messages.clone(),
                session.total_usage.clone(),
                session.total_cost,
                session.model_name.clone(),
                session.thinking_enabled,
                session.thinking_budget,
                session.tasks.clone(),
                session.context_usage.clone(),
            )
        };

        // Copy LLM config if source has one
        let source_llm_config = {
            let configs = self.llm_configs.read().await;
            configs.get(source_id).cloned()
        };

        // Build forked config
        let mut forked_config = source_config;
        if let Some(name) = new_name {
            forked_config.name = name;
        } else {
            forked_config.name = format!("{} (fork)", forked_config.name);
        }
        forked_config.parent_id = Some(source_id.to_string());

        // Create the new session
        let tools = self.tool_executor.definitions();
        let mut new_session = Session::new(new_id.clone(), forked_config, tools).await?;

        // Copy state from source
        new_session.messages = source_messages;
        new_session.total_usage = source_usage;
        new_session.total_cost = source_cost;
        new_session.model_name = source_model_name;
        new_session.thinking_enabled = source_thinking_enabled;
        new_session.thinking_budget = source_thinking_budget;
        new_session.tasks = source_tasks;
        new_session.context_usage = source_context_usage;

        // Start the command queue
        new_session.start_queue().await?;

        // Record storage type
        {
            let mut storage_types = self.session_storage_types.write().await;
            storage_types.insert(new_id.clone(), new_session.config.storage_type.clone());
        }

        // Copy LLM config if source had one
        if let Some(llm_config) = source_llm_config {
            let mut configs = self.llm_configs.write().await;
            configs.insert(new_id.clone(), llm_config);
        }

        // Insert the new session
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(new_id.clone(), Arc::new(RwLock::new(new_session)));
        }

        // Persist to store
        self.persist_in_background(&new_id, "fork");

        tracing::info!(
            "Forked session '{}' -> '{}' with parent_id set",
            source_id,
            new_id,
        );

        Ok(new_id)
    }

    /// Generate a short title for a session based on its conversation content
    ///
    /// Uses the session's LLM client (or default) to generate a concise title
    /// from the first few messages. The title is automatically set on the session.
    ///
    /// Returns the generated title, or None if no LLM client is available
    /// or the session has no messages.
    pub async fn generate_title(&self, session_id: &str) -> Result<Option<String>> {
        tracing::info!(
            name: "a3s.session.generate_title",
            session_id = %session_id,
            "Generating session title"
        );

        // Get the first few messages for context
        let messages = {
            let session_lock = self.get_session(session_id).await?;
            let session = session_lock.read().await;

            if session.messages.is_empty() {
                return Ok(None);
            }

            // Take up to the first 4 messages for title generation
            session.messages.iter().take(4).cloned().collect::<Vec<_>>()
        };

        // Get LLM client
        let llm_client = self.get_llm_for_session(session_id).await?;
        let Some(client) = llm_client else {
            tracing::debug!("No LLM client available for title generation");
            return Ok(None);
        };

        // Build a summary of the conversation for the title prompt
        let mut conversation_summary = String::new();
        for msg in &messages {
            let role = &msg.role;
            for block in &msg.content {
                if let ContentBlock::Text { text } = block {
                    // Limit each message to 200 chars for the title prompt
                    let truncated = if text.len() > 200 {
                        format!("{}...", &text[..200])
                    } else {
                        text.clone()
                    };
                    conversation_summary.push_str(&format!("{}: {}\n", role, truncated));
                }
            }
        }

        if conversation_summary.is_empty() {
            return Ok(None);
        }

        // Ask LLM to generate a title
        let title_prompt = Message::user(&crate::prompts::render(
            crate::prompts::TITLE_GENERATE,
            &[("conversation", &conversation_summary)],
        ));

        let response = client
            .complete(&[title_prompt], None, &[])
            .await
            .context("Failed to generate session title")?;

        // Extract title from response
        let title = response
            .message
            .content
            .iter()
            .find_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        if title.is_empty() {
            return Ok(None);
        }

        // Truncate if too long (safety net)
        let title = if title.len() > 80 {
            format!("{}...", &title[..77])
        } else {
            title
        };

        // Update session name
        {
            let session_lock = self.get_session(session_id).await?;
            let mut session = session_lock.write().await;
            session.config.name = title.clone();
            session.touch();
        }

        // Persist
        self.persist_in_background(session_id, "title_generation");

        tracing::info!("Generated title for session '{}': '{}'", session_id, title);
        Ok(Some(title))
    }
}
