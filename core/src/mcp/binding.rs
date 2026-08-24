//! Immutable, generation-exact MCP runtime bindings.
//!
//! Compatibility MCP APIs resolve calls through a mutable multi-server
//! [`McpManager`](super::McpManager). Capability projections cannot use that
//! boundary: a later registration under the same server name could otherwise
//! change an already admitted Run. [`McpBinding`] instead owns one exact
//! initialized client and one canonical frozen tool catalog.

use std::fmt;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use thiserror::Error;

use super::client::McpClient;
use super::protocol::McpTool;
use super::result::project_tool_result;
use super::tools::annotation_requires_confirmation;
use crate::tools::{Tool, ToolContext, ToolOutput};

pub const MAX_MCP_BINDING_TOOLS: usize = 1_024;
pub const MAX_MCP_BINDING_DEFINITION_BYTES: usize = 16 * 1024 * 1024;
const MAX_MCP_NAME_BYTES: usize = 256;
const MAX_MCP_FULL_TOOL_NAME_BYTES: usize = 768;

/// Invalid or no-longer-ready exact MCP binding.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum McpBindingError {
    #[error("MCP {field} is empty, padded, contains control characters, or exceeds its bound")]
    InvalidName { field: &'static str },
    #[error("MCP binding server name does not match its exact client identity")]
    ClientNameMismatch,
    #[error("MCP binding exceeds the {field} bound of {max}")]
    BoundExceeded { field: &'static str, max: usize },
    #[error("MCP binding repeats tool name '{name}'")]
    DuplicateToolName { name: String },
    #[error("MCP binding tool catalog is not serializable")]
    InvalidToolCatalog,
    #[error("MCP binding client is not initialized and connected")]
    ClientNotReady,
}

/// One exact connected MCP server projected into an immutable Code catalog.
///
/// The binding deliberately contains no manager, package locator, Grant, or
/// mutable discovery handle. Its client and canonical tool definitions are
/// frozen together, while the owning capability transaction retains the
/// client's asynchronous close effect. A Run separately retains the matching
/// non-clone A3S Use generation lease.
pub struct McpBinding {
    server_name: Box<str>,
    client: Arc<McpClient>,
    tools: Arc<[McpTool]>,
}

impl McpBinding {
    pub fn new(
        server_name: impl Into<String>,
        client: Arc<McpClient>,
        tools: impl IntoIterator<Item = McpTool>,
    ) -> std::result::Result<Self, McpBindingError> {
        let server_name = server_name.into();
        validate_name("server name", &server_name)?;
        if client.name != server_name {
            return Err(McpBindingError::ClientNameMismatch);
        }
        if !client.is_ready() {
            return Err(McpBindingError::ClientNotReady);
        }

        let mut tools = tools.into_iter().collect::<Vec<_>>();
        if tools.len() > MAX_MCP_BINDING_TOOLS {
            return Err(McpBindingError::BoundExceeded {
                field: "tool count",
                max: MAX_MCP_BINDING_TOOLS,
            });
        }
        tools.sort_by(|left, right| left.name.cmp(&right.name));

        let mut definition_bytes = 0_usize;
        let mut previous_name: Option<&str> = None;
        for tool in &tools {
            validate_name("tool name", &tool.name)?;
            let full_name_len = "mcp__"
                .len()
                .saturating_add(server_name.len())
                .saturating_add("__".len())
                .saturating_add(tool.name.len());
            if full_name_len > MAX_MCP_FULL_TOOL_NAME_BYTES {
                return Err(McpBindingError::BoundExceeded {
                    field: "fully qualified tool name bytes",
                    max: MAX_MCP_FULL_TOOL_NAME_BYTES,
                });
            }
            if previous_name == Some(tool.name.as_str()) {
                return Err(McpBindingError::DuplicateToolName {
                    name: tool.name.clone(),
                });
            }
            previous_name = Some(&tool.name);
            let encoded =
                serde_json::to_vec(tool).map_err(|_| McpBindingError::InvalidToolCatalog)?;
            definition_bytes = definition_bytes.saturating_add(encoded.len());
            if definition_bytes > MAX_MCP_BINDING_DEFINITION_BYTES {
                return Err(McpBindingError::BoundExceeded {
                    field: "tool definition bytes",
                    max: MAX_MCP_BINDING_DEFINITION_BYTES,
                });
            }
        }

        Ok(Self {
            server_name: server_name.into_boxed_str(),
            client,
            tools: tools.into(),
        })
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    pub fn is_ready(&self) -> bool {
        self.client.is_ready()
    }

    pub fn validate_run_scope(&self) -> std::result::Result<(), McpBindingError> {
        if self.is_ready() {
            Ok(())
        } else {
            Err(McpBindingError::ClientNotReady)
        }
    }

    pub fn contains_public_tool_name(&self, full_name: &str) -> bool {
        self.tools
            .iter()
            .any(|tool| full_name == format!("mcp__{}__{}", self.server_name, tool.name))
    }

    pub(crate) fn projected_tools(self: &Arc<Self>) -> Vec<Arc<dyn Tool>> {
        self.tools
            .iter()
            .enumerate()
            .map(|(tool_index, tool)| {
                Arc::new(ProjectedMcpTool {
                    full_name: format!("mcp__{}__{}", self.server_name, tool.name).into_boxed_str(),
                    tool_index,
                    binding: Arc::clone(self),
                }) as Arc<dyn Tool>
            })
            .collect()
    }

    async fn call_tool(
        &self,
        tool_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<super::protocol::CallToolResult> {
        if self
            .tools
            .binary_search_by(|tool| tool.name.as_str().cmp(tool_name))
            .is_err()
        {
            return Err(anyhow!(
                "MCP tool '{}' is not present in the frozen server binding",
                tool_name
            ));
        }
        self.client.call_tool(tool_name, arguments).await
    }
}

impl fmt::Debug for McpBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpBinding")
            .field("server_name", &self.server_name)
            .field("tool_count", &self.tools.len())
            .field("ready", &self.is_ready())
            .finish_non_exhaustive()
    }
}

struct ProjectedMcpTool {
    full_name: Box<str>,
    tool_index: usize,
    binding: Arc<McpBinding>,
}

impl ProjectedMcpTool {
    fn tool(&self) -> &McpTool {
        &self.binding.tools[self.tool_index]
    }
}

#[async_trait]
impl Tool for ProjectedMcpTool {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> &str {
        self.tool().description.as_deref().unwrap_or("MCP tool")
    }

    fn parameters(&self) -> serde_json::Value {
        self.tool().input_schema.clone()
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        annotation_requires_confirmation(self.tool())
    }

    async fn execute(&self, args: &serde_json::Value, context: &ToolContext) -> Result<ToolOutput> {
        let result = self
            .binding
            .call_tool(&self.tool().name, Some(args.clone()))
            .await;
        match result {
            Ok(result) => project_tool_result(&self.full_name, &result, context).await,
            Err(error) => Ok(ToolOutput::error(format!("MCP tool error: {error}"))),
        }
    }
}

fn validate_name(field: &'static str, value: &str) -> std::result::Result<(), McpBindingError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_MCP_NAME_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(McpBindingError::InvalidName { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::mcp::test_support::{mcp_tool, ready_binding, RecordingMcpTransport};
    use crate::mcp::transport::McpTransport;
    use crate::mcp::McpProjectionAdapter;

    #[tokio::test]
    async fn binding_rejects_clients_that_are_not_ready_or_have_another_identity() {
        let transport = RecordingMcpTransport::new("uninitialized", Vec::new());
        let client = Arc::new(McpClient::new(
            "catalog".to_string(),
            Arc::clone(&transport) as Arc<dyn McpTransport>,
        ));
        assert_eq!(
            McpBinding::new("catalog", Arc::clone(&client), Vec::new()).unwrap_err(),
            McpBindingError::ClientNotReady
        );

        client.initialize().await.unwrap();
        assert_eq!(
            McpBinding::new("another", Arc::clone(&client), Vec::new()).unwrap_err(),
            McpBindingError::ClientNameMismatch
        );

        transport.disconnect();
        assert_eq!(
            McpBinding::new("catalog", client, Vec::new()).unwrap_err(),
            McpBindingError::ClientNotReady
        );
    }

    #[tokio::test]
    async fn binding_canonicalizes_tools_and_rejects_duplicate_or_oversized_catalogs() {
        let (_, _transport, client) = ready_binding("catalog", "one", Vec::new()).await;
        let binding = McpBinding::new(
            "catalog",
            Arc::clone(&client),
            [mcp_tool("zeta", "last"), mcp_tool("alpha", "first")],
        )
        .unwrap();
        assert_eq!(
            binding
                .tools()
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );

        assert_eq!(
            McpBinding::new(
                "catalog",
                Arc::clone(&client),
                [mcp_tool("same", "one"), mcp_tool("same", "two")],
            )
            .unwrap_err(),
            McpBindingError::DuplicateToolName {
                name: "same".to_string()
            }
        );

        let too_many = (0..=MAX_MCP_BINDING_TOOLS)
            .map(|index| mcp_tool(&format!("tool-{index:04}"), "bounded"))
            .collect::<Vec<_>>();
        assert_eq!(
            McpBinding::new("catalog", Arc::clone(&client), too_many).unwrap_err(),
            McpBindingError::BoundExceeded {
                field: "tool count",
                max: MAX_MCP_BINDING_TOOLS,
            }
        );

        let oversized = mcp_tool("oversized", &"x".repeat(MAX_MCP_BINDING_DEFINITION_BYTES));
        assert_eq!(
            McpBinding::new("catalog", client, [oversized]).unwrap_err(),
            McpBindingError::BoundExceeded {
                field: "tool definition bytes",
                max: MAX_MCP_BINDING_DEFINITION_BYTES,
            }
        );
    }

    #[tokio::test]
    async fn projected_wrapper_calls_the_raw_tool_on_the_exact_client() {
        let (binding, transport, _) = ready_binding(
            "catalog",
            "generation-one",
            vec![mcp_tool("lookup", "generation-one")],
        )
        .await;
        let wrappers = binding.projected_tools();
        assert_eq!(wrappers.len(), 1);
        assert_eq!(wrappers[0].name(), "mcp__catalog__lookup");
        assert_eq!(wrappers[0].description(), "generation-one");

        let arguments = serde_json::json!({"generation": "one"});
        let output = wrappers[0]
            .execute(&arguments, &ToolContext::new(PathBuf::from("/tmp")))
            .await
            .unwrap();
        assert!(output.success);
        assert_eq!(output.content, "generation-one");
        assert_eq!(
            transport.calls(),
            [crate::mcp::test_support::RecordedMcpCall {
                name: "lookup".to_string(),
                arguments: Some(arguments),
            }]
        );
    }

    #[test]
    fn projected_mcp_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<McpBinding>();
        assert_send_sync::<McpProjectionAdapter>();
    }
}
