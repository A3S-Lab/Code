#![cfg(test)]

use super::*;
use a3s_effect::{FileLog, LogStore, NewFact};
use serde_json::json;

fn workspace() -> tempfile::TempDir {
    tempfile::tempdir().expect("workspace")
}

fn message(text: &str) -> NewFact {
    NewFact {
        kind: "user.message".into(),
        key: "user:1".into(),
        payload: json!({"text": text}),
    }
}

fn folder(workspace: &Path, id: &str, day: u32, text: &str) -> PathBuf {
    let home = super::super::ensure_session_dir_on(
        workspace,
        id,
        NaiveDate::from_ymd_opt(2026, 10, day).expect("date"),
    )
    .expect("session folder");
    FileLog::open(&home)
        .expect("fixture log")
        .append(
            &crate::fact_control::thread_for_session(id),
            &[message(text)],
            None,
        )
        .expect("append");
    home
}

fn legacy(workspace: &Path, id: &str, text: &str) -> PathBuf {
    let home = super::super::legacy_effect_log_dir(workspace);
    FileLog::open(&home)
        .expect("legacy fixture log")
        .append(id, &[message(text)], None)
        .expect("append");
    home
}

#[test]
fn empty_workspace_is_read_only_and_complete() {
    let root = workspace();
    let page = list_workspace_sessions(root.path(), 30, None).expect("list");
    assert!(page.sessions.is_empty());
    assert!(!page.incomplete);
    assert!(!page.truncated);
    assert!(!root.path().join(".a3s").exists());
}

#[test]
fn dated_and_legacy_facts_share_a_catalog_without_a_snapshot_store() {
    let root = workspace();
    folder(root.path(), "new-id", 11, "新的问题\n第二行");
    legacy(root.path(), "old-id", "legacy prompt");
    let page = list_workspace_sessions(root.path(), 30, None).expect("list");
    assert_eq!(page.sessions.len(), 2);
    assert!(!page.incomplete);
    let new = page
        .sessions
        .iter()
        .find(|e| e.id == "new-id")
        .expect("new");
    assert_eq!(new.first_prompt.as_deref(), Some("新的问题\n第二行"));
    assert_eq!(new.summary, "新的问题");
    assert_eq!(new.num_messages, 1);
    assert_eq!(new.cwd, root.path().canonicalize().expect("canonical"));
    assert_eq!(new.fact_count, 1);
}

#[test]
fn exact_original_unicode_id_comes_from_marker() {
    let root = workspace();
    folder(root.path(), "我的会话/🔬", 11, "原文");
    let page = list_workspace_sessions(root.path(), 30, Some("🔬")).expect("list");
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].id, "我的会话/🔬");
}

#[test]
fn folder_has_runtime_precedence_over_the_legacy_log() {
    let root = workspace();
    folder(root.path(), "same", 11, "folder authority");
    legacy(root.path(), "same", "legacy must not win");
    let page = list_workspace_sessions(root.path(), 30, None).expect("list");
    assert_eq!(page.sessions.len(), 1);
    assert_eq!(page.sessions[0].summary, "folder authority");
    assert!(!page.incomplete);
}

#[test]
fn duplicate_dated_identity_is_ambiguous_even_with_a_legacy_log() {
    let root = workspace();
    let home = folder(root.path(), "same", 10, "first");
    let other = root.path().join(".a3s/sessions/2026-10-11/same");
    std::fs::create_dir_all(&other).expect("second home");
    for name in ["session.json", "same.jsonl"] {
        std::fs::copy(home.join(name), other.join(name)).expect("copy conflicting session");
    }
    legacy(root.path(), "same", "must not substitute legacy");
    let page = list_workspace_sessions(root.path(), 30, None).expect("list");
    assert!(page.sessions.is_empty());
    assert!(page.incomplete);
    assert_eq!(page.skipped_entries, 2);
}

#[test]
fn corrupt_preferred_folder_does_not_fall_back_to_different_facts() {
    let root = workspace();
    let home = folder(root.path(), "same", 11, "original");
    std::fs::write(home.join("session.json"), b"corrupt").expect("corrupt marker");
    legacy(root.path(), "same", "wrong authority");
    let page = list_workspace_sessions(root.path(), 30, None).expect("list");
    assert!(page.sessions.is_empty());
    assert!(page.incomplete);
}

#[test]
fn marker_cannot_select_another_log_or_another_id() {
    let root = workspace();
    let home = folder(root.path(), "id", 11, "prompt");
    let original = std::fs::read(home.join("session.json")).expect("marker");
    for (key, value) in [
        ("effect_log", "../../../../outside.jsonl"),
        ("id", "different"),
    ] {
        let mut marker: serde_json::Value =
            serde_json::from_slice(&original).expect("parse marker");
        marker[key] = json!(value);
        std::fs::write(home.join("session.json"), marker.to_string()).expect("write marker");
        let page = list_workspace_sessions(root.path(), 30, None).expect("list");
        assert!(page.sessions.is_empty());
        assert!(page.incomplete);
    }
}

#[test]
fn a_moved_workspace_uses_its_opened_root_not_the_stored_path() {
    let root = workspace();
    let home = folder(root.path(), "id", 11, "here");
    let marker_path = home.join("session.json");
    let mut marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&marker_path).expect("read")).expect("parse");
    marker["workspace"] = json!("/nonexistent/old-workspace");
    std::fs::write(marker_path, marker.to_string()).expect("write");
    let page = list_workspace_sessions(root.path(), 30, None).expect("list");
    assert_eq!(
        page.sessions[0].cwd,
        root.path().canonicalize().expect("canonical")
    );
}

#[test]
fn malformed_facts_order_and_duplicate_keys_are_reported_as_incomplete() {
    let root = workspace();
    let home = legacy(root.path(), "id", "valid");
    let log = home.join("id.jsonl");
    let fact: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&log).expect("log")).expect("fact");
    for raw in [
        "not JSON\n".to_owned(),
        json!({"seq": 2, "type":"user.message", "key":"one", "payload":{"text":"gap"}}).to_string(),
        format!(
            "{fact}\n{}\n",
            json!({"seq":2, "type":"user.message", "key":"user:1", "payload":{"text":"duplicate"}})
        ),
    ] {
        std::fs::write(&log, raw).expect("corrupt log");
        let page = list_workspace_sessions(root.path(), 30, None).expect("list");
        assert!(page.sessions.is_empty());
        assert!(page.incomplete);
        assert_eq!(page.skipped_entries, 1);
    }
}

#[test]
fn counts_user_and_model_turns_but_not_tools_or_control_facts() {
    let root = workspace();
    let home = legacy(root.path(), "id", "question");
    let facts = [
        NewFact {
            kind: "model.turn".into(),
            key: "model:1".into(),
            payload: serde_json::to_value(a3s_effect::coding::ModelDecision::Text {
                text: "answer".into(),
            })
            .expect("decision"),
        },
        NewFact {
            kind: "tool.result".into(),
            key: "tool:1".into(),
            payload: json!({"output":"result"}),
        },
        NewFact {
            kind: "run.cancelled".into(),
            key: "cancel:1".into(),
            payload: json!({}),
        },
    ];
    FileLog::open(home)
        .expect("log")
        .append("id", &facts, None)
        .expect("append");
    let page = list_workspace_sessions(root.path(), 30, None).expect("list");
    assert_eq!(page.sessions[0].num_messages, 2);
    assert_eq!(page.sessions[0].fact_count, 4);
}

#[test]
fn literal_query_runs_before_page_limit_and_prompt_is_utf8_bounded() {
    let root = workspace();
    legacy(root.path(), "one", "different");
    let prompt = format!("中文 {}", "🔬".repeat(1500));
    legacy(root.path(), "two", &prompt);
    let page = list_workspace_sessions(root.path(), 1, Some("中文")).expect("list");
    assert_eq!(page.sessions[0].id, "two");
    let actual = page.sessions[0].first_prompt.as_ref().expect("prompt");
    assert!(actual.len() <= MAX_PROMPT_BYTES);
    assert!(page.sessions[0].first_prompt_truncated);
    assert!(prompt.starts_with(actual));
    assert!(!page.truncated);
    let page = list_workspace_sessions(root.path(), 1, None).expect("list");
    assert!(page.truncated);
    assert!(!page.incomplete);
    assert!(list_workspace_sessions(root.path(), 0, None).is_err());
    assert!(list_workspace_sessions(root.path(), 101, None).is_err());
    assert!(list_workspace_sessions(root.path(), 30, Some(&"x".repeat(4097))).is_err());
}

#[test]
fn oversized_files_and_aggregate_budget_never_report_complete() {
    let root = workspace();
    let home = legacy(root.path(), "id", "original");
    std::fs::OpenOptions::new()
        .write(true)
        .open(home.join("id.jsonl"))
        .expect("file")
        .set_len(MAX_LOG_BYTES + 1)
        .expect("sparse oversized file");
    let page = list_workspace_sessions(root.path(), 30, None).expect("list");
    assert!(page.sessions.is_empty());
    assert!(page.incomplete);
    std::fs::write(home.join("small"), b"0123456789").expect("small");
    let dir = Dir::open_ambient_dir(home, cap_std::ambient_authority()).expect("dir");
    let mut scan = Scan::new();
    scan.remaining_bytes = 10;
    read_regular(&dir, "small", MAX_LOG_BYTES, &mut scan).expect("first read");
    assert_eq!(scan.remaining_bytes, 0);
    assert!(read_regular(&dir, "small", MAX_LOG_BYTES, &mut scan).is_err());
}

#[test]
fn directory_budget_is_shared_between_layouts() {
    let root = workspace();
    folder(root.path(), "new", 11, "new");
    let legacy = legacy(root.path(), "old", "old");
    let dir = Dir::open_ambient_dir(legacy, cap_std::ambient_authority()).expect("dir");
    let mut scan = Scan::new();
    scan.remaining_entries = 0;
    let mut entries = BTreeMap::new();
    scan_legacy(&dir, root.path(), &mut scan, &mut entries).expect("scan");
    assert!(entries.is_empty());
    assert!(scan.incomplete);
}

#[cfg(unix)]
#[test]
fn query_preserves_permissions_and_all_stored_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let root = workspace();
    let home = folder(root.path(), "id", 11, "unchanged");
    let paths = [
        home.clone(),
        home.join("session.json"),
        home.join("id.jsonl"),
    ];
    for path in &paths {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("mode");
    }
    let marker = std::fs::read(home.join("session.json")).expect("marker");
    let log = std::fs::read(home.join("id.jsonl")).expect("log");
    list_workspace_sessions(root.path(), 30, None).expect("list");
    for path in &paths {
        assert_eq!(
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
    assert_eq!(
        std::fs::read(home.join("session.json")).expect("marker"),
        marker
    );
    assert_eq!(std::fs::read(home.join("id.jsonl")).expect("log"), log);
}

#[cfg(unix)]
#[test]
fn symlinks_at_every_repository_controlled_level_are_refused() {
    use std::os::unix::fs::symlink;
    let outside = workspace();
    folder(outside.path(), "id", 11, "outside secret");
    legacy(outside.path(), "old", "outside legacy secret");
    for rel in [
        ".a3s",
        ".a3s/sessions",
        ".a3s/sessions/2026-10-11",
        ".a3s/sessions/2026-10-11/id",
        ".a3s/effect-log",
    ] {
        let root = workspace();
        let link = root.path().join(rel);
        std::fs::create_dir_all(link.parent().expect("parent")).expect("parents");
        symlink(outside.path().join(rel), &link).expect("symlink");
        if let Ok(page) = list_workspace_sessions(root.path(), 30, None) {
            assert!(page.sessions.is_empty());
            assert!(page.incomplete);
        }
    }
    for name in ["session.json", "id.jsonl"] {
        let root = workspace();
        let home = folder(root.path(), "id", 11, "inside");
        std::fs::remove_file(home.join(name)).expect("remove");
        symlink(
            outside
                .path()
                .join(".a3s/sessions/2026-10-11/id")
                .join(name),
            home.join(name),
        )
        .expect("symlink file");
        let page = list_workspace_sessions(root.path(), 30, None).expect("list");
        assert!(page.sessions.is_empty());
        assert!(page.incomplete);
    }
    let root = workspace();
    let home = legacy(root.path(), "old", "inside legacy");
    std::fs::remove_file(home.join("old.jsonl")).expect("remove legacy log");
    symlink(
        outside.path().join(".a3s/effect-log/old.jsonl"),
        home.join("old.jsonl"),
    )
    .expect("legacy log symlink");
    let page = list_workspace_sessions(root.path(), 30, None).expect("list");
    assert!(page.sessions.is_empty());
    assert!(page.incomplete);
}

#[cfg(unix)]
#[test]
fn fifo_is_rejected_before_open_without_waiting_for_a_writer() {
    use std::os::unix::ffi::OsStrExt;
    let root = workspace();
    let home = legacy(root.path(), "id", "replace");
    let path = home.join("id.jsonl");
    std::fs::remove_file(&path).expect("remove");
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("path");
    // Test fixture only: create a writer-less FIFO to pin the pre-open gate.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let start = std::time::Instant::now();
    let page = list_workspace_sessions(root.path(), 30, None).expect("list");
    assert!(page.sessions.is_empty());
    assert!(page.incomplete);
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[cfg(unix)]
#[test]
fn opened_handle_gate_rejects_a_fifo_swapped_in_after_the_precheck() {
    use std::os::unix::ffi::OsStrExt;
    let root = workspace();
    let path = root.path().join("log");
    std::fs::write(&path, b"regular before the check").expect("regular file");
    let dir = Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).expect("dir");
    assert!(dir.symlink_metadata("log").expect("precheck").is_file());
    std::fs::remove_file(&path).expect("swap");
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("path");
    // Deterministic replacement between the public reader's two gates.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let start = std::time::Instant::now();
    assert!(open_regular(&dir, "log", MAX_LOG_BYTES).is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[cfg(unix)]
#[test]
fn nofollow_open_rejects_a_symlink_swapped_in_after_the_precheck() {
    use std::os::unix::fs::symlink;
    let root = workspace();
    let outside = workspace();
    std::fs::write(root.path().join("log"), b"original").expect("regular file");
    std::fs::write(outside.path().join("secret"), b"outside").expect("outside fixture");
    let dir = Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).expect("dir");
    assert!(dir.symlink_metadata("log").expect("precheck").is_file());
    std::fs::remove_file(root.path().join("log")).expect("swap");
    symlink(outside.path().join("secret"), root.path().join("log")).expect("symlink");
    assert!(open_regular(&dir, "log", MAX_LOG_BYTES).is_err());
}

#[test]
fn exact_resolution_does_not_create_missing_state() {
    let root = workspace();
    assert!(resolve_workspace_session(root.path(), "missing")
        .expect("lookup")
        .is_none());
    assert!(!root.path().join(".a3s").exists());
    for id in ["", ".", "..", &"x".repeat(4097)] {
        assert!(resolve_workspace_session(root.path(), id).is_err());
    }
}

#[test]
fn exact_resolution_accepts_dated_original_ids_and_legacy_thread_ids() {
    let root = workspace();
    folder(root.path(), "我的会话/🔬", 11, "dated text");
    legacy(root.path(), "legacy-id", "legacy text");
    for (id, text) in [("我的会话/🔬", "dated text"), ("legacy-id", "legacy text")] {
        let entry = resolve_workspace_session(root.path(), id)
            .expect("resolve")
            .expect("existing");
        assert_eq!(entry.id, id);
        assert_eq!(entry.first_prompt.as_deref(), Some(text));
        assert_eq!(entry.fact_count, 1);
    }
    assert!(resolve_workspace_session(root.path(), "legacy")
        .expect("lookup")
        .is_none());
}

#[test]
fn exact_resolution_ignores_other_logs_and_a_catalog_page_limit() {
    let root = workspace();
    legacy(root.path(), "z-target", "wanted");
    for n in 0..105 {
        legacy(
            root.path(),
            &format!("a-{n:03}"),
            "z-target mentioned by another session",
        );
    }
    let page = list_workspace_sessions(root.path(), 100, Some("z-target")).expect("page");
    assert!(page.truncated);
    let entry = resolve_workspace_session(root.path(), "z-target")
        .expect("resolve")
        .expect("exact");
    assert_eq!(entry.id, "z-target");
    let other = root.path().join(".a3s/effect-log/unrelated.jsonl");
    std::fs::File::create(other)
        .expect("unrelated")
        .set_len(MAX_LOG_BYTES + 1)
        .expect("large unrelated log");
    assert!(resolve_workspace_session(root.path(), "z-target")
        .expect("resolve")
        .is_some());
}

#[test]
fn exact_resolution_refuses_ambiguous_dated_identity_and_legacy_substitution() {
    let root = workspace();
    let home = folder(root.path(), "same", 10, "first");
    let other = root.path().join(".a3s/sessions/2026-10-11/same");
    std::fs::create_dir_all(&other).expect("other home");
    for name in ["session.json", "same.jsonl"] {
        std::fs::copy(home.join(name), other.join(name)).expect("copy");
    }
    legacy(root.path(), "same", "must not replace the ambiguous log");
    assert!(resolve_workspace_session(root.path(), "same").is_err());
}

#[test]
fn exact_resolution_does_not_equate_colliding_encoded_ids() {
    let root = workspace();
    let first = format!("{}a", "🔬".repeat(50));
    let second = format!("{}b", "🔬".repeat(50));
    assert_eq!(
        crate::fact_control::thread_for_session(&first),
        crate::fact_control::thread_for_session(&second)
    );
    folder(root.path(), &first, 11, "original identity");
    assert!(resolve_workspace_session(root.path(), &first)
        .expect("original")
        .is_some());
    assert!(resolve_workspace_session(root.path(), &second).is_err());
}

#[test]
fn exact_resolution_refuses_corrupt_or_oversized_preferred_logs() {
    let root = workspace();
    let home = folder(root.path(), "id", 11, "original");
    legacy(root.path(), "id", "must not substitute");
    std::fs::write(home.join("id.jsonl"), b"corrupt").expect("corrupt");
    assert!(resolve_workspace_session(root.path(), "id").is_err());
    std::fs::OpenOptions::new()
        .write(true)
        .open(home.join("id.jsonl"))
        .expect("log")
        .set_len(MAX_LOG_BYTES + 1)
        .expect("large log");
    assert!(resolve_workspace_session(root.path(), "id").is_err());
}

#[test]
fn exact_resolution_requires_complete_identity_traversal() {
    let root = workspace();
    legacy(root.path(), "id", "legacy");
    let sessions = root.path().join(".a3s/sessions");
    std::fs::create_dir(&sessions).expect("sessions");
    for n in 0..=MAX_ENTRIES {
        std::fs::write(sessions.join(format!("unrelated-{n}")), b"").expect("entry");
    }
    assert!(resolve_workspace_session(root.path(), "id").is_err());
}

#[cfg(unix)]
#[test]
fn exact_resolution_refuses_linked_state_and_writerless_fifo() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let root = workspace();
    let outside = workspace();
    legacy(outside.path(), "id", "outside");
    symlink(outside.path().join(".a3s"), root.path().join(".a3s")).expect("state symlink");
    assert!(resolve_workspace_session(root.path(), "id").is_err());
    let root = workspace();
    let home = legacy(root.path(), "id", "replace");
    let path = home.join("id.jsonl");
    std::fs::remove_file(&path).expect("remove");
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).expect("path");
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let start = std::time::Instant::now();
    assert!(resolve_workspace_session(root.path(), "id").is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}
