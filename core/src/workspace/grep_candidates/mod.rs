//! Optional content-level candidate pruning for workspace `grep`.
//!
//! Manifest search already limits which paths exist. This layer optionally
//! narrows that set further using a trigram index before the exact regex
//! scan opens file contents. Providers must fail open: when a pattern cannot
//! be safely indexed, return [`GrepCandidateSelection::Unconstrained`] so the
//! existing full-scan path remains authoritative.
//!
//! This plane is intentionally separate from durable zvec FTS/BM25.

use std::collections::BTreeSet;
use std::sync::Arc;

/// Result of asking a candidate index to prune grep paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrepCandidateSelection {
    /// Provider has no usable narrowing advice; callers must full-scan.
    Unconstrained,
    /// Provider is unavailable or failed closed toward the safe path.
    Unavailable,
    /// Relative workspace paths that may contain matches (false positives OK).
    Paths(BTreeSet<String>),
}

/// Content-level grep candidate provider.
///
/// Implementations use an in-tree trigram plan (tgrep-style). They must never
/// become the match authority: Code still runs the exact regex over admitted
/// file bytes.
pub trait GrepCandidateIndex: Send + Sync {
    /// Select candidate relative paths for `pattern`.
    ///
    /// Returning [`GrepCandidateSelection::Unconstrained`] or
    /// [`GrepCandidateSelection::Unavailable`] disables pruning for this
    /// query. Empty [`GrepCandidateSelection::Paths`] means "no file can
    /// match" and is a valid narrowing result.
    fn select_paths(&self, pattern: &str, case_insensitive: bool) -> GrepCandidateSelection;
}

/// Provider that never narrows candidates.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnconstrainedGrepCandidateIndex;

impl GrepCandidateIndex for UnconstrainedGrepCandidateIndex {
    fn select_paths(&self, _pattern: &str, _case_insensitive: bool) -> GrepCandidateSelection {
        GrepCandidateSelection::Unconstrained
    }
}

/// Shared handle used by workspace backends.
pub type SharedGrepCandidateIndex = Arc<dyn GrepCandidateIndex>;

#[cfg(feature = "grep-trigram")]
mod trigram_index;

#[cfg(feature = "grep-trigram")]
pub use trigram_index::{
    TrigramGrepCandidateIndex, TrigramGrepCandidateIndexError, AUTO_GREP_TRIGRAM_MAX_FILES,
    GREP_TRIGRAM_INDEX_RELATIVE_DIR,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconstrained_provider_never_narrows() {
        let index = UnconstrainedGrepCandidateIndex;
        assert_eq!(
            index.select_paths("needle", false),
            GrepCandidateSelection::Unconstrained
        );
    }

    #[cfg(feature = "grep-trigram")]
    mod trigram_tests {
        use super::super::*;
        use std::fs;
        use std::path::{Path, PathBuf};
        use tempfile::tempdir;

        fn write_tree(root: &Path, files: &[(&str, &str)]) -> Vec<PathBuf> {
            let mut abs = Vec::with_capacity(files.len());
            for (rel, body) in files {
                let path = root.join(rel);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(&path, body).unwrap();
                abs.push(path);
            }
            abs
        }

        #[test]
        fn literal_pattern_returns_only_files_that_can_match() {
            let dir = tempdir().unwrap();
            let root = dir.path();
            let files = write_tree(
                root,
                &[
                    ("src/a.rs", "fn alpha() {}\n"),
                    ("src/b.rs", "fn needle_token() {}\n"),
                    ("README.md", "no match here\n"),
                ],
            );
            let index_dir = root.join(".a3s-code/grep-trigram");
            let index =
                TrigramGrepCandidateIndex::build_from_files(root, &index_dir, &files).unwrap();

            match index.select_paths("needle_token", false) {
                GrepCandidateSelection::Paths(paths) => {
                    assert!(
                        paths.contains("src/b.rs"),
                        "expected hit file in candidates: {paths:?}"
                    );
                    assert!(
                        !paths.contains("src/a.rs"),
                        "unrelated file must be pruned: {paths:?}"
                    );
                    assert!(
                        !paths.contains("README.md"),
                        "unrelated file must be pruned: {paths:?}"
                    );
                }
                other => panic!("expected Paths, got {other:?}"),
            }
        }

        #[test]
        fn case_insensitive_literal_still_covers_hits() {
            let dir = tempdir().unwrap();
            let root = dir.path();
            let files = write_tree(
                root,
                &[
                    ("upper.rs", "fn NEEDLE_TOKEN() {}\n"),
                    ("other.rs", "fn alpha() {}\n"),
                ],
            );
            let index_dir = root.join(".a3s-code/grep-trigram");
            let index =
                TrigramGrepCandidateIndex::build_from_files(root, &index_dir, &files).unwrap();

            match index.select_paths("needle_token", true) {
                GrepCandidateSelection::Paths(paths) => {
                    assert!(paths.contains("upper.rs"), "{paths:?}");
                    assert!(!paths.contains("other.rs"), "{paths:?}");
                }
                other => panic!("expected Paths, got {other:?}"),
            }
        }

        #[test]
        fn match_all_pattern_falls_open_to_unconstrained() {
            let dir = tempdir().unwrap();
            let root = dir.path();
            let files = write_tree(root, &[("a.txt", "abc\n"), ("b.txt", "def\n")]);
            let index_dir = root.join(".a3s-code/grep-trigram");
            let index =
                TrigramGrepCandidateIndex::build_from_files(root, &index_dir, &files).unwrap();

            assert_eq!(
                index.select_paths(".", false),
                GrepCandidateSelection::Unconstrained
            );
        }

        #[test]
        fn indexed_candidate_set_covers_every_full_scan_hit() {
            let dir = tempdir().unwrap();
            let root = dir.path();
            let bodies = [
                ("one.rs", "use crate::alpha;\n"),
                ("two.rs", "let unique_marker_xyz = 1;\n"),
                ("three.rs", "unique_marker_xyz and more\n"),
                ("four.rs", "nothing interesting\n"),
            ];
            let files = write_tree(root, &bodies);
            let index_dir = root.join(".a3s-code/grep-trigram");
            let index =
                TrigramGrepCandidateIndex::build_from_files(root, &index_dir, &files).unwrap();

            let pattern = "unique_marker_xyz";
            let full_scan: BTreeSet<String> = bodies
                .iter()
                .filter(|(_, body)| body.contains(pattern))
                .map(|(path, _)| (*path).to_string())
                .collect();

            match index.select_paths(pattern, false) {
                GrepCandidateSelection::Paths(paths) => {
                    assert!(
                        full_scan.is_subset(&paths),
                        "trigram candidates must cover full-scan hits\nfull={full_scan:?}\ncandidates={paths:?}"
                    );
                }
                other => panic!("expected Paths for literal pattern, got {other:?}"),
            }
        }

        #[tokio::test]
        async fn manifest_grep_with_trigram_index_matches_unindexed_paths() {
            use crate::workspace::{
                ManifestWorkspaceBackend, WorkspaceGrepRequest, WorkspacePath, WorkspaceSearch,
            };
            use std::sync::Arc;
            use std::time::Duration;

            let dir = tempdir().unwrap();
            let root = dir.path();
            let files = write_tree(
                root,
                &[
                    ("keep.rs", "fn keep_me_unique_token() {}\n"),
                    ("skip.rs", "fn other() {}\n"),
                    ("notes.md", "docs without the token\n"),
                ],
            );

            let index_dir = root.join(".a3s-code/grep-trigram");
            let index = Arc::new(
                TrigramGrepCandidateIndex::build_from_files(root, &index_dir, &files).unwrap(),
            );

            let backend = ManifestWorkspaceBackend::new(root);
            backend
                .configure_grep_candidate_index(index)
                .expect("configure grep candidates");

            let mut rx = backend.manifest().subscribe();
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();

            let with_index = backend
                .grep_with_sources(WorkspaceGrepRequest {
                    base: WorkspacePath::root(),
                    pattern: "keep_me_unique_token".to_string(),
                    glob: None,
                    context_lines: 0,
                    case_insensitive: false,
                    max_output_size: 0,
                })
                .await
                .unwrap();
            assert_eq!(with_index.result.match_count, 1);
            assert_eq!(with_index.result.file_count, 1);
            assert_eq!(
                with_index.matched_paths.unwrap(),
                vec![WorkspacePath::from_normalized("keep.rs")]
            );

            // Short literal with enough trigrams should still find both Rust
            // files when the index is active.
            let both = backend
                .grep_with_sources(WorkspaceGrepRequest {
                    base: WorkspacePath::root(),
                    pattern: "fn ".to_string(),
                    glob: Some("*.rs".to_string()),
                    context_lines: 0,
                    case_insensitive: false,
                    max_output_size: 0,
                })
                .await
                .unwrap();
            assert_eq!(both.result.file_count, 2);
        }

        #[tokio::test]
        async fn manifest_grep_auto_attaches_trigram_without_configure() {
            use crate::workspace::{
                ManifestWorkspaceBackend, WorkspaceGrepRequest, WorkspacePath, WorkspaceSearch,
            };
            use std::time::Duration;

            let dir = tempdir().unwrap();
            let root = dir.path();
            write_tree(
                root,
                &[
                    ("keep.rs", "fn auto_unique_token_zz() {}\n"),
                    ("skip.rs", "fn other() {}\n"),
                ],
            );

            let backend = ManifestWorkspaceBackend::new(root);
            assert!(
                backend.grep_candidate_index().is_none(),
                "auto index must stay lazy until the first grep"
            );

            let mut rx = backend.manifest().subscribe();
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();

            let with_index = backend
                .grep_with_sources(WorkspaceGrepRequest {
                    base: WorkspacePath::root(),
                    pattern: "auto_unique_token_zz".to_string(),
                    glob: None,
                    context_lines: 0,
                    case_insensitive: false,
                    max_output_size: 0,
                })
                .await
                .unwrap();
            assert_eq!(with_index.result.match_count, 1);
            assert_eq!(
                with_index.matched_paths.unwrap(),
                vec![WorkspacePath::from_normalized("keep.rs")]
            );
            assert!(
                backend.grep_candidate_index().is_some(),
                "first grep must auto-attach the trigram candidate index"
            );
            assert!(
                root.join(".a3s-code/grep-trigram").exists(),
                "auto index must land under .a3s-code/grep-trigram"
            );
            // Grep still must not open durable zvec FTS.
            assert!(backend.persistent_index().is_none());
        }

        #[tokio::test]
        async fn auto_trigram_index_rebuilds_after_manifest_version_change() {
            use crate::workspace::{
                ManifestWorkspaceBackend, WorkspaceGrepRequest, WorkspacePath, WorkspaceSearch,
            };
            use std::time::Duration;

            let dir = tempdir().unwrap();
            let root = dir.path();
            write_tree(root, &[("old.rs", "fn old_only() {}\n")]);

            let backend = ManifestWorkspaceBackend::new(root);
            let mut rx = backend.manifest().subscribe();
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();

            let first = backend
                .grep_with_sources(WorkspaceGrepRequest {
                    base: WorkspacePath::root(),
                    pattern: "old_only".to_string(),
                    glob: None,
                    context_lines: 0,
                    case_insensitive: false,
                    max_output_size: 0,
                })
                .await
                .unwrap();
            assert_eq!(first.result.file_count, 1);

            fs::write(root.join("new.rs"), "fn brand_new_token_qq() {}\n").unwrap();
            // Wait for a newer manifest snapshot that includes the new file.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if backend
                    .manifest()
                    .snapshot()
                    .files
                    .iter()
                    .any(|file| file.path == "new.rs")
                {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    panic!("timed out waiting for manifest to observe new.rs");
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            let second = backend
                .grep_with_sources(WorkspaceGrepRequest {
                    base: WorkspacePath::root(),
                    pattern: "brand_new_token_qq".to_string(),
                    glob: None,
                    context_lines: 0,
                    case_insensitive: false,
                    max_output_size: 0,
                })
                .await
                .unwrap();
            assert_eq!(second.result.match_count, 1);
            assert_eq!(
                second.matched_paths.unwrap(),
                vec![WorkspacePath::from_normalized("new.rs")]
            );
        }
    }
}
