//! Node Session capability methods.

use super::session::Session;
use super::*;

#[napi]
impl Session {
    // ========================================================================
    // Skill API
    // ========================================================================

    /// Add or replace a Skill in this live session.
    ///
    /// The Skill tools and model-visible catalog observe the new definition
    /// immediately. Removing it restores the exact session Skill it shadowed.
    #[napi]
    pub fn add_skill(&self, skill: InlineSkill) -> napi::Result<()> {
        self.inner
            .add_skill(inline_skill_to_rust(skill)?)
            .map_err(node_code_error)
    }

    /// Remove a Skill installed through `addSkill`.
    #[napi]
    pub fn remove_skill(&self, name: String) -> napi::Result<()> {
        self.inner.remove_skill(&name).map_err(node_code_error)
    }

    /// Return the names in the session's current live Skill registry.
    #[napi]
    pub fn skill_names(&self) -> Vec<String> {
        self.inner.skill_names()
    }

    // ========================================================================
    // MCP API
    // ========================================================================

    /// Add an MCP server to this live session.
    ///
    /// Connects the server and registers all its tools immediately so the agent
    /// can call them. Tool names follow the convention `mcp__<name>__<tool>`.
    ///
    /// @param name - Server identifier (used as prefix in tool names)
    /// @param transport - Transport type: `"stdio"` (default), `"http"`, or `"streamable-http"`
    /// @param command - Executable to launch (stdio only, e.g. `"npx"`)
    /// @param args - Arguments for the command (stdio only)
    /// @param url - Server URL (http / streamable-http only)
    /// @param headers - HTTP headers (http / streamable-http only)
    /// @param env - Optional extra environment variables (stdio only)
    /// @returns Number of tools registered from the server
    #[allow(clippy::too_many_arguments)]
    #[napi]
    pub async fn add_mcp_server(
        &self,
        name: String,
        #[napi(ts_arg_type = "'stdio' | 'http' | 'streamable-http'")] transport: Option<String>,
        command: Option<String>,
        args: Option<Vec<String>>,
        url: Option<String>,
        headers: Option<std::collections::HashMap<String, String>>,
        env: Option<std::collections::HashMap<String, String>>,
        timeout_ms: Option<u32>,
    ) -> napi::Result<u32> {
        use a3s_code_core::mcp::protocol::{McpServerConfig, McpTransportConfig};

        let transport_str = transport.as_deref().unwrap_or("stdio");
        let transport_config = match transport_str {
            "stdio" => {
                let command = command.ok_or_else(|| {
                    napi::Error::from_reason("'command' is required for stdio transport")
                })?;
                McpTransportConfig::Stdio {
                    command,
                    args: args.unwrap_or_default(),
                }
            }
            "http" => {
                let url = url.ok_or_else(|| {
                    napi::Error::from_reason("'url' is required for http transport")
                })?;
                McpTransportConfig::Http {
                    url,
                    headers: headers.unwrap_or_default(),
                }
            }
            "streamable-http" | "streamable_http" => {
                let url = url.ok_or_else(|| {
                    napi::Error::from_reason("'url' is required for streamable-http transport")
                })?;
                McpTransportConfig::StreamableHttp {
                    url,
                    headers: headers.unwrap_or_default(),
                }
            }
            other => {
                return Err(napi::Error::from_reason(format!(
                    "Unknown transport '{}'. Use 'stdio', 'http', or 'streamable-http'",
                    other
                )))
            }
        };

        let tool_timeout_secs = timeout_ms
            .map(|ms| timeout_ms_to_secs(ms as u64))
            .unwrap_or(60);
        let session = self.inner.clone();
        let count = session
            .add_mcp_server(McpServerConfig {
                name,
                transport: transport_config,
                enabled: true,
                env: env.unwrap_or_default(),
                oauth: None,
                tool_timeout_secs,
            })
            .await
            .map_err(node_code_error)?;
        Ok(count as u32)
    }

    /// Add an MCP server with a typed object config.
    ///
    /// Preferred over the positional overload for new SDK callers.
    ///
    /// ```js
    /// await session.addMcpServerConfig({
    ///   name: 'github',
    ///   transport: { type: 'stdio', command: 'npx', args: ['-y', '@modelcontextprotocol/server-github'] },
    ///   env: { GITHUB_TOKEN: process.env.GITHUB_TOKEN },
    ///   timeoutMs: 30000,
    /// })
    /// ```
    #[napi(
        js_name = "addMcpServerConfig",
        ts_args_type = "config: McpServerConfig"
    )]
    pub async fn add_mcp_server_config(&self, config: serde_json::Value) -> napi::Result<u32> {
        let config = normalize_mcp_server_config(config)?;
        let session = self.inner.clone();
        let count = session
            .add_mcp_server(config)
            .await
            .map_err(node_code_error)?;
        Ok(count as u32)
    }

    /// Add an MCP server with the compact object-shaped API.
    #[napi(js_name = "addMcp", ts_args_type = "config: McpServerConfig")]
    pub async fn add_mcp(&self, config: serde_json::Value) -> napi::Result<u32> {
        self.add_mcp_server_config(config).await
    }

    /// Dynamically register agent definitions from a directory into the live session.
    ///
    /// Scans the directory for `*.yaml`, `*.yml`, and `*.md` agent definition files
    /// and registers them into the shared AgentRegistry used by the `task` tool.
    /// New agents are immediately callable via `task({ agent: "…", … })` without
    /// restarting the session.
    ///
    /// @param path - Directory to scan for agent definition files
    /// @returns Number of agents successfully loaded
    #[napi]
    pub fn register_agent_dir(&self, path: String) -> napi::Result<u32> {
        let dir = std::path::PathBuf::from(&path);
        self.inner
            .register_agent_dir(&dir)
            .map(|count| count as u32)
            .map_err(node_code_error)
    }

    /// Register a disposable worker agent into the live session.
    ///
    /// The worker is immediately callable through the model-visible `task` tool.
    ///
    /// @param worker - Worker spec to register
    /// @returns Compiled agent definition
    #[napi]
    pub fn register_worker_agent(&self, worker: WorkerAgentSpec) -> napi::Result<AgentDefinition> {
        let worker = js_worker_agent_spec_to_rust(worker)?;
        let definition = self
            .inner
            .register_worker_agent(worker)
            .map_err(node_code_error)?;
        Ok(rust_agent_definition_to_js(definition))
    }

    /// Register many disposable workers into the live session.
    ///
    /// @param workers - Worker specs to register
    /// @returns Compiled agent definitions
    #[napi]
    pub fn register_worker_agents(
        &self,
        workers: Vec<WorkerAgentSpec>,
    ) -> napi::Result<Vec<AgentDefinition>> {
        let workers = workers
            .into_iter()
            .map(js_worker_agent_spec_to_rust)
            .collect::<napi::Result<Vec<_>>>()?;
        Ok(self
            .inner
            .register_worker_agents(workers)
            .map_err(node_code_error)?
            .into_iter()
            .map(rust_agent_definition_to_js)
            .collect())
    }

    /// Register the built-in A3S Flow-backed `dynamic_workflow` tool into this live session.
    ///
    /// The tool becomes visible in `toolNames()` immediately and can be invoked
    /// through the ordinary `tool("dynamic_workflow", ...)` direct-call path or
    /// selected by the model on subsequent runs.
    #[napi]
    pub fn register_dynamic_workflow_runtime(&self) -> napi::Result<()> {
        self.inner
            .register_dynamic_workflow_runtime()
            .map_err(node_code_error)
    }

    /// Remove a previously registered dynamic tool from this live session.
    ///
    /// This is primarily used to unregister host/runtime-added tools such as
    /// `dynamic_workflow` when a capability is disabled.
    #[napi]
    pub fn unregister_dynamic_tool(&self, name: String) -> napi::Result<()> {
        self.inner
            .unregister_dynamic_tool(&name)
            .map_err(node_code_error)
    }

    /// Disconnect and unregister an MCP server, removing its tools from the session.
    ///
    /// @param name - Server name (must match the name used in addMcpServer)
    #[napi]
    pub async fn remove_mcp_server(&self, name: String) -> napi::Result<()> {
        let session = self.inner.clone();
        session
            .remove_mcp_server(&name)
            .await
            .map_err(node_code_error)?;
        Ok(())
    }

    /// Remove an MCP server with the compact API.
    #[napi(js_name = "removeMcp")]
    pub async fn remove_mcp(&self, name: String) -> napi::Result<()> {
        self.remove_mcp_server(name).await
    }

    /// Return connection status for all MCP servers registered on this session.
    ///
    /// @returns Array of `{ name, connected, toolCount }` entries
    #[napi]
    pub async fn mcp_status(&self) -> napi::Result<Vec<McpServerStatusEntry>> {
        let session = self.inner.clone();
        let status = session.mcp_status().await;
        Ok(status
            .into_iter()
            .map(|(name, s)| McpServerStatusEntry {
                name,
                connected: s.connected,
                tool_count: s.tool_count as u32,
                error: s.error,
            })
            .collect())
    }

    /// Return MCP server status with the compact API.
    #[napi]
    pub async fn mcps(&self) -> napi::Result<Vec<McpServerStatusEntry>> {
        self.mcp_status().await
    }

    /// Return the names of all tools currently registered on this session.
    ///
    /// @returns Array of tool name strings
    #[napi]
    pub fn tool_names(&self) -> Vec<String> {
        self.inner.tool_names()
    }

    /// Return full model-visible tool definitions currently registered on this session.
    #[napi]
    pub fn tool_definitions(&self) -> napi::Result<serde_json::Value> {
        serde_json::to_value(self.inner.tool_definitions())
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return the exact live capability catalog generation and digest.
    #[napi]
    pub fn capability_catalog_stamp(&self) -> napi::Result<serde_json::Value> {
        let stamp = self.inner.capability_catalog_stamp();
        Ok(serde_json::json!({
            "generation": stamp.generation().get(),
            "digest": stamp.digest().to_string(),
        }))
    }

    /// Return the typed model-facing Tool presentation profile for this session.
    #[napi]
    pub fn tool_presentation_profile(&self) -> napi::Result<serde_json::Value> {
        serde_json::to_value(self.inner.tool_presentation_profile())
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Preview the model-facing Tool definitions for a prompt.
    #[napi]
    pub fn presented_tool_definitions(&self, prompt: String) -> napi::Result<serde_json::Value> {
        serde_json::to_value(self.inner.presented_tool_definitions(&prompt).map_err(|e| {
            napi::Error::from_reason(format!("Tool presentation error: {e}"))
        })?)
        .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Return the exact cognitive package binding, when one is installed.
    #[napi]
    pub fn current_cognitive_package_binding(&self) -> napi::Result<serde_json::Value> {
        serde_json::to_value(self.inner.current_cognitive_package_binding())
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Validate an exact persisted capability binding before recovery.
    #[napi]
    pub fn ensure_recovery_capability_binding(
        &self,
        binding: serde_json::Value,
    ) -> napi::Result<()> {
        let binding = serde_json::from_value(binding)
            .map_err(|e| napi::Error::from_reason(format!("Invalid capability binding: {e}")))?;
        self.inner
            .ensure_recovery_capability_binding(&binding)
            .map_err(|e| napi::Error::from_reason(format!("Recovery binding error: {e}")))
    }

    /// Drain retired host capability effects and return the cleanup report.
    #[napi]
    pub async fn drain_capability_cleanup(&self) -> napi::Result<serde_json::Value> {
        let report = self.inner.drain_capability_cleanup().await;
        Ok(serde_json::json!({
            "rollbackBatches": report.rollback_batches,
            "retiredBatches": report.retired_batches,
            "effectsClosed": report.effects_closed,
            "effectsFailed": report.effects_failed,
            "effectsTimedOut": report.effects_timed_out,
            "clean": report.is_clean(),
        }))
    }

    /// Return a stored tool artifact by URI, or null if it is not retained.
    #[napi]
    pub fn get_artifact(&self, artifact_uri: String) -> napi::Result<serde_json::Value> {
        serde_json::to_value(self.inner.get_artifact(&artifact_uri))
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }
}
