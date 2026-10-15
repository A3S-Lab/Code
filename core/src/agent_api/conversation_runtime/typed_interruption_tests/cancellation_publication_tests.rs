#![cfg(test)]

use super::*;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::{ContentBlock, LlmClient, LlmResponse, StreamEvent, ToolDefinition};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Default)]
struct WaitingBudget {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    active: AtomicUsize,
    dropped: AtomicUsize,
    completed: AtomicUsize,
}

struct OriginalCallback<'a>(&'a WaitingBudget);

impl Drop for OriginalCallback<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl BudgetGuard for WaitingBudget {
    async fn check_before_tool(&self, _: &str, name: &str) -> BudgetDecision {
        assert_eq!(name, "read");
        self.active.fetch_add(1, Ordering::SeqCst);
        let _original = OriginalCallback(self);
        self.entered.notify_one();
        self.release.notified().await;
        self.completed.fetch_add(1, Ordering::SeqCst);
        BudgetDecision::Allow
    }
}

#[derive(Default)]
struct ReadThenAnswer(AtomicUsize);

#[async_trait::async_trait]
impl LlmClient for ReadThenAnswer {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        panic!("original Done must not fall back")
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let message = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            Message {
                role: "assistant".into(),
                content: vec![ContentBlock::ToolUse {
                    id: "original-cancelled-read".into(),
                    name: "read".into(),
                    input: serde_json::json!({"file_path":"note.txt"}),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            }
        } else {
            Message::assistant("OK")
        };
        let (tx, rx) = mpsc::channel(1);
        tx.try_send(StreamEvent::Done(LlmResponse {
            message,
            usage: crate::llm::TokenUsage::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        }))
        .unwrap();
        Ok(rx)
    }
}

enum Observation {
    Blocking(crate::error::Result<AgentResult>),
    Streaming(Vec<AgentEvent>),
}

async fn cancelled_surface(surface: &'static str) {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("note.txt"), "unchanged fixture").unwrap();
    let guard = Arc::new(WaitingBudget::default());
    let client = Arc::new(ReadThenAnswer::default());
    let session = Arc::new(
        test_agent()
            .await
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &test_options("cancel-publication")
                    .with_budget_guard(guard.clone())
                    .with_permission_policy(crate::permissions::PermissionPolicy {
                        default_decision: crate::permissions::PermissionDecision::Deny,
                        ..crate::permissions::PermissionPolicy::new().allow("read")
                    }),
            )
            .unwrap(),
    );
    let original_session = session.clone();
    let mut observer = tokio::spawn(async move {
        match surface {
            "send" => Observation::Blocking(original_session.send("read note", None).await),
            "send-attachments" => Observation::Blocking(
                original_session
                    .send_with_attachments("read note", &[], None)
                    .await,
            ),
            _ => {
                let (mut events, worker) = if surface == "stream" {
                    original_session.stream("read note", None).await.unwrap()
                } else {
                    original_session
                        .stream_with_attachments("read note", &[], None)
                        .await
                        .unwrap()
                };
                let mut received = Vec::new();
                while let Some(event) = events.recv().await {
                    received.push(event);
                }
                worker.await.unwrap();
                Observation::Streaming(received)
            }
        }
    });
    tokio::time::timeout(Duration::from_secs(5), guard.entered.notified())
        .await
        .unwrap();
    let original = session.run_store.list().await;
    assert_eq!(original.len(), 1);
    let original_id = original[0].id.clone();
    assert!(session.cancel().await);
    let observed = tokio::time::timeout(Duration::from_secs(5), &mut observer).await;
    let timely = observed.is_ok();
    let observation = match observed {
        Ok(joined) => joined.unwrap(),
        Err(_) => {
            // A timeout retains and drains the same original observer and
            // execution. Releasing a diagnostic latch is never acceptance.
            guard.release.notify_one();
            tokio::time::timeout(Duration::from_secs(5), observer)
                .await
                .unwrap()
                .unwrap()
        }
    };
    assert!(timely);
    assert_eq!(guard.active.load(Ordering::SeqCst), 0);
    assert_eq!(guard.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(guard.completed.load(Ordering::SeqCst), 0);
    assert_eq!(
        session
            .run_store
            .snapshot(&original_id)
            .await
            .unwrap()
            .status,
        RunStatus::Cancelled
    );
    let retained = session.run_store.events(&original_id).await;
    assert!(!retained.iter().any(|v| matches!(
        v.event,
        AgentEvent::End { .. } | AgentEvent::ToolExecutionStart { .. }
    )));
    match observation {
        Observation::Blocking(answer) => {
            let error = answer.expect_err("cancelled Run cannot return a successful AgentResult");
            assert_eq!(error.runtime_code(), "RUN_CANCELLED", "{surface}: {error}");
            assert_eq!(error.cancelled_run_id(), Some(original_id.as_str()));
        }
        Observation::Streaming(events) => {
            assert!(
                !events.iter().any(|v| matches!(v, AgentEvent::End { .. })),
                "{surface}"
            );
            let errors: Vec<_> = events
                .iter()
                .filter_map(|v| match v {
                    AgentEvent::Error { message } => Some(message),
                    _ => None,
                })
                .collect();
            assert_eq!(errors.len(), 1, "{surface}");
            assert_eq!(
                errors[0],
                &CodeError::run_cancelled(original_id.clone()).to_string()
            );
        }
    }
    assert!(
        session
            .cancel_and_settle(Duration::from_secs(5), Duration::ZERO)
            .await
    );
    assert!(!session.session_cancel.is_cancelled());
    assert_eq!(
        session
            .send("new independent turn", None)
            .await
            .unwrap()
            .text,
        "OK"
    );
    assert_eq!(client.0.load(Ordering::SeqCst), 2);
    assert_eq!(
        session
            .run_store
            .snapshot(&original_id)
            .await
            .unwrap()
            .status,
        RunStatus::Cancelled
    );
    session.close().await;
}

#[tokio::test]
async fn cancelled_send_returns_original_run_error() {
    cancelled_surface("send").await;
}

#[tokio::test]
async fn cancelled_send_with_attachments_returns_original_run_error() {
    cancelled_surface("send-attachments").await;
}

#[tokio::test]
async fn cancelled_stream_has_no_success_end() {
    cancelled_surface("stream").await;
}

#[tokio::test]
async fn cancelled_stream_with_attachments_has_no_success_end() {
    cancelled_surface("stream-attachments").await;
}

type CancelSlot = Arc<tokio::sync::Mutex<Option<CancellationToken>>>;

#[derive(Debug, Default)]
struct CancelAfterRetainedEnd {
    slot: std::sync::Mutex<Option<CancelSlot>>,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::hooks::HookExecutor for CancelAfterRetainedEnd {
    async fn fire(&self, _: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        crate::hooks::HookResult::continue_()
    }

    async fn record_agent_event(&self, event: &AgentEvent, _: &str, _: &str) {
        if matches!(event, AgentEvent::End { .. }) {
            let slot = self.slot.lock().unwrap().clone().unwrap();
            let token = slot.lock().await.clone().unwrap();
            self.calls.fetch_add(1, Ordering::SeqCst);
            token.cancel();
        }
    }
}

#[tokio::test]
async fn late_token_cancel_cannot_reverse_the_original_sealed_success() {
    for surface in ["send", "stream"] {
        let workspace = tempfile::tempdir().unwrap();
        let hook = Arc::new(CancelAfterRetainedEnd::default());
        let session = test_agent()
            .await
            .build_session(
                workspace.path().display().to_string(),
                Arc::new(crate::agent_api::tests::StaticStreamingClient::new(
                    "ordinary answer",
                )),
                &test_options("late-publication-cancel").with_hook_executor(hook.clone()),
            )
            .unwrap();
        *hook.slot.lock().unwrap() = Some(session.cancel_token.clone());
        if surface == "send" {
            assert_eq!(
                session.send("request", None).await.unwrap().text,
                "ordinary answer"
            );
        } else {
            let (mut events, worker) = session.stream("request", None).await.unwrap();
            let mut ends = 0;
            while let Some(event) = events.recv().await {
                assert!(!matches!(event, AgentEvent::Error { .. }));
                ends += usize::from(matches!(event, AgentEvent::End { .. }));
            }
            worker.await.unwrap();
            assert_eq!(ends, 1);
        }
        assert_eq!(hook.calls.load(Ordering::SeqCst), 1);
        let runs = session.run_store.list().await;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, RunStatus::Completed);
        session.close().await;
    }
}
