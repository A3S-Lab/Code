//! Workspace-scoped semantic code intelligence contracts.
//!
//! This module defines read-only, language-aware queries over saved workspace
//! documents. Concrete runtimes live behind [`WorkspaceCodeIntelligence`] so
//! TUI, web, and agent integrations can share one source of semantic results.

pub(crate) mod diagnostics;
pub(crate) mod document_store;
mod error;
pub(crate) mod language_profile;
pub(crate) mod language_runtime;
mod local_provider;
pub(crate) mod lsp;
pub(crate) mod project_layout;
pub(crate) mod registry;
mod service;
mod types;
pub(crate) mod workspace_runtime;

pub use error::{CodeIntelligenceError, CodeIntelligenceResult};
pub use local_provider::LocalCodeIntelligence;
pub use service::WorkspaceCodeIntelligence;
pub use types::{
    CodeDiagnostic, CodeDiagnosticSeverity, CodeIntelligenceCapabilities,
    CodeIntelligenceLanguageStatus, CodeIntelligenceState, CodeIntelligenceStatus, CodeLocation,
    CodePosition, CodeQueryResult, CodeRange, CodeSymbolKind, DocumentRevision, DocumentSnapshot,
    DocumentSymbol, LanguageId, NavigationKind, SymbolInformation,
};
