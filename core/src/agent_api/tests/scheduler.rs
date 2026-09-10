use super::*;

#[tokio::test]
async fn test_from_config() {
    let agent = Agent::from_config(test_config()).await;
    assert!(agent.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scheduler_prioritizes_interactive_work_across_sessions() {
    let workspace = tempfile::tempdir().unwrap();
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let client = Arc::new(TaskSchedulerProbeClient {
        started: started_tx,
        release: Arc::clone(&release),
    });
    let mut config = test_config();
    config.task_scheduler = crate::task_scheduler::TaskSchedulerConfig {
        max_active: 1,
        aging_interval_ms: 60_000,
    };
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();

    let make_session = |id: &str, priority| {
        SessionOptions::new()
            .with_session_id(id)
            .with_llm_client(client.clone())
            .with_planning_mode(crate::prompts::PlanningMode::Disabled)
            .with_continuation(false)
            .with_task_priority(priority)
    };
    let blocker = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(make_session(
                    "scheduler-blocker",
                    crate::task_scheduler::TaskPriority::Foreground,
                )),
            )
            .await
            .unwrap(),
    );
    let background = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(make_session(
                    "scheduler-background",
                    crate::task_scheduler::TaskPriority::Background,
                )),
            )
            .await
            .unwrap(),
    );
    let interactive = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(make_session(
                    "scheduler-interactive",
                    crate::task_scheduler::TaskPriority::Interactive,
                )),
            )
            .await
            .unwrap(),
    );

    let blocker_run = tokio::spawn({
        let blocker = Arc::clone(&blocker);
        async move { blocker.send("blocker", None).await }
    });
    assert_eq!(started_rx.recv().await.as_deref(), Some("blocker"));

    let background_run = tokio::spawn({
        let background = Arc::clone(&background);
        async move { background.send("background", None).await }
    });
    while agent.task_scheduler_stats().await.unwrap().pending < 1 {
        tokio::task::yield_now().await;
    }
    let interactive_run = tokio::spawn({
        let interactive = Arc::clone(&interactive);
        async move { interactive.send("interactive", None).await }
    });
    while agent.task_scheduler_stats().await.unwrap().pending < 2 {
        tokio::task::yield_now().await;
    }
    let queued = agent.task_scheduler_stats().await.unwrap();
    assert_eq!(queued.pending_by_priority.background, 1);
    assert_eq!(queued.pending_by_priority.interactive, 1);

    release.add_permits(1);
    assert_eq!(started_rx.recv().await.as_deref(), Some("interactive"));
    release.add_permits(1);
    assert_eq!(started_rx.recv().await.as_deref(), Some("background"));
    release.add_permits(1);

    blocker_run.await.unwrap().unwrap();
    interactive_run.await.unwrap().unwrap();
    background_run.await.unwrap().unwrap();
    assert_eq!(agent.task_scheduler_stats().await.unwrap().active, 0);
    agent.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_direct_tools_share_the_agent_scheduler_with_conversation_runs() {
    let workspace = tempfile::tempdir().unwrap();
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let client = Arc::new(TaskSchedulerProbeClient {
        started: started_tx,
        release: Arc::clone(&release),
    });
    let mut config = test_config();
    config.task_scheduler = crate::task_scheduler::TaskSchedulerConfig {
        max_active: 1,
        aging_interval_ms: 60_000,
    };
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();
    let base_options = |id: &str| {
        SessionOptions::new()
            .with_session_id(id)
            .with_llm_client(client.clone())
            .with_planning_mode(crate::prompts::PlanningMode::Disabled)
            .with_continuation(false)
    };
    let conversation = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(base_options("direct-scheduler-conversation")),
            )
            .await
            .unwrap(),
    );
    let direct = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(base_options("direct-scheduler-tool")),
            )
            .await
            .unwrap(),
    );
    direct
        .register_dynamic_tool(Arc::new(NamedSessionTool("scheduled-direct".to_string())))
        .unwrap();

    let conversation_run = tokio::spawn({
        let conversation = Arc::clone(&conversation);
        async move { conversation.send("hold-global-slot", None).await }
    });
    assert_eq!(started_rx.recv().await.as_deref(), Some("hold-global-slot"));
    let direct_run = tokio::spawn({
        let direct = Arc::clone(&direct);
        async move { direct.tool("scheduled-direct", serde_json::json!({})).await }
    });
    while agent.task_scheduler_stats().await.unwrap().pending < 1 {
        tokio::task::yield_now().await;
    }
    assert!(!direct_run.is_finished());

    release.add_permits(1);
    conversation_run.await.unwrap().unwrap();
    assert_eq!(direct_run.await.unwrap().unwrap().output, "ok");
    assert_eq!(agent.task_scheduler_stats().await.unwrap().active, 0);
    agent.close().await;
}

#[tokio::test]
async fn test_session_default() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async("/tmp/test-workspace", None).await;
    assert!(session.is_ok());
    let session = session.unwrap();
    let debug = format!("{:?}", session);
    assert!(debug.contains("AgentSession"));
    let health = session.model_middleware_health();
    assert_eq!(health.trust_admitted, 0);
    assert_eq!(health.trust_rejected, 0);
    let encoded = serde_json::to_value(&health).expect("middleware health must serialize");
    assert_eq!(encoded["trustAdmitted"], 0);
    assert!(encoded.get("prompt").is_none());
    assert!(encoded.get("toolResult").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_session_direct_generations_share_provider_admission() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(SessionAdmissionClient::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(
            dir.path().to_string_lossy().to_string(),
            Some(SessionOptions::new().with_llm_client(client.clone())),
        )
        .await
        .unwrap();
    let args = serde_json::json!({
        "schema": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "ok": {"type": "boolean"}
            },
            "required": ["ok"]
        },
        "schema_name": "session_admission",
        "prompt": "Return an object whose ok field is true.",
        "mode": "prompt",
        "max_repair_attempts": 0,
        "timeout_ms": 2_000
    });

    let (first, second) = tokio::join!(
        session.tool("generate_object", args.clone()),
        session.tool("generate_object", args)
    );
    let results = [first.unwrap(), second.unwrap()];

    assert!(results
        .iter()
        .all(|result| result.exit_code == 0 && result.output.contains(r#""ok":true"#)));
    assert_eq!(
        client.max_active.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "all host-direct loops in one session must share the provider gate"
    );
    let mut queue_waits = results
        .iter()
        .map(|result| {
            result
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/generation_admission/queue_wait_ms"))
                .and_then(serde_json::Value::as_u64)
                .expect("generation admission metadata")
        })
        .collect::<Vec<_>>();
    queue_waits.sort_unstable();
    assert!(
        queue_waits[1] >= 80,
        "the second direct generation should wait for session capacity: {queue_waits:?}"
    );
    let pool_health = session
        .model_generation_pool_health()
        .await
        .unwrap()
        .expect("the test client publishes a provider pool");
    let scheduler_health = pool_health
        .scheduler
        .expect("session provider admission is scheduler-bound");
    assert!(scheduler_health.observed);
    assert!(!scheduler_health.live);
    assert_eq!(scheduler_health.active, 0);
    assert_eq!(scheduler_health.pending, 0);
    assert_eq!(scheduler_health.admitted, 2);
    assert_eq!(scheduler_health.released, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_sessions_share_scheduler_backed_provider_generation_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(SessionAdmissionClient::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let options = |id: &str| {
        SessionOptions::new()
            .with_session_id(id)
            .with_llm_client(client.clone())
            .with_planning_mode(crate::prompts::PlanningMode::Disabled)
            .with_continuation(false)
    };
    let first = Arc::new(
        agent
            .session_async(dir.path().to_string_lossy(), Some(options("provider-a")))
            .await
            .unwrap(),
    );
    let second = Arc::new(
        agent
            .session_async(dir.path().to_string_lossy(), Some(options("provider-b")))
            .await
            .unwrap(),
    );
    let args = serde_json::json!({
        "schema": {
            "type": "object",
            "additionalProperties": false,
            "properties": {"ok": {"type": "boolean"}},
            "required": ["ok"]
        },
        "schema_name": "shared_provider",
        "prompt": "Return an object whose ok field is true.",
        "mode": "prompt",
        "max_repair_attempts": 0,
        "timeout_ms": 2_000
    });
    let (first_result, second_result) = tokio::join!(
        first.tool("generate_object", args.clone()),
        second.tool("generate_object", args)
    );
    assert!(first_result.unwrap().exit_code == 0);
    assert!(second_result.unwrap().exit_code == 0);
    assert_eq!(
        client.max_active.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "sessions sharing one provider pool must not exceed its capacity"
    );
    let pool_health = first
        .model_generation_pool_health()
        .await
        .unwrap()
        .expect("the test client publishes a provider pool");
    let scheduler_health = pool_health
        .scheduler
        .expect("session provider admission is scheduler-bound");
    assert!(scheduler_health.observed);
    assert!(scheduler_health.admitted >= 2);
    assert!(scheduler_health.released >= 2);
    agent.close().await;
}
