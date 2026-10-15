//! Paired ordinary / engineering / goal / combined attempts on the real model
//! (Spec 018 T030), preregistered in
//! `docs/evidence/code-harness/20261013-durable-engineering-m1/t030-preregistration.md`.
//!
//! One primary attempt per frozen T003 task and arm, with identical model,
//! permissions, contract and limits. The engineering sensitivity branch (short
//! rule, policy plus navigation) runs on the development tasks and is reported
//! separately. Every attempt is printed as one `PAIRED` line; only product
//! invariants are asserted. A measurement suite: no arm is declared better
//! while HC-ACCEPT1 is open and the host refuses native execution.
//!
//! ```text
//! cargo +1.97.1 test -p a3s-code-core --test test_durable_engineering_paired_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

// Shared with the calibration target.
#[allow(dead_code)]
#[path = "fixtures/durable-engineering-v1/fixtures.rs"]
mod fixtures;
mod support;

use a3s_code_core::budget::BudgetGuard;
use a3s_code_core::context::RecentWorkspaceFilesContextProvider;
use a3s_code_core::goal_control::{
    GoalCommandKindV1, GoalControlCommandV1, GoalDefinitionCommandV1, GoalPlanCommandV1, GoalState,
};
use a3s_code_core::harness_loop::CompletionTerminal;
use a3s_code_core::llm::TokenUsage;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::planning::{AgentGoal, Complexity, ExecutionPlan, Task};
use a3s_code_core::workspace::{ManifestWorkspaceBackend, WorkspaceServices};
use a3s_code_core::{
    Agent, AgentEvent, AgentSession, CodeConfig, GoalTickStream, PlanningMode, SessionOptions,
    WorkPolicyV1,
};
use fixtures::Fixture;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::layer_c_model::load_pinned_layer_c_config;

const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(600);
const RUN: &str = "attempt";
const ENGINEERING_POLICY: &str = include_str!("../../acp/prompts/engineering.md");
const SHORT_POLICY: &str = "Engineering discipline (short). Make the smallest complete change. Read the affected code and every caller first. Reuse existing code. Fix the root cause and update all callers. Keep protected files and checks unchanged. Verify with the existing check and report what was actually run.";
const NATIVE_REFUSAL: &str = "native_process_lifetime_unqualified";

#[derive(Clone, Copy, PartialEq)]
enum Arm {
    Ordinary,
    Engineering,
    Goal,
    Combined,
    Short,
    Navigation,
}

impl Arm {
    fn name(self) -> &'static str {
        match self {
            Arm::Ordinary => "ordinary",
            Arm::Engineering => "engineering",
            Arm::Goal => "goal",
            Arm::Combined => "combined",
            Arm::Short => "short",
            Arm::Navigation => "navigation",
        }
    }

    fn policy(self) -> Option<WorkPolicyV1> {
        match self {
            Arm::Ordinary | Arm::Goal => None,
            Arm::Short => Some(WorkPolicyV1::new("engineering-short", 1, SHORT_POLICY).unwrap()),
            _ => Some(WorkPolicyV1::new("engineering-v1", 1, ENGINEERING_POLICY).unwrap()),
        }
    }

    fn goal(self) -> bool {
        matches!(self, Arm::Goal | Arm::Combined)
    }
}

#[derive(Default)]
struct UsageProbe(Mutex<Vec<TokenUsage>>);

#[async_trait::async_trait]
impl BudgetGuard for UsageProbe {
    async fn record_after_llm(&self, _: &str, usage: &TokenUsage) {
        self.0.lock().unwrap().push(usage.clone());
    }
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
            max_slices = "1"
            max_no_progress_slices = "1"
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

async fn options(
    fixture: &Fixture,
    arm: Arm,
    root: &Path,
    guard: Arc<UsageProbe>,
) -> SessionOptions {
    let mut rules = ["ls(*)", "read(*)", "search(*)", "grep(*)", "glob(*)"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    rules.push("bash(node test.mjs)".into());
    for path in fixture.writable {
        rules.push(format!("write({path})"));
        rules.push(format!("edit({path})"));
    }
    let rules = rules.iter().map(String::as_str).collect::<Vec<_>>();
    let mut permissions = PermissionPolicy::new().allow_all(&rules);
    permissions.default_decision = PermissionDecision::Deny;
    let mut options = SessionOptions::new()
        .with_session_id(format!("paired-{}-{}", fixture.id, arm.name()))
        .with_permission_policy(permissions)
        .with_default_security()
        .with_planning_mode(if arm.goal() {
            PlanningMode::Enabled
        } else {
            PlanningMode::Disabled
        })
        .with_goal_tracking(arm.goal())
        .with_auto_compact(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_continuation(false)
        .with_max_tool_rounds(20)
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
        );
    if let Some(policy) = arm.policy() {
        options = options.with_work_policy(policy);
    }
    if arm == Arm::Navigation {
        let backend = ManifestWorkspaceBackend::new_deferred(root);
        let manifest = backend.manifest();
        let mut updates = manifest.subscribe();
        assert!(manifest.activate());
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let snapshot = updates.recv().await.unwrap();
                if fixture
                    .writable
                    .iter()
                    .all(|path| snapshot.files.iter().any(|file| file.path == *path))
                {
                    break;
                }
            }
        })
        .await
        .expect("manifest snapshot");
        options = options
            .with_workspace_backend(WorkspaceServices::local_with_manifest_backend(backend))
            .with_context_provider(Arc::new(RecentWorkspaceFilesContextProvider::new(manifest)));
    }
    options
}

/// Bind the request as a goal with one accepted step; return the tick.
fn bind_goal(
    session: &AgentSession,
    fixture: &Fixture,
    root: &Path,
    digest: Option<String>,
) -> GoalControlCommandV1 {
    let criterion = format!(
        "The original `node test.mjs` passes and prints ENGINEERING_{}_OK",
        fixture.id.to_ascii_uppercase()
    );
    let definition = session
        .apply_goal_definition(GoalDefinitionCommandV1 {
            kind: GoalCommandKindV1::Create,
            import_source: None,
            operation_id: "create".into(),
            goal_id: format!("paired-{}", fixture.id),
            session_id: session.session_id().into(),
            workspace: WorkspaceServices::local(root).workspace_ref().clone(),
            goal: AgentGoal::new(fixture.request).with_criteria(vec![criterion.clone()]),
            non_goals: vec!["Do not change protected files or checks".into()],
            scope_fact_sequences: vec![],
            work_policy_digest: digest,
            expected_goal_revision: None,
            expected_plan_revision: None,
            expected_goal_sequence: None,
        })
        .unwrap();
    let mut plan = ExecutionPlan::new(fixture.request, Complexity::Simple);
    plan.upsert_step(Task::new("task", fixture.request).with_success_criteria(criterion));
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
    let goal = session.apply_goal_plan(proposal).unwrap();
    GoalControlCommandV1 {
        kind: GoalCommandKindV1::Tick,
        operation_id: RUN.into(),
        goal_id: goal.binding.goal_id,
        session_id: goal.binding.session_id,
        workspace: goal.binding.workspace,
        work_policy_digest: goal.binding.work_policy_digest,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
    }
}

/// Which frozen reference the writable files equal.
fn stage(fixture: &Fixture, root: &Path) -> &'static str {
    let bytes = |edits: &[(&str, &str)]| {
        fixture
            .writable
            .iter()
            .map(|path| {
                edits
                    .iter()
                    .chain(fixture.files)
                    .find(|(file, _)| file == path)
                    .map(|(_, body)| body.as_bytes().to_vec())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
    };
    let now = fixture
        .writable
        .iter()
        .map(|path| std::fs::read(root.join(path)).unwrap_or_default())
        .collect::<Vec<_>>();
    if now == bytes(&[]) {
        "original"
    } else if now == bytes(fixture.correct) {
        "correct"
    } else if now == bytes(fixture.incorrect) {
        "incorrect"
    } else {
        "other"
    }
}

async fn attempt(fixture: &'static Fixture, arm: Arm, partition: &str) {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    fixture.seed(root);
    let guard = Arc::new(UsageProbe::default());
    let agent = Agent::from_config(config()).await.unwrap();
    let session = agent
        .session_async(
            root.display().to_string(),
            Some(options(fixture, arm, root, guard.clone()).await),
        )
        .await
        .unwrap();
    let started = Instant::now();
    // The live stream, not retained Run events: retention trims a long Run's
    // early tool events.
    let stream = if arm.goal() {
        let digest = arm.policy().map(|policy| policy.digest().unwrap());
        let tick = bind_goal(&session, fixture, root, digest);
        match session.stream_goal_tick(&tick).await.unwrap() {
            GoalTickStream::Started(stream) => stream,
            GoalTickStream::Decided(decision) => {
                panic!("a bound ready step must advance, got {decision:?}")
            }
            GoalTickStream::Replayed(_) => panic!("a fresh attempt must not replay"),
        }
    } else {
        session
            .stream_with_run(fixture.request, None)
            .await
            .unwrap()
    };
    let run_id = stream.run.as_ref().map(|run| run.id().to_string());
    let mut events = stream.events;
    let mut received = Vec::new();
    let timed_out = tokio::time::timeout(ATTEMPT_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            received.push(event);
        }
    })
    .await
    .is_err();
    if timed_out {
        stream.worker.abort();
    } else {
        let _ = stream.worker.await;
    }
    let secs = started.elapsed().as_secs_f64();

    let marker = format!("ENGINEERING_{}_OK", fixture.id.to_ascii_uppercase());
    let (mut executed, mut effects, mut denied, mut intervention) = (0, 0, 0, 0);
    let (mut passed_check, mut native_refused) = (false, false);
    let mut completion = None;
    let mut error = timed_out.then(|| format!("timed out after {ATTEMPT_TIMEOUT:?}"));
    for event in received {
        match event {
            AgentEvent::ToolExecutionStart { .. } => executed += 1,
            AgentEvent::PermissionDenied { reason, .. } => {
                denied += 1;
                intervention += usize::from(reason.contains("requires confirmation"));
                native_refused |= reason.contains(NATIVE_REFUSAL);
            }
            AgentEvent::ToolEnd {
                name,
                exit_code,
                output,
                ..
            } => {
                effects += usize::from(matches!(name.as_str(), "write" | "edit") && exit_code == 0);
                passed_check |= name == "bash" && exit_code == 0 && output.contains(&marker);
                native_refused |= output.contains(NATIVE_REFUSAL);
            }
            AgentEvent::End {
                completion_terminal,
                ..
            } => completion = Some(completion_terminal),
            AgentEvent::Error { message } => error = Some(message),
            _ => {}
        }
    }
    let status = match run_id {
        Some(id) => session
            .run_snapshot(&id)
            .await
            .map(|snapshot| format!("{:?}", snapshot.status))
            .unwrap_or_default(),
        None => String::new(),
    };
    let goal_state = arm
        .goal()
        .then(|| session.goal_definition().unwrap().map(|goal| goal.state))
        .flatten();
    let verified = matches!(completion, Some(CompletionTerminal::Verified { .. }));
    let strict = if arm.goal() {
        goal_state == Some(GoalState::Completed)
    } else {
        verified
    };
    let edit = stage(fixture, root);
    let protected = fixture.protected_bytes_match(root);
    let positive = if strict && passed_check && edit == "correct" && protected {
        "passed"
    } else if native_refused {
        "pending(native_process_lifetime_unqualified)"
    } else {
        "failed"
    };
    let usage = guard.0.lock().unwrap().clone();
    eprintln!(
        "PAIRED task={} partition={partition} arm={} secs={secs:.1} calls={} prompt={} completion={} total={} executed={executed} effects={effects} no_change={} denied={denied} intervention={intervention} stage={edit} protected={protected} passed_check={passed_check} strict={strict} goal={goal_state:?} status={status} native_refused={native_refused} error={:?} positive={positive}",
        fixture.id,
        arm.name(),
        usage.len(),
        usage.iter().map(|u| u.prompt_tokens).sum::<usize>(),
        usage.iter().map(|u| u.completion_tokens).sum::<usize>(),
        usage.iter().map(|u| u.total_tokens).sum::<usize>(),
        executed - effects + denied,
        error.as_deref().map(|e| e.chars().take(160).collect::<String>()),
    );
    session.close().await;
    agent.close().await;

    assert!(!usage.is_empty(), "a real provider call happened");
    assert!(protected, "protected files and checks stay byte-identical");
    assert!(
        !verified || passed_check,
        "Verified requires a passing check"
    );
    assert!(
        goal_state != Some(GoalState::Completed) || passed_check,
        "a completed goal requires a passing check"
    );
}

async fn primary(fixture: &'static Fixture, partition: &str) {
    for arm in [Arm::Ordinary, Arm::Engineering, Arm::Goal, Arm::Combined] {
        attempt(fixture, arm, partition).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn paired_reuse_real_model_four_arms() {
    primary(&fixtures::REUSE, "development").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn paired_callers_real_model_four_arms() {
    primary(&fixtures::CALLERS, "development").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn paired_boundary_real_model_four_arms() {
    primary(&fixtures::BOUNDARY, "held-out").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn paired_scope_real_model_four_arms() {
    primary(&fixtures::SCOPE, "held-out").await;
}

/// Development tasks only; the full-body arm is the primary engineering attempt.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn sensitivity_engineering_short_rule_and_navigation_on_development_tasks() {
    for fixture in [&fixtures::REUSE, &fixtures::CALLERS] {
        for arm in [Arm::Short, Arm::Navigation] {
            attempt(fixture, arm, "development").await;
        }
    }
}
