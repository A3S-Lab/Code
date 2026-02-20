//! Anthropic Claude LLM client

use super::http::{default_http_client, normalize_base_url, HttpClient};
use super::types::*;
use super::LlmClient;
use crate::retry::{AttemptOutcome, RetryConfig};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Default max tokens for LLM responses
pub(crate) const DEFAULT_MAX_TOKENS: usize = 8192;

/// Anthropic Claude client
pub struct AnthropicClient {
    pub(crate) api_key: SecretString,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) max_tokens: usize,
    pub(crate) http: Arc<dyn HttpClient>,
    pub(crate) retry_config: RetryConfig,
}

impl AnthropicClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key: SecretString::new(api_key),
            model,
            base_url: "https://api.anthropic.com".to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            http: default_http_client(),
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

    pub fn with_http_client(mut self, http: Arc<dyn HttpClient>) -> Self {
        self.http = http;
        self
    }

    pub(crate) fn build_request(
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

        // System prompt with cache_control for prompt caching.
        // Anthropic caches system content blocks marked with
        // `cache_control: { type: "ephemeral" }`.
        if let Some(sys) = system {
            request["system"] = serde_json::json!([
                {
                    "type": "text",
                    "text": sys,
                    "cache_control": { "type": "ephemeral" }
                }
            ]);
        }

        if !tools.is_empty() {
            let mut tool_defs: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();

            // Mark the last tool definition with cache_control so the
            // entire tool block is cached on subsequent requests.
            if let Some(last) = tool_defs.last_mut() {
                last["cache_control"] = serde_json::json!({ "type": "ephemeral" });
            }

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
        {
            let request_body = self.build_request(messages, system, tools);
            let url = format!("{}/v1/messages", self.base_url);

            let headers = vec![
                ("x-api-key", self.api_key.expose()),
                ("anthropic-version", "2023-06-01"),
                ("anthropic-beta", "prompt-caching-2024-07-31"),
            ];

            let response = crate::retry::with_retry(&self.retry_config, |_attempt| {
                let http = &self.http;
                let url = &url;
                let headers = headers.clone();
                let request_body = &request_body;
                async move {
                    match http.post(url, headers, request_body).await {
                        Ok(resp) => {
                            let status = reqwest::StatusCode::from_u16(resp.status)
                                .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
                            if status.is_success() {
                                AttemptOutcome::Success(resp.body)
                            } else if self.retry_config.is_retryable_status(status) {
                                AttemptOutcome::Retryable {
                                    status,
                                    body: resp.body,
                                    retry_after: None,
                                }
                            } else {
                                AttemptOutcome::Fatal(anyhow::anyhow!(
                                    "Anthropic API error at {} ({}): {}",
                                    url,
                                    status,
                                    resp.body
                                ))
                            }
                        }
                        Err(e) => AttemptOutcome::Fatal(e),
                    }
                }
            })
            .await?;

            let parsed: AnthropicResponse =
                serde_json::from_str(&response).context("Failed to parse Anthropic response")?;

            tracing::debug!("Anthropic response: {:?}", parsed);

            let content: Vec<ContentBlock> = parsed
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
                    reasoning_content: None,
                },
                usage: TokenUsage {
                    prompt_tokens: parsed.usage.input_tokens,
                    completion_tokens: parsed.usage.output_tokens,
                    total_tokens: parsed.usage.input_tokens + parsed.usage.output_tokens,
                    cache_read_tokens: parsed.usage.cache_read_input_tokens,
                    cache_write_tokens: parsed.usage.cache_creation_input_tokens,
                },
                stop_reason: Some(parsed.stop_reason),
            };

            crate::telemetry::record_llm_usage(
                llm_response.usage.prompt_tokens,
                llm_response.usage.completion_tokens,
                llm_response.usage.total_tokens,
                llm_response.stop_reason.as_deref(),
            );

            Ok(llm_response)
        }
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        {
            let mut request_body = self.build_request(messages, system, tools);
            request_body["stream"] = serde_json::json!(true);

            let url = format!("{}/v1/messages", self.base_url);

            let headers = vec![
                ("x-api-key", self.api_key.expose()),
                ("anthropic-version", "2023-06-01"),
                ("anthropic-beta", "prompt-caching-2024-07-31"),
            ];

            let streaming_resp = crate::retry::with_retry(&self.retry_config, |_attempt| {
                let http = &self.http;
                let url = &url;
                let headers = headers.clone();
                let request_body = &request_body;
                async move {
                    match http.post_streaming(url, headers, request_body).await {
                        Ok(resp) => {
                            let status = reqwest::StatusCode::from_u16(resp.status)
                                .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
                            if status.is_success() {
                                AttemptOutcome::Success(resp)
                            } else {
                                let retry_after = resp
                                    .retry_after
                                    .as_deref()
                                    .and_then(|v| RetryConfig::parse_retry_after(Some(v)));
                                if self.retry_config.is_retryable_status(status) {
                                    AttemptOutcome::Retryable {
                                        status,
                                        body: resp.error_body,
                                        retry_after,
                                    }
                                } else {
                                    AttemptOutcome::Fatal(anyhow::anyhow!(
                                        "Anthropic API error at {} ({}): {}",
                                        url,
                                        status,
                                        resp.error_body
                                    ))
                                }
                            }
                        }
                        Err(e) => AttemptOutcome::Fatal(anyhow::anyhow!(
                            "Failed to send streaming request: {}",
                            e
                        )),
                    }
                }
            })
            .await?;

            let (tx, rx) = mpsc::channel(100);

            let mut stream = streaming_resp.byte_stream;
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

                    while let Some(event_end) = buffer.find("\n\n") {
                        let event_data: String = buffer.drain(..event_end).collect();
                        buffer.drain(..2);

                        for line in event_data.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    continue;
                                }

                                if let Ok(event) =
                                    serde_json::from_str::<AnthropicStreamEvent>(data)
                                {
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
                                        AnthropicStreamEvent::ContentBlockDelta {
                                            index: _,
                                            delta,
                                        } => match delta {
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
                                        },
                                        AnthropicStreamEvent::ContentBlockStop { index: _ } => {
                                            if !current_tool_id.is_empty() {
                                                let input: serde_json::Value =
                                                serde_json::from_str(&current_tool_input)
                                                    .unwrap_or_else(|e| {
                                                        tracing::warn!(
                                                            "Failed to parse tool input JSON for tool '{}': {}",
                                                            current_tool_name, e
                                                        );
                                                        serde_json::json!({
                                                            "__parse_error": format!(
                                                                "Malformed tool arguments: {}. Raw input: {}",
                                                                e, &current_tool_input
                                                            )
                                                        })
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
                                            crate::telemetry::record_llm_usage(
                                                usage.prompt_tokens,
                                                usage.completion_tokens,
                                                usage.total_tokens,
                                                stop_reason.as_deref(),
                                            );

                                            let response = LlmResponse {
                                                message: Message {
                                                    role: "assistant".to_string(),
                                                    content: std::mem::take(&mut content_blocks),
                                                    reasoning_content: None,
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
}

// Anthropic API response types (private)
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicResponse {
    pub(crate) content: Vec<AnthropicContentBlock>,
    pub(crate) stop_reason: String,
    pub(crate) usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum AnthropicContentBlock {
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
pub(crate) struct AnthropicUsage {
    pub(crate) input_tokens: usize,
    pub(crate) output_tokens: usize,
    pub(crate) cache_read_input_tokens: Option<usize>,
    pub(crate) cache_creation_input_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
pub(crate) enum AnthropicStreamEvent {
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
pub(crate) struct AnthropicMessageStart {
    pub(crate) usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicMessageDeltaData {
    pub(crate) stop_reason: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicOutputUsage {
    pub(crate) output_tokens: usize,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct AnthropicError {
    #[serde(rename = "type")]
    pub(crate) error_type: String,
    pub(crate) message: String,
}
