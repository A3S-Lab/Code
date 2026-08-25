use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct ParentVisibility {
    policy: PermissionPolicy,
    hide_use_from_primary: bool,
}

impl PermissionChecker for ParentVisibility {
    fn expose_to_model(&self, tool_name: &str) -> bool {
        !(self.hide_use_from_primary && tool_name.starts_with("mcp__use_"))
            && self.policy.expose_to_model(tool_name)
    }

    fn check(&self, tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        self.policy.check(tool_name, args)
    }
}

fn delegated(
    child_policy: PermissionPolicy,
    parent_policy: PermissionPolicy,
) -> DelegatedPermissionChecker {
    DelegatedPermissionChecker {
        child: Arc::new(child_policy.clone()),
        child_policy: Some(child_policy),
        parent: Arc::new(ParentVisibility {
            policy: parent_policy.clone(),
            hide_use_from_primary: true,
        }),
        parent_policy: Some(parent_policy),
    }
}

#[test]
fn explicitly_scoped_worker_can_see_parent_hidden_tool() {
    let mut child = PermissionPolicy::new().allow("mcp__use_*");
    child.default_decision = PermissionDecision::Deny;
    let parent = PermissionPolicy::new().allow("mcp__use_*");
    let checker = delegated(child, parent);

    assert!(checker.expose_to_model("mcp__use_browser__browser_snapshot"));
    assert_eq!(
        checker.check("mcp__use_browser__browser_snapshot", &serde_json::json!({})),
        PermissionDecision::Allow
    );
}

#[test]
fn unrelated_worker_does_not_inherit_parent_hidden_use_tools() {
    let child = PermissionPolicy::new().allow("read(*)");
    let parent = PermissionPolicy::new().allow("mcp__use_*");
    let checker = delegated(child, parent);

    assert!(!checker.expose_to_model("mcp__use_browser__browser_snapshot"));
}

#[test]
fn parent_deny_remains_authoritative_for_explicit_worker_capability() {
    let mut child = PermissionPolicy::new().allow("mcp__use_*");
    child.default_decision = PermissionDecision::Deny;
    let parent = PermissionPolicy::new().deny("mcp__use_ocr__ocr_extract");
    let checker = delegated(child, parent);

    assert!(!checker.expose_to_model("mcp__use_ocr__ocr_extract"));
    assert_eq!(
        checker.check("mcp__use_ocr__ocr_extract", &serde_json::json!({})),
        PermissionDecision::Deny
    );
}

struct RecordingConfirmationProvider {
    requires_confirmation: bool,
    approved: bool,
    requests: AtomicUsize,
    policy: tokio::sync::RwLock<ConfirmationPolicy>,
}

impl RecordingConfirmationProvider {
    fn new(requires_confirmation: bool, approved: bool, policy: ConfirmationPolicy) -> Self {
        Self {
            requires_confirmation,
            approved,
            requests: AtomicUsize::new(0),
            policy: tokio::sync::RwLock::new(policy),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl ConfirmationProvider for RecordingConfirmationProvider {
    async fn requires_confirmation(&self, _tool_name: &str) -> bool {
        self.requires_confirmation
    }

    async fn request_confirmation(
        &self,
        _tool_id: &str,
        _tool_name: &str,
        _args: &serde_json::Value,
    ) -> oneshot::Receiver<ConfirmationResponse> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        DelegatedConfirmationProvider::immediate_response(
            self.approved,
            (!self.approved).then(|| "recorded rejection".to_string()),
        )
    }

    async fn confirm(
        &self,
        _tool_id: &str,
        _approved: bool,
        _reason: Option<String>,
    ) -> Result<bool, String> {
        Ok(false)
    }

    async fn policy(&self) -> ConfirmationPolicy {
        self.policy.read().await.clone()
    }

    async fn set_policy(&self, policy: ConfirmationPolicy) {
        *self.policy.write().await = policy;
    }

    async fn check_timeouts(&self) -> usize {
        0
    }

    async fn cancel_all(&self) -> usize {
        0
    }
}

fn static_checker(decision: PermissionDecision) -> Arc<dyn PermissionChecker> {
    struct StaticChecker(PermissionDecision);

    impl PermissionChecker for StaticChecker {
        fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
            self.0
        }
    }

    Arc::new(StaticChecker(decision))
}

fn parent_context(
    decision: PermissionDecision,
    confirmation_manager: Option<Arc<dyn ConfirmationProvider>>,
) -> ChildRunContext {
    ChildRunContext {
        security_provider: None,
        hook_engine: None,
        skill_registry: None,
        permission_checker: Some(static_checker(decision)),
        permission_policy: None,
        tool_timeout_ms: None,
        llm_api_timeout_ms: None,
        max_parallel_tasks: None,
        max_execution_time_ms: None,
        circuit_breaker_threshold: None,
        duplicate_tool_call_threshold: None,
        confirmation_manager,
        enforce_active_skill_tool_restrictions: None,
        workspace_services: None,
        immutable_content_adapter: None,
        sandbox_handle: None,
        tool_presentation_profile: None,
        budget_guard: None,
    }
}

fn delegated_config(
    child_decision: PermissionDecision,
    child_confirmation: Option<Arc<dyn ConfirmationProvider>>,
    child_inheritance: Option<ConfirmationInheritance>,
    parent_decision: PermissionDecision,
    parent_confirmation: Option<Arc<dyn ConfirmationProvider>>,
) -> AgentConfig {
    let mut config = AgentConfig {
        permission_checker: Some(static_checker(child_decision)),
        confirmation_manager: child_confirmation,
        confirmation_inheritance: child_inheritance,
        ..AgentConfig::default()
    };
    parent_context(parent_decision, parent_confirmation).apply_to(&mut config);
    config
}

#[test]
fn delegated_run_keeps_its_prompt_context_isolated() {
    let ambient_child_provider: Arc<dyn crate::context::ContextProvider> = Arc::new(
        crate::context::StaticContextProvider::new("ambient-child-context"),
    );
    let parent = parent_context(PermissionDecision::Allow, None);
    let mut config = AgentConfig {
        context_providers: vec![ambient_child_provider],
        ..AgentConfig::default()
    };

    parent.apply_to(&mut config);

    assert_eq!(config.context_providers.len(), 1);
    assert_eq!(config.context_providers[0].name(), "ambient-child-context");
}

#[test]
fn delegated_child_inherits_the_parent_presentation_ceiling_exactly() {
    let parent_profile = crate::tools::ToolPresentationProfileV1::code();
    let mut context = parent_context(PermissionDecision::Allow, None);
    context.tool_presentation_profile = Some(parent_profile.clone());
    let mut child_config = AgentConfig {
        tool_presentation_profile: crate::tools::ToolPresentationProfileV1::direct(),
        ..AgentConfig::default()
    };

    context.apply_to(&mut child_config);

    assert_eq!(child_config.tool_presentation_profile, parent_profile);
    assert!(crate::tools::ToolPresentationProfileV1::direct()
        .ensure_within(&child_config.tool_presentation_profile)
        .is_err());
}

fn confirmation(config: &AgentConfig) -> Arc<dyn ConfirmationProvider> {
    config
        .confirmation_manager
        .as_ref()
        .expect("delegated confirmation provider")
        .clone()
}

fn recording_provider(
    requires_confirmation: bool,
    approved: bool,
    timeout_ms: u64,
) -> Arc<RecordingConfirmationProvider> {
    Arc::new(RecordingConfirmationProvider::new(
        requires_confirmation,
        approved,
        ConfirmationPolicy::enabled().with_timeout(timeout_ms, TimeoutAction::Reject),
    ))
}

#[tokio::test]
async fn child_allow_parent_ask_uses_only_parent_confirmation() {
    let child = recording_provider(false, true, 9_000);
    let parent = recording_provider(true, true, 1_000);
    let child_provider: Arc<dyn ConfirmationProvider> = child.clone();
    let parent_provider: Arc<dyn ConfirmationProvider> = parent.clone();
    let config = delegated_config(
        PermissionDecision::Allow,
        Some(child_provider),
        Some(ConfirmationInheritance::AutoApprove),
        PermissionDecision::Ask,
        Some(parent_provider),
    );
    let provider = confirmation(&config);
    let args = serde_json::json!({"command": "cargo test"});

    assert!(provider.confirmation_available_for("bash", &args).await);
    assert!(provider.requires_confirmation_for("bash", &args).await);
    assert!(
        provider
            .request_confirmation("tool-1", "bash", &args)
            .await
            .await
            .unwrap()
            .approved
    );
    assert_eq!(child.request_count(), 0);
    assert_eq!(parent.request_count(), 1);
}

#[tokio::test]
async fn child_ask_parent_allow_respects_child_auto_approve() {
    let child = recording_provider(false, true, 9_000);
    let parent = recording_provider(true, true, 1_000);
    let child_provider: Arc<dyn ConfirmationProvider> = child.clone();
    let parent_provider: Arc<dyn ConfirmationProvider> = parent.clone();
    let config = delegated_config(
        PermissionDecision::Ask,
        Some(child_provider),
        Some(ConfirmationInheritance::AutoApprove),
        PermissionDecision::Allow,
        Some(parent_provider),
    );
    let provider = confirmation(&config);
    let args = serde_json::json!({"file_path": "notes.txt"});

    assert!(provider.confirmation_available_for("write", &args).await);
    assert!(!provider.requires_confirmation_for("write", &args).await);
    assert_eq!(child.request_count(), 0);
    assert_eq!(parent.request_count(), 0);
}

#[tokio::test]
async fn child_deny_on_ask_fails_closed_without_parent_prompt() {
    let parent = recording_provider(true, true, 1_000);
    let parent_provider: Arc<dyn ConfirmationProvider> = parent.clone();
    let config = delegated_config(
        PermissionDecision::Ask,
        None,
        Some(ConfirmationInheritance::DenyOnAsk),
        PermissionDecision::Allow,
        Some(parent_provider),
    );
    let provider = confirmation(&config);
    let args = serde_json::json!({"file_path": "notes.txt"});

    assert!(!provider.confirmation_available_for("write", &args).await);
    assert_eq!(parent.request_count(), 0);
    assert!(matches!(
        crate::safety_gate::ToolSafetyGate::new(&config)
            .decide(crate::safety_gate::ToolGateInput {
                tool_name: "write",
                args: &args,
                pre_tool_denial: None,
                tool_requires_confirmation: false,
            })
            .await,
        crate::safety_gate::ToolGateDecision::Deny {
            reason: crate::safety_gate::ToolGateDenial::ConfirmationUnavailable,
            ..
        }
    ));
}

#[tokio::test]
async fn parent_ask_cannot_be_waived_by_child_auto_approve() {
    let child = recording_provider(false, true, 9_000);
    let child_provider: Arc<dyn ConfirmationProvider> = child.clone();
    let config = delegated_config(
        PermissionDecision::Allow,
        Some(child_provider),
        Some(ConfirmationInheritance::AutoApprove),
        PermissionDecision::Ask,
        None,
    );
    let provider = confirmation(&config);
    let args = serde_json::json!({"command": "cargo test"});

    assert!(!provider.confirmation_available_for("bash", &args).await);
    assert_eq!(child.request_count(), 0);
}

#[tokio::test]
async fn child_inherit_parent_routes_child_ask_to_parent_provider() {
    let parent = recording_provider(true, true, 1_000);
    let parent_provider: Arc<dyn ConfirmationProvider> = parent.clone();
    let config = delegated_config(
        PermissionDecision::Ask,
        None,
        Some(ConfirmationInheritance::InheritParent),
        PermissionDecision::Allow,
        Some(parent_provider),
    );
    let provider = confirmation(&config);
    let args = serde_json::json!({"file_path": "notes.txt"});

    assert!(provider.confirmation_available_for("write", &args).await);
    assert!(provider.requires_confirmation_for("write", &args).await);
    assert!(
        provider
            .request_confirmation("tool-1", "write", &args)
            .await
            .await
            .unwrap()
            .approved
    );
    assert_eq!(parent.request_count(), 1);
}

#[tokio::test]
async fn both_ask_scopes_are_enforced_and_same_provider_is_deduplicated() {
    let child = recording_provider(true, true, 2_000);
    let parent = recording_provider(true, true, 1_000);
    let child_provider: Arc<dyn ConfirmationProvider> = child.clone();
    let parent_provider: Arc<dyn ConfirmationProvider> = parent.clone();
    let config = delegated_config(
        PermissionDecision::Ask,
        Some(child_provider),
        Some(ConfirmationInheritance::AutoApprove),
        PermissionDecision::Ask,
        Some(parent_provider),
    );
    let provider = confirmation(&config);
    let args = serde_json::json!({"command": "cargo test"});

    assert!(
        provider
            .request_confirmation("tool-1", "bash", &args)
            .await
            .await
            .unwrap()
            .approved
    );
    assert_eq!(child.request_count(), 1);
    assert_eq!(parent.request_count(), 1);

    let shared = recording_provider(true, true, 1_000);
    let shared_child: Arc<dyn ConfirmationProvider> = shared.clone();
    let shared_parent = shared_child.clone();
    let config = delegated_config(
        PermissionDecision::Ask,
        Some(shared_child),
        Some(ConfirmationInheritance::InheritParent),
        PermissionDecision::Ask,
        Some(shared_parent),
    );
    let provider = confirmation(&config);
    assert!(
        provider
            .request_confirmation("tool-2", "bash", &args)
            .await
            .await
            .unwrap()
            .approved
    );
    assert_eq!(shared.request_count(), 1);
}

#[tokio::test]
async fn tool_owned_confirmation_uses_parent_authority_after_both_allow() {
    let child = recording_provider(false, true, 9_000);
    let parent = recording_provider(true, true, 321);
    let child_provider: Arc<dyn ConfirmationProvider> = child.clone();
    let parent_provider: Arc<dyn ConfirmationProvider> = parent.clone();
    let config = delegated_config(
        PermissionDecision::Allow,
        Some(child_provider),
        Some(ConfirmationInheritance::AutoApprove),
        PermissionDecision::Allow,
        Some(parent_provider),
    );
    let provider = confirmation(&config);
    let args = serde_json::json!({"repository": "example"});

    assert!(
        provider
            .requires_confirmation_for("mcp__issue__create", &args)
            .await
    );
    assert_eq!(
        provider
            .policy_for("mcp__issue__create", &args)
            .await
            .default_timeout_ms,
        321
    );
    assert!(
        provider
            .request_confirmation("tool-1", "mcp__issue__create", &args)
            .await
            .await
            .unwrap()
            .approved
    );
    assert_eq!(child.request_count(), 0);
    assert_eq!(parent.request_count(), 1);
}

#[tokio::test]
async fn pending_confirm_cancel_and_timeout_forward_to_both_scopes() {
    let (child_events, _) = tokio::sync::broadcast::channel(8);
    let (parent_events, _) = tokio::sync::broadcast::channel(8);
    let child = Arc::new(crate::hitl::ConfirmationManager::new(
        ConfirmationPolicy::enabled().with_timeout(1_000, TimeoutAction::Reject),
        child_events,
    ));
    let parent = Arc::new(crate::hitl::ConfirmationManager::new(
        ConfirmationPolicy::enabled().with_timeout(1_000, TimeoutAction::Reject),
        parent_events,
    ));
    let child_provider: Arc<dyn ConfirmationProvider> = child.clone();
    let parent_provider: Arc<dyn ConfirmationProvider> = parent.clone();
    let config = delegated_config(
        PermissionDecision::Ask,
        Some(child_provider),
        Some(ConfirmationInheritance::AutoApprove),
        PermissionDecision::Ask,
        Some(parent_provider),
    );
    let provider = confirmation(&config);
    let args = serde_json::json!({"command": "cargo test"});

    let response = provider
        .request_confirmation("tool-confirm", "bash", &args)
        .await;
    let pending = provider.pending_confirmations().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool_id, "tool-confirm");
    assert!(provider
        .confirm("tool-confirm", true, Some("approved".to_string()))
        .await
        .unwrap());
    assert!(response.await.unwrap().approved);
    assert!(provider.pending_confirmations().await.is_empty());

    let response = provider
        .request_confirmation("tool-cancel", "bash", &args)
        .await;
    assert_eq!(provider.cancel_all().await, 2);
    assert!(!response.await.unwrap().approved);
    assert!(provider.pending_confirmations().await.is_empty());

    child
        .set_policy(ConfirmationPolicy::enabled().with_timeout(1, TimeoutAction::Reject))
        .await;
    parent
        .set_policy(ConfirmationPolicy::enabled().with_timeout(1, TimeoutAction::Reject))
        .await;
    let response = provider
        .request_confirmation("tool-timeout", "bash", &args)
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(provider.check_timeouts().await, 2);
    assert!(!response.await.unwrap().approved);
    assert!(provider.pending_confirmations().await.is_empty());
}

#[tokio::test]
async fn delegated_targeted_settlement_leaves_other_confirmation_pending_in_both_scopes() {
    let (child_events, _) = tokio::sync::broadcast::channel(8);
    let (parent_events, _) = tokio::sync::broadcast::channel(8);
    let child = Arc::new(crate::hitl::ConfirmationManager::new(
        ConfirmationPolicy::enabled().with_timeout(1_000, TimeoutAction::Reject),
        child_events,
    ));
    let parent = Arc::new(crate::hitl::ConfirmationManager::new(
        ConfirmationPolicy::enabled().with_timeout(1_000, TimeoutAction::Reject),
        parent_events,
    ));
    let config = delegated_config(
        PermissionDecision::Ask,
        Some(child.clone()),
        Some(ConfirmationInheritance::AutoApprove),
        PermissionDecision::Ask,
        Some(parent.clone()),
    );
    let provider = confirmation(&config);
    let args = serde_json::json!({"command": "cargo test"});

    let cancelled = provider
        .request_confirmation("tool-cancelled", "bash", &args)
        .await;
    let untouched = provider
        .request_confirmation("tool-untouched", "bash", &args)
        .await;
    assert!(provider.cancel("tool-cancelled").await);
    assert!(!cancelled.await.unwrap().approved);
    assert_eq!(child.pending_count().await, 1);
    assert_eq!(parent.pending_count().await, 1);
    assert_eq!(
        provider.pending_confirmations().await[0].tool_id,
        "tool-untouched"
    );
    assert!(provider
        .confirm("tool-untouched", true, None)
        .await
        .unwrap());
    assert!(untouched.await.unwrap().approved);

    let expired = provider
        .request_confirmation("tool-expired", "bash", &args)
        .await;
    let untouched = provider
        .request_confirmation("tool-still-pending", "bash", &args)
        .await;
    assert!(provider.expire("tool-expired", TimeoutAction::Reject).await);
    assert!(!expired.await.unwrap().approved);
    assert_eq!(child.pending_count().await, 1);
    assert_eq!(parent.pending_count().await, 1);
    assert_eq!(
        provider.pending_confirmations().await[0].tool_id,
        "tool-still-pending"
    );
    assert!(provider
        .confirm("tool-still-pending", true, None)
        .await
        .unwrap());
    assert!(untouched.await.unwrap().approved);
}
