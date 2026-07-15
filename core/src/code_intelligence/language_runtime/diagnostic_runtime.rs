use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use lsp_types::{
    DocumentDiagnosticParams, DocumentDiagnosticReport, DocumentDiagnosticReportResult,
    PartialResultParams, PublishDiagnosticsParams, TextDocumentIdentifier, Uri,
    WorkDoneProgressParams,
};
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use super::super::{
    diagnostics::{DiagnosticsLookup, DiagnosticsStore},
    document_store::DocumentStore,
    lsp::{client::LspClientError, mapping, message::JsonRpcNotification},
    CodeDiagnostic, CodeQueryResult, DocumentSnapshot,
};
use super::{
    protocol::{bound_items, ensure_not_cancelled},
    LanguageRuntime, LanguageRuntimeError,
};
use crate::workspace::WorkspacePath;

const MAX_DIAGNOSTICS_PER_DOCUMENT: usize = 1_000;

#[derive(Debug, Default)]
pub(super) struct DiagnosticRuntimeState {
    result_ids: HashMap<WorkspacePath, String>,
    truncated: HashSet<WorkspacePath>,
}

impl LanguageRuntime {
    pub(super) async fn pull_diagnostics(
        &self,
        path: &WorkspacePath,
        uri: Uri,
        snapshot: DocumentSnapshot,
        cancellation: CancellationToken,
    ) -> Result<CodeQueryResult<CodeDiagnostic>, LanguageRuntimeError> {
        let previous_result_id = self
            .diagnostic_state
            .lock()
            .await
            .result_ids
            .get(path)
            .cloned();
        let response: DocumentDiagnosticReportResult = self
            .request_typed(
                "diagnostics",
                "textDocument/diagnostic",
                DocumentDiagnosticParams {
                    text_document: TextDocumentIdentifier::new(uri.clone()),
                    identifier: None,
                    previous_result_id: previous_result_id.clone(),
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                },
                cancellation.clone(),
            )
            .await?;
        ensure_not_cancelled(&cancellation)?;

        match response {
            DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(report)) => {
                let full = report.full_document_diagnostic_report;
                let items = mapping::map_diagnostics(&self.canonical_root, &uri, full.items)
                    .await
                    .map_err(|source| LanguageRuntimeError::Mapping {
                        operation: "diagnostics",
                        source,
                    })?;
                ensure_not_cancelled(&cancellation)?;
                let (items, truncated) = bound_items(items, MAX_DIAGNOSTICS_PER_DOCUMENT);
                let snapshot = self.documents.complete_query(path, snapshot).await;
                if !snapshot.stale {
                    let _update = self.diagnostic_updates.lock().await;
                    self.diagnostics
                        .publish(path, items.clone(), Some(snapshot.revision))
                        .await;
                    let mut state = self.diagnostic_state.lock().await;
                    match full.result_id {
                        Some(result_id) => {
                            state.result_ids.insert(path.clone(), result_id);
                        }
                        None => {
                            state.result_ids.remove(path);
                        }
                    }
                    set_truncated(&mut state, path, truncated);
                }
                Ok(self.query_result(items, truncated, Some(snapshot)))
            }
            DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(report)) => {
                if previous_result_id.is_none() {
                    return Err(LanguageRuntimeError::PendingDiagnostics { path: path.clone() });
                }
                let result_id = report.unchanged_document_diagnostic_report.result_id;
                let _update = self.diagnostic_updates.lock().await;
                let (items, revision) = match self.diagnostics.lookup(path).await {
                    DiagnosticsLookup::NotReceived => {
                        return Err(LanguageRuntimeError::PendingDiagnostics {
                            path: path.clone(),
                        });
                    }
                    DiagnosticsLookup::Received {
                        diagnostics,
                        revision,
                    } => (diagnostics, revision),
                };
                if revision.is_some_and(|revision| revision != snapshot.revision) {
                    return Err(LanguageRuntimeError::PendingDiagnostics { path: path.clone() });
                }
                let mut state = self.diagnostic_state.lock().await;
                state.result_ids.insert(path.clone(), result_id);
                let truncated = state.truncated.contains(path);
                drop(state);
                drop(_update);
                let snapshot = self.documents.complete_query(path, snapshot).await;
                Ok(self.query_result(items, truncated, Some(snapshot)))
            }
            DocumentDiagnosticReportResult::Partial(_) => Err(LanguageRuntimeError::Client {
                operation: "diagnostics",
                source: LspClientError::Protocol {
                    message: "server returned a partial diagnostics object as the final response"
                        .to_owned(),
                },
            }),
        }
    }

    pub(super) async fn published_diagnostics(
        &self,
        path: &WorkspacePath,
        snapshot: DocumentSnapshot,
        cancellation: &CancellationToken,
    ) -> Result<CodeQueryResult<CodeDiagnostic>, LanguageRuntimeError> {
        ensure_not_cancelled(cancellation)?;
        let wait_for_publish = async {
            loop {
                // Register before lookup so a publish between lookup and wait
                // cannot be lost. The update lock is released before waiting.
                let notified = self.diagnostics.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let received = {
                    let _update = self.diagnostic_updates.lock().await;
                    match self.diagnostics.lookup(path).await {
                        DiagnosticsLookup::NotReceived => None,
                        DiagnosticsLookup::Received {
                            revision: Some(revision),
                            ..
                        } if revision != snapshot.revision => None,
                        DiagnosticsLookup::Received { diagnostics, .. } => {
                            let truncated =
                                self.diagnostic_state.lock().await.truncated.contains(path);
                            Some((diagnostics, truncated))
                        }
                    }
                };
                if let Some(received) = received {
                    return received;
                }
                notified.await;
            }
        };
        let (items, truncated) = tokio::select! {
            _ = cancellation.cancelled() => return Err(LanguageRuntimeError::Cancelled),
            result = tokio::time::timeout(self.timeout, wait_for_publish) => {
                result.map_err(|_| LanguageRuntimeError::Timeout {
                    operation: "diagnostics",
                    duration: self.timeout,
                })?
            }
        };
        ensure_not_cancelled(cancellation)?;
        let snapshot = self.documents.complete_query(path, snapshot).await;
        Ok(self.query_result(items, truncated, Some(snapshot)))
    }

    pub(super) async fn clear_diagnostics(&self, path: &WorkspacePath) {
        let _update = self.diagnostic_updates.lock().await;
        self.diagnostics.invalidate(path).await;
        let mut state = self.diagnostic_state.lock().await;
        state.result_ids.remove(path);
        state.truncated.remove(path);
    }
}

pub(super) async fn consume_notifications(
    mut notifications: mpsc::Receiver<JsonRpcNotification>,
    canonical_root: PathBuf,
    documents: Arc<DocumentStore>,
    diagnostics: Arc<DiagnosticsStore>,
    diagnostic_state: Arc<Mutex<DiagnosticRuntimeState>>,
    diagnostic_updates: Arc<Mutex<()>>,
) {
    while let Some(notification) = notifications.recv().await {
        if notification.method != "textDocument/publishDiagnostics" {
            continue;
        }
        let Some(params) = notification.params else {
            continue;
        };
        let Ok(params) = serde_json::from_value::<PublishDiagnosticsParams>(params) else {
            continue;
        };
        let Ok(mapped) =
            mapping::map_diagnostics(&canonical_root, &params.uri, params.diagnostics).await
        else {
            continue;
        };
        let path = match mapped.first() {
            Some(diagnostic) => diagnostic.location.path.clone(),
            None => match mapping::file_uri_to_workspace_path(&canonical_root, &params.uri).await {
                Ok(path) => path,
                Err(_) => continue,
            },
        };
        let (mapped, truncated) = bound_items(mapped, MAX_DIAGNOSTICS_PER_DOCUMENT);
        let _update = diagnostic_updates.lock().await;
        let revision = match params.version {
            Some(version) => match documents.revision_for_lsp_version(&path, version).await {
                Some(revision) => Some(revision),
                None => continue,
            },
            None => None,
        };
        diagnostics.publish(&path, mapped, revision).await;
        let mut state = diagnostic_state.lock().await;
        set_truncated(&mut state, &path, truncated);
    }
}

fn set_truncated(state: &mut DiagnosticRuntimeState, path: &WorkspacePath, truncated: bool) {
    if truncated {
        state.truncated.insert(path.clone());
    } else {
        state.truncated.remove(path);
    }
}
