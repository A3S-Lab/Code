//! MCP Manager
//!
//! Manages MCP server lifecycle and provides unified access to MCP tools.

use crate::mcp::client::McpClient;
use crate::mcp::protocol::{
    CallToolResult, McpServerConfig, McpTool, McpTransportConfig, ToolContent,
};
use crate::mcp::transport::stdio::StdioTransport;
use crate::mcp::transport::McpTransport;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// MCP server status
#[derive(Debug, Clone)]
pub struct McpServerStatus {
    pub name: String,
    pub connected: bool,
    pub enabled: bool,
    pub tool_count: usize,
    pub error: Option<String>,
}

/// MCP Manager for managing multiple MCP servers
pub struct McpManager {
    /// Connected clients
    clients: RwLock<HashMap<String, Arc<McpClient>>>,
    /// Server configurations
    configs: RwLock<HashMap<String, McpServerConfig>>,
}

impl McpManager {
    /// Create a new MCP manager
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            configs: RwLock::new(HashMap::new()),
        }
    }

    /// Register a server configuration
    pub async fn register_server(&self, config: McpServerConfig) {
        let name = config.name.clone();
        let mut configs = self.configs.write().await;
        configs.insert(name.clone(), config);
        tracing::info!("Registered MCP server: {}", name);
    }

    /// Connect to a registered server
    pub async fn connect(&self, name: &str) -> Result<()> {
        // Get config
        let config = {
            let configs = self.configs.read().await;
            configs
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow!("MCP server not found: {}", name))?
        };

        if !config.enabled {
            return Err(anyhow!("MCP server is disabled: {}", name));
        }

        // Create transport based on config
        let transport: Arc<dyn McpTransport> = match &config.transport {
            McpTransportConfig::Stdio { command, args } => {
                Arc::new(StdioTransport::spawn(command, args, &config.env).await?)
            }
            McpTransportConfig::Http { url: _, headers: _ } => {
                // HTTP transport not implemented yet
                return Err(anyhow!("HTTP transport not yet implemented"));
            }
        };

        // Create client
        let client = Arc::new(McpClient::new(name.to_string(), transport));

        // Initialize
        client.initialize().await?;

        // Fetch tools
        let tools = client.list_tools().await?;
        tracing::info!("MCP server '{}' connected with {} tools", name, tools.len());

        // Store client
        {
            let mut clients = self.clients.write().await;
            clients.insert(name.to_string(), client);
        }

        Ok(())
    }

    /// Disconnect from a server
    pub async fn disconnect(&self, name: &str) -> Result<()> {
        let client = {
            let mut clients = self.clients.write().await;
            clients.remove(name)
        };

        if let Some(client) = client {
            client.close().await?;
            tracing::info!("MCP server '{}' disconnected", name);
        }

        Ok(())
    }

    /// Get all MCP tools with server prefix
    ///
    /// Returns tools with names like `mcp__github__create_issue`
    pub async fn get_all_tools(&self) -> Vec<(String, McpTool)> {
        let clients = self.clients.read().await;
        let mut all_tools = Vec::new();

        for (server_name, client) in clients.iter() {
            let tools = client.get_cached_tools().await;
            for tool in tools {
                let full_name = format!("mcp__{}_{}", server_name, tool.name);
                all_tools.push((full_name, tool));
            }
        }

        all_tools
    }

    /// Call an MCP tool by full name
    ///
    /// Full name format: `mcp__<server>__<tool>`
    pub async fn call_tool(
        &self,
        full_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<CallToolResult> {
        // Parse full name
        let (server_name, tool_name) = Self::parse_tool_name(full_name)?;

        // Get client
        let client = {
            let clients = self.clients.read().await;
            clients
                .get(&server_name)
                .cloned()
                .ok_or_else(|| anyhow!("MCP server not connected: {}", server_name))?
        };

        // Call tool
        client.call_tool(&tool_name, arguments).await
    }

    /// Parse MCP tool full name into (server, tool)
    fn parse_tool_name(full_name: &str) -> Result<(String, String)> {
        // Format: mcp__<server>__<tool>
        if !full_name.starts_with("mcp__") {
            return Err(anyhow!("Invalid MCP tool name: {}", full_name));
        }

        let rest = &full_name[5..]; // Skip "mcp__"
        let parts: Vec<&str> = rest.splitn(2, "__").collect();

        if parts.len() != 2 {
            return Err(anyhow!("Invalid MCP tool name format: {}", full_name));
        }

        Ok((parts[0].to_string(), parts[1].to_string()))
    }

    /// Get status of all servers
    pub async fn get_status(&self) -> HashMap<String, McpServerStatus> {
        let configs = self.configs.read().await;
        let clients = self.clients.read().await;
        let mut status = HashMap::new();

        for (name, config) in configs.iter() {
            let client = clients.get(name);
            let (connected, tool_count) = if let Some(c) = client {
                (c.is_connected(), c.get_cached_tools().await.len())
            } else {
                (false, 0)
            };

            status.insert(
                name.clone(),
                McpServerStatus {
                    name: name.clone(),
                    connected,
                    enabled: config.enabled,
                    tool_count,
                    error: None,
                },
            );
        }

        status
    }

    /// Get a specific client
    pub async fn get_client(&self, name: &str) -> Option<Arc<McpClient>> {
        let clients = self.clients.read().await;
        clients.get(name).cloned()
    }

    /// Check if a server is connected
    pub async fn is_connected(&self, name: &str) -> bool {
        let clients = self.clients.read().await;
        clients.get(name).map(|c| c.is_connected()).unwrap_or(false)
    }

    /// List connected server names
    pub async fn list_connected(&self) -> Vec<String> {
        let clients = self.clients.read().await;
        clients.keys().cloned().collect()
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert MCP tool result to string output
pub fn tool_result_to_string(result: &CallToolResult) -> String {
    let mut output = String::new();

    for content in &result.content {
        match content {
            ToolContent::Text { text } => {
                output.push_str(text);
                output.push('\n');
            }
            ToolContent::Image { data: _, mime_type } => {
                output.push_str(&format!("[Image: {}]\n", mime_type));
            }
            ToolContent::Resource { resource } => {
                if let Some(text) = &resource.text {
                    output.push_str(text);
                    output.push('\n');
                } else {
                    output.push_str(&format!("[Resource: {}]\n", resource.uri));
                }
            }
        }
    }

    output.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_name() {
        let (server, tool) = McpManager::parse_tool_name("mcp__github__create_issue").unwrap();
        assert_eq!(server, "github");
        assert_eq!(tool, "create_issue");
    }

    #[test]
    fn test_parse_tool_name_with_underscores() {
        let (server, tool) = McpManager::parse_tool_name("mcp__my_server__my_tool_name").unwrap();
        assert_eq!(server, "my_server");
        assert_eq!(tool, "my_tool_name");
    }

    #[test]
    fn test_parse_tool_name_invalid() {
        assert!(McpManager::parse_tool_name("invalid_name").is_err());
        assert!(McpManager::parse_tool_name("mcp__nodelimiter").is_err());
    }

    #[test]
    fn test_tool_result_to_string() {
        let result = CallToolResult {
            content: vec![
                ToolContent::Text {
                    text: "Line 1".to_string(),
                },
                ToolContent::Text {
                    text: "Line 2".to_string(),
                },
            ],
            is_error: false,
        };

        let output = tool_result_to_string(&result);
        assert!(output.contains("Line 1"));
        assert!(output.contains("Line 2"));
    }

    #[tokio::test]
    async fn test_mcp_manager_new() {
        let manager = McpManager::new();
        let status = manager.get_status().await;
        assert!(status.is_empty());
    }

    #[tokio::test]
    async fn test_mcp_manager_register_server() {
        let manager = McpManager::new();

        let config = McpServerConfig {
            name: "test".to_string(),
            transport: McpTransportConfig::Stdio {
                command: "echo".to_string(),
                args: vec![],
            },
            enabled: true,
            env: HashMap::new(),
            oauth: None,
        };

        manager.register_server(config).await;

        let status = manager.get_status().await;
        assert!(status.contains_key("test"));
        assert!(!status["test"].connected);
    }
}
