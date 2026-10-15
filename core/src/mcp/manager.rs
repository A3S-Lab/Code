//! MCP Manager
//!
//! Manages MCP server lifecycle and provides unified access to MCP tools.

use crate::mcp::client::McpClient;
use crate::mcp::oauth;
use crate::mcp::protocol::{
    CallToolResult, McpServerConfig, McpTool, McpTransportConfig, OAuthConfig,
};
use crate::mcp::transport::http_sse::HttpSseTransport;
use crate::mcp::transport::stdio::StdioTransport;
use crate::mcp::transport::streamable_http::StreamableHttpTransport;
use crate::mcp::transport::McpTransport;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::timeout;

pub use crate::mcp::result::tool_result_to_string;

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
/// Builds the sampling handler a connected server's client will be
/// armed with (server name in). The host that sets the factory owns the
/// composition — the product wiring is `Bounded(ApprovalGated(Llm))`
/// with the consent bridge inside the bound (SEC-SAMPLE1); a host that
/// sets no factory keeps the fail-closed `-32601` refusal for
/// server-initiated sampling.
pub type SamplingHandlerFactory = std::sync::Arc<
    dyn Fn(&str) -> std::sync::Arc<dyn crate::mcp::transport::SamplingHandler> + Send + Sync,
>;

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
    /// Host-provided per-server sampling handler factory; see
    /// [`SamplingHandlerFactory`].
    sampling_factory: RwLock<Option<SamplingHandlerFactory>>,
}

impl McpManager {
    /// Create a new MCP manager
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            configs: RwLock::new(HashMap::new()),
            connect_errors: RwLock::new(HashMap::new()),
            last_used_at_ms: RwLock::new(HashMap::new()),
            sampling_factory: RwLock::new(None),
        }
    }

    /// Arm server-initiated sampling for every client this manager
    /// connects: the factory is consulted once per server at client
    /// construction, BEFORE the initialize handshake (installing is the
    /// capability declaration). Not setting a factory keeps sampling
    /// refused (`-32601`), fail closed.
    pub async fn set_sampling_handler_factory(&self, factory: SamplingHandlerFactory) {
        *self.sampling_factory.write().await = Some(factory);
    }

    /// Whether a sampling factory is armed (host introspection; the
    /// unarmed default refuses sampling).
    pub async fn has_sampling_handler_factory(&self) -> bool {
        self.sampling_factory.read().await.is_some()
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

    /// Connect with a wall-clock budget.
    ///
    /// Outer `tokio::time::timeout` around [`Self::connect`] alone is not enough:
    /// when the budget elapses the connect future is dropped before it can
    /// record the connect error, leaving status as "enabled, disconnected, no
    /// error". Record the timeout here so hosts and settings UIs stay truthful.
    pub async fn connect_with_timeout(&self, name: &str, budget: Duration) -> Result<()> {
        match timeout(budget, self.connect(name)).await {
            Ok(result) => result,
            Err(_) => {
                let message = format!("MCP connect timed out after {}s", budget.as_secs().max(1));
                self.connect_errors
                    .write()
                    .await
                    .insert(name.to_string(), message.clone());
                Err(anyhow!(message))
            }
        }
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

        let sampling = self.sampling_factory.read().await.clone();
        let (client, tools) = connect_ready_client(&config, sampling.as_ref()).await?;
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
    /// constructing the `mcp__<server>__<tool>` prefix (e.g. via
    /// [`create_mcp_tools`](crate::mcp::create_mcp_tools)).
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
        let servers = self
            .configs
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let (server_name, tool_name) = Self::resolve_published_name(full_name, &servers)?;
        self.call_server_tool(&server_name, &tool_name, arguments)
            .await
    }

    /// Call a tool on the server identity already bound to a published wrapper.
    ///
    /// This does not reparse `mcp__<server>__<tool>`. A wrapper that stored
    /// `git__hub` must not be routed to a different registered server named `git`.
    pub async fn call_server_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<CallToolResult> {
        let client = {
            let clients = self.clients.read().await;
            clients
                .get(server_name)
                .cloned()
                .ok_or_else(|| anyhow!("MCP server not connected: {}", server_name))?
        };

        // Refresh the activity timestamp before the await so an idle
        // sweep running concurrently sees this server as recently used.
        self.last_used_at_ms
            .write()
            .await
            .insert(server_name.to_string(), now_epoch_ms());

        // Call tool
        client.call_tool(tool_name, arguments).await
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
    fn resolve_published_name(full_name: &str, servers: &[String]) -> Result<(String, String)> {
        if !full_name.starts_with("mcp__") {
            return Err(anyhow!("Invalid MCP tool name: {full_name}"));
        }
        let rest = &full_name["mcp__".len()..];
        let matches = servers
            .iter()
            .map(String::as_str)
            .filter(|server| {
                !server.is_empty()
                    && rest
                        .strip_prefix(*server)
                        .is_some_and(|tail| tail.starts_with("__") && tail.len() > 2)
            })
            .collect::<Vec<_>>();
        match matches.len() {
            0 => Self::parse_tool_name(full_name),
            1 => Ok((
                matches[0].to_string(),
                rest[matches[0].len() + 2..].to_string(),
            )),
            _ => Err(anyhow!(
                "MCP tool name '{full_name}' is ambiguous among registered servers"
            )),
        }
    }

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

/// Establish one exact initialized client and discover its initial tool set.
///
/// The returned client is not inserted into a mutable manager. Capability
/// projection adapters use this seam to pair the client with a frozen
/// [`crate::mcp::McpBinding`], while compatibility callers publish it through
/// [`McpManager`] after the same readiness barrier.
/// Install the factory-provided sampling handler BEFORE initialize:
/// installing is the `sampling` capability declaration in the handshake
/// (SEC-SAMPLE1 — this manager site is the sanctioned install point the
/// landing guard's API_HOME records). No factory, no capability: the
/// server's sampling requests keep the fail-closed `-32601` refusal.
fn arm_client_sampling(
    client: &McpClient,
    config: &McpServerConfig,
    sampling: Option<&SamplingHandlerFactory>,
) {
    if let Some(factory) = sampling {
        client.install_sampling_handler(factory(&config.name));
    }
}

pub(crate) async fn connect_ready_client(
    config: &McpServerConfig,
    sampling: Option<&SamplingHandlerFactory>,
) -> Result<(Arc<McpClient>, Vec<McpTool>)> {
    connect_ready_client_retained(config, None, None, sampling).await
}

pub(crate) async fn connect_ready_client_retained(
    config: &McpServerConfig,
    retention: Option<Arc<dyn crate::capability::RetainedUseGeneration>>,
    working_directory: Option<&std::path::Path>,
    sampling: Option<&SamplingHandlerFactory>,
) -> Result<(Arc<McpClient>, Vec<McpTool>)> {
    if !config.enabled {
        return Err(anyhow!("MCP server is disabled: {}", config.name));
    }

    let auth_header = McpManager::resolve_auth_header(config.oauth.as_ref()).await?;
    let transport: Arc<dyn McpTransport> = match &config.transport {
        McpTransportConfig::Stdio { command, args } => Arc::new(
            StdioTransport::spawn_with_timeout_and_retention(
                command,
                args,
                &config.env,
                config.tool_timeout_secs,
                retention,
                working_directory,
            )
            .await?,
        ),
        McpTransportConfig::Http { url, headers } => {
            let mut merged = headers.clone();
            if let Some((key, value)) = &auth_header {
                merged.insert(key.clone(), value.clone());
            }
            Arc::new(
                HttpSseTransport::connect_with_timeout(url, merged, config.tool_timeout_secs)
                    .await?,
            )
        }
        McpTransportConfig::StreamableHttp { url, headers } => {
            let mut merged = headers.clone();
            if let Some((key, value)) = &auth_header {
                merged.insert(key.clone(), value.clone());
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

    let client = Arc::new(McpClient::new(config.name.clone(), transport));
    arm_client_sampling(&client, config, sampling);
    if let Err(error) = client.initialize().await {
        if let Err(close_error) = client.close().await {
            tracing::warn!(
                server = %config.name,
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
                    server = %config.name,
                    error = %close_error,
                    "Failed to close MCP transport after tool discovery failure"
                );
            }
            return Err(error);
        }
    };
    // Keep the tools cache current: a server that adds or removes tools
    // after connect re-lists on `tools/list_changed` instead of staying
    // invisible until the next reconnect.
    client.spawn_list_changed_refresher();
    Ok((client, tools))
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod sampling_arm_tests {
    use super::*;

    #[test]
    fn sampling_factory_arms_the_client_capability_declaration() {
        use std::sync::Arc;

        struct CannedSampling;

        #[async_trait::async_trait]
        impl crate::mcp::transport::SamplingHandler for CannedSampling {
            async fn create_message(
                &self,
                _params: serde_json::Value,
            ) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({ "role": "assistant" }))
            }
        }

        let config = crate::mcp::McpServerConfig {
            name: "probe".into(),
            transport: crate::mcp::McpTransportConfig::Stdio {
                command: "true".into(),
                args: Vec::new(),
            },
            enabled: true,
            env: Default::default(),
            oauth: None,
            tool_timeout_secs: 60,
        };
        // The arm step runs before any I/O; a client with no transport
        // wired is enough to pin the capability declaration. A unit
        // transport that answers nothing keeps this test hermetic.
        struct UnwiredTransport;

        #[async_trait::async_trait]
        impl crate::mcp::transport::McpTransport for UnwiredTransport {
            async fn request(
                &self,
                _request: crate::mcp::protocol::JsonRpcRequest,
            ) -> anyhow::Result<crate::mcp::protocol::JsonRpcResponse> {
                Err(anyhow::anyhow!("unwired"))
            }

            async fn notify(
                &self,
                _notification: crate::mcp::protocol::JsonRpcNotification,
            ) -> anyhow::Result<()> {
                Err(anyhow::anyhow!("unwired"))
            }

            fn notifications(&self) -> tokio::sync::mpsc::Receiver<crate::mcp::McpNotification> {
                tokio::sync::mpsc::channel(1).1
            }

            async fn close(&self) -> anyhow::Result<()> {
                Ok(())
            }

            fn is_connected(&self) -> bool {
                false
            }

            fn set_sampling_handler(
                &self,
                _handler: std::sync::Arc<dyn crate::mcp::transport::SamplingHandler>,
            ) {
            }
        }

        let client = McpClient::new("probe".into(), std::sync::Arc::new(UnwiredTransport));
        // No factory: the capability stays undeclared (fail closed).
        arm_client_sampling(&client, &config, None);
        assert!(!client
            .sampling_declared
            .load(std::sync::atomic::Ordering::SeqCst));
        // With the factory: installing IS the declaration.
        let factory: SamplingHandlerFactory = Arc::new(|_server| Arc::new(CannedSampling));
        arm_client_sampling(&client, &config, Some(&factory));
        assert!(client
            .sampling_declared
            .load(std::sync::atomic::Ordering::SeqCst));
    }
}
