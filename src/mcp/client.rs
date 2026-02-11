//! MCP Client
//!
//! Provides a high-level client for interacting with MCP servers.

use crate::mcp::protocol::{
    CallToolParams, CallToolResult, ClientCapabilities, ClientInfo, InitializeParams,
    InitializeResult, JsonRpcNotification, JsonRpcRequest, ListResourcesResult, ListToolsResult,
    McpNotification, McpResource, McpTool, ReadResourceParams, ReadResourceResult,
    ServerCapabilities, PROTOCOL_VERSION,
};
use crate::mcp::transport::McpTransport;
use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// MCP client for communicating with MCP servers
pub struct McpClient {
    /// Server name
    pub name: String,
    /// Transport layer
    transport: Arc<dyn McpTransport>,
    /// Server capabilities (after initialization)
    capabilities: RwLock<ServerCapabilities>,
    /// Cached tools
    tools: RwLock<Vec<McpTool>>,
    /// Cached resources
    resources: RwLock<Vec<McpResource>>,
    /// Request ID counter
    request_id: AtomicU64,
    /// Initialized flag
    initialized: RwLock<bool>,
}

impl McpClient {
    /// Create a new MCP client with the given transport
    pub fn new(name: String, transport: Arc<dyn McpTransport>) -> Self {
        Self {
            name,
            transport,
            capabilities: RwLock::new(ServerCapabilities::default()),
            tools: RwLock::new(Vec::new()),
            resources: RwLock::new(Vec::new()),
            request_id: AtomicU64::new(1),
            initialized: RwLock::new(false),
        }
    }

    /// Get next request ID
    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Initialize the MCP connection
    pub async fn initialize(&self) -> Result<InitializeResult> {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: "a3s-code".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let request = JsonRpcRequest::new(
            self.next_id(),
            "initialize",
            Some(serde_json::to_value(&params)?),
        );

        let response = self.transport.request(request).await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "MCP initialize error: {} ({})",
                error.message,
                error.code
            ));
        }

        let result: InitializeResult = serde_json::from_value(
            response
                .result
                .ok_or_else(|| anyhow!("No result in response"))?,
        )?;

        // Store capabilities
        {
            let mut caps = self.capabilities.write().await;
            *caps = result.capabilities.clone();
        }

        // Send initialized notification
        let notification = JsonRpcNotification::new("notifications/initialized", None);
        self.transport.notify(notification).await?;

        // Mark as initialized
        {
            let mut init = self.initialized.write().await;
            *init = true;
        }

        tracing::info!(
            "MCP client '{}' initialized with server '{}' v{}",
            self.name,
            result.server_info.name,
            result.server_info.version
        );

        Ok(result)
    }

    /// Check if client is initialized
    pub async fn is_initialized(&self) -> bool {
        *self.initialized.read().await
    }

    /// Get server capabilities
    pub async fn capabilities(&self) -> ServerCapabilities {
        self.capabilities.read().await.clone()
    }

    /// List available tools
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let request = JsonRpcRequest::new(self.next_id(), "tools/list", None);
        let response = self.transport.request(request).await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "MCP list_tools error: {} ({})",
                error.message,
                error.code
            ));
        }

        let result: ListToolsResult =
            serde_json::from_value(response.result.ok_or_else(|| anyhow!("No result"))?)?;

        // Cache tools
        {
            let mut tools = self.tools.write().await;
            *tools = result.tools.clone();
        }

        Ok(result.tools)
    }

    /// Get cached tools
    pub async fn get_cached_tools(&self) -> Vec<McpTool> {
        self.tools.read().await.clone()
    }

    /// Call a tool
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<CallToolResult> {
        let params = CallToolParams {
            name: name.to_string(),
            arguments,
        };

        let request = JsonRpcRequest::new(
            self.next_id(),
            "tools/call",
            Some(serde_json::to_value(&params)?),
        );

        let response = self.transport.request(request).await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "MCP call_tool error: {} ({})",
                error.message,
                error.code
            ));
        }

        let result: CallToolResult =
            serde_json::from_value(response.result.ok_or_else(|| anyhow!("No result"))?)?;

        Ok(result)
    }

    /// List available resources
    pub async fn list_resources(&self) -> Result<Vec<McpResource>> {
        let request = JsonRpcRequest::new(self.next_id(), "resources/list", None);
        let response = self.transport.request(request).await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "MCP list_resources error: {} ({})",
                error.message,
                error.code
            ));
        }

        let result: ListResourcesResult =
            serde_json::from_value(response.result.ok_or_else(|| anyhow!("No result"))?)?;

        // Cache resources
        {
            let mut resources = self.resources.write().await;
            *resources = result.resources.clone();
        }

        Ok(result.resources)
    }

    /// Read a resource
    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult> {
        let params = ReadResourceParams {
            uri: uri.to_string(),
        };

        let request = JsonRpcRequest::new(
            self.next_id(),
            "resources/read",
            Some(serde_json::to_value(&params)?),
        );

        let response = self.transport.request(request).await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "MCP read_resource error: {} ({})",
                error.message,
                error.code
            ));
        }

        let result: ReadResourceResult =
            serde_json::from_value(response.result.ok_or_else(|| anyhow!("No result"))?)?;

        Ok(result)
    }

    /// Get notification receiver
    pub fn notifications(&self) -> tokio::sync::mpsc::Receiver<McpNotification> {
        self.transport.notifications()
    }

    /// Close the client
    pub async fn close(&self) -> Result<()> {
        self.transport.close().await
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_info() {
        let info = ClientInfo {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("test"));
    }

    #[test]
    fn test_initialize_params() {
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: "a3s-code".to_string(),
                version: "0.1.0".to_string(),
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("protocolVersion"));
        assert!(json.contains("clientInfo"));
    }

    #[test]
    fn test_client_info_serialize() {
        let info = ClientInfo {
            name: "test-client".to_string(),
            version: "2.0.0".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("test-client"));
        assert!(json.contains("2.0.0"));
    }

    #[test]
    fn test_client_info_deserialize() {
        let json = r#"{"name":"my-client","version":"1.2.3"}"#;
        let info: ClientInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.name, "my-client");
        assert_eq!(info.version, "1.2.3");
    }

    #[test]
    fn test_initialize_params_serialize() {
        let params = InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: "test".to_string(),
                version: "1.0.0".to_string(),
            },
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("2024-11-05"));
        assert!(json.contains("capabilities"));
    }

    #[test]
    fn test_call_tool_params_serialize() {
        let params = CallToolParams {
            name: "test_tool".to_string(),
            arguments: Some(serde_json::json!({"key": "value"})),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("test_tool"));
        assert!(json.contains("key"));
    }

    #[test]
    fn test_call_tool_params_no_arguments() {
        let params = CallToolParams {
            name: "simple_tool".to_string(),
            arguments: None,
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("simple_tool"));
    }

    #[test]
    fn test_read_resource_params_serialize() {
        let params = ReadResourceParams {
            uri: "file:///test.txt".to_string(),
        };
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("file:///test.txt"));
    }

    #[test]
    fn test_read_resource_params_deserialize() {
        let json = r#"{"uri":"http://example.com/resource"}"#;
        let params: ReadResourceParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.uri, "http://example.com/resource");
    }

    #[test]
    fn test_server_capabilities_default() {
        let caps = ServerCapabilities::default();
        let json = serde_json::to_string(&caps).unwrap();
        assert!(!json.is_empty());
    }

    #[test]
    fn test_client_capabilities_default() {
        let caps = ClientCapabilities::default();
        let json = serde_json::to_string(&caps).unwrap();
        assert!(!json.is_empty());
    }

    #[test]
    fn test_protocol_version_constant() {
        assert!(!PROTOCOL_VERSION.is_empty());
        assert!(PROTOCOL_VERSION.contains("-"));
    }
}
