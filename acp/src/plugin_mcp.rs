use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};
use a3s_use_reconcile::PublishedSurface;
use anyhow::{Context, Result};
use serde_json::Value;

pub(crate) enum McpAdmission {
    Ready(Box<McpServerConfig>),
    /// `stdio` whose command is not a file inside the package. It is not started.
    ExternalRuntime,
}

pub(crate) fn admit(surface: &PublishedSurface) -> Result<McpAdmission> {
    let server = surface
        .mcp
        .as_ref()
        .context("native MCP projection is missing")?;
    let transport = match server["type"].as_str() {
        Some("stdio") => {
            let Some(entry) = surface.entry.as_ref() else {
                return Ok(McpAdmission::ExternalRuntime);
            };
            let command = server["command"]
                .as_str()
                .context("native MCP command is missing")?;
            anyhow::ensure!(
                surface.directory.join(command) == *entry,
                "native MCP command differs from its managed entry"
            );
            crate::plugin_tools::validate_executable(entry)?;
            McpTransportConfig::Stdio {
                command: entry
                    .to_str()
                    .context("native MCP entry is not UTF-8")?
                    .to_owned(),
                args: optional(server, "args")?,
            }
        }
        Some("http") | Some("sse") => {
            let url = server["url"]
                .as_str()
                .context("native MCP URL is missing")?
                .to_owned();
            let headers = optional(server, "headers")?;
            if server["type"] == "http" {
                McpTransportConfig::StreamableHttp { url, headers }
            } else {
                McpTransportConfig::Http { url, headers }
            }
        }
        _ => anyhow::bail!("native MCP transport is unsupported"),
    };
    let config = McpServerConfig {
        name: surface.id.clone(),
        transport,
        enabled: match server.get("enabled") {
            Some(value) => serde_json::from_value(value.clone())?,
            None => true,
        },
        env: optional(server, "env")?,
        oauth: optional(server, "oauth")?,
        tool_timeout_secs: match server.get("tool_timeout_secs") {
            Some(value) => serde_json::from_value(value.clone())?,
            None => 60,
        },
    };
    anyhow::ensure!(
        config.tool_timeout_secs > 0,
        "native MCP request timeout must be positive"
    );
    Ok(McpAdmission::Ready(Box::new(config)))
}

fn optional<T: serde::de::DeserializeOwned + Default>(server: &Value, field: &str) -> Result<T> {
    match server.get(field) {
        Some(value) => serde_json::from_value(value.clone())
            .with_context(|| format!("invalid native MCP {field}")),
        None => Ok(T::default()),
    }
}

#[cfg(test)]
#[path = "plugin_mcp_tests.rs"]
mod tests;

#[cfg(test)]
mod admission_tests;
