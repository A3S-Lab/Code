#![cfg(test)]

use super::*;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::fact_control::tool_history_tests::{surface_with_budget, ObservedClient};
use std::time::Duration;

#[derive(Default)]
struct PendingGuard {
    entered: AtomicUsize,
    active: AtomicUsize,
    dropped: AtomicUsize,
    completed: AtomicUsize,
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

struct OriginalGuardCall<'a>(&'a PendingGuard);
impl Drop for OriginalGuardCall<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl BudgetGuard for PendingGuard {
    async fn check_before_tool(&self, session: &str, name: &str) -> BudgetDecision {
        assert_eq!((session, name), (THREAD, "receipt_probe"));
        self.entered.fetch_add(1, Ordering::SeqCst);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _original = OriginalGuardCall(self);
        self.started.notify_one();
        self.release.notified().await;
        self.completed.fetch_add(1, Ordering::SeqCst);
        BudgetDecision::Allow
    }
}

async fn cancellation(also_ready: bool) {
    let root = tempfile::tempdir().unwrap();
    let (tools, effects) = executor_tools(root.path());
    let guard = Arc::new(PendingGuard::default());
    let mut tools = tools.as_ref().clone();
    tools.context.session_id = Some(THREAD.into());
    tools.agent = Some(
        surface_with_budget(
            root.path(),
            Arc::new(ObservedClient::default()),
            Some(guard.clone()),
        )
        .agent,
    );
    let tools = Arc::new(tools);
    let cancel = tokio_util::sync::CancellationToken::new();
    let running_tools = tools.clone();
    let running_cancel = cancel.clone();
    let mut running = tokio::spawn(async move {
        prepare_tools(
            running_tools,
            request("original-cancelled", json!({"write":true})),
            running_cancel,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), guard.started.notified())
        .await
        .unwrap();
    assert_eq!(guard.active.load(Ordering::SeqCst), 1);
    cancel.cancel();
    if also_ready {
        guard.release.notify_one();
    }
    let observed = tokio::time::timeout(Duration::from_millis(250), &mut running).await;
    let timely = observed.is_ok();
    let active_at_observation = guard.active.load(Ordering::SeqCst);
    let completed_at_observation = guard.completed.load(Ordering::SeqCst);
    // Retain and drain the exact task even when the baseline is stuck. A
    // diagnostic timeout does not become evidence of cleanup or permission
    // to restart the preparation.
    let prepared = match observed {
        Ok(joined) => joined.unwrap().unwrap(),
        Err(_) => {
            guard.release.notify_one();
            tokio::time::timeout(Duration::from_secs(2), running)
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        }
    };
    prepared.prepared.validate().unwrap();
    let original = prepared.prepared.clone();
    let result = execute_prepared(tools, prepared).await;
    result.ensure_prepared(&original).unwrap();
    assert_eq!(result.outcome, ToolExecutionOutcome::Cancelled);
    assert_eq!(result.identity.tool_call_id, "original-cancelled");
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(result.effects.changes.is_empty());
    assert_eq!(guard.entered.load(Ordering::SeqCst), 1);
    assert_eq!(guard.active.load(Ordering::SeqCst), 0);
    assert_eq!(guard.dropped.load(Ordering::SeqCst), 1);
    assert!(timely, "original tool-budget future still live after cancellation: active={active_at_observation}, completed={completed_at_observation}; exact task drained after releasing the diagnostic barrier");
    assert_eq!(
        completed_at_observation, 0,
        "cancellation wins over a simultaneously ready budget decision"
    );
}

#[tokio::test]
async fn cancellation_drops_the_original_pending_budget_future_before_tool_effects() {
    cancellation(false).await;
}

#[tokio::test]
async fn cancellation_wins_over_a_ready_original_budget_decision() {
    cancellation(true).await;
}
