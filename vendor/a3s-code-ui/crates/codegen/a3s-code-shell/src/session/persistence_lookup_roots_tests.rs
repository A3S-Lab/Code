use super::{
    find_persisted_session_dir_by_id_in_root_result, find_persisted_session_dir_in_roots,
    session_lookup_roots_from,
};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn primary_is_first_and_legacy_follows_when_paths_differ() {
    let roots = session_lookup_roots_from(
        Path::new("/tmp/product/sessions"),
        Some(Path::new("/tmp/os")),
    );
    assert_eq!(
        roots,
        vec![
            PathBuf::from("/tmp/product/sessions"),
            PathBuf::from("/tmp/os/.grok/sessions"),
        ]
    );
}

#[test]
fn legacy_omitted_when_it_equals_primary() {
    let primary = PathBuf::from("/tmp/os/.grok/sessions");
    let roots = session_lookup_roots_from(&primary, Some(Path::new("/tmp/os")));
    assert_eq!(roots, vec![primary]);
}

#[test]
fn no_os_home_returns_only_primary() {
    let primary = Path::new("/tmp/product/sessions");
    let roots = session_lookup_roots_from(primary, None);
    assert_eq!(roots, vec![PathBuf::from(primary)]);
}

fn write_session(root: &Path, cwd_bucket: &str, session_id: &str) -> PathBuf {
    let dir = root.join(cwd_bucket).join(session_id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("summary.json"), b"{}").unwrap();
    dir
}

#[test]
fn find_persisted_session_dir_in_roots_legacy_hit_when_primary_misses() {
    let tmp = tempfile::TempDir::new().unwrap();
    let primary = tmp.path().join("product");
    let legacy = tmp.path().join("legacy");
    fs::create_dir_all(&primary).unwrap();
    let expected = write_session(&legacy, "cwd-a", "sess-legacy");

    let found = find_persisted_session_dir_in_roots("sess-legacy", &[primary, legacy])
        .unwrap()
        .expect("legacy session");
    assert_eq!(found, expected);
}

#[test]
fn find_persisted_session_dir_in_roots_primary_wins_when_both_have_the_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    let primary = tmp.path().join("product");
    let legacy = tmp.path().join("legacy");
    let expected = write_session(&primary, "cwd-a", "sess-both");
    write_session(&legacy, "cwd-b", "sess-both");

    let found = find_persisted_session_dir_in_roots("sess-both", &[primary, legacy])
        .unwrap()
        .expect("primary session");
    assert_eq!(found, expected);
}

#[test]
fn find_persisted_session_dir_in_roots_missing_root_is_empty() {
    let missing = PathBuf::from("/nonexistent/a3s-session-roots/sessions");
    let found = find_persisted_session_dir_by_id_in_root_result("any-id", &missing).unwrap();
    assert!(found.is_none());
}
