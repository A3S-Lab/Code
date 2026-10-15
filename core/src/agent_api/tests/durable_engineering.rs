#![cfg(test)]
//! The engineering policy and the durable goal together (Spec 018 T027).
//!
//! Single-axis proofs stay where they are: missing source (`fact_log/durable_source.rs`),
//! two compactions (`compact/source_carry.rs`), stale findings with a policy
//! (`goal_facade/tests/execution/review_binding.rs`), stripped or future versions
//! (`store/session_source/tests.rs`, `loop_checkpoint/source_tests.rs`). This file
//! proves the four modes and the combined counterexamples on both session stores.

use super::*;
use crate::agent_api::{AgentRunSpawn, GoalTickAdmission};
use crate::goal_control::{
    GoalCommandKindV1, GoalContinuationPolicyV1, GoalControlCommandV1, GoalDefinitionCommandV1,
    GoalPlanCommandV1, GoalState, GoalTickDecisionV1,
};
use crate::llm::{LlmClient, Message, ToolDefinition};
use crate::planning::{Complexity, ExecutionPlan, Task, TaskStatus};
use crate::store::{FileSessionStore, MemorySessionStore};
use crate::tools::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use crate::verification::{VerificationCheck, VerificationReport, VerificationStatus};
use crate::WorkPolicyV1;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const SESSION: &str = "durable-engineering";
const SOURCE: &str = "combined source bytes";
const POLICY: &str = "COMBINED-POLICY-保留";
const ACHIEVED: &str = r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#;
const GOAL_TOOLS: [&str; 4] = ["task", "parallel_task", "goal", "goal_tick"];

#[derive(Default)]
struct Client {
    calls: AtomicUsize,
    inputs: Mutex<Vec<String>>,
    tools: Mutex<Vec<Vec<String>>>,
    scripted: Mutex<std::collections::VecDeque<LlmResponse>>,
}

#[async_trait::async_trait]
impl LlmClient for Client {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = messages.iter().map(Message::text).collect::<Vec<_>>();
        self.inputs.lock().unwrap().push(format!(
            "{}\n{}",
            system.unwrap_or_default(),
            text.join("\n")
        ));
        self.tools
            .lock()
            .unwrap()
            .push(tools.iter().map(|tool| tool.name.clone()).collect());
        Ok(self
            .scripted
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                reply(vec![ContentBlock::Text {
                    text: "ready".into(),
                }])
            }))
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let response = self.complete(messages, system, tools).await?;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(StreamEvent::Done(response)).await.unwrap();
        Ok(rx)
    }
}

impl Client {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn push(&self, content: Vec<ContentBlock>) {
        self.scripted.lock().unwrap().push_back(reply(content));
    }

    /// One accepted slice: the counted host check, its answer and the judgment.
    fn slice(&self) {
        self.push(vec![ContentBlock::ToolUse {
            id: format!("verify-{}", self.calls()),
            name: "verify_source".into(),
            input: serde_json::json!({}),
        }]);
        self.push(vec![ContentBlock::Text {
            text: "The source is checked.".into(),
        }]);
        self.push(vec![ContentBlock::Text {
            text: ACHIEVED.into(),
        }]);
    }

    /// How often the policy reaches one model input; the step judge is a
    /// separate model input and never carries it.
    fn policy_in(&self, call: usize) -> usize {
        self.inputs.lock().unwrap()[call].matches(POLICY).count()
    }
}

fn reply(content: Vec<ContentBlock>) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content,
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: crate::llm::TranscriptVisibility::Wire,
        },
        usage: TokenUsage {
            prompt_tokens: 5,
            completion_tokens: 2,
            total_tokens: 7,
            ..Default::default()
        },
        stop_reason: None,
        token_logprobs: vec![],
        meta: None,
    }
}

/// The host's independent source check, counting actual executions.
struct Verify(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl Tool for Verify {
    fn name(&self) -> &str {
        "verify_source"
    }
    fn description(&self) -> &str {
        "Independently verify the fixture source bytes."
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
        self.0.fetch_add(1, Ordering::SeqCst);
        let observed = std::fs::read_to_string(ctx.workspace.join("source.txt"))?;
        let status = if observed == SOURCE {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        };
        let report = VerificationReport::new(
            "host:source-check",
            vec![
                VerificationCheck::required("source", "source-bytes", "Source is checked")
                    .with_status(status),
            ],
        );
        Ok(
            ToolOutput::success(observed).with_metadata(serde_json::json!({
                "verification_author":"host", "verification_report":report,
            })),
        )
    }
}

fn policy() -> WorkPolicyV1 {
    WorkPolicyV1::new("engineering-v1", 1, POLICY).unwrap()
}

fn limits(max_slices: u64) -> GoalContinuationPolicyV1 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    serde_json::from_value(serde_json::json!({
        "automatic_enabled": true, "max_slices": max_slices,
        "max_no_progress_slices": 4, "deadline_unix_ms": now + 600_000,
    }))
    .unwrap()
}

async fn agent(goal_control: Option<GoalContinuationPolicyV1>) -> Agent {
    let mut config = test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    config.goal_control = goal_control;
    Agent::from_config(config).await.unwrap()
}

async fn store(file: bool, dir: &std::path::Path) -> Arc<dyn SessionStore> {
    if file {
        Arc::new(FileSessionStore::new(dir).await.unwrap())
    } else {
        Arc::new(MemorySessionStore::new())
    }
}

/// A reopen gets a fresh file store over the same directory; memory keeps its Arc.
async fn reopen_store(
    file: bool,
    dir: &std::path::Path,
    store: Arc<dyn SessionStore>,
) -> Arc<dyn SessionStore> {
    if file {
        Arc::new(FileSessionStore::new(dir).await.unwrap())
    } else {
        store
    }
}

fn options(
    store: Arc<dyn SessionStore>,
    client: Arc<Client>,
    policy: Option<WorkPolicyV1>,
    goal: bool,
) -> crate::SessionOptions {
    let mut options = crate::SessionOptions::new()
        .with_session_id(SESSION)
        .with_session_store(store)
        .with_llm_client(client)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_continuation(false)
        .with_permission_policy(
            crate::permissions::PermissionPolicy::new().allow_all(&["verify_source(*)", "task"]),
        );
    options = if goal {
        options
            .with_planning_mode(crate::PlanningMode::Enabled)
            .with_goal_tracking(true)
    } else {
        options.with_planning_mode(crate::PlanningMode::Disabled)
    };
    if let Some(policy) = policy {
        options = options.with_work_policy(policy);
    }
    options
}

async fn open(
    agent: &Agent,
    root: &std::path::Path,
    options: crate::SessionOptions,
) -> (AgentSession, Arc<AtomicUsize>) {
    let session = agent
        .session_async(root.display().to_string(), Some(options))
        .await
        .unwrap();
    let executed = Arc::new(AtomicUsize::new(0));
    session
        .register_dynamic_tool(Arc::new(Verify(executed.clone())))
        .unwrap();
    (session, executed)
}

async fn resume(agent: &Agent, options: crate::SessionOptions) -> (AgentSession, Arc<AtomicUsize>) {
    let session = agent.resume_session_async(SESSION, options).await.unwrap();
    let executed = Arc::new(AtomicUsize::new(0));
    session
        .register_dynamic_tool(Arc::new(Verify(executed.clone())))
        .unwrap();
    (session, executed)
}

fn facts(root: &std::path::Path) -> Vec<a3s_effect::Fact> {
    crate::fact_control::read_workspace_facts(root, SESSION).unwrap()
}

fn count(facts: &[a3s_effect::Fact], kind: &str) -> usize {
    facts.iter().filter(|fact| fact.kind == kind).count()
}

/// Create the goal and accept a two-step plan; the second step closes the goal.
fn bind_goal(session: &AgentSession, digest: Option<String>) {
    let created = session
        .apply_goal_definition(GoalDefinitionCommandV1 {
            kind: GoalCommandKindV1::Create,
            import_source: None,
            operation_id: "create".into(),
            goal_id: "combined".into(),
            session_id: SESSION.into(),
            workspace: session
                .tool_context
                .workspace_services
                .workspace_ref()
                .clone(),
            goal: crate::planning::AgentGoal::new("Keep the source verified")
                .with_criteria(vec!["Source is checked".into()]),
            non_goals: vec!["Do not modify source.txt".into()],
            scope_fact_sequences: vec![],
            work_policy_digest: digest,
            expected_goal_revision: None,
            expected_plan_revision: None,
            expected_goal_sequence: None,
        })
        .unwrap();
    let proposed = session
        .apply_goal_plan(propose(&created, "plan", two_steps()))
        .unwrap();
    let mut accept = propose(&proposed, "accept", two_steps());
    accept.kind = GoalCommandKindV1::AcceptPlan;
    accept.plan = None;
    accept.proposal_sequence = proposed.plan_fact_sequence;
    session.apply_goal_plan(accept).unwrap();
}

fn two_steps() -> ExecutionPlan {
    let mut plan = ExecutionPlan::new("Keep the source verified", Complexity::Medium);
    plan.upsert_step(
        Task::new("inspect", "Inspect the source").with_success_criteria("Source is checked"),
    );
    plan.upsert_step(
        Task::new("confirm", "Confirm the source again")
            .with_dependencies(vec!["inspect".into()])
            .with_success_criteria("Source is checked"),
    );
    plan
}

fn propose(
    current: &crate::goal_control::DurableGoalDefinitionV1,
    operation: &str,
    plan: ExecutionPlan,
) -> GoalPlanCommandV1 {
    GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: operation.into(),
        goal_id: current.binding.goal_id.clone(),
        session_id: current.binding.session_id.clone(),
        workspace: current.binding.workspace.clone(),
        work_policy_digest: current.binding.work_policy_digest.clone(),
        expected_goal_revision: current.binding.goal_revision,
        expected_plan_revision: current.plan_revision,
        expected_goal_sequence: current.goal_sequence,
        plan: Some(plan),
        proposal_sequence: None,
    }
}

fn tick(session: &AgentSession, operation: &str) -> GoalControlCommandV1 {
    let goal = session.goal_definition().unwrap().unwrap();
    GoalControlCommandV1 {
        kind: GoalCommandKindV1::Tick,
        operation_id: operation.into(),
        goal_id: goal.binding.goal_id,
        session_id: goal.binding.session_id,
        workspace: goal.binding.workspace,
        work_policy_digest: goal.binding.work_policy_digest,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: goal.plan_revision,
        expected_goal_sequence: goal.goal_sequence,
    }
}

async fn run_slice(session: &AgentSession, operation: &str) {
    let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) =
        session.tick_goal(&tick(session, operation)).await.unwrap()
    else {
        panic!("{operation} admits one Run");
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
}

async fn decided(session: &AgentSession, operation: &str) -> GoalTickDecisionV1 {
    match session.tick_goal(&tick(session, operation)).await.unwrap() {
        GoalTickAdmission::Decided(decision) => decision,
        GoalTickAdmission::Admitted(_) => panic!("{operation} must not admit a Run"),
    }
}

fn step_status(session: &AgentSession) -> Vec<TaskStatus> {
    let goal = session.goal_definition().unwrap().unwrap();
    goal.plan
        .unwrap()
        .steps
        .iter()
        .map(|step| step.status)
        .collect()
}

/// Each mode binds once, works, reopens from the real store and keeps working:
/// the policy reaches every model input exactly once, the goal's first step
/// survives reopen and the second closes it. Reopen itself calls nothing.
async fn prove_modes(file: bool) {
    for (with_policy, with_goal) in [(false, false), (true, false), (false, true), (true, true)] {
        let mode = format!("file={file} policy={with_policy} goal={with_goal}");
        let root = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("source.txt"), SOURCE).unwrap();
        let first_store = store(file, storage.path()).await;
        let client = Arc::new(Client::default());
        let configured = with_policy.then(policy);
        let agent_a = agent(Some(limits(4))).await;
        let (session, executed) = open(
            &agent_a,
            root.path(),
            options(
                first_store.clone(),
                client.clone(),
                configured.clone(),
                with_goal,
            ),
        )
        .await;
        let start = client.calls();
        if with_goal {
            bind_goal(&session, configured.as_ref().map(|p| p.digest().unwrap()));
            client.slice();
            run_slice(&session, "wake-1").await;
            assert_eq!(
                step_status(&session),
                [TaskStatus::Completed, TaskStatus::Pending],
                "{mode}"
            );
            assert_eq!(executed.load(Ordering::SeqCst), 1, "{mode}");
        } else {
            session.send("admit the session", None).await.unwrap();
        }
        assert_eq!(client.policy_in(start), usize::from(with_policy), "{mode}");
        let digest = session.work_policy_digest().unwrap();
        assert_eq!(digest.is_some(), with_policy, "{mode}");
        let definition = serde_json::to_value(session.goal_definition().unwrap()).unwrap();
        session.save().await.unwrap();
        session.close().await;
        agent_a.close().await;

        let calls = client.calls();
        let agent_b = agent(Some(limits(4))).await;
        let reopened = reopen_store(file, storage.path(), first_store).await;
        let (resumed, executed) =
            resume(&agent_b, options(reopened, client.clone(), None, with_goal)).await;
        assert_eq!(client.calls(), calls, "reopen calls nothing: {mode}");
        assert_eq!(resumed.work_policy_digest().unwrap(), digest, "{mode}");
        assert_eq!(
            serde_json::to_value(resumed.goal_definition().unwrap()).unwrap(),
            definition,
            "{mode}"
        );
        if with_goal {
            client.slice();
            run_slice(&resumed, "wake-2").await;
            let goal = resumed.goal_definition().unwrap().unwrap();
            assert_eq!(goal.state, GoalState::Completed, "{mode}");
            assert_eq!(executed.load(Ordering::SeqCst), 1, "{mode}");
            assert_eq!(
                decided(&resumed, "wake-3").await,
                GoalTickDecisionV1::Complete,
                "{mode}"
            );
        } else {
            resumed.send("continue after reopen", None).await.unwrap();
        }
        assert_eq!(
            client.policy_in(calls),
            usize::from(with_policy),
            "the restored policy reaches the next Run once: {mode}"
        );
        assert_eq!(
            count(&facts(root.path()), "a3s.goal.completed"),
            usize::from(with_goal),
            "{mode}"
        );
        resumed.close().await;
        agent_b.close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn four_modes_bind_work_and_reopen_on_the_memory_store() {
    prove_modes(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn four_modes_bind_work_and_reopen_on_the_file_store() {
    prove_modes(true).await;
}

/// Combined: concurrent ticks share one finite budget and admit one Run; the
/// exhausted budget survives reopen; a host that brings a different policy is
/// refused. No refusal calls the model or runs a tool.
async fn prove_budget_and_policy(file: bool) {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), SOURCE).unwrap();
    let first_store = store(file, storage.path()).await;
    let client = Arc::new(Client::default());
    let agent_a = agent(Some(limits(1))).await;
    let (session, executed) = open(
        &agent_a,
        root.path(),
        options(first_store.clone(), client.clone(), Some(policy()), true),
    )
    .await;
    bind_goal(&session, Some(policy().digest().unwrap()));
    client.slice();
    let wake = tick(&session, "wake-1");
    let (left, right) = tokio::join!(session.tick_goal(&wake), session.tick_goal(&wake));
    let mut started = 0;
    for result in [left, right] {
        match result {
            Ok(GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. })) => {
                started += 1;
                worker.await.unwrap();
            }
            Ok(GoalTickAdmission::Admitted(AgentRunSpawn::Replayed { .. }))
            | Ok(GoalTickAdmission::Decided(GoalTickDecisionV1::Reconcile))
            | Err(crate::error::CodeError::SessionBusy { .. }) => {}
            Ok(GoalTickAdmission::Decided(other)) => panic!("unexpected decision {other:?}"),
            Err(error) => panic!("unexpected tick failure: {error}"),
        }
    }
    assert_eq!(started, 1, "one finite budget admits one Run");
    assert_eq!((client.calls(), executed.load(Ordering::SeqCst)), (3, 1));
    assert_eq!(
        decided(&session, "wake-2").await,
        GoalTickDecisionV1::BudgetExhausted
    );
    session.save().await.unwrap();
    session.close().await;
    agent_a.close().await;
    let before = facts(root.path());

    let reopened = reopen_store(file, storage.path(), first_store).await;
    let other = WorkPolicyV1::new("engineering-v1", 2, "A different policy.").unwrap();
    let agent_b = agent(Some(limits(1))).await;
    let Err(refused) = agent_b
        .resume_session_async(
            SESSION,
            options(reopened.clone(), client.clone(), Some(other), true),
        )
        .await
    else {
        panic!("a different policy cannot resume the bound session");
    };
    assert!(
        refused.to_string().contains("work_policy.conflict"),
        "{refused}"
    );
    let (resumed, executed) = resume(&agent_b, options(reopened, client.clone(), None, true)).await;
    assert_eq!(
        decided(&resumed, "wake-3").await,
        GoalTickDecisionV1::BudgetExhausted
    );
    assert_eq!(
        step_status(&resumed),
        [TaskStatus::Completed, TaskStatus::Pending]
    );
    assert_eq!((client.calls(), executed.load(Ordering::SeqCst)), (3, 0));
    assert_eq!(facts(root.path()), before, "refusals append nothing");
    resumed.close().await;
    agent_b.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn combined_budget_race_and_policy_conflict_on_both_stores() {
    for file in [false, true] {
        prove_budget_and_policy(file).await;
    }
}

/// Two hosts race to replan the combined goal at the same revision: the writer
/// admits exactly one; the loser changes nothing and calls nothing.
#[tokio::test(flavor = "multi_thread")]
async fn combined_revision_race_admits_one_plan() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), SOURCE).unwrap();
    let client = Arc::new(Client::default());
    let agent = agent(Some(limits(4))).await;
    let (session, _) = open(
        &agent,
        root.path(),
        options(
            store(true, storage.path()).await,
            client.clone(),
            Some(policy()),
            true,
        ),
    )
    .await;
    bind_goal(&session, Some(policy().digest().unwrap()));
    let current = session.goal_definition().unwrap().unwrap();
    let mut single = ExecutionPlan::new("Keep the source verified", Complexity::Simple);
    single
        .upsert_step(Task::new("only", "Inspect once").with_success_criteria("Source is checked"));
    let left = propose(&current, "replan-left", two_steps());
    let right = propose(&current, "replan-right", single);
    let before = facts(root.path());
    let (left, right) = std::thread::scope(|scope| {
        let left = scope.spawn(|| session.apply_goal_plan(left));
        let right = scope.spawn(|| session.apply_goal_plan(right));
        (left.join().unwrap(), right.join().unwrap())
    });
    let (won, lost) = match (left, right) {
        (Ok(won), Err(lost)) | (Err(lost), Ok(won)) => (won, lost),
        (left, right) => panic!("exactly one revision wins: {left:?} {right:?}"),
    };
    assert!(lost.to_string().contains("goal."), "{lost}");
    let after = facts(root.path());
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(
        count(&after, "a3s.goal.plan.proposed"),
        count(&before, "a3s.goal.plan.proposed") + 1
    );
    assert_eq!(
        session.goal_definition().unwrap().unwrap().goal_sequence,
        won.goal_sequence
    );
    assert_eq!(client.calls(), 0);
    session.close().await;
    agent.close().await;
}

/// A combined slice cut after its tool was prepared leaves an unknown outcome.
/// A host without that Run reconciles nothing, ticks nothing and appends nothing.
#[tokio::test(flavor = "multi_thread")]
async fn combined_unknown_receipt_stays_unknown_without_effect() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), SOURCE).unwrap();
    let storage = tempfile::tempdir().unwrap();
    let client = Arc::new(Client::default());
    let agent_a = agent(Some(limits(4))).await;
    let (session, _) = open(
        &agent_a,
        root.path(),
        options(
            store(false, storage.path()).await,
            client.clone(),
            Some(policy()),
            true,
        ),
    )
    .await;
    bind_goal(&session, Some(policy().digest().unwrap()));
    client.slice();
    run_slice(&session, "wake-1").await;
    session.close().await;
    agent_a.close().await;
    let prepared = facts(root.path())
        .iter()
        .find(|fact| fact.kind == "tool.prepared")
        .unwrap()
        .seq;
    let log =
        a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(root.path(), SESSION))
            .unwrap();
    log.truncate_after(&crate::fact_control::thread_for_session(SESSION), prepared)
        .unwrap();
    drop(log);

    let client = Arc::new(Client::default());
    let agent_b = agent(Some(limits(4))).await;
    let (observer, executed) = open(
        &agent_b,
        root.path(),
        options(
            store(false, storage.path()).await,
            client.clone(),
            None,
            true,
        ),
    )
    .await;
    let before = facts(root.path());
    assert!(crate::fact_control::has_unsettled_effects(&before));
    assert_eq!(
        observer
            .preview_goal_tick(&tick(&observer, "wake-2"))
            .await
            .unwrap(),
        GoalTickDecisionV1::Reconcile
    );
    let error = observer.reconcile_goal_slices().unwrap_err().to_string();
    assert!(error.contains("goal.receipt_unsettled"), "{error}");
    assert_eq!(
        decided(&observer, "wake-2").await,
        GoalTickDecisionV1::Reconcile
    );
    assert_eq!(
        facts(root.path()),
        before,
        "an unknown outcome stays unknown"
    );
    assert_eq!((client.calls(), executed.load(Ordering::SeqCst)), (0, 0));
    observer.close().await;
    agent_b.close().await;
}

/// A child Task inside a combined slice runs under the parent's policy but is
/// offered no goal or delegation tool, and admits no slice of its own.
#[tokio::test(flavor = "multi_thread")]
async fn combined_child_task_gets_the_policy_and_no_goal_tool() {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), SOURCE).unwrap();
    let client = Arc::new(Client::default());
    let agent = agent(Some(limits(1))).await;
    let opts = options(
        store(false, storage.path()).await,
        client.clone(),
        Some(policy()),
        true,
    )
    .with_manual_delegation_enabled(true)
    .with_worker_agent(crate::subagent::WorkerAgentSpec::read_only(
        "worker",
        "Read only",
    ));
    let (session, _) = open(&agent, root.path(), opts).await;
    bind_goal(&session, Some(policy().digest().unwrap()));
    client.push(vec![ContentBlock::ToolUse {
        id: "delegate".into(),
        name: "task".into(),
        input: serde_json::json!({
            "agent": "worker", "description": "read", "prompt": "Reply ready without tools."
        }),
    }]);
    client.push(vec![ContentBlock::Text {
        text: "ready".into(),
    }]);
    client.push(vec![ContentBlock::Text {
        text: "The worker replied ready.".into(),
    }]);
    run_slice(&session, "wake-1").await;
    assert_eq!(client.calls(), 3, "parent, child, parent");
    let parent_tools = client.tools.lock().unwrap()[0].clone();
    assert!(
        parent_tools.iter().any(|name| name == "task"),
        "{parent_tools:?}"
    );
    let child_tools = client.tools.lock().unwrap()[1].clone();
    assert!(
        !child_tools
            .iter()
            .any(|name| GOAL_TOOLS.contains(&name.as_str())),
        "{child_tools:?}"
    );
    assert_eq!(client.inputs.lock().unwrap()[1].matches(POLICY).count(), 1);
    let after = facts(root.path());
    assert_eq!(
        count(&after, "a3s.goal.slice.admitted"),
        1,
        "the child admits no slice"
    );
    assert_eq!(
        decided(&session, "wake-2").await,
        GoalTickDecisionV1::BudgetExhausted
    );
    assert_eq!(client.calls(), 3);
    session.close().await;
    agent.close().await;
}
