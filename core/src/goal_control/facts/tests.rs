#![cfg(test)]

use super::*;
use a3s_effect::{FileLog, LogStore, MemoryLog};

mod import;

pub(super) fn command() -> GoalDefinitionCommandV1 {
    GoalDefinitionCommandV1 {
        kind: GoalCommandKindV1::Create,
        import_source: None,
        operation_id: "create".into(),
        goal_id: "goal".into(),
        session_id: "session".into(),
        workspace: WorkspaceRef::new("workspace", "/workspace"),
        goal: AgentGoal::new("Repair the existing caller")
            .with_criteria(vec!["Original checks pass".into()]),
        non_goals: vec!["No new runtime".into()],
        scope_fact_sequences: vec![],
        work_policy_digest: None,
        expected_goal_revision: None,
        expected_plan_revision: None,
        expected_goal_sequence: None,
    }
}

fn prepare(
    facts: &[Fact],
    command: &GoalDefinitionCommandV1,
) -> Result<PreparedDefinition, GoalControlError> {
    prepare_definition(facts, "session", &command.workspace, None, command)
}

fn prove_transitions(log: &dyn LogStore) {
    let original = command();
    let pending = prepare(&[], &original).unwrap().pending.unwrap();
    log.append("session", &[pending], None).unwrap();
    let facts = log.read("session").unwrap();
    let first = prepare(&facts, &original).unwrap();
    assert!(first.pending.is_none());
    assert_eq!(first.definition.goal_sequence, facts[0].seq);
    let mut revision = original.clone();
    revision.kind = GoalCommandKindV1::Revise;
    revision.operation_id = "revise".into();
    revision.goal.description = "Repair both existing callers".into();
    revision.expected_goal_revision = Some(1);
    revision.expected_goal_sequence = Some(facts[0].seq);
    revision.scope_fact_sequences = vec![facts[0].seq];
    let next = prepare(&facts, &revision).unwrap().pending.unwrap();
    log.append("session", &[next], None).unwrap();
    let updated = log.read("session").unwrap();
    let current = definition_from_facts(&updated, "session", &original.workspace, None)
        .unwrap()
        .unwrap();
    assert_eq!(current.binding.goal_revision, 2);
    assert_eq!(current.goal.description, revision.goal.description);
    assert_eq!(current.goal_sequence, updated[1].seq);
    assert_eq!(
        prepare(&updated, &original)
            .unwrap()
            .definition
            .goal_sequence,
        facts[0].seq
    );
    assert!(prepare(&updated, &revision).unwrap().pending.is_none());
    revision.operation_id = "stale".into();
    assert!(prepare(&updated, &revision).is_err());
    let mut changed = original.clone();
    changed.goal.description = "Changed content under the same operation".into();
    assert!(prepare(&updated, &changed).is_err());
    assert_eq!(log.read("session").unwrap(), updated);
}

#[test]
fn definitions_replay_revise_and_reopen_original_memory_and_file_logs() {
    prove_transitions(&MemoryLog::new());
    let directory = tempfile::tempdir().unwrap();
    let before = {
        let log = FileLog::open(directory.path()).unwrap();
        prove_transitions(&log);
        log.read("session").unwrap()
    };
    let reopened = FileLog::open(directory.path())
        .unwrap()
        .read("session")
        .unwrap();
    assert_eq!(reopened, before);
    assert_eq!(
        definition_from_facts(&reopened, "session", &command().workspace, None)
            .unwrap()
            .unwrap()
            .binding
            .goal_revision,
        2
    );
}

#[test]
fn corrupted_or_future_definition_sources_fail_closed() {
    let command = command();
    let log = MemoryLog::new();
    log.append(
        "session",
        &[prepare(&[], &command).unwrap().pending.unwrap()],
        None,
    )
    .unwrap();
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
        facts[0].payload[field] = serde_json::json!("invalid");
        assert!(
            definition_from_facts(&facts, "session", &command.workspace, None).is_err(),
            "{field}"
        );
    }
    for field in [
        "expected_goal_revision",
        "expected_plan_revision",
        "expected_goal_sequence",
    ] {
        let mut facts = original.clone();
        facts[0].payload["body"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(prepare(&facts, &command).is_err(), "{field}");
    }
    for mutation in 0..7 {
        let mut facts = original.clone();
        match mutation {
            0 => facts[0].kind = "a3s.goal.future".into(),
            1 => facts[0].kind = GoalFactKindV1::PlanAccepted.as_str().into(),
            2 => facts[0].key = "foreign".into(),
            3 => facts[0].cause = Some("foreign-run".into()),
            4 => facts[0].seq = 0,
            5 => facts.push(facts[0].clone()),
            _ => facts[0].payload["body"]["goal"]["permissions"] = serde_json::json!("allow"),
        }
        assert!(prepare(&facts, &command).is_err(), "mutation {mutation}");
        assert!(refuse_unbound_goal_execution(&facts).is_err());
    }
    assert!(definition_from_facts(&original, "foreign", &command.workspace, None).is_err());
    assert!(definition_from_facts(
        &original,
        "session",
        &WorkspaceRef::new("foreign", "/workspace"),
        None
    )
    .is_err());
    assert!(refuse_unbound_goal_execution(&[]).is_ok());
}

#[test]
fn invalid_requests_and_ambiguous_wire_fields_never_prepare_a_write() {
    let original = command();
    for mutation in 0..8 {
        let mut request = original.clone();
        match mutation {
            0 => request.session_id = "foreign".into(),
            1 => request.scope_fact_sequences = vec![1],
            2 => request.goal.achieved = true,
            3 => request.expected_goal_revision = Some(1),
            4 => request.expected_plan_revision = Some(1),
            5 => request.expected_goal_sequence = Some(1),
            6 => request.kind = GoalCommandKindV1::Tick,
            _ => request.work_policy_digest = Some("0".repeat(64)),
        }
        assert!(prepare(&[], &request).is_err(), "mutation {mutation}");
    }
    let wire = serde_json::to_value(&original).unwrap();
    assert!(serde_json::from_value::<GoalDefinitionCommandV1>(wire.clone()).is_ok());
    for field in [
        "work_policy_digest",
        "expected_goal_revision",
        "expected_plan_revision",
        "expected_goal_sequence",
    ] {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<GoalDefinitionCommandV1>(missing).is_err());
    }
    for field in ["created_at", "achieved_at", "progress"] {
        let mut missing = wire.clone();
        missing["goal"].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<GoalDefinitionCommandV1>(missing).is_err());
    }
}
