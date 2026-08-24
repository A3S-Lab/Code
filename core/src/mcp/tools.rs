//! MCP Tools Integration
//!
//! Integrates MCP tools with the A3S Code tool system.

use crate::mcp::manager::McpManager;
use crate::mcp::protocol::McpTool;
use crate::mcp::result::project_tool_result;
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
        let full_name = format!("mcp__{}__{}", server_name, mcp_tool.name);
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

pub(super) fn annotation_requires_confirmation(tool: &McpTool) -> bool {
    let Some(annotations) = tool.annotations.as_ref() else {
        // Missing behavior metadata is unknown, not read-only.
        return true;
    };

    if annotations.destructive_hint == Some(true)
        || annotations.read_only_hint != Some(true)
        || annotations.open_world_hint != Some(false)
    {
        return true;
    }

    annotations
        .additional
        .iter()
        .any(|(key, value)| custom_risk_requires_confirmation(key, value))
        || tool.meta.as_ref().is_some_and(|meta| {
            meta.as_object().is_some_and(|fields| {
                fields
                    .iter()
                    .any(|(key, value)| custom_risk_requires_confirmation(key, value))
            })
        })
}

fn custom_risk_requires_confirmation(key: &str, value: &serde_json::Value) -> bool {
    let key = key.to_ascii_lowercase();
    if !matches!(
        key.as_str(),
        "x-a3s-risk" | "a3s/risk" | "a3s.risk" | "risk"
    ) {
        return false;
    }

    match value {
        serde_json::Value::String(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "read" | "read_only" | "read-only" | "routine" | "closed_world_read"
        ),
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| custom_risk_requires_confirmation(key.as_str(), value)),
        // A declared but malformed risk value cannot reduce confirmation.
        _ => true,
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

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        annotation_requires_confirmation(&self.mcp_tool)
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        // Call the MCP tool through the manager
        let result = self
            .manager
            .call_tool(&self.full_name, Some(args.clone()))
            .await;

        match result {
            Ok(tool_result) => project_tool_result(&self.full_name, &tool_result, ctx).await,
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
    use crate::mcp::protocol::McpToolAnnotations;
    use crate::tools::Tool;
    use std::collections::HashMap;

    #[test]
    fn test_mcp_tool_wrapper_name() {
        let manager = Arc::new(McpManager::new());
        let mcp_tool = McpTool {
            name: "create_issue".to_string(),
            title: None,
            description: Some("Create a GitHub issue".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"}
                }
            }),
            output_schema: None,
            annotations: None,
            icons: Vec::new(),
            meta: None,
        };

        let wrapper = McpToolWrapper::new("github".to_string(), mcp_tool, manager);

        assert_eq!(wrapper.name(), "mcp__github__create_issue");
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
                title: None,
                description: Some("Tool 1".to_string()),
                input_schema: serde_json::json!({}),
                output_schema: None,
                annotations: None,
                icons: Vec::new(),
                meta: None,
            },
            McpTool {
                name: "tool2".to_string(),
                title: None,
                description: Some("Tool 2".to_string()),
                input_schema: serde_json::json!({}),
                output_schema: None,
                annotations: None,
                icons: Vec::new(),
                meta: None,
            },
        ];

        let wrappers = create_mcp_tools("test", tools, manager);

        assert_eq!(wrappers.len(), 2);
        assert_eq!(wrappers[0].name(), "mcp__test__tool1");
        assert_eq!(wrappers[1].name(), "mcp__test__tool2");
    }

    fn annotated_tool(annotations: Option<McpToolAnnotations>) -> McpTool {
        McpTool {
            name: "fixture".to_string(),
            title: None,
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: None,
            annotations,
            icons: Vec::new(),
            meta: None,
        }
    }

    #[test]
    fn closed_world_read_only_annotation_does_not_escalate_confirmation() {
        let manager = Arc::new(McpManager::new());
        let wrapper = McpToolWrapper::new(
            "use_fixture".to_string(),
            annotated_tool(Some(McpToolAnnotations {
                read_only_hint: Some(true),
                destructive_hint: Some(false),
                idempotent_hint: Some(true),
                open_world_hint: Some(false),
                ..Default::default()
            })),
            manager,
        );

        assert!(!wrapper.requires_confirmation(&serde_json::json!({})));
    }

    #[test]
    fn unknown_open_world_mutating_and_submit_tools_escalate_confirmation() {
        let cases = [
            None,
            Some(McpToolAnnotations {
                read_only_hint: Some(true),
                open_world_hint: Some(true),
                ..Default::default()
            }),
            Some(McpToolAnnotations {
                read_only_hint: Some(false),
                open_world_hint: Some(false),
                ..Default::default()
            }),
            Some(McpToolAnnotations {
                read_only_hint: Some(true),
                open_world_hint: Some(false),
                additional: HashMap::from([(
                    "x-a3s-risk".to_string(),
                    serde_json::json!("submit"),
                )]),
                ..Default::default()
            }),
        ];

        for annotations in cases {
            let wrapper = McpToolWrapper::new(
                "use_fixture".to_string(),
                annotated_tool(annotations),
                Arc::new(McpManager::new()),
            );
            assert!(wrapper.requires_confirmation(&serde_json::json!({})));
        }
    }
}
