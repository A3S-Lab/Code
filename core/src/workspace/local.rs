//! Local filesystem-backed workspace implementation.
//!
//! [`LocalWorkspaceBackend`] preserves the historical "agent runs on the host
//! filesystem" behavior. It implements every workspace capability trait so
//! local sessions get the full tool surface (read, write, edit, patch, ls,
//! bash, grep, glob, git, git_stash, git_worktree).

use super::local_access::{LocalWorkspaceAccessBoundary, LocalWorkspaceAccessPolicy};
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
        let access_boundary = LocalWorkspaceAccessBoundary::for_policy(access_policy, &root);
        Self {
            root,
            access_boundary,
        }
    }

    fn local_path_for_read(&self, path: &WorkspacePath) -> Result<PathBuf> {
        a3s_common::tools::resolve_path(&self.root, path.as_str()).map_err(|e| anyhow!("{}", e))
    }

    fn local_path_for_write(&self, path: &WorkspacePath) -> Result<PathBuf> {
        let target = if path.is_root() {
            self.root.clone()
        } else {
            self.root.join(path.as_str())
        };

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

    fn ensure_access(
        &self,
        path: &WorkspacePath,
        resolved: Option<&Path>,
        metadata: Option<&std::fs::Metadata>,
        operation: &'static str,
    ) -> Result<()> {
        match &self.access_boundary {
            Some(boundary) => boundary.ensure_access(
                &self.root,
                Path::new(path.as_str()),
                resolved,
                metadata,
                operation,
            ),
            None => Ok(()),
        }
    }

    pub(super) fn ensure_search_base_allowed(&self, path: &WorkspacePath) -> Result<()> {
        let resolved = self.local_path_for_read(path)?;
        let metadata = std::fs::metadata(&resolved).ok();
        self.ensure_access(path, Some(&resolved), metadata.as_ref(), "read")
    }

    pub(super) fn read_search_file(&self, path: &WorkspacePath) -> Option<String> {
        let resolved = self.local_path_for_read(path).ok()?;
        let mut file = std::fs::File::open(&resolved).ok()?;
        let metadata = file.metadata().ok()?;
        self.ensure_access(path, Some(&resolved), Some(&metadata), "read")
            .ok()?;
        let mut content = String::new();
        file.read_to_string(&mut content).ok()?;
        Some(content)
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
            "read",
        )
        .is_ok()
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
        self.ensure_access(path, Some(&resolved), Some(&metadata), "read")?;

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
        self.ensure_access(path, None, None, "write")?;
        let resolved = self.local_path_for_write(path)?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
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
        let metadata = file.metadata().await.map_err(|error| {
            WorkspaceError::Backend(anyhow!(
                "Failed to inspect file {} before writing: {}",
                resolved.display(),
                error
            ))
        })?;
        self.ensure_access(path, Some(&resolved), Some(&metadata), "write")?;
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
        self.ensure_access(path, Some(&resolved), Some(&metadata), "read")?;
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
        builder.hidden(false).git_ignore(true).git_global(true);

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
                if total_size > request.max_output_size {
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
        self.run_blocking_git(move |root| {
            crate::git::create_branch(&root, &request.name, &request.base)
        })
        .await
    }

    async fn checkout(
        &self,
        request: WorkspaceGitCheckoutRequest,
    ) -> Result<WorkspaceGitCheckoutOutput> {
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
        let display_path = path.display().to_string();
        let new_branch = request.new_branch;
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

        let process = crate::tools::process::read_process_output(
            &mut child,
            request.timeout_ms,
            request.output_observer.as_deref(),
        )
        .await
        .map_err(|e| anyhow!("Failed to wait for shell: {}", e))?;

        if process.timed_out {
            return Ok(CommandOutput {
                output: process.combined,
                exit_code: -1,
                timed_out: true,
            });
        }

        let exit_code = process
            .status
            .and_then(|status| status.code())
            .unwrap_or(-1);

        Ok(CommandOutput {
            output: process.combined,
            exit_code,
            timed_out: false,
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
        let mut child = command
            .spawn()
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

fn default_local_worktree_path(root: &Path, branch: &str) -> PathBuf {
    let repo_name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());
    root.parent()
        .unwrap_or(root)
        .join(format!("{repo_name}-{branch}"))
}

pub(super) fn normalize_local_path(root: &Path, input: &str) -> Result<WorkspacePath> {
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
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.name=A3S Test",
                "-c",
                "user.email=test@a3s.local",
            ])
            .args(args)
            .status()
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
}
