#![cfg(test)]

use super::*;
#[cfg(unix)]
use std::fs::{self, File, FileTimes};
use std::io;

#[cfg(unix)]
fn fixture_git(root: &Path, arguments: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[cfg(unix)]
fn fixture_repository(root: &Path) {
    fixture_git(root, &["init"]);
    fixture_git(root, &["config", "user.email", "observer@example.invalid"]);
    fixture_git(root, &["config", "user.name", "Observer Test"]);
    fs::write(root.join("outside.txt"), "outside base\n").unwrap();
    fixture_git(root, &["add", "outside.txt"]);
    fixture_git(root, &["commit", "-m", "baseline"]);
}

#[tokio::test]
#[cfg(unix)]
async fn a_nested_git_workspace_observes_only_its_exact_relative_bytes() {
    use a3s_effect::{ToolFileKind, ToolObservationCompleteness};
    let repository = tempfile::tempdir().unwrap();
    fixture_repository(repository.path());
    let workspace = repository.path().join("run spaced/workspace");
    fs::create_dir_all(&workspace).unwrap();
    let watch = Watch::start(&workspace).await;
    fs::write(repository.path().join("executions.counter"), "1\n").unwrap();
    fs::create_dir_all(repository.path().join("sessions")).unwrap();
    fs::write(repository.path().join("sessions/checkpoint"), "host state").unwrap();
    let expected = b"actual durable tool effect bytes\n";
    fs::write(workspace.join("effect.txt"), expected).unwrap();
    assert_eq!(fs::read(workspace.join("effect.txt")).unwrap(), expected);
    // Independent unscoped Git output reproduces the repo-relative bug.
    let raw = fixture_git(&workspace, &["status", "--porcelain", "-z", "-u"]);
    let raw = String::from_utf8(raw).unwrap();
    assert!(raw.contains("executions.counter"));
    assert!(raw.contains("sessions/checkpoint"));
    assert!(raw.contains("run spaced/workspace/effect.txt"));
    let observed = watch.finish_observed_with_revision(&workspace, false).await;
    assert!(!observed.incomplete);
    assert_eq!(observed.paths, vec!["effect.txt"]);
    let effects = observe_path_effects(&workspace, observed);
    assert_eq!(effects.completeness, ToolObservationCompleteness::Complete);
    assert!(effects.validate().is_ok());
    assert_eq!(effects.changes.len(), 1);
    assert_eq!(effects.changes[0].path, "effect.txt");
    assert_eq!(effects.changes[0].after.kind, ToolFileKind::File);
    // This independent known-byte oracle is the real-live fixture's digest.
    assert_eq!(
        effects.changes[0].after.content_digest.as_deref(),
        Some("sha256:fc9948b06be6945266030ae8f302a97504ae22b60e7be9af03952c2ff3097f76")
    );
    assert!(effects.changes[0].after.mode.is_some());
}

#[tokio::test]
#[cfg(unix)]
async fn harness_directory_components_do_not_hide_literal_unix_backslash_files() {
    use a3s_effect::{ToolFileKind, ToolObservationCompleteness};
    let names = [
        ".a3s\\effect.txt",
        ".a3s-code\\effect.txt",
        ".git\\effect.txt",
    ];
    let payload = b"literal backslash file bytes\n";
    let expected_digest = format!("sha256:{:x}", Sha256::digest(payload));
    for git_backed in [false, true] {
        let root = tempfile::tempdir().unwrap();
        if git_backed {
            fixture_repository(root.path());
        }
        let watch = Watch::start(root.path()).await;
        let run_watch = RunWatch::start(root.path()).await;
        for name in names {
            fs::write(root.path().join(name), payload).unwrap();
            assert!(!is_harness_path(name));
        }
        for metadata in [".a3s", ".a3s-code"] {
            fs::create_dir(root.path().join(metadata)).unwrap();
            fs::write(root.path().join(metadata).join("internal"), "harness state").unwrap();
            assert!(is_harness_path(&format!("{metadata}/internal")));
        }
        let observed = watch
            .finish_observed_with_revision(root.path(), false)
            .await;
        assert!(!observed.incomplete);
        let effects = observe_path_effects(root.path(), observed);
        assert_eq!(effects.completeness, ToolObservationCompleteness::Complete);
        assert!(effects.validate().is_ok());
        assert_eq!(effects.changes.len(), names.len());
        for name in names {
            let change = effects
                .changes
                .iter()
                .find(|change| change.path == name)
                .unwrap();
            assert_eq!(change.after.kind, ToolFileKind::File);
            assert_eq!(change.after.content_digest.as_ref(), Some(&expected_digest));
        }
        if !git_backed {
            let changes = run_watch.file_changes(root.path());
            assert!(!changes.incomplete);
            assert_eq!(changes.paths.len(), names.len());
            assert!(names
                .iter()
                .all(|name| changes.paths.iter().any(|path| path == name)));
        }
    }
}

#[tokio::test]
#[cfg(unix)]
async fn nested_hidden_index_identity_stays_in_workspace_and_detects_preserved_mtime() {
    let repository = tempfile::tempdir().unwrap();
    fixture_repository(repository.path());
    let workspace = repository.path().join("nested/workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("inside.txt"), b"first\n").unwrap();
    fixture_git(repository.path(), &["add", "nested/workspace/inside.txt"]);
    fixture_git(repository.path(), &["commit", "-m", "inside"]);
    fixture_git(
        repository.path(),
        &[
            "update-index",
            "--skip-worktree",
            "outside.txt",
            "nested/workspace/inside.txt",
        ],
    );
    let watch = Watch::start(&workspace).await;
    assert!(!watch.snapshot.stamps_incomplete);
    assert_eq!(watch.snapshot.stamps.len(), 1);
    assert_eq!(watch.snapshot.stamps[0].path, "inside.txt");
    overwrite_preserving_mtime(&workspace.join("inside.txt"), b"other\n");
    fs::write(repository.path().join("outside.txt"), "outside changed\n").unwrap();
    assert!(lines(&workspace).await.unwrap().is_empty());
    let observed = watch.finish_observed_with_revision(&workspace, false).await;
    assert!(!observed.incomplete);
    assert_eq!(observed.paths, vec!["inside.txt"]);
    let effects = observe_path_effects(&workspace, observed);
    assert!(effects.validate().is_ok());
    assert_eq!(
        effects.changes[0].after.content_digest,
        Some(format!(
            "sha256:{:x}",
            Sha256::digest(fs::read(workspace.join("inside.txt")).unwrap())
        ))
    );
}

#[tokio::test]
#[cfg(unix)]
async fn nested_revision_deltas_exclude_outside_commits_and_keep_inside_relative_paths() {
    let repository = tempfile::tempdir().unwrap();
    fixture_repository(repository.path());
    let workspace = repository.path().join("nested/workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("inside.txt"), "base\n").unwrap();
    fixture_git(repository.path(), &["add", "nested/workspace/inside.txt"]);
    fixture_git(repository.path(), &["commit", "-m", "inside"]);
    let watch = Watch::start(&workspace).await;
    fs::write(repository.path().join("outside.txt"), "foreign commit\n").unwrap();
    fixture_git(repository.path(), &["add", "outside.txt"]);
    fixture_git(repository.path(), &["commit", "-m", "outside"]);
    let observed = watch.finish_observed_with_revision(&workspace, true).await;
    assert!(!observed.incomplete);
    assert!(observed.paths.is_empty());
    let watch = Watch::start(&workspace).await;
    fs::write(workspace.join("inside.txt"), "local commit\n").unwrap();
    fixture_git(repository.path(), &["add", "nested/workspace/inside.txt"]);
    fixture_git(repository.path(), &["commit", "-m", "local"]);
    let observed = watch.finish_observed_with_revision(&workspace, true).await;
    assert!(!observed.incomplete);
    assert_eq!(observed.paths, vec!["inside.txt"]);
}

#[test]
#[cfg(unix)]
fn typed_path_observation_keeps_missing_unknown_and_contract_bounds_distinct() {
    use a3s_effect::{ToolFileKind, ToolObservationCompleteness};
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("not-a-directory"), "data").unwrap();
    let missing = observe_path_effects(
        root.path(),
        PathDelta {
            paths: vec!["absent".into()],
            incomplete: false,
        },
    );
    assert!(missing.validate().is_ok());
    assert_eq!(missing.completeness, ToolObservationCompleteness::Complete);
    assert_eq!(missing.changes[0].after.kind, ToolFileKind::Missing);
    let unknown = observe_path_effects(
        root.path(),
        PathDelta {
            paths: vec![
                "not-a-directory/child".into(),
                "../escape".into(),
                "../.a3s/outside".into(),
                "/.a3s/outside".into(),
            ],
            incomplete: false,
        },
    );
    assert!(unknown.validate().is_ok());
    assert_eq!(
        unknown.completeness,
        ToolObservationCompleteness::Incomplete
    );
    assert_eq!(unknown.changes.len(), 1);
    assert_eq!(unknown.changes[0].path, "not-a-directory/child");
    assert!(unknown
        .changes
        .iter()
        .all(|change| change.after.kind == ToolFileKind::Unknown));
    let escaped_metadata = observe_path_effects(
        root.path(),
        PathDelta {
            paths: vec![
                "../.a3s/outside".into(),
                "/.a3s/outside".into(),
                "nul\0path".into(),
            ],
            incomplete: false,
        },
    );
    assert!(escaped_metadata.validate().is_ok());
    assert_eq!(
        escaped_metadata.completeness,
        ToolObservationCompleteness::Incomplete
    );
    assert!(escaped_metadata.changes.is_empty());
    let invalid = observe_path_effects(
        root.path(),
        PathDelta {
            paths: vec!["x".repeat(OBSERVATION_PATH_BYTE_CAP + 1)],
            incomplete: false,
        },
    );
    assert!(invalid.validate().is_ok());
    assert_eq!(
        invalid.completeness,
        ToolObservationCompleteness::Incomplete
    );
    assert!(invalid.changes.is_empty());
    let too_many = observe_path_effects(
        root.path(),
        PathDelta {
            paths: (0..=FILE_WALK_CAP)
                .map(|index| format!("absent-{index}"))
                .collect(),
            incomplete: false,
        },
    );
    assert!(too_many.validate().is_ok());
    assert_eq!(
        too_many.completeness,
        ToolObservationCompleteness::Incomplete
    );
    assert_eq!(too_many.changes.len(), FILE_WALK_CAP);
}

#[tokio::test]
async fn git_output_stops_at_its_byte_bound_and_never_returns_a_prefix_as_success() {
    let exact = vec![b'x'; GIT_OUTPUT_BYTE_CAP as usize];
    assert_eq!(
        read_git_output(exact.as_slice()).await.unwrap().len(),
        exact.len()
    );
    assert!(tokio::time::timeout(
        Duration::from_secs(1),
        read_git_output(tokio::io::repeat(b'x')),
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test]
async fn git_output_read_failure_cannot_become_an_empty_success() {
    struct Fails;
    impl tokio::io::AsyncRead for Fails {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
            _buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Err(io::Error::other("injected git read failure")))
        }
    }
    assert!(read_git_output(Fails).await.is_none());
}

#[test]
fn git_parsers_bound_paths_and_do_not_accept_overflow_as_complete() {
    let status = (0..=FILE_WALK_CAP)
        .map(|index| format!("?? f{index:04}\0"))
        .collect::<String>();
    let (records, incomplete) = porcelain_records(status.as_bytes());
    assert!(incomplete);
    assert_eq!(records.len(), FILE_WALK_CAP);
    let hidden = (0..=FILE_WALK_CAP)
        .map(|index| format!("S f{index:04}\0"))
        .collect::<String>();
    let (paths, incomplete) = parse_hidden_index_paths(hidden.as_bytes());
    assert!(incomplete);
    assert_eq!(paths.len(), FILE_WALK_CAP);
    let oversized = "x".repeat(OBSERVATION_PATH_BYTE_CAP + 1);
    let (records, incomplete) = porcelain_records(format!("?? {oversized}\0").as_bytes());
    assert!(incomplete);
    assert!(records.is_empty());
    let (paths, incomplete) = parse_hidden_index_paths(format!("S {oversized}\0").as_bytes());
    assert!(incomplete);
    assert!(paths.is_empty());
}

#[test]
fn a_rename_cannot_cross_the_path_bound_and_malformed_status_cannot_panic() {
    let mut status = (0..FILE_WALK_CAP - 1)
        .map(|index| format!("?? f{index:04}\0"))
        .collect::<String>();
    status.push_str("R  new\0old\0");
    let (records, incomplete) = porcelain_records(status.as_bytes());
    assert!(incomplete);
    assert_eq!(records.len(), FILE_WALK_CAP - 1);
    let (records, incomplete) = porcelain_records("说 x\0".as_bytes());
    assert!(incomplete);
    assert!(records.is_empty());
    assert!(paths_of("说 x").is_none());
}

#[tokio::test]
#[cfg(unix)]
async fn real_git_nul_names_are_exact_before_hashing_even_with_quotes_spaces_and_arrows() {
    let root = tempfile::tempdir().unwrap();
    let initialized = std::process::Command::new("git")
        .arg("init")
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(initialized.status.success());
    let names = [
        "  padded  ",
        "\"literal\"",
        "箭头 -> 文件",
        "tab\tname",
        "back\\slash",
    ];
    for name in names {
        fs::write(root.path().join(name), b"data").unwrap();
    }
    let records = lines(root.path()).await.unwrap();
    let paths = porcelain_paths(&records).unwrap();
    assert_eq!(paths.len(), names.len());
    for name in names {
        assert!(paths.iter().any(|path| path == name), "{name:?}");
    }
    let mut byte_budget = OBSERVATION_BYTE_CAP;
    let mut path_budget = FILE_WALK_CAP;
    let (stamps, incomplete) =
        stamp_lines(root.path(), &records, &mut byte_budget, &mut path_budget);
    assert!(!incomplete);
    assert!(stamps
        .iter()
        .all(|stamp| stamp.present && stamp.content_digest.is_some()));
    assert_eq!(byte_budget, OBSERVATION_BYTE_CAP - (4 * names.len()) as u64);
}

#[test]
fn nul_rename_fields_and_legacy_unicode_renames_keep_exact_paths_without_panicking() {
    let (records, incomplete) = porcelain_records("R  新 -> 名\0\"老\"\0".as_bytes());
    assert!(!incomplete);
    let paths = porcelain_paths(&records).unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths.iter().any(|path| path == "新 -> 名"));
    assert!(paths.iter().any(|path| path == "\"老\""));
    assert_eq!(paths_of("R  老 -> 新").unwrap(), vec!["老", "新"]);
    let (hidden, incomplete) = parse_hidden_index_paths(b"S   padded  \0");
    assert!(!incomplete);
    assert_eq!(hidden, vec!["  padded  "]);
}

#[cfg(unix)]
fn overwrite_preserving_mtime(path: &Path, bytes: &[u8]) {
    let modified = fs::metadata(path).unwrap().modified().unwrap();
    fs::write(path, bytes).unwrap();
    File::open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
}

#[test]
#[cfg(unix)]
fn same_length_and_preserved_mtime_still_change_the_content_identity() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.rs");
    fs::write(&path, b"before\n").unwrap();
    let mut budget = OBSERVATION_BYTE_CAP;
    let before = observe_file_identity(root.path(), "source.rs", &mut budget).unwrap();
    overwrite_preserving_mtime(&path, b"after!\n");
    let after = observe_file_identity(root.path(), "source.rs", &mut budget).unwrap();
    assert_eq!(before.len, after.len);
    assert_eq!(before.modified, after.modified);
    assert_eq!(before.mode, after.mode);
    assert_eq!(before.kind, after.kind);
    assert_ne!(before.content_digest, after.content_digest);
    assert_eq!(content_changes(&[before], &[after]), vec!["source.rs"]);
}

#[test]
#[cfg(unix)]
fn stable_porcelain_lines_do_not_hide_preserved_metadata_writes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("dirty.txt");
    fs::write(&path, "first\n").unwrap();
    let lines = vec![" M dirty.txt".to_string()];
    let mut byte_budget = OBSERVATION_BYTE_CAP;
    let mut path_budget = FILE_WALK_CAP;
    let (before, incomplete) = stamp_lines(root.path(), &lines, &mut byte_budget, &mut path_budget);
    assert!(!incomplete);
    overwrite_preserving_mtime(&path, b"other\n");
    let (after, incomplete) = stamp_lines(root.path(), &lines, &mut byte_budget, &mut path_budget);
    assert!(!incomplete);
    assert_eq!(before[0].modified, after[0].modified);
    assert_eq!(content_changes(&before, &after), vec!["dirty.txt"]);
}

#[test]
#[cfg(unix)]
fn non_git_delta_sees_preserved_mtime_writes_and_keeps_all_forty_paths() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..40 {
        fs::write(root.path().join(format!("f{index:02}.txt")), b"first\n").unwrap();
    }
    let before = index_files(root.path()).with_content(root.path());
    assert!(!before.incomplete);
    for index in 0..40 {
        overwrite_preserving_mtime(&root.path().join(format!("f{index:02}.txt")), b"other\n");
    }
    let observed = file_delta(root.path(), &before);
    assert!(!observed.incomplete);
    assert_eq!(observed.paths.len(), 40);
    assert!(observed.paths.contains(&"f39.txt".to_string()));
    let watch = Watch {
        snapshot: snapshot_from_parts(None, None, Vec::new()),
        files: Some(before),
    };
    let baseline = watch.receipt_baseline();
    assert_eq!(baseline["nongit"]["entries"].as_array().unwrap().len(), 40);
    assert_eq!(baseline["nongit"]["incomplete"], false);
    assert!(baseline["nongit"]["entries"][39]["content_digest"].is_string());
    let effects = observe_path_effects(root.path(), observed.clone());
    assert_eq!(
        effects.completeness,
        a3s_effect::ToolObservationCompleteness::Complete
    );
    assert_eq!(
        effects.changes[39].after.content_digest,
        Some(format!("sha256:{:x}", Sha256::digest(b"other\n")))
    );
    let mut shown = None;
    attach(&mut shown, &observed.paths);
    assert_eq!(
        shown.as_ref().unwrap()["changed_paths"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    assert_eq!(shown.unwrap()["changed_paths_truncated"], true);
}

#[test]
#[cfg(unix)]
fn path_budget_exhaustion_is_explicit_and_cannot_become_an_empty_success() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..=16 {
        File::create(root.path().join(format!("f{index:04}"))).unwrap();
    }
    let before = index_files_within(root.path(), 16);
    assert!(before.incomplete);
    assert_eq!(before.entries.len(), 16);
    assert!(file_delta(root.path(), &before).incomplete);
    let watch = Watch {
        snapshot: snapshot_from_parts(None, None, Vec::new()),
        files: Some(before),
    };
    assert_eq!(watch.receipt_baseline()["nongit"]["incomplete"], true);
}

#[test]
#[cfg(unix)]
fn an_oversized_sparse_file_is_refused_before_reading_any_bytes() {
    let root = tempfile::tempdir().unwrap();
    File::create(root.path().join("oversized.bin"))
        .unwrap()
        .set_len(OBSERVATION_BYTE_CAP + 1)
        .unwrap();
    let mut budget = OBSERVATION_BYTE_CAP;
    assert!(observe_file_identity(root.path(), "oversized.bin", &mut budget).is_none());
    assert_eq!(budget, OBSERVATION_BYTE_CAP);
    // The walk only stats, so an unchanged large file never blocks a receipt.
    assert!(!index_files(root.path()).incomplete);
}

#[test]
#[cfg(unix)]
fn total_byte_budget_is_shared_and_overflow_marks_the_changed_paths_incomplete() {
    let root = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..5).map(|index| format!("part-{index}.bin")).collect();
    for name in &names {
        std::fs::write(root.path().join(name), b"0123").unwrap();
    }
    let index = index_files(root.path());
    assert!(!index.incomplete);
    assert_eq!(index.entries.len(), 5);
    let effects = observe_path_effects_within(
        root.path(),
        PathDelta {
            paths: names,
            incomplete: false,
        },
        16,
    );
    assert_eq!(
        effects.completeness,
        a3s_effect::ToolObservationCompleteness::Incomplete
    );
    assert_eq!(
        effects
            .changes
            .iter()
            .filter(|change| change.after.content_digest.is_some())
            .count(),
        4
    );
}

#[test]
#[cfg(unix)]
fn a_large_non_git_tree_is_a_complete_index_and_a_bounded_receipt() {
    let root = tempfile::tempdir().unwrap();
    let deps = root.path().join("node_modules");
    fs::create_dir(&deps).unwrap();
    for index in 0..FILE_WALK_CAP + 100 {
        fs::write(deps.join(format!("m{index}.js")), b"x\n").unwrap();
    }
    let before = index_files(root.path()).with_content(root.path());
    assert!(!before.incomplete);
    assert_eq!(before.entries.len(), FILE_WALK_CAP + 101);
    fs::write(root.path().join("answer.txt"), b"42\n").unwrap();
    let observed = file_delta(root.path(), &before);
    assert!(!observed.incomplete);
    assert_eq!(observed.paths, ["answer.txt"]);
    let watch = Watch {
        snapshot: snapshot_from_parts(None, None, Vec::new()),
        files: Some(before),
    };
    let baseline = watch.receipt_baseline();
    assert_eq!(baseline["nongit"]["content_omitted"], true);
    assert_eq!(baseline["nongit"]["entry_count"], FILE_WALK_CAP + 101);
    assert!(serde_json::to_vec(&baseline).unwrap().len() < 1024);
}

#[test]
#[cfg(unix)]
fn a_caller_budget_is_remaining_bytes_not_a_fresh_per_file_allowance() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("one"), b"1234").unwrap();
    fs::write(root.path().join("two"), b"5678").unwrap();
    let mut budget = 7;
    assert!(observe_file_identity(root.path(), "one", &mut budget).is_some());
    assert_eq!(budget, 3);
    assert!(observe_file_identity(root.path(), "two", &mut budget).is_none());
    assert_eq!(budget, 3);
}

#[test]
fn failed_reads_keep_spent_bytes_and_never_produce_a_digest() {
    struct FailsAfterOneByte(bool);
    impl Read for FailsAfterOneByte {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                return Err(io::Error::other("injected read failure"));
            }
            self.0 = true;
            bytes[0] = b'x';
            Ok(1)
        }
    }
    let mut budget = 8;
    assert!(read_content_digest(&mut FailsAfterOneByte(false), 2, &mut budget).is_none());
    assert_eq!(budget, 7);
    assert!(read_content_digest(&mut io::Cursor::new(b"short"), 6, &mut budget).is_none());
    assert_eq!(budget, 2);
}

#[test]
#[cfg(unix)]
fn failed_directory_and_stat_reads_are_explicitly_incomplete() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("not-a-directory");
    fs::write(&file, "data").unwrap();
    assert!(index_files(&file).incomplete);
    assert!(index_files(&root.path().join("missing-root")).incomplete);
    let mut budget = OBSERVATION_BYTE_CAP;
    assert!(observe_file_identity(root.path(), "not-a-directory/child", &mut budget).is_none());
    let mut path_budget = FILE_WALK_CAP;
    let (stamps, incomplete) = stamp_paths(
        root.path(),
        &["not-a-directory/child".to_string()],
        &mut budget,
        &mut path_budget,
    );
    assert!(incomplete);
    assert!(stamps.is_empty());
}

#[test]
#[cfg(unix)]
fn unreadable_subdirectories_are_not_silently_dropped() {
    use std::os::unix::fs::PermissionsExt;
    // Root can read mode-000 directories, so it cannot exercise this failure.
    // SAFETY: geteuid has no arguments or memory-safety preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let closed = root.path().join("closed");
    fs::create_dir(&closed).unwrap();
    fs::write(closed.join("hidden"), "content").unwrap();
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).unwrap();
    let observed = index_files(root.path());
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(observed.incomplete);
}

#[test]
#[cfg(unix)]
fn directory_eof_resets_stale_errno_and_keeps_the_maximum_legal_name() {
    let root = tempfile::tempdir().unwrap();
    let longest = "x".repeat(255);
    fs::write(root.path().join(&longest), "data").unwrap();
    let errno = directory_errno().unwrap();
    assert!(!errno.is_null());
    // SAFETY: directory_errno returned this thread's live errno cell.
    unsafe {
        *errno = libc::EIO;
    }
    let observed = index_files(root.path());
    assert!(!observed.incomplete);
    assert_eq!(observed.entries.len(), 1);
    assert_eq!(observed.entries[0].path, longest);
}

#[test]
#[cfg(target_os = "linux")]
fn an_invalid_utf8_directory_entry_cannot_be_silently_dropped() {
    use std::os::unix::ffi::OsStringExt;
    // macOS filesystems reject this filename at creation; Linux can exercise
    // the observer's actual non-UTF8 entry failure instead of skipping it.
    let root = tempfile::tempdir().unwrap();
    let invalid = std::ffi::OsString::from_vec(b"invalid-\xff".to_vec());
    fs::write(root.path().join(invalid), "hidden").unwrap();
    assert!(index_files(root.path()).incomplete);
}

#[test]
#[cfg(unix)]
fn symlink_identity_hashes_only_its_target_spelling_and_never_external_contents() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret");
    fs::write(&secret, "external bytes must not be read").unwrap();
    symlink(&secret, root.path().join("link")).unwrap();
    let mut budget = OBSERVATION_BYTE_CAP;
    let first = observe_file_identity(root.path(), "link", &mut budget).unwrap();
    let target = secret.as_os_str().as_bytes();
    assert_eq!(
        first.content_digest,
        Some(format!("{:x}", Sha256::digest(target)))
    );
    assert_eq!(budget, OBSERVATION_BYTE_CAP - target.len() as u64);
    fs::write(&secret, "completely different external contents").unwrap();
    let second = observe_file_identity(root.path(), "link", &mut budget).unwrap();
    assert_eq!(first, second);
    assert!(!index_files(root.path()).incomplete);
    symlink(outside.path(), root.path().join("escape")).unwrap();
    let remaining = budget;
    assert!(observe_file_identity(root.path(), "escape/secret", &mut budget).is_none());
    assert_eq!(budget, remaining);
}

#[test]
#[cfg(unix)]
fn unsupported_nodes_are_refused_before_a_fifo_open_can_block() {
    use std::os::unix::ffi::OsStrExt;
    let root = tempfile::tempdir().unwrap();
    let path = std::ffi::CString::new(root.path().join("pipe").as_os_str().as_bytes()).unwrap();
    // SAFETY: path is NUL-terminated and points to a new temporary path.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let directory = root.path().to_path_buf();
    let (send, receive) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let mut budget = OBSERVATION_BYTE_CAP;
        send.send(observe_file_identity(&directory, "pipe", &mut budget))
            .unwrap();
    });
    assert!(receive
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .is_none());
    thread.join().unwrap();
    assert!(index_files(root.path()).incomplete);
}

#[test]
#[cfg(unix)]
fn absolute_parent_and_empty_paths_are_refused_while_absence_is_a_typed_identity() {
    let root = tempfile::tempdir().unwrap();
    let mut budget = OBSERVATION_BYTE_CAP;
    for path in ["", ".", "../escape", "/absolute", "safe/../../escape"] {
        assert!(
            observe_file_identity(root.path(), path, &mut budget).is_none(),
            "{path}"
        );
    }
    let absent = observe_file_identity(root.path(), "missing/child", &mut budget).unwrap();
    assert!(!absent.present);
    assert!(absent.content_digest.is_none());
    assert_eq!(budget, OBSERVATION_BYTE_CAP);
    fs::write(root.path().join("empty"), []).unwrap();
    let empty = observe_file_identity(root.path(), "empty", &mut 0).unwrap();
    assert_eq!(
        empty.content_digest,
        Some(format!("{:x}", Sha256::digest([])))
    );
}

#[test]
fn old_stat_only_baselines_remain_readable_without_fabricating_a_hash() {
    let stamp: ContentStamp = serde_json::from_value(serde_json::json!({
        "path": "legacy", "present": true, "len": 4, "modified": 1,
        "mode": 0, "kind": 0,
    }))
    .unwrap();
    assert!(stamp.content_digest.is_none());
}

#[test]
#[cfg(unix)]
fn oversized_relative_paths_are_refused_before_component_allocation_or_io() {
    let root = tempfile::tempdir().unwrap();
    let path = "a/".repeat(OBSERVATION_PATH_BYTE_CAP);
    let mut budget = OBSERVATION_BYTE_CAP;
    assert!(observe_file_identity(root.path(), &path, &mut budget).is_none());
    assert_eq!(budget, OBSERVATION_BYTE_CAP);
    let mut path_budget = FILE_WALK_CAP;
    let (stamps, incomplete) = stamp_paths(root.path(), &[path], &mut budget, &mut path_budget);
    assert!(incomplete);
    assert!(stamps.is_empty());
}

#[test]
#[cfg(unix)]
fn a_changed_data_file_larger_than_a_baseline_budget_has_byte_identity() {
    let root = tempfile::tempdir().unwrap();
    let bytes = vec![b'x'; 20 * 1024 * 1024];
    std::fs::write(root.path().join("hour_00.json"), &bytes).unwrap();
    let effects = observe_path_effects(
        root.path(),
        PathDelta {
            paths: vec!["hour_00.json".into()],
            incomplete: false,
        },
    );
    assert_eq!(
        effects.completeness,
        a3s_effect::ToolObservationCompleteness::Complete
    );
    assert_eq!(
        effects.changes[0].after.content_digest.as_deref(),
        Some(format!("sha256:{:x}", Sha256::digest(&bytes)).as_str())
    );
}
