//! S-SO-01: invalid objects must stop at the repair bound. The fixture never
//! emits JSON, so a success result or an extra model call is a kernel bug.

use super::structured::{generate_blocking, StructuredMode, StructuredRequest};
use super::{LlmClient, LlmResponse, Message, TokenUsage, ToolDefinition};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

struct InvalidObjectClient {
    calls: AtomicUsize,
}

#[async_trait]
impl LlmClient for InvalidObjectClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(LlmResponse {
            message: Message::assistant("not-an-object"),
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("end_turn".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<super::StreamEvent>> {
        anyhow::bail!("structured repair soak uses the blocking path")
    }
}

#[tokio::test]
#[ignore = "S-SO-01 soak: 25 invalid objects stop at the repair bound"]
async fn soak_invalid_objects_stop_at_the_repair_bound() {
    const CALLS: usize = 25;
    const REPAIR_CAP: u8 = 2;
    let client = InvalidObjectClient {
        calls: AtomicUsize::new(0),
    };
    let request = StructuredRequest {
        prompt: "return the object".to_string(),
        system: None,
        schema: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["ok"],
            "properties": { "ok": { "type": "boolean" } }
        }),
        schema_name: "marker".to_string(),
        schema_description: None,
        mode: StructuredMode::Prompt,
        max_repair_attempts: REPAIR_CAP,
    };

    for index in 0..CALLS {
        let error = generate_blocking(&client, &request)
            .await
            .expect_err("invalid object must not succeed");
        let message = error.to_string();
        assert!(
            message.contains(&format!("{REPAIR_CAP} repair")),
            "call {index} did not stop at the repair bound: {message}"
        );
    }

    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        CALLS * (1 + REPAIR_CAP as usize),
        "repair loop exceeded the bound"
    );
}
