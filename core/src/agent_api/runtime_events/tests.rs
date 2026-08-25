use super::*;

#[derive(Debug, Default)]
struct RecordingRuntimeEvents {
    events: std::sync::Mutex<Vec<AgentEvent>>,
}

#[async_trait::async_trait]
impl crate::hooks::HookExecutor for RecordingRuntimeEvents {
    async fn fire(&self, _event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        crate::hooks::HookResult::Continue(None)
    }

    async fn record_agent_event(&self, event: &AgentEvent, _run_id: &str, _session_id: &str) {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event.clone());
    }
}

fn active_tools() -> ActiveToolMap {
    Arc::new(tokio::sync::RwLock::new(HashMap::new()))
}

fn persistence_state(
) -> Arc<std::sync::RwLock<super::super::session_persistence::SessionPersistenceState>> {
    Arc::new(std::sync::RwLock::new(
        super::super::session_persistence::SessionPersistenceState::default(),
    ))
}

#[tokio::test]
async fn tool_events_update_active_tool_state() {
    let run_store = Arc::new(crate::run::InMemoryRunStore::new());
    let run = run_store.create_run("session-1", "prompt").await;
    let active_tools = active_tools();
    let sink = RuntimeEventSink::new(RuntimeEventSinkConfig {
        run_store: Arc::clone(&run_store),
        run_id: run.id.clone(),
        session_id: "session-1".to_string(),
        hook_executor: None,
        security_provider: None,
        persistence_state: persistence_state(),
        active_tools: Arc::clone(&active_tools),
        subagent_tasks: Arc::new(crate::subagent_task_tracker::InMemorySubagentTaskTracker::new()),
    });

    sink.observe(&AgentEvent::ToolStart {
        id: "tool-1".to_string(),
        name: "bash".to_string(),
    })
    .await;
    assert!(
        active_tools.read().await.is_empty(),
        "model-side tool preparation must not be reported as running"
    );

    sink.observe(&AgentEvent::ToolExecutionStart {
        id: "tool-1".to_string(),
        name: "bash".to_string(),
        args: serde_json::json!({ "command": "true" }),
    })
    .await;
    assert_eq!(active_tools.read().await.len(), 1);
    assert_eq!(
        active_tools
            .read()
            .await
            .get("tool-1")
            .map(|tool| tool.tool_name.as_str()),
        Some("bash")
    );

    sink.observe(&AgentEvent::ToolEnd {
        id: "tool-1".to_string(),
        name: "bash".to_string(),
        args: Some(serde_json::json!({ "command": "true" })),
        output: "ok".to_string(),
        exit_code: 0,
        metadata: None,
        error_kind: None,
    })
    .await;
    assert!(active_tools.read().await.is_empty());
}

#[tokio::test]
async fn observe_records_events_on_run_store() {
    let run_store = Arc::new(crate::run::InMemoryRunStore::new());
    let run = run_store.create_run("session-1", "prompt").await;
    let sink = RuntimeEventSink::new(RuntimeEventSinkConfig {
        run_store: Arc::clone(&run_store),
        run_id: run.id.clone(),
        session_id: "session-1".to_string(),
        hook_executor: None,
        security_provider: None,
        persistence_state: persistence_state(),
        active_tools: active_tools(),
        subagent_tasks: Arc::new(crate::subagent_task_tracker::InMemorySubagentTaskTracker::new()),
    });

    sink.observe(&AgentEvent::TextDelta {
        text: "hello".to_string(),
    })
    .await;

    let events = run_store.events(&run.id).await;
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].event, AgentEvent::TextDelta { .. }));
    assert_eq!(run_store.snapshot(&run.id).await.unwrap().event_count, 1);
}

#[tokio::test]
async fn run_owned_agent_events_do_not_cross_run_boundaries() {
    let run_store = Arc::new(crate::run::InMemoryRunStore::new());
    let run_a = run_store.create_run("session-1", "run a").await;
    let run_b = run_store.create_run("session-1", "run b").await;
    let tracker = Arc::new(crate::subagent_task_tracker::InMemorySubagentTaskTracker::new());

    let sink_a = RuntimeEventSink::new(RuntimeEventSinkConfig {
        run_store: Arc::clone(&run_store),
        run_id: run_a.id.clone(),
        session_id: "session-1".to_string(),
        hook_executor: None,
        security_provider: None,
        persistence_state: persistence_state(),
        active_tools: active_tools(),
        subagent_tasks: Arc::clone(&tracker),
    });
    let sink_b = RuntimeEventSink::new(RuntimeEventSinkConfig {
        run_store: Arc::clone(&run_store),
        run_id: run_b.id.clone(),
        session_id: "session-1".to_string(),
        hook_executor: None,
        security_provider: None,
        persistence_state: persistence_state(),
        active_tools: active_tools(),
        subagent_tasks: tracker,
    });

    let (runtime_tx_a, runtime_rx_a) = mpsc::channel(4);
    let (runtime_tx_b, runtime_rx_b) = mpsc::channel(4);
    let (agent_tx_a, barrier_a, agent_rx_a) = run_agent_event_channel(8);
    let (agent_tx_b, barrier_b, agent_rx_b) = run_agent_event_channel(8);
    let collector_a = sink_a.spawn_collector(runtime_rx_a, Some(agent_rx_a), None);
    let collector_b = sink_b.spawn_collector(runtime_rx_b, Some(agent_rx_b), None);

    // Run B is already active when a background child owned by Run A
    // finishes. The per-run sender must route the late event only to A.
    runtime_tx_b
        .send(AgentEvent::TextDelta {
            text: "run b active".to_string(),
        })
        .await
        .unwrap();
    agent_tx_a
        .send(AgentEvent::SubagentEnd {
            task_id: "late-task-a".to_string(),
            session_id: "task-run-late-task-a".to_string(),
            agent: "explore".to_string(),
            output: "late result".to_string(),
            success: true,
            finished_ms: 1,
        })
        .unwrap();
    barrier_a.flush().await;

    agent_tx_b
        .send(AgentEvent::SubagentStart {
            task_id: "task-b".to_string(),
            session_id: "task-run-task-b".to_string(),
            parent_session_id: "session-1".to_string(),
            agent: "explore".to_string(),
            description: "owned by b".to_string(),
            started_ms: 2,
        })
        .unwrap();
    barrier_b.flush().await;

    drop(runtime_tx_a);
    drop(runtime_tx_b);
    collector_a.await.unwrap();
    collector_b.await.unwrap();

    let events_a = run_store.events(&run_a.id).await;
    let events_b = run_store.events(&run_b.id).await;
    assert!(events_a.iter().any(|record| matches!(
        &record.event,
        AgentEvent::SubagentEnd { task_id, .. } if task_id == "late-task-a"
    )));
    assert!(!events_b.iter().any(|record| matches!(
        &record.event,
        AgentEvent::SubagentEnd { task_id, .. } if task_id == "late-task-a"
    )));
    assert!(events_b.iter().any(|record| matches!(
        &record.event,
        AgentEvent::SubagentStart { task_id, .. } if task_id == "task-b"
    )));
}

#[tokio::test]
async fn forwarder_exposes_delegated_confirmation_lifecycle() {
    let run_store = Arc::new(crate::run::InMemoryRunStore::new());
    let run = run_store.create_run("session-1", "prompt").await;
    let sink = RuntimeEventSink::new(RuntimeEventSinkConfig {
        run_store: Arc::clone(&run_store),
        run_id: run.id.clone(),
        session_id: "session-1".to_string(),
        hook_executor: None,
        security_provider: None,
        persistence_state: persistence_state(),
        active_tools: active_tools(),
        subagent_tasks: Arc::new(crate::subagent_task_tracker::InMemorySubagentTaskTracker::new()),
    });
    let (runtime_tx, runtime_rx) = mpsc::channel(4);
    let (stream_tx, mut stream_rx) = mpsc::channel(4);
    let (agent_tx, barrier, agent_rx) = run_agent_event_channel(8);
    let forwarder = sink.spawn_forwarder(runtime_rx, stream_tx, Some(agent_rx), None);

    let expected = vec![
        AgentEvent::ConfirmationRequired {
            tool_id: "child-tool-1".to_string(),
            tool_name: "install".to_string(),
            args: serde_json::json!({"component": "browser"}),
            timeout_ms: 30_000,
        },
        AgentEvent::ConfirmationReceived {
            tool_id: "child-tool-1".to_string(),
            approved: true,
            reason: Some("approved by parent".to_string()),
        },
        AgentEvent::ConfirmationTimeout {
            tool_id: "child-tool-2".to_string(),
            action_taken: "rejected".to_string(),
        },
    ];
    for event in &expected {
        agent_tx.send(event.clone()).unwrap();
    }
    barrier.flush().await;
    drop(agent_tx);
    drop(runtime_tx);
    forwarder.await.unwrap();

    let mut streamed = Vec::new();
    while let Some(event) = stream_rx.recv().await {
        streamed.push(event);
    }
    assert_eq!(
        serde_json::to_value(&streamed).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    let persisted = run_store
        .events(&run.id)
        .await
        .into_iter()
        .map(|record| record.event)
        .collect::<Vec<_>>();
    assert_eq!(
        serde_json::to_value(&persisted).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
}

#[tokio::test]
async fn forwarder_exposes_and_persists_only_sanitized_events() {
    let run_store = Arc::new(crate::run::InMemoryRunStore::new());
    let run = run_store.create_run("session-1", "prompt").await;
    let provider: Arc<dyn crate::security::SecurityProvider> =
        Arc::new(crate::security::DefaultSecurityProvider::new());
    let sink = RuntimeEventSink::new(RuntimeEventSinkConfig {
        run_store: Arc::clone(&run_store),
        run_id: run.id.clone(),
        session_id: "session-1".to_string(),
        hook_executor: None,
        security_provider: Some(provider),
        persistence_state: persistence_state(),
        active_tools: active_tools(),
        subagent_tasks: Arc::new(crate::subagent_task_tracker::InMemorySubagentTaskTracker::new()),
    });
    let (runtime_tx, runtime_rx) = mpsc::channel(4);
    let (stream_tx, mut stream_rx) = mpsc::channel(4);
    let forwarder = sink.spawn_forwarder(runtime_rx, stream_tx, None, None);

    runtime_tx
        .send(AgentEvent::ToolEnd {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            args: Some(serde_json::json!({"command": "echo user@example.com"})),
            output: "user@example.com".to_string(),
            exit_code: 0,
            metadata: None,
            error_kind: None,
        })
        .await
        .unwrap();
    drop(runtime_tx);
    forwarder.await.unwrap();

    let streamed = stream_rx.recv().await.unwrap();
    let persisted = run_store.events(&run.id).await;
    assert_eq!(persisted.len(), 1);
    for event in [&streamed, &persisted[0].event] {
        let json = serde_json::to_string(event).unwrap();
        assert!(
            !json.contains("user@example.com"),
            "unsanitized event: {json}"
        );
        assert!(json.contains("REDACTED:EMAIL"));
        assert!(json.contains("tool-1"));
    }
}

#[tokio::test]
async fn split_stream_secret_is_sanitized_before_stream_store_and_hooks() {
    let run_store = Arc::new(crate::run::InMemoryRunStore::new());
    let run = run_store.create_run("session-1", "prompt").await;
    let provider: Arc<dyn crate::security::SecurityProvider> =
        Arc::new(crate::security::DefaultSecurityProvider::new());
    let hook = Arc::new(RecordingRuntimeEvents::default());
    let hook_executor: Arc<dyn crate::hooks::HookExecutor> = hook.clone();
    let sink = RuntimeEventSink::new(RuntimeEventSinkConfig {
        run_store: Arc::clone(&run_store),
        run_id: run.id.clone(),
        session_id: "session-1".to_string(),
        hook_executor: Some(hook_executor),
        security_provider: Some(provider),
        persistence_state: persistence_state(),
        active_tools: active_tools(),
        subagent_tasks: Arc::new(crate::subagent_task_tracker::InMemorySubagentTaskTracker::new()),
    });
    let (runtime_tx, runtime_rx) = mpsc::channel(32);
    let (stream_tx, mut stream_rx) = mpsc::channel(32);
    let forwarder = sink.spawn_forwarder(runtime_rx, stream_tx, None, None);

    for event in [
        AgentEvent::TextDelta {
            text: "text@".to_string(),
        },
        AgentEvent::TextDelta {
            text: "example.com".to_string(),
        },
        AgentEvent::ReasoningDelta {
            text: "reasoning@".to_string(),
        },
        AgentEvent::ReasoningDelta {
            text: "example.com".to_string(),
        },
        AgentEvent::ToolInputDelta {
            id: Some("tool-1".to_string()),
            delta: "input@".to_string(),
        },
        AgentEvent::ToolInputDelta {
            id: Some("tool-1".to_string()),
            delta: "example.com".to_string(),
        },
        AgentEvent::ToolExecutionStart {
            id: "tool-1".to_string(),
            name: "test".to_string(),
            args: serde_json::json!({}),
        },
        AgentEvent::ToolOutputDelta {
            id: "tool-1".to_string(),
            name: "test".to_string(),
            delta: "output@".to_string(),
        },
        AgentEvent::ToolOutputDelta {
            id: "tool-1".to_string(),
            name: "test".to_string(),
            delta: "example.com".to_string(),
        },
        AgentEvent::ToolEnd {
            id: "tool-1".to_string(),
            name: "test".to_string(),
            args: Some(serde_json::json!({})),
            output: "done".to_string(),
            exit_code: 0,
            metadata: None,
            error_kind: None,
        },
        AgentEvent::End {
            text: "done".to_string(),
            usage: crate::llm::TokenUsage::default(),
            verification_summary: Box::new(crate::verification::VerificationSummary::from_reports(
                &[],
            )),
            meta: None,
        },
    ] {
        runtime_tx.send(event).await.unwrap();
    }
    drop(runtime_tx);
    forwarder.await.unwrap();

    let mut streamed = Vec::new();
    while let Some(event) = stream_rx.recv().await {
        streamed.push(event);
    }
    let persisted = run_store
        .events(&run.id)
        .await
        .into_iter()
        .map(|record| record.event)
        .collect::<Vec<_>>();
    let hooked = hook
        .events
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();

    for events in [&streamed, &persisted, &hooked] {
        let serialized = serde_json::to_string(events).unwrap();
        for secret in [
            "text@example.com",
            "reasoning@example.com",
            "input@example.com",
            "output@example.com",
        ] {
            assert!(!serialized.contains(secret), "unsanitized secret: {secret}");
        }
        assert_eq!(serialized.matches("REDACTED:EMAIL").count(), 4);
        assert!(matches!(events.last(), Some(AgentEvent::End { .. })));
    }
    assert_eq!(
        serde_json::to_value(&streamed).unwrap(),
        serde_json::to_value(&persisted).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&streamed).unwrap(),
        serde_json::to_value(&hooked).unwrap()
    );
}
