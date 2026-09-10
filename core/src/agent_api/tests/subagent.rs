use super::*;

#[tokio::test]
async fn subagent_events_populate_session_tracker() {
    use super::runtime_events::RuntimeEventSink;
    use crate::agent::AgentEvent;
    use crate::subagent_task_tracker::SubagentStatus;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-subagent-tracker", None)
        .await
        .unwrap();

    // Drive a synthetic subagent lifecycle through the session's runtime sink.
    let run = session
        .run_store
        .create_run(session.session_id(), "parent prompt")
        .await;
    let sink = RuntimeEventSink::from_session(&session, &run.id);

    let task_id = "task-test-1".to_string();
    let child_session_id = format!("task-run-{}", task_id);

    sink.observe(&AgentEvent::SubagentStart {
        task_id: task_id.clone(),
        session_id: child_session_id.clone(),
        parent_session_id: session.session_id().to_string(),
        agent: "explore".to_string(),
        description: "demo delegation".to_string(),
        started_ms: 1000,
    })
    .await;

    let snap = session
        .subagent_task(&task_id)
        .await
        .expect("running task should be visible");
    assert_eq!(snap.status, SubagentStatus::Running);
    assert_eq!(snap.parent_session_id, session.session_id());
    assert_eq!(snap.child_session_id, child_session_id);
    assert_eq!(snap.agent, "explore");
    assert!(snap.finished_ms.is_none());

    let pending = session.pending_subagent_tasks().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].task_id, task_id);

    sink.observe(&AgentEvent::SubagentEnd {
        task_id: task_id.clone(),
        session_id: child_session_id,
        agent: "explore".to_string(),
        output: "found things".to_string(),
        success: true,
        finished_ms: 1500,
    })
    .await;

    let snap = session.subagent_task(&task_id).await.unwrap();
    assert_eq!(snap.status, SubagentStatus::Completed);
    assert_eq!(snap.success, Some(true));
    assert_eq!(snap.output.as_deref(), Some("found things"));
    assert!(snap.finished_ms.is_some());

    assert!(session.pending_subagent_tasks().await.is_empty());
    assert_eq!(session.subagent_tasks().await.len(), 1);
}

#[tokio::test]
async fn subagent_progress_events_accumulate_in_tracker() {
    use super::runtime_events::RuntimeEventSink;
    use crate::agent::AgentEvent;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-subagent-progress", None)
        .await
        .unwrap();

    let run = session
        .run_store
        .create_run(session.session_id(), "parent prompt")
        .await;
    let sink = RuntimeEventSink::from_session(&session, &run.id);

    let task_id = "task-progress".to_string();
    let child_session_id = format!("task-run-{}", task_id);

    sink.observe(&AgentEvent::SubagentStart {
        task_id: task_id.clone(),
        session_id: child_session_id.clone(),
        parent_session_id: session.session_id().to_string(),
        agent: "explore".to_string(),
        description: "demo".to_string(),
        started_ms: 0,
    })
    .await;

    sink.observe(&AgentEvent::SubagentProgress {
        task_id: task_id.clone(),
        session_id: child_session_id.clone(),
        status: "tool_completed".to_string(),
        metadata: serde_json::json!({ "tool": "bash", "exit_code": 0 }),
    })
    .await;

    sink.observe(&AgentEvent::SubagentProgress {
        task_id: task_id.clone(),
        session_id: child_session_id.clone(),
        status: "turn_completed".to_string(),
        metadata: serde_json::json!({ "turn": 1, "total_tokens": 50 }),
    })
    .await;

    let snap = session.subagent_task(&task_id).await.unwrap();
    assert_eq!(snap.progress.len(), 2);
    assert_eq!(snap.progress[0].status, "tool_completed");
    assert_eq!(snap.progress[1].status, "turn_completed");
    assert_eq!(snap.progress[1].metadata["total_tokens"], 50);
}

#[tokio::test]
async fn subagent_tasks_scope_to_parent_session() {
    use super::runtime_events::RuntimeEventSink;
    use crate::agent::AgentEvent;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session_a = agent
        .session_async("/tmp/test-ws-subagent-a", None)
        .await
        .unwrap();
    let session_b = agent
        .session_async("/tmp/test-ws-subagent-b", None)
        .await
        .unwrap();

    let run = session_a
        .run_store
        .create_run(session_a.session_id(), "p")
        .await;
    let sink = RuntimeEventSink::from_session(&session_a, &run.id);

    sink.observe(&AgentEvent::SubagentStart {
        task_id: "task-from-a".to_string(),
        session_id: "task-run-task-from-a".to_string(),
        parent_session_id: session_a.session_id().to_string(),
        agent: "explore".to_string(),
        description: "isolated".to_string(),
        started_ms: 0,
    })
    .await;

    // session A sees the task; session B has its own (empty) tracker.
    assert_eq!(session_a.subagent_tasks().await.len(), 1);
    assert!(session_b.subagent_tasks().await.is_empty());
    assert!(session_b.subagent_task("task-from-a").await.is_none());
}

#[tokio::test]
async fn cancel_subagent_task_marks_snapshot_cancelled() {
    use super::runtime_events::RuntimeEventSink;
    use crate::agent::AgentEvent;
    use crate::subagent_task_tracker::SubagentStatus;
    use tokio_util::sync::CancellationToken;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-subagent-cancel", None)
        .await
        .unwrap();
    let run = session
        .run_store
        .create_run(session.session_id(), "parent")
        .await;
    let sink = RuntimeEventSink::from_session(&session, &run.id);

    let task_id = "task-to-cancel".to_string();
    sink.observe(&AgentEvent::SubagentStart {
        task_id: task_id.clone(),
        session_id: format!("task-run-{}", task_id),
        parent_session_id: session.session_id().to_string(),
        agent: "explore".to_string(),
        description: "long task".to_string(),
        started_ms: 0,
    })
    .await;

    // Simulate what TaskExecutor would do: register a cancellation token
    // for this in-flight task so the public API has something to fire.
    let token = CancellationToken::new();
    session
        .subagent_tasks
        .register_canceller(&task_id, token.clone())
        .await;

    assert!(session.cancel_subagent_task(&task_id).await);
    assert!(token.is_cancelled());

    let snap = session.subagent_task(&task_id).await.unwrap();
    assert_eq!(snap.status, SubagentStatus::Cancelled);

    // A late SubagentEnd from the cancelled child must not downgrade.
    sink.observe(&AgentEvent::SubagentEnd {
        task_id: task_id.clone(),
        session_id: format!("task-run-{}", task_id),
        agent: "explore".to_string(),
        output: "Task cancelled by caller".to_string(),
        success: false,
        finished_ms: 0,
    })
    .await;
    let snap = session.subagent_task(&task_id).await.unwrap();
    assert_eq!(snap.status, SubagentStatus::Cancelled);

    // Cancelling again or against an unknown id is a no-op.
    assert!(!session.cancel_subagent_task(&task_id).await);
    assert!(!session.cancel_subagent_task("task-unknown").await);
}

/// Regression: `agent_executor()` must install the same `ChildRunContext` the
/// model-driven `task` path uses, so orchestrated/scripted steps inherit the
/// session's governance instead of running under weaker ambient authority.
/// Before the fix the executor was built without `.with_parent_context(..)`.
#[tokio::test]
async fn test_agent_executor_inherits_parent_run_context() {
    use crate::security::DefaultSecurityProvider;
    use crate::skills::SkillRegistry;

    let agent = Agent::from_config(test_config()).await.unwrap();

    let security: Arc<dyn crate::security::SecurityProvider> =
        Arc::new(DefaultSecurityProvider::new());
    let skills = Arc::new(SkillRegistry::new());
    let permission_policy = crate::permissions::PermissionPolicy::new().allow("web_fetch(*)");
    let opts = SessionOptions::new()
        .with_security_provider(Arc::clone(&security))
        .with_skill_registry(Arc::clone(&skills))
        .with_llm_api_timeout(45_000)
        .with_duplicate_tool_call_threshold(9)
        .with_confirmation_policy(crate::hitl::ConfirmationPolicy::enabled())
        .with_permission_policy(permission_policy);

    let session = agent
        .session_async("/tmp/test-workspace", Some(opts))
        .await
        .unwrap();
    let runtime_budget: Arc<dyn crate::budget::BudgetGuard> =
        Arc::new(DenyingBudgetGuard::default());
    session
        .set_budget_guard(Some(Arc::clone(&runtime_budget)))
        .unwrap();
    session
        .add_skill(Arc::new(crate::skills::Skill {
            name: "live-use-capability".to_string(),
            description: "Live Use capability".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: crate::skills::SkillKind::Instruction,
            content: "Use the attached capability.".to_string(),
            tags: Vec::new(),
            version: None,
        }))
        .unwrap();
    let ctx = session.parent_run_context();

    assert!(
        ctx.security_provider.is_some(),
        "security provider must propagate to delegated/orchestrated child runs"
    );
    assert!(
        ctx.skill_registry.is_some(),
        "skill registry (skill restrictions) must propagate to child runs"
    );
    let inherited_skills = ctx
        .skill_registry
        .as_ref()
        .expect("inherited effective skill registry");
    assert!(Arc::ptr_eq(
        inherited_skills,
        &session.close_handle.skill_registry
    ));
    assert!(inherited_skills.get("live-use-capability").is_some());
    assert!(
        ctx.permission_checker.is_some(),
        "permission checker must propagate to child runs"
    );
    assert!(
        ctx.permission_policy.is_some(),
        "serializable permission policy must propagate to child runs"
    );
    assert!(
        ctx.confirmation_manager.is_some(),
        "confirmation manager built from policy must propagate to child runs"
    );
    assert!(
        ctx.workspace_services.is_some(),
        "workspace services must propagate so child tools share the workspace"
    );
    assert_eq!(ctx.llm_api_timeout_ms, Some(45_000));
    assert_eq!(ctx.duplicate_tool_call_threshold, Some(9));
    let expected_hook: Arc<dyn crate::hooks::HookExecutor> = session.hook_engine.clone();
    assert!(Arc::ptr_eq(
        ctx.hook_engine.as_ref().expect("inherited hook executor"),
        &expected_hook
    ));
    assert!(Arc::ptr_eq(
        ctx.budget_guard.as_ref().expect("inherited runtime budget"),
        &runtime_budget
    ));
}

/// A session-bound executor may outlive the `AgentSession` value that created
/// it. Closing the session must therefore remain an admission boundary for
/// orchestrated work, not just for `send()`/`stream()` calls.
#[tokio::test]
async fn test_agent_executor_created_before_close_rejects_new_steps() {
    use crate::orchestration::AgentStepSpec;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-agent-executor-close-boundary".into(),
            Arc::new(StaticStreamingClient::new("must not run after close")),
            &SessionOptions::new(),
        )
        .unwrap();
    let executor = session.agent_executor();
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);

    session.close().await;

    let outcome = executor
        .execute_step(
            AgentStepSpec::new(
                "after-close",
                "general",
                "close admission regression",
                "This child must never start.",
            ),
            Some(event_tx),
        )
        .await;

    assert!(!outcome.success);
    assert!(
        outcome.output.to_ascii_lowercase().contains("cancel"),
        "closed-session failure should explain the cancellation: {}",
        outcome.output
    );
    assert!(
        matches!(
            event_rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                | Err(tokio::sync::broadcast::error::TryRecvError::Closed)
        ),
        "a rejected child must not emit SubagentStart"
    );
}

#[tokio::test]
async fn runtime_budget_guard_refreshes_the_registered_task_tool() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_worker_agent(crate::subagent::WorkerAgentSpec::planner(
        "budgeted-child",
        "Exercise delegated budget inheritance",
    ));
    let session = agent
        .build_session(
            "/tmp/test-runtime-budget-delegation".into(),
            Arc::new(StaticStreamingClient::new("must not reach the provider")),
            &opts,
        )
        .unwrap();
    let runtime_budget = Arc::new(DenyingBudgetGuard::default());
    session
        .set_budget_guard(Some(
            Arc::clone(&runtime_budget) as Arc<dyn crate::budget::BudgetGuard>
        ))
        .unwrap();

    let result = session
        .tool(
            "task",
            serde_json::json!({
                "agent": "budgeted-child",
                "description": "budget check",
                "prompt": "Return a short answer."
            }),
        )
        .await
        .unwrap();

    assert_ne!(result.exit_code, 0, "delegated run must be denied");
    assert_eq!(
        runtime_budget
            .checks
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the runtime-installed guard must govern the child provider attempt"
    );
    assert_eq!(
        runtime_budget
            .llm_records
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a denied child call must not record provider usage"
    );
}

#[tokio::test]
async fn runtime_budget_guard_refreshes_the_registered_skill_tool() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};

    let skills = Arc::new(SkillRegistry::new());
    skills.register_unchecked(Arc::new(Skill {
        name: "budgeted-skill".to_string(),
        description: "Exercise runtime budget inheritance".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Return a short answer.".to_string(),
        tags: Vec::new(),
        version: None,
    }));

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-runtime-budget-skill".into(),
            Arc::new(StaticStreamingClient::new("must not reach the provider")),
            &SessionOptions::new().with_skill_registry(skills),
        )
        .unwrap();
    let runtime_budget = Arc::new(DenyingBudgetGuard::default());
    session
        .set_budget_guard(Some(
            Arc::clone(&runtime_budget) as Arc<dyn crate::budget::BudgetGuard>
        ))
        .unwrap();

    let result = session
        .tool("Skill", serde_json::json!({"skill_name": "budgeted-skill"}))
        .await
        .unwrap();

    assert_ne!(result.exit_code, 0, "skill child run must be denied");
    assert_eq!(
        runtime_budget
            .checks
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the runtime-installed guard must govern the skill provider attempt"
    );
    assert_eq!(
        runtime_budget
            .llm_records
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn delegated_child_run_publishes_events_to_the_parent_hook_executor() {
    let hook = Arc::new(RecordingRuntimeHook::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_hook_executor(Arc::clone(&hook) as Arc<dyn crate::hooks::HookExecutor>)
        .with_worker_agent(crate::subagent::WorkerAgentSpec::planner(
            "hooked-child",
            "Exercise delegated hook inheritance",
        ));
    let session = agent
        .build_session(
            "/tmp/test-delegated-hook-inheritance".into(),
            Arc::new(StaticStreamingClient::new("child answer")),
            &opts,
        )
        .unwrap();

    let result = session
        .tool(
            "task",
            serde_json::json!({
                "agent": "hooked-child",
                "description": "hook check",
                "prompt": "Return a short answer."
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        result.exit_code, 0,
        "delegated run failed: {}",
        result.output
    );

    let events = hook.hook_events.lock().unwrap();
    assert!(events.iter().any(|event| {
        event.session_id().starts_with("task-run-task-")
            && matches!(event, crate::hooks::HookEvent::GenerateStart(_))
    }));
    assert!(events.iter().any(|event| {
        event.session_id().starts_with("task-run-task-")
            && matches!(event, crate::hooks::HookEvent::GenerateEnd(_))
    }));
}

#[tokio::test]
async fn test_registered_task_fanout_inherits_final_confirmation_manager() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello from parent context").unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "read-1",
            "read",
            serde_json::json!({"file_path": "note.txt"}),
        ),
        scripted_text_response("child read complete"),
        scripted_text_response("control child complete"),
    ]));
    let worker = crate::subagent::WorkerAgentSpec::custom(
        "needs-parent-hitl",
        "Uses parent HITL for Ask decisions",
    )
    .with_confirmation(crate::subagent::ConfirmationInheritance::InheritParent)
    .with_max_steps(3);
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_worker_agent(worker)
        .with_max_parallel_tasks(1)
        .with_confirmation_policy(crate::hitl::ConfirmationPolicy::enabled());
    let session = agent
        .session_async(dir.path().to_string_lossy().to_string(), Some(opts))
        .await
        .unwrap();

    let (_rx, join) = session.tool_with_events(
        "task",
        serde_json::json!({
            "tasks": [
                {
                    "agent": "needs-parent-hitl",
                    "description": "Read a note",
                    "prompt": "Read note.txt",
                    "max_steps": 3
                },
                {
                    "agent": "needs-parent-hitl",
                    "description": "Independent control",
                    "prompt": "Return a short control result.",
                    "max_steps": 1
                }
            ]
        }),
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut approved = false;
    while !join.is_finished() && std::time::Instant::now() < deadline {
        let pending = session.pending_confirmations().await;
        if let Some(request) = pending.first() {
            assert_eq!(request.tool_name, "read");
            assert!(session
                .confirm_tool_use(&request.tool_id, true, Some("test approval".to_string()))
                .await
                .unwrap());
            approved = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        approved,
        "child run should surface a parent HITL confirmation"
    );
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        result.exit_code, 0,
        "task fan-out output: {}",
        result.output
    );
    assert!(!result.output.contains("requires confirmation but no HITL"));
    assert!(!result.output.contains("Permission denied"));
}

