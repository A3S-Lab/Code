use std::sync::Arc;

use async_trait::async_trait;

use crate::tools::{Tool, ToolCapabilities, ToolContext, ToolOutput};

use super::{CapabilityId, Sha256Digest};

pub(crate) struct ScopedProjectedTool {
    tool: Arc<dyn Tool>,
    id: CapabilityId,
    catalog_digest: Sha256Digest,
}

impl ScopedProjectedTool {
    pub(crate) fn bind(
        tool: Arc<dyn Tool>,
        id: CapabilityId,
        catalog_digest: Sha256Digest,
    ) -> Arc<dyn Tool> {
        Arc::new(Self {
            tool,
            id,
            catalog_digest,
        })
    }
}

#[async_trait]
impl Tool for ScopedProjectedTool {
    fn name(&self) -> &str {
        self.tool.name()
    }
    fn description(&self) -> &str {
        self.tool.description()
    }
    fn parameters(&self) -> serde_json::Value {
        self.tool.parameters()
    }
    fn definition(&self) -> crate::llm::ToolDefinition {
        self.tool.definition()
    }
    fn is_model_visible(&self) -> bool {
        self.tool.is_model_visible()
    }
    fn capabilities(&self, args: &serde_json::Value) -> ToolCapabilities {
        self.tool.capabilities(args)
    }
    fn requires_confirmation(&self, args: &serde_json::Value) -> bool {
        self.tool.requires_confirmation(args)
    }
    fn validate_execution_available(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<()> {
        // Preparation has no live Turn yet. Preserve the admitted Tool's
        // side-effect-free refusal; execute still checks projection authority.
        self.tool.validate_execution_available(args, ctx)
    }
    async fn execute(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let authorized = ctx.capability_context().is_some_and(|context| {
            context
                .authorizes_projection(&self.id, &self.catalog_digest)
                .unwrap_or(false)
        });
        if !authorized {
            return Err(crate::error::CodeError::Tool {
                tool: self.name().to_owned(),
                message: "Tool not found in active capability scope".to_owned(),
            }
            .into());
        }
        self.tool.execute(args, ctx).await
    }
}

#[cfg(test)]
mod tests;
