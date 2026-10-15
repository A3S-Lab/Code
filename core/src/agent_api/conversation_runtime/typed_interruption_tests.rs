#![cfg(test)]

use super::*;
use crate::run::RunStatus;
use tokio_util::sync::CancellationToken;

mod budget_denial_tests;
mod cancellation_publication_tests;

struct NoCommands;

#[async_trait::async_trait]
impl crate::sandbox::BashSandbox for NoCommands {
    async fn exec_command(
        &self,
        _: &str,
        _: &str,
    ) -> anyhow::Result<crate::sandbox::SandboxOutput> {
        anyhow::bail!("typed lifecycle tests refuse command execution")
    }

    async fn shutdown(&self) {}
}

fn test_options(session_id: &str) -> crate::SessionOptions {
    crate::SessionOptions::new()
        .with_session_id(session_id)
        .with_sandbox_handle(Arc::new(NoCommands))
        .with_permission_policy(crate::permissions::PermissionPolicy::new().deny("*"))
        .with_planning_mode(crate::prompts::PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
}

async fn test_agent() -> crate::Agent {
    let mut config = crate::agent_api::tests::test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    crate::Agent::from_config(config).await.unwrap()
}

async fn session(workspace: &std::path::Path, text: &str) -> AgentSession {
    test_agent()
        .await
        .build_session(
            workspace.to_string_lossy().into_owned(),
            Arc::new(crate::agent_api::tests::StaticStreamingClient::new(text)),
            &test_options("typed-interruption"),
        )
        .unwrap()
}

fn result(text: &str) -> AgentResult {
    AgentResult {
        text: text.into(),
        messages: Vec::new(),
        usage: crate::llm::TokenUsage {
            prompt_tokens: 3,
            completion_tokens: 2,
            total_tokens: 5,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        tool_calls_count: 0,
        verification_reports: Vec::new(),
        completion: crate::harness_loop::CompletionTerminal::Narrative,
        forced_stop: None,
        run_admission: "ordinary".into(),
    }
}

async fn finish(session: &AgentSession, token: CancellationToken) -> TurnFinish {
    session
        .run_store
        .reserve_run_with_id("typed-finish".into(), session.session_id(), "request")
        .await;
    *session.current_run_id.lock().await = Some("typed-finish".into());
    *session.cancel_token.lock().await = Some(token.clone());
    TurnFinish::from_session(session, token, Some("typed-finish".into()))
}

#[tokio::test]
async fn ordinary_interruption_words_complete_across_conversation_surfaces() {
    for text in [
        "interrupted",
        "completed without being interrupted",
        "Agent Run interrupted during capability cleanup",
    ] {
        for surface in ["send", "stream", "send-attachments", "stream-attachments"] {
            let workspace = tempfile::tempdir().unwrap();
            let session = session(workspace.path(), text).await;
            match surface {
                "send" => assert_eq!(session.send("request", None).await.unwrap().text, text),
                "send-attachments" => assert_eq!(
                    session
                        .send_with_attachments("request", &[], None)
                        .await
                        .unwrap()
                        .text,
                    text
                ),
                _ => {
                    let (mut events, worker) = if surface == "stream" {
                        session.stream("request", None).await.unwrap()
                    } else {
                        session
                            .stream_with_attachments("request", &[], None)
                            .await
                            .unwrap()
                    };
                    let mut ends = Vec::new();
                    while let Some(event) = events.recv().await {
                        if let AgentEvent::End { text, .. } = event {
                            ends.push(text);
                        }
                    }
                    worker.await.unwrap();
                    assert_eq!(ends, [text], "{surface}");
                }
            }
            let runs = session.run_store.list().await;
            assert_eq!(runs.len(), 1, "{surface}");
            assert_eq!(runs[0].status, RunStatus::Completed, "{surface}: {text}");
            let events = session.run_store.events(&runs[0].id).await;
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event.event, AgentEvent::End { .. }))
                    .count(),
                1,
                "{surface}: {text}"
            );
            assert!(session.cancel_token.lock().await.is_none());
            assert!(!events.iter().any(|event| matches!(
                event.event,
                AgentEvent::RunControlApplied {
                    operation: crate::run_control::RunControlOperation::Interrupt,
                    ..
                }
            )));
            session.close().await;
        }
    }
}

#[tokio::test]
async fn cancelled_success_uses_owned_token_after_active_slot_is_cleared() {
    let workspace = tempfile::tempdir().unwrap();
    let session = session(workspace.path(), "unused").await;
    let token = CancellationToken::new();
    let finish = finish(&session, token.clone()).await;
    *session.cancel_token.lock().await = None;
    token.cancel();
    finish
        .publish("request", &Ok(result("arbitrary completed text")), false)
        .await;
    assert_eq!(
        session
            .run_store
            .snapshot("typed-finish")
            .await
            .unwrap()
            .status,
        RunStatus::Cancelled
    );
    assert!(!session
        .run_store
        .events("typed-finish")
        .await
        .iter()
        .any(|event| matches!(event.event, AgentEvent::End { .. })));
    assert!(session.current_run_id.lock().await.is_none());
}

#[tokio::test]
async fn public_cancel_keeps_cancellation_and_original_cleanup_error_event() {
    use crate::agent_api::tests::NonRetryableStreamingClient;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FailedClose(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl crate::capability::CapabilityEffect for FailedClose {
        fn name(&self) -> &str {
            "original-failed-teardown"
        }
        async fn close(
            self: Box<Self>,
        ) -> std::result::Result<(), crate::capability::CapabilityEffectError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(crate::capability::CapabilityEffectError::new(
                "cleanup remains unproved",
            ))
        }
    }

    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(NonRetryableStreamingClient::default());
    let session = test_agent()
        .await
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &test_options("typed-interruption")
                .with_session_store(Arc::new(crate::store::MemorySessionStore::new()))
                .with_auto_save(true),
        )
        .unwrap();
    let lease = ExecutionCoordinator::admit(&session, "actual cancellation publication")
        .await
        .unwrap();
    let mut captured = OwnedConversationCapture::capture(&session, false).unwrap();
    let (handoff, start, original) = ExecutionCoordinator::prepare_original_handoff(
        &session,
        lease,
        run_admission::ExecutionMode::Blocking,
        captured.preparation.cancellation.clone(),
    )
    .unwrap();
    captured.controls = captured.controls.bind_original(original.clone()).unwrap();
    let context = original.clone();
    let effects = Arc::new(AtomicUsize::new(0));
    let actual_effects = effects.clone();
    let (ready, ready_rx) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let (reported, report) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        context
            .record_scope_not_attempted(run_admission::OriginalScopeKind::Command)
            .unwrap();
        let mut pinned = prepare_owned_fact(captured, context.clone(), "request", None, None)
            .await
            .unwrap();
        pinned
            .guard
            .run_scope()
            .register_effect(FailedClose(actual_effects))
            .unwrap();
        ready
            .send((
                pinned.run.clone(),
                pinned.parts.surface.as_ref().unwrap().cancel.clone(),
            ))
            .unwrap();
        released.await.unwrap();
        let outcome =
            settle_owned_capability_run(&pinned, Ok(result("ordinary completed answer"))).await;
        assert!(matches!(
            outcome
                .as_ref()
                .unwrap_err()
                .downcast_ref::<crate::capability::CapabilityRuntimeError>(),
            Some(crate::capability::CapabilityRuntimeError::RunCloseIncomplete { .. })
        ));
        let diagnostic = outcome.as_ref().unwrap_err().to_string();
        pinned.parts.retire_fact_host_events();
        pinned.parts.retire_checkpoint_senders();
        observe_owned_committer(&context).await.unwrap();
        context.seal_original_controls().await.unwrap();
        pinned
            .finish
            .publish_owned(&context, &outcome, false)
            .await
            .unwrap();
        assert!(
            pinned.cleanup.finish_owned(&context).await.is_err(),
            "failed original close cannot clear slots"
        );
        reported.send(diagnostic).unwrap();
    });
    let (disposition, observed) = tokio::sync::oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), disposition);
    let (run, token) = tokio::time::timeout(std::time::Duration::from_secs(5), ready_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(session.cancel().await);
    assert!(token.is_cancelled());
    assert_eq!(
        session.current_run_id.lock().await.as_deref(),
        Some(run.id())
    );
    assert!(Arc::ptr_eq(
        &original,
        &session.run_admission.current_original().unwrap()
    ));
    release.send(()).unwrap();
    let diagnostic = tokio::time::timeout(std::time::Duration::from_secs(5), report)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), observed)
            .await
            .unwrap()
            .unwrap(),
        run_admission::OwnerDisposition::RetainedUnproved
    );
    assert_eq!(
        original.observed_worker_join(),
        Some(run_admission::ActualJoinObservation::Completed)
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(
        session.run_store.snapshot(run.id()).await.unwrap().status,
        RunStatus::Cancelled
    );
    let events = session.run_store.events(run.id()).await;
    assert_eq!(events.iter().filter(|event| matches!(&event.event, AgentEvent::Error { message } if message == &diagnostic)).count(), 1);
    assert!(!events
        .iter()
        .any(|event| matches!(event.event, AgentEvent::End { .. })));
    assert_eq!(
        session.current_run_id.lock().await.as_deref(),
        Some(run.id())
    );
    assert!(Arc::ptr_eq(
        &original,
        &session.run_admission.current_original().unwrap()
    ));
    assert!(!proxy.is_finished());
    let health = session.task_scheduler.health().await.unwrap();
    assert_eq!((health.active, health.released), (1, 0));
    assert!(
        !session
            .cancel_and_settle(
                std::time::Duration::from_millis(20),
                std::time::Duration::from_millis(20)
            )
            .await
    );
    assert!(
        ExecutionCoordinator::admit(&session, "must not reuse failed close")
            .await
            .is_err()
    );
    assert_eq!(client.complete_calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.streaming_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn provider_error_with_interruption_word_is_failed_without_cancellation() {
    let workspace = tempfile::tempdir().unwrap();
    let session = session(workspace.path(), "unused").await;
    let finish = finish(&session, CancellationToken::new()).await;
    finish
        .publish(
            "request",
            &Err(anyhow::anyhow!("provider stream interrupted")),
            false,
        )
        .await;
    let snapshot = session.run_store.snapshot("typed-finish").await.unwrap();
    assert_eq!(snapshot.status, RunStatus::Failed);
    assert_eq!(
        snapshot.error.as_deref(),
        Some("provider stream interrupted")
    );
}

#[tokio::test]
async fn tool_budget_forced_stop_remains_distinct_from_host_cancellation() {
    let workspace = tempfile::tempdir().unwrap();
    let session = session(workspace.path(), "unused").await;
    let finish = finish(&session, CancellationToken::new()).await;
    let mut result = result("interrupted by the tool budget");
    result.forced_stop = Some(crate::harness_loop::ForcedStop::ToolBudget);
    finish.publish("request", &Ok(result), false).await;
    assert_eq!(
        session
            .run_store
            .snapshot("typed-finish")
            .await
            .unwrap()
            .status,
        RunStatus::Completed
    );
    assert!(session
        .run_store
        .events("typed-finish")
        .await
        .iter()
        .any(|event| matches!(
            event.event,
            AgentEvent::End {
                forced_stop: Some(crate::harness_loop::ForcedStop::ToolBudget),
                ..
            }
        )));
}

#[tokio::test]
async fn budget_guard_denial_fails_without_cancelling_or_calling_provider() {
    use crate::agent_api::tests::{DenyingBudgetGuard, NonRetryableStreamingClient};
    use std::sync::atomic::Ordering;
    let workspace = tempfile::tempdir().unwrap();
    let guard = Arc::new(DenyingBudgetGuard::default());
    let client = Arc::new(NonRetryableStreamingClient::default());
    let session = test_agent()
        .await
        .build_session(
            workspace.path().to_string_lossy().into_owned(),
            client.clone(),
            &test_options("typed-budget-denial").with_budget_guard(guard.clone()),
        )
        .unwrap();
    session.send("request", None).await.unwrap_err();
    assert_eq!(guard.checks.load(Ordering::SeqCst), 1);
    assert_eq!(guard.llm_records.load(Ordering::SeqCst), 0);
    assert_eq!(client.complete_calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.streaming_calls.load(Ordering::SeqCst), 0);
    let runs = session.run_store.list().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Failed);
    assert!(!session
        .run_store
        .events(&runs[0].id)
        .await
        .iter()
        .any(|event| matches!(event.event, AgentEvent::End { .. })));
    session.close().await;
}
