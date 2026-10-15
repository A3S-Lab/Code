//! The engineering policy and a durable goal on one long task, real model
//! (Spec 018 T029).
//!
//! The frozen caller-closure project (T003) runs as a bound goal under the
//! engineering policy through original automatic ticks: retain the failing
//! check, repair, verify. A slice is interrupted through goal pause, the
//! process state is saved and a new Agent reopens it from the file store; two
//! original compactions run between later slices.
//!
//! Asserted on every host: a real provider call; shared files and the check
//! stay byte-identical; the policy digest stays bound to the goal across
//! reopen and both compactions; every admitted slice settles (or reconcile
//! refuses an unknown receipt); a used operation id starts nothing; and a
//! completed goal requires the full reference repair and a passing original
//! check. The model's progress is reported. The edit stage is read from the
//! frozen staged references, never by running model-edited code outside the
//! original tool boundary. Where the host refuses native execution the
//! refusal is recorded and the positive stays pending; Spec 007 independent
//! acceptance needs a qualified native candidate host.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +1.97.1 test -p a3s-code-core --test test_durable_goal_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

// Shared with the calibration target; this suite uses one project.
#[allow(dead_code)]
#[path = "fixtures/durable-engineering-v1/fixtures.rs"]
mod fixtures;
mod support;

use a3s_code_core::budget::BudgetGuard;
use a3s_code_core::goal_control::{
    DurableGoalDefinitionV1, GoalCommandKindV1, GoalControlCommandV1, GoalDefinitionCommandV1,
    GoalPlanCommandV1, GoalState, GoalTickDecisionV1,
};
use a3s_code_core::llm::TokenUsage;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::planning::{AgentGoal, Complexity, ExecutionPlan, Task};
use a3s_code_core::store::FileSessionStore;
use a3s_code_core::workspace::WorkspaceServices;
use a3s_code_core::{
    Agent, AgentEvent, AgentRunSpawn, AgentSession, CodeConfig, GoalTickAdmission, PlanningMode,
    SessionOptions,
};
use fixtures::CALLERS;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::layer_c_model::load_pinned_layer_c_config;

/// Preregistered limits (fixtures README): 600 s per slice, bounded rounds.
const SLICE_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_SLICES: u64 = 5;
const SESSION: &str = "durable-goal-live";
const ENGINEERING_POLICY: &str = include_str!("../../acp/prompts/engineering.md");
const NATIVE_REFUSAL: &str = "native_process_lifetime_unqualified";
const MARKER: &str = "ENGINEERING_CALLERS_OK";

#[derive(Default)]
struct UsageProbe(Mutex<Vec<TokenUsage>>);

#[async_trait::async_trait]
impl BudgetGuard for UsageProbe {
    async fn record_after_llm(&self, _: &str, usage: &TokenUsage) {
        self.0.lock().unwrap().push(usage.clone());
    }
}

/// What one slice did, read from its original Run events.
#[derive(Default, Debug)]
struct Slice {
    id: String,
    secs: f64,
    status: String,
    tools: Vec<String>,
    denied: Vec<String>,
    passed_check: bool,
    error: Option<String>,
}

fn config() -> CodeConfig {
    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 3_600_000;
    config.goal_control = CodeConfig::from_acl(&format!(
        r#"goal_control {{
            automatic_enabled = true
            max_slices = "{MAX_SLICES}"
            max_no_progress_slices = "3"
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
    config
}

fn policy() -> a3s_code_core::WorkPolicyV1 {
    a3s_code_core::WorkPolicyV1::new("engineering-v1", 1, ENGINEERING_POLICY).unwrap()
}

async fn options(store: &Path, guard: Arc<UsageProbe>) -> SessionOptions {
    let mut rules = vec!["ls(*)", "read(*)", "search(*)", "grep(*)", "glob(*)"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    rules.push("bash(node test.mjs)".into());
    for path in CALLERS.writable {
        rules.push(format!("write({path})"));
        rules.push(format!("edit({path})"));
    }
    let rules = rules.iter().map(String::as_str).collect::<Vec<_>>();
    let mut permissions = PermissionPolicy::new().allow_all(&rules);
    permissions.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_store(Arc::new(FileSessionStore::new(store).await.unwrap()))
        .with_permission_policy(permissions)
        .with_default_security()
        .with_planning_mode(PlanningMode::Enabled)
        .with_goal_tracking(true)
        .with_auto_compact(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_continuation(false)
        .with_max_tool_rounds(12)
        .with_llm_api_timeout(180_000)
        .with_temperature(0.0)
        .with_budget_guard(guard)
        .with_verification_contract(
            a3s_code_core::verification::VerificationContractV1::new(
                "node-suite",
                "node test.mjs",
                ["src/**"],
            )
            .unwrap(),
        )
        .with_work_policy(policy())
}

fn define(session: &AgentSession, root: &Path) -> DurableGoalDefinitionV1 {
    let definition = session
        .apply_goal_definition(GoalDefinitionCommandV1 {
            kind: GoalCommandKindV1::Create,
            import_source: None,
            operation_id: "create".into(),
            goal_id: "restore-callers".into(),
            session_id: SESSION.into(),
            workspace: WorkspaceServices::local(root).workspace_ref().clone(),
            goal: AgentGoal::new(fixtures::DURABLE_REQUEST).with_criteria(vec![
                "Both product callers pass the complete record to the shared label API".into(),
                format!("The original `node test.mjs` passes and prints {MARKER}"),
            ]),
            non_goals: vec!["Do not change SPEC.md, lib/label.mjs or test.mjs".into()],
            scope_fact_sequences: vec![],
            work_policy_digest: Some(policy().digest().unwrap()),
            expected_goal_revision: None,
            expected_plan_revision: None,
            expected_goal_sequence: None,
        })
        .unwrap();
    let mut plan = ExecutionPlan::new(fixtures::DURABLE_REQUEST, Complexity::Medium);
    plan.upsert_step(
        Task::new(
            "baseline",
            "Run exactly `node test.mjs` before editing and retain its failing output",
        )
        .with_success_criteria("The original check's failing result is on record"),
    );
    plan.upsert_step(
        Task::new(
            "repair",
            "Make src/service.mjs and src/cli.mjs pass the complete record to label; change no other file",
        )
        .with_dependencies(vec!["baseline".into()])
        .with_success_criteria("Both product callers pass the complete record"),
    );
    plan.upsert_step(
        Task::new(
            "verify",
            format!("Run exactly `node test.mjs` and require it to print {MARKER}"),
        )
        .with_dependencies(vec!["repair".into()])
        .with_success_criteria(format!("The original check passes and prints {MARKER}")),
    );
    let mut proposal = GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: "plan".into(),
        goal_id: definition.binding.goal_id,
        session_id: definition.binding.session_id,
        workspace: definition.binding.workspace,
        work_policy_digest: definition.binding.work_policy_digest,
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
    session.apply_goal_plan(proposal).unwrap()
}

fn command(session: &AgentSession, kind: GoalCommandKindV1, op: &str) -> GoalControlCommandV1 {
    let goal = session.goal_definition().unwrap().unwrap();
    GoalControlCommandV1 {
        kind,
        operation_id: op.into(),
        goal_id: goal.binding.goal_id,
        session_id: goal.binding.session_id,
        workspace: goal.binding.workspace,
        work_policy_digest: goal.binding.work_policy_digest,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
    }
}

/// The slice reached a tool boundary. Run events, not the fact log: opening
/// the log from outside while the Run holds it fails that Run closed.
async fn tool_boundary(session: &AgentSession, op: &str) -> bool {
    session.run_events(op).await.iter().any(|record| {
        matches!(
            record.event,
            AgentEvent::ToolEnd { .. } | AgentEvent::PermissionDenied { .. }
        )
    })
}

/// One automatic tick. With `interrupt`, pause the goal once the slice has
/// reached a tool boundary (the goal's own interrupt path).
async fn tick(
    session: &AgentSession,
    op: &str,
    interrupt: bool,
) -> Result<Slice, GoalTickDecisionV1> {
    let started = Instant::now();
    let worker = match session
        .tick_goal(&command(session, GoalCommandKindV1::Tick, op))
        .await
        .unwrap()
    {
        GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) => worker,
        GoalTickAdmission::Admitted(AgentRunSpawn::Replayed { .. }) => {
            panic!("a fresh operation id must not replay")
        }
        GoalTickAdmission::Decided(decision) => return Err(decision),
    };
    let mut interrupted = false;
    if interrupt {
        let deadline = Instant::now() + Duration::from_secs(300);
        while !worker.is_finished()
            && !tool_boundary(session, op).await
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // A slice recorded after these refs were read conflicts; re-read once.
        for _ in 0..2 {
            let pause = command(session, GoalCommandKindV1::Pause, &format!("{op}-pause"));
            if session
                .control_goal(pause, Duration::from_secs(30))
                .await
                .is_ok()
            {
                interrupted = true;
                break;
            }
        }
        assert!(interrupted, "the goal pause must settle the running slice");
    }
    tokio::time::timeout(SLICE_TIMEOUT, worker)
        .await
        .expect("slice timed out")
        .unwrap();
    let mut slice = Slice {
        id: op.into(),
        secs: started.elapsed().as_secs_f64(),
        status: session
            .run_snapshot(op)
            .await
            .map(|s| {
                format!(
                    "{:?}{}",
                    s.status,
                    if interrupted { "(paused)" } else { "" }
                )
            })
            .unwrap_or_default(),
        ..Default::default()
    };
    for record in session.run_events(op).await {
        match record.event {
            AgentEvent::ToolExecutionStart { name, .. } => slice.tools.push(name),
            AgentEvent::PermissionDenied {
                tool_name,
                args,
                reason,
                ..
            } => slice.denied.push(format!("{tool_name} {args}: {reason}")),
            AgentEvent::ToolEnd {
                name,
                exit_code,
                output,
                ..
            } if name == "bash" && exit_code == 0 && output.contains(MARKER) => {
                slice.passed_check = true
            }
            AgentEvent::Error { message } => slice.error = Some(message),
            _ => {}
        }
    }
    eprintln!(
        "DURABLE-GOAL slice id={} secs={:.1} status={} tools={:?} denied={} passed_check={} error={:?}",
        slice.id,
        slice.secs,
        slice.status,
        slice.tools,
        slice.denied.len(),
        slice.passed_check,
        slice.error
    );
    Ok(slice)
}

/// Which frozen staged reference the writable files equal.
fn stage(root: &Path) -> &'static str {
    let bytes = |edits: &[(&str, &str)]| {
        CALLERS
            .writable
            .iter()
            .map(|path| {
                edits
                    .iter()
                    .chain(CALLERS.files)
                    .find(|(file, _)| file == path)
                    .map(|(_, body)| body.as_bytes().to_vec())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
    };
    let now = CALLERS
        .writable
        .iter()
        .map(|path| std::fs::read(root.join(path)).unwrap_or_default())
        .collect::<Vec<_>>();
    if now == bytes(&[]) {
        "original"
    } else if now == bytes(CALLERS.incorrect) {
        "partial"
    } else if now == bytes(CALLERS.correct) {
        "full"
    } else {
        "other"
    }
}

fn goal_view(session: &AgentSession) -> serde_json::Value {
    serde_json::to_value(session.goal_definition().unwrap().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn durable_goal_real_model_continues_one_policy_goal_across_reopen_and_compaction() {
    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let root = workspace.path();
    CALLERS.seed(root);
    let digest = policy().digest().unwrap();
    let guard = Arc::new(UsageProbe::default());
    let started = Instant::now();
    let mut slices = Vec::new();
    let mut decisions = Vec::new();

    let agent = Agent::from_config(config()).await.unwrap();
    let session = agent
        .session_async(
            root.display().to_string(),
            Some(
                options(storage.path(), guard.clone())
                    .await
                    .with_session_id(SESSION),
            ),
        )
        .await
        .unwrap();
    let defined = define(&session, root);
    assert_eq!(
        defined.binding.work_policy_digest.as_deref(),
        Some(digest.as_str()),
        "the goal binds the admitted engineering policy"
    );
    match tick(&session, "slice-1", false).await {
        Ok(slice) => slices.push(slice),
        Err(decision) => panic!("the first ready step must advance, got {decision:?}"),
    }
    match tick(&session, "slice-2", true).await {
        Ok(slice) => slices.push(slice),
        Err(decision) => decisions.push(format!("slice-2={decision:?}")),
    }
    let before_reopen = goal_view(&session);
    session.save().await.unwrap();
    session.close().await;
    agent.close().await;

    let agent = Agent::from_config(config()).await.unwrap();
    let session = agent
        .resume_session_async(SESSION, options(storage.path(), guard.clone()).await)
        .await
        .unwrap();
    assert_eq!(
        goal_view(&session),
        before_reopen,
        "reopen restores the goal"
    );
    assert_eq!(session.work_policy_digest().unwrap(), Some(digest.clone()));
    let reconciled = session.reconcile_goal_slices().map_err(|e| e.to_string());
    eprintln!("DURABLE-GOAL reopen reconcile={reconciled:?}");
    let compacted = goal_view(&session);
    session.compact_conversation(None).await.unwrap();
    assert_eq!(goal_view(&session), compacted, "compaction keeps the goal");
    session
        .control_goal(
            command(&session, GoalCommandKindV1::Resume, "resume"),
            Duration::from_secs(30),
        )
        .await
        .unwrap();

    let mut compactions = 1;
    for n in 3..=MAX_SLICES + 1 {
        let op = format!("slice-{n}");
        let outcome = tick(&session, &op, false).await;
        if compactions == 1 {
            let kept = goal_view(&session);
            session.compact_conversation(None).await.unwrap();
            assert_eq!(goal_view(&session), kept, "compaction keeps the goal");
            compactions += 1;
        }
        match outcome {
            Ok(slice) => slices.push(slice),
            Err(GoalTickDecisionV1::Reconcile) => {
                let result = session.reconcile_goal_slices().map_err(|e| e.to_string());
                decisions.push(format!("{op}=Reconcile({result:?})"));
                if result.is_err() {
                    break;
                }
                continue;
            }
            Err(decision) => {
                decisions.push(format!("{op}={decision:?}"));
                break;
            }
        }
    }
    let unsettled = session.reconcile_goal_slices().err().map(|e| e.to_string());

    let goal = session.goal_definition().unwrap().unwrap();
    let facts = a3s_code_core::fact_control::read_workspace_facts(
        root,
        &a3s_code_core::fact_control::thread_for_session(SESSION),
    )
    .unwrap();
    let count = |kind: &str| facts.iter().filter(|f| f.kind == kind).count();
    let admitted = count("a3s.goal.slice.admitted");
    let settled = count("a3s.goal.slice.settled");
    let replay = session
        .tick_goal(&command(&session, GoalCommandKindV1::Tick, "slice-1"))
        .await;
    let usage = guard.0.lock().unwrap().clone();
    let protected = CALLERS.protected_bytes_match(root);
    let edit_stage = stage(root);
    let passed_check = slices.iter().any(|s| s.passed_check);
    let native_refused = slices
        .iter()
        .flat_map(|s| &s.denied)
        .any(|reason| reason.contains(NATIVE_REFUSAL));
    let positive = if goal.state == GoalState::Completed && passed_check && edit_stage == "full" {
        "passed"
    } else if native_refused {
        "pending(native_process_lifetime_unqualified)"
    } else {
        "failed(goal not completed by a passing original check)"
    };
    eprintln!(
        "DURABLE-GOAL secs={:.1} calls={} total={} slices={} admitted={admitted} settled={settled} unsettled={unsettled:?} decisions={decisions:?} compactions={compactions} state={:?} steps={:?} stage={edit_stage} passed_check={passed_check} native_refused={native_refused} protected={protected} positive={positive}",
        started.elapsed().as_secs_f64(),
        usage.len(),
        usage.iter().map(|u| u.total_tokens).sum::<usize>(),
        slices.len(),
        goal.state,
        goal.plan
            .as_ref()
            .map(|plan| plan.steps.iter().map(|s| format!("{}={:?}", s.id, s.status)).collect::<Vec<_>>()),
    );
    for denied in slices.iter().flat_map(|s| &s.denied) {
        eprintln!("DURABLE-GOAL denied {denied}");
    }
    session.close().await;
    agent.close().await;

    assert!(!usage.is_empty(), "a real provider call happened");
    assert!(protected, "shared files and the check stay byte-identical");
    assert_eq!(
        goal.binding.work_policy_digest,
        Some(digest),
        "the policy stays bound"
    );
    assert_eq!(compactions, 2, "two original compactions ran");
    assert!(
        settled >= admitted || unsettled.is_some(),
        "every admitted slice settles unless reconcile refuses an unknown receipt"
    );
    assert!(
        !matches!(
            replay,
            Ok(GoalTickAdmission::Admitted(AgentRunSpawn::Started { .. }))
        ),
        "a used operation id starts nothing"
    );
    assert!(
        goal.state != GoalState::Completed || (passed_check && edit_stage == "full"),
        "a completed goal requires the full repair and a passing original check"
    );
}
