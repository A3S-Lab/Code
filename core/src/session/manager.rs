//! SessionManager — manages multiple concurrent sessions

use super::{ContextUsage, Session, SessionConfig, SessionState};
use crate::agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
use crate::hitl::ConfirmationPolicy;
use crate::llm::{self, LlmClient, LlmConfig, Message};
use crate::mcp::McpManager;
use crate::memory::AgentMemory;
use crate::prompts::SystemPromptSlots;
use crate::skills::SkillRegistry;
use crate::store::{FileSessionStore, LlmConfigData, SessionData, SessionStore};
use crate::tools::ToolExecutor;
use a3s_memory::MemoryStore;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Session manager handles multiple concurrent sessions
#[derive(Clone)]
pub struct SessionManager {
    pub(crate) sessions: Arc<RwLock<HashMap<String, Arc<RwLock<Session>>>>>,
    /// Default LLM client used when a session has no per-session client configured.
    /// Wrapped in RwLock so it can be hot-swapped at runtime without restart.
    pub(crate) llm_client: Arc<RwLock<Option<Arc<dyn LlmClient>>>>,
    pub(crate) tool_executor: Arc<ToolExecutor>,
    /// Session stores by storage type
    pub(crate) stores: Arc<RwLock<HashMap<crate::config::StorageBackend, Arc<dyn SessionStore>>>>,
    /// Track which storage type each session uses
    pub(crate) session_storage_types: Arc<RwLock<HashMap<String, crate::config::StorageBackend>>>,
    /// LLM configurations for sessions (stored separately for persistence)
    pub(crate) llm_configs: Arc<RwLock<HashMap<String, LlmConfigData>>>,
    /// Ongoing operations (session_id -> CancellationToken)
    pub(crate) ongoing_operations:
        Arc<RwLock<HashMap<String, tokio_util::sync::CancellationToken>>>,
    /// Skill registry for runtime skill management
    pub(crate) skill_registry: Arc<RwLock<Option<Arc<SkillRegistry>>>>,
    /// Shared memory store for agent long-term memory.
    /// When set, each `generate`/`generate_streaming` call wraps this in
    /// `AgentMemory` and injects it into `AgentConfig.memory`.
    pub(crate) memory_store: Arc<RwLock<Option<Arc<dyn MemoryStore>>>>,
    /// Shared MCP manager. When set, MCP tools are registered into the
    /// `ToolExecutor` at startup so all sessions can use them.
    pub(crate) mcp_manager: Arc<RwLock<Option<Arc<McpManager>>>>,
}

impl SessionManager {
    /// Create a new session manager without persistence
    pub fn new(llm_client: Option<Arc<dyn LlmClient>>, tool_executor: Arc<ToolExecutor>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            llm_client: Arc::new(RwLock::new(llm_client)),
            tool_executor,
            stores: Arc::new(RwLock::new(HashMap::new())),
            session_storage_types: Arc::new(RwLock::new(HashMap::new())),
            llm_configs: Arc::new(RwLock::new(HashMap::new())),
            ongoing_operations: Arc::new(RwLock::new(HashMap::new())),
            skill_registry: Arc::new(RwLock::new(None)),
            memory_store: Arc::new(RwLock::new(None)),
            mcp_manager: Arc::new(RwLock::new(None)),
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
            llm_client: Arc::new(RwLock::new(llm_client)),
            tool_executor,
            stores: Arc::new(RwLock::new(stores)),
            session_storage_types: Arc::new(RwLock::new(HashMap::new())),
            llm_configs: Arc::new(RwLock::new(HashMap::new())),
            ongoing_operations: Arc::new(RwLock::new(HashMap::new())),
            skill_registry: Arc::new(RwLock::new(None)),
            memory_store: Arc::new(RwLock::new(None)),
            mcp_manager: Arc::new(RwLock::new(None)),
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
            llm_client: Arc::new(RwLock::new(llm_client)),
            tool_executor,
            stores: Arc::new(RwLock::new(stores)),
            session_storage_types: Arc::new(RwLock::new(HashMap::new())),
            llm_configs: Arc::new(RwLock::new(HashMap::new())),
            ongoing_operations: Arc::new(RwLock::new(HashMap::new())),
            skill_registry: Arc::new(RwLock::new(None)),
            memory_store: Arc::new(RwLock::new(None)),
            mcp_manager: Arc::new(RwLock::new(None)),
        }
    }

    /// Hot-swap the default LLM client used by sessions without a per-session client.
    ///
    /// Called by SafeClaw when `PUT /api/agent/config` updates the model config.
    /// All subsequent `generate` / `generate_streaming` calls will use the new client.
    pub async fn set_default_llm(&self, client: Option<Arc<dyn LlmClient>>) {
        *self.llm_client.write().await = client;
    }

    /// Set the skill registry for runtime skill management.
    ///
    /// Skills registered here will be injected into the system prompt
    /// for all subsequent `generate` / `generate_streaming` calls.
    /// Also registers the `manage_skill` tool on the `ToolExecutor` so
    /// the Agent can create, list, and remove skills at runtime.
    pub async fn set_skill_registry(
        &self,
        registry: Arc<SkillRegistry>,
        skills_dir: std::path::PathBuf,
    ) {
        // Register the manage_skill tool for self-bootstrap
        let manage_tool = crate::skills::ManageSkillTool::new(registry.clone(), skills_dir);
        self.tool_executor
            .register_dynamic_tool(Arc::new(manage_tool));

        *self.skill_registry.write().await = Some(registry);
    }

    /// Get the current skill registry, if any.
    pub async fn skill_registry(&self) -> Option<Arc<SkillRegistry>> {
        self.skill_registry.read().await.clone()
    }

    /// Set the shared memory store for agent long-term memory.
    ///
    /// When set, every `generate`/`generate_streaming` call wraps this store
    /// in an `AgentMemory` and injects it into `AgentConfig.memory`, enabling
    /// automatic `recall_similar` before prompts and `remember_success`/
    /// `remember_failure` after tool execution.
    pub async fn set_memory_store(&self, store: Arc<dyn MemoryStore>) {
        *self.memory_store.write().await = Some(store);
    }

    /// Get the current memory store, if any.
    pub async fn memory_store(&self) -> Option<Arc<dyn MemoryStore>> {
        self.memory_store.read().await.clone()
    }

    /// Set the shared MCP manager.
    ///
    /// When set, all tools from connected MCP servers are registered into the
    /// `ToolExecutor` so they are available in every session.
    pub async fn set_mcp_manager(&self, manager: Arc<McpManager>) {
        let all_tools = manager.get_all_tools().await;
        let mut by_server: HashMap<String, Vec<crate::mcp::McpTool>> = HashMap::new();
        for (server, tool) in all_tools {
            by_server.entry(server).or_default().push(tool);
        }
        for (server_name, tools) in by_server {
            for tool in
                crate::mcp::tools::create_mcp_tools(&server_name, tools, Arc::clone(&manager))
            {
                self.tool_executor.register_dynamic_tool(tool);
            }
        }
        *self.mcp_manager.write().await = Some(manager);
    }

    /// Dynamically add and connect an MCP server to the shared manager.
    ///
    /// Registers the server config, connects it, and registers its tools into
    /// the `ToolExecutor` so all subsequent sessions can use them.
    pub async fn add_mcp_server(&self, config: crate::mcp::McpServerConfig) -> Result<()> {
        let manager = {
            let guard = self.mcp_manager.read().await;
            match guard.clone() {
                Some(m) => m,
                None => {
                    drop(guard);
                    let m = Arc::new(McpManager::new());
                    *self.mcp_manager.write().await = Some(Arc::clone(&m));
                    m
                }
            }
        };
        let name = config.name.clone();
        manager.register_server(config).await;
        manager.connect(&name).await?;
        let tools = manager.get_server_tools(&name).await;
        for tool in crate::mcp::tools::create_mcp_tools(&name, tools, Arc::clone(&manager)) {
            self.tool_executor.register_dynamic_tool(tool);
        }
        Ok(())
    }

    /// Disconnect and remove an MCP server from the shared manager.
    ///
    /// Also unregisters its tools from the `ToolExecutor`.
    pub async fn remove_mcp_server(&self, name: &str) -> Result<()> {
        let guard = self.mcp_manager.read().await;
        if let Some(ref manager) = *guard {
            manager.disconnect(name).await?;
        }
        self.tool_executor
            .unregister_tools_by_prefix(&format!("mcp__{name}__"));
        Ok(())
    }

    /// Get status of all connected MCP servers.
    pub async fn mcp_status(
        &self,
    ) -> std::collections::HashMap<String, crate::mcp::McpServerStatus> {
        let guard = self.mcp_manager.read().await;
        match guard.as_ref() {
            Some(m) => {
                let m = Arc::clone(m);
                drop(guard);
                m.get_status().await
            }
            None => std::collections::HashMap::new(),
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
            let default_llm = self.llm_client.read().await.clone();
            session.llm_client = parent_llm_client.or(default_llm);
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
            permission_checker,
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
                session.permission_checker.clone(),
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
        } else if let Some(client) = self.llm_client.read().await.clone() {
            client
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

        // Inject skill registry into system prompt and agent config
        let skill_registry = self.skill_registry.read().await.clone();
        let system = if let Some(ref registry) = skill_registry {
            let skill_prompt = registry.to_system_prompt();
            if skill_prompt.is_empty() {
                system
            } else {
                match system {
                    Some(existing) => Some(format!("{}\n\n{}", existing, skill_prompt)),
                    None => Some(skill_prompt),
                }
            }
        } else {
            system
        };

        // Inject matched skill content on-demand before skill_registry is moved into AgentConfig
        let effective_prompt = if let Some(ref registry) = skill_registry {
            let skill_content = registry.match_skills(prompt);
            if skill_content.is_empty() {
                prompt.to_string()
            } else {
                format!("{}\n\n---\n\n{}", skill_content, prompt)
            }
        } else {
            prompt.to_string()
        };

        // Build AgentMemory from shared store (if configured)
        let memory = self
            .memory_store
            .read()
            .await
            .as_ref()
            .map(|store| Arc::new(AgentMemory::new(store.clone())));

        // Create agent loop with permission policy, confirmation manager, and context providers
        let config = AgentConfig {
            prompt_slots: match system {
                Some(s) => SystemPromptSlots::from_legacy(s),
                None => SystemPromptSlots::default(),
            },
            tools,
            max_tool_rounds: 50,
            permission_checker: Some(permission_checker),
            confirmation_manager: Some(confirmation_manager),
            context_providers,
            planning_enabled,
            goal_tracking,
            hook_engine,
            skill_registry,
            memory,
            ..AgentConfig::default()
        };

        let agent = AgentLoop::new(llm_client, self.tool_executor.clone(), tool_context, config)
            .with_tool_metrics(tool_metrics);

        // Execute with session context
        let result = agent
            .execute_with_session(&history, &effective_prompt, Some(session_id), None, None)
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
        tokio_util::sync::CancellationToken,
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
            permission_checker,
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
                session.permission_checker.clone(),
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
        } else if let Some(client) = self.llm_client.read().await.clone() {
            client
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

        // Inject skill registry into system prompt and agent config
        let skill_registry = self.skill_registry.read().await.clone();
        let system = if let Some(ref registry) = skill_registry {
            let skill_prompt = registry.to_system_prompt();
            if skill_prompt.is_empty() {
                system
            } else {
                match system {
                    Some(existing) => Some(format!("{}\n\n{}", existing, skill_prompt)),
                    None => Some(skill_prompt),
                }
            }
        } else {
            system
        };

        // Inject matched skill content on-demand before skill_registry is moved into AgentConfig
        let effective_prompt = if let Some(ref registry) = skill_registry {
            let skill_content = registry.match_skills(prompt);
            if skill_content.is_empty() {
                prompt.to_string()
            } else {
                format!("{}\n\n---\n\n{}", skill_content, prompt)
            }
        } else {
            prompt.to_string()
        };

        // Build AgentMemory from shared store (if configured)
        let memory = self
            .memory_store
            .read()
            .await
            .as_ref()
            .map(|store| Arc::new(AgentMemory::new(store.clone())));

        // Create agent loop with permission policy, confirmation manager, and context providers
        let config = AgentConfig {
            prompt_slots: match system {
                Some(s) => SystemPromptSlots::from_legacy(s),
                None => SystemPromptSlots::default(),
            },
            tools,
            max_tool_rounds: 50,
            permission_checker: Some(permission_checker),
            confirmation_manager: Some(confirmation_manager),
            context_providers,
            planning_enabled,
            goal_tracking,
            hook_engine,
            skill_registry,
            memory,
            ..AgentConfig::default()
        };

        let agent = AgentLoop::new(llm_client, self.tool_executor.clone(), tool_context, config)
            .with_tool_metrics(tool_metrics);

        // Execute with streaming
        let (rx, handle, cancel_token) =
            agent.execute_streaming(&history, &effective_prompt).await?;

        // Store the cancellation token for cancellation support
        let cancel_token_clone = cancel_token.clone();
        {
            let mut ops = self.ongoing_operations.write().await;
            ops.insert(session_id.to_string(), cancel_token);
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

        Ok((rx, wrapped_handle, cancel_token_clone))
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
            } else if let Some(client) = self.llm_client.read().await.clone() {
                client
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

        Ok(self.llm_client.read().await.clone())
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

        // Then, cancel the ongoing operation if any
        let cancel_token = {
            let mut ops = self.ongoing_operations.write().await;
            ops.remove(session_id)
        };

        if let Some(token) = cancel_token {
            token.cancel();
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
}
