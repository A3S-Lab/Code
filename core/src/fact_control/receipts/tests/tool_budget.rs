#![cfg(test)]

use super::*;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::fact_control::tool_history_tests::{surface_with_budget, ObservedClient};

struct Guard {
    entries: AtomicUsize,
    decision: BudgetDecision,
}

#[async_trait::async_trait]
impl BudgetGuard for Guard {
    async fn check_before_tool(&self, session: &str, name: &str) -> BudgetDecision {
        assert_eq!(session, THREAD);
        assert_eq!(name, "receipt_probe");
        self.entries.fetch_add(1, Ordering::SeqCst);
        self.decision.clone()
    }
}

async fn exercise(
    decision: BudgetDecision,
    permitted: bool,
    cancelled: bool,
) -> (
    ToolExecutionResultV1,
    usize,
    usize,
    Vec<crate::agent::AgentEvent>,
) {
    let root = tempfile::tempdir().unwrap();
    let (tools, effects) = executor_tools(root.path());
    let guard = Arc::new(Guard {
        entries: AtomicUsize::new(0),
        decision,
    });
    let mut tools = tools.as_ref().clone();
    tools.agent = Some(
        surface_with_budget(
            root.path(),
            Arc::new(ObservedClient::default()),
            Some(guard.clone()),
        )
        .agent,
    );
    tools.context.session_id = Some(THREAD.into());
    if !permitted {
        tools.permission = PermissionPolicy::new().deny("*");
    }
    let (sender, mut receiver) = tokio::sync::mpsc::channel(128);
    tools.events = Some(sender);
    let tools = Arc::new(tools);
    let cancel = tokio_util::sync::CancellationToken::new();
    if cancelled {
        cancel.cancel();
    }
    let prepared = prepare_tools(
        tools.clone(),
        request("budget", json!({"write":true})),
        cancel,
    )
    .await
    .unwrap();
    let result = execute_prepared(tools, prepared).await;
    result.validate().unwrap();
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }
    (
        result,
        guard.entries.load(Ordering::SeqCst),
        effects.load(Ordering::SeqCst),
        events,
    )
}

#[tokio::test]
async fn denied_tool_budget_prevents_execution_and_keeps_original_typed_receipt() {
    let (result, entries, effects, events) = exercise(
        BudgetDecision::Deny {
            resource: "tool quota".into(),
            reason: "host exhausted".into(),
        },
        true,
        false,
    )
    .await;
    assert_eq!(entries, 1);
    assert_eq!(effects, 0);
    assert_eq!(result.outcome, ToolExecutionOutcome::Denied);
    assert_eq!(result.identity.tool_call_id, "budget");
    assert!(result.effects.changes.is_empty());
    assert_eq!(
        result.effects.completeness,
        ToolObservationCompleteness::Complete
    );
    assert!(events.iter().any(|e| matches!(e, crate::agent::AgentEvent::BudgetThresholdHit { kind, resource, .. } if kind == "hard" && resource == "tool quota")));
    assert!(!events
        .iter()
        .any(|e| matches!(e, crate::agent::AgentEvent::ToolExecutionStart { .. })));
}

#[tokio::test]
async fn allowed_tool_budget_is_checked_once_before_the_original_effect() {
    let (result, entries, effects, _) = exercise(BudgetDecision::Allow, true, false).await;
    assert_eq!(entries, 1);
    assert_eq!(effects, 1);
    assert_eq!(result.outcome, ToolExecutionOutcome::Succeeded);
}

#[tokio::test]
async fn soft_tool_budget_preserves_execution_and_reports_the_original_limit() {
    let (result, entries, effects, events) = exercise(
        BudgetDecision::SoftLimit {
            resource: "tool quota".into(),
            consumed: 3.0,
            limit: 4.0,
            message: Some("near limit".into()),
        },
        true,
        false,
    )
    .await;
    assert_eq!(entries, 1);
    assert_eq!(effects, 1);
    assert_eq!(result.outcome, ToolExecutionOutcome::Succeeded);
    assert!(events.iter().any(|e| matches!(e, crate::agent::AgentEvent::BudgetThresholdHit { kind, consumed, limit, .. } if kind == "soft" && *consumed == 3.0 && *limit == 4.0)));
}

#[tokio::test]
async fn permission_refusal_and_precancel_do_not_enter_the_tool_budget() {
    for (permitted, cancelled) in [(false, false), (true, true)] {
        let (result, entries, effects, _) =
            exercise(BudgetDecision::Allow, permitted, cancelled).await;
        assert_eq!(entries, 0);
        assert_eq!(effects, 0);
        assert!(matches!(
            result.outcome,
            ToolExecutionOutcome::Denied | ToolExecutionOutcome::Cancelled
        ));
    }
}
