//! MCP Protocol Type Definitions
//!
//! Defines the core types for the Model Context Protocol (MCP).
//! Based on the MCP specification: <https://spec.modelcontextprotocol.io/>

use serde::{Deserialize, Deserializer, Serialize};
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
#[derive(Debug, Clone, Serialize)]
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
    /// Per-tool execution timeout in seconds (default: 60)
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_secs: u64,
}

impl<'de> Deserialize<'de> for McpServerConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        use serde_json::Value;

        let mut map = serde_json::Map::deserialize(deserializer)?;

        // Build transport from flat ACL fields (transport = "stdio", command = "...", args = [...])
        // or from a nested transport object ({ type = "stdio", command = "..." })
        let transport = if let Some(t) = map.remove("transport") {
            match &t {
                Value::String(kind) => {
                    // Flat ACL-like format: transport = "stdio", command = "...", args = [...]
                    match kind.as_str() {
                        "stdio" => {
                            let command = map
                                .remove("command")
                                .and_then(|v| v.as_str().map(String::from))
                                .ok_or_else(|| D::Error::missing_field("command"))?;
                            let args = map
                                .remove("args")
                                .and_then(|v| serde_json::from_value(v).ok())
                                .unwrap_or_default();
                            McpTransportConfig::Stdio { command, args }
                        }
                        "http" => {
                            let url = map
                                .remove("url")
                                .and_then(|v| v.as_str().map(String::from))
                                .ok_or_else(|| D::Error::missing_field("url"))?;
                            let headers = map
                                .remove("headers")
                                .and_then(|v| serde_json::from_value(v).ok())
                                .unwrap_or_default();
                            McpTransportConfig::Http { url, headers }
                        }
                        "streamable-http" | "streamable_http" => {
                            let url = map
                                .remove("url")
                                .and_then(|v| v.as_str().map(String::from))
                                .ok_or_else(|| D::Error::missing_field("url"))?;
                            let headers = map
                                .remove("headers")
                                .and_then(|v| serde_json::from_value(v).ok())
                                .unwrap_or_default();
                            McpTransportConfig::StreamableHttp { url, headers }
                        }
                        other => {
                            return Err(D::Error::unknown_variant(
                                other,
                                &["stdio", "http", "streamable-http"],
                            ));
                        }
                    }
                }
                // Nested object format: transport { type = "stdio", command = "..." }
                Value::Object(_) => serde_json::from_value(t).map_err(D::Error::custom)?,
                _ => return Err(D::Error::custom("transport must be a string or object")),
            }
        } else {
            return Err(D::Error::missing_field("transport"));
        };

        let name = map
            .remove("name")
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| D::Error::missing_field("name"))?;
        let enabled = map
            .remove("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let env = map
            .remove("env")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let oauth = map
            .remove("oauth")
            .and_then(|v| serde_json::from_value(v).ok());
        let tool_timeout_secs = map
            .remove("tool_timeout_secs")
            .or_else(|| map.remove("toolTimeoutSecs"))
            .and_then(|v| v.as_u64())
            .unwrap_or(60);

        Ok(McpServerConfig {
            name,
            transport,
            enabled,
            env,
            oauth,
            tool_timeout_secs,
        })
    }
}

#[allow(dead_code)] // used by serde default = "default_tool_timeout"
fn default_tool_timeout() -> u64 {
    60
}

#[allow(dead_code)]
fn default_true() -> bool {
    true
}

/// Transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum McpTransportConfig {
    /// Local process (stdio)
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Remote HTTP + SSE (legacy, pre-2025-03-26)
    Http {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
    /// Streamable HTTP (MCP 2025-03-26 spec)
    ///
    /// Single endpoint handles all communication.
    /// POST with `Accept: application/json, text/event-stream`.
    StreamableHttp {
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
    /// Static access token — if set, skips the OAuth exchange flow.
    /// Useful for long-lived tokens or service accounts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
}

#[cfg(test)]
#[path = "protocol/tests.rs"]
mod tests;
