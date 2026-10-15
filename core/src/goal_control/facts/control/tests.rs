#![cfg(test)]

use super::*;
use a3s_effect::{FileLog, LogStore, MemoryLog};

fn create(log: &dyn LogStore) -> (GoalDefinitionCommandV1, GoalControlCommandV1) {
    let create = super::super::tests::command();
    let pending = prepare_definition(&[], "session", &create.workspace, None, &create)
        .unwrap()
        .pending
        .unwrap();
    log.append("session", &[pending], None).unwrap();
    let current = log.read("session").unwrap();
    let cancel = GoalControlCommandV1 {
        kind: GoalCommandKindV1::Cancel,
        operation_id: "cancel".into(),
        goal_id: create.goal_id.clone(),
        session_id: create.session_id.clone(),
        workspace: create.workspace.clone(),
        work_policy_digest: None,
        expected_goal_revision: 1,
        expected_plan_revision: None,
        expected_goal_sequence: current[0].seq,
    };
    (create, cancel)
}

fn prepare(
    facts: &[Fact],
    command: &GoalControlCommandV1,
) -> Result<PreparedDefinition, GoalControlError> {
    prepare_control(
        facts,
        "session",
        &WorkspaceRef::new("workspace", "/workspace"),
        None,
        command,
    )
}

fn prove_cancel(log: &dyn LogStore) {
    let (original, command) = create(log);
    let before = log.read("session").unwrap();
    let prepared = prepare(&before, &command).unwrap();
    log.append("session", &[prepared.pending.unwrap()], None)
        .unwrap();
    let facts = log.read("session").unwrap();
    assert_eq!(&facts[..before.len()], before.as_slice());
    assert_eq!(facts.len(), before.len() + 1);
    let replay = prepare(&facts, &command).unwrap();
    assert!(replay.pending.is_none());
    assert_eq!(replay.definition.state, GoalState::Cancelled);
    assert_eq!(replay.definition.binding.goal_revision, 1);
    assert_eq!(replay.definition.goal_sequence, facts.last().unwrap().seq);
    assert_eq!(
        serde_json::to_value(&replay.definition.goal).unwrap(),
        serde_json::to_value(&original.goal).unwrap()
    );
    let historical =
        prepare_definition(&facts, "session", &original.workspace, None, &original).unwrap();
    assert!(historical.pending.is_none());
    assert_eq!(historical.definition.state, GoalState::Active);
    assert_eq!(
        definition_from_facts(&facts, "session", &original.workspace, None)
            .unwrap()
            .unwrap()
            .state,
        GoalState::Cancelled
    );
    let mut again = command.clone();
    again.operation_id = "another-cancel".into();
    again.expected_goal_sequence = replay.definition.goal_sequence;
    assert!(prepare(&facts, &again).is_err());
    let mut revise = original;
    revise.kind = GoalCommandKindV1::Revise;
    revise.operation_id = "revive".into();
    revise.expected_goal_revision = Some(1);
    revise.expected_goal_sequence = Some(replay.definition.goal_sequence);
    assert!(prepare_definition(&facts, "session", &revise.workspace, None, &revise).is_err());
    assert!(refuse_unbound_goal_execution(&facts).is_err());
    assert_eq!(log.read("session").unwrap(), facts);
}

#[test]
fn goal_cancel_preserves_history_and_terminal_state_in_original_stores() {
    prove_cancel(&MemoryLog::new());
    let directory = tempfile::tempdir().unwrap();
    let before = {
        let log = FileLog::open(directory.path()).unwrap();
        prove_cancel(&log);
        log.read("session").unwrap()
    };
    let log = FileLog::open(directory.path()).unwrap();
    let facts = log.read("session").unwrap();
    assert_eq!(facts, before);
    assert_eq!(
        definition_from_facts(
            &facts,
            "session",
            &WorkspaceRef::new("workspace", "/workspace"),
            None
        )
        .unwrap()
        .unwrap()
        .state,
        GoalState::Cancelled
    );
}

#[test]
fn goal_control_rejects_foreign_stale_unsupported_and_ambiguous_input() {
    let log = MemoryLog::new();
    let (_, original) = create(&log);
    let facts = log.read("session").unwrap();
    for mutation in 0..11 {
        let mut command = original.clone();
        match mutation {
            0 => command.session_id = "foreign".into(),
            1 => command.workspace.id = "foreign".into(),
            2 => command.goal_id = "foreign".into(),
            3 => command.work_policy_digest = Some("0".repeat(64)),
            4 => command.expected_goal_revision = 0,
            5 => command.expected_goal_sequence += 1,
            6 => command.expected_plan_revision = Some(0),
            7 => command.kind = GoalCommandKindV1::Resume,
            8 => command.kind = GoalCommandKindV1::Tick,
            9 => command.kind = GoalCommandKindV1::DeclareWait,
            _ => command.operation_id.clear(),
        }
        assert!(prepare(&facts, &command).is_err(), "mutation {mutation}");
    }
    let wire = serde_json::to_value(&original).unwrap();
    assert!(serde_json::from_value::<GoalControlCommandV1>(wire.clone()).is_ok());
    for field in wire.as_object().unwrap().keys() {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<GoalControlCommandV1>(missing).is_err(),
            "{field}"
        );
    }
    let mut extra = wire;
    extra["state"] = serde_json::json!("active");
    assert!(serde_json::from_value::<GoalControlCommandV1>(extra).is_err());
    assert_eq!(log.read("session").unwrap(), facts);
}

#[test]
fn goal_cancel_rejects_tampered_facts_and_changed_operation_replay() {
    let log = MemoryLog::new();
    let (_, command) = create(&log);
    let pending = prepare(&log.read("session").unwrap(), &command)
        .unwrap()
        .pending
        .unwrap();
    log.append("session", &[pending], None).unwrap();
    let original = log.read("session").unwrap();
    for field in [
        "schema",
        "session_id",
        "goal_id",
        "operation_id",
        "command_digest",
        "goal_revision",
        "plan_revision",
        "work_policy_digest",
        "extra",
    ] {
        let mut facts = original.clone();
        facts[1].payload[field] = serde_json::json!("invalid");
        assert!(prepare(&facts, &command).is_err(), "{field}");
    }
    for field in [
        "expected_goal_revision",
        "expected_plan_revision",
        "expected_goal_sequence",
    ] {
        let mut facts = original.clone();
        facts[1].payload["body"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(prepare(&facts, &command).is_err(), "{field}");
    }
    for mutation in 0..5 {
        let mut facts = original.clone();
        match mutation {
            0 => facts[1].cause = Some("foreign-run".into()),
            1 => facts[1].key = "foreign".into(),
            2 => facts[1].seq = facts[0].seq,
            3 => facts[1].payload["body"]["state"] = serde_json::json!("active"),
            _ => facts.push(facts[1].clone()),
        }
        assert!(prepare(&facts, &command).is_err(), "mutation {mutation}");
    }
    let mut changed = command;
    changed.expected_goal_sequence = original[1].seq;
    assert!(prepare(&original, &changed).is_err());
    assert_eq!(log.read("session").unwrap(), original);
}
