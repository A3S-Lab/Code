//! gRPC service implementation
//!
//! Implements the CodeAgentService gRPC API defined in code_agent.proto.
//! The service runs on port 4088 and handles:
//! - Lifecycle management (Initialize, Shutdown, HealthCheck, GetCapabilities)
//! - Session management (Create, Destroy, List, Get, Configure)
//! - Code generation (Generate, StreamGenerate, GenerateStructured)
//! - Tool execution (ExecuteTool, ExecuteToolBatch, ListTools, RegisterTool)
//! - Skill management (LoadSkill, UnloadSkill, ListSkills) - skills are global
//! - Context management (GetContextUsage, CompactContext, ClearContext)
//! - Event streaming (SubscribeEvents)
//! - Control operations (Cancel, Pause, Resume)
//! - Human-in-the-Loop (ConfirmToolExecution, SetConfirmationPolicy, GetConfirmationPolicy)
//!
//! ## Skill System
//!
//! Skills are loaded globally and available to all sessions. Use PermissionPolicy
//! to control which tools each session can access.

use crate::agent::AgentEvent;
use crate::convert;
use crate::hooks::{HookEngine, HookEvent, SkillLoadEvent, SkillUnloadEvent};
use crate::llm::{self, ContentBlock};
use crate::session::{SessionConfig, SessionManager};
use crate::tools::ToolExecutor;
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
    /// Tool names loaded from this skill
    tool_names: Vec<String>,
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
            queue_config: None,        // Use default queue config
            confirmation_policy: None, // Use default confirmation policy (HITL disabled)
            permission_policy: None,   // Use default permission policy
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
            finish_reason: FinishReason::Stop as i32,
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
    // Tool Execution
    // ========================================================================

    async fn execute_tool(
        &self,
        request: Request<ExecuteToolRequest>,
    ) -> Result<Response<ExecuteToolResponse>, Status> {
        let req = request.into_inner();

        let args: serde_json::Value =
            serde_json::from_str(&req.arguments).unwrap_or(serde_json::json!({}));

        let result = self
            .session_manager
            .tool_executor()
            .execute(&req.tool_name, &args)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(ExecuteToolResponse {
            result: Some(proto::ToolResult {
                success: result.exit_code == 0,
                output: result.output,
                error: String::new(),
                metadata: HashMap::new(),
            }),
        }))
    }

    async fn execute_tool_batch(
        &self,
        request: Request<ExecuteToolBatchRequest>,
    ) -> Result<Response<ExecuteToolBatchResponse>, Status> {
        let req = request.into_inner();
        let mut results = Vec::new();

        for tool_call in req.tool_calls {
            let args: serde_json::Value =
                serde_json::from_str(&tool_call.arguments).unwrap_or(serde_json::json!({}));

            let result = self
                .session_manager
                .tool_executor()
                .execute(&tool_call.name, &args)
                .await;

            match result {
                Ok(r) => results.push(proto::ToolResult {
                    success: r.exit_code == 0,
                    output: r.output,
                    error: String::new(),
                    metadata: HashMap::new(),
                }),
                Err(e) => results.push(proto::ToolResult {
                    success: false,
                    output: String::new(),
                    error: e.to_string(),
                    metadata: HashMap::new(),
                }),
            }
        }

        Ok(Response::new(ExecuteToolBatchResponse { results }))
    }

    async fn list_tools(
        &self,
        _request: Request<ListToolsRequest>,
    ) -> Result<Response<ListToolsResponse>, Status> {
        let definitions = self.session_manager.tool_executor().definitions();

        let tools: Vec<proto::Tool> = definitions
            .iter()
            .map(|t| proto::Tool {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters_schema: t.parameters.to_string(),
                tags: vec![],
                r#async: false,
            })
            .collect();

        Ok(Response::new(ListToolsResponse { tools }))
    }

    async fn register_tool(
        &self,
        _request: Request<RegisterToolRequest>,
    ) -> Result<Response<RegisterToolResponse>, Status> {
        // Dynamic tool registration is handled through skills
        // This is a placeholder for future direct tool registration
        Ok(Response::new(RegisterToolResponse { success: false }))
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
        _request: Request<CancelRequest>,
    ) -> Result<Response<CancelResponse>, Status> {
        // TODO: Implement cancellation
        Ok(Response::new(CancelResponse { success: true }))
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

    tracing::info!("Workspace: {}", workspace);
    tracing::info!("LLM configuration: Clients must provide via ConfigureSession RPC");

    // Create session manager without default LLM client
    let tool_executor = Arc::new(ToolExecutor::new(workspace));
    let session_manager = Arc::new(SessionManager::new(None, tool_executor));

    let service = CodeAgentServiceImpl::new(session_manager);

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
