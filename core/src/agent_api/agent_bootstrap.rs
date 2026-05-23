//! Agent bootstrap and global capability loading.
//!
//! The `Agent` facade exposes a small creation API. This module owns the
//! heavier bootstrap contract: config source detection, default model
//! validation, global MCP connection, and global skill registry setup.

use super::Agent;
use crate::agent::AgentConfig;
use crate::config::CodeConfig;
use crate::error::{CodeError, Result};
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) fn load_code_config(config_source: String) -> Result<CodeConfig> {
    let expanded = expand_home(&config_source);
    let path = Path::new(&expanded);
    let ext = path.extension().and_then(|ext| ext.to_str());

    if matches!(ext, Some("acl")) {
        if !path.exists() {
            return Err(CodeError::Config(format!(
                "Config file not found: {}",
                path.display()
            )));
        }

        return Ok(CodeConfig::from_file(path)
            .with_context(|| format!("Failed to load config: {}", path.display()))?);
    }

    if matches!(ext, Some("hcl")) {
        return Err(CodeError::Config(
            "HCL config files are not supported in 2.0; rename the file to .acl".into(),
        ));
    }

    if config_source.trim().starts_with('{') {
        return Err(CodeError::Config(
            "JSON config is not supported; use ACL-compatible .acl config".into(),
        ));
    }

    if matches!(ext, Some("json")) {
        return Err(CodeError::Config(
            "JSON config files are not supported; use .acl".into(),
        ));
    }

    Ok(CodeConfig::from_acl(&config_source).context("Failed to parse config as ACL string")?)
}

pub(super) async fn build_agent_from_config(config: CodeConfig) -> Result<Agent> {
    config
        .default_llm_config()
        .context("default_model must be set in 'provider/model' format with a valid API key")?;

    let mut agent_config = base_agent_config(&config);
    install_global_skill_registry(&mut agent_config, &config);
    let (global_mcp, global_mcp_tools) = connect_global_mcp(&config).await;

    Ok(Agent {
        code_config: config,
        config: agent_config,
        global_mcp,
        global_mcp_tools: std::sync::Mutex::new(global_mcp_tools),
    })
}

fn expand_home(source: &str) -> String {
    let Some(rest) = source.strip_prefix("~/") else {
        return source.to_string();
    };

    match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        Some(home) => PathBuf::from(home).join(rest).display().to_string(),
        None => source.to_string(),
    }
}

fn base_agent_config(config: &CodeConfig) -> AgentConfig {
    let mut auto_delegation = config.auto_delegation.clone();
    if let Some(auto_parallel) = config.auto_parallel {
        auto_delegation.auto_parallel = auto_parallel;
    }

    AgentConfig {
        max_tool_rounds: config
            .max_tool_rounds
            .unwrap_or(AgentConfig::default().max_tool_rounds),
        max_parallel_tasks: config
            .max_parallel_tasks
            .unwrap_or(AgentConfig::default().max_parallel_tasks)
            .max(1),
        auto_delegation,
        ..AgentConfig::default()
    }
}

fn install_global_skill_registry(agent_config: &mut AgentConfig, config: &CodeConfig) {
    let registry = Arc::new(crate::skills::SkillRegistry::with_builtins());
    for dir in &config.skill_dirs {
        if let Err(e) = registry.load_from_dir(dir) {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "Failed to load skills from directory - skipping"
            );
        }
    }
    agent_config.skill_registry = Some(registry);
}

async fn connect_global_mcp(
    config: &CodeConfig,
) -> (
    Option<Arc<crate::mcp::manager::McpManager>>,
    Vec<(String, crate::mcp::McpTool)>,
) {
    if config.mcp_servers.is_empty() {
        return (None, Vec::new());
    }

    let manager = Arc::new(crate::mcp::manager::McpManager::new());
    for server in &config.mcp_servers {
        if !server.enabled {
            continue;
        }
        manager.register_server(server.clone()).await;
        if let Err(e) = manager.connect(&server.name).await {
            tracing::warn!(
                server = %server.name,
                error = %e,
                "Failed to connect to MCP server - skipping"
            );
        }
    }

    let tools = manager.get_all_tools().await;
    (Some(manager), tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_home_leaves_non_home_sources_unchanged() {
        assert_eq!(expand_home("provider \"x\" {}"), "provider \"x\" {}");
    }

    #[test]
    fn load_code_config_rejects_json_inline_config() {
        let err = load_code_config("{\"default_model\":\"x\"}".to_string()).unwrap_err();
        assert!(err.to_string().contains("JSON config is not supported"));
    }
}
