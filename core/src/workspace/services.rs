//! Workspace service aggregation and builder.

#[allow(unused_imports)]
use super::{CommandRequest, WorkspaceError, WorkspaceVersionConflict};
use super::{
    LocalWorkspaceBackend, ManifestWorkspaceBackend, VirtualPathResolver, WorkspaceCapabilities,
    WorkspaceChunkCatalog, WorkspaceCommandRunner, WorkspaceFileSystem, WorkspaceFileSystemExt,
    WorkspaceGit, WorkspaceGitStashProvider, WorkspaceGitWorktreeProvider, WorkspacePath,
    WorkspacePathResolver, WorkspaceRef, WorkspaceResult, WorkspaceRetrievalRuntime,
    WorkspaceSearch, WorkspaceTextReader, WorkspaceWriteOutcome,
};
use crate::code_intelligence::{LocalCodeIntelligence, WorkspaceCodeIntelligence};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The host-provided workspace capability bundle used by tool execution.
pub struct WorkspaceServices {
    workspace_ref: WorkspaceRef,
    capabilities: WorkspaceCapabilities,
    path_resolver: Arc<dyn WorkspacePathResolver>,
    file_system: Arc<dyn WorkspaceFileSystem>,
    file_system_ext: Option<Arc<dyn WorkspaceFileSystemExt>>,
    text_reader: Option<Arc<dyn WorkspaceTextReader>>,
    command_runner: Option<Arc<dyn WorkspaceCommandRunner>>,
    search: Option<Arc<dyn WorkspaceSearch>>,
    code_intelligence: Option<Arc<dyn WorkspaceCodeIntelligence>>,
    chunk_catalog: Option<Arc<WorkspaceChunkCatalog>>,
    workspace_retrieval: Option<Arc<WorkspaceRetrievalRuntime>>,
    git: Option<Arc<dyn WorkspaceGit>>,
    git_stash: Option<Arc<dyn WorkspaceGitStashProvider>>,
    git_worktree: Option<Arc<dyn WorkspaceGitWorktreeProvider>>,
    /// Default timeout applied to non-bash workspace operations (file system,
    /// search, git). Bash uses its own per-call timeout in [`CommandRequest`].
    /// `None` means no enforced timeout — appropriate for the local backend.
    operation_timeout: Option<std::time::Duration>,
    local_root: Option<PathBuf>,
}

impl std::fmt::Debug for WorkspaceServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceServices")
            .field("workspace_ref", &self.workspace_ref)
            .field("capabilities", &self.capabilities)
            .field("file_system_ext", &self.file_system_ext.is_some())
            .field("text_reader", &self.text_reader.is_some())
            .field("command_runner", &self.command_runner.is_some())
            .field("search", &self.search.is_some())
            .field("code_intelligence", &self.code_intelligence.is_some())
            .field("chunk_catalog", &self.chunk_catalog.is_some())
            .field("workspace_retrieval", &self.workspace_retrieval.is_some())
            .field("git", &self.git.is_some())
            .field("git_stash", &self.git_stash.is_some())
            .field("git_worktree", &self.git_worktree.is_some())
            .field("local_root", &self.local_root)
            .finish()
    }
}

impl WorkspaceServices {
    pub(crate) fn new_with_git(
        workspace_ref: WorkspaceRef,
        mut capabilities: WorkspaceCapabilities,
        path_resolver: Arc<dyn WorkspacePathResolver>,
        file_system: Arc<dyn WorkspaceFileSystem>,
        command_runner: Option<Arc<dyn WorkspaceCommandRunner>>,
        search: Option<Arc<dyn WorkspaceSearch>>,
        git: Option<Arc<dyn WorkspaceGit>>,
    ) -> Self {
        if command_runner.is_none() {
            capabilities.exec = false;
        }
        if search.is_none() {
            capabilities.search = false;
        }
        if git.is_none() {
            capabilities.git = false;
        }
        capabilities.code_intelligence = false;
        Self {
            workspace_ref,
            capabilities,
            path_resolver,
            file_system,
            file_system_ext: None,
            text_reader: None,
            command_runner,
            search,
            code_intelligence: None,
            chunk_catalog: None,
            workspace_retrieval: None,
            git,
            git_stash: None,
            git_worktree: None,
            operation_timeout: None,
            local_root: None,
        }
    }

    pub fn builder(
        workspace_ref: WorkspaceRef,
        file_system: Arc<dyn WorkspaceFileSystem>,
    ) -> WorkspaceServicesBuilder {
        WorkspaceServicesBuilder::new(workspace_ref, file_system)
    }

    pub fn local(root: impl Into<PathBuf>) -> Arc<Self> {
        let backend = Arc::new(LocalWorkspaceBackend::new(root.into()));
        let workspace_ref = WorkspaceRef::new(
            backend.root.display().to_string(),
            backend.root.display().to_string(),
        );
        let path_resolver: Arc<dyn WorkspacePathResolver> = backend.clone();
        let file_system: Arc<dyn WorkspaceFileSystem> = backend.clone();
        let text_reader: Arc<dyn WorkspaceTextReader> = backend.clone();
        let command_runner: Arc<dyn WorkspaceCommandRunner> = backend.clone();
        let search: Arc<dyn WorkspaceSearch> = backend.clone();
        let git: Arc<dyn WorkspaceGit> = backend.clone();
        let git_stash: Arc<dyn WorkspaceGitStashProvider> = backend.clone();
        let git_worktree: Arc<dyn WorkspaceGitWorktreeProvider> = backend.clone();
        Arc::new(Self {
            workspace_ref,
            capabilities: WorkspaceCapabilities::local_default(),
            path_resolver,
            file_system,
            file_system_ext: None,
            text_reader: Some(text_reader),
            command_runner: Some(command_runner),
            search: Some(search),
            code_intelligence: None,
            chunk_catalog: None,
            workspace_retrieval: None,
            git: Some(git),
            git_stash: Some(git_stash),
            git_worktree: Some(git_worktree),
            operation_timeout: None,
            local_root: Some(backend.root.clone()),
        })
    }

    /// Local workspace services backed by an in-memory file manifest for
    /// search. `read`/`write`/`ls`/`bash`/`git` preserve local backend
    /// behavior; `glob` and `grep` use the manifest once the initial scan has
    /// completed and fall back to filesystem search before that.
    pub fn local_with_manifest(root: impl Into<PathBuf>) -> Arc<Self> {
        let backend = ManifestWorkspaceBackend::new(root);
        Self::local_with_manifest_backend(backend)
    }

    /// Build local manifest services with an asynchronous ephemeral catalog.
    pub fn local_with_retrieval(root: impl Into<PathBuf>) -> Arc<Self> {
        let backend = ManifestWorkspaceBackend::new(root);
        Self::local_with_retrieval_backend(backend)
    }

    /// Build local manifest-backed services with native Code Intelligence.
    ///
    /// The provider subscribes to the manifest's existing change stream and
    /// therefore does not create a second filesystem watcher or file index.
    pub async fn local_with_code_intelligence(
        root: impl Into<PathBuf>,
        isolation_scope: impl Into<String>,
    ) -> Result<Arc<Self>> {
        let backend = ManifestWorkspaceBackend::new(root);
        Self::local_with_code_intelligence_backend(backend, isolation_scope).await
    }

    /// Build one local session with retrieval and Code Intelligence sharing
    /// the same manifest scanner and filesystem change stream.
    pub async fn local_with_retrieval_and_code_intelligence(
        root: impl Into<PathBuf>,
        isolation_scope: impl Into<String>,
    ) -> Result<Arc<Self>> {
        let backend = ManifestWorkspaceBackend::new(root);
        Self::local_with_retrieval_and_code_intelligence_backend(backend, isolation_scope).await
    }

    /// Attach native Code Intelligence to one shared manifest backend.
    pub async fn local_with_code_intelligence_backend(
        backend: Arc<ManifestWorkspaceBackend>,
        isolation_scope: impl Into<String>,
    ) -> Result<Arc<Self>> {
        let manifest = backend.manifest();
        let file_system: Arc<dyn WorkspaceFileSystem> = backend.clone();
        let services = Self::local_with_manifest_backend(backend);
        let provider = LocalCodeIntelligence::start(isolation_scope, manifest, file_system)
            .await
            .map_err(|error| anyhow!("failed to start Code Intelligence: {error}"))?;
        Ok(services.with_code_intelligence(provider))
    }

    /// Attach retrieval and Code Intelligence to one shared manifest backend.
    pub async fn local_with_retrieval_and_code_intelligence_backend(
        backend: Arc<ManifestWorkspaceBackend>,
        isolation_scope: impl Into<String>,
    ) -> Result<Arc<Self>> {
        let manifest = backend.manifest();
        let file_system: Arc<dyn WorkspaceFileSystem> = backend.clone();
        let services = Self::local_with_retrieval_backend(backend);
        let provider = LocalCodeIntelligence::start(isolation_scope, manifest, file_system)
            .await
            .map_err(|error| anyhow!("failed to start Code Intelligence: {error}"))?;
        Ok(services.with_code_intelligence(provider))
    }

    /// Build local workspace services from a shared manifest backend. Hosts
    /// can keep the same manifest for UI file pickers and agent tools.
    pub fn local_with_manifest_backend(backend: Arc<ManifestWorkspaceBackend>) -> Arc<Self> {
        Self::local_with_manifest_backend_and_catalog(backend, None)
    }

    /// Enable retrieval on a shared manifest backend.
    pub fn local_with_retrieval_backend(backend: Arc<ManifestWorkspaceBackend>) -> Arc<Self> {
        let catalog = backend.chunk_catalog();
        Self::local_with_manifest_backend_and_catalog(backend, Some(catalog))
    }

    fn local_with_manifest_backend_and_catalog(
        backend: Arc<ManifestWorkspaceBackend>,
        chunk_catalog: Option<Arc<WorkspaceChunkCatalog>>,
    ) -> Arc<Self> {
        let workspace_ref = WorkspaceRef::new(
            backend.local_root().display().to_string(),
            backend.local_root().display().to_string(),
        );
        let path_resolver: Arc<dyn WorkspacePathResolver> = backend.clone();
        let file_system: Arc<dyn WorkspaceFileSystem> = backend.clone();
        let text_reader: Arc<dyn WorkspaceTextReader> = backend.clone();
        let command_runner: Arc<dyn WorkspaceCommandRunner> = backend.clone();
        let search: Arc<dyn WorkspaceSearch> = backend.clone();
        let git: Arc<dyn WorkspaceGit> = backend.clone();
        let git_stash: Arc<dyn WorkspaceGitStashProvider> = backend.clone();
        let git_worktree: Arc<dyn WorkspaceGitWorktreeProvider> = backend.clone();
        Arc::new(Self {
            workspace_ref,
            capabilities: WorkspaceCapabilities::local_default(),
            path_resolver,
            file_system,
            file_system_ext: None,
            text_reader: Some(text_reader),
            command_runner: Some(command_runner),
            search: Some(search),
            code_intelligence: None,
            chunk_catalog,
            workspace_retrieval: None,
            git: Some(git),
            git_stash: Some(git_stash),
            git_worktree: Some(git_worktree),
            operation_timeout: None,
            local_root: Some(backend.local_root().to_path_buf()),
        })
    }

    pub fn workspace_ref(&self) -> &WorkspaceRef {
        &self.workspace_ref
    }

    pub fn capabilities(&self) -> WorkspaceCapabilities {
        self.capabilities
    }

    pub fn normalize_path(&self, input: &str) -> Result<WorkspacePath> {
        self.path_resolver.normalize(input)
    }

    pub fn fs(&self) -> Arc<dyn WorkspaceFileSystem> {
        Arc::clone(&self.file_system)
    }

    /// Optional compare-and-swap file system extensions.
    ///
    /// Returns `Some` when the backend supports version-aware writes (e.g.
    /// S3 via ETag). Tools that perform read-modify-write cycles should
    /// route through [`Self::read_for_edit`] and [`Self::write_for_edit`]
    /// rather than touching this directly.
    pub fn fs_ext(&self) -> Option<Arc<dyn WorkspaceFileSystemExt>> {
        self.file_system_ext.clone()
    }

    pub fn text_reader(&self) -> Option<Arc<dyn WorkspaceTextReader>> {
        self.text_reader.clone()
    }

    pub fn command_runner(&self) -> Option<Arc<dyn WorkspaceCommandRunner>> {
        self.command_runner.clone()
    }

    pub fn search(&self) -> Option<Arc<dyn WorkspaceSearch>> {
        self.search.clone()
    }

    /// Optional workspace-scoped semantic code query provider.
    pub fn code_intelligence(&self) -> Option<Arc<dyn WorkspaceCodeIntelligence>> {
        self.code_intelligence.clone()
    }

    /// Optional session-local catalog shared by lexical and semantic retrieval.
    pub fn chunk_catalog(&self) -> Option<Arc<WorkspaceChunkCatalog>> {
        self.chunk_catalog.clone()
    }

    /// Optional semantic retrieval runtime bound to this workspace session.
    pub fn workspace_retrieval(&self) -> Option<Arc<WorkspaceRetrievalRuntime>> {
        self.workspace_retrieval.clone()
    }

    pub(crate) fn with_workspace_retrieval(
        &self,
        runtime: Arc<WorkspaceRetrievalRuntime>,
    ) -> Option<Arc<Self>> {
        if self.workspace_retrieval.is_some() {
            return None;
        }
        Some(Arc::new(Self {
            workspace_ref: self.workspace_ref.clone(),
            capabilities: self.capabilities,
            path_resolver: Arc::clone(&self.path_resolver),
            file_system: Arc::clone(&self.file_system),
            file_system_ext: self.file_system_ext.clone(),
            text_reader: self.text_reader.clone(),
            command_runner: self.command_runner.clone(),
            search: self.search.clone(),
            code_intelligence: self.code_intelligence.clone(),
            chunk_catalog: self.chunk_catalog.clone(),
            workspace_retrieval: Some(runtime),
            git: self.git.clone(),
            git_stash: self.git_stash.clone(),
            git_worktree: self.git_worktree.clone(),
            operation_timeout: self.operation_timeout,
            local_root: self.local_root.clone(),
        }))
    }

    /// Attach a semantic code query provider while preserving every existing
    /// workspace capability and backend.
    pub fn with_code_intelligence(
        &self,
        provider: Arc<dyn WorkspaceCodeIntelligence>,
    ) -> Arc<Self> {
        let mut capabilities = self.capabilities;
        capabilities.code_intelligence = true;
        Arc::new(Self {
            workspace_ref: self.workspace_ref.clone(),
            capabilities,
            path_resolver: Arc::clone(&self.path_resolver),
            file_system: Arc::clone(&self.file_system),
            file_system_ext: self.file_system_ext.clone(),
            text_reader: self.text_reader.clone(),
            command_runner: self.command_runner.clone(),
            search: self.search.clone(),
            code_intelligence: Some(provider),
            chunk_catalog: self.chunk_catalog.clone(),
            workspace_retrieval: self.workspace_retrieval.clone(),
            git: self.git.clone(),
            git_stash: self.git_stash.clone(),
            git_worktree: self.git_worktree.clone(),
            operation_timeout: self.operation_timeout,
            local_root: self.local_root.clone(),
        })
    }

    pub fn git(&self) -> Option<Arc<dyn WorkspaceGit>> {
        self.git.clone()
    }

    pub fn git_stash(&self) -> Option<Arc<dyn WorkspaceGitStashProvider>> {
        self.git_stash.clone()
    }

    pub fn git_worktree(&self) -> Option<Arc<dyn WorkspaceGitWorktreeProvider>> {
        self.git_worktree.clone()
    }

    /// Internal helper used by decorators (`with_remote_git` and any
    /// future git-provider override) to swap the git layer of an existing
    /// `WorkspaceServices` without losing unrelated fields.
    ///
    /// Every field is **explicitly listed** in the returned struct
    /// literal. This is the point of the helper — adding a new field to
    /// `WorkspaceServices` will trip a compile error here, and the author
    /// of that new field has to decide whether a git-provider swap
    /// preserves it. Previously the decorator went through
    /// `WorkspaceServicesBuilder`, which silently dropped any field the
    /// builder did not know about (notably `local_root`).
    ///
    /// `git_worktree` is reset to `None` because worktree operations are
    /// part of the same domain as the git provider — keeping the local
    /// worktree provider while routing `status`/`log`/`diff` to a remote
    /// server would surface inconsistent state to the model.
    pub(crate) fn with_git_provider(
        &self,
        git: Arc<dyn WorkspaceGit>,
        git_stash: Option<Arc<dyn WorkspaceGitStashProvider>>,
    ) -> Arc<Self> {
        let mut capabilities = self.capabilities;
        capabilities.git = true;
        Arc::new(Self {
            workspace_ref: self.workspace_ref.clone(),
            capabilities,
            path_resolver: Arc::clone(&self.path_resolver),
            file_system: Arc::clone(&self.file_system),
            file_system_ext: self.file_system_ext.clone(),
            text_reader: self.text_reader.clone(),
            command_runner: self.command_runner.clone(),
            search: self.search.clone(),
            code_intelligence: self.code_intelligence.clone(),
            chunk_catalog: self.chunk_catalog.clone(),
            workspace_retrieval: self.workspace_retrieval.clone(),
            git: Some(git),
            git_stash,
            git_worktree: None,
            operation_timeout: self.operation_timeout,
            local_root: self.local_root.clone(),
        })
    }

    /// Default timeout applied to non-bash workspace operations.
    ///
    /// `None` means no enforced timeout. Backends that may stall (remote,
    /// browser, DFS) should set this so tools using [`Self::run_with_timeout`]
    /// surface a timeout error instead of letting the agent loop hang.
    pub fn operation_timeout(&self) -> Option<std::time::Duration> {
        self.operation_timeout
    }

    /// Run a workspace future under the configured operation timeout.
    ///
    /// Tools that route through file system / search / git providers should
    /// wrap their calls with this helper so non-local backends never stall
    /// the agent loop indefinitely.
    ///
    /// Polymorphic in the error type so the helper works equally well for
    /// futures returning `anyhow::Result<T>` (the legacy callers — search,
    /// git, etc.) and for futures returning [`WorkspaceResult<T>`] (the
    /// migrated `WorkspaceFileSystem` callers). The `E: From<anyhow::Error>`
    /// bound is satisfied by both `anyhow::Error` (trivially) and
    /// [`WorkspaceError`] (via its `#[from]` `Backend` variant); a timeout
    /// surfaces as that From conversion of an `anyhow!(...)` message.
    pub async fn run_with_timeout<F, T, E>(
        &self,
        op: &'static str,
        fut: F,
    ) -> std::result::Result<T, E>
    where
        F: std::future::Future<Output = std::result::Result<T, E>>,
        E: From<anyhow::Error>,
    {
        match self.operation_timeout {
            Some(d) => tokio::time::timeout(d, fut).await.map_err(|_| {
                E::from(anyhow!(
                    "workspace operation '{}' timed out after {:?}",
                    op,
                    d
                ))
            })?,
            None => fut.await,
        }
    }

    /// Read a file for a subsequent modify-write cycle, requesting a version
    /// token when the backend supports compare-and-swap writes.
    ///
    /// Returns `(content, Some(version))` when [`Self::fs_ext`] is available
    /// (e.g. on S3, where the version is the object ETag); `(content, None)`
    /// otherwise. Pair with [`Self::write_for_edit`].
    pub async fn read_for_edit(
        &self,
        path: &WorkspacePath,
    ) -> WorkspaceResult<(String, Option<String>)> {
        if let Some(ext) = self.fs_ext() {
            let path = path.clone();
            return self
                .run_with_timeout("read_text_with_version", async move {
                    let (content, version) = ext.read_text_with_version(&path).await?;
                    Ok((content, Some(version)))
                })
                .await;
        }
        let fs = self.fs();
        let path_owned = path.clone();
        let content = self
            .run_with_timeout("read_text", async move { fs.read_text(&path_owned).await })
            .await?;
        Ok((content, None))
    }

    /// Companion to [`Self::read_for_edit`]. Performs a compare-and-swap
    /// write when both [`Self::fs_ext`] is available *and* a version token
    /// was returned by the prior read; falls back to a plain write
    /// otherwise. On version mismatch the returned error is the typed
    /// [`WorkspaceError::VersionConflict`] variant; callers can also still
    /// downcast `anyhow::Error::downcast_ref::<WorkspaceVersionConflict>()`
    /// when the value has been lifted into an `anyhow::Result`.
    pub async fn write_for_edit(
        &self,
        path: &WorkspacePath,
        content: &str,
        expected_version: Option<&str>,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        if let (Some(ext), Some(version)) = (self.fs_ext(), expected_version) {
            let path = path.clone();
            let content = content.to_string();
            let expected = version.to_string();
            return self
                .run_with_timeout("write_text_if_version", async move {
                    ext.write_text_if_version(&path, &content, &expected).await
                })
                .await;
        }
        let fs = self.fs();
        let path = path.clone();
        let content = content.to_string();
        self.run_with_timeout(
            "write_text",
            async move { fs.write_text(&path, &content).await },
        )
        .await
    }

    pub fn local_root(&self) -> Option<&Path> {
        self.local_root.as_deref()
    }

    pub fn display_path(&self, path: &WorkspacePath) -> String {
        if path.is_root() {
            return self.workspace_ref.display_root.clone();
        }

        let root = self.workspace_ref.display_root.trim_end_matches('/');
        if root.is_empty() {
            path.as_str().to_string()
        } else {
            format!("{root}/{}", path.as_str())
        }
    }
}

/// Builder for assembling workspace services without constructor arity churn.
pub struct WorkspaceServicesBuilder {
    workspace_ref: WorkspaceRef,
    capabilities: WorkspaceCapabilities,
    path_resolver: Arc<dyn WorkspacePathResolver>,
    file_system: Arc<dyn WorkspaceFileSystem>,
    file_system_ext: Option<Arc<dyn WorkspaceFileSystemExt>>,
    text_reader: Option<Arc<dyn WorkspaceTextReader>>,
    command_runner: Option<Arc<dyn WorkspaceCommandRunner>>,
    search: Option<Arc<dyn WorkspaceSearch>>,
    code_intelligence: Option<Arc<dyn WorkspaceCodeIntelligence>>,
    chunk_catalog: Option<Arc<WorkspaceChunkCatalog>>,
    git: Option<Arc<dyn WorkspaceGit>>,
    git_stash: Option<Arc<dyn WorkspaceGitStashProvider>>,
    git_worktree: Option<Arc<dyn WorkspaceGitWorktreeProvider>>,
    operation_timeout: Option<std::time::Duration>,
}

impl WorkspaceServicesBuilder {
    pub fn new(workspace_ref: WorkspaceRef, file_system: Arc<dyn WorkspaceFileSystem>) -> Self {
        Self {
            workspace_ref,
            capabilities: WorkspaceCapabilities::read_write(),
            path_resolver: Arc::new(VirtualPathResolver),
            file_system,
            file_system_ext: None,
            text_reader: None,
            command_runner: None,
            search: None,
            code_intelligence: None,
            chunk_catalog: None,
            git: None,
            git_stash: None,
            git_worktree: None,
            operation_timeout: None,
        }
    }

    pub fn capabilities(mut self, capabilities: WorkspaceCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn command_runner(mut self, command_runner: Arc<dyn WorkspaceCommandRunner>) -> Self {
        self.capabilities.exec = true;
        self.command_runner = Some(command_runner);
        self
    }

    pub fn search(mut self, search: Arc<dyn WorkspaceSearch>) -> Self {
        self.capabilities.search = true;
        self.search = Some(search);
        self
    }

    pub fn code_intelligence(mut self, provider: Arc<dyn WorkspaceCodeIntelligence>) -> Self {
        self.capabilities.code_intelligence = true;
        self.code_intelligence = Some(provider);
        self
    }

    /// Attach a typed, session-local workspace retrieval catalog.
    pub fn chunk_catalog(mut self, catalog: Arc<WorkspaceChunkCatalog>) -> Self {
        self.chunk_catalog = Some(catalog);
        self
    }

    pub fn git(mut self, git: Arc<dyn WorkspaceGit>) -> Self {
        self.capabilities.git = true;
        self.git = Some(git);
        self
    }

    pub fn git_stash(mut self, git_stash: Arc<dyn WorkspaceGitStashProvider>) -> Self {
        self.git_stash = Some(git_stash);
        self
    }

    pub fn git_worktree(mut self, git_worktree: Arc<dyn WorkspaceGitWorktreeProvider>) -> Self {
        self.git_worktree = Some(git_worktree);
        self
    }

    /// Attach optional compare-and-swap file system extensions
    /// ([`WorkspaceFileSystemExt`]). Tools that perform read-modify-write
    /// cycles will pick this up via [`WorkspaceServices::read_for_edit`]
    /// and [`WorkspaceServices::write_for_edit`].
    pub fn file_system_ext(mut self, ext: Arc<dyn WorkspaceFileSystemExt>) -> Self {
        self.file_system_ext = Some(ext);
        self
    }

    pub fn text_reader(mut self, reader: Arc<dyn WorkspaceTextReader>) -> Self {
        self.text_reader = Some(reader);
        self
    }

    /// Apply a default timeout to non-bash workspace operations (file system,
    /// search, git). Backends that may stall — remote, browser, DFS — should
    /// set this so tools surface a timeout error rather than hanging.
    pub fn operation_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.operation_timeout = Some(timeout);
        self
    }

    pub fn build(self) -> Arc<WorkspaceServices> {
        let mut services = WorkspaceServices::new_with_git(
            self.workspace_ref,
            self.capabilities,
            self.path_resolver,
            self.file_system,
            self.command_runner,
            self.search,
            self.git,
        );
        services.file_system_ext = self.file_system_ext;
        services.text_reader = self.text_reader;
        services.capabilities.code_intelligence = self.code_intelligence.is_some();
        services.code_intelligence = self.code_intelligence;
        services.chunk_catalog = self.chunk_catalog;
        services.workspace_retrieval = None;
        services.git_stash = self.git_stash;
        services.git_worktree = self.git_worktree;
        services.operation_timeout = self.operation_timeout;
        Arc::new(services)
    }
}
