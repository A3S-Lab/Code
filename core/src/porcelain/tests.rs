use super::*;

#[test]
fn a_failed_reread_is_not_an_empty_mutation() {
    let before = vec!["?? guest.txt".to_string()];
    let observed = combine_delta(Some(&before), None, Some("abc"), None, None);
    assert!(observed.incomplete);
    assert!(observed.paths.is_empty());
}

#[test]
fn a_quoted_porcelain_path_is_a_mutation() {
    let after = vec!["?? \"my file.txt\"".to_string()];
    let parsed = diff_porcelain(&[], &after);
    assert!(!parsed.incomplete);
    assert_eq!(parsed.paths, vec!["my file.txt".to_string()]);
    let octal = vec!["?? \"\\350\\257\\264\\346\\230\\216.md\"".to_string()];
    let decoded = diff_porcelain(&[], &octal);
    assert!(!decoded.incomplete);
    assert_eq!(decoded.paths, vec!["说明.md".to_string()]);
}

#[test]
fn an_undecodable_quoted_path_is_not_an_empty_mutation() {
    let after = vec!["?? \"unterminated".to_string()];
    let parsed = diff_porcelain(&[], &after);
    assert!(parsed.incomplete);
    assert!(parsed.paths.is_empty());
}

#[test]
fn a_renamed_quoted_pair_records_both_paths() {
    let after = vec!["R  \"old name.txt\" -> \"new name.txt\"".to_string()];
    let parsed = diff_porcelain(&[], &after);
    assert!(!parsed.incomplete);
    assert_eq!(
        parsed.paths,
        vec!["new name.txt".to_string(), "old name.txt".to_string()]
    );
}

#[test]
fn hidden_index_tags_are_paths_and_ordinary_tags_are_not() {
    let output = b"H tracked.txt\0S skipped.txt\0h assumed.txt\0s both.txt\0";
    let (paths, incomplete) = parse_hidden_index_paths(output);
    assert!(!incomplete);
    assert_eq!(
        paths,
        vec![
            "skipped.txt".to_string(),
            "assumed.txt".to_string(),
            "both.txt".to_string()
        ]
    );
}

#[test]
fn nul_status_keeps_a_path_with_a_space() {
    let output = b"?? my file.txt\0?? \xe8\xaf\xb4\xe6\x98\x8e.md\0";
    let (records, incomplete) = porcelain_records(output);
    assert!(!incomplete);
    let parsed = diff_porcelain(&[], &records);
    assert!(parsed.paths.iter().any(|path| path == "my file.txt"));
    assert!(parsed.paths.iter().any(|path| path == "说明.md"));
}

#[test]
fn porcelain_keeps_the_suffix_past_thirty_two_paths() {
    let after: Vec<String> = (0..33).map(|index| format!("?? f{index:02}.txt")).collect();
    let paths = changed_paths(&[], &after);
    assert_eq!(paths.len(), 33);
    assert!(paths.iter().any(|path| path == "f32.txt"));
}

#[test]
fn a_clean_revision_path_is_not_kept_when_git_was_not_invoked() {
    let mut paths = vec!["guest.txt".to_string(), "local.txt".to_string()];
    drop_foreign_revision(
        &mut paths,
        &["guest.txt".to_string()],
        &["local.txt".to_string()],
    );
    assert_eq!(paths, vec!["local.txt".to_string()]);
}

#[test]
fn a_revision_path_left_dirty_stays_on_the_command() {
    let mut paths = vec!["guest.txt".to_string()];
    drop_foreign_revision(
        &mut paths,
        &["guest.txt".to_string()],
        &["guest.txt".to_string()],
    );
    assert_eq!(paths, vec!["guest.txt".to_string()]);
}

#[test]
fn a_missed_status_with_a_known_head_is_not_an_empty_tree() {
    let before = snapshot_from_parts(None, Some("abc".to_string()), Vec::new());
    assert!(before.observes_git());
    assert!(before.stamps_incomplete);
}

#[test]
fn a_missed_status_outside_git_is_not_a_git_observation() {
    let before = snapshot_from_parts(None, None, Vec::new());
    assert!(!before.observes_git());
    assert!(!before.stamps_incomplete);
}

#[tokio::test]
async fn an_unchanged_git_worktree_is_a_complete_empty_delta() {
    let root = tempfile::tempdir().unwrap();
    git_repo(root.path());
    std::fs::write(root.path().join("README.md"), "base\n").unwrap();
    git_at(root.path(), &["add", "README.md"]);
    git_at(root.path(), &["commit", "-m", "base"]);

    let watch = Watch::start(root.path()).await;
    assert!(watch.snapshot.observes_git());
    assert!(!watch.snapshot.stamps_incomplete);
    assert!(watch.files.is_none());
    let paths = watch.finish(root.path()).await;
    assert!(
        paths.is_empty(),
        "an unchanged worktree was reported as a mutation: {paths:?}"
    );
}

#[tokio::test]
async fn a_foreign_commit_is_not_a_command_mutation_unless_this_command_invoked_git() {
    let root = tempfile::tempdir().unwrap();
    git_repo(root.path());
    std::fs::write(root.path().join("README.md"), "base\n").unwrap();
    git_at(root.path(), &["add", "README.md"]);
    git_at(root.path(), &["commit", "-m", "base"]);

    let watch = Watch::start(root.path()).await;
    std::fs::write(root.path().join("guest.txt"), "foreign\n").unwrap();
    git_at(root.path(), &["add", "guest.txt"]);
    git_at(root.path(), &["commit", "-m", "foreign"]);
    let foreign = watch.finish_with_revision(root.path(), false).await;
    assert!(
        !foreign.iter().any(|path| path == "guest.txt"),
        "parallel commit entered the command delta: {foreign:?}"
    );

    let watch = Watch::start(root.path()).await;
    std::fs::write(root.path().join("owned.txt"), "owned\n").unwrap();
    git_at(root.path(), &["add", "owned.txt"]);
    git_at(root.path(), &["commit", "-m", "owned"]);
    let owned = watch.finish_with_revision(root.path(), true).await;
    assert!(
        owned.iter().any(|path| path == "owned.txt"),
        "this command's commit was dropped: {owned:?}"
    );

    let watch = Watch::start(root.path()).await;
    std::fs::write(root.path().join("local.txt"), "local\n").unwrap();
    let local = watch.finish_with_revision(root.path(), false).await;
    assert!(
        local.iter().any(|path| path == "local.txt"),
        "an uncommitted write was dropped: {local:?}"
    );
}

fn git_repo(root: &Path) {
    git_at(root, &["init"]);
    git_at(root, &["config", "user.email", "test@example.com"]);
    git_at(root, &["config", "user.name", "test"]);
    git_at(root, &["config", "commit.gpgsign", "false"]);
}

fn git_at(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?} failed to spawn: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
