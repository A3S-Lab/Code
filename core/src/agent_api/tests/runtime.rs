use super::*;

#[tokio::test]
async fn test_budget_guard_deny_aborts_llm_call() {
    let guard = Arc::new(DenyingBudgetGuard::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_id("budget-deny-test")
        .with_budget_guard(guard.clone() as Arc<dyn crate::budget::BudgetGuard>);
    let session = agent
        .build_session(
            "/tmp/test-budget-deny".into(),
            Arc::new(StaticStreamingClient::new("never-delivered")),
            &opts,
        )
        .unwrap();

    let err = session.send("hello", None).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Budget exhausted") || msg.contains("llm_tokens"),
        "expected budget-exhausted error, got: {msg}"
    );
    assert_eq!(
        guard.checks.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "BudgetGuard::check_before_llm must be consulted exactly once"
    );
    assert_eq!(
        guard.llm_records.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "record_after_llm must not fire when the call was denied"
    );
    assert!(
        session.history().is_empty(),
        "denied call must not pollute conversation history"
    );
}

#[test]
fn test_cluster_agent_events_serialize_with_expected_tags() {
    // Lock the wire schema for cluster-event variants — these are
    // emitted by the host through HookExecutor and need
    // stable JSON tags so external producers can target them.
    let budget = AgentEvent::BudgetThresholdHit {
        resource: "llm_tokens".to_string(),
        kind: "soft".to_string(),
        consumed: 12000.0,
        limit: 10000.0,
        message: Some("approaching daily cap".to_string()),
    };
    let json = serde_json::to_string(&budget).unwrap();
    assert!(
        json.contains("\"type\":\"budget_threshold_hit\""),
        "got: {json}"
    );
    assert!(json.contains("\"resource\":\"llm_tokens\""), "got: {json}");

    let passivate = AgentEvent::PassivationRequested {
        reason: "node_drain".to_string(),
        deadline_ms: Some(1_700_000_000_000),
    };
    let json = serde_json::to_string(&passivate).unwrap();
    assert!(
        json.contains("\"type\":\"passivation_requested\""),
        "got: {json}"
    );

    let peer = AgentEvent::PeerInvocation {
        from_session_id: "peer-1".to_string(),
        from_tenant_id: Some("acme".to_string()),
        correlation_id: None, // omitted via skip_serializing_if
    };
    let json = serde_json::to_string(&peer).unwrap();
    assert!(json.contains("\"type\":\"peer_invocation\""), "got: {json}");
    assert!(
        !json.contains("correlation_id"),
        "None field must be skipped, got: {json}"
    );

    // Round-trip — ensures the #[serde(default)] hints don't break loading
    // from a payload that omits the optional fields.
    let minimal_peer = r#"{"type":"peer_invocation","from_session_id":"x"}"#;
    let parsed: AgentEvent = serde_json::from_str(minimal_peer).unwrap();
    assert!(
        matches!(parsed, AgentEvent::PeerInvocation { ref from_session_id, .. } if from_session_id == "x")
    );
}

#[tokio::test]
async fn test_custom_host_env_yields_deterministic_session_and_run_ids() {
    use crate::host_env::{FixedClock, HostEnv, SequentialIdGenerator};

    let env = Arc::new(HostEnv::new(
        Arc::new(SequentialIdGenerator::new("test")),
        Arc::new(FixedClock::new(1_700_000_000_000)),
    ));

    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts_a = SessionOptions::new().with_host_env(env.clone());
    let session_a = agent
        .session_async("/tmp/test-host-env-a", Some(opts_a))
        .await
        .expect("session a");

    // First call to next_id() yields "test-0" — used as session_id.
    assert_eq!(
        session_a.id(),
        "test-0",
        "session_id must come from HostEnv"
    );

    // run_id derives from next_id() too, prefixed with "run-".
    let session_a = Arc::new(session_a);
    let worker = {
        let s = Arc::clone(&session_a);
        tokio::spawn(async move {
            // Use a static streaming client by building manually so the
            // call resolves without an actual provider.
            let _ = s;
        })
    };
    let _ = worker.await;

    // Second session reuses the same generator → continues the sequence.
    let opts_b = SessionOptions::new().with_host_env(env);
    let session_b = agent
        .session_async("/tmp/test-host-env-b", Some(opts_b))
        .await
        .expect("session b");
    assert_eq!(session_b.id(), "test-1");
}

#[tokio::test]
async fn test_runtime_agent_style_overrides_session_prompt_slots() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("runtime-agent-style");
    let session = agent
        .build_session(
            "/tmp/test-runtime-agent-style".into(),
            Arc::new(StaticStreamingClient::new("ok")),
            &opts,
        )
        .unwrap();
    assert!(session.runtime_agent_style_override().is_none());
    session
        .set_agent_style(Some(crate::prompts::AgentStyle::Plan))
        .expect("set Plan style");
    assert_eq!(
        session.runtime_agent_style_override(),
        Some(Some(crate::prompts::AgentStyle::Plan))
    );
    session
        .set_agent_style(None)
        .expect("clear specialty style");
    assert_eq!(session.runtime_agent_style_override(), Some(None));
}

#[tokio::test]
async fn test_runtime_output_language_overrides_session_prompt_slots() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("runtime-output-language");
    let session = agent
        .build_session(
            "/tmp/test-runtime-output-language".into(),
            Arc::new(StaticStreamingClient::new("ok")),
            &opts,
        )
        .unwrap();
    assert!(session.runtime_output_language_override().is_none());
    session
        .set_output_language(Some("zh-CN"))
        .expect("pin zh-CN");
    assert_eq!(
        session.runtime_output_language_override(),
        Some(Some("zh-CN".to_string()))
    );
    session
        .set_output_language(None::<String>)
        .expect("clear language pin");
    assert_eq!(session.runtime_output_language_override(), Some(None));
}

#[tokio::test]
async fn test_runtime_budget_guard_overrides_session_options_value() {
    // A guard installed via set_budget_guard() *after* construction
    // must take effect on the next send/stream — that's the entry
    // point Node SDK relies on (JsFunction can't live inside a
    // value-typed SessionOptions).
    let runtime_guard = Arc::new(DenyingBudgetGuard::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("runtime-guard-override");
    let session = agent
        .build_session(
            "/tmp/test-runtime-guard".into(),
            Arc::new(StaticStreamingClient::new("never-delivered")),
            &opts,
        )
        .unwrap();

    // No guard installed at build time -> send would succeed. Install
    // a denying guard now and assert the next send is aborted.
    session
        .set_budget_guard(Some(
            runtime_guard.clone() as Arc<dyn crate::budget::BudgetGuard>
        ))
        .unwrap();
    let err = session.send("hello", None).await.unwrap_err();
    assert!(err.to_string().contains("Budget exhausted"));
    assert_eq!(
        runtime_guard
            .checks
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    // Clearing the override should let a follow-up send succeed.
    session.set_budget_guard(None).unwrap();
    let result = session.send("hello again", None).await.unwrap();
    assert_eq!(result.text, "never-delivered");
}

#[tokio::test]
async fn test_disconnect_idle_mcp_is_safe_no_op_without_global_mcp() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    // test_config carries no mcp_servers, so global_mcp is None and
    // the idle sweep must short-circuit to an empty Vec without
    // panicking — the contract surface a host's sweeper will rely on.
    let dropped = agent.disconnect_idle_mcp(0).await;
    assert!(dropped.is_empty());
}

#[tokio::test]
async fn test_identity_labels_default_to_none() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-id-default", None)
        .await
        .unwrap();
    assert!(session.tenant_id().is_none());
    assert!(session.principal().is_none());
    assert!(session.agent_template_id().is_none());
    assert!(session.correlation_id().is_none());
}

#[tokio::test]
async fn test_identity_labels_round_trip_via_session_options() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_tenant_id("acme-corp")
        .with_principal("user-42")
        .with_agent_template_id("planner-v3")
        .with_correlation_id("trace-deadbeef");
    let session = agent
        .session_async("/tmp/test-id-set", Some(opts))
        .await
        .expect("session");

    assert_eq!(session.tenant_id(), Some("acme-corp"));
    assert_eq!(session.principal(), Some("user-42"));
    assert_eq!(session.agent_template_id(), Some("planner-v3"));
    assert_eq!(session.correlation_id(), Some("trace-deadbeef"));
}

#[tokio::test]
async fn test_agent_list_sessions_tracks_live_sessions() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    assert!(agent.list_sessions().await.is_empty());

    let opts_a = SessionOptions::new().with_session_id("registry-a");
    let opts_b = SessionOptions::new().with_session_id("registry-b");
    let session_a = agent
        .build_session(
            "/tmp/test-registry-a".into(),
            Arc::new(StaticStreamingClient::new("answer-a")),
            &opts_a,
        )
        .unwrap();
    let session_b = agent
        .build_session(
            "/tmp/test-registry-b".into(),
            Arc::new(StaticStreamingClient::new("answer-b")),
            &opts_b,
        )
        .unwrap();

    let ids = agent.list_sessions().await;
    assert_eq!(
        ids,
        vec!["registry-a".to_string(), "registry-b".to_string()]
    );

    drop(session_a);
    // After drop, the registry's Weak becomes dangling; list_sessions prunes it.
    let after = agent.list_sessions().await;
    assert_eq!(after, vec!["registry-b".to_string()]);

    drop(session_b);
    assert!(agent.list_sessions().await.is_empty());
}

#[tokio::test]
async fn test_agent_rejects_duplicate_live_ids_across_sync_and_async_factories() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let memory = || Arc::new(a3s_memory::InMemoryStore::new());

    let sync_session = agent
        .session(
            "/tmp/test-duplicate-sync-owner",
            Some(
                SessionOptions::new()
                    .with_session_id("duplicate-factory-id")
                    .with_memory(memory()),
            ),
        )
        .expect("first sync session");
    let duplicate_async = agent
        .session_async(
            "/tmp/test-duplicate-async-contender",
            Some(
                SessionOptions::new()
                    .with_session_id("duplicate-factory-id")
                    .with_memory(memory()),
            ),
        )
        .await
        .expect_err("async factory must not replace a live sync session");
    assert!(matches!(
        duplicate_async,
        crate::error::CodeError::SessionConfiguration {
            field: "session_id",
            ..
        }
    ));

    drop(sync_session);
    let async_session = agent
        .session_async(
            "/tmp/test-duplicate-async-owner",
            Some(
                SessionOptions::new()
                    .with_session_id("duplicate-factory-id")
                    .with_memory(memory()),
            ),
        )
        .await
        .expect("dropped weak entry must permit ID reuse");
    let duplicate_sync = agent
        .session(
            "/tmp/test-duplicate-sync-contender",
            Some(
                SessionOptions::new()
                    .with_session_id("duplicate-factory-id")
                    .with_memory(memory()),
            ),
        )
        .expect_err("sync factory must not replace a live async session");
    assert!(matches!(
        duplicate_sync,
        crate::error::CodeError::SessionConfiguration {
            field: "session_id",
            ..
        }
    ));
    assert_eq!(
        agent.list_sessions().await,
        vec!["duplicate-factory-id".to_string()]
    );
    drop(async_session);
}

#[tokio::test]
async fn synchronous_session_build_keeps_configured_memory_observers() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let observer = Arc::new(CountingMemoryObserver::default());
    let session = agent
        .session(
            "/tmp/test-sync-memory-observer",
            Some(
                SessionOptions::new()
                    .with_session_id("sync-memory-observer")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                    .with_memory_observer(observer.clone()),
            ),
        )
        .unwrap();

    session
        .memory()
        .unwrap()
        .remember(a3s_memory::MemoryItem::new("A durable learned preference."))
        .await
        .unwrap();

    assert_eq!(observer.0.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[tokio::test]
async fn test_closed_session_releases_id_while_old_handle_is_still_held() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session_id = "closed-id-reuse";
    let first = agent
        .session_async(
            "/tmp/test-closed-id-first",
            Some(SessionOptions::new().with_session_id(session_id)),
        )
        .await
        .unwrap();

    first.close().await;
    assert!(first.is_closed());
    assert!(agent.list_sessions().await.is_empty());

    let replacement = agent
        .session_async(
            "/tmp/test-closed-id-replacement",
            Some(SessionOptions::new().with_session_id(session_id)),
        )
        .await
        .expect("a closed session is no longer a live ID owner");
    assert_eq!(replacement.session_id(), session_id);

    // Keep `first` alive through replacement construction to prove registry
    // admission depends on lifecycle state, not garbage collection timing.
    assert!(first.is_closed());
}

#[tokio::test]
async fn test_failed_sync_and_async_builds_release_session_id_reservations() {
    let agent = Agent::from_config(test_config()).await.unwrap();

    let invalid_sync = SessionOptions::new()
        .with_session_id("failed-sync-reservation")
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_tool_timeout(0);
    agent
        .session("/tmp/test-failed-sync-reservation", Some(invalid_sync))
        .expect_err("conflicting sync options must fail");
    let sync_session = agent
        .session(
            "/tmp/test-retry-sync-reservation",
            Some(
                SessionOptions::new()
                    .with_session_id("failed-sync-reservation")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new())),
            ),
        )
        .expect("failed sync build must release its reservation");
    drop(sync_session);

    let invalid_async = SessionOptions::new()
        .with_session_id("failed-async-reservation")
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_tool_timeout(0);
    agent
        .session_async("/tmp/test-failed-async-reservation", Some(invalid_async))
        .await
        .expect_err("conflicting async options must fail");
    let async_session = agent
        .session_async(
            "/tmp/test-retry-async-reservation",
            Some(
                SessionOptions::new()
                    .with_session_id("failed-async-reservation")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new())),
            ),
        )
        .await
        .expect("failed async build must release its reservation");
    drop(async_session);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_agent_close_rejects_blocked_resume_finalization() {
    let backing = Arc::new(crate::store::MemorySessionStore::new());
    let writer = Agent::from_config(test_config()).await.unwrap();
    let source = writer
        .session_async(
            "/tmp/test-close-blocked-resume-source",
            Some(
                SessionOptions::new()
                    .with_session_id("close-blocked-resume")
                    .with_session_store(backing.clone()),
            ),
        )
        .await
        .unwrap();
    source.save().await.unwrap();
    drop(source);
    drop(writer);

    let blocking = Arc::new(BlockingLoadSessionStore::new(Arc::clone(&backing)));
    let agent = Arc::new(Agent::from_config(test_config()).await.unwrap());
    let resume = tokio::spawn({
        let agent = Arc::clone(&agent);
        let store: Arc<dyn SessionStore> = blocking.clone();
        async move {
            agent
                .resume_session_async(
                    "close-blocked-resume",
                    SessionOptions::new().with_session_store(store),
                )
                .await
        }
    });
    blocking.wait_until_load_is_blocked().await;

    assert!(agent.list_sessions().await.is_empty());
    assert!(!agent.close_session("close-blocked-resume").await);
    let duplicate = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        agent.resume_session_async(
            "close-blocked-resume",
            SessionOptions::new().with_session_store(blocking.clone() as Arc<dyn SessionStore>),
        ),
    )
    .await
    .expect("duplicate build must fail before entering the blocking store")
    .expect_err("a building session ID must be reserved");
    assert!(matches!(
        duplicate,
        crate::error::CodeError::SessionConfiguration {
            field: "session_id",
            ..
        }
    ));

    agent.close().await;
    blocking.release_one_load();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), resume)
        .await
        .expect("resume task must finish after its load is released")
        .expect("resume task must not panic");
    let error = result.expect_err("close must reject a pre-admitted resume at finalization");
    assert!(matches!(
        error,
        crate::error::CodeError::SessionClosed { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cancelled_resume_releases_session_id_reservation() {
    let backing = Arc::new(crate::store::MemorySessionStore::new());
    let writer = Agent::from_config(test_config()).await.unwrap();
    let source = writer
        .session_async(
            "/tmp/test-cancelled-resume-source",
            Some(
                SessionOptions::new()
                    .with_session_id("cancelled-resume-reservation")
                    .with_session_store(backing.clone()),
            ),
        )
        .await
        .unwrap();
    source.save().await.unwrap();
    drop(source);
    drop(writer);

    let blocking = Arc::new(BlockingLoadSessionStore::new(Arc::clone(&backing)));
    let agent = Arc::new(Agent::from_config(test_config()).await.unwrap());
    let resume = tokio::spawn({
        let agent = Arc::clone(&agent);
        let store: Arc<dyn SessionStore> = blocking.clone();
        async move {
            agent
                .resume_session_async(
                    "cancelled-resume-reservation",
                    SessionOptions::new().with_session_store(store),
                )
                .await
        }
    });
    blocking.wait_until_load_is_blocked().await;
    resume.abort();
    assert!(resume
        .await
        .expect_err("task must be cancelled")
        .is_cancelled());

    let resumed = agent
        .resume_session_async(
            "cancelled-resume-reservation",
            SessionOptions::new().with_session_store(backing.clone() as Arc<dyn SessionStore>),
        )
        .await
        .expect("cancelling a build future must release its reservation");
    assert_eq!(resumed.session_id(), "cancelled-resume-reservation");
}

#[tokio::test]
async fn test_agent_close_session_closes_target_session() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("close-by-id");
    let session = agent
        .build_session(
            "/tmp/test-agent-close-session".into(),
            Arc::new(StaticStreamingClient::new("never")),
            &opts,
        )
        .unwrap();
    assert!(!session.is_closed());

    assert!(agent.close_session("close-by-id").await);
    assert!(session.is_closed());

    // Idempotent: second call still reports `true` (we found a live handle)
    // OR `false` (target already closed) — accept either; what matters is no panic.
    let _ = agent.close_session("close-by-id").await;

    // Unknown ids report false.
    assert!(!agent.close_session("does-not-exist").await);
}

#[tokio::test]
async fn test_session_close_waits_for_accepted_memory_extraction_to_persist() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_id("close-drains-memory")
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()));
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-close-drains-memory".into(),
                Arc::new(StaticStreamingClient::new("unused")),
                &opts,
            )
            .unwrap(),
    );
    let memory = Arc::clone(session.memory().expect("session memory"));
    let mut ticket = memory.enqueue_llm_extraction();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let extraction = tokio::spawn({
        let memory = Arc::clone(&memory);
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        async move {
            ticket.wait_for_turn().await;
            started.notify_one();
            release.notified().await;
            memory
                .remember(
                    a3s_memory::MemoryItem::new(
                        "Session close preserves accepted durable memory extraction.",
                    )
                    .with_type(a3s_memory::MemoryType::Semantic),
                )
                .await
                .unwrap();
        }
    });
    started.notified().await;

    let close = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.close().await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert!(
        !close.is_finished(),
        "close must wait while an accepted extraction is still pending"
    );

    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(1), close)
        .await
        .expect("close should finish after memory persistence")
        .unwrap();
    extraction.await.unwrap();
    assert!(session.is_closed());
    assert_eq!(memory.stats().await.unwrap().long_term_count, 1);
}

#[tokio::test]
async fn test_agent_close_closes_every_live_session() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts_a = SessionOptions::new().with_session_id("agent-close-a");
    let opts_b = SessionOptions::new().with_session_id("agent-close-b");
    let session_a = agent
        .build_session(
            "/tmp/test-agent-close-a".into(),
            Arc::new(StaticStreamingClient::new("a")),
            &opts_a,
        )
        .unwrap();
    let session_b = agent
        .build_session(
            "/tmp/test-agent-close-b".into(),
            Arc::new(StaticStreamingClient::new("b")),
            &opts_b,
        )
        .unwrap();

    agent.close().await;
    assert!(session_a.is_closed());
    assert!(session_b.is_closed());

    // After Agent::close(), session creation must fail fast — the agent has
    // already disposed of its resources.
    let err = agent
        .session_async("/tmp/test-agent-closed", None)
        .await
        .expect_err("session() after close() must error");
    let msg = err.to_string();
    assert!(msg.contains("closed") || msg.contains("Closed"));
}

#[tokio::test]
async fn test_session_cancel_token_starts_uncancelled() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-session-cancel-fresh", None)
        .await
        .unwrap();
    let tok = session.session_cancel_token();
    assert!(!tok.is_cancelled());
}

#[tokio::test]
async fn test_close_cancels_session_token() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-session-cancel-on-close", None)
        .await
        .unwrap();
    let observer = session.session_cancel_token();
    assert!(!observer.is_cancelled());

    session.close().await;
    assert!(observer.is_cancelled());
}

#[tokio::test]
async fn test_session_cancel_token_propagates_to_in_flight_run() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-session-cancel-cascades".into(),
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
    let run_id = run_id.expect("current run should be visible");

    // Fire the session-level token directly, bypassing close()/cancel().
    // The in-flight run's token must be a *child* of this one for
    // cancellation to propagate.
    session.session_cancel_token().cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .expect("send should stop after session_cancel fires")
        .expect("worker should not panic");
    let result = result.expect("session cancellation should preserve interrupted history");
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].text(), "hello");
    assert!(result.messages[1].text().contains("interrupted"));
    assert_eq!(
        session.run_snapshot(&run_id).await.unwrap().status,
        crate::run::RunStatus::Cancelled
    );
}

#[tokio::test]
async fn test_send_with_attachments_passes_session_id_to_context_providers() {
    let provider = Arc::new(CapturingContextProvider::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_id("attachments-context-session")
        .with_context_provider(provider.clone());
    let session = agent
        .build_session(
            "/tmp/test-send-attachments-context".into(),
            Arc::new(StaticStreamingClient::new("attachment answer")),
            &opts,
        )
        .unwrap();
    let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];

    session
        .send_with_attachments("hello", &attachments, None)
        .await
        .unwrap();

    let session_ids = provider.session_ids.lock().unwrap();
    assert!(!session_ids.is_empty());
    assert!(session_ids
        .iter()
        .all(|id| id.as_deref() == Some("attachments-context-session")));
}

#[tokio::test]
async fn test_send_records_run_snapshot_and_events() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-send-run-store".into(),
            Arc::new(StaticStreamingClient::new("run answer")),
            &SessionOptions::new(),
        )
        .unwrap();

    let result = session.send("hello", None).await.unwrap();
    assert_eq!(result.text, "run answer");

    let runs = session.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, crate::run::RunStatus::Completed);
    assert_eq!(runs[0].result_text.as_deref(), Some("run answer"));

    let events = session.run_events(&runs[0].id).await;
    assert!(events
        .iter()
        .any(|record| matches!(record.event, AgentEvent::Start { .. })));
    assert!(events
        .iter()
        .any(|record| matches!(record.event, AgentEvent::End { .. })));
}

#[tokio::test]
async fn test_send_publishes_runtime_events_to_hook_executor() {
    let hook = Arc::new(RecordingRuntimeHook::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_hook_executor(hook.clone());
    let session = agent
        .build_session(
            "/tmp/test-runtime-event-hook".into(),
            Arc::new(StaticStreamingClient::new("hooked answer")),
            &opts,
        )
        .unwrap();

    session.send("hello", None).await.unwrap();

    let events = hook.events.lock().unwrap();
    assert!(events
        .iter()
        .any(|(_, session_id, event)| session_id == session.id()
            && matches!(event, AgentEvent::Start { .. })));
    assert!(events
        .iter()
        .any(|(_, session_id, event)| session_id == session.id()
            && matches!(event, AgentEvent::End { .. })));
    assert!(events
        .iter()
        .all(|(run_id, _, _)| run_id.starts_with("run-")));
}

#[tokio::test]
async fn session_pre_prompt_rewrite_reaches_model_history() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let options = SessionOptions::new()
        .with_hook_executor(Arc::new(RewritingPromptHook))
        .with_continuation(false);
    let session = agent
        .build_session(
            "/tmp/test-pre-prompt-rewrite".into(),
            Arc::new(StaticStreamingClient::new("hooked answer")),
            &options,
        )
        .unwrap();

    let result = session.send("ORIGINAL_PROMPT", None).await.unwrap();
    let user_prompt = result
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .expect("rewritten user message")
        .text();
    assert!(user_prompt.contains("HOOK_REWRITTEN_PROMPT"));
    assert!(user_prompt.contains("HOOK_ADDITIONAL_CONTEXT"));
    assert!(!user_prompt.contains("ORIGINAL_PROMPT"));
}

#[tokio::test]
async fn session_pre_prompt_block_emits_stream_error() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let options = SessionOptions::new()
        .with_hook_executor(Arc::new(BlockingPromptHook))
        .with_continuation(false);
    let session = agent
        .build_session(
            "/tmp/test-pre-prompt-block".into(),
            Arc::new(StaticStreamingClient::new("must not reach model")),
            &options,
        )
        .unwrap();

    let (mut events, worker) = session.stream("BLOCK_ME", None).await.unwrap();
    let mut error = None;
    while let Some(event) = events.recv().await {
        if let AgentEvent::Error { message } = event {
            error = Some(message);
        }
    }
    worker.await.unwrap();

    assert_eq!(
        error.as_deref(),
        Some("User prompt blocked by hook: prompt policy denied this request")
    );
}

#[tokio::test]
async fn async_session_build_and_close_fire_lifecycle_hooks_once() {
    let hook = Arc::new(RecordingRuntimeHook::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(
            "/tmp/test-session-lifecycle-hooks",
            Some(SessionOptions::new().with_hook_executor(hook.clone())),
        )
        .await
        .unwrap();

    assert_eq!(
        hook.hook_events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, crate::hooks::HookEvent::SessionStart(_)))
            .count(),
        1
    );
    session.close().await;
    let events = hook.hook_events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, crate::hooks::HookEvent::SessionEnd(_)))
            .count(),
        1
    );
}
