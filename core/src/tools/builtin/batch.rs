//! Batch tool - Execute multiple tool calls in parallel
//!
//! Allows the LLM to dispatch several independent tool calls in a single
//! turn, reducing round-trips when operations don't depend on each other.

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::{registry_tool_invoker, ToolInvocation, ToolInvoker, ToolRegistry, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use futures::future::join_all;
use std::sync::Arc;

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
                    "minItems": 1
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

        // Prevent recursive batch calls
        for inv in &invocations {
            if inv.get("tool").and_then(|v| v.as_str()) == Some("batch") {
                return Ok(ToolOutput::error("nested batch calls are not allowed"));
            }
        }

        // Dispatch all calls in parallel
        let invoker = ctx
            .tool_invoker()
            .unwrap_or_else(|| Arc::clone(&self.fallback_invoker));
        let ctx = ctx.clone();
        let calls = invocations.into_iter().map(|inv| {
            let invoker = Arc::clone(&invoker);
            let ctx = ctx.clone();
            async move {
                let tool_name = inv
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_args = inv
                    .get("args")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(Default::default()));

                if tool_name.is_empty() {
                    return (
                        tool_name,
                        ToolResult::error("", "tool name is required".to_string()),
                    );
                }

                let result = invoker
                    .invoke(ToolInvocation::nested(tool_name.clone(), tool_args), &ctx)
                    .await;
                (tool_name, result)
            }
        });

        // Collect results in order
        let mut output = String::new();
        let mut all_success = true;

        for (i, (tool_name, result)) in join_all(calls).await.into_iter().enumerate() {
            output.push_str(&format!("--- [{}: {}] ---\n", i + 1, tool_name));
            if result.exit_code != 0 {
                all_success = false;
                output.push_str(&format!("ERROR: {}\n", result.output));
            } else {
                output.push_str(&result.output);
            }
            output.push('\n');
        }

        if all_success {
            Ok(ToolOutput::success(output))
        } else {
            Ok(ToolOutput::error(output))
        }
    }
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
        // One failure makes the whole batch fail
        assert!(!result.success);
        assert!(result.content.contains("ok"));
        assert!(result.content.contains("intentional failure"));
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
        let invocations: Vec<serde_json::Value> = (0..100)
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
        assert!(result.content.contains("item-99"));
        // Items should appear in order
        let pos_0 = result.content.find("item-0").unwrap();
        let pos_99 = result.content.find("item-99").unwrap();
        assert!(pos_0 < pos_99);
    }

    #[tokio::test]
    async fn test_execute_large_batch_mixed_results() {
        let tool = BatchTool::new(make_registry());
        // 50 successes + 50 failures
        let invocations: Vec<serde_json::Value> = (0..100)
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
        // Any failure makes the batch report failure
        assert!(!result.success);
        // Successful items should still appear in output
        assert!(result.content.contains("ok-0"));
    }
}
