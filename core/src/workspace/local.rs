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

/// Local filesystem-backed workspace implementation.
#[derive(Debug)]
pub struct LocalWorkspaceBackend {
    pub(super) root: PathBuf,
    access_boundary: Option<LocalWorkspaceAccessBoundary>,
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
        let access_boundary = boundary(&root);
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
        let resolved = self.local_path_for_read(path).ok()?;
        let mut file = std::fs::File::open(&resolved).ok()?;
        let metadata = file.metadata().ok()?;
        self.ensure_access(
            path,
            Some(&resolved),
            Some(&metadata),
            self.access_boundary
                .as_ref()
                .map(|_| hard_link_count_for_open_file(&file, &metadata)),
            "read",
        )
        .ok()?;
        let mut content = String::new();
        file.read_to_string(&mut content).ok()?;
        Some(content)
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
        let mut file = match tokio::fs::File::open(&resolved).await {
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

        let mut content = String::new();
        file.read_to_string(&mut content).await.map_err(|error| {
            WorkspaceError::Backend(anyhow!(
                "Failed to read file {}: {}",
                resolved.display(),
                error
            ))
        })?;
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
        let mut lines = BufReader::new(file).lines();
        let mut line_index = 0usize;
        while line_index < offset {
            match lines.next_line().await.map_err(|error| {
                WorkspaceError::Backend(anyhow!(
                    "Failed to read file {}: {}",
                    resolved.display(),
                    error
                ))
            })? {
                Some(_) => line_index += 1,
                None => {
                    return Ok(WorkspaceTextRange {
                        lines: Vec::new(),
                        next_offset: None,
                        eof: true,
                        total_lines: Some(line_index),
                    })
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
                Some(line) => selected.push(line),
                None => {
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
        let mut builder = ignore::WalkBuilder::new(&search_path);
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
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }

            let file_path = entry.path();
            let workspace_path =
                pathbuf_to_workspace_path(file_path.strip_prefix(&self.root).unwrap_or(file_path));
            let Some(content) = self.read_search_file(&workspace_path) else {
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
        if request.refspec.trim().is_empty() || request.refspec.contains('\0') {
            bail!("Git checkout ref must be a non-empty revision");
        }
        if self.access_boundary.is_some() {
            self.refuse_checkout_targets(&request.refspec, request.force)?;
        }
        let args = if request.force {
            vec![
                "checkout".to_string(),
                "--force".to_string(),
                "--end-of-options".to_string(),
                request.refspec,
            ]
        } else {
            vec![
                "checkout".to_string(),
                "--end-of-options".to_string(),
                request.refspec,
            ]
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
        let mut child = crate::tools::process::spawn_tokio_with_native_gate(&mut command)
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
mod tests {
    use super::super::WorkspaceServices;
    use super::*;

    #[tokio::test]
    async fn local_backend_reads_writes_and_lists() {
        let temp = tempfile::tempdir().unwrap();
        let services = WorkspaceServices::local(temp.path());
        let path = services.normalize_path("dir/file.txt").unwrap();

        let written = services
            .fs()
            .write_text(&path, "hello\nworld\n")
            .await
            .unwrap();
        assert_eq!(written.bytes, 12);
        assert_eq!(written.lines, 2);

        let content = services.fs().read_text(&path).await.unwrap();
        assert_eq!(content, "hello\nworld\n");

        let dir = services.normalize_path("dir").unwrap();
        let entries = services.fs().list_dir(&dir).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "file.txt");
    }

    #[tokio::test]
    async fn local_backend_searches_glob_and_grep() {
        let temp = tempfile::tempdir().unwrap();
        let services = WorkspaceServices::local(temp.path());
        services
            .fs()
            .write_text(
                &services.normalize_path("src/main.rs").unwrap(),
                "fn main() {\n    println!(\"hello\");\n}\n",
            )
            .await
            .unwrap();
        services
            .fs()
            .write_text(
                &services.normalize_path("README.md").unwrap(),
                "hello from docs\n",
            )
            .await
            .unwrap();

        let search = services.search().expect("local backend supports search");
        let glob = search
            .glob(WorkspaceGlobRequest {
                base: services.normalize_path("src").unwrap(),
                pattern: "*.rs".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(glob.matches[0].as_str(), "src/main.rs");

        let grep = search
            .grep(WorkspaceGrepRequest {
                base: WorkspacePath::root(),
                pattern: "hello".to_string(),
                glob: Some("**/*.rs".to_string()),
                context_lines: 0,
                case_insensitive: false,
                max_output_size: 1024,
            })
            .await
            .unwrap();
        assert_eq!(grep.match_count, 1);
        assert_eq!(grep.file_count, 1);
        assert!(grep.output.contains("src/main.rs:2"));
    }

    fn credential_boundary_backend(root: &Path) -> LocalWorkspaceBackend {
        LocalWorkspaceBackend::new_with_access_policy(
            root.to_path_buf(),
            LocalWorkspaceAccessPolicy::CredentialBoundary,
        )
    }

    #[tokio::test]
    async fn credential_boundary_denies_direct_secret_reads_and_writes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("apps/api")).unwrap();
        std::fs::write(temp.path().join("apps/api/.env.local"), "TOKEN=secret\n").unwrap();
        let backend = credential_boundary_backend(temp.path());
        let secret = backend.normalize("apps/api/.env.local").unwrap();

        let read_error = backend
            .read_text(&secret)
            .await
            .expect_err("direct secret reads must be denied");
        assert!(read_error.to_string().contains("credential boundary"));

        let range_error = backend
            .read_text_range(&secret, 0, 10)
            .await
            .expect_err("range reads must use the same boundary");
        assert!(range_error.to_string().contains("credential boundary"));

        let write_error = backend
            .write_text(&secret, "TOKEN=overwritten\n")
            .await
            .expect_err("direct secret writes must be denied");
        assert!(write_error.to_string().contains("credential boundary"));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("apps/api/.env.local")).unwrap(),
            "TOKEN=secret\n"
        );

        let new_secret = backend.normalize(".env.generated").unwrap();
        backend
            .write_text(&new_secret, "TOKEN=new\n")
            .await
            .expect_err("creating a new env file must be denied");
        assert!(!temp.path().join(".env.generated").exists());
    }

    #[tokio::test]
    async fn credential_boundary_filters_grep_and_rejects_explicit_secret_base() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(".env"), "BOUNDARY_TOKEN=secret\n").unwrap();
        std::fs::write(
            temp.path().join("README.md"),
            "BOUNDARY_TOKEN is configured externally\n",
        )
        .unwrap();
        let backend = credential_boundary_backend(temp.path());

        let grep = backend
            .grep(WorkspaceGrepRequest {
                base: WorkspacePath::root(),
                pattern: "BOUNDARY_TOKEN".to_string(),
                glob: None,
                context_lines: 0,
                case_insensitive: false,
                max_output_size: 1024,
            })
            .await
            .unwrap();
        assert_eq!(grep.match_count, 1);
        assert_eq!(grep.file_count, 1);
        assert!(grep.output.contains("README.md"));
        assert!(!grep.output.contains("secret"));
        assert!(!grep.output.contains(".env"));

        let error = backend
            .grep(WorkspaceGrepRequest {
                base: backend.normalize(".env").unwrap(),
                pattern: "secret".to_string(),
                glob: None,
                context_lines: 0,
                case_insensitive: false,
                max_output_size: 1024,
            })
            .await
            .expect_err("an explicit secret grep must fail closed");
        assert!(error.to_string().contains("credential boundary"));
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn credential_boundary_denies_source_hardlinks_without_truncating_them() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.txt");
        let alias = temp.path().join("alias.txt");
        std::fs::write(&source, "linked secret\n").unwrap();
        std::fs::hard_link(&source, &alias).unwrap();
        let backend = credential_boundary_backend(temp.path());
        let alias_path = backend.normalize("alias.txt").unwrap();

        backend
            .read_text(&alias_path)
            .await
            .expect_err("source-tree hardlink reads must be denied");
        backend
            .write_text(&alias_path, "overwritten\n")
            .await
            .expect_err("source-tree hardlink writes must be denied");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "linked secret\n");
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn source_egress_boundary_denies_control_paths_and_every_hardlink() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::create_dir_all(temp.path().join(".a3s")).unwrap();
        let source = temp.path().join("source.txt");
        let alias = temp.path().join("src/apparently-safe.txt");
        std::fs::write(&source, "linked source\n").unwrap();
        std::fs::hard_link(&source, &alias).unwrap();
        std::fs::write(temp.path().join(".a3s/config.acl"), "secret = true\n").unwrap();
        std::fs::write(temp.path().join("src/safe.rs"), "pub fn safe() {}\n").unwrap();
        let backend =
            LocalWorkspaceBackend::new_with_source_egress_policy(temp.path().to_path_buf());

        backend
            .read_text(&backend.normalize("src/apparently-safe.txt").unwrap())
            .await
            .expect_err("source egress must reject every multi-link file");
        backend
            .read_text(&backend.normalize(".a3s/config.acl").unwrap())
            .await
            .expect_err("source egress must reject control paths at read time");
        assert_eq!(
            backend
                .read_text(&backend.normalize("src/safe.rs").unwrap())
                .await
                .unwrap(),
            "pub fn safe() {}\n"
        );
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn credential_boundary_allows_package_store_hardlinks_but_denies_secret_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("node_modules/pkg");
        std::fs::create_dir_all(&package).unwrap();

        let package_source = package.join("source.js");
        let package_alias = package.join("alias.js");
        std::fs::write(&package_source, "export const value = 1;\n").unwrap();
        std::fs::hard_link(&package_source, &package_alias).unwrap();

        let env = temp.path().join(".env");
        let env_alias = package.join("credential.txt");
        std::fs::write(&env, "TOKEN=secret\n").unwrap();
        std::fs::hard_link(&env, &env_alias).unwrap();

        let backend = credential_boundary_backend(temp.path());
        let package_content = backend
            .read_text(&backend.normalize("node_modules/pkg/alias.js").unwrap())
            .await
            .expect("ordinary package-store hardlinks should remain readable");
        assert!(package_content.contains("value = 1"));

        let error = backend
            .read_text(
                &backend
                    .normalize("node_modules/pkg/credential.txt")
                    .unwrap(),
            )
            .await
            .expect_err("a package-tree alias of a known credential must be denied");
        assert!(error.to_string().contains("credential boundary"));
    }

    fn run_test_git(root: &Path, args: &[&str]) -> bool {
        let mut command = std::process::Command::new("git");
        command
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.name=A3S Test",
                "-c",
                "user.email=test@a3s.local",
            ])
            .args(args);
        crate::tools::process::status_std_with_native_gate(&mut command)
            .is_ok_and(|status| status.success())
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn credential_boundary_filters_git_diff_content_and_option_like_targets() {
        let temp = tempfile::tempdir().unwrap();
        if !run_test_git(temp.path(), &["init", "-q"]) {
            return;
        }
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join(".env"), "TOKEN=old-secret\n").unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "pub const VALUE: u8 = 1;\n").unwrap();
        std::fs::write(temp.path().join("linked.txt"), "hardlink-old-secret\n").unwrap();
        std::fs::hard_link(
            temp.path().join("linked.txt"),
            temp.path().join("linked-alias.txt"),
        )
        .unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "baseline"]));

        std::fs::write(temp.path().join(".env"), "TOKEN=new-secret\n").unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "pub const VALUE: u8 = 2;\n").unwrap();
        std::fs::write(
            temp.path().join("linked-alias.txt"),
            "hardlink-new-secret\n",
        )
        .unwrap();

        let backend = credential_boundary_backend(temp.path());
        let diff = backend
            .diff(WorkspaceGitDiffRequest { target: None })
            .await
            .unwrap();
        assert!(diff.contains("VALUE: u8 = 2"), "{diff}");
        for denied in [
            "old-secret",
            "new-secret",
            "hardlink-old-secret",
            "hardlink-new-secret",
            ".env",
            "linked.txt",
            "linked-alias.txt",
        ] {
            assert!(!diff.contains(denied), "{denied} leaked in {diff}");
        }

        let output = temp.path().join("injected-diff-output");
        let error = backend
            .diff(WorkspaceGitDiffRequest {
                target: Some(format!("--output={}", output.display())),
            })
            .await
            .expect_err("an option-like target must be parsed only as a revision");
        assert!(error.to_string().contains("Git diff"));
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn credential_boundary_checkout_does_not_overwrite_a_credential_file() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_test_git(temp.path(), &["init", "-q"]));
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join(".env"), "TOKEN=checkout-secret-4e17\n").unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "pub const VALUE: u8 = 1;\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "old"]));
        assert!(run_test_git(temp.path(), &["branch", "old"]));
        std::fs::write(temp.path().join(".env"), "TOKEN=checkout-secret-new\n").unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "pub const VALUE: u8 = 2;\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "new"]));
        let new_branch = if run_test_git(temp.path(), &["rev-parse", "--verify", "main"]) {
            "main"
        } else {
            "master"
        };
        assert!(run_test_git(temp.path(), &["checkout", "-q", "old"]));

        let backend = credential_boundary_backend(temp.path());
        let error = backend
            .checkout(WorkspaceGitCheckoutRequest {
                refspec: new_branch.to_string(),
                force: false,
            })
            .await
            .expect_err("checkout must not apply a credential path");
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert_eq!(
            std::fs::read_to_string(temp.path().join(".env")).unwrap(),
            "TOKEN=checkout-secret-4e17\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("src/lib.rs")).unwrap(),
            "pub const VALUE: u8 = 1;\n"
        );
    }

    #[tokio::test]
    async fn credential_boundary_checkout_still_updates_an_ordinary_file() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_test_git(temp.path(), &["init", "-q"]));
        std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "old"]));
        assert!(run_test_git(temp.path(), &["branch", "old"]));
        std::fs::write(temp.path().join("README.md"), "new\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "new"]));
        let new_branch = if run_test_git(temp.path(), &["rev-parse", "--verify", "main"]) {
            "main"
        } else {
            "master"
        };
        assert!(run_test_git(temp.path(), &["checkout", "-q", "old"]));

        let backend = credential_boundary_backend(temp.path());
        backend
            .checkout(WorkspaceGitCheckoutRequest {
                refspec: new_branch.to_string(),
                force: false,
            })
            .await
            .expect("ordinary checkout");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
            "new\n"
        );
    }

    #[tokio::test]
    async fn credential_boundary_force_checkout_does_not_reset_a_dirty_credential_file() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_test_git(temp.path(), &["init", "-q"]));
        std::fs::write(temp.path().join(".env"), "TOKEN=committed\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
        std::fs::write(temp.path().join(".env"), "TOKEN=dirty-checkout-7c41\n").unwrap();

        let backend = credential_boundary_backend(temp.path());
        let error = backend
            .checkout(WorkspaceGitCheckoutRequest {
                refspec: "HEAD".to_string(),
                force: true,
            })
            .await
            .expect_err("force checkout must not reset a credential file");
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert_eq!(
            std::fs::read_to_string(temp.path().join(".env")).unwrap(),
            "TOKEN=dirty-checkout-7c41\n"
        );
    }

    #[tokio::test]
    async fn credential_boundary_stash_does_not_reset_a_dirty_credential_file() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_test_git(temp.path(), &["init", "-q"]));
        std::fs::write(temp.path().join(".env"), "TOKEN=committed\n").unwrap();
        std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
        std::fs::write(temp.path().join(".env"), "TOKEN=dirty-stash-7c41\n").unwrap();
        std::fs::write(temp.path().join("README.md"), "new\n").unwrap();

        let backend = credential_boundary_backend(temp.path());
        let error = backend
            .stash(WorkspaceGitStashRequest {
                message: Some("should-not-land".to_string()),
                include_untracked: false,
            })
            .await
            .expect_err("stash must not reset a credential file");
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert_eq!(
            std::fs::read_to_string(temp.path().join(".env")).unwrap(),
            "TOKEN=dirty-stash-7c41\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
            "new\n"
        );
        assert!(backend.list_stashes().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn credential_boundary_stash_does_not_remove_an_untracked_credential_file() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_test_git(temp.path(), &["init", "-q"]));
        std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
        std::fs::write(temp.path().join(".env"), "TOKEN=untracked-stash-91aa\n").unwrap();
        std::fs::write(temp.path().join("notes.txt"), "keep\n").unwrap();

        let backend = credential_boundary_backend(temp.path());
        let error = backend
            .stash(WorkspaceGitStashRequest {
                message: Some("should-not-land".to_string()),
                include_untracked: true,
            })
            .await
            .expect_err("stash -u must not remove an untracked credential file");
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert_eq!(
            std::fs::read_to_string(temp.path().join(".env")).unwrap(),
            "TOKEN=untracked-stash-91aa\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("notes.txt")).unwrap(),
            "keep\n"
        );
        assert!(backend.list_stashes().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn credential_boundary_stash_still_saves_an_ordinary_file() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_test_git(temp.path(), &["init", "-q"]));
        std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
        std::fs::write(temp.path().join("README.md"), "new\n").unwrap();

        let backend = credential_boundary_backend(temp.path());
        backend
            .stash(WorkspaceGitStashRequest {
                message: Some("ordinary".to_string()),
                include_untracked: false,
            })
            .await
            .expect("an ordinary dirty file must still be stashable");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
            "old\n"
        );
        let stashes = backend.list_stashes().await.unwrap();
        assert!(
            stashes
                .iter()
                .any(|stash| stash.message.contains("ordinary")),
            "{stashes:?}"
        );
    }

    #[tokio::test]
    async fn create_branch_does_not_treat_an_option_like_base_as_a_git_flag() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_test_git(temp.path(), &["init", "-q"]));
        std::fs::write(temp.path().join(".env"), "TOKEN=committed\n").unwrap();
        std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
        assert!(run_test_git(temp.path(), &["branch", "-M", "main"]));
        std::fs::write(temp.path().join(".env"), "TOKEN=dirty-branch-4e18\n").unwrap();
        std::fs::write(temp.path().join("README.md"), "new\n").unwrap();

        let backend = LocalWorkspaceBackend::new(temp.path().to_path_buf());
        let error = backend
            .create_branch(WorkspaceGitCreateBranchRequest {
                name: "feature".to_string(),
                base: "-f".to_string(),
            })
            .await
            .expect_err("an option-like base must not force-discard the worktree");
        assert!(error.to_string().contains("create branch"), "{error}");
        assert_eq!(
            std::fs::read_to_string(temp.path().join(".env")).unwrap(),
            "TOKEN=dirty-branch-4e18\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
            "new\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join(".git/HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
    }

    #[tokio::test]
    async fn credential_boundary_create_branch_does_not_rewrite_a_credential_file() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_test_git(temp.path(), &["init", "-q"]));
        std::fs::write(temp.path().join(".env"), "TOKEN=branch-secret-4e18\n").unwrap();
        std::fs::write(temp.path().join("README.md"), "one\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "main"]));
        assert!(run_test_git(temp.path(), &["branch", "-M", "main"]));
        assert!(run_test_git(
            temp.path(),
            &["checkout", "-q", "-b", "other"]
        ));
        std::fs::write(temp.path().join(".env"), "TOKEN=other\n").unwrap();
        std::fs::write(temp.path().join("README.md"), "two\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "other"]));
        assert!(run_test_git(temp.path(), &["checkout", "-q", "main"]));

        let backend = credential_boundary_backend(temp.path());
        let error = backend
            .create_branch(WorkspaceGitCreateBranchRequest {
                name: "feature".to_string(),
                base: "other".to_string(),
            })
            .await
            .expect_err("creating a branch must not check out a credential path");
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert_eq!(
            std::fs::read_to_string(temp.path().join(".env")).unwrap(),
            "TOKEN=branch-secret-4e18\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
            "one\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join(".git/HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
    }

    #[tokio::test]
    async fn credential_boundary_create_branch_still_updates_an_ordinary_file() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_test_git(temp.path(), &["init", "-q"]));
        std::fs::write(temp.path().join("README.md"), "one\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "main"]));
        assert!(run_test_git(temp.path(), &["branch", "-M", "main"]));
        assert!(run_test_git(
            temp.path(),
            &["checkout", "-q", "-b", "other"]
        ));
        std::fs::write(temp.path().join("README.md"), "two\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "other"]));
        assert!(run_test_git(temp.path(), &["checkout", "-q", "main"]));

        let backend = credential_boundary_backend(temp.path());
        backend
            .create_branch(WorkspaceGitCreateBranchRequest {
                name: "feature".to_string(),
                base: "other".to_string(),
            })
            .await
            .expect("a branch that only changes an ordinary file must still be created");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
            "two\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join(".git/HEAD")).unwrap(),
            "ref: refs/heads/feature\n"
        );
    }

    #[tokio::test]
    async fn checkout_does_not_treat_an_option_like_ref_as_a_git_flag() {
        let temp = tempfile::tempdir().unwrap();
        assert!(run_test_git(temp.path(), &["init", "-q"]));
        std::fs::write(temp.path().join("README.md"), "old\n").unwrap();
        assert!(run_test_git(temp.path(), &["add", "."]));
        assert!(run_test_git(temp.path(), &["commit", "-qm", "base"]));
        let output = temp.path().join("injected-checkout");
        let backend = LocalWorkspaceBackend::new(temp.path().to_path_buf());
        let error = backend
            .checkout(WorkspaceGitCheckoutRequest {
                refspec: format!("--output={}", output.display()),
                force: true,
            })
            .await
            .expect_err("an option-like ref must not become a Git flag");
        assert!(
            error.to_string().contains("checkout") || error.to_string().contains("Git"),
            "{error}"
        );
        assert!(!output.exists());
    }

    #[test]
    fn local_backend_rejects_absolute_paths_outside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let services = WorkspaceServices::local(temp.path());
        let outside = temp.path().parent().unwrap().join("secret.txt");
        let err = services
            .normalize_path(outside.to_str().unwrap())
            .expect_err("outside absolute path should be rejected");
        assert!(err.to_string().contains("escapes workspace"));
    }

    #[test]
    fn local_backend_rejects_backslash_parent_escape() {
        let temp = tempfile::tempdir().unwrap();
        let services = WorkspaceServices::local(temp.path());
        let err = services
            .normalize_path(r"..\secret.txt")
            .expect_err("backslash parent traversal should be rejected");
        assert!(err.to_string().contains("escapes workspace"));
    }

    #[test]
    fn local_backend_allows_absolute_paths_inside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let services = WorkspaceServices::local(temp.path());
        let absolute = temp.path().join("src/main.rs");
        let path = services
            .normalize_path(absolute.to_str().unwrap())
            .expect("absolute path inside workspace should normalize");
        assert_eq!(path.as_str(), "src/main.rs");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_text_does_not_follow_a_symlink_file_out_of_the_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "outside-token-4c91").unwrap();
        symlink(&outside_file, workspace.path().join("guest.txt")).unwrap();

        let services = WorkspaceServices::local(workspace.path());
        let path = services.normalize_path("guest.txt").unwrap();
        let error = services
            .fs()
            .write_text(&path, "written-through-link")
            .await
            .expect_err("a symlink destination must not be written");
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&outside_file).unwrap(),
            "outside-token-4c91"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_text_does_not_create_directories_through_a_symlink() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), workspace.path().join("escape")).unwrap();

        let services = WorkspaceServices::local(workspace.path());
        let path = services.normalize_path("escape/nested/new.txt").unwrap();
        let error = services
            .fs()
            .write_text(&path, "created-outside")
            .await
            .expect_err("a symlink parent must not receive new directories");
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert!(!outside.path().join("nested").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_text_does_not_return_bytes_through_a_symlink() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "outside-token-b81e").unwrap();
        symlink(&outside_file, workspace.path().join("guest.txt")).unwrap();

        let services = WorkspaceServices::local(workspace.path());
        let path = services.normalize_path("guest.txt").unwrap();
        let error = services
            .fs()
            .read_text(&path)
            .await
            .expect_err("a symlink read must not return outside bytes");
        assert!(!error.to_string().contains("outside-token-b81e"), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn list_dir_does_not_follow_a_symlink_directory() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("hidden.txt"), "outside-token-c44a").unwrap();
        symlink(outside.path(), workspace.path().join("escape")).unwrap();

        let services = WorkspaceServices::local(workspace.path());
        let path = services.normalize_path("escape").unwrap();
        let error = services
            .fs()
            .list_dir(&path)
            .await
            .expect_err("a symlink directory must not be listed");
        let rendered = error.to_string();
        assert!(!rendered.contains("hidden.txt"), "{rendered}");
        assert!(!rendered.contains("outside-token-c44a"), "{rendered}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn grep_and_glob_do_not_surface_a_symlink_target() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("hidden.txt"), "outside-token-e90d").unwrap();
        symlink(outside.path(), workspace.path().join("escape")).unwrap();
        std::fs::write(workspace.path().join("local.txt"), "local-only\n").unwrap();

        let services = WorkspaceServices::local(workspace.path());
        let search = services.search().expect("local backend supports search");
        let grep = search
            .grep(WorkspaceGrepRequest {
                base: WorkspacePath::root(),
                pattern: "outside-token-e90d".to_string(),
                glob: None,
                context_lines: 0,
                case_insensitive: false,
                max_output_size: 4096,
            })
            .await
            .unwrap();
        assert_eq!(grep.match_count, 0, "{}", grep.output);
        assert!(!grep.output.contains("outside-token-e90d"));

        let glob = search
            .glob(WorkspaceGlobRequest {
                base: WorkspacePath::root(),
                pattern: "**/*".to_string(),
            })
            .await
            .unwrap();
        assert!(
            glob.matches
                .iter()
                .all(|path| !path.as_str().contains("hidden.txt")),
            "{:?}",
            glob.matches
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn create_worktree_does_not_follow_a_symlink_directory() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            outside.path().join("keep.txt"),
            "outside-worktree-token-6a41",
        )
        .unwrap();
        symlink(outside.path(), workspace.path().join("escape")).unwrap();
        init_worktree_repo(workspace.path());

        let backend = LocalWorkspaceBackend::new(workspace.path().to_path_buf());
        let error = backend
            .create_worktree(WorkspaceGitCreateWorktreeRequest {
                branch: "feature".to_string(),
                path: Some("escape/wt".to_string()),
                new_branch: true,
            })
            .await
            .expect_err("worktree create must not follow a symlink out of the workspace");
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert!(!outside.path().join("wt").exists());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("keep.txt")).unwrap(),
            "outside-worktree-token-6a41"
        );

        let absolute = workspace.path().join("escape/wt-abs");
        let error = backend
            .create_worktree(WorkspaceGitCreateWorktreeRequest {
                branch: "feature-abs".to_string(),
                path: Some(absolute.display().to_string()),
                new_branch: true,
            })
            .await
            .expect_err("an absolute path must not follow a symlink out of the workspace");
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert!(!outside.path().join("wt-abs").exists());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("keep.txt")).unwrap(),
            "outside-worktree-token-6a41"
        );

        let sibling = tempfile::tempdir().unwrap();
        let error = backend
            .create_worktree(WorkspaceGitCreateWorktreeRequest {
                branch: "sibling".to_string(),
                path: Some(format!(
                    "../{}",
                    sibling.path().file_name().unwrap().to_string_lossy()
                )),
                new_branch: true,
            })
            .await
            .expect_err("a relative parent path must not select a sibling worktree");
        assert!(
            error.to_string().contains("unsupported component"),
            "{error}"
        );
        assert!(std::fs::read_dir(sibling.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn create_worktree_still_creates_an_ordinary_worktree() {
        let workspace = tempfile::tempdir().unwrap();
        init_worktree_repo(workspace.path());
        let backend = LocalWorkspaceBackend::new(workspace.path().to_path_buf());
        let created = backend
            .create_worktree(WorkspaceGitCreateWorktreeRequest {
                branch: "feature".to_string(),
                path: Some("wt".to_string()),
                new_branch: true,
            })
            .await
            .expect("an ordinary worktree path must still be created");
        assert!(created.path.ends_with("wt"), "{}", created.path);
        assert!(workspace.path().join("wt/README.md").is_file());
        backend
            .remove_worktree(WorkspaceGitRemoveWorktreeRequest {
                path: workspace.path().join("wt").display().to_string(),
                force: true,
            })
            .await
            .expect("the ordinary worktree must still be removable");
    }

    #[tokio::test]
    async fn credential_boundary_create_worktree_does_not_check_out_a_credential_file() {
        let workspace = tempfile::tempdir().unwrap();
        init_worktree_repo(workspace.path());
        std::fs::write(
            workspace.path().join(".env"),
            "TOKEN=worktree-secret-2c91\n",
        )
        .unwrap();
        assert!(run_test_git(workspace.path(), &["add", ".env"]));
        assert!(run_test_git(workspace.path(), &["commit", "-qm", "secret"]));

        let backend = credential_boundary_backend(workspace.path());
        let error = backend
            .create_worktree(WorkspaceGitCreateWorktreeRequest {
                branch: "feature".to_string(),
                path: Some("wt".to_string()),
                new_branch: true,
            })
            .await
            .expect_err("worktree create must not check out a credential file");
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert!(!workspace.path().join("wt").exists());
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".env")).unwrap(),
            "TOKEN=worktree-secret-2c91\n"
        );

        assert!(run_test_git(
            workspace.path(),
            &["checkout", "-q", "-b", "other"]
        ));
        std::fs::write(workspace.path().join(".env"), "TOKEN=other-worktree-8f12\n").unwrap();
        assert!(run_test_git(workspace.path(), &["add", ".env"]));
        assert!(run_test_git(workspace.path(), &["commit", "-qm", "other"]));
        assert!(run_test_git(workspace.path(), &["checkout", "-q", "main"]));
        let error = backend
            .create_worktree(WorkspaceGitCreateWorktreeRequest {
                branch: "other".to_string(),
                path: Some("wt2".to_string()),
                new_branch: false,
            })
            .await
            .expect_err("checking out an existing branch must not write a credential file");
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert!(!workspace.path().join("wt2").exists());

        let alias = workspace.path().join("wt-alias");
        let error = backend
            .create_worktree(WorkspaceGitCreateWorktreeRequest {
                branch: "alias-feature".to_string(),
                path: Some(alias.display().to_string()),
                new_branch: true,
            })
            .await
            .expect_err("a non-canonical absolute path must not skip the credential check");
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert!(!alias.exists());
        assert!(!workspace
            .path()
            .canonicalize()
            .unwrap()
            .join("wt-alias")
            .exists());
    }

    #[tokio::test]
    async fn credential_boundary_create_worktree_still_creates_an_ordinary_tree() {
        let workspace = tempfile::tempdir().unwrap();
        init_worktree_repo(workspace.path());
        let backend = credential_boundary_backend(workspace.path());
        backend
            .create_worktree(WorkspaceGitCreateWorktreeRequest {
                branch: "feature".to_string(),
                path: Some("wt".to_string()),
                new_branch: true,
            })
            .await
            .expect("a tree without credential files must still create a worktree");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("wt/README.md")).unwrap(),
            "v1\n"
        );
        assert!(!workspace.path().join("wt/.env").exists());
        backend
            .remove_worktree(WorkspaceGitRemoveWorktreeRequest {
                path: workspace.path().join("wt").display().to_string(),
                force: true,
            })
            .await
            .expect("an ordinary worktree must still be removable");
        assert!(!workspace.path().join("wt").exists());
    }

    #[tokio::test]
    async fn credential_boundary_remove_worktree_does_not_delete_a_credential_file() {
        let workspace = tempfile::tempdir().unwrap();
        init_worktree_repo(workspace.path());
        std::fs::write(workspace.path().join(".env"), "TOKEN=remove-wt-6b20\n").unwrap();
        assert!(run_test_git(workspace.path(), &["add", ".env"]));
        assert!(run_test_git(workspace.path(), &["commit", "-qm", "secret"]));
        assert!(run_test_git(
            workspace.path(),
            &["worktree", "add", "wt", "-b", "feature"]
        ));

        let backend = credential_boundary_backend(workspace.path());
        let error = backend
            .remove_worktree(WorkspaceGitRemoveWorktreeRequest {
                path: workspace.path().join("wt").display().to_string(),
                force: true,
            })
            .await
            .expect_err("worktree remove must not delete a credential file");
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("wt/.env")).unwrap(),
            "TOKEN=remove-wt-6b20\n"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".env")).unwrap(),
            "TOKEN=remove-wt-6b20\n"
        );
    }

    fn init_worktree_repo(root: &Path) {
        assert!(run_test_git(root, &["init"]));
        assert!(run_test_git(
            root,
            &["config", "user.email", "a3s@example.com"]
        ));
        assert!(run_test_git(root, &["config", "user.name", "a3s"]));
        std::fs::write(root.join("README.md"), "v1\n").unwrap();
        assert!(run_test_git(root, &["add", "README.md"]));
        assert!(run_test_git(root, &["commit", "-m", "one"]));
    }
}
