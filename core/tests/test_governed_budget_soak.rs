//! S-GT-01: after the host budget is already exhausted, governed calls must
//! not enter a tool body, and the host counter must stay at the floor.

use a3s_code_core::budget::{BudgetDecision, BudgetGuard};
use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::hitl::ConfirmationPolicy;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, SessionOptions};
use async_trait::async_trait;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;

struct ExhaustedFloor {
    remaining: AtomicI64,
    checks: AtomicUsize,
}

#[async_trait]
impl BudgetGuard for ExhaustedFloor {
    async fn check_before_tool(&self, _session_id: &str, _tool_name: &str) -> BudgetDecision {
        self.checks.fetch_add(1, Ordering::SeqCst);
        let current = self.remaining.load(Ordering::SeqCst);
        if current <= 0 {
            return BudgetDecision::Deny {
                resource: "tool_calls".to_string(),
                reason: "exhausted".to_string(),
            };
        }
        self.remaining.fetch_sub(1, Ordering::SeqCst);
        BudgetDecision::Allow
    }
}

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                family: "claude-sonnet".to_string(),
                api_key: None,
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "S-GT-01 soak: 200 governed calls after budget exhaustion"]
async fn soak_exhausted_budget_rejects_two_hundred_governed_calls() {
    let workspace = tempfile::tempdir().expect("workspace");
    let guard = Arc::new(ExhaustedFloor {
        remaining: AtomicI64::new(0),
        checks: AtomicUsize::new(0),
    });
    let mut policy = PermissionPolicy::new().deny("bash").allow("write");
    policy.default_decision = PermissionDecision::Allow;
    let agent = Agent::from_config(offline_config()).await.expect("agent");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("s-gt-01")
                    .with_model("anthropic/claude-sonnet-4-20250514")
                    .with_budget_guard(guard.clone())
                    .with_permission_policy(policy)
                    .with_confirmation_policy(ConfirmationPolicy::enabled()),
            ),
        )
        .await
        .expect("session");

    let mut permission_denies = 0usize;
    let mut budget_denies = 0usize;
    for index in 0..200 {
        let result = if index % 4 == 0 {
            session
                .governed_tool(
                    "bash",
                    serde_json::json!({"command": "echo LEAK > budget-soak-bash.txt"}),
                )
                .await
                .expect("permission deny is a tool result, not a session error")
        } else {
            session
                .governed_tool(
                    "write",
                    serde_json::json!({
                        "file_path": "budget-soak-leak.txt",
                        "content": "LEAK"
                    }),
                )
                .await
                .expect("budget deny is a tool result, not a session error")
        };
        assert_ne!(result.exit_code, 0, "{}", result.output);
        if result.output.contains("Permission denied") {
            permission_denies += 1;
        } else if result.output.contains("Budget exhausted on 'tool_calls'") {
            budget_denies += 1;
        } else {
            panic!("unexpected governed result: {}", result.output);
        }
    }

    assert_eq!(permission_denies, 50);
    assert_eq!(budget_denies, 150);
    assert_eq!(guard.checks.load(Ordering::SeqCst), 150);
    assert_eq!(guard.remaining.load(Ordering::SeqCst), 0);
    assert!(session.pending_confirmations().await.is_empty());
    assert!(!workspace.path().join("budget-soak-leak.txt").exists());
    assert!(!workspace.path().join("budget-soak-bash.txt").exists());
}
