#![cfg(test)]

use super::*;
use a3s_effect::{FileLog, LogStore, MemoryLog, NewFact};

mod legacy_reader {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/durable-engineering-v1/legacy_snapshot_reader.rs"
    ));
}

fn prove_source(log: &dyn LogStore) {
    assert!(SessionFactSourceV1::from_facts("session", &[])
        .unwrap()
        .is_none());
    log.append(
        "session",
        &[NewFact {
            kind: "a3s.work-policy.admitted".into(),
            key: "policy".into(),
            payload: serde_json::json!({"original":"bytes"}),
        }],
        None,
    )
    .unwrap();
    let facts = log.read("session").unwrap();
    let source = SessionFactSourceV1::from_facts("session", &facts)
        .unwrap()
        .unwrap();
    source.verify("session", &facts).unwrap();
    assert!(source.verify("foreign", &facts).is_err());
    assert!(source.verify("session", &[]).is_err());
    let mut corrupt = facts.clone();
    corrupt[0].payload["original"] = serde_json::json!("changed");
    assert!(source.verify("session", &corrupt).is_err());
    log.append(
        "session",
        &[NewFact {
            kind: "user.message".into(),
            key: "next".into(),
            payload: serde_json::json!({"text":"later history"}),
        }],
        None,
    )
    .unwrap();
    let later = log.read("session").unwrap();
    source.verify("session", &later).unwrap();
    assert_eq!(
        SessionFactSourceV1::from_facts("session", &later).unwrap(),
        Some(source.clone())
    );
    let mut unknown = serde_json::to_value(source).unwrap();
    unknown["grant"] = serde_json::json!(true);
    assert!(serde_json::from_value::<SessionFactSourceV1>(unknown).is_err());
}

#[test]
fn durable_source_preserves_original_prefix_in_both_logs_and_reopen() {
    prove_source(&MemoryLog::new());
    let directory = tempfile::tempdir().unwrap();
    let log = FileLog::open(directory.path()).unwrap();
    prove_source(&log);
    let facts = log.read("session").unwrap();
    let source = SessionFactSourceV1::from_facts("session", &facts)
        .unwrap()
        .unwrap();
    drop(log);
    source
        .verify(
            "session",
            &FileLog::open(directory.path())
                .unwrap()
                .read("session")
                .unwrap(),
        )
        .unwrap();
}

#[tokio::test]
async fn durable_snapshot_requires_version_and_original_old_reader_rejects_it() {
    let mut snapshot = super::super::SessionSnapshotV1::session_only(
        super::super::tests::create_test_snapshot().await.session,
    );
    let wire = serde_json::to_value(&snapshot).unwrap();
    serde_json::from_value::<legacy_reader::LegacySnapshotHeader>(wire)
        .unwrap()
        .ensure_loadable()
        .unwrap();
    let facts = [Fact {
        seq: 1,
        kind: "a3s.goal.created".into(),
        key: "goal".into(),
        cause: None,
        payload: serde_json::json!({}),
    }];
    snapshot.session.durable_source =
        SessionFactSourceV1::from_facts(&snapshot.session.id, &facts).unwrap();
    assert!(
        snapshot.ensure_loadable().is_err(),
        "v1 cannot hide a new binding"
    );
    snapshot.schema_version = super::super::SESSION_SNAPSHOT_SCHEMA_VERSION;
    snapshot.validate_for_session(&snapshot.session.id).unwrap();
    let store_directory = tempfile::tempdir().unwrap();
    for store in [
        std::sync::Arc::new(super::super::MemorySessionStore::new())
            as std::sync::Arc<dyn super::super::SessionStore>,
        std::sync::Arc::new(
            super::super::FileSessionStore::new(store_directory.path())
                .await
                .unwrap(),
        ),
    ] {
        store.save_snapshot(&snapshot).await.unwrap();
        assert!(store.save(&snapshot.session).await.is_err());
        let mut stripped = snapshot.session.clone();
        stripped.durable_source = None;
        assert!(store.save(&stripped).await.is_err());
        assert_eq!(
            store
                .load_snapshot(&snapshot.session.id)
                .await
                .unwrap()
                .unwrap()
                .session
                .durable_source,
            snapshot.session.durable_source
        );
    }
    let raw_directory = tempfile::tempdir().unwrap();
    let raw_store = super::super::FileSessionStore::new(raw_directory.path())
        .await
        .unwrap();
    let raw_path = super::super::tests::encoded_file_path(
        raw_directory.path(),
        "sessions",
        &snapshot.session.id,
    );
    std::fs::create_dir_all(raw_path.parent().unwrap()).unwrap();
    std::fs::write(raw_path, serde_json::to_vec(&snapshot.session).unwrap()).unwrap();
    use super::super::SessionStore;
    assert!(
        raw_store.load_snapshot(&snapshot.session.id).await.is_err(),
        "a bare legacy document cannot carry the new binding"
    );
    let wire = serde_json::to_value(&snapshot).unwrap();
    assert!(
        serde_json::from_value::<legacy_reader::LegacySnapshotHeader>(wire)
            .unwrap()
            .ensure_loadable()
            .is_err()
    );
    assert!(snapshot
        .clone()
        .fork_for_session("fork", "/elsewhere")
        .is_err());
    snapshot.session.durable_source = None;
    assert!(
        snapshot.ensure_loadable().is_err(),
        "stripping a v2 source cannot downgrade recovery"
    );
}
