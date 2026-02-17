//! OpenAI-compatible LLM client

use super::http::{default_http_client, normalize_base_url, HttpClient};
use super::types::*;
use super::LlmClient;
use anyhow::{Context, Result};
use async_trait::async_trait;
use crate::retry::{AttemptOutcome, RetryConfig};
use futures::StreamExt;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::Instrument;

/// OpenAI client
pub struct OpenAiClient {
    pub(crate) api_key: SecretString,
    pub(crate) model: String,
    pub(crate) base_url: String,
    pub(crate) http: Arc<dyn HttpClient>,
    pub(crate) retry_config: RetryConfig,
}

impl OpenAiClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key: SecretString::new(api_key),
            model,
            base_url: "https://api.openai.com".to_string(),
            http: default_http_client(),
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

    pub fn with_http_client(mut self, http: Arc<dyn HttpClient>) -> Self {
        self.http = http;
        self
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

            if !tools.is_empty() {
                request["tools"] = serde_json::json!(self.convert_tools(tools));
            }

            let url = format!("{}/v1/chat/completions", self.base_url);
            let auth_header = format!("Bearer {}", self.api_key.expose());
            let headers = vec![("Authorization", auth_header.as_str())];

            let response = crate::retry::with_retry(&self.retry_config, |_attempt| {
                let http = &self.http;
                let url = &url;
                let headers = headers.clone();
                let request = &request;
                async move {
                    match http.post(url, headers, request).await {
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
                        Err(e) => AttemptOutcome::Fatal(e),
                    }
                }
            })
            .await?;

            let parsed: OpenAiResponse =
                serde_json::from_str(&response).context("Failed to parse OpenAI response")?;

            let choice = parsed.choices.into_iter().next().context("No choices")?;

            let mut content = vec![];

            let reasoning_content = choice.message.reasoning_content.clone();

            let text_content = choice.message.content.or(choice.message.reasoning_content);

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
                    reasoning_content,
                },
                usage: TokenUsage {
                    prompt_tokens: parsed.usage.prompt_tokens,
                    completion_tokens: parsed.usage.completion_tokens,
                    total_tokens: parsed.usage.total_tokens,
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
        let auth_header = format!("Bearer {}", self.api_key.expose());
        let headers = vec![("Authorization", auth_header.as_str())];

        let streaming_resp = crate::retry::with_retry(&self.retry_config, |_attempt| {
            let http = &self.http;
            let url = &url;
            let headers = headers.clone();
            let request = &request;
            async move {
                match http.post_streaming(url, headers, request).await {
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
                                    "OpenAI API error at {} ({}): {}", url, status, resp.error_body
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

        let mut stream = streaming_resp.byte_stream;
        tokio::spawn(async move {
            let mut buffer = String::new();
            let mut content_blocks: Vec<ContentBlock> = Vec::new();
            let mut text_content = String::new();
            let mut reasoning_content_accum = String::new();
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

                while let Some(event_end) = buffer.find("\n\n") {
                    let event_data: String = buffer.drain(..event_end).collect();
                    buffer.drain(..2);

                    for line in event_data.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
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
                                crate::telemetry::record_llm_usage(
                                    usage.prompt_tokens,
                                    usage.completion_tokens,
                                    usage.total_tokens,
                                    finish_reason.as_deref(),
                                );
                                let response = LlmResponse {
                                    message: Message {
                                        role: "assistant".to_string(),
                                        content: std::mem::take(&mut content_blocks),
                                        reasoning_content: if reasoning_content_accum.is_empty() { None } else { Some(std::mem::take(&mut reasoning_content_accum)) },
                                    },
                                    usage: usage.clone(),
                                    stop_reason: std::mem::take(&mut finish_reason),
                                };
                                let _ = tx.send(StreamEvent::Done(response)).await;
                                continue;
                            }

                            if let Ok(event) = serde_json::from_str::<OpenAiStreamChunk>(data) {
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
                                        if let Some(ref rc) = delta.reasoning_content {
                                            reasoning_content_accum.push_str(rc);
                                        }

                                        let text_delta = delta.content
                                            .or(delta.reasoning_content);
                                        if let Some(content) = text_delta {
                                            text_content.push_str(&content);
                                            let _ = tx.send(StreamEvent::TextDelta(content)).await;
                                        }

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

// OpenAI API response types (private)
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiResponse {
    pub(crate) choices: Vec<OpenAiChoice>,
    pub(crate) usage: OpenAiUsage,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiChoice {
    pub(crate) message: OpenAiMessage,
    pub(crate) finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiMessage {
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
    pub(crate) prompt_tokens: usize,
    pub(crate) completion_tokens: usize,
    pub(crate) total_tokens: usize,
}

// OpenAI streaming types
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiStreamChunk {
    pub(crate) choices: Vec<OpenAiStreamChoice>,
    pub(crate) usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiStreamChoice {
    pub(crate) delta: Option<OpenAiDelta>,
    pub(crate) finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiDelta {
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
