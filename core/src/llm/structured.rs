//! Structured object generation from LLM output.
//!
//! Provides reliable JSON object generation with schema validation, automatic
//! repair, and streaming partial object support. Works across all providers by
//! selecting the best available mode (strict JSON schema, json_mode, tool-call,
//! or prompt-only).

use super::{LlmClient, Message, StreamEvent, TokenUsage, ToolDefinition};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

mod partial_json;
use partial_json::parse_partial_json;
#[cfg(test)]
use partial_json::try_parse_partial_json;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Mode selection for structured output generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredMode {
    /// Auto-select best mode based on provider capabilities.
    Auto,
    /// OpenAI native strict JSON schema (response_format.type = json_schema).
    Strict,
    /// OpenAI json_object mode (guarantees valid JSON, not schema-conformant).
    Json,
    /// Use tool-calling: inject a synthetic tool whose parameters IS the schema.
    /// Works on all providers that support tool use (Anthropic, OpenAI, etc).
    Tool,
    /// Prompt-only: append schema instructions to the prompt. Least reliable.
    Prompt,
}

/// Request specification for structured object generation.
#[derive(Debug, Clone)]
pub struct StructuredRequest {
    pub prompt: String,
    pub system: Option<String>,
    pub schema: Value,
    pub schema_name: String,
    pub schema_description: Option<String>,
    pub mode: StructuredMode,
    pub max_repair_attempts: u8,
}

/// Result of a successful structured generation.
#[derive(Debug, Clone, Serialize)]
pub struct StructuredResult {
    pub object: Value,
    pub raw_text: Option<String>,
    pub usage: TokenUsage,
    pub repair_rounds: u8,
    pub mode_used: StructuredMode,
}

/// Provider-native structured-output capability.
///
/// Each [`LlmClient`] reports this so the structured engine can request the
/// strongest enforcement the provider actually supports. Defaults to
/// [`NativeStructuredSupport::None`] for clients that don't override it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStructuredSupport {
    /// No native enforcement — rely on prompt instructions + lenient extraction.
    None,
    /// Can force a specific tool call (Anthropic `tool_choice`, OpenAI function
    /// `tool_choice`). Guarantees the model emits the structured tool call
    /// instead of free-form prose.
    ForcedTool,
    /// Supports OpenAI-style `response_format` (`json_object` and
    /// `json_schema` + `strict`) in addition to forced tool calls.
    JsonSchema,
}

/// A native `response_format` request for OpenAI-compatible providers.
#[derive(Debug, Clone, PartialEq)]
pub enum ResponseFormat {
    /// `{"type":"json_object"}` — guarantees syntactically valid JSON, but not
    /// schema conformance.
    JsonObject,
    /// `{"type":"json_schema","json_schema":{name,schema,strict:true}}` —
    /// parser-enforced schema conformance.
    JsonSchema { name: String, schema: Value },
}

/// Instruction telling a provider how to enforce structured output for a call.
///
/// Carries the union of intents; each provider honors what it supports and
/// ignores the rest (e.g. Anthropic has no `response_format`, so it only acts
/// on `force_tool`). The default (`force_tool: None, response_format: None`)
/// reproduces an ordinary completion, which is why the trait's default
/// `complete_structured` impl is behavior-preserving.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StructuredDirective {
    /// Force the model to call exactly this tool (provider `tool_choice`).
    pub force_tool: Option<String>,
    /// Request a provider-native `response_format` (OpenAI-compatible only).
    pub response_format: Option<ResponseFormat>,
}

/// Callback for streaming partial object snapshots.
pub type PartialObjectCallback = Box<dyn Fn(&Value) + Send>;

/// Provider-facing schema envelope.
///
/// Function/tool parameters are most reliable when the top-level schema is an
/// object. Inspired by Vercel AI SDK's `Output.array` / `Output.choice`
/// wrappers, A3S sends top-level arrays and scalar schemas inside a small object
/// envelope, then unwraps the validated value before returning it to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaEnvelope {
    Direct,
    Elements,
    Value,
}

impl SchemaEnvelope {
    fn for_schema(schema: &Value) -> Self {
        if schema_is_object_like(schema) {
            Self::Direct
        } else if schema.get("type").and_then(Value::as_str) == Some("array") {
            Self::Elements
        } else {
            Self::Value
        }
    }

    fn response_schema(self, schema: &Value) -> Value {
        match self {
            Self::Direct => schema.clone(),
            Self::Elements => serde_json::json!({
                "type": "object",
                "required": ["elements"],
                "additionalProperties": false,
                "properties": {
                    "elements": schema
                }
            }),
            Self::Value => serde_json::json!({
                "type": "object",
                "required": ["value"],
                "additionalProperties": false,
                "properties": {
                    "value": schema
                }
            }),
        }
    }

    fn unwrap_final(self, value: &Value) -> Option<Value> {
        match self {
            Self::Direct => Some(value.clone()),
            Self::Elements => value.get("elements").cloned(),
            Self::Value => value.get("value").cloned(),
        }
    }

    fn project_partial(self, value: &Value, repaired: bool) -> Option<Value> {
        match self {
            Self::Direct => Some(value.clone()),
            Self::Elements => {
                let mut elements = value.get("elements")?.as_array()?.clone();
                // A repaired parse may include a synthetic last element that was
                // closed only so the partial JSON can parse. Match Vercel's
                // array streaming behavior: publish only completed elements.
                if repaired && !elements.is_empty() {
                    elements.pop();
                }
                Some(Value::Array(elements))
            }
            Self::Value => value.get("value").cloned(),
        }
    }

    fn instruction(self) -> &'static str {
        match self {
            Self::Direct => "",
            Self::Elements => {
                "The provider-facing response schema wraps the requested array in an `elements` field. Follow that schema exactly; callers receive the unwrapped array."
            }
            Self::Value => {
                "The provider-facing response schema wraps the requested scalar/enum value in a `value` field. Follow that schema exactly; callers receive the unwrapped value."
            }
        }
    }
}

fn schema_is_object_like(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("object")
        || schema.get("properties").is_some()
        || schema.get("required").is_some()
        || schema.get("additionalProperties").is_some()
}

// ---------------------------------------------------------------------------
// Core generation: blocking (non-streaming)
// ---------------------------------------------------------------------------

/// Generate a structured JSON object using the given LLM client.
///
/// Selects the best mode based on `req.mode`, calls the LLM, validates against
/// the schema, and retries with repair prompts if validation fails.
pub async fn generate_blocking(
    client: &dyn LlmClient,
    req: &StructuredRequest,
) -> Result<StructuredResult> {
    let mode = resolve_mode(req.mode, client.native_structured_support());
    let envelope = SchemaEnvelope::for_schema(&req.schema);
    let mut messages = build_initial_messages(req, mode);
    let system = build_system_prompt(req, mode);
    let tools = build_tools(req, mode);
    let directive = build_directive(req, mode);

    let mut total_usage = TokenUsage::default();
    let mut repair_rounds: u8 = 0;

    loop {
        let resp = client
            .complete_structured(&messages, Some(&system), &tools, &directive)
            .await
            .context("LLM call failed during structured generation")?;

        accumulate_usage(&mut total_usage, &resp.usage);

        // Mine the object from every place a model might have parked it (tool call,
        // text content, AND the reasoning channel), trying each balanced JSON
        // candidate against the schema. Reasoning models routinely leave `content`
        // empty and emit the object inside `reasoning`, so without the reasoning
        // fallback generate_object failed with "no structured output" across models.
        let candidates = extract_raw_candidates(&resp.message, mode);
        let resolution = resolve_structured(&candidates, &req.schema, envelope);

        if let Some((value, raw)) = resolution.valid {
            return Ok(StructuredResult {
                object: value,
                raw_text: Some(raw),
                usage: total_usage,
                repair_rounds,
                mode_used: mode,
            });
        }

        if repair_rounds >= req.max_repair_attempts {
            return Err(match resolution.invalid {
                Some((_, errors)) => anyhow::anyhow!(
                    "Structured output failed schema validation after {} repair attempts. Errors: {}",
                    repair_rounds,
                    errors.join("; ")
                ),
                None => anyhow::anyhow!(
                    "Structured output parsing failed after {} repair attempts: no JSON object found in tool call, text content, or reasoning channel",
                    repair_rounds
                ),
            });
        }

        repair_rounds += 1;
        let (repair_msg, raw_for_ctx) = match resolution.invalid {
            Some((raw, errors)) => (build_repair_message(&raw, &errors), raw),
            None => {
                let raw = resolution.raw_seen.unwrap_or_default();
                (build_parse_failure_repair(&raw), raw)
            }
        };
        append_repair_context(
            &mut messages,
            &resp.message,
            &repair_msg,
            mode,
            &raw_for_ctx,
        );
    }
}

// ---------------------------------------------------------------------------
// Core generation: streaming
// ---------------------------------------------------------------------------

/// Generate a structured JSON object with streaming partial updates.
///
/// Calls `on_partial` with progressively more complete partial objects as tokens
/// arrive. Returns the final validated object.
///
/// A streamed first attempt may be followed by bounded non-streaming repair
/// calls when `max_repair_attempts` is non-zero. Repair calls publish only the
/// final corrected object, avoiding a second misleading partial stream.
pub async fn generate_streaming(
    client: &dyn LlmClient,
    req: &StructuredRequest,
    on_partial: PartialObjectCallback,
) -> Result<StructuredResult> {
    let mode = resolve_mode(req.mode, client.native_structured_support());
    let envelope = SchemaEnvelope::for_schema(&req.schema);
    let mut messages = build_initial_messages(req, mode);
    let system = build_system_prompt(req, mode);
    let tools = build_tools(req, mode);
    let directive = build_directive(req, mode);

    let cancel_token = CancellationToken::new();
    let mut rx = client
        .complete_streaming_structured(
            &messages,
            Some(&system),
            &tools,
            &directive,
            cancel_token.clone(),
        )
        .await
        .context("LLM streaming call failed during structured generation")?;

    let mut json_buffer = String::new();
    let mut last_valid_partial: Option<Value> = None;
    let mut final_response: Option<super::LlmResponse> = None;
    let mut last_parse_len: usize = 0;
    let mut complete_candidate: Option<(Value, String, tokio::time::Instant)> = None;
    // Minimum bytes of new data before attempting a partial parse (reduces CPU)
    const PARSE_THRESHOLD: usize = 8;
    // Well-behaved providers send Done immediately after the complete object.
    // A short grace preserves their final usage metadata while preventing an
    // otherwise valid result from hanging on a compatible endpoint that never
    // terminates its stream.
    const DONE_GRACE: std::time::Duration = std::time::Duration::from_millis(250);
    loop {
        let event = if let Some((_, _, deadline)) = complete_candidate.as_ref() {
            tokio::select! {
                event = rx.recv() => event,
                _ = tokio::time::sleep_until(*deadline) => {
                    let candidate = complete_candidate
                        .take()
                        .expect("complete streamed candidate exists");
                    let (value, raw_text, _) = candidate;
                    cancel_token.cancel();
                    on_partial(&value);
                    return Ok(StructuredResult {
                        object: value,
                        raw_text: Some(raw_text),
                        usage: TokenUsage::default(),
                        repair_rounds: 0,
                        mode_used: mode,
                    });
                }
            }
        } else {
            rx.recv().await
        };
        let Some(event) = event else {
            if let Some((value, raw_text, _)) = complete_candidate.take() {
                cancel_token.cancel();
                on_partial(&value);
                return Ok(StructuredResult {
                    object: value,
                    raw_text: Some(raw_text),
                    usage: TokenUsage::default(),
                    repair_rounds: 0,
                    mode_used: mode,
                });
            }
            break;
        };
        match event {
            StreamEvent::ToolUseInputDelta { delta, .. } if mode == StructuredMode::Tool => {
                if final_response.is_some() {
                    continue;
                }
                json_buffer.push_str(&delta);
                if json_buffer.len() - last_parse_len >= PARSE_THRESHOLD {
                    if let Some(partial) = parse_partial_json(&json_buffer) {
                        if let Some(projected) =
                            envelope.project_partial(&partial.value, partial.repaired)
                        {
                            if last_valid_partial.as_ref() != Some(&projected) {
                                on_partial(&projected);
                                last_valid_partial = Some(projected);
                            }
                        }
                    }
                    last_parse_len = json_buffer.len();
                }
                if complete_candidate.is_none() && (delta.contains('}') || delta.contains(']')) {
                    complete_candidate = resolve_structured(
                        std::slice::from_ref(&json_buffer),
                        &req.schema,
                        envelope,
                    )
                    .valid
                    .map(|(value, raw_text)| {
                        (value, raw_text, tokio::time::Instant::now() + DONE_GRACE)
                    });
                }
            }
            StreamEvent::TextDelta(delta) if mode != StructuredMode::Tool => {
                if final_response.is_some() {
                    continue;
                }
                json_buffer.push_str(&delta);
                if json_buffer.len() - last_parse_len >= PARSE_THRESHOLD {
                    if let Some(json_start) = find_json_start(&json_buffer) {
                        let candidate = &json_buffer[json_start..];
                        if let Some(partial) = parse_partial_json(candidate) {
                            if let Some(projected) =
                                envelope.project_partial(&partial.value, partial.repaired)
                            {
                                if last_valid_partial.as_ref() != Some(&projected) {
                                    on_partial(&projected);
                                    last_valid_partial = Some(projected);
                                }
                            }
                        }
                    }
                    last_parse_len = json_buffer.len();
                }
                if complete_candidate.is_none() && (delta.contains('}') || delta.contains(']')) {
                    complete_candidate = resolve_structured(
                        std::slice::from_ref(&json_buffer),
                        &req.schema,
                        envelope,
                    )
                    .valid
                    .map(|(value, raw_text)| {
                        (value, raw_text, tokio::time::Instant::now() + DONE_GRACE)
                    });
                }
            }
            StreamEvent::Done(resp) => {
                final_response = Some(resp);
                break;
            }
            _ => {}
        }
    }

    let mut resp = final_response.context("Stream ended without Done event")?;
    let mut total_usage = TokenUsage::default();
    accumulate_usage(&mut total_usage, &resp.usage);
    let mut repair_rounds = 0u8;
    // Same multi-source resolution as the blocking path: the final message may carry
    // the object in the tool call, the text content, or the reasoning channel.
    let mut resolution = resolve_structured(
        &extract_raw_candidates(&resp.message, mode),
        &req.schema,
        envelope,
    );
    let (value, raw_text) = loop {
        if let Some(valid) = resolution.valid.take() {
            break valid;
        }

        if repair_rounds >= req.max_repair_attempts {
            return Err(match resolution.invalid {
                Some((_, errors)) => anyhow::anyhow!(
                    "Streamed structured output failed schema validation after {} repair attempts: {}",
                    repair_rounds,
                    errors.join("; ")
                ),
                None => anyhow::anyhow!(
                    "Streamed output produced no parseable JSON object after {} repair attempts (checked tool call, text content, and reasoning channel)",
                    repair_rounds
                ),
            });
        }

        repair_rounds += 1;
        let (repair_message, raw_for_context) = match resolution.invalid.take() {
            Some((raw, errors)) => (build_repair_message(&raw, &errors), raw),
            None => {
                let raw = resolution.raw_seen.take().unwrap_or_default();
                (build_parse_failure_repair(&raw), raw)
            }
        };
        append_repair_context(
            &mut messages,
            &resp.message,
            &repair_message,
            mode,
            &raw_for_context,
        );
        resp = client
            .complete_structured(&messages, Some(&system), &tools, &directive)
            .await
            .context("LLM call failed while repairing streamed structured output")?;
        accumulate_usage(&mut total_usage, &resp.usage);
        resolution = resolve_structured(
            &extract_raw_candidates(&resp.message, mode),
            &req.schema,
            envelope,
        );
    };

    // Emit final complete object
    on_partial(&value);

    Ok(StructuredResult {
        object: value,
        raw_text: Some(raw_text),
        usage: total_usage,
        repair_rounds,
        mode_used: mode,
    })
}

// ---------------------------------------------------------------------------
// JSON extraction and parsing
// ---------------------------------------------------------------------------

/// Extract a JSON value from potentially dirty LLM output.
///
/// Handles: raw JSON, markdown code fences, leading/trailing prose.
pub fn extract_json_value(text: &str) -> Result<Value> {
    let trimmed = text.trim();

    // 1. Direct parse
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if v.is_object() || v.is_array() {
            return Ok(v);
        }
    }

    // 2. Strip markdown code fence
    if let Some(inner) = strip_code_fence(trimmed) {
        if let Ok(v) = serde_json::from_str::<Value>(inner.trim()) {
            if v.is_object() || v.is_array() {
                return Ok(v);
            }
        }
    }

    // 3. Find balanced JSON substring (first { to matching })
    if let Some(candidate) = find_balanced_json_object(trimmed) {
        if let Ok(v) = serde_json::from_str::<Value>(candidate) {
            return Ok(v);
        }
    }

    // 4. Try array
    if let Some(candidate) = find_balanced_json_array(trimmed) {
        if let Ok(v) = serde_json::from_str::<Value>(candidate) {
            return Ok(v);
        }
    }

    bail!("No valid JSON object found in LLM output")
}

/// Strip ```json ... ``` or ``` ... ``` fences.
fn strip_code_fence(text: &str) -> Option<&str> {
    let start_patterns = ["```json\n", "```json\r\n", "```\n", "```\r\n"];
    for pat in &start_patterns {
        if let Some(rest) = text.strip_prefix(pat) {
            // Find closing fence
            if let Some(end) = rest.rfind("```") {
                return Some(&rest[..end]);
            }
        }
    }
    // Also handle inline: ```json{...}```
    if let Some(inner) = text.strip_prefix("```json") {
        if let Some(end) = inner.rfind("```") {
            return Some(inner[..end].trim());
        }
    }
    if let Some(inner) = text.strip_prefix("```") {
        if let Some(end) = inner.rfind("```") {
            return Some(inner[..end].trim());
        }
    }
    None
}

/// Find the first balanced `{...}` substring using bracket counting.
fn find_balanced_json_object(text: &str) -> Option<&str> {
    find_balanced(text, '{', '}')
}

/// Find the first balanced `[...]` substring.
fn find_balanced_json_array(text: &str) -> Option<&str> {
    find_balanced(text, '[', ']')
}

fn find_balanced(text: &str, open: char, close: char) -> Option<&str> {
    find_balanced_range(text, open, close).map(|(start, end)| &text[start..end])
}

/// Byte range `[start, end)` of the first balanced `open..close` substring (quote-aware).
fn find_balanced_range(text: &str, open: char, close: char) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let open_byte = open as u8;
    let close_byte = close as u8;

    // Find the first unquoted occurrence of `open`
    let mut in_string = false;
    let mut escape_next = false;
    let mut start = None;

    for (i, &b) in bytes.iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match b {
            b'\\' if in_string => escape_next = true,
            b'"' => in_string = !in_string,
            _ if in_string => {}
            _ if b == open_byte => {
                start = Some(i);
                break;
            }
            _ => {}
        }
    }

    let start = start?;
    let mut depth = 0i32;
    in_string = false;
    escape_next = false;

    for (i, &b) in bytes[start..].iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match b {
            b'\\' if in_string => escape_next = true,
            b'"' => in_string = !in_string,
            _ if in_string => {}
            _ if b == open_byte => depth += 1,
            _ if b == close_byte => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, start + i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// Every top-level balanced `open..close` substring, in document order.
///
/// Reasoning traces often contain several objects (worked examples, partial drafts)
/// before the final answer, so callers validate each against the schema and keep the
/// one that fits rather than blindly trusting the first `{...}`.
fn find_all_balanced(text: &str, open: char, close: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut base = 0usize;
    while base < text.len() {
        match find_balanced_range(&text[base..], open, close) {
            Some((start, end)) => {
                out.push(text[base + start..base + end].to_string());
                base += end;
            }
            None => break,
        }
    }
    out
}

/// Find the byte offset where JSON content starts in a text stream.
/// Skips leading prose/whitespace to find `{` or `[` that isn't inside a string.
fn find_json_start(text: &str) -> Option<usize> {
    // Skip past code fence markers if present
    let (search_text, offset) = if let Some(rest) = text.strip_prefix("```json") {
        (rest, 7)
    } else if let Some(rest) = text.strip_prefix("```") {
        (rest, 3)
    } else {
        (text, 0)
    };

    let mut in_string = false;
    let mut escape_next = false;
    for (i, &b) in search_text.as_bytes().iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match b {
            b'\\' if in_string => {
                escape_next = true;
            }
            b'"' => {
                in_string = !in_string;
            }
            b'{' | b'[' if !in_string => {
                return Some(offset + i);
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Schema validation
// ---------------------------------------------------------------------------

/// Validate a JSON value against a JSON Schema.
/// Returns Ok(()) on success, or a list of human-readable error strings.
fn validate_against_schema(value: &Value, schema: &Value) -> Result<(), Vec<String>> {
    // Structured-output schemas are host/model input, so compilation is kept
    // entirely in-memory: the dependency is built without HTTP/file resolvers.
    // Local `$ref` / `$defs`, composition keywords, conditional schemas, and
    // exact `oneOf` semantics are handled by the standards-compliant validator.
    let validator = jsonschema::draft202012::options()
        .build(schema)
        .map_err(|error| vec![format!("invalid JSON Schema: {error}")])?;
    let errors = validator
        .iter_errors(value)
        .map(|error| {
            let path = error.instance_path().to_string();
            if path.is_empty() {
                format!("$: {error}")
            } else {
                format!("{path}: {error}")
            }
        })
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Message/prompt construction helpers
// ---------------------------------------------------------------------------

/// Resolve the requested mode against the provider's native capability.
///
/// Prefer native enforcement only when the client explicitly reports support.
/// Unknown OpenAI-compatible endpoints can hang when sent `tool_choice` or
/// `response_format`, so unsupported requests degrade to prompt+schema parsing
/// instead of optimistic native parameters.
fn resolve_mode(requested: StructuredMode, support: NativeStructuredSupport) -> StructuredMode {
    match (requested, support) {
        (StructuredMode::Prompt, _) => StructuredMode::Prompt,
        (StructuredMode::Strict, NativeStructuredSupport::JsonSchema) => StructuredMode::Strict,
        (StructuredMode::Json, NativeStructuredSupport::JsonSchema) => StructuredMode::Json,
        (StructuredMode::Auto | StructuredMode::Tool, NativeStructuredSupport::JsonSchema) => {
            StructuredMode::Tool
        }
        (
            StructuredMode::Auto
            | StructuredMode::Tool
            | StructuredMode::Strict
            | StructuredMode::Json,
            NativeStructuredSupport::ForcedTool,
        ) => StructuredMode::Tool,
        (
            StructuredMode::Auto
            | StructuredMode::Tool
            | StructuredMode::Strict
            | StructuredMode::Json,
            NativeStructuredSupport::None,
        ) => StructuredMode::Prompt,
    }
}

/// Build the provider directive for an already-resolved mode.
fn build_directive(req: &StructuredRequest, mode: StructuredMode) -> StructuredDirective {
    match mode {
        StructuredMode::Tool => StructuredDirective {
            force_tool: Some(format!("emit_{}", req.schema_name)),
            response_format: None,
        },
        StructuredMode::Strict => StructuredDirective {
            force_tool: None,
            response_format: Some(ResponseFormat::JsonSchema {
                name: req.schema_name.clone(),
                schema: SchemaEnvelope::for_schema(&req.schema).response_schema(&req.schema),
            }),
        },
        StructuredMode::Json => StructuredDirective {
            force_tool: None,
            response_format: Some(ResponseFormat::JsonObject),
        },
        StructuredMode::Auto | StructuredMode::Prompt => StructuredDirective::default(),
    }
}

fn build_initial_messages(req: &StructuredRequest, mode: StructuredMode) -> Vec<Message> {
    let envelope = SchemaEnvelope::for_schema(&req.schema);
    let response_schema = envelope.response_schema(&req.schema);
    let envelope_instruction = envelope.instruction();
    match mode {
        StructuredMode::Tool => {
            // For tool mode, the prompt is the user message; the LLM will respond
            // with a tool call whose input is the structured object.
            vec![Message::user(&req.prompt)]
        }
        StructuredMode::Prompt | StructuredMode::Json => {
            // Prompt mode and json_object mode both need the schema in the prompt:
            // json_object only guarantees *syntactic* validity, so the model still
            // has to be told the shape it should produce.
            let augmented = format!(
                "{}\n\n{}{}\n\nYou MUST respond with ONLY a valid JSON object (no markdown, no explanation) that conforms to this JSON Schema:\n\n```json\n{}\n```",
                req.prompt,
                envelope_instruction,
                if envelope_instruction.is_empty() { "" } else { "\n" },
                serde_json::to_string_pretty(&response_schema).unwrap_or_default()
            );
            vec![Message::user(&augmented)]
        }
        _ => {
            // Strict mode: the schema constraint is enforced by the provider via
            // response_format.json_schema, so the user message is just the prompt.
            vec![Message::user(&req.prompt)]
        }
    }
}

fn build_system_prompt(req: &StructuredRequest, mode: StructuredMode) -> String {
    let base = req.system.as_deref().unwrap_or("");
    let envelope_instruction = SchemaEnvelope::for_schema(&req.schema).instruction();

    match mode {
        StructuredMode::Tool => {
            format!(
                "{}{}You MUST respond by calling the `emit_{}` tool exactly once with a valid argument matching the schema. Do not output any text outside the tool call.{}{}",
                base,
                if base.is_empty() { "" } else { "\n\n" },
                req.schema_name,
                if envelope_instruction.is_empty() { "" } else { "\n\n" },
                envelope_instruction
            )
        }
        StructuredMode::Prompt | StructuredMode::Json => {
            format!(
                "{}{}You are a structured data extraction assistant. Always respond with valid JSON only, no markdown fences, no explanation text.{}{}",
                base,
                if base.is_empty() { "" } else { "\n\n" },
                if envelope_instruction.is_empty() { "" } else { "\n\n" },
                envelope_instruction,
            )
        }
        _ => base.to_string(),
    }
}

fn build_tools(req: &StructuredRequest, mode: StructuredMode) -> Vec<ToolDefinition> {
    match mode {
        StructuredMode::Tool => {
            vec![ToolDefinition {
                name: format!("emit_{}", req.schema_name),
                description: req
                    .schema_description
                    .clone()
                    .unwrap_or_else(|| format!("Emit a structured {} object", req.schema_name)),
                parameters: SchemaEnvelope::for_schema(&req.schema).response_schema(&req.schema),
            }]
        }
        _ => vec![],
    }
}

/// Outcome of mining a response for the structured object across all candidate sources.
struct StructuredResolution {
    /// A schema-valid object plus the raw source string it came from.
    valid: Option<(Value, String)>,
    /// First parseable-but-schema-invalid object source + its validation errors,
    /// used to build a targeted repair prompt.
    invalid: Option<(String, Vec<String>)>,
    /// First non-empty raw candidate, shown verbatim in a parse-failure repair prompt.
    raw_seen: Option<String>,
}

/// Append `s` to `out` if it is non-empty and not already present (trimmed, deduped).
fn push_candidate(out: &mut Vec<String>, s: String) {
    let trimmed = s.trim();
    if !trimmed.is_empty() && !out.iter().any(|c| c == trimmed) {
        out.push(trimmed.to_string());
    }
}

/// Ordered raw strings to mine for the structured object, most authoritative first:
/// tool-call arguments, then text content, then the reasoning channel.
///
/// The reasoning fallback is the crux of the cross-model fix: reasoning models
/// (GLM/zhipu, DeepSeek-R1, kimi…) frequently emit the final object inside
/// `reasoning` with `content` empty and no tool call. Earlier extraction only looked
/// at the tool call / text, so those models yielded an empty string and the whole
/// generate_object failed even though a perfectly good object was produced.
fn extract_raw_candidates(message: &super::Message, mode: StructuredMode) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if mode == StructuredMode::Tool {
        if let Some(call) = message.tool_calls().first() {
            push_candidate(
                &mut out,
                serde_json::to_string(&call.args).unwrap_or_default(),
            );
        }
    }
    push_candidate(&mut out, message.text());
    if let Some(reasoning) = message.reasoning_content.as_deref() {
        push_candidate(&mut out, reasoning.to_string());
    }
    out
}

/// Every JSON object/array value mineable from possibly-dirty text, in document order
/// (direct parse, code fences, then all balanced `{...}` / `[...]`). Deduped.
#[cfg(test)]
fn extract_all_json_values(text: &str) -> Vec<Value> {
    extract_json_candidates(text, false)
}

/// Every JSON value mineable from possibly-dirty text for schema-aware structured
/// resolution. When `include_direct_scalars` is true, direct raw/fenced scalar JSON
/// is retained so top-level scalar schemas can recover non-enveloped model output.
fn extract_json_candidates(text: &str, include_direct_scalars: bool) -> Vec<Value> {
    let trimmed = text.trim();
    let mut values: Vec<Value> = Vec::new();
    let consider = |candidate: &str, values: &mut Vec<Value>, allow_scalar: bool| {
        if let Ok(v) = serde_json::from_str::<Value>(candidate.trim()) {
            if (v.is_object() || v.is_array() || allow_scalar) && !values.contains(&v) {
                values.push(v);
            }
        }
    };
    consider(trimmed, &mut values, include_direct_scalars);
    if let Some(inner) = strip_code_fence(trimmed) {
        consider(inner, &mut values, include_direct_scalars);
    }
    for candidate in find_all_balanced(trimmed, '{', '}') {
        consider(&candidate, &mut values, false);
    }
    for candidate in find_all_balanced(trimmed, '[', ']') {
        consider(&candidate, &mut values, false);
    }
    values
}

/// Try every raw candidate × every JSON value it yields against the schema; return the
/// first schema-valid value, else the best parseable-but-invalid value (for repair).
fn resolve_structured(
    candidates: &[String],
    schema: &Value,
    envelope: SchemaEnvelope,
) -> StructuredResolution {
    let mut invalid: Option<(String, Vec<String>)> = None;
    let mut raw_seen: Option<String> = None;
    let response_schema = envelope.response_schema(schema);
    for raw in candidates {
        if raw_seen.is_none() && !raw.trim().is_empty() {
            raw_seen = Some(raw.clone());
        }
        for value in extract_json_candidates(raw, envelope == SchemaEnvelope::Value) {
            match validate_against_schema(&value, schema) {
                Ok(()) => {
                    return StructuredResolution {
                        valid: Some((value, raw.clone())),
                        invalid,
                        raw_seen,
                    };
                }
                Err(errors) => {
                    if invalid.is_none() {
                        invalid = Some((raw.clone(), errors));
                    }
                }
            }

            if envelope != SchemaEnvelope::Direct {
                match validate_against_schema(&value, &response_schema) {
                    Ok(()) => {
                        if let Some(unwrapped) = envelope.unwrap_final(&value) {
                            match validate_against_schema(&unwrapped, schema) {
                                Ok(()) => {
                                    return StructuredResolution {
                                        valid: Some((unwrapped, raw.clone())),
                                        invalid,
                                        raw_seen,
                                    };
                                }
                                Err(errors) => {
                                    if invalid.is_none() {
                                        invalid = Some((raw.clone(), errors));
                                    }
                                }
                            }
                        } else if invalid.is_none() {
                            invalid = Some((
                                raw.clone(),
                                vec!["$: response envelope was missing the expected value field"
                                    .to_string()],
                            ));
                        }
                    }
                    Err(errors) => {
                        if invalid.is_none() {
                            invalid = Some((raw.clone(), errors));
                        }
                    }
                }
            }
        }
    }
    StructuredResolution {
        valid: None,
        invalid,
        raw_seen,
    }
}

/// Extract the first JSON value from possibly dirty model text that validates
/// against `schema`.
///
/// This is the local fast path for callers that already asked an agent to
/// produce structured output. It accepts direct JSON, fenced JSON, and a
/// balanced JSON value embedded in prose, while preserving the same schema
/// and envelope semantics used by [`generate_blocking`]. Callers can fall back
/// to an LLM repair pass only when this returns `None`.
pub(crate) fn parse_validated_output(text: &str, schema: &Value) -> Option<Value> {
    resolve_structured(
        &[text.to_string()],
        schema,
        SchemaEnvelope::for_schema(schema),
    )
    .valid
    .map(|(value, _)| value)
}

/// UTF-8-safe truncation to at most `max` bytes (never splits a multibyte char —
/// repair prompts echo arbitrary model output, including CJK).
fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Repair prompt for when nothing parseable was produced at all.
fn build_parse_failure_repair(raw_text: &str) -> String {
    if raw_text.trim().is_empty() {
        return "Your previous response contained no JSON. Respond with ONLY a single valid JSON object that matches the schema — no prose, no markdown, no analysis, and put the object in your reply content (not in a thinking/reasoning aside).".to_string();
    }
    format!(
        "Your previous output could not be parsed as a JSON object:\n\n{}\n\nReturn ONLY a single valid JSON object matching the schema — no prose, no markdown.",
        truncate_utf8(raw_text, 2000)
    )
}

fn build_repair_message(raw_text: &str, errors: &[String]) -> String {
    // Truncate raw output in repair message to avoid blowing context
    let truncated_raw = if raw_text.len() > 2000 {
        format!(
            "{}...[truncated, {} bytes total]",
            truncate_utf8(raw_text, 2000),
            raw_text.len()
        )
    } else {
        raw_text.to_string()
    };
    format!(
        "Your previous output failed schema validation:\n\n{}\n\nValidation errors:\n{}\n\nPlease return ONLY a corrected JSON object that fixes these errors. No explanation, no markdown.",
        truncated_raw,
        errors.iter().map(|e| format!("- {}", e)).collect::<Vec<_>>().join("\n")
    )
}

fn accumulate_usage(total: &mut TokenUsage, delta: &TokenUsage) {
    total.prompt_tokens += delta.prompt_tokens;
    total.completion_tokens += delta.completion_tokens;
    total.total_tokens += delta.total_tokens;
}

/// Append repair context to the message history, respecting conversation structure.
///
/// In tool mode, the LLM returned a tool_use block. The correct follow-up is:
///   assistant (tool_use) → user (tool_result with error) → assistant (retry)
/// In text modes, it's simply:
///   assistant (text) → user (repair request) → assistant (retry)
fn append_repair_context(
    messages: &mut Vec<Message>,
    assistant_msg: &Message,
    repair_text: &str,
    mode: StructuredMode,
    _raw_text: &str,
) {
    if mode == StructuredMode::Tool {
        // Push the original assistant message (with tool_use block intact)
        messages.push(assistant_msg.clone());
        // Find the tool_use ID to construct a proper tool_result
        let tool_use_id = assistant_msg
            .tool_calls()
            .first()
            .map(|tc| tc.id.clone())
            .unwrap_or_else(|| "unknown".to_string());
        // Return the error as a tool_result so the conversation stays valid
        messages.push(Message::tool_result(&tool_use_id, repair_text, true));
    } else {
        // Text modes: push assistant text then user repair request
        messages.push(assistant_msg.clone());
        messages.push(Message::user(repair_text));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "structured_tests.rs"]
mod structured_tests;
