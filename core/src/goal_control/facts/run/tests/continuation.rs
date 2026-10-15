#![cfg(test)]

use super::*;

fn source(max: u64, stalled: u64, clock: u64) -> ContinuationSource {
    serde_json::from_value(serde_json::json!({
        "policy": {
            "automatic_enabled": true,
            "max_slices": max,
            "max_no_progress_slices": stalled,
            "deadline_unix_ms": 10000,
        },
        "observed_unix_ms": clock,
    }))
    .unwrap()
}

fn usage(log: &dyn LogStore, workspace: &WorkspaceRef) -> ContinuationUsage {
    fold_definitions(
        &log.read("session").unwrap(),
        "session",
        workspace,
        None,
        None,
    )
    .unwrap()
    .continuation
}

fn append_run(
    log: &dyn LogStore,
    workspace: &WorkspaceRef,
    id: &str,
    source: Option<&ContinuationSource>,
) {
    let prepared = prepare_run(
        &log.read("session").unwrap(),
        "session",
        workspace,
        None,
        Some(id),
        source,
    )
    .unwrap()
    .unwrap();
    log.append("session", &[prepared.pending.unwrap()], None)
        .unwrap();
}

fn replan(log: &dyn LogStore, workspace: &WorkspaceRef, plan: ExecutionPlan, id: &str) {
    let view = definition_from_facts(&log.read("session").unwrap(), "session", workspace, None)
        .unwrap()
        .unwrap();
    let mut command = GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: id.into(),
        goal_id: view.binding.goal_id,
        session_id: "session".into(),
        workspace: workspace.clone(),
        work_policy_digest: None,
        expected_goal_revision: view.binding.goal_revision,
        expected_plan_revision: view.plan_revision,
        expected_goal_sequence: view.goal_sequence,
        plan: Some(plan),
        proposal_sequence: None,
    };
    let prepared = prepare_plan(
        &log.read("session").unwrap(),
        "session",
        workspace,
        None,
        &command,
    )
    .unwrap();
    log.append("session", &[prepared.pending.unwrap()], None)
        .unwrap();
    let view = definition_from_facts(&log.read("session").unwrap(), "session", workspace, None)
        .unwrap()
        .unwrap();
    command.kind = GoalCommandKindV1::AcceptPlan;
    command.operation_id = format!("accept-{id}");
    command.expected_plan_revision = view.plan_revision;
    command.expected_goal_sequence = view.goal_sequence;
    command.proposal_sequence = view.plan_fact_sequence;
    command.plan = None;
    let prepared = prepare_plan(
        &log.read("session").unwrap(),
        "session",
        workspace,
        None,
        &command,
    )
    .unwrap();
    log.append("session", &[prepared.pending.unwrap()], None)
        .unwrap();
}

fn prove_history(log: &dyn LogStore) -> WorkspaceRef {
    let (workspace, _) = seed(log);
    append_run(log, &workspace, "ordinary", None);
    let ordinary = log.read("session").unwrap();
    assert!(ordinary.last().unwrap().payload["body"]
        .get("continuation")
        .is_none());
    let ordinary = usage(log, &workspace);
    assert_eq!(
        (
            ordinary.admitted_slices,
            ordinary.slices_since_progress,
            ordinary.last_observed_unix_ms
        ),
        (0, 0, 0)
    );
    let first = source(2, 2, 100);
    append_run(log, &workspace, "auto-1", Some(&first));
    let before = log.read("session").unwrap();
    for _ in 0..4 {
        assert!(prepare_run(
            &before,
            "session",
            &workspace,
            None,
            Some("auto-1"),
            Some(&first)
        )
        .unwrap()
        .unwrap()
        .pending
        .is_none());
    }
    assert!(prepare_run(&before, "session", &workspace, None, Some("auto-1"), None).is_err());
    assert!(prepare_run(
        &before,
        "session",
        &workspace,
        None,
        Some("auto-1"),
        Some(&source(2, 2, 101))
    )
    .is_err());
    assert_eq!(log.read("session").unwrap(), before);
    assert_eq!(usage(log, &workspace).admitted_slices, 1);
    let view = definition_from_facts(&before, "session", &workspace, None)
        .unwrap()
        .unwrap();
    replan(log, &workspace, view.plan.unwrap(), "replan");
    assert_eq!(usage(log, &workspace).slices_since_progress, 1);
    let second = source(2, 2, 200);
    append_run(log, &workspace, "auto-2", Some(&second));
    let before = log.read("session").unwrap();
    assert!(
        prepare_run(
            &before,
            "session",
            &workspace,
            None,
            Some("auto-2"),
            Some(&second)
        )
        .unwrap()
        .unwrap()
        .pending
        .is_none(),
        "exact replay consumes no extra opportunity at the cap"
    );
    assert!(prepare_run(
        &before,
        "session",
        &workspace,
        None,
        Some("auto-3"),
        Some(&source(2, 9, 300))
    )
    .is_err());
    assert!(prepare_run(
        &before,
        "session",
        &workspace,
        None,
        Some("auto-3"),
        Some(&source(9, 2, 300))
    )
    .is_err());
    append_run(log, &workspace, "explicit-independent", None);
    let before = log.read("session").unwrap();
    let view = definition_from_facts(&before, "session", &workspace, None)
        .unwrap()
        .unwrap();
    let plan = view.plan.clone().unwrap();
    let mut revision = crate::goal_control::facts::tests::command();
    revision.kind = GoalCommandKindV1::Revise;
    revision.operation_id = "revise".into();
    revision.expected_goal_revision = Some(view.binding.goal_revision);
    revision.expected_plan_revision = view.plan_revision;
    revision.expected_goal_sequence = Some(view.goal_sequence);
    revision.goal.description = "Revised requirements, same retained history".into();
    let revised = prepare_definition(&before, "session", &workspace, None, &revision).unwrap();
    log.append("session", &[revised.pending.unwrap()], None)
        .unwrap();
    replan(log, &workspace, plan, "replan-revision");
    assert_eq!(usage(log, &workspace).admitted_slices, 2);
    assert_eq!(usage(log, &workspace).slices_since_progress, 2);
    let before = log.read("session").unwrap();
    assert!(prepare_run(
        &before,
        "session",
        &workspace,
        None,
        Some("auto-3"),
        Some(&source(2, 9, 300))
    )
    .is_err());
    append_run(log, &workspace, "auto-3", Some(&source(3, 3, 300)));
    assert_eq!(usage(log, &workspace).admitted_slices, 3);
    assert_eq!(usage(log, &workspace).slices_since_progress, 3);
    workspace
}

/// At the stall bound only a changed accepted plan earns one more slice, once.
#[test]
fn stall_bound_admits_exactly_one_slice_after_a_changed_plan() {
    prove_stall(&MemoryLog::new());
    let directory = tempfile::tempdir().unwrap();
    let workspace = prove_stall(&FileLog::open(directory.path()).unwrap());
    let reopened = FileLog::open(directory.path()).unwrap();
    assert_eq!(usage(&reopened, &workspace).admitted_slices, 2);
    assert_eq!(usage(&reopened, &workspace).slices_since_progress, 2);
}

fn prove_stall(log: &dyn LogStore) -> WorkspaceRef {
    let (workspace, _) = seed(log);
    let check = |clock| {
        check_continuation(
            &log.read("session").unwrap(),
            "session",
            &workspace,
            None,
            &source(9, 1, clock),
        )
    };
    let stalled = |result: Result<(), GoalControlError>| {
        matches!(
            result,
            Err(GoalControlError::Refused {
                refusal: GoalRefusal::Waiting,
                ..
            })
        )
    };
    append_run(log, &workspace, "auto-1", Some(&source(9, 1, 100)));
    assert!(stalled(check(110)));
    let plan = || {
        definition_from_facts(&log.read("session").unwrap(), "session", &workspace, None)
            .unwrap()
            .unwrap()
            .plan
            .unwrap()
    };
    replan(log, &workspace, plan(), "same");
    assert!(stalled(check(120)), "an identical plan is no change");
    let mut changed = plan();
    changed.steps[1].content = "Verify the original source with a second reader".into();
    replan(log, &workspace, changed, "changed");
    assert!(check(130).is_ok());
    append_run(log, &workspace, "auto-replan", Some(&source(9, 1, 130)));
    assert_eq!(usage(log, &workspace).slices_since_progress, 2);
    let mut again = plan();
    again.steps[1].content = "Verify the original source a third way".into();
    replan(log, &workspace, again, "again");
    assert!(stalled(check(140)), "at most one budgeted replan per stall");
    let lifetime = check_continuation(
        &log.read("session").unwrap(),
        "session",
        &workspace,
        None,
        &source(2, 9, 150),
    );
    assert!(matches!(
        lifetime,
        Err(GoalControlError::Refused {
            refusal: GoalRefusal::BudgetExhausted,
            ..
        })
    ));
    assert_eq!(usage(log, &workspace).admitted_slices, 2);
    workspace
}

#[test]
fn continuation_history_preserves_caps_across_replay_revision_replan_and_both_logs() {
    prove_history(&MemoryLog::new());
    let directory = tempfile::tempdir().unwrap();
    let (workspace, before) = {
        let log = FileLog::open(directory.path()).unwrap();
        let workspace = prove_history(&log);
        let before = log.read("session").unwrap();
        let binding = crate::store::SessionFactSourceV1::from_facts("session", &before).unwrap();
        let writer = log.try_writer_fence().unwrap();
        crate::fact_control::append_compaction_done(
            &log,
            &writer,
            "session",
            "All budgets reset to zero",
            binding.as_ref(),
        )
        .unwrap();
        assert_eq!(usage(&log, &workspace).admitted_slices, 3);
        assert_eq!(usage(&log, &workspace).slices_since_progress, 3);
        drop(writer);
        (workspace, log.read("session").unwrap())
    };
    let log = FileLog::open(directory.path()).unwrap();
    assert_eq!(log.read("session").unwrap(), before);
    assert_eq!(usage(&log, &workspace).admitted_slices, 3);
    assert!(prepare_run(
        &before,
        "session",
        &workspace,
        None,
        Some("after-restart"),
        Some(&source(3, 3, 400))
    )
    .is_err());
    assert_eq!(log.read("session").unwrap(), before);
}

#[test]
fn continuation_sources_refuse_recomputed_invalid_limits_clock_and_unbound_goal() {
    for file in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let log: Box<dyn LogStore> = if file {
            Box::new(FileLog::open(directory.path()).unwrap())
        } else {
            Box::new(MemoryLog::new())
        };
        let (workspace, facts) = seed(log.as_ref());
        assert!(prepare_run(
            &[],
            "session",
            &workspace,
            None,
            Some("no-goal"),
            Some(&source(2, 2, 100))
        )
        .is_err());
        let prepared = prepare_run(
            &facts,
            "session",
            &workspace,
            None,
            Some("auto-1"),
            Some(&source(2, 2, 100)),
        )
        .unwrap()
        .unwrap();
        log.append("session", &[prepared.pending.unwrap()], None)
            .unwrap();
        let before = log.read("session").unwrap();
        for case in 0..5 {
            let mut corrupt = before.clone();
            let fact = corrupt.last_mut().unwrap();
            let mut stored = RunFact::deserialize(&fact.payload).unwrap();
            let changed_source = stored.body.continuation.as_mut().unwrap();
            match case {
                0 => changed_source.policy.automatic_enabled = false,
                1 => changed_source.observed_unix_ms = 0,
                2 => changed_source.observed_unix_ms = changed_source.policy.deadline_unix_ms.get(),
                3 => changed_source.policy.max_slices = std::num::NonZeroU64::new(1).unwrap(),
                _ => {
                    changed_source.policy.max_no_progress_slices =
                        std::num::NonZeroU64::new(1).unwrap()
                }
            }
            stored.command_digest = digest(&prepared.definition, &stored.body).unwrap();
            fact.payload = serde_json::to_value(stored).unwrap();
            if case < 3 {
                assert!(fold_definitions(&corrupt, "session", &workspace, None, None).is_err());
            } else {
                let capped = source(1, 1, 200);
                assert!(prepare_run(
                    &corrupt,
                    "session",
                    &workspace,
                    None,
                    Some("next"),
                    Some(&capped)
                )
                .is_err());
            }
        }
        for clock in [0, 99, 10000] {
            assert!(prepare_run(
                &before,
                "session",
                &workspace,
                None,
                Some("next"),
                Some(&source(3, 3, clock))
            )
            .is_err());
        }
        for field in ["policy", "observed_unix_ms"] {
            let mut corrupt = before.clone();
            corrupt.last_mut().unwrap().payload["body"]["continuation"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(fold_definitions(&corrupt, "session", &workspace, None, None).is_err());
        }
        assert_eq!(log.read("session").unwrap(), before);
    }
}

#[test]
fn continuation_source_cannot_disappear_by_substituting_its_fact_kind() {
    let log = MemoryLog::new();
    let (workspace, _) = seed(&log);
    append_run(&log, &workspace, "auto-1", Some(&source(1, 1, 100)));
    let mut facts = log.read("session").unwrap();
    facts.last_mut().unwrap().kind = "host.audit".into();
    assert!(fold_definitions(&facts, "session", &workspace, None, None).is_err());
}

#[test]
fn continuation_source_preserves_ordinary_wire_and_is_refused_by_the_original_body_reader() {
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct OriginalRunBody {
        run_id: String,
        step_id: String,
        expected_goal_sequence: u64,
        plan_fact_sequence: u64,
    }

    let log = MemoryLog::new();
    let (workspace, _) = seed(&log);
    append_run(&log, &workspace, "ordinary", None);
    let body = log.read("session").unwrap().last().unwrap().payload["body"].clone();
    let old = OriginalRunBody::deserialize(&body).unwrap();
    assert_eq!(serde_json::to_value(old).unwrap(), body);
    append_run(&log, &workspace, "auto-1", Some(&source(1, 1, 100)));
    let body = log.read("session").unwrap().last().unwrap().payload["body"].clone();
    assert!(OriginalRunBody::deserialize(&body).is_err());
}
