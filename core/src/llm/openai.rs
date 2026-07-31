//! OpenAI-compatible LLM client

use super::http::{default_http_client, normalize_base_url, HttpClient};
use super::structured;
use super::types::*;
use super::LlmClient;
use crate::llm::types::{ToolResultContent, ToolResultContentField};
use crate::retry::{AttemptOutcome, RetryConfig};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// OpenAI client
pub struct OpenAiClient {
    pub(crate) provider_name: String,
    pub(crate) api_key: SecretString,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) chat_completions_path: String,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) temperature: Option<f32>,
    pub(crate) max_tokens: Option<usize>,
    pub(crate) logprobs: bool,
    pub(crate) top_logprobs: Option<usize>,
    pub(crate) http: Arc<dyn HttpClient>,
    pub(crate) retry_config: RetryConfig,
    pub(crate) native_structured_support: structured::NativeStructuredSupport,
}

impl OpenAiClient {
    pub(crate) fn parse_tool_arguments(tool_name: &str, arguments: &str) -> serde_json::Value {
        if arguments.trim().is_empty() {
            return serde_json::Value::Object(Default::default());
        }

        serde_json::from_str(arguments).unwrap_or_else(|e| {
            tracing::warn!(
                "Failed to parse tool arguments JSON for tool '{}': {}",
                tool_name,
                e
            );
            serde_json::json!({
                "__parse_error": format!(
                    "Malformed tool arguments: {}. Raw input: {}",
                    e, arguments
                )
            })
        })
    }

    fn merge_stream_text(text_content: &mut String, incoming: &str) -> Option<String> {
        if incoming.is_empty() {
            return None;
        }
        if text_content.is_empty() {
            text_content.push_str(incoming);
            return Some(incoming.to_string());
        }
        if incoming == text_content.as_str() || text_content.ends_with(incoming) {
            return None;
        }
        // If incoming contains text_content as a prefix (incoming is the full content),
        // replace text_content instead of appending (avoids duplicate full content)
        if incoming.starts_with(text_content.as_str()) && incoming.len() > text_content.len() {
            let suffix = &incoming[text_content.len()..];
            if !suffix.is_empty() {
                *text_content = incoming.to_string();
                return Some(suffix.to_string());
            }
            return None;
        }
        if let Some(suffix) = incoming.strip_prefix(text_content.as_str()) {
            if suffix.is_empty() {
                return None;
            }
            text_content.push_str(suffix);
            return Some(suffix.to_string());
        }
        text_content.push_str(incoming);
        Some(incoming.to_string())
    }

    pub fn new(api_key: String, model: String) -> Self {
        Self {
            provider_name: "openai".to_string(),
            api_key: SecretString::new(api_key),
            model,
            base_url: "https://api.openai.com".to_string(),
            chat_completions_path: "/v1/chat/completions".to_string(),
            headers: HashMap::new(),
            temperature: None,
            max_tokens: None,
            logprobs: false,
            top_logprobs: None,
            http: default_http_client(),
            retry_config: RetryConfig::default(),
            native_structured_support: structured::NativeStructuredSupport::JsonSchema,
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

    pub fn with_chat_completions_path(mut self, path: impl Into<String>) -> Self {
        let path = path.into();
        self.chat_completions_path = if path.starts_with('/') {
            path
        } else {
            format!("/{}", path)
        };
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_logprobs(mut self, enabled: bool) -> Self {
        self.logprobs = enabled;
        self
    }

    pub fn with_top_logprobs(mut self, top_logprobs: usize) -> Self {
        self.logprobs = true;
        self.top_logprobs = Some(top_logprobs);
        self
    }

    pub fn with_retry_config(mut self, retry_config: RetryConfig) -> Self {
        self.retry_config = retry_config;
        self
    }

    pub fn with_native_structured_support(
        mut self,
        support: structured::NativeStructuredSupport,
    ) -> Self {
        self.native_structured_support = support;
        self
    }

    pub fn with_http_client(mut self, http: Arc<dyn HttpClient>) -> Self {
        self.http = http;
        self
    }

    pub(crate) fn request_headers(&self) -> Vec<(String, String)> {
        let mut headers = Vec::with_capacity(self.headers.len() + 1);
        let has_authorization = self
            .headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("authorization"));
        if !has_authorization {
            headers.push((
                "Authorization".to_string(),
                format!("Bearer {}", self.api_key.expose()),
            ));
        }
        headers.extend(
            self.headers
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        headers
    }

    pub(crate) fn convert_messages(&self, messages: &[Message]) -> Vec<serde_json::Value> {
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
                            let content_str = match content {
                                ToolResultContentField::Text(s) => s.clone(),
                                ToolResultContentField::Blocks(blocks) => blocks
                                    .iter()
                                    .filter_map(|b| {
                                        if let ToolResultContent::Text { text } = b {
                                            Some(text.clone())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                            };
                            return serde_json::json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content_str,
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
                                ContentBlock::Image { source } => serde_json::json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!(
                                            "data:{};base64,{}",
                                            source.media_type, source.data
                                        ),
                                    }
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

                // Handle assistant messages — kimi-k2.5 requires reasoning_content
                // on all assistant messages when thinking mode is enabled
                if msg.role == "assistant" {
                    let rc = msg.reasoning_content.as_deref().unwrap_or("");
                    let tool_calls: Vec<_> = msg.tool_calls();
                    if !tool_calls.is_empty() {
                        return serde_json::json!({
                            "role": "assistant",
                            "content": msg.text(),
                            "reasoning_content": rc,
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
                    return serde_json::json!({
                        "role": "assistant",
                        "content": content,
                        "reasoning_content": rc,
                    });
                }

                serde_json::json!({
                    "role": msg.role,
                    "content": content,
                })
            })
            .collect()
    }

    pub(crate) fn convert_tools(&self, tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
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

impl OpenAiClient {
    /// Apply a structured-output directive to an OpenAI-compatible request.
    ///
    /// OpenAI-compatible APIs support both forced function `tool_choice` and
    /// native `response_format` (`json_object` / `json_schema` + `strict`).
    fn apply_directive(
        request: &mut serde_json::Value,
        directive: &structured::StructuredDirective,
    ) {
        if let Some(tool) = &directive.force_tool {
            request["tool_choice"] = serde_json::json!({
                "type": "function",
                "function": { "name": tool }
            });
        }
        if let Some(rf) = &directive.response_format {
            request["response_format"] = match rf {
                structured::ResponseFormat::JsonObject => {
                    serde_json::json!({ "type": "json_object" })
                }
                structured::ResponseFormat::JsonSchema { name, schema } => serde_json::json!({
                    "type": "json_schema",
                    "json_schema": { "name": name, "schema": schema, "strict": true }
                }),
            };
        }
    }

    /// Build a chat-completions request body, optionally applying a directive.
    fn build_chat_request(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: Option<&structured::StructuredDirective>,
    ) -> serde_json::Value {
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
        });

        if let Some(temp) = self.temperature {
            request["temperature"] = serde_json::json!(temp);
        }
        if let Some(max) = self.max_tokens {
            request["max_tokens"] = serde_json::json!(max);
        }
        if self.logprobs {
            request["logprobs"] = serde_json::json!(true);
            if let Some(top_logprobs) = self.top_logprobs {
                request["top_logprobs"] = serde_json::json!(top_logprobs);
            }
        }

        if !tools.is_empty() {
            request["tools"] = serde_json::json!(self.convert_tools(tools));
        }

        if let Some(directive) = directive {
            Self::apply_directive(&mut request, directive);
        }

        request
    }

    /// Execute a fully-built (non-streaming) chat-completions request.
    async fn send_request(&self, request: serde_json::Value) -> Result<LlmResponse> {
        {
            let request_started_at = Instant::now();
            let url = format!("{}{}", self.base_url, self.chat_completions_path);
            let request_headers = self.request_headers();

            let response = crate::retry::with_retry(&self.retry_config, |_attempt| {
                let http = &self.http;
                let url = &url;
                let request_headers = request_headers.clone();
                let request = &request;
                async move {
                    let headers = request_headers
                        .iter()
                        .map(|(key, value)| (key.as_str(), value.as_str()))
                        .collect::<Vec<_>>();
                    // Non-streaming: use a non-cancelled token for now
                    let cancel_token = tokio_util::sync::CancellationToken::new();
                    match http.post(url, headers, request, cancel_token).await {
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
                                    "OpenAI API error at {} ({}): {}",
                                    url,
                                    status,
                                    resp.body
                                ))
                            }
                        }
                        Err(e) => {
                            tracing::error!("HTTP error: {e:?}");
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

            let parsed: OpenAiResponse =
                serde_json::from_str(&response).context("Failed to parse OpenAI response")?;

            let choice = parsed.choices.into_iter().next().context("No choices")?;
            let token_logprobs = choice
                .logprobs
                .as_ref()
                .map(openai_logprobs_to_token_logprobs)
                .unwrap_or_default();

            let mut content = vec![];

            let reasoning_content = choice.message.reasoning_content;

            let text_content = choice.message.content;

            if let Some(text) = text_content {
                if !text.is_empty() {
                    content.push(ContentBlock::Text { text });
                }
            }

            if let Some(tool_calls) = choice.message.tool_calls {
                for tc in tool_calls {
                    content.push(ContentBlock::ToolUse {
                        id: tc.id,
                        name: tc.function.name.clone(),
                        input: Self::parse_tool_arguments(
                            &tc.function.name,
                            &tc.function.arguments,
                        ),
                    });
                }
            }

            let llm_response = LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content,
                    reasoning_content,
                },
                usage: TokenUsage {
                    prompt_tokens: parsed.usage.prompt_tokens,
                    completion_tokens: parsed.usage.completion_tokens,
                    total_tokens: {
                        let t = parsed.usage.total_tokens;
                        // MiniMax: fall back to total_characters when total_tokens is 0.
                        if t == 0 {
                            parsed.usage.total_characters.unwrap_or(0)
                        } else {
                            t
                        }
                    },
                    cache_read_tokens: parsed
                        .usage
                        .prompt_tokens_details
                        .as_ref()
                        .and_then(|d| d.cached_tokens),
                    cache_write_tokens: None,
                },
                stop_reason: choice.finish_reason,
                token_logprobs,
                meta: Some(LlmResponseMeta {
                    provider: Some(self.provider_name.clone()),
                    request_model: Some(self.model.clone()),
                    request_url: Some(url.clone()),
                    response_id: parsed.id,
                    response_model: parsed.model,
                    response_object: parsed.object,
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
impl LlmClient for OpenAiClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.send_request(self.build_chat_request(messages, system, tools, None))
            .await
    }

    async fn complete_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: &structured::StructuredDirective,
    ) -> Result<LlmResponse> {
        self.send_request(self.build_chat_request(messages, system, tools, Some(directive)))
            .await
    }

    fn native_structured_support(&self) -> structured::NativeStructuredSupport {
        self.native_structured_support
    }

    fn has_distinct_non_streaming_transport(&self) -> bool {
        true
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.send_streaming(
            self.build_chat_request(messages, system, tools, None),
            cancel_token,
        )
        .await
    }

    async fn complete_streaming_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: &structured::StructuredDirective,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.send_streaming(
            self.build_chat_request(messages, system, tools, Some(directive)),
            cancel_token,
        )
        .await
    }
}

#[path = "openai/streaming.rs"]
mod streaming;
use streaming::openai_logprobs_to_token_logprobs;

// OpenAI API response types (private)
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiResponse {
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) object: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    pub(crate) choices: Vec<OpenAiChoice>,
    pub(crate) usage: OpenAiUsage,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiChoice {
    pub(crate) message: OpenAiMessage,
    pub(crate) finish_reason: Option<String>,
    #[serde(default)]
    pub(crate) logprobs: Option<OpenAiChoiceLogprobs>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiChoiceLogprobs {
    #[serde(default)]
    pub(crate) content: Option<Vec<OpenAiTokenLogprob>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiTokenLogprob {
    pub(crate) token: String,
    pub(crate) logprob: f64,
    #[serde(default)]
    pub(crate) bytes: Option<Vec<u8>>,
    #[serde(default)]
    pub(crate) top_logprobs: Vec<OpenAiTopLogprob>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiTopLogprob {
    pub(crate) token: String,
    pub(crate) logprob: f64,
    #[serde(default)]
    pub(crate) bytes: Option<Vec<u8>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiMessage {
    // glm5.1 (and other GLM/zhipu reasoning models) stream/return reasoning under
    // `reasoning`, not the `reasoning_content` kimi/deepseek use. Without this alias the
    // reasoning phase yields zero recognized deltas → no ReasoningDelta events → the
    // stream-stall watchdog kills long reasoning mid-flight (asset-diagnose "未返回结构化输出").
    #[serde(alias = "reasoning")]
    pub(crate) reasoning_content: Option<String>,
    pub(crate) content: Option<String>,
    pub(crate) tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiToolCall {
    pub(crate) id: String,
    pub(crate) function: OpenAiFunction,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiFunction {
    pub(crate) name: String,
    pub(crate) arguments: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiUsage {
    #[serde(default)]
    pub(crate) prompt_tokens: usize,
    #[serde(default)]
    pub(crate) completion_tokens: usize,
    #[serde(default)]
    pub(crate) total_tokens: usize,
    /// MiniMax uses `total_characters` instead of token counts.
    #[serde(default)]
    pub(crate) total_characters: Option<usize>,
    /// OpenAI returns cached token count in `prompt_tokens_details.cached_tokens`
    #[serde(default)]
    pub(crate) prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiPromptTokensDetails {
    #[serde(default)]
    pub(crate) cached_tokens: Option<usize>,
}

// OpenAI streaming types
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiStreamChunk {
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) object: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<String>,
    pub(crate) choices: Vec<OpenAiStreamChoice>,
    pub(crate) usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiStreamChoice {
    pub(crate) message: Option<OpenAiMessage>,
    pub(crate) delta: Option<OpenAiDelta>,
    pub(crate) finish_reason: Option<String>,
    #[serde(default)]
    pub(crate) logprobs: Option<OpenAiChoiceLogprobs>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiDelta {
    // glm5.1 (and other GLM/zhipu reasoning models) stream/return reasoning under
    // `reasoning`, not the `reasoning_content` kimi/deepseek use. Without this alias the
    // reasoning phase yields zero recognized deltas → no ReasoningDelta events → the
    // stream-stall watchdog kills long reasoning mid-flight (asset-diagnose "未返回结构化输出").
    #[serde(alias = "reasoning")]
    pub(crate) reasoning_content: Option<String>,
    pub(crate) content: Option<String>,
    pub(crate) tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiToolCallDelta {
    pub(crate) index: usize,
    pub(crate) id: Option<String>,
    pub(crate) function: Option<OpenAiFunctionDelta>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiFunctionDelta {
    pub(crate) name: Option<String>,
    pub(crate) arguments: Option<String>,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "openai/tests.rs"]
mod tests;
