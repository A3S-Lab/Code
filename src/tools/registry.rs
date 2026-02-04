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
    context: ToolContext,
}

impl ToolRegistry {
    /// Create a new tool registry
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            context: ToolContext::new(workspace),
        }
    }

    /// Register a tool
    ///
    /// If a tool with the same name already exists, it will be replaced.
    pub fn register(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
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

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, args: &serde_json::Value) -> Result<ToolResult> {
        let tool = self.get(name);

        match tool {
            Some(tool) => {
                let output = tool.execute(args, &self.context).await?;
                Ok(ToolResult {
                    name: name.to_string(),
                    output: output.content,
                    exit_code: if output.success { 0 } else { 1 },
                })
            }
            None => Ok(ToolResult::error(name, format!("Unknown tool: {}", name))),
        }
    }

    /// Execute a tool and return raw output
    pub async fn execute_raw(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<Option<ToolOutput>> {
        let tool = self.get(name);

        match tool {
            Some(tool) => {
                let output = tool.execute(args, &self.context).await?;
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
}
