//! Codex / ChatGPT-account LLM client.
//!
//! This provider reads the local Codex CLI login (`$CODEX_HOME/auth.json` or
//! `~/.codex/auth.json`) and talks to the ChatGPT Codex Responses backend. It
//! exists because the ChatGPT-account backend uses a different wire format from
//! OpenAI chat completions.

use super::{
    default_http_client, ContentBlock, HttpClient, LlmClient, LlmResponse, LlmResponseMeta,
    Message, StreamEvent, TokenUsage, ToolDefinition,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const CODEX_BASE: &str = "https://chatgpt.com/backend-api/codex";
const ORIGINATOR: &str = "codex_cli_rs";
const UA: &str = "codex_cli_rs (a3s)";

pub fn codex_home() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Some(expand_home_path(PathBuf::from(configured)));
    }
    std::env::var_os("HOME").map(|home| Path::new(&home).join(".codex"))
}

pub fn codex_auth_path() -> Option<PathBuf> {
    codex_home().map(|home| home.join("auth.json"))
}

fn codex_models_cache_path() -> Option<PathBuf> {
    codex_home().map(|home| home.join("models_cache.json"))
}

fn expand_home_path(path: PathBuf) -> PathBuf {
    let Some(raw) = path.to_str() else {
        return path;
    };
    let Some(rest) = raw.strip_prefix("~/") else {
        return path;
    };
    std::env::var_os("HOME")
        .map(|home| Path::new(&home).join(rest))
        .unwrap_or(path)
}

/// User-facing Codex models from `models_cache.json`, ordered by priority.
pub fn codex_models() -> Vec<String> {
    fn from_cache() -> Option<Vec<String>> {
        let path = codex_models_cache_path()?;
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        let mut list: Vec<(i64, String)> = v
            .get("models")?
            .as_array()?
            .iter()
            .filter_map(|model| {
                if model.get("visibility").and_then(|value| value.as_str()) != Some("list") {
                    return None;
                }
                let slug = model
                    .get("slug")
                    .and_then(|value| value.as_str())?
                    .to_string();
                let priority = model
                    .get("priority")
                    .and_then(|value| value.as_i64())
                    .unwrap_or(999);
                Some((priority, slug))
            })
            .collect();
        list.sort_by_key(|(priority, _)| *priority);
        let out: Vec<String> = list.into_iter().map(|(_, slug)| slug).collect();
        (!out.is_empty()).then_some(out)
    }
    from_cache().unwrap_or_else(|| vec!["gpt-5.5".to_string()])
}

pub fn codex_model_context(model: &str) -> Option<u32> {
    let path = codex_models_cache_path()?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    v.get("models")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("slug").and_then(|value| value.as_str()) == Some(model))
        .and_then(parse_model_context)
}

fn parse_model_context(model: &Value) -> Option<u32> {
    const KEYS: &[&str] = &[
        "context_length",
        "max_context_length",
        "max_model_len",
        "context_window",
        "max_input_tokens",
    ];
    for key in KEYS {
        if let Some(n) = model
            .get(key)
            .and_then(|value| value.as_u64())
            .filter(|n| *n > 0)
        {
            return Some(n as u32);
        }
    }
    model
        .get("model_info")
        .and_then(|info| info.get("max_input_tokens"))
        .and_then(|value| value.as_u64())
        .filter(|n| *n > 0)
        .map(|n| n as u32)
}

pub struct CodexClient {
    access_token: String,
    account_id: String,
    model: String,
    session_id: String,
    http: Arc<dyn HttpClient>,
}

impl CodexClient {
    /// Read Codex CLI auth and bind to `model`.
    pub fn from_codex_login(model: &str, session_id: &str) -> Result<Self> {
        let path = codex_auth_path().ok_or_else(|| anyhow!("HOME unset and CODEX_HOME unset"))?;
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read {} (run `codex login`)", path.display()))?;
        let value: Value =
            serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;

        let access_token = value
            .pointer("/tokens/access_token")
            .or_else(|| value.get("access_token"))
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("no access_token in {} - run `codex login`", path.display()))?
            .to_string();

        let account_id = value
            .pointer("/tokens/account_id")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .or_else(|| {
                value
                    .pointer("/tokens/id_token")
                    .and_then(|value| value.as_str())
                    .and_then(account_id_from_id_token)
            })
            .ok_or_else(|| {
                anyhow!(
                    "no ChatGPT account id in {} - re-run `codex login`",
                    path.display()
                )
            })?;

        Ok(Self {
            access_token,
            account_id,
            model: model.to_string(),
            session_id: session_id.to_string(),
            http: default_http_client(),
        })
    }

    pub fn with_http_client(mut self, http: Arc<dyn HttpClient>) -> Self {
        self.http = http;
        self
    }

    fn build_body(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        stream: bool,
    ) -> Value {
        json!({
            "model": self.model,
            "instructions": system.unwrap_or(""),
            "input": convert_messages(messages),
            "tools": convert_tools(tools),
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "store": false,
            "stream": stream,
            "prompt_cache_key": self.session_id,
        })
    }
}

#[async_trait]
impl LlmClient for CodexClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let mut rx = self
            .complete_streaming(messages, system, tools, CancellationToken::new())
            .await?;
        while let Some(event) = rx.recv().await {
            if let StreamEvent::Done(response) = event {
                return Ok(response);
            }
        }
        Err(anyhow!("codex stream closed before response.completed"))
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let body = self.build_body(messages, system, tools, true);
        let url = format!("{CODEX_BASE}/responses");
        let bearer = format!("Bearer {}", self.access_token);
        let headers = vec![
            ("Authorization", bearer.as_str()),
            ("chatgpt-account-id", self.account_id.as_str()),
            ("OpenAI-Beta", "responses=experimental"),
            ("originator", ORIGINATOR),
            ("session_id", self.session_id.as_str()),
            ("Accept", "text/event-stream"),
            ("User-Agent", UA),
        ];

        let response = self
            .http
            .post_streaming(&url, headers, &body, cancel_token.clone())
            .await?;
        if !(200..300).contains(&response.status) {
            return Err(anyhow!(
                "codex /responses HTTP {}: {}",
                response.status,
                response.error_body
            ));
        }

        let (tx, rx) = mpsc::channel(128);
        let model = self.model.clone();
        let request_url = url.clone();
        let mut stream = response.byte_stream;

        tokio::spawn(async move {
            use futures::StreamExt;
            let mut buf = String::new();
            let mut text = String::new();
            let mut reasoning = String::new();
            let mut response_id: Option<String> = None;
            let mut usage = TokenUsage::default();
            let mut calls: Vec<(String, (String, String, String))> = Vec::new();

            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(_) => break,
                };
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = buf.find("\n\n") {
                    let frame: String = buf.drain(..end).collect();
                    buf.drain(..2);
                    for line in frame.lines() {
                        let Some(data) = line
                            .strip_prefix("data: ")
                            .or_else(|| line.strip_prefix("data:"))
                        else {
                            continue;
                        };
                        let Ok(event) = serde_json::from_str::<Value>(data.trim()) else {
                            continue;
                        };
                        match event
                            .get("type")
                            .and_then(|kind| kind.as_str())
                            .unwrap_or("")
                        {
                            "response.created" => {
                                response_id = event
                                    .pointer("/response/id")
                                    .and_then(|value| value.as_str())
                                    .map(str::to_string);
                            }
                            "response.output_text.delta" => {
                                if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                                    text.push_str(delta);
                                    let _ =
                                        tx.send(StreamEvent::TextDelta(delta.to_string())).await;
                                }
                            }
                            "response.reasoning_text.delta"
                            | "response.reasoning_summary_text.delta" => {
                                if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                                    reasoning.push_str(delta);
                                    let _ = tx
                                        .send(StreamEvent::ReasoningDelta(delta.to_string()))
                                        .await;
                                }
                            }
                            "response.output_item.added" => {
                                let item = event.get("item");
                                if item
                                    .and_then(|value| value.get("type"))
                                    .and_then(|value| value.as_str())
                                    == Some("function_call")
                                {
                                    let id = item_str(item, "id");
                                    let call_id = item_str(item, "call_id");
                                    let name = item_str(item, "name");
                                    calls
                                        .push((id, (call_id.clone(), name.clone(), String::new())));
                                    let _ = tx
                                        .send(StreamEvent::ToolUseStart { id: call_id, name })
                                        .await;
                                }
                            }
                            "response.function_call_arguments.delta" => {
                                let item_id =
                                    event.get("item_id").and_then(|v| v.as_str()).unwrap_or("");
                                if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                                    if let Some(entry) =
                                        calls.iter_mut().find(|(key, _)| key == item_id)
                                    {
                                        entry.1 .2.push_str(delta);
                                    }
                                    let _ = tx
                                        .send(StreamEvent::ToolUseInputDelta(delta.to_string()))
                                        .await;
                                }
                            }
                            "response.output_item.done" => {
                                let item = event.get("item");
                                if item
                                    .and_then(|value| value.get("type"))
                                    .and_then(|value| value.as_str())
                                    == Some("function_call")
                                {
                                    let id = item_str(item, "id");
                                    let entry = (
                                        item_str(item, "call_id"),
                                        item_str(item, "name"),
                                        item_str(item, "arguments"),
                                    );
                                    if let Some(existing) =
                                        calls.iter_mut().find(|(key, _)| *key == id)
                                    {
                                        existing.1 = entry;
                                    } else {
                                        calls.push((id, entry));
                                    }
                                }
                            }
                            "response.completed" => {
                                if let Some(raw_usage) = event.pointer("/response/usage") {
                                    usage.prompt_tokens = raw_usage
                                        .get("input_tokens")
                                        .and_then(|value| value.as_u64())
                                        .unwrap_or(0)
                                        as usize;
                                    usage.completion_tokens = raw_usage
                                        .get("output_tokens")
                                        .and_then(|value| value.as_u64())
                                        .unwrap_or(0)
                                        as usize;
                                    usage.total_tokens = raw_usage
                                        .get("total_tokens")
                                        .and_then(|value| value.as_u64())
                                        .unwrap_or(0)
                                        as usize;
                                    usage.cache_read_tokens = raw_usage
                                        .pointer("/input_tokens_details/cached_tokens")
                                        .and_then(|value| value.as_u64())
                                        .map(|value| value as usize);
                                }

                                let mut content = Vec::new();
                                if !text.is_empty() {
                                    content.push(ContentBlock::Text {
                                        text: std::mem::take(&mut text),
                                    });
                                }
                                let has_calls = !calls.is_empty();
                                for (_, (call_id, name, args)) in calls.drain(..) {
                                    content.push(ContentBlock::ToolUse {
                                        id: call_id,
                                        name,
                                        input: parse_args(&args),
                                    });
                                }
                                let response = LlmResponse {
                                    message: Message {
                                        role: "assistant".into(),
                                        content,
                                        reasoning_content: (!reasoning.is_empty())
                                            .then(|| std::mem::take(&mut reasoning)),
                                    },
                                    usage: usage.clone(),
                                    stop_reason: Some(
                                        if has_calls { "tool_calls" } else { "stop" }.into(),
                                    ),
                                    token_logprobs: Vec::new(),
                                    meta: Some(LlmResponseMeta {
                                        provider: Some("codex".into()),
                                        request_model: Some(model.clone()),
                                        request_url: Some(request_url.clone()),
                                        response_id: response_id.clone(),
                                        ..Default::default()
                                    }),
                                };
                                let _ = tx.send(StreamEvent::Done(response)).await;
                                return;
                            }
                            "response.failed" | "error" => return,
                            _ => {}
                        }
                    }
                }
            }
        });

        Ok(rx)
    }
}

fn item_str(item: Option<&Value>, key: &str) -> String {
    item.and_then(|value| value.get(key))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string()
}

fn parse_args(value: &str) -> Value {
    if value.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(value).unwrap_or_else(|_| json!({}))
}

fn account_id_from_id_token(jwt: &str) -> Option<String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let payload = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn convert_messages(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for message in messages {
        for block in &message.content {
            match block {
                ContentBlock::Text { text } => {
                    let kind = if message.role == "assistant" {
                        "output_text"
                    } else {
                        "input_text"
                    };
                    out.push(json!({
                        "type": "message",
                        "role": message.role,
                        "content": [{"type": kind, "text": text}],
                    }));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    out.push(json!({
                        "type": "function_call",
                        "name": name,
                        "arguments": input.to_string(),
                        "call_id": id,
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    out.push(json!({
                        "type": "function_call_output",
                        "call_id": tool_use_id,
                        "output": content.as_text(),
                    }));
                }
                ContentBlock::Image { source } => {
                    out.push(json!({
                        "type": "message",
                        "role": message.role,
                        "content": [{
                            "type": "input_image",
                            "image_url": format!(
                                "data:{};base64,{}",
                                source.media_type, source.data
                            ),
                        }],
                    }));
                }
            }
        }
    }
    out
}

fn convert_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_context_from_cache_shapes() {
        assert_eq!(
            parse_model_context(&json!({"context_length": 200000})),
            Some(200_000)
        );
        assert_eq!(
            parse_model_context(&json!({"model_info": {"max_input_tokens": 1000000}})),
            Some(1_000_000)
        );
        assert_eq!(parse_model_context(&json!({"context_window": 0})), None);
        assert_eq!(parse_model_context(&json!({"slug": "gpt"})), None);
    }
}
