#![cfg(test)]

use super::*;
use crate::goal_control::GoalPlanCommandV1;
use crate::planning::{Complexity, ExecutionPlan, Task};
use a3s_effect::{FileLog, LogStore, MemoryLog};

fn seed(log: &dyn LogStore) -> (WorkspaceRef, Vec<Fact>) {
    let command = super::super::tests::command();
    let definition =
        prepare_definition(&[], "session", &command.workspace, None, &command).unwrap();
    log.append("session", &[definition.pending.unwrap()], None)
        .unwrap();
    let mut plan = ExecutionPlan::new("Original plan", Complexity::Medium);
    plan.upsert_step(
        Task::new("a", "Read the original file").with_success_criteria("Exact source is read"),
    );
    plan.upsert_step(
        Task::new("b", "Verify the original source")
            .with_success_criteria("Original checks pass")
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
    let facts = log.read("session").unwrap();
    let view = definition_from_facts(&facts, "session", &command.workspace, None)
        .unwrap()
        .unwrap();
    proposal.kind = GoalCommandKindV1::AcceptPlan;
    proposal.operation_id = "accept".into();
    proposal.expected_plan_revision = view.plan_revision;
    proposal.expected_goal_sequence = view.goal_sequence;
    proposal.plan = None;
    proposal.proposal_sequence = view.plan_fact_sequence;
    let pending = prepare_plan(&facts, "session", &command.workspace, None, &proposal)
        .unwrap()
        .pending
        .unwrap();
    log.append("session", &[pending], None).unwrap();
    (command.workspace, log.read("session").unwrap())
}

fn prove_binding(log: &dyn LogStore) {
    let (workspace, facts) = seed(log);
    let prepared = prepare_run(&facts, "session", &workspace, None, Some("run-1"), None)
        .unwrap()
        .unwrap();
    let pending = prepared.pending.unwrap();
    assert_eq!(pending.payload["body"]["step_id"], "a");
    assert_eq!(pending.payload["body"]["expected_goal_sequence"], 3);
    log.append("session", &[pending], None).unwrap();
    let bound = log.read("session").unwrap();
    let view = definition_from_facts(&bound, "session", &workspace, None)
        .unwrap()
        .unwrap();
    assert_eq!(view.goal_sequence, 4);
    assert!(view.plan_accepted);
    assert_eq!(view.state, GoalState::Active);
    assert!(view
        .plan
        .as_ref()
        .unwrap()
        .steps
        .iter()
        .all(|s| s.status == crate::planning::TaskStatus::Pending));
    assert!(
        prepare_run(&bound, "session", &workspace, None, Some("run-1"), None)
            .unwrap()
            .unwrap()
            .pending
            .is_none()
    );
    assert!(prepare_run(&bound, "session", &workspace, None, Some("create"), None).is_err());
    for (field, value) in [
        ("run_id", serde_json::json!("foreign-run")),
        ("step_id", serde_json::json!("b")),
        ("expected_goal_sequence", serde_json::json!(1)),
        ("plan_fact_sequence", serde_json::json!(1)),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut corrupt = bound.clone();
        corrupt.last_mut().unwrap().payload["body"][field] = value;
        if let Ok(mut stored) = RunFact::deserialize(&corrupt.last().unwrap().payload) {
            stored.command_digest = digest(&prepared.definition, &stored.body).unwrap();
            corrupt.last_mut().unwrap().payload = serde_json::to_value(stored).unwrap();
        }
        assert!(
            definition_from_facts(&corrupt, "session", &workspace, None).is_err(),
            "{field}"
        );
    }
    assert!(definition_from_facts(&bound, "foreign-session", &workspace, None).is_err());
    assert!(prepare_run(&facts, "session", &workspace, None, None, None).is_err());
    assert_eq!(log.read("session").unwrap(), bound);
}

#[test]
fn goal_run_binding_replays_and_reopens_original_memory_and_file_logs() {
    prove_binding(&MemoryLog::new());
    let directory = tempfile::tempdir().unwrap();
    let before = {
        let log = FileLog::open(directory.path()).unwrap();
        prove_binding(&log);
        log.read("session").unwrap()
    };
    let reopened = FileLog::open(directory.path())
        .unwrap()
        .read("session")
        .unwrap();
    assert_eq!(reopened, before);
    let workspace = super::super::tests::command().workspace;
    assert_eq!(
        definition_from_facts(&reopened, "session", &workspace, None)
            .unwrap()
            .unwrap()
            .goal_sequence,
        4
    );
}

#[test]
fn goal_tick_preview_uses_both_original_logs_without_appending_or_recovering() {
    fn prove(log: &dyn LogStore) {
        use crate::goal_control::{preview_tick, GoalControlCommandV1, GoalTickDecisionV1};
        let (workspace, facts) = seed(log);
        let view = definition_from_facts(&facts, "session", &workspace, None)
            .unwrap()
            .unwrap();
        let command = GoalControlCommandV1 {
            kind: GoalCommandKindV1::Tick,
            operation_id: "original-run".into(),
            goal_id: view.binding.goal_id,
            session_id: "session".into(),
            workspace: workspace.clone(),
            work_policy_digest: None,
            expected_goal_revision: view.binding.goal_revision,
            expected_plan_revision: view.plan_revision,
            expected_goal_sequence: view.goal_sequence,
        };
        assert_eq!(
            preview_tick(&facts, "session", &workspace, None, &command, false, None).unwrap(),
            GoalTickDecisionV1::BudgetExhausted
        );
        assert_eq!(
            preview_tick(&facts, "session", &workspace, None, &command, true, None).unwrap(),
            GoalTickDecisionV1::Reconcile
        );
        assert_eq!(log.read("session").unwrap(), facts);
        let run = prepare_run(
            &facts,
            "session",
            &workspace,
            None,
            Some("original-run"),
            None,
        )
        .unwrap()
        .unwrap();
        log.append("session", &[run.pending.unwrap()], None)
            .unwrap();
        let admitted = log.read("session").unwrap();
        assert_eq!(
            preview_tick(&admitted, "session", &workspace, None, &command, false, None).unwrap(),
            GoalTickDecisionV1::Reconcile,
            "an unmarked Run with no tool receipt still needs original reconciliation"
        );
        assert_eq!(log.read("session").unwrap(), admitted);
        for field in 0..3 {
            let mut changed = command.clone();
            match field {
                0 => changed.expected_goal_revision += 1,
                1 => changed.expected_plan_revision = Some(99),
                _ => changed.expected_goal_sequence += 1,
            }
            assert!(
                preview_tick(&admitted, "session", &workspace, None, &changed, false, None)
                    .is_err()
            );
        }
        let mut corrupt = admitted.clone();
        corrupt.last_mut().unwrap().payload["body"]["step_id"] = "foreign".into();
        assert!(
            preview_tick(&corrupt, "session", &workspace, None, &command, false, None).is_err()
        );
        let mut substituted = admitted.clone();
        substituted.last_mut().unwrap().kind = "host.audit".into();
        assert!(
            preview_tick(
                &substituted,
                "session",
                &workspace,
                None,
                &command,
                false,
                None
            )
            .is_err(),
            "a matching key and payload cannot substitute for the original typed Run fact"
        );
        assert_eq!(log.read("session").unwrap(), admitted);
    }
    prove(&MemoryLog::new());
    let root = tempfile::tempdir().unwrap();
    let before = {
        let log = FileLog::open(root.path()).unwrap();
        prove(&log);
        log.read("session").unwrap()
    };
    assert_eq!(
        FileLog::open(root.path()).unwrap().read("session").unwrap(),
        before
    );
}

#[test]
fn run_brief_keeps_refs_step_and_evidence_within_4kib_and_512_tokens() {
    let tokens = |text: &str| crate::llm::estimate_prompt_tokens(&[], Some(text), &[]);
    let (workspace, facts) = seed(&MemoryLog::new());
    let mut view = definition_from_facts(&facts, "session", &workspace, None)
        .unwrap()
        .unwrap();
    let brief = render_run_context(&view, "run-1").unwrap();
    assert!(!brief.contains("brief truncated") && tokens(&brief) <= 512);
    assert!(brief.contains(r#""accepted_steps":[],"#) && brief.contains(r#""remaining_steps":2"#));

    // The largest valid goal (CJK description, 32 criteria) cuts only the goal text.
    view.goal.description = "目".repeat(1365);
    view.goal.success_criteria = vec!["标准".repeat(170); 32];
    view.binding.non_goals = vec!["非目标".repeat(113); 32];
    view.plan.as_mut().unwrap().steps[0].status = crate::planning::TaskStatus::Completed;
    let brief = render_run_context(&view, "run-2").unwrap();
    assert!(
        brief.len() <= 4096 && tokens(&brief) <= 512,
        "{}",
        brief.len()
    );
    assert!(brief.ends_with("[brief truncated; the full view is the goal definition]"));
    let goal_at = brief.find("\ngoal: ").unwrap();
    for kept in [
        r#""run_id":"run-2""#,
        r#""id":"b""#,
        "Original checks pass",
        r#""accepted_steps":["a"]"#,
        r#""remaining_steps":1"#,
    ] {
        assert!(brief[..goal_at].contains(kept), "{kept}");
    }
}

mod continuation;
