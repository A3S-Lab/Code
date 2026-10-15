use tokio_util::sync::CancellationToken;

use super::{
    CapabilityScopeError, CapabilityScopeHandle, CapabilityScopeId, CodeCatalogGeneration,
    Sha256Digest, Turn, UseCapabilityGeneration, WorkspaceCapabilityCeiling,
};

/// Workspace operations governed by the admitted scope ceiling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceOperation {
    Read,
    Write,
    Execute,
    Search,
    Git,
    CodeIntelligence,
}

impl WorkspaceOperation {
    pub fn parse(value: &str) -> Result<Self, CapabilityScopeError> {
        match value {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "execute" => Ok(Self::Execute),
            "search" => Ok(Self::Search),
            "git" => Ok(Self::Git),
            "code_intelligence" => Ok(Self::CodeIntelligence),
            _ => Err(CapabilityScopeError::InvalidWorkspaceOperation {
                operation: value.chars().take(128).collect(),
            }),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Execute => "execute",
            Self::Search => "search",
            Self::Git => "git",
            Self::CodeIntelligence => "code_intelligence",
        }
    }

    pub(super) fn allowed_by(self, ceiling: WorkspaceCapabilityCeiling) -> bool {
        match self {
            Self::Read => ceiling.read(),
            Self::Write => ceiling.write(),
            Self::Execute => ceiling.execute(),
            Self::Search => ceiling.search(),
            Self::Git => ceiling.git(),
            Self::CodeIntelligence => ceiling.code_intelligence(),
        }
    }
}

/// Immutable observation of accepted identities; this snapshot is not authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceOperationBinding {
    pub(super) scope_id: CapabilityScopeId,
    pub(super) catalog_generation: CodeCatalogGeneration,
    pub(super) catalog_digest: Sha256Digest,
    pub(super) use_generation: Option<UseCapabilityGeneration>,
}

impl WorkspaceOperationBinding {
    pub fn scope_id(&self) -> &CapabilityScopeId {
        &self.scope_id
    }

    pub const fn catalog_generation(&self) -> CodeCatalogGeneration {
        self.catalog_generation
    }

    pub fn catalog_digest(&self) -> &Sha256Digest {
        &self.catalog_digest
    }

    pub fn use_generation(&self) -> Option<&UseCapabilityGeneration> {
        self.use_generation.as_ref()
    }
}

/// Live workspace operation authority minted only from an actual Tool Turn.
///
/// Clones retain no owner, catalog or Use lease. Every operation rechecks the
/// owner and ceiling. Native adapters still enforce path/resource and package
/// grants; this handle alone cannot admit a resource or contain a process.
#[derive(Clone, Debug)]
pub struct WorkspaceOperationAccess {
    owner: CapabilityScopeHandle<Turn>,
    operation: WorkspaceOperation,
}

impl WorkspaceOperationAccess {
    pub(crate) fn new(
        owner: CapabilityScopeHandle<Turn>,
        operation: WorkspaceOperation,
    ) -> Result<Self, CapabilityScopeError> {
        let access = Self { owner, operation };
        access.check()?;
        Ok(access)
    }

    pub const fn operation(&self) -> WorkspaceOperation {
        self.operation
    }

    pub fn check(&self) -> Result<WorkspaceOperationBinding, CapabilityScopeError> {
        self.authority().map(|(binding, _)| binding)
    }

    /// Cancellation belongs to the real Turn, not a caller-supplied token.
    pub fn cancellation(&self) -> Result<CancellationToken, CapabilityScopeError> {
        self.authority().map(|(_, cancellation)| cancellation)
    }

    fn authority(
        &self,
    ) -> Result<(WorkspaceOperationBinding, CancellationToken), CapabilityScopeError> {
        self.owner
            .workspace_operation_authority(self.operation)
            .map_err(|_| CapabilityScopeError::WorkspaceOperationUnavailable)
    }
}

#[cfg(test)]
#[path = "workspace_access_tests.rs"]
mod tests;
