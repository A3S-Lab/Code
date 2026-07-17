use super::*;
use crate::llm::types::{Message, ToolDefinition};
use futures::StreamExt;

fn make_client() -> OpenAiClient {
    OpenAiClient::new("test-key".to_string(), "gpt-test".to_string())
}

// --- streaming reasoning-channel regression -----------------------------
// Reasoning models (glm5.1/zhipu) stream chain-of-thought under `reasoning`.
// It must land in reasoning_content, NEVER in the text content — otherwise
// response.text() looks like a finished answer and the agent loop terminates
// before the model emits its tool call (asset-diagnose "未返回结构化输出").

struct MockSseHttp {
    chunks: Vec<String>,
}

struct PendingSseHttp;

struct FailingSseHttp {
    chunks: Vec<String>,
}

struct PartialThenPendingSseHttp;

#[async_trait::async_trait]
impl crate::llm::http::HttpClient for MockSseHttp {
    async fn post(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::llm::http::HttpResponse> {
        anyhow::bail!("post is unused in the streaming test")
    }

    async fn post_streaming(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::llm::http::StreamingHttpResponse> {
        let items: Vec<anyhow::Result<bytes::Bytes>> = self
            .chunks
            .iter()
            .map(|s| Ok(bytes::Bytes::from(s.clone())))
            .collect();
        Ok(crate::llm::http::StreamingHttpResponse {
            status: 200,
            retry_after: None,
            byte_stream: Box::pin(futures::stream::iter(items)),
            error_body: String::new(),
        })
    }
}

#[async_trait::async_trait]
impl crate::llm::http::HttpClient for PendingSseHttp {
    async fn post(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::llm::http::HttpResponse> {
        anyhow::bail!("post is unused in the streaming cancellation test")
    }

    async fn post_streaming(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::llm::http::StreamingHttpResponse> {
        Ok(crate::llm::http::StreamingHttpResponse {
            status: 200,
            retry_after: None,
            byte_stream: Box::pin(futures::stream::pending()),
            error_body: String::new(),
        })
    }
}

#[async_trait::async_trait]
impl crate::llm::http::HttpClient for FailingSseHttp {
    async fn post(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::llm::http::HttpResponse> {
        anyhow::bail!("post is unused in the interrupted streaming test")
    }

    async fn post_streaming(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::llm::http::StreamingHttpResponse> {
        let mut items = self
            .chunks
            .iter()
            .map(|chunk| Ok(bytes::Bytes::from(chunk.clone())))
            .collect::<Vec<anyhow::Result<bytes::Bytes>>>();
        items.push(Err(anyhow::anyhow!("connection reset")));
        Ok(crate::llm::http::StreamingHttpResponse {
            status: 200,
            retry_after: None,
            byte_stream: Box::pin(futures::stream::iter(items)),
            error_body: String::new(),
        })
    }
}

#[async_trait::async_trait]
impl crate::llm::http::HttpClient for PartialThenPendingSseHttp {
    async fn post(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::llm::http::HttpResponse> {
        anyhow::bail!("post is unused in the partial cancellation test")
    }

    async fn post_streaming(
        &self,
        _url: &str,
        _headers: Vec<(&str, &str)>,
        _body: &serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<crate::llm::http::StreamingHttpResponse> {
        let partial = Ok(bytes::Bytes::from_static(
            br#"data: {"choices":[{"delta":{"content":"partial"}}]}

"#,
        ));
        Ok(crate::llm::http::StreamingHttpResponse {
            status: 200,
            retry_after: None,
            byte_stream: Box::pin(
                futures::stream::iter(vec![partial]).chain(futures::stream::pending()),
            ),
            error_body: String::new(),
        })
    }
}

fn glm_client(chunks: Vec<String>) -> OpenAiClient {
    OpenAiClient::new("k".to_string(), "glm-test".to_string())
        .with_http_client(std::sync::Arc::new(MockSseHttp { chunks }))
}

async fn drain_to_done(client: &OpenAiClient) -> crate::llm::LlmResponse {
    use crate::llm::{LlmClient, StreamEvent};
    let mut rx = client
        .complete_streaming(
            &[Message::user("go")],
            None,
            &[],
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("stream opened");
    let mut done = None;
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Done(resp) = ev {
            done = Some(resp);
        }
    }
    done.expect("a Done event")
}

#[tokio::test]
async fn streaming_parser_closes_when_caller_cancels() {
    use crate::llm::LlmClient;

    let client = OpenAiClient::new("k".to_string(), "model".to_string())
        .with_http_client(std::sync::Arc::new(PendingSseHttp));
    let cancellation = tokio_util::sync::CancellationToken::new();
    let mut rx = client
        .complete_streaming(&[Message::user("go")], None, &[], cancellation.clone())
        .await
        .expect("stream opened");

    cancellation.cancel();

    let next = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
        .await
        .expect("provider parser must stop after cancellation");
    assert!(next.is_none());
}

#[tokio::test]
async fn streaming_transport_error_after_partial_delta_does_not_emit_done() {
    use crate::llm::{LlmClient, StreamEvent};

    let client = OpenAiClient::new("k".to_string(), "model".to_string()).with_http_client(
        std::sync::Arc::new(FailingSseHttp {
            chunks: vec![
                "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n".to_string(),
            ],
        }),
    );
    let mut rx = client
        .complete_streaming(
            &[Message::user("go")],
            None,
            &[],
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("stream opened");

    let mut text = String::new();
    let mut saw_done = false;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::TextDelta(delta) => text.push_str(&delta),
            StreamEvent::Done(_) => saw_done = true,
            _ => {}
        }
    }

    assert_eq!(text, "partial");
    assert!(
        !saw_done,
        "a failed transport must close without Done so the agent retries the turn"
    );
}

#[tokio::test]
async fn streaming_transport_error_after_finish_reason_can_finalize() {
    let client = OpenAiClient::new("k".to_string(), "model".to_string()).with_http_client(
        std::sync::Arc::new(FailingSseHttp {
            chunks: vec![
                "data: {\"choices\":[{\"delta\":{\"content\":\"complete\"}}]}\n\n".to_string(),
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n".to_string(),
            ],
        }),
    );

    let response = drain_to_done(&client).await;
    assert_eq!(response.text(), "complete");
    assert_eq!(response.stop_reason.as_deref(), Some("stop"));
}

#[tokio::test]
async fn streaming_partial_response_is_not_finalized_after_cancellation() {
    use crate::llm::{LlmClient, StreamEvent};

    let client = OpenAiClient::new("k".to_string(), "model".to_string())
        .with_http_client(std::sync::Arc::new(PartialThenPendingSseHttp));
    let cancellation = tokio_util::sync::CancellationToken::new();
    let mut rx = client
        .complete_streaming(&[Message::user("go")], None, &[], cancellation.clone())
        .await
        .expect("stream opened");

    assert!(matches!(
        rx.recv().await,
        Some(StreamEvent::TextDelta(text)) if text == "partial"
    ));
    cancellation.cancel();

    let next = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
        .await
        .expect("provider parser must stop after cancellation");
    assert!(next.is_none(), "cancellation must not synthesize Done");
}

#[tokio::test]
async fn streaming_reasoning_does_not_leak_into_content_and_keeps_tool_call() {
    let chunks = vec![
            "data: {\"choices\":[{\"delta\":{\"reasoning\":\"Let me plan the workers\"}}]}\n\n"
                .to_string(),
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"parallel_task\",\"arguments\":\"{}\"}}]}}]}\n\n"
                .to_string(),
            "data: [DONE]\n\n".to_string(),
        ];
    let resp = drain_to_done(&glm_client(chunks)).await;
    // Reasoning must NOT appear as text content.
    assert_eq!(resp.message.text(), "", "reasoning leaked into content");
    assert_eq!(
        resp.message.reasoning_content.as_deref(),
        Some("Let me plan the workers")
    );
    // The tool call still survives, so the agent can act.
    let calls = resp.message.tool_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "parallel_task");
}

#[tokio::test]
async fn streaming_reasoning_only_turn_yields_empty_text() {
    // A pure "thinking" turn (reasoning, no content, no tool call) must yield empty
    // text() so the agent loop's looks_incomplete("")==true path CONTINUES instead of
    // terminating prematurely — the multi-worker diagnose failure root cause.
    let chunks = vec![
        "data: {\"choices\":[{\"delta\":{\"reasoning\":\"still thinking, no answer yet\"}}]}\n\n"
            .to_string(),
        "data: [DONE]\n\n".to_string(),
    ];
    let resp = drain_to_done(&glm_client(chunks)).await;
    assert_eq!(resp.message.text(), "");
    assert_eq!(
        resp.message.reasoning_content.as_deref(),
        Some("still thinking, no answer yet")
    );
    assert!(resp.message.tool_calls().is_empty());
}

#[tokio::test]
async fn streaming_collects_token_logprobs() {
    let chunks = vec![
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"logprobs\":{\"content\":[{\"token\":\"hello\",\"logprob\":-0.2,\"bytes\":[104,101,108,108,111],\"top_logprobs\":[{\"token\":\"hi\",\"logprob\":-1.2,\"bytes\":[104,105]}]}]}}]}\n\n"
                .to_string(),
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n"
                .to_string(),
            "data: [DONE]\n\n".to_string(),
        ];
    let resp = drain_to_done(&glm_client(chunks).with_logprobs(true)).await;
    assert_eq!(resp.text(), "hello");
    assert_eq!(resp.token_logprobs.len(), 1);
    assert_eq!(resp.token_logprobs[0].token, "hello");
    assert_eq!(resp.token_logprobs[0].logprob, -0.2);
    assert_eq!(
        resp.token_logprobs[0].bytes.as_deref(),
        Some(&[104, 101, 108, 108, 111][..])
    );
    assert_eq!(resp.token_logprobs[0].top_logprobs[0].token, "hi");
    assert_eq!(resp.token_logprobs[0].top_logprobs[0].logprob, -1.2);
}

#[tokio::test]
async fn streaming_accepts_sse_data_without_space_after_colon() {
    let chunks = vec![
            "data:{\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}],\"usage\":null}\n\n"
                .to_string(),
            "data:{\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n"
                .to_string(),
            "data:[DONE]\n\n".to_string(),
        ];
    let resp = drain_to_done(&glm_client(chunks)).await;
    assert_eq!(resp.text(), "hello");
    assert_eq!(resp.usage.prompt_tokens, 1);
    assert_eq!(resp.usage.completion_tokens, 1);
    assert_eq!(resp.usage.total_tokens, 2);
    assert_eq!(resp.stop_reason.as_deref(), Some("stop"));
}

#[test]
fn test_apply_directive_forced_function_tool_choice() {
    let mut req = serde_json::json!({ "model": "m" });
    OpenAiClient::apply_directive(
        &mut req,
        &structured::StructuredDirective {
            force_tool: Some("emit_person".to_string()),
            response_format: None,
        },
    );
    assert_eq!(req["tool_choice"]["type"], "function");
    assert_eq!(req["tool_choice"]["function"]["name"], "emit_person");
    assert!(req.get("response_format").is_none());
}

#[test]
fn test_apply_directive_json_schema_strict() {
    let mut req = serde_json::json!({});
    OpenAiClient::apply_directive(
        &mut req,
        &structured::StructuredDirective {
            force_tool: None,
            response_format: Some(structured::ResponseFormat::JsonSchema {
                name: "person".to_string(),
                schema: serde_json::json!({ "type": "object" }),
            }),
        },
    );
    assert_eq!(req["response_format"]["type"], "json_schema");
    assert_eq!(req["response_format"]["json_schema"]["name"], "person");
    assert_eq!(req["response_format"]["json_schema"]["strict"], true);
    assert!(req.get("tool_choice").is_none());
}

#[test]
fn test_apply_directive_json_object() {
    let mut req = serde_json::json!({});
    OpenAiClient::apply_directive(
        &mut req,
        &structured::StructuredDirective {
            force_tool: None,
            response_format: Some(structured::ResponseFormat::JsonObject),
        },
    );
    assert_eq!(req["response_format"]["type"], "json_object");
}

#[test]
fn test_build_chat_request_applies_directive_and_system() {
    let req = make_client().build_chat_request(
        &[Message::user("hi")],
        Some("sys"),
        &[ToolDefinition {
            name: "emit_x".to_string(),
            description: "emit".to_string(),
            parameters: serde_json::json!({ "type": "object" }),
        }],
        Some(&structured::StructuredDirective {
            force_tool: Some("emit_x".to_string()),
            response_format: None,
        }),
    );
    assert_eq!(req["messages"][0]["role"], "system");
    assert_eq!(req["tool_choice"]["function"]["name"], "emit_x");
    assert_eq!(req["tools"][0]["function"]["name"], "emit_x");
}

#[test]
fn test_build_chat_request_without_directive_is_plain() {
    let req = make_client().build_chat_request(&[Message::user("hi")], None, &[], None);
    assert!(req.get("tool_choice").is_none());
    assert!(req.get("response_format").is_none());
    assert!(req.get("logprobs").is_none());
    assert!(req.get("top_logprobs").is_none());
}

#[test]
fn test_build_chat_request_includes_logprob_options_when_enabled() {
    let req = make_client().with_top_logprobs(1).build_chat_request(
        &[Message::user("hi")],
        None,
        &[],
        None,
    );
    assert_eq!(req["logprobs"], true);
    assert_eq!(req["top_logprobs"], 1);
}

#[test]
fn test_parse_openai_token_logprobs() {
    let parsed = openai_logprobs_to_token_logprobs(&OpenAiChoiceLogprobs {
        content: Some(vec![OpenAiTokenLogprob {
            token: "hello".to_string(),
            logprob: -0.25,
            bytes: Some(vec![104, 101, 108, 108, 111]),
            top_logprobs: vec![OpenAiTopLogprob {
                token: "hi".to_string(),
                logprob: -1.5,
                bytes: Some(vec![104, 105]),
            }],
        }]),
    });
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].token, "hello");
    assert_eq!(parsed[0].logprob, -0.25);
    assert_eq!(
        parsed[0].bytes.as_deref(),
        Some(&[104, 101, 108, 108, 111][..])
    );
    assert_eq!(parsed[0].top_logprobs[0].token, "hi");
    assert_eq!(parsed[0].top_logprobs[0].logprob, -1.5);
}

#[test]
fn test_native_structured_support_is_json_schema() {
    assert_eq!(
        make_client().native_structured_support(),
        structured::NativeStructuredSupport::JsonSchema
    );
}

#[test]
fn test_native_structured_support_can_be_overridden() {
    assert_eq!(
        make_client()
            .with_native_structured_support(structured::NativeStructuredSupport::None)
            .native_structured_support(),
        structured::NativeStructuredSupport::None
    );
}
