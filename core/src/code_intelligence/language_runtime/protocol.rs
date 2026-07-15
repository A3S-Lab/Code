use std::path::Path;

use lsp_types::{
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, FileChangeType, FileEvent,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem, Uri,
    VersionedTextDocumentIdentifier,
};
use serde::{de::DeserializeOwned, Serialize};
use tokio_util::sync::CancellationToken;

use super::super::{
    document_store::DocumentObservationKind, lsp::initialize::ServerTextSyncMode, CodeQueryResult,
    DocumentSnapshot, NavigationKind,
};
use super::{
    paths::{existing_workspace_file_uri, validate_workspace_path, workspace_file_uri},
    LanguageRuntime, LanguageRuntimeError,
};
use crate::{
    language::LanguageCatalog,
    workspace::{WorkspaceFileChange, WorkspaceFileChangeKind, WorkspacePath},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DocumentSyncStep {
    Close,
    Open,
    Change,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DocumentSyncPlan {
    pub(super) steps: &'static [DocumentSyncStep],
    pub(super) send_did_save: bool,
}

impl LanguageRuntime {
    pub(crate) async fn notify_file_changes(
        &self,
        changes: &[WorkspaceFileChange],
    ) -> Result<(), LanguageRuntimeError> {
        let mut events = Vec::with_capacity(changes.len());
        for change in changes {
            validate_workspace_path(&change.path)?;
            events.push(FileEvent::new(
                workspace_file_uri(&self.canonical_root, &change.path)?,
                file_change_type(change.kind),
            ));
        }
        if events.is_empty() {
            return Ok(());
        }

        let cancellation = CancellationToken::new();
        let _sync = self.document_sync.lock().await;
        for change in changes {
            self.invalidate_document_locked(&change.path, &cancellation)
                .await?;
        }
        self.notify_typed(
            "file_changes",
            "workspace/didChangeWatchedFiles",
            DidChangeWatchedFilesParams { changes: events },
            &cancellation,
        )
        .await
    }

    pub(crate) async fn invalidate_document(
        &self,
        path: &WorkspacePath,
    ) -> Result<(), LanguageRuntimeError> {
        validate_workspace_path(path)?;
        let cancellation = CancellationToken::new();
        let _sync = self.document_sync.lock().await;
        self.invalidate_document_locked(path, &cancellation).await
    }

    pub(super) async fn sync_saved_document(
        &self,
        path: &WorkspacePath,
        saved_content: &str,
        cancellation: &CancellationToken,
    ) -> Result<(Uri, DocumentSnapshot), LanguageRuntimeError> {
        validate_workspace_path(path)?;
        let uri = existing_workspace_file_uri(&self.canonical_root, path).await?;
        let _sync = self.document_sync.lock().await;
        let observation = self.documents.observe(path, saved_content).await?;

        if let Some(evicted) = &observation.evicted {
            if self.open_documents.lock().await.contains(evicted) {
                if let Err(error) = self.close_document(evicted, cancellation).await {
                    self.documents.invalidate(path).await;
                    self.open_documents.lock().await.remove(path);
                    return Err(error);
                }
            }
            self.open_documents.lock().await.remove(evicted);
            self.clear_diagnostics(evicted).await;
        }

        let was_open = self.open_documents.lock().await.contains(path);
        if matches!(
            observation.kind,
            DocumentObservationKind::Changed | DocumentObservationKind::Reopened
        ) {
            self.clear_diagnostics(path).await;
        }
        let plan = document_sync_plan(
            self.initialized.text_sync_mode,
            self.initialized.supports_open_close,
            self.initialized.supports_did_save,
            observation.kind,
            was_open,
        );
        for step in plan.steps {
            let result = match step {
                DocumentSyncStep::Close => self.close_document(path, cancellation).await,
                DocumentSyncStep::Open => {
                    self.open_document(
                        path,
                        &uri,
                        observation.lsp_version,
                        saved_content,
                        cancellation,
                    )
                    .await
                }
                DocumentSyncStep::Change => {
                    self.change_document(&uri, observation.lsp_version, saved_content, cancellation)
                        .await
                }
            };
            if let Err(error) = result {
                self.documents.invalidate(path).await;
                self.open_documents.lock().await.remove(path);
                self.clear_diagnostics(path).await;
                return Err(error);
            }
            match step {
                DocumentSyncStep::Close => {
                    self.open_documents.lock().await.remove(path);
                }
                DocumentSyncStep::Open | DocumentSyncStep::Change => {
                    self.open_documents.lock().await.insert(path.clone());
                }
            }
        }
        if plan.send_did_save {
            self.save_document(&uri, cancellation).await?;
        }
        Ok((uri, observation.snapshot))
    }

    async fn open_document(
        &self,
        path: &WorkspacePath,
        uri: &Uri,
        version: i32,
        content: &str,
        cancellation: &CancellationToken,
    ) -> Result<(), LanguageRuntimeError> {
        let language_id = LanguageCatalog::id_for_path(Path::new(path.as_str()))
            .ok_or_else(|| LanguageRuntimeError::UnsupportedPath { path: path.clone() })?;
        self.notify_typed(
            "did_open",
            "textDocument/didOpen",
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    uri.clone(),
                    language_id.to_owned(),
                    version,
                    content.to_owned(),
                ),
            },
            cancellation,
        )
        .await
    }

    async fn change_document(
        &self,
        uri: &Uri,
        version: i32,
        content: &str,
        cancellation: &CancellationToken,
    ) -> Result<(), LanguageRuntimeError> {
        self.notify_typed(
            "did_change",
            "textDocument/didChange",
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier::new(uri.clone(), version),
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: content.to_owned(),
                }],
            },
            cancellation,
        )
        .await
    }

    async fn close_document(
        &self,
        path: &WorkspacePath,
        cancellation: &CancellationToken,
    ) -> Result<(), LanguageRuntimeError> {
        self.notify_typed(
            "did_close",
            "textDocument/didClose",
            DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier::new(workspace_file_uri(
                    &self.canonical_root,
                    path,
                )?),
            },
            cancellation,
        )
        .await
    }

    async fn save_document(
        &self,
        uri: &Uri,
        cancellation: &CancellationToken,
    ) -> Result<(), LanguageRuntimeError> {
        self.notify_typed(
            "did_save",
            "textDocument/didSave",
            did_save_params(uri),
            cancellation,
        )
        .await
    }

    pub(super) async fn invalidate_document_locked(
        &self,
        path: &WorkspacePath,
        cancellation: &CancellationToken,
    ) -> Result<(), LanguageRuntimeError> {
        self.documents.invalidate(path).await;
        self.navigation_revisions.lock().await.remove(path);
        if self.open_documents.lock().await.contains(path) {
            self.close_document(path, cancellation).await?;
            self.open_documents.lock().await.remove(path);
        }
        self.clear_diagnostics(path).await;
        Ok(())
    }

    pub(super) async fn request_typed<P, R>(
        &self,
        operation: &'static str,
        method: &'static str,
        params: P,
        cancellation: CancellationToken,
    ) -> Result<R, LanguageRuntimeError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let params = serde_json::to_value(params)
            .map_err(|source| LanguageRuntimeError::Serde { operation, source })?;
        let response = self
            .process
            .client()
            .request(method, Some(params), cancellation, self.timeout)
            .await
            .map_err(|source| LanguageRuntimeError::Client { operation, source })?;
        serde_json::from_value(response)
            .map_err(|source| LanguageRuntimeError::Serde { operation, source })
    }

    async fn notify_typed<P: Serialize>(
        &self,
        operation: &'static str,
        method: &'static str,
        params: P,
        cancellation: &CancellationToken,
    ) -> Result<(), LanguageRuntimeError> {
        let params = serde_json::to_value(params)
            .map_err(|source| LanguageRuntimeError::Serde { operation, source })?;
        let client = self.process.client();
        let notify = client.notify(method, Some(params));
        tokio::pin!(notify);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(LanguageRuntimeError::Cancelled),
            result = tokio::time::timeout(self.timeout, &mut notify) => match result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(source)) => Err(LanguageRuntimeError::Client { operation, source }),
                Err(_) => Err(LanguageRuntimeError::Timeout {
                    operation,
                    duration: self.timeout,
                }),
            }
        }
    }

    pub(super) async fn document_result<T>(
        &self,
        path: &WorkspacePath,
        snapshot: DocumentSnapshot,
        items: Vec<T>,
        truncated: bool,
    ) -> CodeQueryResult<T> {
        let snapshot = self.documents.complete_query(path, snapshot).await;
        self.query_result(items, truncated, Some(snapshot))
    }

    pub(super) fn query_result<T>(
        &self,
        items: Vec<T>,
        truncated: bool,
        document: Option<DocumentSnapshot>,
    ) -> CodeQueryResult<T> {
        CodeQueryResult {
            items,
            truncated,
            workspace_revision: self.layout.workspace_revision,
            document,
        }
    }

    pub(super) fn require_path(&self, path: &WorkspacePath) -> Result<(), LanguageRuntimeError> {
        validate_workspace_path(path)?;
        if self.supports_path(path) {
            Ok(())
        } else {
            Err(LanguageRuntimeError::UnsupportedPath { path: path.clone() })
        }
    }

    pub(super) fn require_capability(
        &self,
        supported: bool,
        operation: &'static str,
    ) -> Result<(), LanguageRuntimeError> {
        if supported {
            Ok(())
        } else {
            Err(LanguageRuntimeError::Unsupported { operation })
        }
    }

    pub(super) fn require_navigation(
        &self,
        kind: NavigationKind,
    ) -> Result<(), LanguageRuntimeError> {
        let supported = match kind {
            NavigationKind::Definition => self.initialized.capabilities.definition,
            NavigationKind::Declaration => self.initialized.capabilities.declaration,
            NavigationKind::References => self.initialized.capabilities.references,
            NavigationKind::Implementations => self.initialized.capabilities.implementations,
        };
        self.require_capability(supported, navigation_request(kind).0)
    }
}

pub(super) fn navigation_request(kind: NavigationKind) -> (&'static str, &'static str) {
    match kind {
        NavigationKind::Definition => ("definition", "textDocument/definition"),
        NavigationKind::Declaration => ("declaration", "textDocument/declaration"),
        NavigationKind::References => ("references", "textDocument/references"),
        NavigationKind::Implementations => ("implementations", "textDocument/implementation"),
    }
}

fn file_change_type(kind: WorkspaceFileChangeKind) -> FileChangeType {
    match kind {
        WorkspaceFileChangeKind::Created => FileChangeType::CREATED,
        WorkspaceFileChangeKind::Changed => FileChangeType::CHANGED,
        WorkspaceFileChangeKind::Deleted => FileChangeType::DELETED,
    }
}

pub(super) fn document_sync_plan(
    mode: ServerTextSyncMode,
    supports_open_close: bool,
    supports_did_save: bool,
    observation: DocumentObservationKind,
    already_open: bool,
) -> DocumentSyncPlan {
    use DocumentObservationKind::{Changed, Opened, Reopened, Unchanged};
    use DocumentSyncStep::{Change, Close, Open};

    let steps: &'static [DocumentSyncStep] =
        if !supports_open_close || mode == ServerTextSyncMode::None {
            &[]
        } else {
            match (mode, observation, already_open) {
                (_, Opened, false) | (_, Unchanged, false) => &[Open],
                (_, Opened, true) | (_, Reopened, _) => &[Close, Open],
                (ServerTextSyncMode::Full, Changed, true) => &[Change],
                (ServerTextSyncMode::Full, Changed, false) => &[Open],
                (ServerTextSyncMode::Incremental, Changed, _) => &[Close, Open],
                (_, Unchanged, true) => &[],
                (ServerTextSyncMode::None, _, _) => unreachable!(),
            }
        };
    DocumentSyncPlan {
        steps,
        send_did_save: supports_did_save,
    }
}

pub(super) fn did_save_params(uri: &Uri) -> DidSaveTextDocumentParams {
    DidSaveTextDocumentParams {
        text_document: TextDocumentIdentifier::new(uri.clone()),
        // The negotiated capability currently records support, but not an
        // include-text request. Omitting text avoids duplicating saved source.
        text: None,
    }
}

pub(super) fn bound_items<T>(mut items: Vec<T>, limit: usize) -> (Vec<T>, bool) {
    let truncated = items.len() > limit;
    items.truncate(limit);
    (items, truncated)
}

pub(super) fn ensure_not_cancelled(
    cancellation: &CancellationToken,
) -> Result<(), LanguageRuntimeError> {
    if cancellation.is_cancelled() {
        Err(LanguageRuntimeError::Cancelled)
    } else {
        Ok(())
    }
}
