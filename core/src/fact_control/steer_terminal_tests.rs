#![cfg(test)]

use super::*;
use crate::run_control::RunControlCommand;
use a3s_effect::MemoryLog;

fn steer(text: &str) -> RunControlCommand {
    RunControlCommand::Steer { input: text.into() }
}

fn namespace_campaign(log: &dyn LogStore, old_text: &str) {
    let session = "exact-session";
    let run = "external:运行";
    let id = "request:输入";
    let digest =
        crate::content_digest::digest_json("a3s.code.steer-input.identity.v1", &[session, run, id])
            .unwrap();
    // Every digest-shaped old raw id is still a valid old host request id.
    let old = message_fact(format!("steer:{digest}"), old_text);
    a3s_effect::append_input(log, session, old).unwrap();
    let old_facts = log.read(session).unwrap();
    let exact = "保留 Unicode\n\t  ";
    append_steer_input(log, session, run, id, &steer(exact)).unwrap();
    let committed = log.read(session).unwrap();
    assert_eq!(committed.len(), 2);
    assert_eq!(committed[0], old_facts[0]);
    assert_eq!(committed[1].key, format!("run-steer-v1:{digest}"));
    assert_eq!(committed[1].payload["text"], exact);
    append_steer_input(log, session, run, id, &steer(exact)).unwrap();
    assert_eq!(log.read(session).unwrap(), committed);
    assert!(matches!(
        append_steer_input(log, session, run, id, &steer("changed")),
        Err(ActorError::DuplicateFact { .. })
    ));
    append_steer_input(log, session, "different-run", id, &steer(exact)).unwrap();
    // These would alias under colon concatenation.
    append_steer_input(log, session, "a:b", "c", &steer("one")).unwrap();
    append_steer_input(log, session, "a", "b:c", &steer("two")).unwrap();
    let facts = log.read(session).unwrap();
    assert_eq!(facts.len(), 5);
    assert_eq!(facts[0], old_facts[0]);
    assert_ne!(facts[3].key, facts[4].key);
    assert!(facts[1..]
        .iter()
        .all(|fact| !fact.key.starts_with("steer:")));
}

#[test]
fn new_namespace_is_disjoint_from_future_digest_old_ids_on_memory_and_reopened_file() {
    for old_text in ["保留 Unicode\n\t  ", "different historical text"] {
        namespace_campaign(&MemoryLog::new(), old_text);
        let directory = tempfile::tempdir().unwrap();
        namespace_campaign(&FileLog::open(directory.path()).unwrap(), old_text);
        let reopened = FileLog::open(directory.path()).unwrap();
        assert_eq!(reopened.read("exact-session").unwrap().len(), 5);
    }
}

#[test]
fn actual_file_io_error_is_returned_without_repair_or_another_writer() {
    let directory = tempfile::tempdir().unwrap();
    let log = FileLog::open(directory.path()).unwrap();
    let blocked = directory.path().join("session.jsonl");
    std::fs::create_dir(&blocked).unwrap();
    let error = append_steer_input(&log, "session", "run", "request", &steer("must fail"));
    assert!(matches!(error, Err(ActorError::Schema(_))));
    assert!(blocked.is_dir());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}
