//! Session `-w` create: Grove parent probe before libgit2 discover.
//!
//! is enough to identify the parent.

use std::path::{Path, PathBuf};

use a3s_fast_worktree::{NfsStatusView, NfsWorktreeClient};
use anyhow::Context;

/// Dest-layout root and optional source identity when libgit2 is skipped.
#[derive(Debug, Clone)]
pub(crate) struct GroveParentLayout {
    pub slug_root: PathBuf,
    pub source_git_root: Option<String>,
}

/// Git root used for `~/.a3s/worktrees/<slug>/` and optional `source_git_root`.
#[derive(Debug, Clone)]
pub(crate) struct CreateSourceLayout {
    pub git_root: PathBuf,
    pub source_git_root: Option<String>,
    pub skipped_libgit2: bool,
}

#[cfg(test)]
#[derive(Clone)]
struct GroveInject {
    parent: bool,
    projected: bool,
    covering: Option<PathBuf>,
    status: Option<serde_json::Value>,
}

#[cfg(test)]
impl GroveInject {
    const fn empty() -> GroveInject {
        GroveInject {
            parent: false,
            projected: false,
            covering: None,
            status: None,
        }
    }
}

/// Process-global: `spawn_blocking` and tokio worker hops cannot see a thread-local.
#[cfg(test)]
static INJECT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
static INJECT_STATE: std::sync::Mutex<GroveInject> = std::sync::Mutex::new(GroveInject::empty());

/// Holds the inject mutex for one test so concurrent inject tests serialize.
#[cfg(test)]
pub(crate) struct GroveParentInjectGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for GroveParentInjectGuard {
    fn drop(&mut self) {
        *INJECT_STATE.lock().unwrap_or_else(|e| e.into_inner()) = GroveInject::empty();
    }
}

#[cfg(test)]
pub(crate) fn lock_grove_parent_inject() -> GroveParentInjectGuard {
    let lock = INJECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    *INJECT_STATE.lock().unwrap_or_else(|e| e.into_inner()) = GroveInject::empty();
    GroveParentInjectGuard { _lock: lock }
}

#[cfg(test)]
fn inject_state() -> GroveInject {
    INJECT_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

#[cfg(test)]
pub(crate) fn inject_grove_parent() -> GroveParentInjectGuard {
    let guard = lock_grove_parent_inject();
    INJECT_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .parent = true;
    guard
}

/// Status JSON matching live `MountStatus` keys. Skip follows `keeps_grove_create`.
#[cfg(test)]
pub(crate) fn inject_grove_status(raw: serde_json::Value) -> GroveParentInjectGuard {
    let guard = lock_grove_parent_inject();
    INJECT_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .status = Some(raw);
    guard
}

/// `dest_is_grove_projection` hit with no Status (old daemon / miss).
#[cfg(test)]
pub(crate) fn inject_grove_projected() -> GroveParentInjectGuard {
    let guard = lock_grove_parent_inject();
    INJECT_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .projected = true;
    guard
}

/// Tests cannot read the kernel mount table; this stands in for a covering dest.
#[cfg(test)]
pub(crate) fn inject_grove_covering_mount(mount: PathBuf) -> GroveParentInjectGuard {
    let guard = lock_grove_parent_inject();
    INJECT_STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .covering = Some(mount);
    guard
}

#[cfg(test)]
fn layout_from_source(source: &Path) -> GroveParentLayout {
    GroveParentLayout {
        slug_root: source.to_path_buf(),
        source_git_root: Some(source.to_string_lossy().into_owned()),
    }
}

fn covering_projected_mount(source: &Path) -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(mount) = inject_state().covering
        && dest_contains(&mount, source)
    {
        return Some(mount);
    }
    let mut cur = source;
    loop {
        if a3s_fast_worktree::dest_is_projected_mount(cur) {
            return Some(cur.to_path_buf());
        }
        match cur.parent() {
            Some(parent) if parent != cur => cur = parent,
            _ => return None,
        }
    }
}

fn visible_dest_root(source: &Path, status: Option<&NfsStatusView>) -> PathBuf {
    if let Some(p) = status.and_then(NfsStatusView::slug_root) {
        return p;
    }
    covering_projected_mount(source).unwrap_or_else(|| source.to_path_buf())
}

/// Darwin `/tmp` ≡ `/private/tmp` without `stat`. `canonicalize` hangs on a wedged dest.
fn physical_dest_spelling(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let s = path.to_string_lossy();
        const DATA: &str = "/System/Volumes/Data";
        let s = if s.as_ref() == DATA {
            "/".to_owned()
        } else if let Some(rest) = s.strip_prefix(DATA).filter(|r| r.starts_with('/')) {
            rest.to_owned()
        } else {
            s.into_owned()
        };
        for (from, to) in [
            ("/tmp", "/private/tmp"),
            ("/var", "/private/var"),
            ("/etc", "/private/etc"),
        ] {
            if s == from {
                return PathBuf::from(to);
            }
            let prefix = format!("{from}/");
            if let Some(rest) = s.strip_prefix(&prefix) {
                return PathBuf::from(to).join(rest);
            }
        }
        PathBuf::from(s)
    }
    #[cfg(not(target_os = "macos"))]
    {
        path.to_path_buf()
    }
}

/// Windows mountpoint vs cwd often differs only in case. Never `canonicalize`.
fn dest_paths_match(a: &Path, b: &Path) -> bool {
    if a == b || physical_dest_spelling(a) == physical_dest_spelling(b) {
        return true;
    }
    #[cfg(windows)]
    {
        folded_path(a) == folded_path(b)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn folded_path(path: &Path) -> Vec<String> {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

#[cfg(test)]
fn dest_contains(parent: &Path, child: &Path) -> bool {
    if dest_paths_match(parent, child) || child.starts_with(parent) {
        return true;
    }
    #[cfg(windows)]
    {
        folded_path(child).starts_with(&folded_path(parent))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Launch-cwd spelling of the dest so `strip_prefix` survives `/tmp` vs `/private/tmp`.
fn source_git_root_spelling(source: &Path, slug: &Path) -> PathBuf {
    if let Some(mount) = covering_projected_mount(source) {
        return mount;
    }
    let mut cur = source;
    loop {
        if dest_paths_match(cur, slug) {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(parent) if parent != cur => cur = parent,
            _ => return slug.to_path_buf(),
        }
    }
}

/// Nested clone / submodule inside a Grove dest still uses git.
pub(crate) fn nested_git_inside_mount(source: &Path, mount: &Path) -> bool {
    let mut cur = source;
    loop {
        if dest_paths_match(cur, mount) {
            return false;
        }
        if std::fs::symlink_metadata(cur.join(".git")).is_ok() {
            return true;
        }
        match cur.parent() {
            Some(parent) if parent != cur => cur = parent,
            _ => return false,
        }
    }
}

fn layout_from_status(source: &Path, status: Option<&NfsStatusView>) -> GroveParentLayout {
    let dest = visible_dest_root(source, status);
    GroveParentLayout {
        source_git_root: Some(
            source_git_root_spelling(source, &dest)
                .to_string_lossy()
                .into_owned(),
        ),
        slug_root: dest,
    }
}

/// Covering Grove dest used as git root when libgit2 cannot open `extensions.partialclone`.
/// Walks `dest_is_projected_mount` so a ProjFS subdirectory matches Unix FUSE/NFS prefix.
#[must_use]
pub fn grove_visible_git_root(path: &Path) -> Option<PathBuf> {
    let mount = covering_projected_mount(path);
    #[cfg(test)]
    if inject_state().covering.is_some() {
        return mount;
    }
    let grove = a3s_fast_worktree::dest_is_grove_projection(path)
        || mount
            .as_deref()
            .is_some_and(a3s_fast_worktree::dest_is_grove_projection);
    if !grove {
        return None;
    }
    Some(mount.unwrap_or_else(|| path.to_path_buf()))
}

/// Nested clones keep libgit2; Grove dest is the fallback when discover fails.
#[must_use]
pub fn git_or_grove_root(path: &Path, grove: bool) -> Option<PathBuf> {
    crate::session::git::find_git_root_from_path(path)
        .ok()
        .or_else(|| grove.then(|| grove_visible_git_root(path)).flatten())
}

/// Dest slug groups under the main repo; `source_git_root` stays the nearest checkout.
pub(crate) fn jj_slug_and_source_git_root(
    path: &Path,
    grove: bool,
) -> (Option<PathBuf>, Option<PathBuf>) {
    let nearest = git_or_grove_root(path, grove);
    let slug = crate::session::git::find_main_repo_root_from_path(path)
        .ok()
        .or_else(|| nearest.clone());
    (slug, nearest)
}

/// `git_or_grove_root` off the async runtime (libgit2 discover can stall on a dest).
pub async fn git_or_grove_root_async(path: &Path, grove: bool) -> Option<PathBuf> {
    let path = path.to_path_buf();
    crate::worktree::blocking_copy_on_write(move || git_or_grove_root(&path, grove))
        .await
        .ok()
        .flatten()
}

/// Discover + `.jj` check off the async runtime.
pub async fn git_or_grove_is_jj_async(path: &Path, grove: bool) -> bool {
    let path = path.to_path_buf();
    crate::worktree::blocking_copy_on_write(move || {
        git_or_grove_root(&path, grove)
            .is_some_and(|root| crate::session::git::detect_vcs_kind(&root).is_jj())
    })
    .await
    .unwrap_or(false)
}

///
/// Grove-on ordinary git still waits on Status (ping timeout 250ms) before
/// libgit2: the RPC is the skip decision when the mount table is unavailable.
pub(crate) fn probe_grove_parent_sync(source: &Path) -> Option<GroveParentLayout> {
    #[cfg(test)]
    {
        let inj = inject_state();
        if let Some(raw) = inj.status {
            let status = NfsStatusView {
                hydration_percent: None,
                raw: Some(raw),
                port: None,
                mount_id: None,
                transport: None,
            };
            if status.keeps_grove_create() || inj.projected {
                return grove_parent_unless_nested(source, Some(&status));
            }
            return None;
        }
        if inj.parent {
            return Some(layout_from_source(source));
        }
        if inj.projected {
            return grove_parent_unless_nested(source, None);
        }
    }

    // Covering dest already decided skip vs git; Status is the mount-table miss.
    if grove_visible_git_root(source).is_some() {
        return grove_parent_unless_nested(source, None);
    }
    let opts = crate::worktree::enabled_grove_opts();
    let status = NfsWorktreeClient::from_opts(&opts).status_for_dir(source);
    if status
        .as_ref()
        .is_some_and(NfsStatusView::keeps_grove_create)
    {
        grove_parent_unless_nested(source, status.as_ref())
    } else {
        None
    }
}

fn grove_parent_unless_nested(
    source: &Path,
    status: Option<&NfsStatusView>,
) -> Option<GroveParentLayout> {
    let layout = layout_from_status(source, status);
    (!nested_git_inside_mount(source, &layout.slug_root)).then_some(layout)
}

pub(crate) fn create_source_layout_after_probe(
    source: &Path,
    grove_parent: Option<GroveParentLayout>,
) -> anyhow::Result<CreateSourceLayout> {
    if let Some(layout) = grove_parent {
        return Ok(CreateSourceLayout {
            git_root: layout.slug_root,
            source_git_root: layout.source_git_root,
            skipped_libgit2: true,
        });
    }
    let git_root = crate::session::git::find_main_repo_root_from_path(source)?;
    let source_git_root = crate::session::git::find_git_root_from_path(source)
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    Ok(CreateSourceLayout {
        git_root,
        source_git_root,
        skipped_libgit2: false,
    })
}

pub(crate) async fn resolve_create_source_layout(
    source: &Path,
    grove_requested: bool,
) -> anyhow::Result<CreateSourceLayout> {
    let source_buf = source.to_path_buf();
    crate::worktree::blocking_copy_on_write(move || {
        let probe = if grove_requested {
            probe_grove_parent_sync(&source_buf)
        } else {
            None
        };
        create_source_layout_after_probe(&source_buf, probe)
    })
    .await
    .context("create source layout join failed")?
}

#[cfg(test)]
#[path = "create_root_tests.rs"]
mod tests;
