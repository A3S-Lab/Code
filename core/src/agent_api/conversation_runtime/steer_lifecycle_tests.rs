#![cfg(test)]

use super::*;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::{LlmClient, LlmResponse, StreamEvent, TokenUsage, ToolDefinition};
use crate::run_control::{RunControlReceiptState, SteerRequest};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Semaphore;

struct AccountingBarrier {
    entered: Semaphore,
    release: Semaphore,
}
#[async_trait::async_trait]
impl BudgetGuard for AccountingBarrier {
    async fn check_before_llm(&self, _: &str, _: usize) -> BudgetDecision {
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, _: &str, _: &TokenUsage) {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }
}
#[derive(Default)]
struct ActualAnswer(AtomicUsize);
#[async_trait::async_trait]
impl LlmClient for ActualAnswer {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        panic!("actual streaming Done must not fall back");
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(1);
        tx.try_send(StreamEvent::Done(LlmResponse {
            message: Message::assistant("The original answer."),
            usage: TokenUsage::default(),
            stop_reason: None,
            token_logprobs: vec![],
            meta: None,
        }))
        .unwrap();
        Ok(rx)
    }
}

#[tokio::test]
async fn joined_aborted_worker_fails_steer_and_closes_wire_before_accounting_returns() {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = super::super::test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = crate::Agent::from_config(config).await.unwrap();
    let callback = Arc::new(AccountingBarrier {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let client = Arc::new(ActualAnswer::default());
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &crate::SessionOptions::new()
                .with_session_id("aborted-steer")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_budget_guard(callback.clone()),
        )
        .unwrap();
    let mut stream = session
        .stream_with_run("Finish this answer.", None)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), callback.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let run = stream.run.as_ref().unwrap();
    let original = session.run_admission.current_original().unwrap();
    let inbox = RunControlState::from_session(&session)
        .current_run_control()
        .await
        .unwrap();
    let text = "  未保存的指令\n\t";
    let mut request = SteerRequest::new(text).with_run_id(run.id());
    request.request_id = Some("same-aborted-request".into());
    let retry = request.clone().into_protocol("aborted-steer", run.id());
    assert_eq!(
        session.steer(request).await.unwrap().state,
        RunControlReceiptState::Accepted
    );
    assert!(session
        .run_admission
        .abort_original_stream_workers(&original));
    tokio::time::timeout(Duration::from_secs(5), async {
        while original.observed_worker_join().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        original.observed_worker_join(),
        Some(run_admission::ActualJoinObservation::Cancelled)
    );
    // The actual accounting callback is still blocked. Failed receipts and host
    // EOF cannot depend on joining that original child first.
    let receipt = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let receipt = inbox.submit(retry.clone(), 1).await.unwrap();
            if receipt.state != RunControlReceiptState::Accepted {
                break receipt;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actual joined owner must decide failed input before accounting returns");
    assert_eq!(receipt.state, RunControlReceiptState::Settled);
    assert_eq!(
        receipt.error.as_ref().unwrap().code,
        "STEER_PERSISTENCE_FAILED"
    );
    let mut errors = 0;
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = stream.events.recv().await {
            assert!(!matches!(event, AgentEvent::End { .. }));
            errors += usize::from(matches!(event, AgentEvent::Error { .. }));
        }
    })
    .await
    .expect("failure publisher must drop sender before child join");
    assert_eq!(errors, 1);
    assert_eq!(
        session.run_store.snapshot(run.id()).await.unwrap().status,
        crate::run::RunStatus::Failed
    );
    assert!(
        !session
            .cancel_and_settle(Duration::from_millis(20), Duration::from_millis(20))
            .await
    );
    assert!(session
        .run_admission
        .current_original()
        .is_some_and(|actual| Arc::ptr_eq(&actual, &original)));
    let facts = crate::fact_control::read_workspace_facts(
        workspace.path(),
        &crate::fact_control::thread_for_session("aborted-steer"),
    )
    .unwrap();
    assert!(!facts
        .iter()
        .any(|f| f.kind == "user.message" && f.payload["text"] == text));
    assert!(!facts
        .iter()
        .any(|f| f.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND));
    assert_eq!(client.0.load(Ordering::SeqCst), 1);
    callback.release.add_permits(1);
    assert!(
        !session
            .run_admission
            .wait_until_idle(Duration::from_millis(30))
            .await
    );
    assert_eq!(inbox.submit(retry, 2).await.unwrap(), receipt);
    drop(stream.worker); // Observer Drop never retires the unproved original.
}
