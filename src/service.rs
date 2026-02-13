//! gRPC service implementation
//!
//! Implements the CodeAgentService gRPC API defined in code_agent.proto.
//! The service runs on port 4088 and handles:
//! - Lifecycle management (Initialize, Shutdown, HealthCheck, GetCapabilities)
//! - Session management (Create, Destroy, List, Get, Configure, GetMessages)
//! - Code generation (Generate, StreamGenerate, GenerateStructured)
//! - Skill management (LoadSkill, UnloadSkill, ListSkills) - skills are global
//! - Context management (GetContextUsage, CompactContext, ClearContext)
//! - Event streaming (SubscribeEvents)
//! - Control operations (Cancel, Pause, Resume)
//! - Human-in-the-Loop (ConfirmToolExecution, SetConfirmationPolicy, GetConfirmationPolicy)
//! - Provider configuration (ListProviders, AddProvider, UpdateProvider, RemoveProvider, SetDefaultModel)
//!
//! ## Skill System
//!
//! Skills are loaded globally and available to all sessions. Use PermissionPolicy
//! to control which tools each session can access.

use crate::agent::AgentEvent;
use crate::config::CodeConfig;
use crate::convert;
use crate::hooks::{HookEngine, HookEvent, SkillLoadEvent, SkillUnloadEvent};
use crate::llm::{self, ContentBlock};
use crate::lsp::LspManager;
use crate::mcp::{McpManager, McpServerConfig, McpTransportConfig};
use crate::session::{SessionConfig, SessionManager};
use crate::tools::{builtin_claude_code_skills, ClaudeCodeSkill, ToolExecutor};
use a3s_cron::{parse_natural, CronExpression, CronManager};
use anyhow::Result;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

// Include generated proto code
pub mod proto {
    tonic::include_proto!("a3s.code.agent.v1");
}

use proto::code_agent_service_server::{CodeAgentService, CodeAgentServiceServer};
use proto::*;

/// Convert StorageBackend to proto StorageType i32
fn storage_backend_to_proto(backend: &crate::config::StorageBackend) -> i32 {
    match backend {
        crate::config::StorageBackend::Memory => 1, // STORAGE_TYPE_MEMORY
        crate::config::StorageBackend::File => 2,   // STORAGE_TYPE_FILE
        crate::config::StorageBackend::Custom => 0, // STORAGE_TYPE_UNSPECIFIED
    }
}

/// Agent state for lifecycle management
#[derive(Default)]
struct AgentState {
    initialized: bool,
    workspace: String,
}

/// Information about a loaded skill
#[derive(Clone)]
struct SkillInfo {
    /// Skill name
    #[allow(dead_code)]
    name: String,
    /// Tool names loaded from this skill (A3S format)
    tool_names: Vec<String>,
    /// Claude Code skill data (if loaded from Claude Code format)
    claude_code_skill: Option<ClaudeCodeSkill>,
    /// Skill version (if available)
    #[allow(dead_code)]
    version: Option<String>,
    /// Skill description (if available)
    #[allow(dead_code)]
    description: Option<String>,
    /// Timestamp when skill was loaded (Unix milliseconds)
    loaded_at: i64,
}

/// Code Agent service implementation
pub struct CodeAgentServiceImpl {
    session_manager: Arc<SessionManager>,
    agent_state: Arc<RwLock<AgentState>>,
    event_tx: broadcast::Sender<AgentEvent>,
    hook_engine: Arc<HookEngine>,
    skill_registry: Arc<RwLock<HashMap<String, SkillInfo>>>,
    /// Provider configuration (mutable at runtime)
    provider_config: Arc<RwLock<CodeConfig>>,
    /// Path to persist provider config changes (if set)
    config_path: Option<std::path::PathBuf>,
    /// MCP manager for external tool servers
    mcp_manager: Arc<McpManager>,
    /// LSP manager for language servers
    lsp_manager: Arc<LspManager>,
    /// Cron manager for scheduled tasks (lazily initialized)
    cron_manager: Arc<RwLock<Option<Arc<CronManager>>>>,
}

impl CodeAgentServiceImpl {
    pub fn new(session_manager: Arc<SessionManager>) -> Self {
        let (event_tx, _) = broadcast::channel(100);
        let skill_registry = Arc::new(RwLock::new(HashMap::new()));
        let svc = Self {
            session_manager,
            agent_state: Arc::new(RwLock::new(AgentState::default())),
            event_tx,
            hook_engine: Arc::new(HookEngine::new()),
            skill_registry: skill_registry.clone(),
            provider_config: Arc::new(RwLock::new(CodeConfig::default())),
            config_path: None,
            mcp_manager: Arc::new(McpManager::new()),
            lsp_manager: Arc::new(LspManager::new()),
            cron_manager: Arc::new(RwLock::new(None)),
        };
        Self::register_builtin_claude_code_skills(&skill_registry);
        svc
    }

    /// Create a new service with initial configuration
    pub fn with_config(
        session_manager: Arc<SessionManager>,
        config: CodeConfig,
        config_path: Option<std::path::PathBuf>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(100);
        let skill_registry = Arc::new(RwLock::new(HashMap::new()));
        let skill_dirs = config.skill_dirs.clone();
        let svc = Self {
            session_manager,
            agent_state: Arc::new(RwLock::new(AgentState::default())),
            event_tx,
            hook_engine: Arc::new(HookEngine::new()),
            skill_registry: skill_registry.clone(),
            provider_config: Arc::new(RwLock::new(config)),
            config_path,
            mcp_manager: Arc::new(McpManager::new()),
            lsp_manager: Arc::new(LspManager::new()),
            cron_manager: Arc::new(RwLock::new(None)),
        };
        Self::register_builtin_claude_code_skills(&skill_registry);
        Self::load_claude_code_skills_from_dirs(&skill_registry, &skill_dirs);
        svc
    }

    /// Register built-in Claude Code skills into the skill registry
    fn register_builtin_claude_code_skills(
        skill_registry: &Arc<RwLock<HashMap<String, SkillInfo>>>,
    ) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let mut registry = skill_registry.blocking_write();
        for skill in builtin_claude_code_skills() {
            tracing::info!("Registered built-in Claude Code skill: {}", skill.name);
            let name = skill.name.clone();
            let description = Some(skill.description.clone()).filter(|d| !d.is_empty());
            registry.insert(
                name.clone(),
                SkillInfo {
                    name,
                    tool_names: vec![],
                    claude_code_skill: Some(skill),
                    version: None,
                    description,
                    loaded_at: now,
                },
            );
        }
    }

    /// Load Claude Code skills from configured skill directories into the skill registry
    fn load_claude_code_skills_from_dirs(
        skill_registry: &Arc<RwLock<HashMap<String, SkillInfo>>>,
        skill_dirs: &[std::path::PathBuf],
    ) {
        use crate::tools::load_claude_code_skills;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let mut registry = skill_registry.blocking_write();
        for dir in skill_dirs {
            let skills = load_claude_code_skills(dir);
            for skill in skills {
                tracing::info!(
                    "Registered Claude Code skill '{}' from {}",
                    skill.name,
                    dir.display()
                );
                let name = skill.name.clone();
                let description = Some(skill.description.clone()).filter(|d| !d.is_empty());
                registry.insert(
                    name.clone(),
                    SkillInfo {
                        name,
                        tool_names: vec![],
                        claude_code_skill: Some(skill),
                        version: None,
                        description,
                        loaded_at: now,
                    },
                );
            }
        }
    }

    /// Broadcast an event to all subscribers
    #[allow(dead_code)]
    fn broadcast_event(&self, event: AgentEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Get the hook engine
    pub fn hook_engine(&self) -> &Arc<HookEngine> {
        &self.hook_engine
    }

    /// Get the provider configuration
    pub fn provider_config(&self) -> &Arc<RwLock<CodeConfig>> {
        &self.provider_config
    }

    /// Persist the current provider configuration to disk (if config_path is set)
    async fn persist_config(&self) {
        if let Some(ref path) = self.config_path {
            let config = self.provider_config.read().await;
            if let Err(e) = config.save_to_file(path) {
                tracing::error!("Failed to persist config to {}: {}", path.display(), e);
            } else {
                tracing::debug!("Persisted config to {}", path.display());
            }
        }
    }

    /// Get all loaded Claude Code skills
    ///
    /// Returns skills that have Claude Code format (with allowed-tools, content, etc.)
    /// These can be used for prompt injection in sessions.
    pub async fn get_claude_code_skills(&self) -> Vec<ClaudeCodeSkill> {
        let registry = self.skill_registry.read().await;
        registry
            .values()
            .filter_map(|info| info.claude_code_skill.clone())
            .collect()
    }

    /// Get a specific Claude Code skill by name
    pub async fn get_claude_code_skill(&self, name: &str) -> Option<ClaudeCodeSkill> {
        let registry = self.skill_registry.read().await;
        registry
            .get(name)
            .and_then(|info| info.claude_code_skill.clone())
    }

    /// Parse skill metadata from content (frontmatter)
    fn parse_skill_metadata(content: &str) -> (Option<String>, Option<String>) {
        // Try to parse YAML frontmatter if present
        if let Some(after_prefix) = content.strip_prefix("---") {
            if let Some(end) = after_prefix.find("---") {
                let frontmatter = &after_prefix[..end];
                let mut version = None;
                let mut description = None;

                for line in frontmatter.lines() {
                    let line = line.trim();
                    if let Some(v) = line.strip_prefix("version:") {
                        version = Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
                    } else if let Some(d) = line.strip_prefix("description:") {
                        description =
                            Some(d.trim().trim_matches('"').trim_matches('\'').to_string());
                    }
                }

                return (version, description);
            }
        }
        (None, None)
    }

    /// Get or initialize the cron manager
    async fn get_or_init_cron_manager(&self) -> Result<Arc<CronManager>, Status> {
        // Check if already initialized
        {
            let guard = self.cron_manager.read().await;
            if let Some(ref manager) = *guard {
                return Ok(Arc::clone(manager));
            }
        }

        // Initialize with workspace from agent state
        let state = self.agent_state.read().await;
        let workspace = if state.workspace.is_empty() {
            std::env::current_dir()
                .map_err(|e| Status::internal(format!("Failed to get current dir: {}", e)))?
        } else {
            std::path::PathBuf::from(&state.workspace)
        };
        drop(state);

        let manager = CronManager::new(&workspace)
            .await
            .map_err(|e| Status::internal(format!("Failed to initialize cron manager: {}", e)))?;

        let manager = Arc::new(manager);

        // Store the manager
        let mut guard = self.cron_manager.write().await;
        *guard = Some(Arc::clone(&manager));

        Ok(manager)
    }
}

#[tonic::async_trait]
impl CodeAgentService for CodeAgentServiceImpl {
    // ========================================================================
    // Lifecycle Management
    // ========================================================================

    async fn health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        let state = self.agent_state.read().await;
        let status = if state.initialized {
            health_check_response::Status::Healthy
        } else {
            health_check_response::Status::Degraded
        };

        Ok(Response::new(HealthCheckResponse {
            status: status as i32,
            message: if state.initialized {
                "Agent is healthy".to_string()
            } else {
                "Agent not initialized".to_string()
            },
            details: HashMap::new(),
        }))
    }

    async fn get_capabilities(
        &self,
        _request: Request<GetCapabilitiesRequest>,
    ) -> Result<Response<GetCapabilitiesResponse>, Status> {
        let tools: Vec<ToolCapability> = self
            .session_manager
            .tool_executor()
            .definitions()
            .iter()
            .map(|t| ToolCapability {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: vec![],
                r#async: false,
            })
            .collect();

        Ok(Response::new(GetCapabilitiesResponse {
            info: Some(AgentInfo {
                name: "a3s-code".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                description: "A3S Code Agent - Rust implementation".to_string(),
                author: "A3S Team".to_string(),
                license: "MIT".to_string(),
                homepage: "https://github.com/anthropics/a3s-box".to_string(),
            }),
            features: vec![
                "streaming".to_string(),
                "tool_calling".to_string(),
                "structured_output".to_string(),
                "skills".to_string(),
            ],
            tools,
            models: vec![
                ModelCapability {
                    provider: "anthropic".to_string(),
                    model: "claude-3-5-sonnet-20241022".to_string(),
                    features: vec!["tool_use".to_string(), "streaming".to_string()],
                },
                ModelCapability {
                    provider: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                    features: vec!["tool_use".to_string(), "streaming".to_string()],
                },
            ],
            limits: Some(ResourceLimits {
                max_context_tokens: 200_000,
                max_concurrent_sessions: 100,
                max_tools_per_request: 50,
            }),
            metadata: HashMap::new(),
        }))
    }

    async fn initialize(
        &self,
        request: Request<InitializeRequest>,
    ) -> Result<Response<InitializeResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.agent_state.write().await;

        state.workspace = req.workspace;
        state.initialized = true;

        tracing::info!("Agent initialized with workspace: {}", state.workspace);

        Ok(Response::new(InitializeResponse {
            success: true,
            message: "Agent initialized successfully".to_string(),
            info: Some(AgentInfo {
                name: "a3s-code".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                description: "A3S Code Agent".to_string(),
                author: "A3S Team".to_string(),
                license: "MIT".to_string(),
                homepage: "https://github.com/anthropics/a3s-box".to_string(),
            }),
        }))
    }

    async fn shutdown(
        &self,
        _request: Request<ShutdownRequest>,
    ) -> Result<Response<ShutdownResponse>, Status> {
        let mut state = self.agent_state.write().await;
        state.initialized = false;

        tracing::info!("Agent shutdown requested");

        Ok(Response::new(ShutdownResponse {
            success: true,
            message: "Agent shutdown initiated".to_string(),
        }))
    }

    // ========================================================================
    // Session Management
    // ========================================================================

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let req = request.into_inner();
        let session_id = req
            .session_id
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        tracing::info!(name: "a3s.grpc.create_session", session_id = %session_id, "Creating session");

        let config = req.config.unwrap_or_default();

        // Convert proto StorageType to config::StorageBackend
        let storage_type = match config.storage_type {
            0 => crate::config::StorageBackend::File, // STORAGE_TYPE_UNSPECIFIED defaults to File
            1 => crate::config::StorageBackend::Memory, // STORAGE_TYPE_MEMORY
            2 => crate::config::StorageBackend::File, // STORAGE_TYPE_FILE
            _ => crate::config::StorageBackend::File, // Unknown defaults to File
        };

        // Fall back to server-level workspace if session workspace is empty
        let workspace = if config.workspace.is_empty() {
            let default_workspace = self
                .session_manager
                .tool_executor()
                .workspace()
                .to_string_lossy()
                .to_string();
            tracing::warn!(
                "Session {} created without workspace, using server default: {}",
                session_id,
                default_workspace
            );
            default_workspace
        } else {
            config.workspace
        };

        let session_config = SessionConfig {
            name: config.name,
            workspace,
            system_prompt: if config.system_prompt.is_empty() {
                None
            } else {
                Some(config.system_prompt)
            },
            max_context_length: config.max_context_length,
            auto_compact: config.auto_compact,
            auto_compact_threshold: if config.auto_compact_threshold > 0.0 {
                config.auto_compact_threshold
            } else {
                crate::session::DEFAULT_AUTO_COMPACT_THRESHOLD
            },
            storage_type,
            queue_config: None,        // Use default queue config
            confirmation_policy: None, // Use default confirmation policy (HITL disabled)
            permission_policy: None,   // Use default permission policy
            parent_id: None,           // Not a child session
            security_config: None,     // Security disabled by default
        };

        self.session_manager
            .create_session(session_id.clone(), session_config)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Configure LLM client if provided in session config
        if let Some(llm) = config.llm {
            if !llm.provider.is_empty() && !llm.model.is_empty() {
                let mut llm_config = llm::LlmConfig::new(
                    &llm.provider,
                    &llm.model,
                    &llm.api_key,
                );
                if !llm.base_url.is_empty() {
                    llm_config = llm_config.with_base_url(&llm.base_url);
                }
                self.session_manager
                    .configure(&session_id, None, None, Some(llm_config))
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?;
            }
        }

        // Get session details for response
        let session_lock = self
            .session_manager
            .get_session(&session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let session = session_lock.read().await;

        Ok(Response::new(CreateSessionResponse {
            session_id: session_id.clone(),
            session: Some(Session {
                session_id,
                config: Some(proto::SessionConfig {
                    name: session.config.name.clone(),
                    workspace: session.config.workspace.clone(),
                    llm: None, // Don't echo back LLM config (contains API key)
                    system_prompt: session.config.system_prompt.clone().unwrap_or_default(),
                    max_context_length: session.config.max_context_length,
                    auto_compact: session.config.auto_compact,
                    auto_compact_threshold: session.config.auto_compact_threshold,
                    storage_type: storage_backend_to_proto(&session.config.storage_type),
                }),
                state: session.state.to_proto_i32(),
                context_usage: Some(convert::internal_context_usage_to_proto(
                    &session.context_usage,
                )),
                created_at: session.created_at,
                updated_at: session.updated_at,
            }),
        }))
    }

    async fn destroy_session(
        &self,
        request: Request<DestroySessionRequest>,
    ) -> Result<Response<DestroySessionResponse>, Status> {
        let req = request.into_inner();
        tracing::info!(name: "a3s.grpc.destroy_session", session_id = %req.session_id, "Destroying session");

        self.session_manager
            .destroy_session(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(DestroySessionResponse { success: true }))
    }

    async fn list_sessions(
        &self,
        _request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let sessions = self.session_manager.get_all_sessions().await;
        let mut proto_sessions = Vec::new();

        for session_lock in sessions {
            let session = session_lock.read().await;
            proto_sessions.push(proto::Session {
                session_id: session.id.clone(),
                config: Some(proto::SessionConfig {
                    name: session.config.name.clone(),
                    workspace: session.config.workspace.clone(),
                    llm: None,
                    system_prompt: session.config.system_prompt.clone().unwrap_or_default(),
                    max_context_length: session.config.max_context_length,
                    auto_compact: session.config.auto_compact,
                    auto_compact_threshold: session.config.auto_compact_threshold,
                    storage_type: storage_backend_to_proto(&session.config.storage_type),
                }),
                state: session.state.to_proto_i32(),
                context_usage: Some(convert::internal_context_usage_to_proto(
                    &session.context_usage,
                )),
                created_at: session.created_at,
                updated_at: session.updated_at,
            });
        }

        Ok(Response::new(ListSessionsResponse {
            sessions: proto_sessions,
        }))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        let req = request.into_inner();
        let session_lock = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;
        let session = session_lock.read().await;

        Ok(Response::new(GetSessionResponse {
            session: Some(proto::Session {
                session_id: session.id.clone(),
                config: Some(proto::SessionConfig {
                    name: session.config.name.clone(),
                    workspace: session.config.workspace.clone(),
                    llm: None,
                    system_prompt: session.config.system_prompt.clone().unwrap_or_default(),
                    max_context_length: session.config.max_context_length,
                    auto_compact: session.config.auto_compact,
                    auto_compact_threshold: session.config.auto_compact_threshold,
                    storage_type: storage_backend_to_proto(&session.config.storage_type),
                }),
                state: session.state.to_proto_i32(),
                context_usage: Some(convert::internal_context_usage_to_proto(
                    &session.context_usage,
                )),
                created_at: session.created_at,
                updated_at: session.updated_at,
            }),
        }))
    }

    async fn configure_session(
        &self,
        request: Request<ConfigureSessionRequest>,
    ) -> Result<Response<ConfigureSessionResponse>, Status> {
        let req = request.into_inner();

        // Extract LLM config from request if provided
        let model_config = req.config.as_ref().and_then(|c| c.llm.as_ref()).and_then(|llm| {
            if !llm.provider.is_empty() && !llm.model.is_empty() {
                let mut config = llm::LlmConfig::new(
                    &llm.provider,
                    &llm.model,
                    &llm.api_key,
                );
                if !llm.base_url.is_empty() {
                    config = config.with_base_url(&llm.base_url);
                }
                Some(config)
            } else {
                None
            }
        });

        self.session_manager
            .configure(&req.session_id, None, None, model_config)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Get updated session
        let session_lock = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let session = session_lock.read().await;

        Ok(Response::new(ConfigureSessionResponse {
            session: Some(proto::Session {
                session_id: session.id.clone(),
                config: Some(proto::SessionConfig {
                    name: session.config.name.clone(),
                    workspace: session.config.workspace.clone(),
                    llm: None,
                    system_prompt: session.config.system_prompt.clone().unwrap_or_default(),
                    max_context_length: session.config.max_context_length,
                    auto_compact: session.config.auto_compact,
                    auto_compact_threshold: session.config.auto_compact_threshold,
                    storage_type: storage_backend_to_proto(&session.config.storage_type),
                }),
                state: session.state.to_proto_i32(),
                context_usage: Some(convert::internal_context_usage_to_proto(
                    &session.context_usage,
                )),
                created_at: session.created_at,
                updated_at: session.updated_at,
            }),
        }))
    }

    async fn get_messages(
        &self,
        request: Request<GetMessagesRequest>,
    ) -> Result<Response<GetMessagesResponse>, Status> {
        let req = request.into_inner();

        // Get all messages from session
        let messages = self
            .session_manager
            .history(&req.session_id)
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;

        let total_count = messages.len() as u32;

        // Apply pagination
        let offset = req.offset.unwrap_or(0) as usize;
        let limit = req.limit.map(|l| l as usize);

        let paginated_messages: Vec<_> = messages
            .iter()
            .skip(offset)
            .take(limit.unwrap_or(usize::MAX))
            .collect();

        // Calculate has_more
        let has_more = match limit {
            Some(l) => offset + l < total_count as usize,
            None => false,
        };

        // Get current timestamp for messages (we don't track per-message timestamps)
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        // Convert to proto format
        let proto_messages: Vec<proto::ConversationMessage> = paginated_messages
            .iter()
            .enumerate()
            .map(|(i, msg)| {
                convert::internal_message_to_conversation_message(msg, offset + i, timestamp)
            })
            .collect();

        Ok(Response::new(GetMessagesResponse {
            messages: proto_messages,
            total_count,
            has_more,
        }))
    }

    // ========================================================================
    // Code Generation
    // ========================================================================

    async fn generate(
        &self,
        request: Request<GenerateRequest>,
    ) -> Result<Response<GenerateResponse>, Status> {
        let req = request.into_inner();
        tracing::info!(name: "a3s.grpc.generate", session_id = %req.session_id, "Generate");

        // Extract prompt from messages
        let prompt = req
            .messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let result = self
            .session_manager
            .generate(&req.session_id, &prompt)
            .await
            .map_err(|e| {
                tracing::error!("Generate failed: {:?}", e);
                Status::internal(format!("{:#}", e))
            })?;

        // Convert to proto format
        let usage = Some(convert::internal_usage_to_proto(&result.usage));

        // Extract tool calls from messages
        let mut tool_calls = Vec::new();
        for message in &result.messages {
            for block in &message.content {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    tool_calls.push(proto::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: input.to_string(),
                        result: None,
                    });
                }
            }
        }

        Ok(Response::new(GenerateResponse {
            session_id: req.session_id,
            message: Some(convert::internal_message_to_proto(
                result
                    .messages
                    .last()
                    .unwrap_or(&crate::llm::Message::user("")),
            )),
            tool_calls,
            usage,
            finish_reason: "stop".to_string(),
            metadata: HashMap::new(),
        }))
    }

    type StreamGenerateStream =
        Pin<Box<dyn Stream<Item = Result<GenerateChunk, Status>> + Send + 'static>>;

    async fn stream_generate(
        &self,
        request: Request<GenerateRequest>,
    ) -> Result<Response<Self::StreamGenerateStream>, Status> {
        let req = request.into_inner();
        let session_id = req.session_id.clone();
        tracing::info!(name: "a3s.grpc.stream_generate", session_id = %session_id, "Stream generate");

        // Extract prompt from messages
        let prompt = req
            .messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let (rx, _handle) = self
            .session_manager
            .generate_streaming(&req.session_id, &prompt)
            .await
            .map_err(|e| {
                tracing::error!("Stream failed: {:?}", e);
                Status::internal(format!("{:#}", e))
            })?;

        // Convert agent events to stream chunks
        let stream = convert_events_to_generate_chunks(rx, session_id);

        Ok(Response::new(Box::pin(stream)))
    }

    async fn generate_structured(
        &self,
        request: Request<GenerateStructuredRequest>,
    ) -> Result<Response<GenerateStructuredResponse>, Status> {
        let req = request.into_inner();

        // Extract prompt from messages
        let prompt = req
            .messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();

        // Add schema to prompt for structured output
        let prompt_with_schema = format!(
            "{}\n\nRespond with ONLY a valid JSON object matching this schema (no markdown, no explanation, no code blocks):\n{}",
            prompt, req.schema
        );

        let result = self
            .session_manager
            .generate(&req.session_id, &prompt_with_schema)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Extract JSON from the response
        let json_str = transform_for_structured_output(&result.text);

        Ok(Response::new(GenerateStructuredResponse {
            session_id: req.session_id,
            data: json_str,
            usage: Some(convert::internal_usage_to_proto(&result.usage)),
            metadata: HashMap::new(),
        }))
    }

    type StreamGenerateStructuredStream =
        Pin<Box<dyn Stream<Item = Result<GenerateStructuredChunk, Status>> + Send + 'static>>;

    async fn stream_generate_structured(
        &self,
        request: Request<GenerateStructuredRequest>,
    ) -> Result<Response<Self::StreamGenerateStructuredStream>, Status> {
        // For simplicity, use non-streaming for now
        let response = self.generate_structured(request).await?;
        let inner = response.into_inner();

        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            tx.send(Ok(GenerateStructuredChunk {
                session_id: inner.session_id,
                data: inner.data,
                done: true,
            }))
            .await
            .ok();
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    // ========================================================================
    // Skill Management
    // ========================================================================

    async fn load_skill(
        &self,
        request: Request<LoadSkillRequest>,
    ) -> Result<Response<LoadSkillResponse>, Status> {
        let req = request.into_inner();
        let skill_content = req.skill_content.clone().unwrap_or_default();

        // Parse skill metadata from content
        let (version, description) = Self::parse_skill_metadata(&skill_content);

        // Try to parse as Claude Code skill first
        let claude_code_skill = ClaudeCodeSkill::parse(&skill_content);

        // Load skill globally (session_id is ignored, kept for API compatibility)
        let tool_names = self
            .session_manager
            .load_skill(&req.skill_name, &skill_content);

        // Record load time
        let loaded_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        // Track skill in registry
        {
            let mut registry = self.skill_registry.write().await;
            registry.insert(
                req.skill_name.clone(),
                SkillInfo {
                    name: req.skill_name.clone(),
                    tool_names: tool_names.clone(),
                    claude_code_skill,
                    version: version.clone(),
                    description: description.clone(),
                    loaded_at,
                },
            );
        }

        // Fire SkillLoad hook (after successful load)
        let hook_event = HookEvent::SkillLoad(SkillLoadEvent {
            skill_name: req.skill_name.clone(),
            tool_names: tool_names.clone(),
            version,
            description,
            loaded_at,
        });
        let _ = self.hook_engine.fire(&hook_event).await;

        tracing::info!(
            "LoadSkill: {} loaded {} tools (session_id={} ignored, skills are global)",
            req.skill_name,
            tool_names.len(),
            req.session_id
        );

        Ok(Response::new(LoadSkillResponse {
            success: true,
            tool_names,
        }))
    }

    async fn unload_skill(
        &self,
        request: Request<UnloadSkillRequest>,
    ) -> Result<Response<UnloadSkillResponse>, Status> {
        let req = request.into_inner();

        // Get skill info from registry (for hook payload and tool names)
        let skill_info = {
            let registry = self.skill_registry.read().await;
            registry.get(&req.skill_name).cloned()
        };

        let (tool_names, duration_ms) = match &skill_info {
            Some(info) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let duration = (now - info.loaded_at).max(0) as u64;
                (info.tool_names.clone(), duration)
            }
            None => (vec![], 0),
        };

        // Fire SkillUnload hook BEFORE unload (allows cleanup handlers)
        let hook_event = HookEvent::SkillUnload(SkillUnloadEvent {
            skill_name: req.skill_name.clone(),
            tool_names: tool_names.clone(),
            duration_ms,
        });
        let _ = self.hook_engine.fire(&hook_event).await;

        // Unload tools from session manager
        if !tool_names.is_empty() {
            self.session_manager.unload_skill(&tool_names);
        }

        // Remove from registry
        {
            let mut registry = self.skill_registry.write().await;
            registry.remove(&req.skill_name);
        }

        tracing::info!(
            "UnloadSkill: {} unloaded {} tools (session_id={} ignored, skills are global)",
            req.skill_name,
            tool_names.len(),
            req.session_id
        );

        Ok(Response::new(UnloadSkillResponse {
            success: true,
            removed_tools: tool_names,
        }))
    }

    async fn list_skills(
        &self,
        _request: Request<ListSkillsRequest>,
    ) -> Result<Response<ListSkillsResponse>, Status> {
        // List all loaded tools (skills are global now)
        let tools = self.session_manager.list_tools();

        // Group tools by type
        let builtin_tools = ["bash", "read", "write", "edit", "grep", "glob", "ls"];
        let dynamic_tool_names: Vec<String> = tools
            .iter()
            .filter(|t| !builtin_tools.contains(&t.name.as_str()))
            .map(|t| t.name.clone())
            .collect();

        let mut skills = vec![];

        // Add builtin "skill"
        skills.push(proto::Skill {
            name: "builtin".to_string(),
            description: "Built-in tools".to_string(),
            tools: builtin_tools.iter().map(|s| s.to_string()).collect(),
            metadata: HashMap::new(),
        });

        // Add dynamic tools as a single skill entry (if any)
        if !dynamic_tool_names.is_empty() {
            skills.push(proto::Skill {
                name: "dynamic".to_string(),
                description: "Dynamically loaded tools from skills".to_string(),
                tools: dynamic_tool_names,
                metadata: HashMap::new(),
            });
        }

        Ok(Response::new(ListSkillsResponse { skills }))
    }

    async fn get_claude_code_skills(
        &self,
        request: Request<GetClaudeCodeSkillsRequest>,
    ) -> Result<Response<GetClaudeCodeSkillsResponse>, Status> {
        let req = request.into_inner();

        let skills = if let Some(name) = req.name {
            // Get specific skill by name
            match self.get_claude_code_skill(&name).await {
                Some(skill) => vec![proto::ClaudeCodeSkill {
                    name: skill.name,
                    description: skill.description,
                    allowed_tools: skill.allowed_tools,
                    disable_model_invocation: skill.disable_model_invocation,
                    content: skill.content,
                }],
                None => vec![],
            }
        } else {
            // Get all Claude Code skills
            self.get_claude_code_skills()
                .await
                .into_iter()
                .map(|skill| proto::ClaudeCodeSkill {
                    name: skill.name,
                    description: skill.description,
                    allowed_tools: skill.allowed_tools,
                    disable_model_invocation: skill.disable_model_invocation,
                    content: skill.content,
                })
                .collect()
        };

        Ok(Response::new(GetClaudeCodeSkillsResponse { skills }))
    }

    // ========================================================================
    // Context Management
    // ========================================================================

    async fn get_context_usage(
        &self,
        request: Request<GetContextUsageRequest>,
    ) -> Result<Response<GetContextUsageResponse>, Status> {
        let req = request.into_inner();

        let usage = self
            .session_manager
            .context_usage(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(GetContextUsageResponse {
            usage: Some(convert::internal_context_usage_to_proto(&usage)),
        }))
    }

    async fn compact_context(
        &self,
        request: Request<CompactContextRequest>,
    ) -> Result<Response<CompactContextResponse>, Status> {
        let req = request.into_inner();

        let before = self
            .session_manager
            .context_usage(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        self.session_manager
            .compact(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let after = self
            .session_manager
            .context_usage(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(CompactContextResponse {
            success: true,
            before: Some(convert::internal_context_usage_to_proto(&before)),
            after: Some(convert::internal_context_usage_to_proto(&after)),
        }))
    }

    async fn clear_context(
        &self,
        request: Request<ClearContextRequest>,
    ) -> Result<Response<ClearContextResponse>, Status> {
        let req = request.into_inner();

        self.session_manager
            .clear(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(ClearContextResponse { success: true }))
    }

    // ========================================================================
    // Event Streaming
    // ========================================================================

    type SubscribeEventsStream =
        Pin<Box<dyn Stream<Item = Result<proto::AgentEvent, Status>> + Send + 'static>>;

    async fn subscribe_events(
        &self,
        request: Request<SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        let req = request.into_inner();

        let mut rx = self.event_tx.subscribe();

        let session_filter = req.session_id;
        let event_types: Vec<i32> = req
            .event_types
            .iter()
            .filter_map(|t| match t.as_str() {
                "session_created" => Some(agent_event::EventType::SessionCreated as i32),
                "session_destroyed" => Some(agent_event::EventType::SessionDestroyed as i32),
                "generation_started" => Some(agent_event::EventType::GenerationStarted as i32),
                "generation_completed" => Some(agent_event::EventType::GenerationCompleted as i32),
                "tool_called" => Some(agent_event::EventType::ToolCalled as i32),
                "tool_completed" => Some(agent_event::EventType::ToolCompleted as i32),
                "error" => Some(agent_event::EventType::Error as i32),
                "warning" => Some(agent_event::EventType::Warning as i32),
                "info" => Some(agent_event::EventType::Info as i32),
                _ => None,
            })
            .collect();

        let (tx, out_rx) = mpsc::channel::<Result<proto::AgentEvent, Status>>(100);

        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                // Convert internal event to proto event
                if let Some(proto_event) =
                    convert::internal_event_to_proto_event(event, session_filter.as_deref())
                {
                    // Filter by event types if specified
                    if !event_types.is_empty() && !event_types.contains(&proto_event.r#type) {
                        continue;
                    }

                    if tx.send(Ok(proto_event)).await.is_err() {
                        break;
                    }
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(out_rx))))
    }

    // ========================================================================
    // Control Operations
    // ========================================================================

    async fn cancel(
        &self,
        request: Request<CancelRequest>,
    ) -> Result<Response<CancelResponse>, Status> {
        let req = request.into_inner();

        // Cancel the operation for the session
        let success = self
            .session_manager
            .cancel_operation(&req.session_id)
            .await
            .map_err(|e| {
                tracing::error!("Cancel failed: {:?}", e);
                Status::internal(format!("{:#}", e))
            })?;

        Ok(Response::new(CancelResponse { success }))
    }

    async fn pause(
        &self,
        request: Request<PauseRequest>,
    ) -> Result<Response<PauseResponse>, Status> {
        let req = request.into_inner();

        let success = self
            .session_manager
            .pause_session(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(PauseResponse { success }))
    }

    async fn resume(
        &self,
        request: Request<ResumeRequest>,
    ) -> Result<Response<ResumeResponse>, Status> {
        let req = request.into_inner();

        let success = self
            .session_manager
            .resume_session(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(ResumeResponse { success }))
    }

    // ========================================================================
    // Human-in-the-Loop (HITL)
    // ========================================================================

    async fn confirm_tool_execution(
        &self,
        request: Request<ConfirmToolExecutionRequest>,
    ) -> Result<Response<ConfirmToolExecutionResponse>, Status> {
        let req = request.into_inner();

        let found = self
            .session_manager
            .confirm_tool(&req.session_id, &req.tool_id, req.approved, req.reason)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        if !found {
            return Ok(Response::new(ConfirmToolExecutionResponse {
                success: false,
                error: format!("No pending confirmation found for tool_id: {}", req.tool_id),
            }));
        }

        Ok(Response::new(ConfirmToolExecutionResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn set_confirmation_policy(
        &self,
        request: Request<SetConfirmationPolicyRequest>,
    ) -> Result<Response<SetConfirmationPolicyResponse>, Status> {
        let req = request.into_inner();

        let proto_policy = req
            .policy
            .ok_or_else(|| Status::invalid_argument("Policy is required"))?;

        let internal_policy = convert::proto_confirmation_policy_to_internal(&proto_policy);

        let result = self
            .session_manager
            .set_confirmation_policy(&req.session_id, internal_policy)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(SetConfirmationPolicyResponse {
            success: true,
            policy: Some(convert::internal_confirmation_policy_to_proto(&result)),
        }))
    }

    async fn get_confirmation_policy(
        &self,
        request: Request<GetConfirmationPolicyRequest>,
    ) -> Result<Response<GetConfirmationPolicyResponse>, Status> {
        let req = request.into_inner();

        let policy = self
            .session_manager
            .get_confirmation_policy(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(GetConfirmationPolicyResponse {
            policy: Some(convert::internal_confirmation_policy_to_proto(&policy)),
        }))
    }

    // ========================================================================
    // External Task Handling (Pluggable Handlers)
    // ========================================================================

    async fn set_lane_handler(
        &self,
        request: Request<SetLaneHandlerRequest>,
    ) -> Result<Response<SetLaneHandlerResponse>, Status> {
        let req = request.into_inner();

        let lane = convert::proto_session_lane_to_internal(req.lane)
            .ok_or_else(|| Status::invalid_argument("Invalid lane"))?;

        let proto_config = req
            .config
            .ok_or_else(|| Status::invalid_argument("Config is required"))?;

        let internal_config = convert::proto_lane_handler_config_to_internal(&proto_config);

        self.session_manager
            .set_lane_handler(&req.session_id, lane, internal_config.clone())
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(SetLaneHandlerResponse {
            success: true,
            config: Some(convert::internal_lane_handler_config_to_proto(
                &internal_config,
            )),
        }))
    }

    async fn get_lane_handler(
        &self,
        request: Request<GetLaneHandlerRequest>,
    ) -> Result<Response<GetLaneHandlerResponse>, Status> {
        let req = request.into_inner();

        let lane = convert::proto_session_lane_to_internal(req.lane)
            .ok_or_else(|| Status::invalid_argument("Invalid lane"))?;

        let config = self
            .session_manager
            .get_lane_handler(&req.session_id, lane)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(GetLaneHandlerResponse {
            config: Some(convert::internal_lane_handler_config_to_proto(&config)),
        }))
    }

    async fn complete_external_task(
        &self,
        request: Request<CompleteExternalTaskRequest>,
    ) -> Result<Response<CompleteExternalTaskResponse>, Status> {
        let req = request.into_inner();

        let result =
            convert::proto_complete_request_to_result(req.success, &req.result, &req.error);

        let found = self
            .session_manager
            .complete_external_task(&req.session_id, &req.task_id, result)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        if !found {
            return Ok(Response::new(CompleteExternalTaskResponse {
                success: false,
                error: format!(
                    "No pending external task found for task_id: {}",
                    req.task_id
                ),
            }));
        }

        Ok(Response::new(CompleteExternalTaskResponse {
            success: true,
            error: String::new(),
        }))
    }

    async fn list_pending_external_tasks(
        &self,
        request: Request<ListPendingExternalTasksRequest>,
    ) -> Result<Response<ListPendingExternalTasksResponse>, Status> {
        let req = request.into_inner();

        let tasks = self
            .session_manager
            .pending_external_tasks(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let proto_tasks = tasks
            .iter()
            .map(convert::internal_external_task_to_proto)
            .collect();

        Ok(Response::new(ListPendingExternalTasksResponse {
            tasks: proto_tasks,
        }))
    }

    // ========================================================================
    // Permission System (Allow/Deny/Ask Rules)
    // ========================================================================

    async fn set_permission_policy(
        &self,
        request: Request<SetPermissionPolicyRequest>,
    ) -> Result<Response<SetPermissionPolicyResponse>, Status> {
        let req = request.into_inner();

        let proto_policy = req
            .policy
            .ok_or_else(|| Status::invalid_argument("Policy is required"))?;

        let internal_policy = convert::proto_permission_policy_to_internal(&proto_policy);

        let policy = self
            .session_manager
            .set_permission_policy(&req.session_id, internal_policy)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(SetPermissionPolicyResponse {
            success: true,
            policy: Some(convert::internal_permission_policy_to_proto(&policy)),
        }))
    }

    async fn get_permission_policy(
        &self,
        request: Request<GetPermissionPolicyRequest>,
    ) -> Result<Response<GetPermissionPolicyResponse>, Status> {
        let req = request.into_inner();

        let policy = self
            .session_manager
            .get_permission_policy(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(GetPermissionPolicyResponse {
            policy: Some(convert::internal_permission_policy_to_proto(&policy)),
        }))
    }

    async fn check_permission(
        &self,
        request: Request<CheckPermissionRequest>,
    ) -> Result<Response<CheckPermissionResponse>, Status> {
        let req = request.into_inner();

        let args: serde_json::Value =
            serde_json::from_str(&req.arguments).unwrap_or(serde_json::json!({}));

        let decision = self
            .session_manager
            .check_permission(&req.session_id, &req.tool_name, &args)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        // Get matching rules for debugging
        let session_lock = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(e.to_string()))?;

        let session = session_lock.read().await;
        let policy = session.permission_policy.read().await;
        let matching = policy.get_matching_rules(&req.tool_name, &args);

        let mut matching_rules = Vec::new();
        for rule in matching.deny {
            matching_rules.push(format!("deny:{}", rule));
        }
        for rule in matching.allow {
            matching_rules.push(format!("allow:{}", rule));
        }
        for rule in matching.ask {
            matching_rules.push(format!("ask:{}", rule));
        }

        Ok(Response::new(CheckPermissionResponse {
            decision: convert::internal_permission_decision_to_proto(decision),
            matching_rules,
        }))
    }

    async fn add_permission_rule(
        &self,
        request: Request<AddPermissionRuleRequest>,
    ) -> Result<Response<AddPermissionRuleResponse>, Status> {
        let req = request.into_inner();

        self.session_manager
            .add_permission_rule(&req.session_id, &req.rule_type, &req.rule)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(AddPermissionRuleResponse {
            success: true,
            error: String::new(),
        }))
    }

    // ========================================================================
    // Todo/Task Tracking
    // ========================================================================

    async fn get_todos(
        &self,
        request: Request<GetTodosRequest>,
    ) -> Result<Response<GetTodosResponse>, Status> {
        let req = request.into_inner();

        let todos = self
            .session_manager
            .get_todos(&req.session_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let proto_todos = todos.iter().map(convert::internal_todo_to_proto).collect();

        Ok(Response::new(GetTodosResponse { todos: proto_todos }))
    }

    async fn set_todos(
        &self,
        request: Request<SetTodosRequest>,
    ) -> Result<Response<SetTodosResponse>, Status> {
        let req = request.into_inner();

        let internal_todos: Vec<crate::todo::Todo> = req
            .todos
            .iter()
            .map(convert::proto_todo_to_internal)
            .collect();

        let updated_todos = self
            .session_manager
            .set_todos(&req.session_id, internal_todos)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let proto_todos = updated_todos
            .iter()
            .map(convert::internal_todo_to_proto)
            .collect();

        Ok(Response::new(SetTodosResponse {
            success: true,
            todos: proto_todos,
        }))
    }

    // ========================================================================
    // Provider Configuration
    // ========================================================================

    async fn list_providers(
        &self,
        _request: Request<ListProvidersRequest>,
    ) -> Result<Response<ListProvidersResponse>, Status> {
        let config = self.provider_config.read().await;

        let providers = config
            .providers
            .iter()
            .map(convert::internal_provider_config_to_proto)
            .collect();

        Ok(Response::new(ListProvidersResponse {
            providers,
            default_provider: config.default_provider.clone(),
            default_model: config.default_model.clone(),
        }))
    }

    async fn get_provider(
        &self,
        request: Request<GetProviderRequest>,
    ) -> Result<Response<GetProviderResponse>, Status> {
        let req = request.into_inner();
        let config = self.provider_config.read().await;

        let provider = config
            .find_provider(&req.name)
            .ok_or_else(|| Status::not_found(format!("Provider '{}' not found", req.name)))?;

        Ok(Response::new(GetProviderResponse {
            provider: Some(convert::internal_provider_config_to_proto(provider)),
        }))
    }

    async fn add_provider(
        &self,
        request: Request<AddProviderRequest>,
    ) -> Result<Response<AddProviderResponse>, Status> {
        let req = request.into_inner();
        let proto_provider = req
            .provider
            .ok_or_else(|| Status::invalid_argument("Provider is required"))?;

        let internal_provider = convert::proto_provider_info_to_internal(&proto_provider);

        let mut config = self.provider_config.write().await;

        // Check if provider already exists
        if config.find_provider(&internal_provider.name).is_some() {
            return Ok(Response::new(AddProviderResponse {
                success: false,
                error: format!("Provider '{}' already exists", internal_provider.name),
                provider: None,
            }));
        }

        config.providers.push(internal_provider.clone());

        tracing::info!("Added provider: {}", internal_provider.name);

        // Release lock before persisting
        drop(config);
        self.persist_config().await;

        Ok(Response::new(AddProviderResponse {
            success: true,
            error: String::new(),
            provider: Some(convert::internal_provider_config_to_proto(
                &internal_provider,
            )),
        }))
    }

    async fn update_provider(
        &self,
        request: Request<UpdateProviderRequest>,
    ) -> Result<Response<UpdateProviderResponse>, Status> {
        let req = request.into_inner();
        let proto_provider = req
            .provider
            .ok_or_else(|| Status::invalid_argument("Provider is required"))?;

        let internal_provider = convert::proto_provider_info_to_internal(&proto_provider);

        let mut config = self.provider_config.write().await;

        // Find and update the provider
        let found = config
            .providers
            .iter_mut()
            .find(|p| p.name == internal_provider.name);

        match found {
            Some(existing) => {
                *existing = internal_provider.clone();
                tracing::info!("Updated provider: {}", internal_provider.name);

                // Release lock before persisting
                drop(config);
                self.persist_config().await;

                Ok(Response::new(UpdateProviderResponse {
                    success: true,
                    error: String::new(),
                    provider: Some(convert::internal_provider_config_to_proto(
                        &internal_provider,
                    )),
                }))
            }
            None => Ok(Response::new(UpdateProviderResponse {
                success: false,
                error: format!("Provider '{}' not found", internal_provider.name),
                provider: None,
            })),
        }
    }

    async fn remove_provider(
        &self,
        request: Request<RemoveProviderRequest>,
    ) -> Result<Response<RemoveProviderResponse>, Status> {
        let req = request.into_inner();

        let mut config = self.provider_config.write().await;

        let initial_len = config.providers.len();
        config.providers.retain(|p| p.name != req.name);

        if config.providers.len() < initial_len {
            // Clear default if removed provider was the default
            if config.default_provider.as_ref() == Some(&req.name) {
                config.default_provider = None;
                config.default_model = None;
            }

            tracing::info!("Removed provider: {}", req.name);

            // Release lock before persisting
            drop(config);
            self.persist_config().await;

            Ok(Response::new(RemoveProviderResponse {
                success: true,
                error: String::new(),
            }))
        } else {
            Ok(Response::new(RemoveProviderResponse {
                success: false,
                error: format!("Provider '{}' not found", req.name),
            }))
        }
    }

    async fn set_default_model(
        &self,
        request: Request<SetDefaultModelRequest>,
    ) -> Result<Response<SetDefaultModelResponse>, Status> {
        let req = request.into_inner();

        let mut config = self.provider_config.write().await;

        // Validate provider exists
        let provider = config.find_provider(&req.provider);
        if provider.is_none() {
            return Ok(Response::new(SetDefaultModelResponse {
                success: false,
                error: format!("Provider '{}' not found", req.provider),
                provider: String::new(),
                model: String::new(),
            }));
        }

        // Validate model exists in provider
        let Some(provider) = provider else {
            // Unreachable due to the is_none() check above, but safe
            return Ok(Response::new(SetDefaultModelResponse {
                success: false,
                error: format!("Provider '{}' not found", req.provider),
                provider: String::new(),
                model: String::new(),
            }));
        };
        if provider.find_model(&req.model).is_none() {
            return Ok(Response::new(SetDefaultModelResponse {
                success: false,
                error: format!(
                    "Model '{}' not found in provider '{}'",
                    req.model, req.provider
                ),
                provider: String::new(),
                model: String::new(),
            }));
        }

        config.default_provider = Some(req.provider.clone());
        config.default_model = Some(req.model.clone());

        tracing::info!(
            "Set default model: provider={}, model={}",
            req.provider,
            req.model
        );

        // Release lock before persisting
        drop(config);
        self.persist_config().await;

        Ok(Response::new(SetDefaultModelResponse {
            success: true,
            error: String::new(),
            provider: req.provider,
            model: req.model,
        }))
    }

    async fn get_default_model(
        &self,
        _request: Request<GetDefaultModelRequest>,
    ) -> Result<Response<GetDefaultModelResponse>, Status> {
        let config = self.provider_config.read().await;

        Ok(Response::new(GetDefaultModelResponse {
            provider: config.default_provider.clone(),
            model: config.default_model.clone(),
        }))
    }

    // ========================================================================
    // Planning & Goal Tracking (Phase 1)
    // ========================================================================

    async fn create_plan(
        &self,
        request: Request<CreatePlanRequest>,
    ) -> Result<Response<CreatePlanResponse>, Status> {
        let req = request.into_inner();

        // Get session
        let session = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Generate execution plan using LLM if available, otherwise fallback to heuristics
        let plan = match self
            .session_manager
            .get_llm_for_session(&req.session_id)
            .await
        {
            Ok(Some(llm_client)) => {
                match crate::planning::LlmPlanner::create_plan(&llm_client, &req.prompt).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("LLM plan creation failed, using fallback: {}", e);
                        crate::planning::LlmPlanner::fallback_plan(&req.prompt)
                    }
                }
            }
            _ => crate::planning::LlmPlanner::fallback_plan(&req.prompt),
        };

        // Store plan in session
        let session_guard = session.read().await;
        let mut current_plan = session_guard.current_plan.write().await;
        *current_plan = Some(plan.clone());

        // Convert to proto
        let proto_plan = ExecutionPlan {
            goal: plan.goal,
            steps: plan
                .steps
                .iter()
                .map(|step| PlanStep {
                    id: step.id.clone(),
                    description: step.description.clone(),
                    tool: step.tool.clone(),
                    dependencies: step.dependencies.clone(),
                    status: match step.status {
                        crate::planning::StepStatus::Pending => 0,
                        crate::planning::StepStatus::InProgress => 1,
                        crate::planning::StepStatus::Completed => 2,
                        crate::planning::StepStatus::Failed => 3,
                        crate::planning::StepStatus::Skipped => 4,
                    },
                    success_criteria: step
                        .success_criteria
                        .as_ref()
                        .map(|s| vec![s.clone()])
                        .unwrap_or_default(),
                })
                .collect(),
            complexity: match plan.complexity {
                crate::planning::Complexity::Simple => 0,
                crate::planning::Complexity::Medium => 1,
                crate::planning::Complexity::Complex => 2,
                crate::planning::Complexity::VeryComplex => 3,
            },
            required_tools: plan.required_tools,
            estimated_steps: plan.estimated_steps as u32,
        };

        Ok(Response::new(CreatePlanResponse {
            plan: Some(proto_plan),
        }))
    }

    async fn get_plan(
        &self,
        request: Request<GetPlanRequest>,
    ) -> Result<Response<GetPlanResponse>, Status> {
        let req = request.into_inner();

        // Get session
        let session = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Get current plan
        let session_guard = session.read().await;
        let current_plan = session_guard.current_plan.read().await;

        let plan = current_plan
            .as_ref()
            .ok_or_else(|| Status::not_found("No plan found for this session"))?;

        // Convert to proto
        let proto_plan = ExecutionPlan {
            goal: plan.goal.clone(),
            steps: plan
                .steps
                .iter()
                .map(|step| PlanStep {
                    id: step.id.clone(),
                    description: step.description.clone(),
                    tool: step.tool.clone(),
                    dependencies: step.dependencies.clone(),
                    status: match step.status {
                        crate::planning::StepStatus::Pending => 0,
                        crate::planning::StepStatus::InProgress => 1,
                        crate::planning::StepStatus::Completed => 2,
                        crate::planning::StepStatus::Failed => 3,
                        crate::planning::StepStatus::Skipped => 4,
                    },
                    success_criteria: step
                        .success_criteria
                        .as_ref()
                        .map(|s| vec![s.clone()])
                        .unwrap_or_default(),
                })
                .collect(),
            complexity: match plan.complexity {
                crate::planning::Complexity::Simple => 0,
                crate::planning::Complexity::Medium => 1,
                crate::planning::Complexity::Complex => 2,
                crate::planning::Complexity::VeryComplex => 3,
            },
            required_tools: plan.required_tools.clone(),
            estimated_steps: plan.estimated_steps as u32,
        };

        Ok(Response::new(GetPlanResponse {
            plan: Some(proto_plan),
        }))
    }

    async fn extract_goal(
        &self,
        request: Request<ExtractGoalRequest>,
    ) -> Result<Response<ExtractGoalResponse>, Status> {
        let req = request.into_inner();

        // Get session
        let _session = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Extract goal from prompt using LLM if available, otherwise fallback to heuristics
        let goal = match self
            .session_manager
            .get_llm_for_session(&req.session_id)
            .await
        {
            Ok(Some(llm_client)) => {
                match crate::planning::LlmPlanner::extract_goal(&llm_client, &req.prompt).await {
                    Ok(g) => g,
                    Err(e) => {
                        tracing::warn!("LLM goal extraction failed, using fallback: {}", e);
                        crate::planning::LlmPlanner::fallback_goal(&req.prompt)
                    }
                }
            }
            _ => crate::planning::LlmPlanner::fallback_goal(&req.prompt),
        };

        // Convert to proto
        let proto_goal = AgentGoal {
            description: goal.description,
            success_criteria: goal.success_criteria,
            progress: goal.progress,
            achieved: goal.achieved,
            created_at: goal.created_at,
            achieved_at: goal.achieved_at,
        };

        Ok(Response::new(ExtractGoalResponse {
            goal: Some(proto_goal),
        }))
    }

    async fn check_goal_achievement(
        &self,
        request: Request<CheckGoalAchievementRequest>,
    ) -> Result<Response<CheckGoalAchievementResponse>, Status> {
        let req = request.into_inner();

        // Get session
        let _session = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        let goal = req
            .goal
            .ok_or_else(|| Status::invalid_argument("Goal is required"))?;

        // Evaluate achievement using LLM if available, otherwise fallback to heuristics
        let internal_goal = crate::planning::AgentGoal::new(&goal.description)
            .with_criteria(goal.success_criteria.clone());

        let result = match self
            .session_manager
            .get_llm_for_session(&req.session_id)
            .await
        {
            Ok(Some(llm_client)) => {
                match crate::planning::LlmPlanner::check_achievement(
                    &llm_client,
                    &internal_goal,
                    &req.current_state,
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("LLM achievement check failed, using fallback: {}", e);
                        crate::planning::LlmPlanner::fallback_check_achievement(
                            &internal_goal,
                            &req.current_state,
                        )
                    }
                }
            }
            _ => crate::planning::LlmPlanner::fallback_check_achievement(
                &internal_goal,
                &req.current_state,
            ),
        };

        Ok(Response::new(CheckGoalAchievementResponse {
            achieved: result.achieved,
            progress: result.progress,
            remaining_criteria: result.remaining_criteria,
        }))
    }

    // ========================================================================
    // Memory System (Phase 3)
    // ========================================================================

    async fn store_memory(
        &self,
        request: Request<StoreMemoryRequest>,
    ) -> Result<Response<StoreMemoryResponse>, Status> {
        let req = request.into_inner();

        // Get session
        let session = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Extract memory from request
        let proto_memory = req
            .memory
            .ok_or_else(|| Status::invalid_argument("Memory is required"))?;

        // Convert proto MemoryItem to internal MemoryItem
        let memory_item = crate::memory::MemoryItem {
            id: if proto_memory.id.is_empty() {
                uuid::Uuid::new_v4().to_string()
            } else {
                proto_memory.id
            },
            content: proto_memory.content,
            timestamp: chrono::DateTime::from_timestamp(proto_memory.timestamp, 0)
                .unwrap_or_else(chrono::Utc::now),
            importance: proto_memory.importance.clamp(0.0, 1.0),
            tags: proto_memory.tags,
            memory_type: match proto_memory.memory_type {
                1 => crate::memory::MemoryType::Episodic,
                2 => crate::memory::MemoryType::Semantic,
                3 => crate::memory::MemoryType::Procedural,
                4 => crate::memory::MemoryType::Working,
                _ => crate::memory::MemoryType::Episodic,
            },
            metadata: proto_memory.metadata,
            access_count: proto_memory.access_count,
            last_accessed: proto_memory
                .last_accessed
                .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)),
        };

        // Store memory
        let memory_id = memory_item.id.clone();
        let memory_type_str = match memory_item.memory_type {
            crate::memory::MemoryType::Episodic => "episodic",
            crate::memory::MemoryType::Semantic => "semantic",
            crate::memory::MemoryType::Procedural => "procedural",
            crate::memory::MemoryType::Working => "working",
        };
        let importance = memory_item.importance;
        let tags = memory_item.tags.clone();

        let session_guard = session.read().await;
        let memory = session_guard.memory.read().await;
        memory
            .remember(memory_item)
            .await
            .map_err(|e| Status::internal(format!("Failed to store memory: {}", e)))?;

        // Emit memory stored event
        let _ = session_guard
            .event_tx()
            .send(crate::agent::AgentEvent::MemoryStored {
                memory_id: memory_id.clone(),
                memory_type: memory_type_str.to_string(),
                importance,
                tags,
            });

        Ok(Response::new(StoreMemoryResponse {
            success: true,
            memory_id,
        }))
    }

    async fn retrieve_memory(
        &self,
        request: Request<RetrieveMemoryRequest>,
    ) -> Result<Response<RetrieveMemoryResponse>, Status> {
        let req = request.into_inner();

        // Get session
        let session = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Retrieve memory from store
        let session_guard = session.read().await;
        let memory = session_guard.memory.read().await;

        // Access the underlying store to retrieve by ID
        let memory_item = memory
            .store()
            .retrieve(&req.memory_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to retrieve memory: {}", e)))?;

        // Convert to proto MemoryItem
        let proto_memory = memory_item.map(|item| MemoryItem {
            id: item.id,
            content: item.content,
            timestamp: item.timestamp.timestamp(),
            importance: item.importance,
            tags: item.tags,
            memory_type: match item.memory_type {
                crate::memory::MemoryType::Episodic => 1,
                crate::memory::MemoryType::Semantic => 2,
                crate::memory::MemoryType::Procedural => 3,
                crate::memory::MemoryType::Working => 4,
            },
            metadata: item.metadata,
            access_count: item.access_count,
            last_accessed: item.last_accessed.map(|ts| ts.timestamp()),
        });

        Ok(Response::new(RetrieveMemoryResponse {
            memory: proto_memory,
        }))
    }

    async fn search_memories(
        &self,
        request: Request<SearchMemoriesRequest>,
    ) -> Result<Response<SearchMemoriesResponse>, Status> {
        let req = request.into_inner();

        // Get session
        let session = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Search memories
        let session_guard = session.read().await;
        let memory = session_guard.memory.read().await;
        let limit = if req.limit == 0 {
            10
        } else {
            req.limit as usize
        };

        let mut memories = if !req.tags.is_empty() {
            // Search by tags
            memory
                .recall_by_tags(&req.tags, limit)
                .await
                .map_err(|e| Status::internal(format!("Failed to search memories: {}", e)))?
        } else if let Some(query) = req.query {
            // Search by query
            memory
                .recall_similar(&query, limit)
                .await
                .map_err(|e| Status::internal(format!("Failed to search memories: {}", e)))?
        } else {
            // Return recent memories (up to limit)
            memory
                .get_recent(limit)
                .await
                .map_err(|e| Status::internal(format!("Failed to get memories: {}", e)))?
        };

        // Filter by importance if specified
        if let Some(min_importance) = req.min_importance {
            memories.retain(|m| m.importance >= min_importance);
        }

        // Convert to proto MemoryItems
        let proto_memories: Vec<_> = memories
            .iter()
            .map(|item| MemoryItem {
                id: item.id.clone(),
                content: item.content.clone(),
                timestamp: item.timestamp.timestamp(),
                importance: item.importance,
                tags: item.tags.clone(),
                memory_type: match item.memory_type {
                    crate::memory::MemoryType::Episodic => 1,
                    crate::memory::MemoryType::Semantic => 2,
                    crate::memory::MemoryType::Procedural => 3,
                    crate::memory::MemoryType::Working => 4,
                },
                metadata: item.metadata.clone(),
                access_count: item.access_count,
                last_accessed: item.last_accessed.map(|ts| ts.timestamp()),
            })
            .collect();

        let total_count = proto_memories.len() as u32;

        Ok(Response::new(SearchMemoriesResponse {
            memories: proto_memories,
            total_count,
        }))
    }

    async fn get_memory_stats(
        &self,
        request: Request<GetMemoryStatsRequest>,
    ) -> Result<Response<GetMemoryStatsResponse>, Status> {
        let req = request.into_inner();

        // Get session
        let session = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Get memory statistics
        let session_guard = session.read().await;
        let memory = session_guard.memory.read().await;
        let stats = memory
            .stats()
            .await
            .map_err(|e| Status::internal(format!("Failed to get memory stats: {}", e)))?;

        Ok(Response::new(GetMemoryStatsResponse {
            stats: Some(MemoryStats {
                long_term_count: stats.long_term_count as u64,
                short_term_count: stats.short_term_count as u64,
                working_count: stats.working_count as u64,
            }),
        }))
    }

    async fn clear_memories(
        &self,
        request: Request<ClearMemoriesRequest>,
    ) -> Result<Response<ClearMemoriesResponse>, Status> {
        let req = request.into_inner();

        // Get session
        let session = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Clear memories
        let session_guard = session.read().await;
        let memory = session_guard.memory.read().await;

        let mut cleared_count = 0u64;

        if req.clear_working {
            let working_count = memory.working_count().await;
            memory.clear_working().await;
            cleared_count += working_count as u64;
        }

        if req.clear_short_term {
            let short_term_count = memory.short_term_count().await;
            memory.clear_short_term().await;
            cleared_count += short_term_count as u64;
        }

        if req.clear_long_term {
            let long_term_count = memory.store().count().await.map_err(|e| {
                Status::internal(format!("Failed to count long-term memories: {}", e))
            })?;
            memory.store().clear().await.map_err(|e| {
                Status::internal(format!("Failed to clear long-term memories: {}", e))
            })?;
            cleared_count += long_term_count as u64;
        }

        Ok(Response::new(ClearMemoriesResponse {
            success: true,
            cleared_count,
        }))
    }

    // ========================================================================
    // MCP (Model Context Protocol)
    // ========================================================================

    async fn register_mcp_server(
        &self,
        request: Request<RegisterMcpServerRequest>,
    ) -> Result<Response<RegisterMcpServerResponse>, Status> {
        let req = request.into_inner();

        let config_proto = req
            .config
            .ok_or_else(|| Status::invalid_argument("Missing config"))?;

        // Convert proto to internal config
        let transport = config_proto
            .transport
            .ok_or_else(|| Status::invalid_argument("Missing transport"))?;

        let transport_config = match transport.transport {
            Some(proto::mcp_transport::Transport::Stdio(stdio)) => McpTransportConfig::Stdio {
                command: stdio.command,
                args: stdio.args,
            },
            Some(proto::mcp_transport::Transport::Http(http)) => McpTransportConfig::Http {
                url: http.url,
                headers: http.headers,
            },
            None => return Err(Status::invalid_argument("Missing transport type")),
        };

        let config = McpServerConfig {
            name: config_proto.name.clone(),
            transport: transport_config,
            enabled: config_proto.enabled,
            env: config_proto.env,
            oauth: None,
        };

        self.mcp_manager.register_server(config).await;

        tracing::info!("Registered MCP server: {}", config_proto.name);

        Ok(Response::new(RegisterMcpServerResponse {
            success: true,
            message: format!("Registered MCP server: {}", config_proto.name),
        }))
    }

    async fn connect_mcp_server(
        &self,
        request: Request<ConnectMcpServerRequest>,
    ) -> Result<Response<ConnectMcpServerResponse>, Status> {
        let req = request.into_inner();

        match self.mcp_manager.connect(&req.name).await {
            Ok(()) => {
                // Get tool names
                let tools = self.mcp_manager.get_all_tools().await;
                let tool_names: Vec<String> = tools
                    .iter()
                    .filter(|(name, _)| name.starts_with(&format!("mcp__{}_", req.name)))
                    .map(|(name, _)| name.clone())
                    .collect();

                tracing::info!(
                    "Connected to MCP server '{}' with {} tools",
                    req.name,
                    tool_names.len()
                );

                Ok(Response::new(ConnectMcpServerResponse {
                    success: true,
                    message: format!("Connected to MCP server: {}", req.name),
                    tool_names,
                }))
            }
            Err(e) => {
                tracing::error!("Failed to connect to MCP server '{}': {}", req.name, e);
                Ok(Response::new(ConnectMcpServerResponse {
                    success: false,
                    message: format!("Failed to connect: {}", e),
                    tool_names: vec![],
                }))
            }
        }
    }

    async fn disconnect_mcp_server(
        &self,
        request: Request<DisconnectMcpServerRequest>,
    ) -> Result<Response<DisconnectMcpServerResponse>, Status> {
        let req = request.into_inner();

        match self.mcp_manager.disconnect(&req.name).await {
            Ok(()) => {
                tracing::info!("Disconnected from MCP server: {}", req.name);
                Ok(Response::new(DisconnectMcpServerResponse { success: true }))
            }
            Err(e) => {
                tracing::error!("Failed to disconnect from MCP server '{}': {}", req.name, e);
                Ok(Response::new(DisconnectMcpServerResponse {
                    success: false,
                }))
            }
        }
    }

    async fn list_mcp_servers(
        &self,
        _request: Request<ListMcpServersRequest>,
    ) -> Result<Response<ListMcpServersResponse>, Status> {
        let status = self.mcp_manager.get_status().await;

        let servers: Vec<McpServerInfo> = status
            .into_values()
            .map(|s| McpServerInfo {
                name: s.name,
                connected: s.connected,
                enabled: s.enabled,
                tool_count: s.tool_count as u32,
                error: s.error,
            })
            .collect();

        Ok(Response::new(ListMcpServersResponse { servers }))
    }

    async fn get_mcp_tools(
        &self,
        request: Request<GetMcpToolsRequest>,
    ) -> Result<Response<GetMcpToolsResponse>, Status> {
        let req = request.into_inner();

        let all_tools = self.mcp_manager.get_all_tools().await;

        let tools: Vec<McpToolInfo> = all_tools
            .into_iter()
            .filter(|(full_name, _)| {
                if let Some(ref server_name) = req.server_name {
                    full_name.starts_with(&format!("mcp__{}_", server_name))
                } else {
                    true
                }
            })
            .map(|(full_name, tool)| {
                // Parse server name from full_name (mcp__server__tool)
                let parts: Vec<&str> = full_name
                    .strip_prefix("mcp__")
                    .unwrap_or(&full_name)
                    .splitn(2, "__")
                    .collect();
                let (server_name, tool_name) = if parts.len() == 2 {
                    (parts[0].to_string(), parts[1].to_string())
                } else {
                    ("unknown".to_string(), full_name.clone())
                };

                McpToolInfo {
                    full_name,
                    server_name,
                    tool_name,
                    description: tool.description.unwrap_or_default(),
                    input_schema: serde_json::to_string(&tool.input_schema).unwrap_or_default(),
                }
            })
            .collect();

        Ok(Response::new(GetMcpToolsResponse { tools }))
    }

    // ========================================================================
    // LSP (Language Server Protocol) RPCs
    // ========================================================================

    async fn start_lsp_server(
        &self,
        request: Request<StartLspServerRequest>,
    ) -> Result<Response<StartLspServerResponse>, Status> {
        let req = request.into_inner();

        // Set workspace root if provided
        if !req.root_uri.is_empty() {
            // Convert file:// URI to path
            let root_path = req
                .root_uri
                .strip_prefix("file://")
                .unwrap_or(&req.root_uri);
            self.lsp_manager.set_workspace(root_path).await;
        }

        match self.lsp_manager.start_server(&req.language).await {
            Ok(()) => {
                // Get server info
                let running = self.lsp_manager.list_running().await;
                let server_info = if running.contains(&req.language) {
                    Some(LspServerInfo {
                        language: req.language.clone(),
                        name: format!("{}-language-server", req.language),
                        version: None,
                        running: true,
                    })
                } else {
                    None
                };

                Ok(Response::new(StartLspServerResponse {
                    success: true,
                    message: format!("LSP server for {} started", req.language),
                    server_info,
                }))
            }
            Err(e) => Ok(Response::new(StartLspServerResponse {
                success: false,
                message: format!("Failed to start LSP server: {}", e),
                server_info: None,
            })),
        }
    }

    async fn stop_lsp_server(
        &self,
        request: Request<StopLspServerRequest>,
    ) -> Result<Response<StopLspServerResponse>, Status> {
        let req = request.into_inner();

        match self.lsp_manager.stop_server(&req.language).await {
            Ok(()) => Ok(Response::new(StopLspServerResponse { success: true })),
            Err(_) => Ok(Response::new(StopLspServerResponse { success: false })),
        }
    }

    async fn list_lsp_servers(
        &self,
        _request: Request<ListLspServersRequest>,
    ) -> Result<Response<ListLspServersResponse>, Status> {
        let running = self.lsp_manager.list_running().await;

        let servers: Vec<LspServerInfo> = running
            .into_iter()
            .map(|language| LspServerInfo {
                language: language.clone(),
                name: format!("{}-language-server", language),
                version: None,
                running: true,
            })
            .collect();

        Ok(Response::new(ListLspServersResponse { servers }))
    }

    async fn lsp_hover(
        &self,
        request: Request<LspHoverRequest>,
    ) -> Result<Response<LspHoverResponse>, Status> {
        let req = request.into_inner();
        let path = std::path::Path::new(&req.file_path);

        let client = match self.lsp_manager.ensure_server_for_file(path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Response::new(LspHoverResponse {
                    found: false,
                    content: format!("LSP not available: {}", e),
                    range: None,
                }));
            }
        };

        let uri = format!("file://{}", req.file_path);

        match client.hover(&uri, req.line, req.column).await {
            Ok(Some(hover)) => {
                let content = format_hover_contents(&hover.contents);
                let range = hover.range.map(|r| LspRange {
                    start: Some(LspPosition {
                        line: r.start.line,
                        character: r.start.character,
                    }),
                    end: Some(LspPosition {
                        line: r.end.line,
                        character: r.end.character,
                    }),
                });
                Ok(Response::new(LspHoverResponse {
                    found: true,
                    content,
                    range,
                }))
            }
            Ok(None) => Ok(Response::new(LspHoverResponse {
                found: false,
                content: String::new(),
                range: None,
            })),
            Err(e) => Ok(Response::new(LspHoverResponse {
                found: false,
                content: format!("Hover failed: {}", e),
                range: None,
            })),
        }
    }

    async fn lsp_definition(
        &self,
        request: Request<LspDefinitionRequest>,
    ) -> Result<Response<LspDefinitionResponse>, Status> {
        let req = request.into_inner();
        let path = std::path::Path::new(&req.file_path);

        let client = match self.lsp_manager.ensure_server_for_file(path).await {
            Ok(c) => c,
            Err(_) => {
                return Ok(Response::new(LspDefinitionResponse { locations: vec![] }));
            }
        };

        let uri = format!("file://{}", req.file_path);

        match client.goto_definition(&uri, req.line, req.column).await {
            Ok(Some(response)) => {
                let locations = convert_definition_response(&response);
                Ok(Response::new(LspDefinitionResponse { locations }))
            }
            Ok(None) => Ok(Response::new(LspDefinitionResponse { locations: vec![] })),
            Err(_) => Ok(Response::new(LspDefinitionResponse { locations: vec![] })),
        }
    }

    async fn lsp_references(
        &self,
        request: Request<LspReferencesRequest>,
    ) -> Result<Response<LspReferencesResponse>, Status> {
        let req = request.into_inner();
        let path = std::path::Path::new(&req.file_path);

        let client = match self.lsp_manager.ensure_server_for_file(path).await {
            Ok(c) => c,
            Err(_) => {
                return Ok(Response::new(LspReferencesResponse { locations: vec![] }));
            }
        };

        let uri = format!("file://{}", req.file_path);

        match client
            .find_references(&uri, req.line, req.column, req.include_declaration)
            .await
        {
            Ok(locs) => {
                let locations = locs
                    .into_iter()
                    .map(|loc| LspLocation {
                        uri: loc.uri,
                        range: Some(LspRange {
                            start: Some(LspPosition {
                                line: loc.range.start.line,
                                character: loc.range.start.character,
                            }),
                            end: Some(LspPosition {
                                line: loc.range.end.line,
                                character: loc.range.end.character,
                            }),
                        }),
                    })
                    .collect();
                Ok(Response::new(LspReferencesResponse { locations }))
            }
            Err(_) => Ok(Response::new(LspReferencesResponse { locations: vec![] })),
        }
    }

    async fn lsp_symbols(
        &self,
        request: Request<LspSymbolsRequest>,
    ) -> Result<Response<LspSymbolsResponse>, Status> {
        let req = request.into_inner();
        let limit = if req.limit == 0 {
            20
        } else {
            req.limit as usize
        };

        let running = self.lsp_manager.list_running().await;
        let mut all_symbols = Vec::new();

        for language in running {
            if let Some(client) = self.lsp_manager.get_client(&language).await {
                if let Ok(symbols) = client.workspace_symbols(&req.query).await {
                    for sym in symbols {
                        all_symbols.push(LspSymbol {
                            name: sym.name,
                            kind: format!("{:?}", sym.kind),
                            location: Some(LspLocation {
                                uri: sym.location.uri,
                                range: Some(LspRange {
                                    start: Some(LspPosition {
                                        line: sym.location.range.start.line,
                                        character: sym.location.range.start.character,
                                    }),
                                    end: Some(LspPosition {
                                        line: sym.location.range.end.line,
                                        character: sym.location.range.end.character,
                                    }),
                                }),
                            }),
                            container_name: sym.container_name,
                        });
                    }
                }
            }
        }

        all_symbols.truncate(limit);
        Ok(Response::new(LspSymbolsResponse {
            symbols: all_symbols,
        }))
    }

    async fn lsp_diagnostics(
        &self,
        request: Request<LspDiagnosticsRequest>,
    ) -> Result<Response<LspDiagnosticsResponse>, Status> {
        let req = request.into_inner();

        if let Some(file_path) = req.file_path {
            let path = std::path::Path::new(&file_path);
            let client = match self.lsp_manager.ensure_server_for_file(path).await {
                Ok(c) => c,
                Err(_) => {
                    return Ok(Response::new(LspDiagnosticsResponse {
                        diagnostics: vec![],
                    }));
                }
            };

            let uri = format!("file://{}", file_path);
            let diags = client.get_diagnostics(&uri).await;

            let diagnostics = diags
                .into_iter()
                .map(|d| LspDiagnostic {
                    uri: uri.clone(),
                    range: Some(LspRange {
                        start: Some(LspPosition {
                            line: d.range.start.line,
                            character: d.range.start.character,
                        }),
                        end: Some(LspPosition {
                            line: d.range.end.line,
                            character: d.range.end.character,
                        }),
                    }),
                    severity: match d.severity {
                        Some(crate::lsp::protocol::DiagnosticSeverity::Error) => {
                            "error".to_string()
                        }
                        Some(crate::lsp::protocol::DiagnosticSeverity::Warning) => {
                            "warning".to_string()
                        }
                        Some(crate::lsp::protocol::DiagnosticSeverity::Information) => {
                            "info".to_string()
                        }
                        Some(crate::lsp::protocol::DiagnosticSeverity::Hint) => "hint".to_string(),
                        None => "unknown".to_string(),
                    },
                    message: d.message,
                    code: match d.code {
                        Some(crate::lsp::protocol::DiagnosticCode::String(s)) => Some(s),
                        Some(crate::lsp::protocol::DiagnosticCode::Number(n)) => {
                            Some(n.to_string())
                        }
                        None => None,
                    },
                    source: d.source,
                })
                .collect();

            Ok(Response::new(LspDiagnosticsResponse { diagnostics }))
        } else {
            Ok(Response::new(LspDiagnosticsResponse {
                diagnostics: vec![],
            }))
        }
    }

    // ========================================================================
    // Cron (Scheduled Tasks)
    // ========================================================================

    async fn list_cron_jobs(
        &self,
        _request: Request<ListCronJobsRequest>,
    ) -> Result<Response<ListCronJobsResponse>, Status> {
        let manager = self.get_or_init_cron_manager().await?;
        let jobs = manager
            .list_jobs()
            .await
            .map_err(|e| Status::internal(format!("Failed to list jobs: {}", e)))?;

        let proto_jobs = jobs.into_iter().map(cron_job_to_proto).collect();
        Ok(Response::new(ListCronJobsResponse { jobs: proto_jobs }))
    }

    async fn create_cron_job(
        &self,
        request: Request<CreateCronJobRequest>,
    ) -> Result<Response<CreateCronJobResponse>, Status> {
        let req = request.into_inner();

        // Parse schedule (supports natural language)
        let cron_schedule = parse_natural(&req.schedule).unwrap_or_else(|_| req.schedule.clone());

        // Validate cron expression
        if let Err(e) = CronExpression::parse(&cron_schedule) {
            return Ok(Response::new(CreateCronJobResponse {
                success: false,
                job: None,
                error: format!("Invalid schedule: {}", e),
            }));
        }

        let manager = self.get_or_init_cron_manager().await?;
        match manager
            .add_job(&req.name, &cron_schedule, &req.command)
            .await
        {
            Ok(mut job) => {
                // Update timeout if specified
                if let Some(timeout) = req.timeout_ms {
                    if timeout != 60000 {
                        job = manager
                            .update_job(&job.id, None, None, Some(timeout))
                            .await
                            .map_err(|e| {
                                Status::internal(format!("Failed to set timeout: {}", e))
                            })?;
                    }
                }
                Ok(Response::new(CreateCronJobResponse {
                    success: true,
                    job: Some(cron_job_to_proto(job)),
                    error: String::new(),
                }))
            }
            Err(e) => Ok(Response::new(CreateCronJobResponse {
                success: false,
                job: None,
                error: format!("Failed to create job: {}", e),
            })),
        }
    }

    async fn get_cron_job(
        &self,
        request: Request<GetCronJobRequest>,
    ) -> Result<Response<GetCronJobResponse>, Status> {
        let req = request.into_inner();
        let manager = self.get_or_init_cron_manager().await?;

        let job = if let Some(id) = req.id {
            manager
                .get_job(&id)
                .await
                .map_err(|e| Status::internal(format!("Failed to get job: {}", e)))?
        } else if let Some(name) = req.name {
            manager
                .get_job_by_name(&name)
                .await
                .map_err(|e| Status::internal(format!("Failed to get job: {}", e)))?
        } else {
            return Err(Status::invalid_argument("Either id or name is required"));
        };

        Ok(Response::new(GetCronJobResponse {
            job: job.map(cron_job_to_proto),
        }))
    }

    async fn update_cron_job(
        &self,
        request: Request<UpdateCronJobRequest>,
    ) -> Result<Response<UpdateCronJobResponse>, Status> {
        let req = request.into_inner();

        // Parse and validate schedule if provided
        let cron_schedule = if let Some(schedule) = req.schedule {
            let parsed = parse_natural(&schedule).unwrap_or_else(|_| schedule.clone());
            if let Err(e) = CronExpression::parse(&parsed) {
                return Ok(Response::new(UpdateCronJobResponse {
                    success: false,
                    job: None,
                    error: format!("Invalid schedule: {}", e),
                }));
            }
            Some(parsed)
        } else {
            None
        };

        let manager = self.get_or_init_cron_manager().await?;
        match manager
            .update_job(
                &req.id,
                cron_schedule.as_deref(),
                req.command.as_deref(),
                req.timeout_ms,
            )
            .await
        {
            Ok(job) => Ok(Response::new(UpdateCronJobResponse {
                success: true,
                job: Some(cron_job_to_proto(job)),
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(UpdateCronJobResponse {
                success: false,
                job: None,
                error: format!("Failed to update job: {}", e),
            })),
        }
    }

    async fn pause_cron_job(
        &self,
        request: Request<PauseCronJobRequest>,
    ) -> Result<Response<PauseCronJobResponse>, Status> {
        let req = request.into_inner();
        let manager = self.get_or_init_cron_manager().await?;

        match manager.pause_job(&req.id).await {
            Ok(job) => Ok(Response::new(PauseCronJobResponse {
                success: true,
                job: Some(cron_job_to_proto(job)),
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(PauseCronJobResponse {
                success: false,
                job: None,
                error: format!("Failed to pause job: {}", e),
            })),
        }
    }

    async fn resume_cron_job(
        &self,
        request: Request<ResumeCronJobRequest>,
    ) -> Result<Response<ResumeCronJobResponse>, Status> {
        let req = request.into_inner();
        let manager = self.get_or_init_cron_manager().await?;

        match manager.resume_job(&req.id).await {
            Ok(job) => Ok(Response::new(ResumeCronJobResponse {
                success: true,
                job: Some(cron_job_to_proto(job)),
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(ResumeCronJobResponse {
                success: false,
                job: None,
                error: format!("Failed to resume job: {}", e),
            })),
        }
    }

    async fn delete_cron_job(
        &self,
        request: Request<DeleteCronJobRequest>,
    ) -> Result<Response<DeleteCronJobResponse>, Status> {
        let req = request.into_inner();
        let manager = self.get_or_init_cron_manager().await?;

        match manager.remove_job(&req.id).await {
            Ok(_) => Ok(Response::new(DeleteCronJobResponse {
                success: true,
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(DeleteCronJobResponse {
                success: false,
                error: format!("Failed to delete job: {}", e),
            })),
        }
    }

    async fn get_cron_history(
        &self,
        request: Request<GetCronHistoryRequest>,
    ) -> Result<Response<GetCronHistoryResponse>, Status> {
        let req = request.into_inner();
        let limit = req.limit.unwrap_or(10) as usize;
        let manager = self.get_or_init_cron_manager().await?;

        let executions = manager
            .get_history(&req.id, limit)
            .await
            .map_err(|e| Status::internal(format!("Failed to get history: {}", e)))?;

        let proto_executions = executions
            .into_iter()
            .map(cron_execution_to_proto)
            .collect();
        Ok(Response::new(GetCronHistoryResponse {
            executions: proto_executions,
        }))
    }

    async fn run_cron_job(
        &self,
        request: Request<RunCronJobRequest>,
    ) -> Result<Response<RunCronJobResponse>, Status> {
        let req = request.into_inner();
        let manager = self.get_or_init_cron_manager().await?;

        match manager.run_job(&req.id).await {
            Ok(execution) => Ok(Response::new(RunCronJobResponse {
                success: true,
                execution: Some(cron_execution_to_proto(execution)),
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(RunCronJobResponse {
                success: false,
                execution: None,
                error: format!("Failed to run job: {}", e),
            })),
        }
    }

    async fn parse_cron_schedule(
        &self,
        request: Request<ParseCronScheduleRequest>,
    ) -> Result<Response<ParseCronScheduleResponse>, Status> {
        let req = request.into_inner();

        match parse_natural(&req.input) {
            Ok(cron_expr) => {
                let description = CronExpression::parse(&cron_expr)
                    .map(|e| e.describe())
                    .unwrap_or_else(|_| "unknown".to_string());

                Ok(Response::new(ParseCronScheduleResponse {
                    success: true,
                    cron_expression: cron_expr,
                    description,
                    error: String::new(),
                }))
            }
            Err(e) => Ok(Response::new(ParseCronScheduleResponse {
                success: false,
                cron_expression: String::new(),
                description: String::new(),
                error: format!("Failed to parse: {}", e),
            })),
        }
    }

    async fn get_tool_metrics(
        &self,
        request: Request<GetToolMetricsRequest>,
    ) -> Result<Response<GetToolMetricsResponse>, Status> {
        let req = request.into_inner();

        // Get session
        let session = self
            .session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        let session_guard = session.read().await;
        let metrics = session_guard.tool_metrics.read().await;

        // Filter by tool name if specified
        let tool_stats: Vec<proto::ToolStats> = if req.tool_name.is_empty() {
            metrics
                .stats()
                .into_iter()
                .map(tool_stats_to_proto)
                .collect()
        } else {
            metrics
                .stats_for(&req.tool_name)
                .into_iter()
                .map(tool_stats_to_proto)
                .collect()
        };

        Ok(Response::new(GetToolMetricsResponse {
            tools: tool_stats,
            total_calls: metrics.total_calls(),
            total_duration_ms: metrics.total_duration_ms(),
        }))
    }

    async fn get_cost_summary(
        &self,
        request: Request<GetCostSummaryRequest>,
    ) -> Result<Response<GetCostSummaryResponse>, Status> {
        let req = request.into_inner();

        // Collect cost records from session(s)
        let mut all_records: Vec<crate::telemetry::LlmCostRecord> = Vec::new();

        if req.session_id.is_empty() {
            // Aggregate across all sessions
            let session_ids = self.session_manager.list_sessions().await;
            for sid in &session_ids {
                if let Ok(session) = self.session_manager.get_session(sid).await {
                    let session_guard = session.read().await;
                    all_records.extend(session_guard.cost_records.clone());
                }
            }
        } else {
            // Single session
            let session = self
                .session_manager
                .get_session(&req.session_id)
                .await
                .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

            let session_guard = session.read().await;
            all_records.extend(session_guard.cost_records.clone());
        }

        // Apply filters and aggregate
        let model_filter = if req.model.is_empty() {
            None
        } else {
            Some(req.model.as_str())
        };
        let start_date = if req.start_date.is_empty() {
            None
        } else {
            Some(req.start_date.as_str())
        };
        let end_date = if req.end_date.is_empty() {
            None
        } else {
            Some(req.end_date.as_str())
        };

        let summary = crate::telemetry::aggregate_cost_records(
            &all_records,
            model_filter,
            start_date,
            end_date,
        );

        Ok(Response::new(cost_summary_to_proto(&summary)))
    }
}

/// Convert telemetry ToolStats to proto ToolStats
fn tool_stats_to_proto(stats: crate::telemetry::ToolStats) -> proto::ToolStats {
    proto::ToolStats {
        tool_name: stats.tool_name,
        total_calls: stats.total_calls,
        success_count: stats.success_count,
        failure_count: stats.failure_count,
        total_duration_ms: stats.total_duration_ms,
        min_duration_ms: stats.min_duration_ms,
        max_duration_ms: stats.max_duration_ms,
        avg_duration_ms: stats.avg_duration_ms,
        last_called_at: stats
            .last_called_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
    }
}

/// Convert telemetry CostSummary to proto GetCostSummaryResponse
fn cost_summary_to_proto(
    summary: &crate::telemetry::CostSummary,
) -> GetCostSummaryResponse {
    GetCostSummaryResponse {
        total_cost_usd: summary.total_cost_usd,
        total_prompt_tokens: summary.total_prompt_tokens as u64,
        total_completion_tokens: summary.total_completion_tokens as u64,
        total_tokens: summary.total_tokens as u64,
        call_count: summary.call_count as u64,
        by_model: summary
            .by_model
            .iter()
            .map(|b| ModelCostBreakdownProto {
                model: b.model.clone(),
                prompt_tokens: b.prompt_tokens as u64,
                completion_tokens: b.completion_tokens as u64,
                total_tokens: b.total_tokens as u64,
                cost_usd: b.cost_usd,
                call_count: b.call_count as u64,
            })
            .collect(),
        by_day: summary
            .by_day
            .iter()
            .map(|b| DayCostBreakdownProto {
                date: b.date.clone(),
                cost_usd: b.cost_usd,
                call_count: b.call_count as u64,
                total_tokens: b.total_tokens as u64,
            })
            .collect(),
    }
}

// ============================================================================
// Response Transformers
// ============================================================================

/// Remove <think>...</think> blocks from text
fn remove_think_tags(text: &str) -> String {
    let mut content = text.to_string();
    while let Some(start) = content.find("<think>") {
        if let Some(end) = content.find("</think>") {
            content = format!("{}{}", &content[..start], &content[end + 8..]);
        } else {
            break;
        }
    }
    content
}

// ============================================================================
// Cron Helper Functions
// ============================================================================

/// Convert a3s_cron::CronJob to proto CronJob
fn cron_job_to_proto(job: a3s_cron::CronJob) -> CronJob {
    CronJob {
        id: job.id,
        name: job.name,
        schedule: job.schedule,
        command: job.command,
        status: match job.status {
            a3s_cron::JobStatus::Active => CronJobStatus::Active as i32,
            a3s_cron::JobStatus::Paused => CronJobStatus::Paused as i32,
            a3s_cron::JobStatus::Running => CronJobStatus::Running as i32,
        },
        timeout_ms: job.timeout_ms,
        created_at: job.created_at.timestamp_millis(),
        updated_at: job.updated_at.timestamp_millis(),
        last_run: job.last_run.map(|dt| dt.timestamp_millis()),
        next_run: job.next_run.map(|dt| dt.timestamp_millis()),
        run_count: job.run_count,
        fail_count: job.fail_count,
        working_dir: job.working_dir,
    }
}

/// Convert a3s_cron::JobExecution to proto CronExecution
fn cron_execution_to_proto(exec: a3s_cron::JobExecution) -> CronExecution {
    CronExecution {
        id: exec.id,
        job_id: exec.job_id,
        status: match exec.status {
            a3s_cron::ExecutionStatus::Success => CronExecutionStatus::Success as i32,
            a3s_cron::ExecutionStatus::Failed => CronExecutionStatus::Failed as i32,
            a3s_cron::ExecutionStatus::Timeout => CronExecutionStatus::Timeout as i32,
            a3s_cron::ExecutionStatus::Cancelled => CronExecutionStatus::Cancelled as i32,
        },
        started_at: exec.started_at.timestamp_millis(),
        ended_at: exec.ended_at.map(|dt| dt.timestamp_millis()),
        duration_ms: exec.duration_ms,
        exit_code: exec.exit_code,
        stdout: exec.stdout,
        stderr: exec.stderr,
        error: exec.error,
    }
}

// ============================================================================
// LSP Helper Functions
// ============================================================================

/// Format hover contents to string
fn format_hover_contents(contents: &crate::lsp::protocol::HoverContents) -> String {
    use crate::lsp::protocol::HoverContents;

    match contents {
        HoverContents::Scalar(marked) => format_marked_string(marked),
        HoverContents::Array(items) => items
            .iter()
            .map(format_marked_string)
            .collect::<Vec<_>>()
            .join("\n\n"),
        HoverContents::Markup(markup) => markup.value.clone(),
    }
}

/// Format marked string to string
fn format_marked_string(marked: &crate::lsp::protocol::MarkedString) -> String {
    use crate::lsp::protocol::MarkedString;

    match marked {
        MarkedString::String(s) => s.clone(),
        MarkedString::LanguageString { language, value } => {
            format!("```{}\n{}\n```", language, value)
        }
    }
}

/// Convert definition response to proto locations
fn convert_definition_response(
    response: &crate::lsp::protocol::GotoDefinitionResponse,
) -> Vec<LspLocation> {
    use crate::lsp::protocol::GotoDefinitionResponse;

    match response {
        GotoDefinitionResponse::Scalar(loc) => vec![LspLocation {
            uri: loc.uri.clone(),
            range: Some(LspRange {
                start: Some(LspPosition {
                    line: loc.range.start.line,
                    character: loc.range.start.character,
                }),
                end: Some(LspPosition {
                    line: loc.range.end.line,
                    character: loc.range.end.character,
                }),
            }),
        }],
        GotoDefinitionResponse::Array(locs) => locs
            .iter()
            .map(|loc| LspLocation {
                uri: loc.uri.clone(),
                range: Some(LspRange {
                    start: Some(LspPosition {
                        line: loc.range.start.line,
                        character: loc.range.start.character,
                    }),
                    end: Some(LspPosition {
                        line: loc.range.end.line,
                        character: loc.range.end.character,
                    }),
                }),
            })
            .collect(),
        GotoDefinitionResponse::Link(links) => links
            .iter()
            .map(|link| LspLocation {
                uri: link.target_uri.clone(),
                range: Some(LspRange {
                    start: Some(LspPosition {
                        line: link.target_selection_range.start.line,
                        character: link.target_selection_range.start.character,
                    }),
                    end: Some(LspPosition {
                        line: link.target_selection_range.end.line,
                        character: link.target_selection_range.end.character,
                    }),
                }),
            })
            .collect(),
    }
}

/// Extract JSON from markdown code blocks or raw text
fn extract_json(text: &str) -> String {
    let content = text.trim();

    // Try to extract JSON from markdown code blocks
    if let Some(start) = content.find("```json") {
        if let Some(end) = content[start + 7..].find("```") {
            return content[start + 7..start + 7 + end].trim().to_string();
        }
    }

    // Try generic code block
    if let Some(start) = content.find("```") {
        let after_start = &content[start + 3..];
        let json_start = after_start.find('\n').map(|i| i + 1).unwrap_or(0);
        if let Some(end) = after_start[json_start..].find("```") {
            return after_start[json_start..json_start + end].trim().to_string();
        }
    }

    // Try to find raw JSON object
    if let Some(start) = content.find('{') {
        if let Some(end) = content.rfind('}') {
            if end > start {
                return content[start..=end].to_string();
            }
        }
    }

    // Try to find raw JSON array
    if let Some(start) = content.find('[') {
        if let Some(end) = content.rfind(']') {
            if end > start {
                return content[start..=end].to_string();
            }
        }
    }

    content.to_string()
}

/// Transform LLM response for structured output
fn transform_for_structured_output(text: &str) -> String {
    let without_think = remove_think_tags(text);
    extract_json(&without_think)
}

/// Convert agent events to gRPC GenerateChunk stream
fn convert_events_to_generate_chunks(
    mut rx: mpsc::Receiver<AgentEvent>,
    session_id: String,
) -> impl Stream<Item = Result<GenerateChunk, Status>> {
    async_stream::stream! {
        while let Some(event) = rx.recv().await {
            if let Some(chunk) = convert::internal_event_to_generate_chunk(event, &session_id) {
                yield Ok(chunk);
            }
        }
    }
}

/// Start gRPC server
/// Start the gRPC server with the given configuration
pub async fn start_server_with_config(
    config: CodeConfig,
    workspace: &str,
    listen_addr: &str,
    config_path: Option<&std::path::Path>,
) -> Result<()> {
    tracing::info!("Workspace: {}", workspace);

    // Create default LLM client from config if available
    let default_llm = config.default_llm_config().map(|llm_config| {
        tracing::info!(
            "Creating default LLM client: {}/{}",
            llm_config.provider,
            llm_config.model
        );
        llm::create_client_with_config(llm_config)
    });

    if default_llm.is_none() {
        tracing::info!("LLM configuration: Clients must provide via ConfigureSession RPC");
    }

    // Create session manager based on storage backend
    let tool_executor = Arc::new(ToolExecutor::new(workspace.to_string()));

    let session_manager = match config.storage_backend {
        crate::config::StorageBackend::Memory => {
            tracing::info!("Using in-memory session storage (no persistence)");
            Arc::new(SessionManager::new(default_llm, tool_executor))
        }
        crate::config::StorageBackend::File => {
            // Determine sessions directory
            let sessions_dir = config
                .sessions_dir
                .clone()
                .unwrap_or_else(|| std::path::Path::new(workspace).join("sessions"));

            tracing::info!(
                "Using file-based session storage: {}",
                sessions_dir.display()
            );

            Arc::new(
                SessionManager::with_persistence(default_llm, tool_executor, &sessions_dir)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create session manager: {}", e))?,
            )
        }
        crate::config::StorageBackend::Custom => {
            return Err(anyhow::anyhow!(
                "Custom storage backend not yet implemented"
            ));
        }
    };

    let service = CodeAgentServiceImpl::with_config(
        session_manager,
        config,
        config_path.map(|p| p.to_path_buf()),
    );

    // Parse the base address to extract host and port
    let (host, base_port) = parse_listen_addr(listen_addr)?;

    // Try default port first, fallback to OS-assigned port if busy
    let (listener, actual_port) = {
        let addr = format!("{}:{}", host, base_port);
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => {
                tracing::info!("Starting gRPC server on {}:{}", host, base_port);
                (listener, base_port)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                let fallback_addr = format!("{}:0", host);
                let listener = tokio::net::TcpListener::bind(&fallback_addr)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to bind to {}: {}", fallback_addr, e))?;
                let actual_port = listener.local_addr()?.port();
                tracing::warn!(
                    "Port {} was in use, using port {} instead",
                    base_port,
                    actual_port
                );
                tracing::info!("Starting gRPC server on {}:{}", host, actual_port);
                (listener, actual_port)
            }
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to bind to {}: {}", addr, e));
            }
        }
    };
    let _ = actual_port;

    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    tonic::transport::Server::builder()
        .add_service(CodeAgentServiceServer::new(service))
        .serve_with_incoming(incoming)
        .await?;

    Ok(())
}

/// Parse listen address into host and port
fn parse_listen_addr(addr: &str) -> Result<(String, u16)> {
    let parts: Vec<&str> = addr.rsplitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(anyhow::anyhow!(
            "Invalid listen address '{}': expected format 'host:port'",
            addr
        ));
    }
    let port: u16 = parts[0]
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid port '{}': {}", parts[0], e))?;
    let host = parts[1].to_string();
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelConfig, ModelCost, ModelLimit, ModelModalities, ProviderConfig};
    use crate::session::SessionManager;
    use crate::store::MemorySessionStore;
    use crate::tools::ToolExecutor;

    fn create_test_service() -> CodeAgentServiceImpl {
        let store = Arc::new(MemorySessionStore::new());
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let session_manager = Arc::new(SessionManager::with_store(None, tool_executor, store));
        CodeAgentServiceImpl::new(session_manager)
    }

    fn create_test_provider(name: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            api_key: Some(format!("{}-api-key", name)),
            base_url: Some(format!("https://api.{}.com", name)),
            models: vec![
                ModelConfig {
                    id: format!("{}-model-1", name),
                    name: format!("{} Model 1", name),
                    family: name.to_string(),
                    api_key: None,
                    base_url: None,
                    attachment: false,
                    reasoning: false,
                    tool_call: true,
                    temperature: true,
                    release_date: None,
                    modalities: ModelModalities::default(),
                    cost: ModelCost::default(),
                    limit: ModelLimit::default(),
                },
                ModelConfig {
                    id: format!("{}-model-2", name),
                    name: format!("{} Model 2", name),
                    family: name.to_string(),
                    api_key: None,
                    base_url: None,
                    attachment: true,
                    reasoning: true,
                    tool_call: true,
                    temperature: true,
                    release_date: Some("2025-01-01".to_string()),
                    modalities: ModelModalities {
                        input: vec!["text".to_string(), "image".to_string()],
                        output: vec!["text".to_string()],
                    },
                    cost: ModelCost {
                        input: 3.0,
                        output: 15.0,
                        cache_read: 0.3,
                        cache_write: 3.75,
                    },
                    limit: ModelLimit {
                        context: 200000,
                        output: 64000,
                    },
                },
            ],
        }
    }

    #[tokio::test]
    async fn test_list_providers_empty() {
        let service = create_test_service();

        let response = service
            .list_providers(Request::new(ListProvidersRequest {}))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(resp.providers.is_empty());
        assert!(resp.default_provider.is_none());
        assert!(resp.default_model.is_none());
    }

    #[tokio::test]
    async fn test_add_provider() {
        let service = create_test_service();

        // Add a provider
        let provider = create_test_provider("anthropic");
        let proto_provider = convert::internal_provider_config_to_proto(&provider);

        let response = service
            .add_provider(Request::new(AddProviderRequest {
                provider: Some(proto_provider),
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(resp.success);
        assert!(resp.error.is_empty());
        assert!(resp.provider.is_some());

        let added = resp.provider.unwrap();
        assert_eq!(added.name, "anthropic");
        assert_eq!(added.models.len(), 2);

        // Verify it's in the list
        let list_response = service
            .list_providers(Request::new(ListProvidersRequest {}))
            .await
            .unwrap();

        assert_eq!(list_response.into_inner().providers.len(), 1);
    }

    #[tokio::test]
    async fn test_add_duplicate_provider() {
        let service = create_test_service();

        let provider = create_test_provider("anthropic");
        let proto_provider = convert::internal_provider_config_to_proto(&provider);

        // Add first time - should succeed
        let response1 = service
            .add_provider(Request::new(AddProviderRequest {
                provider: Some(proto_provider.clone()),
            }))
            .await
            .unwrap();
        assert!(response1.into_inner().success);

        // Add second time - should fail
        let response2 = service
            .add_provider(Request::new(AddProviderRequest {
                provider: Some(proto_provider),
            }))
            .await
            .unwrap();

        let resp = response2.into_inner();
        assert!(!resp.success);
        assert!(resp.error.contains("already exists"));
    }

    #[tokio::test]
    async fn test_get_provider() {
        let service = create_test_service();

        // Add a provider
        let provider = create_test_provider("openai");
        let proto_provider = convert::internal_provider_config_to_proto(&provider);

        service
            .add_provider(Request::new(AddProviderRequest {
                provider: Some(proto_provider),
            }))
            .await
            .unwrap();

        // Get the provider
        let response = service
            .get_provider(Request::new(GetProviderRequest {
                name: "openai".to_string(),
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(resp.provider.is_some());
        let p = resp.provider.unwrap();
        assert_eq!(p.name, "openai");
        assert_eq!(p.models.len(), 2);
    }

    #[tokio::test]
    async fn test_get_provider_not_found() {
        let service = create_test_service();

        let result = service
            .get_provider(Request::new(GetProviderRequest {
                name: "nonexistent".to_string(),
            }))
            .await;

        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn test_update_provider() {
        let service = create_test_service();

        // Add a provider
        let provider = create_test_provider("anthropic");
        let proto_provider = convert::internal_provider_config_to_proto(&provider);

        service
            .add_provider(Request::new(AddProviderRequest {
                provider: Some(proto_provider),
            }))
            .await
            .unwrap();

        // Update the provider with new API key
        let mut updated_provider = provider.clone();
        updated_provider.api_key = Some("new-api-key".to_string());
        let updated_proto = convert::internal_provider_config_to_proto(&updated_provider);

        let response = service
            .update_provider(Request::new(UpdateProviderRequest {
                provider: Some(updated_proto),
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(resp.success);

        // Verify the update
        let get_response = service
            .get_provider(Request::new(GetProviderRequest {
                name: "anthropic".to_string(),
            }))
            .await
            .unwrap();

        let p = get_response.into_inner().provider.unwrap();
        assert_eq!(p.api_key, Some("new-api-key".to_string()));
    }

    #[tokio::test]
    async fn test_update_nonexistent_provider() {
        let service = create_test_service();

        let provider = create_test_provider("nonexistent");
        let proto_provider = convert::internal_provider_config_to_proto(&provider);

        let response = service
            .update_provider(Request::new(UpdateProviderRequest {
                provider: Some(proto_provider),
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(!resp.success);
        assert!(resp.error.contains("not found"));
    }

    #[tokio::test]
    async fn test_remove_provider() {
        let service = create_test_service();

        // Add a provider
        let provider = create_test_provider("anthropic");
        let proto_provider = convert::internal_provider_config_to_proto(&provider);

        service
            .add_provider(Request::new(AddProviderRequest {
                provider: Some(proto_provider),
            }))
            .await
            .unwrap();

        // Remove the provider
        let response = service
            .remove_provider(Request::new(RemoveProviderRequest {
                name: "anthropic".to_string(),
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(resp.success);

        // Verify it's removed
        let list_response = service
            .list_providers(Request::new(ListProvidersRequest {}))
            .await
            .unwrap();

        assert!(list_response.into_inner().providers.is_empty());
    }

    #[tokio::test]
    async fn test_remove_nonexistent_provider() {
        let service = create_test_service();

        let response = service
            .remove_provider(Request::new(RemoveProviderRequest {
                name: "nonexistent".to_string(),
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(!resp.success);
        assert!(resp.error.contains("not found"));
    }

    #[tokio::test]
    async fn test_set_default_model() {
        let service = create_test_service();

        // Add a provider
        let provider = create_test_provider("anthropic");
        let proto_provider = convert::internal_provider_config_to_proto(&provider);

        service
            .add_provider(Request::new(AddProviderRequest {
                provider: Some(proto_provider),
            }))
            .await
            .unwrap();

        // Set default model
        let response = service
            .set_default_model(Request::new(SetDefaultModelRequest {
                provider: "anthropic".to_string(),
                model: "anthropic-model-1".to_string(),
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(resp.success);
        assert_eq!(resp.provider, "anthropic");
        assert_eq!(resp.model, "anthropic-model-1");

        // Verify via get_default_model
        let get_response = service
            .get_default_model(Request::new(GetDefaultModelRequest {}))
            .await
            .unwrap();

        let get_resp = get_response.into_inner();
        assert_eq!(get_resp.provider, Some("anthropic".to_string()));
        assert_eq!(get_resp.model, Some("anthropic-model-1".to_string()));
    }

    #[tokio::test]
    async fn test_set_default_model_invalid_provider() {
        let service = create_test_service();

        let response = service
            .set_default_model(Request::new(SetDefaultModelRequest {
                provider: "nonexistent".to_string(),
                model: "some-model".to_string(),
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(!resp.success);
        assert!(resp.error.contains("Provider"));
        assert!(resp.error.contains("not found"));
    }

    #[tokio::test]
    async fn test_set_default_model_invalid_model() {
        let service = create_test_service();

        // Add a provider
        let provider = create_test_provider("anthropic");
        let proto_provider = convert::internal_provider_config_to_proto(&provider);

        service
            .add_provider(Request::new(AddProviderRequest {
                provider: Some(proto_provider),
            }))
            .await
            .unwrap();

        // Try to set invalid model
        let response = service
            .set_default_model(Request::new(SetDefaultModelRequest {
                provider: "anthropic".to_string(),
                model: "nonexistent-model".to_string(),
            }))
            .await
            .unwrap();

        let resp = response.into_inner();
        assert!(!resp.success);
        assert!(resp.error.contains("Model"));
        assert!(resp.error.contains("not found"));
    }

    #[tokio::test]
    async fn test_remove_default_provider_clears_default() {
        let service = create_test_service();

        // Add a provider and set as default
        let provider = create_test_provider("anthropic");
        let proto_provider = convert::internal_provider_config_to_proto(&provider);

        service
            .add_provider(Request::new(AddProviderRequest {
                provider: Some(proto_provider),
            }))
            .await
            .unwrap();

        service
            .set_default_model(Request::new(SetDefaultModelRequest {
                provider: "anthropic".to_string(),
                model: "anthropic-model-1".to_string(),
            }))
            .await
            .unwrap();

        // Remove the provider
        service
            .remove_provider(Request::new(RemoveProviderRequest {
                name: "anthropic".to_string(),
            }))
            .await
            .unwrap();

        // Verify default is cleared
        let get_response = service
            .get_default_model(Request::new(GetDefaultModelRequest {}))
            .await
            .unwrap();

        let resp = get_response.into_inner();
        assert!(resp.provider.is_none());
        assert!(resp.model.is_none());
    }

    #[tokio::test]
    async fn test_service_with_initial_config() {
        let store = Arc::new(MemorySessionStore::new());
        let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let session_manager = Arc::new(SessionManager::with_store(None, tool_executor, store));

        let config = CodeConfig {
            default_provider: Some("anthropic".to_string()),
            default_model: Some("claude-sonnet-4".to_string()),
            providers: vec![ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("test-key".to_string()),
                base_url: Some("https://api.anthropic.com".to_string()),
                models: vec![ModelConfig {
                    id: "claude-sonnet-4".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                    family: "claude".to_string(),
                    api_key: None,
                    base_url: None,
                    attachment: true,
                    reasoning: false,
                    tool_call: true,
                    temperature: true,
                    release_date: None,
                    modalities: ModelModalities::default(),
                    cost: ModelCost::default(),
                    limit: ModelLimit::default(),
                }],
            }],
            ..Default::default()
        };

        let service = CodeAgentServiceImpl::with_config(session_manager, config, None);

        // Verify initial config is loaded
        let list_response = service
            .list_providers(Request::new(ListProvidersRequest {}))
            .await
            .unwrap();

        let resp = list_response.into_inner();
        assert_eq!(resp.providers.len(), 1);
        assert_eq!(resp.default_provider, Some("anthropic".to_string()));
        assert_eq!(resp.default_model, Some("claude-sonnet-4".to_string()));
    }

    // ========================================================================
    // Helper Function Tests
    // ========================================================================

    #[test]
    fn test_storage_backend_to_proto_memory() {
        use crate::config::StorageBackend;
        assert_eq!(storage_backend_to_proto(&StorageBackend::Memory), 1);
    }

    #[test]
    fn test_storage_backend_to_proto_file() {
        use crate::config::StorageBackend;
        assert_eq!(storage_backend_to_proto(&StorageBackend::File), 2);
    }

    #[test]
    fn test_storage_backend_to_proto_custom() {
        use crate::config::StorageBackend;
        assert_eq!(storage_backend_to_proto(&StorageBackend::Custom), 0);
    }

    #[test]
    fn test_remove_think_tags_single_block() {
        assert_eq!(
            remove_think_tags("Hello <think>thought</think> world"),
            "Hello  world"
        );
    }

    #[test]
    fn test_remove_think_tags_multiple_blocks() {
        assert_eq!(
            remove_think_tags("<think>a</think>X<think>b</think>Y"),
            "XY"
        );
    }

    #[test]
    fn test_remove_think_tags_no_tags() {
        assert_eq!(remove_think_tags("Hello world"), "Hello world");
    }

    #[test]
    fn test_remove_think_tags_empty_string() {
        assert_eq!(remove_think_tags(""), "");
    }

    #[test]
    fn test_remove_think_tags_unclosed_tag() {
        assert_eq!(
            remove_think_tags("Hello <think>unclosed"),
            "Hello <think>unclosed"
        );
    }

    #[test]
    fn test_remove_think_tags_empty_think_block() {
        assert_eq!(
            remove_think_tags("Before<think></think>After"),
            "BeforeAfter"
        );
    }

    #[test]
    fn test_extract_json_code_block() {
        assert_eq!(
            extract_json("```json\n{\"key\":\"value\"}\n```"),
            "{\"key\":\"value\"}"
        );
    }

    #[test]
    fn test_extract_json_generic_code_block() {
        assert_eq!(
            extract_json("```\n{\"key\":\"value\"}\n```"),
            "{\"key\":\"value\"}"
        );
    }

    #[test]
    fn test_extract_json_raw_object() {
        assert_eq!(
            extract_json("text {\"key\":\"value\"} more"),
            "{\"key\":\"value\"}"
        );
    }

    #[test]
    fn test_extract_json_raw_array() {
        assert_eq!(extract_json("text [1,2,3] more"), "[1,2,3]");
    }

    #[test]
    fn test_extract_json_no_json() {
        assert_eq!(extract_json("plain text"), "plain text");
    }

    #[test]
    fn test_extract_json_empty_string() {
        assert_eq!(extract_json(""), "");
    }

    #[test]
    fn test_extract_json_whitespace_trimming() {
        assert_eq!(
            extract_json("  \n  {\"key\":\"value\"}  \n  "),
            "{\"key\":\"value\"}"
        );
    }

    #[test]
    fn test_transform_for_structured_output_think_and_json() {
        assert_eq!(
            transform_for_structured_output("<think>hmm</think>```json\n{\"x\":1}\n```"),
            "{\"x\":1}"
        );
    }

    #[test]
    fn test_transform_for_structured_output_just_json() {
        assert_eq!(transform_for_structured_output("{\"x\":1}"), "{\"x\":1}");
    }

    #[test]
    fn test_transform_for_structured_output_plain_text() {
        assert_eq!(transform_for_structured_output("plain text"), "plain text");
    }

    #[test]
    fn test_parse_listen_addr_valid() {
        let (host, port) = parse_listen_addr("0.0.0.0:4088").unwrap();
        assert_eq!(host, "0.0.0.0");
        assert_eq!(port, 4088);
    }

    #[test]
    fn test_parse_listen_addr_localhost() {
        let (host, port) = parse_listen_addr("localhost:8080").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
    }

    #[test]
    fn test_parse_listen_addr_ipv6() {
        let (host, port) = parse_listen_addr("[::1]:9000").unwrap();
        assert_eq!(host, "[::1]");
        assert_eq!(port, 9000);
    }

    #[test]
    fn test_parse_listen_addr_missing_port() {
        let result = parse_listen_addr("localhost");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expected format"));
    }

    #[test]
    fn test_parse_listen_addr_invalid_port() {
        let result = parse_listen_addr("localhost:abc");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid port"));
    }

    #[test]
    fn test_parse_listen_addr_port_overflow() {
        let result = parse_listen_addr("localhost:99999");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid port"));
    }

    #[test]
    fn test_format_marked_string_plain() {
        use crate::lsp::protocol::MarkedString;
        let marked = MarkedString::String("plain text".to_string());
        assert_eq!(format_marked_string(&marked), "plain text");
    }

    #[test]
    fn test_format_marked_string_language() {
        use crate::lsp::protocol::MarkedString;
        let marked = MarkedString::LanguageString {
            language: "rust".to_string(),
            value: "fn main() {}".to_string(),
        };
        assert_eq!(format_marked_string(&marked), "```rust\nfn main() {}\n```");
    }

    #[test]
    fn test_format_marked_string_empty() {
        use crate::lsp::protocol::MarkedString;
        let marked = MarkedString::String("".to_string());
        assert_eq!(format_marked_string(&marked), "");
    }

    #[test]
    fn test_format_hover_contents_scalar() {
        use crate::lsp::protocol::{HoverContents, MarkedString};
        let contents = HoverContents::Scalar(MarkedString::String("hover text".to_string()));
        assert_eq!(format_hover_contents(&contents), "hover text");
    }

    #[test]
    fn test_format_hover_contents_array() {
        use crate::lsp::protocol::{HoverContents, MarkedString};
        let contents = HoverContents::Array(vec![
            MarkedString::String("first".to_string()),
            MarkedString::String("second".to_string()),
        ]);
        assert_eq!(format_hover_contents(&contents), "first\n\nsecond");
    }

    #[test]
    fn test_format_hover_contents_markup() {
        use crate::lsp::protocol::{HoverContents, MarkupContent};
        let contents = HoverContents::Markup(MarkupContent {
            kind: crate::lsp::protocol::MarkupKind::Markdown,
            value: "# Title".to_string(),
        });
        assert_eq!(format_hover_contents(&contents), "# Title");
    }

    #[test]
    fn test_format_hover_contents_empty_array() {
        use crate::lsp::protocol::HoverContents;
        let contents = HoverContents::Array(vec![]);
        assert_eq!(format_hover_contents(&contents), "");
    }

    #[test]
    fn test_format_hover_contents_markup_plaintext() {
        use crate::lsp::protocol::{HoverContents, MarkupContent};
        let contents = HoverContents::Markup(MarkupContent {
            kind: crate::lsp::protocol::MarkupKind::PlainText,
            value: "plain".to_string(),
        });
        assert_eq!(format_hover_contents(&contents), "plain");
    }

    #[test]
    fn test_convert_definition_response_scalar() {
        use crate::lsp::protocol::{GotoDefinitionResponse, Location, Position, Range};
        let response = GotoDefinitionResponse::Scalar(Location {
            uri: "file:///test.rs".to_string(),
            range: Range {
                start: Position {
                    line: 10,
                    character: 5,
                },
                end: Position {
                    line: 10,
                    character: 15,
                },
            },
        });
        let result = convert_definition_response(&response);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].uri, "file:///test.rs");
        assert_eq!(
            result[0]
                .range
                .as_ref()
                .unwrap()
                .start
                .as_ref()
                .unwrap()
                .line,
            10
        );
    }

    #[test]
    fn test_convert_definition_response_array() {
        use crate::lsp::protocol::{GotoDefinitionResponse, Location, Position, Range};
        let response = GotoDefinitionResponse::Array(vec![
            Location {
                uri: "file:///test1.rs".to_string(),
                range: Range {
                    start: Position {
                        line: 1,
                        character: 0,
                    },
                    end: Position {
                        line: 1,
                        character: 10,
                    },
                },
            },
            Location {
                uri: "file:///test2.rs".to_string(),
                range: Range {
                    start: Position {
                        line: 2,
                        character: 0,
                    },
                    end: Position {
                        line: 2,
                        character: 10,
                    },
                },
            },
        ]);
        let result = convert_definition_response(&response);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].uri, "file:///test1.rs");
        assert_eq!(result[1].uri, "file:///test2.rs");
    }

    #[test]
    fn test_convert_definition_response_link() {
        use crate::lsp::protocol::{GotoDefinitionResponse, LocationLink, Position, Range};
        let response = GotoDefinitionResponse::Link(vec![LocationLink {
            origin_selection_range: None,
            target_uri: "file:///target.rs".to_string(),
            target_range: Range {
                start: Position {
                    line: 5,
                    character: 0,
                },
                end: Position {
                    line: 10,
                    character: 0,
                },
            },
            target_selection_range: Range {
                start: Position {
                    line: 7,
                    character: 4,
                },
                end: Position {
                    line: 7,
                    character: 14,
                },
            },
        }]);
        let result = convert_definition_response(&response);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].uri, "file:///target.rs");
        assert_eq!(
            result[0]
                .range
                .as_ref()
                .unwrap()
                .start
                .as_ref()
                .unwrap()
                .line,
            7
        );
    }

    #[test]
    fn test_convert_definition_response_empty_array() {
        use crate::lsp::protocol::GotoDefinitionResponse;
        let response = GotoDefinitionResponse::Array(vec![]);
        let result = convert_definition_response(&response);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_cron_job_to_proto_all_fields() {
        use a3s_cron::{CronJob, JobStatus};
        use chrono::Utc;

        let now = Utc::now();
        let job = CronJob {
            id: "job1".to_string(),
            name: "Test Job".to_string(),
            schedule: "0 * * * *".to_string(),
            command: "echo test".to_string(),
            status: JobStatus::Active,
            timeout_ms: 5000,
            created_at: now,
            updated_at: now,
            last_run: Some(now),
            next_run: Some(now),
            run_count: 10,
            fail_count: 2,
            working_dir: Some("/tmp".to_string()),
            env: vec![],
        };

        let proto = cron_job_to_proto(job);
        assert_eq!(proto.id, "job1");
        assert_eq!(proto.name, "Test Job");
        assert_eq!(proto.schedule, "0 * * * *");
        assert_eq!(proto.command, "echo test");
        assert_eq!(proto.status, CronJobStatus::Active as i32);
        assert_eq!(proto.timeout_ms, 5000);
        assert_eq!(proto.run_count, 10);
        assert_eq!(proto.fail_count, 2);
        assert_eq!(proto.working_dir, Some("/tmp".to_string()));
    }

    #[test]
    fn test_cron_job_to_proto_minimal_fields() {
        use a3s_cron::{CronJob, JobStatus};
        use chrono::Utc;

        let now = Utc::now();
        let job = CronJob {
            id: "job2".to_string(),
            name: "Minimal".to_string(),
            schedule: "* * * * *".to_string(),
            command: "ls".to_string(),
            status: JobStatus::Paused,
            timeout_ms: 0,
            created_at: now,
            updated_at: now,
            last_run: None,
            next_run: None,
            run_count: 0,
            fail_count: 0,
            working_dir: None,
            env: vec![],
        };

        let proto = cron_job_to_proto(job);
        assert_eq!(proto.id, "job2");
        assert_eq!(proto.status, CronJobStatus::Paused as i32);
        assert!(proto.last_run.is_none());
        assert!(proto.next_run.is_none());
        assert!(proto.working_dir.is_none());
    }

    #[test]
    fn test_cron_job_to_proto_running_status() {
        use a3s_cron::{CronJob, JobStatus};
        use chrono::Utc;

        let now = Utc::now();
        let job = CronJob {
            id: "job3".to_string(),
            name: "Running".to_string(),
            schedule: "0 0 * * *".to_string(),
            command: "backup".to_string(),
            status: JobStatus::Running,
            timeout_ms: 60000,
            created_at: now,
            updated_at: now,
            last_run: None,
            next_run: None,
            run_count: 0,
            fail_count: 0,
            working_dir: None,
            env: vec![],
        };

        let proto = cron_job_to_proto(job);
        assert_eq!(proto.status, CronJobStatus::Running as i32);
    }

    #[test]
    fn test_cron_execution_to_proto_success() {
        use a3s_cron::{ExecutionStatus, JobExecution};
        use chrono::Utc;

        let now = Utc::now();
        let exec = JobExecution {
            id: "exec1".to_string(),
            job_id: "job1".to_string(),
            status: ExecutionStatus::Success,
            started_at: now,
            ended_at: Some(now),
            duration_ms: Some(1000),
            exit_code: Some(0),
            stdout: "output".to_string(),
            stderr: "".to_string(),
            error: None,
        };

        let proto = cron_execution_to_proto(exec);
        assert_eq!(proto.id, "exec1");
        assert_eq!(proto.job_id, "job1");
        assert_eq!(proto.status, CronExecutionStatus::Success as i32);
        assert_eq!(proto.duration_ms, Some(1000));
        assert_eq!(proto.exit_code, Some(0));
        assert_eq!(proto.stdout, "output");
    }

    #[test]
    fn test_cron_execution_to_proto_failed() {
        use a3s_cron::{ExecutionStatus, JobExecution};
        use chrono::Utc;

        let now = Utc::now();
        let exec = JobExecution {
            id: "exec2".to_string(),
            job_id: "job2".to_string(),
            status: ExecutionStatus::Failed,
            started_at: now,
            ended_at: Some(now),
            duration_ms: Some(500),
            exit_code: Some(1),
            stdout: "".to_string(),
            stderr: "error message".to_string(),
            error: Some("Command failed".to_string()),
        };

        let proto = cron_execution_to_proto(exec);
        assert_eq!(proto.status, CronExecutionStatus::Failed as i32);
        assert_eq!(proto.exit_code, Some(1));
        assert_eq!(proto.error, Some("Command failed".to_string()));
    }

    #[test]
    fn test_cron_execution_to_proto_timeout() {
        use a3s_cron::{ExecutionStatus, JobExecution};
        use chrono::Utc;

        let now = Utc::now();
        let exec = JobExecution {
            id: "exec3".to_string(),
            job_id: "job3".to_string(),
            status: ExecutionStatus::Timeout,
            started_at: now,
            ended_at: Some(now),
            duration_ms: Some(5000),
            exit_code: None,
            stdout: "".to_string(),
            stderr: "".to_string(),
            error: Some("Timeout".to_string()),
        };

        let proto = cron_execution_to_proto(exec);
        assert_eq!(proto.status, CronExecutionStatus::Timeout as i32);
        assert_eq!(proto.error, Some("Timeout".to_string()));
    }

    #[test]
    fn test_cron_execution_to_proto_cancelled() {
        use a3s_cron::{ExecutionStatus, JobExecution};
        use chrono::Utc;

        let now = Utc::now();
        let exec = JobExecution {
            id: "exec4".to_string(),
            job_id: "job4".to_string(),
            status: ExecutionStatus::Cancelled,
            started_at: now,
            ended_at: Some(now),
            duration_ms: Some(100),
            exit_code: None,
            stdout: "".to_string(),
            stderr: "".to_string(),
            error: Some("Cancelled by user".to_string()),
        };

        let proto = cron_execution_to_proto(exec);
        assert_eq!(proto.status, CronExecutionStatus::Cancelled as i32);
    }

    #[test]
    fn test_cron_execution_to_proto_no_output() {
        use a3s_cron::{ExecutionStatus, JobExecution};
        use chrono::Utc;

        let now = Utc::now();
        let exec = JobExecution {
            id: "exec5".to_string(),
            job_id: "job5".to_string(),
            status: ExecutionStatus::Success,
            started_at: now,
            ended_at: None,
            duration_ms: None,
            exit_code: None,
            stdout: "".to_string(),
            stderr: "".to_string(),
            error: None,
        };

        let proto = cron_execution_to_proto(exec);
        assert!(proto.ended_at.is_none());
        assert!(proto.exit_code.is_none());
        assert!(proto.stdout.is_empty());
        assert!(proto.stderr.is_empty());
        assert!(proto.error.is_none());
    }

    #[test]
    fn test_parse_skill_metadata_with_frontmatter() {
        let content = "---\nversion: 1.0.0\ndescription: Test skill\n---\ncode here";
        let (version, description) = CodeAgentServiceImpl::parse_skill_metadata(content);
        assert_eq!(version, Some("1.0.0".to_string()));
        assert_eq!(description, Some("Test skill".to_string()));
    }

    #[test]
    fn test_parse_skill_metadata_no_frontmatter() {
        let content = "just code without frontmatter";
        let (version, description) = CodeAgentServiceImpl::parse_skill_metadata(content);
        assert_eq!(version, None);
        assert_eq!(description, None);
    }

    #[test]
    fn test_parse_skill_metadata_empty() {
        let content = "";
        let (version, description) = CodeAgentServiceImpl::parse_skill_metadata(content);
        assert_eq!(version, None);
        assert_eq!(description, None);
    }

    #[test]
    fn test_parse_skill_metadata_quoted_values() {
        let content = "---\nversion: \"2.0.0\"\ndescription: 'Quoted description'\n---\n";
        let (version, description) = CodeAgentServiceImpl::parse_skill_metadata(content);
        assert_eq!(version, Some("2.0.0".to_string()));
        assert_eq!(description, Some("Quoted description".to_string()));
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn test_remove_think_tags_basic() {
        assert_eq!(
            remove_think_tags("Hello <think>thought</think> world"),
            "Hello  world"
        );
    }

    #[test]
    fn test_remove_think_tags_multiple() {
        assert_eq!(
            remove_think_tags("<think>a</think>X<think>b</think>Y"),
            "XY"
        );
    }

    #[test]
    fn test_remove_think_tags_none() {
        assert_eq!(remove_think_tags("Hello world"), "Hello world");
    }

    #[test]
    fn test_remove_think_tags_empty() {
        assert_eq!(remove_think_tags(""), "");
    }

    #[test]
    fn test_remove_think_tags_unclosed() {
        assert_eq!(
            remove_think_tags("Hello <think>unclosed"),
            "Hello <think>unclosed"
        );
    }

    #[test]
    fn test_remove_think_tags_at_start() {
        assert_eq!(
            remove_think_tags("<think>thinking</think>Answer is 42."),
            "Answer is 42."
        );
    }

    #[test]
    fn test_extract_json_code_block() {
        assert_eq!(extract_json("```json\n{\"k\":\"v\"}\n```"), "{\"k\":\"v\"}");
    }

    #[test]
    fn test_extract_json_generic_block() {
        assert_eq!(extract_json("```\n{\"k\":\"v\"}\n```"), "{\"k\":\"v\"}");
    }

    #[test]
    fn test_extract_json_raw_object() {
        assert_eq!(extract_json("text {\"k\":\"v\"} more"), "{\"k\":\"v\"}");
    }

    #[test]
    fn test_extract_json_raw_array() {
        assert_eq!(extract_json("text [1,2,3] more"), "[1,2,3]");
    }

    #[test]
    fn test_extract_json_plain() {
        assert_eq!(extract_json("plain text"), "plain text");
    }

    #[test]
    fn test_extract_json_empty() {
        assert_eq!(extract_json(""), "");
    }

    #[test]
    fn test_extract_json_nested() {
        assert_eq!(extract_json("{\"a\":{\"b\":1}}"), "{\"a\":{\"b\":1}}");
    }

    #[test]
    fn test_transform_structured_with_think() {
        assert_eq!(
            transform_for_structured_output("<think>hmm</think>```json\n{\"x\":1}\n```"),
            "{\"x\":1}"
        );
    }

    #[test]
    fn test_transform_structured_plain() {
        assert_eq!(transform_for_structured_output("{\"x\":1}"), "{\"x\":1}");
    }

    #[test]
    fn test_parse_listen_addr_valid() {
        let (h, p) = parse_listen_addr("0.0.0.0:4088").unwrap();
        assert_eq!(h, "0.0.0.0");
        assert_eq!(p, 4088);
    }

    #[test]
    fn test_parse_listen_addr_localhost() {
        let (h, p) = parse_listen_addr("127.0.0.1:8080").unwrap();
        assert_eq!(h, "127.0.0.1");
        assert_eq!(p, 8080);
    }

    #[test]
    fn test_parse_listen_addr_ipv6() {
        let (h, p) = parse_listen_addr("[::1]:4088").unwrap();
        assert_eq!(h, "[::1]");
        assert_eq!(p, 4088);
    }

    #[test]
    fn test_parse_listen_addr_no_port() {
        assert!(parse_listen_addr("0.0.0.0").is_err());
    }

    #[test]
    fn test_parse_listen_addr_bad_port() {
        assert!(parse_listen_addr("0.0.0.0:abc").is_err());
    }

    #[test]
    fn test_storage_backend_to_proto_memory() {
        assert_eq!(
            storage_backend_to_proto(&crate::config::StorageBackend::Memory),
            1
        );
    }

    #[test]
    fn test_storage_backend_to_proto_file() {
        assert_eq!(
            storage_backend_to_proto(&crate::config::StorageBackend::File),
            2
        );
    }

    #[test]
    fn test_storage_backend_to_proto_custom() {
        assert_eq!(
            storage_backend_to_proto(&crate::config::StorageBackend::Custom),
            0
        );
    }

    #[test]
    fn test_format_marked_string_plain() {
        use crate::lsp::protocol::MarkedString;
        assert_eq!(
            format_marked_string(&MarkedString::String("hello".into())),
            "hello"
        );
    }

    #[test]
    fn test_format_marked_string_language() {
        use crate::lsp::protocol::MarkedString;
        let m = MarkedString::LanguageString {
            language: "rust".into(),
            value: "fn main(){}".into(),
        };
        assert_eq!(format_marked_string(&m), "```rust\nfn main(){}\n```");
    }

    #[test]
    fn test_format_hover_scalar() {
        use crate::lsp::protocol::{HoverContents, MarkedString};
        let c = HoverContents::Scalar(MarkedString::String("hover".into()));
        assert_eq!(format_hover_contents(&c), "hover");
    }

    #[test]
    fn test_format_hover_array() {
        use crate::lsp::protocol::{HoverContents, MarkedString};
        let c = HoverContents::Array(vec![
            MarkedString::String("a".into()),
            MarkedString::String("b".into()),
        ]);
        assert_eq!(format_hover_contents(&c), "a\n\nb");
    }

    #[test]
    fn test_format_hover_markup() {
        use crate::lsp::protocol::{HoverContents, MarkupContent, MarkupKind};
        let c = HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: "# Title".into(),
        });
        assert_eq!(format_hover_contents(&c), "# Title");
    }

    #[test]
    fn test_convert_definition_scalar() {
        use crate::lsp::protocol::{GotoDefinitionResponse, Location, Position, Range};
        let r = GotoDefinitionResponse::Scalar(Location {
            uri: "file:///t.rs".into(),
            range: Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 10,
                },
            },
        });
        let locs = convert_definition_response(&r);
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].uri, "file:///t.rs");
    }

    #[test]
    fn test_convert_definition_array() {
        use crate::lsp::protocol::{GotoDefinitionResponse, Location, Position, Range};
        let r = GotoDefinitionResponse::Array(vec![
            Location {
                uri: "file:///a.rs".into(),
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: Position {
                        line: 0,
                        character: 5,
                    },
                },
            },
            Location {
                uri: "file:///b.rs".into(),
                range: Range {
                    start: Position {
                        line: 1,
                        character: 0,
                    },
                    end: Position {
                        line: 1,
                        character: 5,
                    },
                },
            },
        ]);
        assert_eq!(convert_definition_response(&r).len(), 2);
    }

    #[test]
    fn test_convert_definition_link() {
        use crate::lsp::protocol::{GotoDefinitionResponse, LocationLink, Position, Range};
        let r = GotoDefinitionResponse::Link(vec![LocationLink {
            origin_selection_range: None,
            target_uri: "file:///t.rs".into(),
            target_range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 5,
                    character: 0,
                },
            },
            target_selection_range: Range {
                start: Position {
                    line: 1,
                    character: 4,
                },
                end: Position {
                    line: 1,
                    character: 20,
                },
            },
        }]);
        let locs = convert_definition_response(&r);
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].uri, "file:///t.rs");
    }

    #[test]
    fn test_parse_skill_metadata_full() {
        let c = "---\nversion: 1.0.0\ndescription: Test\n---\n# Content";
        let (v, d) = CodeAgentServiceImpl::parse_skill_metadata(c);
        assert_eq!(v, Some("1.0.0".into()));
        assert_eq!(d, Some("Test".into()));
    }

    #[test]
    fn test_parse_skill_metadata_quoted() {
        let c = "---\nversion: \"2.0\"\ndescription: 'Desc'\n---\n";
        let (v, d) = CodeAgentServiceImpl::parse_skill_metadata(c);
        assert_eq!(v, Some("2.0".into()));
        assert_eq!(d, Some("Desc".into()));
    }

    #[test]
    fn test_parse_skill_metadata_none() {
        let (v, d) = CodeAgentServiceImpl::parse_skill_metadata("no frontmatter");
        assert!(v.is_none());
        assert!(d.is_none());
    }

    #[test]
    fn test_parse_skill_metadata_partial() {
        let c = "---\nversion: 1.0\n---\n";
        let (v, d) = CodeAgentServiceImpl::parse_skill_metadata(c);
        assert_eq!(v, Some("1.0".into()));
        assert!(d.is_none());
    }

    #[test]
    fn test_parse_skill_metadata_empty_fm() {
        let (v, d) = CodeAgentServiceImpl::parse_skill_metadata("---\n---\n");
        assert!(v.is_none());
        assert!(d.is_none());
    }

    #[test]
    fn test_parse_skill_metadata_unclosed() {
        let (v, d) = CodeAgentServiceImpl::parse_skill_metadata("---\nversion: 1.0\nno end");
        assert!(v.is_none());
        assert!(d.is_none());
    }

    fn make_test_service() -> CodeAgentServiceImpl {
        let store = Arc::new(crate::store::MemorySessionStore::new());
        let tool_executor = Arc::new(crate::tools::ToolExecutor::new("/tmp".to_string()));
        let sm = Arc::new(crate::session::SessionManager::with_store(
            None,
            tool_executor,
            store,
        ));
        CodeAgentServiceImpl::new(sm)
    }

    #[tokio::test]
    async fn test_health_check_not_initialized() {
        let svc = make_test_service();
        let r = svc
            .health_check(Request::new(HealthCheckRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(r.status, health_check_response::Status::Degraded as i32);
    }

    #[tokio::test]
    async fn test_health_check_after_init() {
        let svc = make_test_service();
        svc.initialize(Request::new(InitializeRequest {
            workspace: "/tmp".into(),
            env: Default::default(),
        }))
        .await
        .unwrap();
        let r = svc
            .health_check(Request::new(HealthCheckRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(r.status, health_check_response::Status::Healthy as i32);
    }

    #[tokio::test]
    async fn test_initialize_success() {
        let svc = make_test_service();
        let r = svc
            .initialize(Request::new(InitializeRequest {
                workspace: "/tmp/w".into(),
                env: Default::default(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.success);
        assert!(r.info.is_some());
        assert_eq!(r.info.unwrap().name, "a3s-code");
    }

    #[tokio::test]
    async fn test_shutdown() {
        let svc = make_test_service();
        svc.initialize(Request::new(InitializeRequest {
            workspace: "/tmp".into(),
            env: Default::default(),
        }))
        .await
        .unwrap();
        let r = svc
            .shutdown(Request::new(ShutdownRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert!(r.success);
        let h = svc
            .health_check(Request::new(HealthCheckRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(h.status, health_check_response::Status::Degraded as i32);
    }

    #[tokio::test]
    async fn test_get_capabilities() {
        let svc = make_test_service();
        let r = svc
            .get_capabilities(Request::new(GetCapabilitiesRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert!(r.info.is_some());
        assert!(!r.features.is_empty());
        assert!(r.features.contains(&"streaming".to_string()));
        assert!(!r.tools.is_empty());
        assert!(!r.models.is_empty());
        assert!(r.limits.is_some());
    }

    #[tokio::test]
    async fn test_create_session_custom_id() {
        let svc = make_test_service();
        let r = svc
            .create_session(Request::new(CreateSessionRequest {
                session_id: Some("my-id".into()),
                config: None,
                initial_context: vec![],
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(r.session_id, "my-id");
        assert!(r.session.is_some());
    }

    #[tokio::test]
    async fn test_create_session_auto_id() {
        let svc = make_test_service();
        let r = svc
            .create_session(Request::new(CreateSessionRequest {
                session_id: None,
                config: None,
                initial_context: vec![],
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!r.session_id.is_empty());
        assert!(uuid::Uuid::parse_str(&r.session_id).is_ok());
    }

    #[tokio::test]
    async fn test_create_session_with_config() {
        let svc = make_test_service();
        let r = svc
            .create_session(Request::new(CreateSessionRequest {
                session_id: None,
                config: Some(proto::SessionConfig {
                    name: "test-sess".into(),
                    workspace: "/tmp/ws".into(),
                    llm: None,
                    system_prompt: "Be helpful".into(),
                    max_context_length: 50000,
                    auto_compact: true,
                    auto_compact_threshold: 0.8,
                    storage_type: 1,
                }),
                initial_context: vec![],
            }))
            .await
            .unwrap()
            .into_inner();
        let cfg = r.session.unwrap().config.unwrap();
        assert_eq!(cfg.name, "test-sess");
    }

    #[tokio::test]
    async fn test_create_and_destroy_session() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("del-me".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        let r = svc
            .destroy_session(Request::new(DestroySessionRequest {
                session_id: "del-me".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.success);
        assert!(svc
            .get_session(Request::new(GetSessionRequest {
                session_id: "del-me".into()
            }))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("s1".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("s2".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        let r = svc
            .list_sessions(Request::new(ListSessionsRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(r.sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_get_session_details() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("det".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        let r = svc
            .get_session(Request::new(GetSessionRequest {
                session_id: "det".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        let s = r.session.unwrap();
        assert_eq!(s.session_id, "det");
        assert!(s.context_usage.is_some());
    }

    #[tokio::test]
    async fn test_get_session_not_found() {
        let svc = make_test_service();
        let r = svc
            .get_session(Request::new(GetSessionRequest {
                session_id: "nope".into(),
            }))
            .await;
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn test_configure_session() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("cfg".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        let r = svc
            .configure_session(Request::new(ConfigureSessionRequest {
                session_id: "cfg".into(),
                config: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.session.is_some());
    }

    #[tokio::test]
    async fn test_get_messages_empty() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("msg".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        let r = svc
            .get_messages(Request::new(GetMessagesRequest {
                session_id: "msg".into(),
                limit: Some(10),
                offset: Some(0),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.messages.is_empty());
        assert_eq!(r.total_count, 0);
        assert!(!r.has_more);
    }

    #[tokio::test]
    async fn test_get_context_usage() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("ctx".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        let r = svc
            .get_context_usage(Request::new(GetContextUsageRequest {
                session_id: "ctx".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.usage.is_some());
    }

    #[tokio::test]
    async fn test_compact_context() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("cmp".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        let r = svc
            .compact_context(Request::new(CompactContextRequest {
                session_id: "cmp".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.success);
        assert!(r.before.is_some());
        assert!(r.after.is_some());
    }

    #[tokio::test]
    async fn test_clear_context() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("clr".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        assert!(
            svc.clear_context(Request::new(ClearContextRequest {
                session_id: "clr".into()
            }))
            .await
            .unwrap()
            .into_inner()
            .success
        );
    }

    #[tokio::test]
    async fn test_list_skills() {
        let svc = make_test_service();
        let r = svc
            .list_skills(Request::new(ListSkillsRequest { session_id: None }))
            .await
            .unwrap()
            .into_inner();
        assert!(!r.skills.is_empty());
    }

    #[tokio::test]
    async fn test_load_and_unload_skill() {
        let svc = make_test_service();
        let lr = svc
            .load_skill(Request::new(LoadSkillRequest {
                session_id: "x".into(),
                skill_name: "sk".into(),
                skill_content: Some("---\nversion: 1.0\n---\ncontent".into()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(lr.success);
        let ur = svc
            .unload_skill(Request::new(UnloadSkillRequest {
                session_id: "x".into(),
                skill_name: "sk".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(ur.success);
    }

    #[tokio::test]
    async fn test_unload_nonexistent_skill() {
        let svc = make_test_service();
        assert!(
            svc.unload_skill(Request::new(UnloadSkillRequest {
                session_id: "x".into(),
                skill_name: "nope".into()
            }))
            .await
            .unwrap()
            .into_inner()
            .success
        );
    }

    #[tokio::test]
    async fn test_get_claude_code_skills_grpc_has_builtins() {
        let svc = make_test_service();
        let r = CodeAgentService::get_claude_code_skills(
            &svc,
            Request::new(GetClaudeCodeSkillsRequest { name: None }),
        )
        .await
        .unwrap()
        .into_inner();
        // Should have built-in Claude Code skills (find-skills)
        assert!(!r.skills.is_empty());
        assert!(r.skills.iter().any(|s| s.name == "find-skills"));
    }

    #[tokio::test]
    async fn test_get_claude_code_skills_grpc_find_skills_by_name() {
        let svc = make_test_service();
        let r = CodeAgentService::get_claude_code_skills(
            &svc,
            Request::new(GetClaudeCodeSkillsRequest {
                name: Some("find-skills".into()),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(r.skills.len(), 1);
        assert_eq!(r.skills[0].name, "find-skills");
        assert!(!r.skills[0].content.is_empty());
    }

    #[tokio::test]
    async fn test_get_claude_code_skills_grpc_not_found() {
        let svc = make_test_service();
        let r = CodeAgentService::get_claude_code_skills(
            &svc,
            Request::new(GetClaudeCodeSkillsRequest {
                name: Some("nope".into()),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(r.skills.is_empty());
    }

    #[tokio::test]
    async fn test_pause_and_resume() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("pr".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        assert!(
            svc.pause(Request::new(PauseRequest {
                session_id: "pr".into()
            }))
            .await
            .unwrap()
            .into_inner()
            .success
        );
        assert!(
            svc.resume(Request::new(ResumeRequest {
                session_id: "pr".into()
            }))
            .await
            .unwrap()
            .into_inner()
            .success
        );
    }

    #[tokio::test]
    async fn test_get_confirmation_policy() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("hp".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        assert!(svc
            .get_confirmation_policy(Request::new(GetConfirmationPolicyRequest {
                session_id: "hp".into()
            }))
            .await
            .unwrap()
            .into_inner()
            .policy
            .is_some());
    }

    #[tokio::test]
    async fn test_confirm_tool_not_found() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("cf".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        let r = svc
            .confirm_tool_execution(Request::new(ConfirmToolExecutionRequest {
                session_id: "cf".into(),
                tool_id: "nope".into(),
                approved: true,
                reason: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!r.success);
        assert!(r.error.contains("No pending confirmation"));
    }

    #[tokio::test]
    async fn test_get_permission_policy() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("pp".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        assert!(svc
            .get_permission_policy(Request::new(GetPermissionPolicyRequest {
                session_id: "pp".into()
            }))
            .await
            .unwrap()
            .into_inner()
            .policy
            .is_some());
    }

    #[tokio::test]
    async fn test_check_permission() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("cp".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        let r = svc
            .check_permission(Request::new(CheckPermissionRequest {
                session_id: "cp".into(),
                tool_name: "bash".into(),
                arguments: "{}".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.decision >= 0);
    }

    #[tokio::test]
    async fn test_get_todos_empty() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("td".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        assert!(svc
            .get_todos(Request::new(GetTodosRequest {
                session_id: "td".into()
            }))
            .await
            .unwrap()
            .into_inner()
            .todos
            .is_empty());
    }

    #[tokio::test]
    async fn test_list_mcp_servers_empty() {
        let svc = make_test_service();
        assert!(svc
            .list_mcp_servers(Request::new(ListMcpServersRequest {}))
            .await
            .unwrap()
            .into_inner()
            .servers
            .is_empty());
    }

    #[tokio::test]
    async fn test_list_lsp_servers_empty() {
        let svc = make_test_service();
        assert!(svc
            .list_lsp_servers(Request::new(ListLspServersRequest {}))
            .await
            .unwrap()
            .into_inner()
            .servers
            .is_empty());
    }

    #[tokio::test]
    async fn test_subscribe_events() {
        let svc = make_test_service();
        let _stream = svc
            .subscribe_events(Request::new(SubscribeEventsRequest {
                session_id: None,
                event_types: vec!["error".into()],
            }))
            .await
            .unwrap()
            .into_inner();
    }

    #[tokio::test]
    async fn test_hook_engine_accessor() {
        let svc = make_test_service();
        let _e = svc.hook_engine();
    }

    #[tokio::test]
    async fn test_provider_config_accessor() {
        let svc = make_test_service();
        let c = svc.provider_config();
        assert!(c.read().await.providers.is_empty());
    }

    #[tokio::test]
    async fn test_get_claude_code_skills_method() {
        let svc = make_test_service();
        assert!(CodeAgentServiceImpl::get_claude_code_skills(&svc)
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn test_get_claude_code_skill_method() {
        let svc = make_test_service();
        assert!(CodeAgentServiceImpl::get_claude_code_skill(&svc, "x")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn test_broadcast_event() {
        let svc = make_test_service();
        let mut rx = svc.event_tx.subscribe();
        svc.broadcast_event(AgentEvent::Start {
            prompt: "hi".into(),
        });
        match rx.recv().await.unwrap() {
            AgentEvent::Start { prompt } => assert_eq!(prompt, "hi"),
            _ => panic!("wrong event"),
        }
    }

    #[tokio::test]
    async fn test_get_mcp_tools_empty() {
        let svc = make_test_service();
        assert!(svc
            .get_mcp_tools(Request::new(GetMcpToolsRequest { server_name: None }))
            .await
            .unwrap()
            .into_inner()
            .tools
            .is_empty());
    }

    #[tokio::test]
    async fn test_get_mcp_tools_filtered() {
        let svc = make_test_service();
        assert!(svc
            .get_mcp_tools(Request::new(GetMcpToolsRequest {
                server_name: Some("x".into())
            }))
            .await
            .unwrap()
            .into_inner()
            .tools
            .is_empty());
    }

    #[tokio::test]
    async fn test_lsp_hover_no_server() {
        let svc = make_test_service();
        assert!(
            !svc.lsp_hover(Request::new(LspHoverRequest {
                file_path: "/tmp/x.rs".into(),
                line: 0,
                column: 0
            }))
            .await
            .unwrap()
            .into_inner()
            .found
        );
    }

    #[tokio::test]
    async fn test_lsp_definition_no_server() {
        let svc = make_test_service();
        assert!(svc
            .lsp_definition(Request::new(LspDefinitionRequest {
                file_path: "/tmp/x.rs".into(),
                line: 0,
                column: 0
            }))
            .await
            .unwrap()
            .into_inner()
            .locations
            .is_empty());
    }

    #[tokio::test]
    async fn test_lsp_references_no_server() {
        let svc = make_test_service();
        assert!(svc
            .lsp_references(Request::new(LspReferencesRequest {
                file_path: "/tmp/x.rs".into(),
                line: 0,
                column: 0,
                include_declaration: false
            }))
            .await
            .unwrap()
            .into_inner()
            .locations
            .is_empty());
    }

    #[tokio::test]
    async fn test_lsp_diagnostics_no_file() {
        let svc = make_test_service();
        assert!(svc
            .lsp_diagnostics(Request::new(LspDiagnosticsRequest { file_path: None }))
            .await
            .unwrap()
            .into_inner()
            .diagnostics
            .is_empty());
    }

    #[tokio::test]
    async fn test_lsp_diagnostics_with_file() {
        let svc = make_test_service();
        assert!(svc
            .lsp_diagnostics(Request::new(LspDiagnosticsRequest {
                file_path: Some("/tmp/x.rs".into())
            }))
            .await
            .unwrap()
            .into_inner()
            .diagnostics
            .is_empty());
    }

    #[tokio::test]
    async fn test_lsp_symbols_no_server() {
        let svc = make_test_service();
        assert!(svc
            .lsp_symbols(Request::new(LspSymbolsRequest {
                query: "test".into(),
                limit: 10
            }))
            .await
            .unwrap()
            .into_inner()
            .symbols
            .is_empty());
    }

    #[tokio::test]
    async fn test_parse_cron_valid() {
        let svc = make_test_service();
        let r = svc
            .parse_cron_schedule(Request::new(ParseCronScheduleRequest {
                input: "0 * * * *".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.success);
    }

    #[tokio::test]
    async fn test_parse_cron_invalid() {
        let svc = make_test_service();
        let r = svc
            .parse_cron_schedule(Request::new(ParseCronScheduleRequest {
                input: "not cron xyz".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!r.success);
    }

    #[tokio::test]
    async fn test_list_pending_external_tasks() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("et".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();
        assert!(svc
            .list_pending_external_tasks(Request::new(ListPendingExternalTasksRequest {
                session_id: "et".into()
            }))
            .await
            .unwrap()
            .into_inner()
            .tasks
            .is_empty());
    }

    // ========================================================================
    // Cron Job CRUD Lifecycle Tests
    // ========================================================================

    #[tokio::test]
    async fn test_cron_list_empty() {
        let svc = make_test_service();
        let tmp = tempfile::tempdir().unwrap();
        svc.initialize(Request::new(InitializeRequest {
            workspace: tmp.path().to_string_lossy().into(),
            env: HashMap::new(),
        }))
        .await
        .unwrap();

        let r = svc
            .list_cron_jobs(Request::new(ListCronJobsRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert!(r.jobs.is_empty());
    }

    #[tokio::test]
    async fn test_cron_create_and_get() {
        let svc = make_test_service();
        let tmp = tempfile::tempdir().unwrap();
        svc.initialize(Request::new(InitializeRequest {
            workspace: tmp.path().to_string_lossy().into(),
            env: HashMap::new(),
        }))
        .await
        .unwrap();

        let r = svc
            .create_cron_job(Request::new(CreateCronJobRequest {
                name: "test-job".into(),
                schedule: "0 * * * *".into(),
                command: "echo hello".into(),
                timeout_ms: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.success, "create failed: {}", r.error);
        let job = r.job.unwrap();
        assert_eq!(job.name, "test-job");
        assert_eq!(job.command, "echo hello");

        // Get by ID
        let g = svc
            .get_cron_job(Request::new(GetCronJobRequest {
                id: Some(job.id.clone()),
                name: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(g.job.is_some());
        assert_eq!(g.job.unwrap().name, "test-job");

        // Get by name
        let g2 = svc
            .get_cron_job(Request::new(GetCronJobRequest {
                id: None,
                name: Some("test-job".into()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(g2.job.is_some());
    }

    #[tokio::test]
    async fn test_cron_create_invalid_schedule() {
        let svc = make_test_service();
        let tmp = tempfile::tempdir().unwrap();
        svc.initialize(Request::new(InitializeRequest {
            workspace: tmp.path().to_string_lossy().into(),
            env: HashMap::new(),
        }))
        .await
        .unwrap();

        let r = svc
            .create_cron_job(Request::new(CreateCronJobRequest {
                name: "bad".into(),
                schedule: "not a cron".into(),
                command: "echo".into(),
                timeout_ms: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!r.success);
        assert!(r.job.is_none());
        assert!(!r.error.is_empty());
    }

    #[tokio::test]
    async fn test_cron_update() {
        let svc = make_test_service();
        let tmp = tempfile::tempdir().unwrap();
        svc.initialize(Request::new(InitializeRequest {
            workspace: tmp.path().to_string_lossy().into(),
            env: HashMap::new(),
        }))
        .await
        .unwrap();

        let r = svc
            .create_cron_job(Request::new(CreateCronJobRequest {
                name: "upd-job".into(),
                schedule: "0 * * * *".into(),
                command: "echo old".into(),
                timeout_ms: None,
            }))
            .await
            .unwrap()
            .into_inner();
        let job_id = r.job.unwrap().id;

        let u = svc
            .update_cron_job(Request::new(UpdateCronJobRequest {
                id: job_id.clone(),
                schedule: None,
                command: Some("echo new".into()),
                timeout_ms: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(u.success, "update failed: {}", u.error);
        assert_eq!(u.job.unwrap().command, "echo new");
    }

    #[tokio::test]
    async fn test_cron_pause_resume() {
        let svc = make_test_service();
        let tmp = tempfile::tempdir().unwrap();
        svc.initialize(Request::new(InitializeRequest {
            workspace: tmp.path().to_string_lossy().into(),
            env: HashMap::new(),
        }))
        .await
        .unwrap();

        let r = svc
            .create_cron_job(Request::new(CreateCronJobRequest {
                name: "pr-job".into(),
                schedule: "0 * * * *".into(),
                command: "echo".into(),
                timeout_ms: None,
            }))
            .await
            .unwrap()
            .into_inner();
        let job_id = r.job.unwrap().id;

        // Pause
        let p = svc
            .pause_cron_job(Request::new(PauseCronJobRequest {
                id: job_id.clone(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(p.success);
        assert_eq!(p.job.unwrap().status, 2); // PAUSED

        // Resume
        let res = svc
            .resume_cron_job(Request::new(ResumeCronJobRequest {
                id: job_id.clone(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(res.success);
        assert_eq!(res.job.unwrap().status, 1); // ACTIVE
    }

    #[tokio::test]
    async fn test_cron_delete() {
        let svc = make_test_service();
        let tmp = tempfile::tempdir().unwrap();
        svc.initialize(Request::new(InitializeRequest {
            workspace: tmp.path().to_string_lossy().into(),
            env: HashMap::new(),
        }))
        .await
        .unwrap();

        let r = svc
            .create_cron_job(Request::new(CreateCronJobRequest {
                name: "del-job".into(),
                schedule: "0 * * * *".into(),
                command: "echo".into(),
                timeout_ms: None,
            }))
            .await
            .unwrap()
            .into_inner();
        let job_id = r.job.unwrap().id;

        let d = svc
            .delete_cron_job(Request::new(DeleteCronJobRequest {
                id: job_id.clone(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(d.success);

        // Verify deleted
        let list = svc
            .list_cron_jobs(Request::new(ListCronJobsRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert!(list.jobs.is_empty());
    }

    #[tokio::test]
    async fn test_cron_run_and_history() {
        let svc = make_test_service();
        let tmp = tempfile::tempdir().unwrap();
        svc.initialize(Request::new(InitializeRequest {
            workspace: tmp.path().to_string_lossy().into(),
            env: HashMap::new(),
        }))
        .await
        .unwrap();

        let r = svc
            .create_cron_job(Request::new(CreateCronJobRequest {
                name: "run-job".into(),
                schedule: "0 * * * *".into(),
                command: "echo hello".into(),
                timeout_ms: None,
            }))
            .await
            .unwrap()
            .into_inner();
        let job_id = r.job.unwrap().id;

        // Run immediately
        let run = svc
            .run_cron_job(Request::new(RunCronJobRequest {
                id: job_id.clone(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(run.success, "run failed: {}", run.error);
        assert!(run.execution.is_some());

        // Check history
        let h = svc
            .get_cron_history(Request::new(GetCronHistoryRequest {
                id: job_id.clone(),
                limit: Some(10),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(!h.executions.is_empty());
    }

    #[tokio::test]
    async fn test_cron_get_missing_args() {
        let svc = make_test_service();
        let tmp = tempfile::tempdir().unwrap();
        svc.initialize(Request::new(InitializeRequest {
            workspace: tmp.path().to_string_lossy().into(),
            env: HashMap::new(),
        }))
        .await
        .unwrap();

        let err = svc
            .get_cron_job(Request::new(GetCronJobRequest {
                id: None,
                name: None,
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // ========================================================================
    // Planning RPC Tests
    // ========================================================================

    #[tokio::test]
    async fn test_create_plan_fallback() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("plan-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        // No LLM configured → falls back to heuristic planner
        let r = svc
            .create_plan(Request::new(CreatePlanRequest {
                session_id: "plan-s".into(),
                prompt: "Refactor the auth module and add tests".into(),
                context: None,
            }))
            .await
            .unwrap()
            .into_inner();

        let plan = r.plan.unwrap();
        assert!(!plan.goal.is_empty());
        assert!(!plan.steps.is_empty());
        assert!(plan.estimated_steps > 0);
    }

    #[tokio::test]
    async fn test_create_plan_session_not_found() {
        let svc = make_test_service();
        let err = svc
            .create_plan(Request::new(CreatePlanRequest {
                session_id: "nonexistent".into(),
                prompt: "do something".into(),
                context: None,
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn test_get_plan_after_create() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("gp-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        // Create a plan first
        svc.create_plan(Request::new(CreatePlanRequest {
            session_id: "gp-s".into(),
            prompt: "Build a REST API".into(),
            context: None,
        }))
        .await
        .unwrap();

        // Now get it
        let r = svc
            .get_plan(Request::new(GetPlanRequest {
                session_id: "gp-s".into(),
                plan_id: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.plan.is_some());
        assert!(!r.plan.unwrap().goal.is_empty());
    }

    #[tokio::test]
    async fn test_get_plan_not_found() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("np-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        // No plan created yet
        let err = svc
            .get_plan(Request::new(GetPlanRequest {
                session_id: "np-s".into(),
                plan_id: String::new(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn test_extract_goal_fallback() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("eg-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        let r = svc
            .extract_goal(Request::new(ExtractGoalRequest {
                session_id: "eg-s".into(),
                prompt: "Fix the login bug and deploy to staging".into(),
            }))
            .await
            .unwrap()
            .into_inner();

        let goal = r.goal.unwrap();
        assert!(!goal.description.is_empty());
        assert!(!goal.achieved);
    }

    #[tokio::test]
    async fn test_check_goal_achievement_fallback() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("cg-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        let r = svc
            .check_goal_achievement(Request::new(CheckGoalAchievementRequest {
                session_id: "cg-s".into(),
                goal: Some(proto::AgentGoal {
                    description: "Deploy the app".into(),
                    success_criteria: vec!["App is running".into()],
                    progress: 0.0,
                    achieved: false,
                    created_at: 0,
                    achieved_at: None,
                }),
                current_state: "Started deployment process".into(),
            }))
            .await
            .unwrap()
            .into_inner();

        // Fallback returns a result (not an error)
        assert!(r.progress >= 0.0);
    }

    #[tokio::test]
    async fn test_check_goal_missing_goal() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("mg-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        let err = svc
            .check_goal_achievement(Request::new(CheckGoalAchievementRequest {
                session_id: "mg-s".into(),
                goal: None,
                current_state: "".into(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    // ========================================================================
    // Metrics & Cost RPC Tests
    // ========================================================================

    #[tokio::test]
    async fn test_get_tool_metrics_empty() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("tm-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        let r = svc
            .get_tool_metrics(Request::new(GetToolMetricsRequest {
                session_id: "tm-s".into(),
                tool_name: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.tools.is_empty());
        assert_eq!(r.total_calls, 0);
        assert_eq!(r.total_duration_ms, 0);
    }

    #[tokio::test]
    async fn test_get_tool_metrics_session_not_found() {
        let svc = make_test_service();
        let err = svc
            .get_tool_metrics(Request::new(GetToolMetricsRequest {
                session_id: "nope".into(),
                tool_name: String::new(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn test_get_tool_metrics_filtered() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("tmf-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        let r = svc
            .get_tool_metrics(Request::new(GetToolMetricsRequest {
                session_id: "tmf-s".into(),
                tool_name: "bash".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.tools.is_empty());
    }

    #[tokio::test]
    async fn test_get_cost_summary_empty_session() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("cs-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        let r = svc
            .get_cost_summary(Request::new(GetCostSummaryRequest {
                session_id: "cs-s".into(),
                model: String::new(),
                start_date: String::new(),
                end_date: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(r.total_cost_usd, 0.0);
        assert_eq!(r.total_tokens, 0);
    }

    #[tokio::test]
    async fn test_get_cost_summary_all_sessions() {
        let svc = make_test_service();
        // Empty session_id → aggregate across all sessions
        let r = svc
            .get_cost_summary(Request::new(GetCostSummaryRequest {
                session_id: String::new(),
                model: String::new(),
                start_date: String::new(),
                end_date: String::new(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(r.total_cost_usd, 0.0);
    }

    #[tokio::test]
    async fn test_get_cost_summary_session_not_found() {
        let svc = make_test_service();
        let err = svc
            .get_cost_summary(Request::new(GetCostSummaryRequest {
                session_id: "nope".into(),
                model: String::new(),
                start_date: String::new(),
                end_date: String::new(),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    // ========================================================================
    // MCP Server Registration Tests
    // ========================================================================

    #[tokio::test]
    async fn test_register_mcp_server_stdio() {
        let svc = make_test_service();
        let r = svc
            .register_mcp_server(Request::new(RegisterMcpServerRequest {
                config: Some(proto::McpServerConfigProto {
                    name: "test-mcp".into(),
                    transport: Some(proto::McpTransport {
                        transport: Some(proto::mcp_transport::Transport::Stdio(
                            proto::McpStdioTransport {
                                command: "echo".into(),
                                args: vec!["hello".into()],
                            },
                        )),
                    }),
                    enabled: true,
                    env: HashMap::new(),
                }),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.success);

        // Verify it shows up in list
        let list = svc
            .list_mcp_servers(Request::new(ListMcpServersRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(list.servers.len(), 1);
        assert_eq!(list.servers[0].name, "test-mcp");
    }

    #[tokio::test]
    async fn test_register_mcp_server_http() {
        let svc = make_test_service();
        let r = svc
            .register_mcp_server(Request::new(RegisterMcpServerRequest {
                config: Some(proto::McpServerConfigProto {
                    name: "http-mcp".into(),
                    transport: Some(proto::McpTransport {
                        transport: Some(proto::mcp_transport::Transport::Http(
                            proto::McpHttpTransport {
                                url: "http://localhost:8080".into(),
                                headers: HashMap::new(),
                            },
                        )),
                    }),
                    enabled: true,
                    env: HashMap::new(),
                }),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(r.success);
    }

    #[tokio::test]
    async fn test_register_mcp_server_missing_config() {
        let svc = make_test_service();
        let err = svc
            .register_mcp_server(Request::new(RegisterMcpServerRequest { config: None }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_register_mcp_server_missing_transport() {
        let svc = make_test_service();
        let err = svc
            .register_mcp_server(Request::new(RegisterMcpServerRequest {
                config: Some(proto::McpServerConfigProto {
                    name: "no-transport".into(),
                    transport: Some(proto::McpTransport { transport: None }),
                    enabled: true,
                    env: HashMap::new(),
                }),
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_disconnect_mcp_unregistered() {
        let svc = make_test_service();
        let r = svc
            .disconnect_mcp_server(Request::new(DisconnectMcpServerRequest {
                name: "nonexistent".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        // Disconnecting a non-existent server should not crash
        // (behavior depends on implementation — may succeed or fail gracefully)
        let _ = r.success;
    }

    // ========================================================================
    // Generate Error Path Tests
    // ========================================================================

    #[tokio::test]
    async fn test_generate_session_not_found() {
        let svc = make_test_service();
        let err = svc
            .generate(Request::new(GenerateRequest {
                session_id: "nonexistent".into(),
                messages: vec![proto::Message {
                    role: "user".into(),
                    content: "hello".into(),
                    metadata: HashMap::new(),
                }],
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
    }

    #[tokio::test]
    async fn test_generate_no_llm_configured() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("gen-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        // No LLM configured → should fail with an error
        let err = svc
            .generate(Request::new(GenerateRequest {
                session_id: "gen-s".into(),
                messages: vec![proto::Message {
                    role: "user".into(),
                    content: "hello".into(),
                    metadata: HashMap::new(),
                }],
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
    }

    #[tokio::test]
    async fn test_generate_empty_messages() {
        let svc = make_test_service();
        svc.create_session(Request::new(CreateSessionRequest {
            session_id: Some("gem-s".into()),
            config: None,
            initial_context: vec![],
        }))
        .await
        .unwrap();

        let err = svc
            .generate(Request::new(GenerateRequest {
                session_id: "gem-s".into(),
                messages: vec![],
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
    }

    // ========================================================================
    // LSP Start/Stop Tests
    // ========================================================================

    #[tokio::test]
    async fn test_start_lsp_server_unsupported_language() {
        let svc = make_test_service();
        let r = svc
            .start_lsp_server(Request::new(StartLspServerRequest {
                language: "brainfuck".into(),
                root_uri: "/tmp".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        // Starting an unsupported language should fail gracefully
        assert!(!r.success);
    }

    #[tokio::test]
    async fn test_stop_lsp_server_not_running() {
        let svc = make_test_service();
        let r = svc
            .stop_lsp_server(Request::new(StopLspServerRequest {
                language: "rust".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        // Stopping a server that isn't running should succeed or fail gracefully
        let _ = r.success;
    }
}
