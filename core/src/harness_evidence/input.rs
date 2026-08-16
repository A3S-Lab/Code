use super::digest::{measure, DigestMeasurement};
use super::{
    HarnessEvidenceError, ModelInputKindV1, ModelInputSnapshotV1, ToolResultContextUsageV1,
    MODEL_INPUT_PAYLOAD_DOMAIN, MODEL_INPUT_SNAPSHOT_V1_SCHEMA, MODEL_MESSAGES_DOMAIN,
    MODEL_STRUCTURED_DOMAIN, MODEL_SYSTEM_DOMAIN, REPEATED_TOOL_RESULT_CONTENTS_DOMAIN,
    RETRIEVAL_RESULTS_DOMAIN, TOOL_RESULT_CONTENTS_DOMAIN, TOOL_RESULT_CONTENT_DOMAIN,
};
use crate::llm::structured::{ResponseFormat, StructuredDirective};
use crate::llm::{
    estimate_tool_result_contents_tokens, ContentBlock, Message, ToolDefinition,
    ToolResultContentField,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;

pub(super) struct ModelInputCapture<'a> {
    pub(super) call_sequence: u64,
    pub(super) kind: ModelInputKindV1,
    pub(super) messages: &'a [Message],
    pub(super) system: Option<&'a str>,
    pub(super) tools: &'a [ToolDefinition],
    pub(super) directive: Option<&'a StructuredDirective>,
    pub(super) estimated_prompt_tokens: usize,
    pub(super) tools_measurement: DigestMeasurement,
    pub(super) capability_snapshot_digest: &'a str,
}

pub(super) fn capture_model_input(
    capture: ModelInputCapture<'_>,
) -> Result<(ModelInputSnapshotV1, ToolResultContextUsageV1), HarnessEvidenceError> {
    let messages_measurement = measure(MODEL_MESSAGES_DOMAIN, capture.messages)?;
    let system_measurement = capture
        .system
        .map(|system| measure(MODEL_SYSTEM_DOMAIN, system))
        .transpose()?;
    let structured = capture.directive.map(StructuredEvidence::from);
    let structured_measurement = structured
        .as_ref()
        .map(|directive| measure(MODEL_STRUCTURED_DOMAIN, directive))
        .transpose()?;
    let retrieval_results = identified_retrieval_results(capture.messages);
    let retrieval_measurement = (!retrieval_results.is_empty())
        .then(|| measure(RETRIEVAL_RESULTS_DOMAIN, &retrieval_results))
        .transpose()?;
    let counts = count_content_blocks(capture.messages);
    let tool_results = analyze_tool_results(capture.messages)?;
    let tool_result_context = ToolResultContextUsageV1 {
        total_count: counts.tool_results,
        unique_count: tool_results.unique_count,
        repeated_count: tool_results.repeated.len(),
        content_bytes: tool_results.content_bytes,
        repeated_content_bytes: tool_results.repeated_content_bytes,
        estimated_tokens: estimate_tool_result_contents_tokens(
            tool_results.contents.iter().copied(),
        ),
        repeated_estimated_tokens: estimate_tool_result_contents_tokens(
            tool_results.repeated.iter().copied(),
        ),
        contents_digest: tool_results.measurement.map(|value| value.digest),
        repeated_contents_digest: tool_results.repeated_measurement.map(|value| value.digest),
    };

    #[derive(Serialize)]
    struct Payload<'a> {
        kind: ModelInputKindV1,
        messages: &'a [Message],
        system: Option<&'a str>,
        tools: &'a [ToolDefinition],
        structured: Option<&'a StructuredEvidence<'a>>,
    }
    let payload_measurement = measure(
        MODEL_INPUT_PAYLOAD_DOMAIN,
        &Payload {
            kind: capture.kind,
            messages: capture.messages,
            system: capture.system,
            tools: capture.tools,
            structured: structured.as_ref(),
        },
    )?;

    let mut snapshot = ModelInputSnapshotV1 {
        schema: MODEL_INPUT_SNAPSHOT_V1_SCHEMA.to_string(),
        call_sequence: capture.call_sequence,
        kind: capture.kind,
        message_count: capture.messages.len(),
        content_block_count: counts.content_blocks,
        image_block_count: counts.image_blocks,
        tool_result_count: counts.tool_results,
        tool_count: capture.tools.len(),
        retrieval_result_count: retrieval_results.len(),
        retrieval_result_bytes: retrieval_measurement
            .as_ref()
            .map_or(0, |value| value.bytes),
        retrieval_results_digest: retrieval_measurement.map(|value| value.digest),
        system_bytes: system_measurement.as_ref().map_or(0, |value| value.bytes),
        message_payload_bytes: messages_measurement.bytes,
        tool_definition_bytes: capture.tools_measurement.bytes,
        structured_output_bytes: structured_measurement
            .as_ref()
            .map_or(0, |value| value.bytes),
        payload_bytes: payload_measurement.bytes,
        estimated_prompt_tokens: capture.estimated_prompt_tokens,
        messages_digest: messages_measurement.digest,
        system_digest: system_measurement.map(|value| value.digest),
        tool_definitions_digest: capture.tools_measurement.digest,
        structured_output_digest: structured_measurement.map(|value| value.digest),
        input_digest: payload_measurement.digest,
        capability_snapshot_digest: capture.capability_snapshot_digest.to_string(),
        snapshot_digest: String::new(),
    };
    snapshot.snapshot_digest = snapshot.expected_digest()?;
    tool_result_context.validate()?;
    Ok((snapshot, tool_result_context))
}

struct ToolResultAnalysis<'a> {
    contents: Vec<&'a ToolResultContentField>,
    repeated: Vec<&'a ToolResultContentField>,
    unique_count: usize,
    content_bytes: u64,
    repeated_content_bytes: u64,
    measurement: Option<DigestMeasurement>,
    repeated_measurement: Option<DigestMeasurement>,
}

fn analyze_tool_results(
    messages: &[Message],
) -> Result<ToolResultAnalysis<'_>, HarnessEvidenceError> {
    let contents = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .collect::<Vec<_>>();
    let measurement = (!contents.is_empty())
        .then(|| measure(TOOL_RESULT_CONTENTS_DOMAIN, &contents))
        .transpose()?;
    let mut seen = HashSet::with_capacity(contents.len());
    let mut repeated = Vec::new();
    let mut content_bytes = 0u64;
    let mut repeated_content_bytes = 0u64;
    for content in &contents {
        let item = measure(TOOL_RESULT_CONTENT_DOMAIN, *content)?;
        content_bytes = content_bytes.saturating_add(item.bytes);
        if !seen.insert(item.digest) {
            repeated_content_bytes = repeated_content_bytes.saturating_add(item.bytes);
            repeated.push(*content);
        }
    }
    let repeated_measurement = (!repeated.is_empty())
        .then(|| measure(REPEATED_TOOL_RESULT_CONTENTS_DOMAIN, &repeated))
        .transpose()?;

    Ok(ToolResultAnalysis {
        contents,
        repeated,
        unique_count: seen.len(),
        content_bytes,
        repeated_content_bytes,
        measurement,
        repeated_measurement,
    })
}

#[derive(Serialize)]
struct StructuredEvidence<'a> {
    force_tool: Option<&'a str>,
    response_format: Option<ResponseFormatEvidence<'a>>,
}

impl<'a> From<&'a StructuredDirective> for StructuredEvidence<'a> {
    fn from(value: &'a StructuredDirective) -> Self {
        let response_format = value.response_format.as_ref().map(|format| match format {
            ResponseFormat::JsonObject => ResponseFormatEvidence::JsonObject,
            ResponseFormat::JsonSchema { name, schema } => {
                ResponseFormatEvidence::JsonSchema { name, schema }
            }
        });
        Self {
            force_tool: value.force_tool.as_deref(),
            response_format,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseFormatEvidence<'a> {
    JsonObject,
    JsonSchema { name: &'a str, schema: &'a Value },
}

#[derive(Default)]
struct ContentCounts {
    content_blocks: usize,
    image_blocks: usize,
    tool_results: usize,
}

fn count_content_blocks(messages: &[Message]) -> ContentCounts {
    let mut counts = ContentCounts::default();
    for block in messages.iter().flat_map(|message| &message.content) {
        counts.content_blocks = counts.content_blocks.saturating_add(1);
        match block {
            ContentBlock::Image { .. } => {
                counts.image_blocks = counts.image_blocks.saturating_add(1);
            }
            ContentBlock::ToolResult { content, .. } => {
                counts.tool_results = counts.tool_results.saturating_add(1);
                if let crate::llm::ToolResultContentField::Blocks(blocks) = content {
                    counts.image_blocks = counts.image_blocks.saturating_add(
                        blocks
                            .iter()
                            .filter(|block| {
                                matches!(block, crate::llm::ToolResultContent::Image { .. })
                            })
                            .count(),
                    );
                }
            }
            _ => {}
        }
    }
    counts
}

fn identified_retrieval_results(messages: &[Message]) -> Vec<&ContentBlock> {
    let mut pending_retrieval_calls = HashSet::new();
    let mut results = Vec::new();
    for block in messages.iter().flat_map(|message| &message.content) {
        match block {
            ContentBlock::ToolUse { id, name, input } => {
                if is_retrieval_call(name, input) {
                    pending_retrieval_calls.insert(id.as_str());
                } else {
                    pending_retrieval_calls.remove(id.as_str());
                }
            }
            ContentBlock::ToolResult { tool_use_id, .. }
                if pending_retrieval_calls.remove(tool_use_id.as_str()) =>
            {
                results.push(block);
            }
            _ => {}
        }
    }
    results
}

fn is_retrieval_call(name: &str, input: &Value) -> bool {
    matches!(name, "semantic" | "hybrid")
        || (name == "search"
            && matches!(
                input.get("mode").and_then(Value::as_str),
                Some("semantic" | "hybrid")
            ))
}
