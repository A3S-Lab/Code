//! Batch tool - Execute multiple tool calls in parallel
//!
//! Allows the LLM to dispatch several independent tool calls in a single
//! turn, reducing round-trips when operations don't depend on each other.

#[cfg(test)]
use crate::tools::registry_tool_invoker;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::{registry_bound_tool_invoker, ToolInvoker, ToolRegistry, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const MAX_BATCH_INVOCATIONS: usize = 32;
const MAX_BATCH_STEPS: usize = 32;
const DEFAULT_BATCH_CONCURRENCY: usize = 8;
const MAX_BATCH_CONCURRENCY: usize = 16;
const MAX_BATCH_BINDING_BYTES: usize = 16 * 1024;
const BATCH_REF_KEY: &str = "$ref";

#[derive(Debug, Clone)]
struct PreparedInvocation {
    index: usize,
    id: Option<String>,
    tool: String,
    args: Value,
    step: usize,
}

/// Executes multiple tool calls concurrently in a single LLM turn.
///
/// Each invocation in the `invocations` array is dispatched in parallel.
/// Results are returned in the same order as the input array.
pub struct BatchTool {
    fallback_invoker: Arc<dyn ToolInvoker>,
}

impl BatchTool {
    #[cfg(test)]
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            fallback_invoker: registry_tool_invoker(registry),
        }
    }

    pub(crate) fn new_registry_bound(registry: Arc<ToolRegistry>) -> Self {
        Self {
            fallback_invoker: registry_bound_tool_invoker(registry),
        }
    }
}

#[async_trait]
impl Tool for BatchTool {
    fn name(&self) -> &str {
        "batch"
    }

    fn description(&self) -> &str {
        "Execute a bounded set of tool calls in one turn. Put independent calls in the same \
         step to run them concurrently; use a later step when it depends on an earlier result. \
         Reference an earlier result with an argument value like {\"$ref\":\"id.output\"}. \
         Each invocation specifies a tool name and its arguments."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "invocations": {
                    "type": "array",
                    "description": "List of tool calls to execute in parallel",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Optional caller-defined result correlation ID."
                            },
                            "tool": {
                                "type": "string",
                                "description": "Required. Name of the tool to call."
                            },
                            "args": {
                                "type": "object",
                                "description": "Required. Arguments to pass to the tool as a JSON object."
                            },
                            "step": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": MAX_BATCH_STEPS,
                                "default": 1,
                                "description": "Optional dependency group. Same-step calls do not see each other's results; a later step may use a previous result reference."
                            }
                        },
                        "required": ["tool", "args"]
                    },
                    "minItems": 1,
                    "maxItems": MAX_BATCH_INVOCATIONS
                },
                "max_concurrency": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_BATCH_CONCURRENCY,
                    "description": "Maximum concurrent calls. Default 8; maximum 16. Mutating or non-idempotent tools are automatically serialized."
                }
            },
            "required": ["invocations"],
            "examples": [
                {
                    "invocations": [
                        { "step": 1, "id": "files", "tool": "search", "args": { "mode": "glob", "query": "**/Cargo.toml" } },
                        { "step": 2, "tool": "read", "args": { "file_path": { "$ref": "files.output_lines.0" } } }
                    ]
                }
            ]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let invocations = match args.get("invocations").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr.clone(),
            Some(_) => return Ok(ToolOutput::error("invocations array must not be empty")),
            None => return Ok(ToolOutput::error("invocations parameter is required")),
        };
        if invocations.len() > MAX_BATCH_INVOCATIONS {
            return Ok(ToolOutput::error(format!(
                "batch accepts at most {MAX_BATCH_INVOCATIONS} invocations"
            )));
        }
        let requested_concurrency = match args.get("max_concurrency") {
            Some(value) => match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
                Some(value) if value > 0 => value,
                _ => {
                    return Ok(ToolOutput::error(
                        "max_concurrency must be a positive integer",
                    ))
                }
            },
            None => DEFAULT_BATCH_CONCURRENCY,
        };
        let requested_concurrency = requested_concurrency.min(MAX_BATCH_CONCURRENCY);

        let invoker = ctx
            .tool_invoker()
            .unwrap_or_else(|| Arc::clone(&self.fallback_invoker));
        let prepared = invocations
            .into_iter()
            .enumerate()
            .map(
                |(index, invocation)| -> std::result::Result<PreparedInvocation, String> {
                    let tool_name = invocation
                        .get("tool")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let tool_args = invocation
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
                    let correlation_id = invocation
                        .get("id")
                        .and_then(|value| value.as_str())
                        .map(ToString::to_string);
                    let step = match invocation.get("step") {
                        None => 1,
                        Some(value) => value
                            .as_u64()
                            .and_then(|value| usize::try_from(value).ok())
                            .ok_or_else(|| {
                                format!(
                                    "batch invocation {} step must be a positive integer",
                                    index + 1
                                )
                            })?,
                    };
                    Ok(PreparedInvocation {
                        index,
                        id: correlation_id,
                        tool: tool_name,
                        args: tool_args,
                        step,
                    })
                },
            )
            .collect::<std::result::Result<Vec<_>, _>>();
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        if let Err(error) = validate_prepared_invocations(&prepared) {
            return Ok(ToolOutput::error(error.to_string()));
        }

        let mut by_step = BTreeMap::<usize, Vec<PreparedInvocation>>::new();
        for invocation in prepared {
            by_step.entry(invocation.step).or_default().push(invocation);
        }
        let stage_count = by_step.len();

        let mut bindings = BTreeMap::<String, Value>::new();
        let mut results = Vec::new();
        let mut execution_modes = BTreeSet::new();
        let mut binding_errors = Vec::new();
        let mut highest_step = 0usize;
        for (step, invocations) in by_step {
            highest_step = highest_step.max(step);
            let mut resolved = Vec::with_capacity(invocations.len());
            for mut invocation in invocations {
                if let Err(error) = resolve_batch_references(&mut invocation.args, &bindings) {
                    resolved.push((invocation, Some(error.to_string())));
                    continue;
                }
                resolved.push((invocation, None));
            }

            let executable = resolved
                .iter()
                .filter(|(_, error)| error.is_none())
                .map(|(invocation, _)| invocation.clone())
                .collect::<Vec<_>>();
            let step_results = execute_step(
                executable,
                invoker.clone(),
                ctx.clone(),
                requested_concurrency,
            )
            .await;
            let mut step_by_index = step_results
                .into_iter()
                .map(|(index, result, mode)| {
                    execution_modes.insert(mode);
                    (index, result)
                })
                .collect::<BTreeMap<_, _>>();

            for (invocation, reference_error) in resolved {
                let result = if let Some(error) = reference_error {
                    ToolResult::error("batch", error)
                } else {
                    step_by_index.remove(&invocation.index).unwrap_or_else(|| {
                        ToolResult::error(
                            &invocation.tool,
                            "batch invocation produced no result".to_string(),
                        )
                    })
                };
                if result.exit_code == 0 {
                    if let Some(id) = invocation.id.as_deref() {
                        match binding_projection(&result) {
                            Ok(binding) => {
                                bindings.insert(id.to_string(), binding);
                            }
                            Err(error) => binding_errors.push(serde_json::json!({
                                "id": id,
                                "index": invocation.index,
                                "error": error.to_string(),
                            })),
                        }
                    }
                }
                results.push((invocation, result));
            }
        }

        results.sort_by_key(|(invocation, _)| invocation.index);
        let mut output = String::new();
        let mut success_count = 0usize;
        let mut result_metadata = Vec::with_capacity(results.len());
        let mut failed_indices = Vec::new();

        for (invocation, result) in results {
            let label = invocation
                .id
                .as_deref()
                .map(|id| format!("{} · {id}", invocation.tool))
                .unwrap_or_else(|| invocation.tool.clone());
            output.push_str(&format!(
                "--- [{} / step {}: {}] ---\n",
                invocation.index + 1,
                invocation.step,
                label
            ));
            if result.exit_code != 0 {
                failed_indices.push(invocation.index);
                output.push_str(&format!("ERROR: {}\n", result.output));
            } else {
                success_count += 1;
                output.push_str(&result.output);
            }
            output.push('\n');
            result_metadata.push(serde_json::json!({
                "index": invocation.index,
                "step": invocation.step,
                "id": invocation.id,
                "tool": invocation.tool,
                "success": result.exit_code == 0,
                "exit_code": result.exit_code,
                "output_bytes": result.output.len(),
                "error_kind": result.error_kind,
                "metadata": compact_child_metadata(result.metadata),
            }));
        }

        let total_count = result_metadata.len();
        let failure_count = total_count.saturating_sub(success_count);
        let partial_failure = success_count > 0 && failure_count > 0;
        if partial_failure {
            output.push_str(&format!(
                "\nBatch completed with {success_count} successful and {failure_count} failed item(s). Retry only failed indices {:?}; do not repeat successful items.\n",
                failed_indices
            ));
        }
        let metadata = serde_json::json!({
            "status": if failure_count == 0 {
                "complete"
            } else if partial_failure {
                "partial_failure"
            } else {
                "failed"
            },
            "requested_concurrency": requested_concurrency,
            "applied_concurrency": execution_modes
                .iter()
                .filter_map(|mode| mode.strip_prefix("parallel:").and_then(|value| value.parse::<usize>().ok()))
                .max()
                .unwrap_or(1),
            "execution_mode": if stage_count > 1 {
                "staged"
            } else if execution_modes.iter().any(|mode| mode.starts_with("parallel:")) {
                "parallel"
            } else {
                "serial"
            },
            "steps": stage_count,
            "highest_step": highest_step,
            "total_count": total_count,
            "success_count": success_count,
            "failure_count": failure_count,
            "failed_indices": failed_indices,
            "binding_errors": binding_errors,
            "results": result_metadata,
        });

        if failure_count == 0 || partial_failure {
            Ok(ToolOutput::success(output).with_metadata(metadata))
        } else {
            Ok(ToolOutput::error(output)
                .with_error_kind(crate::tools::ToolErrorKind::PartialFailure {
                    failed: failure_count,
                    total: total_count,
                })
                .with_metadata(metadata))
        }
    }
}

fn validate_prepared_invocations(invocations: &[PreparedInvocation]) -> Result<()> {
    if invocations.iter().any(|invocation| invocation.step == 0) {
        anyhow::bail!("batch invocation step must be at least 1");
    }
    if invocations
        .iter()
        .any(|invocation| invocation.step > MAX_BATCH_STEPS)
    {
        anyhow::bail!("batch accepts at most {MAX_BATCH_STEPS} steps");
    }
    let mut ids = BTreeSet::new();
    for invocation in invocations {
        if invocation.tool == "batch" {
            anyhow::bail!("nested batch calls are not allowed");
        }
        if let Some(id) = invocation.id.as_deref() {
            if id.trim().is_empty() {
                anyhow::bail!("batch invocation id must not be empty");
            }
            if !ids.insert(id) {
                anyhow::bail!("batch invocation id '{id}' is duplicated");
            }
        }
    }
    Ok(())
}

async fn execute_step(
    invocations: Vec<PreparedInvocation>,
    invoker: Arc<dyn ToolInvoker>,
    ctx: ToolContext,
    requested_concurrency: usize,
) -> Vec<(usize, ToolResult, String)> {
    if invocations.is_empty() {
        return Vec::new();
    }
    let parallel_cap = invocations
        .iter()
        .filter_map(|invocation| invoker.capabilities(&invocation.tool, &invocation.args))
        .filter(|capabilities| capabilities.allows_parallel_batch())
        .map(|capabilities| capabilities.max_parallelism)
        .min();
    let all_parallel_safe = invocations.iter().all(|invocation| {
        invoker
            .capabilities(&invocation.tool, &invocation.args)
            .is_some_and(|capabilities| capabilities.allows_parallel_batch())
    });
    let concurrency = if all_parallel_safe {
        requested_concurrency.min(parallel_cap.unwrap_or(1)).max(1)
    } else {
        1
    };
    let mode = if concurrency > 1 {
        format!("parallel:{concurrency}")
    } else {
        "serial".to_string()
    };
    let calls = invocations.into_iter().map(|invocation| {
        let invoker = Arc::clone(&invoker);
        let ctx = ctx.clone();
        let mode = mode.clone();
        async move {
            if invocation.tool.is_empty() {
                return (
                    invocation.index,
                    ToolResult::error("", "tool name is required".to_string()),
                    mode,
                );
            }
            let mut result = invoker
                .invoke(
                    ctx.nested_tool_invocation(invocation.tool.clone(), invocation.args),
                    &ctx,
                )
                .await;
            if result.name.is_empty() {
                result.name = invocation.tool.clone();
            }
            (invocation.index, result, mode)
        }
    });
    stream::iter(calls)
        .buffered(concurrency)
        .collect::<Vec<_>>()
        .await
}

fn resolve_batch_references(value: &mut Value, bindings: &BTreeMap<String, Value>) -> Result<()> {
    match value {
        Value::Object(object) if object.len() == 1 && object.contains_key(BATCH_REF_KEY) => {
            let reference = object
                .get(BATCH_REF_KEY)
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("batch $ref must be a string"))?;
            *value = resolve_batch_reference(reference, bindings)?;
        }
        Value::Object(object) => {
            if object.contains_key(BATCH_REF_KEY) {
                anyhow::bail!("batch $ref objects may not contain additional fields");
            }
            for child in object.values_mut() {
                resolve_batch_references(child, bindings)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                resolve_batch_references(child, bindings)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn resolve_batch_reference(reference: &str, bindings: &BTreeMap<String, Value>) -> Result<Value> {
    let mut segments = reference.split('.');
    let name = segments
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow::anyhow!("batch $ref must name a previous invocation"))?;
    let mut current = bindings
        .get(name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("batch $ref '{reference}' is unavailable"))?;
    for segment in segments {
        if segment.is_empty() {
            anyhow::bail!("batch $ref '{reference}' contains an empty path segment");
        }
        current = match &current {
            Value::Object(object) => object.get(segment).cloned(),
            Value::Array(items) => segment
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index).cloned()),
            _ => None,
        }
        .ok_or_else(|| anyhow::anyhow!("batch $ref '{reference}' has no field '{segment}'"))?;
    }
    let encoded = serde_json::to_vec(&current)?;
    if encoded.len() > MAX_BATCH_BINDING_BYTES {
        anyhow::bail!(
            "batch $ref '{reference}' exceeds the {} byte binding limit",
            MAX_BATCH_BINDING_BYTES
        );
    }
    Ok(current)
}

fn binding_projection(result: &ToolResult) -> Result<Value> {
    let output = serde_json::from_str::<Value>(&result.output)
        .unwrap_or_else(|_| Value::String(result.output.clone()));
    let output_lines = Value::Array(
        result
            .output
            .lines()
            .map(|line| Value::String(line.to_string()))
            .collect(),
    );
    let metadata = result.metadata.clone().unwrap_or(Value::Null);
    let projection = Value::Object(Map::from_iter([
        ("output".to_string(), output),
        ("output_lines".to_string(), output_lines),
        ("metadata".to_string(), metadata),
        ("exit_code".to_string(), Value::from(result.exit_code)),
        ("tool".to_string(), Value::String(result.name.clone())),
    ]));
    let encoded = serde_json::to_vec(&projection)?;
    if encoded.len() > MAX_BATCH_BINDING_BYTES {
        anyhow::bail!(
            "batch result exceeds the {} byte binding limit",
            MAX_BATCH_BINDING_BYTES
        );
    }
    Ok(projection)
}

fn compact_child_metadata(metadata: Option<serde_json::Value>) -> Option<serde_json::Value> {
    const MAX_CHILD_METADATA_BYTES: usize = 4 * 1024;
    let metadata = metadata?;
    let encoded = serde_json::to_vec(&metadata).ok()?;
    if encoded.len() <= MAX_CHILD_METADATA_BYTES {
        return Some(metadata);
    }

    let mut compacted = serde_json::Map::from_iter([
        ("truncated".to_string(), serde_json::Value::Bool(true)),
        (
            "original_bytes".to_string(),
            serde_json::Value::from(encoded.len()),
        ),
    ]);
    for key in [
        "status",
        "engine_selection_source",
        "selected_engines",
        "engine_fallback",
        "notices",
        "search_fallback",
        "artifact",
    ] {
        if let Some(value) = metadata.get(key) {
            let value = match key {
                "selected_engines" => compact_string_array(value, 8, 96),
                "notices" => compact_string_array(value, 4, 512),
                "search_fallback" => compact_search_fallback(value),
                _ => value.clone(),
            };
            compacted.insert(key.to_string(), value);
            if serde_json::to_vec(&compacted)
                .is_ok_and(|encoded| encoded.len() > MAX_CHILD_METADATA_BYTES)
            {
                compacted.remove(key);
            }
        }
    }
    Some(serde_json::Value::Object(compacted))
}

fn compact_string_array(
    value: &serde_json::Value,
    maximum_items: usize,
    maximum_chars: usize,
) -> serde_json::Value {
    serde_json::Value::Array(
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .take(maximum_items)
            .map(|item| item.chars().take(maximum_chars).collect::<String>().into())
            .collect(),
    )
}

fn compact_search_fallback(value: &serde_json::Value) -> serde_json::Value {
    let Some(fallback) = value.as_object() else {
        return serde_json::Value::Null;
    };
    let mut compacted = serde_json::Map::new();
    for key in ["trigger", "mode", "attempted", "successful"] {
        if let Some(value) = fallback.get(key) {
            compacted.insert(key.to_string(), value.clone());
        }
    }
    if let Some(engines) = fallback.get("engines") {
        compacted.insert("engines".to_string(), compact_string_array(engines, 8, 96));
    }
    if let Some(failures) = fallback
        .get("failures")
        .and_then(serde_json::Value::as_array)
    {
        let failures = failures
            .iter()
            .take(8)
            .filter_map(serde_json::Value::as_object)
            .map(|failure| {
                let mut item = serde_json::Map::new();
                for key in ["engine", "provider", "kind", "transient"] {
                    if let Some(value) = failure.get(key) {
                        let value = value
                            .as_str()
                            .map(|text| text.chars().take(96).collect::<String>().into())
                            .unwrap_or_else(|| value.clone());
                        item.insert(key.to_string(), value);
                    }
                }
                serde_json::Value::Object(item)
            })
            .collect();
        compacted.insert("failures".to_string(), serde_json::Value::Array(failures));
    }
    serde_json::Value::Object(compacted)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::types::ToolOutput;
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echoes input"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "msg": {
                        "type": "string"
                    }
                },
                "required": ["msg"]
            })
        }
        fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
            crate::tools::ToolCapabilities::parallel_safe_read(8)
        }
        async fn execute(
            &self,
            args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            let msg = args.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ToolOutput::success(msg.to_string()))
        }
    }

    struct FailTool;

    #[async_trait]
    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "always fails"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {},
                "required": []
            })
        }
        fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
            crate::tools::ToolCapabilities::parallel_safe_read(8)
        }
        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::error("intentional failure"))
        }
    }

    struct JsonEchoTool;

    #[async_trait]
    impl Tool for JsonEchoTool {
        fn name(&self) -> &str {
            "json_echo"
        }

        fn description(&self) -> &str {
            "returns the supplied JSON value"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "value": {} },
                "required": ["value"]
            })
        }

        fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
            crate::tools::ToolCapabilities::parallel_safe_read(8)
        }

        async fn execute(
            &self,
            args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::success(serde_json::to_string(
                args.get("value").unwrap_or(&Value::Null),
            )?))
        }
    }

    struct DelayedSideEffectTool {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for DelayedSideEffectTool {
        fn name(&self) -> &str {
            "delayed_side_effect"
        }

        fn description(&self) -> &str {
            "records a delayed side effect"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {},
                "required": []
            })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::success("done"))
        }
    }

    struct ParallelProbeTool {
        active: Arc<AtomicUsize>,
        maximum_active: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for ParallelProbeTool {
        fn name(&self) -> &str {
            "parallel_probe"
        }

        fn description(&self) -> &str {
            "observes concurrent execution"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {},
                "required": []
            })
        }

        fn capabilities(&self, _args: &serde_json::Value) -> crate::tools::ToolCapabilities {
            crate::tools::ToolCapabilities::parallel_safe_read(8)
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.maximum_active.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ToolOutput::success("probe"))
        }
    }

    fn make_registry() -> Arc<ToolRegistry> {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(FailTool));
        registry.register(Arc::new(JsonEchoTool));
        registry
    }

    fn make_ctx() -> ToolContext {
        ToolContext::new(PathBuf::from("/tmp"))
    }

    #[test]
    fn test_tool_name() {
        let tool = BatchTool::new(make_registry());
        assert_eq!(tool.name(), "batch");
    }

    #[test]
    fn test_tool_description() {
        let tool = BatchTool::new(make_registry());
        assert!(tool.description().contains("concurrently"));
    }

    #[test]
    fn test_tool_parameters() {
        let tool = BatchTool::new(make_registry());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert_eq!(params["additionalProperties"], false);
        assert!(params["properties"]["invocations"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("invocations")));
        assert_eq!(
            params["properties"]["invocations"]["items"]["additionalProperties"],
            false
        );
        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["invocations"][0]["tool"], "search");
        assert_eq!(examples[0]["invocations"][0]["step"], 1);
        assert_eq!(
            examples[0]["invocations"][1]["args"]["file_path"]["$ref"],
            "files.output_lines.0"
        );
        assert!(examples[0]["invocations"][0].get("name").is_none());
    }

    #[test]
    fn compact_child_metadata_retains_search_routing_fields() {
        let metadata = serde_json::json!({
            "status": "failed",
            "engine_selection_source": "config",
            "selected_engines": ["private-search", "x".repeat(5 * 1024)],
            "engine_fallback": null,
            "notices": ["AnySearch quota is exhausted", "x".repeat(5 * 1024)],
            "search_fallback": {
                "trigger": "engine_failure",
                "mode": "additional_engines",
                "attempted": true,
                "engines": ["brave", "bing"],
                "successful": true,
                "failures": [{
                    "engine": "AnySearch",
                    "provider": "anysearch",
                    "kind": "provider_quota",
                    "transient": false
                }]
            },
            "search_metrics": {
                "oversized": "x".repeat(5 * 1024)
            }
        });

        let compacted = compact_child_metadata(Some(metadata)).expect("compacted metadata");

        assert_eq!(compacted["truncated"], true);
        assert_eq!(compacted["status"], "failed");
        assert_eq!(compacted["engine_selection_source"], "config");
        assert_eq!(compacted["selected_engines"][0], "private-search");
        assert_eq!(compacted["selected_engines"][1].as_str().unwrap().len(), 96);
        assert!(compacted.get("engine_fallback").is_some());
        assert_eq!(compacted["notices"][0], "AnySearch quota is exhausted");
        assert_eq!(compacted["notices"][1].as_str().unwrap().len(), 512);
        assert_eq!(compacted["search_fallback"]["trigger"], "engine_failure");
        assert_eq!(
            compacted["search_fallback"]["failures"][0]["kind"],
            "provider_quota"
        );
        assert!(compacted.get("search_metrics").is_none());
        assert!(serde_json::to_vec(&compacted).unwrap().len() <= 4 * 1024);
    }

    #[test]
    fn batch_references_support_bounded_object_and_array_paths() {
        let bindings = BTreeMap::from([(
            "search".to_string(),
            serde_json::json!({"output": {"paths": ["src/lib.rs", "src/main.rs"]}}),
        )]);

        assert_eq!(
            resolve_batch_reference("search.output.paths.1", &bindings).unwrap(),
            serde_json::json!("src/main.rs")
        );
        assert!(resolve_batch_reference("search.output.paths.2", &bindings).is_err());
    }

    #[tokio::test]
    async fn test_execute_missing_invocations() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(&serde_json::json!({}), &make_ctx())
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("invocations"));
    }

    #[tokio::test]
    async fn test_execute_empty_invocations() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(&serde_json::json!({"invocations": []}), &make_ctx())
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_execute_single() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(
                &serde_json::json!({
                    "invocations": [{"tool": "echo", "args": {"msg": "hello"}}]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.content.contains("hello"));
    }

    #[tokio::test]
    async fn test_execute_multiple_parallel() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(
                &serde_json::json!({
                    "invocations": [
                        {"tool": "echo", "args": {"msg": "first"}},
                        {"tool": "echo", "args": {"msg": "second"}},
                        {"tool": "echo", "args": {"msg": "third"}}
                    ]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.content.contains("first"));
        assert!(result.content.contains("second"));
        assert!(result.content.contains("third"));
        // Results in order
        assert!(result.content.find("first") < result.content.find("second"));
        assert!(result.content.find("second") < result.content.find("third"));
    }

    #[tokio::test]
    async fn test_execute_parallelism_is_observable_for_safe_tools() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum_active = Arc::new(AtomicUsize::new(0));
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(ParallelProbeTool {
            active: Arc::clone(&active),
            maximum_active: Arc::clone(&maximum_active),
        }));
        let tool = BatchTool::new(registry);
        let result = tool
            .execute(
                &serde_json::json!({
                    "max_concurrency": 3,
                    "invocations": [
                        {"tool": "parallel_probe", "args": {}},
                        {"tool": "parallel_probe", "args": {}},
                        {"tool": "parallel_probe", "args": {}}
                    ]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(maximum_active.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn test_execute_with_failure() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(
                &serde_json::json!({
                    "invocations": [
                        {"tool": "echo", "args": {"msg": "ok"}},
                        {"tool": "fail", "args": {}}
                    ]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();
        // The orchestration completed; item-level failure is structured so a
        // caller retries only that item instead of repeating the success.
        assert!(result.success);
        assert!(result.content.contains("ok"));
        assert!(result.content.contains("intentional failure"));
        assert_eq!(result.metadata.unwrap()["status"], "partial_failure");
    }

    #[tokio::test]
    async fn test_execute_staged_reference_and_metadata() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(
                &serde_json::json!({
                    "invocations": [
                        {
                            "step": 1,
                            "id": "producer",
                            "tool": "json_echo",
                            "args": {"value": {"path": "src/lib.rs", "line": 42}}
                        },
                        {
                            "step": 1,
                            "tool": "echo",
                            "args": {"msg": "independent"}
                        },
                        {
                            "step": 2,
                            "id": "consumer",
                            "tool": "json_echo",
                            "args": {"value": {"$ref": "producer.output.path"}}
                        }
                    ]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("src/lib.rs"));
        let metadata = result.metadata.expect("batch metadata");
        assert_eq!(metadata["execution_mode"], "staged");
        assert_eq!(metadata["steps"], 2);
        assert_eq!(metadata["failure_count"], 0);
        assert_eq!(metadata["results"][0]["step"], 1);
        assert_eq!(metadata["results"][2]["step"], 2);
        assert!(metadata["binding_errors"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_execute_same_step_reference_is_rejected_without_race() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(
                &serde_json::json!({
                    "invocations": [
                        {
                            "step": 1,
                            "id": "producer",
                            "tool": "echo",
                            "args": {"msg": "ready"}
                        },
                        {
                            "step": 1,
                            "tool": "echo",
                            "args": {"msg": {"$ref": "producer.output"}}
                        }
                    ]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();

        assert!(
            result.success,
            "the batch itself completed with an item error"
        );
        assert!(result.content.contains("unavailable"));
        assert_eq!(result.metadata.unwrap()["status"], "partial_failure");
    }

    #[tokio::test]
    async fn test_execute_rejects_duplicate_ids_before_side_effects() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(
                &serde_json::json!({
                    "invocations": [
                        {"id": "same", "tool": "echo", "args": {"msg": "a"}},
                        {"id": "same", "tool": "echo", "args": {"msg": "b"}}
                    ]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("duplicated"));
        assert!(!result.content.contains("--- ["));
    }

    #[tokio::test]
    async fn test_execute_rejects_malformed_step_before_side_effects() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(
                &serde_json::json!({
                    "invocations": [{"step": "first", "tool": "echo", "args": {"msg": "x"}}]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("step must be a positive integer"));
        assert!(!result.content.contains("--- ["));
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(
                &serde_json::json!({
                    "invocations": [{"tool": "nonexistent", "args": {}}]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_execute_nested_batch_rejected() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(
                &serde_json::json!({
                    "invocations": [{"tool": "batch", "args": {"invocations": []}}]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("nested batch"));
    }

    #[tokio::test]
    async fn dropping_batch_execution_drops_nested_tool_futures() {
        let calls = Arc::new(AtomicUsize::new(0));
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(DelayedSideEffectTool {
            calls: Arc::clone(&calls),
        }));
        let tool = BatchTool::new(registry);
        let ctx = make_ctx();
        let args = serde_json::json!({
            "invocations": [{"tool": "delayed_side_effect", "args": {}}]
        });

        let timed_out = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            tool.execute(&args, &ctx),
        )
        .await;
        assert!(timed_out.is_err(), "the parent batch should time out first");

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "nested tools must not outlive a dropped batch future"
        );
    }

    #[tokio::test]
    async fn test_execute_result_headers() {
        let tool = BatchTool::new(make_registry());
        let result = tool
            .execute(
                &serde_json::json!({
                    "invocations": [
                        {"tool": "echo", "args": {"msg": "a"}},
                        {"tool": "echo", "args": {"msg": "b"}}
                    ]
                }),
                &make_ctx(),
            )
            .await
            .unwrap();
        assert!(result.content.contains("[1 / step 1: echo]"));
        assert!(result.content.contains("[2 / step 1: echo]"));
    }

    #[tokio::test]
    async fn test_execute_large_batch_all_success() {
        let tool = BatchTool::new(make_registry());
        let invocations: Vec<serde_json::Value> = (0..MAX_BATCH_INVOCATIONS)
            .map(|i| serde_json::json!({"tool": "echo", "args": {"msg": format!("item-{}", i)}}))
            .collect();
        let result = tool
            .execute(
                &serde_json::json!({"invocations": invocations}),
                &make_ctx(),
            )
            .await
            .unwrap();
        assert!(result.success);
        // All items should appear in the output
        assert!(result.content.contains("item-0"));
        assert!(result.content.contains("item-31"));
        // Items should appear in order
        let pos_0 = result.content.find("item-0").unwrap();
        let pos_31 = result.content.find("item-31").unwrap();
        assert!(pos_0 < pos_31);
        assert_eq!(result.metadata.unwrap()["execution_mode"], "parallel");
    }

    #[tokio::test]
    async fn test_execute_large_batch_mixed_results() {
        let tool = BatchTool::new(make_registry());
        let invocations: Vec<serde_json::Value> = (0..MAX_BATCH_INVOCATIONS)
            .map(|i| {
                if i % 2 == 0 {
                    serde_json::json!({"tool": "echo", "args": {"msg": format!("ok-{}", i)}})
                } else {
                    serde_json::json!({"tool": "fail", "args": {}})
                }
            })
            .collect();
        let result = tool
            .execute(
                &serde_json::json!({"invocations": invocations}),
                &make_ctx(),
            )
            .await
            .unwrap();
        assert!(result.success);
        // Successful items should still appear in output
        assert!(result.content.contains("ok-0"));
        assert_eq!(result.metadata.unwrap()["status"], "partial_failure");
    }

    #[tokio::test]
    async fn test_execute_rejects_unbounded_batch() {
        let tool = BatchTool::new(make_registry());
        let invocations = (0..=MAX_BATCH_INVOCATIONS)
            .map(|index| serde_json::json!({"tool": "echo", "args": {"msg": index.to_string()}}))
            .collect::<Vec<_>>();

        let result = tool
            .execute(
                &serde_json::json!({"invocations": invocations}),
                &make_ctx(),
            )
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.content.contains("at most 32"));
    }
}
