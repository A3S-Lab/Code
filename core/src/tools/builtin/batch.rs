//! Batch tool - Execute multiple tool calls in parallel
//!
//! Allows the LLM to dispatch several independent tool calls in a single
//! turn, reducing round-trips when operations don't depend on each other.

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::{registry_tool_invoker, ToolInvocation, ToolInvoker, ToolRegistry, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use std::sync::Arc;

const MAX_BATCH_INVOCATIONS: usize = 32;
const DEFAULT_BATCH_CONCURRENCY: usize = 8;
const MAX_BATCH_CONCURRENCY: usize = 16;

/// Executes multiple tool calls concurrently in a single LLM turn.
///
/// Each invocation in the `invocations` array is dispatched in parallel.
/// Results are returned in the same order as the input array.
pub struct BatchTool {
    fallback_invoker: Arc<dyn ToolInvoker>,
}

impl BatchTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            fallback_invoker: registry_tool_invoker(registry),
        }
    }
}

#[async_trait]
impl Tool for BatchTool {
    fn name(&self) -> &str {
        "batch"
    }

    fn description(&self) -> &str {
        "Execute multiple independent tool calls in parallel. Use this when you need to run \
         several tools that don't depend on each other's results — it's faster than calling \
         them one at a time. Each invocation specifies a tool name and its arguments."
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
                        { "tool": "read", "args": { "file_path": "README.md" } },
                        { "tool": "glob", "args": { "pattern": "**/*.rs" } }
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

        // Prevent recursive batch calls
        for inv in &invocations {
            if inv.get("tool").and_then(|v| v.as_str()) == Some("batch") {
                return Ok(ToolOutput::error("nested batch calls are not allowed"));
            }
        }

        let invoker = ctx
            .tool_invoker()
            .unwrap_or_else(|| Arc::clone(&self.fallback_invoker));
        let prepared = invocations
            .into_iter()
            .enumerate()
            .map(|(index, invocation)| {
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
                (index, correlation_id, tool_name, tool_args)
            })
            .collect::<Vec<_>>();
        let parallel_cap = prepared
            .iter()
            .filter_map(|(_, _, name, args)| invoker.capabilities(name, args))
            .filter(|capabilities| capabilities.allows_parallel_batch())
            .map(|capabilities| capabilities.max_parallelism)
            .min();
        let all_parallel_safe = prepared.iter().all(|(_, _, name, args)| {
            invoker
                .capabilities(name, args)
                .is_some_and(|capabilities| capabilities.allows_parallel_batch())
        });
        let concurrency = if all_parallel_safe {
            requested_concurrency.min(parallel_cap.unwrap_or(1)).max(1)
        } else {
            1
        };

        let ctx = ctx.clone();
        let calls = prepared
            .into_iter()
            .map(|(index, correlation_id, tool_name, tool_args)| {
                let invoker = Arc::clone(&invoker);
                let ctx = ctx.clone();
                async move {
                    if tool_name.is_empty() {
                        return (
                            index,
                            correlation_id,
                            tool_name,
                            ToolResult::error("", "tool name is required".to_string()),
                        );
                    }

                    let result = invoker
                        .invoke(ToolInvocation::nested(tool_name.clone(), tool_args), &ctx)
                        .await;
                    (index, correlation_id, tool_name, result)
                }
            });
        let results = stream::iter(calls)
            .buffered(concurrency)
            .collect::<Vec<_>>()
            .await;

        let mut output = String::new();
        let mut success_count = 0usize;
        let mut result_metadata = Vec::with_capacity(results.len());
        let mut failed_indices = Vec::new();

        for (index, correlation_id, tool_name, result) in results {
            let label = correlation_id
                .as_deref()
                .map(|id| format!("{tool_name} · {id}"))
                .unwrap_or_else(|| tool_name.clone());
            output.push_str(&format!("--- [{}: {}] ---\n", index + 1, label));
            if result.exit_code != 0 {
                failed_indices.push(index);
                output.push_str(&format!("ERROR: {}\n", result.output));
            } else {
                success_count += 1;
                output.push_str(&result.output);
            }
            output.push('\n');
            result_metadata.push(serde_json::json!({
                "index": index,
                "id": correlation_id,
                "tool": tool_name,
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
            "execution_mode": if concurrency > 1 { "parallel" } else { "serial" },
            "requested_concurrency": requested_concurrency,
            "applied_concurrency": concurrency,
            "total_count": total_count,
            "success_count": success_count,
            "failure_count": failure_count,
            "failed_indices": failed_indices,
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

fn compact_child_metadata(metadata: Option<serde_json::Value>) -> Option<serde_json::Value> {
    const MAX_CHILD_METADATA_BYTES: usize = 4 * 1024;
    let metadata = metadata?;
    let encoded = serde_json::to_vec(&metadata).ok()?;
    if encoded.len() <= MAX_CHILD_METADATA_BYTES {
        return Some(metadata);
    }

    Some(serde_json::json!({
        "truncated": true,
        "original_bytes": encoded.len(),
        "artifact": metadata.get("artifact"),
    }))
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

    fn make_registry() -> Arc<ToolRegistry> {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(FailTool));
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
        assert!(tool.description().contains("parallel"));
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
        assert_eq!(examples[0]["invocations"][0]["tool"], "read");
        assert!(examples[0]["invocations"][0].get("name").is_none());
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
        assert!(result.content.contains("[1: echo]"));
        assert!(result.content.contains("[2: echo]"));
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
