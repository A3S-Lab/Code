//! Built-in `generate_object` tool for structured JSON output.
//!
//! This tool allows the agent (or users via `session.tool()`) to generate a
//! JSON object that conforms to a given JSON Schema. It supports streaming
//! partial objects via `ToolStreamEvent::OutputDelta`.

use crate::llm::structured::{self, PartialObjectCallback, StructuredMode, StructuredRequest};
use crate::llm::LlmClient;
use crate::tools::types::{Tool, ToolContext, ToolOutput, ToolStreamEvent};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct GenerateObjectTool {
    llm_client: Arc<dyn LlmClient>,
}

impl GenerateObjectTool {
    pub fn new(llm_client: Arc<dyn LlmClient>) -> Self {
        Self { llm_client }
    }
}

#[async_trait]
impl Tool for GenerateObjectTool {
    fn name(&self) -> &str {
        "generate_object"
    }

    fn description(&self) -> &str {
        "Generate a JSON object that strictly conforms to a provided JSON Schema. \
         Use when you need structured output: extracting fields from text, classifying \
         data, converting natural language to typed records, or producing machine-readable \
         results. Returns the validated object on success."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["schema", "prompt"],
            "additionalProperties": false,
            "properties": {
                "schema": {
                    "type": "object",
                    "description": "JSON Schema that the output object must conform to"
                },
                "schema_name": {
                    "type": "string",
                    "description": "Short name for the schema (used internally for tool naming)",
                    "default": "result"
                },
                "schema_description": {
                    "type": "string",
                    "description": "Optional description of what the schema represents"
                },
                "prompt": {
                    "type": "string",
                    "description": "The prompt describing what object to generate or extract"
                },
                "system": {
                    "type": "string",
                    "description": "Optional system prompt to guide generation"
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
                }
            }
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let schema = match args.get("schema") {
            Some(s) if s.is_object() => s.clone(),
            Some(_) => {
                return Ok(ToolOutput::error(
                    "'schema' must be a JSON object (a valid JSON Schema)",
                ));
            }
            None => {
                return Ok(ToolOutput::error("'schema' parameter is required"));
            }
        };

        let prompt = match args.get("prompt").and_then(|v| v.as_str()) {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => {
                return Ok(ToolOutput::error(
                    "'prompt' parameter is required and must be non-empty",
                ));
            }
        };

        // Validate schema has at minimum a "type" or "properties" or "anyOf" field
        if schema.get("type").is_none()
            && schema.get("properties").is_none()
            && schema.get("anyOf").is_none()
            && schema.get("oneOf").is_none()
            && schema.get("enum").is_none()
        {
            return Ok(ToolOutput::error(
                "'schema' should contain at least one of: type, properties, anyOf, oneOf, or enum",
            ));
        }

        let schema_name: String = args
            .get("schema_name")
            .and_then(|v| v.as_str())
            .unwrap_or("result")
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .take(64)
            .collect();
        let schema_name = if schema_name.is_empty() {
            "result".to_string()
        } else {
            schema_name
        };

        let schema_description = args
            .get("schema_description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let system = args
            .get("system")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let requested_mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("auto");
        let mode = match requested_mode {
            "strict" => StructuredMode::Strict,
            "json" => StructuredMode::Json,
            "tool" => StructuredMode::Tool,
            "prompt" => StructuredMode::Prompt,
            "auto" => StructuredMode::Auto,
            other => {
                return Ok(ToolOutput::error(format!(
                    "'mode' must be one of auto, strict, json, tool, or prompt; got '{other}'"
                )));
            }
        };

        // Mode resolution is delegated to the structured engine, which inspects
        // the client's native capability. Unsupported native modes safely fall
        // back to prompt+schema parsing instead of sending provider parameters
        // that some OpenAI-compatible endpoints hang on.
        let max_repair_attempts = args
            .get("max_repair_attempts")
            .and_then(|v| v.as_u64())
            .unwrap_or(2)
            .min(5) as u8;
        let include_raw_text = args
            .get("include_raw_text")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let req = StructuredRequest {
            prompt,
            system,
            schema,
            schema_name: schema_name.clone(),
            schema_description,
            mode,
            max_repair_attempts,
        };

        let result = if let Some(ref tx) = ctx.event_tx {
            let tx_clone = tx.clone();
            let callback: PartialObjectCallback = Box::new(move |partial: &Value| {
                let delta = serde_json::json!({
                    "object_partial": partial,
                    "final": false,
                });
                let delta_str = serde_json::to_string(&delta).unwrap_or_default();
                let _ = tx_clone.try_send(ToolStreamEvent::OutputDelta(delta_str));
            });
            structured::generate_streaming(&*self.llm_client, &req, callback).await
        } else {
            structured::generate_blocking(&*self.llm_client, &req).await
        };

        match result {
            Ok(sr) => {
                if let Some(ref tx) = ctx.event_tx {
                    let final_delta = serde_json::json!({
                        "object_partial": sr.object,
                        "final": true,
                        "mode_used": sr.mode_used,
                        "repair_rounds": sr.repair_rounds,
                    });
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
                });
                Ok(ToolOutput::success(serde_json::to_string(&output)?).with_metadata(metadata))
            }
            Err(e) => Ok(ToolOutput::error(format!("generate_object failed: {}", e))
                .with_metadata(serde_json::json!({
                    "schema_name": schema_name,
                    "requested_mode": requested_mode,
                    "mode_requested": mode,
                }))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::structured::{NativeStructuredSupport, StructuredDirective, StructuredMode};
    use crate::llm::{ContentBlock, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition};
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

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
    }
}
