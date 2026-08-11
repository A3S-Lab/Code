//! Git helpers backed by the host's existing system Git.
//!
//! Agent-owned tool calls never install host software. Workspace Git operations
//! require an existing Git executable and fail with an actionable error when
//! it is missing.

use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

thread_local! {
    static GIT_CANCELLATION: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

struct GitCancellationScopeGuard {
    previous: Option<Arc<AtomicBool>>,
}

impl Drop for GitCancellationScopeGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        GIT_CANCELLATION.with(|slot| {
            slot.replace(previous);
        });
    }
}

pub(crate) fn with_git_cancellation<T>(
    cancellation: Arc<AtomicBool>,
    operation: impl FnOnce() -> T,
) -> T {
    let previous = GIT_CANCELLATION.with(|slot| slot.replace(Some(cancellation)));
    let _guard = GitCancellationScopeGuard { previous };
    operation()
}

fn git_operation_cancelled() -> bool {
    GIT_CANCELLATION.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|cancellation| cancellation.load(Ordering::Acquire))
    })
}

/// Resolve Git from an absolute PATH directory outside the active workspace.
///
/// Empty/relative PATH entries and symlinks into the repository are resolved
/// before selection, so repository content cannot supply the Git executable.
pub(crate) fn trusted_git_executable(workspace: &Path) -> Result<PathBuf> {
    let workspace = workspace
        .canonicalize()
        .map_err(|error| anyhow!("Failed to resolve Git workspace: {error}"))?;
    let current_directory = std::env::current_dir()
        .map_err(|error| anyhow!("Failed to resolve the current directory: {error}"))?;
    let path = std::env::var_os("PATH").ok_or_else(|| anyhow!("PATH is not configured"))?;
    trusted_git_executable_from_path(&workspace, &current_directory, &path)
}

fn trusted_git_executable_from_path(
    workspace: &Path,
    current_directory: &Path,
    path: &std::ffi::OsStr,
) -> Result<PathBuf> {
    let workspace = workspace
        .canonicalize()
        .map_err(|error| anyhow!("Failed to resolve Git workspace: {error}"))?;
    for directory in std::env::split_paths(path) {
        let directory = if directory.as_os_str().is_empty() {
            current_directory.to_path_buf()
        } else if directory.is_absolute() {
            directory
        } else {
            current_directory.join(directory)
        };
        let Ok(directory) = directory.canonicalize() else {
            continue;
        };
        if directory.starts_with(&workspace) {
            continue;
        }
        for candidate in git_executable_candidates(&directory) {
            let Ok(candidate) = candidate.canonicalize() else {
                continue;
            };
            if candidate.starts_with(&workspace) || !is_executable_file(&candidate) {
                continue;
            }
            return Ok(candidate);
        }
    }

    Err(anyhow!(
        "Git was not found on a trusted absolute PATH entry outside {}",
        workspace.display()
    ))
}

fn git_executable_candidates(directory: &Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        ["git.exe", "git.com", "git.cmd", "git.bat", "git"]
            .into_iter()
            .map(|name| directory.join(name))
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![directory.join("git")]
    }
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn disabled_hooks_path() -> &'static str {
    #[cfg(windows)]
    {
        "NUL"
    }
    #[cfg(not(windows))]
    {
        "/dev/null"
    }
}

fn hardened_git_arguments(repo_path: &Path) -> Vec<OsString> {
    [
        "--no-pager",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "diff.external=",
        "-c",
        "credential.helper=",
        "-c",
        "protocol.allow=never",
        "-c",
    ]
    .into_iter()
    .map(OsString::from)
    .chain(std::iter::once(OsString::from(format!(
        "core.hooksPath={}",
        disabled_hooks_path()
    ))))
    .chain([OsString::from("-C"), repo_path.as_os_str().to_os_string()])
    .collect()
}

fn ambient_git_variables() -> Vec<OsString> {
    std::env::vars_os()
        .filter_map(|(key, _)| {
            let normalized = key.to_string_lossy().to_ascii_uppercase();
            (normalized.starts_with("GIT_")
                || matches!(
                    normalized.as_str(),
                    "SSH_ASKPASS" | "SSH_AUTH_SOCK" | "GCM_INTERACTIVE"
                ))
            .then_some(key)
        })
        .collect()
}

pub(crate) fn configure_git_environment(command: &mut Command, repo_path: &Path) {
    command.args(hardened_git_arguments(repo_path));
    for key in ambient_git_variables() {
        command.env_remove(key);
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", disabled_hooks_path())
        .env("GIT_CONFIG_SYSTEM", disabled_hooks_path())
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_PAGER", "cat")
        .env("GIT_OPTIONAL_LOCKS", "0");
}

pub(crate) fn configure_tokio_git_environment(
    command: &mut tokio::process::Command,
    repo_path: &Path,
) {
    command.args(hardened_git_arguments(repo_path));
    for key in ambient_git_variables() {
        command.env_remove(key);
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", disabled_hooks_path())
        .env("GIT_CONFIG_SYSTEM", disabled_hooks_path())
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_PAGER", "cat")
        .env("GIT_OPTIONAL_LOCKS", "0");
}

/// Check if a path is inside a Git repository without installing software.
pub fn is_git_repo(path: &Path) -> bool {
    run_git(path, &["rev-parse", "--git-dir"])
        .map(|(success, _, _)| success)
        .unwrap_or(false)
}

/// Run a git command.
fn run_git(repo_path: &Path, args: &[&str]) -> Result<(bool, String, String)> {
    let executable = trusted_git_executable(repo_path)?;
    run_git_with_executable(&executable, repo_path, args, GIT_COMMAND_TIMEOUT)
}

fn run_git_with_executable(
    executable: &Path,
    repo_path: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<(bool, String, String)> {
    let args = args.iter().map(OsString::from).collect::<Vec<_>>();
    let (success, stdout, stderr) =
        run_git_os_with_executable(executable, repo_path, &args, timeout)?;
    Ok((
        success,
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    ))
}

fn run_git_os(repo_path: &Path, args: &[OsString]) -> Result<(bool, Vec<u8>, Vec<u8>)> {
    let executable = trusted_git_executable(repo_path)?;
    run_git_os_with_executable(&executable, repo_path, args, GIT_COMMAND_TIMEOUT)
}

fn run_git_os_with_env(
    repo_path: &Path,
    args: &[OsString],
    environment: &[(OsString, OsString)],
) -> Result<(bool, Vec<u8>, Vec<u8>)> {
    let executable = trusted_git_executable(repo_path)?;
    run_git_os_with_executable_and_env(
        &executable,
        repo_path,
        args,
        GIT_COMMAND_TIMEOUT,
        environment,
    )
}

fn run_git_os_with_executable(
    executable: &Path,
    repo_path: &Path,
    args: &[OsString],
    timeout: Duration,
) -> Result<(bool, Vec<u8>, Vec<u8>)> {
    run_git_os_with_executable_and_env(executable, repo_path, args, timeout, &[])
}

fn run_git_os_with_executable_and_env(
    executable: &Path,
    repo_path: &Path,
    args: &[OsString],
    timeout: Duration,
    environment: &[(OsString, OsString)],
) -> Result<(bool, Vec<u8>, Vec<u8>)> {
    if git_operation_cancelled() {
        return Err(anyhow!("Git command cancelled before execution"));
    }

    let mut command = Command::new(executable);
    configure_git_environment(&mut command, repo_path);
    command
        .envs(environment.iter().cloned())
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::tools::process::configure_std_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| anyhow!("Failed to execute git: {}", e))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Git stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Git stderr was not piped"))?;
    let stdout = std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut stdout = stdout;
        stdout.read_to_end(&mut output).map(|_| output)
    });
    let stderr = std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut stderr = stderr;
        stderr.read_to_end(&mut output).map(|_| output)
    });
    let mut process_group =
        crate::tools::process::ProcessGroupGuard::for_process_id(Some(child.id()));
    let deadline = Instant::now() + timeout;
    let mut cancelled = false;
    let mut timed_out = false;

    let status = loop {
        if git_operation_cancelled() {
            cancelled = true;
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(GIT_PROCESS_POLL_INTERVAL);
            }
            Ok(None) => {
                timed_out = true;
                break None;
            }
            Err(error) => {
                process_group.kill();
                let _ = child.kill();
                let _ = child.wait();
                return Err(anyhow!("Failed to wait for git: {error}"));
            }
        }
    };

    // Git commands own no background service. Terminate the group even after
    // the direct process exits so a hook/helper cannot retain inherited pipes.
    process_group.kill();
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let stdout = stdout
        .join()
        .map_err(|_| anyhow!("Git stdout reader panicked"))??;
    let stderr = stderr
        .join()
        .map_err(|_| anyhow!("Git stderr reader panicked"))??;

    if cancelled {
        return Err(anyhow!("Git command cancelled"));
    }
    if timed_out {
        return Err(anyhow!(
            "Git command timed out after {}ms",
            timeout.as_millis()
        ));
    }
    let status = status.ok_or_else(|| anyhow!("Git command exited without a status"))?;

    Ok((status.success(), stdout, stderr))
}

// ==================== Git Operations ====================

/// One exact Git tree written through an isolated temporary index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceTreeSnapshot {
    pub tree: String,
}

/// Create a detached worktree without inheriting mutable files from another
/// Harness session. The source worktree must be clean so admission never
/// silently drops host changes.
pub(crate) fn create_isolated_worktree(repo_path: &Path, destination: &Path) -> Result<()> {
    let status = get_status(repo_path)?;
    if status.is_dirty {
        return Err(anyhow!(
            "Cannot isolate an Agent Harness session from a dirty source worktree"
        ));
    }
    if destination.exists() {
        return Err(anyhow!("Agent Harness worktree destination already exists"));
    }
    let args = vec![
        OsString::from("worktree"),
        OsString::from("add"),
        OsString::from("--detach"),
        destination.as_os_str().to_os_string(),
        OsString::from("HEAD"),
    ];
    let (success, _, stderr) = run_git_os(repo_path, &args)?;
    if !success {
        return Err(anyhow!(
            "Failed to create isolated Agent Harness worktree: {}",
            String::from_utf8_lossy(&stderr).trim_end()
        ));
    }
    Ok(())
}

/// Remove one previously admitted detached Harness worktree.
pub(crate) fn remove_isolated_worktree(repo_path: &Path, destination: &Path) -> Result<()> {
    let args = vec![
        OsString::from("worktree"),
        OsString::from("remove"),
        OsString::from("--force"),
        destination.as_os_str().to_os_string(),
    ];
    let (success, _, stderr) = run_git_os(repo_path, &args)?;
    if !success {
        return Err(anyhow!(
            "Failed to remove isolated Agent Harness worktree: {}",
            String::from_utf8_lossy(&stderr).trim_end()
        ));
    }
    Ok(())
}

/// Capture tracked and untracked, non-ignored workspace content as one Git
/// tree without mutating the worktree index or branch.
pub(crate) fn snapshot_workspace_tree(repo_path: &Path) -> Result<WorkspaceTreeSnapshot> {
    if !is_git_repo(repo_path) {
        return Err(anyhow!("Agent Harness workspace is not a Git repository"));
    }
    let index = tempfile::Builder::new()
        .prefix("a3s-code-change-index-")
        .tempfile()?;
    let index_path = index.path().to_path_buf();
    drop(index);
    if index_path.exists() {
        std::fs::remove_file(&index_path)?;
    }
    let environment = vec![(
        OsString::from("GIT_INDEX_FILE"),
        index_path.as_os_str().to_os_string(),
    )];
    let result = (|| {
        run_git_success_with_env(
            repo_path,
            &[OsString::from("read-tree"), OsString::from("HEAD")],
            &environment,
        )?;
        run_git_success_with_env(
            repo_path,
            &[
                OsString::from("add"),
                OsString::from("-A"),
                OsString::from("--"),
                OsString::from("."),
            ],
            &environment,
        )?;
        let stdout =
            run_git_success_with_env(repo_path, &[OsString::from("write-tree")], &environment)?;
        let tree = String::from_utf8(stdout)
            .map_err(|_| anyhow!("Git tree identity is not UTF-8"))?
            .trim()
            .to_owned();
        if !is_git_object_id(&tree) {
            return Err(anyhow!("Git returned an invalid workspace tree identity"));
        }
        Ok(WorkspaceTreeSnapshot { tree })
    })();
    let _ = std::fs::remove_file(index_path);
    result
}

/// Render the exact binary-capable Git patch between two captured trees.
pub(crate) fn diff_workspace_trees(
    repo_path: &Path,
    base: &WorkspaceTreeSnapshot,
    result: &WorkspaceTreeSnapshot,
    maximum_bytes: usize,
) -> Result<Vec<u8>> {
    if maximum_bytes == 0 || !is_git_object_id(&base.tree) || !is_git_object_id(&result.tree) {
        return Err(anyhow!(
            "Agent Harness change-set bounds or tree identity are invalid"
        ));
    }
    let args = [
        OsString::from("diff"),
        OsString::from("--binary"),
        OsString::from("--full-index"),
        OsString::from("--no-color"),
        OsString::from("--no-ext-diff"),
        OsString::from("--no-textconv"),
        OsString::from("--src-prefix=a/"),
        OsString::from("--dst-prefix=b/"),
        OsString::from(&base.tree),
        OsString::from(&result.tree),
        OsString::from("--"),
    ];
    let output = run_git_success_with_env(repo_path, &args, &[])?;
    if output.len() > maximum_bytes {
        return Err(anyhow!(
            "Agent Harness change set exceeds its {} byte protocol bound",
            maximum_bytes
        ));
    }
    Ok(output)
}

/// Retain a captured run tree in the repository object database across
/// worktree cleanup and Git garbage collection. The external identity is
/// hashed before it becomes part of the private ref namespace.
pub(crate) fn pin_workspace_tree(
    repo_path: &Path,
    identity: &str,
    snapshot: &WorkspaceTreeSnapshot,
) -> Result<()> {
    if identity.is_empty() || !is_git_object_id(&snapshot.tree) {
        return Err(anyhow!("Agent Harness workspace tree pin is invalid"));
    }
    let reference = format!(
        "refs/a3s-code/harness/{:x}",
        Sha256::digest(identity.as_bytes())
    );
    run_git_success_with_env(
        repo_path,
        &[
            OsString::from("update-ref"),
            OsString::from(reference),
            OsString::from(&snapshot.tree),
        ],
        &[],
    )?;
    Ok(())
}

/// Restore a freshly-created protocol worktree to one persisted result tree.
/// HEAD remains detached at the configured source revision, so the restored
/// files continue to appear as the conversation's uncommitted changes.
pub(crate) fn restore_workspace_tree(repo_path: &Path, tree_identity: &str) -> Result<()> {
    let tree = tree_identity
        .strip_prefix("git-tree:")
        .filter(|tree| is_git_object_id(tree))
        .ok_or_else(|| anyhow!("Persisted Agent Harness result tree identity is invalid"))?;
    if get_status(repo_path)?.is_dirty {
        return Err(anyhow!(
            "Cannot restore an Agent Harness result tree into a dirty worktree"
        ));
    }
    run_git_success_with_env(
        repo_path,
        &[
            OsString::from("read-tree"),
            OsString::from("--reset"),
            OsString::from("-u"),
            OsString::from(tree),
        ],
        &[],
    )?;
    Ok(())
}

fn run_git_success_with_env(
    repo_path: &Path,
    args: &[OsString],
    environment: &[(OsString, OsString)],
) -> Result<Vec<u8>> {
    let (success, stdout, stderr) = run_git_os_with_env(repo_path, args, environment)?;
    if !success {
        return Err(anyhow!(
            "Git workspace evidence command failed: {}",
            String::from_utf8_lossy(&stderr).trim_end()
        ));
    }
    Ok(stdout)
}

fn is_git_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Repository status information.
#[derive(Debug, Clone)]
pub struct RepoStatus {
    pub branch: String,
    pub commit: String,
    pub is_worktree: bool,
    pub is_dirty: bool,
    pub dirty_count: usize,
}

/// Get repository status.
pub fn get_status(repo_path: &Path) -> Result<RepoStatus> {
    let (success, stdout, _) = run_git(repo_path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = if success {
        stdout.trim().to_string()
    } else {
        "(detached)".to_string()
    };

    let (_, commit, _) = run_git(repo_path, &["log", "--oneline", "-1", "--no-decorate"])?;
    let commit = commit.trim().to_string();

    let (_, git_dir, _) = run_git(repo_path, &["rev-parse", "--git-dir"])?;
    let is_worktree = git_dir.trim().contains(".git/worktrees");

    let (_, status_output, _) = run_git(repo_path, &["status", "--porcelain", "--short"])?;
    let dirty_count = status_output.lines().filter(|l| !l.is_empty()).count();
    let is_dirty = dirty_count > 0;

    Ok(RepoStatus {
        branch,
        commit,
        is_worktree,
        is_dirty,
        dirty_count,
    })
}

/// Commit information for log display.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: String,
    pub message: String,
    pub author: String,
    pub date: String,
}

/// Get commit log.
pub fn get_log(repo_path: &Path, max_count: usize) -> Result<Vec<CommitInfo>> {
    let format = "%H|%s|%an|%ad";
    let date_format = "%Y-%m-%d %H:%M";
    let args = [
        "log",
        &format!("--format={}", format),
        &format!("--date=format:{}", date_format),
        &format!("-{}", max_count),
    ];

    let (_, stdout, _) = run_git(repo_path, &args)?;

    let commits: Vec<CommitInfo> = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, '|').collect();
            if parts.len() >= 4 {
                Some(CommitInfo {
                    id: parts[0].to_string(),
                    message: parts[1].to_string(),
                    author: parts[2].to_string(),
                    date: parts[3].to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(commits)
}

/// Branch information.
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
}

/// List all local branches.
pub fn list_branches(repo_path: &Path) -> Result<Vec<BranchInfo>> {
    let (_, stdout, _) = run_git(repo_path, &["branch"])?;

    let branches: Vec<BranchInfo> = stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let is_current = line.starts_with('*');
            let name = line.trim_start_matches(['*', ' ']).to_string();
            Some(BranchInfo { name, is_current })
        })
        .collect();

    Ok(branches)
}

/// Create a new branch.
pub fn create_branch(repo_path: &Path, name: &str, base: &str) -> Result<()> {
    let (success, _, stderr) = run_git(repo_path, &["checkout", "-b", name, base])?;
    if !success && !stderr.is_empty() {
        return Err(anyhow!("Failed to create branch: {}", stderr));
    }
    Ok(())
}

/// Worktree information.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub is_bare: bool,
    pub is_detached: bool,
}

/// List all worktrees.
pub fn list_worktrees(repo_path: &Path) -> Result<Vec<WorktreeInfo>> {
    let (_, stdout, _) = run_git(repo_path, &["worktree", "list", "--porcelain"])?;

    let mut worktrees = Vec::new();
    let mut current_path = String::new();
    let mut current_branch = String::new();
    let mut is_bare = false;
    let mut is_detached = false;

    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if !current_path.is_empty() {
                worktrees.push(WorktreeInfo {
                    path: current_path.clone(),
                    branch: current_branch.clone(),
                    is_bare,
                    is_detached,
                });
            }
            current_path = path.to_string();
            current_branch.clear();
            is_bare = false;
            is_detached = false;
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = branch.to_string();
        } else if line == "bare" {
            is_bare = true;
        } else if line == "detached" {
            is_detached = true;
        }
    }

    if !current_path.is_empty() {
        worktrees.push(WorktreeInfo {
            path: current_path,
            branch: current_branch,
            is_bare,
            is_detached,
        });
    }

    Ok(worktrees)
}

/// Create a new worktree.
pub fn create_worktree(
    repo_path: &Path,
    branch: &str,
    path: &Path,
    new_branch: bool,
) -> Result<()> {
    let path_str = path.display().to_string();
    let args: Vec<&str> = if new_branch {
        vec!["worktree", "add", "-b", branch, &path_str]
    } else {
        vec!["worktree", "add", &path_str, branch]
    };

    let (success, _, stderr) = run_git(repo_path, &args)?;
    if !success {
        return Err(anyhow!("Failed to create worktree: {}", stderr));
    }
    Ok(())
}

/// Remove a worktree.
pub fn remove_worktree(repo_path: &Path, path: &Path, force: bool) -> Result<()> {
    let path_str = path.display().to_string();
    let args: Vec<&str> = if force {
        vec!["worktree", "remove", "--force", &path_str]
    } else {
        vec!["worktree", "remove", &path_str]
    };

    let (success, _, stderr) = run_git(repo_path, &args)?;
    if !success {
        return Err(anyhow!("Failed to remove worktree: {}", stderr));
    }
    Ok(())
}

fn diff_arguments(target: Option<&str>, output_options: &[&str]) -> Result<Vec<OsString>> {
    let mut args = ["diff", "--no-ext-diff", "--no-textconv", "--no-renames"]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    args.extend(output_options.iter().map(OsString::from));
    if let Some(target) = target {
        if target.trim().is_empty() || target.contains('\0') {
            return Err(anyhow!("Git diff target must be a non-empty revision"));
        }
        args.push(OsString::from("--end-of-options"));
        args.push(OsString::from(target));
    }
    Ok(args)
}

/// Return changed paths using Git's NUL-delimited, non-quoted format.
pub(crate) fn get_diff_paths(repo_path: &Path, target: Option<&str>) -> Result<Vec<PathBuf>> {
    let mut args = diff_arguments(target, &["--name-only", "-z"])?;
    args.push(OsString::from("--"));
    args.push(OsString::from("."));

    let (success, stdout, stderr) = run_git_os(repo_path, &args)?;
    if !success {
        return Err(anyhow!(
            "Failed to list Git diff paths: {}",
            String::from_utf8_lossy(&stderr).trim_end()
        ));
    }
    stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(git_output_path)
        .collect()
}

/// Get diff output restricted to exact repository-relative paths.
pub(crate) fn get_diff_for_paths(
    repo_path: &Path,
    target: Option<&str>,
    paths: &[PathBuf],
) -> Result<String> {
    if paths.is_empty() {
        return Ok(String::new());
    }
    let mut args = diff_arguments(target, &[])?;
    args.push(OsString::from("--"));
    args.extend(paths.iter().map(|path| path.as_os_str().to_os_string()));

    let (success, stdout, stderr) = run_git_os(repo_path, &args)?;
    if !success {
        return Err(anyhow!(
            "Failed to get Git diff: {}",
            String::from_utf8_lossy(&stderr).trim_end()
        ));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

/// Get unfiltered diff output for embedders without a workspace access policy.
pub fn get_diff(repo_path: &Path, target: Option<&str>) -> Result<String> {
    get_diff_for_paths(repo_path, target, &[PathBuf::from(".")])
}

#[cfg(unix)]
fn git_output_path(path: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(path.to_vec())))
}

#[cfg(not(unix))]
fn git_output_path(path: &[u8]) -> Result<PathBuf> {
    let path = String::from_utf8(path.to_vec())
        .map_err(|_| anyhow!("Git returned a non-UTF-8 path on this platform"))?;
    Ok(PathBuf::from(path))
}

/// Stash information.
#[derive(Debug, Clone)]
pub struct StashInfo {
    pub index: usize,
    pub message: String,
}

/// List stashes.
pub fn list_stashes(repo_path: &Path) -> Result<Vec<StashInfo>> {
    let (_, stdout, _) = run_git(repo_path, &["stash", "list", "--format=%H|%gd|%s"])?;

    let stashes: Vec<StashInfo> = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if parts.len() >= 3 {
                Some(StashInfo {
                    index: parts[1].parse().unwrap_or(0),
                    message: parts[2].to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(stashes)
}

/// Create a stash.
pub fn stash(repo_path: &Path, message: Option<&str>, include_untracked: bool) -> Result<()> {
    let mut args = vec!["stash", "push"];
    if include_untracked {
        args.push("-u");
    }
    if let Some(msg) = message {
        args.push("-m");
        args.push(msg);
    }

    let (success, _, stderr) = run_git(repo_path, &args)?;
    if !success {
        return Err(anyhow!("Failed to stash: {}", stderr));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fake_git_script(directory: &Path, source: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        std::fs::create_dir_all(directory).unwrap();
        let executable = directory.join("git");
        std::fs::write(&executable, format!("#!/bin/sh\n{source}\n")).unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        executable
    }

    #[cfg(unix)]
    fn fake_git(directory: &Path) -> PathBuf {
        fake_git_script(directory, "exit 0")
    }

    #[cfg(unix)]
    #[test]
    fn trusted_git_resolution_rejects_workspace_and_symlinked_executables() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&workspace).unwrap();
        let workspace_git = fake_git(&workspace.join("bin"));
        let outside_git = fake_git(&outside);
        let linked_directory = root.path().join("linked");
        std::os::unix::fs::symlink(workspace_git.parent().unwrap(), &linked_directory).unwrap();
        let path = std::env::join_paths([
            workspace_git.parent().unwrap(),
            linked_directory.as_path(),
            outside.as_path(),
        ])
        .unwrap();

        let selected =
            trusted_git_executable_from_path(&workspace, root.path(), path.as_os_str()).unwrap();

        assert_eq!(selected, outside_git.canonicalize().unwrap());
        let untrusted_only =
            std::env::join_paths([workspace.join("bin"), linked_directory]).unwrap();
        assert!(trusted_git_executable_from_path(
            &workspace,
            root.path(),
            untrusted_only.as_os_str()
        )
        .is_err());
    }

    #[test]
    fn hardened_git_command_disables_hooks_helpers_protocols_and_prompts() {
        let mut command = Command::new("git");
        configure_git_environment(&mut command, Path::new("/workspace"));
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        for expected in [
            "--no-pager",
            "core.fsmonitor=false",
            "diff.external=",
            "credential.helper=",
            "protocol.allow=never",
            "-C",
            "/workspace",
        ] {
            assert!(arguments.iter().any(|argument| argument == expected));
        }
        assert!(arguments
            .iter()
            .any(|argument| argument.starts_with("core.hooksPath=")));
        let environment = command
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            environment.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            environment.get("GCM_INTERACTIVE").map(String::as_str),
            Some("never")
        );
    }

    #[cfg(unix)]
    #[test]
    fn git_timeout_covers_closed_streams_and_kills_descendants() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        let leak = directory.path().join("timeout-leak");
        let executable = fake_git_script(
            &directory.path().join("bin"),
            &format!("exec 1>&- 2>&-; sleep 0.30; touch '{}'", leak.display()),
        );

        let error = run_git_with_executable(
            &executable,
            &repository,
            &["status"],
            Duration::from_millis(50),
        )
        .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        std::thread::sleep(Duration::from_millis(400));
        assert!(!leak.exists());
    }

    #[cfg(unix)]
    #[test]
    fn git_cancellation_kills_the_complete_process_group() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repository");
        std::fs::create_dir_all(&repository).unwrap();
        let descendant_started = directory.path().join("descendant-started");
        let leak = directory.path().join("cancellation-leak");
        let executable = fake_git_script(
            &directory.path().join("bin"),
            &format!(
                "exec 1>&- 2>&-; \
                 (: > '{}'; sleep 0.60; : > '{}') & wait",
                descendant_started.display(),
                leak.display()
            ),
        );
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = Arc::clone(&cancellation);
        let worker = std::thread::spawn(move || {
            with_git_cancellation(worker_cancellation, || {
                run_git_with_executable(
                    &executable,
                    &repository,
                    &["status"],
                    Duration::from_secs(10),
                )
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !descendant_started.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(descendant_started.exists());

        cancellation.store(true, Ordering::Release);
        let error = worker.join().unwrap().unwrap_err();

        assert!(error.to_string().contains("cancelled"));
        std::thread::sleep(Duration::from_millis(700));
        assert!(!leak.exists());
    }
}
