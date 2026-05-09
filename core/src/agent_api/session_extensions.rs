//! Live session extension runtime.
//!
//! A3S Code sessions can be extended after creation with agent definitions and
//! MCP servers. This module owns those dynamic capability mutations so the
//! facade does not need to know how registries, managers, and executors stay in
//! sync.

use super::AgentSession;
use crate::error::Result;
use crate::mcp::{McpServerConfig, McpServerStatus};
use crate::subagent::{AgentDefinition, WorkerAgentSpec};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub(super) struct SessionExtensionRuntime<'a> {
    session: &'a AgentSession,
}

impl<'a> SessionExtensionRuntime<'a> {
    pub(super) fn from_session(session: &'a AgentSession) -> Self {
        Self { session }
    }

    pub(super) fn register_agent_dir(&self, dir: &Path) -> usize {
        let agents = crate::subagent::load_agents_from_dir(dir);
        let count = agents.len();
        for agent in agents {
            tracing::info!(
                session_id = %self.session.session_id,
                agent = agent.name,
                dir = %dir.display(),
                "Dynamically registered agent"
            );
            self.session.agent_registry.register(agent);
        }
        count
    }

    pub(super) fn register_worker_agent(&self, spec: WorkerAgentSpec) -> AgentDefinition {
        let kind = spec.kind;
        let agent = self.session.agent_registry.register_worker(spec);
        tracing::info!(
            session_id = %self.session.session_id,
            agent = %agent.name,
            kind = ?kind,
            "Dynamically registered worker agent"
        );
        agent
    }

    pub(super) fn register_worker_agents<I>(&self, specs: I) -> Vec<AgentDefinition>
    where
        I: IntoIterator<Item = WorkerAgentSpec>,
    {
        specs
            .into_iter()
            .map(|spec| self.register_worker_agent(spec))
            .collect()
    }

    pub(super) async fn add_mcp_server(&self, config: McpServerConfig) -> Result<usize> {
        let server_name = config.name.clone();
        self.session.mcp_manager.register_server(config).await;
        self.session
            .mcp_manager
            .connect(&server_name)
            .await
            .map_err(|e| crate::error::CodeError::Tool {
                tool: server_name.clone(),
                message: format!("Failed to connect MCP server: {}", e),
            })?;

        let tools = self
            .session
            .mcp_manager
            .get_server_tools(&server_name)
            .await;
        let count = tools.len();

        for tool in crate::mcp::tools::create_mcp_tools(
            &server_name,
            tools,
            Arc::clone(&self.session.mcp_manager),
        ) {
            self.session.tool_executor.register_dynamic_tool(tool);
        }

        tracing::info!(
            session_id = %self.session.session_id,
            server = server_name,
            tools = count,
            "MCP server added to live session"
        );

        Ok(count)
    }

    pub(super) async fn remove_mcp_server(&self, server_name: &str) -> Result<()> {
        self.session
            .tool_executor
            .unregister_tools_by_prefix(&format!("mcp__{server_name}__"));
        self.session
            .mcp_manager
            .disconnect(server_name)
            .await
            .map_err(|e| crate::error::CodeError::Tool {
                tool: server_name.to_string(),
                message: format!("Failed to disconnect MCP server: {}", e),
            })?;
        tracing::info!(
            session_id = %self.session.session_id,
            server = server_name,
            "MCP server removed from live session"
        );
        Ok(())
    }

    pub(super) async fn mcp_status(&self) -> HashMap<String, McpServerStatus> {
        self.session.mcp_manager.get_status().await
    }
}
