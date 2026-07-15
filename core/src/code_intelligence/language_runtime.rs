//! One saved-document language runtime.

mod diagnostic_runtime;
#[cfg(test)]
mod integration_tests;
mod paths;
mod protocol;
#[cfg(test)]
mod tests;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use lsp_types::{
    DocumentSymbolParams, DocumentSymbolResponse as LspDocumentSymbolResponse,
    GotoDefinitionParams, GotoDefinitionResponse, Location, PartialResultParams, Position,
    ReferenceContext, ReferenceParams, TextDocumentIdentifier, TextDocumentPositionParams,
    WorkDoneProgressParams, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use tokio::sync::watch;
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use self::{
    diagnostic_runtime::{consume_notifications, DiagnosticRuntimeState},
    paths::{directory_url, valid_utf16_position, validate_canonical_root, workspace_folders},
    protocol::{bound_items, ensure_not_cancelled, navigation_request},
};
use super::{
    diagnostics::DiagnosticsStore,
    document_store::{DocumentStore, DocumentStoreError},
    language_profile::LanguageServerProfile,
    lsp::{
        client::LspClientError,
        initialize::{initialize, InitializeConfig, InitializedServer},
        mapping::{self, MappingError},
        process::{LspProcess, LspProcessError},
        router::{ServerRequestRouter, ServerRequestRouterConfig, WorkspaceSettings},
    },
    project_layout::ProjectLayout,
    CodeDiagnostic, CodeIntelligenceCapabilities, CodeLocation, CodePosition, CodeQueryResult,
    DocumentSymbol, NavigationKind, SymbolInformation,
};
use crate::workspace::WorkspacePath;

const MAX_NAVIGATION_RESULTS: usize = 1_000;
const MAX_DOCUMENT_SYMBOLS: usize = 2_000;

/// Failure from one initialized language runtime.
#[derive(Debug, thiserror::Error)]
pub(crate) enum LanguageRuntimeError {
    #[error("invalid canonical workspace root {root:?}: {message}")]
    InvalidRoot { root: PathBuf, message: String },

    #[error("invalid normalized workspace path {path:?}: {message}")]
    InvalidPath {
        path: WorkspacePath,
        message: String,
    },

    #[error("language runtime does not support path {path:?}")]
    UnsupportedPath { path: WorkspacePath },

    #[error("language runtime does not support operation '{operation}'")]
    Unsupported { operation: &'static str },

    #[error("invalid UTF-16 position {position:?} for saved document {path:?}")]
    InvalidPosition {
        path: WorkspacePath,
        position: CodePosition,
    },

    #[error("diagnostics for saved document {path:?} have not been received yet")]
    PendingDiagnostics { path: WorkspacePath },

    #[error("language runtime operation was cancelled")]
    Cancelled,

    #[error("language runtime operation '{operation}' timed out after {duration:?}")]
    Timeout {
        operation: &'static str,
        duration: Duration,
    },

    #[error("language server process failed while attempting to {operation}: {source}")]
    Process {
        operation: &'static str,
        #[source]
        source: LspProcessError,
    },

    #[error("language server client failed during '{operation}': {source}")]
    Client {
        operation: &'static str,
        #[source]
        source: LspClientError,
    },

    #[error("language server response mapping failed during '{operation}': {source}")]
    Mapping {
        operation: &'static str,
        #[source]
        source: MappingError,
    },

    #[error("language server JSON failed during '{operation}': {source}")]
    Serde {
        operation: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("saved-document metadata failed: {0}")]
    DocumentStore(#[from] DocumentStoreError),
}

/// An initialized process serving one language profile and workspace layout.
pub(crate) struct LanguageRuntime {
    profile: LanguageServerProfile,
    canonical_root: PathBuf,
    layout: ProjectLayout,
    process: LspProcess,
    initialized: InitializedServer,
    documents: Arc<DocumentStore>,
    diagnostics: Arc<DiagnosticsStore>,
    diagnostic_state: Arc<Mutex<DiagnosticRuntimeState>>,
    diagnostic_updates: Arc<Mutex<()>>,
    open_documents: Mutex<HashSet<WorkspacePath>>,
    document_sync: Mutex<()>,
    notification_task: Mutex<Option<JoinHandle<()>>>,
    timeout: Duration,
}

impl std::fmt::Debug for LanguageRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LanguageRuntime")
            .field("profile", &self.profile.id())
            .field("canonical_root", &self.canonical_root)
            .field("layout_hash", &self.layout.layout_hash)
            .field("process", &self.process)
            .field("capabilities", &self.initialized.capabilities)
            .finish_non_exhaustive()
    }
}

impl LanguageRuntime {
    pub(crate) async fn start(
        profile: LanguageServerProfile,
        canonical_root: PathBuf,
        layout: ProjectLayout,
        documents: Arc<DocumentStore>,
        diagnostics: Arc<DiagnosticsStore>,
        timeout: Duration,
    ) -> Result<Self, LanguageRuntimeError> {
        validate_canonical_root(&canonical_root).await?;
        let root_url = directory_url(&canonical_root)?;
        let workspace_folders = workspace_folders(&profile, &canonical_root, &layout).await?;
        let router = ServerRequestRouter::new(ServerRequestRouterConfig::new(
            workspace_folders.clone(),
            WorkspaceSettings::new(profile.workspace_settings(&canonical_root, &layout)),
        ));
        let process =
            LspProcess::spawn(profile.command(), &canonical_root, router).map_err(|source| {
                LanguageRuntimeError::Process {
                    operation: "start",
                    source,
                }
            })?;
        let client = process.client();
        let notifications =
            client
                .take_notifications()
                .ok_or_else(|| LanguageRuntimeError::Client {
                    operation: "take_notifications",
                    source: LspClientError::Protocol {
                        message: "language server notification stream is already owned".to_owned(),
                    },
                })?;
        let diagnostic_state = Arc::new(Mutex::new(DiagnosticRuntimeState::default()));
        let diagnostic_updates = Arc::new(Mutex::new(()));
        let notification_task = tokio::spawn(consume_notifications(
            notifications,
            canonical_root.clone(),
            Arc::clone(&documents),
            Arc::clone(&diagnostics),
            Arc::clone(&diagnostic_state),
            Arc::clone(&diagnostic_updates),
        ));

        let initialization_options = profile.initialization_options(&canonical_root, &layout);
        let initialization_options =
            (!initialization_options.is_null()).then_some(initialization_options);
        let config = InitializeConfig::new(
            root_url,
            workspace_folders,
            initialization_options,
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        );
        let initialized =
            match initialize(&client, &config, CancellationToken::new(), timeout).await {
                Ok(initialized) => initialized,
                Err(source) => {
                    let _ = process.shutdown(timeout, timeout).await;
                    notification_task.abort();
                    let _ = notification_task.await;
                    return Err(LanguageRuntimeError::Client {
                        operation: "initialize",
                        source,
                    });
                }
            };

        Ok(Self {
            profile,
            canonical_root,
            layout,
            process,
            initialized,
            documents,
            diagnostics,
            diagnostic_state,
            diagnostic_updates,
            open_documents: Mutex::new(HashSet::new()),
            document_sync: Mutex::new(()),
            notification_task: Mutex::new(Some(notification_task)),
            timeout,
        })
    }

    pub(crate) fn supports_path(&self, path: &WorkspacePath) -> bool {
        self.profile.supports_path(Path::new(path.as_str()))
    }

    pub(crate) fn capabilities(&self) -> CodeIntelligenceCapabilities {
        self.initialized.capabilities
    }

    pub(crate) async fn document_symbols(
        &self,
        path: &WorkspacePath,
        saved_content: &str,
        cancellation: CancellationToken,
    ) -> Result<CodeQueryResult<DocumentSymbol>, LanguageRuntimeError> {
        self.require_path(path)?;
        self.require_capability(
            self.initialized.capabilities.document_symbols,
            "document_symbols",
        )?;
        let (uri, snapshot) = self
            .sync_saved_document(path, saved_content, &cancellation)
            .await?;
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier::new(uri),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let response: Option<LspDocumentSymbolResponse> = self
            .request_typed(
                "document_symbols",
                "textDocument/documentSymbol",
                params,
                cancellation.clone(),
            )
            .await?;
        ensure_not_cancelled(&cancellation)?;
        let items = match response {
            Some(response) => mapping::map_document_symbol_response(&self.canonical_root, response)
                .await
                .map_err(|source| LanguageRuntimeError::Mapping {
                    operation: "document_symbols",
                    source,
                })?,
            None => Vec::new(),
        };
        ensure_not_cancelled(&cancellation)?;
        let (items, truncated) = bound_document_symbols(items, MAX_DOCUMENT_SYMBOLS);
        Ok(self.document_result(path, snapshot, items, truncated).await)
    }

    pub(crate) async fn search_symbols(
        &self,
        query: &str,
        limit: usize,
        cancellation: CancellationToken,
    ) -> Result<CodeQueryResult<SymbolInformation>, LanguageRuntimeError> {
        self.require_capability(
            self.initialized.capabilities.workspace_symbols,
            "search_symbols",
        )?;
        let params = WorkspaceSymbolParams {
            query: query.to_owned(),
            partial_result_params: PartialResultParams::default(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let response: Option<WorkspaceSymbolResponse> = self
            .request_typed(
                "search_symbols",
                "workspace/symbol",
                params,
                cancellation.clone(),
            )
            .await?;
        ensure_not_cancelled(&cancellation)?;
        let items = match response {
            Some(response) => {
                mapping::map_workspace_symbol_response(&self.canonical_root, response)
                    .await
                    .map_err(|source| LanguageRuntimeError::Mapping {
                        operation: "search_symbols",
                        source,
                    })?
            }
            None => Vec::new(),
        };
        ensure_not_cancelled(&cancellation)?;
        let (items, truncated) = bound_items(items, limit);
        Ok(CodeQueryResult {
            items,
            truncated,
            workspace_revision: self.layout.workspace_revision,
            document: None,
        })
    }

    pub(crate) async fn navigate(
        &self,
        kind: NavigationKind,
        path: &WorkspacePath,
        position: CodePosition,
        saved_content: &str,
        cancellation: CancellationToken,
    ) -> Result<CodeQueryResult<CodeLocation>, LanguageRuntimeError> {
        self.require_path(path)?;
        self.require_navigation(kind)?;
        if !valid_utf16_position(saved_content, position) {
            return Err(LanguageRuntimeError::InvalidPosition {
                path: path.clone(),
                position,
            });
        }
        let (uri, snapshot) = self
            .sync_saved_document(path, saved_content, &cancellation)
            .await?;
        let text_position = TextDocumentPositionParams::new(
            TextDocumentIdentifier::new(uri),
            Position::new(position.line, position.character),
        );
        let items = match kind {
            NavigationKind::References => {
                let response: Option<Vec<Location>> = self
                    .request_typed(
                        "references",
                        "textDocument/references",
                        ReferenceParams {
                            text_document_position: text_position,
                            work_done_progress_params: WorkDoneProgressParams::default(),
                            partial_result_params: PartialResultParams::default(),
                            context: ReferenceContext {
                                include_declaration: true,
                            },
                        },
                        cancellation.clone(),
                    )
                    .await?;
                let mut mapped = Vec::new();
                for location in response.unwrap_or_default() {
                    ensure_not_cancelled(&cancellation)?;
                    mapped.push(
                        mapping::map_location(&self.canonical_root, location)
                            .await
                            .map_err(|source| LanguageRuntimeError::Mapping {
                                operation: "references",
                                source,
                            })?,
                    );
                }
                mapped
            }
            NavigationKind::Definition
            | NavigationKind::Declaration
            | NavigationKind::Implementations => {
                let (operation, method) = navigation_request(kind);
                let response: Option<GotoDefinitionResponse> = self
                    .request_typed(
                        operation,
                        method,
                        GotoDefinitionParams {
                            text_document_position_params: text_position,
                            work_done_progress_params: WorkDoneProgressParams::default(),
                            partial_result_params: PartialResultParams::default(),
                        },
                        cancellation.clone(),
                    )
                    .await?;
                mapping::map_definition_response(&self.canonical_root, response)
                    .await
                    .map_err(|source| LanguageRuntimeError::Mapping { operation, source })?
            }
        };
        ensure_not_cancelled(&cancellation)?;
        let (items, truncated) = bound_items(items, MAX_NAVIGATION_RESULTS);
        Ok(self.document_result(path, snapshot, items, truncated).await)
    }

    pub(crate) async fn diagnostics(
        &self,
        path: &WorkspacePath,
        saved_content: &str,
        cancellation: CancellationToken,
    ) -> Result<CodeQueryResult<CodeDiagnostic>, LanguageRuntimeError> {
        self.require_path(path)?;
        self.require_capability(self.initialized.capabilities.diagnostics, "diagnostics")?;
        let (uri, snapshot) = self
            .sync_saved_document(path, saved_content, &cancellation)
            .await?;
        if self.initialized.supports_pull_diagnostics {
            self.pull_diagnostics(path, uri, snapshot, cancellation)
                .await
        } else {
            self.published_diagnostics(path, snapshot, &cancellation)
                .await
        }
    }

    pub(crate) async fn shutdown(&self) -> Result<(), LanguageRuntimeError> {
        let paths = {
            let _sync = self.document_sync.lock().await;
            self.open_documents
                .lock()
                .await
                .iter()
                .cloned()
                .collect::<Vec<_>>()
        };
        for path in paths {
            let _ = self.invalidate_document(&path).await;
        }

        let process_result = self.process.shutdown(self.timeout, self.timeout).await;
        if let Some(mut task) = self.notification_task.lock().await.take() {
            if tokio::time::timeout(self.timeout, &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
        process_result
            .map(|_| ())
            .map_err(|source| LanguageRuntimeError::Process {
                operation: "shutdown",
                source,
            })
    }

    pub(crate) fn unavailable_message(&self) -> Option<String> {
        use super::lsp::process::LspProcessState;

        let state = self.process.state();
        let message = match state {
            LspProcessState::Running if !self.process.client().is_closed() => return None,
            LspProcessState::Running => "the protocol connection closed unexpectedly".to_owned(),
            LspProcessState::Exited { code, forced } => format!(
                "the language server exited{}{}",
                code.map(|code| format!(" with code {code}"))
                    .unwrap_or_default(),
                if forced {
                    " after forced termination"
                } else {
                    ""
                }
            ),
            LspProcessState::Failed { message } => {
                format!("the language server process failed: {message}")
            }
        };
        let stderr = self.process.stderr_snapshot();
        Some(if stderr.trim().is_empty() {
            message
        } else {
            format!("{message}; stderr: {}", stderr.trim())
        })
    }

    pub(crate) fn subscribe_process_state(
        &self,
    ) -> watch::Receiver<super::lsp::process::LspProcessState> {
        self.process.subscribe_state()
    }
}

/// Bound a hierarchical document outline by total node count while retaining
/// the server's preorder, each retained node's ancestors, and child nesting.
fn bound_document_symbols(
    symbols: Vec<DocumentSymbol>,
    limit: usize,
) -> (Vec<DocumentSymbol>, bool) {
    fn retain_preorder(
        symbols: Vec<DocumentSymbol>,
        remaining: &mut usize,
        truncated: &mut bool,
    ) -> Vec<DocumentSymbol> {
        let mut retained = Vec::new();
        for mut symbol in symbols {
            if *remaining == 0 {
                *truncated = true;
                break;
            }
            *remaining -= 1;
            symbol.children = retain_preorder(symbol.children, remaining, truncated);
            retained.push(symbol);
        }
        retained
    }

    let mut remaining = limit;
    let mut truncated = false;
    let symbols = retain_preorder(symbols, &mut remaining, &mut truncated);
    (symbols, truncated)
}

impl Drop for LanguageRuntime {
    fn drop(&mut self) {
        self.process.force_kill();
        if let Ok(mut task) = self.notification_task.try_lock() {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
    }
}
