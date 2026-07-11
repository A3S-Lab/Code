//! Workspace capability abstractions.
//!
//! Built-in tools expose stable model-facing contracts (`read`, `write`, `ls`,
//! `bash`, ...). The concrete place where those operations happen is supplied
//! by a workspace capability backend. The default backend is the local
//! filesystem (see [`LocalWorkspaceBackend`]); hosts can provide remote,
//! browser, DFS, or container-backed implementations by assembling
//! [`WorkspaceServices`] through [`WorkspaceServicesBuilder`].

#[cfg(test)]
pub(crate) mod conformance;
mod error;
mod local;
mod manifest;
mod path;
mod remote_git;
#[cfg(feature = "s3")]
mod s3;

pub use error::{WorkspaceError, WorkspaceResult};
pub use local::LocalWorkspaceBackend;
pub use manifest::{
    scan_workspace_files, LocalWorkspaceFile, LocalWorkspaceFileStatus, LocalWorkspaceManifest,
    LocalWorkspaceManifestSnapshot, ManifestWorkspaceBackend, RecentWorkspaceFile,
};
pub(crate) use path::validate_relative_pattern;
pub use path::VirtualPathResolver;
use path::{
    default_path_input, escape_control_chars_for_display, has_windows_path_prefix,
    normalize_relative_path, pathbuf_to_workspace_path,
};
pub use remote_git::{RemoteGitBackend, RemoteGitBackendConfig, RemoteGitConflict};
#[cfg(feature = "s3")]
pub use s3::{S3BackendConfig, S3WorkspaceBackend};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Identity and display metadata for a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRef {
    /// Stable workspace identifier used by host backends.
    pub id: String,
    /// Human-readable root shown in tool output.
    pub display_root: String,
}

impl WorkspaceRef {
    pub fn new(id: impl Into<String>, display_root: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_root: display_root.into(),
        }
    }
}

/// A normalized virtual path inside a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspacePath {
    inner: String,
}

impl WorkspacePath {
    pub fn root() -> Self {
        Self {
            inner: ".".to_string(),
        }
    }

    pub fn from_normalized(path: impl Into<String>) -> Self {
        let path = path.into();
        let path = path.trim_matches('/');
        if path.is_empty() || path == "." {
            Self::root()
        } else {
            Self {
                inner: path.replace('\\', "/"),
            }
        }
    }

    pub fn as_str(&self) -> &str {
        &self.inner
    }

    pub fn is_root(&self) -> bool {
        self.inner == "."
    }
}

/// Workspace capability flags used to gate which built-in tools are registered.
///
/// Each flag corresponds to a provider trait on [`WorkspaceServices`]; flags
/// without a backing provider are deliberately omitted so the surface stays
/// minimal until a real consumer appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceCapabilities {
    pub read: bool,
    pub write: bool,
    pub exec: bool,
    pub search: bool,
    pub git: bool,
}

impl WorkspaceCapabilities {
    pub fn local_default() -> Self {
        Self {
            read: true,
            write: true,
            exec: true,
            search: true,
            git: true,
        }
    }

    pub fn read_write() -> Self {
        Self {
            read: true,
            write: true,
            exec: false,
            search: false,
            git: false,
        }
    }
}

impl Default for WorkspaceCapabilities {
    fn default() -> Self {
        Self::read_write()
    }
}

/// Directory entry kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceFileType {
    File,
    Directory,
    Symlink,
    Unknown,
}

impl WorkspaceFileType {
    pub fn as_tool_kind(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "dir",
            Self::Symlink => "link",
            Self::Unknown => "unknown",
        }
    }
}

/// Directory entry returned by a workspace backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDirEntry {
    pub name: String,
    pub kind: WorkspaceFileType,
    pub size: u64,
}

/// Result metadata for a write operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWriteOutcome {
    pub bytes: usize,
    pub lines: usize,
}

/// Glob request for workspace-backed search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGlobRequest {
    pub base: WorkspacePath,
    pub pattern: String,
}

/// Glob result returned by a workspace search provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGlobResult {
    pub matches: Vec<WorkspacePath>,
}

/// Grep request for workspace-backed search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGrepRequest {
    pub base: WorkspacePath,
    pub pattern: String,
    pub glob: Option<String>,
    pub context_lines: usize,
    pub case_insensitive: bool,
    pub max_output_size: usize,
}

/// Grep result returned by a workspace search provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGrepResult {
    pub output: String,
    pub match_count: usize,
    pub file_count: usize,
    pub truncated: bool,
}

/// Grep result plus structured source evidence when supplied by a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGrepOutcome {
    pub result: WorkspaceGrepResult,
    /// Distinct paths that contributed rendered match lines, in result order.
    /// `None` denotes a legacy/custom backend with display output only.
    pub matched_paths: Option<Vec<WorkspacePath>>,
}

/// Repository status returned by a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitStatus {
    pub branch: String,
    pub commit: String,
    pub is_worktree: bool,
    pub is_dirty: bool,
    pub dirty_count: usize,
}

/// Commit information returned by a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitCommit {
    pub id: String,
    pub message: String,
    pub author: String,
    pub date: String,
}

/// Branch information returned by a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitBranch {
    pub name: String,
    pub is_current: bool,
}

/// Branch creation request for a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitCreateBranchRequest {
    pub name: String,
    pub base: String,
}

/// Checkout request for a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitCheckoutRequest {
    pub refspec: String,
    pub force: bool,
}

/// Checkout output returned by a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitCheckoutOutput {
    pub stdout: String,
}

/// Diff request for a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitDiffRequest {
    pub target: Option<String>,
}

/// Stash information returned by a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitStash {
    pub index: usize,
    pub message: String,
}

/// Stash request for a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitStashRequest {
    pub message: Option<String>,
    pub include_untracked: bool,
}

/// Remote information returned by a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitRemote {
    pub name: String,
    pub url: String,
    pub direction: String,
}

/// Worktree information returned by a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitWorktree {
    pub path: String,
    pub branch: String,
    pub is_bare: bool,
    pub is_detached: bool,
}

/// Worktree creation request for a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitCreateWorktreeRequest {
    pub branch: String,
    pub path: Option<String>,
    pub new_branch: bool,
}

/// Worktree removal request for a workspace Git provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitRemoveWorktreeRequest {
    pub path: String,
    pub force: bool,
}

/// Mutation result for workspace Git worktree operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGitWorktreeMutation {
    pub path: String,
    pub branch: Option<String>,
}

/// Observer that receives streaming output deltas from a workspace command.
///
/// Backend implementations call this on each chunk of stdout/stderr they
/// observe. Tool layers wire host event channels behind this trait, so the
/// workspace abstraction does not depend on any tool event type.
#[async_trait]
pub trait CommandOutputObserver: Send + Sync {
    async fn on_output_delta(&self, delta: &str);
}

/// Command execution request.
#[derive(Clone)]
pub struct CommandRequest {
    pub command: String,
    pub timeout_ms: u64,
    pub output_observer: Option<Arc<dyn CommandOutputObserver>>,
    pub env: Option<Arc<HashMap<String, String>>>,
}

impl std::fmt::Debug for CommandRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandRequest")
            .field("command", &self.command)
            .field("timeout_ms", &self.timeout_ms)
            .field("output_observer", &self.output_observer.is_some())
            .field("env", &self.env.as_ref().map(|env| env.len()))
            .finish()
    }
}

/// Command execution output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub output: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

/// Normalizes and validates host-supplied paths before they reach a backend.
pub trait WorkspacePathResolver: Send + Sync {
    fn normalize(&self, input: &str) -> Result<WorkspacePath>;
}

/// File operations available to built-in file tools.
///
/// **Trait stability policy:** new methods added to this trait are a breaking
/// change for every external backend implementation. Until the workspace
/// extension story is stabilised, new methods will be added to a separate
/// `WorkspaceFileSystemExt` trait (with default implementations that fall back
/// to the core methods) rather than to this trait directly. Backend authors
/// can rely on this trait surface remaining additive only through extension
/// traits.
#[async_trait]
pub trait WorkspaceFileSystem: Send + Sync {
    async fn read_text(&self, path: &WorkspacePath) -> WorkspaceResult<String>;
    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome>;
    async fn list_dir(&self, path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>>;
}

/// Error returned by [`WorkspaceFileSystemExt::write_text_if_version`] when
/// the underlying object version no longer matches the expected version.
///
/// Surfaced through `anyhow::Error`; tools recover by downcasting:
/// `err.downcast_ref::<WorkspaceVersionConflict>()`. The typical response is
/// to re-read the file and retry the modify-write cycle once.
#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "version conflict on {path}: expected version {expected:?}, found {actual:?} (file modified by another writer; re-read and retry)"
)]
pub struct WorkspaceVersionConflict {
    pub path: String,
    pub expected: String,
    /// Backend-reported current version, if known. S3 does not return the
    /// current ETag on `412 Precondition Failed`, so this is typically `None`.
    pub actual: Option<String>,
}

/// Optional compare-and-swap extensions to [`WorkspaceFileSystem`].
///
/// Implemented by backends that expose object-level versioning (S3 ETag,
/// future GCS generation, ...) so tools that perform read-modify-write
/// cycles can reject concurrent overwrites. Tools should access this through
/// [`WorkspaceServices::fs_ext`] — when absent, callers fall back to plain
/// `read_text` / `write_text` (last-writer-wins).
///
/// Kept as a separate trait rather than inheriting from
/// [`WorkspaceFileSystem`] so existing backend implementations are not
/// forced to opt in.
#[async_trait]
pub trait WorkspaceFileSystemExt: Send + Sync {
    /// Read text content together with an opaque version token. Tokens are
    /// backend-specific (S3 returns the ETag) and treated as opaque by
    /// callers — they are only ever compared for equality on the backend
    /// side.
    async fn read_text_with_version(
        &self,
        path: &WorkspacePath,
    ) -> WorkspaceResult<(String, String)>;

    /// Write content iff the current object version matches `expected_version`.
    /// On mismatch the returned error is the typed
    /// [`WorkspaceError::VersionConflict`] variant; callers can also still
    /// downcast through `anyhow::Error` when the value has been lifted into
    /// the legacy result type.
    async fn write_text_if_version(
        &self,
        path: &WorkspacePath,
        content: &str,
        expected_version: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome>;
}

/// Shell/command execution available to the `bash` tool.
#[async_trait]
pub trait WorkspaceCommandRunner: Send + Sync {
    async fn exec(&self, request: CommandRequest) -> Result<CommandOutput>;
}

/// Search operations available to `glob` and `grep`.
#[async_trait]
pub trait WorkspaceSearch: Send + Sync {
    async fn glob(&self, request: WorkspaceGlobRequest) -> Result<WorkspaceGlobResult>;
    async fn grep(&self, request: WorkspaceGrepRequest) -> Result<WorkspaceGrepResult>;

    /// Run grep with structured source paths when the backend can provide them.
    ///
    /// The default preserves compatibility for custom backends implementing
    /// only [`Self::grep`]. Callers must treat its display output as untrusted.
    async fn grep_with_sources(
        &self,
        request: WorkspaceGrepRequest,
    ) -> Result<WorkspaceGrepOutcome> {
        let result = self.grep(request).await?;
        Ok(WorkspaceGrepOutcome {
            result,
            matched_paths: None,
        })
    }
}

/// Core Git operations supported by virtually every workspace Git backend.
///
/// Optional features (stash, worktrees) live in separate traits so backends
/// like browser-side `isomorphic-git` can implement only what they support
/// instead of returning runtime "unsupported" errors.
#[async_trait]
pub trait WorkspaceGit: Send + Sync {
    async fn is_repository(&self) -> Result<bool>;
    async fn status(&self) -> Result<WorkspaceGitStatus>;
    async fn log(&self, max_count: usize) -> Result<Vec<WorkspaceGitCommit>>;
    async fn list_branches(&self) -> Result<Vec<WorkspaceGitBranch>>;
    async fn create_branch(&self, request: WorkspaceGitCreateBranchRequest) -> Result<()>;
    async fn checkout(
        &self,
        request: WorkspaceGitCheckoutRequest,
    ) -> Result<WorkspaceGitCheckoutOutput>;
    async fn diff(&self, request: WorkspaceGitDiffRequest) -> Result<String>;
    async fn list_remotes(&self) -> Result<Vec<WorkspaceGitRemote>>;
}

/// Optional Git stash operations.
///
/// Browser-side libraries such as `isomorphic-git` do not implement stash;
/// backends that cannot stash simply do not implement this trait.
#[async_trait]
pub trait WorkspaceGitStashProvider: Send + Sync {
    async fn list_stashes(&self) -> Result<Vec<WorkspaceGitStash>>;
    async fn stash(&self, request: WorkspaceGitStashRequest) -> Result<()>;
}

/// Optional Git worktree operations.
///
/// Worktrees are a local-filesystem concept and are typically not supported
/// by remote or browser-backed git providers.
#[async_trait]
pub trait WorkspaceGitWorktreeProvider: Send + Sync {
    async fn list_worktrees(&self) -> Result<Vec<WorkspaceGitWorktree>>;
    async fn create_worktree(
        &self,
        request: WorkspaceGitCreateWorktreeRequest,
    ) -> Result<WorkspaceGitWorktreeMutation>;
    async fn remove_worktree(
        &self,
        request: WorkspaceGitRemoveWorktreeRequest,
    ) -> Result<WorkspaceGitWorktreeMutation>;
}

/// The host-provided workspace capability bundle used by tool execution.
pub struct WorkspaceServices {
    workspace_ref: WorkspaceRef,
    capabilities: WorkspaceCapabilities,
    path_resolver: Arc<dyn WorkspacePathResolver>,
    file_system: Arc<dyn WorkspaceFileSystem>,
    file_system_ext: Option<Arc<dyn WorkspaceFileSystemExt>>,
    command_runner: Option<Arc<dyn WorkspaceCommandRunner>>,
    search: Option<Arc<dyn WorkspaceSearch>>,
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
            .field("command_runner", &self.command_runner.is_some())
            .field("search", &self.search.is_some())
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
        Self {
            workspace_ref,
            capabilities,
            path_resolver,
            file_system,
            file_system_ext: None,
            command_runner,
            search,
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
            command_runner: Some(command_runner),
            search: Some(search),
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

    /// Build local workspace services from a shared manifest backend. Hosts
    /// can keep the same manifest for UI file pickers and agent tools.
    pub fn local_with_manifest_backend(backend: Arc<ManifestWorkspaceBackend>) -> Arc<Self> {
        let workspace_ref = WorkspaceRef::new(
            backend.local_root().display().to_string(),
            backend.local_root().display().to_string(),
        );
        let path_resolver: Arc<dyn WorkspacePathResolver> = backend.clone();
        let file_system: Arc<dyn WorkspaceFileSystem> = backend.clone();
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
            command_runner: Some(command_runner),
            search: Some(search),
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

    pub fn command_runner(&self) -> Option<Arc<dyn WorkspaceCommandRunner>> {
        self.command_runner.clone()
    }

    pub fn search(&self) -> Option<Arc<dyn WorkspaceSearch>> {
        self.search.clone()
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
            command_runner: self.command_runner.clone(),
            search: self.search.clone(),
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
    command_runner: Option<Arc<dyn WorkspaceCommandRunner>>,
    search: Option<Arc<dyn WorkspaceSearch>>,
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
            command_runner: None,
            search: None,
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
        services.git_stash = self.git_stash;
        services.git_worktree = self.git_worktree;
        services.operation_timeout = self.operation_timeout;
        Arc::new(services)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
