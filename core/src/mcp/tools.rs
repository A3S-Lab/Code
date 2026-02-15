//! MCP Tools Integration
//!
//! Integrates MCP tools with the A3S Code tool system.

use crate::mcp::manager::{tool_result_to_string, McpManager};
use crate::mcp::protocol::McpTool;
use crate::tools::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// MCP tool wrapper that implements the Tool trait
pub struct McpToolWrapper {
    /// Full tool name (mcp__server__tool)
    full_name: String,
    /// Original MCP tool definition
    mcp_tool: McpTool,
    /// Server name
    server_name: String,
    /// MCP manager reference
    manager: Arc<McpManager>,
}

impl McpToolWrapper {
    /// Create a new MCP tool wrapper
    pub fn new(server_name: String, mcp_tool: McpTool, manager: Arc<McpManager>) -> Self {
        let full_name = format!("mcp__{}_{}", server_name, mcp_tool.name);
        Self {
            full_name,
            mcp_tool,
            server_name,
            manager,
        }
    }

    /// Get the server name
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Get the original MCP tool name
    pub fn mcp_tool_name(&self) -> &str {
        &self.mcp_tool.name
    }
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> &str {
        self.mcp_tool.description.as_deref().unwrap_or("MCP tool")
    }

    fn parameters(&self) -> serde_json::Value {
        self.mcp_tool.input_schema.clone()
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        // Call the MCP tool through the manager
        let result = self
            .manager
            .call_tool(&self.full_name, Some(args.clone()))
            .await;

        match result {
            Ok(tool_result) => {
                let output = tool_result_to_string(&tool_result);
                if tool_result.is_error {
                    Ok(ToolOutput::error(output))
                } else {
                    Ok(ToolOutput::success(output))
                }
            }
            Err(e) => Ok(ToolOutput::error(format!("MCP tool error: {}", e))),
        }
    }
}

/// Create tool wrappers for all tools from an MCP server
pub fn create_mcp_tools(
    server_name: &str,
    tools: Vec<McpTool>,
    manager: Arc<McpManager>,
) -> Vec<Arc<dyn Tool>> {
    tools
        .into_iter()
        .map(|tool| {
            Arc::new(McpToolWrapper::new(
                server_name.to_string(),
                tool,
                manager.clone(),
            )) as Arc<dyn Tool>
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_tool_wrapper_name() {
        let manager = Arc::new(McpManager::new());
        let mcp_tool = McpTool {
            name: "create_issue".to_string(),
            description: Some("Create a GitHub issue".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"}
                }
            }),
        };

        let wrapper = McpToolWrapper::new("github".to_string(), mcp_tool, manager);

        assert_eq!(wrapper.name(), "mcp__github_create_issue");
        assert_eq!(wrapper.server_name(), "github");
        assert_eq!(wrapper.mcp_tool_name(), "create_issue");
        assert_eq!(wrapper.description(), "Create a GitHub issue");
    }

    #[test]
    fn test_create_mcp_tools() {
        let manager = Arc::new(McpManager::new());
        let tools = vec![
            McpTool {
                name: "tool1".to_string(),
                description: Some("Tool 1".to_string()),
                input_schema: serde_json::json!({}),
            },
            McpTool {
                name: "tool2".to_string(),
                description: Some("Tool 2".to_string()),
                input_schema: serde_json::json!({}),
            },
        ];

        let wrappers = create_mcp_tools("test", tools, manager);

        assert_eq!(wrappers.len(), 2);
        assert_eq!(wrappers[0].name(), "mcp__test_tool1");
        assert_eq!(wrappers[1].name(), "mcp__test_tool2");
    }
}
