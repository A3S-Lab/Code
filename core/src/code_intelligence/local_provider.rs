//! Local manifest-backed Code Intelligence provider.

use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex as StdMutex, Weak,
    },
    time::Duration,
};

use async_trait::async_trait;
use tokio::sync::{broadcast, watch, RwLock};
use tokio_util::sync::CancellationToken;

use super::{
    project_layout::ProjectLayoutResolver,
    registry::{
        LocalCodeIntelligenceRegistry, RegistryAcquireError, RegistryConfig, RegistryKey,
        RegistryKeyError, RegistryReport, RegistryShutdownError, RegistryShutdownFailure,
        RuntimeLease,
    },
    workspace_runtime::WorkspaceRuntime,
    CodeDiagnostic, CodeIntelligenceError, CodeIntelligenceResult, CodeIntelligenceState,
    CodeIntelligenceStatus, CodeLocation, CodePosition, CodeQueryResult, DocumentSymbol,
    NavigationKind, SymbolInformation, WorkspaceCodeIntelligence,
};
use crate::workspace::{
    LocalWorkspaceManifest, LocalWorkspaceManifestSnapshot, WorkspaceFileChange,
    WorkspaceFileSystem, WorkspacePath,
};

const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(15);
type RuntimeRegistry = LocalCodeIntelligenceRegistry<WorkspaceRuntime, Infallible>;
type WorkspaceRuntimeLease = RuntimeLease<WorkspaceRuntime, Infallible>;

/// Native local provider sharing the workspace manifest's existing watcher.
pub struct LocalCodeIntelligence {
    isolation_scope: String,
    manifest: Arc<LocalWorkspaceManifest>,
    file_system: Arc<dyn WorkspaceFileSystem>,
    registry: RuntimeRegistry,
    current: RwLock<Option<WorkspaceRuntimeLease>>,
    status: watch::Sender<CodeIntelligenceStatus>,
    generation: Arc<AtomicU64>,
    lifetime: CancellationToken,
    manifest_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
    query_timeout: Duration,
}

impl std::fmt::Debug for LocalCodeIntelligence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalCodeIntelligence")
            .field("isolation_scope", &self.isolation_scope)
            .field("manifest_root", &self.manifest.snapshot().root)
            .field("generation", &self.generation.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl LocalCodeIntelligence {
    /// Create a provider and acquire a cheap, lazily-started runtime generation.
    pub async fn start(
        isolation_scope: impl Into<String>,
        manifest: Arc<LocalWorkspaceManifest>,
        file_system: Arc<dyn WorkspaceFileSystem>,
    ) -> CodeIntelligenceResult<Arc<Self>> {
        Self::start_with_timeout(
            isolation_scope,
            manifest,
            file_system,
            DEFAULT_QUERY_TIMEOUT,
        )
        .await
    }

    pub(crate) async fn start_with_timeout(
        isolation_scope: impl Into<String>,
        manifest: Arc<LocalWorkspaceManifest>,
        file_system: Arc<dyn WorkspaceFileSystem>,
        query_timeout: Duration,
    ) -> CodeIntelligenceResult<Arc<Self>> {
        let isolation_scope = isolation_scope.into();
        let snapshot_rx = manifest.subscribe();
        let changes_rx = manifest.subscribe_changes();
        let registry = RuntimeRegistry::new(
            RegistryConfig::new(Duration::ZERO, 0),
            |runtime: Arc<WorkspaceRuntime>| async move {
                runtime.shutdown().await;
                Ok(())
            },
        );
        let (status, _) = watch::channel(CodeIntelligenceStatus {
            state: CodeIntelligenceState::Starting,
            message: Some("Code Intelligence is preparing the saved workspace".to_owned()),
            ..CodeIntelligenceStatus::default()
        });
        let provider = Arc::new(Self {
            isolation_scope,
            manifest,
            file_system,
            registry,
            current: RwLock::new(None),
            status,
            generation: Arc::new(AtomicU64::new(0)),
            lifetime: CancellationToken::new(),
            manifest_task: StdMutex::new(None),
            query_timeout,
        });

        provider
            .refresh_snapshot(provider.manifest.snapshot())
            .await?;
        let weak = Arc::downgrade(&provider);
        let task = tokio::spawn(run_manifest_updates(
            weak,
            snapshot_rx,
            changes_rx,
            provider.lifetime.clone(),
        ));
        *provider
            .manifest_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(task);
        Ok(provider)
    }

    /// Stop manifest forwarding and all language processes owned by this provider.
    pub async fn shutdown(&self) {
        self.lifetime.cancel();
        let task = self
            .manifest_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
        let old = self.current.write().await.take();
        drop(old);
        report_registry_cleanup("shutdown", self.registry.shutdown_all().await);
        self.status.send_replace(CodeIntelligenceStatus {
            state: CodeIntelligenceState::Unavailable,
            message: Some("Code Intelligence is shut down".to_owned()),
            ..CodeIntelligenceStatus::default()
        });
    }

    async fn refresh_snapshot(
        self: &Arc<Self>,
        snapshot: LocalWorkspaceManifestSnapshot,
    ) -> CodeIntelligenceResult<()> {
        let layout = ProjectLayoutResolver::resolve(&snapshot);
        {
            let current = self.current.read().await;
            if let Some(runtime) = current
                .as_ref()
                .filter(|runtime| runtime.layout_hash() == layout.layout_hash)
            {
                runtime.update_snapshot(&snapshot).await;
                return Ok(());
            }
        }

        let key = RegistryKey::new(
            self.isolation_scope.clone(),
            &snapshot.root,
            layout.layout_hash,
        )
        .await
        .map_err(map_key_error)?;
        let canonical_root = key.canonical_root().to_path_buf();
        let runtime_snapshot = snapshot.clone();
        let file_system = Arc::clone(&self.file_system);
        let timeout = self.query_timeout;
        let lease = self
            .registry
            .acquire(key, move |_| async move {
                Ok(WorkspaceRuntime::new(
                    canonical_root,
                    layout,
                    &runtime_snapshot,
                    file_system,
                    timeout,
                ))
            })
            .await
            .map_err(map_acquire_error)?;

        lease.update_snapshot(&snapshot).await;
        let mut receiver = lease.subscribe_status();
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.status.send_replace(receiver.borrow().clone());
        let old = self.current.write().await.replace(lease);
        drop(old);
        report_registry_cleanup("layout refresh", self.registry.cleanup_idle().await);
        self.spawn_status_forwarder(generation, &mut receiver);
        Ok(())
    }

    fn spawn_status_forwarder(
        &self,
        generation: u64,
        receiver: &mut watch::Receiver<CodeIntelligenceStatus>,
    ) {
        let mut receiver = receiver.clone();
        let sender = self.status.clone();
        let current_generation = Arc::clone(&self.generation);
        let lifetime = self.lifetime.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = lifetime.cancelled() => break,
                    changed = receiver.changed() => {
                        if changed.is_err()
                            || current_generation.load(Ordering::Acquire) != generation
                        {
                            break;
                        }
                        sender.send_replace(receiver.borrow().clone());
                    }
                }
            }
        });
    }

    async fn handle_changes(&self, changes: &[WorkspaceFileChange]) {
        let current = self.current.read().await;
        if let Some(runtime) = current.as_ref() {
            runtime.notify_file_changes(changes).await;
        }
    }

    async fn runtime(
        &self,
    ) -> CodeIntelligenceResult<tokio::sync::RwLockReadGuard<'_, Option<WorkspaceRuntimeLease>>>
    {
        let current = self.current.read().await;
        if current.is_none() {
            return Err(CodeIntelligenceError::Unavailable {
                message: "Code Intelligence has not prepared this workspace yet".to_owned(),
            });
        }
        Ok(current)
    }

    fn report_update_error(&self, error: &CodeIntelligenceError) {
        let mut status = self.status.borrow().clone();
        status.state = CodeIntelligenceState::Degraded;
        status.message = Some(format!("workspace refresh failed: {error}"));
        self.status.send_replace(status);
    }
}

#[async_trait]
impl WorkspaceCodeIntelligence for LocalCodeIntelligence {
    fn subscribe_status(&self) -> watch::Receiver<CodeIntelligenceStatus> {
        self.status.subscribe()
    }

    async fn document_symbols(
        &self,
        path: &WorkspacePath,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<DocumentSymbol>> {
        let runtime = self.runtime().await?;
        let Some(runtime) = runtime.as_ref() else {
            return Err(runtime_unavailable());
        };
        runtime.document_symbols(path, cancellation).await
    }

    async fn search_symbols(
        &self,
        query: &str,
        limit: usize,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<SymbolInformation>> {
        let runtime = self.runtime().await?;
        let Some(runtime) = runtime.as_ref() else {
            return Err(runtime_unavailable());
        };
        runtime.search_symbols(query, limit, cancellation).await
    }

    async fn navigate(
        &self,
        kind: NavigationKind,
        path: &WorkspacePath,
        position: CodePosition,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<CodeLocation>> {
        let runtime = self.runtime().await?;
        let Some(runtime) = runtime.as_ref() else {
            return Err(runtime_unavailable());
        };
        runtime.navigate(kind, path, position, cancellation).await
    }

    async fn diagnostics(
        &self,
        path: Option<&WorkspacePath>,
        cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<CodeDiagnostic>> {
        let runtime = self.runtime().await?;
        let Some(runtime) = runtime.as_ref() else {
            return Err(runtime_unavailable());
        };
        runtime.diagnostics(path, cancellation).await
    }
}

impl Drop for LocalCodeIntelligence {
    fn drop(&mut self) {
        self.lifetime.cancel();
        if let Some(task) = self
            .manifest_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

async fn run_manifest_updates(
    provider: Weak<LocalCodeIntelligence>,
    mut snapshots: broadcast::Receiver<LocalWorkspaceManifestSnapshot>,
    mut changes: broadcast::Receiver<WorkspaceFileChange>,
    lifetime: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = lifetime.cancelled() => break,
            update = snapshots.recv() => match update {
                Ok(snapshot) => {
                    let Some(provider) = provider.upgrade() else { break; };
                    if let Err(error) = provider.refresh_snapshot(snapshot).await {
                        provider.report_update_error(&error);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let Some(provider) = provider.upgrade() else { break; };
                    if let Err(error) = provider.refresh_snapshot(provider.manifest.snapshot()).await {
                        provider.report_update_error(&error);
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            update = changes.recv() => match update {
                Ok(change) => {
                    let mut batch = vec![change];
                    while let Ok(change) = changes.try_recv() {
                        batch.push(change);
                    }
                    let Some(provider) = provider.upgrade() else { break; };
                    provider.handle_changes(&batch).await;
                }
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    let Some(provider) = provider.upgrade() else { break; };
                    let mut status = provider.status.borrow().clone();
                    status.state = CodeIntelligenceState::Degraded;
                    status.message = Some(format!(
                        "workspace change stream skipped {count} events; saved documents will resynchronize on query"
                    ));
                    provider.status.send_replace(status);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

fn map_key_error(error: RegistryKeyError) -> CodeIntelligenceError {
    CodeIntelligenceError::Unavailable {
        message: error.to_string(),
    }
}

fn map_acquire_error(error: RegistryAcquireError<Infallible>) -> CodeIntelligenceError {
    match error {
        RegistryAcquireError::ShuttingDown => CodeIntelligenceError::Unavailable {
            message: "the Code Intelligence registry is shutting down".to_owned(),
        },
        RegistryAcquireError::Factory(error) => match *error {},
        RegistryAcquireError::FactoryPanicked { message } => CodeIntelligenceError::Unavailable {
            message: format!("Code Intelligence runtime initialization panicked: {message}"),
        },
        RegistryAcquireError::LeaseLimit => CodeIntelligenceError::Unavailable {
            message: "the Code Intelligence runtime lease limit was exhausted".to_owned(),
        },
    }
}

fn runtime_unavailable() -> CodeIntelligenceError {
    CodeIntelligenceError::Unavailable {
        message: "Code Intelligence has not prepared this workspace yet".to_owned(),
    }
}

fn report_registry_cleanup(context: &'static str, report: RegistryReport<Infallible>) {
    if !report.removed.is_empty() {
        tracing::debug!(
            context,
            retired = report.removed.len(),
            "Code Intelligence retired workspace runtimes"
        );
    }
    for RegistryShutdownError { key, failure } in report.errors {
        match failure {
            RegistryShutdownFailure::Runtime(error) => {
                tracing::error!(
                    context,
                    workspace = ?key.canonical_root(),
                    ?error,
                    "Code Intelligence runtime cleanup returned an impossible error"
                );
            }
            RegistryShutdownFailure::Panicked { message } => {
                tracing::warn!(
                    context,
                    workspace = ?key.canonical_root(),
                    %message,
                    "Code Intelligence runtime cleanup panicked"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{
        LocalWorkspaceFile, LocalWorkspaceFileStatus, ManifestWorkspaceBackend,
    };

    fn file(path: &str) -> LocalWorkspaceFile {
        LocalWorkspaceFile {
            path: path.to_owned(),
            size: 1,
            modified_ms: Some(1),
            language: None,
            status: LocalWorkspaceFileStatus::Tracked,
            binary: false,
            generated: false,
        }
    }

    async fn scanned_snapshot(manifest: &LocalWorkspaceManifest) -> LocalWorkspaceManifestSnapshot {
        if manifest.snapshot().version > 0 {
            return manifest.snapshot();
        }
        let mut snapshots = manifest.subscribe();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = snapshots.recv().await.unwrap();
                if snapshot.version > 0 {
                    break snapshot;
                }
            }
        })
        .await
        .expect("initial manifest scan should finish")
    }

    #[tokio::test]
    async fn provider_reuses_unchanged_layout_and_shutdown_is_idempotent() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("src")).unwrap();
        std::fs::write(workspace.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(workspace.path().join("src/lib.rs"), "pub fn saved() {}\n").unwrap();
        let backend = ManifestWorkspaceBackend::new(workspace.path());
        let manifest = backend.manifest();
        let initial = scanned_snapshot(&manifest).await;
        let file_system: Arc<dyn WorkspaceFileSystem> = backend;
        let provider = LocalCodeIntelligence::start_with_timeout(
            "test-session",
            manifest,
            file_system,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        let generation = provider.generation.load(Ordering::Acquire);
        assert_eq!(generation, 1);

        let mut source_only = initial.clone();
        source_only.version += 1;
        source_only.files.push(file("src/new.rs"));
        provider.refresh_snapshot(source_only).await.unwrap();
        assert_eq!(provider.generation.load(Ordering::Acquire), generation);

        let mut changed_layout = initial;
        changed_layout.version += 2;
        changed_layout
            .files
            .retain(|entry| entry.path != "Cargo.toml");
        changed_layout.files.push(file("package.json"));
        provider.refresh_snapshot(changed_layout).await.unwrap();
        assert_eq!(provider.generation.load(Ordering::Acquire), generation + 1);

        provider.shutdown().await;
        provider.shutdown().await;
        assert!(provider.current.read().await.is_none());
        assert!(provider.lifetime.is_cancelled());
        assert_eq!(
            provider.status.borrow().state,
            CodeIntelligenceState::Unavailable
        );
    }
}
