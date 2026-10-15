#![cfg(test)]

use super::*;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::{LlmClient, LlmResponse, StreamEvent, TokenUsage, ToolDefinition};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

struct Allocation {
    denied: AtomicBool,
    checks: AtomicUsize,
    records: AtomicUsize,
}

#[async_trait::async_trait]
impl BudgetGuard for Allocation {
    async fn check_before_llm(&self, _: &str, estimate: usize) -> BudgetDecision {
        assert!(estimate > 0);
        self.checks.fetch_add(1, Ordering::SeqCst);
        if self.denied.load(Ordering::SeqCst) {
            BudgetDecision::Deny {
                resource: "original host allocation".into(),
                reason: "not replenished".into(),
            }
        } else {
            BudgetDecision::Allow
        }
    }

    async fn record_after_llm(&self, _: &str, _: &TokenUsage) {
        self.records.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Answer {
    streaming: AtomicUsize,
    complete: AtomicUsize,
}

#[async_trait::async_trait]
impl LlmClient for Answer {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.complete.fetch_add(1, Ordering::SeqCst);
        panic!("successful original Done must not fallback")
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.streaming.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = mpsc::channel(1);
        sender
            .try_send(StreamEvent::Done(LlmResponse {
                message: Message::assistant("approved original answer"),
                usage: TokenUsage::default(),
                stop_reason: None,
                token_logprobs: vec![],
                meta: None,
            }))
            .unwrap();
        Ok(receiver)
    }
}

async fn scenario(surface: &str) {
    let workspace = tempfile::tempdir().unwrap();
    let guard = Arc::new(Allocation {
        denied: AtomicBool::new(true),
        checks: AtomicUsize::new(0),
        records: AtomicUsize::new(0),
    });
    let client = Arc::new(Answer::default());
    let session = test_agent()
        .await
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &test_options("typed-budget-reallocation").with_budget_guard(guard.clone()),
        )
        .unwrap();
    match surface {
        "send" => {
            session
                .send("denied original request", None)
                .await
                .unwrap_err();
        }
        "send-attachments" => {
            session
                .send_with_attachments("denied original request", &[], None)
                .await
                .unwrap_err();
        }
        _ => {
            let (mut events, worker) = if surface == "stream" {
                session
                    .stream("denied original request", None)
                    .await
                    .unwrap()
            } else {
                session
                    .stream_with_attachments("denied original request", &[], None)
                    .await
                    .unwrap()
            };
            let mut errors = 0;
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(event) = events.recv().await {
                    assert!(!matches!(event, AgentEvent::End { .. }));
                    errors += usize::from(matches!(event, AgentEvent::Error { .. }));
                }
                worker.await.unwrap();
            })
            .await
            .unwrap();
            assert_eq!(errors, 1, "{surface}");
        }
    }
    assert!(
        session
            .run_admission
            .wait_until_idle(Duration::from_secs(5))
            .await
    );
    assert_eq!(
        guard.checks.load(Ordering::SeqCst),
        1,
        "one host refusal is terminal: {surface}"
    );
    assert_eq!(guard.records.load(Ordering::SeqCst), 0);
    assert_eq!(client.streaming.load(Ordering::SeqCst), 0);
    assert_eq!(client.complete.load(Ordering::SeqCst), 0);
    assert!(!session.session_cancel.is_cancelled());
    let runs = session.run_store.list().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Failed);
    let failed_run = runs[0].id.clone();
    let events = session.run_store.events(&failed_run).await;
    let threshold: Vec<_> = events.iter().enumerate().filter(|(_, event)| matches!(&event.event,
        AgentEvent::BudgetThresholdHit { kind, resource, .. } if kind == "hard" && resource == "original host allocation"
    )).collect();
    let errors: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(&event.event, AgentEvent::Error { .. }))
        .collect();
    assert_eq!(threshold.len(), 1, "{surface}");
    assert_eq!(errors.len(), 1, "{surface}");
    assert!(threshold[0].0 < errors[0].0);
    assert!(!events
        .iter()
        .any(|event| matches!(event.event, AgentEvent::End { .. })));
    assert!(!events.iter().any(|event| matches!(
        event.event,
        AgentEvent::ModelInputBound { .. } | AgentEvent::ModelUsageBound { .. }
    )));
    let facts = crate::fact_control::read_workspace_facts(
        workspace.path(),
        &crate::fact_control::thread_for_session("typed-budget-reallocation"),
    )
    .unwrap();
    assert!(!facts.iter().any(|fact| fact.kind == "model.turn"
        || fact.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND));

    // The same host may replenish allocation. Refusal neither cancels nor
    // closes this Session; the next original admission still checks policy.
    guard.denied.store(false, Ordering::SeqCst);
    let answer = session
        .send("approved original request", None)
        .await
        .unwrap();
    assert_eq!(answer.text, "approved original answer");
    assert_eq!(answer.tool_calls_count, 0);
    assert_eq!(guard.checks.load(Ordering::SeqCst), 2);
    assert_eq!(guard.records.load(Ordering::SeqCst), 1);
    assert_eq!(client.streaming.load(Ordering::SeqCst), 1);
    assert_eq!(client.complete.load(Ordering::SeqCst), 0);
    let runs = session.run_store.list().await;
    assert_eq!(runs.len(), 2);
    assert!(runs
        .iter()
        .any(|run| run.id != failed_run && run.status == RunStatus::Completed));
    assert_eq!(
        session
            .run_store
            .snapshot(&failed_run)
            .await
            .unwrap()
            .status,
        RunStatus::Failed
    );
    session.close().await;
}

#[tokio::test]
async fn denied_original_budget_stops_once_and_same_session_accepts_reallocated_budget() {
    for surface in ["send", "send-attachments", "stream", "stream-attachments"] {
        tokio::time::timeout(Duration::from_secs(15), scenario(surface))
            .await
            .unwrap();
    }
}
