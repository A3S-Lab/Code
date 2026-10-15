#![cfg(test)]

use super::*;
use crate::goal_control::{GoalImportSourceV1, GOAL_LEGACY_SOURCE_DOMAIN_V1};

fn prove_import(log: &dyn LogStore) {
    let plain = command();
    let wire = serde_json::to_value(&plain).unwrap();
    assert!(wire.get("import_source").is_none());
    let roundtrip: GoalDefinitionCommandV1 = serde_json::from_value(wire).unwrap();
    assert_eq!(
        command_digest(&plain).unwrap(),
        command_digest(&roundtrip).unwrap()
    );
    let mut imported = plain;
    imported.import_source = Some(GoalImportSourceV1 {
        identity: ExecutionIdentityV1::derive(GOAL_LEGACY_SOURCE_DOMAIN_V1, &"original bytes")
            .unwrap(),
        paused: true,
    });
    let pending = prepare(&[], &imported).unwrap().pending.unwrap();
    assert_eq!(pending.kind, GoalFactKindV1::Created.as_str());
    log.append("session", &[pending], None).unwrap();
    let facts = log.read("session").unwrap();
    let original = prepare(&facts, &imported).unwrap();
    assert!(original.pending.is_none());
    assert_eq!(original.definition.state, GoalState::Paused);
    assert_eq!(original.definition.import_source, imported.import_source);
    for key in ["identity", "paused"] {
        let mut broken = facts.clone();
        broken[0].payload["body"]["import_source"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(prepare(&broken, &imported).is_err());
    }
    for mutation in 0..4 {
        let mut broken = facts.clone();
        match mutation {
            0 => {
                broken[0].payload["body"]
                    .as_object_mut()
                    .unwrap()
                    .remove("import_source");
            }
            1 => broken[0].payload["body"]["import_source"]["paused"] = serde_json::json!(false),
            2 => {
                broken[0].payload["body"]["import_source"]["identity"]["domain"] =
                    serde_json::json!("other.v1")
            }
            _ => broken[0].payload["body"]["import_source"]["execute"] = serde_json::json!(true),
        }
        assert!(prepare(&broken, &imported).is_err(), "{mutation}");
    }
    let mut changed = imported.clone();
    changed.import_source.as_mut().unwrap().paused = false;
    assert!(prepare(&facts, &changed).is_err());
    changed = imported.clone();
    changed.import_source.as_mut().unwrap().identity.domain = "other.v1".into();
    assert!(prepare(&[], &changed).is_err());
    let mut revise = imported.clone();
    revise.kind = GoalCommandKindV1::Revise;
    revise.operation_id = "revise".into();
    revise.expected_goal_revision = Some(1);
    revise.expected_goal_sequence = Some(facts[0].seq);
    assert!(prepare(&facts, &revise).is_err());
    revise.import_source = None;
    revise.goal.description = "Revised scope while still paused".into();
    let revised = prepare(&facts, &revise).unwrap();
    assert_eq!(revised.definition.state, GoalState::Paused);
    log.append("session", &[revised.pending.unwrap()], None)
        .unwrap();
    let current = definition_from_facts(
        &log.read("session").unwrap(),
        "session",
        &imported.workspace,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(current.state, GoalState::Paused);
    assert_eq!(current.import_source, imported.import_source);
    assert_eq!(current.binding.goal_revision, 2);
    assert_eq!(
        prepare(&log.read("session").unwrap(), &imported)
            .unwrap()
            .definition
            .binding
            .goal_revision,
        1
    );
}

#[test]
fn import_source_is_authenticated_and_pause_survives_revision_on_both_logs() {
    prove_import(&MemoryLog::new());
    let root = tempfile::tempdir().unwrap();
    {
        let log = FileLog::open(root.path()).unwrap();
        prove_import(&log);
    }
    let log = FileLog::open(root.path()).unwrap();
    assert_eq!(
        definition_from_facts(
            &log.read("session").unwrap(),
            "session",
            &command().workspace,
            None
        )
        .unwrap()
        .unwrap()
        .state,
        GoalState::Paused
    );
}
