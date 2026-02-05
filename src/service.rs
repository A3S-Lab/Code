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
use crate::tools::{ClaudeCodeSkill, ToolExecutor};
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
    /// MCP manager for external tool servers
    mcp_manager: Arc<McpManager>,
    /// LSP manager for language servers
    lsp_manager: Arc<LspManager>,
}

impl CodeAgentServiceImpl {
    pub fn new(session_manager: Arc<SessionManager>) -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            session_manager,
            agent_state: Arc::new(RwLock::new(AgentState::default())),
            event_tx,
            hook_engine: Arc::new(HookEngine::new()),
            skill_registry: Arc::new(RwLock::new(HashMap::new())),
            provider_config: Arc::new(RwLock::new(CodeConfig::default())),
            mcp_manager: Arc::new(McpManager::new()),
            lsp_manager: Arc::new(LspManager::new()),
        }
    }

    /// Create a new service with initial configuration
    pub fn with_config(session_manager: Arc<SessionManager>, config: CodeConfig) -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            session_manager,
            agent_state: Arc::new(RwLock::new(AgentState::default())),
            event_tx,
            hook_engine: Arc::new(HookEngine::new()),
            skill_registry: Arc::new(RwLock::new(HashMap::new())),
            provider_config: Arc::new(RwLock::new(config)),
            mcp_manager: Arc::new(McpManager::new()),
            lsp_manager: Arc::new(LspManager::new()),
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

        let config = req.config.unwrap_or_default();

        // Convert proto StorageType to config::StorageBackend
        let storage_type = match config.storage_type {
            0 => crate::config::StorageBackend::File, // STORAGE_TYPE_UNSPECIFIED defaults to File
            1 => crate::config::StorageBackend::Memory, // STORAGE_TYPE_MEMORY
            2 => crate::config::StorageBackend::File, // STORAGE_TYPE_FILE
            _ => crate::config::StorageBackend::File, // Unknown defaults to File
        };

        let session_config = SessionConfig {
            name: config.name,
            workspace: config.workspace,
            system_prompt: if config.system_prompt.is_empty() {
                None
            } else {
                Some(config.system_prompt)
            },
            max_context_length: config.max_context_length,
            auto_compact: config.auto_compact,
            storage_type,
            queue_config: None,        // Use default queue config
            confirmation_policy: None, // Use default confirmation policy (HITL disabled)
            permission_policy: None,   // Use default permission policy
            parent_id: None,           // Not a child session
        };

        self.session_manager
            .create_session(session_id.clone(), session_config)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

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
                    llm: None,
                    system_prompt: session.config.system_prompt.clone().unwrap_or_default(),
                    max_context_length: session.config.max_context_length,
                    auto_compact: session.config.auto_compact,
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

        // Convert proto LLMConfig to internal LlmConfig if provided
        let model_config = req.config.as_ref().and_then(|c| {
            c.llm.as_ref().map(|llm| {
                let mut config = llm::LlmConfig::new(&llm.provider, &llm.model, &llm.api_key);
                if !llm.base_url.is_empty() {
                    config = config.with_base_url(&llm.base_url);
                }
                config
            })
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
            .map(|(i, msg)| convert::internal_message_to_conversation_message(msg, offset + i, timestamp))
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

        let proto_todos = todos
            .iter()
            .map(convert::internal_todo_to_proto)
            .collect();

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

        Ok(Response::new(AddProviderResponse {
            success: true,
            error: String::new(),
            provider: Some(convert::internal_provider_config_to_proto(&internal_provider)),
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
                Ok(Response::new(UpdateProviderResponse {
                    success: true,
                    error: String::new(),
                    provider: Some(convert::internal_provider_config_to_proto(&internal_provider)),
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
        let provider = provider.unwrap();
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
        let session = self.session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Create a simple execution plan
        // TODO: Use LLM to generate more sophisticated plans
        let complexity = if req.prompt.len() < 50 {
            crate::planning::Complexity::Simple
        } else if req.prompt.len() < 150 {
            crate::planning::Complexity::Medium
        } else if req.prompt.len() < 300 {
            crate::planning::Complexity::Complex
        } else {
            crate::planning::Complexity::VeryComplex
        };

        let mut plan = crate::planning::ExecutionPlan::new(&req.prompt, complexity);

        // Add basic steps based on complexity
        let step_count = match complexity {
            crate::planning::Complexity::Simple => 2,
            crate::planning::Complexity::Medium => 4,
            crate::planning::Complexity::Complex => 7,
            crate::planning::Complexity::VeryComplex => 10,
        };

        for i in 0..step_count {
            let step = crate::planning::PlanStep::new(
                format!("step-{}", i + 1),
                format!("Execute step {} of the plan", i + 1),
            );
            plan.add_step(step);
        }

        // Store plan in session
        let session_guard = session.read().await;
        let mut current_plan = session_guard.current_plan.write().await;
        *current_plan = Some(plan.clone());

        // Convert to proto
        let proto_plan = ExecutionPlan {
            goal: plan.goal,
            steps: plan.steps.iter().map(|step| {
                PlanStep {
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
                    success_criteria: step.success_criteria.as_ref()
                        .map(|s| vec![s.clone()])
                        .unwrap_or_default(),
                }
            }).collect(),
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
        let session = self.session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Get current plan
        let session_guard = session.read().await;
        let current_plan = session_guard.current_plan.read().await;

        let plan = current_plan.as_ref()
            .ok_or_else(|| Status::not_found("No plan found for this session"))?;

        // Convert to proto
        let proto_plan = ExecutionPlan {
            goal: plan.goal.clone(),
            steps: plan.steps.iter().map(|step| {
                PlanStep {
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
                    success_criteria: step.success_criteria.as_ref()
                        .map(|s| vec![s.clone()])
                        .unwrap_or_default(),
                }
            }).collect(),
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
        let _session = self.session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Extract goal from prompt
        // TODO: Use LLM to extract goal and success criteria
        let goal = crate::planning::AgentGoal::new(&req.prompt)
            .with_criteria(vec![
                "Task is completed successfully".to_string(),
                "All requirements are met".to_string(),
            ]);

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
        let _session = self.session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        let goal = req.goal.ok_or_else(|| Status::invalid_argument("Goal is required"))?;

        // Simple heuristic: check if current_state mentions completion
        // TODO: Use LLM to evaluate goal achievement
        let achieved = req.current_state.to_lowercase().contains("complete")
            || req.current_state.to_lowercase().contains("done")
            || req.current_state.to_lowercase().contains("finished");

        let progress = if achieved { 1.0 } else { goal.progress };

        // Find remaining criteria (simple heuristic)
        let remaining_criteria: Vec<String> = if achieved {
            Vec::new()
        } else {
            goal.success_criteria.clone()
        };

        Ok(Response::new(CheckGoalAchievementResponse {
            achieved,
            progress,
            remaining_criteria,
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
        let session = self.session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Extract memory from request
        let proto_memory = req.memory.ok_or_else(|| Status::invalid_argument("Memory is required"))?;

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
            last_accessed: proto_memory.last_accessed
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
        memory.remember(memory_item).await
            .map_err(|e| Status::internal(format!("Failed to store memory: {}", e)))?;

        // Emit memory stored event
        let _ = session_guard.event_tx().send(crate::agent::AgentEvent::MemoryStored {
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
        let session = self.session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Retrieve memory from store
        let session_guard = session.read().await;
        let memory = session_guard.memory.read().await;

        // Access the underlying store to retrieve by ID
        let memory_item = memory.store().retrieve(&req.memory_id).await
            .map_err(|e| Status::internal(format!("Failed to retrieve memory: {}", e)))?;

        // Convert to proto MemoryItem
        let proto_memory = memory_item.map(|item| {
            MemoryItem {
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
            }
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
        let session = self.session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Search memories
        let session_guard = session.read().await;
        let memory = session_guard.memory.read().await;
        let limit = if req.limit == 0 { 10 } else { req.limit as usize };

        let mut memories = if !req.tags.is_empty() {
            // Search by tags
            memory.recall_by_tags(&req.tags, limit).await
                .map_err(|e| Status::internal(format!("Failed to search memories: {}", e)))?
        } else if let Some(query) = req.query {
            // Search by query
            memory.recall_similar(&query, limit).await
                .map_err(|e| Status::internal(format!("Failed to search memories: {}", e)))?
        } else {
            // Return recent memories (up to limit)
            memory.get_recent(limit).await
                .map_err(|e| Status::internal(format!("Failed to get memories: {}", e)))?
        };

        // Filter by importance if specified
        if let Some(min_importance) = req.min_importance {
            memories.retain(|m| m.importance >= min_importance);
        }

        // Convert to proto MemoryItems
        let proto_memories: Vec<_> = memories.iter().map(|item| {
            MemoryItem {
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
            }
        }).collect();

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
        let session = self.session_manager
            .get_session(&req.session_id)
            .await
            .map_err(|e| Status::not_found(format!("Session not found: {}", e)))?;

        // Get memory statistics
        let session_guard = session.read().await;
        let memory = session_guard.memory.read().await;
        let stats = memory.stats().await
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
        let session = self.session_manager
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
            let long_term_count = memory.store().count().await
                .map_err(|e| Status::internal(format!("Failed to count long-term memories: {}", e)))?;
            memory.store().clear().await
                .map_err(|e| Status::internal(format!("Failed to clear long-term memories: {}", e)))?;
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

        let config_proto = req.config.ok_or_else(|| Status::invalid_argument("Missing config"))?;

        // Convert proto to internal config
        let transport = config_proto
            .transport
            .ok_or_else(|| Status::invalid_argument("Missing transport"))?;

        let transport_config = match transport.transport {
            Some(proto::mcp_transport::Transport::Stdio(stdio)) => {
                McpTransportConfig::Stdio {
                    command: stdio.command,
                    args: stdio.args,
                }
            }
            Some(proto::mcp_transport::Transport::Http(http)) => {
                McpTransportConfig::Http {
                    url: http.url,
                    headers: http.headers,
                }
            }
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
                Ok(Response::new(DisconnectMcpServerResponse { success: false }))
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
                let parts: Vec<&str> = full_name.strip_prefix("mcp__").unwrap_or(&full_name).splitn(2, "__").collect();
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
            let root_path = req.root_uri.strip_prefix("file://").unwrap_or(&req.root_uri);
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
        let limit = if req.limit == 0 { 20 } else { req.limit as usize };

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
        Ok(Response::new(LspSymbolsResponse { symbols: all_symbols }))
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
                    return Ok(Response::new(LspDiagnosticsResponse { diagnostics: vec![] }));
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
                        Some(crate::lsp::protocol::DiagnosticSeverity::Error) => "error".to_string(),
                        Some(crate::lsp::protocol::DiagnosticSeverity::Warning) => "warning".to_string(),
                        Some(crate::lsp::protocol::DiagnosticSeverity::Information) => "info".to_string(),
                        Some(crate::lsp::protocol::DiagnosticSeverity::Hint) => "hint".to_string(),
                        None => "unknown".to_string(),
                    },
                    message: d.message,
                    code: match d.code {
                        Some(crate::lsp::protocol::DiagnosticCode::String(s)) => Some(s),
                        Some(crate::lsp::protocol::DiagnosticCode::Number(n)) => Some(n.to_string()),
                        None => None,
                    },
                    source: d.source,
                })
                .collect();

            Ok(Response::new(LspDiagnosticsResponse { diagnostics }))
        } else {
            Ok(Response::new(LspDiagnosticsResponse { diagnostics: vec![] }))
        }
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
pub async fn start_server() -> Result<()> {
    // Get configuration from environment
    let workspace = std::env::var("WORKSPACE")
        .or_else(|_| std::env::var("A3S_WORKSPACE"))
        .unwrap_or_else(|_| "/a3s/workspace".to_string());

    let listen_addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:4088".to_string());

    // Use default config
    let config = CodeConfig::default();

    start_server_with_config(config, &workspace, &listen_addr).await
}

/// Start the gRPC server with the given configuration
pub async fn start_server_with_config(
    config: CodeConfig,
    workspace: &str,
    listen_addr: &str,
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

            tracing::info!("Using file-based session storage: {}", sessions_dir.display());

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

    let service = CodeAgentServiceImpl::with_config(session_manager, config);

    // Parse the base address to extract host and port
    let (host, base_port) = parse_listen_addr(&listen_addr)?;

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

        let service = CodeAgentServiceImpl::with_config(session_manager, config);

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
}
