//! MCP Protocol Type Definitions
//!
//! Defines the core types for the Model Context Protocol (MCP).
//! Based on the MCP specification: https://spec.modelcontextprotocol.io/

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// MCP protocol version
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        }
    }
}

/// JSON-RPC response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC notification (no id)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcNotification {
    pub fn new(method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        }
    }
}

// ============================================================================
// MCP Initialize
// ============================================================================

/// Client capabilities
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapability>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RootsCapability {
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SamplingCapability {}

/// Client info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Initialize request params
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

/// Server capabilities
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsCapability {
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesCapability {
    #[serde(default)]
    pub subscribe: bool,
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptsCapability {
    #[serde(default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoggingCapability {}

/// Server info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// Initialize result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}

// ============================================================================
// MCP Tools
// ============================================================================

/// MCP tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// List tools result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpTool>,
}

/// Call tool params
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// Tool content types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolContent {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Resource {
        resource: ResourceContent,
    },
}

/// Resource content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

/// Call tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    #[serde(default)]
    pub is_error: bool,
}

// ============================================================================
// MCP Resources
// ============================================================================

/// MCP resource definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// List resources result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResourcesResult {
    pub resources: Vec<McpResource>,
}

/// Read resource params
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceParams {
    pub uri: String,
}

/// Read resource result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceResult {
    pub contents: Vec<ResourceContent>,
}

// ============================================================================
// MCP Prompts
// ============================================================================

/// MCP prompt definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
}

/// Prompt argument
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// List prompts result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPromptsResult {
    pub prompts: Vec<McpPrompt>,
}

// ============================================================================
// MCP Notifications
// ============================================================================

/// MCP notification types
#[derive(Debug, Clone)]
pub enum McpNotification {
    ToolsListChanged,
    ResourcesListChanged,
    PromptsListChanged,
    Progress {
        progress_token: String,
        progress: f64,
        total: Option<f64>,
    },
    Log {
        level: String,
        logger: Option<String>,
        data: serde_json::Value,
    },
    Unknown {
        method: String,
        params: Option<serde_json::Value>,
    },
}

impl McpNotification {
    pub fn from_json_rpc(notification: &JsonRpcNotification) -> Self {
        match notification.method.as_str() {
            "notifications/tools/list_changed" => McpNotification::ToolsListChanged,
            "notifications/resources/list_changed" => McpNotification::ResourcesListChanged,
            "notifications/prompts/list_changed" => McpNotification::PromptsListChanged,
            "notifications/progress" => {
                if let Some(params) = &notification.params {
                    let progress_token = params
                        .get("progressToken")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let progress = params
                        .get("progress")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let total = params.get("total").and_then(|v| v.as_f64());
                    McpNotification::Progress {
                        progress_token,
                        progress,
                        total,
                    }
                } else {
                    McpNotification::Unknown {
                        method: notification.method.clone(),
                        params: notification.params.clone(),
                    }
                }
            }
            "notifications/message" => {
                if let Some(params) = &notification.params {
                    let level = params
                        .get("level")
                        .and_then(|v| v.as_str())
                        .unwrap_or("info")
                        .to_string();
                    let logger = params
                        .get("logger")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let data = params
                        .get("data")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    McpNotification::Log {
                        level,
                        logger,
                        data,
                    }
                } else {
                    McpNotification::Unknown {
                        method: notification.method.clone(),
                        params: notification.params.clone(),
                    }
                }
            }
            _ => McpNotification::Unknown {
                method: notification.method.clone(),
                params: notification.params.clone(),
            },
        }
    }
}

// ============================================================================
// Configuration Types
// ============================================================================

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name (used for tool prefix)
    pub name: String,
    /// Transport configuration
    pub transport: McpTransportConfig,
    /// Whether enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// OAuth configuration (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthConfig>,
}

fn default_true() -> bool {
    true
}

/// Transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpTransportConfig {
    /// Local process (stdio)
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Remote HTTP + SSE
    Http {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

/// OAuth configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub redirect_uri: String,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_request_serialize() {
        let req = JsonRpcRequest::new(1, "initialize", Some(serde_json::json!({"test": true})));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"initialize\""));
    }

    #[test]
    fn test_json_rpc_response_deserialize() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"success":true}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_json_rpc_error_deserialize() {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
    }

    #[test]
    fn test_mcp_tool_deserialize() {
        let json = r#"{
            "name": "create_issue",
            "description": "Create a GitHub issue",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "body": {"type": "string"}
                },
                "required": ["title"]
            }
        }"#;
        let tool: McpTool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "create_issue");
        assert!(tool.description.is_some());
    }

    #[test]
    fn test_tool_content_text() {
        let content = ToolContent::Text {
            text: "Hello".to_string(),
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"Hello\""));
    }

    #[test]
    fn test_mcp_transport_config_stdio() {
        let json = r#"{
            "type": "stdio",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-github"]
        }"#;
        let config: McpTransportConfig = serde_json::from_str(json).unwrap();
        match config {
            McpTransportConfig::Stdio { command, args } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("Expected Stdio transport"),
        }
    }

    #[test]
    fn test_mcp_transport_config_http() {
        let json = r#"{
            "type": "http",
            "url": "https://mcp.example.com/api",
            "headers": {"Authorization": "Bearer token"}
        }"#;
        let config: McpTransportConfig = serde_json::from_str(json).unwrap();
        match config {
            McpTransportConfig::Http { url, headers } => {
                assert_eq!(url, "https://mcp.example.com/api");
                assert!(headers.contains_key("Authorization"));
            }
            _ => panic!("Expected Http transport"),
        }
    }

    #[test]
    fn test_mcp_notification_parse() {
        let notification = JsonRpcNotification::new("notifications/tools/list_changed", None);
        let mcp_notif = McpNotification::from_json_rpc(&notification);
        match mcp_notif {
            McpNotification::ToolsListChanged => {}
            _ => panic!("Expected ToolsListChanged"),
        }
    }
}
