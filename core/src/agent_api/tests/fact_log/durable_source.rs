#![cfg(test)]

use super::*;
use crate::goal_control::{GoalCommandKindV1, GoalDefinitionCommandV1};
use crate::store::{FileSessionStore, MemorySessionStore, SessionStore};

pub(super) async fn source_agent() -> crate::Agent {
    let mut config = test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    crate::Agent::from_config(config).await.unwrap()
}

async fn prove_snapshot_source(file: bool) {
    for (policy_enabled, goal_enabled) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
        } else {
            Arc::new(MemorySessionStore::new())
        };
        let agent = source_agent().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let client = Arc::new(CountingText {
            calls: calls.clone(),
        });
        let policy =
            crate::WorkPolicyV1::new("source-test", 1, "Retain the exact original policy.")
                .unwrap();
        let mut options = crate::SessionOptions::new()
            .with_session_id("source/session")
            .with_session_store(store.clone())
            .with_llm_client(client.clone())
            .with_planning_mode(crate::PlanningMode::Disabled)
            .with_auto_delegation_enabled(false)
            .with_manual_delegation_enabled(false)
            .with_continuation(false);
        if policy_enabled {
            options = options.with_work_policy(policy.clone());
        }
        let session = agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &options,
            )
            .unwrap();
        if goal_enabled {
            session
                .apply_goal_definition(GoalDefinitionCommandV1 {
                    kind: GoalCommandKindV1::Create,
                    import_source: None,
                    operation_id: "create".into(),
                    goal_id: "source-goal".into(),
                    session_id: session.session_id().into(),
                    workspace: session
                        .tool_context
                        .workspace_services
                        .workspace_ref()
                        .clone(),
                    goal: crate::planning::AgentGoal::new("Preserve the source")
                        .with_criteria(vec!["Original source can be verified".into()]),
                    non_goals: vec![],
                    scope_fact_sequences: vec![],
                    work_policy_digest: policy_enabled.then(|| policy.digest().unwrap()),
                    expected_goal_revision: None,
                    expected_plan_revision: None,
                    expected_goal_sequence: None,
                })
                .unwrap();
        } else {
            session.send("record original history", None).await.unwrap();
        }
        session.save().await.unwrap();
        let snapshot = store
            .load_snapshot(session.session_id())
            .await
            .unwrap()
            .unwrap();
        let bound = policy_enabled || goal_enabled;
        assert_eq!(snapshot.session.durable_source.is_some(), bound);
        assert_eq!(snapshot.schema_version, if bound { 2 } else { 1 });
        let checkpoint: crate::loop_checkpoint::LoopCheckpoint = serde_json::from_value(serde_json::json!({
            "schema_version": if bound {2} else {1}, "durable_source": snapshot.session.durable_source,
            "run_id":"source-checkpoint", "session_id": session.session_id(), "turn":1,
            "messages":[], "total_usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0},
            "tool_calls_count":0,"checkpoint_ms":1,
        })).unwrap();
        store
            .save_loop_checkpoint(&checkpoint.run_id, &checkpoint)
            .await
            .unwrap();
        if bound {
            let thread = crate::fact_control::thread_for_session("source/session");
            let original =
                crate::fact_control::read_workspace_facts(workspace.path(), &thread).unwrap();
            let attempts = calls.load(Ordering::SeqCst);
            let error = session.fork_session_at(0).await.unwrap_err();
            assert!(error.to_string().contains("cannot be truncated"), "{error}");
            assert_eq!(
                crate::fact_control::read_workspace_facts(workspace.path(), &thread).unwrap(),
                original
            );
            assert_eq!(session.fork_session_at(u64::MAX).await.unwrap(), 0);
            let log = a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
                workspace.path(),
                "source/session",
            ))
            .unwrap();
            a3s_effect::LogStore::append(
                &log,
                &thread,
                &[a3s_effect::NewFact {
                    kind: "user.message".into(),
                    key: "fork-boundary-probe".into(),
                    payload: serde_json::json!({"text":"unexecuted later input"}),
                }],
                None,
            )
            .unwrap();
            let head = original.last().unwrap().seq;
            if goal_enabled {
                let error = session.fork_session_at(head).await.unwrap_err();
                assert!(error.to_string().contains("cannot be truncated"), "{error}");
                assert_eq!(
                    a3s_effect::LogStore::read(&log, &thread).unwrap().len(),
                    original.len() + 1
                );
            } else {
                assert_eq!(
                    session.fork_session_at(head).await.unwrap(),
                    1,
                    "policy-only later history remains forkable"
                );
                assert_eq!(a3s_effect::LogStore::read(&log, &thread).unwrap(), original);
            }
            assert_eq!(calls.load(Ordering::SeqCst), attempts);
        }
        let before = calls.load(Ordering::SeqCst);
        session.close().await;
        agent.close().await;
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
        } else {
            store
        };
        let agent = source_agent().await;
        options.work_policy = None;
        options.session_store = Some(store.clone());
        let source_dir = crate::session_folder::effect_log_dir(workspace.path(), "source/session");
        let retained = workspace.path().join("retained-original-log");
        if bound {
            std::fs::rename(&source_dir, &retained).unwrap();
            let error = agent
                .resume_session_async("source/session", options.clone())
                .await
                .expect_err("missing source must refuse resume");
            assert!(error.to_string().contains("durable session"), "{error}");
            assert_eq!(calls.load(Ordering::SeqCst), before);
            assert!(
                !source_dir.exists(),
                "failed resume must not initialize replacement facts"
            );
            std::fs::rename(&retained, &source_dir).unwrap();
        }
        let resumed = agent
            .resume_session_async("source/session", options)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), before);
        if goal_enabled {
            assert_eq!(
                resumed.goal_definition().unwrap().unwrap().binding.goal_id,
                "source-goal"
            );
        } else {
            resumed
                .send("continue original history", None)
                .await
                .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), before + 1);
        }
        resumed.save().await.unwrap();
        assert_eq!(
            store
                .load_snapshot("source/session")
                .await
                .unwrap()
                .unwrap()
                .session
                .durable_source,
            snapshot.session.durable_source
        );
        if bound {
            let attempts = calls.load(Ordering::SeqCst);
            let thread = crate::fact_control::thread_for_session("source/session");
            let source_file = source_dir.join(format!("{thread}.jsonl"));
            std::fs::rename(&source_file, &retained).unwrap();
            let error = resumed.fork_session_at(u64::MAX).await.unwrap_err();
            assert!(error.to_string().contains("source is missing"), "{error}");
            assert!(
                !source_file.exists(),
                "fork must not recreate its missing source"
            );
            assert!(resumed
                .send("source removed after restore", None)
                .await
                .is_err());
            assert!(resumed.resume_run(&checkpoint.run_id).await.is_err());
            assert!(resumed
                .spawn_recovery_with_run_id(&checkpoint.run_id, "refused-exact")
                .await
                .is_err());
            assert!(resumed.run_snapshot("refused-exact").await.is_none());
            let evidence =
                crate::session_checkpoint::SessionLogicalResumeEvidenceV1::from_checkpoint(
                    &checkpoint,
                )
                .unwrap();
            assert!(resumed
                .prepare_recovery_from_checkpoint(
                    &evidence,
                    "source-evidence",
                    "refused-prepared",
                    checkpoint.clone()
                )
                .await
                .is_err());
            assert!(resumed.run_snapshot("refused-prepared").await.is_none());
            assert_eq!(calls.load(Ordering::SeqCst), attempts);
        }
        resumed.close().await;
        agent.close().await;
    }
}

#[tokio::test]
async fn durable_source_four_modes_memory_store() {
    prove_snapshot_source(false).await;
}

#[tokio::test]
async fn durable_source_four_modes_file_store_reopen() {
    prove_snapshot_source(true).await;
}

#[tokio::test]
async fn fork_refuses_invalid_source_before_first_snapshot() {
    for file in [false, true] {
        for kind in ["a3s.work-policy.future", "a3s.goal.future"] {
            let workspace = tempfile::tempdir().unwrap();
            let storage = tempfile::tempdir().unwrap();
            let store: Arc<dyn SessionStore> = if file {
                Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
            } else {
                Arc::new(MemorySessionStore::new())
            };
            let agent = source_agent().await;
            let calls = Arc::new(AtomicUsize::new(0));
            let session = agent
                .build_session(
                    workspace.path().display().to_string(),
                    Arc::new(CountingText {
                        calls: calls.clone(),
                    }),
                    &crate::SessionOptions::new()
                        .with_session_id("invalid-fork-source")
                        .with_session_store(store.clone())
                        .with_auto_save(false)
                        .with_planning_mode(crate::PlanningMode::Disabled),
                )
                .unwrap();
            session.send("original history", None).await.unwrap();
            assert!(session.saved_durable_source().is_none());
            assert!(store
                .load_snapshot(session.session_id())
                .await
                .unwrap()
                .is_none());
            let thread = crate::fact_control::thread_for_session(session.session_id());
            let log = a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
                workspace.path(),
                session.session_id(),
            ))
            .unwrap();
            a3s_effect::LogStore::append(
                &log,
                &thread,
                &[a3s_effect::NewFact {
                    kind: kind.into(),
                    key: "unrecognized-source".into(),
                    payload: serde_json::json!({}),
                }],
                None,
            )
            .unwrap();
            let original = a3s_effect::LogStore::read(&log, &thread).unwrap();
            let attempts = calls.load(Ordering::SeqCst);
            let error = session.fork_session_at(u64::MAX).await.unwrap_err();
            assert!(
                error.to_string().contains("durable session source"),
                "{error}"
            );
            assert_eq!(a3s_effect::LogStore::read(&log, &thread).unwrap(), original);
            assert_eq!(calls.load(Ordering::SeqCst), attempts);
            session.close().await;
            agent.close().await;
        }
    }
}
