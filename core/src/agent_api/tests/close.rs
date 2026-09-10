use super::*;

#[tokio::test]
async fn test_is_closed_starts_false() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-close-default", None)
        .await
        .unwrap();
    assert!(!session.is_closed());
}

#[tokio::test]
async fn test_close_marks_session_closed_and_is_idempotent() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-close-idempotent", None)
        .await
        .unwrap();
    assert!(!session.is_closed());

    session.close().await;
    assert!(session.is_closed());

    session.close().await;
    assert!(session.is_closed());
}

#[tokio::test]
async fn test_send_after_close_returns_session_closed_error() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("send-after-close");
    let session = agent
        .build_session(
            "/tmp/test-send-after-close".into(),
            Arc::new(StaticStreamingClient::new("never delivered")),
            &opts,
        )
        .unwrap();

    session.close().await;
    let err = session.send("hello", None).await.unwrap_err();
    match err {
        crate::error::CodeError::SessionClosed { session_id } => {
            assert_eq!(session_id, "send-after-close");
        }
        other => panic!("expected SessionClosed, got {other:?}"),
    }
}

#[tokio::test]
async fn test_direct_tools_after_close_return_session_closed_error() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("tool-after-close");
    let session = agent
        .build_session(
            "/tmp/test-tool-after-close".into(),
            Arc::new(StaticStreamingClient::new("never delivered")),
            &opts,
        )
        .unwrap();

    session.close().await;
    let error = session
        .tool("read", serde_json::json!({ "file_path": "missing.txt" }))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::SessionClosed { ref session_id }
            if session_id == "tool-after-close"
    ));

    let (_events, handle) = session.tool_with_events("read", serde_json::json!({}));
    let error = handle.await.unwrap().unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::SessionClosed { ref session_id }
            if session_id == "tool-after-close"
    ));
}

#[tokio::test]
async fn test_immediate_capability_mutations_after_close_fail_closed() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("extensions-after-close");
    let session = agent
        .build_session(
            "/tmp/test-extensions-after-close".into(),
            Arc::new(StaticStreamingClient::new("never delivered")),
            &opts,
        )
        .unwrap();
    session.close().await;

    let dynamic_name = "late-dynamic-tool";
    let errors = [
        session
            .register_dynamic_tool(Arc::new(NamedSessionTool(dynamic_name.to_string())))
            .unwrap_err(),
        session.register_dynamic_workflow_runtime().unwrap_err(),
        session.unregister_dynamic_tool(dynamic_name).unwrap_err(),
        session
            .register_command(Arc::new(NoopSessionCommand))
            .unwrap_err(),
        session
            .register_hook(crate::hooks::Hook::new(
                "late-hook",
                crate::hooks::HookEventType::PreToolUse,
            ))
            .unwrap_err(),
        session.unregister_hook("late-hook").unwrap_err(),
        session
            .register_worker_agent(crate::subagent::WorkerAgentSpec::planner(
                "late-worker",
                "Must not register",
            ))
            .unwrap_err(),
        session
            .register_agent_dir(std::path::Path::new("/nonexistent/late-agent-dir"))
            .unwrap_err(),
        session.set_budget_guard(None).unwrap_err(),
    ];
    assert!(errors.iter().all(|error| matches!(
        error,
        crate::error::CodeError::SessionClosed { session_id }
            if session_id == "extensions-after-close"
    )));
    let lane_error = session
        .set_lane_handler(
            crate::queue::SessionLane::Execute,
            crate::queue::LaneHandlerConfig {
                mode: crate::queue::TaskHandlerMode::External,
                timeout_ms: 1_000,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        lane_error,
        crate::error::CodeError::SessionClosed { ref session_id }
            if session_id == "extensions-after-close"
    ));
    assert!(!session.tool_names().iter().any(|name| name == dynamic_name));
    assert!(!session.agent_registry.exists("late-worker"));
    assert_eq!(session.hook_count(), 0);
}

#[tokio::test]
async fn test_stream_after_close_returns_session_closed_error() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("stream-after-close");
    let session = agent
        .build_session(
            "/tmp/test-stream-after-close".into(),
            Arc::new(StaticStreamingClient::new("never delivered")),
            &opts,
        )
        .unwrap();

    session.close().await;
    let err = session.stream("hello", None).await.unwrap_err();
    assert!(matches!(
        err,
        crate::error::CodeError::SessionClosed { ref session_id }
            if session_id == "stream-after-close"
    ));
}

#[tokio::test]
async fn test_close_cancels_in_flight_send() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-close-in-flight".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &SessionOptions::new(),
            )
            .unwrap(),
    );

    let worker_session = Arc::clone(&session);
    let worker = tokio::spawn(async move { worker_session.send("hello", None).await });

    let mut run_id = None;
    for _ in 0..50 {
        if let Some(current) = session.current_run().await {
            run_id = Some(current.id().to_string());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let run_id = run_id.expect("current run should be visible before close()");

    session.close().await;
    assert!(session.is_closed());

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .expect("send should stop after close")
        .expect("worker should not panic");
    let result = result.expect("close cancellation should preserve interrupted history");
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].text(), "hello");
    assert!(result.messages[1].text().contains("interrupted"));
    assert_eq!(
        session.run_snapshot(&run_id).await.unwrap().status,
        crate::run::RunStatus::Cancelled
    );
}
