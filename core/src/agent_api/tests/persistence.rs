use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn test_session_has_id() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async("/tmp/test-ws-id", None).await.unwrap();
    // Auto-generated UUID
    assert!(!session.session_id().is_empty());
    assert_eq!(session.session_id().len(), 36); // UUID format
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_explicit_id() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("my-session-42");
    let session = agent
        .session_async("/tmp/test-ws-eid", Some(opts))
        .await
        .unwrap();
    assert_eq!(session.session_id(), "my-session-42");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_artifact_store_limits_option() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts =
        SessionOptions::new().with_artifact_store_limits(crate::tools::ArtifactStoreLimits {
            max_artifacts: 3,
            max_bytes: 4096,
        });
    let session = agent
        .session_async("/tmp/test-ws-artifact-limits", Some(opts))
        .await
        .unwrap();

    let limits = session.tool_executor.artifact_store().limits();
    assert_eq!(limits.max_artifacts, 3);
    assert_eq!(limits.max_bytes, 4096);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_save_no_store() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-save", None)
        .await
        .unwrap();
    // save() is a no-op when no store is configured
    session.save().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_save_and_load() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("persist-test");
    let session = agent
        .session_async("/tmp/test-ws-persist", Some(opts))
        .await
        .unwrap();

    // Save empty session
    session.save().await.unwrap();

    // Verify it was stored
    assert!(store.exists("persist-test").await.unwrap());

    let data = store.load("persist-test").await.unwrap().unwrap();
    assert_eq!(data.id, "persist-test");
    assert!(data.messages.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_save_persists_runtime_config() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let queue_config = SessionQueueConfig::default().with_metrics();
    let confirmation_policy = crate::hitl::ConfirmationPolicy::enabled();
    let permission_policy = crate::permissions::PermissionPolicy::new().allow("bash(echo:*)");

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("runtime-config-test")
        .with_model("openai/gpt-4o")
        .with_queue_config(queue_config)
        .with_confirmation_policy(confirmation_policy)
        .with_permission_policy(permission_policy)
        .with_active_skill_tool_restrictions(true);
    let session = agent
        .session_async("/tmp/test-ws-runtime-config", Some(opts))
        .await
        .unwrap();
    session.save().await.unwrap();

    let data = store.load("runtime-config-test").await.unwrap().unwrap();
    assert_eq!(data.model_name.as_deref(), Some("openai/gpt-4o"));
    assert_eq!(
        data.llm_config.as_ref().map(|c| c.provider.as_str()),
        Some("openai")
    );
    assert_eq!(
        data.llm_config.as_ref().map(|c| c.model.as_str()),
        Some("gpt-4o")
    );
    assert!(data.config.queue_config.is_some());
    assert!(data
        .config
        .confirmation_policy
        .as_ref()
        .is_some_and(|p| p.enabled));
    assert!(data
        .config
        .permission_policy
        .as_ref()
        .is_some_and(|p| !p.allow.is_empty()));
    assert!(data.config.enforce_active_skill_tool_restrictions);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_restores_runtime_config() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let queue_config = SessionQueueConfig::default().with_metrics();
    let confirmation_policy = crate::hitl::ConfirmationPolicy::enabled();
    let permission_policy = crate::permissions::PermissionPolicy::new().allow("bash(echo:*)");

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-runtime-config-test")
        .with_model("openai/gpt-4o")
        .with_queue_config(queue_config)
        .with_confirmation_policy(confirmation_policy)
        .with_permission_policy(permission_policy)
        .with_active_skill_tool_restrictions(true);
    let session = agent
        .session_async("/tmp/test-ws-resume-runtime", Some(opts))
        .await
        .unwrap();
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-runtime-config-test", opts2)
        .await
        .unwrap();

    assert_eq!(resumed.model_name, "openai/gpt-4o");
    assert!(resumed.has_queue());
    assert!(resumed.config.confirmation_policy.is_some());
    assert!(resumed.config.confirmation_manager.is_some());
    assert!(resumed.config.permission_policy.is_some());
    assert!(resumed.config.permission_checker.is_some());
    assert!(resumed.config.enforce_active_skill_tool_restrictions);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_save_with_history() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("history-test");
    let session = agent
        .session_async("/tmp/test-ws-hist", Some(opts))
        .await
        .unwrap();

    // Manually inject history
    {
        let mut h = session.history.write().unwrap();
        h.push(Message::user("Hello"));
        h.push(Message::user("How are you?"));
    }

    session.save().await.unwrap();

    let data = store.load("history-test").await.unwrap().unwrap();
    assert_eq!(data.messages.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    // Create and save a session with history
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-test");
    let session = agent
        .session_async("/tmp/test-ws-resume", Some(opts))
        .await
        .unwrap();
    {
        let mut h = session.history.write().unwrap();
        h.push(Message::user("What is Rust?"));
        h.push(Message::user("Tell me more"));
    }
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    // Resume the session
    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-test", opts2)
        .await
        .unwrap();

    assert_eq!(resumed.session_id(), "resume-test");
    let history = resumed.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].text(), "What is Rust?");
}

#[tokio::test]
async fn test_session_snapshots_accumulate_completed_run_usage() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("usage-snapshot-test");
    let session = agent
        .build_session(
            "/tmp/test-usage-snapshot".into(),
            Arc::new(StaticStreamingClient::new("done")),
            &opts,
        )
        .unwrap();

    session.send("first", None).await.unwrap();
    session.save().await.unwrap();
    let session_store: Arc<dyn crate::store::SessionStore> = store.clone();
    let first = session_store
        .load_snapshot("usage-snapshot-test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.session.total_usage.total_tokens, 2);

    session.send("second", None).await.unwrap();
    session.save().await.unwrap();
    let second = session_store
        .load_snapshot("usage-snapshot-test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.session.total_usage.prompt_tokens, 2);
    assert_eq!(second.session.total_usage.completion_tokens, 2);
    assert_eq!(second.session.total_usage.total_tokens, 4);
    assert_eq!(second.session.created_at, first.session.created_at);
}

#[tokio::test]
async fn test_session_uses_finite_retention_by_default_and_allows_explicit_unbounded() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let default_session = agent
        .build_session(
            "/tmp/test-default-retention".into(),
            Arc::new(StaticStreamingClient::new("unused")),
            &SessionOptions::new(),
        )
        .unwrap();
    for index in 0..65 {
        default_session
            .run_store
            .create_run(default_session.id(), &format!("run {index}"))
            .await;
    }
    assert_eq!(default_session.runs().await.len(), 64);

    let unbounded_session = agent
        .build_session(
            "/tmp/test-unbounded-retention".into(),
            Arc::new(StaticStreamingClient::new("unused")),
            &SessionOptions::new()
                .with_retention_limits(crate::retention::SessionRetentionLimits::unbounded()),
        )
        .unwrap();
    for index in 0..65 {
        unbounded_session
            .run_store
            .create_run(unbounded_session.id(), &format!("run {index}"))
            .await;
    }
    assert_eq!(unbounded_session.runs().await.len(), 65);
}

/// H4 regression: a run that completes in-process must DELETE its loop
/// checkpoint (the checkpoint exists only to survive a crash). Before
/// the fix, every tool-using run leaked a checkpoint forever.
///
/// We use a deterministic HostEnv so the run id is predictable, seed a
/// checkpoint under that id, run a (no-tool) send that completes through
/// the normal lifecycle, and assert the checkpoint was cleared.
#[tokio::test(flavor = "multi_thread")]
async fn test_completed_run_clears_its_loop_checkpoint() {
    use crate::host_env::{HostEnv, SequentialIdGenerator, SystemClock};
    use crate::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};

    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    // Deterministic ids: session_id is set explicitly (consumes no
    // counter), so the first next_id() goes to the run -> "run-seq-0".
    let env = Arc::new(HostEnv::new(
        Arc::new(SequentialIdGenerator::new("seq")),
        Arc::new(SystemClock),
    ));
    let opts = SessionOptions::new()
        .with_session_id("ckpt-clear-session")
        .with_session_store(store.clone() as Arc<dyn crate::store::SessionStore>)
        .with_host_env(env);
    let session = agent
        .build_session(
            "/tmp/test-ckpt-clear".into(),
            Arc::new(StaticStreamingClient::new("done")),
            &opts,
        )
        .unwrap();

    // Seed a checkpoint under the run id this send will use.
    let predicted_run_id = "run-seq-0";
    let cp_store: Arc<dyn crate::store::SessionStore> = store.clone();
    cp_store
        .save_loop_checkpoint(
            predicted_run_id,
            &LoopCheckpoint {
                schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
                run_id: predicted_run_id.to_string(),
                session_id: "ckpt-clear-session".to_string(),
                capability_binding: None,
                turn: 1,
                messages: vec![Message::user("seed")],
                total_usage: crate::llm::TokenUsage::default(),
                tool_calls_count: 0,
                verification_reports: Vec::new(),
                convergence: Default::default(),
                checkpoint_ms: 1,
            },
        )
        .await
        .unwrap();

    let result = session.send("hello", None).await.unwrap();
    assert_eq!(result.text, "done");

    // Self-document the predicted run id.
    let runs = session.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, predicted_run_id, "run id must be deterministic");

    // The checkpoint must have been cleared by the run lifecycle.
    let after: Arc<dyn crate::store::SessionStore> = store.clone();
    assert!(
        after
            .load_loop_checkpoint(predicted_run_id)
            .await
            .unwrap()
            .is_none(),
        "completed run must delete its loop checkpoint (else unbounded leak)"
    );
}

/// P3 happy path (cut 2 E2E): a manually-seeded `LoopCheckpoint` in
/// the SessionStore can be picked up by `AgentSession::resume_run`,
/// the loop runs from the checkpoint's message vec (no new user
/// prompt is appended — `execute_from_messages` path), and the
/// resumed run is allocated a **fresh** run id (not the
/// checkpoint's).
///
/// This exercises the contract surface the host will sit on: write a
/// checkpoint on node A, hand the run id to node B which builds a
/// session against the shared store and calls `resume_run`. Crash
/// simulation is reduced to a manual checkpoint seed because the
/// in-process agent loop has no "die mid-round" affordance suitable
/// for unit testing.
#[tokio::test(flavor = "multi_thread")]
async fn test_resume_run_picks_up_from_persisted_checkpoint() {
    use crate::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};

    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    // Seed a checkpoint as if a previous run on another node had
    // completed one tool round and persisted the boundary state.
    let seeded_run_id = "ckpt-old-run-x";
    let seeded_messages = vec![
        Message::user("kick off"),
        Message {
            role: "assistant".to_string(),
            content: vec![crate::llm::ContentBlock::Text {
                text: "intermediate work".to_string(),
            }],
            reasoning_content: None,
        },
    ];
    // Seed NON-ZERO cumulative metrics so the test can detect whether
    // resume_run carries them forward (H2 regression: it used to reset
    // them to zero, under-reporting the resumed AgentResult).
    let checkpoint = LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: seeded_run_id.to_string(),
        session_id: "resume-run-target".to_string(),
        capability_binding: None,
        turn: 1,
        messages: seeded_messages.clone(),
        total_usage: crate::llm::TokenUsage {
            prompt_tokens: 800,
            completion_tokens: 200,
            total_tokens: 1000,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        tool_calls_count: 3,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1_700_000_000_000,
    };
    {
        let cp_store: Arc<dyn crate::store::SessionStore> = store.clone();
        cp_store
            .save_loop_checkpoint(seeded_run_id, &checkpoint)
            .await
            .expect("seed checkpoint");
    }

    // Build a session bound to the same store + a mock LLM that
    // produces a final-answer text. resume_run will feed it the
    // seeded `messages` and the loop should finish on this turn.
    let opts = SessionOptions::new()
        .with_session_store(store.clone() as Arc<dyn crate::store::SessionStore>)
        .with_session_id("resume-run-target");
    let session = agent
        .build_session(
            "/tmp/test-resume-run-target".into(),
            Arc::new(StaticStreamingClient::new("resumed and completed")),
            &opts,
        )
        .unwrap();

    let result = session
        .resume_run(seeded_run_id)
        .await
        .expect("resume_run must succeed");
    assert_eq!(result.text, "resumed and completed");

    // H2: the resumed run must CONTINUE accounting from the checkpoint's
    // cumulative metrics, not reset to zero. The mock LLM adds 2 tokens
    // (1 prompt + 1 completion) for its single turn, so the result must
    // reflect the seeded 1000 + 2 = 1002, and the seeded tool-call count
    // (3) must carry forward (this turn ran no tools).
    assert_eq!(
        result.usage.total_tokens, 1002,
        "resumed run must add to the checkpoint's cumulative token usage, not reset it"
    );
    assert_eq!(result.usage.prompt_tokens, 801);
    assert_eq!(result.usage.completion_tokens, 201);
    assert_eq!(
        result.tool_calls_count, 3,
        "resumed run must preserve the checkpoint's tool-call count"
    );

    // The resumed run records its own run id in the in-memory store,
    // and that id must NOT match the seeded checkpoint id — the
    // framework allocates a fresh run rather than pretending to
    // continue the old one.
    let runs = session.runs().await;
    assert_eq!(runs.len(), 1, "resume_run creates exactly one new run");
    let resumed_run = &runs[0];
    assert_ne!(
        resumed_run.id, seeded_run_id,
        "resumed run must have a fresh id, got the seeded one"
    );
    assert_eq!(resumed_run.status, crate::run::RunStatus::Completed);

    // The checkpoint stays in the store under the OLD run id —
    // resume does not delete it. (The host decides retention.)
    let still_there: Arc<dyn crate::store::SessionStore> = store.clone();
    let cp = still_there
        .load_loop_checkpoint(seeded_run_id)
        .await
        .expect("load")
        .expect("old checkpoint preserved");
    assert_eq!(cp.run_id, seeded_run_id);
    assert_eq!(cp.turn, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_run_preserves_exhausted_tool_budget_for_finalization() {
    use crate::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};

    let store = Arc::new(crate::store::MemorySessionStore::new());
    let checkpoint_run_id = "budget-exhausted-checkpoint";
    let checkpoint = LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: checkpoint_run_id.to_string(),
        session_id: "resume-budget-target".to_string(),
        capability_binding: None,
        turn: 2,
        messages: vec![Message::user("continue")],
        total_usage: Default::default(),
        tool_calls_count: 2,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1,
    };
    let checkpoint_store: Arc<dyn crate::store::SessionStore> = store.clone();
    checkpoint_store
        .save_loop_checkpoint(checkpoint_run_id, &checkpoint)
        .await
        .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let options = SessionOptions::new()
        .with_session_store(store as Arc<dyn crate::store::SessionStore>)
        .with_session_id("resume-budget-target")
        .with_max_tool_rounds(2);
    let session = agent
        .build_session(
            "/tmp/test-resume-budget-target".into(),
            Arc::new(StaticStreamingClient::new(
                "Best bounded answer from the checkpoint evidence.",
            )),
            &options,
        )
        .unwrap();

    let result = session.resume_run(checkpoint_run_id).await.unwrap();
    assert_eq!(
        result.text,
        "Best bounded answer from the checkpoint evidence."
    );
    assert_eq!(
        result.tool_calls_count, 2,
        "the reserved tool-free finalization turn must not reset or consume the restored tool budget"
    );
    assert!(result.messages.iter().any(|message| {
        message
            .text()
            .contains("Tool-use budget reached. Stop gathering evidence")
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_run_rejects_checkpoint_owned_by_another_session() {
    use crate::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};

    let store = Arc::new(crate::store::MemorySessionStore::new());
    let run_id = "foreign-checkpoint-run";
    let checkpoint = LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        session_id: "foreign-session".to_string(),
        capability_binding: None,
        turn: 1,
        messages: vec![Message::user("private foreign transcript")],
        total_usage: crate::llm::TokenUsage::default(),
        tool_calls_count: 0,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1,
    };
    let checkpoint_store: Arc<dyn crate::store::SessionStore> = store.clone();
    checkpoint_store
        .save_loop_checkpoint(run_id, &checkpoint)
        .await
        .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-resume-run-owner".into(),
            Arc::new(StaticStreamingClient::new("must not execute")),
            &SessionOptions::new()
                .with_session_store(store as Arc<dyn crate::store::SessionStore>)
                .with_session_id("current-session"),
        )
        .unwrap();

    let error = session.resume_run(run_id).await.unwrap_err();
    assert!(error.to_string().contains("ownership mismatch"));
    assert!(
        session.runs().await.is_empty(),
        "rejected resume must not start a run"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_restores_artifacts() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-artifacts-test");
    let session = agent
        .session_async("/tmp/test-ws-artifacts", Some(opts))
        .await
        .unwrap();
    session
        .tool_executor
        .artifact_store()
        .put(crate::tools::ToolArtifact {
            artifact_id: "tool-output:test:a".to_string(),
            artifact_uri: "a3s://tool-output/test/a".to_string(),
            tool_name: "test".to_string(),
            content: "artifact content".to_string(),
            original_bytes: 16,
            shown_bytes: 4,
        });

    session.save().await.unwrap();
    session.close().await;
    drop(session);
    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-artifacts-test", opts2)
        .await
        .unwrap();

    let artifact = resumed
        .get_artifact("a3s://tool-output/test/a")
        .expect("artifact");
    assert_eq!(artifact.content, "artifact content");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_preserves_artifacts_beyond_default_store_limits() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let defaults = crate::tools::ArtifactStoreLimits::default();
    let artifact_count = defaults.max_artifacts + 1;
    let bytes_per_artifact = defaults.max_bytes / artifact_count + 1;
    let persisted_bytes = artifact_count * bytes_per_artifact;
    let artifact_content = "x".repeat(bytes_per_artifact);
    assert!(artifact_count > defaults.max_artifacts);
    assert!(persisted_bytes > defaults.max_bytes);

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-large-artifacts-test")
        .with_artifact_store_limits(crate::tools::ArtifactStoreLimits {
            max_artifacts: artifact_count,
            max_bytes: persisted_bytes,
        });
    let session = agent
        .session_async("/tmp/test-ws-large-artifacts", Some(opts))
        .await
        .unwrap();
    for index in 0..artifact_count {
        session
            .tool_executor
            .artifact_store()
            .put(crate::tools::ToolArtifact {
                artifact_id: format!("tool-output:test:{index}"),
                artifact_uri: format!("a3s://tool-output/test/{index}"),
                tool_name: "test".to_string(),
                content: artifact_content.clone(),
                original_bytes: bytes_per_artifact,
                shown_bytes: 0,
            });
    }

    session.save().await.unwrap();
    drop(session);

    let resumed = agent
        .resume_session_async(
            "resume-large-artifacts-test",
            SessionOptions::new().with_session_store(store),
        )
        .await
        .unwrap();
    let restored = resumed.tool_executor.artifact_store();
    assert_eq!(restored.len(), artifact_count);
    assert_eq!(restored.total_bytes(), persisted_bytes);
    assert!(restored.get("a3s://tool-output/test/0").is_some());
    assert!(restored
        .get(&format!("a3s://tool-output/test/{}", artifact_count - 1))
        .is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_restores_trace_events() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let event = crate::trace::TraceEvent::tool_execution(
        "read",
        true,
        0,
        std::time::Duration::from_millis(3),
        32,
        Some(&serde_json::json!({
            "artifact": {
                "artifact_uri": "a3s://tool-output/read/abc"
            }
        })),
    );

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-trace-test");
    let session = agent
        .session_async("/tmp/test-ws-trace", Some(opts))
        .await
        .unwrap();
    session.trace_sink.replace_events(vec![event.clone()]);
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-trace-test", opts2)
        .await
        .unwrap();

    assert_eq!(resumed.trace_events(), vec![event]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_restores_run_records() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-runs-test");
    let session = agent
        .session_async("/tmp/test-ws-runs", Some(opts))
        .await
        .unwrap();
    let run = session
        .run_store
        .create_run(session.session_id(), "persist run")
        .await;
    session
        .run_store
        .record_event(
            &run.id,
            AgentEvent::Start {
                prompt: "persist run".to_string(),
            },
        )
        .await;
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-runs-test", opts2)
        .await
        .unwrap();

    let runs = resumed.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].prompt, "persist run");
    assert_eq!(resumed.run_events(&run.id).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_restores_verification_reports() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let report = crate::verification::VerificationReport::new(
        "program:test",
        vec![
            crate::verification::VerificationCheck::required("check:test", "test", "Run tests")
                .with_status(crate::verification::VerificationStatus::Passed),
        ],
    );

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-verification-test");
    let session = agent
        .session_async("/tmp/test-ws-verification", Some(opts))
        .await
        .unwrap();
    session.record_verification_reports([report.clone()]);
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-verification-test", opts2)
        .await
        .unwrap();

    assert_eq!(resumed.verification_reports(), vec![report]);
    assert_eq!(
        resumed.verification_summary().status,
        crate::verification::VerificationStatus::Passed
    );
    assert!(resumed
        .verification_summary_text()
        .contains("Verification passed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_verify_commands_builds_report_from_shell_results() {
    #[cfg(windows)]
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(temp_dir.path().display().to_string(), None)
        .await
        .unwrap();
    let commands = vec![
        crate::verification::VerificationCommand::required(
            "check:smoke",
            "smoke",
            "Run smoke command",
            "echo ok",
        ),
        crate::verification::VerificationCommand::required(
            "check:failure",
            "smoke",
            "Run failing command",
            "exit 7",
        ),
    ];

    let report = session.verify_commands("turn", &commands).await.unwrap();

    assert_eq!(report.subject, "turn");
    assert_eq!(
        report.status,
        crate::verification::VerificationStatus::Failed
    );
    assert_eq!(
        report.checks[0].status,
        crate::verification::VerificationStatus::Passed
    );
    assert_eq!(
        report.checks[1].status,
        crate::verification::VerificationStatus::Failed
    );
    assert_eq!(
        report.checks[1].residual_risk.as_deref(),
        Some("verification command exited with code 7: exit 7")
    );
    assert_eq!(session.verification_reports(), vec![report]);
    assert_eq!(
        session.verification_summary().status,
        crate::verification::VerificationStatus::Failed
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_verify_commands_use_host_direct_policy_but_keep_budget_governance() {
    let temp_dir = tempfile::tempdir().unwrap();
    let marker = temp_dir.path().join("verification-must-not-run.txt");
    let guard = Arc::new(DenyingToolBudgetGuard::default());
    let options = SessionOptions::new()
        .with_permission_policy(crate::permissions::PermissionPolicy::new().deny("bash"))
        .with_budget_guard(guard.clone() as Arc<dyn crate::budget::BudgetGuard>);
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(temp_dir.path().display().to_string(), Some(options))
        .await
        .unwrap();
    let commands = [crate::verification::VerificationCommand::required(
        "check:governed",
        "smoke",
        "Exercise governed verification",
        "echo should-not-run > verification-must-not-run.txt",
    )];

    let report = session.verify_commands("turn", &commands).await.unwrap();

    assert_eq!(
        guard.checks.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "host-direct verification must skip model permission but still consult the budget guard"
    );
    assert_eq!(
        report.checks[0].status,
        crate::verification::VerificationStatus::Failed
    );
    assert!(report.checks[0]
        .residual_risk
        .as_deref()
        .is_some_and(|risk| risk.contains("Budget exhausted")));
    assert!(
        !marker.exists(),
        "budget denial must happen before bash runs"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_verification_presets_reflect_workspace() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("package.json"),
        r#"{"scripts":{"test":"vitest","typecheck":"tsc --noEmit"}}"#,
    )
    .unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(temp_dir.path().display().to_string(), None)
        .await
        .unwrap();

    let presets = session.verification_presets();

    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].project_kind, "node");
    assert_eq!(presets[0].commands[0].command, "npm test");
    assert_eq!(presets[0].commands[1].command, "npm run typecheck");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_not_found() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new().with_session_store(store.clone());
    let result = agent.resume_session_async("nonexistent", opts).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_no_store() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new();
    let result = agent.resume_session_async("any-id", opts).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("session_store"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_file_session_store_persistence() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Arc::new(
        crate::store::FileSessionStore::new(dir.path())
            .await
            .unwrap(),
    );
    let agent = Agent::from_config(test_config()).await.unwrap();

    // Save
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("file-persist");
    let session = agent
        .session_async("/tmp/test-ws-file-persist", Some(opts))
        .await
        .unwrap();
    {
        let mut h = session.history.write().unwrap();
        h.push(Message::user("test message"));
    }
    session.save().await.unwrap();

    // Load from a fresh store instance pointing to same dir
    let store2 = Arc::new(
        crate::store::FileSessionStore::new(dir.path())
            .await
            .unwrap(),
    );
    let data = store2.load("file-persist").await.unwrap().unwrap();
    assert_eq!(data.messages.len(), 1);
}
