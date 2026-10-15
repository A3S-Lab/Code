#![cfg(test)]

use super::*;

fn policy() -> GoalContinuationPolicyV1 {
    serde_json::from_value(serde_json::json!({
        "automatic_enabled": true,
        "max_slices": 8,
        "max_no_progress_slices": 3,
        "deadline_unix_ms": 1000,
    }))
    .unwrap()
}

#[test]
fn continuation_policy_requires_every_explicit_finite_field() {
    let original = serde_json::to_value(policy()).unwrap();
    for field in [
        "automatic_enabled",
        "max_slices",
        "max_no_progress_slices",
        "deadline_unix_ms",
    ] {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<GoalContinuationPolicyV1>(missing).is_err());
        let mut null = original.clone();
        null[field] = serde_json::Value::Null;
        assert!(serde_json::from_value::<GoalContinuationPolicyV1>(null).is_err());
    }
    for field in ["max_slices", "max_no_progress_slices", "deadline_unix_ms"] {
        for value in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("100"),
            serde_json::json!(true),
        ] {
            let mut invalid = original.clone();
            invalid[field] = value;
            assert!(serde_json::from_value::<GoalContinuationPolicyV1>(invalid).is_err());
        }
        let overflow = serde_json::to_string(&original).unwrap().replace(
            &format!("\"{field}\":{}", original[field]),
            &format!("\"{field}\":18446744073709551616"),
        );
        assert!(serde_json::from_str::<GoalContinuationPolicyV1>(&overflow).is_err());
    }
    let mut unknown = original.clone();
    unknown["reset_on_replan"] = serde_json::json!(true);
    assert!(serde_json::from_value::<GoalContinuationPolicyV1>(unknown).is_err());
    assert_eq!(
        serde_json::from_value::<GoalContinuationPolicyV1>(original).unwrap(),
        policy()
    );
}

#[test]
fn continuation_limits_refuse_disabled_expired_unknown_clock_and_exhausted_counts() {
    let mut limits = policy();
    assert!(limits.check_limits(999, 7, 2).is_ok());
    for (now, spent, stalled, expected) in [
        (1000, 0, 0, GoalRefusal::DeadlineExpired),
        (1001, 0, 0, GoalRefusal::DeadlineExpired),
        (999, 8, 0, GoalRefusal::BudgetExhausted),
        (999, 9, 0, GoalRefusal::BudgetExhausted),
        (999, 2, 3, GoalRefusal::BudgetExhausted),
        (999, 7, 4, GoalRefusal::BudgetExhausted),
    ] {
        assert!(matches!(
            limits.check_limits(now, spent, stalled),
            Err(GoalControlError::Refused { refusal, .. }) if refusal == expected
        ));
    }
    assert!(matches!(
        limits.check_limits(0, 0, 0),
        Err(GoalControlError::Invalid {
            field: "now_unix_ms",
            ..
        })
    ));
    limits.automatic_enabled = false;
    assert!(matches!(
        limits.check_limits(999, 0, 0),
        Err(GoalControlError::Refused {
            refusal: GoalRefusal::BudgetExhausted,
            ..
        })
    ));
}

#[test]
fn continuation_limits_keep_u64_boundaries_without_saturation_or_wrap() {
    let limits = GoalContinuationPolicyV1 {
        automatic_enabled: true,
        max_slices: NonZeroU64::new(u64::MAX).unwrap(),
        max_no_progress_slices: NonZeroU64::new(u64::MAX).unwrap(),
        deadline_unix_ms: NonZeroU64::new(u64::MAX).unwrap(),
    };
    assert!(limits
        .check_limits(u64::MAX - 1, u64::MAX - 1, u64::MAX - 1)
        .is_ok());
    assert!(limits.check_limits(u64::MAX - 1, u64::MAX, 0).is_err());
    assert!(limits.check_limits(u64::MAX - 1, 0, u64::MAX).is_err());
    assert!(limits.check_limits(u64::MAX, 0, 0).is_err());
    let encoded = serde_json::to_string(&limits).unwrap();
    assert_eq!(
        serde_json::from_str::<GoalContinuationPolicyV1>(&encoded).unwrap(),
        limits
    );
}

#[test]
fn a_new_policy_is_capped_at_1000_slices_and_30_days_but_replay_is_not() {
    let mut limits = policy();
    limits.max_slices = NonZeroU64::new(MAX_GOAL_SLICES).unwrap();
    limits.deadline_unix_ms = NonZeroU64::new(1 + MAX_GOAL_HORIZON_MS).unwrap();
    assert!(limits.check_admission(1).is_ok());

    limits.max_slices = NonZeroU64::new(MAX_GOAL_SLICES + 1).unwrap();
    assert!(matches!(
        limits.check_admission(1),
        Err(GoalControlError::Invalid {
            field: "max_slices",
            ..
        })
    ));
    assert!(limits.check_limits(1, 0, 0).is_ok());

    limits.max_slices = NonZeroU64::new(MAX_GOAL_SLICES).unwrap();
    limits.deadline_unix_ms = NonZeroU64::new(2 + MAX_GOAL_HORIZON_MS).unwrap();
    assert!(matches!(
        limits.check_admission(1),
        Err(GoalControlError::Invalid {
            field: "deadline_unix_ms",
            ..
        })
    ));
    assert!(limits.check_admission(2).is_ok());
}
