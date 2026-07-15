//! Live session extension runtime.
//!
//! A3S Code sessions can be extended after creation with agent definitions and
//! MCP servers. This module owns those dynamic capability mutations so the
//! facade does not need to know how registries, managers, and executors stay in
//! sync.

use super::AgentSession;
use crate::error::Result;
use crate::mcp::{McpServerConfig, McpServerStatus};
use crate::skills::{Skill, SkillRegistry};
use crate::subagent::{AgentDefinition, WorkerAgentSpec};
use crate::tools::{Tool, ToolExecutor};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

struct OwnedMcpTool {
    name: String,
    installed: Arc<dyn Tool>,
    shadowed: Option<Arc<dyn Tool>>,
}

/// Exact dynamic-tool registrations owned by session-local MCP servers.
///
/// Pointer identity is the ownership token. Removal only restores a shadowed
/// registration while the same wrapper still owns the name, so a later host or
/// inherited registration is never removed accidentally.
#[derive(Default)]
pub(crate) struct SessionMcpToolOwnership {
    by_server: HashMap<String, Vec<OwnedMcpTool>>,
}

impl SessionMcpToolOwnership {
    pub(crate) fn install(
        &mut self,
        server_name: &str,
        executor: &ToolExecutor,
        tools: Vec<Arc<dyn Tool>>,
    ) -> usize {
        let mut owned = Vec::with_capacity(tools.len());
        for tool in tools {
            let name = tool.name().to_string();
            let installed = Arc::clone(&tool);
            let (accepted, shadowed) = executor.register_dynamic_tool_with_shadow(tool);
            if accepted {
                owned.push(OwnedMcpTool {
                    name,
                    installed,
                    shadowed,
                });
            }
        }
        let count = owned.len();
        self.by_server.insert(server_name.to_string(), owned);
        count
    }

    pub(crate) fn remove(&mut self, server_name: &str, executor: &ToolExecutor) -> bool {
        let Some(mut owned) = self.by_server.remove(server_name) else {
            return false;
        };

        // Repeated names are unwound in reverse registration order so the
        // original shadow is restored even if a server returned duplicates.
        while let Some(tool) = owned.pop() {
            if executor.restore_dynamic_tool_if_same(
                &tool.name,
                &tool.installed,
                tool.shadowed.clone(),
            ) {
                continue;
            }

            // Another session-local server may currently shadow this wrapper
            // under the same fully-qualified name (server/tool delimiter
            // combinations are not injective). Splice the removed owner out
            // of that tracked shadow chain so removing the later server cannot
            // resurrect this one.
            for remaining in self.by_server.values_mut().flatten() {
                let shadows_removed = remaining
                    .shadowed
                    .as_ref()
                    .is_some_and(|shadowed| Arc::ptr_eq(shadowed, &tool.installed));
                if shadows_removed {
                    remaining.shadowed = tool.shadowed.clone();
                }
            }
        }
        true
    }

    pub(crate) fn server_names(&self) -> Vec<String> {
        self.by_server.keys().cloned().collect()
    }
}

struct OwnedSkill {
    installed: Arc<Skill>,
    shadowed: Option<Arc<Skill>>,
}

/// Exact skill registrations installed through the live session API.
///
/// A captured pointer is the ownership token. Removing a live skill restores
/// its exact prior registration only while the installed pointer still owns
/// the name, so direct host mutations made later are never undone.
#[derive(Default)]
pub(crate) struct SessionSkillOwnership {
    by_name: HashMap<String, OwnedSkill>,
}

impl SessionSkillOwnership {
    pub(crate) fn install(&mut self, registry: &SkillRegistry, skill: Arc<Skill>) -> Result<()> {
        if skill.name.trim().is_empty() {
            return Err(crate::error::CodeError::Config(
                "Live skill name must not be empty".to_string(),
            ));
        }

        let name = skill.name.clone();
        let installed = Arc::clone(&skill);
        let (accepted, mut shadowed) = registry.register_with_shadow(skill).map_err(|error| {
            crate::error::CodeError::Config(format!(
                "Live skill '{name}' failed validation: {error}"
            ))
        })?;
        if !accepted {
            return Err(crate::error::CodeError::Config(format!(
                "Live skill '{name}' cannot shadow a built-in skill"
            )));
        }

        if let Some(previous) = self.by_name.remove(&name) {
            if shadowed
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &previous.installed))
            {
                shadowed = previous.shadowed;
            }
        }
        self.by_name.insert(
            name,
            OwnedSkill {
                installed,
                shadowed,
            },
        );
        Ok(())
    }

    pub(crate) fn remove(&mut self, name: &str, registry: &SkillRegistry) -> bool {
        let Some(owned) = self.by_name.remove(name) else {
            return false;
        };
        registry.restore_if_same(name, &owned.installed, owned.shadowed)
    }

    pub(crate) fn remove_all(&mut self, registry: &SkillRegistry) {
        let names = self.by_name.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.remove(&name, registry);
        }
    }
}

pub(super) struct SessionExtensionRuntime<'a> {
    session: &'a AgentSession,
}

impl<'a> SessionExtensionRuntime<'a> {
    pub(super) fn from_session(session: &'a AgentSession) -> Self {
        Self { session }
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

    pub(super) fn add_skill(&self, skill: Arc<Skill>) -> Result<()> {
        let name = skill.name.clone();
        self.session
            .close_handle
            .skill_ownership
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .install(&self.session.close_handle.skill_registry, skill)?;
        tracing::info!(
            session_id = %self.session.session_id,
            skill = %name,
            "Skill added to live session"
        );
        Ok(())
    }

    pub(super) fn remove_skill(&self, name: &str) {
        let restored = self
            .session
            .close_handle
            .skill_ownership
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(name, &self.session.close_handle.skill_registry);
        tracing::info!(
            session_id = %self.session.session_id,
            skill = %name,
            restored_shadow = restored,
            "Skill removed from live session"
        );
    }

    pub(super) async fn add_mcp_server(&self, config: McpServerConfig) -> Result<usize> {
        let _mutation = self.extension_mutation().await?;
        let server_name = config.name.clone();
        if self.session.mcp_manager.contains_server(&server_name).await {
            return Err(crate::error::CodeError::Tool {
                tool: server_name,
                message: "MCP server is already registered in this session".to_string(),
            });
        }

        self.session.mcp_manager.register_server(config).await;

        let connect = tokio::select! {
            biased;
            _ = self.session.session_cancel.cancelled() => {
                Err(crate::error::CodeError::SessionClosed {
                    session_id: self.session.session_id.clone(),
                })
            }
            result = self.session.mcp_manager.connect(&server_name) => {
                result.map_err(|e| crate::error::CodeError::Tool {
                    tool: server_name.clone(),
                    message: format!("Failed to connect MCP server: {e}"),
                })
            }
        };
        if let Err(error) = connect {
            self.rollback_server(&server_name).await;
            return Err(error);
        }

        // Close may have established its admission boundary just as connect
        // completed. Roll back instead of publishing tools into a closed
        // session; close will wait on this mutation guard before its final MCP
        // cleanup pass.
        if self.session.close_handle.is_closed() {
            self.rollback_server(&server_name).await;
            return Err(crate::error::CodeError::SessionClosed {
                session_id: self.session.session_id.clone(),
            });
        }

        let tools = self
            .session
            .mcp_manager
            .get_server_tools(&server_name)
            .await;
        let wrappers = crate::mcp::tools::create_mcp_tools(
            &server_name,
            tools,
            Arc::clone(&self.session.mcp_manager),
        );
        let count = self
            .session
            .close_handle
            .mcp_tool_ownership
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .install(&server_name, &self.session.tool_executor, wrappers);

        tracing::info!(
            session_id = %self.session.session_id,
            server = server_name,
            tools = count,
            "MCP server added to live session"
        );

        // TaskTool owns a TaskExecutor assembled from the session's effective
        // MCP sources. Refresh the delegation boundary after publishing live
        // wrappers so the next child run inherits the same capability set as
        // its parent session.
        self.session.refresh_task_delegation_tools();

        Ok(count)
    }

    pub(super) async fn remove_mcp_server(&self, server_name: &str) -> Result<()> {
        let _mutation = self.extension_mutation().await?;
        let remove_result = self.session.mcp_manager.remove_server(server_name).await;

        let had_owned_tools = self
            .session
            .close_handle
            .mcp_tool_ownership
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(server_name, &self.session.tool_executor);

        if had_owned_tools {
            self.restore_missing_inherited_tools(server_name).await;
        }

        // Stop advertising the removed server to newly delegated child runs.
        // An already-running task retains its own executor and settles through
        // the normal MCP/session cancellation path.
        self.session.refresh_task_delegation_tools();

        if let Err(error) = remove_result {
            return Err(crate::error::CodeError::Tool {
                tool: server_name.to_string(),
                message: format!("MCP server was removed, but transport cleanup failed: {error}"),
            });
        }

        tracing::info!(
            session_id = %self.session.session_id,
            server = server_name,
            "MCP server removed from live session"
        );
        Ok(())
    }

    async fn extension_mutation(&self) -> Result<tokio::sync::MutexGuard<'_, ()>> {
        let guard = self.session.close_handle.extension_mutation.lock().await;
        if self.session.close_handle.is_closed() {
            return Err(crate::error::CodeError::SessionClosed {
                session_id: self.session.session_id.clone(),
            });
        }
        Ok(guard)
    }

    async fn rollback_server(&self, server_name: &str) {
        if let Err(error) = self.session.mcp_manager.remove_server(server_name).await {
            tracing::warn!(
                session_id = %self.session.session_id,
                server = server_name,
                error = %error,
                "Failed to close MCP transport while rolling back live add"
            );
        }
    }

    async fn restore_missing_inherited_tools(&self, server_name: &str) {
        // Build the effective inherited set in source precedence order. Exact
        // shadows captured at install time have already been restored; this
        // pass only fills names that are currently absent, so it cannot
        // overwrite a host tool registered while the local server was active.
        let mut effective = BTreeMap::new();
        for inherited in &self.session.inherited_mcp_managers {
            let tools = inherited.get_server_tools(server_name).await;
            for tool in
                crate::mcp::tools::create_mcp_tools(server_name, tools, Arc::clone(inherited))
            {
                effective.insert(tool.name().to_string(), tool);
            }
        }
        for tool in effective.into_values() {
            self.session
                .tool_executor
                .register_dynamic_tool_if_absent(tool);
        }
    }

    pub(super) async fn mcp_status(&self) -> HashMap<String, McpServerStatus> {
        let mut status = HashMap::new();
        for manager in &self.session.mcp_managers {
            status.extend(manager.get_status().await);
        }
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ToolContext, ToolOutput};
    use async_trait::async_trait;

    struct NamedTool(String);

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            &self.0
        }

        fn description(&self) -> &str {
            "test tool"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::success("ok"))
        }
    }

    fn tool(name: &str) -> Arc<dyn Tool> {
        Arc::new(NamedTool(name.to_string()))
    }

    #[test]
    fn mcp_ownership_removes_only_exact_still_owned_registrations() {
        let executor = ToolExecutor::new("/tmp/a3s-mcp-ownership-test".to_string());
        let inherited = tool("mcp__alpha__shared");
        let unrelated_prefix_collision = tool("mcp__alpha__nested__other-server-tool");
        executor.register_dynamic_tool(Arc::clone(&inherited));
        executor.register_dynamic_tool(Arc::clone(&unrelated_prefix_collision));

        let local_shared = tool("mcp__alpha__shared");
        let local_only = tool("mcp__alpha__local");
        let mut ownership = SessionMcpToolOwnership::default();
        assert_eq!(
            ownership.install(
                "alpha",
                &executor,
                vec![Arc::clone(&local_shared), Arc::clone(&local_only)],
            ),
            2
        );

        // A host registration after MCP add owns the name now and must survive
        // local-server removal.
        let later_host = tool("mcp__alpha__local");
        executor.register_dynamic_tool(Arc::clone(&later_host));
        assert!(ownership.remove("alpha", &executor));

        let restored = executor.registry().get("mcp__alpha__shared").unwrap();
        assert!(Arc::ptr_eq(&restored, &inherited));
        let current_host = executor.registry().get("mcp__alpha__local").unwrap();
        assert!(Arc::ptr_eq(&current_host, &later_host));
        let collision = executor
            .registry()
            .get("mcp__alpha__nested__other-server-tool")
            .unwrap();
        assert!(Arc::ptr_eq(&collision, &unrelated_prefix_collision));
        assert!(!ownership.remove("alpha", &executor));
    }

    #[test]
    fn removing_shadowed_local_server_splices_it_out_of_owner_chain() {
        let executor = ToolExecutor::new("/tmp/a3s-mcp-owner-chain-test".to_string());
        let full_name = "mcp__alpha__nested__tool";
        let first = tool(full_name);
        let second = tool(full_name);
        let mut ownership = SessionMcpToolOwnership::default();
        ownership.install("alpha", &executor, vec![Arc::clone(&first)]);
        ownership.install("alpha__nested", &executor, vec![Arc::clone(&second)]);

        assert!(ownership.remove("alpha", &executor));
        let current = executor.registry().get(full_name).unwrap();
        assert!(Arc::ptr_eq(&current, &second));

        assert!(ownership.remove("alpha__nested", &executor));
        assert!(executor.registry().get(full_name).is_none());
    }
}
