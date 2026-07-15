use super::{
    CodeDiagnostic, CodeIntelligenceResult, CodeIntelligenceStatus, CodeLocation, CodePosition,
    CodeQueryResult, DocumentSymbol, NavigationKind, SymbolInformation,
};
use crate::workspace::WorkspacePath;
use async_trait::async_trait;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Workspace-scoped provider for read-only semantic code queries.
///
/// Implementations operate on saved documents. Unsaved editor buffers are
/// intentionally outside this contract so independent views cannot overwrite
/// one another's semantic state.
#[async_trait]
pub trait WorkspaceCodeIntelligence: Send + Sync {
    /// Subscribe to runtime lifecycle and capability changes.
    fn subscribe_status(&self) -> watch::Receiver<CodeIntelligenceStatus>;

    /// Return the latest status snapshot without waiting for a state change.
    fn status(&self) -> CodeIntelligenceStatus {
        let receiver = self.subscribe_status();
        let status = receiver.borrow().clone();
        status
    }

    /// Return the hierarchical symbol outline for a saved document.
    async fn document_symbols(
        &self,
        path: &WorkspacePath,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<DocumentSymbol>>;

    /// Search symbols across the workspace, bounded by `limit`.
    async fn search_symbols(
        &self,
        query: &str,
        limit: usize,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<SymbolInformation>>;

    /// Resolve one semantic navigation operation from a saved document.
    async fn navigate(
        &self,
        kind: NavigationKind,
        path: &WorkspacePath,
        position: CodePosition,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<CodeLocation>>;

    /// Pull diagnostics for one saved document, or for the workspace when
    /// `path` is `None`.
    async fn diagnostics(
        &self,
        path: Option<&WorkspacePath>,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<CodeDiagnostic>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync + ?Sized>() {}

    #[test]
    fn service_trait_is_object_safe_send_and_sync() {
        assert_send_sync::<dyn WorkspaceCodeIntelligence>();
    }
}
