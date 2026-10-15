#![cfg(test)]

use super::*;
use a3s_code_core::goal_control::{
    GoalCommandKindV1, GoalControlCommandV1, GoalDefinitionCommandV1, GoalPlanCommandV1, GoalState,
    GoalTickDecisionV1,
};
use a3s_code_core::planning::{AgentGoal, Complexity, ExecutionPlan, Task, TaskStatus};
use a3s_code_core::store::FileSessionStore;
use a3s_code_core::tools::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use a3s_code_core::verification::{VerificationCheck, VerificationReport, VerificationStatus};
use a3s_code_core::workspace::WorkspaceServices;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct ByteAttestor {
    root: std::path::PathBuf,
    expected: String,
    calls: AtomicUsize,
}

fn tick_request(session: &a3s_code_core::AgentSession, operation_id: &str) -> GoalControlCommandV1 {
    let goal = session.goal_definition().unwrap().unwrap();
    GoalControlCommandV1 {
        kind: GoalCommandKindV1::Tick,
        operation_id: operation_id.into(),
        goal_id: goal.binding.goal_id,
        session_id: goal.binding.session_id,
        workspace: goal.binding.workspace,
        work_policy_digest: goal.binding.work_policy_digest,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
    }
}

async fn preview_does_not_execute(session: &a3s_code_core::AgentSession) {
    let command = tick_request(session, "preview-only");
    let workspace = std::path::PathBuf::from(&command.workspace.display_root);
    let before =
        a3s_code_core::fact_control::read_workspace_facts(&workspace, session.session_id())
            .unwrap();
    let runs = serde_json::to_value(session.runs().await).unwrap();
    assert_eq!(
        session.preview_goal_tick(&command).await.unwrap(),
        GoalTickDecisionV1::BudgetExhausted
    );
    assert_eq!(
        a3s_code_core::fact_control::read_workspace_facts(&workspace, session.session_id())
            .unwrap(),
        before
    );
    assert_eq!(serde_json::to_value(session.runs().await).unwrap(), runs);
}

impl a3s_code_core::CompletionAttestor for ByteAttestor {
    fn attest(
        &self,
        digest: &str,
        paths: &[a3s_code_core::MutatedPathRecord],
    ) -> Option<VerificationReport> {
        use std::io::Read;
        self.calls.fetch_add(1, Ordering::SeqCst);
        if paths.len() != 1 || paths[0].path != "source.txt" {
            return None;
        }
        let path = self.root.join("source.txt");
        if !std::fs::symlink_metadata(&path).ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(4097)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes != self.expected.as_bytes() || paths[0].content_digest != sha256::digest(&bytes) {
            return None;
        }
        Some(
            VerificationReport::new(
                "host:authored-source",
                vec![VerificationCheck::required(
                    "source-bytes",
                    "host_source_bytes",
                    format!(
                        "source.txt contains exactly {} and matches the original receipt digest",
                        self.expected
                    ),
                )
                .with_status(VerificationStatus::Passed)],
            )
            .with_effect_digest(digest),
        )
    }
}

struct VerifySource(String);

#[async_trait::async_trait]
impl Tool for VerifySource {
    fn name(&self) -> &str {
        "verify_source"
    }
    fn description(&self) -> &str {
        "Read source.txt and independently verify its bytes against the host's expected value. Returns the current value and verification report."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{},"additionalProperties":false})
    }
    fn capabilities(&self, _: &serde_json::Value) -> ToolCapabilities {
        ToolCapabilities::read_only_paginated(1)
    }
    async fn execute(
        &self,
        _: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let value = std::fs::read_to_string(ctx.workspace.join("source.txt"))?;
        let status = if value == self.0 {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        };
        let report = VerificationReport::new(
            "host:source-check",
            vec![VerificationCheck::required(
                "inspect",
                "source-bytes",
                "The bytes read from source.txt match the host's expected value",
            )
            .with_status(status)],
        );
        let output = serde_json::to_string(&serde_json::json!({
            "path":"source.txt", "observed":value, "verification_report":report,
        }))?;
        Ok(
            ToolOutput::success(output).with_metadata(serde_json::json!({
                "verification_author":"host", "verification_report":report,
            })),
        )
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn current_step_real_judgment_accepts_and_reopens_original_evidence() {
    prove_current_step(false).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn current_step_real_modification_uses_fresh_host_attestation_and_reopens() {
    prove_current_step(true).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn current_step_real_exact_operation_replays_after_file_reopen() {
    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let value = format!("VERIFIED-{}", uuid::Uuid::new_v4());
    std::fs::write(workspace.path().join("source.txt"), &value).unwrap();
    let mut config = load_pinned_layer_c_config();
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 180_000;
    // Parsed host bounds do not relabel an ordinary Run as an automatic slice.
    config.goal_control = a3s_code_core::CodeConfig::from_acl(&format!(
        r#"goal_control {{
            automatic_enabled = true
            max_slices = "1"
            max_no_progress_slices = "1"
            deadline_unix_ms = "{deadline}"
        }}"#
    ))
    .unwrap()
    .goal_control;
    assert!(config.goal_control.as_ref().unwrap().automatic_enabled);
    config.memory = Some(a3s_code_core::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let agent = Agent::from_config(config.clone()).await.unwrap();
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let guard = Arc::new(compaction::UsageProbe::default());
    let workflow =
        Arc::new(a3s_code_core::WorkflowBudget::new(Some(100_000)).with_inner(guard.clone()));
    let mut permission = PermissionPolicy::new().allow("verify_source(*)");
    permission.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("goal-exact-live")
                    .with_session_store(store)
                    .with_planning_mode(PlanningMode::Enabled)
                    .with_goal_tracking(true)
                    .with_auto_compact(false)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_continuation(false)
                    .with_permission_policy(permission)
                    .with_budget_guard(workflow.clone())
                    .with_max_tool_rounds(4)
                    .with_llm_api_timeout(180_000)
                    .with_temperature(0.0),
            ),
        )
        .await
        .unwrap();
    session
        .register_dynamic_tool(Arc::new(VerifySource(value.clone())))
        .unwrap();
    admit_source_plan(&session, workspace.path(), false, &value);
    let run_id = format!("goal-exact-{}", uuid::Uuid::new_v4());
    let request = tick_request(&session, &run_id);
    assert_eq!(
        session.preview_goal_tick(&request).await.unwrap(),
        GoalTickDecisionV1::Advance
    );
    assert!(guard.0.lock().unwrap().is_empty());
    let prompt = "Execute only the current admitted step using fresh evidence.";
    let started = std::time::Instant::now();
    let a3s_code_core::AgentRunSpawn::Started { worker, .. } =
        session.spawn_run_with_id(&run_id, prompt).await.unwrap()
    else {
        panic!("the explicit original request must create its first Run");
    };
    assert_eq!(
        session.preview_goal_tick(&request).await.unwrap(),
        GoalTickDecisionV1::Reconcile
    );
    tokio::time::timeout(std::time::Duration::from_secs(180), worker)
        .await
        .unwrap()
        .unwrap();
    let elapsed = started.elapsed().as_secs_f64();
    let snapshot = session.run_snapshot(&run_id).await.unwrap();
    eprintln!(
        "GOAL-EXACT status={:?} result={:?}",
        snapshot.status, snapshot.result_text
    );
    assert_eq!(snapshot.status, a3s_code_core::run::RunStatus::Completed);
    assert!(snapshot.result_text.as_deref().unwrap().contains(&value));
    let goal = session.goal_definition().unwrap().unwrap();
    assert_eq!(
        goal.plan.as_ref().unwrap().steps[0].status,
        TaskStatus::Completed
    );
    assert_eq!(
        goal.plan.as_ref().unwrap().steps[1].status,
        TaskStatus::Pending
    );
    assert_eq!(goal.state, GoalState::Active);
    let before =
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "goal-exact-live")
            .unwrap();
    let slices: Vec<_> = before
        .iter()
        .filter(|f| f.kind == "a3s.goal.slice.admitted")
        .collect();
    assert_eq!(slices.len(), 1);
    assert!(slices[0].payload["body"].get("continuation").is_none());
    assert_eq!(slices[0].payload["operation_id"], run_id);
    assert_eq!(slices[0].payload["body"]["run_id"], run_id);
    assert_eq!(
        slices[0].payload["body"]["expected_goal_sequence"],
        request.expected_goal_sequence
    );
    for fact in before.iter().filter(|f| f.kind == "tool.result") {
        let receipt: a3s_effect::ToolExecutionResultV1 =
            serde_json::from_value(fact.payload["receipt"].clone()).unwrap();
        assert_eq!(receipt.identity.admission.as_ref().unwrap().run_id, run_id);
        assert!(receipt.effects.changes.is_empty());
    }
    assert_eq!(
        before
            .iter()
            .filter(|f| f.kind == "a3s.goal.step.accepted" && f.payload["body"]["run_id"] == run_id)
            .count(),
        1
    );
    assert!(!before.iter().any(|f| f.kind == "a3s.goal.completed"));
    let calls = guard.0.lock().unwrap().clone();
    let total = calls.iter().map(|usage| usage.total_tokens).sum::<usize>();
    let events = session.run_events(&run_id).await;
    let end = events
        .iter()
        .find_map(|event| match &event.event {
            a3s_code_core::AgentEvent::End { usage, .. } => Some(usage),
            _ => None,
        })
        .unwrap();
    assert_eq!(end.total_tokens, total);
    assert_eq!(workflow.consumed_tokens(), total as u64);
    let infer = before.iter().filter(|f| f.kind == "model.turn").count();
    assert!(calls.len() > infer);
    eprintln!("GOAL-EXACT elapsed={elapsed:.2}s governed_calls={} infer_calls={infer} tokens={total} usage={calls:?}", calls.len());
    for _ in 0..4 {
        assert_eq!(
            session.preview_goal_tick(&request).await.unwrap(),
            GoalTickDecisionV1::Reconcile
        );
        assert!(session
            .spawn_run_with_id(&run_id, prompt)
            .await
            .unwrap()
            .replayed());
    }
    assert!(session
        .spawn_run_with_id(&run_id, "different immutable input")
        .await
        .is_err());
    assert!(session
        .preview_goal_tick(&tick_request(&session, &run_id))
        .await
        .is_err());
    assert_eq!(guard.0.lock().unwrap().len(), calls.len());
    assert_eq!(workflow.consumed_tokens(), total as u64);
    assert_eq!(
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "goal-exact-live")
            .unwrap(),
        before
    );
    session.save().await.unwrap();
    session.close().await;
    agent.close().await;
    let agent = Agent::from_config(config).await.unwrap();
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let restored = agent
        .resume_session_async(
            "goal-exact-live",
            SessionOptions::new()
                .with_session_store(store)
                .with_budget_guard(workflow.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        restored.preview_goal_tick(&request).await.unwrap(),
        GoalTickDecisionV1::Reconcile
    );
    let replay = restored.spawn_run_with_id(&run_id, prompt).await.unwrap();
    assert!(replay.replayed());
    assert_eq!(replay.snapshot().result_text, snapshot.result_text);
    assert_eq!(restored.runs().await.len(), 1);
    assert_eq!(guard.0.lock().unwrap().len(), calls.len());
    assert_eq!(workflow.consumed_tokens(), total as u64);
    assert_eq!(
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "goal-exact-live")
            .unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(workspace.path().join("source.txt")).unwrap(),
        value.as_bytes()
    );
    restored.close().await;
    agent.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn automatic_tick_real_admits_one_run_and_keeps_finite_use_after_reopen() {
    use a3s_code_core::{AgentRunSpawn, GoalTickAdmission};
    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let value = format!("VERIFIED-{}", uuid::Uuid::new_v4());
    std::fs::write(workspace.path().join("source.txt"), &value).unwrap();
    let mut config = load_pinned_layer_c_config();
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 180_000;
    config.goal_control = a3s_code_core::CodeConfig::from_acl(&format!(
        r#"goal_control {{
            automatic_enabled = true
            max_slices = "1"
            max_no_progress_slices = "2"
            deadline_unix_ms = "{deadline}"
        }}"#
    ))
    .unwrap()
    .goal_control;
    config.memory = Some(a3s_code_core::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let agent = Agent::from_config(config.clone()).await.unwrap();
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let guard = Arc::new(compaction::UsageProbe::default());
    let workflow =
        Arc::new(a3s_code_core::WorkflowBudget::new(Some(100_000)).with_inner(guard.clone()));
    let mut permission = PermissionPolicy::new().allow("verify_source(*)");
    permission.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("goal-auto-live")
                    .with_session_store(store)
                    .with_planning_mode(PlanningMode::Enabled)
                    .with_goal_tracking(true)
                    .with_auto_compact(false)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_continuation(false)
                    .with_permission_policy(permission)
                    .with_budget_guard(workflow.clone())
                    .with_max_tool_rounds(4)
                    .with_llm_api_timeout(180_000)
                    .with_temperature(0.0),
            ),
        )
        .await
        .unwrap();
    session
        .register_dynamic_tool(Arc::new(VerifySource(value.clone())))
        .unwrap();
    admit_source_plan(&session, workspace.path(), false, &value);
    let facts = || {
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "goal-auto-live")
            .unwrap()
    };
    let tick = tick_request(&session, &format!("auto-{}", uuid::Uuid::new_v4()));
    let defined = facts();
    assert_eq!(
        session.preview_goal_tick(&tick).await.unwrap(),
        GoalTickDecisionV1::Advance
    );
    assert_eq!(facts(), defined);
    assert!(guard.0.lock().unwrap().is_empty());

    let started = std::time::Instant::now();
    let (first, second) = tokio::join!(session.tick_goal(&tick), session.tick_goal(&tick));
    let mut admitted = 0;
    for result in [first, second] {
        match result {
            Ok(GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. })) => {
                admitted += 1;
                tokio::time::timeout(std::time::Duration::from_secs(180), worker)
                    .await
                    .unwrap()
                    .unwrap();
            }
            Ok(GoalTickAdmission::Admitted(AgentRunSpawn::Replayed { .. }))
            | Ok(GoalTickAdmission::Decided(GoalTickDecisionV1::Reconcile))
            | Err(a3s_code_core::CodeError::SessionBusy { .. }) => {}
            Ok(GoalTickAdmission::Decided(other)) => panic!("unexpected decision {other:?}"),
            Err(error) => panic!("unexpected tick failure: {error}"),
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    assert_eq!(admitted, 1, "one automatic tick admits one original Run");
    let snapshot = session.run_snapshot(&tick.operation_id).await.unwrap();
    eprintln!(
        "GOAL-AUTO status={:?} result={:?}",
        snapshot.status, snapshot.result_text
    );
    assert_eq!(snapshot.status, a3s_code_core::run::RunStatus::Completed);
    assert!(snapshot.result_text.as_deref().unwrap().contains(&value));
    let goal = session.goal_definition().unwrap().unwrap();
    assert_eq!(
        goal.plan.as_ref().unwrap().steps[0].status,
        TaskStatus::Completed
    );
    assert_eq!(
        goal.plan.as_ref().unwrap().steps[1].status,
        TaskStatus::Pending
    );
    let after = facts();
    let slices: Vec<_> = after
        .iter()
        .filter(|f| f.kind == "a3s.goal.slice.admitted")
        .collect();
    assert_eq!(slices.len(), 1);
    assert_eq!(slices[0].payload["body"]["run_id"], tick.operation_id);
    let source = &slices[0].payload["body"]["continuation"];
    assert_eq!(
        source["policy"],
        serde_json::to_value(config.goal_control.as_ref().unwrap()).unwrap()
    );
    assert!(source["observed_unix_ms"].as_u64().unwrap() > 0);
    let calls = guard.0.lock().unwrap().clone();
    let total = calls.iter().map(|usage| usage.total_tokens).sum::<usize>();
    assert_eq!(workflow.consumed_tokens(), total as u64);
    let infer = after.iter().filter(|f| f.kind == "model.turn").count();
    eprintln!(
        "GOAL-AUTO elapsed={elapsed:.2}s governed_calls={} infer_calls={infer} tokens={total} usage={calls:?}",
        calls.len()
    );

    for _ in 0..3 {
        assert!(matches!(
            session.tick_goal(&tick).await.unwrap(),
            GoalTickAdmission::Decided(GoalTickDecisionV1::Reconcile)
        ));
    }
    let next = tick_request(&session, &format!("auto-{}", uuid::Uuid::new_v4()));
    assert!(matches!(
        session.tick_goal(&next).await.unwrap(),
        GoalTickAdmission::Decided(GoalTickDecisionV1::BudgetExhausted)
    ));
    assert_eq!(guard.0.lock().unwrap().len(), calls.len());
    assert_eq!(facts(), after);
    session.save().await.unwrap();
    session.close().await;
    agent.close().await;

    let agent = Agent::from_config(config).await.unwrap();
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let restored = agent
        .resume_session_async(
            "goal-auto-live",
            SessionOptions::new()
                .with_session_store(store)
                .with_budget_guard(workflow.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        restored.preview_goal_tick(&next).await.unwrap(),
        GoalTickDecisionV1::BudgetExhausted,
        "reopen keeps finite automatic use"
    );
    assert!(matches!(
        restored.tick_goal(&tick).await.unwrap(),
        GoalTickAdmission::Decided(GoalTickDecisionV1::Reconcile)
    ));
    assert_eq!(guard.0.lock().unwrap().len(), calls.len());
    assert_eq!(workflow.consumed_tokens(), total as u64);
    assert_eq!(facts(), after);
    restored.close().await;
    agent.close().await;
}

async fn prove_current_step(modify: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let value = format!("VERIFIED-{}", uuid::Uuid::new_v4());
    std::fs::write(
        workspace.path().join("source.txt"),
        if modify { "old source" } else { &value },
    )
    .unwrap();
    let mut config = load_pinned_layer_c_config();
    config.memory = Some(a3s_code_core::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let agent = Agent::from_config(config.clone()).await.unwrap();
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let guard = Arc::new(compaction::UsageProbe::default());
    let mut permission = if modify {
        PermissionPolicy::new().allow_all(&["read(*)", "write(source.txt)", "edit(source.txt)"])
    } else {
        PermissionPolicy::new().allow("verify_source(*)")
    };
    permission.default_decision = PermissionDecision::Deny;
    let attestor = Arc::new(ByteAttestor {
        root: workspace.path().into(),
        expected: value.clone(),
        calls: AtomicUsize::new(0),
    });
    let mut options = SessionOptions::new()
        .with_session_id("goal-step-live")
        .with_session_store(store)
        .with_planning_mode(PlanningMode::Enabled)
        .with_goal_tracking(true)
        .with_auto_compact(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_continuation(false)
        .with_assistant_final_response(modify)
        .with_permission_policy(permission)
        .with_budget_guard(guard.clone())
        .with_max_tool_rounds(4)
        .with_llm_api_timeout(180_000)
        .with_temperature(0.0);
    if modify {
        options = options.with_completion_attestor(attestor.clone());
    }
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .unwrap();
    if !modify {
        session
            .register_dynamic_tool(Arc::new(VerifySource(value.clone())))
            .unwrap();
    }
    admit_source_plan(&session, workspace.path(), modify, &value);
    preview_does_not_execute(&session).await;
    let (measurement, answer) = timed_send(
        if modify { "GOAL-MODIFY" } else { "GOAL-STEP" },
        &session,
        if modify {
            "This fixture contains only source.txt. Shell execution is unavailable. Execute only the current admitted step using fresh evidence, then give the final answer."
        } else {
            "Execute only the current admitted step using fresh evidence."
        },
    )
    .await;
    print_row(&measurement);
    eprintln!("GOAL-STEP tool_uses={:?}", tool_uses(&answer));
    eprintln!(
        "GOAL-STEP answer={} forced_stop={:?} reports={} governed_usage={:?}",
        answer.text,
        answer.forced_stop,
        serde_json::to_string(&answer.verification_reports).unwrap(),
        guard.0.lock().unwrap()
    );
    let diagnostic =
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), session.session_id())
            .unwrap();
    for fact in diagnostic.iter().filter(|f| f.kind == "tool.result") {
        let receipt: a3s_effect::ToolExecutionResultV1 =
            serde_json::from_value(fact.payload["receipt"].clone()).unwrap();
        let body = receipt.output.as_str().unwrap_or("");
        let kind = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.get("kind").cloned());
        eprintln!(
            "GOAL-STEP receipt outcome={:?} admission={:?} output_bytes={} envelope={:?}",
            receipt.outcome,
            receipt.identity.admission,
            body.len(),
            kind
        );
    }
    eprintln!(
        "GOAL-STEP infer_facts={}",
        diagnostic.iter().filter(|f| f.kind == "model.turn").count()
    );
    assert!(answer.text.contains(&value), "{}", answer.text);
    let goal = session.goal_definition().unwrap().unwrap();
    let plan = goal.plan.as_ref().unwrap();
    assert_eq!(plan.steps[0].status, TaskStatus::Completed);
    assert_eq!(plan.steps[1].status, TaskStatus::Pending);
    assert_eq!(plan.get_ready_steps()[0].id, "follow-up");
    assert_eq!(goal.state, GoalState::Active);
    let facts =
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), session.session_id())
            .unwrap();
    let accepted: Vec<_> = facts
        .iter()
        .filter(|f| f.kind == "a3s.goal.step.accepted")
        .collect();
    assert_eq!(accepted.len(), 1);
    let admission = facts
        .iter()
        .find(|f| f.kind == "a3s.goal.slice.admitted")
        .unwrap();
    assert_eq!(
        accepted[0].payload["body"]["run_id"],
        admission.payload["body"]["run_id"]
    );
    assert_eq!(accepted[0].payload["body"]["run_sequence"], admission.seq);
    let results: Vec<_> = facts.iter().filter(|f| f.kind == "tool.result").collect();
    assert!(!results.is_empty());
    let mut changes = 0;
    for fact in results {
        let receipt: a3s_effect::ToolExecutionResultV1 =
            serde_json::from_value(fact.payload["receipt"].clone()).unwrap();
        assert_eq!(
            receipt.identity.admission.as_ref().unwrap().run_id,
            admission.payload["body"]["run_id"].as_str().unwrap()
        );
        if !modify {
            assert!(receipt.output.to_string().contains(&value));
            assert!(receipt.output.to_string().contains("host:source-check"));
        }
        for change in &receipt.effects.changes {
            assert_eq!(change.path, "source.txt");
            assert_eq!(
                change.after.content_digest,
                Some(format!("sha256:{}", sha256::digest(value.as_bytes())))
            );
            changes += 1;
        }
    }
    assert_eq!(
        std::fs::read(workspace.path().join("source.txt")).unwrap(),
        value.as_bytes()
    );
    if modify {
        assert!(
            changes > 0,
            "the actual original tool must modify the fixture"
        );
        assert_eq!(attestor.calls.load(Ordering::SeqCst), 1);
        let report: VerificationReport =
            serde_json::from_value(accepted[0].payload["body"]["attestation"].clone()).unwrap();
        assert_eq!(report.subject, "host:authored-source");
        let a3s_code_core::harness_loop::CompletionTerminal::Verified { effect_digest } =
            &answer.completion
        else {
            panic!("original completion must be verified");
        };
        assert_eq!(report.effect_digest.as_ref(), Some(effect_digest));
    } else {
        assert_eq!(changes, 0);
        assert!(accepted[0].payload["body"].get("attestation").is_none());
    }
    let cut_index = facts
        .iter()
        .position(|f| {
            f.seq
                == accepted[0].payload["body"]["source_sequence"]
                    .as_u64()
                    .unwrap()
        })
        .unwrap();
    let cut = a3s_effect::cut_log(&facts[..=cut_index]);
    assert_eq!(accepted[0].payload["body"]["source_digest"], cut.digest);
    assert_eq!(facts[cut_index + 1].kind, "turn.complete");
    assert!(!facts.iter().any(|f| f.kind == "a3s.goal.completed"));
    let calls = guard.0.lock().unwrap().clone();
    let infer_calls = facts.iter().filter(|f| f.kind == "model.turn").count();
    assert!(
        calls.len() > infer_calls,
        "original judgment must use the governed provider"
    );
    assert_eq!(
        answer.usage.total_tokens,
        calls.iter().map(|usage| usage.total_tokens).sum::<usize>()
    );
    eprintln!(
        "GOAL-STEP governed_calls={} infer_calls={} judgment_calls={} total_tokens={}",
        calls.len(),
        infer_calls,
        calls.len() - infer_calls,
        answer.usage.total_tokens
    );
    preview_does_not_execute(&session).await;
    session.save().await.unwrap();
    session.close().await;
    agent.close().await;
    let agent = Agent::from_config(config.clone()).await.unwrap();
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let recheck_usage = Arc::new(compaction::UsageProbe::default());
    let mut recheck_permission = PermissionPolicy::new().allow("verify_source(*)");
    recheck_permission.default_decision = PermissionDecision::Deny;
    let restored = agent
        .resume_session_async(
            "goal-step-live",
            SessionOptions::new()
                .with_session_store(store)
                .with_budget_guard(recheck_usage.clone())
                .with_permission_policy(recheck_permission)
                .with_planning_mode(PlanningMode::Enabled)
                .with_goal_tracking(true)
                .with_auto_compact(false)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false)
                .with_continuation(false)
                .with_assistant_final_response(true)
                .with_max_tool_rounds(4)
                .with_llm_api_timeout(180_000)
                .with_temperature(0.0),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(restored.goal_definition().unwrap().unwrap()).unwrap(),
        serde_json::to_value(&goal).unwrap()
    );
    preview_does_not_execute(&restored).await;
    assert_eq!(
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "goal-step-live")
            .unwrap(),
        facts
    );
    if modify {
        restored
            .register_dynamic_tool(Arc::new(VerifySource(value.clone())))
            .unwrap();
        let mut plan = goal.plan.unwrap();
        plan.steps[0].content = "Revalidate the retained source bytes without modifying any file, and quote the current value".into();
        plan.steps[0].success_criteria = Some(format!("The retained source is independently verified to equal {value} and the answer quotes it exactly"));
        plan.steps[0].tool = Some("verify_source".into());
        let mut command = GoalPlanCommandV1 {
            kind: GoalCommandKindV1::ProposePlan,
            operation_id: "replan".into(),
            goal_id: goal.binding.goal_id,
            session_id: goal.binding.session_id,
            workspace: goal.binding.workspace,
            work_policy_digest: goal.binding.work_policy_digest,
            expected_goal_revision: goal.binding.goal_revision,
            expected_plan_revision: goal.plan_revision,
            expected_goal_sequence: goal.goal_sequence,
            plan: Some(plan),
            proposal_sequence: None,
        };
        let proposed = restored.apply_goal_plan(command.clone()).unwrap();
        assert!(proposed
            .plan
            .as_ref()
            .unwrap()
            .steps
            .iter()
            .all(|step| step.status == TaskStatus::Pending));
        command.kind = GoalCommandKindV1::AcceptPlan;
        command.operation_id = "accept-replan".into();
        command.expected_plan_revision = proposed.plan_revision;
        command.expected_goal_sequence = proposed.goal_sequence;
        command.proposal_sequence = proposed.plan_fact_sequence;
        command.plan = None;
        restored.apply_goal_plan(command).unwrap();
        let before =
            a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "goal-step-live")
                .unwrap();
        let (measurement, rechecked) = timed_send("GOAL-REVALIDATE", &restored, "Execute the current revalidation step with fresh evidence. Preserve the earlier modification.").await;
        print_row(&measurement);
        assert!(rechecked.text.contains(&value), "{}", rechecked.text);
        let goal = restored.goal_definition().unwrap().unwrap();
        assert_eq!(goal.plan_revision, Some(2));
        assert_eq!(
            goal.plan.as_ref().unwrap().steps[0].status,
            TaskStatus::Completed
        );
        assert_eq!(
            goal.plan.as_ref().unwrap().steps[1].status,
            TaskStatus::Pending
        );
        let after =
            a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "goal-step-live")
                .unwrap();
        assert_eq!(&after[..before.len()], before.as_slice());
        let fresh = &after[before.len()..];
        let mut observations = 0;
        for fact in fresh.iter().filter(|f| f.kind == "tool.result") {
            let receipt: a3s_effect::ToolExecutionResultV1 =
                serde_json::from_value(fact.payload["receipt"].clone()).unwrap();
            assert!(
                receipt.effects.changes.is_empty(),
                "the old write must not replay"
            );
            assert!(receipt.output.to_string().contains(&value));
            observations += 1;
        }
        assert!(observations > 0);
        assert_eq!(
            fresh
                .iter()
                .filter(|f| f.kind == "a3s.goal.step.accepted" && f.payload["plan_revision"] == 2)
                .count(),
            1
        );
        assert!(!after.iter().any(|f| f.kind == "a3s.goal.completed"));
        assert_eq!(attestor.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read(workspace.path().join("source.txt")).unwrap(),
            value.as_bytes()
        );
        assert_eq!(
            rechecked.usage.total_tokens,
            recheck_usage
                .0
                .lock()
                .unwrap()
                .iter()
                .map(|u| u.total_tokens)
                .sum::<usize>()
        );
        preview_does_not_execute(&restored).await;
        restored.save().await.unwrap();
        restored.close().await;
        agent.close().await;
        let agent = Agent::from_config(config).await.unwrap();
        let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
        let second = agent
            .resume_session_async(
                "goal-step-live",
                SessionOptions::new().with_session_store(store),
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(second.goal_definition().unwrap().unwrap()).unwrap(),
            serde_json::to_value(goal).unwrap()
        );
        assert_eq!(
            a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "goal-step-live")
                .unwrap(),
            after
        );
        preview_does_not_execute(&second).await;
        second.close().await;
        agent.close().await;
        return;
    }
    restored.close().await;
    agent.close().await;
    assert_eq!(
        attestor.calls.load(Ordering::SeqCst),
        usize::from(modify),
        "reopen must not repeat attestation"
    );
}

fn admit_source_plan(
    session: &a3s_code_core::AgentSession,
    workspace: &std::path::Path,
    modify: bool,
    value: &str,
) {
    let non_goals = if modify {
        "Do not change any file other than source.txt"
    } else {
        "Do not change any file"
    };
    let definition = session
        .apply_goal_definition(GoalDefinitionCommandV1 {
            kind: GoalCommandKindV1::Create,
            import_source: None,
            operation_id: "create".into(),
            goal_id: "verified-source".into(),
            session_id: session.session_id().into(),
            workspace: WorkspaceServices::local(workspace).workspace_ref().clone(),
            goal: AgentGoal::new("Verify the source and then review follow-up requirements")
                .with_criteria(vec![
                    "The current source is independently verified".into(),
                    "Follow-up requirements are reviewed".into(),
                ]),
            non_goals: vec![non_goals.into()],
            scope_fact_sequences: vec![],
            work_policy_digest: None,
            expected_goal_revision: None,
            expected_plan_revision: None,
            expected_goal_sequence: None,
        })
        .unwrap();
    let mut plan = ExecutionPlan::new(
        "Verify the source and review follow-up requirements",
        Complexity::Simple,
    );
    let instruction = if modify {
        format!("Set source.txt to exactly {value}, with no trailing newline, then quote its exact resulting value")
    } else {
        "Read and independently verify source.txt, then quote its exact current value".into()
    };
    plan.upsert_step(Task::new("inspect", instruction).with_success_criteria(
        "The current source value is independently verified and reported exactly",
    ));
    plan.upsert_step(
        Task::new("follow-up", "Review follow-up requirements")
            .with_success_criteria("Follow-up requirements are reviewed")
            .with_dependencies(vec!["inspect".into()]),
    );
    let mut proposal = GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: "plan".into(),
        goal_id: definition.binding.goal_id,
        session_id: definition.binding.session_id,
        workspace: definition.binding.workspace,
        work_policy_digest: None,
        expected_goal_revision: definition.binding.goal_revision,
        expected_plan_revision: definition.plan_revision,
        expected_goal_sequence: definition.goal_sequence,
        plan: Some(plan),
        proposal_sequence: None,
    };
    let proposed = session.apply_goal_plan(proposal.clone()).unwrap();
    proposal.kind = GoalCommandKindV1::AcceptPlan;
    proposal.operation_id = "accept".into();
    proposal.expected_plan_revision = proposed.plan_revision;
    proposal.expected_goal_sequence = proposed.goal_sequence;
    proposal.plan = None;
    proposal.proposal_sequence = proposed.plan_fact_sequence;
    session.apply_goal_plan(proposal).unwrap();
}

#[path = "goal_wait.rs"]
mod goal_wait;
