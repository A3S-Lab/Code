use std::path::Path;
use std::sync::Arc;

use a3s_code_core::llm::{
    default_http_client, structured, ContentBlock, HttpClient, LlmClient, LlmResponse,
    LlmResponseMeta, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const CODEX_BASE: &str = "https://chatgpt.com/backend-api/codex";
const ORIGINATOR: &str = "codex_cli_rs";
const USER_AGENT: &str = "codex_cli_rs (a3s-code-core-test)";

pub struct CodexLoginClient {
    access_token: String,
    account_id: String,
    model: String,
    session_id: String,
    http: Arc<dyn HttpClient>,
}

impl CodexLoginClient {
    pub fn from_local_login(model: &str, session_id: &str) -> Result<Self> {
        let auth_path = codex_auth_path()?;
        let raw = std::fs::read_to_string(&auth_path)
            .with_context(|| format!("read {} (run `codex login`)", auth_path.display()))?;
        let auth: Value = serde_json::from_str(&raw).context("parse ~/.codex/auth.json")?;

        let access_token = auth
            .pointer("/tokens/access_token")
            .or_else(|| auth.get("access_token"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("no access_token in ~/.codex/auth.json; run `codex login`"))?
            .to_string();

        let account_id = auth
            .pointer("/tokens/account_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                auth.pointer("/tokens/id_token")
                    .and_then(Value::as_str)
                    .and_then(account_id_from_id_token)
            })
            .ok_or_else(|| {
                anyhow!("no ChatGPT account id in ~/.codex/auth.json; re-run `codex login`")
            })?;

        Ok(Self {
            access_token,
            account_id,
            model: model.to_string(),
            session_id: session_id.to_string(),
            http: default_http_client(),
        })
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
impl LlmClient for CodexLoginClient {
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
            ("User-Agent", USER_AGENT),
        ];

        let response = self
            .http
            .post_streaming(&url, headers, &body, cancel_token)
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
        let mut stream = response.byte_stream;

        tokio::spawn(async move {
            use futures::StreamExt;

            let mut buffer = String::new();
            let mut text = String::new();
            let mut reasoning = String::new();
            let mut response_id: Option<String> = None;
            let mut usage = TokenUsage::default();
            let mut tool_calls: Vec<(String, PendingToolCall)> = Vec::new();

            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(_) => break,
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(end) = buffer.find("\n\n") {
                    let frame: String = buffer.drain(..end).collect();
                    buffer.drain(..2);
                    for event in parse_sse_frame(&frame) {
                        match event.get("type").and_then(Value::as_str).unwrap_or("") {
                            "response.created" => {
                                response_id = event
                                    .pointer("/response/id")
                                    .and_then(Value::as_str)
                                    .map(str::to_string);
                            }
                            "response.output_text.delta" => {
                                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                                    text.push_str(delta);
                                    let _ =
                                        tx.send(StreamEvent::TextDelta(delta.to_string())).await;
                                }
                            }
                            "response.reasoning_text.delta"
                            | "response.reasoning_summary_text.delta" => {
                                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                                    reasoning.push_str(delta);
                                    let _ = tx
                                        .send(StreamEvent::ReasoningDelta(delta.to_string()))
                                        .await;
                                }
                            }
                            "response.output_item.added" => {
                                let item = event.get("item");
                                if item.and_then(|i| i.get("type")).and_then(Value::as_str)
                                    == Some("function_call")
                                {
                                    let id = item_str(item, "id");
                                    let call = PendingToolCall {
                                        call_id: item_str(item, "call_id"),
                                        name: item_str(item, "name"),
                                        arguments: String::new(),
                                    };
                                    let _ = tx
                                        .send(StreamEvent::ToolUseStart {
                                            id: call.call_id.clone(),
                                            name: call.name.clone(),
                                        })
                                        .await;
                                    tool_calls.push((id, call));
                                }
                            }
                            "response.function_call_arguments.delta" => {
                                let item_id =
                                    event.get("item_id").and_then(Value::as_str).unwrap_or("");
                                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                                    if let Some((_, call)) =
                                        tool_calls.iter_mut().find(|(id, _)| id == item_id)
                                    {
                                        call.arguments.push_str(delta);
                                    }
                                    let _ = tx
                                        .send(StreamEvent::ToolUseInputDelta(delta.to_string()))
                                        .await;
                                }
                            }
                            "response.output_item.done" => {
                                let item = event.get("item");
                                if item.and_then(|i| i.get("type")).and_then(Value::as_str)
                                    == Some("function_call")
                                {
                                    let id = item_str(item, "id");
                                    let call = PendingToolCall {
                                        call_id: item_str(item, "call_id"),
                                        name: item_str(item, "name"),
                                        arguments: item_str(item, "arguments"),
                                    };
                                    if let Some((_, existing)) =
                                        tool_calls.iter_mut().find(|(stored, _)| *stored == id)
                                    {
                                        *existing = call;
                                    } else {
                                        tool_calls.push((id, call));
                                    }
                                }
                            }
                            "response.completed" => {
                                if let Some(response) = event.get("response") {
                                    if response_id.is_none() {
                                        response_id = response
                                            .get("id")
                                            .and_then(Value::as_str)
                                            .map(str::to_string);
                                    }
                                    if let Some(raw_usage) = response.get("usage") {
                                        usage = parse_usage(raw_usage);
                                    }
                                }

                                let mut content = Vec::new();
                                if !text.is_empty() {
                                    content.push(ContentBlock::Text {
                                        text: std::mem::take(&mut text),
                                    });
                                }
                                let has_tool_calls = !tool_calls.is_empty();
                                for (_, call) in tool_calls.drain(..) {
                                    content.push(ContentBlock::ToolUse {
                                        id: call.call_id,
                                        name: call.name,
                                        input: parse_tool_arguments(&call.arguments),
                                    });
                                }

                                let response = LlmResponse {
                                    message: Message {
                                        role: "assistant".to_string(),
                                        content,
                                        reasoning_content: (!reasoning.is_empty())
                                            .then(|| std::mem::take(&mut reasoning)),
                                    },
                                    usage,
                                    stop_reason: Some(
                                        if has_tool_calls { "tool_calls" } else { "stop" }
                                            .to_string(),
                                    ),
                                    token_logprobs: Vec::new(),
                                    meta: Some(LlmResponseMeta {
                                        provider: Some("codex".to_string()),
                                        request_model: Some(model.clone()),
                                        request_url: Some(url.clone()),
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

    fn native_structured_support(&self) -> structured::NativeStructuredSupport {
        structured::NativeStructuredSupport::None
    }
}

#[derive(Debug)]
struct PendingToolCall {
    call_id: String,
    name: String,
    arguments: String,
}

pub fn default_codex_model() -> String {
    std::env::var("A3S_CODEX_MODEL")
        .ok()
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| {
            codex_models_from_cache()
                .into_iter()
                .next()
                .unwrap_or_else(|| "gpt-5.5".to_string())
        })
}

fn codex_models_from_cache() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let path = Path::new(&home).join(".codex/models_cache.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(cache) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };

    let Some(models) = cache.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut listed: Vec<(i64, String)> = models
        .iter()
        .filter_map(|model| {
            if model.get("visibility").and_then(Value::as_str) != Some("list") {
                return None;
            }
            let slug = model.get("slug").and_then(Value::as_str)?.to_string();
            let priority = model.get("priority").and_then(Value::as_i64).unwrap_or(999);
            Some((priority, slug))
        })
        .collect();
    listed.sort_by_key(|(priority, _)| *priority);
    listed.into_iter().map(|(_, slug)| slug).collect()
}

fn codex_auth_path() -> Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME unset"))?;
    Ok(Path::new(&home).join(".codex/auth.json"))
}

fn parse_sse_frame(frame: &str) -> Vec<Value> {
    let mut data = String::new();
    for line in frame.lines() {
        let Some(part) = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
        else {
            continue;
        };
        if !data.is_empty() {
            data.push('\n');
        }
        data.push_str(part.trim());
    }
    if data.is_empty() || data == "[DONE]" {
        return Vec::new();
    }
    serde_json::from_str::<Value>(&data)
        .ok()
        .into_iter()
        .collect()
}

fn parse_usage(usage: &Value) -> TokenUsage {
    TokenUsage {
        prompt_tokens: usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        completion_tokens: usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        total_tokens: usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        cache_read_tokens: usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .map(|value| value as usize),
        cache_write_tokens: None,
    }
}

fn item_str(item: Option<&Value>, key: &str) -> String {
    item.and_then(|item| item.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn parse_tool_arguments(arguments: &str) -> Value {
    if arguments.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({}))
}

fn account_id_from_id_token(jwt: &str) -> Option<String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let payload = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims
        .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn convert_messages(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for message in messages {
        for block in &message.content {
            match block {
                ContentBlock::Text { text } => {
                    let text_type = if message.role == "assistant" {
                        "output_text"
                    } else {
                        "input_text"
                    };
                    out.push(json!({
                        "type": "message",
                        "role": message.role,
                        "content": [{ "type": text_type, "text": text }],
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
                            "image_url": format!("data:{};base64,{}", source.media_type, source.data),
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
