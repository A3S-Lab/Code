//! LLM client implementations
//!
//! Provides unified interface for multiple LLM providers:
//! - Anthropic Claude (Messages API)
//! - OpenAI GPT (Chat Completions API)
//!
//! Features:
//! - Tool calling support
//! - Token usage tracking
//! - Automatic retry with exponential backoff for transient errors

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::Instrument;

use crate::retry::{AttemptOutcome, RetryConfig};

// ============================================================================
// Public Types
// ============================================================================

/// Tool definition for LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value, // JSON Schema
}

/// Message content types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: Option<bool>,
    },
}

/// Message in conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    pub fn tool_result(tool_use_id: &str, content: &str, is_error: bool) -> Self {
        Self {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error: Some(is_error),
            }],
        }
    }

    /// Extract text content from message
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::Text { text } = block {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Extract tool calls from message
    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    Some(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        args: input.clone(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

/// LLM response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub message: Message,
    pub usage: TokenUsage,
    pub stop_reason: Option<String>,
}

impl LlmResponse {
    /// Get text content
    pub fn text(&self) -> String {
        self.message.text()
    }

    /// Get tool calls
    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.message.tool_calls()
    }
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    pub cache_read_tokens: Option<usize>,
    pub cache_write_tokens: Option<usize>,
}

/// Tool call from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: serde_json::Value,
}

/// Streaming event from LLM
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Text content delta
    TextDelta(String),
    /// Tool use started (id, name)
    ToolUseStart { id: String, name: String },
    /// Tool use input delta (for the current tool)
    /// Note: Currently not forwarded to clients, but kept for future use
    #[allow(dead_code)]
    ToolUseInputDelta(String),
    /// Response complete
    Done(LlmResponse),
}

/// LLM client trait
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Complete a conversation (non-streaming)
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse>;

    /// Complete a conversation with streaming
    /// Returns a receiver for streaming events
    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<mpsc::Receiver<StreamEvent>>;
}

// ============================================================================
// HTTP Utilities
// ============================================================================

/// Normalize base URL by stripping trailing /v1
fn normalize_base_url(base_url: &str) -> String {
    base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .trim_end_matches('/')
        .to_string()
}

/// Make HTTP POST request with JSON body
async fn http_post_json(
    client: &reqwest::Client,
    url: &str,
    headers: Vec<(&str, &str)>,
    body: &serde_json::Value,
) -> Result<(reqwest::StatusCode, String)> {
    tracing::debug!(
        "HTTP POST to {}: {}",
        url,
        serde_json::to_string_pretty(body)?
    );

    let mut request = client.post(url);
    // Add custom headers first
    for (key, value) in headers {
        request = request.header(key, value);
    }
    // Set body as JSON (this will set Content-Type: application/json automatically)
    request = request.json(body);

    let response = request
        .send()
        .await
        .context(format!("Failed to send request to {}", url))?;

    let status = response.status();
    let body = response.text().await?;

    Ok((status, body))
}

// ============================================================================
// Anthropic Claude Client
// ============================================================================

/// Default max tokens for LLM responses
const DEFAULT_MAX_TOKENS: usize = 8192;

/// Anthropic Claude client
pub struct AnthropicClient {
    api_key: String,
    model: String,
    base_url: String,
    max_tokens: usize,
    client: reqwest::Client,
    retry_config: RetryConfig,
}

impl AnthropicClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            base_url: "https://api.anthropic.com".to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            client: reqwest::Client::new(),
            retry_config: RetryConfig::default(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = normalize_base_url(&base_url);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_retry_config(mut self, retry_config: RetryConfig) -> Self {
        self.retry_config = retry_config;
        self
    }

    fn build_request(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> serde_json::Value {
        let mut request = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": messages,
        });

        if let Some(sys) = system {
            request["system"] = serde_json::json!(sys);
        }

        if !tools.is_empty() {
            let tool_defs: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            request["tools"] = serde_json::json!(tool_defs);
        }

        request
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let span = tracing::info_span!(
            "a3s.llm.completion",
            "a3s.llm.provider" = "anthropic",
            "a3s.llm.model" = %self.model,
            "a3s.llm.streaming" = false,
            "a3s.llm.prompt_tokens" = tracing::field::Empty,
            "a3s.llm.completion_tokens" = tracing::field::Empty,
            "a3s.llm.total_tokens" = tracing::field::Empty,
            "a3s.llm.stop_reason" = tracing::field::Empty,
        );
        async {
        let request_body = self.build_request(messages, system, tools);
        let url = format!("{}/v1/messages", self.base_url);

        let headers = vec![
            ("x-api-key", self.api_key.as_str()),
            ("anthropic-version", "2023-06-01"),
        ];

        let (_status, body) = crate::retry::with_retry(&self.retry_config, |_attempt| {
            let client = &self.client;
            let url = &url;
            let headers = headers.clone();
            let request_body = &request_body;
            async move {
                match http_post_json(client, url, headers, request_body).await {
                    Ok((status, body)) => {
                        if status.is_success() {
                            AttemptOutcome::Success((status, body))
                        } else if self.retry_config.is_retryable_status(status) {
                            AttemptOutcome::Retryable {
                                status,
                                body,
                                retry_after: None,
                            }
                        } else {
                            AttemptOutcome::Fatal(anyhow::anyhow!(
                                "Anthropic API error at {} ({}): {}", url, status, body
                            ))
                        }
                    }
                    Err(e) => AttemptOutcome::Fatal(e),
                }
            }
        })
        .await?;

        // Success path - body is already validated as success status

        let response: AnthropicResponse =
            serde_json::from_str(&body).context("Failed to parse Anthropic response")?;

        tracing::debug!("Anthropic response: {:?}", response);

        // Convert to our format
        let content: Vec<ContentBlock> = response
            .content
            .into_iter()
            .map(|block| match block {
                AnthropicContentBlock::Text { text } => ContentBlock::Text { text },
                AnthropicContentBlock::ToolUse { id, name, input } => {
                    ContentBlock::ToolUse { id, name, input }
                }
            })
            .collect();

        let llm_response = LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content,
            },
            usage: TokenUsage {
                prompt_tokens: response.usage.input_tokens,
                completion_tokens: response.usage.output_tokens,
                total_tokens: response.usage.input_tokens + response.usage.output_tokens,
                cache_read_tokens: response.usage.cache_read_input_tokens,
                cache_write_tokens: response.usage.cache_creation_input_tokens,
            },
            stop_reason: Some(response.stop_reason),
        };

        crate::telemetry::record_llm_usage(
            llm_response.usage.prompt_tokens,
            llm_response.usage.completion_tokens,
            llm_response.usage.total_tokens,
            llm_response.stop_reason.as_deref(),
        );

        Ok(llm_response)
        }
        .instrument(span)
        .await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let span = tracing::info_span!(
            "a3s.llm.completion",
            "a3s.llm.provider" = "anthropic",
            "a3s.llm.model" = %self.model,
            "a3s.llm.streaming" = true,
            "a3s.llm.prompt_tokens" = tracing::field::Empty,
            "a3s.llm.completion_tokens" = tracing::field::Empty,
            "a3s.llm.total_tokens" = tracing::field::Empty,
            "a3s.llm.stop_reason" = tracing::field::Empty,
        );
        async {
        let mut request_body = self.build_request(messages, system, tools);
        request_body["stream"] = serde_json::json!(true);

        let url = format!("{}/v1/messages", self.base_url);

        let response = crate::retry::with_retry(&self.retry_config, |_attempt| {
            let client = &self.client;
            let url = &url;
            let api_key = &self.api_key;
            let request_body = &request_body;
            async move {
                match client
                    .post(url.as_str())
                    .header("x-api-key", api_key.as_str())
                    .header("anthropic-version", "2023-06-01")
                    .json(request_body)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            AttemptOutcome::Success(resp)
                        } else {
                            let status = resp.status();
                            let retry_after = RetryConfig::parse_retry_after(
                                resp.headers()
                                    .get("retry-after")
                                    .and_then(|v| v.to_str().ok()),
                            );
                            let body = resp.text().await.unwrap_or_default();
                            if self.retry_config.is_retryable_status(status) {
                                AttemptOutcome::Retryable {
                                    status,
                                    body,
                                    retry_after,
                                }
                            } else {
                                AttemptOutcome::Fatal(anyhow::anyhow!(
                                    "Anthropic API error at {} ({}): {}", url, status, body
                                ))
                            }
                        }
                    }
                    Err(e) => AttemptOutcome::Fatal(anyhow::anyhow!(
                        "Failed to send streaming request: {}", e
                    )),
                }
            }
        })
        .await?;

        let (tx, rx) = mpsc::channel(100);

        // Spawn task to process SSE stream
        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut content_blocks: Vec<ContentBlock> = Vec::new();
            let mut current_tool_id = String::new();
            let mut current_tool_name = String::new();
            let mut current_tool_input = String::new();
            let mut usage = TokenUsage::default();
            let mut stop_reason = None;

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Stream error: {}", e);
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete SSE events
                while let Some(event_end) = buffer.find("\n\n") {
                    let event_data = buffer[..event_end].to_string();
                    buffer = buffer[event_end + 2..].to_string();

                    // Parse SSE event
                    for line in event_data.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                continue;
                            }

                            if let Ok(event) = serde_json::from_str::<AnthropicStreamEvent>(data) {
                                match event {
                                    AnthropicStreamEvent::ContentBlockStart {
                                        index: _,
                                        content_block,
                                    } => match content_block {
                                        AnthropicContentBlock::Text { .. } => {}
                                        AnthropicContentBlock::ToolUse { id, name, .. } => {
                                            current_tool_id = id.clone();
                                            current_tool_name = name.clone();
                                            current_tool_input.clear();
                                            let _ = tx
                                                .send(StreamEvent::ToolUseStart { id, name })
                                                .await;
                                        }
                                    },
                                    AnthropicStreamEvent::ContentBlockDelta { index: _, delta } => {
                                        match delta {
                                            AnthropicDelta::TextDelta { text } => {
                                                let _ = tx.send(StreamEvent::TextDelta(text)).await;
                                            }
                                            AnthropicDelta::InputJsonDelta { partial_json } => {
                                                current_tool_input.push_str(&partial_json);
                                                let _ = tx
                                                    .send(StreamEvent::ToolUseInputDelta(
                                                        partial_json,
                                                    ))
                                                    .await;
                                            }
                                        }
                                    }
                                    AnthropicStreamEvent::ContentBlockStop { index: _ } => {
                                        // If we were building a tool use, finalize it
                                        if !current_tool_id.is_empty() {
                                            let input: serde_json::Value =
                                                serde_json::from_str(&current_tool_input)
                                                    .unwrap_or_else(|e| {
                                                        tracing::warn!(
                                                            "Failed to parse tool input JSON for tool '{}': {}",
                                                            current_tool_name, e
                                                        );
                                                        serde_json::json!({})
                                                    });
                                            content_blocks.push(ContentBlock::ToolUse {
                                                id: current_tool_id.clone(),
                                                name: current_tool_name.clone(),
                                                input,
                                            });
                                            current_tool_id.clear();
                                            current_tool_name.clear();
                                            current_tool_input.clear();
                                        }
                                    }
                                    AnthropicStreamEvent::MessageStart { message } => {
                                        usage.prompt_tokens = message.usage.input_tokens;
                                    }
                                    AnthropicStreamEvent::MessageDelta {
                                        delta,
                                        usage: msg_usage,
                                    } => {
                                        stop_reason = Some(delta.stop_reason);
                                        usage.completion_tokens = msg_usage.output_tokens;
                                        usage.total_tokens =
                                            usage.prompt_tokens + usage.completion_tokens;
                                    }
                                    AnthropicStreamEvent::MessageStop => {
                                        // Record telemetry for streaming completion
                                        crate::telemetry::record_llm_usage(
                                            usage.prompt_tokens,
                                            usage.completion_tokens,
                                            usage.total_tokens,
                                            stop_reason.as_deref(),
                                        );

                                        // Build final response
                                        let response = LlmResponse {
                                            message: Message {
                                                role: "assistant".to_string(),
                                                content: content_blocks.clone(),
                                            },
                                            usage: usage.clone(),
                                            stop_reason: stop_reason.clone(),
                                        };
                                        let _ = tx.send(StreamEvent::Done(response)).await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
        }
        .instrument(span)
        .await
    }
}

// Anthropic API response types
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    stop_reason: String,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: usize,
    output_tokens: usize,
    cache_read_input_tokens: Option<usize>,
    cache_creation_input_tokens: Option<usize>,
}

// Anthropic streaming event types
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)] // API response fields may not all be used
enum AnthropicStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: AnthropicMessageStart },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: AnthropicContentBlock,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: AnthropicDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: AnthropicMessageDeltaData,
        usage: AnthropicOutputUsage,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: AnthropicError },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDeltaData {
    stop_reason: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicOutputUsage {
    output_tokens: usize,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // API response fields may not all be used
struct AnthropicError {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

// ============================================================================
// OpenAI Client
// ============================================================================

/// OpenAI client
pub struct OpenAiClient {
    api_key: String,
    model: String,
    base_url: String,
    client: reqwest::Client,
    retry_config: RetryConfig,
}

impl OpenAiClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            base_url: "https://api.openai.com".to_string(),
            client: reqwest::Client::new(),
            retry_config: RetryConfig::default(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = normalize_base_url(&base_url);
        self
    }

    pub fn with_retry_config(mut self, retry_config: RetryConfig) -> Self {
        self.retry_config = retry_config;
        self
    }

    fn convert_messages(&self, messages: &[Message]) -> Vec<serde_json::Value> {
        messages
            .iter()
            .map(|msg| {
                let content: serde_json::Value = if msg.content.len() == 1 {
                    match &msg.content[0] {
                        ContentBlock::Text { text } => serde_json::json!(text),
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            return serde_json::json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content,
                            });
                        }
                        _ => serde_json::json!(""),
                    }
                } else {
                    serde_json::json!(msg
                        .content
                        .iter()
                        .map(|block| {
                            match block {
                                ContentBlock::Text { text } => serde_json::json!({
                                    "type": "text",
                                    "text": text,
                                }),
                                ContentBlock::ToolUse { id, name, input } => serde_json::json!({
                                    "type": "function",
                                    "id": id,
                                    "function": {
                                        "name": name,
                                        "arguments": input.to_string(),
                                    }
                                }),
                                _ => serde_json::json!({}),
                            }
                        })
                        .collect::<Vec<_>>())
                };

                // Handle assistant messages with tool calls
                if msg.role == "assistant" {
                    let tool_calls: Vec<_> = msg.tool_calls();
                    if !tool_calls.is_empty() {
                        return serde_json::json!({
                            "role": "assistant",
                            "content": msg.text(),
                            "tool_calls": tool_calls.iter().map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.args.to_string(),
                                    }
                                })
                            }).collect::<Vec<_>>(),
                        });
                    }
                }

                serde_json::json!({
                    "role": msg.role,
                    "content": content,
                })
            })
            .collect()
    }

    fn convert_tools(&self, tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
        tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect()
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let span = tracing::info_span!(
            "a3s.llm.completion",
            "a3s.llm.provider" = "openai",
            "a3s.llm.model" = %self.model,
            "a3s.llm.streaming" = false,
            "a3s.llm.prompt_tokens" = tracing::field::Empty,
            "a3s.llm.completion_tokens" = tracing::field::Empty,
            "a3s.llm.total_tokens" = tracing::field::Empty,
            "a3s.llm.stop_reason" = tracing::field::Empty,
        );
        async {
        let mut openai_messages = Vec::new();

        // Add system message
        if let Some(sys) = system {
            openai_messages.push(serde_json::json!({
                "role": "system",
                "content": sys,
            }));
        }

        // Add conversation messages
        openai_messages.extend(self.convert_messages(messages));

        let mut request = serde_json::json!({
            "model": self.model,
            "messages": openai_messages,
        });

        if !tools.is_empty() {
            request["tools"] = serde_json::json!(self.convert_tools(tools));
        }

        let url = format!("{}/v1/chat/completions", self.base_url);
        let auth_header = format!("Bearer {}", self.api_key);
        let headers = vec![("Authorization", auth_header.as_str())];

        let (_status, body) = crate::retry::with_retry(&self.retry_config, |_attempt| {
            let client = &self.client;
            let url = &url;
            let headers = headers.clone();
            let request = &request;
            async move {
                match http_post_json(client, url, headers, request).await {
                    Ok((status, body)) => {
                        if status.is_success() {
                            AttemptOutcome::Success((status, body))
                        } else if self.retry_config.is_retryable_status(status) {
                            AttemptOutcome::Retryable {
                                status,
                                body,
                                retry_after: None,
                            }
                        } else {
                            AttemptOutcome::Fatal(anyhow::anyhow!(
                                "OpenAI API error at {} ({}): {}", url, status, body
                            ))
                        }
                    }
                    Err(e) => AttemptOutcome::Fatal(e),
                }
            }
        })
        .await?;

        // Success path - body is already validated as success status

        let response: OpenAiResponse =
            serde_json::from_str(&body).context("Failed to parse OpenAI response")?;

        let choice = response.choices.into_iter().next().context("No choices")?;

        // Convert to our format
        let mut content = vec![];

        if let Some(text) = choice.message.content {
            if !text.is_empty() {
                content.push(ContentBlock::Text { text });
            }
        }

        if let Some(tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                content.push(ContentBlock::ToolUse {
                    id: tc.id,
                    name: tc.function.name.clone(),
                    input: serde_json::from_str(&tc.function.arguments).unwrap_or_else(|e| {
                        tracing::warn!(
                            "Failed to parse tool arguments JSON for tool '{}': {}",
                            tc.function.name,
                            e
                        );
                        serde_json::Value::default()
                    }),
                });
            }
        }

        let llm_response = LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content,
            },
            usage: TokenUsage {
                prompt_tokens: response.usage.prompt_tokens,
                completion_tokens: response.usage.completion_tokens,
                total_tokens: response.usage.total_tokens,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: choice.finish_reason,
        };

        crate::telemetry::record_llm_usage(
            llm_response.usage.prompt_tokens,
            llm_response.usage.completion_tokens,
            llm_response.usage.total_tokens,
            llm_response.stop_reason.as_deref(),
        );

        Ok(llm_response)
        }
        .instrument(span)
        .await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let span = tracing::info_span!(
            "a3s.llm.completion",
            "a3s.llm.provider" = "openai",
            "a3s.llm.model" = %self.model,
            "a3s.llm.streaming" = true,
            "a3s.llm.prompt_tokens" = tracing::field::Empty,
            "a3s.llm.completion_tokens" = tracing::field::Empty,
            "a3s.llm.total_tokens" = tracing::field::Empty,
            "a3s.llm.stop_reason" = tracing::field::Empty,
        );
        async {
        let mut openai_messages = Vec::new();

        if let Some(sys) = system {
            openai_messages.push(serde_json::json!({
                "role": "system",
                "content": sys,
            }));
        }

        openai_messages.extend(self.convert_messages(messages));

        let mut request = serde_json::json!({
            "model": self.model,
            "messages": openai_messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });

        if !tools.is_empty() {
            request["tools"] = serde_json::json!(self.convert_tools(tools));
        }

        let url = format!("{}/v1/chat/completions", self.base_url);

        let response = crate::retry::with_retry(&self.retry_config, |_attempt| {
            let client = &self.client;
            let url = &url;
            let api_key = &self.api_key;
            let request = &request;
            async move {
                match client
                    .post(url.as_str())
                    .header("Authorization", format!("Bearer {}", api_key))
                    .json(request)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            AttemptOutcome::Success(resp)
                        } else {
                            let status = resp.status();
                            let retry_after = RetryConfig::parse_retry_after(
                                resp.headers()
                                    .get("retry-after")
                                    .and_then(|v| v.to_str().ok()),
                            );
                            let body = resp.text().await.unwrap_or_default();
                            if self.retry_config.is_retryable_status(status) {
                                AttemptOutcome::Retryable {
                                    status,
                                    body,
                                    retry_after,
                                }
                            } else {
                                AttemptOutcome::Fatal(anyhow::anyhow!(
                                    "OpenAI API error at {} ({}): {}", url, status, body
                                ))
                            }
                        }
                    }
                    Err(e) => AttemptOutcome::Fatal(anyhow::anyhow!(
                        "Failed to send streaming request: {}", e
                    )),
                }
            }
        })
        .await?;

        let (tx, rx) = mpsc::channel(100);

        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut content_blocks: Vec<ContentBlock> = Vec::new();
            let mut text_content = String::new();
            let mut tool_calls: std::collections::BTreeMap<usize, (String, String, String)> =
                std::collections::BTreeMap::new();
            let mut usage = TokenUsage::default();
            let mut finish_reason = None;

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Stream error: {}", e);
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete SSE events
                while let Some(event_end) = buffer.find("\n\n") {
                    let event_data = buffer[..event_end].to_string();
                    buffer = buffer[event_end + 2..].to_string();

                    for line in event_data.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                // Build final response
                                if !text_content.is_empty() {
                                    content_blocks.push(ContentBlock::Text {
                                        text: text_content.clone(),
                                    });
                                }
                                for (_, (id, name, args)) in tool_calls.iter() {
                                    content_blocks.push(ContentBlock::ToolUse {
                                        id: id.clone(),
                                        name: name.clone(),
                                        input: serde_json::from_str(args).unwrap_or_else(|e| {
                                            tracing::warn!(
                                                "Failed to parse tool arguments JSON for tool '{}': {}",
                                                name, e
                                            );
                                            serde_json::Value::default()
                                        }),
                                    });
                                }
                                tool_calls.clear();
                                // Record telemetry for streaming completion
                                crate::telemetry::record_llm_usage(
                                    usage.prompt_tokens,
                                    usage.completion_tokens,
                                    usage.total_tokens,
                                    finish_reason.as_deref(),
                                );
                                let response = LlmResponse {
                                    message: Message {
                                        role: "assistant".to_string(),
                                        content: content_blocks.clone(),
                                    },
                                    usage: usage.clone(),
                                    stop_reason: finish_reason.clone(),
                                };
                                let _ = tx.send(StreamEvent::Done(response)).await;
                                continue;
                            }

                            if let Ok(event) = serde_json::from_str::<OpenAiStreamChunk>(data) {
                                // Handle usage in final chunk
                                if let Some(u) = event.usage {
                                    usage.prompt_tokens = u.prompt_tokens;
                                    usage.completion_tokens = u.completion_tokens;
                                    usage.total_tokens = u.total_tokens;
                                }

                                if let Some(choice) = event.choices.into_iter().next() {
                                    if let Some(reason) = choice.finish_reason {
                                        finish_reason = Some(reason);
                                    }

                                    if let Some(delta) = choice.delta {
                                        // Handle text content
                                        if let Some(content) = delta.content {
                                            text_content.push_str(&content);
                                            let _ = tx.send(StreamEvent::TextDelta(content)).await;
                                        }

                                        // Handle tool calls
                                        if let Some(tcs) = delta.tool_calls {
                                            for tc in tcs {
                                                let entry = tool_calls
                                                    .entry(tc.index)
                                                    .or_insert_with(|| {
                                                        (
                                                            String::new(),
                                                            String::new(),
                                                            String::new(),
                                                        )
                                                    });

                                                if let Some(id) = tc.id {
                                                    entry.0 = id;
                                                }
                                                if let Some(func) = tc.function {
                                                    if let Some(name) = func.name {
                                                        entry.1 = name.clone();
                                                        let _ = tx
                                                            .send(StreamEvent::ToolUseStart {
                                                                id: entry.0.clone(),
                                                                name,
                                                            })
                                                            .await;
                                                    }
                                                    if let Some(args) = func.arguments {
                                                        entry.2.push_str(&args);
                                                        let _ = tx
                                                            .send(StreamEvent::ToolUseInputDelta(
                                                                args,
                                                            ))
                                                            .await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok(rx)
        }
        .instrument(span)
        .await
    }
}

// OpenAI API response types
#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    usage: OpenAiUsage,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    id: String,
    function: OpenAiFunction,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
}

// OpenAI streaming types
#[derive(Debug, Deserialize)]
struct OpenAiStreamChunk {
    choices: Vec<OpenAiStreamChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    delta: Option<OpenAiDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

// ============================================================================
// Factory
// ============================================================================

/// LLM client configuration
#[derive(Debug, Clone, Default)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub retry_config: Option<RetryConfig>,
}

impl LlmConfig {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            api_key: api_key.into(),
            base_url: None,
            retry_config: None,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    pub fn with_retry_config(mut self, retry_config: RetryConfig) -> Self {
        self.retry_config = Some(retry_config);
        self
    }
}

/// Create LLM client with full configuration (supports custom base_url)
pub fn create_client_with_config(config: LlmConfig) -> Arc<dyn LlmClient> {
    let retry = config.retry_config.unwrap_or_default();

    match config.provider.as_str() {
        "anthropic" | "claude" => {
            let mut client = AnthropicClient::new(config.api_key, config.model)
                .with_retry_config(retry);
            if let Some(base_url) = config.base_url {
                client = client.with_base_url(base_url);
            }
            Arc::new(client)
        }
        "openai" | "gpt" => {
            let mut client = OpenAiClient::new(config.api_key, config.model)
                .with_retry_config(retry);
            if let Some(base_url) = config.base_url {
                client = client.with_base_url(base_url);
            }
            Arc::new(client)
        }
        // OpenAI-compatible providers (deepseek, groq, together, ollama, etc.)
        _ => {
            tracing::info!(
                "Using OpenAI-compatible client for provider '{}'",
                config.provider
            );
            let mut client = OpenAiClient::new(config.api_key, config.model)
                .with_retry_config(retry);
            if let Some(base_url) = config.base_url {
                client = client.with_base_url(base_url);
            }
            Arc::new(client)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation() {
        let msg = Message::user("Hello");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.text(), "Hello");
    }

    #[test]
    fn test_normalize_base_url() {
        assert_eq!(
            normalize_base_url("https://api.example.com"),
            "https://api.example.com"
        );
        assert_eq!(
            normalize_base_url("https://api.example.com/"),
            "https://api.example.com"
        );
        assert_eq!(
            normalize_base_url("https://api.example.com/v1"),
            "https://api.example.com"
        );
        assert_eq!(
            normalize_base_url("https://api.example.com/v1/"),
            "https://api.example.com"
        );
    }

    // ========================================================================
    // Integration Tests (require real LLM API)
    // ========================================================================
    // These tests are ignored by default. Run with:
    //   cargo test -p a3s-code --lib test_real_llm -- --ignored
    //
    // Requires config.json in workspace root with:
    // {
    //   "llm": {
    //     "api_base": "http://...",
    //     "api_key": "sk-...",
    //     "model": "..."
    //   }
    // }

    fn load_test_config() -> Option<(String, String, String)> {
        let config_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .parent()?
            .join("config.json");

        let content = std::fs::read_to_string(&config_path).ok()?;
        let config: serde_json::Value = serde_json::from_str(&content).ok()?;

        // Try new format first (providers array)
        if let Some(providers) = config.get("providers").and_then(|p| p.as_array()) {
            // Look for openai provider with kimi-k2.5 model
            for provider in providers {
                if let Some(models) = provider.get("models").and_then(|m| m.as_array()) {
                    for model in models {
                        if model.get("id")?.as_str()? == "kimi-k2.5" {
                            let api_base = model.get("baseUrl")?.as_str()?.to_string();
                            let api_key = model.get("apiKey")?.as_str()?.to_string();
                            let model_id = model.get("id")?.as_str()?.to_string();
                            return Some((api_base, api_key, model_id));
                        }
                    }
                }
            }

            // Fallback: use first provider's first model
            if let Some(provider) = providers.first() {
                let api_base = provider.get("baseUrl")?.as_str()?.to_string();
                let api_key = provider.get("apiKey")?.as_str()?.to_string();
                let models = provider.get("models")?.as_array()?;
                let model_id = models.first()?.get("id")?.as_str()?.to_string();
                return Some((api_base, api_key, model_id));
            }
        }

        // Try old format (llm object)
        if let Some(llm) = config.get("llm") {
            let api_base = llm.get("api_base")?.as_str()?.to_string();
            let api_key = llm.get("api_key")?.as_str()?.to_string();
            let model = llm.get("model")?.as_str()?.to_string();
            return Some((api_base, api_key, model));
        }

        None
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -p a3s-code --lib test_real_llm_openai_complete -- --ignored
    async fn test_real_llm_openai_complete() {
        let Some((api_base, api_key, model)) = load_test_config() else {
            eprintln!("Skipping test: config.json not found or invalid");
            return;
        };

        let client = OpenAiClient::new(api_key, model).with_base_url(api_base);

        let messages = vec![Message::user("Say 'Hello, World!' and nothing else.")];

        let response = client.complete(&messages, None, &[]).await;
        assert!(response.is_ok(), "LLM call failed: {:?}", response.err());

        let response = response.unwrap();
        let text = response.text().to_lowercase();
        assert!(
            text.contains("hello") && text.contains("world"),
            "Unexpected response: {}",
            text
        );

        println!("Response: {}", response.text());
        println!("Usage: {:?}", response.usage);
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -p a3s-code --lib test_real_llm_openai_streaming -- --ignored
    async fn test_real_llm_openai_streaming() {
        let Some((api_base, api_key, model)) = load_test_config() else {
            eprintln!("Skipping test: config.json not found or invalid");
            return;
        };

        let client = OpenAiClient::new(api_key, model).with_base_url(api_base);

        let messages = vec![Message::user("Count from 1 to 5, one number per line.")];

        let result = client.complete_streaming(&messages, None, &[]).await;
        assert!(result.is_ok(), "Streaming call failed: {:?}", result.err());

        let mut rx = result.unwrap();
        let mut full_text = String::new();
        let mut event_count = 0;

        while let Some(event) = rx.recv().await {
            event_count += 1;
            match event {
                StreamEvent::TextDelta(delta) => {
                    full_text.push_str(&delta);
                    print!("{}", delta);
                }
                StreamEvent::Done(response) => {
                    println!("\n\nStreaming complete. Usage: {:?}", response.usage);
                }
                _ => {}
            }
        }

        assert!(event_count > 0, "No events received");
        assert!(full_text.contains("1"), "Response should contain '1'");
        println!("\nFull response: {}", full_text);
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -p a3s-code --lib test_real_llm_context_compaction -- --ignored
    async fn test_real_llm_context_compaction() {
        use crate::session::{Session, SessionConfig};

        let Some((api_base, api_key, model)) = load_test_config() else {
            eprintln!("Skipping test: config.json not found or invalid");
            return;
        };

        let client: std::sync::Arc<dyn LlmClient> =
            std::sync::Arc::new(OpenAiClient::new(api_key, model).with_base_url(api_base));

        let config = SessionConfig::default();
        let mut session = Session::new("test-compact".to_string(), config, vec![])
            .await
            .unwrap();

        // Add many messages to trigger compaction
        for i in 0..50 {
            session.messages.push(Message::user(&format!(
                "This is message number {}. The topic is about testing context compaction.",
                i
            )));
            session.messages.push(Message {
                role: "assistant".to_string(),
                content: vec![super::ContentBlock::Text {
                    text: format!("I acknowledge message {}.", i),
                }],
            });
        }

        println!("Before compaction: {} messages", session.messages.len());

        let result = session.compact(&client).await;
        assert!(result.is_ok(), "Compaction failed: {:?}", result.err());

        println!("After compaction: {} messages", session.messages.len());

        // Check that summary was created
        let has_summary = session
            .messages
            .iter()
            .any(|m| m.text().contains("[Context Summary:"));
        assert!(has_summary, "Summary message not found");

        // Print the summary
        for msg in &session.messages {
            if msg.text().contains("[Context Summary:") {
                println!("\nGenerated Summary:\n{}", msg.text());
                break;
            }
        }
    }

    // ========================================================================
    // Unit Tests for LLM types and telemetry integration
    // ========================================================================

    #[test]
    fn test_message_tool_result() {
        let msg = Message::tool_result("tool-123", "result data", false);
        assert_eq!(msg.role, "user");
        match &msg.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool-123");
                assert_eq!(content, "result data");
                assert_eq!(*is_error, Some(false));
            }
            _ => panic!("Expected ToolResult content block"),
        }
    }

    #[test]
    fn test_message_tool_result_error() {
        let msg = Message::tool_result("tool-456", "error msg", true);
        match &msg.content[0] {
            ContentBlock::ToolResult { is_error, .. } => {
                assert_eq!(*is_error, Some(true));
            }
            _ => panic!("Expected ToolResult content block"),
        }
    }

    #[test]
    fn test_message_text_multiple_blocks() {
        let msg = Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "Hello ".to_string(),
                },
                ContentBlock::Text {
                    text: "World".to_string(),
                },
            ],
        };
        assert_eq!(msg.text(), "Hello World");
    }

    #[test]
    fn test_message_text_with_tool_use() {
        let msg = Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "Let me run that.".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                },
            ],
        };
        assert_eq!(msg.text(), "Let me run that.");
    }

    #[test]
    fn test_message_tool_calls() {
        let msg = Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "Running tools".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                },
                ContentBlock::ToolUse {
                    id: "t2".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file": "test.rs"}),
                },
            ],
        };
        let calls = msg.tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "bash");
        assert_eq!(calls[1].name, "read");
        assert_eq!(calls[0].id, "t1");
    }

    #[test]
    fn test_message_no_tool_calls() {
        let msg = Message::user("Hello");
        assert!(msg.tool_calls().is_empty());
    }

    #[test]
    fn test_token_usage_default() {
        let usage = TokenUsage::default();
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
        assert!(usage.cache_read_tokens.is_none());
        assert!(usage.cache_write_tokens.is_none());
    }

    #[test]
    fn test_llm_response_text() {
        let response = LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Hello!".to_string(),
                }],
            },
            usage: TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("end_turn".to_string()),
        };
        assert_eq!(response.text(), "Hello!");
        assert!(response.tool_calls().is_empty());
        assert_eq!(response.usage.total_tokens, 15);
        assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn test_llm_response_with_tool_calls() {
        let response = LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "call-1".to_string(),
                    name: "grep".to_string(),
                    input: serde_json::json!({"pattern": "fn main"}),
                }],
            },
            usage: TokenUsage::default(),
            stop_reason: Some("tool_use".to_string()),
        };
        let calls = response.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "grep");
        assert_eq!(calls[0].args["pattern"], "fn main");
    }

    #[test]
    fn test_tool_definition_creation() {
        let def = ToolDefinition {
            name: "bash".to_string(),
            description: "Execute shell commands".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"]
            }),
        };
        assert_eq!(def.name, "bash");
        assert!(def.parameters["properties"]["command"].is_object());
    }

    #[test]
    fn test_content_block_serialization() {
        let text = ContentBlock::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&text).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"hello\""));

        let tool = ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({}),
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("\"type\":\"tool_use\""));
        assert!(json.contains("\"name\":\"bash\""));
    }

    #[test]
    fn test_anthropic_client_builder() {
        let client = AnthropicClient::new("sk-test".to_string(), "claude-sonnet".to_string());
        assert_eq!(client.model, "claude-sonnet");

        let client = client
            .with_base_url("https://custom.api.com".to_string())
            .with_max_tokens(2048);
        assert_eq!(client.base_url, "https://custom.api.com");
        assert_eq!(client.max_tokens, 2048);
    }

    #[test]
    fn test_openai_client_builder() {
        let client = OpenAiClient::new("sk-test".to_string(), "gpt-4o".to_string());
        assert_eq!(client.model, "gpt-4o");

        let client = client.with_base_url("https://custom.openai.com".to_string());
        assert_eq!(client.base_url, "https://custom.openai.com");
    }

    #[test]
    fn test_normalize_base_url_edge_cases() {
        assert_eq!(normalize_base_url("http://localhost:8080"), "http://localhost:8080");
        assert_eq!(normalize_base_url("http://localhost:8080/"), "http://localhost:8080");
        assert_eq!(normalize_base_url("http://localhost:8080/v1/"), "http://localhost:8080");
    }

    #[test]
    fn test_llm_config_creation() {
        let config = LlmConfig::new("anthropic", "claude-sonnet", "sk-key");
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.model, "claude-sonnet");
        assert_eq!(config.api_key, "sk-key");
    }
}

#[cfg(test)]
mod extra_llm_tests {
    use super::*;

    #[test]
    fn test_message_assistant_text() {
        let msg = Message { role: "assistant".into(), content: vec![ContentBlock::Text { text: "Hello".into() }] };
        assert_eq!(msg.text(), "Hello");
    }

    #[test]
    fn test_message_text_empty() {
        let msg = Message { role: "assistant".into(), content: vec![] };
        assert_eq!(msg.text(), "");
    }

    #[test]
    fn test_message_text_mixed() {
        let msg = Message { role: "assistant".into(), content: vec![
            ContentBlock::Text { text: "A ".into() },
            ContentBlock::ToolUse { id: "t1".into(), name: "bash".into(), input: serde_json::json!({}) },
            ContentBlock::Text { text: "B".into() },
        ]};
        assert_eq!(msg.text(), "A B");
    }

    #[test]
    fn test_message_tool_calls_extraction() {
        let msg = Message { role: "assistant".into(), content: vec![
            ContentBlock::Text { text: "help".into() },
            ContentBlock::ToolUse { id: "t1".into(), name: "bash".into(), input: serde_json::json!({"cmd":"ls"}) },
            ContentBlock::ToolUse { id: "t2".into(), name: "read".into(), input: serde_json::json!({"p":"/"}) },
        ]};
        let calls = msg.tool_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "bash");
        assert_eq!(calls[1].name, "read");
    }

    #[test]
    fn test_message_tool_calls_empty() {
        let msg = Message { role: "assistant".into(), content: vec![ContentBlock::Text { text: "no tools".into() }] };
        assert!(msg.tool_calls().is_empty());
    }

    #[test]
    fn test_message_tool_result_success() {
        let msg = Message::tool_result("t1", "output", false);
        assert_eq!(msg.role, "user");
        match &msg.content[0] {
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                assert_eq!(tool_use_id, "t1");
                assert_eq!(content, "output");
                assert_eq!(*is_error, Some(false));
            }
            _ => panic!("Expected ToolResult"),
        }
    }

    #[test]
    fn test_message_tool_result_error() {
        let msg = Message::tool_result("t1", "err", true);
        match &msg.content[0] {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(*is_error, Some(true)),
            _ => panic!("Expected ToolResult"),
        }
    }

    #[test]
    fn test_llm_response_text() {
        let r = LlmResponse { message: Message { role: "assistant".into(), content: vec![ContentBlock::Text { text: "resp".into() }] }, usage: TokenUsage::default(), stop_reason: None };
        assert_eq!(r.text(), "resp");
    }

    #[test]
    fn test_llm_response_tool_calls() {
        let r = LlmResponse { message: Message { role: "assistant".into(), content: vec![ContentBlock::ToolUse { id: "t1".into(), name: "bash".into(), input: serde_json::json!({}) }] }, usage: TokenUsage::default(), stop_reason: None };
        assert_eq!(r.tool_calls().len(), 1);
    }

    #[test]
    fn test_token_usage_default() {
        let u = TokenUsage::default();
        assert_eq!(u.prompt_tokens, 0);
        assert_eq!(u.completion_tokens, 0);
        assert_eq!(u.total_tokens, 0);
        assert_eq!(u.cache_read_tokens, None);
        assert_eq!(u.cache_write_tokens, None);
    }

    #[test]
    fn test_llm_config_new() {
        let c = LlmConfig::new("anthropic", "claude-3", "sk-test");
        assert_eq!(c.provider, "anthropic");
        assert_eq!(c.model, "claude-3");
        assert!(c.base_url.is_none());
    }

    #[test]
    fn test_llm_config_with_base_url() {
        let c = LlmConfig::new("openai", "gpt-4", "k").with_base_url("https://x.com");
        assert_eq!(c.base_url, Some("https://x.com".into()));
    }

    #[test]
    fn test_anthropic_client_new() {
        let c = AnthropicClient::new("key".into(), "claude-3".into());
        assert_eq!(c.model, "claude-3");
        assert_eq!(c.max_tokens, 8192);
    }

    #[test]
    fn test_anthropic_client_max_tokens() {
        let c = AnthropicClient::new("k".into(), "m".into()).with_max_tokens(4096);
        assert_eq!(c.max_tokens, 4096);
    }

    #[test]
    fn test_openai_client_new() {
        let c = OpenAiClient::new("key".into(), "gpt-4".into());
        assert_eq!(c.model, "gpt-4");
        assert_eq!(c.base_url, "https://api.openai.com");
    }

    #[test]
    fn test_normalize_strips_v1() {
        assert_eq!(normalize_base_url("https://api.com/v1"), "https://api.com");
    }

    #[test]
    fn test_normalize_strips_trailing_slash() {
        assert_eq!(normalize_base_url("https://api.com/"), "https://api.com");
    }

    #[test]
    fn test_normalize_no_change() {
        assert_eq!(normalize_base_url("https://api.com"), "https://api.com");
    }

    #[test]
    fn test_create_client_anthropic() {
        let _c = create_client_with_config(LlmConfig::new("anthropic", "claude-3", "k"));
    }

    #[test]
    fn test_create_client_openai() {
        let _c = create_client_with_config(LlmConfig::new("openai", "gpt-4", "k"));
    }

    #[test]
    fn test_create_client_unknown() {
        let _c = create_client_with_config(LlmConfig::new("unknown", "m", "k"));
    }

    #[test]
    fn test_anthropic_build_request_basic() {
        let c = AnthropicClient::new("k".into(), "claude-3".into());
        let b = c.build_request(&[Message::user("Hi")], None, &[]);
        assert_eq!(b["model"], "claude-3");
        assert!(b.get("system").is_none());
        assert!(b.get("tools").is_none());
    }

    #[test]
    fn test_anthropic_build_request_system() {
        let c = AnthropicClient::new("k".into(), "claude-3".into());
        let b = c.build_request(&[Message::user("Hi")], Some("Be helpful"), &[]);
        assert_eq!(b["system"], "Be helpful");
    }

    #[test]
    fn test_anthropic_build_request_tools() {
        let c = AnthropicClient::new("k".into(), "claude-3".into());
        let tools = vec![ToolDefinition { name: "bash".into(), description: "Run".into(), parameters: serde_json::json!({"type":"object"}) }];
        let b = c.build_request(&[Message::user("Hi")], None, &tools);
        assert_eq!(b["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_openai_convert_user_msg() {
        let c = OpenAiClient::new("k".into(), "gpt-4".into());
        let m = c.convert_messages(&[Message::user("Hello")]);
        assert_eq!(m[0]["role"], "user");
        assert_eq!(m[0]["content"], "Hello");
    }

    #[test]
    fn test_openai_convert_tool_result() {
        let c = OpenAiClient::new("k".into(), "gpt-4".into());
        let m = c.convert_messages(&[Message::tool_result("c1", "out", false)]);
        assert_eq!(m[0]["role"], "tool");
        assert_eq!(m[0]["tool_call_id"], "c1");
    }

    #[test]
    fn test_openai_convert_tools_empty() {
        let c = OpenAiClient::new("k".into(), "gpt-4".into());
        assert!(c.convert_tools(&[]).is_empty());
    }

    #[test]
    fn test_openai_convert_tools_single() {
        let c = OpenAiClient::new("k".into(), "gpt-4".into());
        let t = c.convert_tools(&[ToolDefinition { name: "read".into(), description: "Read".into(), parameters: serde_json::json!({"type":"object"}) }]);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0]["type"], "function");
    }

    #[test]
    fn test_anthropic_response_with_cache() {
        let j = r#"{"content":[{"type":"text","text":"Hi"}],"stop_reason":"end_turn","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":80,"cache_creation_input_tokens":20}}"#;
        let r: AnthropicResponse = serde_json::from_str(j).unwrap();
        assert_eq!(r.usage.cache_read_input_tokens, Some(80));
        assert_eq!(r.usage.cache_creation_input_tokens, Some(20));
    }

    #[test]
    fn test_anthropic_response_no_cache() {
        let j = r#"{"content":[{"type":"text","text":"Hi"}],"stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let r: AnthropicResponse = serde_json::from_str(j).unwrap();
        assert!(r.usage.cache_read_input_tokens.is_none());
    }

    #[test]
    fn test_anthropic_response_tool_use() {
        let j = r#"{"content":[{"type":"text","text":"ok"},{"type":"tool_use","id":"t1","name":"bash","input":{"cmd":"ls"}}],"stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":20}}"#;
        let r: AnthropicResponse = serde_json::from_str(j).unwrap();
        assert_eq!(r.content.len(), 2);
        assert_eq!(r.stop_reason, "tool_use");
    }

    #[test]
    fn test_openai_response_null_content() {
        let j = r#"{"id":"c1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let r: OpenAiResponse = serde_json::from_str(j).unwrap();
        assert!(r.choices[0].message.content.is_none());
    }

    #[test]
    fn test_openai_response_tool_calls() {
        let j = r#"{"id":"c1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"bash","arguments":"{\"cmd\":\"ls\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let r: OpenAiResponse = serde_json::from_str(j).unwrap();
        assert_eq!(r.choices[0].message.tool_calls.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_content_block_text_serialize() {
        let b = ContentBlock::Text { text: "hi".into() };
        let j = serde_json::to_value(&b).unwrap();
        assert_eq!(j["type"], "text");
    }

    #[test]
    fn test_content_block_tool_use_serialize() {
        let b = ContentBlock::ToolUse { id: "t1".into(), name: "bash".into(), input: serde_json::json!({}) };
        let j = serde_json::to_value(&b).unwrap();
        assert_eq!(j["type"], "tool_use");
    }

    #[test]
    fn test_content_block_tool_result_serialize() {
        let b = ContentBlock::ToolResult { tool_use_id: "t1".into(), content: "out".into(), is_error: Some(false) };
        let j = serde_json::to_value(&b).unwrap();
        assert_eq!(j["type"], "tool_result");
    }

    #[test]
    fn test_tool_definition() {
        let t = ToolDefinition { name: "grep".into(), description: "Search".into(), parameters: serde_json::json!({"type":"object"}) };
        assert_eq!(t.name, "grep");
    }
}

#[cfg(test)]
mod extra_llm_tests2 {
    use super::*;

    // ========================================================================
    // AnthropicClient build_request
    // ========================================================================

    #[test]
    fn test_anthropic_build_request_basic() {
        let client = AnthropicClient::new("key".to_string(), "claude-sonnet-4-20250514".to_string());
        let msgs = vec![Message::user("Hello")];
        let req = client.build_request(&msgs, None, &[]);

        assert_eq!(req["model"], "claude-sonnet-4-20250514");
        assert!(req["system"].is_null());
        assert!(req["tools"].is_null());
        assert!(req["messages"].is_array());
    }

    #[test]
    fn test_anthropic_build_request_with_system() {
        let client = AnthropicClient::new("key".to_string(), "claude-sonnet-4-20250514".to_string());
        let msgs = vec![Message::user("Hello")];
        let req = client.build_request(&msgs, Some("You are helpful"), &[]);

        assert_eq!(req["system"], "You are helpful");
    }

    #[test]
    fn test_anthropic_build_request_with_tools() {
        let client = AnthropicClient::new("key".to_string(), "claude-sonnet-4-20250514".to_string());
        let msgs = vec![Message::user("Hello")];
        let tools = vec![ToolDefinition {
            name: "bash".to_string(),
            description: "Run a command".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {"command": {"type": "string"}}}),
        }];
        let req = client.build_request(&msgs, None, &tools);

        assert!(req["tools"].is_array());
        assert_eq!(req["tools"][0]["name"], "bash");
        assert_eq!(req["tools"][0]["description"], "Run a command");
        assert!(req["tools"][0]["input_schema"].is_object());
    }

    #[test]
    fn test_anthropic_build_request_max_tokens() {
        let client = AnthropicClient::new("key".to_string(), "model".to_string())
            .with_max_tokens(4096);
        let req = client.build_request(&[], None, &[]);
        assert_eq!(req["max_tokens"], 4096);
    }

    // ========================================================================
    // OpenAiClient convert_messages
    // ========================================================================

    #[test]
    fn test_openai_convert_messages_simple_text() {
        let client = OpenAiClient::new("key".to_string(), "gpt-4".to_string());
        let msgs = vec![Message::user("Hello")];
        let converted = client.convert_messages(&msgs);

        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "user");
        assert_eq!(converted[0]["content"], "Hello");
    }

    #[test]
    fn test_openai_convert_messages_tool_result() {
        let client = OpenAiClient::new("key".to_string(), "gpt-4".to_string());
        let msgs = vec![Message::tool_result("call-1", "output data", false)];
        let converted = client.convert_messages(&msgs);

        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "tool");
        assert_eq!(converted[0]["tool_call_id"], "call-1");
        assert_eq!(converted[0]["content"], "output data");
    }

    #[test]
    fn test_openai_convert_messages_assistant_with_tool_calls() {
        let client = OpenAiClient::new("key".to_string(), "gpt-4".to_string());
        let msgs = vec![Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Text { text: "Let me help.".to_string() },
                ContentBlock::ToolUse {
                    id: "call-1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                },
            ],
        }];
        let converted = client.convert_messages(&msgs);

        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "assistant");
        assert!(converted[0]["tool_calls"].is_array());
        assert_eq!(converted[0]["tool_calls"][0]["function"]["name"], "bash");
    }

    #[test]
    fn test_openai_convert_messages_multi_block_text() {
        let client = OpenAiClient::new("key".to_string(), "gpt-4".to_string());
        let msgs = vec![Message {
            role: "user".to_string(),
            content: vec![
                ContentBlock::Text { text: "Part 1".to_string() },
                ContentBlock::Text { text: "Part 2".to_string() },
            ],
        }];
        let converted = client.convert_messages(&msgs);

        assert_eq!(converted.len(), 1);
        // Multi-block should be an array
        assert!(converted[0]["content"].is_array());
    }

    // ========================================================================
    // OpenAiClient convert_tools
    // ========================================================================

    #[test]
    fn test_openai_convert_tools() {
        let client = OpenAiClient::new("key".to_string(), "gpt-4".to_string());
        let tools = vec![
            ToolDefinition {
                name: "bash".to_string(),
                description: "Run command".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
            ToolDefinition {
                name: "read".to_string(),
                description: "Read file".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
        ];
        let converted = client.convert_tools(&tools);

        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["type"], "function");
        assert_eq!(converted[0]["function"]["name"], "bash");
        assert_eq!(converted[1]["function"]["name"], "read");
    }

    #[test]
    fn test_openai_convert_tools_empty() {
        let client = OpenAiClient::new("key".to_string(), "gpt-4".to_string());
        let converted = client.convert_tools(&[]);
        assert!(converted.is_empty());
    }

    // ========================================================================
    // Anthropic Response Deserialization
    // ========================================================================

    #[test]
    fn test_anthropic_response_text_only() {
        let json = r#"{
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.stop_reason, "end_turn");
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
        assert!(resp.usage.cache_read_input_tokens.is_none());
    }

    #[test]
    fn test_anthropic_response_with_tool_use() {
        let json = r#"{
            "content": [
                {"type": "text", "text": "Let me check."},
                {"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": "ls"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 20, "output_tokens": 15}
        }"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 2);
        assert_eq!(resp.stop_reason, "tool_use");
    }

    #[test]
    fn test_anthropic_response_with_cache_tokens() {
        let json = r#"{
            "content": [{"type": "text", "text": "Hi"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 80,
                "cache_creation_input_tokens": 20
            }
        }"#;
        let resp: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.usage.cache_read_input_tokens, Some(80));
        assert_eq!(resp.usage.cache_creation_input_tokens, Some(20));
    }

    // ========================================================================
    // OpenAI Response Deserialization
    // ========================================================================

    #[test]
    fn test_openai_response_text_only() {
        let json = r#"{
            "choices": [{
                "message": {"content": "Hello!", "tool_calls": null},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }"#;
        let resp: OpenAiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].message.content, Some("Hello!".to_string()));
        assert_eq!(resp.choices[0].finish_reason, Some("stop".to_string()));
        assert_eq!(resp.usage.total_tokens, 15);
    }

    #[test]
    fn test_openai_response_with_tool_calls() {
        let json = r#"{
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "function": {"name": "bash", "arguments": "{\"command\":\"ls\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 10, "total_tokens": 30}
        }"#;
        let resp: OpenAiResponse = serde_json::from_str(json).unwrap();
        assert!(resp.choices[0].message.content.is_none());
        let tool_calls = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call-1");
        assert_eq!(tool_calls[0].function.name, "bash");
    }

    #[test]
    fn test_openai_response_null_content_and_tool_calls() {
        let json = r#"{
            "choices": [{
                "message": {"content": null, "tool_calls": null},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        }"#;
        let resp: OpenAiResponse = serde_json::from_str(json).unwrap();
        assert!(resp.choices[0].message.content.is_none());
        assert!(resp.choices[0].message.tool_calls.is_none());
    }

    // ========================================================================
    // Anthropic Streaming Event Deserialization
    // ========================================================================

    #[test]
    fn test_anthropic_stream_message_start() {
        let json = r#"{
            "type": "message_start",
            "message": {
                "usage": {"input_tokens": 100, "output_tokens": 0}
            }
        }"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::MessageStart { .. }));
    }

    #[test]
    fn test_anthropic_stream_content_block_start_text() {
        let json = r#"{
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::ContentBlockStart { .. }));
    }

    #[test]
    fn test_anthropic_stream_content_block_start_tool() {
        let json = r#"{
            "type": "content_block_start",
            "index": 1,
            "content_block": {"type": "tool_use", "id": "t1", "name": "bash", "input": {}}
        }"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::ContentBlockStart { .. }));
    }

    #[test]
    fn test_anthropic_stream_text_delta() {
        let json = r#"{
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "Hello"}
        }"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::ContentBlockDelta { .. }));
    }

    #[test]
    fn test_anthropic_stream_input_json_delta() {
        let json = r#"{
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"cmd\":"}
        }"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::ContentBlockDelta { .. }));
    }

    #[test]
    fn test_anthropic_stream_message_delta() {
        let json = r#"{
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 50}
        }"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::MessageDelta { .. }));
    }

    #[test]
    fn test_anthropic_stream_message_stop() {
        let json = r#"{"type": "message_stop"}"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::MessageStop));
    }

    #[test]
    fn test_anthropic_stream_ping() {
        let json = r#"{"type": "ping"}"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::Ping));
    }

    #[test]
    fn test_anthropic_stream_error() {
        let json = r#"{
            "type": "error",
            "error": {"type": "overloaded_error", "message": "Server overloaded"}
        }"#;
        let event: AnthropicStreamEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, AnthropicStreamEvent::Error { .. }));
    }

    // ========================================================================
    // OpenAI Streaming Event Deserialization
    // ========================================================================

    #[test]
    fn test_openai_stream_chunk_text() {
        let json = r#"{
            "choices": [{
                "delta": {"content": "Hello"},
                "finish_reason": null
            }],
            "usage": null
        }"#;
        let chunk: OpenAiStreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].delta.as_ref().unwrap().content, Some("Hello".to_string()));
        assert!(chunk.choices[0].finish_reason.is_none());
    }

    #[test]
    fn test_openai_stream_chunk_tool_call() {
        let json = r#"{
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "function": {"name": "bash", "arguments": ""}
                    }]
                },
                "finish_reason": null
            }],
            "usage": null
        }"#;
        let chunk: OpenAiStreamChunk = serde_json::from_str(json).unwrap();
        let delta = chunk.choices[0].delta.as_ref().unwrap();
        let tool_calls = delta.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls[0].id, Some("call-1".to_string()));
        assert_eq!(tool_calls[0].function.as_ref().unwrap().name, Some("bash".to_string()));
    }

    #[test]
    fn test_openai_stream_chunk_done() {
        let json = r#"{
            "choices": [{
                "delta": {},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }"#;
        let chunk: OpenAiStreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].finish_reason, Some("stop".to_string()));
        assert!(chunk.usage.is_some());
    }

    // ========================================================================
    // LlmConfig and create_client_with_config
    // ========================================================================

    #[test]
    fn test_llm_config_with_retry() {
        let retry = RetryConfig::default();
        let config = LlmConfig::new("anthropic", "claude-sonnet-4-20250514", "key")
            .with_retry_config(retry);
        assert!(config.retry_config.is_some());
    }

    #[test]
    fn test_create_client_anthropic() {
        let config = LlmConfig::new("anthropic", "claude-sonnet-4-20250514", "key");
        let _client = create_client_with_config(config);
    }

    #[test]
    fn test_create_client_openai() {
        let config = LlmConfig::new("openai", "gpt-4", "key");
        let _client = create_client_with_config(config);
    }

    #[test]
    fn test_create_client_unknown_defaults_anthropic() {
        let config = LlmConfig::new("unknown_provider", "model", "key");
        let _client = create_client_with_config(config);
    }

    #[test]
    fn test_create_client_with_base_url() {
        let config = LlmConfig::new("openai", "gpt-4", "key")
            .with_base_url("https://custom.api.com");
        let _client = create_client_with_config(config);
    }

    // ========================================================================
    // normalize_base_url
    // ========================================================================

    #[test]
    fn test_normalize_base_url_strips_trailing_slash() {
        assert_eq!(normalize_base_url("https://api.example.com/"), "https://api.example.com");
    }

    #[test]
    fn test_normalize_base_url_strips_v1() {
        assert_eq!(normalize_base_url("https://api.example.com/v1"), "https://api.example.com");
    }

    #[test]
    fn test_normalize_base_url_strips_v1_slash() {
        assert_eq!(normalize_base_url("https://api.example.com/v1/"), "https://api.example.com");
    }

    #[test]
    fn test_normalize_base_url_no_change() {
        assert_eq!(normalize_base_url("https://api.example.com"), "https://api.example.com");
    }

    // ========================================================================
    // AnthropicClient with_retry_config
    // ========================================================================

    #[test]
    fn test_anthropic_client_with_retry_config() {
        let retry = RetryConfig::default();
        let client = AnthropicClient::new("key".to_string(), "model".to_string())
            .with_retry_config(retry.clone());
        assert_eq!(client.retry_config.max_retries, retry.max_retries);
    }

    #[test]
    fn test_openai_client_with_retry_config() {
        let retry = RetryConfig::default();
        let client = OpenAiClient::new("key".to_string(), "model".to_string())
            .with_retry_config(retry.clone());
        assert_eq!(client.retry_config.max_retries, retry.max_retries);
    }

    #[test]
    fn test_openai_client_with_base_url() {
        let client = OpenAiClient::new("key".to_string(), "model".to_string())
            .with_base_url("https://custom.openai.com".to_string());
        assert_eq!(client.base_url, "https://custom.openai.com");
    }

    #[test]
    fn test_anthropic_client_with_base_url() {
        let client = AnthropicClient::new("key".to_string(), "model".to_string())
            .with_base_url("https://custom.anthropic.com".to_string());
        assert_eq!(client.base_url, "https://custom.anthropic.com");
    }
}
