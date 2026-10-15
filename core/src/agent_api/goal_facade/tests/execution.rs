#![cfg(test)]

use super::*;
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition};
use crate::Agent;
use std::sync::{atomic::AtomicUsize, Mutex};

mod acceptance;
mod attestation;
mod automatic;
mod completion;
mod continuation;
mod control;
mod cumulative;
mod evidence;
mod review_binding;
mod settlement;
mod tick;
mod waits;

type ResponseHook = Box<dyn Fn(usize) + Send>;

#[derive(Default)]
struct GoalClient {
    calls: AtomicUsize,
    inputs: Mutex<Vec<String>>,
    scripted: Mutex<std::collections::VecDeque<LlmResponse>>,
    before_response: Mutex<Option<ResponseHook>>,
}

#[async_trait::async_trait]
impl LlmClient for GoalClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(hook) = self.before_response.lock().unwrap().as_ref() {
            hook(call);
        }
        self.inputs.lock().unwrap().push(format!(
            "{}\n{}",
            system.unwrap_or_default(),
            messages
                .iter()
                .map(Message::text)
                .collect::<Vec<_>>()
                .join("\n")
        ));
        Ok(self
            .scripted
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(LlmResponse {
                message: Message::assistant("ready"),
                usage: TokenUsage::default(),
                stop_reason: None,
                token_logprobs: vec![],
                meta: None,
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

async fn fixture(
    root: &std::path::Path,
    accepted: bool,
    opts: crate::SessionOptions,
) -> (Agent, AgentSession, Arc<GoalClient>) {
    fixture_with(root, accepted, opts, None).await
}

fn fixture_config(
    goal_control: Option<crate::goal_control::GoalContinuationPolicyV1>,
) -> crate::config::CodeConfig {
    let mut config = test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    config.goal_control = goal_control;
    config
}

async fn fixture_with(
    root: &std::path::Path,
    accepted: bool,
    opts: crate::SessionOptions,
    goal_control: Option<crate::goal_control::GoalContinuationPolicyV1>,
) -> (Agent, AgentSession, Arc<GoalClient>) {
    let agent = Agent::from_config(fixture_config(goal_control))
        .await
        .unwrap();
    let client = Arc::new(GoalClient::default());
    let session = agent
        .build_session(
            root.display().to_string(),
            client.clone(),
            &opts
                .clone()
                .with_planning_mode(crate::PlanningMode::Enabled)
                .with_goal_tracking(true),
        )
        .unwrap();
    let definition = session
        .apply_goal_definition(command(
            &session,
            opts.work_policy
                .as_ref()
                .map(crate::WorkPolicyV1::digest)
                .transpose()
                .unwrap(),
        ))
        .unwrap();
    let mut plan = crate::planning::ExecutionPlan::new(
        "Repair existing callers",
        crate::planning::Complexity::Medium,
    );
    plan.upsert_step(
        crate::planning::Task::new("inspect", "Inspect the original source")
            .with_success_criteria("Original source is checked"),
    );
    plan.upsert_step(
        crate::planning::Task::new("verify", "Verify the repair")
            .with_dependencies(vec!["inspect".into()])
            .with_success_criteria("All original checks pass"),
    );
    let mut proposal = crate::goal_control::GoalPlanCommandV1 {
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
    if accepted {
        proposal.kind = GoalCommandKindV1::AcceptPlan;
        proposal.operation_id = "accept".into();
        proposal.expected_plan_revision = proposed.plan_revision;
        proposal.expected_goal_sequence = proposed.goal_sequence;
        proposal.plan = None;
        proposal.proposal_sequence = proposed.plan_fact_sequence;
        session.apply_goal_plan(proposal).unwrap();
    }
    (agent, session, client)
}

#[tokio::test]
async fn accepted_goal_plan_binds_the_original_run_without_another_planner() {
    let root = tempfile::tempdir().unwrap();
    let (agent, session, client) = fixture(root.path(), true, options("goal-run")).await;
    let source = session.goal_definition().unwrap().unwrap();
    let result = session
        .send("Continue the admitted current step.", None)
        .await
        .unwrap();
    assert_eq!(result.text, "ready");
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        1,
        "no second goal extractor or planner"
    );
    let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let bindings = facts
        .iter()
        .filter(|f| f.kind == "a3s.goal.slice.admitted")
        .collect::<Vec<_>>();
    assert_eq!(bindings.len(), 1);
    let binding = bindings[0];
    assert_eq!(binding.payload["goal_id"], source.binding.goal_id);
    assert_eq!(
        binding.payload["goal_revision"],
        source.binding.goal_revision
    );
    assert_eq!(
        binding.payload["plan_revision"],
        source.plan_revision.unwrap()
    );
    assert_eq!(binding.payload["body"]["step_id"], "inspect");
    assert_eq!(
        binding.payload["body"]["expected_goal_sequence"],
        source.goal_sequence
    );
    let run_id = binding.payload["body"]["run_id"].as_str().unwrap();
    assert_eq!(
        session.run_snapshot(run_id).await.unwrap().status,
        crate::run::RunStatus::Completed
    );
    let events = session.run_events(run_id).await;
    assert!(events.iter().any(|e| matches!(&e.event, crate::AgentEvent::GoalExtracted { goal } if goal.description == source.goal.description)));
    assert!(!events
        .iter()
        .any(|e| matches!(&e.event, crate::AgentEvent::GoalAchieved { .. })));
    let turns = facts
        .iter()
        .filter(|f| f.kind == "model.turn")
        .collect::<Vec<_>>();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].cause.as_deref(), Some("infer:1:0"));
    assert!(!facts
        .iter()
        .any(|f| f.kind == "a3s.goal.step.accepted" || f.kind == "a3s.goal.completed"));
    {
        let inputs = client.inputs.lock().unwrap();
        assert!(inputs[0].contains("Inspect the original source"));
        assert!(inputs[0].contains("Repair existing callers"));
        assert!(inputs[0].contains("No second scheduler"));
    }
    let after = session.goal_definition().unwrap().unwrap();
    assert_eq!(after.binding, source.binding);
    assert_eq!(after.state, GoalState::Active);
    assert!(after
        .plan
        .unwrap()
        .steps
        .iter()
        .all(|s| s.status == crate::planning::TaskStatus::Pending));
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn proposed_goal_plan_still_refuses_before_explicit_planning_model() {
    let root = tempfile::tempdir().unwrap();
    let (agent, session, client) = fixture(root.path(), false, options("goal-run")).await;
    let before = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    assert!(session
        .send("Continue", None)
        .await
        .unwrap_err()
        .to_string()
        .contains("goal.host_driver_conflict"));
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
        before
    );
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn accepted_goal_plan_refuses_the_legacy_opener_without_original_worker() {
    let root = tempfile::tempdir().unwrap();
    let (agent, session, client) = fixture(root.path(), true, options("goal-run")).await;
    let before = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let error = crate::agent_api::conversation_runtime::FactSession::from(&session)
        .open()
        .err()
        .unwrap();
    assert!(error.to_string().contains("goal.host_driver_conflict"));
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
        before
    );
    session.close().await;
    agent.close().await;
}

#[derive(Default)]
struct Exports(Mutex<Vec<crate::session_checkpoint::SessionCheckpointExportV1>>);

#[tokio::test]
async fn accepted_goal_does_not_bypass_unknown_host_mount_admission() {
    let root = tempfile::tempdir().unwrap();
    let harness = crate::HarnessComposeOptions::compose(
        vec!["system".into(), "host:nope".into(), "infer".into()],
        None,
        None,
        vec![],
    )
    .unwrap();
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run")
            .with_harness(harness)
            .with_host_harness_registry(Arc::new(crate::meta_harness::BuiltinHostHarnessRegistry)),
    )
    .await;
    let before = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let error = session.send("Continue", None).await.unwrap_err();
    assert!(
        format!("{error:#}").contains("unknown builtin"),
        "{error:#}"
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
        before
    );
    session.close().await;
    agent.close().await;
}

#[async_trait::async_trait]
impl crate::session_checkpoint::SessionCheckpointExportSink for Exports {
    async fn export_checkpoint(
        &self,
        value: crate::session_checkpoint::SessionCheckpointExportV1,
    ) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(value);
        Ok(())
    }
}

async fn prove_goal_recovery(file: bool, policy_enabled: bool, ingress: usize) {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), "original source").unwrap();
    let store: Arc<dyn SessionStore> = if file {
        Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
    } else {
        Arc::new(MemorySessionStore::new())
    };
    let policy = crate::WorkPolicyV1::new("goal-recovery", 1, "GOAL-RECOVERY-POLICY-保留").unwrap();
    let mut opts = options("goal-run")
        .with_session_store(store.clone())
        .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("read(*)"));
    if policy_enabled {
        opts = opts.with_work_policy(policy.clone());
    }
    let (agent, session, client) = fixture(root.path(), true, opts.clone()).await;
    client.scripted.lock().unwrap().push_back(LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![crate::llm::ContentBlock::ToolUse {
                id: "read-source".into(),
                name: "read".into(),
                input: serde_json::json!({"file_path":"source.txt"}),
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: crate::llm::TranscriptVisibility::Wire,
        },
        usage: TokenUsage::default(),
        stop_reason: None,
        token_logprobs: vec![],
        meta: None,
    });
    let exports = Arc::new(Exports::default());
    session
        .set_session_checkpoint_export_sink(Some(exports.clone()))
        .unwrap();
    let answer = session
        .send("Execute the current admitted step.", None)
        .await
        .unwrap();
    assert_eq!(answer.tool_calls_count, 1);
    session.save().await.unwrap();
    let payload = exports.0.lock().unwrap().last().unwrap().open().unwrap();
    let checkpoint = payload.logical_resume.unwrap();
    assert_eq!(checkpoint.schema_version, 2);
    assert_eq!(
        checkpoint.durable_source,
        payload.snapshot.session.durable_source
    );
    let before = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let original_binding = before
        .iter()
        .find(|f| f.kind == "a3s.goal.slice.admitted")
        .unwrap();
    assert_eq!(
        checkpoint.durable_source.as_ref().unwrap().sequence,
        original_binding.seq
    );
    let goal = session.goal_definition().unwrap().unwrap();
    session.close().await;
    agent.close().await;
    let store: Arc<dyn SessionStore> = if file {
        Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
    } else {
        store
    };
    store
        .save_loop_checkpoint(&checkpoint.run_id, &checkpoint)
        .await
        .unwrap();
    opts.work_policy = None;
    opts.session_store = Some(store);
    opts.llm_client = Some(client.clone());
    let mut config = test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();
    let resumed = agent.resume_session_async("goal-run", opts).await.unwrap();
    if ingress == 0 {
        let result = resumed.resume_run(&checkpoint.run_id).await.unwrap();
        assert_eq!(result.text, "ready");
    } else {
        let spawned = if ingress == 1 {
            resumed
                .spawn_recovery_with_run_id(&checkpoint.run_id, "exact-goal-recovery")
                .await
                .unwrap()
        } else {
            let evidence =
                crate::session_checkpoint::SessionLogicalResumeEvidenceV1::from_checkpoint(
                    &checkpoint,
                )
                .unwrap();
            let prepared = resumed
                .prepare_recovery_from_checkpoint(
                    &evidence,
                    "goal-export",
                    "exact-goal-recovery",
                    checkpoint,
                )
                .await
                .unwrap();
            let crate::agent_api::ExactRecoveryPreparation::Ready(prepared) = prepared else {
                panic!("fresh original recovery required");
            };
            resumed.spawn_prepared_recovery(*prepared).await.unwrap()
        };
        let crate::agent_api::AgentRunSpawn::Started { worker, .. } = spawned else {
            panic!("fresh original recovery required");
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            resumed
                .run_snapshot("exact-goal-recovery")
                .await
                .unwrap()
                .status,
            crate::run::RunStatus::Completed
        );
    }
    let after = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    assert_eq!(&after[..before.len()], before.as_slice());
    assert_eq!(
        after.iter().filter(|f| f.kind == "tool.result").count(),
        1,
        "recovery must not replay the original read"
    );
    let bindings = after
        .iter()
        .filter(|f| f.kind == "a3s.goal.slice.admitted")
        .collect::<Vec<_>>();
    assert_eq!(bindings.len(), 2);
    assert_ne!(
        bindings[0].payload["body"]["run_id"],
        bindings[1].payload["body"]["run_id"]
    );
    assert!(bindings
        .iter()
        .all(|f| f.payload["body"]["step_id"] == "inspect"));
    assert_eq!(
        resumed.goal_definition().unwrap().unwrap().binding,
        goal.binding
    );
    assert!(!after
        .iter()
        .any(|f| f.kind == "a3s.goal.completed" || f.kind == "a3s.goal.step.accepted"));
    for input in client.inputs.lock().unwrap().iter() {
        assert_eq!(
            input.matches(policy.instructions()).count(),
            usize::from(policy_enabled)
        );
        assert_eq!(input.matches("## Current admitted goal").count(), 1);
    }
    resumed.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_run_binding_survives_all_original_recovery_ingresses_and_two_stores() {
    for file in [false, true] {
        for policy in [false, true] {
            for ingress in 0..3 {
                prove_goal_recovery(file, policy, ingress).await;
            }
        }
    }
}
