//! RESTful API for A3S Code
//!
//! Axum-based HTTP/JSON API that provides:
//! - Session management (CRUD)
//! - Agent execution (send / stream via SSE)
//! - Direct tool execution
//! - OpenAI-compatible `/v1/chat/completions` endpoint
//! - Bearer token authentication
//! - Swagger UI at `/docs`

use crate::agent::AgentEvent;
use crate::config::CodeConfig;
use anyhow::Result;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::{OpenApi, ToSchema};

// ============================================================================
// OpenAPI Schema
// ============================================================================

#[derive(OpenApi)]
#[openapi(
    paths(
        list_sessions,
        create_session,
        get_session,
        delete_session,
        send_message,
        stream_message,
        execute_tool,
        chat_completions,
        health,
    ),
    components(schemas(
        CreateSessionRequest,
        SessionResponse,
        SendRequest,
        SendResponse,
        StreamRequest,
        ToolExecRequest,
        ToolExecResponse,
        ChatCompletionRequest,
        ChatCompletionResponse,
        ChatMessage,
        ChatChoice,
        ChatUsage,
        HealthResponse,
        ErrorResponse,
    )),
    tags(
        (name = "sessions", description = "Session management"),
        (name = "agent", description = "Agent execution"),
        (name = "tools", description = "Direct tool execution"),
        (name = "openai", description = "OpenAI-compatible API"),
        (name = "system", description = "System endpoints"),
    )
)]
pub struct ApiDoc;

// ============================================================================
// Shared State
// ============================================================================

/// Shared application state for the REST API.
#[derive(Clone)]
pub struct AppState {
    /// Active agent sessions keyed by session ID
    pub agents: Arc<RwLock<HashMap<String, AgentSession>>>,
    pub config: Arc<RwLock<CodeConfig>>,
    pub api_token: Option<String>,
}

/// A REST API session wrapping an Agent.
pub struct AgentSession {
    pub agent: crate::agent_api::Agent,
    pub model: String,
    pub workspace: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ============================================================================
// Request / Response Types
// ============================================================================

#[derive(Deserialize, ToSchema)]
pub struct CreateSessionRequest {
    /// LLM model identifier (e.g., "claude-sonnet-4-20250514", "gpt-4o")
    pub model: String,
    /// API key for the LLM provider
    pub api_key: String,
    /// Workspace directory path
    pub workspace: Option<String>,
    /// System prompt
    pub system_prompt: Option<String>,
    /// Base URL override for the LLM API
    pub base_url: Option<String>,
    /// Maximum tool execution rounds per turn
    pub max_tool_rounds: Option<usize>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SessionResponse {
    pub id: String,
    pub model: String,
    pub workspace: String,
    pub created_at: String,
}

#[derive(Deserialize, ToSchema)]
pub struct SendRequest {
    /// The user prompt
    pub prompt: String,
}

#[derive(Serialize, ToSchema)]
pub struct SendResponse {
    pub text: String,
    pub tool_calls_count: usize,
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

#[derive(Deserialize, ToSchema)]
pub struct StreamRequest {
    /// The user prompt
    pub prompt: String,
}

#[derive(Deserialize, ToSchema)]
pub struct ToolExecRequest {
    /// Tool name (e.g., "read", "bash", "glob", "grep")
    pub name: String,
    /// Tool arguments as JSON object
    pub args: serde_json::Value,
}

#[derive(Serialize, ToSchema)]
pub struct ToolExecResponse {
    pub name: String,
    pub output: String,
    pub exit_code: i32,
}

#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

// OpenAI-compatible types

#[derive(Deserialize, ToSchema)]
pub struct ChatCompletionRequest {
    pub model: String,
    /// Messages in OpenAI format
    pub messages: Vec<ChatMessage>,
    /// Whether to stream the response (SSE)
    #[serde(default)]
    pub stream: bool,
    /// API key (alternative to Authorization header)
    pub api_key: Option<String>,
    /// Workspace directory
    pub workspace: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, ToSchema)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, ToSchema)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: ChatUsage,
}

#[derive(Serialize, ToSchema)]
pub struct ChatChoice {
    pub index: usize,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Serialize, ToSchema)]
pub struct ChatUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

// ============================================================================
// Router
// ============================================================================

/// Build the Axum router for the REST API.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        // System
        .route("/health", get(health))
        // Sessions
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/:id", get(get_session).delete(delete_session))
        // Agent execution
        .route("/sessions/:id/send", post(send_message))
        .route("/sessions/:id/stream", post(stream_message))
        // Direct tool execution
        .route("/sessions/:id/tool", post(execute_tool))
        // OpenAI-compatible
        .route("/v1/chat/completions", post(chat_completions));

    let swagger = utoipa_swagger_ui::SwaggerUi::new("/docs")
        .url("/api-doc/openapi.json", ApiDoc::openapi());

    Router::new()
        .merge(api)
        .merge(swagger)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Start the REST server on the given address.
pub async fn start_rest_server(state: AppState, listen_addr: &str) -> Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    tracing::info!("REST API listening on {}", listen_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

// ============================================================================
// Auth Helper
// ============================================================================

/// Extract and validate bearer token from headers.
fn check_auth(headers: &HeaderMap, state: &AppState) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let Some(ref expected) = state.api_token else {
        return Ok(()); // No token configured = open access
    };

    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match auth {
        Some(token) if token == expected => Ok(()),
        _ => Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Invalid or missing Bearer token".into(),
            }),
        )),
    }
}

/// Helper to build an Agent from session-like params.
async fn build_agent(
    model: &str,
    api_key: &str,
    workspace: Option<&str>,
    system_prompt: Option<&str>,
    base_url: Option<&str>,
    max_tool_rounds: Option<usize>,
) -> Result<crate::agent_api::Agent> {
    let mut builder = crate::agent_api::Agent::builder()
        .model(model)
        .api_key(api_key);

    if let Some(ws) = workspace {
        builder = builder.workspace(ws);
    }
    if let Some(sp) = system_prompt {
        builder = builder.system_prompt(sp);
    }
    if let Some(url) = base_url {
        builder = builder.base_url(url);
    }
    if let Some(max) = max_tool_rounds {
        builder = builder.max_tool_rounds(max);
    }

    builder.build().await
}

// ============================================================================
// Endpoint Handlers
// ============================================================================

/// Health check
#[utoipa::path(get, path = "/health", tag = "system",
    responses((status = 200, body = HealthResponse))
)]
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

/// List all active sessions
#[utoipa::path(get, path = "/sessions", tag = "sessions",
    responses((status = 200, body = Vec<SessionResponse>))
)]
async fn list_sessions(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<SessionResponse>>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&headers, &state)?;
    let agents = state.agents.read().await;
    let sessions: Vec<SessionResponse> = agents
        .iter()
        .map(|(id, s)| SessionResponse {
            id: id.clone(),
            model: s.model.clone(),
            workspace: s.workspace.clone(),
            created_at: s.created_at.to_rfc3339(),
        })
        .collect();
    Ok(Json(sessions))
}

/// Create a new session
#[utoipa::path(post, path = "/sessions", tag = "sessions",
    request_body = CreateSessionRequest,
    responses(
        (status = 201, body = SessionResponse),
        (status = 400, body = ErrorResponse),
    )
)]
async fn create_session(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionResponse>), (StatusCode, Json<ErrorResponse>)> {
    check_auth(&headers, &state)?;

    let workspace = req.workspace.clone().unwrap_or_else(|| "/tmp".into());
    let agent = build_agent(
        &req.model,
        &req.api_key,
        req.workspace.as_deref(),
        req.system_prompt.as_deref(),
        req.base_url.as_deref(),
        req.max_tool_rounds,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Failed to create agent: {e}"),
            }),
        )
    })?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let session = AgentSession {
        agent,
        model: req.model.clone(),
        workspace: workspace.clone(),
        created_at: now,
    };

    state.agents.write().await.insert(id.clone(), session);

    Ok((
        StatusCode::CREATED,
        Json(SessionResponse {
            id,
            model: req.model,
            workspace,
            created_at: now.to_rfc3339(),
        }),
    ))
}

/// Get session details
#[utoipa::path(get, path = "/sessions/:id", tag = "sessions",
    params(("id" = String, Path, description = "Session ID")),
    responses(
        (status = 200, body = SessionResponse),
        (status = 404, body = ErrorResponse),
    )
)]
async fn get_session(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionResponse>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&headers, &state)?;
    let agents = state.agents.read().await;
    let session = agents.get(&id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Session '{id}' not found"),
            }),
        )
    })?;
    Ok(Json(SessionResponse {
        id,
        model: session.model.clone(),
        workspace: session.workspace.clone(),
        created_at: session.created_at.to_rfc3339(),
    }))
}

/// Stream agent events via Server-Sent Events (SSE)
#[utoipa::path(post, path = "/sessions/:id/stream", tag = "agent",
    params(("id" = String, Path, description = "Session ID")),
    request_body = StreamRequest,
    responses(
        (status = 200, description = "SSE event stream"),
        (status = 404, body = ErrorResponse),
    )
)]
async fn stream_message(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<StreamRequest>,
) -> Result<Sse<Pin<Box<dyn Stream<Item = Result<SseEvent, Infallible>> + Send>>>, (StatusCode, Json<ErrorResponse>)>
{
    check_auth(&headers, &state)?;

    let agents = state.agents.read().await;
    let session = agents.get(&id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Session '{id}' not found"),
            }),
        )
    })?;

    let (rx, _handle) = session.agent.stream(&req.prompt).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to start stream: {e}"),
            }),
        )
    })?;

    let stream = async_stream::stream! {
        let mut rx = rx;
        while let Some(event) = rx.recv().await {
            let is_end = matches!(event, AgentEvent::End { .. });
            let is_error = matches!(event, AgentEvent::Error { .. });

            let json = serde_json::to_string(&event).unwrap_or_default();
            let event_type = match &event {
                AgentEvent::Start { .. } => "start",
                AgentEvent::TextDelta { .. } => "text_delta",
                AgentEvent::TurnStart { .. } => "turn_start",
                AgentEvent::TurnEnd { .. } => "turn_end",
                AgentEvent::ToolStart { .. } => "tool_start",
                AgentEvent::ToolEnd { .. } => "tool_end",
                AgentEvent::End { .. } => "end",
                AgentEvent::Error { .. } => "error",
                _ => "event",
            };

            yield Ok(SseEvent::default().event(event_type).data(json));

            if is_end || is_error {
                break;
            }
        }
    };

    Ok(Sse::new(Box::pin(stream) as Pin<Box<dyn Stream<Item = Result<SseEvent, Infallible>> + Send>>)
        .keep_alive(KeepAlive::default()))
}
/// Delete a session
#[utoipa::path(delete, path = "/sessions/:id", tag = "sessions",
    params(("id" = String, Path, description = "Session ID")),
    responses(
        (status = 204, description = "Session deleted"),
        (status = 404, body = ErrorResponse),
    )
)]
async fn delete_session(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&headers, &state)?;
    let removed = state.agents.write().await.remove(&id);
    match removed {
        Some(_) => Ok(StatusCode::NO_CONTENT),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Session '{id}' not found"),
            }),
        )),
    }
}

/// Send a prompt and wait for the complete response
#[utoipa::path(post, path = "/sessions/:id/send", tag = "agent",
    params(("id" = String, Path, description = "Session ID")),
    request_body = SendRequest,
    responses(
        (status = 200, body = SendResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
async fn send_message(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendRequest>,
) -> Result<Json<SendResponse>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&headers, &state)?;

    let agents = state.agents.read().await;
    let session = agents.get(&id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Session '{id}' not found"),
            }),
        )
    })?;

    let result = session.agent.send(&req.prompt).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Agent execution failed: {e}"),
            }),
        )
    })?;

    Ok(Json(SendResponse {
        text: result.text,
        tool_calls_count: result.tool_calls_count,
        prompt_tokens: result.usage.prompt_tokens,
        completion_tokens: result.usage.completion_tokens,
        total_tokens: result.usage.total_tokens,
    }))
}

/// Execute a tool directly, bypassing the LLM
#[utoipa::path(post, path = "/sessions/:id/tool", tag = "tools",
    params(("id" = String, Path, description = "Session ID")),
    request_body = ToolExecRequest,
    responses(
        (status = 200, body = ToolExecResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
async fn execute_tool(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ToolExecRequest>,
) -> Result<Json<ToolExecResponse>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&headers, &state)?;

    let agents = state.agents.read().await;
    let session = agents.get(&id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Session '{id}' not found"),
            }),
        )
    })?;

    let result = session.agent.tool(&req.name, req.args).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Tool execution failed: {e}"),
            }),
        )
    })?;

    Ok(Json(ToolExecResponse {
        name: result.name,
        output: result.output,
        exit_code: result.exit_code,
    }))
}

/// OpenAI-compatible chat completions endpoint
#[utoipa::path(post, path = "/v1/chat/completions", tag = "openai",
    request_body = ChatCompletionRequest,
    responses(
        (status = 200, body = ChatCompletionResponse),
        (status = 400, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
async fn chat_completions(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Json<ChatCompletionResponse>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&headers, &state)?;

    // Extract API key from request body or Authorization header
    let api_key = req
        .api_key
        .clone()
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(|s| s.to_string())
        })
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "api_key required in body or Authorization header".into(),
                }),
            )
        })?;

    // Extract the last user message as the prompt
    let prompt = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "At least one user message is required".into(),
                }),
            )
        })?;

    // Extract system prompt from messages
    let system_prompt = req
        .messages
        .iter()
        .find(|m| m.role == "system")
        .map(|m| m.content.clone());

    // Build a temporary agent
    let agent = build_agent(
        &req.model,
        &api_key,
        req.workspace.as_deref(),
        system_prompt.as_deref(),
        None,
        None,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Failed to create agent: {e}"),
            }),
        )
    })?;

    let result = agent.send(&prompt).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Agent execution failed: {e}"),
            }),
        )
    })?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(Json(ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".into(),
        created: now,
        model: req.model,
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".into(),
                content: result.text,
            },
            finish_reason: "stop".into(),
        }],
        usage: ChatUsage {
            prompt_tokens: result.usage.prompt_tokens,
            completion_tokens: result.usage.completion_tokens,
            total_tokens: result.usage.total_tokens,
        },
    }))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState {
            agents: Arc::new(RwLock::new(HashMap::new())),
            config: Arc::new(RwLock::new(CodeConfig::default())),
            api_token: None,
        }
    }

    fn test_state_with_auth() -> AppState {
        AppState {
            agents: Arc::new(RwLock::new(HashMap::new())),
            config: Arc::new(RwLock::new(CodeConfig::default())),
            api_token: Some("test-token".into()),
        }
    }

    #[tokio::test]
    async fn test_health() {
        let app = router(test_state());
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_list_sessions_empty() {
        let app = router(test_state());
        let req = Request::builder()
            .uri("/sessions")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let sessions: Vec<SessionResponse> = serde_json::from_slice(&body).unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_get_session_not_found() {
        let app = router(test_state());
        let req = Request::builder()
            .uri("/sessions/nonexistent")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_session_not_found() {
        let app = router(test_state());
        let req = Request::builder()
            .method("DELETE")
            .uri("/sessions/nonexistent")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_auth_required() {
        let app = router(test_state_with_auth());
        let req = Request::builder()
            .uri("/sessions")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_valid_token() {
        let app = router(test_state_with_auth());
        let req = Request::builder()
            .uri("/sessions")
            .header("Authorization", "Bearer test-token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_invalid_token() {
        let app = router(test_state_with_auth());
        let req = Request::builder()
            .uri("/sessions")
            .header("Authorization", "Bearer wrong-token")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_create_session() {
        let app = router(test_state());
        let body = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "api_key": "test-key",
            "workspace": "/tmp/test"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let session: SessionResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(session.model, "claude-sonnet-4-20250514");
        assert_eq!(session.workspace, "/tmp/test");
        assert!(!session.id.is_empty());
    }

    #[tokio::test]
    async fn test_create_and_get_session() {
        let state = test_state();
        let app = router(state.clone());

        // Create
        let body = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "api_key": "test-key",
            "workspace": "/tmp/test"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: SessionResponse = serde_json::from_slice(&body).unwrap();

        // Get
        let app2 = router(state.clone());
        let req = Request::builder()
            .uri(&format!("/sessions/{}", created.id))
            .body(Body::empty())
            .unwrap();
        let resp = app2.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_and_delete_session() {
        let state = test_state();
        let app = router(state.clone());

        // Create
        let body = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "api_key": "test-key"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: SessionResponse = serde_json::from_slice(&body).unwrap();

        // Delete
        let app2 = router(state.clone());
        let req = Request::builder()
            .method("DELETE")
            .uri(&format!("/sessions/{}", created.id))
            .body(Body::empty())
            .unwrap();
        let resp = app2.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify gone
        let app3 = router(state);
        let req = Request::builder()
            .uri(&format!("/sessions/{}", created.id))
            .body(Body::empty())
            .unwrap();
        let resp = app3.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_swagger_ui() {
        let app = router(test_state());
        let req = Request::builder()
            .uri("/docs/")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::MOVED_PERMANENTLY,
            "Swagger UI should be accessible"
        );
    }

    #[tokio::test]
    async fn test_openapi_spec() {
        let app = router(test_state());
        let req = Request::builder()
            .uri("/api-doc/openapi.json")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(spec.get("openapi").is_some());
        assert!(spec.get("paths").is_some());
    }
}
