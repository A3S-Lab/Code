//! Session management
//!
//! Provides session-based conversation management:
//! - Multiple independent sessions per agent
//! - Conversation history tracking
//! - Context usage monitoring
//! - Per-session LLM client configuration
//! - Session state management (Active, Paused, Completed, Error)
//! - Per-session command queue with lane-based priority
//! - Human-in-the-Loop (HITL) confirmation support
//! - Session persistence (JSONL file storage)
//!
//! ## Skill System
//!
//! Skills are loaded globally via `SessionManager::load_skill()` and available
//! to all sessions. Per-session tool access is controlled through `PermissionPolicy`.

use crate::agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult};
use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
use crate::llm::{self, ContentBlock, LlmClient, LlmConfig, Message, TokenUsage, ToolDefinition};
use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::queue::{ExternalTaskResult, LaneHandlerConfig, SessionQueueConfig};
use crate::session_lane_queue::SessionLaneQueue;
use crate::store::{FileSessionStore, LlmConfigData, SessionData, SessionStore};
use crate::todo::Todo;
use crate::tools::ToolExecutor;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};

/// Session state enum matching proto SessionState
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SessionState {
    #[default]
    Unknown = 0,
    Active = 1,
    Paused = 2,
    Completed = 3,
    Error = 4,
}

impl SessionState {
    /// Convert to proto i32 value
    pub fn to_proto_i32(self) -> i32 {
        self as i32
    }

    /// Create from proto i32 value
    pub fn from_proto_i32(value: i32) -> Self {
        match value {
            1 => SessionState::Active,
            2 => SessionState::Paused,
            3 => SessionState::Completed,
            4 => SessionState::Error,
            _ => SessionState::Unknown,
        }
    }
}

/// Context usage statistics
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

/// Session configuration (matches proto SessionConfig)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionConfig {
    pub name: String,
    pub workspace: String,
    pub system_prompt: Option<String>,
    pub max_context_length: u32,
    pub auto_compact: bool,
    /// Storage type for this session
    #[serde(default)]
    pub storage_type: crate::config::StorageBackend,
    /// Queue configuration (optional, uses defaults if None)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_config: Option<SessionQueueConfig>,
    /// Confirmation policy (optional, uses defaults if None)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_policy: Option<ConfirmationPolicy>,
    /// Permission policy (optional, uses defaults if None)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_policy: Option<PermissionPolicy>,
    /// Parent session ID (for subagent sessions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// SafeClaw security configuration (optional, enables security features)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safeclaw_config: Option<crate::safeclaw::SafeClawConfig>,
}
#[allow(dead_code)]
pub struct Session {
    pub id: String,
    pub config: SessionConfig,
    pub state: SessionState,
    pub messages: Vec<Message>,
    pub context_usage: ContextUsage,
    pub total_usage: TokenUsage,
    pub tools: Vec<ToolDefinition>,
    pub thinking_enabled: bool,
    pub thinking_budget: Option<usize>,
    /// Per-session LLM client (overrides default if set)
    pub llm_client: Option<Arc<dyn LlmClient>>,
    /// Creation timestamp (Unix epoch seconds)
    pub created_at: i64,
    /// Last update timestamp (Unix epoch seconds)
    pub updated_at: i64,
    /// Per-session command queue (a3s-lane backed)
    pub command_queue: SessionLaneQueue,
    /// HITL confirmation manager
    pub confirmation_manager: Arc<ConfirmationManager>,
    /// Permission policy for tool execution
    pub permission_policy: Arc<RwLock<PermissionPolicy>>,
    /// Event broadcaster for this session
    event_tx: broadcast::Sender<AgentEvent>,
    /// Context providers for augmenting prompts with external context
    pub context_providers: Vec<Arc<dyn crate::context::ContextProvider>>,
    /// Todo list for task tracking
    pub todos: Vec<Todo>,
    /// Parent session ID (for subagent sessions)
    pub parent_id: Option<String>,
    /// Agent memory system for this session
    pub memory: Arc<RwLock<crate::memory::AgentMemory>>,
    /// Current execution plan (if any)
    pub current_plan: Arc<RwLock<Option<crate::planning::ExecutionPlan>>>,
    /// SafeClaw security guard (if enabled)
    pub safeclaw_guard: Option<Arc<crate::safeclaw::SafeClawGuard>>,
}

impl Session {
    /// Create a new session (async due to SessionLaneQueue initialization)
    pub async fn new(
        id: String,
        config: SessionConfig,
        tools: Vec<ToolDefinition>,
    ) -> Result<Self> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Create event broadcaster
        let (event_tx, _) = broadcast::channel(100);

        // Create command queue with config or defaults
        let queue_config = config.queue_config.clone().unwrap_or_default();
        let command_queue = SessionLaneQueue::new(&id, queue_config, event_tx.clone()).await?;

        // Create confirmation manager with policy or defaults
        let confirmation_policy = config.confirmation_policy.clone().unwrap_or_default();
        let confirmation_manager = Arc::new(ConfirmationManager::new(
            confirmation_policy,
            event_tx.clone(),
        ));

        // Create permission policy with config or defaults
        let permission_policy = Arc::new(RwLock::new(
            config.permission_policy.clone().unwrap_or_default(),
        ));

        // Extract parent_id from config
        let parent_id = config.parent_id.clone();

        // Create memory system with file-based storage
        // Memory file is stored in workspace/.a3s/memories/{session_id}.jsonl
        let memory_dir = std::path::PathBuf::from(&config.workspace)
            .join(".a3s")
            .join("memories");
        let memory_file = memory_dir.join(format!("{}.jsonl", &id));

        let memory_store: Arc<dyn crate::memory::MemoryStore> =
            match crate::memory::FileStore::new(&memory_file) {
                Ok(store) => Arc::new(store),
                Err(e) => {
                    // Fall back to in-memory store if file store fails
                    tracing::warn!(
                    "Failed to create file-based memory store at {:?}: {}. Using in-memory store.",
                    memory_file,
                    e
                );
                    Arc::new(crate::memory::InMemoryStore::new())
                }
            };
        let memory = Arc::new(RwLock::new(crate::memory::AgentMemory::new(memory_store)));

        // Initialize empty plan
        let current_plan = Arc::new(RwLock::new(None));

        // Initialize SafeClaw guard if configured
        let safeclaw_guard = config.safeclaw_config.as_ref().and_then(|sc| {
            if sc.enabled {
                Some(Arc::new(crate::safeclaw::SafeClawGuard::new(
                    id.clone(),
                    sc.clone(),
                    &crate::hooks::HookEngine::new(), // Per-session hook engine
                )))
            } else {
                None
            }
        });

        Ok(Self {
            id,
            config,
            state: SessionState::Active,
            messages: Vec::new(),
            context_usage: ContextUsage::default(),
            total_usage: TokenUsage::default(),
            tools,
            thinking_enabled: false,
            thinking_budget: None,
            llm_client: None,
            created_at: now,
            updated_at: now,
            command_queue,
            confirmation_manager,
            permission_policy,
            event_tx,
            context_providers: Vec::new(),
            todos: Vec::new(),
            parent_id,
            memory,
            current_plan,
            safeclaw_guard,
        })
    }

    /// Check if this is a child session (has a parent)
    pub fn is_child_session(&self) -> bool {
        self.parent_id.is_some()
    }

    /// Get the parent session ID if this is a child session
    pub fn parent_session_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    /// Get a receiver for session events
    pub fn subscribe_events(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    /// Get the event broadcaster
    pub fn event_tx(&self) -> broadcast::Sender<AgentEvent> {
        self.event_tx.clone()
    }

    /// Update the confirmation policy
    pub async fn set_confirmation_policy(&self, policy: ConfirmationPolicy) {
        self.confirmation_manager.set_policy(policy).await;
    }

    /// Get the current confirmation policy
    pub async fn confirmation_policy(&self) -> ConfirmationPolicy {
        self.confirmation_manager.policy().await
    }

    /// Update the permission policy
    pub async fn set_permission_policy(&self, policy: PermissionPolicy) {
        let mut p = self.permission_policy.write().await;
        *p = policy;
    }

    /// Get the current permission policy
    pub async fn permission_policy(&self) -> PermissionPolicy {
        self.permission_policy.read().await.clone()
    }

    /// Check permission for a tool invocation
    pub async fn check_permission(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> PermissionDecision {
        self.permission_policy.read().await.check(tool_name, args)
    }

    /// Add an allow rule to the permission policy
    pub async fn add_allow_rule(&self, rule: &str) {
        let mut p = self.permission_policy.write().await;
        p.allow.push(crate::permissions::PermissionRule::new(rule));
    }

    /// Add a deny rule to the permission policy
    pub async fn add_deny_rule(&self, rule: &str) {
        let mut p = self.permission_policy.write().await;
        p.deny.push(crate::permissions::PermissionRule::new(rule));
    }

    /// Add an ask rule to the permission policy
    pub async fn add_ask_rule(&self, rule: &str) {
        let mut p = self.permission_policy.write().await;
        p.ask.push(crate::permissions::PermissionRule::new(rule));
    }

    /// Add a context provider to the session
    pub fn add_context_provider(&mut self, provider: Arc<dyn crate::context::ContextProvider>) {
        self.context_providers.push(provider);
    }

    /// Remove a context provider by name
    ///
    /// Returns true if a provider was removed, false otherwise.
    pub fn remove_context_provider(&mut self, name: &str) -> bool {
        let initial_len = self.context_providers.len();
        self.context_providers.retain(|p| p.name() != name);
        self.context_providers.len() < initial_len
    }

    /// Get the names of all registered context providers
    pub fn context_provider_names(&self) -> Vec<String> {
        self.context_providers
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }

    // ========================================================================
    // Todo Management
    // ========================================================================

    /// Get the current todo list
    pub fn get_todos(&self) -> &[Todo] {
        &self.todos
    }

    /// Set the todo list (replaces entire list)
    ///
    /// Broadcasts a TodoUpdated event after updating.
    pub fn set_todos(&mut self, todos: Vec<Todo>) {
        self.todos = todos.clone();
        self.touch();

        // Broadcast event
        let _ = self.event_tx.send(AgentEvent::TodoUpdated {
            session_id: self.id.clone(),
            todos,
        });
    }

    /// Get count of active (non-completed, non-cancelled) todos
    pub fn active_todo_count(&self) -> usize {
        self.todos.iter().filter(|t| t.is_active()).count()
    }

    /// Set handler mode for a lane
    pub async fn set_lane_handler(
        &self,
        lane: crate::hitl::SessionLane,
        config: LaneHandlerConfig,
    ) {
        self.command_queue.set_lane_handler(lane, config).await;
    }

    /// Get handler config for a lane
    pub async fn get_lane_handler(&self, lane: crate::hitl::SessionLane) -> LaneHandlerConfig {
        self.command_queue.get_lane_handler(lane).await
    }

    /// Complete an external task
    pub async fn complete_external_task(&self, task_id: &str, result: ExternalTaskResult) -> bool {
        self.command_queue
            .complete_external_task(task_id, result)
            .await
    }

    /// Get pending external tasks
    pub async fn pending_external_tasks(&self) -> Vec<crate::queue::ExternalTask> {
        self.command_queue.pending_external_tasks().await
    }

    /// Get dead letters from the queue's DLQ
    pub async fn dead_letters(&self) -> Vec<a3s_lane::DeadLetter> {
        self.command_queue.dead_letters().await
    }

    /// Get queue metrics snapshot
    pub async fn queue_metrics(&self) -> Option<a3s_lane::MetricsSnapshot> {
        self.command_queue.metrics_snapshot().await
    }

    /// Get queue statistics
    pub async fn queue_stats(&self) -> crate::queue::SessionQueueStats {
        self.command_queue.stats().await
    }

    /// Start the command queue scheduler
    pub async fn start_queue(&self) -> Result<()> {
        self.command_queue.start().await
    }

    /// Stop the command queue scheduler
    pub async fn stop_queue(&self) {
        self.command_queue.stop().await;
    }

    /// Get the system prompt from config
    pub fn system(&self) -> Option<&str> {
        self.config.system_prompt.as_deref()
    }

    /// Get conversation history
    #[allow(dead_code)]
    pub fn history(&self) -> &[Message] {
        &self.messages
    }

    /// Add a message to history
    #[allow(dead_code)]
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.context_usage.turns = self.messages.len();
        self.touch();
    }

    /// Update context usage after a response
    pub fn update_usage(&mut self, usage: &TokenUsage) {
        self.total_usage.prompt_tokens += usage.prompt_tokens;
        self.total_usage.completion_tokens += usage.completion_tokens;
        self.total_usage.total_tokens += usage.total_tokens;

        // Estimate context usage (rough approximation)
        self.context_usage.used_tokens = usage.prompt_tokens;
        self.context_usage.percent =
            self.context_usage.used_tokens as f32 / self.context_usage.max_tokens as f32;
        self.touch();
    }

    /// Clear conversation history
    pub fn clear(&mut self) {
        self.messages.clear();
        self.context_usage = ContextUsage::default();
        self.touch();
    }

    /// Compact context by summarizing old messages
    pub async fn compact(&mut self, llm_client: &Arc<dyn LlmClient>) -> Result<()> {
        // Configuration for compaction
        const KEEP_RECENT_MESSAGES: usize = 20; // Keep last N messages intact
        const MIN_MESSAGES_FOR_COMPACTION: usize = 30; // Only compact if we have more than this
        const KEEP_INITIAL_MESSAGES: usize = 2; // Keep first N messages (usually system context)

        // Check if compaction is needed
        if self.messages.len() <= MIN_MESSAGES_FOR_COMPACTION {
            tracing::debug!(
                "Session {} has {} messages, no compaction needed (threshold: {})",
                self.id,
                self.messages.len(),
                MIN_MESSAGES_FOR_COMPACTION
            );
            return Ok(());
        }

        tracing::info!(
            "Compacting session {} with {} messages",
            self.id,
            self.messages.len()
        );

        // Split messages into: initial (keep), middle (summarize), recent (keep)
        let total = self.messages.len();
        let summarize_start = KEEP_INITIAL_MESSAGES;
        let summarize_end = total.saturating_sub(KEEP_RECENT_MESSAGES);

        // If there's nothing to summarize, just keep recent messages
        if summarize_end <= summarize_start {
            tracing::debug!(
                "Not enough messages to summarize, keeping last {}",
                KEEP_RECENT_MESSAGES
            );
            self.messages = self
                .messages
                .split_off(total.saturating_sub(KEEP_RECENT_MESSAGES));
            self.touch();
            return Ok(());
        }

        // Extract messages to summarize
        let initial_messages = self.messages[..summarize_start].to_vec();
        let messages_to_summarize = self.messages[summarize_start..summarize_end].to_vec();
        let recent_messages = self.messages[summarize_end..].to_vec();

        tracing::debug!(
            "Compaction split: {} initial, {} to summarize, {} recent",
            initial_messages.len(),
            messages_to_summarize.len(),
            recent_messages.len()
        );

        // Build summarization prompt
        let conversation_text = messages_to_summarize
            .iter()
            .map(|msg| {
                let role = &msg.role;
                let text = msg.text();
                format!("{}: {}", role, text)
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let summarization_prompt = format!(
            "Please provide a concise summary of the following conversation history. \
            Focus on key decisions, important information, and context that would be \
            useful for continuing the conversation. Keep the summary under 500 words.\n\n\
            Conversation:\n{}\n\n\
            Summary:",
            conversation_text
        );

        // Call LLM to generate summary
        let summary_message = Message::user(&summarization_prompt);
        let response = llm_client
            .complete(&[summary_message], None, &[])
            .await
            .context("Failed to generate conversation summary")?;

        let summary_text = response.text();
        tracing::debug!("Generated summary: {} chars", summary_text.len());

        // Create a summary message
        let summary_message = Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: format!(
                    "[Context Summary: The following is a summary of earlier conversation]\n\n{}",
                    summary_text
                ),
            }],
        };

        // Reconstruct messages: initial + summary + recent
        let mut new_messages = initial_messages;
        new_messages.push(summary_message);
        new_messages.extend(recent_messages);

        tracing::info!(
            "Compaction complete: {} messages -> {} messages",
            self.messages.len(),
            new_messages.len()
        );

        self.messages = new_messages;
        self.touch();
        Ok(())
    }

    /// Pause the session
    pub fn pause(&mut self) -> bool {
        if self.state == SessionState::Active {
            self.state = SessionState::Paused;
            self.touch();
            true
        } else {
            false
        }
    }

    /// Resume the session
    pub fn resume(&mut self) -> bool {
        if self.state == SessionState::Paused {
            self.state = SessionState::Active;
            self.touch();
            true
        } else {
            false
        }
    }

    /// Set session state to error
    pub fn set_error(&mut self) {
        self.state = SessionState::Error;
        self.touch();
    }

    /// Set session state to completed
    pub fn set_completed(&mut self) {
        self.state = SessionState::Completed;
        self.touch();
    }

    /// Update the updated_at timestamp
    fn touch(&mut self) {
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
    }

    /// Convert to serializable SessionData for persistence
    pub fn to_session_data(&self, llm_config: Option<LlmConfigData>) -> SessionData {
        SessionData {
            id: self.id.clone(),
            config: self.config.clone(),
            state: self.state,
            messages: self.messages.clone(),
            context_usage: self.context_usage.clone(),
            total_usage: self.total_usage.clone(),
            tool_names: SessionData::tool_names_from_definitions(&self.tools),
            thinking_enabled: self.thinking_enabled,
            thinking_budget: self.thinking_budget,
            created_at: self.created_at,
            updated_at: self.updated_at,
            llm_config,
            todos: self.todos.clone(),
            parent_id: self.parent_id.clone(),
        }
    }

    /// Restore session state from SessionData
    ///
    /// Note: This only restores serializable fields. Non-serializable fields
    /// (event_tx, command_queue, confirmation_manager) are already initialized
    /// in Session::new().
    pub fn restore_from_data(&mut self, data: &SessionData) {
        self.state = data.state;
        self.messages = data.messages.clone();
        self.context_usage = data.context_usage.clone();
        self.total_usage = data.total_usage.clone();
        self.thinking_enabled = data.thinking_enabled;
        self.thinking_budget = data.thinking_budget;
        self.created_at = data.created_at;
        self.updated_at = data.updated_at;
        self.todos = data.todos.clone();
        self.parent_id = data.parent_id.clone();
    }
}

/// Session manager handles multiple concurrent sessions
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Arc<RwLock<Session>>>>>,
    llm_client: Option<Arc<dyn LlmClient>>, // Optional default LLM client
    tool_executor: Arc<ToolExecutor>,
    /// Session stores by storage type
    stores: Arc<RwLock<HashMap<crate::config::StorageBackend, Arc<dyn SessionStore>>>>,
    /// Track which storage type each session uses
    session_storage_types: Arc<RwLock<HashMap<String, crate::config::StorageBackend>>>,
    /// LLM configurations for sessions (stored separately for persistence)
    llm_configs: Arc<RwLock<HashMap<String, LlmConfigData>>>,
    /// Ongoing operations (session_id -> JoinHandle)
    ongoing_operations: Arc<RwLock<HashMap<String, tokio::task::AbortHandle>>>,
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

        let mut manager = Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            llm_client,
            tool_executor,
            stores: Arc::new(RwLock::new(stores)),
            session_storage_types: Arc::new(RwLock::new(HashMap::new())),
            llm_configs: Arc::new(RwLock::new(HashMap::new())),
            ongoing_operations: Arc::new(RwLock::new(HashMap::new())),
        };

        // Load existing sessions
        manager.load_all_sessions().await?;

        Ok(manager)
    }

    /// Create a session manager with a custom store
    pub fn with_store(
        llm_client: Option<Arc<dyn LlmClient>>,
        tool_executor: Arc<ToolExecutor>,
        store: Arc<dyn SessionStore>,
    ) -> Self {
        let mut stores = HashMap::new();
        stores.insert(crate::config::StorageBackend::File, store);

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

    /// Load all sessions from the store
    pub async fn load_all_sessions(&mut self) -> Result<usize> {
        let stores = self.stores.read().await;
        let file_store = stores.get(&crate::config::StorageBackend::File);

        let Some(store) = file_store else {
            return Ok(0);
        };

        let session_ids = store.list().await?;
        let mut loaded = 0;

        for id in session_ids {
            match store.load(&id).await {
                Ok(Some(data)) => {
                    // Record the storage type for this session
                    {
                        let mut storage_types = self.session_storage_types.write().await;
                        storage_types.insert(data.id.clone(), data.config.storage_type.clone());
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

    /// Create a new session
    pub async fn create_session(&self, id: String, config: SessionConfig) -> Result<String> {
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
        if let Err(e) = self.save_session(&id).await {
            tracing::warn!("Failed to persist session {}: {}", id, e);
        }

        tracing::info!("Created session: {}", id);
        Ok(id)
    }

    /// Destroy a session
    pub async fn destroy_session(&self, id: &str) -> Result<()> {
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

    /// List all session IDs
    #[allow(dead_code)]
    pub async fn list_sessions(&self) -> Vec<String> {
        let sessions = self.sessions.read().await;
        sessions.keys().cloned().collect()
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
        // Verify parent exists
        let parent_lock = self.get_session(parent_id).await?;
        let parent_llm_client = {
            let parent = parent_lock.read().await;
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
        if let Err(e) = self.save_session(&child_id).await {
            tracing::warn!("Failed to persist child session {}: {}", child_id, e);
        }

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

        // Create agent loop with permission policy, confirmation manager, and context providers
        let config = AgentConfig {
            system_prompt: system,
            tools,
            max_tool_rounds: 50,
            permission_policy: Some(permission_policy),
            confirmation_manager: Some(confirmation_manager),
            context_providers,
            planning_enabled: false,
            goal_tracking: false,
        };

        let agent = AgentLoop::new(llm_client, self.tool_executor.clone(), config);

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
        if let Err(e) = self.save_session(session_id).await {
            tracing::warn!(
                "Failed to persist session {} after generate: {}",
                session_id,
                e
            );
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

        // Create agent loop with permission policy, confirmation manager, and context providers
        let config = AgentConfig {
            system_prompt: system,
            tools,
            max_tool_rounds: 50,
            permission_policy: Some(permission_policy),
            confirmation_manager: Some(confirmation_manager),
            context_providers,
            planning_enabled: false,
            goal_tracking: false,
        };

        let agent = AgentLoop::new(llm_client, self.tool_executor.clone(), config);

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
        if let Err(e) = self.save_session(session_id).await {
            tracing::warn!(
                "Failed to persist session {} after clear: {}",
                session_id,
                e
            );
        }

        Ok(())
    }

    /// Compact session context
    pub async fn compact(&self, session_id: &str) -> Result<()> {
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
                if let Err(e) = self.save_session(session_id).await {
                    tracing::warn!(
                        "Failed to persist session {} after compact: {}",
                        session_id,
                        e
                    );
                }
                return Ok(());
            };

            session.compact(&llm_client).await?;
        }

        // Persist to store
        if let Err(e) = self.save_session(session_id).await {
            tracing::warn!(
                "Failed to persist session {} after compact: {}",
                session_id,
                e
            );
        }

        Ok(())
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
        if let Err(e) = self.save_session(session_id).await {
            tracing::warn!(
                "Failed to persist session {} after configure: {}",
                session_id,
                e
            );
        }

        Ok(())
    }

    /// Get session count
    #[allow(dead_code)]
    pub async fn session_count(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }

    /// Load a skill globally (available to all sessions)
    ///
    /// Registers the skill's tools with the shared tool executor.
    /// Returns the names of tools that were registered.
    pub fn load_skill(&self, skill_name: &str, skill_content: &str) -> Vec<String> {
        let tool_names = self.tool_executor.register_skill_tools(skill_content);

        if tool_names.is_empty() {
            tracing::warn!("No tools found in skill: {}", skill_name);
        } else {
            tracing::info!("Loaded skill {} with tools: {:?}", skill_name, tool_names);
        }

        tool_names
    }

    /// Unload a skill globally (removes tools from all sessions)
    ///
    /// Unregisters the skill's tools from the shared tool executor.
    /// Returns the names of tools that were unregistered.
    pub fn unload_skill(&self, tool_names: &[String]) -> Vec<String> {
        let removed = self.tool_executor.unregister_tools(tool_names);

        if !removed.is_empty() {
            tracing::info!("Unloaded skill tools: {:?}", removed);
        }

        removed
    }

    /// List all loaded tools (from built-in and skills)
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
            if let Err(e) = self.save_session(session_id).await {
                tracing::warn!(
                    "Failed to persist session {} after pause: {}",
                    session_id,
                    e
                );
            }
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
            if let Err(e) = self.save_session(session_id).await {
                tracing::warn!(
                    "Failed to persist session {} after resume: {}",
                    session_id,
                    e
                );
            }
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
        if let Err(e) = self.save_session(session_id).await {
            tracing::warn!(
                "Failed to persist session {} after set_confirmation_policy: {}",
                session_id,
                e
            );
        }

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
        if let Err(e) = self.save_session(session_id).await {
            tracing::warn!(
                "Failed to persist session {} after set_permission_policy: {}",
                session_id,
                e
            );
        }

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
    // Todo Management
    // ========================================================================

    /// Get todos for a session
    pub async fn get_todos(&self, session_id: &str) -> Result<Vec<Todo>> {
        let session_lock = self.get_session(session_id).await?;
        let session = session_lock.read().await;
        Ok(session.get_todos().to_vec())
    }

    /// Set todos for a session
    pub async fn set_todos(&self, session_id: &str, todos: Vec<Todo>) -> Result<Vec<Todo>> {
        {
            let session_lock = self.get_session(session_id).await?;
            let mut session = session_lock.write().await;
            session.set_todos(todos);
        }

        // Save session after updating todos
        if let Err(e) = self.save_session(session_id).await {
            tracing::warn!(
                "Failed to persist session {} after todo update: {}",
                session_id,
                e
            );
        }

        // Return updated todos
        self.get_todos(session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hitl::{ConfirmationPolicy, SessionLane, TimeoutAction};
    use crate::permissions::{PermissionDecision, PermissionPolicy};
    use crate::queue::{
        ExternalTaskResult, LaneHandlerConfig, SessionQueueConfig, TaskHandlerMode,
    };
    use crate::store::MemorySessionStore;

    // ========================================================================
    // Basic Session Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_creation() {
        let config = SessionConfig {
            name: "test".to_string(),
            workspace: "/tmp".to_string(),
            system_prompt: Some("You are helpful.".to_string()),
            max_context_length: 0,
            auto_compact: false,
            storage_type: crate::config::StorageBackend::Memory,
            queue_config: None,
            confirmation_policy: None,
            permission_policy: None,
            parent_id: None,
            safeclaw_config: None,
        };
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert_eq!(session.id, "test-1");
        assert_eq!(session.system(), Some("You are helpful."));
        assert!(session.messages.is_empty());
        assert_eq!(session.state, SessionState::Active);
        assert!(session.created_at > 0);
    }

    #[tokio::test]
    async fn test_session_creation_with_queue_config() {
        let queue_config = SessionQueueConfig {
            control_max_concurrency: 1,
            query_max_concurrency: 2,
            execute_max_concurrency: 3,
            generate_max_concurrency: 4,
            lane_handlers: std::collections::HashMap::new(),
            ..Default::default()
        };
        let config = SessionConfig {
            queue_config: Some(queue_config),
            ..Default::default()
        };
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert_eq!(session.id, "test-1");
    }

    #[tokio::test]
    async fn test_session_creation_with_confirmation_policy() {
        let policy = ConfirmationPolicy::enabled()
            .with_yolo_lanes([SessionLane::Query])
            .with_timeout(5000, TimeoutAction::AutoApprove);

        let config = SessionConfig {
            confirmation_policy: Some(policy),
            ..Default::default()
        };
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert_eq!(session.id, "test-1");
    }

    #[test]
    fn test_context_usage_default() {
        let usage = ContextUsage::default();
        assert_eq!(usage.used_tokens, 0);
        assert_eq!(usage.max_tokens, 200_000);
        assert_eq!(usage.percent, 0.0);
    }

    #[tokio::test]
    async fn test_session_pause_resume() {
        let config = SessionConfig::default();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        assert_eq!(session.state, SessionState::Active);

        // Pause
        assert!(session.pause());
        assert_eq!(session.state, SessionState::Paused);

        // Can't pause again
        assert!(!session.pause());

        // Resume
        assert!(session.resume());
        assert_eq!(session.state, SessionState::Active);

        // Can't resume again
        assert!(!session.resume());
    }

    #[test]
    fn test_session_state_conversion() {
        assert_eq!(SessionState::Active.to_proto_i32(), 1);
        assert_eq!(SessionState::Paused.to_proto_i32(), 2);
        assert_eq!(SessionState::from_proto_i32(1), SessionState::Active);
        assert_eq!(SessionState::from_proto_i32(2), SessionState::Paused);
        assert_eq!(SessionState::from_proto_i32(99), SessionState::Unknown);
    }

    // ========================================================================
    // Session HITL Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_confirmation_policy() {
        let config = SessionConfig::default();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Default policy (HITL disabled)
        let policy = session.confirmation_policy().await;
        assert!(!policy.enabled);

        // Update policy
        let new_policy = ConfirmationPolicy::enabled()
            .with_yolo_lanes([SessionLane::Execute])
            .with_timeout(10000, TimeoutAction::Reject);

        session.set_confirmation_policy(new_policy).await;

        let policy = session.confirmation_policy().await;
        assert!(policy.enabled);
        assert!(policy.yolo_lanes.contains(&SessionLane::Execute));
        assert_eq!(policy.default_timeout_ms, 10000);
        assert_eq!(policy.timeout_action, TimeoutAction::Reject);
    }

    #[tokio::test]
    async fn test_session_subscribe_events() {
        let config = SessionConfig::default();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Subscribe to events
        let mut rx = session.subscribe_events();

        // Send an event through the broadcaster
        let tx = session.event_tx();
        tx.send(crate::agent::AgentEvent::Start {
            prompt: "test".to_string(),
        })
        .unwrap();

        // Should receive the event
        let event = rx.recv().await.unwrap();
        match event {
            crate::agent::AgentEvent::Start { prompt } => {
                assert_eq!(prompt, "test");
            }
            _ => panic!("Expected Start event"),
        }
    }

    // ========================================================================
    // Session Lane Handler Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_lane_handler() {
        let config = SessionConfig::default();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Default handler mode
        let handler = session.get_lane_handler(SessionLane::Execute).await;
        assert_eq!(handler.mode, TaskHandlerMode::Internal);

        // Set new handler
        session
            .set_lane_handler(
                SessionLane::Execute,
                LaneHandlerConfig {
                    mode: TaskHandlerMode::External,
                    timeout_ms: 30000,
                },
            )
            .await;

        let handler = session.get_lane_handler(SessionLane::Execute).await;
        assert_eq!(handler.mode, TaskHandlerMode::External);
        assert_eq!(handler.timeout_ms, 30000);
    }

    #[tokio::test]
    async fn test_session_external_tasks() {
        let config = SessionConfig::default();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Initially no pending external tasks
        let pending = session.pending_external_tasks().await;
        assert!(pending.is_empty());

        // Complete non-existent task
        let completed = session
            .complete_external_task(
                "non-existent",
                ExternalTaskResult {
                    success: true,
                    result: serde_json::json!({}),
                    error: None,
                },
            )
            .await;
        assert!(!completed);
    }

    // ========================================================================
    // SessionManager Tests
    // ========================================================================

    fn create_test_session_manager() -> SessionManager {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        SessionManager::new(None, tool_executor)
    }

    #[tokio::test]
    async fn test_session_manager_create_session() {
        let manager = create_test_session_manager();

        let config = SessionConfig {
            name: "test-session".to_string(),
            ..Default::default()
        };

        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        let session_lock = manager.get_session("session-1").await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.id, "session-1");
        assert_eq!(session.config.name, "test-session");
    }

    #[tokio::test]
    async fn test_session_manager_destroy_session() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Session exists
        assert!(manager.get_session("session-1").await.is_ok());

        // Destroy session
        manager.destroy_session("session-1").await.unwrap();

        // Session no longer exists
        assert!(manager.get_session("session-1").await.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_list_sessions() {
        let manager = create_test_session_manager();

        // Create multiple sessions
        for i in 0..3 {
            let config = SessionConfig {
                name: format!("session-{}", i),
                ..Default::default()
            };
            manager
                .create_session(format!("session-{}", i), config)
                .await
                .unwrap();
        }

        let sessions = manager.get_all_sessions().await;
        assert_eq!(sessions.len(), 3);
    }

    #[tokio::test]
    async fn test_session_manager_pause_resume() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Pause
        assert!(manager.pause_session("session-1").await.unwrap());

        // Resume
        assert!(manager.resume_session("session-1").await.unwrap());
    }

    // ========================================================================
    // SessionManager HITL Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_confirmation_policy() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Get default policy
        let policy = manager.get_confirmation_policy("session-1").await.unwrap();
        assert!(!policy.enabled);

        // Set new policy
        let new_policy = ConfirmationPolicy::enabled()
            .with_yolo_lanes([SessionLane::Query, SessionLane::Execute])
            .with_auto_approve_tools(["bash".to_string()]);

        let result = manager
            .set_confirmation_policy("session-1", new_policy)
            .await
            .unwrap();
        assert!(result.enabled);
        assert!(result.yolo_lanes.contains(&SessionLane::Query));
        assert!(result.yolo_lanes.contains(&SessionLane::Execute));
        assert!(result.auto_approve_tools.contains("bash"));

        // Verify policy was persisted
        let policy = manager.get_confirmation_policy("session-1").await.unwrap();
        assert!(policy.enabled);
    }

    #[tokio::test]
    async fn test_session_manager_confirm_tool_not_found() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Confirm non-existent tool
        let result = manager
            .confirm_tool("session-1", "non-existent", true, None)
            .await
            .unwrap();
        assert!(!result); // Not found
    }

    #[tokio::test]
    async fn test_session_manager_confirm_tool_session_not_found() {
        let manager = create_test_session_manager();

        // Session doesn't exist
        let result = manager
            .confirm_tool("non-existent-session", "tool-1", true, None)
            .await;
        assert!(result.is_err());
    }

    // ========================================================================
    // SessionManager Lane Handler Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_lane_handler() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Get default handler
        let handler = manager
            .get_lane_handler("session-1", SessionLane::Execute)
            .await
            .unwrap();
        assert_eq!(handler.mode, TaskHandlerMode::Internal);

        // Set new handler
        manager
            .set_lane_handler(
                "session-1",
                SessionLane::Execute,
                LaneHandlerConfig {
                    mode: TaskHandlerMode::External,
                    timeout_ms: 45000,
                },
            )
            .await
            .unwrap();

        // Verify handler was set
        let handler = manager
            .get_lane_handler("session-1", SessionLane::Execute)
            .await
            .unwrap();
        assert_eq!(handler.mode, TaskHandlerMode::External);
        assert_eq!(handler.timeout_ms, 45000);
    }

    #[tokio::test]
    async fn test_session_manager_lane_handler_session_not_found() {
        let manager = create_test_session_manager();

        let result = manager
            .get_lane_handler("non-existent", SessionLane::Execute)
            .await;
        assert!(result.is_err());

        let result = manager
            .set_lane_handler(
                "non-existent",
                SessionLane::Execute,
                LaneHandlerConfig::default(),
            )
            .await;
        assert!(result.is_err());
    }

    // ========================================================================
    // SessionManager External Task Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_external_tasks() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Initially no pending tasks
        let pending = manager.pending_external_tasks("session-1").await.unwrap();
        assert!(pending.is_empty());

        // Complete non-existent task
        let result = manager
            .complete_external_task(
                "session-1",
                "non-existent-task",
                ExternalTaskResult {
                    success: true,
                    result: serde_json::json!({}),
                    error: None,
                },
            )
            .await
            .unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_session_manager_external_tasks_session_not_found() {
        let manager = create_test_session_manager();

        let result = manager.pending_external_tasks("non-existent").await;
        assert!(result.is_err());

        let result = manager
            .complete_external_task(
                "non-existent",
                "task-1",
                ExternalTaskResult {
                    success: true,
                    result: serde_json::json!({}),
                    error: None,
                },
            )
            .await;
        assert!(result.is_err());
    }

    // ========================================================================
    // Integration Tests: Multiple Sessions
    // ========================================================================

    #[tokio::test]
    async fn test_multiple_sessions_independent_policies() {
        let manager = create_test_session_manager();

        // Create two sessions with different policies
        let config1 = SessionConfig {
            confirmation_policy: Some(ConfirmationPolicy::enabled()),
            ..Default::default()
        };
        let config2 = SessionConfig {
            confirmation_policy: Some(
                ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Execute]),
            ),
            ..Default::default()
        };

        manager
            .create_session("session-1".to_string(), config1)
            .await
            .unwrap();
        manager
            .create_session("session-2".to_string(), config2)
            .await
            .unwrap();

        // Verify policies are independent
        let policy1 = manager.get_confirmation_policy("session-1").await.unwrap();
        let policy2 = manager.get_confirmation_policy("session-2").await.unwrap();

        assert!(policy1.enabled);
        assert!(policy1.yolo_lanes.is_empty());

        assert!(policy2.enabled);
        assert!(policy2.yolo_lanes.contains(&SessionLane::Execute));

        // Update session-1 policy
        manager
            .set_confirmation_policy(
                "session-1",
                ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Query]),
            )
            .await
            .unwrap();

        // session-2 should be unchanged
        let policy2 = manager.get_confirmation_policy("session-2").await.unwrap();
        assert!(!policy2.yolo_lanes.contains(&SessionLane::Query));
        assert!(policy2.yolo_lanes.contains(&SessionLane::Execute));
    }

    #[tokio::test]
    async fn test_multiple_sessions_independent_handlers() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config.clone())
            .await
            .unwrap();
        manager
            .create_session("session-2".to_string(), config)
            .await
            .unwrap();

        // Set different handlers for each session
        manager
            .set_lane_handler(
                "session-1",
                SessionLane::Execute,
                LaneHandlerConfig {
                    mode: TaskHandlerMode::External,
                    timeout_ms: 10000,
                },
            )
            .await
            .unwrap();

        manager
            .set_lane_handler(
                "session-2",
                SessionLane::Execute,
                LaneHandlerConfig {
                    mode: TaskHandlerMode::Hybrid,
                    timeout_ms: 20000,
                },
            )
            .await
            .unwrap();

        // Verify handlers are independent
        let handler1 = manager
            .get_lane_handler("session-1", SessionLane::Execute)
            .await
            .unwrap();
        let handler2 = manager
            .get_lane_handler("session-2", SessionLane::Execute)
            .await
            .unwrap();

        assert_eq!(handler1.mode, TaskHandlerMode::External);
        assert_eq!(handler1.timeout_ms, 10000);

        assert_eq!(handler2.mode, TaskHandlerMode::Hybrid);
        assert_eq!(handler2.timeout_ms, 20000);
    }

    // ========================================================================
    // Permission Policy Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_permission_policy() {
        let config = SessionConfig::default();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Default policy asks for everything
        let decision = session
            .check_permission("Bash", &serde_json::json!({"command": "ls -la"}))
            .await;
        assert_eq!(decision, PermissionDecision::Ask);
    }

    #[tokio::test]
    async fn test_session_permission_policy_custom() {
        let policy = PermissionPolicy::new()
            .allow("Bash(cargo:*)")
            .deny("Bash(rm:*)");

        let config = SessionConfig {
            permission_policy: Some(policy),
            ..Default::default()
        };
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // cargo commands are allowed
        let decision = session
            .check_permission("Bash", &serde_json::json!({"command": "cargo build"}))
            .await;
        assert_eq!(decision, PermissionDecision::Allow);

        // rm commands are denied
        let decision = session
            .check_permission("Bash", &serde_json::json!({"command": "rm -rf /tmp"}))
            .await;
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[tokio::test]
    async fn test_session_add_permission_rules() {
        let config = SessionConfig::default();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Add allow rule
        session.add_allow_rule("Bash(npm:*)").await;

        // npm commands should now be allowed
        let decision = session
            .check_permission("Bash", &serde_json::json!({"command": "npm install"}))
            .await;
        assert_eq!(decision, PermissionDecision::Allow);

        // Add deny rule
        session.add_deny_rule("Bash(npm audit:*)").await;

        // npm audit should be denied (deny wins)
        let decision = session
            .check_permission("Bash", &serde_json::json!({"command": "npm audit fix"}))
            .await;
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[tokio::test]
    async fn test_session_manager_permission_policy() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Get default policy
        let policy = manager.get_permission_policy("session-1").await.unwrap();
        assert_eq!(policy.default_decision, PermissionDecision::Ask);

        // Set custom policy
        let new_policy = PermissionPolicy::new()
            .allow("Bash(cargo:*)")
            .allow("Grep(*)");

        manager
            .set_permission_policy("session-1", new_policy)
            .await
            .unwrap();

        // Check permission
        let decision = manager
            .check_permission(
                "session-1",
                "Bash",
                &serde_json::json!({"command": "cargo test"}),
            )
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Allow);

        // Grep is also allowed
        let decision = manager
            .check_permission("session-1", "Grep", &serde_json::json!({"pattern": "TODO"}))
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Allow);

        // Other tools still ask
        let decision = manager
            .check_permission(
                "session-1",
                "Write",
                &serde_json::json!({"file_path": "/tmp/test"}),
            )
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Ask);
    }

    #[tokio::test]
    async fn test_session_manager_add_permission_rule() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Add allow rule
        manager
            .add_permission_rule("session-1", "allow", "Bash(just:*)")
            .await
            .unwrap();

        // just commands should be allowed
        let decision = manager
            .check_permission(
                "session-1",
                "Bash",
                &serde_json::json!({"command": "just test"}),
            )
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Allow);

        // Add deny rule
        manager
            .add_permission_rule("session-1", "deny", "Bash(just clean:*)")
            .await
            .unwrap();

        // just clean should be denied
        let decision = manager
            .check_permission(
                "session-1",
                "Bash",
                &serde_json::json!({"command": "just clean"}),
            )
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Deny);
    }

    #[tokio::test]
    async fn test_session_manager_permission_policy_session_not_found() {
        let manager = create_test_session_manager();

        let result = manager.get_permission_policy("non-existent").await;
        assert!(result.is_err());

        let result = manager
            .set_permission_policy("non-existent", PermissionPolicy::default())
            .await;
        assert!(result.is_err());

        let result = manager
            .check_permission(
                "non-existent",
                "Bash",
                &serde_json::json!({"command": "ls"}),
            )
            .await;
        assert!(result.is_err());

        let result = manager
            .add_permission_rule("non-existent", "allow", "Bash(*)")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_multiple_sessions_independent_permission_policies() {
        let manager = create_test_session_manager();

        // Create sessions with different permission policies
        let config1 = SessionConfig {
            permission_policy: Some(PermissionPolicy::new().allow("Bash(cargo:*)")),
            ..Default::default()
        };
        let config2 = SessionConfig {
            permission_policy: Some(PermissionPolicy::new().allow("Bash(npm:*)")),
            ..Default::default()
        };

        manager
            .create_session("session-1".to_string(), config1)
            .await
            .unwrap();
        manager
            .create_session("session-2".to_string(), config2)
            .await
            .unwrap();

        // Session 1 allows cargo, not npm
        let decision = manager
            .check_permission(
                "session-1",
                "Bash",
                &serde_json::json!({"command": "cargo build"}),
            )
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Allow);

        let decision = manager
            .check_permission(
                "session-1",
                "Bash",
                &serde_json::json!({"command": "npm install"}),
            )
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Ask);

        // Session 2 allows npm, not cargo
        let decision = manager
            .check_permission(
                "session-2",
                "Bash",
                &serde_json::json!({"command": "npm install"}),
            )
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Allow);

        let decision = manager
            .check_permission(
                "session-2",
                "Bash",
                &serde_json::json!({"command": "cargo build"}),
            )
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Ask);
    }

    // ========================================================================
    // Session Persistence Tests
    // ========================================================================

    fn create_test_session_manager_with_store() -> SessionManager {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let store = Arc::new(MemorySessionStore::new());
        SessionManager::with_store(None, tool_executor, store)
    }

    #[tokio::test]
    async fn test_session_manager_with_persistence() {
        let manager = create_test_session_manager_with_store();

        let config = SessionConfig {
            name: "persistent-session".to_string(),
            system_prompt: Some("You are helpful.".to_string()),
            ..Default::default()
        };

        // Create session
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Verify session exists
        let session_lock = manager.get_session("session-1").await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.config.name, "persistent-session");
    }

    #[tokio::test]
    async fn test_session_to_session_data() {
        let config = SessionConfig {
            name: "test".to_string(),
            system_prompt: Some("Hello".to_string()),
            ..Default::default()
        };
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Add some messages
        session.messages.push(Message::user("Hello"));

        // Convert to SessionData
        let data = session.to_session_data(None);

        assert_eq!(data.id, "test-1");
        assert_eq!(data.config.name, "test");
        assert_eq!(data.messages.len(), 1);
        assert!(data.llm_config.is_none());
    }

    #[tokio::test]
    async fn test_session_to_session_data_with_llm_config() {
        let config = SessionConfig::default();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        let llm_config = LlmConfigData {
            provider: "anthropic".to_string(),
            model: "claude-3-5-sonnet".to_string(),
            api_key: None,
            base_url: None,
        };

        let data = session.to_session_data(Some(llm_config));

        assert!(data.llm_config.is_some());
        let llm = data.llm_config.unwrap();
        assert_eq!(llm.provider, "anthropic");
        assert_eq!(llm.model, "claude-3-5-sonnet");
    }

    #[tokio::test]
    async fn test_session_restore_from_data() {
        let config = SessionConfig::default();
        let mut session = Session::new("test-1".to_string(), config.clone(), vec![])
            .await
            .unwrap();

        // Create data with different state
        let data = SessionData {
            id: "test-1".to_string(),
            config,
            state: SessionState::Paused,
            messages: vec![Message::user("Restored message")],
            context_usage: ContextUsage {
                used_tokens: 100,
                max_tokens: 200000,
                percent: 0.0005,
                turns: 1,
            },
            total_usage: TokenUsage {
                prompt_tokens: 50,
                completion_tokens: 50,
                total_tokens: 100,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            tool_names: vec![],
            thinking_enabled: true,
            thinking_budget: Some(1000),
            created_at: 1700000000,
            updated_at: 1700000100,
            llm_config: None,
            todos: vec![],
            parent_id: None,
        };

        // Restore
        session.restore_from_data(&data);

        // Verify
        assert_eq!(session.state, SessionState::Paused);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.context_usage.used_tokens, 100);
        assert!(session.thinking_enabled);
        assert_eq!(session.thinking_budget, Some(1000));
        assert_eq!(session.created_at, 1700000000);
    }

    #[tokio::test]
    async fn test_session_manager_persistence_on_pause_resume() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let store = Arc::new(MemorySessionStore::new());
        let manager = SessionManager::with_store(None, tool_executor, store.clone());

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Pause should persist
        manager.pause_session("session-1").await.unwrap();

        // Check store
        let stored = store.load("session-1").await.unwrap().unwrap();
        assert_eq!(stored.state, SessionState::Paused);

        // Resume should persist
        manager.resume_session("session-1").await.unwrap();

        let stored = store.load("session-1").await.unwrap().unwrap();
        assert_eq!(stored.state, SessionState::Active);
    }

    #[tokio::test]
    async fn test_session_manager_persistence_on_clear() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let store = Arc::new(MemorySessionStore::new());
        let manager = SessionManager::with_store(None, tool_executor, store.clone());

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Add a message manually for testing
        {
            let session_lock = manager.get_session("session-1").await.unwrap();
            let mut session = session_lock.write().await;
            session.messages.push(Message::user("Test message"));
        }

        // Clear should persist
        manager.clear("session-1").await.unwrap();

        // Check store
        let stored = store.load("session-1").await.unwrap().unwrap();
        assert!(stored.messages.is_empty());
    }

    #[tokio::test]
    async fn test_session_manager_persistence_on_destroy() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let store = Arc::new(MemorySessionStore::new());
        let manager = SessionManager::with_store(None, tool_executor, store.clone());

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Verify exists in store
        assert!(store.exists("session-1").await.unwrap());

        // Destroy should delete from store
        manager.destroy_session("session-1").await.unwrap();

        // Verify deleted from store
        assert!(!store.exists("session-1").await.unwrap());
    }

    #[tokio::test]
    async fn test_session_manager_persistence_on_policy_change() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let store = Arc::new(MemorySessionStore::new());
        let manager = SessionManager::with_store(None, tool_executor, store.clone());

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Set confirmation policy
        let policy = ConfirmationPolicy::enabled().with_yolo_lanes([SessionLane::Query]);
        manager
            .set_confirmation_policy("session-1", policy)
            .await
            .unwrap();

        // Check store
        let stored = store.load("session-1").await.unwrap().unwrap();
        let stored_policy = stored.config.confirmation_policy.unwrap();
        assert!(stored_policy.enabled);
        assert!(stored_policy.yolo_lanes.contains(&SessionLane::Query));
    }

    #[tokio::test]
    async fn test_session_manager_no_store_no_error() {
        // Manager without store should work fine
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // All operations should succeed without persistence
        manager.pause_session("session-1").await.unwrap();
        manager.resume_session("session-1").await.unwrap();
        manager.clear("session-1").await.unwrap();
        manager.destroy_session("session-1").await.unwrap();
    }

    // ========================================================================
    // Context Provider Tests
    // ========================================================================

    use crate::context::{ContextItem, ContextProvider, ContextQuery, ContextResult, ContextType};

    /// Mock context provider for testing
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
    async fn test_session_context_providers_default() {
        let config = SessionConfig::default();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(session.context_providers.is_empty());
        assert!(session.context_provider_names().is_empty());
    }

    #[tokio::test]
    async fn test_session_add_context_provider() {
        let config = SessionConfig::default();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        let provider = Arc::new(MockContextProvider::new("test-provider"));
        session.add_context_provider(provider);

        assert_eq!(session.context_providers.len(), 1);
        assert_eq!(session.context_provider_names(), vec!["test-provider"]);
    }

    #[tokio::test]
    async fn test_session_add_multiple_context_providers() {
        let config = SessionConfig::default();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        session.add_context_provider(Arc::new(MockContextProvider::new("provider-1")));
        session.add_context_provider(Arc::new(MockContextProvider::new("provider-2")));
        session.add_context_provider(Arc::new(MockContextProvider::new("provider-3")));

        assert_eq!(session.context_providers.len(), 3);
        let names = session.context_provider_names();
        assert!(names.contains(&"provider-1".to_string()));
        assert!(names.contains(&"provider-2".to_string()));
        assert!(names.contains(&"provider-3".to_string()));
    }

    #[tokio::test]
    async fn test_session_remove_context_provider() {
        let config = SessionConfig::default();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        session.add_context_provider(Arc::new(MockContextProvider::new("keep")));
        session.add_context_provider(Arc::new(MockContextProvider::new("remove")));

        assert_eq!(session.context_providers.len(), 2);

        // Remove provider
        let removed = session.remove_context_provider("remove");
        assert!(removed);
        assert_eq!(session.context_providers.len(), 1);
        assert_eq!(session.context_provider_names(), vec!["keep"]);

        // Try to remove non-existent provider
        let removed = session.remove_context_provider("non-existent");
        assert!(!removed);
        assert_eq!(session.context_providers.len(), 1);
    }

    #[tokio::test]
    async fn test_session_manager_add_context_provider() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Initially no providers
        let names = manager.list_context_providers("session-1").await.unwrap();
        assert!(names.is_empty());

        // Add provider
        let provider =
            Arc::new(
                MockContextProvider::new("test-provider").with_items(vec![ContextItem::new(
                    "item-1",
                    ContextType::Resource,
                    "Test content",
                )]),
            );
        manager
            .add_context_provider("session-1", provider)
            .await
            .unwrap();

        // Now has provider
        let names = manager.list_context_providers("session-1").await.unwrap();
        assert_eq!(names, vec!["test-provider"]);
    }

    #[tokio::test]
    async fn test_session_manager_remove_context_provider() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config)
            .await
            .unwrap();

        // Add providers
        manager
            .add_context_provider("session-1", Arc::new(MockContextProvider::new("p1")))
            .await
            .unwrap();
        manager
            .add_context_provider("session-1", Arc::new(MockContextProvider::new("p2")))
            .await
            .unwrap();

        assert_eq!(
            manager
                .list_context_providers("session-1")
                .await
                .unwrap()
                .len(),
            2
        );

        // Remove one
        let removed = manager
            .remove_context_provider("session-1", "p1")
            .await
            .unwrap();
        assert!(removed);

        let names = manager.list_context_providers("session-1").await.unwrap();
        assert_eq!(names, vec!["p2"]);

        // Remove non-existent
        let removed = manager
            .remove_context_provider("session-1", "non-existent")
            .await
            .unwrap();
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_session_manager_context_provider_session_not_found() {
        let manager = create_test_session_manager();

        let result = manager.list_context_providers("non-existent").await;
        assert!(result.is_err());

        let result = manager
            .add_context_provider("non-existent", Arc::new(MockContextProvider::new("p")))
            .await;
        assert!(result.is_err());

        let result = manager.remove_context_provider("non-existent", "p").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_multiple_sessions_independent_context_providers() {
        let manager = create_test_session_manager();

        let config = SessionConfig::default();
        manager
            .create_session("session-1".to_string(), config.clone())
            .await
            .unwrap();
        manager
            .create_session("session-2".to_string(), config)
            .await
            .unwrap();

        // Add different providers to each session
        manager
            .add_context_provider(
                "session-1",
                Arc::new(MockContextProvider::new("provider-for-1")),
            )
            .await
            .unwrap();
        manager
            .add_context_provider(
                "session-2",
                Arc::new(MockContextProvider::new("provider-for-2")),
            )
            .await
            .unwrap();

        // Verify independence
        let names1 = manager.list_context_providers("session-1").await.unwrap();
        let names2 = manager.list_context_providers("session-2").await.unwrap();

        assert_eq!(names1, vec!["provider-for-1"]);
        assert_eq!(names2, vec!["provider-for-2"]);
    }

    // ========================================================================
    // Cancellation Tests
    // ========================================================================

    #[tokio::test]
    async fn test_cancel_operation_no_ongoing() {
        let manager = create_test_session_manager();
        let config = SessionConfig::default();
        manager
            .create_session("test-session".to_string(), config)
            .await
            .unwrap();

        // Cancel when no operation is running
        let result = manager.cancel_operation("test-session").await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // No operation was cancelled
    }

    #[tokio::test]
    async fn test_cancel_operation_session_not_found() {
        let manager = create_test_session_manager();

        let result = manager.cancel_operation("non-existent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cancel_operation_with_pending_confirmations() {
        let manager = create_test_session_manager();
        let config = SessionConfig {
            confirmation_policy: Some(ConfirmationPolicy::enabled()),
            ..Default::default()
        };
        manager
            .create_session("test-session".to_string(), config)
            .await
            .unwrap();

        // Add a pending confirmation
        let session_lock = manager.get_session("test-session").await.unwrap();
        {
            let session = session_lock.read().await;
            let args = serde_json::json!({});
            session
                .confirmation_manager
                .request_confirmation("tool-1", "test_tool", &args)
                .await;
        }

        // Cancel should cancel the pending confirmation
        let result = manager.cancel_operation("test-session").await;
        assert!(result.is_ok());
        assert!(result.unwrap()); // Confirmation was cancelled
    }

    // ========================================================================
    // Context Compaction Tests
    // ========================================================================

    #[tokio::test]
    async fn test_compact_not_needed() {
        let config = SessionConfig::default();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Add only a few messages (less than threshold)
        for i in 0..10 {
            session
                .messages
                .push(Message::user(&format!("Message {}", i)));
        }

        // Create a mock LLM client that should NOT be called
        struct NeverCalledLlmClient;

        #[async_trait::async_trait]
        impl LlmClient for NeverCalledLlmClient {
            async fn complete(
                &self,
                _messages: &[Message],
                _system: Option<&str>,
                _tools: &[ToolDefinition],
            ) -> anyhow::Result<crate::llm::LlmResponse> {
                panic!("LLM should not be called when compaction is not needed");
            }

            async fn complete_streaming(
                &self,
                _messages: &[Message],
                _system: Option<&str>,
                _tools: &[ToolDefinition],
            ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
                panic!("LLM should not be called when compaction is not needed");
            }
        }

        let client: Arc<dyn LlmClient> = Arc::new(NeverCalledLlmClient);
        let result = session.compact(&client).await;
        assert!(result.is_ok());
        assert_eq!(session.messages.len(), 10); // Messages unchanged
    }

    #[tokio::test]
    async fn test_compact_with_many_messages() {
        let config = SessionConfig::default();
        let mut session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();

        // Add many messages (more than threshold of 30)
        for i in 0..50 {
            session
                .messages
                .push(Message::user(&format!("Message {}", i)));
        }

        // Create a mock LLM client that returns a summary
        struct MockSummaryLlmClient;

        #[async_trait::async_trait]
        impl LlmClient for MockSummaryLlmClient {
            async fn complete(
                &self,
                _messages: &[Message],
                _system: Option<&str>,
                _tools: &[ToolDefinition],
            ) -> anyhow::Result<crate::llm::LlmResponse> {
                Ok(crate::llm::LlmResponse {
                    message: Message {
                        role: "assistant".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "This is a summary of the conversation.".to_string(),
                        }],
                    },
                    usage: crate::llm::TokenUsage::default(),
                    stop_reason: Some("end_turn".to_string()),
                })
            }

            async fn complete_streaming(
                &self,
                _messages: &[Message],
                _system: Option<&str>,
                _tools: &[ToolDefinition],
            ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
                let (tx, rx) = mpsc::channel(1);
                drop(tx);
                Ok(rx)
            }
        }

        let client: Arc<dyn LlmClient> = Arc::new(MockSummaryLlmClient);
        let result = session.compact(&client).await;
        assert!(result.is_ok());

        // Should have: 2 initial + 1 summary + 20 recent = 23 messages
        assert_eq!(session.messages.len(), 23);

        // Check that the summary message is present
        let summary_msg = &session.messages[2];
        assert!(summary_msg.text().contains("[Context Summary:"));
    }

    // ========================================================================
    // Child Session Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_is_child_session() {
        // Create a regular session (no parent)
        let config = SessionConfig::default();
        let session = Session::new("test-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(!session.is_child_session());
        assert!(session.parent_session_id().is_none());

        // Create a child session (with parent)
        let child_config = SessionConfig {
            parent_id: Some("parent-1".to_string()),
            ..Default::default()
        };
        let child_session = Session::new("child-1".to_string(), child_config, vec![])
            .await
            .unwrap();
        assert!(child_session.is_child_session());
        assert_eq!(child_session.parent_session_id(), Some("parent-1"));
    }

    #[tokio::test]
    async fn test_session_manager_create_child_session() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        // Create parent session
        let parent_config = SessionConfig::default();
        manager
            .create_session("parent-1".to_string(), parent_config)
            .await
            .unwrap();

        // Create child session
        let child_config = SessionConfig {
            name: "Child Session".to_string(),
            ..Default::default()
        };
        let child_id = manager
            .create_child_session("parent-1", "child-1".to_string(), child_config)
            .await
            .unwrap();

        assert_eq!(child_id, "child-1");

        // Verify child session has parent_id set
        let child_lock = manager.get_session("child-1").await.unwrap();
        let child = child_lock.read().await;
        assert!(child.is_child_session());
        assert_eq!(child.parent_session_id(), Some("parent-1"));
    }

    #[tokio::test]
    async fn test_session_manager_get_child_sessions() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        // Create parent session
        let parent_config = SessionConfig::default();
        manager
            .create_session("parent-1".to_string(), parent_config)
            .await
            .unwrap();

        // Create multiple child sessions
        for i in 1..=3 {
            let child_config = SessionConfig::default();
            manager
                .create_child_session("parent-1", format!("child-{}", i), child_config)
                .await
                .unwrap();
        }

        // Get child sessions
        let children = manager.get_child_sessions("parent-1").await;
        assert_eq!(children.len(), 3);
        assert!(children.contains(&"child-1".to_string()));
        assert!(children.contains(&"child-2".to_string()));
        assert!(children.contains(&"child-3".to_string()));

        // Non-existent parent should return empty list
        let no_children = manager.get_child_sessions("nonexistent").await;
        assert!(no_children.is_empty());
    }

    #[tokio::test]
    async fn test_session_manager_is_child_session() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        // Create parent session
        let parent_config = SessionConfig::default();
        manager
            .create_session("parent-1".to_string(), parent_config)
            .await
            .unwrap();

        // Create child session
        let child_config = SessionConfig::default();
        manager
            .create_child_session("parent-1", "child-1".to_string(), child_config)
            .await
            .unwrap();

        // Check is_child_session
        assert!(!manager.is_child_session("parent-1").await.unwrap());
        assert!(manager.is_child_session("child-1").await.unwrap());
    }

    #[tokio::test]
    async fn test_session_manager_create_child_session_parent_not_found() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        // Try to create child session with non-existent parent
        let child_config = SessionConfig::default();
        let result = manager
            .create_child_session("nonexistent", "child-1".to_string(), child_config)
            .await;

        assert!(result.is_err());
    }

    // ========================================================================
    // LLM Resolution Tests
    // ========================================================================

    #[tokio::test]
    async fn test_get_llm_for_session_no_client() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let config = SessionConfig::default();
        manager
            .create_session("test-1".to_string(), config)
            .await
            .unwrap();

        // No LLM client configured at any level
        let result = manager.get_llm_for_session("test-1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_llm_for_session_default_client() {
        struct DummyLlmClient;

        #[async_trait::async_trait]
        impl LlmClient for DummyLlmClient {
            async fn complete(
                &self,
                _messages: &[Message],
                _system: Option<&str>,
                _tools: &[crate::llm::ToolDefinition],
            ) -> anyhow::Result<crate::llm::LlmResponse> {
                unimplemented!()
            }

            async fn complete_streaming(
                &self,
                _messages: &[Message],
                _system: Option<&str>,
                _tools: &[crate::llm::ToolDefinition],
            ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
                unimplemented!()
            }
        }

        let client: Arc<dyn LlmClient> = Arc::new(DummyLlmClient);
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(Some(client), tool_executor);

        let config = SessionConfig::default();
        manager
            .create_session("test-1".to_string(), config)
            .await
            .unwrap();

        // Should resolve to default client
        let result = manager.get_llm_for_session("test-1").await.unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_get_llm_for_session_not_found() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let result = manager.get_llm_for_session("nonexistent").await;
        assert!(result.is_err());
    }
}
