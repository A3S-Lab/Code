//! Capability projection adapter for one exact MCP connection.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::manager::connect_ready_client;
use super::{McpBinding, McpClient, McpServerConfig};
use crate::capability::{
    CapabilityAdapterError, CapabilityEffect, CapabilityEffectError, CapabilityProjectionAdapter,
    CapabilityValue, PreparedCapability,
};

/// Fallible MCP preparation owned by one capability contribution.
///
/// A trusted host constructs the configuration from an already selected A3S
/// Use surface and its exact Runtime/Gateway evidence. This adapter performs
/// only standard MCP transport connection, initialize, and `tools/list`; it
/// does not inspect package files, select a provider, resolve a Registry, or
/// publish a Use generation.
pub struct McpProjectionAdapter {
    config: McpServerConfig,
}

impl McpProjectionAdapter {
    pub fn new(config: McpServerConfig) -> Self {
        Self { config }
    }
}

impl fmt::Debug for McpProjectionAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Configuration can contain environment values, OAuth credentials,
        // and authorization headers. Never include it in diagnostics.
        formatter
            .debug_struct("McpProjectionAdapter")
            .field("server_name", &self.config.name)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CapabilityProjectionAdapter for McpProjectionAdapter {
    async fn prepare(
        self: Box<Self>,
        cancellation: CancellationToken,
    ) -> std::result::Result<PreparedCapability, CapabilityAdapterError> {
        if cancellation.is_cancelled() {
            return Err(CapabilityAdapterError::new(
                "MCP projection preparation was cancelled",
            ));
        }

        let (client, tools) = connect_ready_client(&self.config)
            .await
            .map_err(|error| CapabilityAdapterError::new(error.to_string()))?;
        if cancellation.is_cancelled() {
            close_after_failed_prepare(&self.config.name, &client).await;
            return Err(CapabilityAdapterError::new(
                "MCP projection preparation was cancelled",
            ));
        }

        let binding = match McpBinding::new(&self.config.name, Arc::clone(&client), tools) {
            Ok(binding) => Arc::new(binding),
            Err(error) => {
                close_after_failed_prepare(&self.config.name, &client).await;
                return Err(CapabilityAdapterError::new(error.to_string()));
            }
        };
        let mut prepared = PreparedCapability::new(CapabilityValue::Mcp(binding));
        prepared.push_effect(McpConnectionEffect {
            server_name: self.config.name.into_boxed_str(),
            client,
        })?;
        Ok(prepared)
    }
}

struct McpConnectionEffect {
    server_name: Box<str>,
    client: Arc<McpClient>,
}

#[async_trait]
impl CapabilityEffect for McpConnectionEffect {
    fn name(&self) -> &str {
        "mcp.projected.connection"
    }

    async fn close(self: Box<Self>) -> std::result::Result<(), CapabilityEffectError> {
        self.client.close().await.map_err(|error| {
            CapabilityEffectError::new(format!(
                "Failed to close projected MCP server '{}': {error}",
                self.server_name
            ))
        })
    }
}

async fn close_after_failed_prepare(server_name: &str, client: &McpClient) {
    if let Err(error) = client.close().await {
        tracing::warn!(
            server = %server_name,
            error = %error,
            "Failed to close MCP connection after projection preparation failed"
        );
    }
}
