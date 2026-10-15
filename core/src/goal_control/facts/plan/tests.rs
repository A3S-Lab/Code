#![cfg(test)]

use super::*;
use crate::planning::{Complexity, Task};
use a3s_effect::{FileLog, LogStore, MemoryLog};

fn initial(log: &dyn LogStore) -> DurableGoalDefinitionV1 {
    let command = super::super::tests::command();
    let prepared = prepare_definition(&[], "session", &command.workspace, None, &command).unwrap();
    log.append("session", &[prepared.pending.unwrap()], None)
        .unwrap();
    definition_from_facts(
        &log.read("session").unwrap(),
        "session",
        &command.workspace,
        None,
    )
    .unwrap()
    .unwrap()
}

fn proposal(view: &DurableGoalDefinitionV1, operation: &str) -> GoalPlanCommandV1 {
    let mut plan = ExecutionPlan::new("Repair the existing callers", Complexity::Medium);
    plan.upsert_step(
        Task::new("a", "Repair first caller")
            .with_success_criteria("First check passes")
            .with_status(TaskStatus::Completed),
    );
    plan.upsert_step(
        Task::new("b", "Repair second caller")
            .with_success_criteria("Second check passes")
            .with_dependencies(vec!["a".into()])
            .with_status(TaskStatus::Completed),
    );
    GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: operation.into(),
        goal_id: view.binding.goal_id.clone(),
        session_id: view.binding.session_id.clone(),
        workspace: view.binding.workspace.clone(),
        work_policy_digest: view.binding.work_policy_digest.clone(),
        expected_goal_revision: view.binding.goal_revision,
        expected_plan_revision: view.plan_revision,
        expected_goal_sequence: view.goal_sequence,
        plan: Some(plan),
        proposal_sequence: None,
    }
}

fn accept(view: &DurableGoalDefinitionV1, operation: &str) -> GoalPlanCommandV1 {
    let mut command = proposal(view, operation);
    command.kind = GoalCommandKindV1::AcceptPlan;
    command.plan = None;
    command.proposal_sequence = view.plan_fact_sequence;
    command
}

fn apply(log: &dyn LogStore, command: &GoalPlanCommandV1) -> DurableGoalDefinitionV1 {
    let facts = log.read("session").unwrap();
    let prepared = prepare_plan(&facts, "session", &command.workspace, None, command).unwrap();
    let Some(pending) = prepared.pending else {
        return prepared.definition;
    };
    log.append("session", &[pending], None).unwrap();
    definition_from_facts(
        &log.read("session").unwrap(),
        "session",
        &command.workspace,
        None,
    )
    .unwrap()
    .unwrap()
}

fn prove_plan_transitions(log: &dyn LogStore) {
    let original = initial(log);
    let command = proposal(&original, "propose-1");
    let proposed = apply(log, &command);
    assert_eq!(proposed.plan_revision, Some(1));
    assert!(!proposed.plan_accepted);
    let projected = proposed.plan.as_ref().unwrap();
    assert!(projected
        .steps
        .iter()
        .all(|step| step.status == TaskStatus::Pending));
    assert_eq!(
        projected
            .get_ready_steps()
            .iter()
            .map(|step| step.id.as_str())
            .collect::<Vec<_>>(),
        ["a"]
    );
    assert_eq!(
        projected.definition_identity().unwrap(),
        command
            .plan
            .as_ref()
            .unwrap()
            .definition_identity()
            .unwrap()
    );
    let acceptance = accept(&proposed, "accept-1");
    let accepted = apply(log, &acceptance);
    assert!(accepted.plan_accepted);
    assert_eq!(accepted.plan_revision, Some(1));
    assert!(accepted
        .plan
        .as_ref()
        .unwrap()
        .steps
        .iter()
        .all(|step| step.status == TaskStatus::Pending));
    let facts = log.read("session").unwrap();
    assert!(facts.last().unwrap().payload["body"]["plan"].is_null());
    assert_eq!(
        facts.last().unwrap().payload["body"]["proposal_sequence"],
        proposed.goal_sequence
    );
    assert_eq!(apply(log, &command).goal_sequence, proposed.goal_sequence);
    assert_eq!(
        apply(log, &acceptance).goal_sequence,
        accepted.goal_sequence
    );
    let replan = proposal(&accepted, "propose-2");
    let updated = apply(log, &replan);
    assert_eq!(updated.plan_revision, Some(2));
    assert!(!updated.plan_accepted);
    let mut stale = acceptance.clone();
    stale.operation_id = "stale-accept".into();
    assert!(prepare_plan(
        &log.read("session").unwrap(),
        "session",
        &stale.workspace,
        None,
        &stale
    )
    .is_err());
    let mut definition = super::super::tests::command();
    definition.kind = GoalCommandKindV1::Revise;
    definition.operation_id = "revise".into();
    definition.expected_goal_revision = Some(updated.binding.goal_revision);
    definition.expected_plan_revision = updated.plan_revision;
    definition.expected_goal_sequence = Some(updated.goal_sequence);
    let pending = prepare_definition(
        &log.read("session").unwrap(),
        "session",
        &definition.workspace,
        None,
        &definition,
    )
    .unwrap()
    .pending
    .unwrap();
    log.append("session", &[pending], None).unwrap();
    let revised = definition_from_facts(
        &log.read("session").unwrap(),
        "session",
        &definition.workspace,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(revised.binding.goal_revision, 2);
    assert_eq!(revised.plan_revision, Some(2));
    assert!(revised.plan.is_none());
    assert!(!revised.plan_accepted);
    assert_eq!(
        apply(log, &proposal(&revised, "propose-3")).plan_revision,
        Some(3)
    );
    assert!(refuse_unbound_goal_execution(&log.read("session").unwrap()).is_err());
}

#[test]
fn plan_proposals_and_acceptance_share_original_memory_and_file_scenarios() {
    prove_plan_transitions(&MemoryLog::new());
    let directory = tempfile::tempdir().unwrap();
    let retained = {
        let log = FileLog::open(directory.path()).unwrap();
        prove_plan_transitions(&log);
        log.read("session").unwrap()
    };
    let reopened = FileLog::open(directory.path())
        .unwrap()
        .read("session")
        .unwrap();
    assert_eq!(retained, reopened);
    assert_eq!(
        definition_from_facts(
            &reopened,
            "session",
            &super::super::tests::command().workspace,
            None
        )
        .unwrap()
        .unwrap()
        .plan_revision,
        Some(3)
    );
}

#[test]
fn plan_wire_and_sources_refuse_unknown_missing_forged_and_stale_fields() {
    let log = MemoryLog::new();
    let initial = initial(&log);
    let command = proposal(&initial, "proposal");
    let wire = serde_json::to_value(&command).unwrap();
    assert!(serde_json::from_value::<GoalPlanCommandV1>(wire.clone()).is_ok());
    for field in [
        "plan",
        "proposal_sequence",
        "expected_plan_revision",
        "work_policy_digest",
    ] {
        let mut bad = wire.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<GoalPlanCommandV1>(bad).is_err(),
            "{field}"
        );
    }
    for location in ["root", "plan", "task"] {
        let mut bad = wire.clone();
        let target = match location {
            "root" => &mut bad,
            "plan" => &mut bad["plan"],
            _ => &mut bad["plan"]["steps"][0],
        };
        target["permission"] = serde_json::json!("allow");
        assert!(
            serde_json::from_value::<GoalPlanCommandV1>(bad).is_err(),
            "{location}"
        );
    }
    let proposed = apply(&log, &command);
    let facts = log.read("session").unwrap();
    for mutation in 0..9 {
        let mut bad = facts.clone();
        let last = bad.last_mut().unwrap();
        match mutation {
            0 => last.payload["body"]["identity"]["digest"] = serde_json::json!("sha256:bad"),
            1 => {
                last.payload["body"]["plan"]["steps"][0]["success_criteria"] =
                    serde_json::json!("foreign criteria")
            }
            2 => last.payload["plan_revision"] = serde_json::json!(2),
            3 => last.payload["goal_revision"] = serde_json::json!(2),
            4 => last.payload["body"]["expected_goal_sequence"] = serde_json::json!(last.seq),
            5 => last.payload["operation_id"] = serde_json::json!("create"),
            6 => last.cause = Some("foreign-run".into()),
            7 => last.seq = facts[0].seq,
            _ => last.payload["body"]["plan"]["extra"] = serde_json::json!(true),
        }
        assert!(
            definition_from_facts(&bad, "session", &command.workspace, None).is_err(),
            "mutation {mutation}"
        );
    }
    for mutation in 0..6 {
        let mut bad = accept(&proposed, "bad-accept");
        match mutation {
            0 => bad.proposal_sequence = Some(proposed.goal_sequence + 1),
            1 => bad.expected_goal_revision += 1,
            2 => bad.expected_plan_revision = None,
            3 => bad.plan = command.plan.clone(),
            4 => bad.operation_id = "proposal".into(),
            _ => bad.session_id = "foreign".into(),
        }
        assert!(prepare_plan(&facts, "session", &command.workspace, None, &bad).is_err());
    }
    let mut max = proposed;
    max.plan_revision = Some(u32::MAX);
    assert!(transition(&max, &proposal(&max, "overflow"), 4).is_err());
    assert_eq!(log.read("session").unwrap(), facts);
}
