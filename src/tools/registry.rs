//! Tool Registry
//!
//! Central registry for all tools (built-in and dynamic).
//! Provides thread-safe registration, lookup, and execution.

use super::types::{Tool, ToolContext, ToolOutput};
use super::ToolResult;
use crate::llm::ToolDefinition;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Tool registry for managing all available tools
pub struct ToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    /// Names of builtin tools that cannot be overridden
    builtins: RwLock<std::collections::HashSet<String>>,
    context: ToolContext,
}

impl ToolRegistry {
    /// Create a new tool registry
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            builtins: RwLock::new(std::collections::HashSet::new()),
            context: ToolContext::new(workspace),
        }
    }

    /// Register a builtin tool (cannot be overridden by dynamic tools)
    pub fn register_builtin(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        let mut tools = self.tools.write().unwrap();
        let mut builtins = self.builtins.write().unwrap();
        tracing::debug!("Registering builtin tool: {}", name);
        tools.insert(name.clone(), tool);
        builtins.insert(name);
    }

    /// Register a tool
    ///
    /// If a tool with the same name already exists as a builtin, the registration
    /// is rejected to prevent shadowing of core tools.
    pub fn register(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        let builtins = self.builtins.read().unwrap();
        if builtins.contains(&name) {
            tracing::warn!("Rejected registration of tool '{}': cannot shadow builtin", name);
            return;
        }
        drop(builtins);
        let mut tools = self.tools.write().unwrap();
        tracing::debug!("Registering tool: {}", name);
        tools.insert(name, tool);
    }

    /// Unregister a tool by name
    ///
    /// Returns true if the tool was found and removed.
    pub fn unregister(&self, name: &str) -> bool {
        let mut tools = self.tools.write().unwrap();
        tracing::debug!("Unregistering tool: {}", name);
        tools.remove(name).is_some()
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let tools = self.tools.read().unwrap();
        tools.get(name).cloned()
    }

    /// Check if a tool exists
    pub fn contains(&self, name: &str) -> bool {
        let tools = self.tools.read().unwrap();
        tools.contains_key(name)
    }

    /// Get all tool definitions for LLM
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let tools = self.tools.read().unwrap();
        tools
            .values()
            .map(|tool| ToolDefinition {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters(),
            })
            .collect()
    }

    /// List all registered tool names
    pub fn list(&self) -> Vec<String> {
        let tools = self.tools.read().unwrap();
        tools.keys().cloned().collect()
    }

    /// Get the number of registered tools
    pub fn len(&self) -> usize {
        let tools = self.tools.read().unwrap();
        tools.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the tool context
    pub fn context(&self) -> &ToolContext {
        &self.context
    }

    /// Execute a tool by name using the registry's default context
    pub async fn execute(&self, name: &str, args: &serde_json::Value) -> Result<ToolResult> {
        self.execute_with_context(name, args, &self.context).await
    }

    /// Execute a tool by name with an external context
    pub async fn execute_with_context(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult> {
        let span = tracing::info_span!(
            "a3s.tool.execute",
            "a3s.tool.name" = %name,
            "a3s.tool.success" = tracing::field::Empty,
            "a3s.tool.exit_code" = tracing::field::Empty,
            "a3s.tool.duration_ms" = tracing::field::Empty,
        );
        let _guard = span.enter();
        let start = std::time::Instant::now();

        let tool = self.get(name);

        let result = match tool {
            Some(tool) => {
                let output = tool.execute(args, ctx).await?;
                Ok(ToolResult {
                    name: name.to_string(),
                    output: output.content,
                    exit_code: if output.success { 0 } else { 1 },
                    metadata: output.metadata,
                })
            }
            None => Ok(ToolResult::error(name, format!("Unknown tool: {}", name))),
        };

        if let Ok(ref r) = result {
            crate::telemetry::record_tool_result(r.exit_code, start.elapsed());
        }

        result
    }

    /// Execute a tool and return raw output using the registry's default context
    pub async fn execute_raw(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<Option<ToolOutput>> {
        self.execute_raw_with_context(name, args, &self.context)
            .await
    }

    /// Execute a tool and return raw output with an external context
    pub async fn execute_raw_with_context(
        &self,
        name: &str,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<Option<ToolOutput>> {
        let tool = self.get(name);

        match tool {
            Some(tool) => {
                let output = tool.execute(args, ctx).await?;
                Ok(Some(output))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct MockTool {
        name: String,
    }

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
                "properties": {},
                "required": []
            })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::success("mock output"))
        }
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
    fn test_registry_definitions() {
        let registry = ToolRegistry::new(PathBuf::from("/tmp"));

        registry.register(Arc::new(MockTool {
            name: "tool1".to_string(),
        }));
        registry.register(Arc::new(MockTool {
            name: "tool2".to_string(),
        }));

        let definitions = registry.definitions();
        assert_eq!(definitions.len(), 2);
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
            serde_json::json!({"type": "object", "properties": {}, "required": []})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
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
            name: "alpha".to_string(),
        }));
        registry.register(Arc::new(MockTool {
            name: "beta".to_string(),
        }));

        let names = registry.list();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
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
}
