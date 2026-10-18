#![cfg(test)]

use super::*;
use a3s_effect::{
    Fact, ToolFileIdentityV1, ToolFileKind, ToolInvocationIdentityV1, ToolPathChangeV1,
};
use serde_json::json;

mod argument_validation;
mod cache_proof;
mod cache_provenance;
mod execution_availability;
mod pre_spawn;
mod tool_budget;
mod tool_budget_cancel;
mod workspace_boundary;

fn preparation(id: &str) -> ToolPreparedV1 {
    let digest = a3s_effect::tool_args_digest(&json!({"command":"write"})).unwrap();
    ToolPreparedV1 {
        schema: TOOL_PREPARED_SCHEMA.into(),
        identity: ToolInvocationIdentityV1 {
            turn: 1,
            cycle: 0,
            tool_call_id: id.into(),
            batch_index: 0,
            admission: None,
        },
        tool_name: "bash".into(),
        original_args_digest: digest.clone(),
        effective_args_digest: digest,
        recovery: ToolRecoveryPolicy::NeverReplay,
        baseline: None,
    }
}

fn fact(kind: &str, key: &str, payload: serde_json::Value) -> Fact {
    Fact {
        seq: 1,
        kind: kind.into(),
        key: key.into(),
        cause: None,
        payload,
    }
}

fn prepared_fact(prepared: &ToolPreparedV1) -> Fact {
    fact("tool.prepared", "prepared", json!({"prepared":prepared}))
}

fn result(prepared: &ToolPreparedV1, outcome: ToolExecutionOutcome) -> ToolExecutionResultV1 {
    ToolExecutionResultV1 {
        schema: TOOL_RESULT_SCHEMA.into(),
        identity: prepared.identity.clone(),
        prepared_digest: prepared.digest().unwrap(),
        outcome,
        exit_code: Some(7),
        output: json!("failed after writing"),
        effects: ToolEffectObservationV1::complete(vec![ToolPathChangeV1 {
            path: "changed.txt".into(),
            before: None,
            after: ToolFileIdentityV1 {
                kind: ToolFileKind::File,
                content_digest: Some(format!("sha256:{}", sha256::digest("actual bytes"))),
                mode: Some(0o644),
            },
        }]),
    }
}

fn result_fact(result: &ToolExecutionResultV1) -> Fact {
    fact(
        "tool.result",
        "result",
        json!({"receipt":result,"toolCallId":result.identity.tool_call_id,"ok":false,"output":"failed after writing"}),
    )
}

#[test]
fn failed_and_cancelled_writes_restore_actual_byte_identity_once() {
    for outcome in [
        ToolExecutionOutcome::Failed,
        ToolExecutionOutcome::Cancelled,
    ] {
        let prepared = preparation("write");
        let result = result(&prepared, outcome);
        let facts = vec![prepared_fact(&prepared), result_fact(&result)];
        let debt = fold_debt(&facts);
        assert_eq!(debt.paths, ["changed.txt"]);
        assert!(debt.unresolved.is_empty());
        let mut ledger = crate::harness_loop::MutationLedger::default();
        ledger.observe_tool_receipt("bash", &result);
        let digest = ledger.digest().to_string();
        ledger.observe_tool_receipt("bash", &result);
        assert_eq!(ledger.digest(), digest);
        assert!(ledger.content_digest_matches("changed.txt", &sha256::digest("actual bytes")));
    }
}

#[test]
fn preparation_and_unknown_cannot_be_forgiven_by_a_turn_marker_or_scan() {
    let prepared = preparation("uncertain");
    let marker = fact(
        TURN_COMPLETE_FACT_KIND,
        "marker",
        json!({"receipt_keys":[prepared.identity.key().unwrap()]}),
    );
    for terminal in [
        None,
        Some(ToolExecutionResultV1::unknown(&prepared, "process died").unwrap()),
    ] {
        let mut facts = vec![prepared_fact(&prepared)];
        if let Some(result) = terminal {
            facts.push(result_fact(&result));
        }
        facts.push(marker.clone());
        facts.push(fact(
            "compaction.done",
            "compact",
            json!({"summary":"done","kept":[]}),
        ));
        let debt = fold_debt(&facts);
        assert!(!debt.unresolved.is_empty());
        let mut ledger = crate::harness_loop::MutationLedger::default();
        ledger.set_unresolved_tool_effects(debt.unresolved);
        ledger.set_observation_incomplete(false);
        assert!(ledger.has_uncertain_tool_invocations());
        assert!(
            matches!(crate::harness_loop::decide_completion(&ledger, &[], &[], false),
            crate::harness_loop::CompletionGate::Incomplete { message } if message.contains("tool_effect_unresolved"))
        );
    }
}

#[test]
fn a_marker_only_covers_the_receipts_it_explicitly_names() {
    let prepared = preparation("write");
    let terminal = result(&prepared, ToolExecutionOutcome::Failed);
    let mut facts = vec![
        prepared_fact(&prepared),
        result_fact(&terminal),
        fact(
            TURN_COMPLETE_FACT_KIND,
            "unrelated",
            json!({"receipt_keys":[]}),
        ),
    ];
    assert_eq!(fold_debt(&facts).paths, ["changed.txt"]);
    facts.push(fact(
        TURN_COMPLETE_FACT_KIND,
        "covered",
        json!({"receipt_keys":[prepared.identity.key().unwrap()]}),
    ));
    let debt = fold_debt(&facts);
    assert!(debt.paths.is_empty());
    assert!(debt.unresolved.is_empty());
}

#[test]
fn malformed_cross_call_and_missing_preparation_results_fail_closed() {
    let prepared = preparation("one");
    let mut terminal = result(&prepared, ToolExecutionOutcome::Succeeded);
    terminal.identity.tool_call_id = "other".into();
    for facts in [
        vec![prepared_fact(&prepared), result_fact(&terminal)],
        vec![result_fact(&result(
            &prepared,
            ToolExecutionOutcome::Succeeded,
        ))],
        vec![
            prepared_fact(&prepared),
            fact("tool.result", "bad", json!({"receipt":null})),
        ],
    ] {
        assert!(!fold_debt(&facts).unresolved.is_empty());
    }
}

#[test]
fn incomplete_paths_and_external_unknown_are_distinct_from_a_local_scan() {
    let prepared = preparation("external");
    let mut terminal = result(&prepared, ToolExecutionOutcome::Failed);
    terminal.effects.completeness = ToolObservationCompleteness::Incomplete;
    terminal.effects.reason = Some("unreadable suffix".into());
    terminal
        .effects
        .unresolved_external_operations
        .push("provider-operation-opaque-id".into());
    let debt = fold_debt(&[prepared_fact(&prepared), result_fact(&terminal)]);
    assert_eq!(debt.paths, ["changed.txt"]);
    assert!(debt.unresolved.contains("provider-operation-opaque-id"));
    assert!(debt.unresolved.contains(&prepared.identity.key().unwrap()));
}

#[test]
fn durable_background_terminal_settles_only_its_exact_child() {
    let facts = background_debt_fixture(false);
    let debt = fold_debt(&facts);
    assert!(!debt.unresolved.contains("workspace-child:one"));
    assert!(debt.unresolved.contains("workspace-child:two"));
    assert_eq!(debt.paths, ["background.txt"]);
    assert_eq!(debt.background_results.len(), 1);
}

fn background_debt_fixture(cancelled: bool) -> Vec<Fact> {
    let mut prepared = preparation("background");
    prepared.identity.admission = Some(ToolAdmissionIdentityV1 {
        run_id: "source-one".into(),
        thread_id: "thread-one".into(),
        capability_binding_digest: a3s_effect::tool_args_digest(&json!({"binding":1})).unwrap(),
    });
    let mut terminal = result_without_execution(
        &prepared,
        ToolExecutionOutcome::Succeeded,
        "accepted".into(),
    )
    .unwrap();
    terminal
        .effects
        .unresolved_external_operations
        .extend(["workspace-child:one".into(), "workspace-child:two".into()]);
    terminal.output = runner_result_value(
        "accepted",
        false,
        &[],
        0,
        Some(&json!({
            "workspace_child":"one", "source_run":"source-one", "run":"effect-one"
        })),
    );
    let effects = ToolEffectObservationV1::complete(vec![ToolPathChangeV1 {
        path: "background.txt".into(),
        before: None,
        after: ToolFileIdentityV1 {
            kind: ToolFileKind::File,
            content_digest: Some(format!("sha256:{}", sha256::digest(b"background bytes"))),
            mode: Some(0o644),
        },
    }]);
    vec![
        prepared_fact(&prepared),
        result_fact(&terminal),
        fact(
            "background.output",
            "bg:one:2",
            json!({
                "event":"exited","task_id":"one","workspace_child":"one","changed_paths":["background.txt"],
                "observation_incomplete":false,"output_complete":!cancelled,
                "exit_code":if cancelled { None } else { Some(7) },
                "cancelled":cancelled,"timed_out":false,
                "disposition":if cancelled { "cancelled" } else { "completed" },
                "process_settled":true,"execution_future_released":true,"output_facts_retained":true,
                "process_settlement_basis":if cancelled { "backend_drop_contract" } else { "execution_returned" },
                "source_run":"source-one","run":"effect-one","effects":effects
            }),
        ),
    ]
}

#[test]
fn admission_refused_background_settles_without_an_exit_code() {
    let mut facts = background_debt_fixture(true);
    facts[2].payload["cancelled"] = json!(false);
    facts[2].payload["disposition"] = json!("failed");
    facts[2].payload["process_settlement_basis"] = json!("admission_refused");
    facts[2].payload["exit_code"] = json!(null);
    facts[2].payload["output_complete"] = json!(false);
    let debt = fold_debt(&facts);
    assert!(!debt.unresolved.contains("workspace-child:one"));
    assert!(debt
        .settled_workspace_children
        .contains("workspace-child:one"));

    facts[2].payload["exit_code"] = json!(1);
    let debt = fold_debt(&facts);
    assert!(
        debt.unresolved.contains("workspace-child:one"),
        "a fabricated exit code must not settle an admission refusal"
    );
}

#[test]
fn cancelled_background_can_settle_without_inventing_complete_output() {
    let facts = background_debt_fixture(true);
    assert_eq!(facts[2].payload["output_complete"], false);
    let debt = fold_debt(&facts);
    assert!(!debt.unresolved.contains("workspace-child:one"));
    assert!(debt.unresolved.contains("workspace-child:two"));
    assert!(debt
        .settled_workspace_children
        .contains("workspace-child:one"));
}

#[test]
fn background_evidence_does_not_clear_wrong_run_or_incomplete_terminals() {
    for (field, value) in [
        ("source_run", json!("other-source")),
        ("run", json!("other-effect")),
        ("task_id", json!("other-task")),
        ("observation_incomplete", json!(true)),
        ("output_facts_retained", json!(false)),
        ("process_settled", json!(false)),
        ("execution_future_released", json!(false)),
        ("timed_out", json!(true)),
        ("exit_code", json!(null)),
        ("exit_code", json!(-1)),
        ("disposition", json!("failed")),
        ("disposition", json!("unknown-state")),
        ("process_settlement_basis", json!("unproven")),
        ("effects", json!(null)),
        ("changed_paths", json!(["wrong.txt"])),
    ] {
        let mut facts = background_debt_fixture(false);
        facts[2].payload[field] = value;
        let debt = fold_debt(&facts);
        assert!(
            debt.unresolved.contains("workspace-child:one"),
            "invalid {field} cleared execution debt"
        );
        assert!(!debt
            .settled_workspace_children
            .contains("workspace-child:one"));
    }
}

#[test]
fn mixed_background_and_foreground_receipts_project_in_fact_order() {
    let mut facts = background_debt_fixture(false);
    let prepared = preparation("later-write");
    let mut later = result(&prepared, ToolExecutionOutcome::Succeeded);
    later.effects.changes[0].path = "background.txt".into();
    facts.extend([prepared_fact(&prepared), result_fact(&later)]);
    let debt = fold_debt(&facts);
    let mut ledger = crate::harness_loop::MutationLedger::default();
    for projection in debt.projection_order {
        match projection {
            ReceiptProjection::Tool(index) => {
                let (tool, result) = &debt.results[index];
                ledger.observe_tool_receipt(tool, result);
            }
            ReceiptProjection::Background(index) => {
                let (key, effects) = &debt.background_results[index];
                ledger.observe_background_receipt(key, effects);
            }
        }
    }
    assert!(ledger.content_digest_matches("background.txt", &sha256::digest("actual bytes")));
}

#[test]
fn legacy_unsettled_is_unknown_but_settled_legacy_history_stays_readable() {
    let legacy = fact(
        "tool.result",
        "old",
        json!({"toolCallId":"old","ok":true,"output":"ok"}),
    );
    assert!(!fold_debt(std::slice::from_ref(&legacy))
        .unresolved
        .is_empty());
    assert!(fold_debt(&[
        legacy,
        fact(
            TURN_COMPLETE_FACT_KIND,
            "old-marker",
            json!({"terminal":"verified"})
        )
    ])
    .unresolved
    .is_empty());
    let explicitly_unknown = fact(
        "tool.result",
        "legacy-unknown",
        json!({"toolCallId":"old","ok":false,"output":"unknown","unresolvedLegacy":true}),
    );
    assert!(!fold_debt(&[
        explicitly_unknown,
        fact(TURN_COMPLETE_FACT_KIND, "bad-marker", json!({}))
    ])
    .unresolved
    .is_empty());
}

#[test]
fn exact_confirmation_and_budget_refusals_prove_zero_execution() {
    let model = fact(
        "model.turn",
        "model:1:0",
        json!({"kind":"tool","call":{
        "id":"one","name":"write","args":{},"needs_confirmation":true
    },"also":[{"id":"two","name":"write","args":{},"needs_confirmation":false}]}),
    );
    let rejected = fact(
        "confirmation.answered",
        "answer",
        json!({"tool_call_id":"one","approved":false}),
    );
    let debt = fold_debt(&[model.clone(), rejected]);
    assert!(!debt.unresolved.iter().any(|key| key.ends_with(":one")));
    assert!(debt.unresolved.iter().any(|key| key.ends_with(":two")));
    let wrong = fact(
        "confirmation.answered",
        "wrong",
        json!({"tool_call_id":"two","approved":false}),
    );
    assert_eq!(fold_debt(&[model.clone(), wrong]).unresolved.len(), 2);
    let denied = fact(
        "budget.denied",
        "budget-denied:1:0:one",
        json!({"toolCallId":"one"}),
    );
    assert!(fold_debt(&[model.clone(), denied]).unresolved.is_empty());
    let unrelated = fact(
        "budget.denied",
        "budget-denied:9:0:one",
        json!({"toolCallId":"one"}),
    );
    assert_eq!(fold_debt(&[model, unrelated]).unresolved.len(), 2);
}

#[test]
fn protocol_one_unprepared_call_is_proven_unexecuted_but_preparation_opens_debt() {
    let model = fact(
        "model.turn",
        "model:1:0",
        json!({"kind":"tool","tool_effect_protocol":1,
        "call":{"id":"new","name":"write","args":{},"needs_confirmation":true}}),
    );
    assert!(fold_debt(std::slice::from_ref(&model))
        .unresolved
        .is_empty());
    let prepared = preparation("new");
    assert!(fold_debt(&[model, prepared_fact(&prepared)])
        .unresolved
        .contains(&prepared.identity.key().unwrap()));
}

#[test]
#[cfg(unix)]
fn byte_receipts_keep_all_paths_and_explicit_deletion() {
    let root = tempfile::tempdir().unwrap();
    let paths = (0..40)
        .map(|index| {
            let path = format!("f{index}.txt");
            std::fs::write(root.path().join(&path), format!("bytes-{index}")).unwrap();
            path
        })
        .chain(std::iter::once("deleted.txt".into()))
        .collect();
    let observed = crate::porcelain::observe_path_effects(
        root.path(),
        crate::porcelain::PathDelta {
            paths,
            incomplete: false,
        },
    );
    observed.validate().unwrap();
    assert_eq!(observed.changes.len(), 41);
    assert_eq!(observed.completeness, ToolObservationCompleteness::Complete);
    assert_eq!(
        observed
            .changes
            .iter()
            .find(|change| change.path == "deleted.txt")
            .unwrap()
            .after
            .kind,
        ToolFileKind::Missing
    );
    assert!(observed
        .changes
        .iter()
        .filter(|change| change.after.kind == ToolFileKind::File)
        .all(|change| change
            .after
            .content_digest
            .as_deref()
            .unwrap()
            .starts_with("sha256:")));
}

struct NoModel;
impl Completion for NoModel {
    fn complete(
        &self,
        _: CompletionRequest,
        _: tokio_util::sync::CancellationToken,
    ) -> a3s_effect::coding::BoxFuture<Result<ModelDecision, ActorError>> {
        Box::pin(async { Err(ActorError::Defect("test must not call a model".into())) })
    }
}
struct NoCompaction;
impl Compactor for NoCompaction {
    fn compact(&self, _: &[String]) -> a3s_effect::coding::BoxFuture<Result<String, ActorError>> {
        Box::pin(async { Err(ActorError::Defect("test must not compact".into())) })
    }
}
struct ReceiptProbe(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl crate::tools::Tool for ReceiptProbe {
    fn name(&self) -> &str {
        "receipt_probe"
    }
    fn description(&self) -> &str {
        "Controlled receipt boundary probe"
    }
    fn parameters(&self) -> serde_json::Value {
        json!({"type":"object"})
    }
    fn capabilities(&self, args: &serde_json::Value) -> crate::tools::ToolCapabilities {
        if args["read_only"] == true {
            crate::tools::ToolCapabilities::parallel_safe_read(2)
        } else {
            crate::tools::ToolCapabilities::conservative()
        }
    }
    async fn execute(
        &self,
        args: &serde_json::Value,
        ctx: &crate::tools::ToolContext,
    ) -> anyhow::Result<crate::tools::ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if let Some(path) = args["external_effect"].as_str() {
            std::fs::write(path, b"external side effect")?;
            anyhow::bail!("Workspace boundary: failure after external effect");
        }
        if args["write"] == true {
            std::fs::write(ctx.workspace.join("changed.txt"), "exact bytes")?;
        }
        if args["cancel_context"] == true {
            ctx.cancellation_token().cancel();
            std::future::pending::<()>().await;
        }
        if args["fail"] == true {
            Ok(crate::tools::ToolOutput::error("known failure after write"))
        } else {
            Ok(crate::tools::ToolOutput::success("observed"))
        }
    }
}
fn executor_tools(root: &Path) -> (Arc<ExecutorTools>, Arc<AtomicUsize>) {
    executor_tools_with(root, None)
}

fn executor_tools_with(
    root: &Path,
    sandbox: Option<Arc<dyn crate::sandbox::BashSandbox>>,
) -> (Arc<ExecutorTools>, Arc<AtomicUsize>) {
    let executor = Arc::new(ToolExecutor::new(root.to_string_lossy().into_owned()));
    let count = Arc::new(AtomicUsize::new(0));
    executor.register_dynamic_tool(Arc::new(ReceiptProbe(Arc::clone(&count))));
    let mut context = crate::tools::ToolContext::new(root.to_path_buf());
    if let Some(sandbox) = sandbox {
        context = context.with_sandbox(sandbox);
    }
    (
        Arc::new(ExecutorTools {
            executor,
            capability_operation: None,
            checkpoint: None,
            events: None,
            context,
            permission: PermissionPolicy::new().allow("*"),
            checker: None,
            agent: None,
            execution_started: None,
            run_store: None,
            run_id: None,
            ledger: Some(Arc::new(Mutex::new(
                crate::harness_loop::MutationLedger::default(),
            ))),
            reports: None,
            targeted_paths: None,
            pre_tool_hook_resolved: false,
            repeats: Arc::default(),
        }),
        count,
    )
}
fn request(id: &str, args: serde_json::Value) -> ToolPreparationRequest {
    ToolPreparationRequest {
        identity: preparation(id).identity,
        call: ToolCall {
            id: id.into(),
            name: "receipt_probe".into(),
            args,
            needs_confirmation: false,
            text: None,
            reasoning: None,
        },
    }
}
async fn execute_prepared(
    tools: Arc<ExecutorTools>,
    prepared: PreparedToolExecution,
) -> ToolExecutionResultV1 {
    let services = Arc::new(CodingServices {
        completion: Arc::new(NoModel),
        tools,
        compactor: Arc::new(NoCompaction),
    });
    prepared.execute.run(services).await.0.unwrap()
}

#[tokio::test]
#[cfg(unix)]
async fn prepared_batch_observes_each_actual_execution_boundary_and_failed_bytes() {
    let root = tempfile::tempdir().unwrap();
    let (tools, count) = executor_tools(root.path());
    let write = prepare_tools(
        Arc::clone(&tools),
        request("write", json!({"write":true,"fail":true})),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let verify = prepare_tools(
        Arc::clone(&tools),
        request("verify", json!({})),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let read = prepare_tools(
        Arc::clone(&tools),
        request("read", json!({"read_only":true})),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(read.prepared.baseline.is_none());
    let written = execute_prepared(Arc::clone(&tools), write).await;
    assert_eq!(written.outcome, ToolExecutionOutcome::Failed);
    assert_eq!(written.exit_code, Some(1));
    assert_eq!(
        written.effects.completeness,
        ToolObservationCompleteness::Complete
    );
    assert!(written
        .effects
        .changes
        .iter()
        .any(|change| change.path == "changed.txt"
            && change.after.content_digest
                == Some(format!("sha256:{}", sha256::digest("exact bytes")))));
    let digest = tools
        .ledger
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .digest()
        .to_string();
    assert!(execute_prepared(Arc::clone(&tools), verify)
        .await
        .effects
        .changes
        .is_empty());
    assert!(execute_prepared(Arc::clone(&tools), read)
        .await
        .effects
        .changes
        .is_empty());
    assert_eq!(
        tools.ledger.as_ref().unwrap().lock().unwrap().digest(),
        digest
    );
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_call_prepared_before_another_unknown_cannot_mutate_or_run_twice() {
    let root = tempfile::tempdir().unwrap();
    let (tools, count) = executor_tools(root.path());
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request("later", json!({"write":true})),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    tools
        .ledger
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .set_unresolved_tool_effects(std::collections::BTreeSet::from(["earlier-call".into()]));
    let repeated = PreparedToolExecution {
        prepared: prepared.prepared.clone(),
        execute: prepared.execute.clone(),
    };
    assert_eq!(
        execute_prepared(Arc::clone(&tools), prepared).await.outcome,
        ToolExecutionOutcome::Denied
    );
    assert_eq!(
        execute_prepared(Arc::clone(&tools), repeated).await.outcome,
        ToolExecutionOutcome::Unknown
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("changed.txt").exists());
}

#[tokio::test]
async fn missing_admission_and_permission_refusal_execute_nothing() {
    let root = tempfile::tempdir().unwrap();
    let (tools, count) = executor_tools(root.path());
    let mut admitted = tools.as_ref().clone();
    admitted.run_id = Some("admitted-without-binding".into());
    assert!(
        matches!(prepare_tools(Arc::new(admitted), request("missing", json!({"write":true})),
        tokio_util::sync::CancellationToken::new()).await,
        Err(ActorError::Schema(message)) if message.contains("admission_missing"))
    );
    let mut refused = tools.as_ref().clone();
    refused.permission = PermissionPolicy::new().deny("receipt_probe");
    let prepared = prepare_tools(
        Arc::new(refused),
        request("denied", json!({"write":true})),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        execute_prepared(tools, prepared).await.outcome,
        ToolExecutionOutcome::Denied
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn executed_error_text_cannot_prove_zero_execution_or_discharge_external_debt() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let external = root.path().join("external.counter");
    let (tools, count) = executor_tools(&workspace);
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request(
            "external-boundary-error",
            json!({"external_effect":external}),
        ),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let key = prepared.prepared.identity.key().unwrap();
    let result = execute_prepared(Arc::clone(&tools), prepared).await;
    assert_eq!(std::fs::read(&external).unwrap(), b"external side effect");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(result.outcome, ToolExecutionOutcome::Unknown);
    assert!(result.effects.unresolved_external_operations.contains(&key));
    assert!(matches!(crate::harness_loop::decide_completion(
        &tools.ledger.as_ref().unwrap().lock().unwrap(), &[], &[], false),
        crate::harness_loop::CompletionGate::Incomplete { message } if message.contains("tool_effect_unresolved")));
}

#[tokio::test]
async fn context_cancellation_after_a_write_is_not_a_pre_execution_denial() {
    let root = tempfile::tempdir().unwrap();
    let (tools, count) = executor_tools(root.path());
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request(
            "context-cancel",
            json!({"write":true,"cancel_context":true}),
        ),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let key = prepared.prepared.identity.key().unwrap();
    let result = execute_prepared(tools, prepared).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(root.path().join("changed.txt")).unwrap(),
        b"exact bytes"
    );
    assert_eq!(result.outcome, ToolExecutionOutcome::Cancelled);
    assert_eq!(result.exit_code, None);
    assert!(result.effects.unresolved_external_operations.contains(&key));
    assert!(result
        .effects
        .changes
        .iter()
        .any(|change| change.path == "changed.txt"));
}

#[derive(Debug)]
struct RewritingReceiptHook(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl crate::hooks::HookExecutor for RewritingReceiptHook {
    async fn fire(&self, event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        if matches!(event, crate::hooks::HookEvent::PreToolUse(_)) {
            self.0.fetch_add(1, Ordering::SeqCst);
            crate::hooks::HookResult::continue_with(json!({"updatedInput":{"write":true}}))
        } else {
            crate::hooks::HookResult::Continue(None)
        }
    }
}

#[tokio::test]
async fn unchanged_ask_obeys_the_frozen_host_confirmation_authority() {
    for (label, host, manager, confirmed, expected) in [
        ("bare", false, false, false, ToolExecutionOutcome::Denied),
        (
            "host-disabled",
            true,
            false,
            false,
            ToolExecutionOutcome::Succeeded,
        ),
        ("host-asks", true, true, false, ToolExecutionOutcome::Denied),
        (
            "actor-confirmed",
            true,
            true,
            true,
            ToolExecutionOutcome::Succeeded,
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (base, count) = executor_tools(root.path());
        let mut tools = base.as_ref().clone();
        tools.permission = PermissionPolicy {
            default_decision: PermissionDecision::Ask,
            ..Default::default()
        };
        if host {
            let confirmation_manager = if manager {
                let (events, _) = tokio::sync::broadcast::channel(8);
                Some(Arc::new(crate::hitl::ConfirmationManager::new(
                    crate::hitl::ConfirmationPolicy::enabled(),
                    events,
                ))
                    as Arc<dyn crate::hitl::ConfirmationProvider>)
            } else {
                None
            };
            tools.agent = Some(crate::agent::AgentLoop::new(
                Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
                Arc::clone(&tools.executor),
                tools.context.clone(),
                crate::agent::AgentConfig {
                    confirmation_manager,
                    ..Default::default()
                },
            ));
        }
        let tools = Arc::new(tools);
        let mut call = request(label, json!({"write":true}));
        call.call.needs_confirmation = confirmed;
        let prepared = prepare_tools(
            Arc::clone(&tools),
            call,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
        let result = execute_prepared(tools, prepared).await;
        assert_eq!(result.outcome, expected, "{label}");
        let executed = expected == ToolExecutionOutcome::Succeeded;
        assert_eq!(
            count.load(Ordering::SeqCst),
            usize::from(executed),
            "{label}"
        );
        assert_eq!(
            root.path().join("changed.txt").exists(),
            executed,
            "{label}"
        );
    }
}

#[tokio::test]
async fn preparation_resolves_hooks_once_binds_effective_input_and_rechecks_approval() {
    let root = tempfile::tempdir().unwrap();
    let (base, count) = executor_tools(root.path());
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let mut tools = base.as_ref().clone();
    tools.agent = Some(crate::agent::AgentLoop::new(
        Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
        Arc::clone(&tools.executor),
        tools.context.clone(),
        crate::agent::AgentConfig {
            hook_engine: Some(Arc::new(RewritingReceiptHook(Arc::clone(&hook_calls)))),
            ..Default::default()
        },
    ));
    let tools = Arc::new(tools);
    let original = json!({"write":false});
    let prepared = prepare_tools(
        Arc::clone(&tools),
        request("hook", original.clone()),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "preparation cannot execute"
    );
    assert_eq!(
        prepared.prepared.original_args_digest,
        a3s_effect::tool_args_digest(&original).unwrap()
    );
    assert_eq!(
        prepared.prepared.effective_args_digest,
        a3s_effect::tool_args_digest(&json!({"write":true})).unwrap()
    );
    assert_eq!(
        execute_prepared(Arc::clone(&tools), prepared).await.outcome,
        ToolExecutionOutcome::Succeeded
    );
    assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(root.path().join("changed.txt")).unwrap(),
        b"exact bytes"
    );

    let mut unapproved = tools.as_ref().clone();
    unapproved.permission = PermissionPolicy {
        default_decision: PermissionDecision::Ask,
        ..Default::default()
    };
    let unapproved = Arc::new(unapproved);
    let mut call = request("hook-unapproved", original);
    call.call.needs_confirmation = true;
    let prepared = prepare_tools(
        Arc::clone(&unapproved),
        call,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        execute_prepared(unapproved, prepared).await.outcome,
        ToolExecutionOutcome::Denied
    );
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "changed input cannot reuse old approval"
    );
}

struct NoCheckpoint;
#[async_trait::async_trait]
impl crate::loop_checkpoint::LoopCheckpointSink for NoCheckpoint {
    async fn save_checkpoint(&self, _: &crate::loop_checkpoint::LoopCheckpoint) {}
    async fn load_latest(&self, _: &str) -> Option<crate::loop_checkpoint::LoopCheckpoint> {
        None
    }
}
fn binding(rounds: usize) -> crate::capability::RunCapabilityBindingV1 {
    use crate::capability::*;
    let set = CapabilitySet::empty().unwrap();
    let ceiling = CapabilityCeiling::all(
        &set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        CapabilityExecutionCeiling::new(rounds, 4, None, None, None).unwrap(),
    )
    .unwrap();
    RunCapabilityBindingV1::from_set_and_ceiling(&set, &ceiling).unwrap()
}

#[tokio::test]
async fn preparation_binds_the_actual_run_and_refuses_frozen_generation_drift() {
    let root = tempfile::tempdir().unwrap();
    let (base, count) = executor_tools(root.path());
    let store = Arc::new(crate::run::InMemoryRunStore::new());
    store
        .create_run_with_id("admitted".into(), "session", "prompt")
        .await;
    let original = binding(8);
    store
        .bind_capability_generation("admitted", original.clone())
        .await
        .unwrap();
    let mut tools = base.as_ref().clone();
    tools.run_id = Some("admitted".into());
    tools.run_store = Some(store);
    tools.context = tools.context.with_session_id("session");
    tools.checkpoint = Some(CheckpointState {
        sink: Arc::new(NoCheckpoint),
        run_id: "admitted".into(),
        session_id: "session".into(),
        capability_binding: Some(original.clone()),
        standing_context_digest: None,
        durable_source: None,
        turns: Arc::new(AtomicUsize::new(0)),
    });
    let prepared = prepare_tools(
        Arc::new(tools.clone()),
        request("bound", json!({"write":true})),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let admission = prepared.prepared.identity.admission.unwrap();
    assert_eq!(admission.run_id, "admitted");
    assert_eq!(admission.thread_id, thread_for_session("session"));
    assert_eq!(
        admission.capability_binding_digest,
        crate::content_digest::digest_json("a3s.code.tool-admission-binding.v1", &original)
            .unwrap()
    );
    tools.checkpoint.as_mut().unwrap().capability_binding = Some(binding(9));
    assert!(
        matches!(prepare_tools(Arc::new(tools), request("drift", json!({"write":true})),
        tokio_util::sync::CancellationToken::new()).await,
        Err(ActorError::Schema(message)) if message.contains("admission_drift"))
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[test]
fn recovery_seed_and_legacy_reset_preserve_the_authoritative_prefix() {
    let root = tempfile::tempdir().unwrap();
    let (tools, _) = executor_tools(root.path());
    let config = HarnessConfig::new(4, 100_000, 16, 1, Vec::new(), Vec::new()).unwrap();
    let run = FactRun::open(root.path(), Arc::new(NoModel), tools, config, "immutable").unwrap();
    let prepared = preparation("interrupted");
    let initial = [
        message_fact("input", "do it"),
        NewFact {
            kind: "tool.prepared".into(),
            key: "prepared".into(),
            payload: json!({"prepared":prepared}),
        },
    ];
    run.log.append(&run.thread, &initial, None).unwrap();
    let before = run.read_facts().unwrap();
    assert!(run
        .reset_thread_log()
        .unwrap_err()
        .to_string()
        .contains("log_immutable"));
    run.seed_recovery_history(&[Message::user("cached, older history")], "resume")
        .unwrap();
    assert_eq!(run.read_facts().unwrap(), before);
    assert!(fold_debt(&before)
        .unresolved
        .contains(&prepared.identity.key().unwrap()));
}
