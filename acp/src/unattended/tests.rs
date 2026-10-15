use super::*;

fn digest(index: u32) -> String {
    format!("{index:064x}")
}

fn facts(gate_digest: Option<String>, tools: usize, failed: bool) -> AttemptFacts {
    AttemptFacts {
        gate_digest,
        tools,
        failed,
        last_error: None,
    }
}

#[test]
fn distinct_mutation_digests_do_not_consume_the_idle_retry_budget() {
    let mut bound = HashSet::new();
    let mut budget = HostAdmissionBudget::default();
    for index in 0..32 {
        assert_eq!(
            host_admission_step(
                &mut bound,
                &mut budget,
                &facts(Some(digest(index)), 3, true),
                true
            ),
            HostAdmissionStep::BindNewDigest,
        );
    }
    assert_eq!(bound.len(), 32);
    assert_eq!(budget.idle_used, 0);
    assert_eq!(budget.non_terminal_used, 0);
    assert_eq!(budget.unproven_stops, 0);
    assert_eq!(
        host_admission_step(
            &mut bound,
            &mut budget,
            &facts(Some(digest(0)), 3, true),
            true
        ),
        HostAdmissionStep::SameDigestStillOpen,
    );
}

#[test]
fn a_tool_using_stop_after_a_bind_is_not_a_grade_until_the_cap() {
    let mut bound = HashSet::from([digest(7)]);
    let mut budget = HostAdmissionBudget::default();
    let stop = facts(None, 2, false);
    for expected in 1..=HOST_UNPROVEN_STOPS {
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, &stop, true),
            HostAdmissionStep::NudgeUnprovenStop,
        );
        assert_eq!(budget.unproven_stops, expected);
    }
    assert_eq!(
        host_admission_step(&mut bound, &mut budget, &stop, true),
        HostAdmissionStep::FreshContextReview,
    );
    assert!(budget.fresh_review_used);
    assert_eq!(budget.unproven_stops, 0);
    for _ in 1..=HOST_UNPROVEN_STOPS {
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, &stop, true),
            HostAdmissionStep::NudgeUnprovenStop,
        );
    }
    assert_eq!(
        host_admission_step(&mut bound, &mut budget, &stop, true),
        HostAdmissionStep::Finish,
    );
    assert_eq!(budget.idle_used, 0);
    assert_eq!(
        host_admission_step(
            &mut bound,
            &mut budget,
            &facts(Some(digest(7)), 2, true),
            true
        ),
        HostAdmissionStep::SameDigestStillOpen,
    );
}

#[test]
fn idle_and_non_terminal_retries_still_stop() {
    let mut bound = HashSet::new();
    let mut budget = HostAdmissionBudget::default();
    for _ in 0..(HOST_IDLE_ATTEMPTS - 1) {
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, &facts(None, 2, false), true),
            HostAdmissionStep::NudgeBeforeAnyGate,
        );
    }
    assert_eq!(
        host_admission_step(&mut bound, &mut budget, &facts(None, 2, false), true),
        HostAdmissionStep::Finish,
    );

    let mut bound = HashSet::from([digest(4)]);
    let mut budget = HostAdmissionBudget::default();
    for _ in 0..(HOST_IDLE_ATTEMPTS - 1) {
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, &facts(None, 0, true), false),
            HostAdmissionStep::NudgeIdleAfterBind,
        );
    }
    assert_eq!(
        host_admission_step(&mut bound, &mut budget, &facts(None, 0, false), true),
        HostAdmissionStep::Finish,
    );

    let mut bound = HashSet::new();
    let mut budget = HostAdmissionBudget::default();
    for _ in 0..(HOST_IDLE_ATTEMPTS - 1) {
        assert_eq!(
            host_admission_step(&mut bound, &mut budget, &facts(None, 4, true), false),
            HostAdmissionStep::RetryNonTerminal,
        );
    }
    assert_eq!(
        host_admission_step(&mut bound, &mut budget, &facts(None, 4, true), false),
        HostAdmissionStep::NonTerminalExhausted,
    );
}

#[test]
fn host_report_binds_a_passed_required_check_to_the_digest() {
    let digest = digest(9);
    let report = host_verification_report(&digest);
    assert_eq!(
        report.schema,
        a3s_code_core::verification::VERIFICATION_REPORT_SCHEMA
    );
    assert_eq!(report.effect_digest.as_deref(), Some(digest.as_str()));
    assert_eq!(report.status, VerificationStatus::Passed);
    assert!(report
        .checks
        .iter()
        .all(|check| check.required && check.status == VerificationStatus::Passed));
}

#[test]
fn host_notes_keep_the_bind_distinct_from_the_grader() {
    let digest = digest(1);
    let bound = bind_continuation(&digest);
    assert!(bound.contains(&digest));
    for note in [
        TASK_PROOF_NOTE,
        bound.as_str(),
        UNPROVEN_STOP_CONTINUATION,
        &first_prompt("task"),
    ] {
        assert!(note.contains("not the task grader"), "{note}");
    }
    assert!(first_prompt("the task").starts_with("the task\n\n"));
    assert!(FRESH_REVIEW_FOCUS.contains("Drop claims"));
    assert!(non_terminal_continuation("boom").contains("(boom)"));
}

#[test]
fn the_profile_answers_for_itself_with_every_capability_on() {
    let profile = Unattended::new(1_000, None).expect("profile");
    let skills = PathBuf::from("/skills");
    let options = profile
        .session_options("sid", "openai/gpt-test", vec![skills.clone()])
        .expect("options");
    assert_eq!(options.session_id.as_deref(), Some("sid"));
    assert_eq!(options.model.as_deref(), Some("openai/gpt-test"));
    assert!(options.confirmation_manager.is_some());
    assert!(options.permission_checker.is_none());
    assert!(options.confirmation_policy.is_none());
    // File memory resolves under the state root, so no store override.
    assert!(options.memory_store.is_none());
    assert!(options.skill_registry.is_none());
    assert_eq!(options.skill_dirs, vec![skills]);
    assert!(options.watch_skill_dirs.is_empty());
    assert!(options.code_intelligence);
    let delegation = options.auto_delegation.as_ref().expect("delegation");
    assert!(delegation.enabled && delegation.auto_parallel && delegation.allow_manual_delegation);
    assert_eq!(options.max_parallel_tasks, Some(MAX_PARALLEL_TASKS));
    assert!(options.session_store.is_none());
    assert_eq!(options.planning_mode, PlanningMode::Disabled);
    assert_eq!(options.reasoning_effort.as_deref(), Some("max"));
    assert_eq!(options.llm_api_timeout_ms, Some(LLM_TIMEOUT_MS));
    assert_eq!(options.max_tool_rounds, Some(MAX_TOOL_ROUNDS));
    assert_eq!(options.max_continuation_turns, Some(MAX_CONTINUATION_TURNS));
    assert_eq!(options.max_execution_time_ms, Some(1_000));
    let slots = options.prompt_slots.expect("prompt slots");
    assert_eq!(slots.style, Some(AgentStyle::GeneralPurpose));
    assert_eq!(slots.extra.as_deref(), Some(TASK_PROOF_NOTE));
    assert!(options.work_policy.is_none());
    assert!(Unattended::new(0, None).is_err());
}

#[test]
fn an_engineering_launch_puts_the_named_policy_on_every_session() {
    let profile = Unattended::new(1_000, None)
        .and_then(|profile| profile.with_engineering_policy("engineering-v1"))
        .expect("profile");
    let options = profile
        .session_options("sid", "m", Vec::new())
        .expect("options");
    let expected = crate::engineering::named_policy("engineering-v1").expect("policy");
    assert!(options.work_policy.as_ref() == Some(&expected));
    assert!(Unattended::new(1_000, None)
        .and_then(|profile| profile.with_engineering_policy("engineering-v9"))
        .is_err());
}

#[test]
fn a_goal_launch_bounds_one_slice_to_the_run_deadline() {
    let plain = Unattended::new(1_000, None).expect("profile");
    assert_eq!(plain.goal_control(5_000).expect("policy"), None);
    let goal = plain.with_goal_continuation();
    let policy = goal.goal_control(5_000).expect("policy").expect("goal");
    assert!(policy.automatic_enabled);
    assert_eq!(policy.max_slices.get(), 1);
    assert_eq!(policy.deadline_unix_ms.get(), 6_000);
    policy.check_admission(5_000).expect("admissible");
    let options = goal
        .session_options("sid", "m", Vec::new())
        .expect("options");
    assert_eq!(options.planning_mode, PlanningMode::Enabled);
}

#[tokio::test]
async fn auto_approval_never_parks_a_tool() {
    let profile = Unattended::new(1_000, None).expect("profile");
    let manager = profile
        .session_options("sid", "m", Vec::new())
        .expect("options")
        .confirmation_manager
        .expect("confirmation manager");
    assert!(!manager.requires_confirmation("bash").await);
}

#[test]
fn events_count_into_the_record_and_the_gate_digest_is_read_back() {
    let mut progress = RunProgress::new();
    let mut attempt = AttemptFacts::default();
    let gate = format!(
        "completion gate: workspace mutation {} (src/lib.rs) has no bound Passed verification",
        digest(3)
    );
    for event in [
        AgentEvent::TurnStart { turn: 0 },
        AgentEvent::ToolStart {
            id: "t1".into(),
            name: "bash".into(),
        },
        AgentEvent::ToolEnd {
            id: "t1".into(),
            name: "bash".into(),
            output: "ok".into(),
            exit_code: 0,
            metadata: Some(serde_json::json!({"artifact_uri": "a3s://one"})),
            error_kind: None,
            args: None,
        },
        AgentEvent::ToolEnd {
            id: "t2".into(),
            name: "read".into(),
            output: "denied".into(),
            exit_code: 1,
            metadata: None,
            error_kind: None,
            args: None,
        },
        AgentEvent::Error { message: gate },
    ] {
        note_event(&mut progress, &mut attempt, &event);
    }
    assert_eq!(progress.turns, 1);
    assert_eq!(progress.tool_calls, 2, "a start alone is not a call");
    assert_eq!(progress.successful_tool_calls, 1);
    assert_eq!(progress.artifact_evidence_count, 1);
    assert_eq!(progress.error_count, 1);
    assert!(!progress.terminal_event);
    assert_eq!(attempt.tools, 2);
    assert!(attempt.failed);
    assert_eq!(attempt.gate_digest, Some(digest(3)));

    let mut other = AttemptFacts::default();
    note_event(
        &mut progress,
        &mut other,
        &AgentEvent::Error {
            message: "provider returned HTTP 500".into(),
        },
    );
    assert!(other.failed);
    assert_eq!(other.gate_digest, None);
}
