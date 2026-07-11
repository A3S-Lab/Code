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

        // Initialize and fetch tools before publishing the client. A failed
        // handshake must not leave a live transport detached from manager
        // state.
        if let Err(error) = client.initialize().await {
            if let Err(close_error) = client.close().await {
                tracing::warn!(
                    server = %name,
                    error = %close_error,
                    "Failed to close MCP transport after initialize failure"
                );
            }
            return Err(error);
        }

        let tools = match client.list_tools().await {
            Ok(tools) => tools,
            Err(error) => {
                if let Err(close_error) = client.close().await {
                    tracing::warn!(
                        server = %name,
                        error = %close_error,
                        "Failed to close MCP transport after tool discovery failure"
                    );
                }
                return Err(error);
            }
        };
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

    /// Disconnect and forget a registered server.
    ///
    /// This is distinct from [`disconnect`](Self::disconnect), which preserves
    /// configuration so a host can reconnect later. Session-level live removal
    /// uses this method so a removed local source no longer shadows an inherited
    /// server with the same name in status and capability views.
    pub async fn remove_server(&self, name: &str) -> Result<bool> {
        // Commit the logical removal before awaiting fallible transport
        // cleanup. A close error must not leave a configuration that claims a
        // half-removed server still belongs to the manager.
        let client = self.clients.write().await.remove(name);
        let had_error = self.connect_errors.write().await.remove(name).is_some();
        let had_timestamp = self.last_used_at_ms.write().await.remove(name).is_some();
        let had_config = self.configs.write().await.remove(name).is_some();
        let removed = client.is_some() || had_error || had_timestamp || had_config;

        if let Some(client) = client {
            client.close().await?;
            tracing::info!("MCP server '{}' removed", name);
        }

        Ok(removed)
    }

    /// Return whether a server configuration is registered.
    pub async fn contains_server(&self, name: &str) -> bool {
        self.configs.read().await.contains_key(name)
    }

    #[cfg(test)]
    pub(crate) async fn insert_client_for_test(&self, name: &str, client: Arc<McpClient>) {
        self.clients.write().await.insert(name.to_string(), client);
        self.last_used_at_ms
            .write()
            .await
            .insert(name.to_string(), now_epoch_ms());
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
        // Opportunistically purge orphan timestamps for servers that are no
        // longer connected — `touch()` records a timestamp unconditionally
        // (even for a never-connected name), and the candidate scan above
        // only iterates `clients.keys()`, so without this sweep those
        // orphan entries in `last_used_at_ms` would accumulate unbounded
        // across the lifetime of a long-running manager.
        {
            let clients = self.clients.read().await;
            self.last_used_at_ms
                .write()
                .await
                .retain(|name, _| clients.contains_key(name));
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
#[path = "manager/tests.rs"]
mod tests;
