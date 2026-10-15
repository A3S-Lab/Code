#![cfg(test)]

use super::*;
use crate::goal_control::{GoalImportSourceV1, GOAL_LEGACY_SOURCE_DOMAIN_V1};

async fn prove_import(
    store: Arc<dyn SessionStore>,
    reopen: Option<std::path::PathBuf>,
    paused: bool,
    policy: bool,
) {
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(NonRetryableStreamingClient::default());
    let work = crate::WorkPolicyV1::new("engineering", 1, "Keep original mechanisms.").unwrap();
    let mut opts = options("import/session").with_session_store(store.clone());
    if policy {
        opts = opts.with_work_policy(work.clone());
    }
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &opts,
        )
        .unwrap();
    let mut cmd = command(&session, policy.then(|| work.digest().unwrap()));
    cmd.import_source = Some(GoalImportSourceV1 {
        identity: crate::execution_identity::ExecutionIdentityV1::derive(
            GOAL_LEGACY_SOURCE_DOMAIN_V1,
            &"host source",
        )
        .unwrap(),
        paused,
    });
    let thread = crate::fact_control::thread_for_session(session.session_id());
    let directory = crate::session_folder::effect_log_dir(workspace.path(), session.session_id());
    let foreign = a3s_effect::FileLog::open(&directory).unwrap();
    let before = foreign.read(&thread).unwrap();
    assert!(session.apply_goal_definition(cmd.clone()).is_err());
    assert!(session
        .apply_goal_import(cmd.clone(), || async { Err(goal_error("source drift")) })
        .await
        .is_err());
    assert_eq!(foreign.read(&thread).unwrap(), before);
    let imported = session
        .apply_goal_import(cmd.clone(), || async {
            assert!(foreign.try_writer_fence().is_err());
            assert!(foreign.try_run_owner(&thread).is_err());
            assert!(session
                .apply_goal_definition(command(&session, None))
                .is_err());
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        imported.state,
        if paused {
            GoalState::Paused
        } else {
            GoalState::Active
        }
    );
    assert_eq!(imported.import_source, cmd.import_source);
    let after = foreign.read(&thread).unwrap();
    assert_eq!(&after[..before.len()], before.as_slice());
    assert_eq!(after.last().unwrap().kind, "a3s.goal.created");
    let replay = session
        .apply_goal_import(cmd.clone(), || async {
            panic!("accepted source must not be reread")
        })
        .await
        .unwrap();
    assert_eq!(replay.goal_sequence, imported.goal_sequence);
    assert_eq!(foreign.read(&thread).unwrap(), after);
    assert!(session
        .send("A Goal import must not authorize an ordinary Run", None)
        .await
        .is_err());
    assert_eq!(client.streaming_calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.complete_calls.load(Ordering::SeqCst), 0);
    session.save().await.unwrap();
    session.close().await;
    agent.close().await;
    let store: Arc<dyn SessionStore> = match reopen {
        Some(path) => Arc::new(FileSessionStore::new(path).await.unwrap()),
        None => store,
    };
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let resumed = agent
        .resume_session_async(
            "import/session",
            options("import/session").with_session_store(store),
        )
        .await
        .unwrap();
    assert_eq!(
        resumed.goal_definition().unwrap().unwrap().state,
        imported.state
    );
    assert_eq!(
        resumed.goal_definition().unwrap().unwrap().import_source,
        cmd.import_source
    );
    let cancelled = resumed
        .apply_goal_control(GoalControlCommandV1 {
            kind: GoalCommandKindV1::Cancel,
            operation_id: "cancel-import".into(),
            goal_id: cmd.goal_id.clone(),
            session_id: cmd.session_id.clone(),
            workspace: cmd.workspace.clone(),
            work_policy_digest: cmd.work_policy_digest.clone(),
            expected_goal_revision: 1,
            expected_plan_revision: None,
            expected_goal_sequence: imported.goal_sequence,
        })
        .unwrap();
    assert_eq!(cancelled.state, GoalState::Cancelled);
    assert_eq!(
        resumed
            .apply_goal_import(cmd.clone(), || async { panic!("replay") })
            .await
            .unwrap()
            .state,
        imported.state
    );
    assert_eq!(
        resumed.goal_definition().unwrap().unwrap().state,
        GoalState::Cancelled
    );
    resumed.close().await;
    assert!(resumed
        .apply_goal_import(cmd, || async { panic!("closed") })
        .await
        .is_err());
    agent.close().await;
}

#[tokio::test]
async fn import_uses_original_fences_and_reopens_both_stores_without_execution() {
    for paused in [false, true] {
        for policy in [false, true] {
            prove_import(Arc::new(MemorySessionStore::new()), None, paused, policy).await;
            let root = tempfile::tempdir().unwrap();
            prove_import(
                Arc::new(FileSessionStore::new(root.path()).await.unwrap()),
                Some(root.path().to_owned()),
                paused,
                policy,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn dropped_or_closed_import_source_check_never_commits() {
    let root = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .build_session(
                root.path().display().to_string(),
                Arc::new(NonRetryableStreamingClient::default()),
                &options("abandoned-import"),
            )
            .unwrap(),
    );
    let mut cmd = command(&session, None);
    cmd.import_source = Some(GoalImportSourceV1 {
        identity: crate::execution_identity::ExecutionIdentityV1::derive(
            GOAL_LEGACY_SOURCE_DOMAIN_V1,
            &"source",
        )
        .unwrap(),
        paused: true,
    });
    let (entered, observed) = tokio::sync::oneshot::channel();
    let mut future = Box::pin(session.apply_goal_import(cmd.clone(), || async {
        entered.send(()).unwrap();
        std::future::pending::<Result<()>>().await
    }));
    tokio::select! { _ = &mut future => panic!("source is parked"), result = observed => result.unwrap() }
    assert!(session
        .run_admission
        .try_acquire(session.session_id())
        .is_err());
    drop(future);
    drop(
        session
            .run_admission
            .try_acquire(session.session_id())
            .unwrap(),
    );
    assert!(session.goal_definition().unwrap().is_none());
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        session.apply_goal_import(cmd, || async {
            session.close().await;
            Ok(())
        }),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(error, CodeError::SessionClosed { .. }));
    assert!(session.goal_definition().unwrap().is_none());
    agent.close().await;
}
