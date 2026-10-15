#![cfg(test)]

use super::automatic::{decided, now_ms, policy, request, slices};
use super::*;
use crate::agent_api::{AgentRunSpawn, GoalTickAdmission};
use crate::goal_control::{GoalState, GoalTickDecisionV1};

fn control(
    session: &AgentSession,
    kind: GoalCommandKindV1,
    operation: &str,
) -> GoalControlCommandV1 {
    let mut command = request(session, operation);
    command.kind = kind;
    command
}

/// A pause or cancel the goal refuses leaves the running Run alone.
#[tokio::test(flavor = "multi_thread")]
async fn refused_goal_control_does_not_cancel_the_running_run() {
    for kind in [GoalCommandKindV1::Pause, GoalCommandKindV1::Cancel] {
        let root = tempfile::tempdir().unwrap();
        let (agent, session, client) = fixture_with(
            root.path(),
            true,
            options("goal-run"),
            Some(policy(true, 1, 8, now_ms() + 600_000)),
        )
        .await;
        *client.before_response.lock().unwrap() = Some(Box::new(|call| {
            if call == 1 {
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        }));
        let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) = session
            .tick_goal(&request(&session, "wake-1"))
            .await
            .unwrap()
        else {
            panic!("the first tick admits one Run");
        };
        while client.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let mut wrong = control(&session, kind, "control");
        wrong.goal_id = "other-goal".into();
        assert!(session
            .control_goal(wrong, std::time::Duration::from_secs(5))
            .await
            .is_err());
        tokio::time::timeout(std::time::Duration::from_secs(10), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            session.run_snapshot("wake-1").await.unwrap().status,
            crate::run::RunStatus::Completed,
            "{kind:?}"
        );
        session.close().await;
        agent.close().await;
    }
}

/// Replaying an already recorded pause records nothing and leaves a later slice alone.
#[tokio::test(flavor = "multi_thread")]
async fn replayed_goal_control_does_not_cancel_a_later_slice() {
    let root = tempfile::tempdir().unwrap();
    let (agent, session, client) = fixture_with(
        root.path(),
        true,
        options("goal-run"),
        Some(policy(true, 1, 8, now_ms() + 600_000)),
    )
    .await;
    let grace = std::time::Duration::from_secs(5);
    let pause = control(&session, GoalCommandKindV1::Pause, "pause");
    session.control_goal(pause.clone(), grace).await.unwrap();
    session
        .control_goal(
            control(&session, GoalCommandKindV1::Resume, "resume"),
            grace,
        )
        .await
        .unwrap();
    *client.before_response.lock().unwrap() = Some(Box::new(|call| {
        if call == 1 {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }));
    let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) = session
        .tick_goal(&request(&session, "wake-1"))
        .await
        .unwrap()
    else {
        panic!("the resumed goal admits one Run");
    };
    while client.calls.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    session.control_goal(pause, grace).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        session.run_snapshot("wake-1").await.unwrap().status,
        crate::run::RunStatus::Completed
    );
    assert_ne!(
        session.goal_definition().unwrap().unwrap().state,
        GoalState::Paused,
        "the replay recorded nothing"
    );
    session.close().await;
    agent.close().await;
}

/// A pause or cancel during a running slice goes through the original RunControl;
/// a wake prepared earlier executes nothing, and resume refunds no opportunity.
#[tokio::test(flavor = "multi_thread")]
async fn goal_control_settles_the_running_slice_and_an_old_wake_executes_nothing() {
    for (kind, immediate) in [
        (GoalCommandKindV1::Pause, false),
        (GoalCommandKindV1::Cancel, false),
        (GoalCommandKindV1::Pause, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (agent, session, client) = fixture_with(
            root.path(),
            true,
            options("goal-run"),
            Some(policy(true, 1, 8, now_ms() + 600_000)),
        )
        .await;
        *client.before_response.lock().unwrap() = Some(Box::new(|call| {
            if call == 1 {
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        }));
        let old_wake = request(&session, "wake-old");
        let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) = session
            .tick_goal(&request(&session, "wake-1"))
            .await
            .unwrap()
        else {
            panic!("the first tick admits one Run");
        };
        // `immediate` pauses before the Run may have recorded its slice.
        while !immediate && client.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let grace = std::time::Duration::from_secs(5);
        let paused = match session
            .control_goal(control(&session, kind, "control"), grace)
            .await
        {
            Ok(paused) => paused,
            // The slice was recorded after the refs were read; the Run is settled.
            Err(_) if immediate => session
                .control_goal(control(&session, kind, "control-retry"), grace)
                .await
                .unwrap(),
            Err(error) => panic!("{kind:?}: {error}"),
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), worker)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            session.run_snapshot("wake-1").await.unwrap().status,
            crate::run::RunStatus::Completed,
            "{kind:?}"
        );
        // The original owner closes a cancelled slice too (outcome interrupted,
        // usage unknown); an older wake still executes nothing.
        let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
        let recorded = slices(root.path()).len();
        let unsettled = crate::goal_control::slice_unsettled(&facts);
        assert!(recorded == 1 || (immediate && recorded == 0), "{kind:?}");
        assert!(!unsettled, "{kind:?}: the owner closes its cancelled slice");
        let closeouts = facts
            .iter()
            .filter(|fact| fact.kind == "a3s.goal.slice.settled")
            .collect::<Vec<_>>();
        assert_eq!(closeouts.len(), recorded, "{kind:?}");
        for closeout in closeouts {
            assert_eq!(closeout.payload["body"]["run_id"], "wake-1");
            assert_eq!(closeout.payload["body"]["outcome"], "interrupted");
            assert!(
                closeout.payload["body"]["usage"].is_null(),
                "usage stays unknown"
            );
        }
        let calls = client.calls.load(Ordering::SeqCst);
        assert!(
            !matches!(
                session.tick_goal(&old_wake).await,
                Ok(GoalTickAdmission::Admitted(_))
            ),
            "{kind:?}: an older wake executes nothing"
        );
        if kind == GoalCommandKindV1::Cancel {
            assert_eq!(paused.state, GoalState::Cancelled);
            assert!(session
                .tick_goal(&request(&session, "wake-2"))
                .await
                .is_err());
        } else {
            assert_eq!(paused.state, GoalState::Paused);
            let reconcile_or = |decision| {
                if unsettled {
                    GoalTickDecisionV1::Reconcile
                } else {
                    decision
                }
            };
            assert_eq!(
                decided(&session, &request(&session, "wake-2")).await,
                reconcile_or(GoalTickDecisionV1::Paused)
            );
            let resumed = session
                .control_goal(
                    control(&session, GoalCommandKindV1::Resume, "resume"),
                    std::time::Duration::from_secs(5),
                )
                .await
                .unwrap();
            assert_eq!(resumed.state, GoalState::Active);
            // An unrecorded slice consumed nothing; a recorded one is not refunded.
            assert_eq!(
                decided(&session, &request(&session, "wake-3")).await,
                reconcile_or(if recorded == 1 {
                    GoalTickDecisionV1::BudgetExhausted
                } else {
                    GoalTickDecisionV1::Advance
                }),
                "resume re-checks bounds and refunds nothing"
            );
        }
        assert_eq!(client.calls.load(Ordering::SeqCst), calls, "{kind:?}");
        session.close().await;
        agent.close().await;
    }
}

fn revision(goal: &crate::goal_control::DurableGoalDefinitionV1) -> GoalPlanCommandV1 {
    let mut plan = crate::planning::ExecutionPlan::new(
        "Repair existing callers after the slice",
        crate::planning::Complexity::Medium,
    );
    plan.upsert_step(
        crate::planning::Task::new("recheck", "Recheck the original source")
            .with_success_criteria("Original source is rechecked"),
    );
    GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: "revise-plan".into(),
        goal_id: goal.binding.goal_id.clone(),
        session_id: goal.binding.session_id.clone(),
        workspace: goal.binding.workspace.clone(),
        work_policy_digest: goal.binding.work_policy_digest.clone(),
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
        plan: Some(plan),
        proposal_sequence: None,
    }
}

/// A plan revision while a slice runs is refused by the original Run
/// admission: the slice settles under its own plan and the next slice, after
/// reopen, runs under the revision.
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_revision_during_a_running_slice_waits_for_the_next_slice() {
    for file in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
        } else {
            Arc::new(MemorySessionStore::new())
        };
        let continuation = policy(true, 8, 8, now_ms() + 600_000);
        let (agent, session, client) = fixture_with(
            root.path(),
            true,
            options("goal-run").with_session_store(store.clone()),
            Some(continuation.clone()),
        )
        .await;
        *client.before_response.lock().unwrap() = Some(Box::new(|call| {
            if call == 1 {
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        }));
        let original = session.goal_definition().unwrap().unwrap();
        let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) = session
            .tick_goal(&request(&session, "wake-1"))
            .await
            .unwrap()
        else {
            panic!("the first tick admits one Run");
        };
        while client.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let read = || crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
        let running = session.goal_definition().unwrap().unwrap();
        let before = read();
        assert!(
            matches!(
                session.apply_goal_plan(revision(&running)),
                Err(CodeError::SessionBusy { .. })
            ),
            "file={file}"
        );
        assert_eq!(read(), before, "a refused revision writes nothing");
        tokio::time::timeout(std::time::Duration::from_secs(10), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            session.run_snapshot("wake-1").await.unwrap().status,
            crate::run::RunStatus::Completed
        );
        let settled = read()
            .into_iter()
            .filter(|fact| fact.kind == "a3s.goal.slice.settled")
            .collect::<Vec<_>>();
        assert_eq!(settled.len(), 1);
        assert_eq!(
            settled[0].payload["plan_revision"],
            serde_json::json!(original.plan_revision)
        );

        let mut command = revision(&session.goal_definition().unwrap().unwrap());
        let proposed = session.apply_goal_plan(command.clone()).unwrap();
        command.kind = GoalCommandKindV1::AcceptPlan;
        command.operation_id = "accept-revised-plan".into();
        command.expected_plan_revision = proposed.plan_revision;
        command.expected_goal_sequence = proposed.goal_sequence;
        command.proposal_sequence = proposed.plan_fact_sequence;
        command.plan = None;
        let revised = session.apply_goal_plan(command).unwrap().plan_revision;
        assert_ne!(revised, original.plan_revision);
        session.save().await.unwrap();
        session.close().await;
        agent.close().await;

        let agent = Agent::from_config(fixture_config(Some(continuation)))
            .await
            .unwrap();
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
        } else {
            store
        };
        let restored = agent
            .resume_session_async(
                "goal-run",
                options("goal-run")
                    .with_session_store(store)
                    .with_llm_client(client.clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            restored.goal_definition().unwrap().unwrap().plan_revision,
            revised
        );
        *client.before_response.lock().unwrap() = None;
        let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) = restored
            .tick_goal(&request(&restored, "wake-2"))
            .await
            .unwrap()
        else {
            panic!("the next tick admits a slice under the revised plan");
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), worker)
            .await
            .unwrap()
            .unwrap();
        let recorded = slices(root.path());
        assert_eq!(recorded.len(), 2);
        assert_eq!(
            recorded[0].payload["plan_revision"],
            serde_json::json!(original.plan_revision)
        );
        assert_eq!(
            recorded[1].payload["plan_revision"],
            serde_json::json!(revised)
        );
        restored.close().await;
        agent.close().await;
    }
}
