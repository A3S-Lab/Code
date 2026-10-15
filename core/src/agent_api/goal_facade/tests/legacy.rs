#![cfg(test)]

use super::*;

#[tokio::test]
async fn legacy_goal_state_uses_original_owner_writer_and_refuses_bound_or_closed() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let other_agent = crate::Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(NonRetryableStreamingClient::default());
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("legacy-state"),
        )
        .unwrap();
    let other = other_agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("legacy-state"),
        )
        .unwrap();
    let directory = crate::session_folder::effect_log_dir(workspace.path(), session.session_id());
    let foreign = a3s_effect::FileLog::open(&directory).unwrap();
    let thread = crate::fact_control::thread_for_session(session.session_id());
    let before = foreign.read(&thread).unwrap();
    session
        .with_legacy_goal_state(|| async {
            assert!(foreign.try_writer_fence().is_err());
            assert!(foreign.try_run_owner(&thread).is_err());
            assert!(other.apply_goal_definition(command(&other, None)).is_err());
            assert!(other
                .with_legacy_goal_state::<(), _, _>(|| async { panic!("busy callback") })
                .await
                .is_err());
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        foreign.read(&thread).unwrap(),
        before,
        "metadata callback adds no facts"
    );
    other.apply_goal_definition(command(&other, None)).unwrap();
    assert!(session
        .with_legacy_goal_state::<(), _, _>(|| async { panic!("bound callback") })
        .await
        .is_err());
    other.close().await;
    session.close().await;
    assert!(session
        .with_legacy_goal_state::<(), _, _>(|| async { panic!("closed callback") })
        .await
        .is_err());
    assert_eq!(client.streaming_calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.complete_calls.load(Ordering::SeqCst), 0);
    other_agent.close().await;
    agent.close().await;
}

#[tokio::test]
async fn legacy_goal_state_abort_releases_original_fences() {
    let root = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.path().display().to_string(),
            Arc::new(NonRetryableStreamingClient::default()),
            &options("abort-legacy"),
        )
        .unwrap();
    let entered = tokio::sync::Notify::new();
    let mut access = Box::pin(session.with_legacy_goal_state(|| async {
        entered.notify_one();
        std::future::pending::<Result<()>>().await
    }));
    tokio::select! {
        _ = entered.notified() => {},
        _ = &mut access => panic!("must remain suspended"),
    }
    drop(access);
    session
        .with_legacy_goal_state(|| async { Ok(()) })
        .await
        .unwrap();
    session
        .apply_goal_definition(command(&session, None))
        .unwrap();
    session.close().await;
    agent.close().await;
}
