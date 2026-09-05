use super::*;

pub(super) fn compact_task_output(output: &str) -> (String, bool) {
    if output.len() <= TASK_OUTPUT_CONTEXT_LIMIT {
        return (output.to_string(), false);
    }

    let head = crate::text::truncate_utf8(output, TASK_OUTPUT_CONTEXT_HEAD);
    let tail_start = output
        .char_indices()
        .find_map(|(idx, _)| {
            if output.len().saturating_sub(idx) <= TASK_OUTPUT_CONTEXT_TAIL {
                Some(idx)
            } else {
                None
            }
        })
        .unwrap_or(output.len());
    let tail = &output[tail_start..];

    (
        format!(
            "{}\n\n[{} bytes omitted from delegated task output]\n\n{}",
            head,
            output.len().saturating_sub(head.len() + tail.len()),
            tail
        ),
        true,
    )
}

/// Translate selected child-loop events into a `SubagentProgress` milestone
/// for the parent broadcast. Returns `None` for events that aren't worth
/// surfacing as progress (text deltas, tool starts, subagent events from
/// nested delegation, etc.).
///
/// Currently emits progress for:
/// - `ToolEnd`   → `status = "tool_completed"`,
///   `metadata = { tool, exit_code, output_bytes, error_kind? }`
/// - `TurnEnd`   → `status = "turn_completed"`,
///   `metadata = { turn, total_tokens, prompt_tokens, completion_tokens }`
pub(super) fn synthesize_subagent_progress(
    event: &AgentEvent,
    task_id: &str,
    session_id: &str,
) -> Option<AgentEvent> {
    match event {
        AgentEvent::ToolEnd {
            name,
            output,
            exit_code,
            error_kind,
            ..
        } => {
            let mut metadata = serde_json::json!({
                "tool": name,
                "exit_code": exit_code,
                "output_bytes": output.len(),
            });
            if let Some(kind) = error_kind {
                metadata["error_kind"] =
                    serde_json::to_value(kind).unwrap_or(serde_json::Value::Null);
            }
            Some(AgentEvent::SubagentProgress {
                task_id: task_id.to_string(),
                session_id: session_id.to_string(),
                status: "tool_completed".to_string(),
                metadata,
            })
        }
        AgentEvent::TurnEnd { turn, usage } => Some(AgentEvent::SubagentProgress {
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            status: "turn_completed".to_string(),
            metadata: serde_json::json!({
                "turn": turn,
                "total_tokens": usage.total_tokens,
                "prompt_tokens": usage.prompt_tokens,
                "completion_tokens": usage.completion_tokens,
            }),
        }),
        _ => None,
    }
}

pub(super) fn normalize_tool_source_anchor(
    tool: &str,
    value: &str,
    ctx: &ToolContext,
) -> Option<String> {
    match tool {
        "web_fetch" | "web_search" | "download" => {
            crate::tools::builtin::safe_http_source_url(value)
        }
        "read" | "search" => {
            let path = ctx.resolve_workspace_path(value.trim()).ok()?;
            (!path.is_root()).then(|| path.as_str().to_string())
        }
        _ => None,
    }
}

pub(super) fn sanitize_task_json(
    provider: &dyn crate::security::SecurityProvider,
    value: &serde_json::Value,
) -> serde_json::Value {
    match value {
        serde_json::Value::String(value) => {
            serde_json::Value::String(crate::security::sanitize_text(provider, value))
        }
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .map(|value| sanitize_task_json(provider, value))
                .collect(),
        ),
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), sanitize_task_json(provider, value)))
                .collect(),
        ),
        value => value.clone(),
    }
}

/// Sanitize structured task output while retaining only protocol literals the
/// host explicitly authorized with a schema `const` or `enum`. Pattern or type
/// acceptance is intentionally insufficient to bypass output redaction.
pub(super) fn sanitize_task_json_with_schema(
    provider: &dyn crate::security::SecurityProvider,
    value: &serde_json::Value,
    schema: &serde_json::Value,
) -> serde_json::Value {
    match value {
        serde_json::Value::String(value) => {
            let sanitized = crate::security::sanitize_text(provider, value);
            if sanitized != *value && schema_authorizes_literal(schema, value) {
                serde_json::Value::String(value.clone())
            } else {
                serde_json::Value::String(sanitized)
            }
        }
        serde_json::Value::Array(values) => {
            let item_schema = schema.get("items").unwrap_or(&serde_json::Value::Null);
            serde_json::Value::Array(
                values
                    .iter()
                    .map(|value| sanitize_task_json_with_schema(provider, value, item_schema))
                    .collect(),
            )
        }
        serde_json::Value::Object(values) => {
            let properties = schema
                .get("properties")
                .and_then(serde_json::Value::as_object);
            serde_json::Value::Object(
                values
                    .iter()
                    .map(|(key, value)| {
                        let property_schema = properties
                            .and_then(|properties| properties.get(key))
                            .unwrap_or(&serde_json::Value::Null);
                        (
                            key.clone(),
                            sanitize_task_json_with_schema(provider, value, property_schema),
                        )
                    })
                    .collect(),
            )
        }
        value => value.clone(),
    }
}

fn schema_authorizes_literal(schema: &serde_json::Value, value: &str) -> bool {
    schema.get("const").and_then(serde_json::Value::as_str) == Some(value)
        || schema
            .get("enum")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|values| {
                values
                    .iter()
                    .any(|candidate| candidate.as_str() == Some(value))
            })
        || ["allOf", "anyOf", "oneOf"].iter().any(|keyword| {
            schema
                .get(keyword)
                .and_then(serde_json::Value::as_array)
                .is_some_and(|branches| {
                    branches
                        .iter()
                        .any(|branch| schema_authorizes_literal(branch, value))
                })
        })
}

pub(super) fn collect_tool_source_anchors(
    event: &AgentEvent,
    ctx: &ToolContext,
    anchors: &mut Vec<ToolSourceAnchor>,
    seen: &mut std::collections::HashSet<ToolSourceAnchor>,
    scanned_candidates: &mut usize,
) {
    let AgentEvent::ToolEnd {
        name,
        exit_code,
        metadata: Some(metadata),
        error_kind,
        ..
    } = event
    else {
        return;
    };
    if *exit_code != 0
        || error_kind.is_some()
        || name.len() > MAX_TASK_SOURCE_TOOL_BYTES
        || !matches!(
            name.as_str(),
            "web_fetch" | "web_search" | "download" | "read" | "search"
        )
        || anchors.len() >= MAX_TASK_SOURCE_ANCHORS
        || *scanned_candidates >= MAX_TASK_SOURCE_CANDIDATES
    {
        return;
    }
    let Some(values) = metadata
        .get("source_anchors")
        .and_then(serde_json::Value::as_array)
    else {
        return;
    };
    for value in values {
        if *scanned_candidates >= MAX_TASK_SOURCE_CANDIDATES {
            return;
        }
        *scanned_candidates += 1;
        let Some(value) = value.as_str() else {
            continue;
        };
        if value.len() > MAX_TASK_SOURCE_VALUE_BYTES {
            continue;
        }
        let Some(url_or_path) = normalize_tool_source_anchor(name, value, ctx) else {
            continue;
        };
        if url_or_path.len() > MAX_TASK_SOURCE_VALUE_BYTES {
            continue;
        }
        let anchor = ToolSourceAnchor {
            tool: name.clone(),
            url_or_path,
        };
        if seen.insert(anchor.clone()) {
            anchors.push(anchor);
            if anchors.len() >= MAX_TASK_SOURCE_ANCHORS {
                return;
            }
        }
    }
}

pub(super) fn parallel_source_anchor_counts(results: &[TaskResult]) -> Vec<usize> {
    let mut counts = vec![0; results.len()];
    let mut remaining = MAX_PARALLEL_TASK_SOURCE_ANCHORS;
    for success in [true, false] {
        if remaining == 0 {
            break;
        }
        for (index, result) in results.iter().enumerate() {
            if remaining == 0 {
                break;
            }
            if result.success != success {
                continue;
            }
            let count = result
                .source_anchors
                .len()
                .min(MAX_TASK_SOURCE_ANCHORS)
                .min(remaining);
            counts[index] = count;
            remaining -= count;
        }
    }
    counts
}

pub(super) fn task_artifact_id(result: &TaskResult) -> String {
    format!("task-output:{}", result.task_id)
}

pub(super) fn task_artifact_uri(result: &TaskResult) -> String {
    format!(
        "a3s://tasks/{}/runs/{}/output",
        result.session_id, result.task_id
    )
}

pub(super) fn epoch_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn format_task_result_for_context(result: &TaskResult) -> (String, bool) {
    let (output, truncated) = compact_task_output(&result.output);
    let status = if result.success {
        "completed"
    } else {
        "failed"
    };
    let artifact_id = task_artifact_id(result);
    let artifact_uri = task_artifact_uri(result);
    let mut formatted = format!(
        "Task {status}: {}\nAgent: {}\nSession: {}\nTask ID: {}\nArtifact ID: {}\nArtifact URI: {}\n",
        result.task_id, result.agent, result.session_id, result.task_id, artifact_id, artifact_uri
    );
    if truncated {
        formatted.push_str(
            "Output excerpt: truncated for parent context. Use the artifact URI or child run session/events if exact omitted content is needed.\n",
        );
    } else {
        formatted.push_str("Output:\n");
    }
    formatted.push_str(&output);
    (formatted, truncated)
}
