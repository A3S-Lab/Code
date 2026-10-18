#![cfg(test)]

use super::super::WorkspaceServices;
use super::*;

struct Buffers;

#[tokio::test]
async fn sdk_snapshot_normalizes_paths_and_preserves_backend_fallback() {
    let snapshot = WorkspaceReadOverlaySnapshot::new(HashMap::from([(
        "./dir/open.txt".into(),
        "unsaved\n".into(),
    )]))
    .unwrap();
    assert_eq!(
        snapshot
            .read_text(&WorkspacePath::from_normalized("dir/open.txt"))
            .await
            .as_deref(),
        Some("unsaved\n")
    );
    assert!(snapshot
        .read_text(&WorkspacePath::from_normalized("other.txt"))
        .await
        .is_none());
}

#[test]
fn sdk_snapshot_refuses_escaping_duplicate_root_and_oversized_files() {
    for path in ["../outside.txt", "/outside.txt", "C:\\outside.txt", "."] {
        assert!(
            WorkspaceReadOverlaySnapshot::new(HashMap::from([(path.into(), "x".into())])).is_err(),
            "{path}"
        );
    }
    assert!(WorkspaceReadOverlaySnapshot::new(HashMap::from([
        ("a.txt".into(), "a".into()),
        ("./a.txt".into(), "b".into()),
    ]))
    .is_err());
    assert!(WorkspaceReadOverlaySnapshot::new(HashMap::from([(
        "large.txt".into(),
        "x".repeat(super::super::local::MAX_TEXT_FILE_BYTES as usize + 1)
    ),]))
    .is_err());
}

#[async_trait]
impl WorkspaceReadOverlay for Buffers {
    async fn read_text(&self, path: &WorkspacePath) -> Option<String> {
        (path.as_str() == "open.txt").then(|| "unsaved one\nunsaved two\nunsaved three\n".into())
    }
}

#[tokio::test]
async fn overlay_reads_win_and_other_operations_use_the_backend() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("open.txt"), "saved\n").unwrap();
    std::fs::write(temp.path().join("closed.txt"), "closed on disk\n").unwrap();
    let services = WorkspaceServices::local(temp.path()).with_read_overlay(Arc::new(Buffers));
    let open = services.normalize_path("open.txt").unwrap();
    let closed = services.normalize_path("closed.txt").unwrap();

    assert_eq!(
        services.fs().read_text(&open).await.unwrap(),
        "unsaved one\nunsaved two\nunsaved three\n"
    );
    assert_eq!(
        services.fs().read_text(&closed).await.unwrap(),
        "closed on disk\n"
    );
    let (edit_view, _) = services.read_for_edit(&open).await.unwrap();
    assert!(edit_view.starts_with("unsaved one"));

    let reader = services.text_reader().unwrap();
    let range = reader.read_text_range(&open, 1, 1).await.unwrap();
    assert_eq!(range.lines, vec!["unsaved two".to_string()]);
    assert_eq!(range.next_offset, Some(2));
    assert_eq!(range.total_lines, Some(3));
    let range = reader.read_text_range(&closed, 0, 10).await.unwrap();
    assert_eq!(range.lines, vec!["closed on disk".to_string()]);

    services.fs().write_text(&open, "written\n").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(temp.path().join("open.txt")).unwrap(),
        "written\n"
    );
    let names = services
        .fs()
        .list_dir(&WorkspacePath::root())
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
    assert!(names.contains(&"closed.txt".to_string()));
}

#[test]
fn text_range_from_text_matches_streaming_semantics() {
    let range = WorkspaceTextRange::from_text("a\nb\nc", 3, 5);
    assert!(range.lines.is_empty());
    assert!(range.eof);
    assert_eq!(range.next_offset, None);
    let range = WorkspaceTextRange::from_text("a\nb\nc", 0, 2);
    assert_eq!(range.lines, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(range.next_offset, Some(2));
    assert!(!range.eof);
}
