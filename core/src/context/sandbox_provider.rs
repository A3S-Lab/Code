//! Current host execution availability, independent of retrieval relevance.

use super::{ContextItem, ContextProvider, ContextQuery, ContextResult, ContextType};
use crate::sandbox::BashSandbox;
use std::sync::Arc;

pub(crate) struct SandboxContextProvider {
    sandbox: Arc<dyn BashSandbox>,
}

impl SandboxContextProvider {
    pub(crate) fn new(sandbox: Arc<dyn BashSandbox>) -> Self {
        Self { sandbox }
    }
}

#[async_trait::async_trait]
impl ContextProvider for SandboxContextProvider {
    fn name(&self) -> &str {
        "sandbox_availability"
    }

    async fn query(&self, _query: &ContextQuery) -> anyhow::Result<ContextResult> {
        let mut result = ContextResult::new(self.name());
        if let Err(error) = self.sandbox.validate_execution_available() {
            let reason = error.to_string();
            let reason = crate::text::truncate_utf8(&reason, 2048);
            result.add_item(
                ContextItem::new(
                    "sandbox_availability",
                    ContextType::Resource,
                    format!(
                        "Command execution through this session's sandbox is unavailable.\n\
                         Backend refusal: {reason}\n\
                         Do not repeat command execution calls while this condition is unchanged. \
                         If the task requires execution, report the missing backend capability. \
                         Independently available file and lookup tools remain usable."
                    ),
                )
                .with_source("a3s://runtime/execution")
                .with_provenance("host_sandbox")
                .with_relevance(1.0)
                .with_required(),
            );
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
