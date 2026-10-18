//! Cursor account completion client.
//!
//! Speaks Connect `AgentService/Run` on the account endpoint
//! (`https://api2.cursor.sh`, or the provider base URL). The session bearer
//! is the `Authorization` header. This client does not launch `agent` or
//! `cursor-agent`, and it does not execute `execServerMessage`.
//!
//! `Run` uses a duplex HTTP/2 stream: acknowledge conversation blobs in
//! bounded, request-local memory while A3S alone executes presented tools.
//! `RunSSE` only heartbeats for this account. The Run stream stays open
//! after the assistant text (heartbeats and `kvServerMessage`, often with
//! no `turnEnded`), so a quiet gap after text completes the turn.
//!
//! The session selects a catalog id. `GetUsableModels` is the account's
//! model list: each entry's `modelId` is what `Run` accepts, and
//! `displayModelId`, `displayName`, `displayNameShort`, and `aliases` are
//! selectors for that same entry. Auto is `displayModelId` `auto` and
//! `modelId` `default`. A catalog lookup that fails leaves the selected
//! id unchanged.

mod context;
mod kv;
mod tool_protocol;

use super::http::{HttpClient, StreamingHttpResponse};
use super::types::{LlmResponse, LlmResponseMeta, Message, TokenUsage};
use super::{LlmClient, StreamEvent};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const RUN_PATH: &str = "/agent.v1.AgentService/Run";
const USABLE_MODELS_PATH: &str = "/agent.v1.AgentService/GetUsableModels";
const DEFAULT_ENDPOINT: &str = "https://api2.cursor.sh";
const GENERATION_DEADLINE: Duration = Duration::from_secs(90);
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
/// The Run stream keeps sending heartbeats after the answer. Return once
/// assistant text has been quiet for this long.
const QUIET_AFTER_TEXT: Duration = Duration::from_millis(2_500);

static MESSAGE_IDS: AtomicU64 = AtomicU64::new(1);

pub(crate) struct CursorAccountClient {
    api_key: String,
    model: String,
    base_url: String,
    headers: HashMap<String, String>,
    http: Arc<dyn HttpClient>,
    /// `GetUsableModels` alias map. `None` until a successful lookup.
    catalog: Mutex<Option<HashMap<String, String>>>,
}

impl CursorAccountClient {
    pub(crate) fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            base_url: DEFAULT_ENDPOINT.to_string(),
            headers: HashMap::new(),
            http: super::http::default_http_client(),
            catalog: Mutex::new(None),
        }
    }

    pub(crate) fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url =
            account_origin(&base_url).unwrap_or_else(|| base_url.trim_end_matches('/').to_string());
        self
    }

    pub(crate) fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    pub(crate) fn with_http_client(mut self, http: Arc<dyn HttpClient>) -> Self {
        self.http = http;
        self
    }

    fn endpoint(&self) -> String {
        format!("{}{RUN_PATH}", self.base_url.trim_end_matches('/'))
    }

    fn usable_models_url(&self) -> String {
        format!(
            "{}{USABLE_MODELS_PATH}",
            self.base_url.trim_end_matches('/')
        )
    }

    /// `modelId` from the account catalog when `self.model` is one of that
    /// entry's selectors. A failed lookup returns `self.model`.
    async fn wire_model_id(&self, cancel_token: CancellationToken) -> String {
        if let Some(resolved) = self.cached_model_id() {
            return resolved;
        }
        let Some(map) = self.fetch_account_models(cancel_token).await else {
            return self.model.clone();
        };
        let resolved = resolve_model_id(&map, &self.model);
        *self
            .catalog
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(map);
        resolved
    }

    fn cached_model_id(&self) -> Option<String> {
        self.catalog
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .map(|map| resolve_model_id(map, &self.model))
    }

    async fn fetch_account_models(
        &self,
        cancel_token: CancellationToken,
    ) -> Option<HashMap<String, String>> {
        let url = self.usable_models_url();
        let owned_headers = self.catalog_headers();
        let header_refs: Vec<(&str, &str)> = owned_headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let response = self
            .http
            .post(
                &url,
                header_refs,
                &json!({ "customModelIds": [] }),
                cancel_token,
            )
            .await
            .ok()?;
        if !(200..300).contains(&response.status) {
            return None;
        }
        let value = serde_json::from_str::<Value>(&response.body).ok()?;
        Some(account_model_alias_map(&value))
    }

    fn catalog_headers(&self) -> Vec<(String, String)> {
        let mut headers = self.request_headers();
        for (key, value) in &mut headers {
            if key.eq_ignore_ascii_case("content-type") {
                *value = "application/json".to_string();
            }
        }
        headers
    }

    fn request_headers(&self) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        let has_authorization = self
            .headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("authorization"));
        if !has_authorization {
            headers.push((
                "Authorization".to_string(),
                format!("Bearer {}", self.api_key),
            ));
        }
        headers.push((
            "Content-Type".to_string(),
            "application/connect+json".to_string(),
        ));
        headers.push(("Connect-Protocol-Version".to_string(), "1".to_string()));
        if !self
            .headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("x-cursor-client-type"))
        {
            headers.push(("x-cursor-client-type".to_string(), "cli".to_string()));
        }
        if !self
            .headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("x-cursor-client-version"))
        {
            headers.push(("x-cursor-client-version".to_string(), "cli".to_string()));
        }
        if !self
            .headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("x-ghost-mode"))
        {
            headers.push(("x-ghost-mode".to_string(), "true".to_string()));
        }
        headers.push(("x-cursor-streaming".to_string(), "true".to_string()));
        for (key, value) in &self.headers {
            if key.eq_ignore_ascii_case("content-type") {
                continue;
            }
            headers.push((key.clone(), value.clone()));
        }
        headers
    }

    async fn perform(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[super::types::ToolDefinition],
        cancel_token: CancellationToken,
    ) -> Result<LlmResponse> {
        let url = self.endpoint();
        if url.contains("/v1/chat/completions") {
            return Err(anyhow!(
                "cursor account client refused a chat-completions URL"
            ));
        }
        let prompt = tool_protocol::prompt(messages, system, tools)?;
        if prompt.trim().is_empty() {
            return Err(anyhow!("cursor account request has no user text"));
        }
        let model = self.wire_model_id(cancel_token.clone()).await;
        if cancel_token.is_cancelled() {
            return Err(anyhow!("cursor account request was cancelled"));
        }
        let body = connect_envelope(&run_request_json(&model, &prompt));
        let owned_headers = self.request_headers();
        let header_refs: Vec<(&str, &str)> = owned_headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let (replies, reply_rx) = mpsc::channel(32);
        let response = self
            .http
            .post_bytes_duplex(&url, header_refs, &body, reply_rx, cancel_token.clone())
            .await?;
        let text = read_assistant_text(response, &self.api_key, cancel_token, replies).await?;
        if text.trim().is_empty() {
            return Err(anyhow!("cursor account returned no assistant text"));
        }
        let message = tool_protocol::message(&text, tools)?;
        let stop_reason = if message.tool_calls().is_empty() {
            "end_turn"
        } else {
            "tool_use"
        };
        Ok(LlmResponse {
            message,
            usage: TokenUsage::default(),
            stop_reason: Some(stop_reason.to_string()),
            token_logprobs: Vec::new(),
            meta: Some(LlmResponseMeta {
                provider: Some("cursor".to_string()),
                request_model: Some(model),
                request_url: Some(url),
                ..LlmResponseMeta::default()
            }),
        })
    }
}

fn account_origin(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
        return None;
    }
    let mut endpoint = format!("{}://{}", url.scheme(), url.host_str()?);
    if let Some(port) = url.port() {
        endpoint.push(':');
        endpoint.push_str(&port.to_string());
    }
    Some(endpoint)
}

fn next_id() -> String {
    format!("a3s-{}", MESSAGE_IDS.fetch_add(1, Ordering::Relaxed))
}

fn run_request_json(model: &str, prompt: &str) -> Vec<u8> {
    let body = json!({
        "runRequest": {
            "action": {
                "userMessageAction": {
                    "userMessage": {
                        "text": prompt,
                        "messageId": next_id(),
                        "mode": "ASK"
                    },
                    "requestContext": context::empty()
                }
            },
            "modelDetails": { "modelId": model },
            "requestedModel": { "modelId": model },
            "conversationId": next_id(),
            "conversationState": {}
        }
    });
    serde_json::to_vec(&body).unwrap_or_default()
}

/// Selectors from one `GetUsableModels` entry, lowercased, mapped to that
/// entry's `modelId`. Later entries overwrite an earlier selector.
fn account_model_alias_map(value: &Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(models) = value.get("models").and_then(Value::as_array) else {
        return map;
    };
    for entry in models {
        let Some(model_id) = text_field(entry, "modelId") else {
            continue;
        };
        insert_model_alias(&mut map, &model_id, &model_id);
        for field in ["displayModelId", "displayName", "displayNameShort"] {
            if let Some(selector) = text_field(entry, field) {
                insert_model_alias(&mut map, &selector, &model_id);
            }
        }
        if let Some(aliases) = entry.get("aliases").and_then(Value::as_array) {
            for alias in aliases {
                if let Some(alias) = alias
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    insert_model_alias(&mut map, alias, &model_id);
                }
            }
        }
    }
    map
}

fn text_field(entry: &Value, field: &str) -> Option<String> {
    entry
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn insert_model_alias(map: &mut HashMap<String, String>, selector: &str, model_id: &str) {
    let key = selector.trim().to_ascii_lowercase();
    if key.is_empty() {
        return;
    }
    map.insert(key, model_id.to_string());
}

fn resolve_model_id(map: &HashMap<String, String>, requested: &str) -> String {
    let key = requested.trim().to_ascii_lowercase();
    map.get(&key)
        .filter(|id| !id.is_empty())
        .cloned()
        .unwrap_or_else(|| requested.to_string())
}

fn connect_envelope(json: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + json.len());
    out.push(0);
    let len = u32::try_from(json.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(json);
    out
}

enum FrameControl {
    Continue,
    Done,
    Failed(anyhow::Error),
}

fn pop_frame(buf: &mut Vec<u8>) -> Result<Option<(bool, Vec<u8>)>> {
    if buf.len() < 5 {
        return Ok(None);
    }
    let flags = buf[0];
    let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(anyhow!(
            "cursor account response frame exceeds the read bound"
        ));
    }
    if buf.len() < 5 + len {
        return Ok(None);
    }
    if flags & 0x01 != 0 {
        return Err(anyhow!("cursor account sent a compressed envelope"));
    }
    let payload = buf[5..5 + len].to_vec();
    buf.drain(..5 + len);
    Ok(Some((flags & 0x02 != 0, payload)))
}

fn apply_payload(payload: &[u8], text: &mut String) -> FrameControl {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return FrameControl::Continue;
    };
    if let Some(error) = value.get("error") {
        let code = error.get("code").and_then(Value::as_str).unwrap_or("error");
        return FrameControl::Failed(anyhow!("cursor account error: {code}"));
    }
    if value.get("execServerMessage").is_some() {
        return FrameControl::Failed(anyhow!(super::NonRetryableLlmError::new(
            "Cursor requested its native tools instead of an A3S tool response; select another Cursor model"
        )));
    }
    // `kvServerMessage`, heartbeats, and other non-text frames are not
    // assistant text and are not executed.
    let Some(update) = value.get("interactionUpdate") else {
        return FrameControl::Continue;
    };
    if let Some(delta) = update.get("textDelta") {
        let notice = delta
            .get("isServerNotice")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !notice {
            if let Some(piece) = delta.get("text").and_then(Value::as_str) {
                text.push_str(piece);
            }
        }
    }
    if update.get("turnEnded").is_some() {
        FrameControl::Done
    } else {
        FrameControl::Continue
    }
}

async fn read_assistant_text(
    response: StreamingHttpResponse,
    secret: &str,
    cancel_token: CancellationToken,
    replies: mpsc::Sender<Vec<u8>>,
) -> Result<String> {
    if !(200..300).contains(&response.status) {
        return Err(anyhow!(
            "cursor account endpoint returned HTTP {} ({})",
            response.status,
            redact(&response.error_body, secret)
        ));
    }
    let mut stream = response.byte_stream;
    let mut buf = Vec::new();
    let mut text = String::new();
    let mut last_text: Option<Instant> = None;
    let mut blobs = kv::Store::default();
    let deadline = Instant::now() + GENERATION_DEADLINE;
    loop {
        if quiet_gap_elapsed(&text, last_text) {
            return finish_text(text);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let wait = next_wait(&text, last_text, remaining);
        let next = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                return Err(anyhow!("cursor account request was cancelled"));
            }
            item = tokio::time::timeout(wait, stream.next()) => item,
        };
        let chunk = match next {
            Err(_) => {
                if !text.trim().is_empty() {
                    return finish_text(text);
                }
                break;
            }
            Ok(None) => break,
            Ok(Some(Err(error))) => return Err(error),
            Ok(Some(Ok(chunk))) => chunk,
        };
        buf.extend_from_slice(&chunk);
        loop {
            match pop_frame(&mut buf)? {
                None => break,
                Some((end, payload)) => {
                    let reply = match blobs.reply(&payload)? {
                        Some(reply) => Some(reply),
                        None => context::reply(&payload)?,
                    };
                    if let Some(reply) = reply {
                        tokio::select! {
                            biased;
                            _ = cancel_token.cancelled() => return Err(anyhow!("cursor account request was cancelled")),
                            result = replies.send(connect_envelope(&reply)) => {
                                result.map_err(|_| anyhow!("cursor account duplex request closed before a protocol reply"))?;
                            }
                        }
                        continue;
                    }
                    let before = text.len();
                    let control = apply_payload(&payload, &mut text);
                    if text.len() > MAX_FRAME_BYTES {
                        return Err(anyhow!(
                            "cursor account assistant text exceeds the read bound"
                        ));
                    }
                    match control {
                        FrameControl::Continue if end => {
                            return finish_text(text);
                        }
                        FrameControl::Continue => {
                            if text.len() > before {
                                last_text = Some(Instant::now());
                            }
                        }
                        FrameControl::Done => return finish_text(text),
                        FrameControl::Failed(error) => return Err(error),
                    }
                }
            }
        }
    }
    finish_text(text)
}

fn quiet_gap_elapsed(text: &str, last_text: Option<Instant>) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    last_text
        .map(|at| at.elapsed() >= QUIET_AFTER_TEXT)
        .unwrap_or(true)
}

fn next_wait(text: &str, last_text: Option<Instant>, remaining: Duration) -> Duration {
    if text.trim().is_empty() {
        return remaining;
    }
    let Some(at) = last_text else {
        return Duration::ZERO;
    };
    QUIET_AFTER_TEXT.saturating_sub(at.elapsed()).min(remaining)
}

fn finish_text(text: String) -> Result<String> {
    if text.trim().is_empty() {
        Err(anyhow!("cursor account returned no assistant text"))
    } else {
        Ok(text)
    }
}

fn redact(body: &str, secret: &str) -> String {
    if !secret.is_empty() && body.contains(secret) {
        return "response body redacted".to_string();
    }
    body.chars().take(180).collect()
}

#[async_trait]
impl LlmClient for CursorAccountClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[super::types::ToolDefinition],
    ) -> Result<LlmResponse> {
        self.perform(messages, system, tools, CancellationToken::new())
            .await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[super::types::ToolDefinition],
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let response = self.perform(messages, system, tools, cancel_token).await?;
        let (tx, rx) = mpsc::channel(4);
        let text = response.text();
        let _ = tx.try_send(StreamEvent::TextDelta(text));
        let _ = tx.try_send(StreamEvent::Done(response));
        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::super::http::{HttpResponse, StreamingHttpResponse};
    use super::*;
    use crate::llm::create_client_with_config;
    use crate::llm::LlmConfig;
    use std::sync::Mutex;

    const BEARER: &str = "cursor-fixture-bearer-9f3a7c";

    struct Record {
        url: Mutex<String>,
        headers: Mutex<Vec<(String, String)>>,
        body: Mutex<Vec<u8>>,
        json_posts: Mutex<u32>,
        catalog_urls: Mutex<Vec<String>>,
    }

    struct FixtureHttp {
        record: Arc<Record>,
        response: Vec<u8>,
        /// Heartbeats after the first chunk. The stream then stays open.
        heartbeats_after: u32,
        /// `GetUsableModels` body. `None` fails the lookup.
        catalog: Option<String>,
    }

    fn empty_record() -> Arc<Record> {
        Arc::new(Record {
            url: Mutex::new(String::new()),
            headers: Mutex::new(Vec::new()),
            body: Mutex::new(Vec::new()),
            json_posts: Mutex::new(0),
            catalog_urls: Mutex::new(Vec::new()),
        })
    }

    #[async_trait]
    impl HttpClient for FixtureHttp {
        async fn post(
            &self,
            url: &str,
            _headers: Vec<(&str, &str)>,
            _body: &Value,
            _cancel_token: CancellationToken,
        ) -> Result<HttpResponse> {
            *self
                .record
                .json_posts
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) += 1;
            self.record
                .catalog_urls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(url.to_string());
            let Some(body) = self.catalog.clone() else {
                return Err(anyhow!("cursor fixture refused a JSON chat post"));
            };
            Ok(HttpResponse {
                status: 200,
                body,
                retry_after: None,
            })
        }

        async fn post_streaming(
            &self,
            _url: &str,
            _headers: Vec<(&str, &str)>,
            _body: &Value,
            _cancel_token: CancellationToken,
        ) -> Result<StreamingHttpResponse> {
            *self
                .record
                .json_posts
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) += 1;
            Err(anyhow!("cursor fixture refused a JSON chat stream"))
        }

        async fn post_bytes_streaming(
            &self,
            url: &str,
            headers: Vec<(&str, &str)>,
            body: &[u8],
            _cancel_token: CancellationToken,
        ) -> Result<StreamingHttpResponse> {
            *self
                .record
                .url
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = url.to_string();
            *self
                .record
                .headers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = headers
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            *self
                .record
                .body
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = body.to_vec();
            let first = std::sync::Arc::new(self.response.clone());
            let heartbeats = self.heartbeats_after;
            let stream = futures::stream::unfold(0u32, move |step| {
                let first = std::sync::Arc::clone(&first);
                async move {
                    match step {
                        0 => Some((Ok(bytes::Bytes::from((*first).clone())), 1u32)),
                        n if n <= heartbeats => {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            Some((Ok(bytes::Bytes::from(heartbeat_frame())), n + 1))
                        }
                        _ => {
                            std::future::pending::<()>().await;
                            None
                        }
                    }
                }
            });
            Ok(StreamingHttpResponse {
                status: 200,
                retry_after: None,
                byte_stream: Box::pin(stream),
                error_body: String::new(),
            })
        }

        async fn post_bytes_duplex(
            &self,
            url: &str,
            headers: Vec<(&str, &str)>,
            body: &[u8],
            _replies: mpsc::Receiver<Vec<u8>>,
            cancel_token: CancellationToken,
        ) -> Result<StreamingHttpResponse> {
            self.post_bytes_streaming(url, headers, body, cancel_token)
                .await
        }
    }

    fn heartbeat_frame() -> Vec<u8> {
        envelope(r#"{"interactionUpdate":{"heartbeat":{}}}"#)
    }

    fn envelope(json: &str) -> Vec<u8> {
        let bytes = json.as_bytes();
        let mut out = vec![0];
        out.extend_from_slice(&(u32::try_from(bytes.len()).unwrap()).to_be_bytes());
        out.extend_from_slice(bytes);
        out
    }

    fn fixture_body() -> Vec<u8> {
        let mut body = envelope(
            r#"{"interactionUpdate":{"textDelta":{"text":"fixture-assistant-answer","isServerNotice":false}}}"#,
        );
        body.extend(envelope(r#"{"interactionUpdate":{"heartbeat":{}}}"#));
        body.extend(envelope(
            r#"{"interactionUpdate":{"textDelta":{"text":" ignore-me","isServerNotice":true}}}"#,
        ));
        body.extend(envelope(r#"{"conversationCheckpointUpdate":{}}"#));
        body.extend(envelope(r#"{"interactionUpdate":{"turnEnded":{}}}"#));
        body.extend(envelope(
            r#"{"execServerMessage":{"shell":{"command":"rm -rf ."}}}"#,
        ));
        body
    }

    fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    async fn complete_against(base_url: Option<&str>, record: Arc<Record>) -> LlmResponse {
        let http = Arc::new(FixtureHttp {
            record,
            response: fixture_body(),
            heartbeats_after: 0,
            catalog: None,
        });
        let mut config = LlmConfig::new("cursor", "composer-2.5", BEARER).with_http_client(http);
        if let Some(base_url) = base_url {
            config = config.with_base_url(base_url);
        }
        let client = create_client_with_config(config);
        client
            .complete(
                &[
                    Message::user("earlier turn"),
                    Message::assistant("earlier answer"),
                    Message::user("say fixture"),
                ],
                Some("stay in text"),
                &[],
            )
            .await
            .expect("fixture completion")
    }

    #[tokio::test]
    async fn fixture_response_parses_assistant_text_from_the_account_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("spawned");
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("agent");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        let other = bin.join("cursor-agent");
        std::fs::write(&other, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&script, mode.clone()).unwrap();
            std::fs::set_permissions(&other, mode).unwrap();
        }
        struct RestorePath(Option<std::ffi::OsString>);
        impl Drop for RestorePath {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(path) => std::env::set_var("PATH", path),
                    None => std::env::remove_var("PATH"),
                }
            }
        }
        let saved_path = std::env::var_os("PATH");
        let mut paths = vec![bin.clone()];
        paths.extend(std::env::split_paths(
            &saved_path.clone().unwrap_or_default(),
        ));
        let path = std::env::join_paths(&paths).unwrap();
        std::env::set_var("PATH", path);
        let _restore_path = RestorePath(saved_path);
        let record = empty_record();
        let response = complete_against(None, record.clone()).await;
        drop(_restore_path);
        assert_eq!(response.text(), "fixture-assistant-answer");
        assert!(!response.text().contains("not-assistant-text"));
        assert!(!response.text().contains("ignore-me"));
        assert!(!marker.exists(), "completion must not spawn agent");
        let catalog_urls = record
            .catalog_urls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        assert_eq!(catalog_urls.len(), 1);
        assert!(
            catalog_urls[0].ends_with(USABLE_MODELS_PATH),
            "catalog url {}",
            catalog_urls[0]
        );
        assert!(!catalog_urls[0].contains("chat/completions"));
        let url = record
            .url
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("api2.cursor.sh"));
        assert_eq!(parsed.path(), RUN_PATH);
        assert!(!url.contains("chat/completions"));
        let headers = record
            .headers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        assert_eq!(
            header_value(&headers, "authorization").as_deref(),
            Some(format!("Bearer {BEARER}").as_str())
        );
        assert_eq!(
            header_value(&headers, "content-type").as_deref(),
            Some("application/connect+json")
        );
        let body = record
            .body
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        assert!(body.len() > 5);
        let payload = &body[5..];
        let value: Value = serde_json::from_slice(payload).unwrap();
        let text = value
            .pointer("/runRequest/action/userMessageAction/userMessage/text")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert!(text.contains("say fixture"));
        assert!(text.contains("earlier turn"));
        assert!(text.contains("stay in text"));
        assert_eq!(
            value.pointer("/runRequest/modelDetails/modelId"),
            Some(&Value::String("composer-2.5".into()))
        );
        assert_eq!(
            value.pointer("/runRequest/action/userMessageAction/userMessage/mode"),
            Some(&Value::String("ASK".into()))
        );
        assert_eq!(
            value.pointer("/runRequest/action/userMessageAction/requestContext/envInfoComplete"),
            Some(&Value::Bool(true))
        );
        let rendered = serde_json::to_string(&value).unwrap();
        assert!(!rendered.contains("rm -rf"));
        assert!(!rendered.contains("cursor-agent"));
        assert!(!rendered.contains("excludeWorkspaceContext"));
        assert!(!rendered.contains("customSystemPrompt"));
    }

    #[tokio::test]
    async fn endpoint_override_replaces_the_account_host() {
        let record = empty_record();
        let response =
            complete_against(Some("https://override.cursor.example/v1"), record.clone()).await;
        assert_eq!(response.text(), "fixture-assistant-answer");
        let url = record
            .url
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let parsed = url::Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("override.cursor.example"));
        assert_eq!(parsed.path(), RUN_PATH);
        assert!(!url.contains("chat/completions"));
    }

    #[tokio::test]
    async fn open_stream_returns_text_after_a_quiet_gap_without_turn_ended() {
        let record = empty_record();
        let http = Arc::new(FixtureHttp {
            record,
            response: envelope(
                r#"{"interactionUpdate":{"textDelta":{"text":"fixture-assistant-answer"}}}"#,
            ),
            heartbeats_after: 3,
            catalog: None,
        });
        let config = LlmConfig::new("cursor", "gpt-5.2", BEARER).with_http_client(http);
        let client = create_client_with_config(config);
        let started = std::time::Instant::now();
        let response = tokio::time::timeout(
            Duration::from_secs(8),
            client.complete(&[Message::user("say fixture")], None, &[]),
        )
        .await
        .expect("quiet gap must not wait for the generation deadline")
        .expect("fixture completion");
        let elapsed = started.elapsed();
        assert_eq!(response.text(), "fixture-assistant-answer");
        assert!(
            elapsed >= Duration::from_secs(2),
            "returned before the quiet gap: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "heartbeats extended the wait: {elapsed:?}"
        );
    }

    fn usable_models_fixture() -> String {
        r#"{"models":[
            {"modelId":"default","displayModelId":"auto","displayName":"Auto","displayNameShort":"Auto","aliases":[]},
            {"modelId":"composer-2.5","displayModelId":"composer-2.5","displayName":"Composer 2.5"},
            {"modelId":"gpt-5.2","displayName":"GPT-5.2"},
            {"modelId":" ","displayModelId":"blank"},
            {"displayModelId":"missing-wire"}
        ]}"#
        .to_string()
    }

    fn run_model_id(record: &Record) -> String {
        let body = record
            .body
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let value: Value = serde_json::from_slice(&body[5..]).unwrap();
        let details = value
            .pointer("/runRequest/modelDetails/modelId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let requested = value
            .pointer("/runRequest/requestedModel/modelId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        assert_eq!(details, requested);
        details
    }

    async fn complete_model(model: &str, catalog: Option<String>, record: Arc<Record>) {
        let http = Arc::new(FixtureHttp {
            record,
            response: fixture_body(),
            heartbeats_after: 0,
            catalog,
        });
        let config = LlmConfig::new("cursor", model, BEARER).with_http_client(http);
        let client = create_client_with_config(config);
        let response = client
            .complete(&[Message::user("say fixture")], None, &[])
            .await
            .expect("fixture completion");
        assert_eq!(response.text(), "fixture-assistant-answer");
    }

    #[test]
    fn account_catalog_maps_selectors_onto_model_id() {
        let map = account_model_alias_map(&serde_json::from_str(&usable_models_fixture()).unwrap());
        assert_eq!(map.get("auto").map(String::as_str), Some("default"));
        assert_eq!(map.get("default").map(String::as_str), Some("default"));
        assert_eq!(
            map.get("composer-2.5").map(String::as_str),
            Some("composer-2.5")
        );
        assert_eq!(map.get("gpt-5.2").map(String::as_str), Some("gpt-5.2"));
        assert!(!map.contains_key("blank"));
        assert!(!map.contains_key("missing-wire"));
        assert_eq!(resolve_model_id(&map, "auto"), "default");
        assert_eq!(resolve_model_id(&map, "Auto"), "default");
        assert_eq!(resolve_model_id(&map, "composer-2.5"), "composer-2.5");
        assert_eq!(resolve_model_id(&map, "not-in-catalog"), "not-in-catalog");
    }

    #[tokio::test]
    async fn auto_selector_is_sent_as_the_account_model_id() {
        let record = empty_record();
        let http = Arc::new(FixtureHttp {
            record: record.clone(),
            response: fixture_body(),
            heartbeats_after: 0,
            catalog: Some(usable_models_fixture()),
        });
        let config = LlmConfig::new("cursor", "auto", BEARER).with_http_client(http);
        let client = create_client_with_config(config);
        client
            .complete(&[Message::user("say fixture")], None, &[])
            .await
            .expect("first completion");
        client
            .complete(&[Message::user("say fixture again")], None, &[])
            .await
            .expect("second completion");
        assert_eq!(run_model_id(&record), "default");
        let catalog_urls = record
            .catalog_urls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        assert_eq!(catalog_urls.len(), 1, "the account catalog is fetched once");
        assert_eq!(
            catalog_urls[0],
            format!("https://api2.cursor.sh{USABLE_MODELS_PATH}")
        );
    }

    #[tokio::test]
    async fn listed_model_ids_stay_on_the_wire_and_a_failed_lookup_does_not_rewrite() {
        let catalog = Some(usable_models_fixture());
        for model in ["composer-2.5", "gpt-5.2", "not-in-catalog"] {
            let record = empty_record();
            complete_model(model, catalog.clone(), record.clone()).await;
            assert_eq!(run_model_id(&record), model);
        }
        let record = empty_record();
        complete_model("auto", None, record.clone()).await;
        assert_eq!(run_model_id(&record), "auto");
    }

    #[tokio::test]
    async fn account_stream_returns_governed_tool_calls_and_sends_results_back() {
        let record = empty_record();
        let text = r#"{"text":"Reading","tool_calls":[{"name":"read","arguments":{"file_path":"input.txt"}}]}"#;
        let mut response =
            envelope(&json!({"interactionUpdate":{"textDelta":{"text":text}}}).to_string());
        response.extend(envelope(r#"{"interactionUpdate":{"turnEnded":{}}}"#));
        let http = Arc::new(FixtureHttp {
            record: record.clone(),
            response,
            heartbeats_after: 0,
            catalog: None,
        });
        let client = create_client_with_config(
            LlmConfig::new("cursor", "composer-2.5", BEARER).with_http_client(http),
        );
        let tools = [super::super::types::ToolDefinition {
            name: "read".into(),
            description: "Read file".into(),
            parameters: json!({"type":"object"}),
        }];
        let mut stream = client
            .complete_streaming(
                &[Message::user("read input.txt")],
                None,
                &tools,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(
            matches!(stream.recv().await, Some(StreamEvent::TextDelta(text)) if text == "Reading")
        );
        let Some(StreamEvent::Done(response)) = stream.recv().await else {
            panic!("missing Done")
        };
        assert_eq!(response.stop_reason.as_deref(), Some("tool_use"));
        let call = &response.message.tool_calls()[0];
        assert_eq!(call.name, "read");
        client
            .complete(
                &[
                    response.message.clone(),
                    Message::tool_result(&call.id, "actual contents", false),
                ],
                None,
                &tools,
            )
            .await
            .unwrap();
        let body = record.body.lock().unwrap().clone();
        let request: Value = serde_json::from_slice(&body[5..]).unwrap();
        let prompt = request
            .pointer("/runRequest/action/userMessageAction/userMessage/text")
            .unwrap()
            .as_str()
            .unwrap();
        let transcript: Value = serde_json::from_str(prompt.lines().last().unwrap()).unwrap();
        assert_eq!(transcript["tools"][0]["name"], "read");
        assert_eq!(
            transcript["messages"][1]["content"][0]["content"],
            "actual contents"
        );
        assert_eq!(
            transcript["messages"][1]["content"][0]["tool_use_id"],
            call.id
        );
        assert!(transcript["messages"][1]["content"][0]
            .get("trust")
            .is_none());
    }
}
