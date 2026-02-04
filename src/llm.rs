//! LLM client implementations
//!
//! Provides unified interface for multiple LLM providers:
//! - Anthropic Claude (Messages API)
//! - OpenAI GPT (Chat Completions API)
//!
//! Features:
//! - Tool calling support
//! - Token usage tracking

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;

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

/// Anthropic Claude client
pub struct AnthropicClient {
    api_key: String,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            base_url: "https://api.anthropic.com".to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = normalize_base_url(&base_url);
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
            "max_tokens": 8192,
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
        let request_body = self.build_request(messages, system, tools);
        let url = format!("{}/v1/messages", self.base_url);

        let headers = vec![
            ("x-api-key", self.api_key.as_str()),
            ("anthropic-version", "2023-06-01"),
        ];

        let (status, body) = http_post_json(&self.client, &url, headers, &request_body).await?;

        if !status.is_success() {
            anyhow::bail!("Anthropic API error at {} ({}): {}", url, status, body);
        }

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

        Ok(LlmResponse {
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
        })
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let mut request_body = self.build_request(messages, system, tools);
        request_body["stream"] = serde_json::json!(true);

        let url = format!("{}/v1/messages", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send streaming request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error at {} ({}): {}", url, status, body);
        }

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
                                                    .unwrap_or(serde_json::json!({}));
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
}

impl OpenAiClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            base_url: "https://api.openai.com".to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = normalize_base_url(&base_url);
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

        let (status, body) = http_post_json(&self.client, &url, headers, &request).await?;

        if !status.is_success() {
            anyhow::bail!("OpenAI API error at {} ({}): {}", url, status, body);
        }

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
                    name: tc.function.name,
                    input: serde_json::from_str(&tc.function.arguments).unwrap_or_default(),
                });
            }
        }

        Ok(LlmResponse {
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
        })
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<mpsc::Receiver<StreamEvent>> {
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

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .context("Failed to send streaming request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error at {} ({}): {}", url, status, body);
        }

        let (tx, rx) = mpsc::channel(100);

        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut content_blocks: Vec<ContentBlock> = Vec::new();
            let mut text_content = String::new();
            let mut tool_calls: std::collections::HashMap<usize, (String, String, String)> =
                std::collections::HashMap::new();
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
                                for (_, (id, name, args)) in tool_calls.drain() {
                                    content_blocks.push(ContentBlock::ToolUse {
                                        id,
                                        name,
                                        input: serde_json::from_str(&args).unwrap_or_default(),
                                    });
                                }
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
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }
}

/// Create LLM client with full configuration (supports custom base_url)
pub fn create_client_with_config(config: LlmConfig) -> Arc<dyn LlmClient> {
    match config.provider.as_str() {
        "anthropic" | "claude" => {
            let mut client = AnthropicClient::new(config.api_key, config.model);
            if let Some(base_url) = config.base_url {
                client = client.with_base_url(base_url);
            }
            Arc::new(client)
        }
        "openai" | "gpt" => {
            let mut client = OpenAiClient::new(config.api_key, config.model);
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
            let mut client = OpenAiClient::new(config.api_key, config.model);
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
}
