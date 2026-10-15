//! Local filesystem-backed workspace implementation.
//!
//! [`LocalWorkspaceBackend`] preserves the historical "agent runs on the host
//! filesystem" behavior. It implements every workspace capability trait so
//! local sessions get the full tool surface (read, write, edit, patch, ls,
//! bash, grep, glob, git, git_stash, git_worktree).

use super::local_access::{LocalWorkspaceAccessBoundary, LocalWorkspaceAccessPolicy};
use super::DirectWriteGuard;
use super::{
    default_path_input, escape_control_chars_for_display, has_windows_path_prefix,
    normalize_relative_path, pathbuf_to_workspace_path, validate_relative_pattern, CommandOutput,
    CommandRequest, WorkspaceCommandRunner, WorkspaceDirEntry, WorkspaceError, WorkspaceFileSystem,
    WorkspaceFileType, WorkspaceGit, WorkspaceGitBranch, WorkspaceGitCheckoutOutput,
    WorkspaceGitCheckoutRequest, WorkspaceGitCommit, WorkspaceGitCreateBranchRequest,
    WorkspaceGitCreateWorktreeRequest, WorkspaceGitDiffRequest, WorkspaceGitRemote,
    WorkspaceGitRemoveWorktreeRequest, WorkspaceGitStash, WorkspaceGitStashProvider,
    WorkspaceGitStashRequest, WorkspaceGitStatus, WorkspaceGitWorktree,
    WorkspaceGitWorktreeMutation, WorkspaceGitWorktreeProvider, WorkspaceGlobRequest,
    WorkspaceGlobResult, WorkspaceGrepOutcome, WorkspaceGrepRequest, WorkspaceGrepResult,
    WorkspacePath, WorkspacePathResolver, WorkspaceResult, WorkspaceSearch, WorkspaceTextRange,
    WorkspaceTextReader, WorkspaceWriteOutcome,
};
use crate::sandbox::native::hard_link_count_for_open_file;
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// Per-file byte bound for text reads that materialize a file in memory.
///
/// `read_text` (edit/grep verification/bm25/retrieval reconcile share it)
/// and the ranged reader (a file with no newlines is one unbounded line)
/// refuse files above this size with [`WorkspaceError::TooLarge`] instead
/// of allocating: one oversized file must not turn into an OOM that kills
/// every colocated workload. 16 MiB matches the artifact-store content
/// bound and sits far beyond what any consumer — model context, edit
/// diff, chunked index — processes in one piece.
const MAX_TEXT_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Local filesystem-backed workspace implementation.
#[derive(Debug)]
pub struct LocalWorkspaceBackend {
    pub(super) root: PathBuf,
    access_boundary: Option<Arc<LocalWorkspaceAccessBoundary>>,
}

struct CancelGitWorkerOnDrop {
    cancellation: Arc<AtomicBool>,
    armed: bool,
}

impl CancelGitWorkerOnDrop {
    fn new(cancellation: Arc<AtomicBool>) -> Self {
        Self {
            cancellation,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelGitWorkerOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.cancellation.store(true, Ordering::Release);
        }
    }
}

impl LocalWorkspaceBackend {
    pub fn new(root: PathBuf) -> Self {
        Self::new_with_access_policy(root, LocalWorkspaceAccessPolicy::Unrestricted)
    }

    pub fn new_with_access_policy(
        root: PathBuf,
        access_policy: LocalWorkspaceAccessPolicy,
    ) -> Self {
        Self::new_with_boundary(root, |root| {
            LocalWorkspaceAccessBoundary::for_policy(access_policy, root)
        })
    }

    pub(crate) fn new_with_source_egress_policy(root: PathBuf) -> Self {
        Self::new_with_boundary(root, |_| {
            Some(LocalWorkspaceAccessBoundary::for_source_egress())
        })
    }

    fn new_with_boundary(
        root: PathBuf,
        boundary: impl FnOnce(&Path) -> Option<LocalWorkspaceAccessBoundary>,
    ) -> Self {
        let canonical = root.canonicalize();
        let root = match canonical {
            Ok(canonical) => canonical,
            Err(e) => {
                tracing::warn!(
                    "LocalWorkspaceBackend: failed to canonicalize root '{}' at construction: {} \
                     (path resolution will fail-closed at first use)",
                    root.display(),
                    e
                );
                root
            }
        };
        let access_boundary = boundary(&root).map(Arc::new);
        Self {
            root,
            access_boundary,
        }
    }

    fn local_path_for_read(&self, path: &WorkspacePath) -> Result<PathBuf> {
        a3s_common::tools::resolve_path(&self.root, path.as_str()).map_err(|e| anyhow!("{}", e))
    }

    fn local_path_for_write(&self, path: &WorkspacePath) -> Result<PathBuf> {
        if path.is_root() {
            bail!("write path must name a file");
        }
        // Refuse before create_dir_all. A symlink directory already inside the
        // workspace would otherwise receive new parent directories outside it,
        // and a symlink file would be opened and followed by the later write.
        refuse_existing_symlink_on_write_path(&self.root, path.as_str())?;

        let target = self.root.join(path.as_str());
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow!(
                    "Failed to create parent directories for {}: {}",
                    target.display(),
                    e
                )
            })?;
        }

        a3s_common::tools::resolve_path_for_write(&self.root, path.as_str())
            .map_err(|e| anyhow!("{}", e))
    }

    pub(crate) fn refuse_direct_write(&self, path: &WorkspacePath) -> Result<()> {
        refuse_existing_symlink_on_write_path(&self.root, path.as_str())?;
        let candidate = self.root.join(path.as_str());
        let metadata = std::fs::metadata(&candidate).ok();
        self.ensure_access(path, Some(&candidate), metadata.as_ref(), None, "write")
    }

    fn ensure_access(
        &self,
        path: &WorkspacePath,
        resolved: Option<&Path>,
        metadata: Option<&std::fs::Metadata>,
        opened_hard_link_count: Option<u64>,
        operation: &'static str,
    ) -> Result<()> {
        match &self.access_boundary {
            Some(boundary) => boundary.ensure_access(
                &self.root,
                Path::new(path.as_str()),
                resolved,
                metadata,
                opened_hard_link_count,
                operation,
            ),
            None => Ok(()),
        }
    }

    pub(super) fn ensure_search_base_allowed(&self, path: &WorkspacePath) -> Result<()> {
        let resolved = self.local_path_for_read(path)?;
        let metadata = std::fs::metadata(&resolved).ok();
        self.ensure_access(path, Some(&resolved), metadata.as_ref(), None, "read")
    }

    pub(super) fn read_search_file(&self, path: &WorkspacePath) -> Option<String> {
        read_search_file_at(&self.root, self.access_boundary.as_deref(), path)
    }

    fn refuse_checkout_targets(&self, refspec: &str, force: bool) -> Result<()> {
        let mut paths = crate::git::paths_changed_between(&self.root, "HEAD", refspec)?;
        if force {
            paths.extend(crate::git::get_diff_paths(&self.root, None)?);
        }
        paths.sort();
        paths.dedup();
        for path in paths {
            self.refuse_checkout_path(&path)?;
        }
        Ok(())
    }

    fn refuse_worktree_checkout(
        &self,
        branch: &str,
        new_branch: bool,
        destination: &Path,
    ) -> Result<()> {
        let Some(destination) = resolved_workspace_destination(&self.root, destination) else {
            return Ok(());
        };
        let revision = if new_branch { "HEAD" } else { branch };
        let relative_destination = destination.strip_prefix(&self.root).unwrap_or(&destination);
        for path in crate::git::tracked_tree_paths(&self.root, revision)? {
            self.refuse_checkout_path(&relative_destination.join(path))?;
        }
        Ok(())
    }

    fn refuse_stash_targets(&self, include_untracked: bool) -> Result<()> {
        let mut paths = crate::git::get_diff_paths(&self.root, None)?;
        paths.extend(crate::git::staged_diff_paths(&self.root)?);
        if include_untracked {
            paths.extend(crate::git::untracked_paths(&self.root)?);
        }
        paths.sort();
        paths.dedup();
        for path in paths {
            self.refuse_checkout_path(&path)?;
        }
        Ok(())
    }

    fn refuse_checkout_path(&self, path: &Path) -> Result<()> {
        let path_text = path
            .to_str()
            .ok_or_else(|| anyhow!("checkout path is not utf-8"))?;
        let workspace_path = normalize_local_path(&self.root, path_text)?;
        let candidate = self.root.join(path);
        let metadata = std::fs::metadata(&candidate).ok();
        self.ensure_access(
            &workspace_path,
            Some(&candidate),
            metadata.as_ref(),
            None,
            "write",
        )
    }

    fn git_diff_path_allowed(&self, path: &Path) -> bool {
        let Some(path_text) = path.to_str() else {
            return false;
        };
        let Ok(workspace_path) = normalize_local_path(&self.root, path_text) else {
            return false;
        };
        let candidate = self.root.join(path);
        let resolved = candidate.canonicalize().ok();
        let metadata = resolved
            .as_deref()
            .and_then(|resolved| std::fs::metadata(resolved).ok());
        self.ensure_access(
            &workspace_path,
            resolved.as_deref(),
            metadata.as_ref(),
            None,
            "read",
        )
        .is_ok()
    }
}

impl DirectWriteGuard for LocalWorkspaceBackend {
    fn refuse_direct_write(&self, path: &WorkspacePath) -> Result<()> {
        LocalWorkspaceBackend::refuse_direct_write(self, path)
    }
}

impl WorkspacePathResolver for LocalWorkspaceBackend {
    fn normalize(&self, input: &str) -> Result<WorkspacePath> {
        normalize_local_path(&self.root, input)
    }
}

#[async_trait]
impl WorkspaceFileSystem for LocalWorkspaceBackend {
    async fn read_text(&self, path: &WorkspacePath) -> WorkspaceResult<String> {
        let resolved = self.local_path_for_read(path)?;
        // FIFO/device liveness gate: opening a writer-less FIFO blocks, so
        // the regular-file check must run before the open (metadata follows
        // symlinks; the open-file re-check below stays authoritative for
        // the swap race). A stat error falls through to the open's own
        // error mapping, keeping the not-found contract unchanged.
        if let Ok(metadata) = tokio::fs::metadata(&resolved).await {
            if !metadata.is_file() {
                return Err(WorkspaceError::Backend(anyhow!(
                    "Refusing to read {}: not a regular file",
                    resolved.display()
                )));
            }
        }
        let file = match tokio::fs::File::open(&resolved).await {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(WorkspaceError::NotFound {
                    path: resolved.display().to_string(),
                })
            }
            Err(e) => {
                return Err(WorkspaceError::Backend(anyhow!(
                    "Failed to open file {}: {}",
                    resolved.display(),
                    e
                )))
            }
        };
        let metadata = file.metadata().await.map_err(|error| {
            WorkspaceError::Backend(anyhow!(
                "Failed to inspect file {}: {}",
                resolved.display(),
                error
            ))
        })?;
        self.ensure_access(
            path,
            Some(&resolved),
            Some(&metadata),
            self.access_boundary
                .as_ref()
                .map(|_| hard_link_count_for_open_file(&file, &metadata)),
            "read",
        )?;
        // A non-regular file (FIFO, device) has no meaningful text size:
        // refuse it instead of blocking on an open-ended read.
        if !metadata.is_file() {
            return Err(WorkspaceError::Backend(anyhow!(
                "Refusing to read {}: not a regular file",
                resolved.display()
            )));
        }
        if metadata.len() > MAX_TEXT_FILE_BYTES {
            return Err(WorkspaceError::TooLarge {
                path: path.as_str().to_string(),
                size_bytes: metadata.len(),
                max_bytes: MAX_TEXT_FILE_BYTES,
            });
        }

        // The metadata check above covers the observed size; `take` bounds
        // the allocation itself if the file grows between stat and read
        // (a byte straddling the UTF-8 boundary then surfaces as a read
        // error — still fail-closed, never an unbounded String).
        let mut content = String::new();
        file.take(MAX_TEXT_FILE_BYTES + 1)
            .read_to_string(&mut content)
            .await
            .map_err(|error| {
                WorkspaceError::Backend(anyhow!(
                    "Failed to read file {}: {}",
                    resolved.display(),
                    error
                ))
            })?;
        if content.len() as u64 > MAX_TEXT_FILE_BYTES {
            return Err(WorkspaceError::TooLarge {
                path: path.as_str().to_string(),
                size_bytes: content.len() as u64,
                max_bytes: MAX_TEXT_FILE_BYTES,
            });
        }
        Ok(content)
    }

    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        self.ensure_access(path, None, None, None, "write")?;
        let resolved = self.local_path_for_write(path)?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&resolved)
            .await
            .map_err(|e| {
                WorkspaceError::Backend(anyhow!(
                    "Failed to open file {} for writing: {}",
                    resolved.display(),
                    e
                ))
            })?;
        opened_write_stays_in_workspace(&file, &self.root).map_err(|error| {
            WorkspaceError::Backend(anyhow!(
                "Failed to write file {}: {}",
                resolved.display(),
                error
            ))
        })?;
        let metadata = file.metadata().await.map_err(|error| {
            WorkspaceError::Backend(anyhow!(
                "Failed to inspect file {} before writing: {}",
                resolved.display(),
                error
            ))
        })?;
        self.ensure_access(
            path,
            Some(&resolved),
            Some(&metadata),
            self.access_boundary
                .as_ref()
                .map(|_| hard_link_count_for_open_file(&file, &metadata)),
            "write",
        )?;
        file.set_len(0).await.map_err(|e| {
            WorkspaceError::Backend(anyhow!(
                "Failed to write file {}: {}",
                resolved.display(),
                e
            ))
        })?;
        file.write_all(content.as_bytes()).await.map_err(|e| {
            WorkspaceError::Backend(anyhow!(
                "Failed to write file {}: {}",
                resolved.display(),
                e
            ))
        })?;
        file.flush().await.map_err(|e| {
            WorkspaceError::Backend(anyhow!(
                "Failed to flush file {} after writing: {}",
                resolved.display(),
                e
            ))
        })?;

        Ok(WorkspaceWriteOutcome {
            bytes: content.len(),
            lines: content.lines().count(),
        })
    }

    async fn list_dir(&self, path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
        let target = self.local_path_for_read(path)?;
        if !target.exists() {
            return Err(WorkspaceError::NotFound {
                path: target.display().to_string(),
            });
        }
        if !target.is_dir() {
            return Err(WorkspaceError::InvalidArgument {
                message: format!("Not a directory: {}", target.display()),
            });
        }

        let mut dir = tokio::fs::read_dir(&target).await.map_err(|e| {
            WorkspaceError::Backend(anyhow!(
                "Failed to read directory {}: {}",
                target.display(),
                e
            ))
        })?;
        let mut entries = Vec::new();

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| WorkspaceError::Backend(anyhow!("Failed to iterate directory: {}", e)))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().await;
            let metadata = entry.metadata().await;
            let (kind, size) = match (&file_type, &metadata) {
                (Ok(ft), Ok(m)) => {
                    let kind = if ft.is_dir() {
                        WorkspaceFileType::Directory
                    } else if ft.is_symlink() {
                        WorkspaceFileType::Symlink
                    } else {
                        WorkspaceFileType::File
                    };
                    (kind, m.len())
                }
                _ => (WorkspaceFileType::Unknown, 0),
            };
            entries.push(WorkspaceDirEntry { name, kind, size });
        }

        Ok(entries)
    }
}

#[async_trait]
impl WorkspaceTextReader for LocalWorkspaceBackend {
    async fn read_text_range(
        &self,
        path: &WorkspacePath,
        offset: usize,
        limit: usize,
    ) -> WorkspaceResult<WorkspaceTextRange> {
        let resolved = self.local_path_for_read(path)?;
        // FIFO/device liveness gate: opening a writer-less FIFO blocks, so
        // the regular-file check must run before the open (metadata follows
        // symlinks; the open-file re-check below stays authoritative for
        // the swap race). A stat error falls through to the open's own
        // error mapping, keeping the not-found contract unchanged.
        if let Ok(metadata) = tokio::fs::metadata(&resolved).await {
            if !metadata.is_file() {
                return Err(WorkspaceError::Backend(anyhow!(
                    "Refusing to read {}: not a regular file",
                    resolved.display()
                )));
            }
        }
        let file = tokio::fs::File::open(&resolved).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                WorkspaceError::NotFound {
                    path: resolved.display().to_string(),
                }
            } else {
                WorkspaceError::Backend(anyhow!(
                    "Failed to open file {}: {}",
                    resolved.display(),
                    error
                ))
            }
        })?;
        let metadata = file.metadata().await.map_err(|error| {
            WorkspaceError::Backend(anyhow!(
                "Failed to inspect file {}: {}",
                resolved.display(),
                error
            ))
        })?;
        self.ensure_access(
            path,
            Some(&resolved),
            Some(&metadata),
            self.access_boundary
                .as_ref()
                .map(|_| hard_link_count_for_open_file(&file, &metadata)),
            "read",
        )?;
        // A non-regular file (FIFO, device) has no meaningful line window:
        // refuse it instead of blocking on an open-ended read.
        if !metadata.is_file() {
            return Err(WorkspaceError::Backend(anyhow!(
                "Refusing to read {}: not a regular file",
                resolved.display()
            )));
        }
        // The line window is bounded, but a file with no newlines would
        // hand the ranged reader one unbounded line — apply the same
        // whole-file bound as `read_text`. The `take` cap plus the
        // consumed-byte accounting close the growth race: a file that grew
        // past the bound between stat and read surfaces as TooLarge
        // instead of a silently truncated window pretending to be EOF.
        if metadata.len() > MAX_TEXT_FILE_BYTES {
            return Err(WorkspaceError::TooLarge {
                path: path.as_str().to_string(),
                size_bytes: metadata.len(),
                max_bytes: MAX_TEXT_FILE_BYTES,
            });
        }
        let mut consumed = 0usize;
        let mut lines = BufReader::new(file.take(MAX_TEXT_FILE_BYTES + 1)).lines();
        let mut line_index = 0usize;
        while line_index < offset {
            match lines.next_line().await.map_err(|error| {
                WorkspaceError::Backend(anyhow!(
                    "Failed to read file {}: {}",
                    resolved.display(),
                    error
                ))
            })? {
                Some(line) => {
                    consumed += line.len() + 1;
                    line_index += 1;
                }
                None => {
                    ensure_range_within_bound(path, consumed)?;
                    return Ok(WorkspaceTextRange {
                        lines: Vec::new(),
                        next_offset: None,
                        eof: true,
                        total_lines: Some(line_index),
                    });
                }
            }
        }

        let mut selected = Vec::with_capacity(limit);
        while selected.len() < limit {
            match lines.next_line().await.map_err(|error| {
                WorkspaceError::Backend(anyhow!(
                    "Failed to read file {}: {}",
                    resolved.display(),
                    error
                ))
            })? {
                Some(line) => {
                    consumed += line.len() + 1;
                    selected.push(line);
                }
                None => {
                    ensure_range_within_bound(path, consumed)?;
                    let total_lines = offset.saturating_add(selected.len());
                    return Ok(WorkspaceTextRange {
                        lines: selected,
                        next_offset: None,
                        eof: true,
                        total_lines: Some(total_lines),
                    });
                }
            }
        }

        let has_more = lines
            .next_line()
            .await
            .map_err(|error| {
                WorkspaceError::Backend(anyhow!(
                    "Failed to read file {}: {}",
                    resolved.display(),
                    error
                ))
            })?
            .is_some();
        ensure_range_within_bound(path, consumed)?;
        Ok(WorkspaceTextRange {
            lines: selected,
            next_offset: has_more.then_some(offset.saturating_add(limit)),
            eof: !has_more,
            total_lines: (!has_more).then_some(offset.saturating_add(limit)),
        })
    }
}

#[async_trait]
impl WorkspaceSearch for LocalWorkspaceBackend {
    async fn glob(&self, request: WorkspaceGlobRequest) -> Result<WorkspaceGlobResult> {
        validate_relative_pattern(&request.pattern, "glob pattern")?;
        let base = self.local_path_for_read(&request.base)?;
        let full_pattern = base.join(&request.pattern);
        let full_pattern = full_pattern.to_string_lossy().replace('\\', "/");

        let entries = glob::glob(&full_pattern)
            .map_err(|e| anyhow!("Invalid glob pattern '{}': {}", request.pattern, e))?;

        let mut matches = Vec::new();
        for entry in entries {
            match entry {
                Ok(path) => {
                    // On Windows, `canonicalize()` commonly gives the backend
                    // root a verbatim `\\?\` prefix while `glob` returns a
                    // regular drive path. Canonicalize each match before
                    // stripping so equivalent paths use the same form.
                    let normalized = path.canonicalize().unwrap_or(path);
                    if let Ok(relative) = normalized.strip_prefix(&self.root) {
                        matches.push(pathbuf_to_workspace_path(relative));
                    }
                }
                Err(e) => tracing::warn!("Glob entry error: {}", e),
            }
        }

        matches.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        Ok(WorkspaceGlobResult { matches })
    }

    async fn grep(&self, request: WorkspaceGrepRequest) -> Result<WorkspaceGrepResult> {
        Ok(self.grep_with_sources(request).await?.result)
    }

    async fn grep_with_sources(
        &self,
        request: WorkspaceGrepRequest,
    ) -> Result<WorkspaceGrepOutcome> {
        if let Some(ref glob) = request.glob {
            validate_relative_pattern(glob, "grep glob filter")?;
        }

        let regex_pattern = if request.case_insensitive {
            format!("(?i){}", request.pattern)
        } else {
            request.pattern.clone()
        };
        let regex = regex::Regex::new(&regex_pattern)
            .map_err(|e| anyhow!("Invalid regex pattern '{}': {}", request.pattern, e))?;

        let search_path = self.local_path_for_read(&request.base)?;
        self.ensure_search_base_allowed(&request.base)?;

        // The interactive ACP host is a current-thread LocalSet. A walk that
        // reads files inside this future holds that thread, so session/cancel
        // stays unread and the effect select never polls. The walk runs on the
        // blocking pool. Dropping this future sets the flag. Dropping the
        // JoinHandle does not stop the worker, so the walk checks the flag
        // before each file.
        let root = self.root.clone();
        let boundary = self.access_boundary.clone();
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = Arc::clone(&cancellation);
        let mut cancel_on_drop = CancelGitWorkerOnDrop::new(cancellation);
        let joined = tokio::task::spawn_blocking(move || {
            grep_walk(
                &root,
                boundary.as_deref(),
                &search_path,
                &regex,
                &request,
                &worker_cancellation,
            )
        })
        .await;
        cancel_on_drop.disarm();
        joined.map_err(|error| anyhow!("grep worker failed: {error}"))?
    }
}

#[async_trait]
impl WorkspaceGit for LocalWorkspaceBackend {
    async fn is_repository(&self) -> Result<bool> {
        self.run_blocking_git(|root| Ok(crate::git::is_git_repo(&root)))
            .await
    }

    async fn status(&self) -> Result<WorkspaceGitStatus> {
        self.run_blocking_git(|root| {
            let status = crate::git::get_status(&root)?;
            Ok(WorkspaceGitStatus {
                branch: status.branch,
                commit: status.commit,
                is_worktree: status.is_worktree,
                is_dirty: status.is_dirty,
                dirty_count: status.dirty_count,
            })
        })
        .await
    }

    async fn log(&self, max_count: usize) -> Result<Vec<WorkspaceGitCommit>> {
        self.run_blocking_git(move |root| {
            Ok(crate::git::get_log(&root, max_count)?
                .into_iter()
                .map(|commit| WorkspaceGitCommit {
                    id: commit.id,
                    message: commit.message,
                    author: commit.author,
                    date: commit.date,
                })
                .collect())
        })
        .await
    }

    async fn list_branches(&self) -> Result<Vec<WorkspaceGitBranch>> {
        self.run_blocking_git(|root| {
            Ok(crate::git::list_branches(&root)?
                .into_iter()
                .map(|branch| WorkspaceGitBranch {
                    name: branch.name,
                    is_current: branch.is_current,
                })
                .collect())
        })
        .await
    }

    async fn create_branch(&self, request: WorkspaceGitCreateBranchRequest) -> Result<()> {
        if self.access_boundary.is_some() {
            self.refuse_checkout_targets(&request.base, false)?;
        }
        self.run_blocking_git(move |root| {
            crate::git::create_branch(&root, &request.name, &request.base)
        })
        .await
    }

    async fn checkout(
        &self,
        request: WorkspaceGitCheckoutRequest,
    ) -> Result<WorkspaceGitCheckoutOutput> {
        crate::git::refuse_flag_like_git_revision(&request.refspec, "checkout ref")?;
        if self.access_boundary.is_some() {
            self.refuse_checkout_targets(&request.refspec, request.force)?;
        }
        let args = if request.force {
            vec![
                "checkout".to_string(),
                "--force".to_string(),
                request.refspec,
            ]
        } else {
            vec!["checkout".to_string(), request.refspec]
        };
        let (success, stdout, stderr) = self.run_git_command(args).await?;
        if !success {
            bail!("{}", stderr.trim_end());
        }
        Ok(WorkspaceGitCheckoutOutput { stdout })
    }

    async fn diff(&self, request: WorkspaceGitDiffRequest) -> Result<String> {
        let target = request.target;
        if self.access_boundary.is_none() {
            return self
                .run_blocking_git(move |root| crate::git::get_diff(&root, target.as_deref()))
                .await;
        }

        let target_for_paths = target.clone();
        let paths = self
            .run_blocking_git(move |root| {
                crate::git::get_diff_paths(&root, target_for_paths.as_deref())
            })
            .await?;
        let paths = paths
            .into_iter()
            .filter(|path| self.git_diff_path_allowed(path))
            .collect::<Vec<_>>();
        self.run_blocking_git(move |root| {
            crate::git::get_diff_for_paths(&root, target.as_deref(), &paths)
        })
        .await
    }

    async fn list_remotes(&self) -> Result<Vec<WorkspaceGitRemote>> {
        let (success, stdout, stderr) = self
            .run_git_command(vec!["remote".to_string(), "-v".to_string()])
            .await?;
        if !success {
            bail!("{}", stderr.trim_end());
        }

        Ok(stdout.lines().filter_map(parse_git_remote_line).collect())
    }
}

#[async_trait]
impl WorkspaceGitStashProvider for LocalWorkspaceBackend {
    async fn list_stashes(&self) -> Result<Vec<WorkspaceGitStash>> {
        self.run_blocking_git(|root| {
            Ok(crate::git::list_stashes(&root)?
                .into_iter()
                .map(|stash| WorkspaceGitStash {
                    index: stash.index,
                    message: stash.message,
                })
                .collect())
        })
        .await
    }

    async fn stash(&self, request: WorkspaceGitStashRequest) -> Result<()> {
        if self.access_boundary.is_some() {
            self.refuse_stash_targets(request.include_untracked)?;
        }
        self.run_blocking_git(move |root| {
            crate::git::stash(&root, request.message.as_deref(), request.include_untracked)
        })
        .await
    }
}

#[async_trait]
impl WorkspaceGitWorktreeProvider for LocalWorkspaceBackend {
    async fn list_worktrees(&self) -> Result<Vec<WorkspaceGitWorktree>> {
        self.run_blocking_git(|root| {
            Ok(crate::git::list_worktrees(&root)?
                .into_iter()
                .map(|worktree| WorkspaceGitWorktree {
                    path: worktree.path,
                    branch: worktree.branch,
                    is_bare: worktree.is_bare,
                    is_detached: worktree.is_detached,
                })
                .collect())
        })
        .await
    }

    async fn create_worktree(
        &self,
        request: WorkspaceGitCreateWorktreeRequest,
    ) -> Result<WorkspaceGitWorktreeMutation> {
        let branch = request.branch;
        let new_branch = request.new_branch;
        if request.path.as_deref().is_some_and(|path| {
            let path = Path::new(path);
            !path.is_absolute()
                && path
                    .components()
                    .any(|component| matches!(component, Component::ParentDir))
        }) {
            bail!("worktree path contains an unsupported component");
        }
        let path = request
            .path
            .map(|path| {
                let path = PathBuf::from(path);
                if path.is_absolute() {
                    path
                } else {
                    self.root.join(path)
                }
            })
            .unwrap_or_else(|| default_local_worktree_path(&self.root, &branch));
        refuse_symlink_worktree_path(&self.root, &path)?;
        refuse_symlink_escape(&self.root, &path)?;
        if self.access_boundary.is_some() {
            self.refuse_worktree_checkout(&branch, new_branch, &path)?;
        }
        let display_path = path.display().to_string();
        let branch_for_git = branch.clone();

        self.run_blocking_git(move |root| {
            crate::git::create_worktree(&root, &branch_for_git, &path, new_branch)
        })
        .await?;

        Ok(WorkspaceGitWorktreeMutation {
            path: display_path,
            branch: Some(branch),
        })
    }

    async fn remove_worktree(
        &self,
        request: WorkspaceGitRemoveWorktreeRequest,
    ) -> Result<WorkspaceGitWorktreeMutation> {
        let path = PathBuf::from(request.path);
        let display_path = path.display().to_string();
        let force = request.force;
        if self.access_boundary.is_some() {
            if let Some(canonical) = canonicalize_inside_workspace(&self.root, &path) {
                refuse_symlink_worktree_path(&self.root, &canonical)?;
                let relative = canonical
                    .strip_prefix(&self.root)
                    .unwrap_or(canonical.as_path());
                for file in crate::git::worktree_removable_paths(&canonical)? {
                    self.refuse_checkout_path(&relative.join(file))?;
                }
            }
        }

        self.run_blocking_git(move |root| crate::git::remove_worktree(&root, &path, force))
            .await?;

        Ok(WorkspaceGitWorktreeMutation {
            path: display_path,
            branch: None,
        })
    }
}

#[async_trait]
impl WorkspaceCommandRunner for LocalWorkspaceBackend {
    async fn exec(&self, request: CommandRequest) -> Result<CommandOutput> {
        #[cfg(windows)]
        if let Some(output) =
            crate::tools::builtin::bash::maybe_execute_simple_windows_http_command(&request.command)
                .await
        {
            let exit_code = output
                .metadata
                .as_ref()
                .and_then(|m| m.get("exit_code"))
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .unwrap_or(if output.success { 0 } else { -1 });
            return Ok(CommandOutput {
                output: output.content,
                exit_code,
                timed_out: false,
            });
        }

        let mut child = crate::tools::builtin::bash::spawn_shell(
            &request.command,
            &self.root,
            request.env.as_deref(),
        )
        .map_err(|e| anyhow!("Failed to spawn shell: {}", e))?;

        let output = crate::tools::process::read_process_output(
            &mut child,
            request.timeout_ms,
            request.output_observer.as_deref(),
        )
        .await
        .map_err(|error| anyhow!("Failed to capture shell output: {error}"))?;
        let exit_code = output.status.and_then(|status| status.code()).unwrap_or(-1);

        Ok(CommandOutput {
            output: output.combined,
            exit_code,
            timed_out: output.timed_out,
        })
    }
}

impl LocalWorkspaceBackend {
    async fn run_blocking_git<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(PathBuf) -> Result<T> + Send + 'static,
    {
        let root = self.root.clone();
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = Arc::clone(&cancellation);
        let mut cancel_on_drop = CancelGitWorkerOnDrop::new(cancellation);
        let joined = tokio::task::spawn_blocking(move || {
            crate::git::with_git_cancellation(worker_cancellation, || operation(root))
        })
        .await;
        cancel_on_drop.disarm();
        joined.map_err(|e| anyhow!("Git worker failed: {}", e))?
    }

    async fn run_git_command(&self, args: Vec<String>) -> Result<(bool, String, String)> {
        const GIT_COMMAND_TIMEOUT_MS: u64 = 30_000;

        let executable = crate::git::trusted_git_executable(&self.root)?;
        let mut command = tokio::process::Command::new(executable);
        crate::git::configure_tokio_git_environment(&mut command, &self.root);
        command
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        crate::tools::process::configure_process_group(&mut command);
        let mut child = crate::tools::process::spawn_tokio_child(&mut command)
            .map_err(|e| anyhow!("Failed to execute git: {}", e))?;
        let output =
            crate::tools::process::read_process_output(&mut child, GIT_COMMAND_TIMEOUT_MS, None)
                .await
                .map_err(|e| anyhow!("Failed to wait for git: {}", e))?;
        if output.timed_out {
            bail!("Git command timed out after {GIT_COMMAND_TIMEOUT_MS}ms");
        }
        let success = output.status.is_some_and(|status| status.success());

        Ok((success, output.stdout, output.stderr))
    }
}

fn parse_git_remote_line(line: &str) -> Option<WorkspaceGitRemote> {
    let mut parts = line.split_whitespace();
    let name = parts.next()?;
    let url = parts.next()?;
    let direction = parts
        .next()
        .unwrap_or_default()
        .trim_start_matches('(')
        .trim_end_matches(')');

    Some(WorkspaceGitRemote {
        name: name.to_string(),
        url: url.to_string(),
        direction: direction.to_string(),
    })
}

/// A write must not follow a symlink that already exists on the requested path.
///
/// `create_dir_all` and `File::open` both follow directory and file symlinks.
/// Checking after either of those has already created or overwritten the
/// destination outside the workspace.
fn refuse_existing_symlink_on_write_path(root: &Path, relative: &str) -> Result<()> {
    let mut current = root
        .canonicalize()
        .map_err(|error| anyhow!("Failed to resolve local workspace root: {error}"))?;
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty() {
        bail!("write path must name a file");
    }
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("write path must stay inside the workspace");
        };
        current.push(name);
        let last = index + 1 == components.len();
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("write path crosses a symbolic link")
            }
            Ok(metadata) if !last && !metadata.is_dir() => {
                bail!("A write path parent component is not a directory")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => bail!("Failed to inspect write path: {error}"),
        }
    }
    Ok(())
}

/// Last check before truncation. `/dev/fd` and `/proc/self/fd` are inconclusive
/// on some hosts; a resolved path that is neither of those and sits outside
/// the workspace is a followed link and must not be truncated.
fn opened_write_stays_in_workspace(file: &tokio::fs::File, workspace: &Path) -> Result<()> {
    let Some(canonical) = opened_file_canonical_path(file) else {
        return Ok(());
    };
    if canonical.starts_with("/dev") || canonical.starts_with("/proc") {
        return Ok(());
    }
    let root = workspace
        .canonicalize()
        .map_err(|error| anyhow!("Failed to resolve local workspace root: {error}"))?;
    if !canonical.starts_with(&root) {
        bail!("write path resolves outside the workspace");
    }
    Ok(())
}

fn opened_file_canonical_path(file: &tokio::fs::File) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        for candidate in [format!("/dev/fd/{fd}"), format!("/proc/self/fd/{fd}")] {
            if let Ok(path) = std::fs::canonicalize(&candidate) {
                return Some(path);
            }
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        None
    }
}

fn default_local_worktree_path(root: &Path, branch: &str) -> PathBuf {
    let repo_name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());
    root.parent()
        .unwrap_or(root)
        .join(format!("{repo_name}-{branch}"))
}

fn resolved_workspace_destination(root: &Path, path: &Path) -> Option<PathBuf> {
    if path.starts_with(root) {
        return Some(path.to_path_buf());
    }
    let mut existing = path.to_path_buf();
    let mut missing = Vec::new();
    while !existing.exists() {
        let parent = existing.parent()?.to_path_buf();
        missing.push(existing.file_name()?.to_os_string());
        if parent == existing {
            return None;
        }
        existing = parent;
    }
    let canonical_existing = std::fs::canonicalize(&existing).ok()?;
    if !canonical_existing.starts_with(root) {
        return None;
    }
    let mut resolved = canonical_existing;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    resolved.starts_with(root).then_some(resolved)
}

fn canonicalize_inside_workspace(root: &Path, path: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(path).ok()?;
    canonical.starts_with(root).then_some(canonical)
}

fn refuse_symlink_escape(root: &Path, path: &Path) -> Result<()> {
    let mut cursor = PathBuf::new();
    for component in path.components() {
        cursor.push(component);
        let Ok(metadata) = std::fs::symlink_metadata(&cursor) else {
            break;
        };
        if !metadata.file_type().is_symlink() {
            continue;
        }
        let Ok(target) = std::fs::canonicalize(&cursor) else {
            bail!("refusing to follow a symbolic link in the worktree path");
        };
        if target.starts_with(root) || root.starts_with(&target) {
            continue;
        }
        bail!("refusing to follow a symbolic link in the worktree path");
    }
    Ok(())
}

fn refuse_symlink_worktree_path(root: &Path, path: &Path) -> Result<()> {
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) && !path.is_absolute()
    {
        bail!("worktree path contains an unsupported component");
    }
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if path.is_absolute() && !candidate.starts_with(root) {
        return Ok(());
    }
    let relative = candidate.strip_prefix(root).unwrap_or(candidate.as_path());
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            bail!("worktree path contains an unsupported component");
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("refusing to follow a symbolic link in the worktree path");
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => bail!("failed to inspect worktree path: {error}"),
        }
    }
    Ok(())
}

fn normalize_local_path(root: &Path, input: &str) -> Result<WorkspacePath> {
    let input = default_path_input(input);
    let candidate = Path::new(input);

    if candidate.is_absolute() {
        let root = normalize_absolute_path(root)?;
        let target = normalize_absolute_path(candidate)?;
        if !target.starts_with(&root) {
            bail!(
                "Workspace boundary violation: path '{}' escapes workspace '{}'",
                input,
                root.display()
            );
        }
        let relative = target
            .strip_prefix(&root)
            .map_err(|_| anyhow!("Failed to compute workspace-relative path"))?;
        return Ok(pathbuf_to_workspace_path(relative));
    }

    if has_windows_path_prefix(input) {
        bail!("Absolute paths are not supported by this workspace backend");
    }

    let normalized_input = input.replace('\\', "/");
    let path = Path::new(&normalized_input);
    if path.is_absolute() {
        bail!("Absolute paths are not supported by this workspace backend");
    }

    let relative = normalize_relative_path(path)?;
    Ok(pathbuf_to_workspace_path(&relative))
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf> {
    let lexical = normalize_absolute_path_lexical(path)?;
    if let Ok(canonical) = lexical.canonicalize() {
        return Ok(canonical);
    }

    let mut current = lexical.as_path();
    let mut suffix = Vec::new();
    while !current.exists() {
        let Some(file_name) = current.file_name() else {
            return Ok(lexical);
        };
        suffix.push(file_name.to_os_string());
        let Some(parent) = current.parent() else {
            return Ok(lexical);
        };
        current = parent;
    }

    let mut normalized = current.canonicalize().unwrap_or_else(|_| {
        normalize_absolute_path_lexical(current).unwrap_or_else(|_| current.into())
    });
    for part in suffix.iter().rev() {
        normalized.push(part);
    }
    Ok(normalized)
}

/// Refuse a ranged read whose consumed bytes crossed the whole-file bound:
/// the take() cap turns mid-read growth into a truncated EOF, and this
/// turns that truncation into the honest TooLarge refusal instead.
fn ensure_range_within_bound(path: &WorkspacePath, consumed: usize) -> Result<(), WorkspaceError> {
    if consumed as u64 > MAX_TEXT_FILE_BYTES {
        return Err(WorkspaceError::TooLarge {
            path: path.as_str().to_string(),
            size_bytes: consumed as u64,
            max_bytes: MAX_TEXT_FILE_BYTES,
        });
    }
    Ok(())
}

fn read_search_file_at(
    root: &Path,
    boundary: Option<&LocalWorkspaceAccessBoundary>,
    path: &WorkspacePath,
) -> Option<String> {
    let resolved = a3s_common::tools::resolve_path(root, path.as_str()).ok()?;
    // FIFO/device liveness gate: opening a writer-less FIFO blocks the
    // walk thread, so the regular-file check runs before the open
    // (metadata follows symlinks; the open-file re-check below stays
    // authoritative for the swap race). A stat error falls through to the
    // open's own tolerant `?`.
    if let Ok(metadata) = std::fs::metadata(&resolved) {
        if !metadata.is_file() {
            return None;
        }
    }
    let file = std::fs::File::open(&resolved).ok()?;
    let metadata = file.metadata().ok()?;
    // Non-regular files (FIFOs, devices) have no meaningful text size and
    // can block an open-ended read; the walk skips them like unreadable
    // files. Oversize files get the same skip: the walk's contract is
    // per-file tolerance (an unreadable file never fails the whole grep),
    // and a file past the text bound is outside the searchable domain.
    if !metadata.is_file() || metadata.len() > MAX_TEXT_FILE_BYTES {
        return None;
    }
    let opened_hard_link_count = boundary.map(|_| hard_link_count_for_open_file(&file, &metadata));
    if let Some(boundary) = boundary {
        boundary
            .ensure_access(
                root,
                Path::new(path.as_str()),
                Some(&resolved),
                Some(&metadata),
                opened_hard_link_count,
                "read",
            )
            .ok()?;
    }
    // take() bounds the materialization if the file grows between stat and
    // read; the post-check turns that growth into the same skip.
    let mut content = String::new();
    file.take(MAX_TEXT_FILE_BYTES + 1)
        .read_to_string(&mut content)
        .ok()?;
    if content.len() as u64 > MAX_TEXT_FILE_BYTES {
        return None;
    }
    Some(content)
}

fn grep_walk(
    root: &Path,
    boundary: Option<&LocalWorkspaceAccessBoundary>,
    search_path: &Path,
    regex: &regex::Regex,
    request: &WorkspaceGrepRequest,
    cancel: &AtomicBool,
) -> Result<WorkspaceGrepOutcome> {
    if cancel.load(Ordering::Acquire) {
        bail!("cancelled");
    }

    let mut builder = ignore::WalkBuilder::new(search_path);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .follow_links(false);

    if let Some(ref glob_pat) = request.glob {
        let mut types = ignore::types::TypesBuilder::new();
        types.add("custom", glob_pat).ok();
        types.select("custom");
        if let Ok(built) = types.build() {
            builder.types(built);
        }
    }

    let mut output = String::new();
    let mut match_count = 0;
    let mut file_count = 0;
    let mut total_size = 0;
    let mut matched_paths = Vec::new();
    let metadata_only = request.max_output_size == 0;

    for entry in builder.build().flatten() {
        if cancel.load(Ordering::Acquire) {
            bail!("cancelled");
        }
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }

        let file_path = entry.path();
        let workspace_path =
            pathbuf_to_workspace_path(file_path.strip_prefix(root).unwrap_or(file_path));
        let Some(content) = read_search_file_at(root, boundary, &workspace_path) else {
            continue;
        };

        let lines: Vec<&str> = content.lines().collect();
        let mut file_matches = Vec::new();
        for (line_idx, line) in lines.iter().enumerate() {
            if regex.is_match(line) {
                file_matches.push(line_idx);
            }
        }

        if file_matches.is_empty() {
            continue;
        }

        file_count += 1;
        let rel_path = workspace_path.as_str();
        let display_path = escape_control_chars_for_display(rel_path);
        let mut path_recorded = false;

        for &match_idx in &file_matches {
            if !metadata_only && total_size > request.max_output_size {
                return Ok(WorkspaceGrepOutcome {
                    result: WorkspaceGrepResult {
                        output,
                        match_count,
                        file_count,
                        truncated: true,
                    },
                    matched_paths: Some(matched_paths),
                });
            }

            if !path_recorded {
                matched_paths.push(workspace_path.clone());
                path_recorded = true;
            }
            match_count += 1;
            if metadata_only {
                continue;
            }

            let start = match_idx.saturating_sub(request.context_lines);
            let end = (match_idx + request.context_lines + 1).min(lines.len());

            for (i, line) in lines[start..end].iter().enumerate() {
                let abs_i = start + i;
                let prefix = if abs_i == match_idx { ">" } else { " " };
                let line = format!("{}{}:{}: {}\n", prefix, display_path, abs_i + 1, line);
                total_size += line.len();
                output.push_str(&line);
            }

            if request.context_lines > 0 {
                output.push_str("--\n");
                total_size += 3;
            }
        }
    }

    Ok(WorkspaceGrepOutcome {
        result: WorkspaceGrepResult {
            output,
            match_count,
            file_count,
            truncated: false,
        },
        matched_paths: Some(matched_paths),
    })
}

fn normalize_absolute_path_lexical(path: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir => {
                if !out.pop() {
                    bail!("Invalid absolute path");
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
