//! Materialize `AgentDefinition.mcp_servers` and overlay them onto a disk/client merge.
//!
//! Agent.md servers win over the disk/client merge by name. Rebuild keeps live names
//! that were not part of the previous overlay; hot-reload and `UpdateMcpServers` replace
//! wholesale from merge plus overlay.

use std::collections::HashSet;
use std::path::Path;

use a3s_code_agent::config::{AgentDefinition, McpServerRef};
use a3s_code_tools::types::compat::CompatConfig;
use agent_client_protocol as acp;
use serde::Deserialize;

use crate::session::mcp_servers::mcp_server_name;

/// Inputs shared by disk rematerialize and the rebuild keep-live path.
pub(crate) struct RematerializeParams<'a> {
    pub initial_client_mcp_servers: Vec<acp::McpServer>,
    /// Seat cwd for disk/client merge so child/worktree owned-only + mcpInheritance stay.
    pub cwd: &'a Path,
    /// Parent project cwd for the kill switch. `None` uses `cwd`.
    pub parent_cwd: Option<&'a Path>,
    pub plugin_registry: Option<&'a a3s_code_agent::plugins::PluginRegistry>,
    pub compat: &'a CompatConfig,
    pub definition: &'a AgentDefinition,
}

/// Plugin ignore, folder-trust, inline/named parse, then managed-policy plus the
/// `enabled = false` / `disabled_mcp_servers` kill switch.
///
/// `named_lookup` is the pre-overlay merge (parent path) or parent snapshot (child).
/// Disabled names never leave this function, so overlay cannot re-append them.
pub(crate) fn materialize_agent_mcp_servers(
    definition: &AgentDefinition,
    named_lookup: &[acp::McpServer],
    cwd: &Path,
) -> Vec<acp::McpServer> {
    let servers = if definition.mcp_servers.is_empty() {
        Vec::new()
    } else if definition.plugin_name.is_some() {
        tracing::warn!(
            agent = %definition.name,
            plugin = ?definition.plugin_name,
            "ignoring mcpServers on plugin agent (not supported for security)"
        );
        Vec::new()
    } else if !crate::agent::folder_trust::agent_inline_hooks_allowed(definition.scope, || {
        crate::agent::folder_trust::project_scope_allowed(cwd)
    }) {
        tracing::warn!(
            agent = %definition.name,
            "ignoring mcpServers on untrusted project agent (folder not trusted; re-run with --trust)"
        );
        Vec::new()
    } else {
        parse_agent_mcp_server_refs(definition, named_lookup)
    };
    crate::session::managed_mcp::filter_policy_blocked_agent_mcp(servers, cwd)
}

/// Insert materialized agent.md servers by name so they win over the disk/client merge.
pub(crate) fn overlay_agent_mcp_servers(
    merged: &mut Vec<acp::McpServer>,
    overlay: Vec<acp::McpServer>,
) {
    for server in overlay {
        let name = mcp_server_name(&server);
        if let Some(slot) = merged
            .iter_mut()
            .find(|existing| mcp_server_name(existing) == name)
        {
            *slot = server;
        } else {
            merged.push(server);
        }
    }
}

/// Materialize against the current merge, then insert-wins by name.
pub(crate) fn apply_agent_mcp_overlay(
    merged: &mut Vec<acp::McpServer>,
    definition: &AgentDefinition,
    cwd: &Path,
) {
    let overlay = materialize_agent_mcp_servers(definition, merged, cwd);
    overlay_agent_mcp_servers(merged, overlay);
}

/// What a fork copies from the parent handle. A snapshot: a later seat switch
/// on the parent does not rewrite a child that already inherited.
pub(crate) fn mcp_servers_for_fork(
    parent: &crate::session::mcp_servers::AdmittedMcpServers,
) -> Vec<acp::McpServer> {
    parent.snapshot()
}

/// Disk merge uses seat `cwd`. Kill-switch policy uses `parent_cwd` when the seat is a child.
pub(crate) fn rematerialize_with_agent_overlay(
    params: RematerializeParams<'_>,
) -> Vec<acp::McpServer> {
    let policy_cwd = params.parent_cwd.unwrap_or(params.cwd);
    let mut merged = crate::session::managed_mcp::merge_managed_mcp_servers(
        params.initial_client_mcp_servers,
        params.cwd,
        params.plugin_registry,
        params.compat,
    );
    apply_agent_mcp_overlay(&mut merged, params.definition, policy_cwd);
    merged
}

/// Rebuild: rematerialize disk/client + the new seat, then keep live names outside
/// the previous overlay. The disk/client layer is the actor's admitted seed.
pub(crate) fn rematerialize_live_for_agent_seat(
    live: &[acp::McpServer],
    previous_mcp_servers: &[McpServerRef],
    params: RematerializeParams<'_>,
) -> Vec<acp::McpServer> {
    let previous_names = agent_overlay_names(previous_mcp_servers);
    let mut desired = rematerialize_with_agent_overlay(params);
    let mut desired_names: HashSet<String> = desired
        .iter()
        .map(|s| mcp_server_name(s).to_owned())
        .collect();
    for server in live {
        let name = mcp_server_name(server).to_owned();
        if desired_names.contains(&name) || previous_names.contains(&name) {
            continue;
        }
        desired.push(server.clone());
        desired_names.insert(name);
    }
    desired
}

fn agent_overlay_names(mcp_servers: &[McpServerRef]) -> HashSet<String> {
    mcp_servers
        .iter()
        .map(mcp_server_ref_name)
        .map(str::to_owned)
        .collect()
}

fn mcp_server_ref_name(entry: &McpServerRef) -> &str {
    match entry {
        McpServerRef::Named(name) | McpServerRef::Inline { name, .. } => name,
    }
}

fn parse_agent_mcp_server_refs(
    definition: &AgentDefinition,
    named_lookup: &[acp::McpServer],
) -> Vec<acp::McpServer> {
    definition
        .mcp_servers
        .iter()
        .filter_map(|entry| match entry {
            McpServerRef::Named(name) => named_lookup
                .iter()
                .find(|s| mcp_server_name(s) == name)
                .cloned()
                .or_else(|| {
                    tracing::warn!(
                        agent = %definition.name,
                        server = name,
                        "mcpServers: named ref not found in parent"
                    );
                    None
                }),
            McpServerRef::Inline { name, config } => {
                parse_inline_mcp_server(definition, name, config)
            }
        })
        .collect()
}

fn parse_inline_mcp_server(
    definition: &AgentDefinition,
    name: &str,
    config: &serde_json::Value,
) -> Option<acp::McpServer> {
    let obj = config.as_object()?;
    match parse_acp_mcp_object(name, obj) {
        Ok(server) => return Some(server),
        Err(acp_err) => {
            tracing::debug!(
                server = name,
                error = %acp_err,
                "ACP wire format parse failed, trying map-keyed"
            );
        }
    }
    match a3s_code_config_types::McpServerConfig::deserialize(config) {
        Ok(cfg) => acp_server_from_config(definition, name, cfg),
        Err(e) => {
            tracing::warn!(
                agent = %definition.name,
                server = name,
                error = %e,
                "mcpServers: inline config could not be parsed"
            );
            None
        }
    }
}

fn acp_server_from_config(
    definition: &AgentDefinition,
    name: &str,
    cfg: a3s_code_config_types::McpServerConfig,
) -> Option<acp::McpServer> {
    let Some(mut server) = cfg.to_acp_mcp_server(name) else {
        if cfg.enabled {
            tracing::warn!(
                agent = %definition.name,
                server = name,
                "mcpServers: inline config dropped (setup block or empty url)"
            );
        } else {
            tracing::debug!(
                agent = %definition.name,
                server = name,
                "mcpServers: skipping disabled inline server"
            );
        }
        return None;
    };
    crate::session::managed_mcp::canonicalize_mcp_maps(&mut server);
    Some(server)
}

fn parse_acp_mcp_object(
    name: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<acp::McpServer, serde_json::Error> {
    let mut flat = obj.clone();
    flat.insert(
        "name".to_owned(),
        serde_json::Value::String(name.to_owned()),
    );
    serde_json::from_value(serde_json::Value::Object(flat))
}

#[cfg(test)]
#[path = "agent_mcp_tests.rs"]
mod tests;
