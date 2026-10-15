//! Capability projection adapter for one exact MCP connection.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::manager::connect_ready_client_retained;
use super::{McpBinding, McpClient, McpServerConfig};
use crate::capability::{
    CapabilityAdapterError, CapabilityEffect, CapabilityEffectError, CapabilityProjectionAdapter,
    CapabilityValue, PreparedCapability, RetainedUseGeneration,
};

/// Same bound as agent bootstrap. One unreachable server must not stall session admission.
const MCP_PROJECTION_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Fallible MCP preparation owned by one capability contribution.
///
/// A trusted host constructs the configuration from an already selected A3S
/// Use surface and its exact Runtime/Gateway evidence. This adapter performs
/// only standard MCP transport connection, initialize, and `tools/list`; it
/// does not inspect package files, select a provider, resolve a Registry, or
/// publish a Use generation.
pub struct McpProjectionAdapter {
    config: McpServerConfig,
    retention: Option<Arc<dyn RetainedUseGeneration>>,
    working_directory: Option<std::path::PathBuf>,
}

impl McpProjectionAdapter {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            retention: None,
            working_directory: None,
        }
    }

    /// Set the trusted host's working directory for the exact stdio process.
    /// This selects process routing; it does not grant filesystem authority.
    pub fn with_working_directory(mut self, working_directory: std::path::PathBuf) -> Self {
        self.working_directory = Some(working_directory);
        self
    }

    /// Retain the trusted host's exact artifact selection while its process exists.
    /// Run admission separately acquires and validates its own generation lease.
    pub fn with_retained_use_generation(
        mut self,
        retention: Arc<dyn RetainedUseGeneration>,
    ) -> Self {
        self.retention = Some(retention);
        self
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

        // Sampling stays unarmed here by decision: capability projection
        // adapters are frozen-binding contexts, and the sampling authority
        // belongs to the session-facing manager (SEC-SAMPLE1).
        let (client, tools) = match tokio::time::timeout(
            MCP_PROJECTION_CONNECT_TIMEOUT,
            connect_ready_client_retained(
                &self.config,
                self.retention.clone(),
                self.working_directory.as_deref(),
                None,
            ),
        )
        .await
        {
            Ok(result) => result.map_err(|error| CapabilityAdapterError::new(error.to_string()))?,
            Err(_) => {
                return Err(CapabilityAdapterError::new(format!(
                    "MCP projection connect timed out after {}s",
                    MCP_PROJECTION_CONNECT_TIMEOUT.as_secs()
                )));
            }
        };
        if cancellation.is_cancelled() {
            close_after_failed_prepare(&self.config.name, &client).await;
            return Err(CapabilityAdapterError::new(
                "MCP projection preparation was cancelled",
            ));
        }

        let binding = match McpBinding::new(&self.config.name, Arc::clone(&client), tools) {
            Ok(binding) => Arc::new(
                if matches!(
                    self.config.transport,
                    super::McpTransportConfig::Stdio { .. }
                ) {
                    binding.with_owned_stdio_process()
                } else {
                    binding
                },
            ),
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
