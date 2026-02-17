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

pub(crate) mod compaction;
pub mod manager;

pub use manager::SessionManager;

#[cfg(test)]
#[path = "tests.rs"]
mod tests_file;

use crate::agent::AgentEvent;
use crate::hitl::{ConfirmationManager, ConfirmationPolicy};
use crate::llm::{LlmClient, Message, TokenUsage, ToolDefinition};
use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::planning::Task;
use crate::queue::{ExternalTaskResult, LaneHandlerConfig, SessionQueueConfig};
use crate::session_lane_queue::SessionLaneQueue;
use crate::store::{LlmConfigData, SessionData};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

/// Session state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SessionState {
    #[default]
    Unknown = 0,
    Active = 1,
    Paused = 2,
    Completed = 3,
    Error = 4,
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

/// Session configuration
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
    /// Shared hook engine for lifecycle events
    #[serde(skip)]
    pub hook_engine: Option<std::sync::Arc<crate::hooks::HookEngine>>,
    /// Enable planning phase before execution
    #[serde(default)]
    pub planning_enabled: bool,
    /// Enable goal tracking
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
            planning_enabled: false,
            goal_tracking: false,
        }
    }
}

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
    /// Task list for tracking
    pub tasks: Vec<Task>,
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
    /// Loaded skills for tool filter enforcement in the agent loop
    pub loaded_skills: Vec<crate::tools::Skill>,
    /// Context store client for semantic search over ingested content
    #[cfg(feature = "context-store")]
    pub context_client: Option<Arc<crate::context_store::A3SContextClient>>,
}

/// Validate that an identifier is safe for use in file paths.
/// Rejects path traversal attempts and non-alphanumeric characters.
fn validate_path_safe_id(id: &str, label: &str) -> Result<()> {
    if id.is_empty() {
        anyhow::bail!("{label} must not be empty");
    }
    // Allow alphanumeric, hyphens, underscores, and dots (but not leading dots)
    let is_safe = id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !id.starts_with('.')
        && !id.contains("..");
    if !is_safe {
        anyhow::bail!("{label} contains unsafe characters: {id:?}");
    }
    Ok(())
}

impl Session {
    /// Create a new session (async due to SessionLaneQueue initialization)
    pub async fn new(
        id: String,
        config: SessionConfig,
        tools: Vec<ToolDefinition>,
    ) -> Result<Self> {
        // Validate session ID to prevent path traversal
        validate_path_safe_id(&id, "Session ID")?;

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
        let agent_memory = crate::memory::AgentMemory::new(memory_store);
        let memory = Arc::new(RwLock::new(agent_memory.clone()));

        // Create memory context provider to inject past memories as context
        let memory_provider: Arc<dyn crate::context::ContextProvider> =
            Arc::new(crate::memory::MemoryContextProvider::new(agent_memory));

        // Create context store client with in-memory backend for semantic search
        #[cfg(feature = "context-store")]
        let (context_client, context_providers) = {
            let mut context_store_config = crate::context_store::config::Config::default();
            context_store_config.storage.backend =
                crate::context_store::config::StorageBackend::Memory;
            match crate::context_store::A3SContextClient::new(
                context_store_config,
                None,
                crate::context_store::ProviderInfo::default(),
            ) {
                Ok(client) => {
                    let client = Arc::new(client);
                    let ctx_provider: Arc<dyn crate::context::ContextProvider> = Arc::new(
                        crate::context_store::A3SContextProvider::new(client.clone()),
                    );
                    (Some(client), vec![memory_provider, ctx_provider])
                }
                Err(e) => {
                    tracing::warn!("Failed to create context store client: {}. Skipping.", e);
                    (None, vec![memory_provider])
                }
            }
        };
        #[cfg(not(feature = "context-store"))]
        let context_providers = vec![memory_provider];

        // Initialize empty plan
        let current_plan = Arc::new(RwLock::new(None));

        // Initialize security guard if configured, using shared hook engine when available
        let security_guard = config.security_config.as_ref().and_then(|sc| {
            if sc.enabled {
                let guard = crate::security::SecurityGuard::new(id.clone(), sc.clone());
                if let Some(ref shared) = config.hook_engine {
                    guard.register_hooks(shared.as_ref());
                } else {
                    tracing::warn!(
                        "Session {}: security guard created without a hook engine — \
                         security hooks will not be active",
                        id
                    );
                }
                Some(Arc::new(guard))
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
            context_providers,
            tasks: Vec::new(),
            parent_id,
            memory,
            current_plan,
            security_guard,
            tool_metrics: Arc::new(RwLock::new(crate::telemetry::ToolMetrics::new())),
            cost_records: Vec::new(),
            loaded_skills: Vec::new(),
            #[cfg(feature = "context-store")]
            context_client,
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
    // Task Management
    // ========================================================================

    /// Get the current task list
    pub fn get_tasks(&self) -> &[Task] {
        &self.tasks
    }

    /// Set the task list (replaces entire list)
    ///
    /// Broadcasts a TaskUpdated event after updating.
    pub fn set_tasks(&mut self, tasks: Vec<Task>) {
        self.tasks = tasks.clone();
        self.touch();

        // Broadcast event
        let _ = self.event_tx.send(AgentEvent::TaskUpdated {
            session_id: self.id.clone(),
            tasks,
        });
    }

    /// Get count of active (non-completed, non-cancelled) tasks
    pub fn active_task_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.is_active()).count()
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
    pub fn history(&self) -> &[Message] {
        &self.messages
    }

    /// Add a message to history
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
            if model_str.is_empty() {
                "unknown"
            } else {
                &model_str
            },
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
        if let Some(new_messages) =
            compaction::compact_messages(&self.id, &self.messages, llm_client).await?
        {
            self.messages = new_messages;
            self.touch();
        }
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
            tasks: self.tasks.clone(),
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
        self.tasks = data.tasks.clone();
        self.parent_id = data.parent_id.clone();
    }
}
