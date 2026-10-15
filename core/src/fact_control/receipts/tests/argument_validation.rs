#![cfg(test)]

use super::*;

struct StrictReceiptProbe(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl crate::tools::Tool for StrictReceiptProbe {
    fn name(&self) -> &str {
        "receipt_probe"
    }

    fn description(&self) -> &str {
        "Schema-admitted receipt probe"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type":"object", "properties":{"write":{"type":"boolean"}},
            "required":["write"], "additionalProperties":false,
        })
    }

    async fn execute(
        &self,
        args: &serde_json::Value,
        context: &crate::tools::ToolContext,
    ) -> anyhow::Result<crate::tools::ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if args["write"] == true {
            std::fs::write(
                context.workspace.join("changed.txt"),
                b"schema-admitted bytes",
            )?;
        }
        Ok(crate::tools::ToolOutput::success(
            "schema-admitted execution",
        ))
    }
}

#[derive(Debug)]
struct SchemaInputHook {
    calls: Arc<AtomicUsize>,
    replacement: Option<serde_json::Value>,
    effect_path: PathBuf,
}

#[async_trait::async_trait]
impl crate::hooks::HookExecutor for SchemaInputHook {
    async fn fire(&self, event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        if matches!(event, crate::hooks::HookEvent::PreToolUse(_)) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::fs::write(&self.effect_path, b"actual input-hook effect").unwrap();
            if let Some(replacement) = &self.replacement {
                return crate::hooks::HookResult::continue_with(
                    json!({"updatedInput":replacement}),
                );
            }
        }
        crate::hooks::HookResult::Continue(None)
    }
}

pub(super) fn schema_tools(
    root: &Path,
    replacement: Option<serde_json::Value>,
) -> (Arc<ExecutorTools>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let (base, executions) = executor_tools(root);
    base.executor
        .register_dynamic_tool(Arc::new(StrictReceiptProbe(Arc::clone(&executions))));
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut tools = base.as_ref().clone();
    tools.agent = Some(crate::agent::AgentLoop::new(
        Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
        Arc::clone(&tools.executor),
        tools.context.clone(),
        crate::agent::AgentConfig {
            hook_engine: Some(Arc::new(SchemaInputHook {
                calls: Arc::clone(&hook_calls),
                replacement,
                effect_path: root.join("hook-effect.txt"),
            })),
            ..Default::default()
        },
    ));
    (Arc::new(tools), executions, hook_calls)
}

#[tokio::test]
async fn invalid_original_arguments_fail_before_hook_or_tool_execution() {
    let root = tempfile::tempdir().unwrap();
    let (tools, executions, hook_calls) = schema_tools(root.path(), Some(json!({"write":true})));
    let invalid = json!({"write":true,"unexpected":true});
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request("invalid-original", invalid.clone()),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let identity = prepared.prepared.clone();
    let digest = a3s_effect::tool_args_digest(&invalid).unwrap();
    assert_eq!(identity.original_args_digest, digest);
    assert_eq!(identity.effective_args_digest, digest);
    assert!(identity.baseline.is_none());
    let result = execute_prepared(tools, prepared).await;
    result.ensure_prepared(&identity).unwrap();
    assert_eq!(result.outcome, ToolExecutionOutcome::Failed);
    assert_eq!(result.exit_code, None);
    assert!(result
        .output
        .as_str()
        .unwrap()
        .contains("Invalid arguments for tool 'receipt_probe'"));
    assert_eq!(
        result.effects.completeness,
        ToolObservationCompleteness::Complete
    );
    assert!(result.effects.changes.is_empty());
    assert!(result.effects.unresolved_external_operations.is_empty());
    assert_eq!(hook_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("hook-effect.txt").exists());
    assert!(!root.path().join("changed.txt").exists());
}

#[tokio::test]
async fn permission_denial_with_invalid_arguments_remains_denied_without_execution() {
    let root = tempfile::tempdir().unwrap();
    let (base, executions, hook_calls) = schema_tools(root.path(), Some(json!({"write":true})));
    let (events, mut received) = tokio::sync::mpsc::channel(8);
    let mut tools = base.as_ref().clone();
    tools.permission = PermissionPolicy::new().deny("receipt_probe");
    tools.events = Some(events);
    let tools = Arc::new(tools);
    let invalid = json!({"write":"invalid boolean"});
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request("denied-invalid-original", invalid.clone()),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let identity = prepared.prepared.clone();
    let digest = a3s_effect::tool_args_digest(&invalid).unwrap();
    assert_eq!(identity.original_args_digest, digest);
    assert_eq!(identity.effective_args_digest, digest);
    let result = execute_prepared(tools, prepared).await;
    result.ensure_prepared(&identity).unwrap();
    assert_eq!(result.outcome, ToolExecutionOutcome::Denied);
    assert_eq!(result.exit_code, None);
    assert!(result
        .output
        .as_str()
        .unwrap()
        .contains("tool_effect_effective_arguments_unapproved"));
    assert_eq!(
        result.effects.completeness,
        ToolObservationCompleteness::Complete
    );
    assert!(result.effects.changes.is_empty());
    assert!(result.effects.unresolved_external_operations.is_empty());
    assert!(
        matches!(received.try_recv().unwrap(), crate::agent::AgentEvent::PermissionDenied {
        tool_id, tool_name, args, ..
    } if tool_id == "denied-invalid-original" && tool_name == "receipt_probe" && args == invalid)
    );
    assert!(received.try_recv().is_err());
    assert_eq!(hook_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("hook-effect.txt").exists());
    assert!(!root.path().join("changed.txt").exists());
}

#[tokio::test]
async fn invalid_rewritten_arguments_preserve_both_inputs_and_hook_uncertainty() {
    let root = tempfile::tempdir().unwrap();
    let invalid = json!({"write":"invalid boolean"});
    let (tools, executions, hook_calls) = schema_tools(root.path(), Some(invalid.clone()));
    let original = json!({"write":true});
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request("invalid-rewrite", original.clone()),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let identity = prepared.prepared.clone();
    assert_eq!(
        identity.original_args_digest,
        a3s_effect::tool_args_digest(&original).unwrap()
    );
    assert_eq!(
        identity.effective_args_digest,
        a3s_effect::tool_args_digest(&invalid).unwrap()
    );
    assert_ne!(
        identity.original_args_digest,
        identity.effective_args_digest
    );
    assert!(identity.baseline.is_none());
    let result = execute_prepared(Arc::clone(&tools), prepared).await;
    result.ensure_prepared(&identity).unwrap();
    assert_eq!(result.outcome, ToolExecutionOutcome::Failed);
    assert_eq!(result.exit_code, None);
    assert_eq!(
        result.effects.completeness,
        ToolObservationCompleteness::Incomplete
    );
    assert!(result.effects.changes.is_empty());
    assert_eq!(
        result.effects.unresolved_external_operations,
        [identity.identity.key().unwrap()]
    );
    assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(root.path().join("hook-effect.txt")).unwrap(),
        b"actual input-hook effect"
    );
    assert!(!root.path().join("changed.txt").exists());

    let debt = fold_debt(&[prepared_fact(&identity), result_fact(&result)]);
    assert!(debt.unresolved.contains(&identity.identity.key().unwrap()));
    let mut ledger = crate::harness_loop::MutationLedger::default();
    ledger.set_unresolved_tool_effects(debt.unresolved);
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger, &[], &[], false),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
}

#[tokio::test]
async fn valid_unchanged_arguments_still_execute_once() {
    let root = tempfile::tempdir().unwrap();
    let (tools, executions, hook_calls) = schema_tools(root.path(), None);
    let original = json!({"write":true});
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request("valid-unchanged", original.clone()),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let identity = prepared.prepared.clone();
    assert_eq!(
        identity.original_args_digest,
        a3s_effect::tool_args_digest(&original).unwrap()
    );
    assert_eq!(
        identity.original_args_digest,
        identity.effective_args_digest
    );
    let result = execute_prepared(tools, prepared).await;
    result.ensure_prepared(&identity).unwrap();
    assert_eq!(result.outcome, ToolExecutionOutcome::Succeeded);
    assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(root.path().join("changed.txt")).unwrap(),
        b"schema-admitted bytes"
    );
}
