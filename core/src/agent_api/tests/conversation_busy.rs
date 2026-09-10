use super::*;

#[tokio::test]
async fn test_stream_exposes_current_run_handle_and_replay() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-stream-run-handle".into(),
            Arc::new(CancellableStreamingClient::new("partial answer")),
            &SessionOptions::new(),
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    let mut saw_delta = false;
    for _ in 0..16 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("stream event before timeout")
            .expect("stream emits event");
        if matches!(event, AgentEvent::TextDelta { .. }) {
            saw_delta = true;
            break;
        }
    }
    assert!(saw_delta);

    let run = session.current_run().await.expect("current run handle");
    assert_eq!(run.session_id(), session.id());
    assert!(matches!(
        run.status().await,
        Some(crate::run::RunStatus::Executing | crate::run::RunStatus::Planning)
    ));
    assert!(run.cancel().await);

    while rx.recv().await.is_some() {}
    handle.await.unwrap();

    let snapshot = run
        .snapshot()
        .await
        .expect("run snapshot remains replayable");
    assert_eq!(snapshot.status, crate::run::RunStatus::Cancelled);
    assert!(!run.events().await.is_empty());
}

#[tokio::test]
async fn test_active_stream_rejects_slash_command_with_session_busy() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("single-flight-slash");
    let session = agent
        .build_session(
            "/tmp/test-single-flight-slash".into(),
            Arc::new(CancellableStreamingClient::new("partial answer")),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::TextDelta { .. }) {
            break;
        }
    }

    // Slash commands read and may mutate session state, so they use the same
    // fail-fast admission gate as model-backed conversation operations. Hold
    // the history lock to prove admission happens before any transcript read.
    let history = Arc::clone(&session.history);
    let (locked_tx, locked_rx) = std::sync::mpsc::sync_channel(0);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    let history_holder = std::thread::spawn(move || {
        let _history_guard = history
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locked_tx.send(()).expect("test lock receiver remains open");
        release_rx
            .recv()
            .expect("test lock release sender remains open");
    });
    locked_rx
        .recv()
        .expect("history holder acquires the lock before admission check");
    let concurrent = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        session.send("/help", None),
    )
    .await
    .expect("busy admission must not wait for the history lock");
    release_tx
        .send(())
        .expect("history holder remains alive until released");
    history_holder
        .join()
        .expect("history holder must not panic");

    assert!(session.cancel().await);
    while rx.recv().await.is_some() {}
    handle.await.unwrap();

    match concurrent {
        Err(crate::error::CodeError::SessionBusy { session_id }) => {
            assert_eq!(session_id, "single-flight-slash");
        }
        other => panic!("expected SessionBusy, got {other:?}"),
    }

    let command = session.send("/help", None).await.unwrap();
    assert!(command.text.contains("/help"));
}

#[tokio::test]
async fn test_slash_command_outputs_obey_session_security_provider() {
    struct SensitiveCommand;

    impl crate::commands::SlashCommand for SensitiveCommand {
        fn name(&self) -> &str {
            "sensitive"
        }

        fn description(&self) -> &str {
            "Returns sensitive test data"
        }

        fn execute(
            &self,
            _args: &str,
            _ctx: &crate::commands::CommandContext,
        ) -> crate::commands::CommandOutput {
            crate::commands::CommandOutput::text("contact user@example.com")
        }
    }

    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_security_provider(Arc::new(crate::security::DefaultSecurityProvider::new()));
    let session = agent
        .build_session(
            "/tmp/test-secure-slash-command".into(),
            Arc::new(StaticStreamingClient::new("unused")),
            &opts,
        )
        .unwrap();
    session
        .register_command(Arc::new(SensitiveCommand))
        .unwrap();

    let result = session.send("/sensitive", None).await.unwrap();
    assert!(!result.text.contains("user@example.com"));
    assert!(result.text.contains("REDACTED:EMAIL"));

    let (mut rx, handle) = session.stream("/sensitive", None).await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    handle.await.unwrap();
    assert!(matches!(events.last(), Some(AgentEvent::End { .. })));
    for event in events {
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            !json.contains("user@example.com"),
            "unsanitized event: {json}"
        );
    }
}

#[tokio::test]
async fn test_active_stream_rejects_all_conversation_entrypoints() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("single-flight-all-entrypoints");
    let session = agent
        .build_session(
            "/tmp/test-single-flight-all-entrypoints".into(),
            Arc::new(CancellableStreamingClient::new("partial answer")),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("first", None).await.unwrap();
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::TextDelta { .. }) {
            break;
        }
    }

    let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];
    assert_session_busy(
        session.send("second", None).await,
        "single-flight-all-entrypoints",
    );
    assert_session_busy(
        session.stream("second", None).await,
        "single-flight-all-entrypoints",
    );
    assert_session_busy(
        session
            .send_with_attachments("second", &attachments, None)
            .await,
        "single-flight-all-entrypoints",
    );
    assert_session_busy(
        session
            .stream_with_attachments("second", &attachments, None)
            .await,
        "single-flight-all-entrypoints",
    );
    assert_session_busy(
        session.resume_run("not-loaded-while-busy").await,
        "single-flight-all-entrypoints",
    );
    assert_session_busy(session.save().await, "single-flight-all-entrypoints");

    assert!(session.cancel().await);
    while rx.recv().await.is_some() {}
    handle.await.unwrap();

    // The public stream handle completes only after its guardian has released
    // admission, so the next operation can start immediately after awaiting it.
    assert!(session.send("/help", None).await.is_ok());
}

#[tokio::test]
async fn test_active_blocking_send_rejects_send_and_stream_then_releases() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("single-flight-blocking");
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-single-flight-blocking".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &opts,
            )
            .unwrap(),
    );

    let worker_session = Arc::clone(&session);
    let first = tokio::spawn(async move { worker_session.send("first", None).await });

    let mut active_run = None;
    for _ in 0..50 {
        if let Some(run) = session.current_run().await {
            active_run = Some(run);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        active_run.is_some(),
        "the first blocking send must become active"
    );

    assert_session_busy(session.send("second", None).await, "single-flight-blocking");
    assert_session_busy(
        session.stream("second", None).await,
        "single-flight-blocking",
    );

    assert!(session.cancel().await);
    let first_result = tokio::time::timeout(std::time::Duration::from_secs(1), first)
        .await
        .expect("the cancelled blocking send must finish")
        .expect("the blocking send task must not panic")
        .expect("cancellation preserves interrupted history");
    assert!(first_result
        .messages
        .last()
        .is_some_and(|message| message.text().contains("interrupted")));

    // Awaiting the blocking operation releases the lease for the next call.
    assert!(session.send("/help", None).await.is_ok());
}

