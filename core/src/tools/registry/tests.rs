use super::*;
use crate::trace::{InMemoryTraceSink, TraceEventKind};
use async_trait::async_trait;

struct MockTool {
    name: String,
}

struct HiddenMockTool;

#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "A mock tool for testing"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::success("mock output"))
    }
}

#[async_trait]
impl Tool for HiddenMockTool {
    fn name(&self) -> &str {
        "compatibility_alias"
    }

    fn description(&self) -> &str {
        "A runtime-only compatibility alias"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {}
        })
    }

    fn is_model_visible(&self) -> bool {
        false
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::success("compatibility output"))
    }
}

#[test]
fn governed_argument_validation_enforces_required_types_and_unknown_fields() {
    struct ValidatedTool;

    #[async_trait]
    impl Tool for ValidatedTool {
        fn name(&self) -> &str {
            "validated"
        }

        fn description(&self) -> &str {
            "validated test tool"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "count": {"type": "integer", "minimum": 1}
                },
                "required": ["count"]
            })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::success("ok"))
        }
    }

    let registry = ToolRegistry::new(std::env::temp_dir());
    registry.register(Arc::new(ValidatedTool));
    assert!(registry
        .validate_arguments("validated", &serde_json::json!({"count": 2}))
        .is_ok());
    let missing = registry
        .validate_arguments("validated", &serde_json::json!({}))
        .unwrap_err();
    assert!(missing.contains("count"));
    let unknown = registry
        .validate_arguments(
            "validated",
            &serde_json::json!({"count": 2, "surprise": true}),
        )
        .unwrap_err();
    assert!(unknown.contains("surprise"));
}

#[tokio::test]
async fn large_change_metadata_is_bounded_hashed_and_artifact_backed() {
    struct LargeChangeTool;

    #[async_trait]
    impl Tool for LargeChangeTool {
        fn name(&self) -> &str {
            "large_change"
        }

        fn description(&self) -> &str {
            "returns large before and after metadata"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(
                ToolOutput::success("changed").with_metadata(serde_json::json!({
                    "file_path": "large.txt",
                    "before": format!("before\n{}", "a".repeat(40 * 1024)),
                    "after": format!("after\n{}", "b".repeat(40 * 1024)),
                })),
            )
        }
    }

    let registry = ToolRegistry::new(std::env::temp_dir());
    registry.register(Arc::new(LargeChangeTool));
    let output = registry
        .execute_raw("large_change", &serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    let metadata = output.metadata.unwrap();

    assert_eq!(metadata["change"]["compacted"], true);
    assert!(metadata["before"].as_str().unwrap().len() < 9 * 1024);
    assert!(metadata["after"].as_str().unwrap().len() < 9 * 1024);
    assert_eq!(
        metadata["change"]["before"]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    let before_uri = metadata["change"]["before"]["artifact"]["artifact_uri"]
        .as_str()
        .unwrap();
    let artifact = registry.get_artifact(before_uri).unwrap();
    assert!(artifact.content.starts_with("before\n"));
    assert!(metadata["change"]["unified_diff"]
        .as_str()
        .is_some_and(|diff| diff.contains("--- before")));
}

#[test]
fn test_registry_register_and_get() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));

    let tool = Arc::new(MockTool {
        name: "test".to_string(),
    });
    registry.register(tool);

    assert!(registry.contains("test"));
    assert!(!registry.contains("nonexistent"));

    let retrieved = registry.get("test");
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().name(), "test");
}

#[test]
fn test_registry_unregister() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));

    let tool = Arc::new(MockTool {
        name: "test".to_string(),
    });
    registry.register(tool);

    assert!(registry.contains("test"));
    assert!(registry.unregister("test"));
    assert!(!registry.contains("test"));
    assert!(!registry.unregister("test")); // Already removed
}

#[test]
fn test_registry_unregister_preserves_builtins() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    registry.register_builtin(Arc::new(MockTool {
        name: "read".to_string(),
    }));

    assert!(!registry.unregister("read"));
    assert!(registry.contains("read"));
}

#[test]
fn test_registry_unregister_by_prefix_preserves_builtins() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    registry.register_builtin(Arc::new(MockTool {
        name: "mcp__builtin".to_string(),
    }));
    registry.register(Arc::new(MockTool {
        name: "mcp__dynamic".to_string(),
    }));

    registry.unregister_by_prefix("mcp__");

    assert!(registry.contains("mcp__builtin"));
    assert!(!registry.contains("mcp__dynamic"));
}

#[test]
fn concurrent_owned_registration_cannot_overwrite_builtin() {
    for iteration in 0..32 {
        let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
        let name = format!("atomic_builtin_{iteration}");
        let builtin: Arc<dyn Tool> = Arc::new(MockTool { name: name.clone() });
        let dynamic: Arc<dyn Tool> = Arc::new(MockTool { name: name.clone() });
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let builtin_registry = Arc::clone(&registry);
        let builtin_tool = Arc::clone(&builtin);
        let builtin_barrier = Arc::clone(&barrier);
        let builtin_thread = std::thread::spawn(move || {
            builtin_barrier.wait();
            builtin_registry.register_builtin(builtin_tool);
        });

        let dynamic_registry = Arc::clone(&registry);
        let dynamic_barrier = Arc::clone(&barrier);
        let dynamic_thread = std::thread::spawn(move || {
            dynamic_barrier.wait();
            dynamic_registry.register_with_shadow(dynamic);
        });

        barrier.wait();
        builtin_thread.join().unwrap();
        dynamic_thread.join().unwrap();

        let current = registry.get(&name).unwrap();
        assert!(Arc::ptr_eq(&current, &builtin));
        assert!(!registry.unregister(&name));
    }
}

#[test]
fn test_registry_definitions() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));

    registry.register(Arc::new(MockTool {
        name: "tool2".to_string(),
    }));
    registry.register(Arc::new(MockTool {
        name: "tool1".to_string(),
    }));

    let definitions = registry.definitions();
    assert_eq!(definitions.len(), 2);
    let names: Vec<&str> = definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect();
    assert_eq!(names, vec!["tool1", "tool2"]);
}

#[tokio::test]
async fn hidden_tools_remain_registered_and_executable_without_model_schema_cost() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    registry.register_builtin(Arc::new(HiddenMockTool));

    assert!(registry.contains("compatibility_alias"));
    assert!(registry.list().contains(&"compatibility_alias".to_string()));
    assert!(!registry
        .definitions()
        .iter()
        .any(|definition| definition.name == "compatibility_alias"));

    let result = registry
        .execute("compatibility_alias", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output, "compatibility output");
}

#[tokio::test]
async fn test_registry_execute() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));

    registry.register(Arc::new(MockTool {
        name: "test".to_string(),
    }));

    let result = registry
        .execute("test", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output, "mock output");
}

#[tokio::test]
async fn test_registry_execute_unknown() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));

    let result = registry
        .execute("unknown", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(result.exit_code, 1);
    assert!(result.output.contains("Unknown tool"));
}

#[tokio::test]
async fn test_registry_execute_with_context_success() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    let ctx = ToolContext::new(PathBuf::from("/tmp"));
    let trace_sink = InMemoryTraceSink::default();
    registry.set_trace_sink(Arc::new(trace_sink.clone()));

    registry.register(Arc::new(MockTool {
        name: "my_tool".to_string(),
    }));

    let result = registry
        .execute_with_context("my_tool", &serde_json::json!({}), &ctx)
        .await
        .unwrap();
    assert_eq!(result.name, "my_tool");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output, "mock output");

    let events = trace_sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, TraceEventKind::ToolExecution);
    assert_eq!(events[0].name, "my_tool");
    assert!(events[0].success);
    assert_eq!(events[0].output_bytes, "mock output".len());
}

#[tokio::test]
async fn test_registry_execute_with_context_unknown_tool() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    let result = registry
        .execute_with_context("nonexistent", &serde_json::json!({}), &ctx)
        .await
        .unwrap();
    assert_eq!(result.exit_code, 1);
    assert!(result.output.contains("Unknown tool: nonexistent"));
}

struct FailingTool;

#[async_trait]
impl Tool for FailingTool {
    fn name(&self) -> &str {
        "failing"
    }

    fn description(&self) -> &str {
        "A tool that returns failure"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::error("something went wrong"))
    }
}

#[tokio::test]
async fn test_registry_execute_failing_tool() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    registry.register(Arc::new(FailingTool));

    let result = registry
        .execute("failing", &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.output, "something went wrong");
}

struct LargeOutputTool;

#[async_trait]
impl Tool for LargeOutputTool {
    fn name(&self) -> &str {
        "large_output"
    }

    fn description(&self) -> &str {
        "A tool that returns more than the maximum output size"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::success(
            "x".repeat(super::super::MAX_OUTPUT_SIZE + 1),
        ))
    }
}

#[tokio::test]
async fn test_registry_truncates_large_tool_output() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    let trace_sink = InMemoryTraceSink::default();
    registry.set_trace_sink(Arc::new(trace_sink.clone()));
    registry.register(Arc::new(LargeOutputTool));

    let result = registry
        .execute("large_output", &serde_json::json!({}))
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.output.contains("[tool output truncated:"));
    assert!(result
        .output
        .contains("Full output artifact: a3s://tool-output/large_output/"));
    assert!(result.output.len() < super::super::MAX_OUTPUT_SIZE + 512);
    let metadata = result.metadata.expect("artifact metadata");
    assert_eq!(
        metadata["artifact"]["original_bytes"],
        serde_json::json!(super::super::MAX_OUTPUT_SIZE + 1)
    );
    assert_eq!(
        metadata["artifact"]["shown_bytes"],
        serde_json::json!(super::super::MAX_OUTPUT_SIZE)
    );
    assert!(metadata["artifact"]["artifact_id"]
        .as_str()
        .unwrap()
        .starts_with("tool-output:large_output:"));
    assert!(metadata["artifact"]["artifact_uri"]
        .as_str()
        .unwrap()
        .starts_with("a3s://tool-output/large_output/"));

    let artifact_uri = metadata["artifact"]["artifact_uri"].as_str().unwrap();
    let artifact = registry
        .get_artifact(artifact_uri)
        .expect("full output artifact");
    assert_eq!(artifact.tool_name, "large_output");
    assert_eq!(artifact.original_bytes, super::super::MAX_OUTPUT_SIZE + 1);
    assert_eq!(artifact.shown_bytes, super::super::MAX_OUTPUT_SIZE);
    assert_eq!(
        artifact.content,
        "x".repeat(super::super::MAX_OUTPUT_SIZE + 1)
    );

    let events = trace_sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].artifact_uris, vec![artifact_uri]);
}

#[tokio::test]
async fn test_registry_execute_raw_success() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    registry.register(Arc::new(MockTool {
        name: "raw_test".to_string(),
    }));

    let output = registry
        .execute_raw("raw_test", &serde_json::json!({}))
        .await
        .unwrap();
    assert!(output.is_some());
    let output = output.unwrap();
    assert!(output.success);
    assert_eq!(output.content, "mock output");
}

#[tokio::test]
async fn test_registry_execute_raw_stores_truncated_artifact() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    registry.register(Arc::new(LargeOutputTool));

    let output = registry
        .execute_raw("large_output", &serde_json::json!({}))
        .await
        .unwrap()
        .expect("raw output");

    assert!(output.content.contains("[tool output truncated:"));
    let metadata = output.metadata.expect("artifact metadata");
    let artifact_uri = metadata["artifact"]["artifact_uri"].as_str().unwrap();
    let artifact = registry
        .get_artifact(artifact_uri)
        .expect("full output artifact");
    assert_eq!(artifact.tool_name, "large_output");
    assert_eq!(artifact.content.len(), super::super::MAX_OUTPUT_SIZE + 1);
}

#[tokio::test]
async fn test_registry_execute_raw_unknown() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));

    let output = registry
        .execute_raw("missing", &serde_json::json!({}))
        .await
        .unwrap();
    assert!(output.is_none());
}

#[test]
fn test_registry_list() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    registry.register(Arc::new(MockTool {
        name: "beta".to_string(),
    }));
    registry.register(Arc::new(MockTool {
        name: "alpha".to_string(),
    }));

    let names = registry.list();
    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn test_registry_len_and_is_empty() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);

    registry.register(Arc::new(MockTool {
        name: "t".to_string(),
    }));
    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 1);
}

#[test]
fn test_registry_replace_tool() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    registry.register(Arc::new(MockTool {
        name: "dup".to_string(),
    }));
    registry.register(Arc::new(MockTool {
        name: "dup".to_string(),
    }));
    // Should still have only 1 tool (replaced)
    assert_eq!(registry.len(), 1);
}
