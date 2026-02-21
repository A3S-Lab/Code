//! Batch tool - Execute multiple tool calls in parallel
//!
//! Allows the LLM to dispatch several independent tool calls in a single
//! turn, reducing round-trips when operations don't depend on each other.

use crate::tools::registry::ToolRegistry;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Executes multiple tool calls concurrently in a single LLM turn.
///
/// Each invocation in the `invocations` array is dispatched in parallel.
/// Results are returned in the same order as the input array.
pub struct BatchTool {
    registry: Arc<ToolRegistry>,
}

impl BatchTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
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
            "properties": {
                "invocations": {
                    "type": "array",
                    "description": "List of tool calls to execute in parallel",
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": {
                                "type": "string",
                                "description": "Name of the tool to call"
                            },
                            "args": {
                                "type": "object",
                                "description": "Arguments to pass to the tool"
                            }
                        },
                        "required": ["tool", "args"]
                    },
                    "minItems": 1
                }
            },
            "required": ["invocations"]
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
        let registry = Arc::clone(&self.registry);
        let ctx = ctx.clone();
        let handles: Vec<_> = invocations
            .into_iter()
            .map(|inv| {
                let registry = Arc::clone(&registry);
                let ctx = ctx.clone();
                tokio::spawn(async move {
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
                        return (tool_name, Err(anyhow::anyhow!("tool name is required")));
                    }

                    let result = registry
                        .execute_with_context(&tool_name, &tool_args, &ctx)
                        .await;
                    (tool_name, result)
                })
            })
            .collect();

        // Collect results in order
        let mut output = String::new();
        let mut all_success = true;

        for (i, handle) in handles.into_iter().enumerate() {
            let (tool_name, result) = handle
                .await
                .map_err(|e| anyhow::anyhow!("task panicked: {}", e))?;

            output.push_str(&format!("--- [{}: {}] ---\n", i + 1, tool_name));
            match result {
                Ok(r) => {
                    if r.exit_code != 0 {
                        all_success = false;
                        output.push_str(&format!("ERROR: {}\n", r.output));
                    } else {
                        output.push_str(&r.output);
                    }
                }
                Err(e) => {
                    all_success = false;
                    output.push_str(&format!("ERROR: {}\n", e));
                }
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
            serde_json::json!({"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]})
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
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::error("intentional failure"))
        }
    }

    fn make_registry() -> Arc<ToolRegistry> {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(FailTool));
        registry
    }

    fn make_ctx() -> ToolContext {
        ToolContext {
            workspace: PathBuf::from("/tmp"),
            session_id: None,
            event_tx: None,
            agent_event_tx: None,
            search_config: None,
            sandbox: None,
        }
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
        assert!(params["properties"]["invocations"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("invocations")));
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
}
