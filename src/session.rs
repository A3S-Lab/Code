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
//! Skills are managed at the service level via the skill registry.
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

/// Default auto-compact threshold (80% of context window)
pub const DEFAULT_AUTO_COMPACT_THRESHOLD: f32 = 0.80;

/// Serde default function for auto_compact_threshold
fn default_auto_compact_threshold() -> f32 {
    DEFAULT_AUTO_COMPACT_THRESHOLD
}

/// Session configuration (matches proto SessionConfig)
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
    /// Security configuration (optional, enables security features)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_config: Option<crate::security::SecurityConfig>,
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
        }
    }
}

#[allow(dead_code)]
pub struct Session {
    pub id: String,
    pub config: SessionConfig,
    pub state: SessionState,
    pub messages: Vec<Message>,
    pub context_usage: ContextUsage,
    pub total_usage: TokenUsage,
    /// Cumulative dollar cost for this session
    pub total_cost: f64,
    /// Model name for cost calculation
    pub model_name: Option<String>,
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
    /// Security guard (if enabled)
    pub security_guard: Option<Arc<crate::security::SecurityGuard>>,
    /// Per-session tool execution metrics
    pub tool_metrics: Arc<RwLock<crate::telemetry::ToolMetrics>>,
    /// Per-call LLM cost records for cross-session aggregation
    pub cost_records: Vec<crate::telemetry::LlmCostRecord>,
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

        // Create confirmation manager with policy or secure default (HITL enabled)
        let confirmation_policy = config
            .confirmation_policy
            .clone()
            .unwrap_or_else(ConfirmationPolicy::enabled);
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

        // Initialize security guard if configured
        let security_guard = config.security_config.as_ref().and_then(|sc| {
            if sc.enabled {
                Some(Arc::new(crate::security::SecurityGuard::new(
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
            total_cost: 0.0,
            model_name: None,
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
            security_guard,
            tool_metrics: Arc::new(RwLock::new(crate::telemetry::ToolMetrics::new())),
            cost_records: Vec::new(),
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

        // Calculate cost if model pricing is available
        let cost_usd = if let Some(ref model) = self.model_name {
            let pricing_map = crate::telemetry::default_model_pricing();
            if let Some(pricing) = pricing_map.get(model) {
                let cost = pricing.calculate_cost(usage.prompt_tokens, usage.completion_tokens);
                self.total_cost += cost;
                Some(cost)
            } else {
                None
            }
        } else {
            None
        };

        // Record per-call cost for aggregation
        let model_str = self.model_name.clone().unwrap_or_default();
        self.cost_records.push(crate::telemetry::LlmCostRecord {
            model: model_str.clone(),
            provider: String::new(),
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            cost_usd,
            timestamp: chrono::Utc::now(),
            session_id: Some(self.id.clone()),
        });

        // Record OTLP metrics (counters only; duration recorded in agent loop)
        crate::telemetry::record_llm_metrics(
            if model_str.is_empty() { "unknown" } else { &model_str },
            usage.prompt_tokens,
            usage.completion_tokens,
            cost_usd.unwrap_or(0.0),
            0.0, // Duration not available here; recorded via spans
        );

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

        let summarization_prompt = crate::prompts::render(
            crate::prompts::CONTEXT_COMPACT,
            &[("conversation", &conversation_text)],
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
                    "{}{}",
                    crate::prompts::CONTEXT_SUMMARY_PREFIX,
                    summary_text
                ),
            }],
            reasoning_content: None,
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
            total_cost: self.total_cost,
            model_name: self.model_name.clone(),
            cost_records: self.cost_records.clone(),
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
        self.total_cost = data.total_cost;
        self.model_name = data.model_name.clone();
        self.cost_records = data.cost_records.clone();
        self.thinking_enabled = data.thinking_enabled;
        self.thinking_budget = data.thinking_budget;
        self.created_at = data.created_at;
        self.updated_at = data.updated_at;
        self.todos = data.todos.clone();
        self.parent_id = data.parent_id.clone();
    }
}

/// Session manager handles multiple concurrent sessions
#[derive(Clone)]
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
                    tracing::warn!("Failed to load session {} from {:?}: {}", session_id, backend, e);
                    continue;
                }
            }
        }

        Err(anyhow::anyhow!("Session {} not found in any store", session_id))
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
        if let Err(e) = self.save_session(&id).await {
            tracing::warn!("Failed to persist session {}: {}", id, e);
        }

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
            session_workspace,
            tool_metrics,
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
        } else {
            crate::tools::ToolContext::new(std::path::PathBuf::from(&session_workspace))
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
            skill_tool_filters: Vec::new(),
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
        if let Err(e) = self.save_session(session_id).await {
            tracing::warn!(
                "Failed to persist session {} after generate: {}",
                session_id,
                e
            );
        }

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
        } else {
            crate::tools::ToolContext::new(std::path::PathBuf::from(&session_workspace))
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
            skill_tool_filters: Vec::new(),
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

    /// Fork a session, creating a new session with copied history and configuration
    ///
    /// The forked session gets:
    /// - A new unique ID
    /// - Copied conversation history (messages)
    /// - Copied configuration (with optional name override)
    /// - Copied usage statistics and cost
    /// - Copied todos
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
            source_todos,
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
                session.todos.clone(),
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
        new_session.todos = source_todos;
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
        if let Err(e) = self.save_session(&new_id).await {
            tracing::warn!("Failed to persist forked session {}: {}", new_id, e);
        }

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
        if let Err(e) = self.save_session(session_id).await {
            tracing::warn!(
                "Failed to persist session {} after title generation: {}",
                session_id,
                e
            );
        }

        tracing::info!("Generated title for session '{}': '{}'", session_id, title);
        Ok(Some(title))
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
            auto_compact_threshold: DEFAULT_AUTO_COMPACT_THRESHOLD,
            storage_type: crate::config::StorageBackend::Memory,
            queue_config: None,
            confirmation_policy: None,
            permission_policy: None,
            parent_id: None,
            security_config: None,
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

        // Default policy (HITL enabled — secure default)
        let policy = session.confirmation_policy().await;
        assert!(policy.enabled);

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

        // Get default policy (HITL enabled — secure default)
        let policy = manager.get_confirmation_policy("session-1").await.unwrap();
        assert!(policy.enabled);

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
        SessionManager::with_store(None, tool_executor, store, crate::config::StorageBackend::File)
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
            total_cost: 0.0,
            model_name: None,
            cost_records: Vec::new(),
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
        let manager = SessionManager::with_store(None, tool_executor, store.clone(), crate::config::StorageBackend::File);

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
        let manager = SessionManager::with_store(None, tool_executor, store.clone(), crate::config::StorageBackend::File);

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
        let manager = SessionManager::with_store(None, tool_executor, store.clone(), crate::config::StorageBackend::File);

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
        let manager = SessionManager::with_store(None, tool_executor, store.clone(), crate::config::StorageBackend::File);

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
                        reasoning_content: None,
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

    // ========================================================================
    // Auto-Compact Tests
    // ========================================================================

    #[test]
    fn test_session_config_default_auto_compact_threshold() {
        let config = SessionConfig::default();
        assert!(!config.auto_compact);
        assert_eq!(
            config.auto_compact_threshold,
            DEFAULT_AUTO_COMPACT_THRESHOLD
        );
        assert_eq!(config.auto_compact_threshold, 0.80);
    }

    #[test]
    fn test_session_config_custom_auto_compact_threshold() {
        let config = SessionConfig {
            auto_compact: true,
            auto_compact_threshold: 0.90,
            ..Default::default()
        };
        assert!(config.auto_compact);
        assert_eq!(config.auto_compact_threshold, 0.90);
    }

    #[test]
    fn test_session_config_auto_compact_threshold_serde() {
        // Serialize with threshold
        let config = SessionConfig {
            auto_compact: true,
            auto_compact_threshold: 0.75,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: SessionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.auto_compact_threshold, 0.75);

        // Deserialize without threshold (should use default)
        let json_no_threshold = r#"{"name":"","workspace":"","system_prompt":null,"max_context_length":0,"auto_compact":true}"#;
        let parsed: SessionConfig = serde_json::from_str(json_no_threshold).unwrap();
        assert_eq!(
            parsed.auto_compact_threshold,
            DEFAULT_AUTO_COMPACT_THRESHOLD
        );
    }

    #[tokio::test]
    async fn test_maybe_auto_compact_disabled() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let config = SessionConfig {
            auto_compact: false,
            ..Default::default()
        };
        manager
            .create_session("test-1".to_string(), config)
            .await
            .unwrap();

        // Should return false (auto_compact disabled)
        let result = manager.maybe_auto_compact("test-1").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_maybe_auto_compact_below_threshold() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let config = SessionConfig {
            auto_compact: true,
            auto_compact_threshold: 0.80,
            ..Default::default()
        };
        manager
            .create_session("test-1".to_string(), config)
            .await
            .unwrap();

        // Set context usage below threshold
        {
            let session_lock = manager.get_session("test-1").await.unwrap();
            let mut session = session_lock.write().await;
            session.context_usage.percent = 0.50; // 50% < 80%
        }

        // Should return false (below threshold)
        let result = manager.maybe_auto_compact("test-1").await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_maybe_auto_compact_triggers_at_threshold() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let config = SessionConfig {
            auto_compact: true,
            auto_compact_threshold: 0.80,
            ..Default::default()
        };
        manager
            .create_session("test-1".to_string(), config)
            .await
            .unwrap();

        // Add enough messages and set high context usage
        {
            let session_lock = manager.get_session("test-1").await.unwrap();
            let mut session = session_lock.write().await;
            // Add 35 messages (above MIN_MESSAGES_FOR_COMPACTION = 30)
            for i in 0..35 {
                session.messages.push(Message::user(&format!("msg {}", i)));
            }
            session.context_usage.percent = 0.85; // 85% > 80%
            session.context_usage.used_tokens = 170_000;
            session.context_usage.max_tokens = 200_000;
        }

        // Should trigger compaction (no LLM client, so it falls back to simple truncation)
        let result = manager.maybe_auto_compact("test-1").await.unwrap();
        assert!(result);

        // Verify messages were reduced
        let session_lock = manager.get_session("test-1").await.unwrap();
        let session = session_lock.read().await;
        assert!(session.messages.len() < 35);
    }

    #[tokio::test]
    async fn test_maybe_auto_compact_session_not_found() {
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let manager = SessionManager::new(None, tool_executor);

        let result = manager.maybe_auto_compact("nonexistent").await;
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod extra_session_tests {
    use super::*;
    use crate::store::MemorySessionStore;
    use crate::todo::{Todo, TodoPriority, TodoStatus};

    fn default_config() -> SessionConfig {
        SessionConfig::default()
    }

    async fn make_session(id: &str) -> Session {
        Session::new(id.to_string(), default_config(), vec![])
            .await
            .unwrap()
    }

    fn make_manager() -> SessionManager {
        let store = Arc::new(MemorySessionStore::new());
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        SessionManager::with_store(None, tool_executor, store, crate::config::StorageBackend::File)
    }

    // ========================================================================
    // Session state transitions
    // ========================================================================

    #[tokio::test]
    async fn test_session_set_error() {
        let mut session = make_session("s1").await;
        assert_eq!(session.state, SessionState::Active);
        session.set_error();
        assert_eq!(session.state, SessionState::Error);
    }

    #[tokio::test]
    async fn test_session_set_completed() {
        let mut session = make_session("s1").await;
        session.set_completed();
        assert_eq!(session.state, SessionState::Completed);
    }

    #[tokio::test]
    async fn test_session_set_completed_stops_queue() {
        let config = SessionConfig {
            queue_config: Some(crate::queue::SessionQueueConfig::default()),
            ..Default::default()
        };
        let mut session = Session::new("s1".to_string(), config, vec![])
            .await
            .unwrap();
        session.start_queue().await.unwrap();
        session.set_completed();
        assert_eq!(session.state, SessionState::Completed);
    }

    // ========================================================================
    // Session message management
    // ========================================================================

    #[tokio::test]
    async fn test_session_add_message() {
        let mut session = make_session("s1").await;
        assert!(session.history().is_empty());

        session.add_message(Message::user("Hello"));
        assert_eq!(session.history().len(), 1);
        assert_eq!(session.context_usage.turns, 1);

        session.add_message(Message::user("World"));
        assert_eq!(session.history().len(), 2);
        assert_eq!(session.context_usage.turns, 2);
    }

    #[tokio::test]
    async fn test_session_update_usage() {
        let mut session = make_session("s1").await;
        let usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        session.update_usage(&usage);
        assert_eq!(session.total_usage.prompt_tokens, 100);
        assert_eq!(session.total_usage.completion_tokens, 50);
        assert_eq!(session.total_usage.total_tokens, 150);
        assert_eq!(session.context_usage.used_tokens, 100);
        assert!(session.context_usage.percent > 0.0);

        // Update again - should accumulate
        session.update_usage(&usage);
        assert_eq!(session.total_usage.prompt_tokens, 200);
        assert_eq!(session.total_usage.total_tokens, 300);
    }

    #[tokio::test]
    async fn test_session_clear() {
        let mut session = make_session("s1").await;
        session.add_message(Message::user("Hello"));
        session.add_message(Message::user("World"));
        assert_eq!(session.history().len(), 2);

        session.clear();
        assert!(session.history().is_empty());
        assert_eq!(session.context_usage.used_tokens, 0);
        assert_eq!(session.context_usage.turns, 0);
    }

    // ========================================================================
    // Session todos
    // ========================================================================

    #[tokio::test]
    async fn test_session_todos() {
        let mut session = make_session("s1").await;
        assert!(session.get_todos().is_empty());

        let todos = vec![Todo::new("t1", "Fix bug"), Todo::new("t2", "Write tests")];
        session.set_todos(todos);
        assert_eq!(session.get_todos().len(), 2);
    }

    #[tokio::test]
    async fn test_session_active_todo_count() {
        let mut session = make_session("s1").await;
        let todos = vec![
            Todo {
                id: "t1".to_string(),
                content: "Fix bug".to_string(),
                status: TodoStatus::Pending,
                priority: TodoPriority::High,
            },
            Todo {
                id: "t2".to_string(),
                content: "Write tests".to_string(),
                status: TodoStatus::InProgress,
                priority: TodoPriority::Medium,
            },
            Todo {
                id: "t3".to_string(),
                content: "Done task".to_string(),
                status: TodoStatus::Completed,
                priority: TodoPriority::Low,
            },
            Todo {
                id: "t4".to_string(),
                content: "Cancelled".to_string(),
                status: TodoStatus::Cancelled,
                priority: TodoPriority::Low,
            },
        ];
        session.set_todos(todos);
        // Active = Pending + InProgress
        assert_eq!(session.active_todo_count(), 2);
    }

    // ========================================================================
    // SessionState conversions
    // ========================================================================

    #[test]
    fn test_session_state_all_conversions() {
        assert_eq!(
            SessionState::from_proto_i32(SessionState::Active.to_proto_i32()),
            SessionState::Active
        );
        assert_eq!(
            SessionState::from_proto_i32(SessionState::Paused.to_proto_i32()),
            SessionState::Paused
        );
        assert_eq!(
            SessionState::from_proto_i32(SessionState::Completed.to_proto_i32()),
            SessionState::Completed
        );
        assert_eq!(
            SessionState::from_proto_i32(SessionState::Error.to_proto_i32()),
            SessionState::Error
        );
    }

    #[test]
    fn test_session_state_unknown_defaults_unknown() {
        assert_eq!(SessionState::from_proto_i32(999), SessionState::Unknown);
    }

    // ========================================================================
    // Session system prompt
    // ========================================================================

    #[tokio::test]
    async fn test_session_system_prompt() {
        let config = SessionConfig {
            system_prompt: Some("You are helpful".to_string()),
            ..Default::default()
        };
        let session = Session::new("s1".to_string(), config, vec![])
            .await
            .unwrap();
        assert_eq!(session.system(), Some("You are helpful"));
    }

    #[tokio::test]
    async fn test_session_no_system_prompt() {
        let session = make_session("s1").await;
        assert!(session.system().is_none());
    }

    // ========================================================================
    // Session child/parent
    // ========================================================================

    #[tokio::test]
    async fn test_session_not_child() {
        let session = make_session("s1").await;
        assert!(!session.is_child_session());
        assert!(session.parent_session_id().is_none());
    }

    #[tokio::test]
    async fn test_session_is_child() {
        let config = SessionConfig {
            parent_id: Some("parent-1".to_string()),
            ..Default::default()
        };
        let session = Session::new("child-1".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(session.is_child_session());
        assert_eq!(session.parent_session_id(), Some("parent-1"));
    }

    // ========================================================================
    // SessionManager - session access via get_session
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_add_and_read_messages() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        // Add messages via get_session
        {
            let session_lock = sm.get_session(&id).await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("Hello"));
        }

        // Read messages
        let session_lock = sm.get_session(&id).await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.history().len(), 1);
    }

    #[tokio::test]
    async fn test_session_manager_context_usage_via_session() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let session_lock = sm.get_session(&id).await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.context_usage.used_tokens, 0);
        assert_eq!(session.context_usage.turns, 0);
    }

    #[tokio::test]
    async fn test_session_manager_clear_via_session() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        // Add then clear
        {
            let session_lock = sm.get_session(&id).await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("Hello"));
            session.add_message(Message::user("World"));
            assert_eq!(session.history().len(), 2);
            session.clear();
        }

        let session_lock = sm.get_session(&id).await.unwrap();
        let session = session_lock.read().await;
        assert!(session.history().is_empty());
    }

    // ========================================================================
    // SessionManager - configure
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_configure_thinking() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        sm.configure(&id, Some(true), Some(1000), None)
            .await
            .unwrap();

        let session_lock = sm.get_session(&id).await.unwrap();
        let session = session_lock.read().await;
        assert!(session.thinking_enabled);
        assert_eq!(session.thinking_budget, Some(1000));
    }

    #[tokio::test]
    async fn test_session_manager_configure_with_llm() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let llm_config =
            crate::llm::LlmConfig::new("anthropic", "claude-sonnet-4-20250514", "sk-key");
        sm.configure(&id, None, None, Some(llm_config))
            .await
            .unwrap();

        // Verify LLM config was stored
        let configs = sm.llm_configs.read().await;
        assert!(configs.contains_key(&id));
    }

    #[tokio::test]
    async fn test_session_manager_configure_not_found() {
        let sm = make_manager();
        let result = sm.configure("nonexistent", Some(true), None, None).await;
        assert!(result.is_err());
    }

    // ========================================================================
    // SessionManager - get_session
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_get_session_ok() {
        let sm = make_manager();
        let id = sm
            .create_session("s1".to_string(), default_config())
            .await
            .unwrap();
        let session_lock = sm.get_session(&id).await;
        assert!(session_lock.is_ok());
    }

    #[tokio::test]
    async fn test_session_manager_get_session_not_found() {
        let sm = make_manager();
        let result = sm.get_session("nonexistent").await;
        assert!(result.is_err());
    }

    // ========================================================================
    // SessionManager - session_count
    // ========================================================================

    #[tokio::test]
    async fn test_session_manager_session_count() {
        let sm = make_manager();
        assert_eq!(sm.session_count().await, 0);
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();
        assert_eq!(sm.session_count().await, 1);
        sm.create_session("s2".to_string(), default_config())
            .await
            .unwrap();
        assert_eq!(sm.session_count().await, 2);
        sm.destroy_session("s1").await.unwrap();
        assert_eq!(sm.session_count().await, 1);
    }

    // ========================================================================
    // Session event subscription
    // ========================================================================

    #[tokio::test]
    async fn test_session_event_tx() {
        let session = make_session("s1").await;
        let tx = session.event_tx();
        let mut rx = session.subscribe_events();

        tx.send(AgentEvent::Start {
            prompt: "test".to_string(),
        })
        .unwrap();
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, AgentEvent::Start { .. }));
    }

    // ========================================================================
    // ContextUsage
    // ========================================================================

    #[test]
    fn test_context_usage_default_values() {
        let usage = ContextUsage::default();
        assert_eq!(usage.used_tokens, 0);
        assert_eq!(usage.max_tokens, 200_000);
        assert_eq!(usage.percent, 0.0);
        assert_eq!(usage.turns, 0);
    }

    // ========================================================================
    // SessionConfig default
    // ========================================================================

    #[test]
    fn test_session_config_default() {
        let config = SessionConfig::default();
        assert!(config.name.is_empty());
        assert!(config.workspace.is_empty());
        assert!(config.system_prompt.is_none());
        assert_eq!(config.max_context_length, 0);
        assert!(!config.auto_compact);
        assert_eq!(
            config.auto_compact_threshold,
            DEFAULT_AUTO_COMPACT_THRESHOLD
        );
        assert!(config.queue_config.is_none());
        assert!(config.confirmation_policy.is_none());
        assert!(config.permission_policy.is_none());
        assert!(config.parent_id.is_none());
    }

    // ========================================================================
    // Session Fork
    // ========================================================================

    #[tokio::test]
    async fn test_fork_session_basic() {
        let sm = make_manager();
        let mut config = default_config();
        config.name = "Original Session".to_string();
        sm.create_session("src".to_string(), config).await.unwrap();

        // Add messages to source
        {
            let session_lock = sm.get_session("src").await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("Hello"));
            session.add_message(Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Hi there!".to_string(),
                }],
                reasoning_content: None,
            });
        }

        // Fork
        let new_id = sm
            .fork_session("src", "forked".to_string(), None)
            .await
            .unwrap();
        assert_eq!(new_id, "forked");

        // Verify forked session exists
        let session_lock = sm.get_session("forked").await.unwrap();
        let session = session_lock.read().await;

        // Messages copied
        assert_eq!(session.messages.len(), 2);

        // Name has "(fork)" suffix
        assert_eq!(session.config.name, "Original Session (fork)");

        // Parent ID set
        assert_eq!(session.parent_id, Some("src".to_string()));

        // State is Active (fresh)
        assert_eq!(session.state, SessionState::Active);
    }

    #[tokio::test]
    async fn test_fork_session_with_custom_name() {
        let sm = make_manager();
        sm.create_session("src".to_string(), default_config())
            .await
            .unwrap();

        let new_id = sm
            .fork_session("src", "forked".to_string(), Some("My Fork".to_string()))
            .await
            .unwrap();
        assert_eq!(new_id, "forked");

        let session_lock = sm.get_session("forked").await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.config.name, "My Fork");
    }

    #[tokio::test]
    async fn test_fork_session_copies_usage() {
        let sm = make_manager();
        sm.create_session("src".to_string(), default_config())
            .await
            .unwrap();

        // Set usage on source
        {
            let session_lock = sm.get_session("src").await.unwrap();
            let mut session = session_lock.write().await;
            session.total_usage.prompt_tokens = 500;
            session.total_usage.completion_tokens = 200;
            session.total_usage.total_tokens = 700;
            session.total_cost = 0.05;
            session.model_name = Some("claude-sonnet-4-20250514".to_string());
        }

        sm.fork_session("src", "forked".to_string(), None)
            .await
            .unwrap();

        let session_lock = sm.get_session("forked").await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.total_usage.prompt_tokens, 500);
        assert_eq!(session.total_usage.completion_tokens, 200);
        assert_eq!(session.total_usage.total_tokens, 700);
        assert_eq!(session.total_cost, 0.05);
        assert_eq!(
            session.model_name,
            Some("claude-sonnet-4-20250514".to_string())
        );
    }

    #[tokio::test]
    async fn test_fork_session_copies_todos() {
        let sm = make_manager();
        sm.create_session("src".to_string(), default_config())
            .await
            .unwrap();

        // Add todos to source
        {
            let session_lock = sm.get_session("src").await.unwrap();
            let mut session = session_lock.write().await;
            session.todos.push(Todo {
                id: "t1".to_string(),
                content: "Fix bug".to_string(),
                status: TodoStatus::Pending,
                priority: TodoPriority::High,
            });
        }

        sm.fork_session("src", "forked".to_string(), None)
            .await
            .unwrap();

        let session_lock = sm.get_session("forked").await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.todos.len(), 1);
        assert_eq!(session.todos[0].content, "Fix bug");
    }

    #[tokio::test]
    async fn test_fork_session_source_not_found() {
        let sm = make_manager();
        let result = sm
            .fork_session("nonexistent", "forked".to_string(), None)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fork_session_independent_modification() {
        let sm = make_manager();
        sm.create_session("src".to_string(), default_config())
            .await
            .unwrap();

        // Add a message to source
        {
            let session_lock = sm.get_session("src").await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("Original message"));
        }

        // Fork
        sm.fork_session("src", "forked".to_string(), None)
            .await
            .unwrap();

        // Add message only to forked session
        {
            let session_lock = sm.get_session("forked").await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("Forked-only message"));
        }

        // Source should still have 1 message
        let src_lock = sm.get_session("src").await.unwrap();
        let src = src_lock.read().await;
        assert_eq!(src.messages.len(), 1);

        // Forked should have 2 messages
        let fork_lock = sm.get_session("forked").await.unwrap();
        let fork = fork_lock.read().await;
        assert_eq!(fork.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_fork_session_copies_thinking_config() {
        let sm = make_manager();
        sm.create_session("src".to_string(), default_config())
            .await
            .unwrap();

        {
            let session_lock = sm.get_session("src").await.unwrap();
            let mut session = session_lock.write().await;
            session.thinking_enabled = true;
            session.thinking_budget = Some(10000);
        }

        sm.fork_session("src", "forked".to_string(), None)
            .await
            .unwrap();

        let session_lock = sm.get_session("forked").await.unwrap();
        let session = session_lock.read().await;
        assert!(session.thinking_enabled);
        assert_eq!(session.thinking_budget, Some(10000));
    }

    #[tokio::test]
    async fn test_fork_session_fresh_timestamps() {
        let sm = make_manager();
        sm.create_session("src".to_string(), default_config())
            .await
            .unwrap();

        let src_created_at = {
            let session_lock = sm.get_session("src").await.unwrap();
            let session = session_lock.read().await;
            session.created_at
        };

        sm.fork_session("src", "forked".to_string(), None)
            .await
            .unwrap();

        let session_lock = sm.get_session("forked").await.unwrap();
        let session = session_lock.read().await;
        // Forked session should have its own created_at (>= source)
        assert!(session.created_at >= src_created_at);
    }

    // ========================================================================
    // Session Auto Title
    // ========================================================================

    #[tokio::test]
    async fn test_generate_title_no_messages() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        // No messages -> returns None
        let title = sm.generate_title("s1").await.unwrap();
        assert!(title.is_none());
    }

    #[tokio::test]
    async fn test_generate_title_no_llm_client() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        // Add a message
        {
            let session_lock = sm.get_session("s1").await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("Help me fix a Rust compilation error"));
        }

        // No LLM client -> returns None
        let title = sm.generate_title("s1").await.unwrap();
        assert!(title.is_none());
    }

    #[tokio::test]
    async fn test_generate_title_session_not_found() {
        let sm = make_manager();
        let result = sm.generate_title("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_generate_title_only_tool_messages() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        // Add only tool result messages (no text content)
        {
            let session_lock = sm.get_session("s1").await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::tool_result("t1", "some output", false));
        }

        // No text content -> returns None (no LLM client anyway)
        let title = sm.generate_title("s1").await.unwrap();
        assert!(title.is_none());
    }

    // ========================================================================
    // Additional Coverage Tests
    // ========================================================================

    #[test]
    fn test_session_state_unknown_to_proto() {
        assert_eq!(SessionState::Unknown.to_proto_i32(), 0);
    }

    #[test]
    fn test_session_state_from_proto_zero() {
        assert_eq!(SessionState::from_proto_i32(0), SessionState::Unknown);
    }

    #[test]
    fn test_session_state_from_proto_negative() {
        assert_eq!(SessionState::from_proto_i32(-1), SessionState::Unknown);
    }

    #[tokio::test]
    async fn test_session_context_provider_names() {
        let session = make_session("s1").await;
        assert!(session.context_provider_names().is_empty());
    }

    #[tokio::test]
    async fn test_session_remove_context_provider_not_found() {
        let mut session = make_session("s1").await;
        let removed = session.remove_context_provider("nonexistent");
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_session_update_usage_with_cost() {
        let mut session = make_session("s1").await;
        session.model_name = Some("claude-3-5-sonnet-20241022".to_string());

        let usage = TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };

        session.update_usage(&usage);
        assert_eq!(session.total_usage.prompt_tokens, 1000);
        assert_eq!(session.total_usage.completion_tokens, 500);
        assert!(session.total_cost > 0.0);
    }

    #[tokio::test]
    async fn test_session_update_usage_no_model() {
        let mut session = make_session("s1").await;
        session.model_name = None;

        let usage = TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };

        session.update_usage(&usage);
        assert_eq!(session.total_cost, 0.0);
    }

    #[tokio::test]
    async fn test_session_update_usage_unknown_model() {
        let mut session = make_session("s1").await;
        session.model_name = Some("unknown-model-xyz".to_string());

        let usage = TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };

        session.update_usage(&usage);
        assert_eq!(session.total_cost, 0.0);
    }

    #[tokio::test]
    async fn test_session_to_session_data() {
        let mut session = make_session("s1").await;
        session.add_message(Message::user("test"));
        session.model_name = Some("test-model".to_string());

        let data = session.to_session_data(None);
        assert_eq!(data.id, "s1");
        assert_eq!(data.messages.len(), 1);
        assert_eq!(data.model_name, Some("test-model".to_string()));
        assert!(data.llm_config.is_none());
    }

    #[tokio::test]
    async fn test_session_to_session_data_with_llm_config() {
        let session = make_session("s1").await;
        let llm_config = LlmConfigData {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            api_key: None,
            base_url: None,
        };

        let data = session.to_session_data(Some(llm_config.clone()));
        assert!(data.llm_config.is_some());
        assert_eq!(data.llm_config.unwrap().model, "claude-sonnet-4-20250514");
    }

    #[tokio::test]
    async fn test_session_restore_from_data() {
        let mut session = make_session("s1").await;

        let data = SessionData {
            id: "s1".to_string(),
            config: default_config(),
            state: SessionState::Paused,
            messages: vec![Message::user("restored")],
            context_usage: ContextUsage {
                used_tokens: 100,
                max_tokens: 200_000,
                percent: 0.0005,
                turns: 1,
            },
            total_usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            total_cost: 0.05,
            model_name: Some("test-model".to_string()),
            tool_names: vec![],
            thinking_enabled: true,
            thinking_budget: Some(5000),
            created_at: 1000,
            updated_at: 2000,
            llm_config: None,
            todos: vec![Todo::new("t1", "test todo")],
            parent_id: Some("parent".to_string()),
            cost_records: Vec::new(),
        };

        session.restore_from_data(&data);

        assert_eq!(session.state, SessionState::Paused);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.context_usage.used_tokens, 100);
        assert_eq!(session.total_usage.prompt_tokens, 100);
        assert_eq!(session.total_cost, 0.05);
        assert_eq!(session.model_name, Some("test-model".to_string()));
        assert!(session.thinking_enabled);
        assert_eq!(session.thinking_budget, Some(5000));
        assert_eq!(session.created_at, 1000);
        assert_eq!(session.updated_at, 2000);
        assert_eq!(session.todos.len(), 1);
        assert_eq!(session.parent_id, Some("parent".to_string()));
    }

    #[tokio::test]
    async fn test_session_manager_list_sessions() {
        let sm = make_manager();
        assert!(sm.list_sessions().await.is_empty());

        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();
        sm.create_session("s2".to_string(), default_config())
            .await
            .unwrap();

        let sessions = sm.list_sessions().await;
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&"s1".to_string()));
        assert!(sessions.contains(&"s2".to_string()));
    }

    #[tokio::test]
    async fn test_session_manager_get_child_sessions() {
        let sm = make_manager();
        sm.create_session("parent".to_string(), default_config())
            .await
            .unwrap();

        let mut child_config = default_config();
        child_config.parent_id = Some("parent".to_string());
        sm.create_session("child1".to_string(), child_config.clone())
            .await
            .unwrap();
        sm.create_session("child2".to_string(), child_config)
            .await
            .unwrap();

        let children = sm.get_child_sessions("parent").await;
        assert_eq!(children.len(), 2);
        assert!(children.contains(&"child1".to_string()));
        assert!(children.contains(&"child2".to_string()));
    }

    #[tokio::test]
    async fn test_session_manager_get_child_sessions_none() {
        let sm = make_manager();
        sm.create_session("parent".to_string(), default_config())
            .await
            .unwrap();

        let children = sm.get_child_sessions("parent").await;
        assert!(children.is_empty());
    }

    #[tokio::test]
    async fn test_session_manager_is_child_session() {
        let sm = make_manager();
        sm.create_session("parent".to_string(), default_config())
            .await
            .unwrap();

        let mut child_config = default_config();
        child_config.parent_id = Some("parent".to_string());
        sm.create_session("child".to_string(), child_config)
            .await
            .unwrap();

        assert!(!sm.is_child_session("parent").await.unwrap());
        assert!(sm.is_child_session("child").await.unwrap());
    }

    #[tokio::test]
    async fn test_session_manager_is_child_session_not_found() {
        let sm = make_manager();
        let result = sm.is_child_session("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_list_tools() {
        let sm = make_manager();
        let tools = sm.list_tools();
        assert!(!tools.is_empty());
    }

    #[tokio::test]
    async fn test_session_manager_tool_executor() {
        let sm = make_manager();
        let executor = sm.tool_executor();
        assert_eq!(executor.workspace().to_str().unwrap(), "/tmp");
    }

    #[tokio::test]
    async fn test_session_manager_context_usage() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let usage = sm.context_usage("s1").await.unwrap();
        assert_eq!(usage.used_tokens, 0);
        assert_eq!(usage.turns, 0);
    }

    #[tokio::test]
    async fn test_session_manager_context_usage_not_found() {
        let sm = make_manager();
        let result = sm.context_usage("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_history() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        {
            let session_lock = sm.get_session("s1").await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("test"));
        }

        let history = sm.history("s1").await.unwrap();
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn test_session_manager_history_not_found() {
        let sm = make_manager();
        let result = sm.history("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_clear() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        {
            let session_lock = sm.get_session("s1").await.unwrap();
            let mut session = session_lock.write().await;
            session.add_message(Message::user("test"));
        }

        sm.clear("s1").await.unwrap();

        let history = sm.history("s1").await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_session_manager_clear_not_found() {
        let sm = make_manager();
        let result = sm.clear("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_cancel_operation_no_operation() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let cancelled = sm.cancel_operation("s1").await.unwrap();
        assert!(!cancelled);
    }

    #[tokio::test]
    async fn test_session_manager_cancel_operation_not_found() {
        let sm = make_manager();
        let result = sm.cancel_operation("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_get_all_sessions() {
        let sm = make_manager();
        assert!(sm.get_all_sessions().await.is_empty());

        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();
        sm.create_session("s2".to_string(), default_config())
            .await
            .unwrap();

        let sessions = sm.get_all_sessions().await;
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_session_manager_add_context_provider() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        // We can't easily test with a real provider, but we can test the error path
        let result = sm.list_context_providers("s1").await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_session_manager_list_context_providers_not_found() {
        let sm = make_manager();
        let result = sm.list_context_providers("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_remove_context_provider() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let removed = sm
            .remove_context_provider("s1", "nonexistent")
            .await
            .unwrap();
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_session_manager_remove_context_provider_not_found() {
        let sm = make_manager();
        let result = sm.remove_context_provider("nonexistent", "provider").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_add_permission_rule_invalid_type() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let result = sm.add_permission_rule("s1", "invalid", "rule").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_add_ask_rule() {
        let session = make_session("s1").await;
        session.add_ask_rule("Bash(git:*)").await;

        let decision = session
            .check_permission("Bash", &serde_json::json!({"command": "git status"}))
            .await;
        assert_eq!(decision, PermissionDecision::Ask);
    }

    #[tokio::test]
    async fn test_session_manager_add_ask_rule() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        sm.add_permission_rule("s1", "ask", "Bash(docker:*)")
            .await
            .unwrap();

        let decision = sm
            .check_permission("s1", "Bash", &serde_json::json!({"command": "docker ps"}))
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Ask);
    }

    #[tokio::test]
    async fn test_session_queue_metrics() {
        let session = make_session("s1").await;
        session.start_queue().await.unwrap();
        let _metrics = session.queue_metrics().await;
        // Metrics may or may not be available depending on queue state
        // Just verify the call doesn't panic
    }

    #[tokio::test]
    async fn test_session_queue_stats() {
        let session = make_session("s1").await;
        let stats = session.queue_stats().await;
        assert_eq!(stats.total_pending, 0);
        assert_eq!(stats.total_active, 0);
    }

    #[tokio::test]
    async fn test_session_dead_letters() {
        let session = make_session("s1").await;
        let dead_letters = session.dead_letters().await;
        assert!(dead_letters.is_empty());
    }

    #[tokio::test]
    async fn test_session_stop_queue() {
        let session = make_session("s1").await;
        session.stop_queue().await;
        // No assertion needed, just verify it doesn't panic
    }

    #[tokio::test]
    async fn test_session_config_with_security() {
        let security_config = crate::security::SecurityConfig {
            enabled: true,
            ..Default::default()
        };

        let config = SessionConfig {
            security_config: Some(security_config),
            ..Default::default()
        };

        let session = Session::new("s1".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(session.security_guard.is_some());
    }

    #[tokio::test]
    async fn test_session_config_security_disabled() {
        let security_config = crate::security::SecurityConfig {
            enabled: false,
            ..Default::default()
        };

        let config = SessionConfig {
            security_config: Some(security_config),
            ..Default::default()
        };

        let session = Session::new("s1".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(session.security_guard.is_none());
    }

    #[tokio::test]
    async fn test_session_pause_from_non_active() {
        let mut session = make_session("s1").await;
        session.set_completed();

        let paused = session.pause();
        assert!(!paused);
        assert_eq!(session.state, SessionState::Completed);
    }

    #[tokio::test]
    async fn test_session_resume_from_non_paused() {
        let mut session = make_session("s1").await;
        assert_eq!(session.state, SessionState::Active);

        let resumed = session.resume();
        assert!(!resumed);
        assert_eq!(session.state, SessionState::Active);
    }

    #[tokio::test]
    async fn test_session_manager_pause_session_not_found() {
        let sm = make_manager();
        let result = sm.pause_session("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_resume_session_not_found() {
        let sm = make_manager();
        let result = sm.resume_session("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_get_todos_not_found() {
        let sm = make_manager();
        let result = sm.get_todos("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_set_todos() {
        let sm = make_manager();
        sm.create_session("s1".to_string(), default_config())
            .await
            .unwrap();

        let todos = vec![Todo::new("t1", "test")];
        let result = sm.set_todos("s1", todos).await.unwrap();
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn test_session_manager_set_todos_not_found() {
        let sm = make_manager();
        let result = sm.set_todos("nonexistent", vec![]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_manager_compact_not_found() {
        let sm = make_manager();
        let result = sm.compact("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_compact_not_enough_messages() {
        let mut session = make_session("s1").await;

        // Add only a few messages (below MIN_MESSAGES_FOR_COMPACTION = 30)
        for i in 0..10 {
            session.add_message(Message::user(&format!("msg {}", i)));
        }

        // Create a dummy LLM client
        struct DummyClient;
        impl LlmClient for DummyClient {
            fn complete<'life0, 'life1, 'life2, 'life3, 'async_trait>(
                &'life0 self,
                _messages: &'life1 [Message],
                _system: Option<&'life2 str>,
                _tools: &'life3 [crate::llm::ToolDefinition],
            ) -> core::pin::Pin<
                Box<
                    dyn core::future::Future<Output = anyhow::Result<crate::llm::LlmResponse>>
                        + core::marker::Send
                        + 'async_trait,
                >,
            >
            where
                'life0: 'async_trait,
                'life1: 'async_trait,
                'life2: 'async_trait,
                'life3: 'async_trait,
                Self: 'async_trait,
            {
                unimplemented!()
            }

            fn complete_streaming<'life0, 'life1, 'life2, 'life3, 'async_trait>(
                &'life0 self,
                _messages: &'life1 [Message],
                _system: Option<&'life2 str>,
                _tools: &'life3 [crate::llm::ToolDefinition],
            ) -> core::pin::Pin<
                Box<
                    dyn core::future::Future<
                            Output = anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>>,
                        > + core::marker::Send
                        + 'async_trait,
                >,
            >
            where
                'life0: 'async_trait,
                'life1: 'async_trait,
                'life2: 'async_trait,
                'life3: 'async_trait,
                Self: 'async_trait,
            {
                unimplemented!()
            }
        }

        let client: Arc<dyn LlmClient> = Arc::new(DummyClient);
        let result = session.compact(&client).await;
        assert!(result.is_ok());
        assert_eq!(session.messages.len(), 10); // No compaction happened
    }

    #[tokio::test]
    async fn test_session_compact_nothing_to_summarize() {
        let mut session = make_session("s1").await;

        // Add 31 messages so compaction triggers
        for i in 0..31 {
            session.add_message(Message::user(&format!("msg {}", i)));
        }

        struct DummyClient;
        impl LlmClient for DummyClient {
            fn complete<'life0, 'life1, 'life2, 'life3, 'async_trait>(
                &'life0 self,
                _messages: &'life1 [Message],
                _system: Option<&'life2 str>,
                _tools: &'life3 [crate::llm::ToolDefinition],
            ) -> core::pin::Pin<
                Box<
                    dyn core::future::Future<Output = anyhow::Result<crate::llm::LlmResponse>>
                        + core::marker::Send
                        + 'async_trait,
                >,
            >
            where
                'life0: 'async_trait,
                'life1: 'async_trait,
                'life2: 'async_trait,
                'life3: 'async_trait,
                Self: 'async_trait,
            {
                Box::pin(async {
                    Ok(crate::llm::LlmResponse {
                        message: Message {
                            role: "assistant".to_string(),
                            content: vec![crate::llm::ContentBlock::Text {
                                text: "Summary of conversation".to_string(),
                            }],
                            reasoning_content: None,
                        },
                        usage: crate::llm::TokenUsage::default(),
                        stop_reason: None,
                    })
                })
            }

            fn complete_streaming<'life0, 'life1, 'life2, 'life3, 'async_trait>(
                &'life0 self,
                _messages: &'life1 [Message],
                _system: Option<&'life2 str>,
                _tools: &'life3 [crate::llm::ToolDefinition],
            ) -> core::pin::Pin<
                Box<
                    dyn core::future::Future<
                            Output = anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>>,
                        > + core::marker::Send
                        + 'async_trait,
                >,
            >
            where
                'life0: 'async_trait,
                'life1: 'async_trait,
                'life2: 'async_trait,
                'life3: 'async_trait,
                Self: 'async_trait,
            {
                unimplemented!()
            }
        }

        let client: Arc<dyn LlmClient> = Arc::new(DummyClient);
        let result = session.compact(&client).await;
        assert!(result.is_ok());
        // After compaction, message count should be reduced
        assert!(session.messages.len() <= 31);
    }

    #[test]
    fn test_default_auto_compact_threshold_function() {
        assert_eq!(default_auto_compact_threshold(), 0.80);
    }

    #[tokio::test]
    async fn test_session_manager_create_child_session() {
        let sm = make_manager();
        sm.create_session("parent".to_string(), default_config())
            .await
            .unwrap();

        let child_id = sm
            .create_child_session("parent", "child".to_string(), default_config())
            .await
            .unwrap();

        assert_eq!(child_id, "child");

        let session_lock = sm.get_session("child").await.unwrap();
        let session = session_lock.read().await;
        assert_eq!(session.parent_id, Some("parent".to_string()));
    }

    #[tokio::test]
    async fn test_session_manager_create_child_session_parent_not_found() {
        let sm = make_manager();
        let result = sm
            .create_child_session("nonexistent", "child".to_string(), default_config())
            .await;
        assert!(result.is_err());
    }

    // ========================================================================
    // Cost Records Tests
    // ========================================================================

    #[tokio::test]
    async fn test_update_usage_records_cost_record() {
        let config = default_config();
        let mut session = Session::new("test-cost".to_string(), config, vec![])
            .await
            .unwrap();
        session.model_name = Some("gpt-4o".to_string());

        let usage = crate::llm::TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        session.update_usage(&usage);

        assert_eq!(session.cost_records.len(), 1);
        assert_eq!(session.cost_records[0].model, "gpt-4o");
        assert_eq!(session.cost_records[0].prompt_tokens, 1000);
        assert_eq!(session.cost_records[0].completion_tokens, 500);
        assert!(session.cost_records[0].cost_usd.is_some());
    }

    #[tokio::test]
    async fn test_update_usage_accumulates_cost_records() {
        let config = default_config();
        let mut session = Session::new("test-cost-2".to_string(), config, vec![])
            .await
            .unwrap();
        session.model_name = Some("gpt-4o".to_string());

        for _ in 0..3 {
            let usage = crate::llm::TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                cache_read_tokens: None,
                cache_write_tokens: None,
            };
            session.update_usage(&usage);
        }

        assert_eq!(session.cost_records.len(), 3);
    }

    #[tokio::test]
    async fn test_to_session_data_includes_cost_records() {
        let config = default_config();
        let mut session = Session::new("test-cost-3".to_string(), config, vec![])
            .await
            .unwrap();
        session.model_name = Some("gpt-4o".to_string());

        let usage = crate::llm::TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        session.update_usage(&usage);

        let data = session.to_session_data(None);
        assert_eq!(data.cost_records.len(), 1);
        assert_eq!(data.cost_records[0].model, "gpt-4o");
    }

    #[tokio::test]
    async fn test_restore_from_data_restores_cost_records() {
        let config = default_config();
        let mut session = Session::new("test-cost-4".to_string(), config.clone(), vec![])
            .await
            .unwrap();
        session.model_name = Some("gpt-4o".to_string());

        let usage = crate::llm::TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        session.update_usage(&usage);

        let data = session.to_session_data(None);

        // Create a fresh session and restore
        let mut fresh_session = Session::new("test-cost-4".to_string(), config, vec![])
            .await
            .unwrap();
        assert!(fresh_session.cost_records.is_empty());

        fresh_session.restore_from_data(&data);
        assert_eq!(fresh_session.cost_records.len(), 1);
        assert_eq!(fresh_session.cost_records[0].model, "gpt-4o");
    }
}
