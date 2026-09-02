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
mod local_access;
mod manifest;
mod path;
mod remote_git;
pub(crate) mod retrieval;
#[cfg(feature = "s3")]
mod s3;
mod services;
mod source_egress;

pub use error::{WorkspaceError, WorkspaceResult};
pub use local::LocalWorkspaceBackend;
pub use local_access::LocalWorkspaceAccessPolicy;
pub use manifest::{
    scan_workspace_files, LocalWorkspaceFile, LocalWorkspaceFileStatus, LocalWorkspaceManifest,
    LocalWorkspaceManifestSnapshot, ManifestWorkspaceBackend, RecentWorkspaceFile,
    WorkspaceFileChange, WorkspaceFileChangeKind,
};
pub use path::VirtualPathResolver;
use path::{
    default_path_input, has_windows_path_prefix, normalize_relative_path, pathbuf_to_workspace_path,
};
pub(crate) use path::{escape_control_chars_for_display, validate_relative_pattern};
pub use remote_git::{RemoteGitBackend, RemoteGitBackendConfig, RemoteGitConflict};
pub use retrieval::{
    ChunkCatalogLimits, ChunkCatalogSnapshot, ChunkingConfig, CustomWorkspaceChunkingStrategy,
    FixedWindowChunkingOptions, LexicalSearchHit, LexicalSearchRequest, LexicalSearchResult,
    RecursiveChunkingOptions, WorkspaceChunk, WorkspaceChunkCatalog, WorkspaceChunkId,
    WorkspaceChunkRange, WorkspaceChunkingError, WorkspaceChunkingInput, WorkspaceChunkingStrategy,
    WorkspaceEligibilityPolicy, WorkspaceEmbeddingBatchMetrics, WorkspaceHybridChannelRank,
    WorkspaceHybridChannelStatus, WorkspaceHybridFallbackReason, WorkspaceHybridSearchHit,
    WorkspaceHybridSearchRequest, WorkspaceHybridSearchResult, WorkspaceIndexError,
    WorkspaceRerankAlgorithm, WorkspaceRerankFallbackReason, WorkspaceRerankMode,
    WorkspaceRerankOptions, WorkspaceRerankStatus, WorkspaceRetrievalChannel,
    WorkspaceRetrievalError, WorkspaceRetrievalOptions, WorkspaceRetrievalPhase,
    WorkspaceRetrievalResult, WorkspaceRetrievalRuntime, WorkspaceRetrievalStatus,
    WorkspaceSemanticFallbackReason, WorkspaceSemanticIndexLimits, WorkspaceSemanticSearchHit,
    WorkspaceSemanticSearchRequest, WorkspaceSemanticSearchResult, WorkspaceVecShadowPhase,
    WorkspaceVecShadowStatus, WorkspaceVectorEngine,
};
#[cfg(feature = "s3")]
pub use s3::{S3BackendConfig, S3WorkspaceBackend};
pub use services::{WorkspaceServices, WorkspaceServicesBuilder};

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
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
    pub code_intelligence: bool,
}

impl WorkspaceCapabilities {
    pub fn local_default() -> Self {
        Self {
            read: true,
            write: true,
            exec: true,
            search: true,
            git: true,
            code_intelligence: false,
        }
    }

    pub fn read_write() -> Self {
        Self {
            read: true,
            write: true,
            exec: false,
            search: false,
            git: false,
            code_intelligence: false,
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
    /// Maximum rendered match bytes. Zero requests a metadata-only scan:
    /// backends count every matching line and collect distinct source paths
    /// without constructing match text.
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

    /// Receive the final bounded-capture accounting for the command.
    ///
    /// The default keeps existing remote workspace runners source-compatible.
    /// Runners that bound output should report the original byte count so
    /// callers can distinguish a complete result from a partial observation.
    async fn on_output_complete(&self, _summary: &CommandOutputSummary) {}
}

/// Final accounting for a bounded command-output capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandOutputSummary {
    /// Total stdout and stderr bytes observed before completion or timeout.
    pub total_bytes: usize,
    /// Original process bytes retained in the rendered output.
    pub captured_bytes: usize,
    /// Whether bytes were omitted from the middle of the rendered output.
    pub truncated: bool,
    /// Whether command execution reached its own deadline.
    pub timed_out: bool,
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

/// A bounded text range returned by an optional streaming workspace reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceTextRange {
    pub lines: Vec<String>,
    pub next_offset: Option<usize>,
    pub eof: bool,
    /// Exact line count when EOF was observed while satisfying the request.
    pub total_lines: Option<usize>,
}

/// Optional streaming text capability for backends that can avoid loading a
/// complete file when a caller needs only a line range.
#[async_trait]
pub trait WorkspaceTextReader: Send + Sync {
    async fn read_text_range(
        &self,
        path: &WorkspacePath,
        offset: usize,
        limit: usize,
    ) -> WorkspaceResult<WorkspaceTextRange>;
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
