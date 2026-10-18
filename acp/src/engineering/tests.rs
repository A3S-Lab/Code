#![cfg(test)]

use super::*;
use serde_json::json;

#[test]
fn explicit_policy_is_validated_and_omission_keeps_defaults() {
    assert!(policy_from_meta(None).unwrap().is_none());
    assert!(
        policy_from_meta(json!({"a3sEngineering":{"policy":"none"}}).as_object())
            .unwrap()
            .is_none()
    );
    assert!(policy_from_meta(json!({"unrelated":true}).as_object())
        .unwrap()
        .is_none());
    let policy =
        policy_from_meta(json!({"a3sEngineering":{"policy":"engineering-v1"}}).as_object())
            .unwrap()
            .unwrap();
    assert_eq!(policy.id(), EngineeringPolicy::V1.as_str());
    assert_eq!(policy.revision(), 1);
    assert_eq!(policy.instructions(), INSTRUCTIONS);
    assert!(INSTRUCTIONS.len() <= 4096);
    assert!(!INSTRUCTIONS.contains('\0'));
    assert_eq!(
        policy.digest().unwrap(),
        WorkPolicyV1::new("engineering-v1", 1, INSTRUCTIONS)
            .unwrap()
            .digest()
            .unwrap()
    );
}

#[test]
fn malformed_or_unknown_experiment_never_defaults() {
    for value in [
        Value::Null,
        json!(true),
        json!("engineering-v1"),
        json!([]),
        json!({}),
        json!({"policy":null}),
        json!({"policy":"unknown"}),
        json!({"policy":"engineering-v1","automatic":true}),
        json!({"policy":"engineering-v1","instructions":"replace"}),
    ] {
        assert!(
            policy_from_meta(json!({"a3sEngineering":value}).as_object()).is_err(),
            "{value}"
        );
    }
}

#[test]
fn a_goal_step_subject_binds_every_scope_field_and_a_session_target_keeps_the_old_subject() {
    let target = format!("sha256:{}", "a".repeat(64));
    assert_eq!(
        ReviewScope::SessionTarget.subject(&target).unwrap(),
        ReviewSubjectV1::opaque(REVIEW_TARGET_KIND, &target).unwrap(),
        "a Finding admitted before scopes were kept settles as session-target only"
    );
    let step =
        |goal: &str, goal_revision, plan_revision, step: &str, run: &str| ReviewScope::GoalStep {
            goal_id: goal.into(),
            goal_revision,
            plan_revision,
            step_id: step.into(),
            run_id: run.into(),
        };
    let bound = step("g", 1, 1, "s", "r").subject(&target).unwrap();
    let ReviewSubjectV1::Opaque { kind, .. } = &bound else {
        panic!("goal-step subject is opaque");
    };
    assert_eq!(kind, REVIEW_GOAL_STEP_KIND);
    assert_eq!(step("g", 1, 1, "s", "r").subject(&target).unwrap(), bound);
    for other in [
        step("h", 1, 1, "s", "r"),
        step("g", 2, 1, "s", "r"),
        step("g", 1, 2, "s", "r"),
        step("g", 1, 1, "t", "r"),
        step("g", 1, 1, "s", "q"),
    ] {
        assert_ne!(other.subject(&target).unwrap(), bound);
    }
    assert_ne!(
        step("g", 1, 1, "s", "r")
            .subject(&format!("sha256:{}", "b".repeat(64)))
            .unwrap(),
        bound
    );
}
