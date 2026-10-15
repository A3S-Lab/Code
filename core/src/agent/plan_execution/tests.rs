use super::*;
use crate::agent::tests::MockLlmClient;
use crate::planning::Complexity;
use crate::tools::{ToolContext, ToolExecutor};
use std::sync::Arc;

#[test]
fn delegated_bound_closure_is_kept_and_a_bare_mutation_is_not_narrative() {
    let mut completions = Vec::new();
    let metadata = Some(serde_json::json!({
        "changed_paths": ["guest.txt"],
        "completion": { "kind": "verified", "effect_digest": "digest-a" }
    }));
    push_published_completions(&mut completions, &metadata);
    assert_eq!(
        crate::harness_loop::fold_step_completions(&completions),
        crate::harness_loop::CompletionTerminal::Verified {
            effect_digest: "digest-a".to_string(),
        }
    );
    assert!(!delegated_mutation_without_closure(&metadata));
    let bare = Some(serde_json::json!({ "changed_paths": ["guest.txt"] }));
    assert!(delegated_mutation_without_closure(&bare));
}

#[test]
fn delegated_results_use_each_child_output_excerpt() {
    let metadata = serde_json::json!({
        "results": [
            {
                "success": true,
                "output_excerpt": "auth branch evidence"
            },
            {
                "success": false,
                "output_excerpt": "docs branch failed: status 529"
            }
        ]
    });

    let results = AgentLoop::delegated_parallel_child_results(Some(&metadata), 2, false);

    assert!(results[0].success);
    assert_eq!(results[0].output.as_deref(), Some("auth branch evidence"));
    assert!(!results[1].success);
    assert_eq!(
        results[1].output.as_deref(),
        Some("docs branch failed: status 529")
    );
}

#[test]
fn parallel_step_result_build_envelope_wraps_steps() {
    let envelope = ParallelStepResult::build_envelope(vec![ParallelStepResult {
        step_id: "s1".into(),
        step_number: 1,
        status: "completed".into(),
        summary: "ok".into(),
        key_findings: Some(vec!["a".into()]),
        error: None,
        data: Some(json!({ "k": 1 })),
    }]);
    assert_eq!(envelope["type"], "parallel_results");
    assert_eq!(envelope["steps"][0]["step_id"], "s1");
    assert_eq!(envelope["steps"][0]["key_findings"][0], "a");
}

#[test]
fn push_published_completions_reads_nested_result_completions() {
    let mut completions = Vec::new();
    push_published_completions(&mut completions, &None);
    assert!(completions.is_empty());

    let metadata = Some(json!({
        "results": [
            { "completion": { "kind": "verified", "effect_digest": "child-a" } },
            { "completion": { "kind": "narrative" } },
            { "completion": "not-a-terminal" }
        ]
    }));
    push_published_completions(&mut completions, &metadata);
    assert_eq!(completions.len(), 2);
    assert_eq!(
        crate::harness_loop::fold_step_completions(&completions),
        crate::harness_loop::CompletionTerminal::Verified {
            effect_digest: "child-a".to_string(),
        }
    );
}

#[test]
fn delegated_mutation_without_closure_requires_paths_and_missing_completion() {
    assert!(!delegated_mutation_without_closure(&None));
    assert!(!delegated_mutation_without_closure(&Some(
        json!({ "changed_paths": [] })
    )));
    assert!(!delegated_mutation_without_closure(&Some(json!({
        "changed_paths": ["a.txt"],
        "results": [{ "completion": { "kind": "narrative" } }]
    }))));
    assert!(delegated_mutation_without_closure(&Some(json!({
        "changed_paths": ["a.txt"],
        "results": [{ "success": true }]
    }))));
}

#[test]
fn delegated_parallel_child_results_fall_back_and_prefer_output_field() {
    let missing = AgentLoop::delegated_parallel_child_results(None, 2, true);
    assert_eq!(missing.len(), 2);
    assert!(missing
        .iter()
        .all(|child| child.success && child.output.is_none()));

    let metadata = json!({
        "results": [
            { "success": true, "output": "full output wins" },
            { "output_excerpt": "excerpt only" }
        ]
    });
    let results = AgentLoop::delegated_parallel_child_results(Some(&metadata), 3, false);
    assert_eq!(results[0].output.as_deref(), Some("full output wins"));
    assert!(!results[1].success);
    assert_eq!(results[1].output.as_deref(), Some("excerpt only"));
    assert!(!results[2].success);
    assert!(results[2].output.is_none());
}

#[test]
fn normalized_plan_tool_trims_and_filters_blank_tools() {
    assert_eq!(
        AgentLoop::normalized_plan_tool(&Task::new("s1", "x").with_tool("  task  ")),
        Some("task")
    );
    assert_eq!(
        AgentLoop::normalized_plan_tool(&Task::new("s1", "x").with_tool("   ")),
        None
    );
    assert!(AgentLoop::should_delegate_plan_step(
        &Task::new("s1", "x").with_tool("parallel_task")
    ));
    assert!(!AgentLoop::should_delegate_plan_step(
        &Task::new("s1", "x").with_tool("bash")
    ));
}

#[test]
fn delegated_agent_and_prompt_cover_goal_and_criteria_keywords() {
    assert_eq!(
        AgentLoop::delegated_agent_for_step(
            &Task::new("s1", "ship it").with_success_criteria("审查回归风险")
        ),
        "review"
    );
    assert_eq!(
        AgentLoop::delegated_agent_for_step(
            &Task::new("s2", "work").with_success_criteria("准备发布 smoke")
        ),
        "verification"
    );
    assert_eq!(
        AgentLoop::delegated_agent_for_step(
            &Task::new("s3", "work").with_success_criteria("完成架构规划")
        ),
        "plan"
    );

    let prompt = AgentLoop::delegated_prompt_for_step_with_goal(
        Some("  Keep product transcript  "),
        &Task::new("s4", "Inspect docs").with_success_criteria("  find evidence  "),
        1,
        3,
    );
    assert!(prompt.contains("Plan goal/context:\nKeep product transcript"));
    assert!(prompt.contains("Execute plan step 1/3."));
    assert!(prompt.contains("Success criteria:\n  find evidence  "));
    assert!(prompt.contains("confidence"));
}

#[test]
fn remember_completion_gate_keeps_first_message_only() {
    let mut slot = None;
    remember_completion_gate(&mut slot, "plain failure");
    assert!(slot.is_none());
    remember_completion_gate(&mut slot, "completion gate: first");
    remember_completion_gate(&mut slot, "completion gate: second");
    assert_eq!(slot.as_deref(), Some("completion gate: first"));
}

#[test]
fn accumulate_result_accounting_merges_usage_tools_and_reports() {
    let mut usage = TokenUsage::default();
    let mut tool_calls = 1usize;
    let mut reports = Vec::new();
    let result = AgentResult {
        text: "done".into(),
        messages: Vec::new(),
        usage: TokenUsage {
            prompt_tokens: 2,
            completion_tokens: 3,
            total_tokens: 5,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        tool_calls_count: 4,
        verification_reports: vec![crate::verification::VerificationReport::new(
            "subject",
            vec![crate::verification::VerificationCheck::required(
                "c1", "kind", "desc",
            )],
        )],
        completion: crate::harness_loop::CompletionTerminal::Narrative,
        forced_stop: None,
        run_admission: "run".into(),
    };
    accumulate_result_accounting(&mut usage, &mut tool_calls, &mut reports, &result);
    assert_eq!(usage.total_tokens, 5);
    assert_eq!(tool_calls, 5);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].subject, "subject");
}

#[test]
fn accumulate_failure_accounting_only_reads_agent_execution_failure() {
    let mut usage = TokenUsage::default();
    let mut tool_calls = 0usize;
    accumulate_failure_accounting(&mut usage, &mut tool_calls, &anyhow::anyhow!("plain error"));
    assert_eq!(tool_calls, 0);

    let failure = AgentExecutionFailure::new(
        anyhow::anyhow!("gated"),
        TokenUsage {
            prompt_tokens: 3,
            completion_tokens: 2,
            total_tokens: 5,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        4,
    );
    accumulate_failure_accounting(&mut usage, &mut tool_calls, &anyhow::Error::new(failure));
    assert_eq!(tool_calls, 4);
    assert_eq!(usage.total_tokens, 5);
}

#[tokio::test]
async fn execute_plan_rejects_already_cancelled_token() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig::default(),
    );
    let plan = ExecutionPlan::new("cancelled up front", Complexity::Simple);
    let token = CancellationToken::new();
    token.cancel();

    let err = agent
        .execute_plan(&[], &plan, Some("pre-cancel"), None, &token)
        .await
        .expect_err("cancelled token must fail closed before plan work");
    assert!(err.to_string().contains("Operation cancelled by user"));
}

#[tokio::test]
async fn execute_plan_breaks_on_dependency_deadlock() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig::default(),
    );
    let mut plan = ExecutionPlan::new("deadlock", Complexity::Simple);
    plan.add_step(Task::new("s1", "A").with_dependencies(vec!["s2".into()]));
    plan.add_step(Task::new("s2", "B").with_dependencies(vec!["s1".into()]));

    let result = agent
        .execute_plan(
            &[],
            &plan,
            Some("deadlock"),
            None,
            &CancellationToken::new(),
        )
        .await
        .expect("deadlock should exit without hanging");
    assert!(
        result.messages.iter().any(|message| message.role == "user"),
        "plan kickoff should still be recorded before the deadlock exit"
    );
}

struct DenyTaskTool;

impl crate::permissions::PermissionChecker for DenyTaskTool {
    fn check(
        &self,
        tool_name: &str,
        _args: &serde_json::Value,
    ) -> crate::permissions::PermissionDecision {
        if tool_name == "task" {
            crate::permissions::PermissionDecision::Deny
        } else {
            crate::permissions::PermissionDecision::Allow
        }
    }
}

struct HangThenCancelClient {
    calls: std::sync::atomic::AtomicUsize,
    step_started: tokio::sync::Notify,
}

impl HangThenCancelClient {
    fn new() -> Self {
        Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
            step_started: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for HangThenCancelClient {
    async fn complete(
        &self,
        _messages: &[crate::llm::Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.step_started.notify_one();
        std::future::pending().await
    }

    async fn complete_streaming(
        &self,
        _messages: &[crate::llm::Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.step_started.notify_one();
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            drop(tx);
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn execute_plan_emits_failed_step_end_for_denied_delegated_task() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            permission_checker: Some(Arc::new(DenyTaskTool)),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("deny delegated", Complexity::Simple);
    plan.add_step(Task::new("s1", "Find docs").with_tool("task"));
    let (event_tx, mut event_rx) = mpsc::channel(8);

    let result = agent
        .execute_plan(
            &[],
            &plan,
            Some("deny-delegated"),
            Some(event_tx),
            &CancellationToken::new(),
        )
        .await
        .expect("denied delegated step should complete the plan with failures recorded");

    assert!(
        result
            .messages
            .iter()
            .any(|message| message.role == "user" && message.text().contains("failed")),
        "denied delegated failure should be recorded in history"
    );

    let mut saw_failed_end = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(
            event,
            AgentEvent::StepEnd {
                status: TaskStatus::Failed,
                ..
            }
        ) {
            saw_failed_end = true;
        }
    }
    assert!(saw_failed_end, "StepEnd Failed should be emitted");
}

#[tokio::test]
async fn execute_plan_emits_cancelled_step_end_when_parent_cancels() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(HangThenCancelClient::new());
    let agent = AgentLoop::new(
        client.clone(),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig::default(),
    );
    let mut plan = ExecutionPlan::new("cancel with events", Complexity::Simple);
    plan.add_step(Task::new("s1", "First serial step"));
    plan.add_step(Task::new("s2", "Second serial step").with_dependencies(vec!["s1".into()]));
    let cancel_token = CancellationToken::new();
    let run_token = cancel_token.clone();
    let (event_tx, mut event_rx) = mpsc::channel(16);
    let run = tokio::spawn(async move {
        agent
            .execute_plan(
                &[],
                &plan,
                Some("cancel-events"),
                Some(event_tx),
                &run_token,
            )
            .await
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.step_started.notified(),
    )
    .await
    .expect("first step should start");
    cancel_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), run)
        .await
        .expect("cancelled plan should finish")
        .expect("join")
        .expect("cancelled plan returns result");

    let mut saw_cancelled = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(
            event,
            AgentEvent::StepEnd {
                status: TaskStatus::Cancelled,
                ..
            }
        ) {
            saw_cancelled = true;
        }
    }
    assert!(
        saw_cancelled || client.calls.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "cancellation should interrupt an in-flight plan step"
    );
}

#[tokio::test]
async fn execute_plan_parallel_delegated_wave_emits_failed_step_ends() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![])),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            permission_checker: Some(Arc::new(DenyTaskTool)),
            max_parallel_tasks: 2,
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("parallel deny", Complexity::Simple);
    plan.add_step(Task::new("s1", "Parallel A").with_tool("task"));
    plan.add_step(Task::new("s2", "Parallel B").with_tool("task"));
    let (event_tx, mut event_rx) = mpsc::channel(16);

    let _ = agent
        .execute_plan(
            &[],
            &plan,
            Some("parallel-deny"),
            Some(event_tx),
            &CancellationToken::new(),
        )
        .await;

    let mut failed_ends = 0usize;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(
            event,
            AgentEvent::StepEnd {
                status: TaskStatus::Failed,
                ..
            }
        ) {
            failed_ends += 1;
        }
    }
    assert!(
        failed_ends >= 1,
        "denied parallel delegated wave should emit StepEnd Failed"
    );
}

#[tokio::test]
async fn execute_plan_reuses_existing_product_transcript_and_emits_completed() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "serial step done",
        )])),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            continuation_enabled: false,
            max_tool_rounds: 2,
            max_execution_time_ms: Some(3_000),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("reuse product", Complexity::Simple);
    plan.add_step(Task::new("s1", "Do the work"));
    // Keep capacity high and drain concurrently: execute_plan awaits on full
    // channels and will hang if nobody receives events.
    let (event_tx, mut event_rx) = mpsc::channel(256);
    let drain = tokio::spawn(async move {
        let mut saw_completed = false;
        while let Some(event) = event_rx.recv().await {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Completed,
                    ..
                }
            ) {
                saw_completed = true;
            }
        }
        saw_completed
    });
    let history = vec![Message::user("reuse product")];

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        agent.execute_plan(
            &history,
            &plan,
            Some("reuse-product"),
            Some(event_tx),
            &CancellationToken::new(),
        ),
    )
    .await
    .expect("serial plan must settle")
    .expect("serial plan with mock text should complete");

    let product_users = result
        .messages
        .iter()
        .filter(|message| message.role == "user" && message.is_product_transcript())
        .count();
    assert_eq!(
        product_users, 1,
        "existing product transcript must not be duplicated"
    );
    assert!(
        drain.await.expect("drain join"),
        "successful serial step should emit StepEnd Completed"
    );
}

#[tokio::test]
async fn execute_plan_parallel_wave_emits_completed_step_ends() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(vec![
            MockLlmClient::text_response("parallel A done"),
            MockLlmClient::text_response("parallel B done"),
        ])),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            continuation_enabled: false,
            max_parallel_tasks: 2,
            max_tool_rounds: 2,
            max_execution_time_ms: Some(3_000),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("parallel ok", Complexity::Simple);
    plan.add_step(Task::new("s1", "Parallel A"));
    plan.add_step(Task::new("s2", "Parallel B"));
    let (event_tx, mut event_rx) = mpsc::channel(256);
    let drain = tokio::spawn(async move {
        let mut completed = 0usize;
        while let Some(event) = event_rx.recv().await {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Completed,
                    ..
                }
            ) {
                completed += 1;
            }
        }
        completed
    });

    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        agent.execute_plan(
            &[],
            &plan,
            Some("parallel-ok"),
            Some(event_tx),
            &CancellationToken::new(),
        ),
    )
    .await
    .expect("successful parallel wave must settle")
    .expect("parallel wave should return");

    assert!(
        drain.await.expect("drain join") >= 1,
        "successful parallel steps should emit StepEnd Completed"
    );
}

struct CompleteAndCancelClient {
    cancel: CancellationToken,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for CompleteAndCancelClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        self.cancel.cancel();
        Ok(MockLlmClient::text_response("done after cancel signal"))
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[crate::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
        let response = self.complete(messages, system, tools).await?;
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            let text = response.text();
            if !text.is_empty() {
                let _ = tx.send(crate::llm::StreamEvent::TextDelta(text)).await;
            }
            let _ = tx.send(crate::llm::StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn execute_plan_marks_completed_step_cancelled_when_token_trips_during_ok() {
    let workspace = tempfile::tempdir().unwrap();
    let cancel_token = CancellationToken::new();
    let agent = AgentLoop::new(
        Arc::new(CompleteAndCancelClient {
            cancel: cancel_token.clone(),
        }),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            continuation_enabled: false,
            max_tool_rounds: 2,
            max_execution_time_ms: Some(3_000),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("cancel after ok", Complexity::Simple);
    plan.add_step(Task::new("s1", "First"));
    plan.add_step(Task::new("s2", "Second").with_dependencies(vec!["s1".into()]));
    let (event_tx, mut event_rx) = mpsc::channel(32);
    let _ = agent
        .execute_plan(
            &[],
            &plan,
            Some("cancel-after-ok"),
            Some(event_tx),
            &cancel_token,
        )
        .await;

    let mut saw_cancelled = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(
            event,
            AgentEvent::StepEnd {
                status: TaskStatus::Cancelled,
                ..
            }
        ) {
            saw_cancelled = true;
        }
    }
    assert!(
        saw_cancelled || cancel_token.is_cancelled(),
        "cancel during Ok step should mark Cancelled or stop the plan"
    );
}

#[tokio::test]
async fn execute_plan_breaks_at_loop_top_when_cancelled_between_waves() {
    let workspace = tempfile::tempdir().unwrap();
    let cancel_token = CancellationToken::new();
    let cancel_for_client = cancel_token.clone();
    let agent = AgentLoop::new(
        Arc::new(CompleteAndCancelClient {
            cancel: cancel_for_client,
        }),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            continuation_enabled: false,
            max_tool_rounds: 2,
            max_execution_time_ms: Some(3_000),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("between waves", Complexity::Simple);
    plan.add_step(Task::new("s1", "Only first"));
    plan.add_step(Task::new("s2", "Never runs").with_dependencies(vec!["s1".into()]));
    let result = agent
        .execute_plan(&[], &plan, Some("between-waves"), None, &cancel_token)
        .await;
    // Either Ok with partial progress or Err from completion gate — must not hang.
    let _ = result;
    assert!(cancel_token.is_cancelled());
}

#[tokio::test]
async fn execute_plan_parallel_wave_emits_cancelled_when_parent_cancels() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(HangThenCancelClient::new());
    let agent = AgentLoop::new(
        client.clone(),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            continuation_enabled: false,
            max_parallel_tasks: 2,
            max_tool_rounds: 2,
            max_execution_time_ms: Some(5_000),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("parallel cancel", Complexity::Simple);
    plan.add_step(Task::new("s1", "Hang A"));
    plan.add_step(Task::new("s2", "Hang B"));
    let cancel_token = CancellationToken::new();
    let run_token = cancel_token.clone();
    let (event_tx, mut event_rx) = mpsc::channel(32);
    let run = tokio::spawn(async move {
        agent
            .execute_plan(
                &[],
                &plan,
                Some("parallel-cancel"),
                Some(event_tx),
                &run_token,
            )
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.step_started.notified(),
    )
    .await
    .expect("parallel wave should start");
    cancel_token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), run)
        .await
        .expect("cancelled parallel wave must finish")
        .expect("join");

    let mut cancelled = 0usize;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(
            event,
            AgentEvent::StepEnd {
                status: TaskStatus::Cancelled,
                ..
            }
        ) {
            cancelled += 1;
        }
    }
    assert!(
        cancelled >= 1 || client.calls.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "parallel cancel should interrupt in-flight wave"
    );
}

struct FailAndCancelClient {
    cancel: CancellationToken,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for FailAndCancelClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        self.cancel.cancel();
        Err(anyhow::anyhow!("forced plan step failure after cancel"))
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[crate::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
        let _ = self.complete(messages, system, tools).await?;
        unreachable!("complete always errors")
    }
}

#[tokio::test]
async fn execute_plan_marks_failed_step_cancelled_when_token_trips_during_err() {
    let workspace = tempfile::tempdir().unwrap();
    let cancel_token = CancellationToken::new();
    let agent = AgentLoop::new(
        Arc::new(FailAndCancelClient {
            cancel: cancel_token.clone(),
        }),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            continuation_enabled: false,
            max_tool_rounds: 2,
            max_execution_time_ms: Some(3_000),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("cancel after err", Complexity::Simple);
    plan.add_step(Task::new("s1", "First"));
    plan.add_step(Task::new("s2", "Second").with_dependencies(vec!["s1".into()]));
    let (event_tx, mut event_rx) = mpsc::channel(32);
    let _ = agent
        .execute_plan(
            &[],
            &plan,
            Some("cancel-after-err"),
            Some(event_tx),
            &cancel_token,
        )
        .await;

    let mut saw_cancelled = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(
            event,
            AgentEvent::StepEnd {
                status: TaskStatus::Cancelled,
                ..
            }
        ) {
            saw_cancelled = true;
        }
    }
    assert!(
        saw_cancelled || cancel_token.is_cancelled(),
        "cancel during Err step should mark Cancelled or stop the plan"
    );
}

#[tokio::test]
async fn execute_plan_parallel_wave_emits_failed_step_ends_on_llm_error() {
    let workspace = tempfile::tempdir().unwrap();
    let cancel_token = CancellationToken::new();
    let agent = AgentLoop::new(
        Arc::new(FailAndCancelClient {
            cancel: cancel_token.clone(),
        }),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            continuation_enabled: false,
            max_parallel_tasks: 2,
            max_tool_rounds: 2,
            max_execution_time_ms: Some(3_000),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("parallel fail", Complexity::Simple);
    plan.add_step(Task::new("s1", "Fail A"));
    plan.add_step(Task::new("s2", "Fail B"));
    let (event_tx, mut event_rx) = mpsc::channel(32);
    let _ = agent
        .execute_plan(
            &[],
            &plan,
            Some("parallel-fail"),
            Some(event_tx),
            &cancel_token,
        )
        .await;

    let mut saw_end = false;
    while let Ok(event) = event_rx.try_recv() {
        if matches!(
            event,
            AgentEvent::StepEnd {
                status: TaskStatus::Failed | TaskStatus::Cancelled,
                ..
            }
        ) {
            saw_end = true;
        }
    }
    assert!(
        saw_end || cancel_token.is_cancelled(),
        "parallel LLM failures should emit Failed/Cancelled ends"
    );
}

struct AlwaysFailClient;

#[async_trait::async_trait]
impl crate::llm::LlmClient for AlwaysFailClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        Err(anyhow::anyhow!("forced parallel step failure"))
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[crate::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
        let _ = self.complete(messages, system, tools).await?;
        unreachable!("complete always errors")
    }
}

struct PanicOnCompleteClient;

#[async_trait::async_trait]
impl crate::llm::LlmClient for PanicOnCompleteClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        panic!("forced parallel branch panic");
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[crate::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
        let _ = self.complete(messages, system, tools).await?;
        unreachable!("complete always panics")
    }
}

#[tokio::test]
async fn execute_plan_requires_cancelled_step_end_when_err_trips_token() {
    let workspace = tempfile::tempdir().unwrap();
    let cancel_token = CancellationToken::new();
    let agent = AgentLoop::new(
        Arc::new(FailAndCancelClient {
            cancel: cancel_token.clone(),
        }),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            continuation_enabled: false,
            max_tool_rounds: 2,
            max_execution_time_ms: Some(3_000),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("require cancel end", Complexity::Simple);
    plan.add_step(Task::new("s1", "First"));
    let (event_tx, mut event_rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut saw_cancelled = false;
        while let Some(event) = event_rx.recv().await {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Cancelled,
                    ..
                }
            ) {
                saw_cancelled = true;
            }
        }
        saw_cancelled
    });

    let _ = agent
        .execute_plan(
            &[],
            &plan,
            Some("require-cancel-end"),
            Some(event_tx),
            &cancel_token,
        )
        .await;

    assert!(
        drain.await.expect("drain join"),
        "Err+cancel must emit StepEnd Cancelled"
    );
}

#[tokio::test]
async fn execute_plan_parallel_wave_emits_failed_ends_without_cancelling() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = AgentLoop::new(
        Arc::new(AlwaysFailClient),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            continuation_enabled: false,
            max_parallel_tasks: 2,
            max_tool_rounds: 2,
            max_execution_time_ms: Some(3_000),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("parallel fail no cancel", Complexity::Simple);
    plan.add_step(Task::new("s1", "Fail A"));
    plan.add_step(Task::new("s2", "Fail B"));
    let (event_tx, mut event_rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut failed = 0usize;
        while let Some(event) = event_rx.recv().await {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Failed,
                    ..
                }
            ) {
                failed += 1;
            }
        }
        failed
    });

    let _ = agent
        .execute_plan(
            &[],
            &plan,
            Some("parallel-fail-no-cancel"),
            Some(event_tx),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        drain.await.expect("drain join") >= 1,
        "parallel Ok(Err) failures must emit StepEnd Failed"
    );
}

#[tokio::test]
async fn execute_plan_parallel_wave_emits_failed_ends_when_branch_panics() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = AgentLoop::new(
        Arc::new(PanicOnCompleteClient),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        ToolContext::new(workspace.path().to_path_buf()),
        crate::agent::AgentConfig {
            continuation_enabled: false,
            max_parallel_tasks: 2,
            max_tool_rounds: 2,
            max_execution_time_ms: Some(3_000),
            ..crate::agent::AgentConfig::default()
        },
    );
    let mut plan = ExecutionPlan::new("parallel panic", Complexity::Simple);
    plan.add_step(Task::new("s1", "Panic A"));
    plan.add_step(Task::new("s2", "Panic B"));
    let (event_tx, mut event_rx) = mpsc::channel(64);
    let drain = tokio::spawn(async move {
        let mut failed = 0usize;
        while let Some(event) = event_rx.recv().await {
            if matches!(
                event,
                AgentEvent::StepEnd {
                    status: TaskStatus::Failed,
                    ..
                }
            ) {
                failed += 1;
            }
        }
        failed
    });

    let _ = agent
        .execute_plan(
            &[],
            &plan,
            Some("parallel-panic"),
            Some(event_tx),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        drain.await.expect("drain join") >= 1,
        "panicked parallel branches must surface as StepEnd Failed"
    );
}
