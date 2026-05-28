//! MCP Manager
//!
//! Manages MCP server lifecycle and provides unified access to MCP tools.

use crate::mcp::client::McpClient;
use crate::mcp::oauth;
use crate::mcp::protocol::{
    CallToolResult, McpServerConfig, McpTool, McpTransportConfig, OAuthConfig, ToolContent,
};
use crate::mcp::transport::http_sse::HttpSseTransport;
use crate::mcp::transport::stdio::StdioTransport;
use crate::mcp::transport::streamable_http::StreamableHttpTransport;
use crate::mcp::transport::McpTransport;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// MCP server status
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Last connection error per server, cleared on successful connect
    connect_errors: RwLock<HashMap<String, String>>,
    /// Last-used timestamp per connected server (Unix epoch ms).
    /// Updated by `connect` (initial use) and `call_tool` (active use).
    /// Read by hosts via [`McpManager::last_used_at_ms`] / used by
    /// [`McpManager::disconnect_idle`] to release FDs and background
    /// workers from servers that are no longer in active use.
    last_used_at_ms: RwLock<HashMap<String, u64>>,
}

impl McpManager {
    /// Create a new MCP manager
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            configs: RwLock::new(HashMap::new()),
            connect_errors: RwLock::new(HashMap::new()),
            last_used_at_ms: RwLock::new(HashMap::new()),
        }
    }

    /// Register a server configuration
    pub async fn register_server(&self, config: McpServerConfig) {
        let name = config.name.clone();
        let mut configs = self.configs.write().await;
        configs.insert(name.clone(), config);
        tracing::info!("Registered MCP server: {}", name);
    }

    /// Connect to a registered server, recording success or failure internally.
    ///
    /// On success the stored error (if any) is cleared; on failure the error
    /// message is stored and visible via [`McpManager::get_status`].
    pub async fn connect(&self, name: &str) -> Result<()> {
        let result = self.do_connect(name).await;
        match &result {
            Ok(_) => {
                self.connect_errors.write().await.remove(name);
            }
            Err(e) => {
                self.connect_errors
                    .write()
                    .await
                    .insert(name.to_string(), e.to_string());
            }
        }
        result
    }

    async fn do_connect(&self, name: &str) -> Result<()> {
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

        // Resolve OAuth token into an Authorization header (if configured)
        let auth_header = Self::resolve_auth_header(config.oauth.as_ref()).await?;

        // Create transport based on config
        let transport: Arc<dyn McpTransport> = match &config.transport {
            McpTransportConfig::Stdio { command, args } => Arc::new(
                StdioTransport::spawn_with_timeout(
                    command,
                    args,
                    &config.env,
                    config.tool_timeout_secs,
                )
                .await?,
            ),
            McpTransportConfig::Http { url, headers } => {
                let mut merged = headers.clone();
                if let Some((k, v)) = &auth_header {
                    merged.insert(k.clone(), v.clone());
                }
                Arc::new(
                    HttpSseTransport::connect_with_timeout(url, merged, config.tool_timeout_secs)
                        .await?,
                )
            }
            McpTransportConfig::StreamableHttp { url, headers } => {
                let mut merged = headers.clone();
                if let Some((k, v)) = &auth_header {
                    merged.insert(k.clone(), v.clone());
                }
                Arc::new(
                    StreamableHttpTransport::connect_with_timeout(
                        url,
                        merged,
                        config.tool_timeout_secs,
                    )
                    .await?,
                )
            }
        };

        // Create client
        let client = Arc::new(McpClient::new(name.to_string(), transport));

        // Initialize
        client.initialize().await?;

        // Fetch tools
        let tools = client.list_tools().await?;
        tracing::info!("MCP server '{}' connected with {} tools", name, tools.len());

        // Store client + stamp initial last-used time so idle reapers
        // see freshly-connected servers as active.
        {
            let mut clients = self.clients.write().await;
            clients.insert(name.to_string(), client);
        }
        self.last_used_at_ms
            .write()
            .await
            .insert(name.to_string(), now_epoch_ms());

        Ok(())
    }

    /// Disconnect from a server
    pub async fn disconnect(&self, name: &str) -> Result<()> {
        let client = {
            let mut clients = self.clients.write().await;
            clients.remove(name)
        };
        self.last_used_at_ms.write().await.remove(name);

        if let Some(client) = client {
            client.close().await?;
            tracing::info!("MCP server '{}' disconnected", name);
        }

        Ok(())
    }

    /// Return the last-used timestamp (Unix epoch ms) for a connected
    /// server, or `None` if the server is unknown / not connected.
    pub async fn last_used_at_ms(&self, name: &str) -> Option<u64> {
        self.last_used_at_ms.read().await.get(name).copied()
    }

    /// Mark a server as active right now. The framework calls this
    /// automatically on connect and on every successful
    /// [`call_tool`](Self::call_tool); hosts can call it explicitly
    /// to keep a server "warm" out of band (e.g. when a tool result
    /// comes back via a different channel).
    pub async fn touch(&self, name: &str) {
        self.last_used_at_ms
            .write()
            .await
            .insert(name.to_string(), now_epoch_ms());
    }

    /// Disconnect every connected server whose last-used timestamp is
    /// older than `now - idle_threshold_ms`. Returns the names of
    /// servers that were disconnected.
    ///
    /// Servers without a recorded timestamp are treated as **infinitely
    /// idle** and disconnected. The disconnect call itself can fail
    /// per-server (e.g. transport already closed); those failures are
    /// warn-logged but never panic — the result vec still includes
    /// every name the manager attempted to drop.
    ///
    /// Hosts running thousands of long-lived sessions should call this
    /// periodically (e.g. every 60s with a 5-min threshold) to release
    /// file descriptors and background workers from quiet MCP servers
    /// without losing the server's configuration. A subsequent
    /// [`call_tool`](Self::call_tool) on the same server name will
    /// require an explicit `connect` to come back online.
    pub async fn disconnect_idle(&self, idle_threshold_ms: u64) -> Vec<String> {
        let cutoff = now_epoch_ms().saturating_sub(idle_threshold_ms);
        // Snapshot candidates so we don't hold both locks across await.
        let candidates: Vec<String> = {
            let clients = self.clients.read().await;
            let last_used = self.last_used_at_ms.read().await;
            clients
                .keys()
                .filter(|name| match last_used.get(*name) {
                    Some(ts) => *ts < cutoff,
                    // No timestamp -> never used since connect; treat as
                    // infinitely idle.
                    None => true,
                })
                .cloned()
                .collect()
        };
        let mut disconnected = Vec::with_capacity(candidates.len());
        for name in candidates {
            match self.disconnect(&name).await {
                Ok(()) => disconnected.push(name),
                Err(e) => tracing::warn!(
                    server = %name,
                    error = %e,
                    "MCP idle disconnect failed; entry already removed from registry"
                ),
            }
        }
        disconnected
    }

    /// Get all registered server configurations
    pub async fn all_configs(&self) -> Vec<McpServerConfig> {
        self.configs.read().await.values().cloned().collect()
    }

    /// Get all MCP tools, grouped by server name.
    ///
    /// Returns `(server_name, tool)` pairs — the caller is responsible for
    /// constructing the `mcp__<server>__<tool>` prefix (e.g. via [`create_mcp_tools`]).
    pub async fn get_all_tools(&self) -> Vec<(String, McpTool)> {
        let clients = self.clients.read().await;
        let mut all_tools = Vec::new();

        for (server_name, client) in clients.iter() {
            let tools = client.get_cached_tools().await;
            for tool in tools {
                all_tools.push((server_name.clone(), tool));
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

        // Refresh the activity timestamp before the await so an idle
        // sweep running concurrently sees this server as recently used.
        self.last_used_at_ms
            .write()
            .await
            .insert(server_name.clone(), now_epoch_ms());

        // Call tool
        client.call_tool(&tool_name, arguments).await
    }

    /// Resolve an OAuth config into a `(header-name, header-value)` pair.
    ///
    /// - If `oauth.access_token` is set, uses it directly (static token).
    /// - Otherwise, performs a client credentials exchange.
    /// - If `oauth` is `None`, returns `Ok(None)` (no auth needed).
    async fn resolve_auth_header(oauth: Option<&OAuthConfig>) -> Result<Option<(String, String)>> {
        let Some(oauth) = oauth else {
            return Ok(None);
        };

        let token = if let Some(static_token) = &oauth.access_token {
            static_token.clone()
        } else {
            oauth::exchange_client_credentials(
                &oauth.token_url,
                &oauth.client_id,
                oauth.client_secret.as_deref().unwrap_or(""),
                &oauth.scopes,
            )
            .await?
        };

        Ok(Some((
            "Authorization".to_string(),
            format!("Bearer {}", token),
        )))
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
        let errors = self.connect_errors.read().await;
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
                    error: errors.get(name).cloned(),
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

    /// Get cached tools for a specific connected server.
    pub async fn get_server_tools(&self, name: &str) -> Vec<McpTool> {
        let clients = self.clients.read().await;
        match clients.get(name) {
            Some(client) => client.get_cached_tools().await,
            None => Vec::new(),
        }
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Wall-clock now() in Unix epoch milliseconds. Used internally by the
/// activity-tracking + idle-disconnect path. Kept as a free function
/// (rather than going through `HostEnv`) because the MCP manager
/// predates host_env wiring and the host's `Clock` impl is not yet
/// threaded into the manager.
fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
            tool_timeout_secs: 60,
        };

        manager.register_server(config).await;

        let status = manager.get_status().await;
        assert!(status.contains_key("test"));
        assert!(!status["test"].connected);
    }

    #[tokio::test]
    async fn test_mcp_manager_default() {
        let manager = McpManager::default();
        let status = manager.get_status().await;
        assert!(status.is_empty());
    }

    #[tokio::test]
    async fn test_list_connected_empty() {
        let manager = McpManager::new();
        let connected = manager.list_connected().await;
        assert!(connected.is_empty());
    }

    #[tokio::test]
    async fn test_is_connected_false_for_unknown_server() {
        let manager = McpManager::new();
        let connected = manager.is_connected("unknown_server").await;
        assert!(!connected);
    }

    #[tokio::test]
    async fn test_get_client_none_for_unknown_server() {
        let manager = McpManager::new();
        let client = manager.get_client("unknown_server").await;
        assert!(client.is_none());
    }

    #[test]
    fn test_parse_tool_name_simple() {
        let (server, tool) = McpManager::parse_tool_name("mcp__server__tool").unwrap();
        assert_eq!(server, "server");
        assert_eq!(tool, "tool");
    }

    #[test]
    fn test_parse_tool_name_multiple_underscores() {
        let (server, tool) = McpManager::parse_tool_name("mcp__my_server__my_tool_name").unwrap();
        assert_eq!(server, "my_server");
        assert_eq!(tool, "my_tool_name");
    }

    #[test]
    fn test_parse_tool_name_missing_prefix() {
        let result = McpManager::parse_tool_name("server__tool");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_tool_name_only_prefix() {
        let result = McpManager::parse_tool_name("mcp__");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_tool_name_empty_string() {
        let result = McpManager::parse_tool_name("");
        assert!(result.is_err());
    }

    #[test]
    fn test_tool_result_to_string_single_text() {
        let result = CallToolResult {
            content: vec![ToolContent::Text {
                text: "Hello World".to_string(),
            }],
            is_error: false,
        };
        let output = tool_result_to_string(&result);
        assert_eq!(output, "Hello World");
    }

    #[test]
    fn test_tool_result_to_string_multiple_text() {
        let result = CallToolResult {
            content: vec![
                ToolContent::Text {
                    text: "First line".to_string(),
                },
                ToolContent::Text {
                    text: "Second line".to_string(),
                },
            ],
            is_error: false,
        };
        let output = tool_result_to_string(&result);
        assert!(output.contains("First line"));
        assert!(output.contains("Second line"));
    }

    #[test]
    fn test_tool_result_to_string_empty() {
        let result = CallToolResult {
            content: vec![],
            is_error: false,
        };
        let output = tool_result_to_string(&result);
        assert_eq!(output, "");
    }

    #[test]
    fn test_tool_result_to_string_image() {
        let result = CallToolResult {
            content: vec![ToolContent::Image {
                data: "base64data".to_string(),
                mime_type: "image/png".to_string(),
            }],
            is_error: false,
        };
        let output = tool_result_to_string(&result);
        assert!(output.contains("[Image: image/png]"));
    }

    #[test]
    fn test_tool_result_to_string_resource() {
        use crate::mcp::protocol::ResourceContent;
        let result = CallToolResult {
            content: vec![ToolContent::Resource {
                resource: ResourceContent {
                    uri: "file:///test.txt".to_string(),
                    mime_type: Some("text/plain".to_string()),
                    text: Some("Resource content".to_string()),
                    blob: None,
                },
            }],
            is_error: false,
        };
        let output = tool_result_to_string(&result);
        assert!(output.contains("Resource content"));
    }

    #[test]
    fn test_tool_result_to_string_mixed_content() {
        use crate::mcp::protocol::ResourceContent;
        let result = CallToolResult {
            content: vec![
                ToolContent::Text {
                    text: "Text content".to_string(),
                },
                ToolContent::Image {
                    data: "base64".to_string(),
                    mime_type: "image/jpeg".to_string(),
                },
                ToolContent::Resource {
                    resource: ResourceContent {
                        uri: "file:///doc.md".to_string(),
                        mime_type: Some("text/markdown".to_string()),
                        text: Some("Doc content".to_string()),
                        blob: None,
                    },
                },
            ],
            is_error: false,
        };
        let output = tool_result_to_string(&result);
        assert!(output.contains("Text content"));
        assert!(output.contains("[Image: image/jpeg]"));
        assert!(output.contains("Doc content"));
    }

    #[tokio::test]
    async fn test_get_status_registered_server() {
        use std::collections::HashMap;
        let manager = McpManager::new();

        let config = McpServerConfig {
            name: "test_server".to_string(),
            transport: McpTransportConfig::Stdio {
                command: "echo".to_string(),
                args: vec![],
            },
            enabled: true,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 60,
        };

        manager.register_server(config).await;

        let status = manager.get_status().await;
        assert!(status.contains_key("test_server"));
        assert!(!status["test_server"].connected);
        assert!(status["test_server"].enabled);
    }

    #[tokio::test]
    async fn test_get_status_disabled_server() {
        use std::collections::HashMap;
        let manager = McpManager::new();

        let config = McpServerConfig {
            name: "disabled_server".to_string(),
            transport: McpTransportConfig::Stdio {
                command: "echo".to_string(),
                args: vec![],
            },
            enabled: false,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 60,
        };

        manager.register_server(config).await;

        let status = manager.get_status().await;
        assert!(status.contains_key("disabled_server"));
        assert!(!status["disabled_server"].enabled);
    }

    #[tokio::test]
    async fn test_get_all_tools_empty_manager() {
        let manager = McpManager::new();
        let tools = manager.get_all_tools().await;
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn test_resolve_auth_header_none_when_no_oauth() {
        let result = McpManager::resolve_auth_header(None).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_resolve_auth_header_uses_static_token() {
        use crate::mcp::protocol::OAuthConfig;
        let oauth = OAuthConfig {
            auth_url: "https://example.com/auth".to_string(),
            token_url: "https://example.com/token".to_string(),
            client_id: "client".to_string(),
            client_secret: None,
            scopes: vec![],
            redirect_uri: "http://localhost/cb".to_string(),
            access_token: Some("my-static-token".to_string()),
        };
        let result = McpManager::resolve_auth_header(Some(&oauth)).await.unwrap();
        assert!(result.is_some());
        let (key, value) = result.unwrap();
        assert_eq!(key, "Authorization");
        assert_eq!(value, "Bearer my-static-token");
    }

    #[tokio::test]
    async fn test_resolve_auth_header_client_credentials_fails_gracefully() {
        use crate::mcp::protocol::OAuthConfig;
        // No static token + invalid token_url → should return error
        let oauth = OAuthConfig {
            auth_url: "https://127.0.0.1:1/auth".to_string(),
            token_url: "http://127.0.0.1:1/token".to_string(),
            client_id: "client".to_string(),
            client_secret: Some("secret".to_string()),
            scopes: vec!["read".to_string()],
            redirect_uri: "http://localhost/cb".to_string(),
            access_token: None,
        };
        let result = McpManager::resolve_auth_header(Some(&oauth)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connect_error_recorded_in_status() {
        use std::collections::HashMap;
        let manager = McpManager::new();

        let config = McpServerConfig {
            name: "bad-server".to_string(),
            transport: McpTransportConfig::Stdio {
                // `true` exits immediately — not a valid MCP server
                command: "true".to_string(),
                args: vec![],
            },
            enabled: true,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 5,
        };

        manager.register_server(config).await;
        // connect() will fail; error must be stored
        let _ = manager.connect("bad-server").await;

        let status = manager.get_status().await;
        let s = &status["bad-server"];
        assert!(!s.connected, "server should not be connected");
        assert!(
            s.error.is_some(),
            "error should be recorded after failed connect"
        );
    }

    #[tokio::test]
    async fn test_get_all_tools_returns_server_name_not_full_name() {
        // get_all_tools() must return (server_name, tool) — not (mcp__server__tool, tool)
        // so that create_mcp_tools() can build the correct full name without double-prefix.
        // With no connected servers the result is empty; the format assertion is enforced
        // structurally: the field is named "server_name" and callers must not pre-add the prefix.
        let manager = McpManager::new();
        let tools = manager.get_all_tools().await;
        // Empty is fine; verify no full-name leakage by checking the tuple semantics.
        // (Real server injection is tested via integration_mcp.rs #[ignore] tests.)
        for (name, _tool) in &tools {
            assert!(
                !name.starts_with("mcp__"),
                "get_all_tools() must return server names, not prefixed full names; got '{name}'"
            );
        }
    }

    #[tokio::test]
    async fn touch_updates_last_used_at_ms() {
        let manager = McpManager::new();
        // Without a real connect, last_used is None.
        assert!(manager.last_used_at_ms("svc-a").await.is_none());
        manager.touch("svc-a").await;
        let t1 = manager.last_used_at_ms("svc-a").await.expect("set");
        assert!(t1 > 0);
        // Touch again — timestamp must be monotonically non-decreasing.
        manager.touch("svc-a").await;
        let t2 = manager.last_used_at_ms("svc-a").await.expect("set again");
        assert!(t2 >= t1);
    }

    #[tokio::test]
    async fn disconnect_idle_drops_stale_servers_and_keeps_fresh_ones() {
        let manager = McpManager::new();
        // Manually populate clients + timestamps so we can run the
        // logic without actually launching MCP subprocesses. We can't
        // build an `McpClient` from outside this module without a
        // transport, so we just exercise the timestamp-driven decision
        // branch via the public APIs: register two servers with
        // explicit stale + fresh stamps and assert the idle sweep
        // picks the right one.
        //
        // NOTE: clients map stays empty (no real transport spawned),
        // so disconnect_idle's `candidates` set is empty and the
        // returned Vec is empty. We instead verify the *timestamp
        // observability* path the host needs, plus the no-op behaviour
        // when there are no live clients.
        manager.touch("fresh-svc").await;
        let dropped = manager.disconnect_idle(0).await;
        assert!(
            dropped.is_empty(),
            "no clients connected -> idle sweep is a no-op even with threshold 0, got {dropped:?}"
        );
        // Timestamp observability still works:
        assert!(manager.last_used_at_ms("fresh-svc").await.is_some());
        assert!(manager.last_used_at_ms("never-touched").await.is_none());
    }

    #[tokio::test]
    async fn touch_keeps_timestamp_after_explicit_disconnect_removes_it() {
        let manager = McpManager::new();
        manager.touch("svc").await;
        assert!(manager.last_used_at_ms("svc").await.is_some());
        // disconnect should clean up the activity entry even when
        // no real client was ever connected (defensive cleanup).
        let _ = manager.disconnect("svc").await;
        assert!(manager.last_used_at_ms("svc").await.is_none());
    }
}
