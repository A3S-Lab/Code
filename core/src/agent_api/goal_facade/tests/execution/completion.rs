#![cfg(test)]

use super::acceptance::{script, VerifySource};
use super::automatic::ACHIEVED;
use super::settlement::{complete_slice, facts, opts, seq_of, tick, SOURCE};
use super::*;
use crate::goal_control::GoalTickDecisionV1;

/// A host that runs the next ready step through the original send path.
async fn host(root: &std::path::Path) -> (Agent, AgentSession, Arc<GoalClient>) {
    let agent = Agent::from_config(fixture_config(None)).await.unwrap();
    let client = Arc::new(GoalClient::default());
    let session = agent
        .build_session(
            root.display().to_string(),
            client.clone(),
            &opts()
                .with_planning_mode(crate::PlanningMode::Enabled)
                .with_goal_tracking(true),
        )
        .unwrap();
    session
        .register_dynamic_tool(Arc::new(VerifySource(SOURCE.into())))
        .unwrap();
    (agent, session, client)
}

/// Accept `inspect`, then run `verify`, the plan's last step, with `judgment`.
async fn last_step(root: &std::path::Path, judgment: &str) -> Vec<String> {
    complete_slice(root).await;
    let (agent, session, client) = host(root).await;
    script(&client, judgment);
    session
        .send("Execute the admitted current step.", None)
        .await
        .unwrap();
    let inputs = client.inputs.lock().unwrap().clone();
    session.close().await;
    agent.close().await;
    inputs
}

fn kinds(facts: &[a3s_effect::Fact]) -> Vec<&str> {
    facts
        .iter()
        .filter(|fact| fact.kind.starts_with("a3s.goal."))
        .map(|fact| fact.kind.as_str())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_last_step_judges_the_goal_and_its_owner_records_completion_once() {
    let root = tempfile::tempdir().unwrap();
    let inputs = last_step(root.path(), ACHIEVED).await;
    let judge = &inputs[2];
    assert!(judge.contains("All original checks pass"));
    assert!(judge.contains("This is the plan's last step"));
    assert!(judge.contains("Repair existing callers"));
    assert!(
        judge.contains(
            "- inspect: Inspect the original source (criteria: Original source is checked)"
        ),
        "{judge}"
    );
    let all = facts(root.path());
    let tail = &kinds(&all)[kinds(&all).len() - 4..];
    assert_eq!(
        tail,
        [
            "a3s.goal.slice.admitted",
            "a3s.goal.step.accepted",
            "a3s.goal.slice.settled",
            "a3s.goal.completed"
        ]
    );
    let accepted = all
        .iter()
        .filter(|fact| fact.kind == "a3s.goal.step.accepted")
        .collect::<Vec<_>>();
    assert!(accepted[0].payload["body"].get("closes_goal").is_none());
    assert_eq!(accepted[1].payload["body"]["closes_goal"], true);
    let completed = all.last().unwrap();
    assert_eq!(completed.payload["body"]["step_sequence"], accepted[1].seq);
    assert_eq!(
        completed.payload["body"]["settle_sequence"],
        completed.seq - 1
    );

    let (agent, session, client) = host(root.path()).await;
    let goal = session.goal_definition().unwrap().unwrap();
    assert_eq!(goal.state, GoalState::Completed);
    assert_eq!(goal.goal_sequence, completed.seq);
    assert_eq!(
        session.preview_goal_tick(&tick(&session)).await.unwrap(),
        GoalTickDecisionV1::Complete
    );
    assert_eq!(session.reconcile_goal_slices().unwrap(), 0);
    let workspace = goal.binding.workspace.clone();
    let fold = |facts: &[a3s_effect::Fact]| {
        crate::goal_control::definition_from_facts(facts, "goal-run", &workspace, None)
    };
    for field in ["step_sequence", "settle_sequence"] {
        let mut forged = all.clone();
        forged.last_mut().unwrap().payload["body"][field] = serde_json::json!(1);
        assert!(fold(&forged).is_err(), "{field}");
    }
    let mut early = all[..all.len() - 2].to_vec();
    let mut copy = completed.clone();
    copy.seq = early.last().unwrap().seq + 1;
    early.push(copy);
    assert!(fold(&early).is_err(), "completion before the last closeout");
    let mut repeated = all.clone();
    let mut copy = completed.clone();
    copy.seq += 1;
    repeated.push(copy);
    assert!(fold(&repeated).is_err(), "one completion per goal");
    assert_eq!(facts(root.path()), all);
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    session.close().await;
    agent.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unmet_goal_criterion_keeps_the_last_step_open_and_the_goal_active() {
    let root = tempfile::tempdir().unwrap();
    last_step(
        root.path(),
        r#"{"achieved":false,"progress":0.9,"remaining_criteria":["All original checks pass"]}"#,
    )
    .await;
    let all = facts(root.path());
    assert_eq!(
        all.iter()
            .filter(|fact| fact.kind == "a3s.goal.step.accepted")
            .count(),
        1
    );
    assert!(!all.iter().any(|fact| fact.kind == "a3s.goal.completed"));
    assert_eq!(all.last().unwrap().kind, "a3s.goal.slice.settled");
    let (agent, session, _) = host(root.path()).await;
    let goal = session.goal_definition().unwrap().unwrap();
    assert_eq!(goal.state, GoalState::Active);
    assert_eq!(goal.ready_steps().first().unwrap().id, "verify");
    session.close().await;
    agent.close().await;
}

/// An earlier owner stopped before its closeout; a later host still runs the
/// last step. The goal stays open until that earlier slice is reconciled.
#[tokio::test(flavor = "multi_thread")]
async fn an_earlier_unclosed_slice_holds_completion_until_reconcile() {
    let root = tempfile::tempdir().unwrap();
    complete_slice(root.path()).await;
    let first = seq_of(&facts(root.path()), "a3s.goal.step.accepted");
    let log = a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
        root.path(),
        "goal-run",
    ))
    .unwrap();
    log.truncate_after(&crate::fact_control::thread_for_session("goal-run"), first)
        .unwrap();
    drop(log);
    let (agent, session, client) = host(root.path()).await;
    script(&client, ACHIEVED);
    session
        .send("Execute the admitted current step.", None)
        .await
        .unwrap();
    let all = facts(root.path());
    assert!(
        !all.iter().any(|fact| fact.kind == "a3s.goal.completed"),
        "{:?}",
        kinds(&all)
    );
    assert_eq!(
        session.preview_goal_tick(&tick(&session)).await.unwrap(),
        GoalTickDecisionV1::Reconcile
    );
    let calls = client.calls.load(Ordering::SeqCst);
    assert_eq!(session.reconcile_goal_slices().unwrap(), 2);
    let after = facts(root.path());
    assert_eq!(
        &kinds(&after)[kinds(&after).len() - 2..],
        ["a3s.goal.slice.settled", "a3s.goal.completed"]
    );
    assert_eq!(
        session.goal_definition().unwrap().unwrap().state,
        GoalState::Completed
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), calls, "no model call");
    session.close().await;
    agent.close().await;
}

/// The owner stops after accepting the last step, or after its closeout:
/// another host closes the slice and records completion once, with no model call.
#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_owner_after_the_last_acceptance_completes_on_reconcile() {
    let probe = tempfile::tempdir().unwrap();
    last_step(probe.path(), ACHIEVED).await;
    let reference = facts(probe.path());
    let accepted = reference
        .iter()
        .rfind(|fact| fact.kind == "a3s.goal.step.accepted")
        .unwrap()
        .seq;
    for (stop, appended) in [(accepted, 2), (accepted + 1, 1)] {
        let root = tempfile::tempdir().unwrap();
        last_step(root.path(), ACHIEVED).await;
        let log = a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
            root.path(),
            "goal-run",
        ))
        .unwrap();
        log.truncate_after(&crate::fact_control::thread_for_session("goal-run"), stop)
            .unwrap();
        drop(log);
        let (agent, session, client) = host(root.path()).await;
        assert_eq!(
            session.preview_goal_tick(&tick(&session)).await.unwrap(),
            GoalTickDecisionV1::Reconcile
        );
        assert_eq!(session.reconcile_goal_slices().unwrap(), appended);
        assert_eq!(session.reconcile_goal_slices().unwrap(), 0);
        let after = facts(root.path());
        assert_eq!(after.last().unwrap().kind, "a3s.goal.completed");
        assert_eq!(
            after.last().unwrap().payload["body"]["step_sequence"],
            accepted
        );
        assert_eq!(
            seq_of(&after, "a3s.goal.completed"),
            after.last().unwrap().seq
        );
        assert_eq!(
            session.goal_definition().unwrap().unwrap().state,
            GoalState::Completed
        );
        assert_eq!(
            session.preview_goal_tick(&tick(&session)).await.unwrap(),
            GoalTickDecisionV1::Complete
        );
        assert_eq!(client.calls.load(Ordering::SeqCst), 0);
        assert_eq!(session.recorded_usage_tokens(), (0, 0, 0));
        session.close().await;
        agent.close().await;
    }
}
