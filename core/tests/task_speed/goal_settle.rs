#![cfg(test)]

use super::*;

fn facts(workspace: &std::path::Path, id: &str) -> Vec<a3s_effect::Fact> {
    a3s_code_core::fact_control::read_workspace_facts(workspace, id).unwrap()
}

fn confirm_step() -> Task {
    Task::new(
        "confirm",
        "Verify source.txt again with the verification tool and quote its exact value",
    )
    .with_dependencies(vec!["inspect".into()])
    .with_success_criteria(
        "The current source value is independently verified and reported exactly",
    )
}

fn tokens(guard: &compaction::UsageProbe) -> (usize, usize) {
    let calls = guard.0.lock().unwrap();
    (
        calls.len(),
        calls.iter().map(|usage| usage.total_tokens).sum::<usize>(),
    )
}

/// The real owner closes a completed slice with exactly its governed usage. An
/// owner stopped after acceptance is closed once by another host without a
/// model call, and the next step then runs for real and closes normally.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn settlement_real_owner_closes_with_governed_usage_and_a_stopped_owner_reconciles_once() {
    let workspace = tempfile::tempdir().unwrap();
    let value = format!("VERIFIED-{}", uuid::Uuid::new_v4());
    std::fs::write(workspace.path().join("source.txt"), &value).unwrap();
    let id = "goal-settle-live";
    let (agent, session, guard) = live_session(workspace.path(), id, &value, 4, 4, false).await;
    accept_plan(
        &session,
        "plan",
        vec![inspect_step(), confirm_step(), report_step("confirm")],
    );
    let (inspected, inspect_secs) = run_tick(&session, "wake-inspect").await;
    assert!(inspected.contains(&value));
    assert_eq!(step_status(&session, "inspect"), TaskStatus::Completed);
    let (inspect_calls, inspect_tokens) = tokens(&guard);
    let closeout = facts(workspace.path(), id).pop().unwrap();
    assert_eq!(closeout.kind, "a3s.goal.slice.settled");
    let body = &closeout.payload["body"];
    assert_eq!(body["run_id"], "wake-inspect");
    assert_eq!(body["outcome"], "completed");
    assert_eq!(
        body["usage"]["total_tokens"], inspect_tokens,
        "the closeout carries the governed usage once"
    );
    let accepted = body["step_sequence"].as_u64().unwrap();
    session.close().await;
    agent.close().await;

    // The owner stops after acceptance, before its closeout.
    let log = a3s_effect::FileLog::open(a3s_code_core::session_folder::effect_log_dir(
        workspace.path(),
        id,
    ))
    .unwrap();
    log.truncate_after(
        &a3s_code_core::fact_control::thread_for_session(id),
        accepted,
    )
    .unwrap();
    drop(log);
    let (agent, session, guard) = open_live(workspace.path(), id, &value, 4, 4).await;
    assert_eq!(
        session
            .preview_goal_tick(&tick_request(&session, "wake-check"))
            .await
            .unwrap(),
        GoalTickDecisionV1::Reconcile
    );
    let started = std::time::Instant::now();
    assert_eq!(session.reconcile_goal_slices().unwrap(), 1);
    assert_eq!(session.reconcile_goal_slices().unwrap(), 0, "closes once");
    let reconcile_ms = started.elapsed().as_millis();
    let after = facts(workspace.path(), id);
    let body = &after.last().unwrap().payload["body"];
    assert_eq!(body["run_id"], "wake-inspect");
    assert_eq!(body["outcome"], "interrupted");
    assert!(body["usage"].is_null(), "unseen usage stays unknown");
    assert_eq!(body["step_sequence"], accepted);
    assert_eq!(
        after
            .iter()
            .filter(|f| f.kind == "a3s.goal.step.accepted")
            .count(),
        1
    );
    assert_eq!(tokens(&guard), (0, 0), "reconciliation calls no model");

    let (confirmed, confirm_secs) = run_tick(&session, "wake-confirm").await;
    assert!(confirmed.contains(&value));
    assert_eq!(step_status(&session, "confirm"), TaskStatus::Completed);
    let (confirm_calls, confirm_tokens) = tokens(&guard);
    let last = facts(workspace.path(), id);
    let body = &last.last().unwrap().payload["body"];
    assert_eq!(body["run_id"], "wake-confirm");
    assert_eq!(body["outcome"], "completed");
    assert_eq!(body["usage"]["total_tokens"], confirm_tokens);
    assert_eq!(
        last.iter()
            .filter(|f| f.kind == "a3s.goal.slice.settled")
            .count(),
        2
    );
    eprintln!(
        "GOAL-SETTLE inspect={inspect_secs:.2}s calls={inspect_calls} tokens={inspect_tokens} reconcile_ms={reconcile_ms} confirm={confirm_secs:.2}s calls={confirm_calls} tokens={confirm_tokens}"
    );
    session.close().await;
    agent.close().await;
}

/// A goal two real steps can meet. The last step's judgment covers the goal's
/// own criterion, its owner records completion, and later ticks run nothing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn completion_real_last_step_closes_the_goal_and_later_ticks_run_nothing() {
    let workspace = tempfile::tempdir().unwrap();
    let value = format!("VERIFIED-{}", uuid::Uuid::new_v4());
    std::fs::write(workspace.path().join("source.txt"), &value).unwrap();
    let id = "goal-complete-live";
    let (agent, session, guard) = open_live(workspace.path(), id, &value, 4, 4).await;
    session
        .apply_goal_definition(GoalDefinitionCommandV1 {
            kind: GoalCommandKindV1::Create,
            import_source: None,
            operation_id: "create".into(),
            goal_id: "verified-source".into(),
            session_id: id.into(),
            workspace: WorkspaceServices::local(workspace.path())
                .workspace_ref()
                .clone(),
            goal: AgentGoal::new("Verify the source twice and report its exact value")
                .with_criteria(vec![
                    "The current source value is independently verified and reported exactly"
                        .into(),
                ]),
            non_goals: vec!["Do not change any file".into()],
            scope_fact_sequences: vec![],
            work_policy_digest: None,
            expected_goal_revision: None,
            expected_plan_revision: None,
            expected_goal_sequence: None,
        })
        .unwrap();
    accept_plan(&session, "plan", vec![inspect_step(), confirm_step()]);
    let (inspected, inspect_secs) = run_tick(&session, "wake-inspect").await;
    assert!(inspected.contains(&value));
    assert_eq!(step_status(&session, "inspect"), TaskStatus::Completed);
    assert_eq!(
        session.goal_definition().unwrap().unwrap().state,
        GoalState::Active
    );
    let (_, inspect_tokens) = tokens(&guard);
    let (confirmed, confirm_secs) = run_tick(&session, "wake-confirm").await;
    assert!(confirmed.contains(&value));
    let (calls, total) = tokens(&guard);
    let goal = session.goal_definition().unwrap().unwrap();
    assert_eq!(
        goal.state,
        GoalState::Completed,
        "the last step closes the goal"
    );
    let all = facts(workspace.path(), id);
    let tail = all
        .iter()
        .rev()
        .take(3)
        .map(|fact| fact.kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        tail,
        [
            "a3s.goal.completed",
            "a3s.goal.slice.settled",
            "a3s.goal.step.accepted"
        ]
    );
    let accepted = &all[all.len() - 3];
    assert_eq!(accepted.payload["body"]["closes_goal"], true);
    let closeout = &all[all.len() - 2].payload["body"];
    assert_eq!(closeout["outcome"], "completed");
    assert_eq!(closeout["usage"]["total_tokens"], total - inspect_tokens);
    let completed = &all[all.len() - 1];
    assert_eq!(completed.payload["body"]["step_sequence"], accepted.seq);
    assert_eq!(goal.goal_sequence, completed.seq);

    assert_eq!(
        session
            .preview_goal_tick(&tick_request(&session, "wake-after"))
            .await
            .unwrap(),
        GoalTickDecisionV1::Complete
    );
    assert!(matches!(
        session
            .tick_goal(&tick_request(&session, "wake-after"))
            .await
            .unwrap(),
        GoalTickAdmission::Decided(GoalTickDecisionV1::Complete)
    ));
    assert_eq!(session.reconcile_goal_slices().unwrap(), 0);
    assert_eq!(
        tokens(&guard),
        (calls, total),
        "a completed goal runs nothing"
    );
    assert_eq!(facts(workspace.path(), id), all);
    session.close().await;
    agent.close().await;

    let (agent, session, guard) = open_live(workspace.path(), id, &value, 4, 4).await;
    assert_eq!(
        session.goal_definition().unwrap().unwrap().state,
        GoalState::Completed
    );
    assert_eq!(
        session
            .preview_goal_tick(&tick_request(&session, "wake-reopen"))
            .await
            .unwrap(),
        GoalTickDecisionV1::Complete
    );
    assert_eq!(tokens(&guard), (0, 0));
    eprintln!(
        "GOAL-COMPLETE inspect={inspect_secs:.2}s confirm={confirm_secs:.2}s calls={calls} tokens={total}"
    );
    session.close().await;
    agent.close().await;
}
