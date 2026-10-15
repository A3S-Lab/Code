#![cfg(test)]

use super::*;
use crate::external_observation::{ExternalObservationV1, RequiredAction};
use crate::goal_control::{
    preview_tick, GoalControlCommandV1, GoalPlanCommandV1, GoalTickDecisionV1,
};
use crate::planning::{Complexity, ExecutionPlan, Task};
use a3s_effect::{FileLog, LogStore, MemoryLog};

fn seed(log: &dyn LogStore) -> WorkspaceRef {
    let command = super::super::tests::command();
    let definition =
        prepare_definition(&[], "session", &command.workspace, None, &command).unwrap();
    log.append("session", &[definition.pending.unwrap()], None)
        .unwrap();
    let mut plan = ExecutionPlan::new("Independent steps", Complexity::Medium);
    plan.upsert_step(Task::new("a", "Ask the owner").with_success_criteria("Answer is used"));
    plan.upsert_step(Task::new("b", "Independent work").with_success_criteria("Done"));
    plan.upsert_step(
        Task::new("c", "After a")
            .with_success_criteria("Done")
            .with_dependencies(vec!["a".into()]),
    );
    let mut proposal = GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: "propose".into(),
        goal_id: "goal".into(),
        session_id: "session".into(),
        workspace: command.workspace.clone(),
        work_policy_digest: None,
        expected_goal_revision: 1,
        expected_plan_revision: None,
        expected_goal_sequence: 1,
        plan: Some(plan),
        proposal_sequence: None,
    };
    let facts = log.read("session").unwrap();
    let pending = prepare_plan(&facts, "session", &command.workspace, None, &proposal)
        .unwrap()
        .pending
        .unwrap();
    log.append("session", &[pending], None).unwrap();
    let view = current(log, &command.workspace);
    proposal.kind = GoalCommandKindV1::AcceptPlan;
    proposal.operation_id = "accept".into();
    proposal.expected_plan_revision = view.plan_revision;
    proposal.expected_goal_sequence = view.goal_sequence;
    proposal.plan = None;
    proposal.proposal_sequence = view.plan_fact_sequence;
    let facts = log.read("session").unwrap();
    let pending = prepare_plan(&facts, "session", &command.workspace, None, &proposal)
        .unwrap()
        .pending
        .unwrap();
    log.append("session", &[pending], None).unwrap();
    command.workspace
}

fn current(log: &dyn LogStore, workspace: &WorkspaceRef) -> DurableGoalDefinitionV1 {
    definition_from_facts(&log.read("session").unwrap(), "session", workspace, None)
        .unwrap()
        .unwrap()
}

fn wait(
    view: &DurableGoalDefinitionV1,
    op: &str,
    step: &str,
    condition: Option<GoalWaitConditionV1>,
    evidence: Option<GoalWaitEvidenceV1>,
) -> GoalWaitCommandV1 {
    GoalWaitCommandV1 {
        kind: if condition.is_some() {
            GoalCommandKindV1::DeclareWait
        } else {
            GoalCommandKindV1::ObserveWait
        },
        operation_id: op.into(),
        goal_id: "goal".into(),
        session_id: "session".into(),
        workspace: view.binding.workspace.clone(),
        work_policy_digest: None,
        expected_goal_revision: view.binding.goal_revision,
        expected_plan_revision: view.plan_revision,
        expected_goal_sequence: view.goal_sequence,
        step_id: step.into(),
        condition,
        evidence,
    }
}

/// Apply through the same prepare/append path as the facade; returns whether a fact was appended.
fn apply(
    log: &dyn LogStore,
    command: &GoalWaitCommandV1,
    now: u64,
) -> Result<bool, GoalControlError> {
    let facts = log.read("session").unwrap();
    let (recorded, prepared) =
        prepare_wait(&facts, "session", &command.workspace, None, command, now)?;
    if let Some(pending) = prepared.pending {
        log.append("session", &[pending], None).unwrap();
    }
    Ok(recorded)
}

fn tick(view: &DurableGoalDefinitionV1) -> GoalControlCommandV1 {
    GoalControlCommandV1 {
        kind: GoalCommandKindV1::Tick,
        operation_id: "tick".into(),
        goal_id: "goal".into(),
        session_id: "session".into(),
        workspace: view.binding.workspace.clone(),
        work_policy_digest: None,
        expected_goal_revision: view.binding.goal_revision,
        expected_plan_revision: view.plan_revision,
        expected_goal_sequence: view.goal_sequence,
    }
}

fn decision(log: &dyn LogStore, workspace: &WorkspaceRef) -> GoalTickDecisionV1 {
    let facts = log.read("session").unwrap();
    let view = current(log, workspace);
    preview_tick(
        &facts,
        "session",
        workspace,
        None,
        &tick(&view),
        false,
        None,
    )
    .unwrap()
}

fn observation(subject: &str, digest: &str) -> GoalWaitEvidenceV1 {
    GoalWaitEvidenceV1::SourceRevision {
        observation: ExternalObservationV1::new(
            "ci",
            subject,
            digest,
            "build is green",
            RequiredAction::None,
        )
        .unwrap(),
    }
}

fn prove_waits(log: &dyn LogStore) {
    let workspace = seed(log);
    let question = GoalWaitConditionV1::QuestionAnswer {
        question: "Which port should the service use?".into(),
    };
    let view = current(log, &workspace);
    assert!(apply(
        log,
        &wait(&view, "w-a", "a", Some(question.clone()), None),
        0
    )
    .unwrap());

    // A waits; independent B is the only ready step and the original Run binds it.
    let view = current(log, &workspace);
    assert_eq!(
        view.ready_steps()
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
    let facts = log.read("session").unwrap();
    let bound = prepare_run(&facts, "session", &workspace, None, Some("probe"), None)
        .unwrap()
        .unwrap();
    assert_eq!(bound.pending.unwrap().payload["body"]["step_id"], "b");
    assert_eq!(
        decision(log, &workspace),
        GoalTickDecisionV1::BudgetExhausted
    );

    // Out-of-plan, duplicate, mixed and unknown inputs refuse before any fact.
    let before = log.read("session").unwrap();
    for bad in [
        wait(&view, "w-z", "z", Some(question.clone()), None),
        wait(&view, "w-a2", "a", Some(question.clone()), None),
        wait(
            &view,
            "w-mix",
            "b",
            Some(question.clone()),
            Some(GoalWaitEvidenceV1::UtcDue {}),
        ),
        wait(&view, "w-none", "b", None, None),
    ] {
        assert!(apply(log, &bad, 1).is_err(), "{}", bad.operation_id);
    }
    let mut stale = wait(&view, "w-stale", "b", Some(question.clone()), None);
    stale.expected_goal_sequence -= 1;
    assert!(apply(log, &stale, 1).is_err());
    let wire =
        serde_json::to_value(wait(&view, "w-wire", "b", Some(question.clone()), None)).unwrap();
    for (field, value) in [
        (
            "condition",
            serde_json::json!({"kind": "webhook", "url": "x"}),
        ),
        (
            "condition",
            serde_json::json!({"kind": "utc_due", "due_unix_ms": 0}),
        ),
        (
            "evidence",
            serde_json::json!({"kind": "utc_due", "observed_unix_ms": 99}),
        ),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut input = wire.clone();
        input[field] = value;
        assert!(
            serde_json::from_value::<GoalWaitCommandV1>(input).is_err(),
            "{field}"
        );
    }
    let mut missing = wire.clone();
    missing.as_object_mut().unwrap().remove("evidence");
    assert!(serde_json::from_value::<GoalWaitCommandV1>(missing).is_err());
    assert_eq!(log.read("session").unwrap(), before);

    // B waits on time: nothing is ready, and the open question needs a person.
    let due = GoalWaitConditionV1::UtcDue {
        due_unix_ms: std::num::NonZeroU64::new(10_000).unwrap(),
    };
    assert!(apply(log, &wait(&view, "w-b", "b", Some(due), None), 0).unwrap());
    assert_eq!(decision(log, &workspace), GoalTickDecisionV1::NeedsInput);

    // Not due is quiet; due appends one clock fact and only makes B ready.
    let view = current(log, &workspace);
    let before = log.read("session").unwrap();
    let due_now = wait(&view, "o-b", "b", None, Some(GoalWaitEvidenceV1::UtcDue {}));
    assert!(!apply(log, &due_now, 9_999).unwrap());
    assert_eq!(log.read("session").unwrap(), before);
    assert!(apply(log, &due_now, 10_000).unwrap());
    let view = current(log, &workspace);
    let observed = view.waits[1].observed.as_ref().unwrap();
    assert_eq!(observed.observed_unix_ms, Some(10_000));
    assert_eq!(
        view.ready_steps()
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["b"]
    );
    assert!(view
        .plan
        .as_ref()
        .unwrap()
        .steps
        .iter()
        .all(|s| s.status == crate::planning::TaskStatus::Pending));

    // Exact replay is idempotent; changed content under the same operation conflicts.
    assert!(apply(log, &due_now, 1).unwrap());
    let mut changed = due_now.clone();
    changed.step_id = "a".into();
    assert!(apply(log, &changed, 10_001).is_err());

    // A later due wait cannot observe a host clock that moved backwards.
    let due_again = GoalWaitConditionV1::UtcDue {
        due_unix_ms: std::num::NonZeroU64::new(1).unwrap(),
    };
    assert!(apply(log, &wait(&view, "w-b2", "b", Some(due_again), None), 0).unwrap());
    let view = current(log, &workspace);
    assert!(apply(
        log,
        &wait(
            &view,
            "o-b2",
            "b",
            None,
            Some(GoalWaitEvidenceV1::UtcDue {})
        ),
        9_000
    )
    .is_err());
    assert!(apply(
        log,
        &wait(
            &view,
            "o-b2",
            "b",
            None,
            Some(GoalWaitEvidenceV1::UtcDue {})
        ),
        10_500
    )
    .unwrap());

    // Source revision: unchanged digest is quiet; foreign subject refuses; a new digest records.
    let view = current(log, &workspace);
    let source = GoalWaitConditionV1::SourceRevision {
        subject: "build".into(),
        after_digest: "red-1".into(),
    };
    assert!(apply(log, &wait(&view, "w-src", "b", Some(source), None), 0).unwrap());
    let view = current(log, &workspace);
    let before = log.read("session").unwrap();
    assert!(!apply(
        log,
        &wait(
            &view,
            "o-src",
            "b",
            None,
            Some(observation("build", "red-1"))
        ),
        0
    )
    .unwrap());
    assert!(apply(
        log,
        &wait(
            &view,
            "o-src",
            "b",
            None,
            Some(observation("deploy", "green-2"))
        ),
        0
    )
    .is_err());
    assert!(apply(
        log,
        &wait(
            &view,
            "o-src",
            "b",
            None,
            Some(GoalWaitEvidenceV1::ConfirmationAnswer { approved: true })
        ),
        0
    )
    .is_err());
    assert_eq!(log.read("session").unwrap(), before);
    assert!(apply(
        log,
        &wait(
            &view,
            "o-src",
            "b",
            None,
            Some(observation("build", "green-2"))
        ),
        0
    )
    .unwrap());

    // Answering A: kind mismatch and blank text refuse; the answer reaches A's Run context.
    let view = current(log, &workspace);
    let answer = |text: &str| {
        Some(GoalWaitEvidenceV1::QuestionAnswer {
            answer: text.into(),
        })
    };
    assert!(apply(
        log,
        &wait(&view, "o-a", "a", None, Some(GoalWaitEvidenceV1::UtcDue {})),
        20_000
    )
    .is_err());
    assert!(apply(log, &wait(&view, "o-a", "a", None, answer("  ")), 0).is_err());
    assert!(apply(log, &wait(&view, "o-a", "a", None, answer("port 8443")), 0).unwrap());
    let view = current(log, &workspace);
    assert_eq!(view.ready_steps()[0].id, "a");
    let context = render_run_context(&view, "run-a").unwrap();
    assert!(context.contains("port 8443") && context.contains("not approval"));
    // An old answer for an already observed wait cannot be admitted again.
    assert!(apply(
        log,
        &wait(&view, "o-a-late", "a", None, answer("port 9000")),
        0
    )
    .is_err());

    // Tampered history fails closed rather than projecting a different readiness.
    let facts = log.read("session").unwrap();
    let due_fact = facts
        .iter()
        .position(|fact| fact.payload["operation_id"] == "o-b")
        .unwrap();
    for (field, value) in [
        ("observed_unix_ms", serde_json::json!(9_999)),
        ("observed_unix_ms", serde_json::Value::Null),
        ("step_id", serde_json::json!("c")),
    ] {
        let mut corrupt = facts.clone();
        corrupt[due_fact].payload["body"][field] = value;
        assert!(
            definition_from_facts(&corrupt, "session", &workspace, None).is_err(),
            "{field}"
        );
    }
    let mut kind = facts.clone();
    kind[due_fact].kind = GoalFactKindV1::WaitDeclared.as_str().into();
    assert!(definition_from_facts(&kind, "session", &workspace, None).is_err());

    // No wait is admitted while the original Run association is unsettled.
    let view = current(log, &workspace);
    let facts = log.read("session").unwrap();
    let pending = prepare_run(&facts, "session", &workspace, None, Some("run-a"), None)
        .unwrap()
        .unwrap()
        .pending
        .unwrap();
    log.append("session", &[pending], None).unwrap();
    let view_after = current(log, &workspace);
    let error = apply(log, &wait(&view_after, "w-c", "c", Some(question), None), 0).unwrap_err();
    assert!(error.to_string().contains("receipt_unsettled"), "{error}");
    assert_eq!(view_after.waits, view.waits);
}

#[test]
fn step_waits_gate_only_their_step_and_quiet_observations_append_nothing() {
    prove_waits(&MemoryLog::new());
    let directory = tempfile::tempdir().unwrap();
    let before = {
        let log = FileLog::open(directory.path()).unwrap();
        prove_waits(&log);
        current(&log, &log_workspace())
    };
    let reopened = FileLog::open(directory.path()).unwrap();
    let after = current(&reopened, &log_workspace());
    assert_eq!(after.waits, before.waits);
    assert_eq!(after.goal_sequence, before.goal_sequence);
}

fn log_workspace() -> WorkspaceRef {
    super::super::tests::command().workspace
}

#[test]
fn a_new_plan_proposal_clears_waits_of_the_previous_revision() {
    let log = MemoryLog::new();
    let workspace = seed(&log);
    let view = current(&log, &workspace);
    let question = GoalWaitConditionV1::ConfirmationAnswer {
        request: "Deploy to staging?".into(),
    };
    assert!(apply(&log, &wait(&view, "w-a", "a", Some(question), None), 0).unwrap());
    let view = current(&log, &workspace);
    let proposal = GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: "replan".into(),
        goal_id: "goal".into(),
        session_id: "session".into(),
        workspace: workspace.clone(),
        work_policy_digest: None,
        expected_goal_revision: 1,
        expected_plan_revision: view.plan_revision,
        expected_goal_sequence: view.goal_sequence,
        plan: view.plan.clone(),
        proposal_sequence: None,
    };
    let facts = log.read("session").unwrap();
    let pending = prepare_plan(&facts, "session", &workspace, None, &proposal)
        .unwrap()
        .pending
        .unwrap();
    log.append("session", &[pending], None).unwrap();
    assert!(current(&log, &workspace).waits.is_empty());
}
