#![cfg(test)]

use super::*;
use crate::store::{FileSessionStore, MemorySessionStore, SessionFactSourceV1, SessionStore};
use std::sync::Arc;

mod legacy_reader {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/durable-engineering-v1/legacy_loop_reader.rs"
    ));
}

fn checkpoint(source: Option<SessionFactSourceV1>) -> LoopCheckpoint {
    let mut value: LoopCheckpoint = serde_json::from_value(serde_json::json!({
        "schema_version": 1, "run_id": "run", "session_id": "session", "turn": 1,
        "messages": [], "total_usage": {"prompt_tokens":0,"completion_tokens":0,"total_tokens":0},
        "tool_calls_count": 1, "checkpoint_ms": 1
    }))
    .unwrap();
    value.schema_version = LoopCheckpoint::schema_for(source.as_ref());
    value.durable_source = source;
    value
}

fn source(kind: &str) -> SessionFactSourceV1 {
    SessionFactSourceV1::from_facts(
        "session",
        &[a3s_effect::Fact {
            seq: 1,
            kind: kind.into(),
            key: "source".into(),
            cause: None,
            payload: serde_json::json!({"original":"source bytes"}),
        }],
    )
    .unwrap()
    .unwrap()
}

#[test]
fn durable_loop_source_rejects_downgrade_stripping_foreign_and_future_versions() {
    let policy = source("a3s.work-policy.admitted");
    let goal = source("a3s.goal.created");
    let original = checkpoint(Some(policy.clone()));
    original.validate_source(Some(&policy)).unwrap();
    assert!(original.validate_source(None).is_err());
    assert!(original.validate_source(Some(&goal)).is_err());
    let old: legacy_reader::LegacyLoopReader =
        serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
    assert!(old.ensure_loadable().is_err());
    for version in [0, 1, 3] {
        let mut bad = original.clone();
        bad.schema_version = version;
        assert!(bad.ensure_loadable().is_err());
    }
    let mut stripped = original;
    stripped.durable_source = None;
    assert!(stripped.ensure_loadable().is_err());
    for version in [0, 1] {
        let mut ordinary = checkpoint(None);
        ordinary.schema_version = version;
        ordinary.ensure_loadable().unwrap();
        assert!(ordinary.validate_source(Some(&policy)).is_err());
        let old: legacy_reader::LegacyLoopReader =
            serde_json::from_value(serde_json::to_value(ordinary).unwrap()).unwrap();
        old.ensure_loadable().unwrap();
    }
}

#[tokio::test]
async fn durable_loop_source_roundtrips_both_stores_and_actual_file_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let memory: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    for file in [false, true] {
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(directory.path()).await.unwrap())
        } else {
            memory.clone()
        };
        for (index, source) in [
            None,
            Some(source("a3s.work-policy.admitted")),
            Some(source("a3s.goal.created")),
            Some(source("a3s.goal.plan.accepted")),
        ]
        .into_iter()
        .enumerate()
        {
            let mut value = checkpoint(source);
            value.run_id = format!("run-{index}");
            store
                .save_loop_checkpoint(&value.run_id, &value)
                .await
                .unwrap();
        }
        drop(store);
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(directory.path()).await.unwrap())
        } else {
            memory.clone()
        };
        for index in 0..4 {
            let value = store
                .load_loop_checkpoint(&format!("run-{index}"))
                .await
                .unwrap()
                .unwrap();
            value
                .ensure_owned_by(&format!("run-{index}"), "session")
                .unwrap();
            assert_eq!(value.schema_version, if index == 0 { 1 } else { 2 });
            value
                .validate_source(value.durable_source.as_ref())
                .unwrap();
        }
    }
}
