use super::*;

#[tokio::test]
async fn test_history_empty_on_new_session() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace", None)
        .await
        .unwrap();
    assert!(session.history().is_empty());
}

#[tokio::test]
async fn test_stream_updates_history_and_auto_saves() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("stream-history-test")
        .with_auto_save(true);
    let session = agent
        .build_session(
            "/tmp/test-stream-history".into(),
            Arc::new(StaticStreamingClient::new("streamed answer")),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    let mut saw_end = false;
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::End { .. }) {
            saw_end = true;
            break;
        }
    }
    handle.await.unwrap();

    assert!(saw_end);
    let history = session.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].text(), "hello");
    assert_eq!(history[1].text(), "streamed answer");

    let saved = store
        .load("stream-history-test")
        .await
        .unwrap()
        .expect("saved session");
    assert_eq!(saved.messages.len(), 2);
    assert_eq!(saved.messages[1].text(), "streamed answer");

    let run_records = store
        .load_run_records("stream-history-test")
        .await
        .unwrap()
        .expect("saved run records");
    assert_eq!(run_records.len(), 1);
    assert_eq!(
        run_records[0].snapshot.status,
        crate::run::RunStatus::Completed
    );
    assert!(run_records[0]
        .events
        .iter()
        .any(|record| matches!(record.event, AgentEvent::End { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stream_bridges_subagent_lifecycle_events() {
    use crate::prompts::PlanningMode;
    use crate::subagent_task_tracker::SubagentStatus;

    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "call-parallel",
            "task",
            serde_json::json!({
                "tasks": [
                    {
                        "agent": "explore",
                        "description": "Find auth code",
                        "prompt": "Find the auth code."
                    },
                    {
                        "agent": "explore",
                        "description": "Find docs",
                        "prompt": "Find the docs."
                    }
                ]
            }),
        ),
        scripted_text_response("auth child result"),
        scripted_text_response("docs child result"),
        scripted_text_response("final answer"),
    ]));
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_id("stream-subagents-test")
        .with_confirmation_policy(crate::hitl::ConfirmationPolicy::default())
        .with_planning_mode(PlanningMode::Disabled);
    let session = agent
        .build_session("/tmp/test-stream-subagents".into(), client, &opts)
        .unwrap();

    let (mut rx, handle) = session.stream("fan out this work", None).await.unwrap();
    let mut subagent_starts = 0;
    let mut subagent_ends = 0;
    let mut event_index = 0usize;
    let mut last_subagent_end = None;
    let mut parent_tool_end = None;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::SubagentStart { .. } => subagent_starts += 1,
            AgentEvent::SubagentEnd { .. } => {
                subagent_ends += 1;
                last_subagent_end = Some(event_index);
            }
            AgentEvent::ToolEnd { id, .. } if id == "call-parallel" => {
                parent_tool_end = Some(event_index);
            }
            AgentEvent::End { .. } => break,
            _ => {}
        }
        event_index += 1;
    }
    handle.await.unwrap();

    assert_eq!(subagent_starts, 2);
    assert_eq!(subagent_ends, 2);
    assert!(
        last_subagent_end.expect("foreground tasks must emit SubagentEnd")
            < parent_tool_end.expect("task fan-out must emit ToolEnd"),
        "all foreground SubagentEnd events must precede the parent ToolEnd"
    );

    let tasks = session.subagent_tasks().await;
    assert_eq!(tasks.len(), 2);
    assert!(tasks
        .iter()
        .all(|task| task.status == SubagentStatus::Completed));
}

#[tokio::test]
async fn test_stream_with_custom_history_does_not_update_session_history() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-stream-custom-history".into(),
            Arc::new(StaticStreamingClient::new("custom history answer")),
            &SessionOptions::new(),
        )
        .unwrap();
    let custom_history = vec![Message::user("custom prompt")];

    let (mut rx, handle) = session
        .stream("ignored", Some(&custom_history))
        .await
        .unwrap();
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::End { .. }) {
            break;
        }
    }
    handle.await.unwrap();

    assert!(session.history().is_empty());
}

#[tokio::test]
async fn test_stream_error_does_not_update_history_or_auto_save() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("stream-error-test")
        .with_auto_save(true);
    let session = agent
        .build_session(
            "/tmp/test-stream-error".into(),
            Arc::new(FailingStreamingClient),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    let mut saw_error = false;
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::Error { .. }) {
            saw_error = true;
            break;
        }
    }
    handle.await.unwrap();

    assert!(saw_error);
    assert!(session.history().is_empty());
    assert!(store.load("stream-error-test").await.unwrap().is_none());
}

#[tokio::test]
async fn test_non_retryable_stream_error_skips_fallback_and_circuit_retries() {
    let client = Arc::new(NonRetryableStreamingClient::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-non-retryable-stream-error".into(),
            client.clone(),
            &SessionOptions::new().with_planning_mode(PlanningMode::Disabled),
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    let mut error_message = None;
    while let Some(event) = rx.recv().await {
        if let AgentEvent::Error { message } = event {
            error_message = Some(message);
            break;
        }
    }
    handle.await.unwrap();

    assert_eq!(
        error_message.as_deref(),
        Some("Codex Pro usage limit reached. Quota resets in about 2h 45m.")
    );
    assert_eq!(
        client
            .streaming_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a non-retryable provider error must make one streaming call"
    );
    assert_eq!(
        client
            .complete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a non-retryable provider error must not use non-streaming fallback"
    );
}

#[tokio::test]
async fn test_non_retryable_pre_analysis_stops_before_main_turn() {
    let client = Arc::new(NonRetryableStreamingClient::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-non-retryable-pre-analysis".into(),
            client.clone(),
            &SessionOptions::new().with_planning_mode(PlanningMode::Auto),
        )
        .unwrap();

    let (mut rx, handle) = session
        .stream("review this repository", None)
        .await
        .unwrap();
    let mut error_message = None;
    while let Some(event) = rx.recv().await {
        if let AgentEvent::Error { message } = event {
            error_message = Some(message);
            break;
        }
    }
    handle.await.unwrap();

    assert_eq!(
        error_message.as_deref(),
        Some("Codex Pro usage limit reached. Quota resets in about 2h 45m.")
    );
    assert_eq!(
        client
            .complete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "pre-analysis should make exactly one provider request"
    );
    assert_eq!(
        client
            .streaming_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a terminal pre-analysis error must stop before the main turn"
    );
}

#[tokio::test]
async fn test_stream_cancel_records_interrupted_history_and_auto_saves() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("stream-cancel-test")
        .with_auto_save(true);
    let session = agent
        .build_session(
            "/tmp/test-stream-cancel".into(),
            Arc::new(CancellableStreamingClient::new("partial answer")),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    let mut saw_delta = false;
    for _ in 0..16 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("stream event before timeout")
            .expect("stream should stay open until cancelled");
        if matches!(event, AgentEvent::TextDelta { ref text } if text == "partial answer") {
            saw_delta = true;
            break;
        }
    }
    assert!(saw_delta);
    assert!(session.cancel().await);

    while rx.recv().await.is_some() {}
    handle.await.unwrap();

    let history = session.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].text(), "hello");
    assert_eq!(history[1].role, "assistant");
    assert!(history[1].text().contains("interrupted"));

    let saved = store
        .load("stream-cancel-test")
        .await
        .unwrap()
        .expect("interrupted stream should auto-save");
    assert_eq!(saved.messages.len(), 2);
    assert_eq!(saved.messages[0].text(), "hello");
    assert!(saved.messages[1].text().contains("interrupted"));
    assert!(!session.cancel().await);
}

#[tokio::test]
async fn test_stream_with_attachments_cancel_records_interrupted_history_and_auto_saves() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("stream-attachments-cancel-test")
        .with_auto_save(true);
    let session = agent
        .build_session(
            "/tmp/test-stream-attachments-cancel".into(),
            Arc::new(CancellableStreamingClient::new("partial attachment answer")),
            &opts,
        )
        .unwrap();
    let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];

    let (mut rx, handle) = session
        .stream_with_attachments("hello", &attachments, None)
        .await
        .unwrap();
    let mut saw_delta = false;
    for _ in 0..16 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("stream event before timeout")
            .expect("stream should stay open until cancelled");
        if matches!(event, AgentEvent::TextDelta { .. }) {
            saw_delta = true;
            break;
        }
    }
    assert!(saw_delta);
    assert!(session.cancel().await);

    while rx.recv().await.is_some() {}
    handle.await.unwrap();

    let history = session.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].text(), "hello");
    assert_eq!(history[1].role, "assistant");
    assert!(history[1].text().contains("interrupted"));

    let saved = store
        .load("stream-attachments-cancel-test")
        .await
        .unwrap()
        .expect("interrupted attachment stream should auto-save");
    assert_eq!(saved.messages.len(), 2);
    assert_eq!(saved.messages[0].text(), "hello");
    assert!(saved.messages[1].text().contains("interrupted"));
    assert_eq!(
        session.runs().await[0].status,
        crate::run::RunStatus::Cancelled
    );
    assert!(!session.cancel().await);
}

#[tokio::test]
async fn test_run_handle_cancels_send_with_attachments() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-send-attachments-run-handle-cancel".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &SessionOptions::new(),
            )
            .unwrap(),
    );
    let worker_session = Arc::clone(&session);
    let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];

    let worker = tokio::spawn(async move {
        worker_session
            .send_with_attachments("hello", &attachments, None)
            .await
    });

    let mut run = None;
    for _ in 0..20 {
        if let Some(current) = session.current_run().await {
            run = Some(current);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let run = run.expect("current run should be visible");
    assert!(run.cancel().await);

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .expect("send_with_attachments should stop after cancellation")
        .expect("worker should not panic");
    let result = result.expect("cancellation should preserve interrupted history");
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].text(), "hello");
    assert!(result.messages[1].text().contains("interrupted"));
    assert_eq!(run.status().await, Some(crate::run::RunStatus::Cancelled));
    let history = session.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].text(), "hello");
    assert!(history[1].text().contains("interrupted"));
    assert!(!session.cancel().await);
}

#[tokio::test]
async fn test_cancel_run_only_cancels_matching_current_run() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-cancel-run-by-id".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &SessionOptions::new(),
            )
            .unwrap(),
    );
    let worker_session = Arc::clone(&session);
    let worker = tokio::spawn(async move { worker_session.send("hello", None).await });

    let mut run_id = None;
    for _ in 0..20 {
        if let Some(current) = session.current_run().await {
            run_id = Some(current.id().to_string());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let run_id = run_id.expect("current run should be visible");

    assert!(!session.cancel_run("stale-run").await);
    assert!(session.cancel_run(&run_id).await);

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .expect("send should stop after cancellation")
        .expect("worker should not panic");
    let result = result.expect("cancellation should preserve interrupted history");
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].text(), "hello");
    assert!(result.messages[1].text().contains("interrupted"));
    assert_eq!(
        session.run_snapshot(&run_id).await.unwrap().status,
        crate::run::RunStatus::Cancelled
    );
    assert!(!session.cancel_run(&run_id).await);
}

