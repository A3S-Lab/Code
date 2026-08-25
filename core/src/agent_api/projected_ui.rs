//! Host inspection handle for one exact projected UI generation.

use std::fmt;
use std::sync::Arc;

use super::AgentSession;
use crate::capability::{
    CapabilityDescriptor, CapabilityId, CapabilityKind, CapabilityValue, CodeCatalogGeneration,
    ScopeCloseReport, SessionCapabilityRun, Sha256Digest, UiBinding, UiDocument,
    UseCapabilityGeneration,
};
use crate::error::Result;

/// Non-clone host handle retaining one exact Code projection and A3S Use lease.
///
/// The handle exposes immutable path-free content and the exact dependency
/// edges selected at admission. It grants no renderer, filesystem, network,
/// process, secret, state, or backend-message authority. The embedding host
/// must close it after the complete render or interaction window drains.
#[must_use = "a projected UI handle must remain alive for the complete host render window"]
pub struct ProjectedUiHandle {
    descriptor: CapabilityDescriptor,
    binding: Arc<UiBinding>,
    capability_run: SessionCapabilityRun,
}

impl ProjectedUiHandle {
    pub fn capability_id(&self) -> &CapabilityId {
        self.descriptor.id()
    }

    pub fn public_name(&self) -> &str {
        self.binding.public_name()
    }

    pub fn title(&self) -> &str {
        self.binding.title()
    }

    pub fn description(&self) -> &str {
        self.binding.description()
    }

    pub fn icon(&self) -> &str {
        self.binding.icon()
    }

    pub fn order(&self) -> i32 {
        self.binding.order()
    }

    pub fn document(&self) -> &UiDocument {
        self.binding.document()
    }

    pub fn surface_digest(&self) -> &Sha256Digest {
        self.binding.surface_digest()
    }

    /// Exact Tool, Skill, MCP, and Flow edges authorized for this UI value.
    pub fn dependencies(&self) -> &[CapabilityId] {
        self.descriptor.dependencies()
    }

    pub fn catalog_generation(&self) -> CodeCatalogGeneration {
        self.capability_run.projection().set().generation()
    }

    pub fn use_generation(&self) -> Option<&UseCapabilityGeneration> {
        self.capability_run.run_scope().use_generation()
    }

    /// Whether Session shutdown has cancelled this host render window.
    pub fn is_cancelled(&self) -> bool {
        self.capability_run
            .run_scope()
            .cancellation()
            .is_cancelled()
    }

    /// Close the exact Code/Use scope after the host has drained this UI.
    pub async fn close(self) -> Result<ScopeCloseReport> {
        self.capability_run.close().await.map_err(Into::into)
    }
}

impl fmt::Debug for ProjectedUiHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectedUiHandle")
            .field("capability_id", &self.capability_id())
            .field("public_name", &self.public_name())
            .field("catalog_generation", &self.catalog_generation())
            .field("use_generation", &self.use_generation())
            .field("surface_digest", &self.surface_digest())
            .finish_non_exhaustive()
    }
}

impl AgentSession {
    /// Admit one exact named UI value from the current atomic catalog.
    ///
    /// Missing names return `None` without acquiring an A3S Use lease. The
    /// returned handle pins the selected descriptor, bytes, Code generation,
    /// and exact Use generation across later N+1 publication.
    pub async fn projected_ui(&self, public_name: &str) -> Result<Option<ProjectedUiHandle>> {
        let Some(admitted) = self
            .admit_projected_host_capability(CapabilityKind::Ui, public_name, |value| match value {
                CapabilityValue::Ui(binding) => Some(Arc::clone(binding)),
                _ => None,
            })
            .await?
        else {
            return Ok(None);
        };

        Ok(Some(ProjectedUiHandle {
            descriptor: admitted.descriptor,
            binding: admitted.value,
            capability_run: admitted.capability_run,
        }))
    }
}
