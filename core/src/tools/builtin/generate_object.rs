//! Built-in `generate_object` tool for structured JSON output.
//!
//! This tool allows the agent (or users via `session.tool()`) to generate a
//! JSON value that conforms to a given JSON Schema. It supports streaming
//! partial values via `ToolStreamEvent::OutputDelta`.

use crate::llm::structured::{self, PartialObjectCallback, StructuredMode, StructuredRequest};
use crate::llm::{
    LlmClient, ModelGenerationAdmission, ModelGenerationAdmissionError, ModelGenerationConcurrency,
    ModelGenerationPermit,
};
use crate::tools::types::{Tool, ToolContext, ToolErrorKind, ToolOutput, ToolStreamEvent};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_SCHEMA_DEPTH: usize = 32;
const MAX_SCHEMA_NAME_BYTES: usize = 59;
const MAX_SCHEMA_DESCRIPTION_BYTES: usize = 4 * 1024;
const MAX_PROMPT_BYTES: usize = 128 * 1024;
const MAX_SYSTEM_BYTES: usize = 32 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const PARTIAL_EVENT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
const MAX_PARTIAL_EVENT_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerateObjectParams {
    schema: Value,
    #[serde(default)]
    schema_name: Option<String>,
    #[serde(default)]
    schema_description: Option<String>,
    prompt: String,
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    max_repair_attempts: Option<u64>,
    #[serde(default)]
    include_raw_text: bool,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub struct GenerateObjectTool {
    llm_client: Arc<dyn LlmClient>,
    admission: ModelGenerationAdmission,
}

impl GenerateObjectTool {
    pub fn new(llm_client: Arc<dyn LlmClient>) -> Self {
        let admission = ModelGenerationAdmission::new(llm_client.model_generation_concurrency());
        Self {
            llm_client,
            admission,
        }
    }
}

#[async_trait]
impl Tool for GenerateObjectTool {
    fn name(&self) -> &str {
        "generate_object"
    }

    fn description(&self) -> &str {
        "Generate a JSON value that strictly conforms to a provided JSON Schema. \
         Use when you need structured output: extracting fields from text, classifying \
         data, converting natural language to typed records, or producing machine-readable \
         results. The root value may be an object, array, or scalar. Returns the validated \
         value on success."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["schema", "prompt"],
            "additionalProperties": false,
            "properties": {
                "schema": {
                    "type": "object",
                    "description": "JSON Schema that the output value must conform to"
                },
                "schema_name": {
                    "type": "string",
                    "description": "Short name for the schema (used internally for tool naming)",
                    "minLength": 1,
                    "maxLength": MAX_SCHEMA_NAME_BYTES,
                    "pattern": "^[A-Za-z0-9_-]+$",
                    "default": "result"
                },
                "schema_description": {
                    "type": "string",
                    "description": "Optional description of what the schema represents",
                    "maxLength": MAX_SCHEMA_DESCRIPTION_BYTES
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt describing what value to generate or extract",
                    "minLength": 1,
                    "maxLength": MAX_PROMPT_BYTES
                },
                "system": {
                    "type": "string",
                    "description": "Optional system prompt to guide generation",
                    "maxLength": MAX_SYSTEM_BYTES
                },
                "mode": {
                    "type": "string",
                    "enum": ["auto", "strict", "json", "tool", "prompt"],
                    "description": "Output mode. 'auto' selects the best mode for the provider. 'tool' uses tool-calling (most reliable cross-provider). 'strict' uses OpenAI native JSON schema. 'json' uses json_object mode. 'prompt' appends schema to prompt.",
                    "default": "auto"
                },
                "max_repair_attempts": {
                    "type": "integer",
                    "description": "Maximum repair attempts if output fails validation (0-5)",
                    "default": 2,
                    "minimum": 0,
                    "maximum": 5
                },
                "include_raw_text": {
                    "type": "boolean",
                    "description": "Include the raw model text/tool arguments used to extract the final value. Defaults to false to avoid exposing reasoning-channel text.",
                    "default": false
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1000,
                    "maximum": MAX_TIMEOUT_MS,
                    "description": "Active generation deadline in milliseconds. Admission queue wait is excluded. Default 120000; maximum 600000.",
                    "default": DEFAULT_TIMEOUT_MS
                }
            }
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let params: GenerateObjectParams = match serde_json::from_value(args.clone()) {
            Ok(params) => params,
            Err(error) => {
                return Ok(invalid_argument(format!(
                    "Invalid generate_object parameters: {error}"
                )));
            }
        };
        let GenerateObjectParams {
            schema,
            schema_name,
            schema_description,
            prompt,
            system,
            mode,
            max_repair_attempts,
            include_raw_text,
            timeout_ms,
        } = params;
        if !schema.is_object() {
            return Ok(invalid_argument(
                "'schema' must be a JSON object (a valid JSON Schema)".to_string(),
            ));
        }
        let schema_bytes = serde_json::to_vec(&schema)?.len();
        if schema_bytes > MAX_SCHEMA_BYTES {
            return Ok(invalid_argument(format!(
                "'schema' exceeds the {MAX_SCHEMA_BYTES} byte limit"
            )));
        }
        if json_depth(&schema) > MAX_SCHEMA_DEPTH {
            return Ok(invalid_argument(format!(
                "'schema' exceeds the maximum nesting depth of {MAX_SCHEMA_DEPTH}"
            )));
        }
        if let Err(error) = jsonschema::draft202012::options().build(&schema) {
            return Ok(invalid_argument(format!(
                "'schema' is not a valid JSON Schema: {error}"
            )));
        }

        if prompt.trim().is_empty() {
            return Ok(invalid_argument(
                "'prompt' parameter is required and must contain non-whitespace text".to_string(),
            ));
        }
        if prompt.len() > MAX_PROMPT_BYTES {
            return Ok(invalid_argument(format!(
                "'prompt' exceeds the {MAX_PROMPT_BYTES} byte limit"
            )));
        }

        let schema_name = schema_name.unwrap_or_else(|| "result".to_string());
        if schema_name.is_empty()
            || schema_name.len() > MAX_SCHEMA_NAME_BYTES
            || !schema_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Ok(invalid_argument(format!(
                "'schema_name' must match ^[A-Za-z0-9_-]+$ and contain at most {MAX_SCHEMA_NAME_BYTES} bytes"
            )));
        }
        if schema_description
            .as_ref()
            .is_some_and(|value| value.len() > MAX_SCHEMA_DESCRIPTION_BYTES)
        {
            return Ok(invalid_argument(format!(
                "'schema_description' exceeds the {MAX_SCHEMA_DESCRIPTION_BYTES} byte limit"
            )));
        }
        if system
            .as_ref()
            .is_some_and(|value| value.len() > MAX_SYSTEM_BYTES)
        {
            return Ok(invalid_argument(format!(
                "'system' exceeds the {MAX_SYSTEM_BYTES} byte limit"
            )));
        }

        let requested_mode = mode.unwrap_or_else(|| "auto".to_string());
        let structured_mode = match requested_mode.as_str() {
            "strict" => StructuredMode::Strict,
            "json" => StructuredMode::Json,
            "tool" => StructuredMode::Tool,
            "prompt" => StructuredMode::Prompt,
            "auto" => StructuredMode::Auto,
            other => {
                return Ok(invalid_argument(format!(
                    "'mode' must be one of auto, strict, json, tool, or prompt; got '{other}'"
                )));
            }
        };

        // Mode resolution is delegated to the structured engine, which inspects
        // the client's native capability. Unsupported native modes safely fall
        // back to prompt+schema parsing instead of sending provider parameters
        // that some OpenAI-compatible endpoints hang on.
        let max_repair_attempts = max_repair_attempts.unwrap_or(2);
        if max_repair_attempts > 5 {
            return Ok(invalid_argument(
                "'max_repair_attempts' must be between 0 and 5".to_string(),
            ));
        }
        let max_repair_attempts = max_repair_attempts as u8;
        let timeout_ms = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        if !(1_000..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
            return Ok(invalid_argument(format!(
                "'timeout_ms' must be between 1000 and {MAX_TIMEOUT_MS}"
            )));
        }

        let req = StructuredRequest {
            prompt,
            system,
            schema,
            schema_name: schema_name.clone(),
            schema_description,
            mode: structured_mode,
            max_repair_attempts,
        };

        let admission = ctx
            .model_generation_admission()
            .unwrap_or_else(|| self.admission.clone());
        let mut preadmitted = ctx.model_generation_permit(&admission);
        let llm_client = ctx
            .llm_client()
            .unwrap_or_else(|| Arc::clone(&self.llm_client));
        let active_timeout = Duration::from_millis(timeout_ms);
        let mut llm_client = llm_client
            .with_active_generation_timeout(active_timeout)
            .unwrap_or(llm_client);
        // A governed LlmInvoker admits each underlying provider call. If an
        // outer workflow already acquired a permit, transfer it into that
        // facade for the first call; holding it around the whole structured
        // repair transaction would recursively deadlock a single-flight pool.
        let managed_generation = if llm_client.model_generation_is_managed() {
            // Pass a clone so a third-party managed facade that declines the
            // rebinding hook does not accidentally consume the only outer
            // permit. In that fallback case the ordinary outer admission path
            // remains responsible for the call.
            match llm_client.bind_model_generation_admission(admission.clone(), preadmitted.clone())
            {
                Some(rebound) => {
                    llm_client = rebound;
                    preadmitted = None;
                    true
                }
                None => {
                    tracing::warn!(
                        "managed LLM client did not provide a model-generation rebinding hook"
                    );
                    false
                }
            }
        } else {
            false
        };
        let cancellation = ctx.cancellation_token();
        let generation = async {
            if let Some(ref tx) = ctx.event_tx {
                let tx_clone = tx.clone();
                let last_event = Arc::new(std::sync::Mutex::new(None::<std::time::Instant>));
                let callback: PartialObjectCallback = Box::new(move |partial: &Value| {
                    let now = std::time::Instant::now();
                    let mut last_event = last_event.lock().unwrap();
                    if last_event
                        .is_some_and(|last| now.duration_since(last) < PARTIAL_EVENT_INTERVAL)
                    {
                        return;
                    }
                    *last_event = Some(now);
                    let encoded = serde_json::to_vec(partial).unwrap_or_default();
                    let delta = if encoded.len() <= MAX_PARTIAL_EVENT_BYTES {
                        serde_json::json!({
                            "object_partial": partial,
                            "final": false,
                        })
                    } else {
                        serde_json::json!({
                            "object_partial_omitted": true,
                            "partial_bytes": encoded.len(),
                            "final": false,
                        })
                    };
                    let delta_str = serde_json::to_string(&delta).unwrap_or_default();
                    let _ = tx_clone.try_send(ToolStreamEvent::OutputDelta(delta_str));
                });
                structured::generate_streaming_with_cancellation(
                    &*llm_client,
                    &req,
                    callback,
                    cancellation.clone(),
                )
                .await
            } else {
                structured::generate_blocking_with_cancellation(
                    &*llm_client,
                    &req,
                    cancellation.clone(),
                )
                .await
            }
        };
        let execution = if managed_generation {
            run_generation_without_admission(&cancellation, active_timeout, generation).await
        } else {
            run_generation_with_admission(
                &admission,
                preadmitted,
                &cancellation,
                active_timeout,
                generation,
            )
            .await
        };
        let queue_wait = execution
            .queue_wait
            .saturating_add(llm_client.take_model_generation_queue_wait());
        let admission_metadata =
            generation_admission_metadata(admission.concurrency(), queue_wait, timeout_ms);
        let result = execution.result;

        match result {
            Ok(sr) => {
                if let Some(ref tx) = ctx.event_tx {
                    let object_bytes = serde_json::to_vec(&sr.object).unwrap_or_default().len();
                    let final_delta = if object_bytes <= MAX_PARTIAL_EVENT_BYTES {
                        serde_json::json!({
                            "object_partial": sr.object,
                            "final": true,
                            "mode_used": sr.mode_used,
                            "repair_rounds": sr.repair_rounds,
                        })
                    } else {
                        serde_json::json!({
                            "object_partial_omitted": true,
                            "partial_bytes": object_bytes,
                            "final": true,
                            "mode_used": sr.mode_used,
                            "repair_rounds": sr.repair_rounds,
                        })
                    };
                    let _ = tx.try_send(ToolStreamEvent::OutputDelta(
                        serde_json::to_string(&final_delta).unwrap_or_default(),
                    ));
                }

                let mut output = serde_json::json!({
                    "object": sr.object,
                    "repair_rounds": sr.repair_rounds,
                    "mode_used": sr.mode_used,
                    "usage": {
                        "prompt_tokens": sr.usage.prompt_tokens,
                        "completion_tokens": sr.usage.completion_tokens,
                        "total_tokens": sr.usage.total_tokens,
                        "cache_read_tokens": sr.usage.cache_read_tokens,
                        "cache_write_tokens": sr.usage.cache_write_tokens,
                    }
                });
                if include_raw_text {
                    output["raw_text"] = sr.raw_text.map(Value::String).unwrap_or(Value::Null);
                }
                let metadata = serde_json::json!({
                    "schema_name": schema_name,
                    "requested_mode": requested_mode,
                    "mode_used": sr.mode_used,
                    "repair_rounds": sr.repair_rounds,
                    "usage": output["usage"].clone(),
                    "raw_text_included": include_raw_text,
                    "generation_admission": admission_metadata,
                });
                Ok(ToolOutput::success(serde_json::to_string(&output)?).with_metadata(metadata))
            }
            Err(stop) => {
                let (message, kind) = match stop {
                    GenerationStop::Cancelled => (
                        "generate_object cancelled by caller".to_string(),
                        Some(ToolErrorKind::Cancelled {
                            op: "generate_object".to_string(),
                        }),
                    ),
                    GenerationStop::TimedOut => (
                        format!("generate_object timed out after {timeout_ms}ms"),
                        Some(ToolErrorKind::Timeout {
                            op: "generate_object".to_string(),
                            duration_ms: timeout_ms,
                        }),
                    ),
                    GenerationStop::Failed(error) => {
                        let message = generation_failure_message(&error);
                        let kind = generation_failure_kind(&error);
                        (format!("generate_object failed: {message}"), kind)
                    }
                };
                let output = ToolOutput::error(message).with_metadata(serde_json::json!({
                    "schema_name": schema_name,
                    "requested_mode": requested_mode,
                    "mode_requested": structured_mode,
                    "timeout_ms": timeout_ms,
                    "generation_admission": admission_metadata,
                }));
                Ok(match kind {
                    Some(kind) => output.with_error_kind(kind),
                    None => output,
                })
            }
        }
    }
}

enum GenerationStop {
    Cancelled,
    TimedOut,
    Failed(anyhow::Error),
}

fn generation_failure_message(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

fn generation_failure_kind(error: &anyhow::Error) -> Option<ToolErrorKind> {
    if let Some(exhausted) = error.downcast_ref::<crate::retry::RetryExhaustedError>() {
        let status = exhausted.status();
        return if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            Some(ToolErrorKind::RateLimited {
                retry_after_ms: None,
            })
        } else if status == reqwest::StatusCode::REQUEST_TIMEOUT || status.is_server_error() {
            Some(ToolErrorKind::Transport {
                op: "generate_object".to_string(),
            })
        } else {
            None
        };
    }

    match error.downcast_ref::<crate::llm::HttpClientError>() {
        Some(crate::llm::HttpClientError::Cancelled { .. }) => Some(ToolErrorKind::Cancelled {
            op: "generate_object".to_string(),
        }),
        Some(crate::llm::HttpClientError::Transport { .. }) => Some(ToolErrorKind::Transport {
            op: "generate_object".to_string(),
        }),
        Some(crate::llm::HttpClientError::InvalidRequest { .. }) | None => None,
    }
}

struct GenerationExecution<T> {
    result: std::result::Result<T, GenerationStop>,
    queue_wait: Duration,
}

async fn run_generation_with_admission<T, F>(
    admission: &ModelGenerationAdmission,
    preadmitted: Option<Arc<ModelGenerationPermit>>,
    cancellation: &tokio_util::sync::CancellationToken,
    active_timeout: Duration,
    generation: F,
) -> GenerationExecution<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    let queued_at = Instant::now();
    let permit = match preadmitted {
        Some(permit) => permit,
        None => match admission.acquire(cancellation).await {
            Ok(permit) => Arc::new(permit),
            Err(ModelGenerationAdmissionError::Cancelled) => {
                return GenerationExecution {
                    result: Err(GenerationStop::Cancelled),
                    queue_wait: queued_at.elapsed(),
                };
            }
            Err(error) => {
                return GenerationExecution {
                    result: Err(GenerationStop::Failed(anyhow::Error::new(error))),
                    queue_wait: queued_at.elapsed(),
                };
            }
        },
    };
    let queue_wait = permit.queue_wait();
    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(GenerationStop::Cancelled),
        _ = tokio::time::sleep(active_timeout) => Err(GenerationStop::TimedOut),
        result = generation => result.map_err(GenerationStop::Failed),
    };
    drop(permit);
    GenerationExecution { result, queue_wait }
}

/// Run a structured transaction whose individual provider calls are admitted
/// by a governed [`LlmClient`] facade. The outer deadline still covers the
/// complete initial-plus-repair transaction, but no second semaphore is held
/// around it (which would deadlock a single-flight provider while the facade
/// is trying to start its first call).
async fn run_generation_without_admission<T, F>(
    cancellation: &tokio_util::sync::CancellationToken,
    active_timeout: Duration,
    generation: F,
) -> GenerationExecution<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(GenerationStop::Cancelled),
        _ = tokio::time::sleep(active_timeout) => Err(GenerationStop::TimedOut),
        result = generation => result.map_err(GenerationStop::Failed),
    };
    GenerationExecution {
        result,
        queue_wait: Duration::ZERO,
    }
}

fn generation_admission_metadata(
    concurrency: ModelGenerationConcurrency,
    queue_wait: Duration,
    active_timeout_ms: u64,
) -> Value {
    let queue_wait_ms = u64::try_from(queue_wait.as_millis()).unwrap_or(u64::MAX);
    serde_json::json!({
        "mode": "bounded",
        "max_concurrency": concurrency.max_concurrency().get(),
        "queue_wait_ms": queue_wait_ms,
        "active_timeout_ms": active_timeout_ms,
    })
}

fn invalid_argument(message: String) -> ToolOutput {
    ToolOutput::error(&message).with_error_kind(ToolErrorKind::InvalidArgument { message })
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentConfig, AgentLoop};
    use crate::budget::{BudgetDecision, BudgetGuard};
    use crate::llm::structured::{NativeStructuredSupport, StructuredDirective, StructuredMode};
    use crate::llm::{ContentBlock, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition};
    use crate::tools::ToolExecutor;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::{mpsc, Notify};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn generation_failure_kind_uses_typed_status_not_error_prose() {
        let prose = anyhow::anyhow!("Human-readable text says rate limit and too many requests.");
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
    }

    #[test]
    fn generation_failure_message_preserves_context_chain() {
        let error = anyhow::anyhow!("provider detail").context("structured generation failed");

        assert_eq!(
            generation_failure_message(&error),
            "structured generation failed: provider detail"
        );
    }

    struct MockObjectClient {
        response: Mutex<Option<LlmResponse>>,
    }

    impl MockObjectClient {
        fn new(response: LlmResponse) -> Self {
            Self {
                response: Mutex::new(Some(response)),
            }
        }

        fn response() -> LlmResponse {
            LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "emit_colors".to_string(),
                        input: serde_json::json!({ "elements": ["red", "blue"] }),
                    }],
                    reasoning_content: None,
                },
                usage: TokenUsage {
                    prompt_tokens: 11,
                    completion_tokens: 7,
                    total_tokens: 18,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                stop_reason: Some("tool_use".to_string()),
                token_logprobs: Vec::new(),
                meta: None,
            }
        }
    }

    #[derive(Clone)]
    struct TimeoutAwareObjectClient {
        observed_timeout_ms: Arc<AtomicU64>,
        response: Arc<Mutex<Option<LlmResponse>>>,
    }

    #[async_trait]
    impl LlmClient for TimeoutAwareObjectClient {
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
                .expect("timeout-aware response")
                .take()
                .ok_or_else(|| anyhow::anyhow!("response already used"))
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
            anyhow::bail!("streaming is not used in this test")
        }

        fn native_structured_support(&self) -> NativeStructuredSupport {
            NativeStructuredSupport::ForcedTool
        }
    }

    #[async_trait]
    impl LlmClient for MockObjectClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<LlmResponse> {
            self.response
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| anyhow::anyhow!("response already used"))
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
            anyhow::bail!("streaming is not used in this test")
        }

        fn native_structured_support(&self) -> NativeStructuredSupport {
            NativeStructuredSupport::ForcedTool
        }

        async fn complete_structured(
            &self,
            messages: &[Message],
            system: Option<&str>,
            tools: &[ToolDefinition],
            directive: &StructuredDirective,
        ) -> anyhow::Result<LlmResponse> {
            assert_eq!(messages.len(), 1);
            assert!(system.unwrap_or_default().contains("emit_colors"));
            assert_eq!(directive.force_tool.as_deref(), Some("emit_colors"));
            assert_eq!(tools[0].parameters["required"][0], "elements");
            self.complete(messages, system, tools).await
        }
    }

    struct RepairingObjectClient {
        responses: Mutex<Vec<LlmResponse>>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmClient for RepairingObjectClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<LlmResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                anyhow::bail!("no response left")
            }
            Ok(responses.remove(0))
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
            anyhow::bail!("streaming is not used by repair tests")
        }

        fn native_structured_support(&self) -> NativeStructuredSupport {
            NativeStructuredSupport::ForcedTool
        }
    }

    #[derive(Default)]
    struct GenerateObjectBudgetGuard {
        checks: AtomicUsize,
        records: AtomicUsize,
    }

    #[async_trait]
    impl BudgetGuard for GenerateObjectBudgetGuard {
        async fn check_before_llm(
            &self,
            _session_id: &str,
            _estimated_prompt_tokens: usize,
        ) -> BudgetDecision {
            self.checks.fetch_add(1, Ordering::SeqCst);
            BudgetDecision::Allow
        }

        async fn record_after_llm(&self, _session_id: &str, _usage: &TokenUsage) {
            self.records.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct BlockingObjectClient {
        started: Arc<Notify>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmClient for BlockingObjectClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<LlmResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            std::future::pending::<anyhow::Result<LlmResponse>>().await
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
            anyhow::bail!("streaming is not used by cancellation tests")
        }

        fn native_structured_support(&self) -> NativeStructuredSupport {
            NativeStructuredSupport::ForcedTool
        }
    }

    fn object_tool_response(input: Value) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "call".to_string(),
                    name: "emit_result".to_string(),
                    input,
                }],
                reasoning_content: None,
            },
            usage: TokenUsage {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("tool_use".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }

    #[tokio::test]
    async fn admission_queue_wait_does_not_consume_active_generation_timeout() {
        let admission = ModelGenerationAdmission::default();
        let holder = admission
            .acquire(&CancellationToken::new())
            .await
            .expect("hold the only active-generation slot");
        let cancellation = CancellationToken::new();
        let run = tokio::spawn({
            let admission = admission.clone();
            let cancellation = cancellation.clone();
            async move {
                run_generation_with_admission(
                    &admission,
                    None,
                    &cancellation,
                    Duration::from_millis(20),
                    async { Ok::<_, anyhow::Error>("completed") },
                )
                .await
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !run.is_finished(),
            "admission queue time must not consume the active deadline"
        );
        drop(holder);

        let execution = tokio::time::timeout(Duration::from_millis(100), run)
            .await
            .expect("generation should start after admission")
            .expect("generation task should join");
        assert!(execution.queue_wait >= Duration::from_millis(40));
        match execution.result {
            Ok(value) => assert_eq!(value, "completed"),
            Err(_) => panic!("queued generation should retain its full active deadline"),
        }
    }

    #[tokio::test]
    async fn generate_object_tool_unwraps_array_schema_and_sets_metadata() {
        let tool = GenerateObjectTool::new(Arc::new(MockObjectClient::new(
            MockObjectClient::response(),
        )));
        let temp = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let output = tool
            .execute(
                &serde_json::json!({
                    "schema_name": "colors",
                    "schema": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 2
                    },
                    "prompt": "Return two colors",
                    "mode": "tool"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(output.success);
        let content: Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(content["object"], serde_json::json!(["red", "blue"]));
        assert_eq!(
            content["mode_used"],
            serde_json::json!(StructuredMode::Tool)
        );
        assert_eq!(content["usage"]["total_tokens"], 18);

        let metadata = output.metadata.unwrap();
        assert_eq!(metadata["schema_name"], "colors");
        assert_eq!(metadata["requested_mode"], "tool");
        assert_eq!(metadata["raw_text_included"], false);
        assert_eq!(
            metadata["generation_admission"]["max_concurrency"],
            serde_json::json!(1)
        );
        assert_eq!(
            metadata["generation_admission"]["active_timeout_ms"],
            serde_json::json!(DEFAULT_TIMEOUT_MS)
        );
    }

    #[tokio::test]
    async fn generate_object_propagates_its_active_timeout_through_the_invocation_gateway() {
        let temp = tempfile::tempdir().unwrap();
        let observed_timeout_ms = Arc::new(AtomicU64::new(0));
        let raw_client: Arc<dyn LlmClient> = Arc::new(TimeoutAwareObjectClient {
            observed_timeout_ms: Arc::clone(&observed_timeout_ms),
            response: Arc::new(Mutex::new(Some(object_tool_response(
                serde_json::json!({"value": "ok"}),
            )))),
        });
        let agent = AgentLoop::new(
            Arc::clone(&raw_client),
            Arc::new(ToolExecutor::new(temp.path().to_string_lossy().to_string())),
            ToolContext::new(temp.path().to_path_buf()),
            AgentConfig::default(),
        );
        let cancellation = CancellationToken::new();
        let governed =
            agent.scoped_llm_client_for_parts(Some("timeout-session"), &None, &cancellation);
        let ctx = ToolContext::new(temp.path().to_path_buf())
            .with_session_id("timeout-session")
            .with_cancellation(cancellation)
            .with_llm_client(governed);
        let tool = GenerateObjectTool::new(raw_client);
        let requested_timeout_ms = DEFAULT_TIMEOUT_MS / 2;

        let output = tool
            .execute(
                &serde_json::json!({
                    "schema": {
                        "type": "object",
                        "properties": {"value": {"type": "string"}},
                        "required": ["value"]
                    },
                    "prompt": "Return a value",
                    "mode": "tool",
                    "timeout_ms": requested_timeout_ms
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(output.success, "{}", output.content);
        assert_eq!(
            observed_timeout_ms.load(Ordering::SeqCst),
            requested_timeout_ms
        );
    }

    #[tokio::test]
    async fn generate_object_repairs_use_the_tool_context_llm_budget_scope() {
        let temp = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let raw_client: Arc<dyn LlmClient> = Arc::new(RepairingObjectClient {
            responses: Mutex::new(vec![
                object_tool_response(serde_json::json!({})),
                object_tool_response(serde_json::json!({"value": "ok"})),
            ]),
            calls: Arc::clone(&calls),
        });
        let guard = Arc::new(GenerateObjectBudgetGuard::default());
        let agent = AgentLoop::new(
            Arc::clone(&raw_client),
            Arc::new(ToolExecutor::new(temp.path().to_string_lossy().to_string())),
            ToolContext::new(temp.path().to_path_buf()),
            AgentConfig {
                budget_guard: Some(Arc::clone(&guard) as Arc<dyn BudgetGuard>),
                ..Default::default()
            },
        );
        let cancellation = CancellationToken::new();
        let event_tx = None;
        let governed =
            agent.scoped_llm_client_for_parts(Some("generate-session"), &event_tx, &cancellation);
        let ctx = ToolContext::new(temp.path().to_path_buf())
            .with_session_id("generate-session")
            .with_cancellation(cancellation)
            .with_llm_client(governed);
        let tool = GenerateObjectTool::new(raw_client);

        let output = tool
            .execute(
                &serde_json::json!({
                    "schema": {
                        "type": "object",
                        "properties": {"value": {"type": "string"}},
                        "required": ["value"]
                    },
                    "prompt": "Return a value",
                    "mode": "tool",
                    "max_repair_attempts": 1
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(
            output.success,
            "generate_object should repair: {}",
            output.content
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(guard.checks.load(Ordering::SeqCst), 2);
        assert_eq!(guard.records.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn generate_object_stops_on_tool_context_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let started = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = Arc::new(GenerateObjectTool::new(Arc::new(BlockingObjectClient {
            started: Arc::clone(&started),
            calls: Arc::clone(&calls),
        })));
        let cancellation = CancellationToken::new();
        let ctx =
            ToolContext::new(temp.path().to_path_buf()).with_cancellation(cancellation.clone());
        let started_wait = started.notified();
        let running_tool = Arc::clone(&tool);
        let run = tokio::spawn(async move {
            running_tool
                .execute(
                    &serde_json::json!({
                        "schema": {
                            "type": "object",
                            "properties": {"value": {"type": "string"}},
                            "required": ["value"]
                        },
                        "prompt": "Wait forever",
                        "mode": "tool"
                    }),
                    &ctx,
                )
                .await
        });

        tokio::time::timeout(Duration::from_secs(1), started_wait)
            .await
            .expect("structured provider call should start");
        cancellation.cancel();
        let output = tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .expect("cancellation must stop structured generation")
            .expect("generate_object join should succeed")
            .expect("generate_object should return a typed failed output");

        assert!(!output.success);
        assert!(output.content.contains("cancelled"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let replacement = tokio::time::timeout(
            Duration::from_millis(100),
            tool.admission.acquire(&CancellationToken::new()),
        )
        .await
        .expect("cancellation must release active-generation admission")
        .expect("replacement permit");
        drop(replacement);
    }
}

#[cfg(test)]
#[path = "generate_object_contract_tests.rs"]
mod contract_tests;
