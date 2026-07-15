//! One lazily-started semantic runtime generation for a workspace layout.

#[cfg(test)]
mod integration_tests;
mod support;
#[cfg(test)]
mod tests;

use support::*;

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use futures::{stream::FuturesUnordered, StreamExt};
use tokio::sync::{watch, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use super::{
    diagnostics::DiagnosticsStore,
    document_store::DocumentStore,
    language_profile::LanguageServerProfile,
    language_runtime::{LanguageRuntime, LanguageRuntimeError},
    project_layout::{ProjectLanguageProfile, ProjectLayout},
    CodeDiagnostic, CodeIntelligenceCapabilities, CodeIntelligenceError,
    CodeIntelligenceLanguageStatus, CodeIntelligenceResult, CodeIntelligenceState,
    CodeIntelligenceStatus, CodeLocation, CodePosition, CodeQueryResult, DocumentSymbol,
    NavigationKind, SymbolInformation,
};
use crate::workspace::{
    LocalWorkspaceManifestSnapshot, WorkspaceFileChange, WorkspaceFileSystem, WorkspacePath,
};

const DOCUMENT_CAPACITY: usize = 256;
const DIAGNOSTICS_CAPACITY: usize = 512;
const WORKSPACE_DIAGNOSTIC_LIMIT: usize = 2_000;
const WORKSPACE_DIAGNOSTIC_DOCUMENT_LIMIT: usize = 128;
const WORKSPACE_DIAGNOSTIC_CONCURRENCY: usize = 8;
const MAX_SYMBOL_LIMIT: usize = 1_000;
const START_RETRY_DELAY: Duration = Duration::from_secs(2);

struct StartFailure {
    at: Instant,
    message: String,
}

enum SlotState {
    Dormant,
    Ready(Arc<LanguageRuntime>),
    Failed(StartFailure),
}

struct LanguageSlot {
    profile: LanguageServerProfile,
    relevant: AtomicBool,
    documents: Arc<DocumentStore>,
    state: Arc<Mutex<SlotState>>,
}

impl LanguageSlot {
    fn new(profile: LanguageServerProfile, relevant: bool, document_capacity: usize) -> Self {
        Self {
            profile,
            relevant: AtomicBool::new(relevant),
            documents: Arc::new(DocumentStore::new(document_capacity)),
            state: Arc::new(Mutex::new(SlotState::Dormant)),
        }
    }
}

/// Runtime processes and saved-document state for one stable layout hash.
pub(crate) struct WorkspaceRuntime {
    canonical_root: std::path::PathBuf,
    layout: ProjectLayout,
    file_system: Arc<dyn WorkspaceFileSystem>,
    diagnostics: Arc<DiagnosticsStore>,
    slots: Vec<LanguageSlot>,
    source_paths: RwLock<Vec<WorkspacePath>>,
    workspace_revision: AtomicU64,
    timeout: Duration,
    status: watch::Sender<CodeIntelligenceStatus>,
    shutting_down: AtomicBool,
    lifetime: CancellationToken,
}

impl std::fmt::Debug for WorkspaceRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceRuntime")
            .field("canonical_root", &self.canonical_root)
            .field("layout_hash", &self.layout.layout_hash)
            .field("workspace_revision", &self.workspace_revision())
            .field("language_slots", &self.slots.len())
            .finish_non_exhaustive()
    }
}

impl Drop for WorkspaceRuntime {
    fn drop(&mut self) {
        // Registry owners normally call `shutdown`, but a host can be dropped
        // during runtime teardown when awaiting is no longer possible. Wake
        // every process monitor so its final Arc is released and
        // `LanguageRuntime::drop` can terminate the child process.
        self.lifetime.cancel();
    }
}

impl WorkspaceRuntime {
    pub(crate) fn new(
        canonical_root: std::path::PathBuf,
        layout: ProjectLayout,
        snapshot: &LocalWorkspaceManifestSnapshot,
        file_system: Arc<dyn WorkspaceFileSystem>,
        timeout: Duration,
    ) -> Self {
        let profiles = LanguageServerProfile::built_in_defaults();
        Self::new_with_profile_set(
            canonical_root,
            layout,
            snapshot,
            file_system,
            timeout,
            profiles,
            DOCUMENT_CAPACITY,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_profiles(
        canonical_root: std::path::PathBuf,
        layout: ProjectLayout,
        snapshot: &LocalWorkspaceManifestSnapshot,
        file_system: Arc<dyn WorkspaceFileSystem>,
        timeout: Duration,
        profiles: Vec<LanguageServerProfile>,
    ) -> Self {
        Self::new_with_profile_set(
            canonical_root,
            layout,
            snapshot,
            file_system,
            timeout,
            profiles,
            DOCUMENT_CAPACITY,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_profiles_and_document_capacity(
        canonical_root: std::path::PathBuf,
        layout: ProjectLayout,
        snapshot: &LocalWorkspaceManifestSnapshot,
        file_system: Arc<dyn WorkspaceFileSystem>,
        timeout: Duration,
        profiles: Vec<LanguageServerProfile>,
        document_capacity: usize,
    ) -> Self {
        Self::new_with_profile_set(
            canonical_root,
            layout,
            snapshot,
            file_system,
            timeout,
            profiles,
            document_capacity,
        )
    }

    fn new_with_profile_set(
        canonical_root: std::path::PathBuf,
        layout: ProjectLayout,
        snapshot: &LocalWorkspaceManifestSnapshot,
        file_system: Arc<dyn WorkspaceFileSystem>,
        timeout: Duration,
        profiles: Vec<LanguageServerProfile>,
        document_capacity: usize,
    ) -> Self {
        let source_paths = supported_source_paths(snapshot, |path| {
            profiles.iter().any(|profile| profile.supports_path(path))
        });
        let slots = profiles
            .into_iter()
            .map(|profile| {
                let relevant = source_paths
                    .iter()
                    .any(|path| profile.supports_path(Path::new(path.as_str())));
                LanguageSlot::new(profile, relevant, document_capacity)
            })
            .collect();
        let (status, _) = watch::channel(CodeIntelligenceStatus {
            state: CodeIntelligenceState::Starting,
            message: Some("Code Intelligence starts language runtimes on demand".to_owned()),
            ..CodeIntelligenceStatus::default()
        });
        Self {
            canonical_root,
            workspace_revision: AtomicU64::new(snapshot.version),
            layout,
            file_system,
            diagnostics: Arc::new(DiagnosticsStore::new(DIAGNOSTICS_CAPACITY)),
            slots,
            source_paths: RwLock::new(source_paths),
            timeout,
            status,
            shutting_down: AtomicBool::new(false),
            lifetime: CancellationToken::new(),
        }
    }

    pub(crate) fn layout_hash(&self) -> u64 {
        self.layout.layout_hash
    }

    pub(crate) fn subscribe_status(&self) -> watch::Receiver<CodeIntelligenceStatus> {
        self.status.subscribe()
    }

    pub(crate) async fn update_snapshot(&self, snapshot: &LocalWorkspaceManifestSnapshot) {
        let source_paths = supported_source_paths(snapshot, |path| {
            self.slots
                .iter()
                .any(|slot| slot.profile.supports_path(path))
        });
        self.workspace_revision
            .store(snapshot.version, Ordering::Release);
        *self.source_paths.write().await = source_paths.clone();
        for slot in &self.slots {
            let relevant = source_paths
                .iter()
                .any(|path| slot.profile.supports_path(Path::new(path.as_str())));
            let was_relevant = slot.relevant.swap(relevant, Ordering::AcqRel);
            if was_relevant && !relevant {
                let runtime = {
                    let mut state = slot.state.lock().await;
                    match std::mem::replace(&mut *state, SlotState::Dormant) {
                        SlotState::Ready(runtime) => Some(runtime),
                        SlotState::Dormant | SlotState::Failed(_) => None,
                    }
                };
                if let Some(runtime) = runtime {
                    if let Err(error) = runtime.shutdown().await {
                        tracing::warn!(
                            language = %profile_language(slot.profile.id()),
                            error = %error,
                            "Code Intelligence could not stop an irrelevant language runtime"
                        );
                    }
                }
            }
        }
        self.refresh_status().await;
    }

    pub(crate) async fn document_symbols(
        &self,
        path: &WorkspacePath,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<DocumentSymbol>> {
        let content = self.read_saved(path, &cancellation).await?;
        let (profile, runtime) = self.runtime_for_path(path, &cancellation).await?;
        let mut result = runtime
            .document_symbols(path, &content, cancellation.clone())
            .await
            .map_err(|error| map_language_error(profile, error))?;
        self.complete_saved_query(path, &cancellation, &mut result)
            .await?;
        result.workspace_revision = self.workspace_revision();
        Ok(result)
    }

    pub(crate) async fn search_symbols(
        &self,
        query: &str,
        limit: usize,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<SymbolInformation>> {
        if cancellation.is_cancelled() {
            return Err(CodeIntelligenceError::Cancelled);
        }
        let limit = limit.min(MAX_SYMBOL_LIMIT);
        if limit == 0 {
            return Ok(self.workspace_result(Vec::new(), false));
        }

        let mut queries = FuturesUnordered::new();
        for slot in self
            .slots
            .iter()
            .filter(|slot| slot.relevant.load(Ordering::Acquire))
        {
            let cancellation = cancellation.clone();
            queries.push(async move {
                let runtime = self.ensure_runtime(slot, &cancellation).await?;
                runtime
                    .search_symbols(query, limit, cancellation)
                    .await
                    .map_err(|error| map_language_error(slot.profile.id(), error))
            });
        }

        if queries.is_empty() {
            return Err(CodeIntelligenceError::Unavailable {
                message: "no supported saved source files were found in this workspace".to_owned(),
            });
        }

        let mut items = Vec::new();
        let mut truncated = false;
        let mut first_error = None;
        let mut succeeded = false;
        while let Some(result) = queries.next().await {
            match result {
                Ok(result) => {
                    succeeded = true;
                    truncated |= result.truncated;
                    items.extend(result.items);
                }
                Err(CodeIntelligenceError::Cancelled) => {
                    return Err(CodeIntelligenceError::Cancelled)
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            };
        }
        if !succeeded {
            return Err(
                first_error.unwrap_or_else(|| CodeIntelligenceError::Unavailable {
                    message: "no language runtime could search workspace symbols".to_owned(),
                }),
            );
        }

        items.sort_by_cached_key(symbol_key);
        items.dedup_by(|left, right| symbol_key(left) == symbol_key(right));
        truncated |= items.len() > limit;
        items.truncate(limit);
        Ok(self.workspace_result(items, truncated))
    }

    pub(crate) async fn navigate(
        &self,
        kind: NavigationKind,
        path: &WorkspacePath,
        position: CodePosition,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<CodeLocation>> {
        let content = self.read_saved(path, &cancellation).await?;
        let (profile, runtime) = self.runtime_for_path(path, &cancellation).await?;
        let mut result = runtime
            .navigate(kind, path, position, &content, cancellation.clone())
            .await
            .map_err(|error| map_language_error(profile, error))?;
        self.complete_saved_query(path, &cancellation, &mut result)
            .await?;
        result.workspace_revision = self.workspace_revision();
        Ok(result)
    }

    pub(crate) async fn diagnostics(
        &self,
        path: Option<&WorkspacePath>,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<CodeDiagnostic>> {
        let Some(path) = path else {
            return self.workspace_diagnostics(cancellation).await;
        };

        let content = self.read_saved(path, &cancellation).await?;
        let (profile, runtime) = self.runtime_for_path(path, &cancellation).await?;
        let mut result = runtime
            .diagnostics(path, &content, cancellation.clone())
            .await
            .map_err(|error| map_language_error(profile, error))?;
        self.complete_saved_query(path, &cancellation, &mut result)
            .await?;
        result.workspace_revision = self.workspace_revision();
        Ok(result)
    }

    async fn workspace_diagnostics(
        &self,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<CodeDiagnostic>> {
        if cancellation.is_cancelled() {
            return Err(CodeIntelligenceError::Cancelled);
        }
        let source_paths = tokio::select! {
            _ = cancellation.cancelled() => return Err(CodeIntelligenceError::Cancelled),
            paths = self.source_paths.read() => paths.clone(),
        };
        let relevant_slots = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.relevant.load(Ordering::Acquire))
            .collect::<Vec<_>>();
        if relevant_slots.is_empty() {
            return Err(CodeIntelligenceError::Unavailable {
                message: "no supported saved source files were found in this workspace".to_owned(),
            });
        }

        // Start each relevant profile independently so one missing executable
        // cannot prevent diagnostics from another language.
        let mut starts = FuturesUnordered::new();
        for (slot_index, slot) in relevant_slots {
            let cancellation = cancellation.clone();
            starts
                .push(async move { (slot_index, self.ensure_runtime(slot, &cancellation).await) });
        }
        let mut runtimes = vec![None; self.slots.len()];
        let mut first_error = None;
        let mut truncated = false;
        while let Some((slot_index, result)) = starts.next().await {
            match result {
                Ok(runtime) => runtimes[slot_index] = Some(runtime),
                Err(CodeIntelligenceError::Cancelled) => {
                    return Err(CodeIntelligenceError::Cancelled)
                }
                Err(error) => {
                    truncated = true;
                    first_error.get_or_insert(error);
                }
            }
        }

        let mut diagnostic_slots = Vec::new();
        for (slot_index, runtime) in runtimes.iter().enumerate() {
            let Some(runtime) = runtime else {
                continue;
            };
            if runtime.capabilities().diagnostics {
                diagnostic_slots.push(slot_index);
            } else {
                truncated = true;
                first_error.get_or_insert_with(|| {
                    map_language_error(
                        self.slots[slot_index].profile.id(),
                        LanguageRuntimeError::Unsupported {
                            operation: "diagnostics",
                        },
                    )
                });
            }
        }
        if diagnostic_slots.is_empty() {
            return Err(
                first_error.unwrap_or_else(|| CodeIntelligenceError::Unavailable {
                    message: "no language runtime can provide workspace diagnostics".to_owned(),
                }),
            );
        }

        let (documents, selection_truncated) = select_workspace_diagnostic_paths(
            &self.slots,
            &diagnostic_slots,
            &source_paths,
            WORKSPACE_DIAGNOSTIC_DOCUMENT_LIMIT,
        );
        truncated |= selection_truncated;
        if documents.is_empty() {
            return Err(
                first_error.unwrap_or_else(|| CodeIntelligenceError::Unavailable {
                    message: "no supported saved source files could be queried".to_owned(),
                }),
            );
        }

        let mut diagnostic_documents = Vec::with_capacity(documents.len());
        for (slot_index, path) in documents {
            let Some(runtime) = runtimes[slot_index].as_ref() else {
                truncated = true;
                first_error.get_or_insert_with(|| CodeIntelligenceError::Unavailable {
                    message: format!(
                        "the {} language runtime became unavailable before diagnostics",
                        profile_language(self.slots[slot_index].profile.id())
                    ),
                });
                continue;
            };
            diagnostic_documents.push((
                self.slots[slot_index].profile.id(),
                Arc::clone(runtime),
                path,
            ));
        }
        if diagnostic_documents.is_empty() {
            return Err(
                first_error.unwrap_or_else(|| CodeIntelligenceError::Unavailable {
                    message: "no language runtime remained available for diagnostics".to_owned(),
                }),
            );
        }

        let queries = diagnostic_documents
            .into_iter()
            .map(|(profile, runtime, path)| {
                let cancellation = cancellation.clone();
                async move {
                    let content = self.read_saved(&path, &cancellation).await?;
                    runtime
                        .diagnostics(&path, &content, cancellation)
                        .await
                        .map_err(|error| map_language_error(profile, error))
                }
            });
        let mut queries =
            futures::stream::iter(queries).buffer_unordered(WORKSPACE_DIAGNOSTIC_CONCURRENCY);
        let mut items = Vec::new();
        let mut succeeded = 0_usize;
        while let Some(result) = queries.next().await {
            match result {
                Ok(result) => {
                    succeeded += 1;
                    truncated |= result.truncated;
                    truncated |=
                        append_bounded(&mut items, result.items, WORKSPACE_DIAGNOSTIC_LIMIT);
                }
                Err(CodeIntelligenceError::Cancelled) => {
                    return Err(CodeIntelligenceError::Cancelled)
                }
                Err(error) => {
                    truncated = true;
                    first_error.get_or_insert(error);
                }
            }
        }
        if succeeded == 0 {
            return Err(
                first_error.unwrap_or_else(|| CodeIntelligenceError::Unavailable {
                    message: "no saved source document returned diagnostics".to_owned(),
                }),
            );
        }

        items.sort_by(diagnostic_order);
        Ok(self.workspace_result(items, truncated))
    }

    pub(crate) async fn notify_file_changes(&self, changes: &[WorkspaceFileChange]) {
        let runtimes = self.ready_runtimes().await;
        for runtime in runtimes {
            if let Err(error) = runtime.notify_file_changes(changes).await {
                tracing::warn!(error = %error, "Code Intelligence file-change notification failed");
            }
        }
    }

    pub(crate) async fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        self.lifetime.cancel();
        let runtimes = self.ready_runtimes().await;
        for runtime in runtimes {
            if let Err(error) = runtime.shutdown().await {
                tracing::warn!(error = %error, "Code Intelligence runtime shutdown failed");
            }
        }
        self.status.send_replace(CodeIntelligenceStatus {
            state: CodeIntelligenceState::Unavailable,
            message: Some("Code Intelligence runtime is shut down".to_owned()),
            ..CodeIntelligenceStatus::default()
        });
    }

    async fn runtime_for_path(
        &self,
        path: &WorkspacePath,
        cancellation: &CancellationToken,
    ) -> CodeIntelligenceResult<(ProjectLanguageProfile, Arc<LanguageRuntime>)> {
        let slot = self
            .slots
            .iter()
            .find(|slot| slot.profile.supports_path(Path::new(path.as_str())))
            .ok_or_else(|| CodeIntelligenceError::Unsupported {
                operation: "language".to_owned(),
                message: format!(
                    "no language profile supports saved document {}",
                    path.as_str()
                ),
            })?;
        slot.relevant.store(true, Ordering::Release);
        let runtime = self.ensure_runtime(slot, cancellation).await?;
        Ok((slot.profile.id(), runtime))
    }

    async fn ensure_runtime(
        &self,
        slot: &LanguageSlot,
        cancellation: &CancellationToken,
    ) -> CodeIntelligenceResult<Arc<LanguageRuntime>> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(CodeIntelligenceError::Unavailable {
                message: "the workspace runtime is shutting down".to_owned(),
            });
        }
        let mut state = tokio::select! {
            _ = cancellation.cancelled() => return Err(CodeIntelligenceError::Cancelled),
            state = slot.state.lock() => state,
        };
        let retiring = match &*state {
            SlotState::Ready(runtime) => {
                if let Some(message) = runtime.unavailable_message() {
                    Some((Arc::clone(runtime), message))
                } else {
                    return Ok(Arc::clone(runtime));
                }
            }
            SlotState::Failed(failure) if failure.at.elapsed() < START_RETRY_DELAY => {
                return Err(CodeIntelligenceError::Unavailable {
                    message: failure.message.clone(),
                });
            }
            SlotState::Dormant | SlotState::Failed(_) => None,
        };
        if let Some((runtime, message)) = retiring {
            tracing::warn!(
                language = %profile_language(slot.profile.id()),
                message,
                "Code Intelligence will restart an exited language runtime"
            );
            // The client can observe protocol EOF before the process monitor
            // has reaped a server that kept running. Keep the slot locked and
            // finish the old generation before making it startable again.
            if let Err(error) = runtime.shutdown().await {
                tracing::warn!(
                    language = %profile_language(slot.profile.id()),
                    error = %error,
                    "Code Intelligence could not fully retire an exited language runtime"
                );
                // Keep the failed generation in Ready. A later query may retry
                // cleanup, but no replacement may start while the old process
                // has not been confirmed reaped.
                return Err(map_language_error(slot.profile.id(), error));
            }
            *state = SlotState::Dormant;
        }
        if cancellation.is_cancelled() {
            return Err(CodeIntelligenceError::Cancelled);
        }
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(CodeIntelligenceError::Unavailable {
                message: "the workspace runtime is shutting down".to_owned(),
            });
        }

        let result = LanguageRuntime::start(
            slot.profile.clone(),
            self.canonical_root.clone(),
            self.layout.clone(),
            Arc::clone(&slot.documents),
            Arc::clone(&self.diagnostics),
            cancellation.clone(),
            self.timeout,
        )
        .await;
        if cancellation.is_cancelled() || self.shutting_down.load(Ordering::Acquire) {
            if let Ok(runtime) = result {
                if let Err(error) = runtime.shutdown().await {
                    tracing::warn!(
                        language = %profile_language(slot.profile.id()),
                        error = %error,
                        "Code Intelligence could not retire a cancelled language runtime start"
                    );
                }
            }
            *state = SlotState::Dormant;
            return if cancellation.is_cancelled() {
                Err(CodeIntelligenceError::Cancelled)
            } else {
                Err(CodeIntelligenceError::Unavailable {
                    message: "the workspace runtime is shutting down".to_owned(),
                })
            };
        }
        match result {
            Ok(runtime) => {
                let runtime = Arc::new(runtime);
                *state = SlotState::Ready(Arc::clone(&runtime));
                drop(state);
                self.spawn_runtime_monitor(slot, Arc::clone(&runtime));
                self.refresh_status().await;
                Ok(runtime)
            }
            Err(error) => {
                let message = error.to_string();
                let public = map_language_error(slot.profile.id(), error);
                *state = SlotState::Failed(StartFailure {
                    at: Instant::now(),
                    message,
                });
                drop(state);
                self.refresh_status().await;
                Err(public)
            }
        }
    }

    fn spawn_runtime_monitor(&self, slot: &LanguageSlot, runtime: Arc<LanguageRuntime>) {
        let state = Arc::clone(&slot.state);
        let status = self.status.clone();
        let lifetime = self.lifetime.clone();
        let language = profile_language(slot.profile.id());
        let mut process_state = runtime.subscribe_process_state();
        tokio::spawn(async move {
            loop {
                if !matches!(
                    *process_state.borrow(),
                    super::lsp::process::LspProcessState::Running
                ) {
                    break;
                }
                tokio::select! {
                    _ = lifetime.cancelled() => return,
                    changed = process_state.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                }
            }
            if lifetime.is_cancelled() {
                return;
            }
            let message = runtime
                .unavailable_message()
                .unwrap_or_else(|| "the language runtime stopped unexpectedly".to_owned());
            let mut slot_state = state.lock().await;
            let is_current = matches!(
                &*slot_state,
                SlotState::Ready(current) if Arc::ptr_eq(current, &runtime)
            );
            if !is_current {
                return;
            }
            if let Err(error) = runtime.shutdown().await {
                tracing::warn!(
                    language = %language,
                    error = %error,
                    "Code Intelligence could not clean up a stopped language runtime"
                );
                // Leave the current generation installed. `ensure_runtime`
                // will retry its cleanup and must not start a replacement
                // until shutdown confirms the process has been reaped.
                drop(slot_state);
                publish_stopped_language_status(
                    &status,
                    language,
                    format!("{message}; cleanup failed: {error}"),
                );
                return;
            }
            *slot_state = SlotState::Dormant;
            drop(slot_state);
            publish_stopped_language_status(&status, language, message);
        });
    }

    async fn read_saved(
        &self,
        path: &WorkspacePath,
        cancellation: &CancellationToken,
    ) -> CodeIntelligenceResult<String> {
        let read = self.file_system.read_text(path);
        tokio::select! {
            _ = cancellation.cancelled() => Err(CodeIntelligenceError::Cancelled),
            result = tokio::time::timeout(self.timeout, read) => match result {
                Ok(Ok(content)) => Ok(content),
                Ok(Err(error)) => Err(map_workspace_error(path, error)),
                Err(_) => Err(CodeIntelligenceError::Timeout {
                    operation: "read_saved_document".to_owned(),
                    duration: self.timeout,
                }),
            }
        }
    }

    /// Confirm that the saved bytes which produced a semantic result are
    /// still current. File watchers are intentionally not part of this check:
    /// their delivery may lag behind a query that overlaps an external save.
    async fn complete_saved_query<T>(
        &self,
        path: &WorkspacePath,
        cancellation: &CancellationToken,
        result: &mut CodeQueryResult<T>,
    ) -> CodeIntelligenceResult<()> {
        let Some(snapshot) = result.document.as_mut() else {
            return Ok(());
        };
        match self.read_saved(path, cancellation).await {
            Ok(content) => {
                snapshot.stale |= sha256::digest(content.as_bytes()) != snapshot.content_hash;
                Ok(())
            }
            Err(CodeIntelligenceError::Cancelled) => Err(CodeIntelligenceError::Cancelled),
            Err(_) => {
                // A result based on a file that disappeared or became
                // unreadable during the request is still useful, but cannot
                // be presented as current.
                snapshot.stale = true;
                Ok(())
            }
        }
    }

    async fn ready_runtimes(&self) -> Vec<Arc<LanguageRuntime>> {
        let mut runtimes = Vec::new();
        for slot in &self.slots {
            if let SlotState::Ready(runtime) = &*slot.state.lock().await {
                runtimes.push(Arc::clone(runtime));
            }
        }
        runtimes
    }

    async fn refresh_status(&self) {
        let mut languages = Vec::new();
        let mut capabilities = CodeIntelligenceCapabilities::default();
        let mut ready = 0_usize;
        let mut failed = 0_usize;
        let mut dormant = 0_usize;
        for slot in &self.slots {
            if !slot.relevant.load(Ordering::Acquire) {
                continue;
            }
            let state = slot.state.lock().await;
            let (runtime_state, runtime_capabilities, message) = match &*state {
                SlotState::Dormant => {
                    dormant += 1;
                    (
                        CodeIntelligenceState::Starting,
                        CodeIntelligenceCapabilities::default(),
                        Some("starts on first semantic query".to_owned()),
                    )
                }
                SlotState::Ready(runtime) => {
                    ready += 1;
                    let current = runtime.capabilities();
                    union_capabilities(&mut capabilities, current);
                    (CodeIntelligenceState::Ready, current, None)
                }
                SlotState::Failed(failure) => {
                    failed += 1;
                    (
                        CodeIntelligenceState::Unavailable,
                        CodeIntelligenceCapabilities::default(),
                        Some(failure.message.clone()),
                    )
                }
            };
            languages.push(CodeIntelligenceLanguageStatus {
                language: profile_language(slot.profile.id()),
                state: runtime_state,
                capabilities: runtime_capabilities,
                message,
            });
        }
        let state = if ready > 0 && failed > 0 {
            CodeIntelligenceState::Degraded
        } else if ready > 0 {
            CodeIntelligenceState::Ready
        } else if failed > 0 && dormant == 0 {
            CodeIntelligenceState::Unavailable
        } else if dormant > 0 {
            CodeIntelligenceState::Starting
        } else {
            CodeIntelligenceState::Unavailable
        };
        self.status.send_replace(CodeIntelligenceStatus {
            state,
            capabilities,
            languages,
            message: (failed > 0)
                .then(|| "one or more language runtimes are unavailable".to_owned()),
        });
    }

    fn workspace_revision(&self) -> u64 {
        self.workspace_revision.load(Ordering::Acquire)
    }

    fn workspace_result<T>(&self, items: Vec<T>, truncated: bool) -> CodeQueryResult<T> {
        CodeQueryResult {
            items,
            truncated,
            workspace_revision: self.workspace_revision(),
            document: None,
        }
    }
}
