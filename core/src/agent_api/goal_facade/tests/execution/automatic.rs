#![cfg(test)]

use super::*;
use crate::agent_api::{AgentRunSpawn, GoalTickAdmission};
use crate::goal_control::{
    ContinuationSource, GoalContinuationPolicyV1, GoalControlCommandV1, GoalTickDecisionV1,
};

pub(super) const ACHIEVED: &str = r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#;

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub(super) fn policy(
    enabled: bool,
    max_slices: u64,
    stalled: u64,
    deadline: u64,
) -> GoalContinuationPolicyV1 {
    serde_json::from_value(serde_json::json!({
        "automatic_enabled": enabled,
        "max_slices": max_slices,
        "max_no_progress_slices": stalled,
        "deadline_unix_ms": deadline,
    }))
    .unwrap()
}

pub(super) fn request(session: &AgentSession, operation: &str) -> GoalControlCommandV1 {
    let goal = session.goal_definition().unwrap().unwrap();
    GoalControlCommandV1 {
        kind: GoalCommandKindV1::Tick,
        operation_id: operation.into(),
        goal_id: goal.binding.goal_id,
        session_id: goal.binding.session_id,
        workspace: goal.binding.workspace,
        work_policy_digest: goal.binding.work_policy_digest,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
    }
}

pub(super) fn slices(root: &std::path::Path) -> Vec<a3s_effect::Fact> {
    crate::fact_control::read_workspace_facts(root, "goal-run")
        .unwrap()
        .into_iter()
        .filter(|fact| fact.kind == "a3s.goal.slice.admitted")
        .collect()
}

pub(super) async fn decided(
    session: &AgentSession,
    tick: &GoalControlCommandV1,
) -> GoalTickDecisionV1 {
    match session.tick_goal(tick).await.unwrap() {
        GoalTickAdmission::Decided(decision) => decision,
        GoalTickAdmission::Admitted(_) => panic!("tick must not admit a Run here"),
    }
}

#[tokio::test]
async fn goal_tick_admits_one_original_run_and_persists_finite_use_across_reopen() {
    for file in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("source.txt"), "current source").unwrap();
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
        } else {
            Arc::new(MemorySessionStore::new())
        };
        let mut permission = crate::permissions::PermissionPolicy::new().allow("verify_source(*)");
        permission.default_decision = crate::permissions::PermissionDecision::Deny;
        let limits = policy(true, 1, 4, now_ms() + 600_000);
        let opts = options("goal-run")
            .with_session_store(store.clone())
            .with_permission_policy(permission);
        let (agent, session, client) =
            fixture_with(root.path(), true, opts, Some(limits.clone())).await;
        session
            .register_dynamic_tool(Arc::new(acceptance::VerifySource("current source".into())))
            .unwrap();
        acceptance::script(&client, ACHIEVED);
        let tick = request(&session, "wake-1");
        let before = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
        assert_eq!(
            session.preview_goal_tick(&tick).await.unwrap(),
            GoalTickDecisionV1::Advance
        );
        assert_eq!(
            crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
            before,
            "preview reserves nothing"
        );
        assert!(session.runs().await.is_empty());

        let (first, second) = tokio::join!(session.tick_goal(&tick), session.tick_goal(&tick));
        let mut started = 0;
        for result in [first, second] {
            match result {
                Ok(GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. })) => {
                    started += 1;
                    tokio::time::timeout(std::time::Duration::from_secs(10), worker)
                        .await
                        .unwrap()
                        .unwrap();
                }
                Ok(GoalTickAdmission::Admitted(AgentRunSpawn::Replayed { snapshot })) => {
                    assert_eq!(snapshot.id, tick.operation_id)
                }
                Ok(GoalTickAdmission::Decided(GoalTickDecisionV1::Reconcile))
                | Err(CodeError::SessionBusy { .. }) => {}
                Ok(GoalTickAdmission::Decided(other)) => panic!("unexpected decision {other:?}"),
                Err(error) => panic!("unexpected tick failure: {error}"),
            }
        }
        assert_eq!(started, 1, "one tick admits at most one original Run");
        assert_eq!(client.calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            session.run_snapshot("wake-1").await.unwrap().status,
            crate::run::RunStatus::Completed
        );
        let admitted = slices(root.path());
        assert_eq!(admitted.len(), 1);
        let source = &admitted[0].payload["body"]["continuation"];
        assert_eq!(source["policy"], serde_json::to_value(&limits).unwrap());
        assert!(source["observed_unix_ms"].as_u64().unwrap() > 0);
        let goal = session.goal_definition().unwrap().unwrap();
        assert_eq!(
            goal.plan.as_ref().unwrap().steps[0].status,
            crate::planning::TaskStatus::Completed
        );

        for _ in 0..3 {
            assert_eq!(
                decided(&session, &tick).await,
                GoalTickDecisionV1::Reconcile
            );
        }
        let mut stale = tick.clone();
        stale.operation_id = "wake-stale".into();
        assert!(session.tick_goal(&stale).await.is_err());
        let exhausted = request(&session, "wake-2");
        assert_eq!(
            decided(&session, &exhausted).await,
            GoalTickDecisionV1::BudgetExhausted,
            "the ready second step stays bounded by max_slices"
        );
        assert_eq!(client.calls.load(Ordering::SeqCst), 3);
        assert_eq!(slices(root.path()).len(), 1);

        session.save().await.unwrap();
        session.close().await;
        agent.close().await;
        let agent = Agent::from_config(fixture_config(Some(limits)))
            .await
            .unwrap();
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
        } else {
            store
        };
        let restored = agent
            .resume_session_async("goal-run", options("goal-run").with_session_store(store))
            .await
            .unwrap();
        assert_eq!(
            restored.preview_goal_tick(&exhausted).await.unwrap(),
            GoalTickDecisionV1::BudgetExhausted,
            "reopen does not reset finite use"
        );
        assert_eq!(
            decided(&restored, &tick).await,
            GoalTickDecisionV1::Reconcile
        );
        assert_eq!(slices(root.path()).len(), 1);
        restored.close().await;
        agent.close().await;
    }
}

#[tokio::test]
async fn goal_stream_tick_hands_the_slice_events_to_the_host_and_accepts_like_tick() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), "current source").unwrap();
    let mut permission = crate::permissions::PermissionPolicy::new().allow("verify_source(*)");
    permission.default_decision = crate::permissions::PermissionDecision::Deny;
    let limits = policy(true, 1, 4, now_ms() + 600_000);
    let opts = options("goal-run").with_permission_policy(permission);
    let (agent, session, client) = fixture_with(root.path(), true, opts, Some(limits)).await;
    session
        .register_dynamic_tool(Arc::new(acceptance::VerifySource("current source".into())))
        .unwrap();
    acceptance::script(&client, ACHIEVED);
    let tick = request(&session, "wake-1");
    let crate::agent_api::GoalTickStream::Started(stream) =
        session.stream_goal_tick(&tick).await.unwrap()
    else {
        panic!("an Advance tick starts one streamed slice");
    };
    assert_eq!(stream.run.as_ref().map(|run| run.id()), Some("wake-1"));
    let mut events = stream.events;
    let mut seen = Vec::new();
    while let Some(event) = events.recv().await {
        seen.push(event);
    }
    stream.worker.await.unwrap();
    assert!(seen.iter().any(|event| matches!(
        event,
        crate::agent::AgentEvent::ToolStart { name, .. } if name == "verify_source"
    )));
    assert!(matches!(
        seen.last(),
        Some(crate::agent::AgentEvent::End { .. })
    ));
    assert_eq!(client.calls.load(Ordering::SeqCst), 3);
    assert_eq!(slices(root.path()).len(), 1);
    let goal = session.goal_definition().unwrap().unwrap();
    assert_eq!(
        goal.plan.as_ref().unwrap().steps[0].status,
        crate::planning::TaskStatus::Completed
    );
    assert!(matches!(
        session.stream_goal_tick(&tick).await.unwrap(),
        crate::agent_api::GoalTickStream::Decided(GoalTickDecisionV1::Reconcile)
    ));
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_tick_without_progress_stops_at_the_stall_bound() {
    let root = tempfile::tempdir().unwrap();
    let limits = policy(true, 8, 1, now_ms() + 600_000);
    let (agent, session, client) =
        fixture_with(root.path(), true, options("goal-run"), Some(limits)).await;
    let tick = request(&session, "wake-1");
    let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) =
        session.tick_goal(&tick).await.unwrap()
    else {
        panic!("the first tick admits one Run");
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
    let calls = client.calls.load(Ordering::SeqCst);
    assert!(calls > 0);
    let goal = session.goal_definition().unwrap().unwrap();
    assert_eq!(
        goal.plan.as_ref().unwrap().steps[0].status,
        crate::planning::TaskStatus::Pending,
        "text alone is not accepted progress"
    );
    // Stalled ticks need input and never count as another failed slice.
    for op in ["wake-2", "wake-3", "wake-4"] {
        assert_eq!(
            decided(&session, &request(&session, op)).await,
            GoalTickDecisionV1::NeedsInput
        );
    }
    assert_eq!(client.calls.load(Ordering::SeqCst), calls);
    assert_eq!(slices(root.path()).len(), 1);

    // One changed accepted plan earns exactly one budgeted slice.
    super::waits::independent_plan(&session);
    let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) = session
        .tick_goal(&request(&session, "wake-replan"))
        .await
        .unwrap()
    else {
        panic!("the replan earns one Run");
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
    let calls = client.calls.load(Ordering::SeqCst);
    assert_eq!(
        decided(&session, &request(&session, "wake-after")).await,
        GoalTickDecisionV1::NeedsInput
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), calls);
    assert_eq!(slices(root.path()).len(), 2);
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_tick_refuses_without_trusted_enabled_unexpired_bounds() {
    for (case, limits) in [
        ("absent", None),
        ("disabled", Some(policy(false, 8, 8, now_ms() + 600_000))),
        ("expired", Some(policy(true, 8, 8, 1))),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (agent, session, client) =
            fixture_with(root.path(), true, options("goal-run"), limits).await;
        let tick = request(&session, "wake-1");
        let before = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
        match case {
            "expired" => assert!(session
                .tick_goal(&tick)
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("deadline")),
            _ => assert_eq!(
                decided(&session, &tick).await,
                GoalTickDecisionV1::BudgetExhausted,
                "{case}"
            ),
        }
        assert_eq!(client.calls.load(Ordering::SeqCst), 0, "{case}");
        assert!(session.runs().await.is_empty(), "{case}");
        assert_eq!(
            crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
            before,
            "{case}"
        );
        session.close().await;
        agent.close().await;
    }
}

#[tokio::test]
async fn goal_tick_occupied_session_reconciles_without_admission() {
    let root = tempfile::tempdir().unwrap();
    let limits = policy(true, 8, 8, now_ms() + 600_000);
    let (agent, session, client) =
        fixture_with(root.path(), true, options("goal-run"), Some(limits)).await;
    let tick = request(&session, "wake-1");
    let lease = session.run_admission.try_acquire("goal-run").unwrap();
    assert_eq!(
        decided(&session, &tick).await,
        GoalTickDecisionV1::Reconcile
    );
    drop(lease);
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    assert!(session.runs().await.is_empty());
    assert!(slices(root.path()).is_empty());
    session.close().await;
    agent.close().await;
}

/// The original FactRun re-validates the tick under its writer, so an origin
/// that no longer advances cannot reach the model even past the preview.
#[tokio::test]
async fn goal_tick_writer_revalidation_refuses_stale_or_invalid_origins() {
    for case in 0..4 {
        let root = tempfile::tempdir().unwrap();
        let limits = policy(true, 8, 8, now_ms() + 600_000);
        let (agent, session, client) =
            fixture_with(root.path(), true, options("goal-run"), Some(limits.clone())).await;
        let mut tick = request(&session, "wake-1");
        let mut source = ContinuationSource {
            policy: limits,
            observed_unix_ms: now_ms(),
        };
        match case {
            0 => tick.expected_goal_sequence += 1,
            1 => source.policy = policy(false, 8, 8, now_ms() + 600_000),
            2 => source.observed_unix_ms = source.policy.deadline_unix_ms.get(),
            _ => {
                let mut cancel = tick.clone();
                cancel.kind = GoalCommandKindV1::Cancel;
                cancel.operation_id = "cancel".into();
                session.apply_goal_control(cancel).unwrap();
            }
        }
        let before = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
        match crate::agent_api::conversation_runtime::spawn_goal_tick(
            &session, tick, source, "stale",
        )
        .await
        {
            Ok(AgentRunSpawn::Started { worker, .. }) => {
                tokio::time::timeout(std::time::Duration::from_secs(10), worker)
                    .await
                    .unwrap()
                    .unwrap();
                assert_ne!(
                    session.run_snapshot("wake-1").await.unwrap().status,
                    crate::run::RunStatus::Completed,
                    "case {case}"
                );
            }
            Ok(AgentRunSpawn::Replayed { .. }) => panic!("case {case} replayed"),
            Err(_) => {}
        }
        assert_eq!(client.calls.load(Ordering::SeqCst), 0, "case {case}");
        assert!(slices(root.path()).is_empty(), "case {case}");
        assert_eq!(
            crate::fact_control::read_workspace_facts(root.path(), "goal-run")
                .unwrap()
                .into_iter()
                .filter(|fact| fact.kind.starts_with("a3s.goal."))
                .count(),
            before
                .iter()
                .filter(|fact| fact.kind.starts_with("a3s.goal."))
                .count(),
            "case {case}"
        );
        session.close().await;
        agent.close().await;
    }
}

#[tokio::test]
async fn goal_tick_refuses_a_policy_above_the_admission_ceilings_without_a_run() {
    for limits in [
        policy(true, 1_001, 4, now_ms() + 600_000),
        policy(true, 1, 4, now_ms() + 31 * 24 * 60 * 60 * 1_000),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (_agent, session, client) =
            fixture_with(root.path(), true, options("goal-run"), Some(limits)).await;
        let tick = request(&session, "wake-over-ceiling");
        assert!(session.preview_goal_tick(&tick).await.is_err());
        assert!(!matches!(
            session.tick_goal(&tick).await,
            Ok(GoalTickAdmission::Admitted(_))
        ));
        assert!(slices(root.path()).is_empty());
        assert!(session.runs().await.is_empty());
        assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    }
}
