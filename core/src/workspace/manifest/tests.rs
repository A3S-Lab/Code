use super::watcher::normalize_file_changes;
use super::*;
use notify::{
    event::{ModifyKind, RenameMode},
    Event, EventKind,
};
use std::collections::HashSet;
use std::process::Command;

fn write(path: &Path, body: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

fn run_git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "git command failed: git -C {root:?} {args:?}"
    );
}

fn test_file(path: &str) -> LocalWorkspaceFile {
    LocalWorkspaceFile {
        path: path.to_string(),
        size: 0,
        modified_ms: None,
        language: None,
        status: LocalWorkspaceFileStatus::Unknown,
        binary: false,
        generated: false,
    }
}

#[test]
fn manifest_index_reduces_glob_candidates() {
    let files = vec![
        test_file("src/main.rs"),
        test_file("crates/demo/src/lib.rs"),
        test_file("README.md"),
        test_file("docs/README.md"),
    ];
    let index = ManifestIndex::build(&files);

    let exact = candidate_indices_for_glob(&index, &WorkspacePath::root(), "src/main.rs")
        .iter()
        .collect::<Vec<_>>();
    assert_eq!(exact, vec![0]);

    let basename = candidate_indices_for_glob(&index, &WorkspacePath::root(), "**/README.md")
        .iter()
        .collect::<Vec<_>>();
    assert_eq!(basename, vec![2, 3]);

    let extension = candidate_indices_for_glob(&index, &WorkspacePath::root(), "**/*.rs")
        .iter()
        .collect::<Vec<_>>();
    assert_eq!(extension, vec![0, 1]);
}

#[test]
fn recent_files_are_bounded_and_ranked_by_heat() {
    let mut recent = RecentFiles::default();
    for index in 0..(RECENT_FILE_LIMIT + 5) {
        recent.touch(format!("src/file_{index:03}.rs"), index as u64);
    }
    recent.touch("src/file_000.rs".to_string(), 10_000);
    recent.touch("src/file_000.rs".to_string(), 10_001);

    let entries = recent.entries(None, usize::MAX, 10_001);

    assert_eq!(entries.len(), RECENT_FILE_LIMIT);
    assert_eq!(entries[0].path, "src/file_000.rs");
    assert_eq!(entries[0].touch_count, 2);
}

#[tokio::test]
async fn manifest_search_matches_glob_and_grep() {
    let temp = tempfile::tempdir().unwrap();
    write(
        &temp.path().join("src/main.rs"),
        b"fn main() {\n    println!(\"hello\");\n}\n",
    );
    write(&temp.path().join("README.md"), b"hello from docs\n");

    let backend = ManifestWorkspaceBackend::new(temp.path());
    let mut rx = backend.manifest().subscribe();
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();

    let glob = backend
        .glob(WorkspaceGlobRequest {
            base: backend.normalize("src").unwrap(),
            pattern: "*.rs".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(glob.matches[0].as_str(), "src/main.rs");

    let grep = backend
        .grep(WorkspaceGrepRequest {
            base: WorkspacePath::root(),
            pattern: "hello".to_string(),
            glob: Some("*.rs".to_string()),
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

#[tokio::test]
async fn manifest_backend_read_write_touch_recent_files() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("src/main.rs"), b"fn main() {}\n");

    let backend = ManifestWorkspaceBackend::new(temp.path());
    let mut rx = backend.manifest().subscribe();
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();

    let path = backend.normalize("src/main.rs").unwrap();
    backend.read_text(&path).await.unwrap();
    assert_eq!(
        backend.manifest().recent_file_paths(4),
        vec!["src/main.rs".to_string()]
    );

    backend
        .write_text(&path, "fn main() { println!(\"hi\"); }\n")
        .await
        .unwrap();
    let entries = backend.manifest().recent_file_entries(4);
    assert_eq!(entries[0].path, "src/main.rs");
    assert_eq!(entries[0].touch_count, 2);
}

#[tokio::test]
async fn manifest_glob_prioritizes_recent_matching_files() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("src/a.rs"), b"pub fn a() {}\n");
    write(&temp.path().join("src/z.rs"), b"pub fn z() {}\n");

    let backend = ManifestWorkspaceBackend::new(temp.path());
    let mut rx = backend.manifest().subscribe();
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();

    backend.manifest().touch_file("src/z.rs");
    let glob = backend
        .glob(WorkspaceGlobRequest {
            base: WorkspacePath::root(),
            pattern: "**/*.rs".to_string(),
        })
        .await
        .unwrap();

    assert_eq!(glob.matches[0].as_str(), "src/z.rs");
    assert_eq!(glob.matches[1].as_str(), "src/a.rs");
}

#[tokio::test]
async fn manifest_grep_prioritizes_recent_matches_when_truncated() {
    let temp = tempfile::tempdir().unwrap();
    write(
        &temp.path().join("src/a.rs"),
        b"pub const HIT: &str = \"a\";\n",
    );
    write(
        &temp.path().join("src/z.rs"),
        b"pub const HIT: &str = \"z\";\n",
    );

    let backend = ManifestWorkspaceBackend::new(temp.path());
    let mut rx = backend.manifest().subscribe();
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();

    backend.manifest().touch_file("src/z.rs");
    let grep = backend
        .grep(WorkspaceGrepRequest {
            base: WorkspacePath::root(),
            pattern: "HIT".to_string(),
            glob: Some("**/*.rs".to_string()),
            context_lines: 0,
            case_insensitive: false,
            max_output_size: 1,
        })
        .await
        .unwrap();

    assert!(grep.truncated);
    assert!(grep.output.starts_with(">src/z.rs:"), "{}", grep.output);
}

#[tokio::test]
async fn manifest_refreshes_after_file_event() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("README.md"), b"# hello\n");
    let manifest = LocalWorkspaceManifest::start(temp.path());
    let mut rx = manifest.subscribe();
    let initial = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(initial.files.iter().any(|file| file.path == "README.md"));

    write(&temp.path().join("src/lib.rs"), b"pub fn lib() {}\n");
    let updated = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = rx.recv().await.unwrap();
            if snapshot.files.iter().any(|file| file.path == "src/lib.rs") {
                break snapshot;
            }
        }
    })
    .await
    .unwrap();

    assert!(updated.version > initial.version);
}

#[test]
fn file_change_batch_normalizes_and_merges_duplicate_events() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let created = root.join("src/new.rs");
    let changed = root.join("src/lib.rs");
    let ephemeral = root.join("src/ephemeral.rs");
    let events = vec![
        Event::new(EventKind::Create(notify::event::CreateKind::File)).add_path(created.clone()),
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(created.clone()),
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(changed.clone()),
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(changed),
        Event::new(EventKind::Create(notify::event::CreateKind::File)).add_path(ephemeral.clone()),
        Event::new(EventKind::Remove(notify::event::RemoveKind::File)).add_path(ephemeral),
    ];

    let changes = normalize_file_changes(&root, &events, &HashSet::new());

    assert_eq!(
        changes,
        vec![
            WorkspaceFileChange {
                path: WorkspacePath::from_normalized("src/new.rs"),
                kind: WorkspaceFileChangeKind::Created,
            },
            WorkspaceFileChange {
                path: WorkspacePath::from_normalized("src/lib.rs"),
                kind: WorkspaceFileChangeKind::Changed,
            },
        ]
    );
}

#[test]
fn file_change_batch_treats_create_for_known_file_as_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let existing = root.join("src/lib.rs");
    let events =
        vec![Event::new(EventKind::Create(notify::event::CreateKind::File)).add_path(existing)];
    let known_paths = HashSet::from(["src/lib.rs".to_string()]);

    assert_eq!(
        normalize_file_changes(&root, &events, &known_paths),
        vec![WorkspaceFileChange {
            path: WorkspacePath::from_normalized("src/lib.rs"),
            kind: WorkspaceFileChangeKind::Changed,
        }]
    );
}

#[test]
fn file_change_batch_reports_create_delete_and_rename_in_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let created = root.join("src/created.rs");
    let deleted = root.join("src/deleted.rs");
    let rename_from = root.join("src/old.rs");
    let rename_to = root.join("src/new.rs");
    let events = vec![
        Event::new(EventKind::Create(notify::event::CreateKind::File)).add_path(created),
        Event::new(EventKind::Remove(notify::event::RemoveKind::File)).add_path(deleted),
        Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(rename_from)
            .add_path(rename_to),
    ];

    let known_paths = HashSet::from(["src/new.rs".to_string()]);
    let changes = normalize_file_changes(&root, &events, &known_paths);

    assert_eq!(
        changes,
        vec![
            WorkspaceFileChange {
                path: WorkspacePath::from_normalized("src/created.rs"),
                kind: WorkspaceFileChangeKind::Created,
            },
            WorkspaceFileChange {
                path: WorkspacePath::from_normalized("src/deleted.rs"),
                kind: WorkspaceFileChangeKind::Deleted,
            },
            WorkspaceFileChange {
                path: WorkspacePath::from_normalized("src/old.rs"),
                kind: WorkspaceFileChangeKind::Deleted,
            },
            WorkspaceFileChange {
                path: WorkspacePath::from_normalized("src/new.rs"),
                kind: WorkspaceFileChangeKind::Created,
            },
        ]
    );
}

#[test]
fn file_change_batch_ignores_external_and_invalid_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let outside = root.parent().unwrap().join("outside.rs");
    let invalid = root.join("../escape.rs");
    let ignored = root.join("node_modules/package/index.js");
    let events = vec![
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(outside),
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(invalid),
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(ignored),
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(root.clone()),
    ];

    assert!(normalize_file_changes(&root, &events, &HashSet::new()).is_empty());
}

#[tokio::test]
async fn manifest_change_subscription_reports_same_size_content_changes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("src/lib.rs");
    write(&path, b"aaaa\n");
    let manifest = LocalWorkspaceManifest::start(temp.path());
    let mut snapshots = manifest.subscribe();
    let mut changes = manifest.subscribe_changes();
    tokio::time::timeout(Duration::from_secs(5), snapshots.recv())
        .await
        .unwrap()
        .unwrap();

    let received = tokio::time::timeout(Duration::from_secs(10), async {
        let contents = [b"bbbb\n".as_slice(), b"cccc\n".as_slice()];
        let mut attempt = 0;
        loop {
            assert!(attempt < 20, "no content change was observed");
            write(&path, contents[attempt % contents.len()]);
            attempt += 1;
            match tokio::time::timeout(Duration::from_millis(400), changes.recv()).await {
                Ok(Ok(change)) if change.path.as_str() == "src/lib.rs" => break change,
                Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {}
            }
        }
    })
    .await
    .unwrap();

    assert_eq!(received.kind, WorkspaceFileChangeKind::Changed);
}

#[tokio::test]
async fn manifest_search_falls_back_before_initial_scan() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("src/main.rs"), b"fn main() {}\n");
    let backend = ManifestWorkspaceBackend::new(temp.path());
    let glob = backend
        .glob(WorkspaceGlobRequest {
            base: backend.normalize("src").unwrap(),
            pattern: "*.rs".to_string(),
        })
        .await
        .unwrap();
    assert!(glob
        .matches
        .iter()
        .any(|path| path.as_str() == "src/main.rs"));
}

#[test]
fn scan_includes_files_inside_nested_git_workspaces() {
    if !git_available() {
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    run_git(temp.path(), &["init"]);
    write(&temp.path().join("README.md"), b"# root\n");

    let nested = temp.path().join("vendor/child");
    std::fs::create_dir_all(&nested).unwrap();
    run_git(&nested, &["init"]);
    write(&nested.join("src/lib.rs"), b"pub fn child() {}\n");

    let files = scan_workspace_files(temp.path());
    assert!(files.iter().any(|file| file.path == "README.md"));
    assert!(files
        .iter()
        .any(|file| file.path == "vendor/child/src/lib.rs"));
}
