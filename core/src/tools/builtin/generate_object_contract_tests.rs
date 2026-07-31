use super::*;
use crate::llm::structured::NativeStructuredSupport;
use crate::llm::{ContentBlock, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition};
use crate::tools::ToolExecutor;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct OneResponseClient {
    response: Mutex<Option<LlmResponse>>,
}

impl OneResponseClient {
    fn new(input: Value) -> Self {
        Self {
            response: Mutex::new(Some(LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: "contract-call".to_string(),
                        name: "emit_result".to_string(),
                        input,
                    }],
                    reasoning_content: None,
                },
                usage: TokenUsage::default(),
                stop_reason: Some("tool_use".to_string()),
                token_logprobs: Vec::new(),
                meta: None,
            })),
        }
    }
}

#[async_trait]
impl LlmClient for OneResponseClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.response
            .lock()
            .expect("contract response lock")
            .take()
            .ok_or_else(|| anyhow::anyhow!("contract response already consumed"))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("contract tests use blocking generation")
    }

    fn native_structured_support(&self) -> NativeStructuredSupport {
        NativeStructuredSupport::ForcedTool
    }
}

#[derive(Clone)]
struct TimeoutAwareClient {
    observed_timeout_ms: Arc<AtomicU64>,
    response: Arc<Mutex<Option<LlmResponse>>>,
}

#[async_trait]
impl LlmClient for TimeoutAwareClient {
    fn with_active_generation_timeout(&self, timeout: Duration) -> Option<Arc<dyn LlmClient>> {
        self.observed_timeout_ms.store(
            u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            Ordering::SeqCst,
        );
        Some(Arc::new(self.clone()))
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.response
            .lock()
            .expect("timeout response lock")
            .take()
            .ok_or_else(|| anyhow::anyhow!("timeout response already consumed"))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("timeout contract uses blocking generation")
    }

    fn native_structured_support(&self) -> NativeStructuredSupport {
        NativeStructuredSupport::ForcedTool
    }
}

async fn run_contract_case(schema: Value, provider_input: Value) -> ToolOutput {
    let workspace = tempfile::tempdir().expect("contract workspace");
    let tool = GenerateObjectTool::new(Arc::new(OneResponseClient::new(provider_input)));
    tool.execute(
        &serde_json::json!({
            "schema": schema,
            "prompt": "Return the requested value.",
            "mode": "tool"
        }),
        &ToolContext::new(workspace.path().to_path_buf()),
    )
    .await
    .expect("generate_object contract execution")
}

#[tokio::test]
async fn generate_object_accepts_standard_root_schema_keywords() {
    let cases = [
        (
            serde_json::json!({
                "$defs": {
                    "record": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {"answer": {"const": "ok"}},
                        "required": ["answer"]
                    }
                },
                "$ref": "#/$defs/record"
            }),
            serde_json::json!({"answer": "ok"}),
            serde_json::json!({"answer": "ok"}),
        ),
        (
            serde_json::json!({
                "allOf": [
                    {"type": "object"},
                    {
                        "additionalProperties": false,
                        "properties": {"answer": {"const": "ok"}},
                        "required": ["answer"]
                    }
                ]
            }),
            serde_json::json!({"answer": "ok"}),
            serde_json::json!({"answer": "ok"}),
        ),
        (
            serde_json::json!({"const": "fixed"}),
            serde_json::json!({"value": "fixed"}),
            serde_json::json!("fixed"),
        ),
        (
            serde_json::json!({
                "$defs": {
                    "list": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1
                    }
                },
                "$ref": "#/$defs/list"
            }),
            serde_json::json!({"elements": ["ready"]}),
            serde_json::json!(["ready"]),
        ),
    ];

    for (schema, provider_input, expected) in cases {
        let output = run_contract_case(schema, provider_input).await;
        assert!(output.success, "unexpected contract failure: {output:#?}");
        let payload: Value = serde_json::from_str(&output.content).expect("JSON tool output");
        assert_eq!(payload["object"], expected);
    }
}

#[tokio::test]
async fn generate_object_rejects_parameters_outside_its_declared_contract() {
    let valid_schema = serde_json::json!({
        "type": "object",
        "properties": {"value": {"type": "string"}},
        "required": ["value"]
    });
    let base = || {
        serde_json::json!({
            "schema": valid_schema.clone(),
            "prompt": "Return a value.",
            "mode": "tool"
        })
    };
    let mut invalid_arguments = Vec::new();

    let mut whitespace_prompt = base();
    whitespace_prompt["prompt"] = Value::String(" \n\t ".to_string());
    invalid_arguments.push(whitespace_prompt);

    let mut unicode_name = base();
    unicode_name["schema_name"] = Value::String("结果".to_string());
    invalid_arguments.push(unicode_name);

    let mut long_name = base();
    long_name["schema_name"] = Value::String("a".repeat(60));
    invalid_arguments.push(long_name);

    let mut long_description = base();
    long_description["schema_description"] = Value::String("d".repeat(4 * 1024 + 1));
    invalid_arguments.push(long_description);

    let mut excess_repairs = base();
    excess_repairs["max_repair_attempts"] = serde_json::json!(6);
    invalid_arguments.push(excess_repairs);

    let mut short_timeout = base();
    short_timeout["timeout_ms"] = serde_json::json!(999);
    invalid_arguments.push(short_timeout);

    let mut unknown_field = base();
    unknown_field["unexpected"] = serde_json::json!(true);
    invalid_arguments.push(unknown_field);

    for args in invalid_arguments {
        let workspace = tempfile::tempdir().expect("validation workspace");
        let tool = GenerateObjectTool::new(Arc::new(OneResponseClient::new(
            serde_json::json!({"value": "must not be used"}),
        )));
        let output = tool
            .execute(&args, &ToolContext::new(workspace.path().to_path_buf()))
            .await
            .expect("validation output");
        assert!(!output.success, "arguments should be rejected: {args:#}");
        assert!(
            matches!(
                output.error_kind,
                Some(ToolErrorKind::InvalidArgument { .. })
            ),
            "expected InvalidArgument for {args:#}, got {output:#?}"
        );
    }
}

#[test]
fn generate_object_parameter_schema_declares_runtime_limits() {
    let tool = GenerateObjectTool::new(Arc::new(OneResponseClient::new(Value::Null)));
    let parameters = tool.parameters();
    let properties = &parameters["properties"];

    assert_eq!(properties["prompt"]["minLength"], 1);
    assert_eq!(properties["schema_name"]["maxLength"], 59);
    assert_eq!(properties["schema_name"]["pattern"], "^[A-Za-z0-9_-]+$");
    assert_eq!(properties["schema_description"]["maxLength"], 4 * 1024);
}

#[test]
fn generation_failure_kind_uses_typed_errors_instead_of_diagnostic_prose() {
    let prose = anyhow::anyhow!("rate limit: too many requests; status 429");
    assert_eq!(generation_failure_kind(&prose), None);

    let rate_limited = anyhow::Error::new(crate::retry::RetryExhaustedError::new(
        1,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "opaque",
    ));
    assert_eq!(
        generation_failure_kind(&rate_limited),
        Some(ToolErrorKind::RateLimited {
            retry_after_ms: None,
        })
    );

    let unavailable = anyhow::Error::new(crate::retry::RetryExhaustedError::new(
        1,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        "opaque",
    ));
    assert_eq!(
        generation_failure_kind(&unavailable),
        Some(ToolErrorKind::Transport {
            op: "generate_object".to_string(),
        })
    );

    let transport = anyhow::Error::new(crate::llm::HttpClientError::transport(
        "HTTP request",
        "opaque diagnostic",
    ));
    assert_eq!(
        generation_failure_kind(&transport),
        Some(ToolErrorKind::Transport {
            op: "generate_object".to_string(),
        })
    );

    let cancelled = anyhow::Error::new(crate::llm::HttpClientError::cancelled("HTTP request"));
    assert_eq!(
        generation_failure_kind(&cancelled),
        Some(ToolErrorKind::Cancelled {
            op: "generate_object".to_string(),
        })
    );
}

#[tokio::test]
async fn generate_object_propagates_active_timeout_through_the_invocation_gateway() {
    let workspace = tempfile::tempdir().expect("timeout workspace");
    let observed_timeout_ms = Arc::new(AtomicU64::new(0));
    let raw_client: Arc<dyn LlmClient> = Arc::new(TimeoutAwareClient {
        observed_timeout_ms: Arc::clone(&observed_timeout_ms),
        response: Arc::new(Mutex::new(Some(LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "timeout-call".to_string(),
                    name: "emit_result".to_string(),
                    input: serde_json::json!({"value": "ok"}),
                }],
                reasoning_content: None,
            },
            usage: TokenUsage::default(),
            stop_reason: Some("tool_use".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }))),
    });
    let agent = crate::agent::AgentLoop::new(
        Arc::clone(&raw_client),
        Arc::new(ToolExecutor::new(
            workspace.path().to_string_lossy().to_string(),
        )),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig::default(),
    );
    let cancellation = CancellationToken::new();
    let governed = agent.scoped_llm_client_for_parts(Some("timeout-session"), &None, &cancellation);
    let context = ToolContext::new(workspace.path().to_path_buf())
        .with_session_id("timeout-session")
        .with_cancellation(cancellation)
        .with_llm_client(governed);
    let tool = GenerateObjectTool::new(raw_client);
    let requested_timeout_ms = 45_000;

    let output = tool
        .execute(
            &serde_json::json!({
                "schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {"value": {"type": "string"}},
                    "required": ["value"]
                },
                "prompt": "Return a value.",
                "mode": "tool",
                "timeout_ms": requested_timeout_ms
            }),
            &context,
        )
        .await
        .expect("timeout-aware output");

    assert!(output.success, "{}", output.content);
    assert_eq!(
        observed_timeout_ms.load(Ordering::SeqCst),
        requested_timeout_ms
    );
}
