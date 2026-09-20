//! S-MA-01: an HTTP 500 that is retried inside the model client must not
//! run the turn's tool once per attempt.

use crate::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use crate::llm::OpenAiClient;
use crate::memory::MemoryConfig;
use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::prompts::PlanningMode;
use crate::retry::RetryConfig;
use crate::tools::{Tool, ToolContext, ToolOutput};
use crate::{Agent, SessionOptions};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct CountingTool {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &str {
        "soak_marker"
    }

    fn description(&self) -> &str {
        "Records one host-visible tool execution"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::success("marked"))
    }
}

fn tool_body(call_id: usize) -> String {
    format!(
        r#"{{"choices":[{{"message":{{"content":null,"tool_calls":[{{"id":"call-{call_id}","function":{{"name":"soak_marker","arguments":"{{}}"}}}}]}},"finish_reason":"tool_calls"}}],"usage":{{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}}}"#
    )
}

fn text_body() -> String {
    r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#
        .to_string()
}

fn content_length(headers: &str) -> usize {
    headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

async fn read_request(stream: &mut tokio::net::TcpStream) {
    let mut buf = Vec::new();
    let mut tmp = [0_u8; 2048];
    loop {
        let read = stream.read(&mut tmp).await.unwrap_or(0);
        if read == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..read]);
        if let Some(end) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..end]);
            let body = content_length(&headers);
            if buf.len() >= end + 4 + body {
                return;
            }
        }
        if buf.len() > 1_048_576 {
            return;
        }
    }
}

#[tokio::test]
#[ignore = "S-MA-01 soak: 40 turns with two HTTP 500s before each success"]
async fn soak_http_retries_do_not_multiply_tool_execution() {
    let requests = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("addr");
    let request_counter = Arc::clone(&requests);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let index = request_counter.fetch_add(1, Ordering::SeqCst);
            read_request(&mut stream).await;
            let (status, body) = if index % 3 == 2 {
                let group = index / 3;
                if group % 2 == 0 {
                    (200, tool_body(group))
                } else {
                    (200, text_body())
                }
            } else {
                (500, "unavailable".to_string())
            };
            let reason = if status == 200 {
                "OK"
            } else {
                "Internal Server Error"
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });

    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(
        OpenAiClient::new("offline-key".to_string(), "fixture-model".to_string())
            .with_base_url(format!("http://{address}"))
            .with_retry_config(RetryConfig {
                max_retries: 2,
                base_delay_ms: 0,
                max_delay_ms: 0,
                retryable_status_codes: vec![500],
            }),
    );
    let config = CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        memory: Some(MemoryConfig {
            llm_extraction: false,
            ..MemoryConfig::default()
        }),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude".to_string(),
                family: "claude".to_string(),
                api_key: None,
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..CodeConfig::default()
    };
    let workspace = tempfile::tempdir().expect("workspace");
    let agent = Agent::from_config(config).await.expect("agent");
    let mut policy = PermissionPolicy::new().allow("soak_marker");
    policy.default_decision = PermissionDecision::Allow;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("s-ma-01")
                    .with_model("anthropic/claude-sonnet-4-20250514")
                    .with_llm_client(client)
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_continuation(false)
                    .with_permission_policy(policy),
            ),
        )
        .await
        .expect("session");
    session
        .register_dynamic_tool(Arc::new(CountingTool {
            calls: Arc::clone(&calls),
        }))
        .expect("register");

    for turn in 0..40 {
        session
            .send("mark once", None)
            .await
            .unwrap_or_else(|error| panic!("turn {turn} failed: {error}"));
    }

    assert_eq!(
        calls.load(Ordering::SeqCst),
        40,
        "tool executions tracked retries"
    );
    let hits = requests.load(Ordering::SeqCst);
    assert_eq!(
        hits % 3,
        0,
        "each model call must be 500, 500, 200; hits={hits}"
    );
    assert!(
        hits / 3 <= 40 * 2,
        "retry cap exceeded: {hits} http responses for 40 turns"
    );
}
