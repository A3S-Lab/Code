//! Behavioural conformance suite for workspace backend implementations.
//!
//! This module captures the **implicit invariants** that every
//! [`WorkspaceFileSystem`] and [`WorkspaceFileSystemExt`] implementation must
//! satisfy. Until this suite landed those invariants only lived in commits,
//! comments, and the maintainer's head — new backends would have to
//! reverse-engineer them from existing implementations.
//!
//! # How to use
//!
//! When you add a new backend (S3, GCS, container, browser, ...):
//!
//! ```ignore
//! #[tokio::test]
//! async fn my_backend_conforms_to_filesystem() {
//!     let fs: Arc<dyn WorkspaceFileSystem> = Arc::new(MyBackend::new(...));
//!     conformance::assert_filesystem_conformance(fs, "MyBackend").await;
//! }
//! ```
//!
//! When the suite grows (new invariant discovered, e.g. an S3 production
//! incident), every backend running it gets the regression test for free —
//! that is the point.
//!
//! # What is covered
//!
//! * [`assert_filesystem_conformance`] — the required
//!   [`WorkspaceFileSystem`] surface: read-after-write round-trips, errors
//!   on nonexistent reads / list_dirs, listing returns directory entries,
//!   write overwrites existing content, write creates parent path
//!   components.
//! * [`assert_filesystem_ext_conformance`] — the optional
//!   [`WorkspaceFileSystemExt`] CAS surface: version round-trip,
//!   compare-and-swap success on matching version, downcastable
//!   [`WorkspaceVersionConflict`] on mismatch, empty-version rejection.
//!
//! Out of scope (intentionally) for the first cut:
//!
//! * `WorkspaceCommandRunner` — shell semantics differ enough across
//!   sandboxes that a single contract is hard to write.
//! * `WorkspaceSearch` — glob and regex behaviour are inherently
//!   implementation-defined; conformance tests would over-constrain.
//! * `WorkspaceGit*` — implementation behaviour is too coupled to the
//!   underlying git engine to capture in a single trait contract.
//!
//! # Reference backend
//!
//! [`InMemoryFileSystem`] is a tiny `HashMap`-backed implementation that
//! exists for two purposes: (1) it runs the conformance suite against
//! itself in this module, proving the contract is internally consistent;
//! (2) it serves as the smallest possible reference for what a backend
//! that satisfies the suite has to look like. Real production backends
//! (`LocalWorkspaceBackend`, `S3WorkspaceBackend`) layer real I/O on top
//! of the same observable semantics.

use super::{
    WorkspaceDirEntry, WorkspaceFileSystem, WorkspaceFileSystemExt, WorkspacePath,
    WorkspaceVersionConflict, WorkspaceWriteOutcome,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ============================================================================
// Conformance assertions for WorkspaceFileSystem
// ============================================================================

/// Run the full required-surface conformance suite against a backend.
///
/// On failure, the panic message includes `ctx` so test output names the
/// backend under test even when several backends share the same harness.
pub(crate) async fn assert_filesystem_conformance(fs: Arc<dyn WorkspaceFileSystem>, ctx: &str) {
    fs_read_after_write_roundtrip(&fs, ctx).await;
    fs_read_nonexistent_errors(&fs, ctx).await;
    fs_write_overwrites_existing(&fs, ctx).await;
    fs_write_creates_parent_components(&fs, ctx).await;
    fs_list_dir_root_succeeds(&fs, ctx).await;
    fs_list_dir_after_write_sees_the_entry(&fs, ctx).await;
    fs_list_dir_nonexistent_errors(&fs, ctx).await;
}

/// Run the optional CAS-surface conformance suite against a backend that
/// implements both [`WorkspaceFileSystem`] and [`WorkspaceFileSystemExt`].
///
/// Backends that don't implement `Ext` just don't call this.
pub(crate) async fn assert_filesystem_ext_conformance(
    fs: Arc<dyn WorkspaceFileSystem>,
    ext: Arc<dyn WorkspaceFileSystemExt>,
    ctx: &str,
) {
    ext_version_token_is_non_empty(&fs, &ext, ctx).await;
    ext_write_with_matching_version_succeeds(&fs, &ext, ctx).await;
    ext_write_with_stale_version_yields_conflict(&fs, &ext, ctx).await;
    ext_empty_expected_version_is_rejected(&ext, ctx).await;
}

// ---- individual invariants ----

async fn fs_read_after_write_roundtrip(fs: &Arc<dyn WorkspaceFileSystem>, ctx: &str) {
    let path = WorkspacePath::from_normalized("conformance/roundtrip.txt");
    fs.write_text(&path, "hello world")
        .await
        .unwrap_or_else(|e| panic!("[{ctx}] write_text failed: {e}"));
    let content = fs
        .read_text(&path)
        .await
        .unwrap_or_else(|e| panic!("[{ctx}] read_text after write failed: {e}"));
    assert_eq!(
        content, "hello world",
        "[{ctx}] read_text must return exactly what write_text wrote"
    );
}

async fn fs_read_nonexistent_errors(fs: &Arc<dyn WorkspaceFileSystem>, ctx: &str) {
    let path = WorkspacePath::from_normalized("conformance/definitely-not-there.txt");
    let result = fs.read_text(&path).await;
    assert!(
        result.is_err(),
        "[{ctx}] read_text on a path that was never written must error, got Ok({:?})",
        result.ok()
    );
}

async fn fs_write_overwrites_existing(fs: &Arc<dyn WorkspaceFileSystem>, ctx: &str) {
    let path = WorkspacePath::from_normalized("conformance/overwrite.txt");
    fs.write_text(&path, "v1").await.unwrap();
    fs.write_text(&path, "v2").await.unwrap();
    let content = fs.read_text(&path).await.unwrap();
    assert_eq!(
        content, "v2",
        "[{ctx}] second write_text must overwrite first"
    );
}

async fn fs_write_creates_parent_components(fs: &Arc<dyn WorkspaceFileSystem>, ctx: &str) {
    let path = WorkspacePath::from_normalized("conformance/deep/nested/path/file.txt");
    fs.write_text(&path, "deep").await.unwrap_or_else(|e| {
        panic!("[{ctx}] write_text must create missing parent components: {e}")
    });
    let content = fs.read_text(&path).await.unwrap();
    assert_eq!(content, "deep", "[{ctx}] write_text round-trip at depth");
}

async fn fs_list_dir_root_succeeds(fs: &Arc<dyn WorkspaceFileSystem>, ctx: &str) {
    let result = fs.list_dir(&WorkspacePath::root()).await;
    assert!(
        result.is_ok(),
        "[{ctx}] list_dir on the workspace root must always succeed (may be empty), got {:?}",
        result
    );
}

async fn fs_list_dir_after_write_sees_the_entry(fs: &Arc<dyn WorkspaceFileSystem>, ctx: &str) {
    // Use a fresh subdirectory so we know exactly what should appear.
    let dir = WorkspacePath::from_normalized("conformance/listing-test");
    let file = WorkspacePath::from_normalized("conformance/listing-test/visible.txt");
    fs.write_text(&file, "hello").await.unwrap();

    let entries = fs
        .list_dir(&dir)
        .await
        .unwrap_or_else(|e| panic!("[{ctx}] list_dir after write failed: {e}"));
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"visible.txt"),
        "[{ctx}] list_dir must include just-written entry; got {names:?}"
    );
}

async fn fs_list_dir_nonexistent_errors(fs: &Arc<dyn WorkspaceFileSystem>, ctx: &str) {
    let path = WorkspacePath::from_normalized("conformance/never-created");
    let result = fs.list_dir(&path).await;
    assert!(
        result.is_err(),
        "[{ctx}] list_dir on a nonexistent path must error, got Ok({:?}); \
         a silent empty result masks user typos",
        result.ok()
    );
}

async fn ext_version_token_is_non_empty(
    fs: &Arc<dyn WorkspaceFileSystem>,
    ext: &Arc<dyn WorkspaceFileSystemExt>,
    ctx: &str,
) {
    let path = WorkspacePath::from_normalized("conformance/cas-version.txt");
    fs.write_text(&path, "seed").await.unwrap();
    let (content, version) = ext
        .read_text_with_version(&path)
        .await
        .unwrap_or_else(|e| panic!("[{ctx}] read_text_with_version failed: {e}"));
    assert_eq!(content, "seed");
    assert!(
        !version.is_empty(),
        "[{ctx}] read_text_with_version must return a non-empty opaque token"
    );
}

async fn ext_write_with_matching_version_succeeds(
    fs: &Arc<dyn WorkspaceFileSystem>,
    ext: &Arc<dyn WorkspaceFileSystemExt>,
    ctx: &str,
) {
    let path = WorkspacePath::from_normalized("conformance/cas-match.txt");
    fs.write_text(&path, "v0").await.unwrap();
    let (_, version) = ext.read_text_with_version(&path).await.unwrap();
    ext.write_text_if_version(&path, "v1", &version)
        .await
        .unwrap_or_else(|e| {
            panic!("[{ctx}] write_text_if_version with matching version must succeed: {e}")
        });
    assert_eq!(fs.read_text(&path).await.unwrap(), "v1");
}

async fn ext_write_with_stale_version_yields_conflict(
    fs: &Arc<dyn WorkspaceFileSystem>,
    ext: &Arc<dyn WorkspaceFileSystemExt>,
    ctx: &str,
) {
    let path = WorkspacePath::from_normalized("conformance/cas-conflict.txt");
    fs.write_text(&path, "alpha").await.unwrap();
    let (_, stale_version) = ext.read_text_with_version(&path).await.unwrap();

    // Simulate a concurrent writer that bumps the version.
    fs.write_text(&path, "concurrent").await.unwrap();

    let err = ext
        .write_text_if_version(&path, "beta", &stale_version)
        .await
        .expect_err(&format!(
            "[{ctx}] write_text_if_version with stale version must reject"
        ));
    assert!(
        err.downcast_ref::<WorkspaceVersionConflict>().is_some(),
        "[{ctx}] CAS rejection must be downcastable to WorkspaceVersionConflict; got: {err:?}"
    );
}

async fn ext_empty_expected_version_is_rejected(ext: &Arc<dyn WorkspaceFileSystemExt>, ctx: &str) {
    let path = WorkspacePath::from_normalized("conformance/cas-empty-ver.txt");
    let err = ext
        .write_text_if_version(&path, "anything", "")
        .await
        .expect_err(&format!(
            "[{ctx}] write_text_if_version with empty expected version must reject"
        ));
    // Don't require a specific message — just that it's a clear failure.
    let _ = err;
}

// ============================================================================
// Reference backend: InMemoryFileSystem
// ============================================================================

/// A `HashMap<String, (content, version)>`-backed reference implementation.
///
/// Purpose:
/// * **Self-test the conformance suite.** The suite must pass against an
///   ideally-behaving backend; if it fails here, the contract itself is
///   inconsistent.
/// * **Document the shape.** A few hundred lines of obvious code show new
///   backend authors what the trait surface "wants".
/// * **Mock for higher-level tests.** Helper modules can wire this into a
///   `WorkspaceServices` to exercise tool code paths without touching disk
///   or network.
///
/// The file map and the version counter live behind a single mutex so the
/// CAS write is genuinely atomic — a separate counter mutex would allow
/// a concurrent writer to slip in between the version check and the
/// version bump, defeating the conformance test for stale-version
/// rejection.
struct InMemoryState {
    files: HashMap<String, (String, String)>,
    counter: u64,
}

pub(crate) struct InMemoryFileSystem {
    state: Mutex<InMemoryState>,
}

impl InMemoryFileSystem {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(InMemoryState {
                files: HashMap::new(),
                counter: 0,
            }),
        }
    }

    fn bump_version(state: &mut InMemoryState) -> String {
        state.counter += 1;
        format!("v{}", state.counter)
    }
}

impl Default for InMemoryFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WorkspaceFileSystem for InMemoryFileSystem {
    async fn read_text(&self, path: &WorkspacePath) -> Result<String> {
        self.state
            .lock()
            .unwrap()
            .files
            .get(path.as_str())
            .map(|(c, _)| c.clone())
            .ok_or_else(|| anyhow!("path not found: {}", path.as_str()))
    }

    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> Result<WorkspaceWriteOutcome> {
        let mut state = self.state.lock().unwrap();
        let version = Self::bump_version(&mut state);
        state
            .files
            .insert(path.as_str().to_string(), (content.to_string(), version));
        Ok(WorkspaceWriteOutcome {
            bytes: content.len(),
            lines: content.lines().count(),
        })
    }

    async fn list_dir(&self, path: &WorkspacePath) -> Result<Vec<WorkspaceDirEntry>> {
        // Synthesize a directory view from the flat key space. For a
        // requested directory at `path`, anything stored under
        // `<path>/<rest>` shows up. Mid-path components become Directory
        // entries; direct children become File entries.
        let prefix: String = if path.is_root() {
            String::new()
        } else {
            format!("{}/", path.as_str())
        };
        let state = self.state.lock().unwrap();
        let mut seen_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut entries: Vec<WorkspaceDirEntry> = Vec::new();
        let mut any = false;
        for (key, (content, _)) in state.files.iter() {
            let Some(rest) = key.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            any = true;
            if let Some((dir_name, _)) = rest.split_once('/') {
                if seen_dirs.insert(dir_name.to_string()) {
                    entries.push(WorkspaceDirEntry {
                        name: dir_name.to_string(),
                        kind: super::WorkspaceFileType::Directory,
                        size: 0,
                    });
                }
            } else {
                entries.push(WorkspaceDirEntry {
                    name: rest.to_string(),
                    kind: super::WorkspaceFileType::File,
                    size: content.len() as u64,
                });
            }
        }
        if !path.is_root() && !any {
            bail!("path not found: {}", path.as_str());
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
}

#[async_trait]
impl WorkspaceFileSystemExt for InMemoryFileSystem {
    async fn read_text_with_version(&self, path: &WorkspacePath) -> Result<(String, String)> {
        self.state
            .lock()
            .unwrap()
            .files
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| anyhow!("path not found: {}", path.as_str()))
    }

    async fn write_text_if_version(
        &self,
        path: &WorkspacePath,
        content: &str,
        expected_version: &str,
    ) -> Result<WorkspaceWriteOutcome> {
        if expected_version.is_empty() {
            bail!("expected_version must not be empty");
        }
        // Hold the single mutex across the entire compare-and-swap so a
        // concurrent writer cannot slip between the version check and the
        // version bump.
        let mut state = self.state.lock().unwrap();
        let actual = state.files.get(path.as_str()).map(|(_, v)| v.clone());
        match actual {
            Some(actual) if actual == expected_version => {
                let new_version = Self::bump_version(&mut state);
                state.files.insert(
                    path.as_str().to_string(),
                    (content.to_string(), new_version),
                );
                Ok(WorkspaceWriteOutcome {
                    bytes: content.len(),
                    lines: content.lines().count(),
                })
            }
            Some(actual) => Err(anyhow::Error::new(WorkspaceVersionConflict {
                path: path.as_str().to_string(),
                expected: expected_version.to_string(),
                actual: Some(actual),
            })),
            None => Err(anyhow!("path not found: {}", path.as_str())),
        }
    }
}

// ============================================================================
// Self-test: the conformance suite must pass against the in-memory backend.
// And the local backend must too, proving the contract is implementable
// over real I/O.
// ============================================================================

#[cfg(test)]
mod self_tests {
    use super::*;
    use crate::workspace::LocalWorkspaceBackend;

    #[tokio::test]
    async fn in_memory_backend_satisfies_filesystem_conformance() {
        let fs: Arc<dyn WorkspaceFileSystem> = Arc::new(InMemoryFileSystem::new());
        assert_filesystem_conformance(fs, "InMemoryFileSystem").await;
    }

    #[tokio::test]
    async fn in_memory_backend_satisfies_filesystem_ext_conformance() {
        let backend = Arc::new(InMemoryFileSystem::new());
        let fs: Arc<dyn WorkspaceFileSystem> = backend.clone();
        let ext: Arc<dyn WorkspaceFileSystemExt> = backend;
        assert_filesystem_ext_conformance(fs, ext, "InMemoryFileSystem").await;
    }

    #[tokio::test]
    async fn local_backend_satisfies_filesystem_conformance() {
        let temp = tempfile::tempdir().unwrap();
        let fs: Arc<dyn WorkspaceFileSystem> =
            Arc::new(LocalWorkspaceBackend::new(temp.path().to_path_buf()));
        assert_filesystem_conformance(fs, "LocalWorkspaceBackend").await;
    }
}
