#![cfg(test)]

use super::*;
use crate::goal_control::{GoalControlCommandV1, GoalTickDecisionV1};

fn request(session: &AgentSession) -> GoalControlCommandV1 {
    let goal = session.goal_definition().unwrap().unwrap();
    GoalControlCommandV1 {
        kind: GoalCommandKindV1::Tick,
        operation_id: "wake-1".into(),
        goal_id: goal.binding.goal_id,
        session_id: goal.binding.session_id,
        workspace: goal.binding.workspace,
        work_policy_digest: goal.binding.work_policy_digest,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
    }
}

#[tokio::test]
async fn goal_tick_exact_operation_tracks_the_original_run_across_replan_cancel_and_reopen() {
    for file in [false, true] {
        for policy in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let storage = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("source.txt"), "current source").unwrap();
            let store: Arc<dyn SessionStore> = if file {
                Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
            } else {
                Arc::new(MemorySessionStore::new())
            };
            let mut permission =
                crate::permissions::PermissionPolicy::new().allow("verify_source(*)");
            permission.default_decision = crate::permissions::PermissionDecision::Deny;
            let mut opts = options("goal-run")
                .with_session_store(store.clone())
                .with_permission_policy(permission);
            if policy {
                opts = opts.with_work_policy(
                    crate::WorkPolicyV1::new("engineering", 1, "Reuse the original mechanisms.")
                        .unwrap(),
                );
            }
            let (agent, session, client) = fixture(root.path(), true, opts).await;
            session
                .register_dynamic_tool(Arc::new(acceptance::VerifySource("current source".into())))
                .unwrap();
            acceptance::script(
                &client,
                r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#,
            );
            let tick = request(&session);
            assert_eq!(
                session.preview_goal_tick(&tick).await.unwrap(),
                GoalTickDecisionV1::BudgetExhausted
            );
            let prompt = "Execute the current admitted step using fresh evidence.";
            let (first, second) = tokio::join!(
                session.spawn_run_with_id(&tick.operation_id, prompt),
                session.spawn_run_with_id(&tick.operation_id, prompt),
            );
            let mut started = 0;
            for result in [first, second] {
                match result {
                    Ok(crate::agent_api::AgentRunSpawn::Started { worker, .. }) => {
                        started += 1;
                        tokio::time::timeout(std::time::Duration::from_secs(10), worker)
                            .await
                            .unwrap()
                            .unwrap();
                    }
                    Ok(crate::agent_api::AgentRunSpawn::Replayed { snapshot }) => {
                        assert_eq!(snapshot.id, tick.operation_id)
                    }
                    Err(CodeError::SessionBusy { .. }) => {}
                    Err(error) => panic!("unexpected exact admission failure: {error}"),
                }
            }
            assert_eq!(started, 1);
            assert_eq!(client.calls.load(Ordering::SeqCst), 3);
            assert_eq!(
                session
                    .run_snapshot(&tick.operation_id)
                    .await
                    .unwrap()
                    .status,
                crate::run::RunStatus::Completed
            );
            let goal = session.goal_definition().unwrap().unwrap();
            assert_eq!(
                goal.plan.as_ref().unwrap().steps[0].status,
                crate::planning::TaskStatus::Completed
            );
            assert_ne!(goal.goal_sequence, tick.expected_goal_sequence);
            let before =
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
            assert_eq!(
                before
                    .iter()
                    .filter(|f| f.kind == "a3s.goal.slice.admitted")
                    .count(),
                1
            );
            assert_eq!(before.iter().filter(|f| f.kind == "tool.result").count(), 1);
            for _ in 0..4 {
                assert_eq!(
                    session.preview_goal_tick(&tick).await.unwrap(),
                    GoalTickDecisionV1::Reconcile
                );
                assert!(session
                    .spawn_run_with_id(&tick.operation_id, prompt)
                    .await
                    .unwrap()
                    .replayed());
            }
            let observer_agent = Agent::from_config(test_config()).await.unwrap();
            let observer_client = Arc::new(GoalClient::default());
            let observer = observer_agent
                .build_session(
                    root.path().display().to_string(),
                    observer_client.clone(),
                    &options("goal-run"),
                )
                .unwrap();
            assert!(observer.runs().await.is_empty());
            assert_eq!(
                observer.preview_goal_tick(&tick).await.unwrap(),
                GoalTickDecisionV1::Reconcile,
                "the original fact remains authoritative when this host has no Run cache"
            );
            assert!(observer.runs().await.is_empty());
            assert_eq!(observer_client.calls.load(Ordering::SeqCst), 0);
            observer.close().await;
            observer_agent.close().await;
            assert!(session
                .spawn_run_with_id(&tick.operation_id, "different immutable input")
                .await
                .is_err());
            assert!(
                session.preview_goal_tick(&request(&session)).await.is_err(),
                "same operation cannot adopt new references"
            );
            let mut new_request = request(&session);
            new_request.operation_id = "new-wake".into();
            assert_eq!(
                session.preview_goal_tick(&new_request).await.unwrap(),
                GoalTickDecisionV1::BudgetExhausted
            );
            assert_eq!(
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
                before
            );
            let mut plan = crate::goal_control::GoalPlanCommandV1 {
                kind: GoalCommandKindV1::ProposePlan,
                operation_id: "replan".into(),
                goal_id: goal.binding.goal_id,
                session_id: goal.binding.session_id,
                workspace: goal.binding.workspace,
                work_policy_digest: goal.binding.work_policy_digest,
                expected_goal_revision: goal.binding.goal_revision,
                expected_plan_revision: goal.plan_revision,
                expected_goal_sequence: goal.goal_sequence,
                plan: goal.plan,
                proposal_sequence: None,
            };
            let proposed = session.apply_goal_plan(plan.clone()).unwrap();
            plan.kind = GoalCommandKindV1::AcceptPlan;
            plan.operation_id = "accept-replan".into();
            plan.expected_plan_revision = proposed.plan_revision;
            plan.expected_goal_sequence = proposed.goal_sequence;
            plan.proposal_sequence = proposed.plan_fact_sequence;
            plan.plan = None;
            session.apply_goal_plan(plan).unwrap();
            assert_eq!(
                session.preview_goal_tick(&tick).await.unwrap(),
                GoalTickDecisionV1::Reconcile
            );
            let mut cancel = request(&session);
            cancel.kind = GoalCommandKindV1::Cancel;
            cancel.operation_id = "cancel".into();
            session.apply_goal_control(cancel).unwrap();
            let before =
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
            assert_eq!(
                session.preview_goal_tick(&tick).await.unwrap(),
                GoalTickDecisionV1::Reconcile
            );
            assert_eq!(client.calls.load(Ordering::SeqCst), 3);
            session.save().await.unwrap();
            session.close().await;
            agent.close().await;
            let agent = Agent::from_config(test_config()).await.unwrap();
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
                restored.goal_definition().unwrap().unwrap().state,
                GoalState::Cancelled
            );
            assert_eq!(
                restored.preview_goal_tick(&tick).await.unwrap(),
                GoalTickDecisionV1::Reconcile
            );
            assert!(restored
                .spawn_run_with_id(&tick.operation_id, prompt)
                .await
                .unwrap()
                .replayed());
            assert_eq!(restored.runs().await.len(), 1);
            assert_eq!(
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
                before
            );
            assert_eq!(
                std::fs::read(root.path().join("source.txt")).unwrap(),
                b"current source"
            );
            restored.close().await;
            agent.close().await;
        }
    }
}

#[tokio::test]
async fn goal_tick_preview_is_read_only_and_missing_activation_survives_reopen() {
    for file in [false, true] {
        for policy in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let storage = tempfile::tempdir().unwrap();
            let store: Arc<dyn SessionStore> = if file {
                Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
            } else {
                Arc::new(MemorySessionStore::new())
            };
            let mut opts = options("goal-run").with_session_store(store.clone());
            if policy {
                opts = opts.with_work_policy(
                    crate::WorkPolicyV1::new(
                        "engineering",
                        1,
                        "Reuse the existing execution path.",
                    )
                    .unwrap(),
                );
            }
            let (agent, session, client) = fixture(root.path(), true, opts).await;
            let tick = request(&session);
            let before =
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
            let checks = (0..16).map(|_| session.preview_goal_tick(&tick));
            for result in futures::future::join_all(checks).await {
                assert_eq!(result.unwrap(), GoalTickDecisionV1::BudgetExhausted);
            }
            assert_eq!(client.calls.load(Ordering::SeqCst), 0);
            assert!(session.runs().await.is_empty());
            assert_eq!(
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
                before
            );
            let lease = session.run_admission.try_acquire("goal-run").unwrap();
            assert_eq!(
                session.preview_goal_tick(&tick).await.unwrap(),
                GoalTickDecisionV1::Reconcile
            );
            let log = lease
                .nonexecuting_fact_log(&session.run_admission, "goal-run")
                .unwrap();
            assert!(log.try_writer_fence().is_ok());
            drop(lease);
            session.save().await.unwrap();
            session.close().await;
            assert!(session.preview_goal_tick(&tick).await.is_err());
            agent.close().await;
            let agent = Agent::from_config(test_config()).await.unwrap();
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
                restored.preview_goal_tick(&tick).await.unwrap(),
                GoalTickDecisionV1::BudgetExhausted
            );
            assert!(restored.runs().await.is_empty());
            assert_eq!(
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
                before
            );
            restored.close().await;
            agent.close().await;
        }
    }
}

#[tokio::test]
async fn goal_tick_preview_rejects_stale_scope_and_operation_without_execution() {
    let root = tempfile::tempdir().unwrap();
    let (agent, session, client) = fixture(root.path(), true, options("goal-run")).await;
    let tick = request(&session);
    let before = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    for change in 0..9 {
        let mut wrong = tick.clone();
        match change {
            0 => wrong.session_id = "foreign".into(),
            1 => wrong.goal_id = "foreign".into(),
            2 => wrong.workspace.display_root = "foreign".into(),
            3 => wrong.work_policy_digest = Some("foreign".into()),
            4 => wrong.expected_goal_revision += 1,
            5 => wrong.expected_plan_revision = Some(99),
            6 => wrong.expected_goal_sequence += 1,
            7 => wrong.operation_id = "create".into(),
            _ => wrong.kind = GoalCommandKindV1::Cancel,
        }
        assert!(
            session.preview_goal_tick(&wrong).await.is_err(),
            "case {change}"
        );
    }
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    assert!(session.runs().await.is_empty());
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
        before
    );
    let mut cancel = tick.clone();
    cancel.kind = GoalCommandKindV1::Cancel;
    cancel.operation_id = "cancel".into();
    session.apply_goal_control(cancel).unwrap();
    assert!(session.preview_goal_tick(&tick).await.is_err());
    assert!(session
        .preview_goal_tick(&request(&session))
        .await
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_tick_preview_requires_plan_acceptance_and_respects_imported_pause() {
    let root = tempfile::tempdir().unwrap();
    let (agent, session, client) = fixture(root.path(), false, options("goal-run")).await;
    assert_eq!(
        session.preview_goal_tick(&request(&session)).await.unwrap(),
        GoalTickDecisionV1::NeedsInput
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    session.close().await;
    agent.close().await;

    let root = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(GoalClient::default());
    let session = agent
        .build_session(
            root.path().display().to_string(),
            client.clone(),
            &options("paused"),
        )
        .unwrap();
    let mut create = command(&session, None);
    create.import_source = Some(crate::goal_control::GoalImportSourceV1 {
        identity: crate::execution_identity::ExecutionIdentityV1::derive(
            crate::goal_control::GOAL_LEGACY_SOURCE_DOMAIN_V1,
            &"explicit host source",
        )
        .unwrap(),
        paused: true,
    });
    session
        .apply_goal_import(create, || async { Ok(()) })
        .await
        .unwrap();
    let before = crate::fact_control::read_workspace_facts(root.path(), "paused").unwrap();
    assert_eq!(
        session.preview_goal_tick(&request(&session)).await.unwrap(),
        GoalTickDecisionV1::Paused
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    assert!(session.runs().await.is_empty());
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "paused").unwrap(),
        before
    );
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_tick_preview_reads_original_debt_without_starting_recovery() {
    for mutation in [false, true] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("source.txt"), "old source").unwrap();
        let (agent, session, client) = fixture(
            root.path(),
            true,
            options("goal-run").with_permission_policy(
                crate::permissions::PermissionPolicy::new().allow("write(*)"),
            ),
        )
        .await;
        if mutation {
            attestation::script(&client, true);
            assert!(session
                .send("Execute the admitted step.", None)
                .await
                .unwrap_err()
                .to_string()
                .contains("completion gate"));
            assert_eq!(
                std::fs::read(root.path().join("source.txt")).unwrap(),
                b"independent expected source"
            );
        } else {
            session
                .run_store
                .create_run("goal-run", "original unfinished Run")
                .await;
        }
        let calls = client.calls.load(Ordering::SeqCst);
        let before = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
        let runs = serde_json::to_value(session.runs().await).unwrap();
        for _ in 0..3 {
            assert_eq!(
                session.preview_goal_tick(&request(&session)).await.unwrap(),
                GoalTickDecisionV1::Reconcile
            );
        }
        assert_eq!(client.calls.load(Ordering::SeqCst), calls);
        assert_eq!(serde_json::to_value(session.runs().await).unwrap(), runs);
        assert_eq!(
            crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
            before
        );
        session.close().await;
        agent.close().await;
    }
}
