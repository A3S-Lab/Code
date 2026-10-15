#![cfg(test)]

use super::*;

#[tokio::test]
async fn continuation_history_resets_only_on_original_verified_progress_and_survives_session_reopen(
) {
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
                    crate::WorkPolicyV1::new("engineering", 1, "Reuse original source evidence.")
                        .unwrap(),
                );
            }
            let (agent, session, client) = fixture(root.path(), true, opts).await;
            let view = session.goal_definition().unwrap().unwrap();
            let finite_source = serde_json::from_value(serde_json::json!({
                "policy": { "automatic_enabled":true, "max_slices":2, "max_no_progress_slices":1, "deadline_unix_ms":10000 },
                "observed_unix_ms":100,
            })).unwrap();
            // Protocol history fixture, not a claim that an automatic Run executed.
            {
                let lease = session.run_admission.try_acquire("goal-run").unwrap();
                let log = lease
                    .nonexecuting_fact_log(&session.run_admission, "goal-run")
                    .unwrap();
                let _writer = log.try_writer_fence().unwrap();
                let facts = log.read("goal-run").unwrap();
                let prepared = crate::goal_control::prepare_run(
                    &facts,
                    "goal-run",
                    &view.binding.workspace,
                    view.binding.work_policy_digest.as_deref(),
                    Some("historical-auto"),
                    Some(&finite_source),
                )
                .unwrap()
                .unwrap();
                log.append("goal-run", &[prepared.pending.unwrap()], None)
                    .unwrap();
            }
            let before =
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
            assert!(
                crate::goal_control::prepare_run(
                    &before,
                    "goal-run",
                    &view.binding.workspace,
                    view.binding.work_policy_digest.as_deref(),
                    Some("next-auto"),
                    Some(&finite_source),
                )
                .is_err(),
                "unaccepted progress cannot refresh the finite bound"
            );
            session
                .register_dynamic_tool(Arc::new(acceptance::VerifySource("current source".into())))
                .unwrap();
            acceptance::script(
                &client,
                r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#,
            );
            session
                .send(
                    "Perform the current step using its fresh independent verification.",
                    None,
                )
                .await
                .unwrap();
            assert_eq!(client.calls.load(Ordering::SeqCst), 3);
            let after = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
            assert!(after.starts_with(&before));
            assert_eq!(
                after
                    .iter()
                    .filter(|f| f.kind == "a3s.goal.step.accepted")
                    .count(),
                1
            );
            assert!(crate::goal_control::prepare_run(
                &after,
                "goal-run",
                &view.binding.workspace,
                view.binding.work_policy_digest.as_deref(),
                Some("next-auto"),
                Some(&finite_source),
            )
            .unwrap()
            .unwrap()
            .pending
            .is_some());
            let mut exhausted = finite_source.clone();
            exhausted.policy.max_slices = std::num::NonZeroU64::new(1).unwrap();
            assert!(
                crate::goal_control::prepare_run(
                    &after,
                    "goal-run",
                    &view.binding.workspace,
                    view.binding.work_policy_digest.as_deref(),
                    Some("next-auto"),
                    Some(&exhausted),
                )
                .is_err(),
                "accepted progress never resets lifetime opportunities"
            );
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
                GoalState::Active
            );
            let reopened =
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
            assert_eq!(reopened, after);
            assert!(crate::goal_control::prepare_run(
                &reopened,
                "goal-run",
                &view.binding.workspace,
                view.binding.work_policy_digest.as_deref(),
                Some("next-auto"),
                Some(&exhausted),
            )
            .is_err());
            assert!(crate::goal_control::prepare_run(
                &reopened,
                "goal-run",
                &view.binding.workspace,
                view.binding.work_policy_digest.as_deref(),
                Some("next-auto"),
                Some(&finite_source),
            )
            .unwrap()
            .unwrap()
            .pending
            .is_some());
            assert_eq!(
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
                after
            );
            restored.close().await;
            agent.close().await;
        }
    }
}
