use super::*;
use crate::agent::AgentConfig;
use crate::hitl::{ConfirmationManager, ConfirmationPolicy, ConfirmationProvider};
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use crate::permissions::{PermissionChecker, PermissionDecision};
use crate::tools::{Tool, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

struct UnusedLlm;

#[async_trait]
impl LlmClient for UnusedLlm {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("governed direct-tool tests must not invoke the model")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("governed direct-tool tests must not invoke the model")
    }
}

struct StaticPermission(PermissionDecision);

impl PermissionChecker for StaticPermission {
    fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
        self.0
    }
}

struct CountingTool {
    calls: Arc<AtomicUsize>,
    confirmation_required: bool,
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &str {
        "governed_counting"
    }

    fn description(&self) -> &str {
        "records one governed side effect"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }

    fn requires_confirmation(&self, _args: &serde_json::Value) -> bool {
        self.confirmation_required
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::success("counted"))
    }
}

fn runtime(
    decision: PermissionDecision,
    confirmation_required: bool,
    manager: Option<Arc<ConfirmationManager>>,
) -> (DirectToolRuntime, Arc<AtomicUsize>, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(ToolExecutor::new(
        directory.path().to_string_lossy().to_string(),
    ));
    executor.register_dynamic_tool(Arc::new(CountingTool {
        calls: Arc::clone(&calls),
        confirmation_required,
    }));
    let confirmation_manager = manager.map(|manager| manager as Arc<dyn ConfirmationProvider>);
    let session_id = "governed-host-tool-test".to_string();
    let context = ToolContext::new(directory.path().to_path_buf()).with_session_id(&session_id);
    let agent_loop = AgentLoop::new(
        Arc::new(UnusedLlm),
        Arc::clone(&executor),
        context.clone(),
        AgentConfig {
            permission_checker: Some(Arc::new(StaticPermission(decision))),
            confirmation_manager,
            ..AgentConfig::default()
        },
    );
    (
        DirectToolRuntime {
            tool_executor: executor,
            tool_context: context,
            agent_loop,
            session_id,
            session_cancel: CancellationToken::new(),
            closed: Arc::new(AtomicBool::new(false)),
            security_provider: None,
        },
        calls,
        directory,
    )
}

async fn resolve_confirmation(manager: &ConfirmationManager, calls: &AtomicUsize, approved: bool) {
    let request = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let Some(request) = manager
                .pending_confirmation_details()
                .await
                .first()
                .cloned()
            {
                return request;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("governed tool must request confirmation");
    assert_eq!(request.tool_name, "governed_counting");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(manager
        .confirm(
            &request.tool_id,
            approved,
            Some(if approved {
                "approved by test".to_string()
            } else {
                "rejected by test".to_string()
            }),
        )
        .await
        .unwrap());
}

#[tokio::test]
async fn trusted_host_call_preserves_explicit_control_plane_authority() {
    let (runtime, calls, _directory) = runtime(PermissionDecision::Deny, true, None);

    let result = runtime
        .call("governed_counting", serde_json::json!({}))
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn governed_host_call_obeys_permission_before_side_effects() {
    let (runtime, calls, _directory) = runtime(PermissionDecision::Deny, false, None);

    let result = runtime
        .call_governed("governed_counting", serde_json::json!({}))
        .await
        .unwrap();

    assert_ne!(result.exit_code, 0);
    assert!(result.output.contains("Permission denied"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn governed_host_call_requires_approval_before_side_effects() {
    let (event_tx, _) = broadcast::channel(8);
    let manager = Arc::new(ConfirmationManager::new(
        ConfirmationPolicy::enabled(),
        event_tx,
    ));
    let (runtime, calls, _directory) =
        runtime(PermissionDecision::Allow, true, Some(Arc::clone(&manager)));

    let (rejected, ()) = tokio::join!(
        runtime.call_governed("governed_counting", serde_json::json!({})),
        resolve_confirmation(&manager, &calls, false),
    );
    let rejected = rejected.unwrap();
    assert_ne!(rejected.exit_code, 0);
    assert!(rejected.output.contains("REJECTED"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let (approved, ()) = tokio::join!(
        runtime.call_governed("governed_counting", serde_json::json!({})),
        resolve_confirmation(&manager, &calls, true),
    );
    let approved = approved.unwrap();
    assert_eq!(approved.exit_code, 0, "{}", approved.output);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
