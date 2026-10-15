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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// MCP client for communicating with MCP servers
use async_trait::async_trait;

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
    /// Crate-visible: the manager seam's tests pin the capability
    /// declaration without speaking a full handshake.
    pub(crate) sampling_declared: std::sync::atomic::AtomicBool,
    /// Initialized flag
    initialized: AtomicBool,
    /// Cache refresh subscribes once without taking the host notification FIFO.
    list_refresher_started: AtomicBool,
    cache_refresh_shutdown: tokio_util::sync::CancellationToken,
}

impl McpClient {
    pub(crate) fn start_close(&self) {
        self.initialized.store(false, Ordering::SeqCst);
        self.cache_refresh_shutdown.cancel();
        self.transport.start_close();
    }
    /// Create a new MCP client with the given transport
    pub fn new(name: String, transport: Arc<dyn McpTransport>) -> Self {
        Self {
            name,
            transport,
            capabilities: RwLock::new(ServerCapabilities::default()),
            tools: RwLock::new(Vec::new()),
            resources: RwLock::new(Vec::new()),
            request_id: AtomicU64::new(1),
            sampling_declared: std::sync::atomic::AtomicBool::new(false),
            initialized: AtomicBool::new(false),
            list_refresher_started: AtomicBool::new(false),
            cache_refresh_shutdown: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Get next request ID
    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Initialize the MCP connection
    /// Install the sampling handler BEFORE initialize: installing
    /// declares the `sampling` capability in the handshake and answers
    /// `sampling/createMessage` with the host's model. The handler is the
    /// approval point — hosts that do not install keep the -32601 refusal.
    pub fn install_sampling_handler(
        &self,
        handler: Arc<dyn crate::mcp::transport::SamplingHandler>,
    ) {
        self.transport.set_sampling_handler(handler);
        self.sampling_declared
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub async fn initialize(&self) -> Result<InitializeResult> {
        let capabilities = if self
            .sampling_declared
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            ClientCapabilities {
                sampling: Some(crate::mcp::protocol::SamplingCapability {}),
                ..Default::default()
            }
        } else {
            ClientCapabilities::default()
        };
        let params = InitializeParams {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities,
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
        self.initialized.store(true, Ordering::Release);

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
        self.initialized.load(Ordering::Acquire)
    }

    /// Return whether this exact client completed MCP initialization and its
    /// transport is still connected.
    ///
    /// This synchronous readiness check is used while freezing a capability
    /// Run. It never performs discovery or reconnects through mutable manager
    /// state.
    pub fn is_ready(&self) -> bool {
        self.initialized.load(Ordering::Acquire) && self.transport.is_connected()
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

    /// Keep the tools cache current across `tools/list_changed`
    /// notifications from the server.
    ///
    /// A server that adds or removes tools after connect would otherwise
    /// stay invisible until a full reconnect: the transports parse the
    /// notification, but nothing re-listed the tools. Built-in transports
    /// expose a separate coalesced change signal, preserving host progress
    /// notifications. The task holds no client while idle and ends when its
    /// actual transport reader closes or the exact client closes/drops.
    /// Custom transports without that signal
    /// retain the legacy one-consumer notification behavior.
    pub fn spawn_list_changed_refresher(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        if self.list_refresher_started.swap(true, Ordering::AcqRel) {
            return tokio::spawn(async {});
        }
        let mut changes = self.transport.tool_list_changes();
        let mut notifications = changes.is_none().then(|| self.notifications());
        let client = Arc::downgrade(self);
        let shutdown = self.cache_refresh_shutdown.clone();
        tokio::spawn(async move {
            loop {
                let changed = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    changed = async {
                        match (&mut changes, &mut notifications) {
                            (Some(changes), _) => changes.changed().await.is_ok(),
                            (_, Some(notifications)) => loop {
                                match notifications.recv().await {
                                    Some(McpNotification::ToolsListChanged) => break true,
                                    Some(_) => {}
                                    None => break false,
                                }
                            },
                            _ => false,
                        }
                    } => changed,
                };
                if !changed || shutdown.is_cancelled() {
                    break;
                }
                let Some(client) = client.upgrade() else {
                    break;
                };
                match client.list_tools().await {
                    Ok(tools) => {
                        tracing::debug!(
                            server = %client.name,
                            count = tools.len(),
                            "MCP tools refreshed after list_changed"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            server = %client.name,
                            %error,
                            "MCP tools refresh after list_changed failed; cache unchanged"
                        );
                    }
                }
            }
        })
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

    /// Get the host notification receiver. Built-in cache refresh does not
    /// claim this FIFO; a host should drain it after taking ownership.
    pub fn notifications(&self) -> tokio::sync::mpsc::Receiver<McpNotification> {
        self.transport.notifications()
    }

    /// Close the client
    pub async fn close(&self) -> Result<()> {
        self.start_close();
        self.transport.close().await
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.transport.is_connected()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Revoke idle refresh even when another host still owns the transport.
        // The transport remains responsible for its actual request settlement.
        self.cache_refresh_shutdown.cancel();
    }
}

/// Per-sample host approval (the Claude-Code-parity pattern): each
/// server-initiated sampling request is shown to the host's approver
/// first; only an explicit approve delegates to the inner handler. A
/// denial surfaces as a JSON-RPC error the server can see — sampling is
/// refused for THIS request, not silently dropped.
#[async_trait]
pub trait SamplingApprover: Send + Sync {
    async fn approve_sampling(&self, server: &str, prompt_excerpt: &str) -> bool;
}

pub struct ApprovalGatedSampling {
    server: String,
    inner: std::sync::Arc<dyn crate::mcp::transport::SamplingHandler>,
    approver: std::sync::Arc<dyn SamplingApprover>,
}

impl ApprovalGatedSampling {
    pub fn new(
        server: impl Into<String>,
        inner: std::sync::Arc<dyn crate::mcp::transport::SamplingHandler>,
        approver: std::sync::Arc<dyn SamplingApprover>,
    ) -> Self {
        Self {
            server: server.into(),
            inner,
            approver,
        }
    }
}

#[async_trait]
impl crate::mcp::transport::SamplingHandler for ApprovalGatedSampling {
    async fn create_message(&self, params: serde_json::Value) -> Result<serde_json::Value> {
        let excerpt = params
            .get("messages")
            .and_then(|messages| messages.as_array())
            .and_then(|messages| messages.first())
            .and_then(|message| message.get("content"))
            .and_then(|content| content.get("text"))
            .and_then(|text| text.as_str())
            .unwrap_or_default()
            .chars()
            .take(160)
            .collect::<String>();
        anyhow::ensure!(
            self.approver.approve_sampling(&self.server, &excerpt).await,
            "sampling request denied by the host"
        );
        self.inner.create_message(params).await
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

/// The product sampling bridge: translates the MCP
/// `sampling/createMessage` payload into one call on the host's model
/// client and returns the CreateMessageResult object. Installing this
/// handler is the host's approval of sampling for that server — the
/// model sees exactly the messages the server supplied, nothing else.
/// A per-server ceiling on sampled generations: a compromised MCP server
/// must not turn the host's model into an unbounded oracle
/// (SEC-SAMPLE1 precondition 3). A count bound over the server's
/// lifetime also bounds any burst, so a separate rate window adds
/// nothing. Wrap OUTSIDE the approval gate so even approved requests
/// draw from the same ceiling.
pub struct BoundedSamplingHandler {
    inner: std::sync::Arc<dyn crate::mcp::transport::SamplingHandler>,
    ceiling: usize,
    used: std::sync::atomic::AtomicUsize,
}

impl BoundedSamplingHandler {
    /// The default per-server ceiling for sampled generations.
    pub const DEFAULT_CEILING: usize = 64;

    pub fn new(inner: std::sync::Arc<dyn crate::mcp::transport::SamplingHandler>) -> Self {
        Self {
            inner,
            ceiling: Self::DEFAULT_CEILING,
            used: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Ceiling override for tests and explicit host configuration; the
    /// product wiring uses the default.
    pub fn with_ceiling(mut self, ceiling: usize) -> Self {
        self.ceiling = ceiling;
        self
    }
}

#[async_trait]
impl crate::mcp::transport::SamplingHandler for BoundedSamplingHandler {
    async fn create_message(&self, params: serde_json::Value) -> Result<serde_json::Value> {
        let used = self.used.fetch_add(1, Ordering::AcqRel);
        anyhow::ensure!(
            used < self.ceiling,
            "this MCP server has exhausted its {} sampled generations; \
             refusing further server-initiated sampling",
            self.ceiling
        );
        self.inner.create_message(params).await
    }
}

pub struct LlmSamplingHandler {
    client: std::sync::Arc<dyn crate::llm::LlmClient>,
    model: String,
}

impl LlmSamplingHandler {
    pub fn new(client: std::sync::Arc<dyn crate::llm::LlmClient>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl crate::mcp::transport::SamplingHandler for LlmSamplingHandler {
    async fn create_message(&self, params: serde_json::Value) -> Result<serde_json::Value> {
        let system = params.get("systemPrompt").and_then(|value| value.as_str());
        let max_tokens = params
            .get("maxTokens")
            .and_then(|value| value.as_u64())
            .unwrap_or(1024)
            .min(8192) as usize;
        // Flatten the wire messages into plain text turns; the host model
        // answers as the assistant.
        let mut prompt = String::new();
        if let Some(messages) = params.get("messages").and_then(|value| value.as_array()) {
            for message in messages {
                let role = message
                    .get("role")
                    .and_then(|r| r.as_str())
                    .unwrap_or("user");
                let text = message
                    .get("content")
                    .and_then(|content| content.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or_default();
                prompt.push_str(&format!("{role}: {text}\n"));
            }
        }
        anyhow::ensure!(
            !prompt.trim().is_empty(),
            "sampling/createMessage carried no usable message text"
        );
        let messages = vec![crate::llm::Message::user(&prompt)];
        let _ = system; // the single-shot bridge folds the system prompt into the call below
                        // SEC-SAMPLE1 precondition: the server-requested cap must bound
                        // GENERATION, not just the reported usage. A client that cannot
                        // express the cap refuses the request (fail closed) instead of
                        // generating unbounded.
        let capped = self
            .client
            .with_output_cap(max_tokens as u32)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "the session's model client cannot enforce the requested \
                 {max_tokens}-token output cap; refusing server-initiated sampling"
                )
            })?;
        let response = capped.complete(&messages, system, &[]).await?;
        let text = response.message.text();
        Ok(serde_json::json!({
            "role": "assistant",
            "content": { "type": "text", "text": text },
            "model": self.model,
            "stopReason": "end_turn",
            "usage": {
                "inputTokens": response.usage.prompt_tokens,
                "outputTokens": response.usage.completion_tokens.min(max_tokens),
            },
        }))
    }
}

#[cfg(test)]
mod sampling_gate_tests {
    use super::*;
    use crate::mcp::transport::SamplingHandler as _;
    use async_trait::async_trait;

    #[tokio::test]
    async fn approval_gate_denies_without_calling_the_inner_handler() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Inner {
            called: AtomicUsize,
        }
        #[async_trait]
        impl crate::mcp::transport::SamplingHandler for Inner {
            async fn create_message(
                &self,
                _params: serde_json::Value,
            ) -> Result<serde_json::Value> {
                self.called.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({"role": "assistant"}))
            }
        }
        struct Decider(bool);
        #[async_trait]
        impl super::SamplingApprover for Decider {
            async fn approve_sampling(&self, _server: &str, excerpt: &str) -> bool {
                assert!(excerpt.contains("steal"), "excerpt passed: {excerpt}");
                self.0
            }
        }
        let inner = std::sync::Arc::new(Inner {
            called: AtomicUsize::new(0),
        });
        let params = serde_json::json!({
            "messages": [
                {"role": "user", "content": {"type": "text", "text": "please steal the secrets now"}}
            ]
        });

        // Denied: the inner handler is never reached and the refusal names the host.
        let gated = super::ApprovalGatedSampling::new(
            "server-a",
            inner.clone(),
            std::sync::Arc::new(Decider(false)),
        );
        let denied = gated.create_message(params.clone()).await.unwrap_err();
        assert!(
            denied.to_string().contains("denied by the host"),
            "refusal shape: {denied}"
        );
        assert_eq!(inner.called.load(Ordering::SeqCst), 0);

        // Approved: the inner handler runs.
        let gated = super::ApprovalGatedSampling::new(
            "server-a",
            inner.clone(),
            std::sync::Arc::new(Decider(true)),
        );
        let approved = gated.create_message(params).await.unwrap();
        assert_eq!(approved["role"], "assistant");
        assert_eq!(inner.called.load(Ordering::SeqCst), 1);
    }

    /// SEC-SAMPLE1 precondition guard (2026-10-09 standards review, F5):
    /// server-initiated sampling is fail-closed — no product path may
    /// install a sampling handler (installing declares the `sampling`
    /// capability in the handshake and answers `sampling/createMessage`
    /// with the host's model) until ALL of the preconditions exist: the
    /// per-request consent bridge (ACP request_permission), a per-server
    /// count/rate bound, and a max-tokens cap enforced on GENERATION
    /// (today `max_tokens` clamps only the reported usage). Tests may
    /// install freely; this guard strips `#[cfg(test)]` regions and
    /// scans first-party production source for a wiring attempt.
    #[test]
    fn sampling_installation_stays_unwired_until_its_preconditions_land() {
        use std::path::{Path, PathBuf};

        /// The only production file that may mention the installer: its
        /// definition. Adding a second entry means product wiring — do it
        /// together with the three preconditions above, never before.
        /// The installer's definition (client.rs) and the one sanctioned
        /// product install site (manager.rs's factory seam — landed with
        /// all three SEC-SAMPLE1 preconditions). Adding another entry
        /// means new product wiring — do it with the preconditions
        /// shown, never before.
        const API_HOME: &[&str] = &["core/src/mcp/client.rs", "core/src/mcp/manager.rs"];

        fn strip_cfg_test_regions(source: &str) -> String {
            // Line-structural: rustfmt pins a top-level module's closing
            // brace to column 0, so an inline `#[cfg(...test...)] mod x {`
            // region ends at the next column-0 `}` line — braces inside
            // string literals in the test body cannot break the match the
            // way a naive depth counter's can. External test modules
            // (`mod x;`) and non-test attributes pass through untouched;
            // those files are separately filtered by name below.
            let mut out = Vec::new();
            let lines: Vec<&str> = source.lines().collect();
            let mut index = 0usize;
            while index < lines.len() {
                let line = lines[index].trim_start();
                let is_test_gate = line.starts_with("#[cfg(")
                    && line.contains("test")
                    && !line.contains("cfg(not(");
                if is_test_gate {
                    // Collect the attribute run, then decide by the mod shape.
                    let mut cursor = index + 1;
                    while cursor < lines.len() && lines[cursor].trim_start().starts_with("#[") {
                        cursor += 1;
                    }
                    let declaration = lines.get(cursor).map(|l| l.trim_start()).unwrap_or("");
                    if declaration.starts_with("mod ") && declaration.ends_with(";") {
                        // External test module declaration: keep it (its
                        // file is filtered by name), skip the attributes.
                        out.extend_from_slice(&lines[index..=cursor]);
                        index = cursor + 1;
                        continue;
                    }
                    if declaration.starts_with("mod ") && declaration.ends_with("{") {
                        // Inline test module: drop through its column-0 `}`.
                        let mut close = cursor + 1;
                        while close < lines.len() && lines[close] != "}" {
                            close += 1;
                        }
                        index = (close + 1).min(lines.len());
                        continue;
                    }
                }
                out.push(lines[index]);
                index += 1;
            }
            out.join("\n")
        }

        fn collect_rust_files(root: &Path, out: &mut Vec<PathBuf>) {
            let entries = match std::fs::read_dir(root) {
                Ok(entries) => entries,
                Err(_) => return,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "vendor" || name == "target" {
                    continue;
                }
                if path.is_dir() {
                    collect_rust_files(&path, out);
                } else if name.ends_with(".rs") {
                    out.push(path);
                }
            }
        }

        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut offenders = Vec::new();
        for tree in ["core/src", "acp/src", "tui/src", "kernel/src"] {
            let mut files = Vec::new();
            collect_rust_files(&workspace.join(tree), &mut files);
            for file in files {
                let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.ends_with("_tests.rs")
                    || name == "tests.rs"
                    || file.to_string_lossy().contains("/tests/")
                {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&file) else {
                    continue;
                };
                if source.trim_start().starts_with("#![cfg(test)]") {
                    continue;
                }
                let stripped = strip_cfg_test_regions(&source);
                if stripped.contains("install_sampling_handler") {
                    let relative = file
                        .strip_prefix(workspace)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| file.to_string_lossy().into_owned());
                    if !API_HOME.contains(&relative.as_str()) {
                        offenders.push(relative);
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "product wiring of server-initiated sampling requires the SEC-SAMPLE1 \
             preconditions (per-request consent bridge, per-server count/rate bound, \
             generation-enforced max_tokens): {offenders:?}"
        );
    }

    struct UncappedScriptedClient;

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for UncappedScriptedClient {
        async fn complete(
            &self,
            _messages: &[crate::llm::Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
        ) -> anyhow::Result<crate::llm::LlmResponse> {
            unreachable!("the refusal must land before any generation")
        }

        async fn complete_streaming(
            &self,
            _messages: &[crate::llm::Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
            unreachable!("the refusal must land before any generation")
        }
    }

    #[tokio::test]
    async fn sampling_refuses_when_the_client_cannot_enforce_the_cap() {
        use crate::mcp::transport::SamplingHandler;
        // A client without a generation-enforced output cap must REFUSE
        // server-initiated sampling (fail closed) — unbounded generation
        // is not a substitute for the requested cap.
        let handler = super::LlmSamplingHandler::new(
            std::sync::Arc::new(UncappedScriptedClient),
            "mock/probe".to_string(),
        );
        let error = handler
            .create_message(serde_json::json!({
                "maxTokens": 512,
                "messages": [
                    { "role": "user", "content": { "type": "text", "text": "probe" } }
                ]
            }))
            .await
            .expect_err("an uncapped client must refuse sampling");
        let message = error.to_string();
        assert!(
            message.contains("512") && message.contains("output cap"),
            "the refusal names the cap it cannot enforce: {message}"
        );
    }

    /// Records the cap the capped view was built with, then answers.
    struct CappedScriptedClient {
        seen_cap: std::sync::Arc<std::sync::Mutex<Option<u32>>>,
    }

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for CappedScriptedClient {
        fn with_output_cap(
            &self,
            max_tokens: u32,
        ) -> Option<std::sync::Arc<dyn crate::llm::LlmClient>> {
            *self.seen_cap.lock().unwrap() = Some(max_tokens);
            Some(std::sync::Arc::new(CappedScriptedClient {
                seen_cap: std::sync::Arc::clone(&self.seen_cap),
            }))
        }

        async fn complete(
            &self,
            _messages: &[crate::llm::Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
        ) -> anyhow::Result<crate::llm::LlmResponse> {
            Ok(crate::llm::LlmResponse {
                message: crate::llm::Message::assistant("capped-answer"),
                usage: Default::default(),
                stop_reason: None,
                meta: Default::default(),
                token_logprobs: Vec::new(),
            })
        }

        async fn complete_streaming(
            &self,
            _messages: &[crate::llm::Message],
            _system: Option<&str>,
            _tools: &[crate::llm::ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
            unreachable!("streaming is not part of this pin")
        }
    }

    #[tokio::test]
    async fn sampling_routes_the_requested_cap_into_generation() {
        use crate::mcp::transport::SamplingHandler;
        // The server-requested maxTokens must reach GENERATION (the
        // capped view), not merely clamp the reported usage.
        let seen_cap = std::sync::Arc::new(std::sync::Mutex::new(None));
        let handler = super::LlmSamplingHandler::new(
            std::sync::Arc::new(CappedScriptedClient {
                seen_cap: std::sync::Arc::clone(&seen_cap),
            }),
            "mock/probe".to_string(),
        );
        let answer = handler
            .create_message(serde_json::json!({
                "maxTokens": 777,
                "messages": [
                    { "role": "user", "content": { "type": "text", "text": "probe" } }
                ]
            }))
            .await
            .expect("the capped client answers");
        assert_eq!(answer["content"]["text"], "capped-answer");
        assert_eq!(
            *seen_cap.lock().unwrap(),
            Some(777),
            "the exact requested cap reaches the generation view"
        );
    }

    struct CountingSampling {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl crate::mcp::transport::SamplingHandler for CountingSampling {
        async fn create_message(&self, _params: serde_json::Value) -> Result<serde_json::Value> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(serde_json::json!({ "role": "assistant" }))
        }
    }

    #[tokio::test]
    async fn sampling_calls_beyond_the_per_server_ceiling_are_refused() {
        use crate::mcp::transport::SamplingHandler;
        // SEC-SAMPLE1 precondition 3: the count bound caps a server's
        // total sampled generations — approved or not — and the refusal
        // names the ceiling it exhausted.
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handler = super::BoundedSamplingHandler::new(std::sync::Arc::new(CountingSampling {
            calls: std::sync::Arc::clone(&calls),
        }))
        .with_ceiling(2);
        let params = serde_json::json!({
            "messages": [
                { "role": "user", "content": { "type": "text", "text": "probe" } }
            ]
        });
        handler
            .create_message(params.clone())
            .await
            .expect("first call is within the ceiling");
        handler
            .create_message(params.clone())
            .await
            .expect("second call is within the ceiling");
        let error = handler
            .create_message(params)
            .await
            .expect_err("the third call must refuse");
        let message = error.to_string();
        assert!(
            message.contains("2 sampled generations"),
            "the refusal names the exhausted ceiling: {message}"
        );
        assert_eq!(
            calls.load(Ordering::Acquire),
            2,
            "the inner ran exactly twice"
        );
    }

    #[tokio::test]
    async fn sampling_ceilings_are_per_server() {
        use crate::mcp::transport::SamplingHandler;
        // Each server's handler carries its own ceiling — one server
        // exhausting its budget never silences another.
        let first = super::BoundedSamplingHandler::new(std::sync::Arc::new(CountingSampling {
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }))
        .with_ceiling(1);
        let second = super::BoundedSamplingHandler::new(std::sync::Arc::new(CountingSampling {
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }))
        .with_ceiling(1);
        let params = serde_json::json!({
            "messages": [
                { "role": "user", "content": { "type": "text", "text": "probe" } }
            ]
        });
        first
            .create_message(params.clone())
            .await
            .expect("first server's budget");
        assert!(
            first.create_message(params.clone()).await.is_err(),
            "first server exhausted"
        );
        second
            .create_message(params)
            .await
            .expect("the second server keeps its own budget");
    }
}
