//! Anthropic Claude LLM client

use super::http::{default_http_client, normalize_base_url, HttpClient};
use super::structured;
use super::types::*;
use super::{LlmClient, ModelGenerationPool};
use crate::retry::{AttemptOutcome, RetryConfig};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Default max tokens for LLM responses
pub(crate) const DEFAULT_MAX_TOKENS: usize = 8192;

/// Anthropic rejects `budget_tokens` below this value.
const MIN_THINKING_BUDGET: usize = 1024;

/// Tokens kept for the visible answer when extended thinking is enabled.
const THINKING_ANSWER_RESERVE: usize = 1024;

/// Anthropic Claude client
pub struct AnthropicClient {
    pub(crate) provider_name: String,
    pub(crate) api_key: SecretString,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) max_tokens: usize,
    pub(crate) temperature: Option<f32>,
    pub(crate) thinking_budget: Option<usize>,
    pub(crate) http: Arc<dyn HttpClient>,
    pub(crate) retry_config: RetryConfig,
}

impl AnthropicClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            provider_name: "anthropic".to_string(),
            api_key: SecretString::new(api_key),
            model,
            base_url: "https://api.anthropic.com".to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            temperature: None,
            thinking_budget: None,
            http: default_http_client(),
            retry_config: RetryConfig::default(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = normalize_base_url(&base_url);
        self
    }

    pub fn with_provider_name(mut self, provider_name: impl Into<String>) -> Self {
        self.provider_name = provider_name.into();
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_thinking_budget(mut self, budget: usize) -> Self {
        self.thinking_budget = Some(budget);
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

    fn initial_tool_input_json(input: &serde_json::Value) -> Option<String> {
        match input {
            serde_json::Value::Object(map) if map.is_empty() => None,
            serde_json::Value::Null => None,
            value => serde_json::to_string(value).ok(),
        }
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

        // Apply optional sampling parameters
        if let Some(temp) = self.temperature {
            request["temperature"] = serde_json::json!(temp);
        }

        // GLM-5.2/5.3 always reason. Leaving the field off selects the server
        // default `max`, and `thinking.type=disabled` is rejected. The latency
        // knob is `reasoning_effort`, not a 16k token budget.
        if glm_reasoning_model(&self.model) {
            request["thinking"] = serde_json::json!({ "type": "enabled" });
            request["reasoning_effort"] =
                serde_json::json!(glm_reasoning_effort(self.thinking_budget));
            request["temperature"] = serde_json::json!(1.0);
        } else if let Some(budget) = fitted_thinking_budget(self.max_tokens, self.thinking_budget) {
            // A budget that does not leave room for the answer is omitted.
            // Raising max_tokens to fit it makes the model think for the whole
            // effort budget before any visible text.
            request["thinking"] = serde_json::json!({
                "type": "enabled",
                "budget_tokens": budget
            });
            // Thinking requires temperature=1 per Anthropic docs
            request["temperature"] = serde_json::json!(1.0);
        }

        request
    }
}

/// Keep a thinking budget only when Anthropic can accept it and the output
/// window still has room for the visible answer.
pub(crate) fn fitted_thinking_budget(max_tokens: usize, budget: Option<usize>) -> Option<usize> {
    let budget = budget.filter(|budget| *budget >= MIN_THINKING_BUDGET)?;
    let answer_room = max_tokens.saturating_sub(THINKING_ANSWER_RESERVE);
    if budget < answer_room && budget < max_tokens {
        Some(budget)
    } else {
        None
    }
}

fn glm_reasoning_model(model: &str) -> bool {
    let id = model.rsplit(['/', ':']).next().unwrap_or(model);
    let id = id.trim().to_ascii_lowercase();
    id.starts_with("glm-5.2")
        || id.starts_with("glm-5.3")
        || id.starts_with("glm-5-2")
        || id.starts_with("glm-5-3")
}

/// Map an a3s thinking budget onto GLM-5.2/5.3 `low` / `high` / `max`.
///
/// An unset budget becomes `low`. The server default is `max`.
fn glm_reasoning_effort(budget: Option<usize>) -> &'static str {
    match budget.unwrap_or(0) {
        0..=2_048 => "low",
        2_049..=16_384 => "high",
        _ => "max",
    }
}

impl AnthropicClient {
    /// Apply a structured-output directive to an Anthropic request.
    ///
    /// Anthropic supports forced tool choice (`tool_choice`) but has no
    /// `response_format`, so only `force_tool` is honored.
    fn apply_directive(
        request: &mut serde_json::Value,
        directive: &structured::StructuredDirective,
    ) {
        if let Some(tool) = &directive.force_tool {
            request["tool_choice"] = serde_json::json!({ "type": "tool", "name": tool });
        }
    }

    /// Execute a fully-built (non-streaming) request body.
    async fn send_request(&self, request_body: serde_json::Value) -> Result<LlmResponse> {
        {
            let request_started_at = Instant::now();
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
                    match http
                        .post(url, headers, request_body, CancellationToken::new())
                        .await
                    {
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
                                AttemptOutcome::Fatal(anyhow::Error::new(
                                    crate::llm::NonRetryableLlmError::from_status(
                                        &self.provider_name,
                                        status.as_u16(),
                                        format!("at {url}: {}", resp.body),
                                    ),
                                ))
                            }
                        }
                        Err(e) => {
                            if crate::llm::http::is_retryable_http_failure(&e) {
                                AttemptOutcome::Retryable {
                                    status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
                                    body: format!("network error: {e}"),
                                    retry_after: None,
                                }
                            } else {
                                AttemptOutcome::Fatal(e)
                            }
                        }
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
                .filter_map(|block| match block {
                    AnthropicContentBlock::Text { text } => Some(ContentBlock::Text { text }),
                    AnthropicContentBlock::ToolUse { id, name, input } => {
                        Some(ContentBlock::ToolUse { id, name, input })
                    }
                    AnthropicContentBlock::Thinking { .. } => None,
                })
                .collect();

            let llm_response = LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content,
                    reasoning_content: None,
                    transcript_text: None,
                    transcript_visibility: Default::default(),
                },
                usage: TokenUsage {
                    prompt_tokens: parsed.usage.input_tokens,
                    completion_tokens: parsed.usage.output_tokens,
                    total_tokens: parsed.usage.input_tokens + parsed.usage.output_tokens,
                    cache_read_tokens: parsed.usage.cache_read_input_tokens,
                    cache_write_tokens: parsed.usage.cache_creation_input_tokens,
                },
                stop_reason: Some(parsed.stop_reason),
                token_logprobs: Vec::new(),
                meta: Some(LlmResponseMeta {
                    provider: Some(self.provider_name.clone()),
                    request_model: Some(self.model.clone()),
                    request_url: Some(url.clone()),
                    response_id: parsed.id,
                    response_model: parsed.model,
                    response_object: parsed.response_type,
                    first_token_ms: None,
                    duration_ms: Some(request_started_at.elapsed().as_millis() as u64),
                }),
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
}

#[async_trait]
impl LlmClient for AnthropicClient {
    fn model_generation_pool(&self) -> Option<ModelGenerationPool> {
        ModelGenerationPool::for_endpoint(
            &self.provider_name,
            &self.model,
            &self.base_url,
            self.model_generation_concurrency(),
        )
        .ok()
    }

    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.send_request(self.build_request(messages, system, tools))
            .await
    }

    async fn complete_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: &structured::StructuredDirective,
    ) -> Result<LlmResponse> {
        let mut request_body = self.build_request(messages, system, tools);
        Self::apply_directive(&mut request_body, directive);
        self.send_request(request_body).await
    }

    fn native_structured_support(&self) -> structured::NativeStructuredSupport {
        structured::NativeStructuredSupport::ForcedTool
    }

    fn has_distinct_non_streaming_transport(&self) -> bool {
        true
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.send_streaming(self.build_request(messages, system, tools), cancel_token)
            .await
    }

    async fn complete_streaming_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: &structured::StructuredDirective,
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let mut request_body = self.build_request(messages, system, tools);
        Self::apply_directive(&mut request_body, directive);
        self.send_streaming(request_body, cancel_token).await
    }
}

impl AnthropicClient {
    /// Execute a fully-built streaming request body (sets `stream: true`).
    async fn send_streaming(
        &self,
        mut request_body: serde_json::Value,
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        {
            let request_started_at = Instant::now();
            request_body["stream"] = serde_json::json!(true);

            let url = format!("{}/v1/messages", self.base_url);

            let headers = vec![
                ("x-api-key", self.api_key.expose()),
                ("anthropic-version", "2023-06-01"),
                ("anthropic-beta", "prompt-caching-2024-07-31"),
            ];

            let streaming_resp = crate::retry::with_retry_cancellable(
                &self.retry_config,
                &cancel_token,
                |_attempt| {
                let http = &self.http;
                let url = &url;
                let headers = headers.clone();
                let request_body = &request_body;
                let cancel_token = cancel_token.clone();
                async move {
                    let resp = tokio::select! {
                        _ = cancel_token.cancelled() => {
                            return AttemptOutcome::Fatal(anyhow::Error::new(
                                crate::llm::HttpClientError::cancelled(
                                    "Anthropic streaming HTTP request",
                                ),
                            ));
                        }
                        result = http.post_streaming(url, headers, request_body, cancel_token.clone()) => {
                            match result {
                                Ok(r) => r,
                                Err(e) => {
                                    return if crate::llm::http::is_retryable_http_failure(&e) {
                                        AttemptOutcome::Retryable {
                                            status: reqwest::StatusCode::SERVICE_UNAVAILABLE,
                                            body: format!("network error: {e}"),
                                            retry_after: None,
                                        }
                                    } else {
                                        AttemptOutcome::Fatal(e.context("HTTP request failed"))
                                    };
                                }
                            }
                        }
                    };
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
                            AttemptOutcome::Fatal(anyhow::Error::new(
                                crate::llm::NonRetryableLlmError::from_status(
                                    &self.provider_name,
                                    status.as_u16(),
                                    format!("at {url}: {}", resp.error_body),
                                ),
                            ))
                        }
                    }
                }
                },
            )
            .await?;

            let (tx, rx) = mpsc::channel(100);

            let mut stream = streaming_resp.byte_stream;
            let provider_name = self.provider_name.clone();
            let request_model = self.model.clone();
            let request_url = url.clone();
            let stream_cancellation = cancel_token.clone();
            tokio::spawn(async move {
                let mut buffer = String::new();
                let mut utf8_decoder = crate::sse::Utf8StreamDecoder::default();
                let mut content_blocks: Vec<ContentBlock> = Vec::new();
                let mut text_content = String::new();
                let mut current_tool_id = String::new();
                let mut current_tool_name = String::new();
                let mut current_tool_input = String::new();
                let mut usage = TokenUsage::default();
                let mut stop_reason = None;
                let mut response_id = None;
                let mut response_model = None;
                let mut response_object = Some("message".to_string());
                let mut first_token_ms = None;

                loop {
                    let chunk_result = tokio::select! {
                        biased;
                        _ = stream_cancellation.cancelled() => break,
                        _ = tx.closed() => break,
                        chunk = stream.next() => match chunk {
                            Some(chunk) => chunk,
                            None => break,
                        },
                    };
                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::error!("Stream error: {}", e);
                            break;
                        }
                    };

                    if let Err(error) = utf8_decoder.push_to(&chunk, &mut buffer) {
                        tracing::error!(%error, "Anthropic stream returned invalid UTF-8");
                        break;
                    }

                    while let Some(event_end) = buffer.find("\n\n") {
                        let event_data: String = buffer.drain(..event_end).collect();
                        buffer.drain(..2);

                        for line in event_data.lines() {
                            if let Some(data) = crate::sse::data_field_value(line) {
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
                                            AnthropicContentBlock::Text { .. }
                                            | AnthropicContentBlock::Thinking { .. } => {}
                                            AnthropicContentBlock::ToolUse { id, name, input } => {
                                                if !text_content.is_empty() {
                                                    content_blocks.push(ContentBlock::Text {
                                                        text: std::mem::take(&mut text_content),
                                                    });
                                                }
                                                current_tool_id = id.clone();
                                                current_tool_name = name.clone();
                                                current_tool_input =
                                                    Self::initial_tool_input_json(&input)
                                                        .unwrap_or_default();
                                                let _ = tx
                                                    .send(StreamEvent::ToolUseStart { id, name })
                                                    .await;
                                                if !current_tool_input.is_empty() {
                                                    if first_token_ms.is_none() {
                                                        first_token_ms = Some(
                                                            request_started_at.elapsed().as_millis()
                                                                as u64,
                                                        );
                                                    }
                                                    let _ = tx
                                                        .send(StreamEvent::ToolUseInputDelta {
                                                            id: Some(current_tool_id.clone()),
                                                            delta: current_tool_input.clone(),
                                                        })
                                                        .await;
                                                }
                                            }
                                        },
                                        AnthropicStreamEvent::ContentBlockDelta {
                                            index: _,
                                            delta,
                                        } => match delta {
                                            AnthropicDelta::TextDelta { text } => {
                                                if first_token_ms.is_none() {
                                                    first_token_ms = Some(
                                                        request_started_at.elapsed().as_millis()
                                                            as u64,
                                                    );
                                                }
                                                text_content.push_str(&text);
                                                let _ = tx.send(StreamEvent::TextDelta(text)).await;
                                            }
                                            AnthropicDelta::ThinkingDelta { thinking } => {
                                                if thinking.is_empty() {
                                                    continue;
                                                }
                                                if first_token_ms.is_none() {
                                                    first_token_ms = Some(
                                                        request_started_at.elapsed().as_millis()
                                                            as u64,
                                                    );
                                                }
                                                let _ = tx
                                                    .send(StreamEvent::ReasoningDelta(thinking))
                                                    .await;
                                            }
                                            AnthropicDelta::SignatureDelta { .. } => {}
                                            AnthropicDelta::InputJsonDelta { partial_json } => {
                                                if first_token_ms.is_none() {
                                                    first_token_ms = Some(
                                                        request_started_at.elapsed().as_millis()
                                                            as u64,
                                                    );
                                                }
                                                current_tool_input.push_str(&partial_json);
                                                let _ = tx
                                                    .send(StreamEvent::ToolUseInputDelta {
                                                        id: Some(current_tool_id.clone()),
                                                        delta: partial_json,
                                                    })
                                                    .await;
                                            }
                                        },
                                        AnthropicStreamEvent::ContentBlockStop { index: _ }
                                            if !current_tool_id.is_empty() =>
                                        {
                                            let input: serde_json::Value = if current_tool_input
                                                .trim()
                                                .is_empty()
                                            {
                                                serde_json::Value::Object(Default::default())
                                            } else {
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
                                                    })
                                            };
                                            content_blocks.push(ContentBlock::ToolUse {
                                                id: current_tool_id.clone(),
                                                name: current_tool_name.clone(),
                                                input,
                                            });
                                            current_tool_id.clear();
                                            current_tool_name.clear();
                                            current_tool_input.clear();
                                        }
                                        AnthropicStreamEvent::MessageStart { message } => {
                                            response_id = message.id;
                                            response_model = message.model;
                                            response_object = message.message_type;
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
                                            if !text_content.is_empty() {
                                                content_blocks.push(ContentBlock::Text {
                                                    text: std::mem::take(&mut text_content),
                                                });
                                            }
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
                                                    transcript_text: None,
                                                    transcript_visibility: Default::default(),
                                                },
                                                usage: usage.clone(),
                                                stop_reason: stop_reason.clone(),
                                                token_logprobs: Vec::new(),
                                                meta: Some(LlmResponseMeta {
                                                    provider: Some(provider_name.clone()),
                                                    request_model: Some(request_model.clone()),
                                                    request_url: Some(request_url.clone()),
                                                    response_id: response_id.clone(),
                                                    response_model: response_model.clone(),
                                                    response_object: response_object.clone(),
                                                    first_token_ms,
                                                    duration_ms: Some(
                                                        request_started_at.elapsed().as_millis()
                                                            as u64,
                                                    ),
                                                }),
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
                if let Err(error) = utf8_decoder.finish() {
                    tracing::error!(%error, "Anthropic stream ended inside a UTF-8 code point");
                }
            });

            Ok(rx)
        }
    }
}

// Anthropic API response types (private)
#[derive(Debug, Deserialize)]
pub(crate) struct AnthropicResponse {
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(rename = "type", default)]
    pub(crate) response_type: Option<String>,
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
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
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
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(rename = "type", default)]
    pub(crate) message_type: Option<String>,
    pub(crate) usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{Message, ToolDefinition};

    fn make_client() -> AnthropicClient {
        AnthropicClient::new("test-key".to_string(), "claude-opus-4-6".to_string())
    }

    #[test]
    fn test_build_request_basic() {
        let client = make_client();
        let messages = vec![Message::user("Hello")];
        let req = client.build_request(&messages, None, &[]);

        assert_eq!(req["model"], "claude-opus-4-6");
        assert_eq!(req["max_tokens"], DEFAULT_MAX_TOKENS);
        assert!(req["thinking"].is_null());
    }

    #[test]
    fn test_build_request_with_thinking_budget() {
        let client = make_client()
            .with_max_tokens(16_000)
            .with_thinking_budget(10_000);
        let messages = vec![Message::user("Think carefully.")];
        let req = client.build_request(&messages, None, &[]);

        // thinking block must be present
        assert_eq!(req["thinking"]["type"], "enabled");
        assert_eq!(req["thinking"]["budget_tokens"], 10_000);
        // temperature must be 1.0 when thinking is enabled
        assert_eq!(req["temperature"], 1.0_f64);
    }

    #[test]
    fn oversized_thinking_budget_is_omitted_on_the_default_window() {
        // Default high effort is 16384. The client window is 8192. Sending the
        // budget would be an invalid Anthropic request and a long hidden think.
        let client = make_client().with_thinking_budget(16_384);
        let req = client.build_request(&[Message::user("fix the bug")], None, &[]);

        assert_eq!(req["max_tokens"], DEFAULT_MAX_TOKENS);
        assert!(req["thinking"].is_null());
    }

    #[test]
    fn glm_default_effort_uses_reasoning_effort_high_without_a_token_budget() {
        let mut client = make_client().with_thinking_budget(16_384);
        client.model = "glm-5.3-flash".to_string();
        let req = client.build_request(&[Message::user("fix the bug")], None, &[]);

        assert_eq!(req["thinking"]["type"], "enabled");
        assert!(req["thinking"].get("budget_tokens").is_none());
        assert_eq!(req["reasoning_effort"], "high");
        assert_eq!(req["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn glm_effort_tracks_the_a3s_budget() {
        let mut low = make_client().with_thinking_budget(2_048);
        low.model = "glm-5.3".to_string();
        let low_req = low.build_request(&[Message::user("hi")], None, &[]);
        assert_eq!(low_req["reasoning_effort"], "low");

        let mut deep = make_client().with_thinking_budget(65_536);
        deep.model = "glm-5.2".to_string();
        let deep_req = deep.build_request(&[Message::user("hi")], None, &[]);
        assert_eq!(deep_req["reasoning_effort"], "max");

        let mut unset = make_client();
        unset.model = "glm-5.3-flashx".to_string();
        let unset_req = unset.build_request(&[Message::user("hi")], None, &[]);
        assert_eq!(unset_req["reasoning_effort"], "low");
    }

    #[test]
    fn thinking_budget_that_fits_stays_under_the_answer_reserve() {
        let client = make_client().with_thinking_budget(2_048);
        let req = client.build_request(&[Message::user("fix the bug")], None, &[]);

        assert_eq!(req["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(req["thinking"]["budget_tokens"], 2_048);
    }

    #[test]
    fn thinking_delta_deserializes_for_the_stream_parser() {
        let event: AnthropicStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"look at the call site"}}"#,
        )
        .expect("thinking delta");
        match event {
            AnthropicStreamEvent::ContentBlockDelta { delta, .. } => match delta {
                AnthropicDelta::ThinkingDelta { thinking } => {
                    assert_eq!(thinking, "look at the call site");
                }
                other => panic!("unexpected delta: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }

        let signature: AnthropicStreamEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"abc"}}"#,
        )
        .expect("signature delta");
        assert!(matches!(
            signature,
            AnthropicStreamEvent::ContentBlockDelta {
                delta: AnthropicDelta::SignatureDelta { .. },
                ..
            }
        ));
    }

    #[test]
    fn test_build_request_thinking_overrides_temperature() {
        // Even if temperature was set, thinking forces it to 1.0
        let client = make_client()
            .with_temperature(0.5)
            .with_thinking_budget(5_000);
        let messages = vec![Message::user("Test")];
        let req = client.build_request(&messages, None, &[]);

        assert_eq!(req["temperature"], 1.0_f64);
        assert_eq!(req["thinking"]["budget_tokens"], 5_000);
    }

    #[test]
    fn test_build_request_no_thinking_uses_temperature() {
        let client = make_client().with_temperature(0.7);
        let messages = vec![Message::user("Test")];
        let req = client.build_request(&messages, None, &[]);

        // Use approximate comparison for f64
        let temp = req["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 0.01);
        assert!(req["thinking"].is_null());
    }

    #[test]
    fn test_build_request_with_system_prompt() {
        let client = make_client();
        let messages = vec![Message::user("Hello")];
        let req = client.build_request(&messages, Some("You are helpful."), &[]);

        let system = &req["system"];
        assert!(system.is_array());
        assert_eq!(system[0]["type"], "text");
        assert_eq!(system[0]["text"], "You are helpful.");
        assert!(system[0]["cache_control"].is_object());
    }

    #[test]
    fn test_build_request_with_tools() {
        let client = make_client();
        let messages = vec![Message::user("Use a tool")];
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];
        let req = client.build_request(&messages, None, &tools);

        assert!(req["tools"].is_array());
        assert_eq!(req["tools"][0]["name"], "read_file");
        // Last tool should have cache_control
        assert!(req["tools"][0]["cache_control"].is_object());
    }

    #[test]
    fn test_build_request_thinking_budget_sets_max_tokens() {
        // max_tokens is still respected when thinking is enabled
        let client = make_client()
            .with_max_tokens(16_000)
            .with_thinking_budget(8_000);
        let messages = vec![Message::user("Test")];
        let req = client.build_request(&messages, None, &[]);

        assert_eq!(req["max_tokens"], 16_000);
        assert_eq!(req["thinking"]["budget_tokens"], 8_000);
    }

    #[test]
    fn test_apply_directive_forces_tool_choice() {
        let mut req = serde_json::json!({ "model": "m", "messages": [] });
        let directive = structured::StructuredDirective {
            force_tool: Some("emit_person".to_string()),
            response_format: None,
            validation_schema: None,
        };
        AnthropicClient::apply_directive(&mut req, &directive);
        assert_eq!(req["tool_choice"]["type"], "tool");
        assert_eq!(req["tool_choice"]["name"], "emit_person");
    }

    #[test]
    fn test_apply_directive_ignores_response_format() {
        // Anthropic has no response_format; both a response_format-only and an
        // empty directive must be no-ops.
        let mut req = serde_json::json!({ "model": "m" });
        AnthropicClient::apply_directive(
            &mut req,
            &structured::StructuredDirective {
                force_tool: None,
                response_format: Some(structured::ResponseFormat::JsonObject),
                validation_schema: None,
            },
        );
        assert!(req.get("response_format").is_none());
        assert!(req.get("tool_choice").is_none());
    }

    #[test]
    fn test_native_structured_support_is_forced_tool() {
        assert_eq!(
            make_client().native_structured_support(),
            structured::NativeStructuredSupport::ForcedTool
        );
    }
}
