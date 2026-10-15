#![cfg(test)]

use super::*;
use a3s_code_core::goal_control::{
    GoalControlCommandV1, GoalState, GoalWaitCommandV1, GoalWaitConditionV1, GoalWaitEvidenceV1,
};
use a3s_code_core::{AgentRunSpawn, GoalTickAdmission};

fn wait(
    session: &a3s_code_core::AgentSession,
    operation: &str,
    step: &str,
    condition: Option<GoalWaitConditionV1>,
    evidence: Option<GoalWaitEvidenceV1>,
) -> GoalWaitCommandV1 {
    let goal = session.goal_definition().unwrap().unwrap();
    GoalWaitCommandV1 {
        kind: if condition.is_some() {
            GoalCommandKindV1::DeclareWait
        } else {
            GoalCommandKindV1::ObserveWait
        },
        operation_id: operation.into(),
        goal_id: goal.binding.goal_id,
        session_id: goal.binding.session_id,
        workspace: goal.binding.workspace,
        work_policy_digest: goal.binding.work_policy_digest,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
        step_id: step.into(),
        condition,
        evidence,
    }
}

/// `padded` fills the description and non-goals to their admitted maximum.
fn create_goal(session: &a3s_code_core::AgentSession, workspace: &std::path::Path, padded: bool) {
    let mut description = "Verify the source and report the owner's release code".to_string();
    let mut non_goals = vec!["Do not change any file".to_string()];
    if padded {
        description.push_str(&" 背景说明。".repeat(250));
        non_goals.extend(vec!["不要修改无关内容。".repeat(37); 31]);
    }
    session
        .apply_goal_definition(GoalDefinitionCommandV1 {
            kind: GoalCommandKindV1::Create,
            import_source: None,
            operation_id: "create".into(),
            goal_id: "release".into(),
            session_id: session.session_id().into(),
            workspace: WorkspaceServices::local(workspace).workspace_ref().clone(),
            goal: AgentGoal::new(description).with_criteria(vec![
                "The current source is independently verified".into(),
                "The owner's release code is reported exactly".into(),
            ]),
            non_goals,
            scope_fact_sequences: vec![],
            work_policy_digest: None,
            expected_goal_revision: None,
            expected_plan_revision: None,
            expected_goal_sequence: None,
        })
        .unwrap();
}

/// Propose and accept `steps` through the original plan commands.
fn accept_plan(session: &a3s_code_core::AgentSession, id: &str, steps: Vec<Task>) {
    let goal = session.goal_definition().unwrap().unwrap();
    let mut plan = ExecutionPlan::new("Release check", Complexity::Simple);
    for step in steps {
        plan.upsert_step(step);
    }
    let mut proposal = GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: id.into(),
        goal_id: goal.binding.goal_id,
        session_id: goal.binding.session_id,
        workspace: goal.binding.workspace,
        work_policy_digest: None,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
        plan: Some(plan),
        proposal_sequence: None,
    };
    let proposed = session.apply_goal_plan(proposal.clone()).unwrap();
    proposal.kind = GoalCommandKindV1::AcceptPlan;
    proposal.operation_id = format!("accept-{id}");
    proposal.expected_plan_revision = proposed.plan_revision;
    proposal.expected_goal_sequence = proposed.goal_sequence;
    proposal.plan = None;
    proposal.proposal_sequence = proposed.plan_fact_sequence;
    session.apply_goal_plan(proposal).unwrap();
}

fn inspect_step() -> Task {
    Task::new(
        "inspect",
        "Read and independently verify source.txt, then quote its exact current value",
    )
    .with_success_criteria(
        "The current source value is independently verified and reported exactly",
    )
}

/// A last step no tool here can satisfy, so the steps before it never close the goal.
fn report_step(after: &str) -> Task {
    Task::new("report", "Report the owner's release code exactly")
        .with_dependencies(vec![after.into()])
        .with_success_criteria("The owner's release code is reported exactly")
}

async fn live_session(
    workspace: &std::path::Path,
    session_id: &str,
    value: &str,
    max_slices: u64,
    max_no_progress: u64,
    padded: bool,
) -> (
    Agent,
    a3s_code_core::AgentSession,
    Arc<compaction::UsageProbe>,
) {
    let opened = open_live(workspace, session_id, value, max_slices, max_no_progress).await;
    create_goal(&opened.1, workspace, padded);
    opened
}

/// The pinned provider session with trusted finite bounds, without creating a goal.
async fn open_live(
    workspace: &std::path::Path,
    session_id: &str,
    value: &str,
    max_slices: u64,
    max_no_progress: u64,
) -> (
    Agent,
    a3s_code_core::AgentSession,
    Arc<compaction::UsageProbe>,
) {
    let mut config = load_pinned_layer_c_config();
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 300_000;
    config.goal_control = a3s_code_core::CodeConfig::from_acl(&format!(
        r#"goal_control {{
            automatic_enabled = true
            max_slices = "{max_slices}"
            max_no_progress_slices = "{max_no_progress}"
            deadline_unix_ms = "{deadline}"
        }}"#
    ))
    .unwrap()
    .goal_control;
    config.memory = Some(a3s_code_core::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();
    let guard = Arc::new(compaction::UsageProbe::default());
    let mut permission = PermissionPolicy::new().allow("verify_source(*)");
    permission.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            workspace.display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id(session_id)
                    .with_planning_mode(PlanningMode::Enabled)
                    .with_goal_tracking(true)
                    .with_auto_compact(false)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_continuation(false)
                    .with_permission_policy(permission)
                    .with_budget_guard(guard.clone())
                    .with_max_tool_rounds(4)
                    .with_llm_api_timeout(180_000)
                    .with_temperature(0.0),
            ),
        )
        .await
        .unwrap();
    session
        .register_dynamic_tool(Arc::new(VerifySource(value.into())))
        .unwrap();
    (agent, session, guard)
}

async fn run_tick(session: &a3s_code_core::AgentSession, operation: &str) -> (String, f64) {
    let started = std::time::Instant::now();
    let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) = session
        .tick_goal(&tick_request(session, operation))
        .await
        .unwrap()
    else {
        panic!("{operation} must admit one Run");
    };
    tokio::time::timeout(std::time::Duration::from_secs(180), worker)
        .await
        .unwrap()
        .unwrap();
    let snapshot = session.run_snapshot(operation).await.unwrap();
    assert_eq!(snapshot.status, a3s_code_core::run::RunStatus::Completed);
    (
        snapshot.result_text.unwrap_or_default(),
        started.elapsed().as_secs_f64(),
    )
}

fn step_status(session: &a3s_code_core::AgentSession, id: &str) -> TaskStatus {
    let goal = session.goal_definition().unwrap().unwrap();
    goal.plan
        .unwrap()
        .steps
        .into_iter()
        .find(|s| s.id == id)
        .unwrap()
        .status
}

fn slice_steps(workspace: &std::path::Path, session_id: &str) -> Vec<String> {
    a3s_code_core::fact_control::read_workspace_facts(workspace, session_id)
        .unwrap()
        .into_iter()
        .filter(|f| f.kind == "a3s.goal.slice.admitted")
        .map(|f| f.payload["body"]["step_id"].as_str().unwrap().to_owned())
        .collect()
}

/// A question gates only its step: the independent step runs first, the open
/// question stops further model calls, and the recorded answer reaches the next Run.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn waits_real_independent_step_runs_and_the_answer_reaches_the_waiting_step() {
    let workspace = tempfile::tempdir().unwrap();
    let value = format!("VERIFIED-{}", uuid::Uuid::new_v4());
    let code = format!("RC-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    std::fs::write(workspace.path().join("source.txt"), &value).unwrap();
    let (agent, session, guard) =
        live_session(workspace.path(), "goal-wait-live", &value, 4, 4, false).await;
    accept_plan(
        &session,
        "plan",
        vec![
            Task::new(
                "release-code",
                "Report the release code the owner gave in the answered wait, exactly",
            )
            .with_success_criteria("The owner's release code is reported exactly"),
            inspect_step(),
        ],
    );
    let facts = || {
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "goal-wait-live")
            .unwrap()
    };
    let question = GoalWaitConditionV1::QuestionAnswer {
        question: "Which release code should be reported?".into(),
    };
    assert!(
        session
            .apply_goal_wait(wait(
                &session,
                "w-code",
                "release-code",
                Some(question),
                None
            ))
            .unwrap()
            .recorded
    );

    let (inspected, inspect_secs) = run_tick(&session, "wake-inspect").await;
    let inspect_calls = guard.0.lock().unwrap().len();
    eprintln!("GOAL-WAIT inspect={inspect_secs:.2}s calls={inspect_calls} result={inspected:?}");
    assert!(inspected.contains(&value));
    assert_eq!(step_status(&session, "inspect"), TaskStatus::Completed);
    assert_eq!(step_status(&session, "release-code"), TaskStatus::Pending);
    assert!(!inspected.contains(&code));

    let before = facts();
    assert!(matches!(
        session
            .tick_goal(&tick_request(&session, "wake-idle"))
            .await
            .unwrap(),
        GoalTickAdmission::Decided(GoalTickDecisionV1::NeedsInput)
    ));
    assert_eq!(
        guard.0.lock().unwrap().len(),
        inspect_calls,
        "an open question starts no model"
    );
    assert_eq!(facts(), before);

    let answer = GoalWaitEvidenceV1::QuestionAnswer {
        answer: format!("The release code is {code}."),
    };
    let observed = session
        .apply_goal_wait(wait(&session, "o-code", "release-code", None, Some(answer)))
        .unwrap();
    assert!(observed.recorded);
    let (reported, report_secs) = run_tick(&session, "wake-code").await;
    let calls = guard.0.lock().unwrap().clone();
    let total = calls.iter().map(|usage| usage.total_tokens).sum::<usize>();
    eprintln!(
        "GOAL-WAIT report={report_secs:.2}s governed_calls={} tokens={total} result={reported:?}",
        calls.len()
    );
    assert!(
        reported.contains(&code),
        "the next Run used the recorded answer"
    );
    assert_eq!(
        slice_steps(workspace.path(), "goal-wait-live"),
        ["inspect", "release-code"]
    );
    session.close().await;
    agent.close().await;
}

/// A step nobody can verify stalls; ticks then need input at zero cost until one
/// changed plan earns one budgeted slice. A pause during a real slice cancels it
/// through the original RunControl, an older wake runs nothing, and resume refunds nothing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn stall_real_one_replan_recovers_and_pause_settles_the_running_slice() {
    let workspace = tempfile::tempdir().unwrap();
    let value = format!("VERIFIED-{}", uuid::Uuid::new_v4());
    std::fs::write(workspace.path().join("source.txt"), &value).unwrap();
    let id = "goal-stall-live";
    let (agent, session, guard) = live_session(workspace.path(), id, &value, 3, 1, false).await;
    accept_plan(
        &session,
        "plan",
        vec![Task::new(
            "token",
            "Report the host-only deployment token that no tool in this session can read",
        )
        .with_success_criteria("The deployment token is independently verified")],
    );
    let (stuck, stuck_secs) = run_tick(&session, "wake-stuck").await;
    assert_eq!(step_status(&session, "token"), TaskStatus::Pending);
    let stuck_calls = guard.0.lock().unwrap().len();
    eprintln!("GOAL-STALL stuck={stuck_secs:.2}s calls={stuck_calls} result={stuck:?}");
    for op in ["wake-2", "wake-3"] {
        assert!(matches!(
            session
                .tick_goal(&tick_request(&session, op))
                .await
                .unwrap(),
            GoalTickAdmission::Decided(GoalTickDecisionV1::NeedsInput)
        ));
    }
    assert_eq!(
        guard.0.lock().unwrap().len(),
        stuck_calls,
        "stalled ticks start no model"
    );

    accept_plan(
        &session,
        "replan",
        vec![
            inspect_step(),
            Task::new(
                "summary",
                "Summarize the verified source value in one sentence",
            )
            .with_dependencies(vec!["inspect".into()])
            .with_success_criteria("The summary quotes the verified value"),
        ],
    );
    let (inspected, inspect_secs) = run_tick(&session, "wake-replan").await;
    let replan_calls = guard.0.lock().unwrap().len();
    eprintln!("GOAL-STALL replan={inspect_secs:.2}s calls={replan_calls} result={inspected:?}");
    assert!(inspected.contains(&value));
    assert_eq!(step_status(&session, "inspect"), TaskStatus::Completed);

    let old_wake = tick_request(&session, "wake-old");
    let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) = session
        .tick_goal(&tick_request(&session, "wake-summary"))
        .await
        .unwrap()
    else {
        panic!("progress re-opens the next step");
    };
    let started = std::time::Instant::now();
    let pause = |operation: &str| {
        let mut command: GoalControlCommandV1 = tick_request(&session, operation);
        command.kind = GoalCommandKindV1::Pause;
        command
    };
    // The Run may record its slice after these refs were read; the settled
    // control then conflicts and the host retries once with fresh refs.
    let paused = match session
        .control_goal(pause("pause"), std::time::Duration::from_secs(30))
        .await
    {
        Ok(paused) => paused,
        Err(error) => {
            eprintln!("GOAL-STALL pause retried after {error}");
            session
                .control_goal(pause("pause-retry"), std::time::Duration::from_secs(30))
                .await
                .unwrap()
        }
    };
    let pause_secs = started.elapsed().as_secs_f64();
    tokio::time::timeout(std::time::Duration::from_secs(60), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(paused.state, GoalState::Paused);
    assert_ne!(
        session.run_snapshot("wake-summary").await.unwrap().status,
        a3s_code_core::run::RunStatus::Completed
    );
    // The original owner closes the cancelled slice itself: interrupted, usage unknown.
    let slices = slice_steps(workspace.path(), id);
    let recorded = slices.len() == 3;
    assert_eq!(slices[..2], ["token", "inspect"]);
    let closeouts = a3s_code_core::fact_control::read_workspace_facts(workspace.path(), id)
        .unwrap()
        .into_iter()
        .filter(|f| f.kind == "a3s.goal.slice.settled")
        .collect::<Vec<_>>();
    assert_eq!(
        closeouts.len(),
        slices.len(),
        "every recorded slice is closed"
    );
    if recorded {
        let body = &closeouts[2].payload["body"];
        assert_eq!(body["run_id"], "wake-summary");
        assert_eq!(body["outcome"], "interrupted");
        assert!(
            body["usage"].is_null(),
            "a cancelled Run's usage stays unknown"
        );
    }
    let calls = guard.0.lock().unwrap().len();
    assert!(
        !matches!(
            session.tick_goal(&old_wake).await,
            Ok(GoalTickAdmission::Admitted(_))
        ),
        "an older wake executes nothing"
    );
    assert_eq!(
        session
            .preview_goal_tick(&tick_request(&session, "wake-paused"))
            .await
            .unwrap(),
        GoalTickDecisionV1::Paused
    );
    let mut resume: GoalControlCommandV1 = tick_request(&session, "resume");
    resume.kind = GoalCommandKindV1::Resume;
    assert_eq!(
        session
            .control_goal(resume, std::time::Duration::from_secs(30))
            .await
            .unwrap()
            .state,
        GoalState::Active
    );
    // An unrecorded slice consumed nothing; a recorded one is not refunded.
    assert_eq!(
        session
            .preview_goal_tick(&tick_request(&session, "wake-resumed"))
            .await
            .unwrap(),
        if recorded {
            GoalTickDecisionV1::BudgetExhausted
        } else {
            GoalTickDecisionV1::Advance
        },
        "resume refunds no opportunity"
    );
    assert_eq!(guard.0.lock().unwrap().len(), calls);
    let usage = guard.0.lock().unwrap().clone();
    eprintln!(
        "GOAL-STALL pause={pause_secs:.2}s slice_recorded={recorded} governed_calls={} tokens={}",
        usage.len(),
        usage.iter().map(|u| u.total_tokens).sum::<usize>()
    );
    session.close().await;
    agent.close().await;
}

/// The goal brief is bounded: a maximum-size goal (≈36KiB of description and
/// non-goals) adds at most the brief bound to each call, and the step still runs.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn brief_real_maximum_goal_stays_bounded_and_the_current_step_is_accepted() {
    let mut first_prompt = Vec::new();
    for padded in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let value = format!("VERIFIED-{}", uuid::Uuid::new_v4());
        std::fs::write(workspace.path().join("source.txt"), &value).unwrap();
        let (agent, session, guard) =
            live_session(workspace.path(), "goal-brief-live", &value, 2, 2, padded).await;
        accept_plan(
            &session,
            "plan",
            vec![inspect_step(), report_step("inspect")],
        );
        let (result, secs) = run_tick(&session, "wake-brief").await;
        let usage = guard.0.lock().unwrap().clone();
        eprintln!(
            "GOAL-BRIEF padded={padded} secs={secs:.2} calls={} first_prompt={} tokens={} result={result:?}",
            usage.len(),
            usage[0].prompt_tokens,
            usage.iter().map(|u| u.total_tokens).sum::<usize>()
        );
        assert_eq!(step_status(&session, "inspect"), TaskStatus::Completed);
        assert!(result.contains(&value));
        first_prompt.push(usage[0].prompt_tokens);
        session.close().await;
        agent.close().await;
    }
    // An unbounded brief would add well over 10k tokens here.
    assert!(
        first_prompt[1] <= first_prompt[0] + 1024,
        "{first_prompt:?}"
    );
}

#[path = "goal_settle.rs"]
mod goal_settle;

#[path = "goal_cumulative.rs"]
mod goal_cumulative;
