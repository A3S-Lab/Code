use super::tests::MockLlmClient;
use super::{AgentConfig, AgentLoop};
use crate::hitl::{ConfirmationManager, ConfirmationPolicy, ConfirmationProvider};
use crate::permissions::{PermissionChecker, PermissionDecision};
use crate::tools::{AgentDirScriptTool, Tool, ToolContext, ToolExecutor, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct SideEffectTool {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for SideEffectTool {
    fn name(&self) -> &str {
        "side_effect"
    }

    fn description(&self) -> &str {
        "Records a test-only side effect"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false})
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::success("side-effect-ok"))
    }
}

struct RecordingTargetPermission {
    decision: PermissionDecision,
    checks: Arc<AtomicUsize>,
}

impl PermissionChecker for RecordingTargetPermission {
    fn check(&self, tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
        if tool_name == "side_effect" {
            self.checks.fetch_add(1, Ordering::SeqCst);
            self.decision
        } else {
            PermissionDecision::Allow
        }
    }
}

fn agent_dir_script_agent(
    nested_decision: PermissionDecision,
    confirmation_manager: Option<Arc<dyn ConfirmationProvider>>,
) -> (
    AgentLoop,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    tempfile::TempDir,
) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("tools")).unwrap();
    std::fs::write(directory.path().join("instructions.md"), "test role").unwrap();
    std::fs::write(
        directory.path().join("tools/project-script.md"),
        "---\nkind: script\nname: project_script\npath: project-script.js\nallowed_tools: [side_effect]\n---\nCall the test side-effect tool.\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("project-script.js"),
        r#"async function run(ctx) {
               return await ctx.tool("side_effect", {});
           }"#,
    )
    .unwrap();

    let loaded = crate::config::AgentDir::load(directory.path()).unwrap();
    let crate::config::ToolSpec::Script(spec) = &loaded.tools[0] else {
        panic!("expected project script tool");
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let permission_checks = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(ToolExecutor::new(
        directory.path().to_string_lossy().to_string(),
    ));
    executor.register_dynamic_tool(Arc::new(SideEffectTool {
        calls: Arc::clone(&calls),
    }));
    executor.register_dynamic_tool(Arc::new(AgentDirScriptTool::new(
        spec.clone(),
        Arc::clone(executor.registry()),
    )));
    let responses = vec![
        MockLlmClient::tool_call_response(
            "project-script-1",
            "project_script",
            serde_json::json!({}),
        ),
        MockLlmClient::text_response("done"),
    ];
    let agent = AgentLoop::new(
        Arc::new(MockLlmClient::new(responses)),
        executor,
        ToolContext::new(directory.path().to_path_buf()),
        AgentConfig {
            permission_checker: Some(Arc::new(RecordingTargetPermission {
                decision: nested_decision,
                checks: Arc::clone(&permission_checks),
            })),
            confirmation_manager,
            ..Default::default()
        },
    );

    (agent, calls, permission_checks, directory)
}

#[tokio::test]
async fn agent_dir_script_nested_tool_obeys_permission_deny_without_side_effects() {
    let (agent, calls, permission_checks, _directory) =
        agent_dir_script_agent(PermissionDecision::Deny, None);

    agent
        .execute_with_session(
            &[],
            "run project script",
            Some("agent-dir-permission"),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the allow-list must not bypass the session permission denial"
    );
    assert!(
        permission_checks.load(Ordering::SeqCst) > 0,
        "the nested call must reach the session permission checker"
    );
}

#[tokio::test]
async fn agent_dir_script_nested_tool_waits_for_hitl_before_side_effects() {
    let (confirmation_events, _) = tokio::sync::broadcast::channel(8);
    let confirmations = Arc::new(ConfirmationManager::new(
        ConfirmationPolicy::enabled(),
        confirmation_events,
    ));
    let confirmation_provider = confirmations.clone() as Arc<dyn ConfirmationProvider>;
    let (agent, calls, permission_checks, _directory) =
        agent_dir_script_agent(PermissionDecision::Ask, Some(confirmation_provider));

    let run = agent.execute_with_session(
        &[],
        "run project script",
        Some("agent-dir-hitl"),
        None,
        None,
    );
    let reject_nested_call = async {
        let request = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let Some(request) = confirmations.pending_confirmations().await.first().cloned()
                {
                    return request;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the nested project tool call must request confirmation");
        assert_eq!(request.1, "side_effect");
        assert!(permission_checks.load(Ordering::SeqCst) > 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        confirmations
            .confirm(
                &request.0,
                false,
                Some("rejected nested project tool in test".to_string()),
            )
            .await
            .unwrap();
    };

    let (result, ()) = tokio::join!(run, reject_nested_call);
    result.unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "HITL rejection must happen before the nested side effect"
    );
}
