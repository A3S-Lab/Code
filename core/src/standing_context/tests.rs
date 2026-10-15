#![cfg(test)]

use super::*;
use a3s_effect::{FileLog, MemoryLog};

fn prove_original_log_admission(log: &dyn LogStore, log_dir: &Path, workspace: &Path) {
    let instructions = workspace.join("instructions.md");
    std::fs::write(&instructions, "Original governing bytes.\n").unwrap();
    let tools = ["read".to_owned()];
    let admitted = admit_with_log(log, log_dir, "shared", Some("base"), &[], &tools, 4, 32, 2)
        .unwrap()
        .unwrap();
    let first_facts = log.read("shared").unwrap();
    assert_eq!(first_facts.len(), 1);
    assert_eq!(first_facts[0].key, STANDING_CONTEXT_FACT_KEY);
    let again = admit_with_log(log, log_dir, "shared", Some("base"), &[], &tools, 4, 32, 2)
        .unwrap()
        .unwrap();
    assert_eq!(admitted.digest, again.digest);
    assert_eq!(log.read("shared").unwrap(), first_facts);

    let new_tools = ["read".to_owned(), "search".to_owned()];
    let changed = admit_with_log(
        log,
        log_dir,
        "shared",
        Some("base"),
        &[],
        &new_tools,
        4,
        32,
        2,
    )
    .unwrap()
    .unwrap();
    assert_ne!(changed.digest, admitted.digest);
    assert_eq!(changed.projection, admitted.projection);
    let before_refusal = log.read("shared").unwrap();
    std::fs::write(&instructions, "Changed governing bytes.\n").unwrap();
    assert!(admit_with_log(
        log,
        log_dir,
        "shared",
        Some("base"),
        &[],
        &new_tools,
        4,
        32,
        2
    )
    .is_err());
    assert_eq!(log.read("shared").unwrap(), before_refusal);

    let legacy = NewFact {
        kind: "user.message".into(),
        key: "old-message".into(),
        payload: serde_json::json!({"text":"existing history"}),
    };
    log.append("legacy-shared", &[legacy], None).unwrap();
    let before_legacy = log.read("legacy-shared").unwrap();
    assert!(
        admit_with_log(log, log_dir, "legacy-shared", None, &[], &[], 4, 32, 2)
            .unwrap()
            .is_none()
    );
    assert_eq!(log.read("legacy-shared").unwrap(), before_legacy);
}

#[test]
fn supplied_memory_log_is_used_without_opening_a_second_file_log() {
    let workspace = tempfile::tempdir().unwrap();
    let log_dir = workspace.path().join(".a3s/effect-log");
    prove_original_log_admission(&MemoryLog::new(), &log_dir, workspace.path());
    assert!(
        !log_dir.exists(),
        "admission must not open a hidden FileLog"
    );
}

#[test]
fn supplied_file_log_survives_actual_reopen_with_original_admission_semantics() {
    let workspace = tempfile::tempdir().unwrap();
    let log_dir = workspace.path().join(".a3s/effect-log");
    let retained = {
        let log = FileLog::open(&log_dir).unwrap();
        prove_original_log_admission(&log, &log_dir, workspace.path());
        log.read("shared").unwrap()
    };
    assert_eq!(
        FileLog::open(&log_dir).unwrap().read("shared").unwrap(),
        retained
    );
}

fn prove_read_append_is_not_compare_and_swap(first: &dyn LogStore, second: &dyn LogStore) {
    first
        .append(
            "unfenced",
            &[NewFact {
                kind: "a3s.goal.created".into(),
                key: "initial".into(),
                payload: serde_json::json!({"goal_revision":1}),
            }],
            None,
        )
        .unwrap();
    let observed_first = first.read("unfenced").unwrap();
    let observed_second = second.read("unfenced").unwrap();
    assert_eq!(observed_first, observed_second);
    let stale_seq = observed_first.last().unwrap().seq;
    for (log, key) in [(first, "pause"), (second, "resume")] {
        log.append(
            "unfenced",
            &[NewFact {
                kind: format!("a3s.goal.{key}"),
                key: key.into(),
                payload: serde_json::json!({"expected_goal_sequence":stale_seq}),
            }],
            None,
        )
        .unwrap();
    }
    let facts = first.read("unfenced").unwrap();
    assert_eq!(
        facts.len(),
        3,
        "raw append does not validate command expectations"
    );
    assert_eq!(facts[2].payload["expected_goal_sequence"], stale_seq);
    assert!(facts[1].seq > stale_seq);
}

#[test]
fn raw_log_read_append_cannot_supply_goal_writer_exclusion() {
    let memory = MemoryLog::new();
    prove_read_append_is_not_compare_and_swap(&memory, &memory);
    let directory = tempfile::tempdir().unwrap();
    let first = FileLog::open(directory.path()).unwrap();
    let second = FileLog::open(directory.path()).unwrap();
    prove_read_append_is_not_compare_and_swap(&first, &second);
    drop((first, second));
    assert_eq!(
        FileLog::open(directory.path())
            .unwrap()
            .read("unfenced")
            .unwrap()
            .len(),
        3
    );
}

/// These fixtures keep their logs under `<workspace>/.a3s`.
fn workspace_of_log(log_dir: &Path) -> &Path {
    log_dir
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(".a3s"))
        .and_then(Path::parent)
        .expect("fixture log under <workspace>/.a3s")
}

#[allow(clippy::too_many_arguments)]
fn admit_with_log(
    log: &dyn LogStore,
    log_dir: &Path,
    thread: &str,
    agent_system: Option<&str>,
    harness_system: &[String],
    tools: &[String],
    tool_rounds: u32,
    step_limit: u32,
    model_attempts: u32,
) -> Result<Option<AdmittedStanding>> {
    let facts = log.read(thread).unwrap();
    super::admit(
        log,
        &facts,
        workspace_of_log(log_dir),
        thread,
        agent_system,
        harness_system,
        tools,
        tool_rounds,
        step_limit,
        model_attempts,
    )
}

#[allow(clippy::too_many_arguments)]
fn admit(
    log_dir: &Path,
    thread: &str,
    agent_system: Option<&str>,
    harness_system: &[String],
    tools: &[String],
    tool_rounds: u32,
    step_limit: u32,
    model_attempts: u32,
) -> Result<Option<AdmittedStanding>> {
    let log = FileLog::open(log_dir).unwrap();
    admit_with_log(
        &log,
        log_dir,
        thread,
        agent_system,
        harness_system,
        tools,
        tool_rounds,
        step_limit,
        model_attempts,
    )
}

#[test]
fn instructions_change_the_digest_and_the_fact_keeps_only_the_digest() {
    let workspace = tempfile::tempdir().unwrap();
    let log_dir = workspace.path().join(".a3s").join("effect-log");
    std::fs::write(workspace.path().join("instructions.md"), "OPEN-TIME\n").unwrap();
    let admitted = admit(
        &log_dir,
        "standing-test",
        Some("base prompt"),
        &["careful".into()],
        &["read".into(), "bash".into()],
        4,
        32,
        2,
    )
    .unwrap()
    .expect("new log");
    assert!(admitted.projection.contains("OPEN-TIME"));
    assert!(admitted.projection.contains("base prompt"));
    let facts = FileLog::open(&log_dir)
        .unwrap()
        .read("standing-test")
        .unwrap();
    let payload = facts[0].payload.to_string();
    assert!(payload.contains(&admitted.digest));
    assert!(!payload.contains("OPEN-TIME"));
    assert!(!payload.contains("base prompt"));

    std::fs::write(workspace.path().join("instructions.md"), "CHANGED\n").unwrap();
    let error = admit(
        &log_dir,
        "standing-test",
        Some("base prompt"),
        &["careful".into()],
        &["read".into(), "bash".into()],
        4,
        32,
        2,
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("standing context digest mismatch"));
}

#[test]
fn a_legacy_log_does_not_gain_a_synthetic_digest() {
    let workspace = tempfile::tempdir().unwrap();
    let log_dir = workspace.path().join(".a3s").join("effect-log");
    let log = FileLog::open(&log_dir).unwrap();
    log.append(
        "legacy",
        &[NewFact {
            kind: "user.message".into(),
            key: "m-0".into(),
            payload: serde_json::json!({ "text": "already running" }),
        }],
        None,
    )
    .unwrap();
    let admitted = admit(&log_dir, "legacy", None, &[], &[], 1, 32, 2).unwrap();
    assert!(admitted.is_none());
    let facts = log.read("legacy").unwrap();
    assert!(facts
        .iter()
        .all(|fact| fact.kind != STANDING_CONTEXT_FACT_KIND));
}

#[test]
fn a_malformed_standing_fact_fails_closed() {
    let workspace = tempfile::tempdir().unwrap();
    let log_dir = workspace.path().join(".a3s").join("effect-log");
    let log = FileLog::open(&log_dir).unwrap();
    log.append(
        "broken",
        &[NewFact {
            kind: STANDING_CONTEXT_FACT_KIND.into(),
            key: STANDING_CONTEXT_FACT_KEY.into(),
            payload: serde_json::json!({ "digest": "not-a-digest" }),
        }],
        None,
    )
    .unwrap();
    log.append(
        "broken",
        &[NewFact {
            kind: "user.message".into(),
            key: "m-0".into(),
            payload: serde_json::json!({ "text": "already running" }),
        }],
        None,
    )
    .unwrap();
    let error = admit(&log_dir, "broken", None, &[], &[], 1, 32, 2).unwrap_err();
    assert!(error.to_string().contains("malformed"));
}

#[test]
fn a_new_tool_catalog_admits_again_when_the_files_stay() {
    let workspace = tempfile::tempdir().unwrap();
    let log_dir = workspace.path().join(".a3s").join("effect-log");
    std::fs::write(workspace.path().join("instructions.md"), "OPEN-TIME\n").unwrap();
    let first = admit(
        &log_dir,
        "tools",
        Some("base"),
        &[],
        &["read".into()],
        4,
        32,
        2,
    )
    .unwrap()
    .expect("first admission");
    let second = admit(
        &log_dir,
        "tools",
        Some("base"),
        &[],
        &["bash".into(), "read".into()],
        4,
        32,
        2,
    )
    .unwrap()
    .expect("tool catalog change");
    assert_ne!(first.digest, second.digest);
    assert_eq!(first.projection, second.projection);
    let again = admit(
        &log_dir,
        "tools",
        Some("base"),
        &[],
        &["read".into()],
        4,
        32,
        2,
    )
    .unwrap()
    .expect("original catalog");
    assert_eq!(again.digest, first.digest);
    std::fs::write(workspace.path().join("instructions.md"), "CHANGED\n").unwrap();
    let error = admit(
        &log_dir,
        "tools",
        Some("base"),
        &[],
        &["bash".into()],
        4,
        32,
        2,
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("standing context digest mismatch"));
}

#[test]
fn live_surfaces_stay_admitted_while_instructions_fail_closed() {
    let workspace = tempfile::tempdir().unwrap();
    let log_dir = workspace.path().join(".a3s").join("effect-log");
    let skills = workspace.path().join("skills");
    let memory = workspace.path().join(".a3s").join("memory");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(skills.join("skill.md"), "SKILL-OPEN\n").unwrap();
    std::fs::write(memory.join("note.md"), "MEMORY-OPEN\n").unwrap();
    let tools = ["read".to_string()];
    let first = admit(&log_dir, "files", None, &[], &tools, 4, 32, 2)
        .unwrap()
        .expect("first admission");
    let payload = FileLog::open(&log_dir).unwrap().read("files").unwrap()[0]
        .payload
        .to_string();
    assert!(payload.contains(&first.digest));
    assert!(!payload.contains("SKILL-OPEN"));
    assert!(!payload.contains("MEMORY-OPEN"));

    std::fs::write(skills.join("skill.md"), "SKILL-CHANGED\n").unwrap();
    let skill_live = admit(&log_dir, "files", None, &[], &tools, 4, 32, 2)
        .unwrap()
        .expect("skill catalog is a live surface");
    assert_eq!(skill_live.digest, first.digest);

    std::fs::write(memory.join("note.md"), "MEMORY-CHANGED\n").unwrap();
    let memory_live = admit(&log_dir, "files", None, &[], &tools, 4, 32, 2)
        .unwrap()
        .expect("memory tree is runtime state");
    assert_eq!(memory_live.digest, first.digest);

    std::fs::write(workspace.path().join("instructions.md"), "GOVERNING-TEXT\n").unwrap();
    let error = admit(&log_dir, "files", None, &[], &tools, 4, 32, 2).unwrap_err();
    assert!(error
        .to_string()
        .contains("standing context digest mismatch"));
}

#[test]
fn turn_end_extraction_and_skill_install_keep_the_next_turn_admitted() {
    let workspace = tempfile::tempdir().unwrap();
    let log_dir = workspace.path().join(".a3s").join("effect-log");
    std::fs::write(workspace.path().join("instructions.md"), "OPEN-TIME\n").unwrap();
    let tools = ["read".to_string()];
    let first = admit(&log_dir, "extraction", None, &[], &tools, 4, 32, 2)
        .unwrap()
        .expect("turn one admits");

    // Between turns the system writes its own state: turn-end memory
    // extraction appends an item with fresh read telemetry, and a skill
    // the agent installed lands in the live catalog.
    let items = workspace.path().join(".a3s").join("memory").join("items");
    std::fs::create_dir_all(&items).unwrap();
    std::fs::write(
        items.join("item.json"),
        memory_item_json("extracted fact", 1, "\"2026-10-06T11:42:56.074142Z\""),
    )
    .unwrap();
    let skills = workspace.path().join("skills");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::write(skills.join("stc-answer.md"), "INSTALLED-SKILL\n").unwrap();

    let next = admit(&log_dir, "extraction", None, &[], &tools, 4, 32, 2)
        .unwrap()
        .expect("the system's own writes never fail the next turn");
    assert_eq!(next.digest, first.digest);
    let facts = FileLog::open(&log_dir).unwrap().read("extraction").unwrap();
    assert_eq!(
        facts
            .iter()
            .filter(|fact| fact.kind == STANDING_CONTEXT_FACT_KIND)
            .count(),
        1,
        "an unchanged standing context appends no new fact"
    );
}

#[test]
fn a_session_folder_log_still_admits_the_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("instructions.md"), "FOLDER-OPEN\n").unwrap();
    let session_id = "22222222-2222-4222-8222-222222222222";
    let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
    let dir =
        crate::session_folder::ensure_session_dir_on(workspace.path(), session_id, day).unwrap();
    let tools = ["read".to_string()];
    let first = admit(&dir, session_id, None, &[], &tools, 4, 32, 2)
        .unwrap()
        .expect("admit");
    assert!(dir.join(format!("{session_id}.jsonl")).is_file());

    std::fs::write(workspace.path().join("instructions.md"), "FOLDER-CHANGED\n").unwrap();
    let error = admit(&dir, session_id, None, &[], &tools, 4, 32, 2).unwrap_err();
    assert!(error
        .to_string()
        .contains("standing context digest mismatch"));

    std::fs::write(workspace.path().join("instructions.md"), "FOLDER-OPEN\n").unwrap();
    let restored = admit(&dir, session_id, None, &[], &tools, 4, 32, 2)
        .unwrap()
        .unwrap();
    assert_eq!(restored.digest, first.digest);
}

fn memory_item_json(content: &str, access_count: u32, last_accessed: &str) -> String {
    format!(
        r#"{{
  "id": "item",
  "content": "{content}",
  "timestamp": "2026-09-25T15:58:20.435254Z",
  "importance": 0.7,
  "tags": ["project_fact"],
  "memory_type": "semantic",
  "metadata": {{"source": "project_fact"}},
  "access_count": {access_count},
  "last_accessed": {last_accessed}
}}"#
    )
}
