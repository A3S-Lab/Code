//! One lazily-started semantic runtime generation for a workspace layout.

#[cfg(test)]
mod integration_test_support;
#[cfg(test)]
mod integration_tests;
mod lifecycle;
mod support;
#[cfg(test)]
mod tests;

use support::*;

use std::{
    any::Any,
    panic::AssertUnwindSafe,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
use tokio::{
    sync::{oneshot, watch, Mutex, RwLock},
    task::AbortHandle,
};
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
const SHUTDOWN_GRACE: Duration = Duration::from_secs(4);
const SHUTDOWN_ABORT_SETTLE: Duration = Duration::from_millis(500);

type RuntimeStartOutcome = CodeIntelligenceResult<Arc<LanguageRuntime>>;

#[derive(Clone)]
struct RuntimeStart {
    generation: u64,
    cancellation: CancellationToken,
    outcome: watch::Receiver<Option<RuntimeStartOutcome>>,
    abort: AbortHandle,
}

struct StartFailure {
    at: Instant,
    message: String,
    retained_runtime: Option<Arc<LanguageRuntime>>,
}

struct StartAttemptFailure {
    public: CodeIntelligenceError,
    message: String,
    retained_runtime: Option<Arc<LanguageRuntime>>,
}

enum SlotState {
    Dormant,
    Starting(RuntimeStart),
    Ready(Arc<LanguageRuntime>),
    Failed(StartFailure),
}

struct LanguageSlot {
    profile: LanguageServerProfile,
    relevant: AtomicBool,
    generation: AtomicU64,
    documents: Arc<DocumentStore>,
    state: Arc<Mutex<SlotState>>,
}

impl LanguageSlot {
    fn new(profile: LanguageServerProfile, relevant: bool, document_capacity: usize) -> Self {
        Self {
            profile,
            relevant: AtomicBool::new(relevant),
            generation: AtomicU64::new(0),
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
    status_updates: Arc<Mutex<()>>,
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
        let slots: Vec<_> = profiles
            .into_iter()
            .map(|profile| {
                let relevant = source_paths
                    .iter()
                    .any(|path| profile.supports_path(Path::new(path.as_str())));
                LanguageSlot::new(profile, relevant, document_capacity)
            })
            .collect();
        let languages = slots
            .iter()
            .filter(|slot| slot.relevant.load(Ordering::Acquire))
            .map(|slot| CodeIntelligenceLanguageStatus {
                language: profile_language(slot.profile.id()),
                state: CodeIntelligenceState::Starting,
                capabilities: CodeIntelligenceCapabilities::default(),
                message: Some("starts on first semantic query".to_owned()),
            })
            .collect();
        let (status, _) = watch::channel(CodeIntelligenceStatus {
            state: CodeIntelligenceState::Starting,
            languages,
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
            status_updates: Arc::new(Mutex::new(())),
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
                let mut state = slot.state.lock().await;
                if let SlotState::Starting(start) = &*state {
                    // Cancel while the slot is still locked. Otherwise the
                    // start task can publish Ready after this lock is dropped
                    // but before bounded cleanup observes the generation.
                    start.cancellation.cancel();
                    let start = start.clone();
                    let generation = start.generation;
                    drop(state);
                    self.stop_generations(&[], std::slice::from_ref(&start), "source removal")
                        .await;
                    state = slot.state.lock().await;
                    if matches!(
                        &*state,
                        SlotState::Starting(current) if current.generation == generation
                    ) {
                        *state = SlotState::Dormant;
                    }
                    continue;
                }

                let runtime = match &*state {
                    SlotState::Ready(runtime) => Some(Arc::clone(runtime)),
                    SlotState::Failed(failure) => failure.retained_runtime.clone(),
                    SlotState::Dormant => None,
                    SlotState::Starting(_) => None,
                };
                if let Some(runtime) = runtime {
                    self.stop_generations(std::slice::from_ref(&runtime), &[], "source removal")
                        .await;
                }
                *state = SlotState::Dormant;
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

        let source_paths = tokio::select! {
            _ = cancellation.cancelled() => return Err(CodeIntelligenceError::Cancelled),
            paths = self.source_paths.read() => paths.clone(),
        };
        let mut queries = FuturesUnordered::new();
        for slot in self
            .slots
            .iter()
            .filter(|slot| slot.relevant.load(Ordering::Acquire))
        {
            let anchor = source_paths
                .iter()
                .find(|path| slot.profile.supports_path(Path::new(path.as_str())))
                .cloned();
            let cancellation = cancellation.clone();
            queries.push(async move {
                let anchor = anchor.ok_or_else(|| CodeIntelligenceError::Unavailable {
                    message: format!(
                        "no saved source file can prepare the {} language runtime",
                        profile_language(slot.profile.id())
                    ),
                })?;
                let runtime = self.ensure_runtime(slot, &cancellation).await?;
                let content = self.read_saved(&anchor, &cancellation).await?;
                runtime
                    .prepare_saved_document(&anchor, &content, &cancellation)
                    .await
                    .map_err(|error| map_language_error(slot.profile.id(), error))?;
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
        let _status_update = self.status_updates.lock().await;
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
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
                SlotState::Dormant | SlotState::Starting(_) => {
                    dormant += 1;
                    (
                        CodeIntelligenceState::Starting,
                        CodeIntelligenceCapabilities::default(),
                        Some("starts on first semantic query".to_owned()),
                    )
                }
                SlotState::Ready(runtime) => {
                    if let Some(message) = runtime.unavailable_message() {
                        failed += 1;
                        (
                            CodeIntelligenceState::Unavailable,
                            CodeIntelligenceCapabilities::default(),
                            Some(message),
                        )
                    } else {
                        ready += 1;
                        let current = runtime.capabilities();
                        union_capabilities(&mut capabilities, current);
                        (CodeIntelligenceState::Ready, current, None)
                    }
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
