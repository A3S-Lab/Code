#![cfg(test)]

use super::*;

struct AvailabilityProbe(Arc<AtomicUsize>);

#[derive(Debug)]
struct DisablingHook {
    unavailable: Arc<AtomicBool>,
    path: PathBuf,
}

#[async_trait::async_trait]
impl crate::hooks::HookExecutor for DisablingHook {
    async fn fire(&self, event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        if matches!(event, crate::hooks::HookEvent::PreToolUse(_)) {
            std::fs::write(&self.path, b"hook changed backend availability").unwrap();
            self.unavailable.store(true, Ordering::SeqCst);
        }
        crate::hooks::HookResult::Continue(None)
    }
}

struct StatefulAvailabilityProbe {
    unavailable: Arc<AtomicBool>,
    executions: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl crate::tools::Tool for StatefulAvailabilityProbe {
    fn name(&self) -> &str {
        "receipt_probe"
    }
    fn description(&self) -> &str {
        "Backend availability can change during a hook"
    }
    fn parameters(&self) -> serde_json::Value {
        json!({"type":"object"})
    }
    fn validate_execution_available(
        &self,
        _: &serde_json::Value,
        _: &crate::tools::ToolContext,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.unavailable.load(Ordering::SeqCst),
            "backend is unavailable"
        );
        Ok(())
    }
    async fn execute(
        &self,
        _: &serde_json::Value,
        _: &crate::tools::ToolContext,
    ) -> anyhow::Result<crate::tools::ToolOutput> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(crate::tools::ToolOutput::success("executed"))
    }
}

#[async_trait::async_trait]
impl crate::tools::Tool for AvailabilityProbe {
    fn name(&self) -> &str {
        "receipt_probe"
    }

    fn description(&self) -> &str {
        "Probe a side-effect-free execution refusal"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({"type":"object"})
    }

    fn validate_execution_available(
        &self,
        args: &serde_json::Value,
        _ctx: &crate::tools::ToolContext,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(args["unavailable"] != true, "backend is unavailable");
        Ok(())
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &crate::tools::ToolContext,
    ) -> anyhow::Result<crate::tools::ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        // The identical diagnostic after dispatch does NOT prove zero effects.
        anyhow::bail!("backend is unavailable")
    }
}

#[tokio::test]
async fn unavailable_backend_refuses_before_input_hooks_without_creating_debt() {
    let root = tempfile::tempdir().unwrap();
    let (tools, executions, hook_calls) =
        super::argument_validation::schema_tools(root.path(), None);
    tools
        .executor
        .register_dynamic_tool(Arc::new(AvailabilityProbe(Arc::clone(&executions))));
    for turn in 1..=2 {
        let mut call = request(&format!("unavailable-{turn}"), json!({"unavailable":true}));
        call.identity.turn = turn;
        let prepared = prepare_tools(Arc::clone(&tools), call, Default::default())
            .await
            .unwrap();
        let identity = prepared.prepared.clone();
        assert!(identity.baseline.is_none());
        let result = execute_prepared(Arc::clone(&tools), prepared).await;
        result.ensure_prepared(&identity).unwrap();
        assert_eq!(result.outcome, ToolExecutionOutcome::Denied);
        assert_eq!(result.exit_code, None);
        assert_eq!(
            result.effects,
            ToolEffectObservationV1::complete(Vec::new())
        );
        let debt = fold_debt(&[prepared_fact(&identity), result_fact(&result)]);
        assert!(debt.unresolved.is_empty());
        let ledger = tools.ledger.as_ref().unwrap();
        ledger
            .lock()
            .unwrap()
            .observe_tool_receipt("receipt_probe", &result);
        assert!(!ledger.lock().unwrap().has_uncertain_tool_invocations());
    }
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(hook_calls.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("hook-effect.txt").exists());
}

#[tokio::test]
async fn rewritten_unavailable_invocation_retains_the_input_hooks_uncertainty() {
    let root = tempfile::tempdir().unwrap();
    let (tools, executions, hook_calls) =
        super::argument_validation::schema_tools(root.path(), Some(json!({"unavailable":true})));
    tools
        .executor
        .register_dynamic_tool(Arc::new(AvailabilityProbe(Arc::clone(&executions))));
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request("rewritten-unavailable", json!({})),
        Default::default(),
    )
    .await
    .unwrap();
    let identity = prepared.prepared.clone();
    let result = execute_prepared(Arc::clone(&tools), prepared).await;
    assert_eq!(result.outcome, ToolExecutionOutcome::Denied);
    assert_eq!(
        result.effects.completeness,
        ToolObservationCompleteness::Incomplete
    );
    assert_eq!(
        result.effects.unresolved_external_operations,
        [identity.identity.key().unwrap()]
    );
    assert!(
        !fold_debt(&[prepared_fact(&identity), result_fact(&result)])
            .unresolved
            .is_empty()
    );
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(root.path().join("hook-effect.txt")).unwrap(),
        b"actual input-hook effect"
    );
}

#[tokio::test]
async fn identical_error_after_dispatch_remains_unknown() {
    let root = tempfile::tempdir().unwrap();
    let (tools, executions) = executor_tools(root.path());
    tools
        .executor
        .register_dynamic_tool(Arc::new(AvailabilityProbe(Arc::clone(&executions))));
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request("dispatched-error", json!({})),
        Default::default(),
    )
    .await
    .unwrap();
    let identity = prepared.prepared.clone();
    let result = execute_prepared(tools, prepared).await;
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(result.outcome, ToolExecutionOutcome::Unknown);
    assert!(
        !fold_debt(&[prepared_fact(&identity), result_fact(&result)])
            .unresolved
            .is_empty()
    );
}

#[tokio::test]
async fn hook_without_argument_rewrite_still_rechecks_backend_availability() {
    let root = tempfile::tempdir().unwrap();
    let (base, executions) = executor_tools(root.path());
    let unavailable = Arc::new(AtomicBool::new(false));
    base.executor
        .register_dynamic_tool(Arc::new(StatefulAvailabilityProbe {
            unavailable: Arc::clone(&unavailable),
            executions: Arc::clone(&executions),
        }));
    let mut tools = base.as_ref().clone();
    tools.agent = Some(crate::agent::AgentLoop::new(
        Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
        Arc::clone(&tools.executor),
        tools.context.clone(),
        crate::agent::AgentConfig {
            hook_engine: Some(Arc::new(DisablingHook {
                unavailable,
                path: root.path().join("hook-effect.txt"),
            })),
            ..Default::default()
        },
    ));
    let tools = Arc::new(tools);
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request("hook-disables-backend", json!({})),
        Default::default(),
    )
    .await
    .unwrap();
    let identity = prepared.prepared.clone();
    assert_eq!(
        identity.original_args_digest,
        identity.effective_args_digest
    );
    let result = execute_prepared(tools, prepared).await;
    assert_eq!(result.outcome, ToolExecutionOutcome::Denied);
    assert_eq!(
        result.effects.completeness,
        ToolObservationCompleteness::Incomplete
    );
    assert!(
        !fold_debt(&[prepared_fact(&identity), result_fact(&result)])
            .unresolved
            .is_empty()
    );
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(root.path().join("hook-effect.txt")).unwrap(),
        b"hook changed backend availability"
    );
}

#[tokio::test]
#[cfg(target_os = "macos")]
async fn native_refusal_in_an_unobservable_workspace_does_not_poison_later_turns() {
    assert_unavailable_sandbox_does_not_open_debt(false).await;
}

#[tokio::test]
async fn initialization_refusal_in_an_unobservable_workspace_does_not_poison_later_turns() {
    assert_unavailable_sandbox_does_not_open_debt(true).await;
}

async fn assert_unavailable_sandbox_does_not_open_debt(initialization_failure: bool) {
    let root = tempfile::tempdir().unwrap();
    let oversized = std::fs::File::create(root.path().join("oversized.bin")).unwrap();
    oversized
        .set_len(crate::porcelain::OBSERVATION_BYTE_CAP + 1)
        .unwrap();
    let (base, _) = executor_tools(root.path());
    let mut tools = base.as_ref().clone();
    let refusal = if initialization_failure {
        "fixture native initialization refused"
    } else {
        "native_process_lifetime_unqualified"
    };
    tools.context.sandbox = Some(if initialization_failure {
        crate::agent_api::select_default_local_sandbox(
            root.path(),
            crate::sandbox::SandboxNetworkMode::Mediated,
            true,
            Err(anyhow::anyhow!("fixture native initialization refused")),
        )
    } else {
        Arc::new(crate::sandbox::native::NativeBashSandbox::new(root.path()).unwrap())
    });
    let tools = Arc::new(tools);
    let mut facts = Vec::new();
    for turn in 1..=4 {
        let tool = if turn % 2 == 1 { "bash" } else { "background" };
        let mut call = request(
            &format!("native-unavailable-{turn}"),
            json!({
                "command":"printf must-not-run > invocation-sentinel"
            }),
        );
        if tool == "background" {
            call.call.args["action"] = json!("start");
            call.call.args["name"] = json!("must-not-run");
        }
        call.call.name = tool.into();
        call.identity.turn = turn;
        let prepared = prepare_tools(Arc::clone(&tools), call, Default::default())
            .await
            .unwrap();
        let identity = prepared.prepared.clone();
        assert!(identity.baseline.is_none());
        let result = execute_prepared(Arc::clone(&tools), prepared).await;
        result.ensure_prepared(&identity).unwrap();
        assert_eq!(result.outcome, ToolExecutionOutcome::Denied);
        assert!(result.output.as_str().unwrap().contains(refusal));
        assert_eq!(
            result.effects,
            ToolEffectObservationV1::complete(Vec::new())
        );
        facts.extend([prepared_fact(&identity), result_fact(&result)]);
        let debt = fold_debt(&facts);
        assert!(debt.unresolved.is_empty());
        let mut ledger = tools.ledger.as_ref().unwrap().lock().unwrap();
        ledger.observe_tool_receipt(tool, &result);
        ledger.set_unresolved_tool_effects(debt.unresolved);
        assert!(!ledger.has_uncertain_tool_invocations());
    }
    assert!(!root.path().join("invocation-sentinel").exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn unconfirmed_ask_keeps_a_pre_spawn_refusal() {
    let root = tempfile::tempdir().unwrap();
    let (base, executions) = executor_tools(root.path());
    base.executor
        .register_dynamic_tool(Arc::new(AvailabilityProbe(Arc::clone(&executions))));
    let (events, _) = tokio::sync::broadcast::channel(1);
    let manager = Arc::new(crate::hitl::ConfirmationManager::new(
        crate::hitl::ConfirmationPolicy::enabled(),
        events,
    ));
    let mut tools = base.as_ref().clone();
    tools.permission = PermissionPolicy::new();
    tools.agent = Some(crate::agent::AgentLoop::new(
        Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
        Arc::clone(&tools.executor),
        tools.context.clone(),
        crate::agent::AgentConfig {
            confirmation_manager: Some(manager),
            ..Default::default()
        },
    ));
    let tools = Arc::new(tools);

    let refused = prepare_tools(
        Arc::clone(&tools),
        request("unavailable-ask", json!({"unavailable": true})),
        Default::default(),
    )
    .await
    .unwrap();
    let refused = execute_prepared(Arc::clone(&tools), refused).await;
    let refused_output = refused.output.as_str().unwrap();
    assert_eq!(refused.outcome, ToolExecutionOutcome::Denied);
    assert!(
        refused_output.contains("backend is unavailable"),
        "{refused_output}"
    );
    assert!(!refused_output.contains("unapproved"), "{refused_output}");
    assert_eq!(executions.load(Ordering::SeqCst), 0);

    let admitted = prepare_tools(
        Arc::clone(&tools),
        request("available-ask", json!({})),
        Default::default(),
    )
    .await
    .unwrap();
    let admitted = execute_prepared(tools, admitted).await;
    let admitted_output = admitted.output.as_str().unwrap();
    assert_eq!(admitted.outcome, ToolExecutionOutcome::Denied);
    assert!(
        admitted_output.contains("tool_effect_effective_arguments_unapproved"),
        "{admitted_output}"
    );
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}
