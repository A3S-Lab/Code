//! Conversation writes land in a resettable git worktree.
//!
//! Isolation uses [`WorkspaceGitWorktreeProvider`]. A non-git root fails
//! closed. Discard does not touch the source tree. Promote applies one
//! change-set digest and refuses a moved source revision before any apply.

use crate::workspace::{
    LocalWorkspaceAccessBoundary, LocalWorkspaceAccessPolicy, LocalWorkspaceBackend,
    WorkspaceGitRemoveWorktreeRequest, WorkspaceGitWorktreeProvider,
};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::content_digest::digest_bytes;

const ISOLATION_UNAVAILABLE: &str = "isolation unavailable";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationBinding {
    pub session_id: String,
    pub source_root: PathBuf,
    pub worktree_path: PathBuf,
    pub source_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromoteOutcome {
    Applied {
        digest: String,
    },
    Idempotent {
        digest: String,
    },
    Conflict {
        bound_revision: String,
        current_revision: String,
    },
}

#[derive(Debug, Default)]
struct IsolationState {
    bindings: HashMap<String, IsolationBinding>,
    applied: HashMap<String, String>,
}

fn state() -> &'static Mutex<IsolationState> {
    static STATE: OnceLock<Mutex<IsolationState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(IsolationState::default()))
}

/// Read-only sessions bind the source tree and do not create a worktree.
pub fn skip_for_read_only(can_write: bool) -> bool {
    !can_write
}

/// Sync bind used by session construction. Uses the same git worktree helpers
/// as [`LocalWorkspaceBackend`].
pub fn bind_sync(
    session_id: &str,
    source_root: &Path,
    requested: bool,
    can_write: bool,
) -> Result<Option<IsolationBinding>> {
    if !requested || skip_for_read_only(can_write) {
        return Ok(None);
    }
    if !is_git_repository(source_root) {
        return Err(anyhow!("{ISOLATION_UNAVAILABLE}"));
    }
    let revision = source_revision(source_root)?;
    let worktree_path = worktree_path_for(source_root, session_id);
    if worktree_path.starts_with(source_root) {
        return Err(anyhow!(
            "{ISOLATION_UNAVAILABLE}: worktree path must not sit inside the source tree"
        ));
    }
    refuse_retargeted_worktree(source_root, &worktree_path)?;
    refuse_symlink_worktree(&worktree_path)?;
    if let Some(existing) = state()
        .lock()
        .expect("isolation state")
        .bindings
        .get(session_id)
        .cloned()
    {
        if existing.source_root == source_root && existing.worktree_path.exists() {
            register_shell_on_worktree(session_id, &existing.worktree_path);
            return Ok(Some(existing));
        }
    }
    if worktree_path.join(".git").exists() {
        let binding = IsolationBinding {
            session_id: session_id.to_string(),
            source_root: source_root.to_path_buf(),
            worktree_path,
            source_revision: revision,
        };
        state()
            .lock()
            .expect("isolation state")
            .bindings
            .insert(session_id.to_string(), binding.clone());
        register_shell_on_worktree(session_id, &binding.worktree_path);
        return Ok(Some(binding));
    }
    // Sibling paths are keyed only by session id under the source parent. A
    // crashed prior run can leave an empty `.a3s-isolate-*` directory that is
    // not a git worktree; refuse to adopt it and clear the orphan so bind can
    // recreate. Do not delete a path that still has `.git` metadata.
    if worktree_path.exists() {
        std::fs::remove_dir_all(&worktree_path).map_err(|error| {
            anyhow!(
                "{ISOLATION_UNAVAILABLE}: stale isolation path {} could not be cleared: {error}",
                worktree_path.display()
            )
        })?;
    }
    crate::git::create_worktree(
        source_root,
        &format!("a3s-isolate-{session_id}"),
        &worktree_path,
        true,
    )?;
    let binding = IsolationBinding {
        session_id: session_id.to_string(),
        source_root: source_root.to_path_buf(),
        worktree_path,
        source_revision: revision,
    };
    state()
        .lock()
        .expect("isolation state")
        .bindings
        .insert(session_id.to_string(), binding.clone());
    register_shell_on_worktree(session_id, &binding.worktree_path);
    Ok(Some(binding))
}

/// Open an isolated worktree, or fail closed without writing `source_root`.
pub async fn bind(
    session_id: &str,
    source_root: &Path,
    can_write: bool,
) -> Result<IsolationBinding> {
    bind_sync(session_id, source_root, true, can_write)?
        .ok_or_else(|| anyhow!("isolation is not created for a session that cannot write"))
}

/// Digest of the isolated change set, without applying it.
///
/// `Ok(None)` means the worktree has nothing to promote. A missing binding
/// is an error.
pub fn current_change_digest(session_id: &str) -> Result<Option<String>> {
    let bound = binding(session_id)
        .ok_or_else(|| anyhow!("isolation unavailable: conversation {session_id} is not bound"))?;
    let changes = capture_change_set(&bound)?;
    if changes.is_empty() {
        return Ok(None);
    }
    Ok(Some(changes.digest()))
}

/// Promote the isolated worktree onto its source tree.
///
/// The change-set digest covers the exact patch and untracked files. A source
/// revision that moved returns [`PromoteOutcome::Conflict`] before that patch
/// is applied. Replaying the same digest is idempotent and does not apply again.
pub fn promote_current(session_id: &str) -> Result<PromoteOutcome> {
    let bound = binding(session_id)
        .ok_or_else(|| anyhow!("isolation unavailable: conversation {session_id} is not bound"))?;
    let changes = capture_change_set(&bound)?;
    if changes.is_empty() {
        return Err(anyhow!("nothing to promote"));
    }
    let digest = changes.digest();
    let current = source_revision(&bound.source_root)?;
    promote(session_id, &digest, &current, move |binding| {
        changes.apply(binding)
    })
}

enum PromoteFile {
    Write { path: String, content: Vec<u8> },
    Delete { path: String },
}

struct IsolatedChangeSet {
    files: Vec<PromoteFile>,
}

impl IsolatedChangeSet {
    fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    fn digest(&self) -> String {
        let mut bytes = Vec::new();
        for file in &self.files {
            match file {
                PromoteFile::Write { path, content } => {
                    bytes.extend(b"W\0");
                    bytes.extend(path.as_bytes());
                    bytes.push(0);
                    bytes.extend(content);
                    bytes.push(0);
                }
                PromoteFile::Delete { path } => {
                    bytes.extend(b"D\0");
                    bytes.extend(path.as_bytes());
                    bytes.push(0);
                }
            }
        }
        digest_bytes("a3s.code.isolation-promote.v1", &bytes)
    }

    fn apply(self, binding: &IsolationBinding) -> Result<()> {
        let boundary = LocalWorkspaceAccessBoundary::for_policy(
            LocalWorkspaceAccessPolicy::CredentialBoundary,
            &binding.source_root,
        )
        .ok_or_else(|| anyhow!("credential boundary unavailable for isolation promote"))?;
        for file in self.files {
            match file {
                PromoteFile::Write { path, content } => {
                    write_promoted_file(
                        &binding.session_id,
                        &binding.source_root,
                        &path,
                        &content,
                        &boundary,
                    )?;
                }
                PromoteFile::Delete { path } => {
                    let relative = safe_relative_path(&path)?;
                    refuse_symlink_components(&binding.source_root, &relative)?;
                    boundary
                        .refuse_promote(&binding.source_root, &relative)
                        .map_err(|error| anyhow!("refusing to promote: {error}"))?;
                    let destination = binding.source_root.join(&relative);
                    if destination.is_file() {
                        let claim =
                            begin_promote_claim(&binding.session_id, &binding.source_root, &path)?;
                        if let Err(error) = std::fs::remove_file(&destination) {
                            claim.release_if_new();
                            return Err(anyhow!(
                                "failed to remove {}: {error}",
                                destination.display()
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn capture_change_set(binding: &IsolationBinding) -> Result<IsolatedChangeSet> {
    let listing = git_stdout(
        &binding.worktree_path,
        &[
            "diff",
            "--name-status",
            "--no-renames",
            "-z",
            binding.source_revision.as_str(),
        ],
    )?;
    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut records = listing
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty());
    while let Some(status) = records.next() {
        let path_raw = records
            .next()
            .ok_or_else(|| anyhow!("isolation diff listed a status without a path"))?;
        let status = String::from_utf8(status.to_vec())
            .map_err(|_| anyhow!("isolation diff listed a non-utf8 status"))?;
        let path = String::from_utf8(path_raw.to_vec())
            .map_err(|_| anyhow!("isolation diff listed a non-utf8 path"))?;
        let kind = status
            .chars()
            .next()
            .ok_or_else(|| anyhow!("isolation diff listed an empty status"))?;
        match kind {
            'A' | 'M' | 'T' => {
                files.push(read_promoted_file(&binding.worktree_path, &path)?);
            }
            'D' => files.push(PromoteFile::Delete { path: path.clone() }),
            _ => {
                return Err(anyhow!(
                    "refusing to promote unsupported git status {status}"
                ))
            }
        }
        seen.insert(path);
    }
    let untracked = git_stdout(
        &binding.worktree_path,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    for raw in untracked.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let path = String::from_utf8(raw.to_vec())
            .map_err(|_| anyhow!("isolation worktree listed a non-utf8 path"))?;
        if seen.contains(&path) {
            continue;
        }
        files.push(read_promoted_file(&binding.worktree_path, &path)?);
    }
    files.sort_by(|left, right| left.path().cmp(right.path()));
    Ok(IsolatedChangeSet { files })
}

impl PromoteFile {
    fn path(&self) -> &str {
        match self {
            Self::Write { path, .. } | Self::Delete { path } => path,
        }
    }
}

fn read_promoted_file(root: &Path, path: &str) -> Result<PromoteFile> {
    let relative = safe_relative_path(path)?;
    refuse_symlink_components(root, &relative)?;
    let file = root.join(&relative);
    let metadata = std::fs::symlink_metadata(&file)
        .map_err(|error| anyhow!("failed to read isolated {}: {error}", file.display()))?;
    if !metadata.is_file() {
        return Err(anyhow!("refusing to promote non-regular file {path}"));
    }
    let content = std::fs::read(&file)
        .map_err(|error| anyhow!("failed to read isolated {}: {error}", file.display()))?;
    Ok(PromoteFile::Write {
        path: path.to_string(),
        content,
    })
}

struct PromoteClaim {
    session_id: String,
    root: PathBuf,
    path: String,
    newly_acquired: bool,
}

impl PromoteClaim {
    fn release_if_new(&self) {
        if self.newly_acquired {
            crate::external_observation::release_write_claim(
                &self.session_id,
                &self.root,
                &self.path,
            );
        }
    }
}

/// Refuse a path another session already owns, then hold the claim only if
/// the following mutation lands.
fn begin_promote_claim(session_id: &str, root: &Path, path: &str) -> Result<PromoteClaim> {
    let newly_acquired = !crate::external_observation::session_owns_write(session_id, root, path);
    crate::external_observation::claim_bound_write(Some(session_id), root, path)
        .map_err(|error| anyhow!(error))?;
    Ok(PromoteClaim {
        session_id: session_id.to_string(),
        root: root.to_path_buf(),
        path: path.to_string(),
        newly_acquired,
    })
}

fn write_promoted_file(
    session_id: &str,
    root: &Path,
    path: &str,
    content: &[u8],
    boundary: &LocalWorkspaceAccessBoundary,
) -> Result<()> {
    let relative = safe_relative_path(path)?;
    refuse_symlink_components(root, &relative)?;
    boundary
        .refuse_promote(root, &relative)
        .map_err(|error| anyhow!("refusing to promote: {error}"))?;
    let destination = root.join(&relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| anyhow!("failed to create {}: {error}", parent.display()))?;
    }
    let claim = begin_promote_claim(session_id, root, path)?;
    if let Err(error) = std::fs::write(&destination, content) {
        claim.release_if_new();
        return Err(anyhow!(
            "failed to promote {}: {error}",
            destination.display()
        ));
    }
    Ok(())
}

fn git_stdout(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let _guard = git_process_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    git_stdout_unlocked(root, args)
}

fn git_stdout_unlocked(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| {
            anyhow!(
                "git {} failed: {error}",
                args.first().copied().unwrap_or("diff")
            )
        })?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(anyhow!(
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn refuse_symlink_components(root: &Path, relative: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(anyhow!("refusing to promote unsafe path {relative:?}"));
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(anyhow!("refusing to promote through a symbolic link"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(anyhow!(
                    "failed to inspect promote path {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn safe_relative_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    // `is_absolute()` is not enough. On Windows `/tmp/escape` is not absolute,
    // but a root or drive prefix is still not a workspace-relative path.
    let escapes = path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if path.is_absolute() || escapes {
        return Err(anyhow!("refusing to promote unsafe path {path:?}"));
    }
    Ok(path.to_path_buf())
}

pub fn binding(session_id: &str) -> Option<IsolationBinding> {
    state()
        .lock()
        .expect("isolation state")
        .bindings
        .get(session_id)
        .cloned()
}

/// Remove the conversation worktree. Does not delete source files.
///
/// Idempotent when the worktree directory is already gone or Git no longer
/// lists it as a worktree (for example after an earlier cleanup).
pub async fn discard(session_id: &str) -> Result<()> {
    let binding = state()
        .lock()
        .expect("isolation state")
        .bindings
        .remove(session_id)
        .ok_or_else(|| anyhow!("isolation unavailable: no binding for {session_id}"))?;
    let backend = LocalWorkspaceBackend::new(binding.source_root.clone());
    match backend
        .remove_worktree(WorkspaceGitRemoveWorktreeRequest {
            path: binding.worktree_path.display().to_string(),
            force: true,
        })
        .await
    {
        Ok(_) => {}
        Err(error) => {
            let message = error.to_string();
            let already_gone = !binding.worktree_path.exists()
                || message.contains("is not a working tree")
                || message.contains("not a valid path");
            if !already_gone {
                // Re-bind so a later discard/retry can still find the session.
                state()
                    .lock()
                    .expect("isolation state")
                    .bindings
                    .insert(session_id.to_string(), binding);
                return Err(error);
            }
            let _ = std::fs::remove_dir_all(&binding.worktree_path);
        }
    }
    crate::shell_session::drop_session(session_id);
    Ok(())
}

/// Apply one change-set. A moved source revision returns before `apply`.
pub fn promote<F>(
    session_id: &str,
    digest: &str,
    current_revision: &str,
    apply: F,
) -> Result<PromoteOutcome>
where
    F: FnOnce(&IsolationBinding) -> Result<()>,
{
    if digest.trim().is_empty() {
        return Err(anyhow!("promote requires a change-set digest"));
    }
    let guard = state().lock().expect("isolation state");
    // The digest is the applied change-set identity. A later session must not
    // apply it again just because the first marker was recorded under another id.
    if guard.applied.contains_key(digest) {
        return Ok(PromoteOutcome::Idempotent {
            digest: digest.to_string(),
        });
    }
    let binding = guard
        .bindings
        .get(session_id)
        .cloned()
        .ok_or_else(|| anyhow!("isolation unavailable: no binding for {session_id}"))?;
    if current_revision != binding.source_revision {
        return Ok(PromoteOutcome::Conflict {
            bound_revision: binding.source_revision,
            current_revision: current_revision.to_string(),
        });
    }
    drop(guard);
    apply(&binding)?;
    state()
        .lock()
        .expect("isolation state")
        .applied
        .insert(digest.to_string(), session_id.to_string());
    Ok(PromoteOutcome::Applied {
        digest: digest.to_string(),
    })
}

pub fn refuse_retargeted_worktree(source_root: &Path, worktree_path: &Path) -> Result<()> {
    let expected_parent = source_root.parent().unwrap_or(source_root);
    let name = worktree_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let single_sibling = worktree_path.parent() == Some(expected_parent)
        && name.starts_with(".a3s-isolate-")
        && !name.contains(['/', '\\'])
        && !worktree_path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir));
    if single_sibling {
        return Ok(());
    }
    Err(anyhow!(
        "{ISOLATION_UNAVAILABLE}: worktree path must be a sibling of the source tree"
    ))
}

fn refuse_symlink_worktree(path: &Path) -> Result<()> {
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(anyhow!(
            "{ISOLATION_UNAVAILABLE}: worktree path is a symlink"
        ));
    }
    Ok(())
}

/// Sibling path an isolated session uses for its worktree.
///
/// Hosts that clean up after a session must use this path. The session id is
/// not trusted as a relative path; bind still refuses a retarget onto the source.
pub fn worktree_path_for(source_root: &Path, session_id: &str) -> PathBuf {
    let parent = source_root.parent().unwrap_or(source_root);
    parent.join(format!(".a3s-isolate-{session_id}"))
}

fn git_process_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

fn is_git_repository(root: &Path) -> bool {
    let _guard = git_process_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-dir"])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn source_revision(root: &Path) -> Result<String> {
    let _guard = git_process_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "{ISOLATION_UNAVAILABLE}: source revision is unknown"
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn register_shell_on_worktree(session_id: &str, worktree: &Path) {
    crate::shell_session::bind_session_rooted(session_id, worktree, worktree);
}

#[cfg(test)]
mod tests;
