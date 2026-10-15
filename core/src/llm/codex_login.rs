use std::path::PathBuf;
use std::sync::Arc;

use crate::llm::http::ReqwestHttpClient;
use crate::llm::{
    default_http_client, structured, ContentBlock, HttpClient, LlmClient, LlmResponse,
    LlmResponseMeta, Message, ModelGenerationPool, StreamEvent, TokenUsage, ToolDefinition,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const CODEX_BASE: &str = "https://chatgpt.com/backend-api/codex";
const ORIGINATOR: &str = "codex_cli_rs";
const USER_AGENT: &str = "codex_cli_rs (a3s-code-core)";

/// Codex account-backed Responses client used by local evaluation adapters.
///
/// The client reads a short-lived auth snapshot supplied by the caller and never
/// writes credentials to the workspace or session store.
pub struct CodexLoginClient {
    access_token: String,
    account_id: String,
    model: String,
    reasoning_effort: Option<String>,
    session_id: String,
    http: Arc<dyn HttpClient>,
    /// One encrypted reasoning item per assistant tool-call turn. The
    /// Responses API requires the items to be replayed in their original
    /// order when a tool result is followed by another model request.
    encrypted_reasoning: Arc<std::sync::Mutex<Vec<Option<String>>>>,
}

impl CodexLoginClient {
    /// Load the local Codex login used by developer-side evaluations.
    pub fn from_local_login(model: &str, session_id: &str) -> Result<Self> {
        let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME unset"))?;
        let auth_path = PathBuf::from(home).join(".codex/auth.json");
        let reasoning_effort = std::env::var("A3S_CODEX_REASONING_EFFORT")
            .ok()
            .filter(|effort| !effort.trim().is_empty());
        Self::from_auth_file(auth_path, model, session_id, reasoning_effort)
    }

    pub fn from_auth_file(
        auth_path: impl Into<PathBuf>,
        model: &str,
        session_id: &str,
        reasoning_effort: Option<String>,
    ) -> Result<Self> {
        let auth_path = auth_path.into();
        let raw = crate::bounded_io::read_utf8_file_bounded(
            &auth_path,
            crate::bounded_io::MAX_AUTH_FILE_BYTES,
        )
        .with_context(|| format!("read Codex auth file {}", auth_path.display()))?;
        let auth: Value = serde_json::from_str(&raw).context("parse Codex auth file")?;

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
            reasoning_effort: reasoning_effort.filter(|effort| !effort.trim().is_empty()),
            session_id: session_id.to_string(),
            http: default_http_client(),
            encrypted_reasoning: Arc::new(std::sync::Mutex::new(Vec::new())),
        })
    }

    fn build_body(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        stream: bool,
    ) -> Value {
        let encrypted_reasoning = self
            .encrypted_reasoning
            .lock()
            .ok()
            .map(|value| value.clone())
            .unwrap_or_default();
        let mut body = json!({
            "model": self.model,
            "instructions": system.unwrap_or(""),
            "input": convert_messages(messages, &encrypted_reasoning),
            "tools": convert_tools(tools),
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "store": false,
            "stream": stream,
            "prompt_cache_key": self.session_id,
        });
        if let Some(effort) = self.reasoning_effort.as_deref() {
            body["include"] = json!(["reasoning.encrypted_content"]);
            body["reasoning"] = json!({"effort": effort, "summary": "auto"});
        }
        body
    }
}

#[async_trait]
impl LlmClient for CodexLoginClient {
    fn model_generation_pool(&self) -> Option<ModelGenerationPool> {
        ModelGenerationPool::for_account_endpoint(
            "codex",
            &self.model,
            CODEX_BASE,
            &self.account_id,
            self.model_generation_concurrency(),
        )
        .ok()
    }

    fn with_active_generation_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Option<Arc<dyn LlmClient>> {
        let http = Arc::new(ReqwestHttpClient::with_timeout(timeout).ok()?);
        Some(Arc::new(Self {
            access_token: self.access_token.clone(),
            account_id: self.account_id.clone(),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            session_id: self.session_id.clone(),
            http,
            encrypted_reasoning: Arc::new(std::sync::Mutex::new(Vec::new())),
        }))
    }

    fn fork_for_session(&self, session_id: &str) -> Option<Arc<dyn LlmClient>> {
        Some(Arc::new(Self {
            access_token: self.access_token.clone(),
            account_id: self.account_id.clone(),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            session_id: session_id.to_string(),
            http: Arc::clone(&self.http),
            encrypted_reasoning: Arc::new(std::sync::Mutex::new(Vec::new())),
        }))
    }

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
        Err(anyhow!("codex stream closed before a terminal response"))
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
            // Authentication, authorization, request-shape, and billing
            // failures cannot be repaired by replaying the same Responses
            // request. Preserve the status as a typed terminal error so the
            // outer Agent loop does not turn a 402/401 into repeated calls.
            if matches!(response.status, 400..=404) {
                return Err(anyhow::Error::new(
                    crate::llm::NonRetryableLlmError::from_status(
                        "codex",
                        response.status,
                        response.error_body,
                    ),
                ));
            }
            return Err(anyhow!(
                "codex /responses HTTP {}: {}",
                response.status,
                response.error_body
            ));
        }

        let (tx, rx) = mpsc::channel(128);
        let model = self.model.clone();
        let encrypted_reasoning = Arc::clone(&self.encrypted_reasoning);
        let mut stream = response.byte_stream;

        tokio::spawn(async move {
            use futures::StreamExt;

            let mut buffer = String::new();
            let mut text = String::new();
            let mut reasoning = String::new();
            let mut response_id: Option<String> = None;
            let mut usage = TokenUsage::default();
            let mut tool_calls: Vec<(String, PendingToolCall)> = Vec::new();
            let mut response_encrypted_reasoning: Option<String> = None;

            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(_) => break,
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Same blank-line splitter as the other model streams. A
                // trailing CR is not a frame: the next chunk may complete CRLF.
                for frame in crate::sse::take_sse_events(&mut buffer, false) {
                    for event in parse_sse_frame(&frame) {
                        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
                        match event_type {
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
                                    let tool_id = if let Some((_, call)) =
                                        tool_calls.iter_mut().find(|(id, _)| id == item_id)
                                    {
                                        call.arguments.push_str(delta);
                                        Some(call.call_id.clone())
                                    } else {
                                        None
                                    };
                                    let _ = tx
                                        .send(StreamEvent::ToolUseInputDelta {
                                            id: tool_id,
                                            delta: delta.to_string(),
                                        })
                                        .await;
                                }
                            }
                            "response.output_item.done" => {
                                let item = event.get("item");
                                if item
                                    .and_then(|item| item.get("type"))
                                    .and_then(Value::as_str)
                                    == Some("reasoning")
                                {
                                    if let Some(value) = item
                                        .and_then(|item| item.get("encrypted_content"))
                                        .and_then(Value::as_str)
                                    {
                                        response_encrypted_reasoning = Some(value.to_string());
                                    }
                                }
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
                            "response.completed" | "response.incomplete" => {
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
                                if has_tool_calls {
                                    if let Ok(mut stored) = encrypted_reasoning.lock() {
                                        stored.push(response_encrypted_reasoning.take());
                                    }
                                }
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
                                        transcript_text: None,
                                        transcript_visibility: Default::default(),
                                    },
                                    usage,
                                    stop_reason: Some(
                                        if has_tool_calls {
                                            "tool_calls"
                                        } else if event_type == "response.incomplete" {
                                            "length"
                                        } else {
                                            "stop"
                                        }
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

fn convert_messages(messages: &[Message], encrypted_reasoning: &[Option<String>]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut reasoning = encrypted_reasoning.iter();
    for message in messages {
        if message.role == "assistant"
            && message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
        {
            if let Some(Some(encrypted_content)) = reasoning.next() {
                out.push(json!({
                    "type": "reasoning",
                    "encrypted_content": encrypted_content,
                }));
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replays_encrypted_reasoning_in_tool_turn_order() {
        let messages = vec![
            Message::user("inspect the workspace"),
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "call-1".to_string(),
                    name: "bash".to_string(),
                    input: json!({"command": "pwd"}),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: Default::default(),
            },
            Message::tool_result("call-1", "workspace", false),
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "call-2".to_string(),
                    name: "read".to_string(),
                    input: json!({"path": "README.md"}),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: Default::default(),
            },
            Message::tool_result("call-2", "contents", false),
        ];

        let converted = convert_messages(
            &messages,
            &[
                Some("encrypted-1".to_string()),
                Some("encrypted-2".to_string()),
            ],
        );
        let reasoning = converted
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
            .map(|item| item["encrypted_content"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(reasoning, ["encrypted-1", "encrypted-2"]);
    }

    fn codex_hello_sse(separator: &str) -> String {
        let delta = r#"{"type":"response.output_text.delta","delta":"hello"}"#;
        let done = r#"{"type":"response.completed","response":{"id":"resp-1","usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#;
        format!("data: {delta}{separator}{separator}data: {done}{separator}{separator}")
    }

    fn codex_client(http: Arc<dyn HttpClient>) -> CodexLoginClient {
        CodexLoginClient {
            access_token: "token".to_string(),
            account_id: "acct".to_string(),
            model: "gpt-test".to_string(),
            reasoning_effort: None,
            session_id: "sess".to_string(),
            http,
            encrypted_reasoning: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    struct ScriptedCodexSse {
        incoming:
            std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Option<bytes::Bytes>>>>,
        parked: std::sync::Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl HttpClient for ScriptedCodexSse {
        async fn post(
            &self,
            _url: &str,
            _headers: Vec<(&str, &str)>,
            _body: &Value,
            _cancel: CancellationToken,
        ) -> Result<crate::llm::HttpResponse> {
            anyhow::bail!("post is unused in the scripted codex test")
        }

        async fn post_streaming(
            &self,
            _url: &str,
            _headers: Vec<(&str, &str)>,
            _body: &Value,
            _cancel: CancellationToken,
        ) -> Result<crate::llm::StreamingHttpResponse> {
            let rx = self
                .incoming
                .lock()
                .expect("scripted sse lock")
                .take()
                .expect("one scripted stream");
            let parked = std::sync::Arc::clone(&self.parked);
            let byte_stream =
                futures::stream::unfold((rx, parked, false), |(mut rx, parked, seen)| async move {
                    if seen {
                        parked.notify_one();
                    }
                    match rx.recv().await {
                        Some(Some(bytes)) => {
                            Some((Ok::<bytes::Bytes, anyhow::Error>(bytes), (rx, parked, true)))
                        }
                        _ => None,
                    }
                });
            Ok(crate::llm::StreamingHttpResponse {
                status: 200,
                retry_after: None,
                byte_stream: Box::pin(byte_stream),
                error_body: String::new(),
            })
        }
    }

    struct ChunkCodexSse {
        chunks: Vec<bytes::Bytes>,
    }

    #[async_trait]
    impl HttpClient for ChunkCodexSse {
        async fn post(
            &self,
            _url: &str,
            _headers: Vec<(&str, &str)>,
            _body: &Value,
            _cancel: CancellationToken,
        ) -> Result<crate::llm::HttpResponse> {
            anyhow::bail!("post is unused in the codex frame test")
        }

        async fn post_streaming(
            &self,
            _url: &str,
            _headers: Vec<(&str, &str)>,
            _body: &Value,
            _cancel: CancellationToken,
        ) -> Result<crate::llm::StreamingHttpResponse> {
            let items = self
                .chunks
                .iter()
                .cloned()
                .map(Ok::<bytes::Bytes, anyhow::Error>)
                .collect::<Vec<_>>();
            Ok(crate::llm::StreamingHttpResponse {
                status: 200,
                retry_after: None,
                byte_stream: Box::pin(futures::stream::iter(items)),
                error_body: String::new(),
            })
        }
    }

    async fn codex_text_deltas(wire: String) -> (Vec<String>, LlmResponse) {
        let client = codex_client(Arc::new(ChunkCodexSse {
            chunks: vec![bytes::Bytes::from(wire)],
        }));
        let mut rx = client
            .complete_streaming(&[Message::user("go")], None, &[], CancellationToken::new())
            .await
            .expect("stream opened");
        let mut deltas = Vec::new();
        let mut done = None;
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::TextDelta(delta) => deltas.push(delta),
                StreamEvent::Done(response) => {
                    done = Some(response);
                    break;
                }
                _ => {}
            }
        }
        (deltas, done.expect("Done"))
    }

    #[tokio::test]
    async fn codex_login_crlf_data_frame_matches_lf_payload() {
        let (lf_deltas, lf) = codex_text_deltas(codex_hello_sse("\n")).await;
        let (crlf_deltas, crlf) = codex_text_deltas(codex_hello_sse("\r\n")).await;
        assert_eq!(lf_deltas, vec!["hello".to_string()]);
        assert_eq!(crlf_deltas, lf_deltas);
        assert_eq!(lf.text(), "hello");
        assert_eq!(crlf.text(), lf.text());
        assert!(!crlf.text().contains('\r'));
    }

    #[tokio::test]
    async fn codex_login_split_cr_does_not_emit_partial_event() {
        let wire = codex_hello_sse("\r\n");
        let blank = wire.find("\r\n\r\n").expect("crlf blank line");
        let head = wire[..blank + 3].to_string();
        assert!(head.ends_with('\r'), "the held suffix is a bare CR");
        let tail = wire[blank + 3..].to_string();

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let parked = std::sync::Arc::new(tokio::sync::Notify::new());
        let client = codex_client(Arc::new(ScriptedCodexSse {
            incoming: std::sync::Mutex::new(Some(rx)),
            parked: std::sync::Arc::clone(&parked),
        }));
        let mut events = client
            .complete_streaming(&[Message::user("go")], None, &[], CancellationToken::new())
            .await
            .expect("stream opened");

        tx.send(Some(bytes::Bytes::from(head)))
            .expect("reader accepts the partial frame");
        tokio::time::timeout(std::time::Duration::from_secs(2), parked.notified())
            .await
            .expect("reader parked while the buffer ends on CR");
        assert!(
            matches!(
                events.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "a buffer ending on CR must not emit a partial event"
        );

        tx.send(Some(bytes::Bytes::from(tail)))
            .expect("reader accepts the rest of the frame");
        tx.send(None).expect("close the scripted stream");
        let mut deltas = Vec::new();
        let mut done = None;
        let finished = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Some(event) = events.recv().await {
                match event {
                    StreamEvent::TextDelta(delta) => deltas.push(delta),
                    StreamEvent::Done(response) => {
                        done = Some(response);
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await;
        assert!(finished.is_ok(), "the completed CRLF frame must finish");
        assert_eq!(deltas, vec!["hello".to_string()]);
        let response = done.expect("Done");
        assert_eq!(response.text(), "hello");
        assert!(!response.text().contains('\r'));
    }
}
