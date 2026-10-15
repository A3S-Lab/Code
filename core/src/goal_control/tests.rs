#![cfg(test)]

use super::*;
use crate::planning::{Complexity, Task};

fn binding() -> DurableGoalBindingV1 {
    DurableGoalBindingV1 {
        goal_id: "goal-1".into(),
        session_id: "session-1".into(),
        workspace: WorkspaceRef::new("workspace-1", "/workspace"),
        goal_revision: 1,
        non_goals: vec!["Do not change unrelated files.".into()],
        scope_fact_sequences: vec![1],
        authorization_source: ExecutionIdentityV1::derive(
            "a3s.test.request.v1",
            &"explicit request",
        )
        .unwrap(),
        work_policy_digest: None,
    }
}

fn step(id: &str) -> Task {
    Task::new(id, "Implement a bounded change").with_success_criteria("Original check passes")
}

fn plan() -> ExecutionPlan {
    let mut plan = ExecutionPlan::new("Fix both callers", Complexity::Medium);
    plan.upsert_step(step("a"));
    plan.upsert_step(step("b").with_dependencies(vec!["a".into()]));
    plan
}

#[test]
fn closed_sets_share_canonical_serde_and_explicit_parsing() {
    macro_rules! prove {
        ($name:ident; $($variant:ident),+) => {
            for value in [$($name::$variant),+] {
                assert_eq!($name::parse(value.as_str()).unwrap(), value);
                let wire = serde_json::to_value(value).unwrap();
                assert_eq!(wire, value.as_str());
                assert_eq!(serde_json::from_value::<$name>(wire).unwrap(), value);
            }
            for unknown in ["", "unknown", "ACTIVE", " active"] {
                assert!($name::parse(unknown).unwrap_err().to_string().contains(stringify!($name)));
                assert!(serde_json::from_value::<$name>(serde_json::json!(unknown)).is_err());
            }
        };
    }
    prove!(GoalState; Active, Waiting, Paused, Completed, Cancelled);
    prove!(GoalTickDecisionV1; Advance, Wait, NeedsInput, Paused, BudgetExhausted, Reconcile, Complete);
    prove!(GoalCommandKindV1; Create, Revise, ProposePlan, AcceptPlan, Pause, Resume, Cancel, Tick,
        DeclareWait, ObserveWait);
    prove!(GoalWaitKindV1; QuestionAnswer, ConfirmationAnswer, SourceRevision, UtcDue);
    prove!(GoalRefusal; NotFound, Conflict, WriterUnavailable, Waiting, Paused, BudgetExhausted,
        DeadlineExpired, ReceiptUnsettled, EvidenceStale, ConditionUnsupported, LegacyAmbiguous, HostDriverConflict);
    prove!(GoalFactKindV1; Created, Revised, PlanProposed, PlanAccepted, SliceAdmitted, SliceSettled,
        StepAccepted, WaitDeclared, WaitObserved, Paused, Resumed, Cancelled, Completed);
}

#[test]
fn binding_preserves_existing_workspace_and_source_types() {
    let original = binding();
    original.validate().unwrap();
    let decoded: DurableGoalBindingV1 =
        serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
    assert_eq!(decoded, original);
    assert_eq!(original.next_revision().unwrap(), 2);
    let mut maximum = original;
    maximum.goal_revision = u32::MAX;
    assert!(maximum.next_revision().is_err());
}

#[test]
fn new_binding_wire_rejects_unknown_fields_including_nested_sources() {
    for path in ["root", "workspace", "authorization_source"] {
        let mut wire = serde_json::to_value(binding()).unwrap();
        let target = if path == "root" {
            &mut wire
        } else {
            &mut wire[path]
        };
        target["permissions"] = serde_json::json!("allow");
        assert!(serde_json::from_value::<DurableGoalBindingV1>(wire).is_err());
    }
    let mut wire = serde_json::to_value(binding()).unwrap();
    wire.as_object_mut().unwrap().remove("work_policy_digest");
    assert!(serde_json::from_value::<DurableGoalBindingV1>(wire).is_err());
}

#[test]
fn binding_validation_refuses_invalid_references_without_rewriting_them() {
    let mut cases = Vec::new();
    let mut bad = binding();
    bad.goal_id = "中".repeat(43);
    cases.push(bad);
    let mut bad = binding();
    bad.session_id = "line\nbreak".into();
    cases.push(bad);
    let mut bad = binding();
    bad.goal_revision = 0;
    cases.push(bad);
    let mut bad = binding();
    bad.workspace.id = " ".into();
    cases.push(bad);
    let mut bad = binding();
    bad.workspace.display_root = "x".repeat(4097);
    cases.push(bad);
    let mut bad = binding();
    bad.non_goals = vec!["x".into(); 33];
    cases.push(bad);
    let mut bad = binding();
    bad.non_goals = vec!["中".repeat(342)];
    cases.push(bad);
    let mut bad = binding();
    bad.scope_fact_sequences = vec![1, 1];
    cases.push(bad);
    let mut bad = binding();
    bad.scope_fact_sequences = vec![0];
    cases.push(bad);
    let mut bad = binding();
    bad.scope_fact_sequences = (1..=33).collect();
    cases.push(bad);
    let mut bad = binding();
    bad.authorization_source.schema = "future".into();
    cases.push(bad);
    let mut bad = binding();
    bad.work_policy_digest = Some("sha256:BAD".into());
    cases.push(bad);
    for bad in cases {
        assert!(bad.validate().is_err(), "accepted {bad:?}");
    }
    let mut boundary = binding();
    boundary.goal_id = "中".repeat(42);
    boundary.non_goals = vec!["x".repeat(1024); 32];
    boundary.scope_fact_sequences = (1..=32).collect();
    boundary.work_policy_digest = Some(crate::content_digest::digest_bytes("test.v1", b"policy"));
    boundary.validate().unwrap();
}

#[test]
fn goal_content_is_original_type_and_client_progress_is_refused() {
    let goal = AgentGoal::new("Fix callers").with_criteria(vec!["Original check passes".into()]);
    validate_goal_definition(&goal).unwrap();
    let mut bad = goal.clone();
    bad.success_criteria.clear();
    assert!(validate_goal_definition(&bad).is_err());
    bad = goal.clone();
    bad.success_criteria = vec!["check".into(); 33];
    assert!(validate_goal_definition(&bad).is_err());
    bad = goal.clone();
    bad.description = "中".repeat(1366);
    assert!(validate_goal_definition(&bad).is_err());
    bad = goal.clone();
    bad.success_criteria = vec!["bad\0check".into()];
    assert!(validate_goal_definition(&bad).is_err());
    for progress in [1.0, f32::NAN, f32::INFINITY, -1.0] {
        bad = goal.clone();
        bad.progress = progress;
        assert!(validate_goal_definition(&bad).is_err());
    }
    bad = goal.clone();
    bad.achieved = true;
    assert!(validate_goal_definition(&bad).is_err());
    bad = goal;
    bad.achieved_at = Some(1);
    assert!(validate_goal_definition(&bad).is_err());
}

#[test]
fn valid_plan_returns_original_identity_without_accepting_display_statuses() {
    let mut plan = plan();
    plan.steps[0].status = TaskStatus::Completed;
    plan.steps[1].status = TaskStatus::Failed;
    let before = serde_json::to_value(&plan).unwrap();
    assert_eq!(
        validate_goal_plan(&plan).unwrap(),
        plan.definition_identity().unwrap()
    );
    assert_eq!(serde_json::to_value(&plan).unwrap(), before);
}

#[test]
fn plan_limits_and_missing_duplicate_self_or_cyclic_dependencies_are_refused() {
    let mut cases = Vec::new();
    let mut bad = plan();
    bad.steps.clear();
    cases.push(bad);
    let mut bad = plan();
    bad.steps.push(bad.steps[0].clone());
    cases.push(bad);
    let mut bad = plan();
    bad.steps[1].dependencies = vec!["missing".into()];
    cases.push(bad);
    let mut bad = plan();
    bad.steps[1].dependencies = vec!["b".into()];
    cases.push(bad);
    let mut bad = plan();
    bad.steps[1].dependencies = vec!["a".into(), "a".into()];
    cases.push(bad);
    let mut bad = plan();
    bad.steps[0].dependencies = vec!["b".into()];
    for step in &mut bad.steps {
        step.status = TaskStatus::Completed;
    }
    cases.push(bad);
    let mut bad = plan();
    bad.steps[0].success_criteria = None;
    cases.push(bad);
    let mut bad = plan();
    bad.steps[0].success_criteria = Some(" \n".into());
    cases.push(bad);
    let mut bad = plan();
    bad.steps[0].content = "x".repeat(4097);
    cases.push(bad);
    let mut bad = plan();
    bad.execution_context = Some("x".repeat(32769));
    cases.push(bad);
    let mut bad = plan();
    bad.required_tools = vec!["read_file".into(); 65];
    cases.push(bad);
    let mut bad = plan();
    bad.steps[0].tool = Some("bad\0tool".into());
    cases.push(bad);
    for bad in cases {
        assert!(validate_goal_plan(&bad).is_err(), "accepted {bad:?}");
    }
    let mut boundary = ExecutionPlan::new("Bounded chain", Complexity::VeryComplex);
    for i in 0..64 {
        let mut item = step(&format!("s{i}"));
        if i > 0 {
            item.dependencies.push(format!("s{}", i - 1));
        }
        boundary.upsert_step(item);
    }
    validate_goal_plan(&boundary).unwrap();
    boundary.upsert_step(step("s64"));
    assert!(validate_goal_plan(&boundary).is_err());
}

#[test]
fn graph_validation_matches_independent_reachability_for_every_four_node_graph() {
    let edges = (0..4)
        .flat_map(|a| (0..4).filter(move |b| *b != a).map(move |b| (a, b)))
        .collect::<Vec<_>>();
    for mask in 0..(1 << edges.len()) {
        let mut reachable = [[false; 4]; 4];
        let mut plan = ExecutionPlan::new("Graph", Complexity::Medium);
        for i in 0..4 {
            plan.add_step(step(&i.to_string()).with_status(TaskStatus::Completed));
        }
        for (bit, &(a, b)) in edges.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                reachable[a][b] = true;
                plan.steps[a].dependencies.push(b.to_string());
            }
        }
        for k in 0..4 {
            for a in 0..4 {
                for b in 0..4 {
                    reachable[a][b] |= reachable[a][k] && reachable[k][b];
                }
            }
        }
        let cyclic = (0..4).any(|i| reachable[i][i]);
        assert_eq!(
            validate_goal_plan(&plan).is_err(),
            cyclic,
            "graph mask {mask}"
        );
    }
}

#[test]
fn fact_keys_are_unambiguous_and_bound_to_goal_operation_and_kind() {
    let key = goal_fact_key("a/b", "c", GoalFactKindV1::Created).unwrap();
    assert_eq!(
        goal_fact_key("a/b", "c", GoalFactKindV1::Created).unwrap(),
        key
    );
    assert_ne!(
        goal_fact_key("a", "b/c", GoalFactKindV1::Created).unwrap(),
        key
    );
    assert_ne!(
        goal_fact_key("a/b", "c", GoalFactKindV1::Revised).unwrap(),
        key
    );
    assert_ne!(
        goal_fact_key("a/b", "d", GoalFactKindV1::Created).unwrap(),
        key
    );
    assert!(goal_fact_key("", "c", GoalFactKindV1::Created).is_err());
    assert!(goal_fact_key("a", "x".repeat(129).as_str(), GoalFactKindV1::Created).is_err());
    assert!(key.ends_with("/a3s.goal.created"));
}
