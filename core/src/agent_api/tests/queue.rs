use super::*;

#[tokio::test]
async fn test_session_options_with_agent_dir() {
    let opts = SessionOptions::new()
        .with_agent_dir("/tmp/agents")
        .with_agent_dir("/tmp/more-agents");
    assert_eq!(opts.agent_dirs.len(), 2);
    assert_eq!(opts.agent_dirs[0], PathBuf::from("/tmp/agents"));
    assert_eq!(opts.agent_dirs[1], PathBuf::from("/tmp/more-agents"));
}

// ========================================================================
// Queue Integration Tests
// ========================================================================

#[test]
fn test_session_options_with_queue_config() {
    let qc = SessionQueueConfig::default().with_lane_features();
    let opts = SessionOptions::new().with_queue_config(qc.clone());
    assert!(opts.queue_config.is_some());

    let config = opts.queue_config.unwrap();
    assert!(config.enable_dlq);
    assert!(config.enable_metrics);
    assert!(config.enable_alerts);
    assert_eq!(config.default_timeout_ms, Some(60_000));
}

#[tokio::test]
async fn test_session_uses_single_delegation_tool_surface() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace-delegation-tools", None)
        .await
        .unwrap();
    let names = session.tool_names();

    assert!(names.contains(&"task".to_string()));
    assert!(!names.contains(&"parallel_task".to_string()));
    assert!(
        !session.tool_executor.registry().contains("parallel_task"),
        "HARNESS-CONV4 removes model-visible parallel_task from the registry"
    );
    assert!(!names.contains(&"run_team".to_string()));
}

#[tokio::test]
async fn test_session_can_disable_manual_delegation_tools_without_dropping_registry() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_worker_agent(crate::subagent::WorkerAgentSpec::planner(
            "release-planner",
            "Plan releases",
        ))
        .with_manual_delegation_enabled(false);
    let session = agent
        .session_async("/tmp/test-workspace-no-manual-delegation", Some(opts))
        .await
        .unwrap();
    let names = session.tool_names();

    assert!(!names.contains(&"task".to_string()));
    assert!(!names.contains(&"parallel_task".to_string()));
    assert!(session.agent_registry.exists("release-planner"));
    assert!(!session.config.auto_delegation.allow_manual_delegation);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_with_queue_config() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-queue", Some(opts))
        .await;
    assert!(session.is_ok());
    let session = session.unwrap();
    assert!(session.has_queue());
}

#[tokio::test]
async fn test_session_without_queue_config() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace-noqueue", None)
        .await
        .unwrap();
    assert!(!session.has_queue());
}

#[tokio::test]
async fn test_session_queue_stats_without_queue() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace-stats", None)
        .await
        .unwrap();
    let stats = session.queue_stats().await;
    // Without a queue, stats should have zero values
    assert_eq!(stats.total_pending, 0);
    assert_eq!(stats.total_active, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_queue_stats_with_queue() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-qstats", Some(opts))
        .await
        .unwrap();
    let stats = session.queue_stats().await;
    // Fresh queue with no commands should have zero stats
    assert_eq!(stats.total_pending, 0);
    assert_eq!(stats.total_active, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_pending_external_tasks_empty() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-ext", Some(opts))
        .await
        .unwrap();
    let tasks = session.pending_external_tasks().await;
    assert!(tasks.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_confirmation_api_resolves_pending_request() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    let manager = Arc::new(crate::hitl::ConfirmationManager::new(
        crate::hitl::ConfirmationPolicy::enabled(),
        event_tx,
    ));
    let opts = SessionOptions::new().with_confirmation_manager(manager.clone());
    let session = agent
        .session_async("/tmp/test-workspace", Some(opts))
        .await
        .unwrap();

    let receiver = manager
        .request_confirmation(
            "tool-1",
            "bash",
            &serde_json::json!({ "command": "echo hi" }),
        )
        .await;

    let pending = session.pending_confirmations().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool_id, "tool-1");
    assert_eq!(pending[0].tool_name, "bash");

    let found = session
        .confirm_tool_use("tool-1", true, Some("approved by test".to_string()))
        .await
        .unwrap();
    assert!(found);

    let response = receiver.await.unwrap();
    assert!(response.approved);
    assert_eq!(response.reason.as_deref(), Some("approved by test"));
    assert!(session.pending_confirmations().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn session_close_rejects_and_clears_every_pending_confirmation() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    let manager = Arc::new(crate::hitl::ConfirmationManager::new(
        crate::hitl::ConfirmationPolicy::enabled(),
        event_tx,
    ));
    let opts = SessionOptions::new().with_confirmation_manager(manager.clone());
    let session = agent
        .session_async("/tmp/test-workspace-close-confirmations", Some(opts))
        .await
        .unwrap();

    let first = manager
        .request_confirmation("tool-close-1", "bash", &serde_json::json!({}))
        .await;
    let second = manager
        .request_confirmation("tool-close-2", "write", &serde_json::json!({}))
        .await;
    assert_eq!(session.pending_confirmations().await.len(), 2);

    session.close().await;

    for response in [first, second] {
        let response = tokio::time::timeout(std::time::Duration::from_secs(1), response)
            .await
            .expect("session close must settle confirmations promptly")
            .expect("confirmation sender must return a rejection");
        assert!(!response.approved);
        assert_eq!(response.reason.as_deref(), Some("Confirmation cancelled"));
    }
    assert!(session.pending_confirmations().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn session_close_settles_a_stream_blocked_on_confirmation() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    let manager = Arc::new(crate::hitl::ConfirmationManager::new(
        crate::hitl::ConfirmationPolicy::enabled()
            .with_timeout(5_000, crate::hitl::TimeoutAction::Reject),
        event_tx,
    ));
    let opts = SessionOptions::new()
        .with_confirmation_manager(manager.clone())
        .with_permission_policy(crate::permissions::PermissionPolicy::new());
    let session = agent
        .build_session(
            "/tmp/test-close-stream-confirmation".into(),
            Arc::new(ScriptedStreamingClient::new(vec![
                scripted_tool_call_response(
                    "tool-close-stream",
                    "bash",
                    serde_json::json!({"command": "echo must-not-run"}),
                ),
                scripted_text_response("must not continue"),
            ])),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("run a command", None).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while manager.pending_count().await != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the tool confirmation must become pending");
    let run_id = session.current_run().await.unwrap().id().to_string();

    session.close().await;
    tokio::time::timeout(std::time::Duration::from_secs(1), handle)
        .await
        .expect("close must settle the stream lifecycle")
        .expect("stream lifecycle must not panic");

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ConfirmationRequired {
            tool_id,
            tool_name,
            ..
        } if tool_id == "tool-close-stream" && tool_name == "bash"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ConfirmationReceived {
            tool_id,
            approved: false,
            reason,
        } if tool_id == "tool-close-stream"
            && reason.as_deref() == Some("Confirmation cancelled")
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStart { id, .. } if id == "tool-close-stream"
    )));
    assert_eq!(manager.pending_count().await, 0);
    assert!(session.current_run().await.is_none());
    assert_eq!(
        session.run_snapshot(&run_id).await.unwrap().status,
        crate::run::RunStatus::Cancelled
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_confirmation_api_without_manager_is_noop() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace", None)
        .await
        .unwrap();

    assert!(session.pending_confirmations().await.is_empty());
    assert!(!session
        .confirm_tool_use("missing", true, None)
        .await
        .unwrap());
    assert_eq!(session.cancel_confirmations().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_dead_letters_empty() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default().with_dlq(Some(100));
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-dlq", Some(opts))
        .await
        .unwrap();
    let dead = session.dead_letters().await;
    assert!(dead.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_queue_metrics_disabled() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    // Metrics not enabled
    let qc = SessionQueueConfig::default();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-nomet", Some(opts))
        .await
        .unwrap();
    let metrics = session.queue_metrics().await;
    assert!(metrics.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_queue_metrics_enabled() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default().with_metrics();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-met", Some(opts))
        .await
        .unwrap();
    let metrics = session.queue_metrics().await;
    assert!(metrics.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_set_lane_handler() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-handler", Some(opts))
        .await
        .unwrap();

    // Set Execute lane to External mode
    session
        .set_lane_handler(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: crate::queue::TaskHandlerMode::External,
                timeout_ms: 30_000,
            },
        )
        .await
        .unwrap();

    // No panic = success. The handler config is stored internally.
    // We can't directly read it back but we verify no errors.
}

// ========================================================================
// Session Persistence Tests
// ========================================================================
