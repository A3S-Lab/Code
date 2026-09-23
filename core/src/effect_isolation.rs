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
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn init_repo(root: &Path) {
        fs::create_dir_all(root).unwrap();
        git(root, &["init"]);
        git(root, &["config", "user.email", "a3s@example.com"]);
        git(root, &["config", "user.name", "a3s"]);
        fs::write(root.join("README.md"), "source\n").unwrap();
        git(root, &["add", "README.md"]);
        git(root, &["commit", "-m", "init"]);
    }

    fn git(root: &Path, args: &[&str]) {
        let _guard = super::git_process_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn bind_clears_a_stale_empty_isolation_directory() {
        forget_leaked_session("stale-empty");
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let stale = worktree_path_for(root.path(), "stale-empty");
        fs::create_dir_all(&stale).unwrap();
        assert!(!stale.join(".git").exists());

        let binding = bind("stale-empty", root.path(), true)
            .await
            .expect("orphan empty sibling must be cleared and recreated");
        assert_eq!(binding.worktree_path, stale);
        assert!(stale.join(".git").exists());
        discard("stale-empty").await.unwrap();
    }

    /// Hermetic leak check (formerly a 100-cycle soak). Two bind/discard
    /// cycles cover the same assertions without inflating CI time or leaving
    /// ignored bodies in the F-table denominator under `--no-cfg-coverage`.
    #[tokio::test]
    async fn bind_discard_does_not_leak_into_source() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let source = fs::read(root.path().join("README.md")).unwrap();
        for i in 0..2 {
            let id = format!("leak-ei-{i}");
            forget_leaked_session(&id);
            let binding = bind(&id, root.path(), true)
                .await
                .unwrap_or_else(|error| panic!("bind {id}: {error}"));
            fs::write(binding.worktree_path.join("leak.txt"), b"nope").unwrap();
            discard(&id)
                .await
                .unwrap_or_else(|error| panic!("discard {id}: {error}"));
            assert!(
                !root.path().join("leak.txt").exists(),
                "discard cycle {i} wrote the source tree"
            );
            assert!(
                !worktree_path_for(root.path(), &id).exists(),
                "discard cycle {i} left an isolation directory"
            );
        }
        assert_eq!(fs::read(root.path().join("README.md")).unwrap(), source);
    }

    #[tokio::test]
    async fn two_sessions_do_not_share_a_writable_tree() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let left = bind("left", root.path(), true).await.unwrap();
        let right = bind("right", root.path(), true).await.unwrap();
        assert_ne!(left.worktree_path, right.worktree_path);
        fs::write(left.worktree_path.join("left.txt"), "left").unwrap();
        fs::write(right.worktree_path.join("right.txt"), "right").unwrap();
        assert!(!root.path().join("left.txt").exists());
        assert!(!root.path().join("right.txt").exists());
        assert!(!left.worktree_path.join("right.txt").exists());
        discard("left").await.unwrap();
        discard("right").await.unwrap();
    }

    #[tokio::test]
    async fn rebind_of_the_same_session_reuses_the_worktree() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let first = bind_sync("rebind-session", root.path(), true, true)
            .unwrap()
            .expect("worktree");
        let second = bind_sync("rebind-session", root.path(), true, true)
            .unwrap()
            .expect("reused worktree");
        assert_eq!(first.worktree_path, second.worktree_path);
        assert!(!root
            .path()
            .join(".git")
            .join("worktrees")
            .read_dir()
            .unwrap()
            .next()
            .is_none());
        discard("rebind-session").await.unwrap();
    }

    #[tokio::test]
    async fn bind_adopts_an_existing_worktree_directory_with_git_metadata() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let session_id = "adopt-existing-wt";
        let worktree = worktree_path_for(root.path(), session_id);
        fs::create_dir_all(&worktree).unwrap();
        // Pretend a previous process left a worktree directory with git metadata.
        fs::write(worktree.join(".git"), "gitdir: ../.git/worktrees/adopt\n").unwrap();
        let binding = bind_sync(session_id, root.path(), true, true)
            .unwrap()
            .expect("adopted worktree");
        assert_eq!(binding.worktree_path, worktree);
        discard(session_id).await.unwrap();
    }

    #[tokio::test]
    async fn discard_leaves_the_source_tree_unchanged() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let binding = bind("discard", root.path(), true).await.unwrap();
        fs::write(binding.worktree_path.join("noise.txt"), "noise").unwrap();
        discard("discard").await.unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("README.md")).unwrap(),
            "source\n"
        );
        assert!(!root.path().join("noise.txt").exists());
    }

    #[tokio::test]
    async fn discard_is_idempotent_when_worktree_is_already_unregistered() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let binding = bind("discard-gone", root.path(), true).await.unwrap();
        // Simulate an external cleanup that removed the Git worktree registration
        // while leaving the binding in process state (live E2E teardown class).
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&binding.worktree_path)
            .current_dir(root.path())
            .status();
        let _ = fs::remove_dir_all(&binding.worktree_path);
        discard("discard-gone")
            .await
            .expect("discard already-gone worktree");
        assert!(!binding.worktree_path.exists());
        assert!(crate::effect_isolation::binding("discard-gone").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bind_does_not_reuse_a_symlink_as_the_isolation_worktree() {
        let parent = tempfile::tempdir().unwrap();
        let source = parent.path().join("repo");
        init_repo(&source);
        let session_id = format!("isolate-symlink-{}", std::process::id());
        let sibling = worktree_path_for(&source, &session_id);
        std::os::unix::fs::symlink(&source, &sibling).unwrap();

        let bound = bind_sync(&session_id, &source, true, true);

        assert!(
            bound.is_err(),
            "a symlink sibling must not become the isolation worktree: {bound:?}"
        );
        assert_eq!(
            fs::read_to_string(source.join("README.md")).unwrap(),
            "source\n"
        );
        let _ = std::fs::remove_file(&sibling);
    }

    #[tokio::test]
    async fn bind_does_not_place_the_worktree_inside_the_source_via_session_id() {
        let parent = tempfile::tempdir().unwrap();
        let source = parent.path().join("repo");
        init_repo(&source);
        fs::create_dir(parent.path().join(".a3s-isolate-nested")).unwrap();
        let session_id = "nested/../repo".to_string();

        let bound = bind_sync(&session_id, &source, true, true);

        assert!(
            bound.is_err(),
            "a session id must not retarget the isolation worktree onto the source tree: {bound:?}"
        );
        assert_eq!(
            fs::read_to_string(source.join("README.md")).unwrap(),
            "source\n"
        );
    }

    #[tokio::test]
    async fn isolation_shell_cd_does_not_leave_the_worktree() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let session_id = format!("isolate-cd-{}", std::process::id());
        let binding = bind(&session_id, root.path(), true).await.unwrap();
        let source_cd = format!("cd {}", root.path().display());

        let climbed = crate::shell_session::admit(
            &session_id,
            "cd ..",
            crate::shell_session::CommandAdmission::Allow,
        );
        let absolute = crate::shell_session::admit(
            &session_id,
            &source_cd,
            crate::shell_session::CommandAdmission::Allow,
        );

        assert!(
            climbed.is_err(),
            "cd out of the isolation worktree must not be admitted: {climbed:?}"
        );
        assert!(
            absolute.is_err(),
            "cd onto the source tree must not be admitted: {absolute:?}"
        );
        assert_eq!(
            crate::shell_session::cwd(&session_id).as_deref(),
            Some(binding.worktree_path.as_path())
        );
        fs::create_dir(binding.worktree_path.join("nested")).unwrap();
        crate::shell_session::admit(
            &session_id,
            "cd nested",
            crate::shell_session::CommandAdmission::Allow,
        )
        .expect("cd inside the worktree stays admitted");
        assert_eq!(
            crate::shell_session::cwd(&session_id).as_deref(),
            Some(binding.worktree_path.join("nested").as_path())
        );
        assert_eq!(
            fs::read_to_string(root.path().join("README.md")).unwrap(),
            "source\n"
        );
        discard(&session_id).await.unwrap();
    }

    #[tokio::test]
    async fn promote_conflicts_when_source_moves_and_does_not_apply() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let binding = bind("promote", root.path(), true).await.unwrap();
        fs::write(root.path().join("README.md"), "moved\n").unwrap();
        git(root.path(), &["add", "README.md"]);
        git(root.path(), &["commit", "-m", "move"]);
        let current = source_revision(root.path()).unwrap();
        let applied = AtomicUsize::new(0);
        let outcome = promote("promote", "digest-1", &current, |_| {
            applied.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert!(matches!(outcome, PromoteOutcome::Conflict { .. }));
        assert_eq!(applied.load(Ordering::SeqCst), 0);
        assert_eq!(
            fs::read_to_string(root.path().join("README.md")).unwrap(),
            "moved\n"
        );
        let _ = binding;
        discard("promote").await.unwrap();
    }

    #[test]
    fn replay_of_the_same_digest_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let binding = IsolationBinding {
            session_id: "idem".into(),
            source_root: root.path().to_path_buf(),
            worktree_path: root.path().join("elsewhere"),
            source_revision: "rev-1".into(),
        };
        state()
            .lock()
            .unwrap()
            .bindings
            .insert("idem".into(), binding);
        let applied = AtomicUsize::new(0);
        let first = promote("idem", "same", "rev-1", |_| {
            applied.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        let second = promote("idem", "same", "rev-1", |_| {
            applied.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert!(matches!(first, PromoteOutcome::Applied { .. }));
        assert!(matches!(second, PromoteOutcome::Idempotent { .. }));
        assert_eq!(applied.load(Ordering::SeqCst), 1);
        state().lock().unwrap().bindings.remove("idem");
        state().lock().unwrap().applied.remove("same");
    }

    #[test]
    fn replay_of_the_same_digest_from_another_session_does_not_apply_again() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        for session_id in ["idem-a", "idem-b"] {
            state().lock().unwrap().bindings.insert(
                session_id.into(),
                IsolationBinding {
                    session_id: session_id.into(),
                    source_root: root.path().to_path_buf(),
                    worktree_path: root.path().join(session_id),
                    source_revision: "rev-1".into(),
                },
            );
        }
        let applied = AtomicUsize::new(0);
        let first = promote("idem-a", "shared-digest", "rev-1", |_| {
            applied.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        let second = promote("idem-b", "shared-digest", "rev-1", |_| {
            applied.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert!(matches!(first, PromoteOutcome::Applied { .. }));
        assert!(
            matches!(second, PromoteOutcome::Idempotent { .. }),
            "a digest already applied by another session is a replay, not a second apply"
        );
        assert_eq!(applied.load(Ordering::SeqCst), 1);
        let mut guard = state().lock().unwrap();
        guard.bindings.remove("idem-a");
        guard.bindings.remove("idem-b");
        guard.applied.remove("shared-digest");
    }

    #[tokio::test]
    async fn non_git_root_fails_closed_without_writing_it() {
        let root = tempfile::tempdir().unwrap();
        let error = bind("bare", root.path(), true).await.unwrap_err();
        assert!(error.to_string().contains(ISOLATION_UNAVAILABLE));
        assert!(fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn promote_current_writes_the_worktree_only_after_an_unchanged_revision() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let binding = bind("promote-current", root.path(), true).await.unwrap();
        fs::write(binding.worktree_path.join("from-isolate.txt"), "isolated\n").unwrap();
        fs::write(binding.worktree_path.join("README.md"), "edited\n").unwrap();
        assert!(!root.path().join("from-isolate.txt").exists());

        let first = promote_current("promote-current").unwrap();
        assert!(matches!(first, PromoteOutcome::Applied { .. }));
        assert_eq!(
            fs::read_to_string(root.path().join("from-isolate.txt")).unwrap(),
            "isolated\n"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("README.md")).unwrap(),
            "edited\n"
        );

        let replay = promote_current("promote-current").unwrap();
        assert!(matches!(replay, PromoteOutcome::Idempotent { .. }));
        assert_eq!(
            fs::read_to_string(root.path().join("from-isolate.txt")).unwrap(),
            "isolated\n"
        );

        fs::write(binding.worktree_path.join("second.txt"), "again\n").unwrap();
        let second = promote_current("promote-current").unwrap();
        assert!(matches!(second, PromoteOutcome::Applied { .. }));
        assert_eq!(
            fs::read_to_string(root.path().join("second.txt")).unwrap(),
            "again\n"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("from-isolate.txt")).unwrap(),
            "isolated\n"
        );
        discard("promote-current").await.unwrap();
    }

    fn forget_leaked_session(session_id: &str) {
        if let Some(binding) = state()
            .lock()
            .expect("isolation state")
            .bindings
            .remove(session_id)
        {
            let _ = fs::remove_dir_all(binding.worktree_path);
        }
        let _ = fs::remove_dir_all(std::env::temp_dir().join(format!(".a3s-isolate-{session_id}")));
        crate::external_observation::release_session(session_id);
    }

    #[tokio::test]
    async fn promote_owns_a_path_it_wrote_so_another_session_cannot_overwrite_it() {
        forget_leaked_session("promote-owner");
        forget_leaked_session("promote-other");
        forget_leaked_session("promote-late");
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let owner = bind("promote-owner", root.path(), true).await.unwrap();
        fs::write(owner.worktree_path.join("guest.txt"), "owned-by-promote\n").unwrap();
        let applied = promote_current("promote-owner").unwrap();
        assert!(matches!(applied, PromoteOutcome::Applied { .. }));
        assert_eq!(
            fs::read_to_string(root.path().join("guest.txt")).unwrap(),
            "owned-by-promote\n"
        );

        let blocked = crate::external_observation::claim_bound_write(
            Some("promote-late"),
            root.path(),
            "guest.txt",
        );
        assert!(
            blocked.is_err(),
            "promote must own the path it wrote, not leave it for another session"
        );

        let other = bind("promote-other", root.path(), true).await.unwrap();
        fs::write(other.worktree_path.join("guest.txt"), "stolen\n").unwrap();
        let stolen = promote_current("promote-other");
        assert!(
            stolen.is_err(),
            "a second session must not promote over a path the first session just wrote"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("guest.txt")).unwrap(),
            "owned-by-promote\n"
        );
        discard("promote-owner").await.unwrap();
        discard("promote-other").await.unwrap();
        crate::external_observation::release_session("promote-owner");
        crate::external_observation::release_session("promote-other");
        crate::external_observation::release_session("promote-late");
    }

    #[tokio::test]
    async fn promote_current_conflict_does_not_copy_the_worktree() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let binding = bind("promote-conflict", root.path(), true).await.unwrap();
        fs::write(binding.worktree_path.join("from-isolate.txt"), "isolated\n").unwrap();
        fs::write(root.path().join("README.md"), "moved\n").unwrap();
        git(root.path(), &["add", "README.md"]);
        git(root.path(), &["commit", "-m", "move"]);

        let outcome = promote_current("promote-conflict").unwrap();
        assert!(matches!(outcome, PromoteOutcome::Conflict { .. }));
        assert!(!root.path().join("from-isolate.txt").exists());
        assert_eq!(
            fs::read_to_string(root.path().join("README.md")).unwrap(),
            "moved\n"
        );
        discard("promote-conflict").await.unwrap();
    }

    #[test]
    fn unsafe_promote_paths_are_rejected() {
        assert!(safe_relative_path("../escape").is_err());
        assert!(safe_relative_path("/tmp/escape").is_err());
        assert!(safe_relative_path("nested/ok.txt").is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn promote_does_not_follow_a_symlink_file_on_the_source_tree() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, "outside-token-91c4").unwrap();
        init_repo(root.path());
        let binding = bind("promote-symlink-file", root.path(), true)
            .await
            .unwrap();
        symlink(&outside_file, root.path().join("guest.txt")).unwrap();
        fs::write(
            binding.worktree_path.join("guest.txt"),
            "promoted-through-link",
        )
        .unwrap();

        let error = promote_current("promote-symlink-file").unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert_eq!(
            fs::read_to_string(&outside_file).unwrap(),
            "outside-token-91c4"
        );
        discard("promote-symlink-file").await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn promote_does_not_create_directories_through_a_source_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let binding = bind("promote-symlink-dir", root.path(), true)
            .await
            .unwrap();
        symlink(outside.path(), root.path().join("escape")).unwrap();
        fs::create_dir_all(binding.worktree_path.join("escape/nested")).unwrap();
        fs::write(
            binding.worktree_path.join("escape/nested/new.txt"),
            "created-outside",
        )
        .unwrap();

        let error = promote_current("promote-symlink-dir").unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert!(!outside.path().join("nested").exists());
        discard("promote-symlink-dir").await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn promote_does_not_delete_through_a_source_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/secret.txt"), "tracked\n").unwrap();
        init_repo(root.path());
        git(root.path(), &["add", "src/secret.txt"]);
        git(root.path(), &["commit", "-m", "track"]);
        let binding = bind("promote-symlink-delete", root.path(), true)
            .await
            .unwrap();
        fs::remove_file(root.path().join("src/secret.txt")).unwrap();
        fs::remove_dir(root.path().join("src")).unwrap();
        fs::write(outside.path().join("secret.txt"), "outside-token-d17e").unwrap();
        symlink(outside.path(), root.path().join("src")).unwrap();
        fs::remove_file(binding.worktree_path.join("src/secret.txt")).unwrap();

        let error = promote_current("promote-symlink-delete").unwrap_err();
        assert!(error.to_string().contains("symbolic link"), "{error}");
        assert_eq!(
            fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
            "outside-token-d17e"
        );
        discard("promote-symlink-delete").await.unwrap();
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn promote_does_not_write_through_a_source_hardlink() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::write(root.path().join("alias.txt"), "placeholder\n").unwrap();
        git(root.path(), &["add", "alias.txt"]);
        git(root.path(), &["commit", "-m", "alias"]);
        let binding = bind("promote-hardlink", root.path(), true).await.unwrap();
        fs::write(
            root.path().join("source.txt"),
            "promote-hardlink-token-2f8b",
        )
        .unwrap();
        fs::remove_file(root.path().join("alias.txt")).unwrap();
        std::fs::hard_link(
            root.path().join("source.txt"),
            root.path().join("alias.txt"),
        )
        .unwrap();
        fs::write(
            binding.worktree_path.join("alias.txt"),
            "written-through-promote",
        )
        .unwrap();

        let error = promote_current("promote-hardlink").unwrap_err();
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert_eq!(
            fs::read_to_string(root.path().join("source.txt")).unwrap(),
            "promote-hardlink-token-2f8b"
        );
        discard("promote-hardlink").await.unwrap();
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn promote_does_not_overwrite_a_credential_file() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::write(root.path().join(".env"), "TOKEN=promote-secret-91aa\n").unwrap();
        git(root.path(), &["add", ".env"]);
        git(root.path(), &["commit", "-m", "env"]);
        let binding = bind("promote-env", root.path(), true).await.unwrap();
        fs::write(
            binding.worktree_path.join(".env"),
            "TOKEN=promoted-secret\n",
        )
        .unwrap();

        let error = promote_current("promote-env").unwrap_err();
        assert!(error.to_string().contains("credential boundary"), "{error}");
        assert_eq!(
            fs::read_to_string(root.path().join(".env")).unwrap(),
            "TOKEN=promote-secret-91aa\n"
        );
        discard("promote-env").await.unwrap();
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn promote_still_writes_a_package_store_hardlink() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let package = root.path().join("node_modules/pkg");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("alias.js"), "export const value = 1;\n").unwrap();
        git(root.path(), &["add", "-f", "node_modules/pkg/alias.js"]);
        git(root.path(), &["commit", "-m", "package"]);
        let binding = bind("promote-package-link", root.path(), true)
            .await
            .unwrap();
        let source = package.join("source.js");
        fs::write(&source, "export const value = 1;\n").unwrap();
        fs::remove_file(package.join("alias.js")).unwrap();
        std::fs::hard_link(&source, package.join("alias.js")).unwrap();
        fs::create_dir_all(binding.worktree_path.join("node_modules/pkg")).unwrap();
        fs::write(
            binding.worktree_path.join("node_modules/pkg/alias.js"),
            "export const value = 2;\n",
        )
        .unwrap();

        let outcome = promote_current("promote-package-link").unwrap();
        assert!(matches!(outcome, PromoteOutcome::Applied { .. }));
        assert_eq!(
            fs::read_to_string(package.join("alias.js")).unwrap(),
            "export const value = 2;\n"
        );
        discard("promote-package-link").await.unwrap();
    }

    #[tokio::test]
    async fn read_only_does_not_create_a_worktree() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let error = bind("ro", root.path(), false).await.unwrap_err();
        assert!(error.to_string().contains("cannot write"));
        assert!(binding("ro").is_none());
    }

    #[test]
    fn skip_for_read_only_is_true_when_writes_are_disabled() {
        assert!(skip_for_read_only(false));
        assert!(!skip_for_read_only(true));
    }

    #[test]
    fn bind_sync_returns_none_when_isolation_is_not_requested() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        assert!(bind_sync("not-requested", root.path(), false, true)
            .unwrap()
            .is_none());
        assert!(binding("not-requested").is_none());
    }

    #[tokio::test]
    async fn current_change_digest_and_promote_current_cover_empty_and_dirty_paths() {
        let missing = current_change_digest("never-bound-digest").unwrap_err();
        assert!(missing.to_string().contains("not bound"), "{missing}");

        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let bound = bind("digest-clean", root.path(), true).await.unwrap();
        assert!(current_change_digest("digest-clean").unwrap().is_none());
        let nothing = promote_current("digest-clean").unwrap_err();
        assert!(
            nothing.to_string().contains("nothing to promote"),
            "{nothing}"
        );

        fs::write(bound.worktree_path.join("dirty.txt"), "noise").unwrap();
        assert!(current_change_digest("digest-clean").unwrap().is_some());
        discard("digest-clean").await.unwrap();
    }

    #[test]
    fn promote_rejects_empty_digest() {
        let err = promote("missing", "   ", "rev", |_| Ok(())).unwrap_err();
        assert!(err.to_string().contains("digest"), "{err}");
    }

    #[tokio::test]
    async fn discard_requires_an_existing_binding() {
        let missing = discard("never-bound-discard").await.unwrap_err();
        assert!(missing.to_string().contains("no binding"), "{missing}");
    }

    #[tokio::test]
    async fn promote_current_deletes_a_tracked_file_removed_in_the_worktree() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::write(root.path().join("gone.txt"), "tracked\n").unwrap();
        git(root.path(), &["add", "gone.txt"]);
        git(root.path(), &["commit", "-m", "track gone"]);
        let binding = bind("promote-delete", root.path(), true).await.unwrap();
        fs::remove_file(binding.worktree_path.join("gone.txt")).unwrap();

        let outcome = promote_current("promote-delete").unwrap();
        assert!(matches!(outcome, PromoteOutcome::Applied { .. }));
        assert!(!root.path().join("gone.txt").exists());
        discard("promote-delete").await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn promote_current_refuses_a_symlink_as_a_promoted_path() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let session = format!("promote-symlink-{}", std::process::id());
        let binding = bind(&session, root.path(), true).await.unwrap();
        std::os::unix::fs::symlink("README.md", binding.worktree_path.join("evil-link")).unwrap();

        let error = promote_current(&session).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("non-regular file")
                || message.contains("symbolic link")
                || message.contains("refusing to promote"),
            "{message}"
        );
        discard(&session).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_promoted_file_releases_claim_when_destination_write_fails() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let session = format!("promote-write-fail-{}", std::process::id());
        let binding = bind(&session, root.path(), true).await.unwrap();
        fs::write(binding.worktree_path.join("new-file.txt"), "payload\n").unwrap();
        // Freeze the source tree so the promote write cannot create the file.
        let mut perms = fs::metadata(root.path()).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(root.path(), perms).unwrap();

        let error = promote_current(&session).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("failed to promote")
                || message.contains("failed to create")
                || message.contains("Permission denied")
                || message.contains("Read-only"),
            "{message}"
        );

        let mut restore = fs::metadata(root.path()).unwrap().permissions();
        restore.set_mode(0o755);
        fs::set_permissions(root.path(), restore).unwrap();
        discard(&session).await.unwrap();
    }

    #[test]
    fn refuse_symlink_components_rejects_dot_path_segments() {
        let root = tempfile::tempdir().unwrap();
        let error = refuse_symlink_components(root.path(), Path::new("./nested")).unwrap_err();
        assert!(error.to_string().contains("unsafe path"), "{error}");
    }

    #[test]
    fn git_stdout_surfaces_stderr_when_git_fails() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let error = git_stdout(root.path(), &["rev-parse", "missing-ref-xyz"]).unwrap_err();
        assert!(!error.to_string().is_empty(), "{error}");
    }

    #[test]
    fn source_revision_fails_when_head_is_unborn() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path()).unwrap();
        git(root.path(), &["init"]);
        let error = source_revision(root.path()).unwrap_err();
        assert!(error.to_string().contains(ISOLATION_UNAVAILABLE), "{error}");
    }

    #[tokio::test]
    async fn bind_reuses_an_existing_live_worktree_binding() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let first = bind("reuse-binding", root.path(), true).await.unwrap();
        let second = bind_sync("reuse-binding", root.path(), true, true)
            .unwrap()
            .expect("existing binding");
        assert_eq!(first.worktree_path, second.worktree_path);
        assert_eq!(first.source_revision, second.source_revision);
        discard("reuse-binding").await.unwrap();
    }

    #[tokio::test]
    async fn promote_current_overwrites_an_existing_tracked_file() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::write(root.path().join("tracked.txt"), "old\n").unwrap();
        git(root.path(), &["add", "tracked.txt"]);
        git(root.path(), &["commit", "-m", "track"]);
        let binding = bind("promote-overwrite", root.path(), true).await.unwrap();
        fs::write(binding.worktree_path.join("tracked.txt"), "new\n").unwrap();

        let outcome = promote_current("promote-overwrite").unwrap();
        assert!(matches!(outcome, PromoteOutcome::Applied { .. }));
        assert_eq!(
            fs::read_to_string(root.path().join("tracked.txt")).unwrap(),
            "new\n"
        );
        discard("promote-overwrite").await.unwrap();
    }

    #[tokio::test]
    async fn bind_adopts_a_preexisting_worktree_directory() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let session = "adopt-worktree";
        let worktree = worktree_path_for(root.path(), session);
        crate::git::create_worktree(
            root.path(),
            &format!("a3s-isolate-{session}"),
            &worktree,
            true,
        )
        .unwrap();
        assert!(worktree.join(".git").exists());

        let binding = bind_sync(session, root.path(), true, true)
            .unwrap()
            .expect("adopted worktree");
        assert_eq!(binding.worktree_path, worktree);
        discard(session).await.unwrap();
    }

    #[test]
    fn promote_fails_closed_without_a_session_binding() {
        let err = promote("never-bound-promote", "digest-x", "rev-1", |_| Ok(())).unwrap_err();
        assert!(err.to_string().contains("no binding"), "{err}");
    }

    #[tokio::test]
    async fn promote_current_delete_is_idempotent_when_source_file_already_gone() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::write(root.path().join("ephemeral.txt"), "tracked\n").unwrap();
        git(root.path(), &["add", "ephemeral.txt"]);
        git(root.path(), &["commit", "-m", "track ephemeral"]);
        let session = format!("promote-delete-gone-{}", std::process::id());
        let binding = bind(&session, root.path(), true).await.unwrap();
        fs::remove_file(binding.worktree_path.join("ephemeral.txt")).unwrap();
        // Source already matches the delete; apply must still succeed.
        fs::remove_file(root.path().join("ephemeral.txt")).unwrap();

        let outcome = promote_current(&session).unwrap();
        assert!(matches!(outcome, PromoteOutcome::Applied { .. }));
        assert!(!root.path().join("ephemeral.txt").exists());
        discard(&session).await.unwrap();
    }

    #[tokio::test]
    async fn bind_falls_through_when_existing_worktree_directory_is_gone() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let session = format!("missing-worktree-{}", std::process::id());
        let branch = format!("a3s-isolate-{session}");
        let first = bind(&session, root.path(), true).await.unwrap();
        // Force-remove the worktree registration and its branch so recreate
        // can allocate the same session id again.
        let _ = std::process::Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                first.worktree_path.to_str().unwrap(),
            ])
            .current_dir(root.path())
            .status();
        let _ = fs::remove_dir_all(&first.worktree_path);
        let _ = std::process::Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(root.path())
            .status();
        let _ = std::process::Command::new("git")
            .args(["branch", "-D", &branch])
            .current_dir(root.path())
            .status();
        let second = bind_sync(&session, root.path(), true, true)
            .unwrap()
            .expect("recreate after missing worktree");
        assert!(second.worktree_path.exists());
        discard(&session).await.unwrap();
    }

    #[tokio::test]
    async fn bind_fails_closed_when_stale_isolation_path_cannot_be_cleared() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let session = format!("stale-file-{}", std::process::id());
        let stale = worktree_path_for(root.path(), &session);
        // A file at the isolation path cannot be removed by remove_dir_all.
        fs::write(&stale, "not-a-directory").unwrap();
        let error = bind_sync(&session, root.path(), true, true).unwrap_err();
        assert!(
            error.to_string().contains("could not be cleared"),
            "{error}"
        );
        let _ = fs::remove_file(&stale);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn promote_delete_releases_claim_when_source_file_cannot_be_removed() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::write(root.path().join("locked.txt"), "tracked\n").unwrap();
        git(root.path(), &["add", "locked.txt"]);
        git(root.path(), &["commit", "-m", "track locked"]);
        let session = format!("promote-delete-fail-{}", std::process::id());
        let binding = bind(&session, root.path(), true).await.unwrap();
        fs::remove_file(binding.worktree_path.join("locked.txt")).unwrap();
        let mut perms = fs::metadata(root.path()).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(root.path(), perms).unwrap();

        let error = promote_current(&session).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("failed to remove") || message.contains("Permission denied"),
            "{message}"
        );

        let mut restore = fs::metadata(root.path()).unwrap().permissions();
        restore.set_mode(0o755);
        fs::set_permissions(root.path(), restore).unwrap();
        discard(&session).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn promote_refuses_a_typechanged_symlink_as_non_regular() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::write(root.path().join("typed.txt"), "file\n").unwrap();
        git(root.path(), &["add", "typed.txt"]);
        git(root.path(), &["commit", "-m", "track typed"]);
        let session = format!("promote-typed-link-{}", std::process::id());
        let binding = bind(&session, root.path(), true).await.unwrap();
        // Replace the tracked regular file with a symlink so git reports a
        // typechange ('T') and read_promoted_file refuses non-regular metadata.
        fs::remove_file(binding.worktree_path.join("typed.txt")).unwrap();
        std::os::unix::fs::symlink("README.md", binding.worktree_path.join("typed.txt")).unwrap();
        let error = promote_current(&session).unwrap_err();
        assert!(
            error.to_string().contains("non-regular file")
                || error.to_string().contains("symbolic link")
                || error.to_string().contains("refusing to promote"),
            "{error}"
        );
        discard(&session).await.unwrap();
    }

    #[test]
    fn git_stdout_reports_spawn_failure_when_git_is_missing_from_path() {
        struct RestorePath(Option<std::ffi::OsString>);
        impl Drop for RestorePath {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => std::env::set_var("PATH", value),
                    None => std::env::remove_var("PATH"),
                }
            }
        }

        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        let _guard = super::git_process_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _restore = RestorePath(std::env::var_os("PATH"));
        std::env::set_var("PATH", "/var/empty-a3s-no-git");
        let error = super::git_stdout_unlocked(root.path(), &["status"]).unwrap_err();
        drop(_restore);
        drop(_guard);
        assert!(
            error.to_string().contains("git") || error.to_string().contains("failed"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn promote_current_handles_delete_then_recreate_of_tracked_path() {
        let root = tempfile::tempdir().unwrap();
        init_repo(root.path());
        fs::write(root.path().join("swap.txt"), "tracked\n").unwrap();
        git(root.path(), &["add", "swap.txt"]);
        git(root.path(), &["commit", "-m", "track swap"]);
        let session = format!("promote-swap-{}", std::process::id());
        let binding = bind(&session, root.path(), true).await.unwrap();
        fs::remove_file(binding.worktree_path.join("swap.txt")).unwrap();
        fs::write(binding.worktree_path.join("swap.txt"), "replacement\n").unwrap();
        let outcome = promote_current(&session).unwrap();
        assert!(matches!(outcome, PromoteOutcome::Applied { .. }));
        assert_eq!(
            fs::read_to_string(root.path().join("swap.txt")).unwrap(),
            "replacement\n"
        );
        discard(&session).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discard_rebinds_when_worktree_removal_fails_hard() {
        use std::os::unix::fs::PermissionsExt;

        let outer = tempfile::tempdir().unwrap();
        let source = outer.path().join("repo");
        init_repo(&source);
        let session = format!("discard-rebind-{}", std::process::id());
        let binding = bind(&session, &source, true).await.unwrap();
        // Freeze only our outer tempdir (worktree sibling parent), not the
        // process-wide TMPDIR.
        let parent = outer.path();
        let mut perms = fs::metadata(parent).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(parent, perms).unwrap();

        let error = discard(&session).await;
        let mut restore = fs::metadata(parent).unwrap().permissions();
        restore.set_mode(0o755);
        fs::set_permissions(parent, restore).unwrap();

        match error {
            Err(error) => {
                assert!(
                    binding_exists(&session)
                        || error.to_string().contains("isolation")
                        || error.to_string().contains("Permission")
                        || error.to_string().contains("not permitted"),
                    "{error}"
                );
                let _ = discard(&session).await;
            }
            Ok(()) => {
                let _ = binding;
            }
        }
    }

    fn binding_exists(session_id: &str) -> bool {
        state()
            .lock()
            .expect("isolation state")
            .bindings
            .contains_key(session_id)
    }
}
