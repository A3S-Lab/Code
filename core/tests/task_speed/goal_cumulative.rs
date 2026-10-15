#![cfg(test)]

use super::*;

fn facts(workspace: &std::path::Path, id: &str) -> Vec<a3s_effect::Fact> {
    a3s_code_core::fact_control::read_workspace_facts(workspace, id).unwrap()
}

fn total(guard: &compaction::UsageProbe) -> usize {
    guard
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|usage| usage.total_tokens)
        .sum()
}

fn steps() -> Vec<Task> {
    vec![
        inspect_step(),
        Task::new(
            "confirm",
            "Verify source.txt again with the verification tool and quote its exact value",
        )
        .with_dependencies(vec!["inspect".into()])
        .with_success_criteria(
            "The current source value is independently verified and reported exactly",
        ),
        report_step("confirm"),
    ]
}

/// Finite automatic use is one lifetime across actual slices, an original
/// compaction, a new host and a goal revision. Every governed token is either a
/// slice closeout or the compaction, once; the bound then admits nothing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn cumulative_real_lifetime_survives_compaction_restart_and_revision() {
    let workspace = tempfile::tempdir().unwrap();
    let value = format!("VERIFIED-{}", uuid::Uuid::new_v4());
    std::fs::write(workspace.path().join("source.txt"), &value).unwrap();
    let id = "goal-cumulative-live";
    let (agent, session, guard) = live_session(workspace.path(), id, &value, 3, 3, false).await;
    accept_plan(&session, "plan", steps());
    let (first, first_secs) = run_tick(&session, "wake-1").await;
    assert!(first.contains(&value));
    let before = total(&guard);
    session.compact_conversation(None).await.unwrap();
    let compaction = total(&guard) - before;
    assert!(compaction > 0, "the original compaction is governed");
    let (second, second_secs) = run_tick(&session, "wake-2").await;
    assert!(second.contains(&value));
    let mut governed = total(&guard);
    session.close().await;
    agent.close().await;

    let (agent, session, guard) = open_live(workspace.path(), id, &value, 3, 3).await;
    let goal = session.goal_definition().unwrap().unwrap();
    session
        .apply_goal_definition(GoalDefinitionCommandV1 {
            kind: GoalCommandKindV1::Revise,
            import_source: None,
            operation_id: "revise".into(),
            goal_id: goal.binding.goal_id.clone(),
            session_id: id.into(),
            workspace: goal.binding.workspace.clone(),
            goal: AgentGoal::new(
                "Verify the current source again and report the owner's release code",
            )
            .with_criteria(goal.goal.success_criteria.clone()),
            non_goals: vec!["Do not change any file".into()],
            scope_fact_sequences: vec![],
            work_policy_digest: None,
            expected_goal_revision: Some(goal.binding.goal_revision),
            expected_plan_revision: goal.plan_revision,
            expected_goal_sequence: Some(goal.goal_sequence),
        })
        .unwrap();
    accept_plan(&session, "plan-2", steps());
    let (third, third_secs) = run_tick(&session, "wake-3").await;
    assert!(third.contains(&value));
    governed += total(&guard);
    let calls = guard.0.lock().unwrap().len();

    assert!(
        matches!(
            session
                .tick_goal(&tick_request(&session, "wake-4"))
                .await
                .unwrap(),
            GoalTickAdmission::Decided(GoalTickDecisionV1::BudgetExhausted)
        ),
        "three slices across compaction, restart and revision exhaust the lifetime"
    );
    assert!(
        !matches!(
            session.tick_goal(&tick_request(&session, "wake-3")).await,
            Ok(GoalTickAdmission::Admitted(AgentRunSpawn::Started { .. }))
        ),
        "a repeated operation starts nothing"
    );
    assert_eq!(
        guard.0.lock().unwrap().len(),
        calls,
        "the bound calls no model"
    );

    let all = facts(workspace.path(), id);
    let admitted = all
        .iter()
        .filter(|fact| fact.kind == "a3s.goal.slice.admitted")
        .collect::<Vec<_>>();
    assert_eq!(admitted.len(), 3);
    assert_ne!(
        admitted[0].payload["goal_revision"], admitted[2].payload["goal_revision"],
        "the last slice ran under the revised goal"
    );
    let closed = all
        .iter()
        .filter(|fact| fact.kind == "a3s.goal.slice.settled")
        .map(|fact| &fact.payload["body"])
        .collect::<Vec<_>>();
    assert_eq!(closed.len(), 3);
    let settled = closed
        .iter()
        .map(|body| {
            assert_eq!(body["outcome"], "completed");
            body["usage"]["total_tokens"].as_u64().unwrap() as usize
        })
        .sum::<usize>();
    assert_eq!(
        settled + compaction,
        governed,
        "every governed token is a slice closeout or the compaction, once"
    );
    eprintln!(
        "GOAL-CUMULATIVE slices={first_secs:.2}/{second_secs:.2}/{third_secs:.2}s settled={settled} compaction={compaction} governed={governed}"
    );
    session.close().await;
    agent.close().await;
}
