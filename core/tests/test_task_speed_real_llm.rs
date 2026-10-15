//! SPEED1 — task-level execution speed and completion capability on the
//! real model.
//!
//! The Meta Harness rule is evidence before synthesis: no speed
//! optimization without a retained baseline. This rail runs three
//! decision-neutral task shapes (no choreography, no tool names in the
//! prompt — the model completes them NATURALLY) and records, per task:
//! wall-clock, provider rounds (tool_calls_count), and prompt/completion
//! tokens. Completion is oracled mechanically per task; the numbers are
//! the deliverable and are asserted only against generous sanity bounds
//! so provider variance fails loudly, not constantly.
//!
//! Task shapes:
//! - LOOKUP: find a marked value in a small file tree (read+answer).
//! - COMPUTE: three disjoint data files, one arithmetic answer
//!   (parallel-batching opportunity).
//! - REPAIR: fix a broken function against its test and verify
//!   (mutate+verify; the completion-gate posture).
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +1.97.1 test -p a3s-code-core --test test_task_speed_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

#[path = "task_speed/goal_step.rs"]
mod goal_step;
mod support;

#[tokio::test]
#[ignore = "requires the pinned real provider"]
async fn child_policy_reaches_real_task_skill_and_workflow_after_reopen() {
    use a3s_code_core::orchestration::{AgentStepSpec, WorkflowBudget};
    use a3s_code_core::skills::{Skill, SkillKind, SkillRegistry};
    use a3s_code_core::store::{FileSessionStore, SessionStore};
    use std::sync::Arc;

    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let marker = format!("CHILD-POLICY-{}", uuid::Uuid::new_v4());
    let work = a3s_code_core::WorkPolicyV1::new("child-live", 1, format!("The selected policy marker is {marker}. When asked for the selected policy marker, quote it exactly. No tools are needed for that answer.")).unwrap();
    let guard = Arc::new(WorkflowBudget::new(Some(250_000)));
    let skills = Arc::new(SkillRegistry::new());
    skills.register_unchecked(Arc::new(Skill {
        name: "selected-policy".into(),
        description: "Answer a question from the selected policy".into(),
        allowed_tools: Some("read(*)".into()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Answer the question using the selected policy. Do not call tools.".into(),
        tags: vec![],
        version: None,
    }));
    let prompt =
        "What is the exact selected policy marker? Return only that marker. Do not use tools.";
    for reopened in [false, true] {
        let agent = pinned_agent().await;
        let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
        let mut permission = PermissionPolicy::new().allow_all(&["task(*)", "Skill(*)", "read(*)"]);
        permission.default_decision = PermissionDecision::Deny;
        let options = SessionOptions::new()
            .with_session_id("child-live")
            .with_session_store(store.clone())
            .with_auto_save(true)
            .with_planning_mode(PlanningMode::Disabled)
            .with_continuation(false)
            .with_auto_delegation_enabled(false)
            .with_manual_delegation_enabled(true)
            .with_permission_policy(permission)
            .with_budget_guard(guard.clone())
            .with_max_tool_rounds(3)
            .with_llm_api_timeout(180_000)
            .with_temperature(0.0)
            .with_worker_agent(
                a3s_code_core::subagent::WorkerAgentSpec::read_only(
                    "policy-reader",
                    "Read-only policy question",
                )
                .with_prompt(
                    "Answer the user's question from the selected policy. Do not call tools.",
                ),
            )
            .with_skill_registry(skills.clone())
            .with_prompt_slots(SystemPromptSlots::default().with_role(
                "PARENT-ONLY-DECOY; this role belongs only to the coordinating parent.",
            ));
        let session = if reopened {
            agent
                .resume_session_async("child-live", options)
                .await
                .unwrap()
        } else {
            agent
                .session_async(
                    workspace.path().display().to_string(),
                    Some(options.with_work_policy(work.clone())),
                )
                .await
                .unwrap()
        };
        if !reopened {
            session
                .send("Reply ready. Do not use tools.", None)
                .await
                .unwrap();
        }
        let snapshot = store.load_snapshot("child-live").await.unwrap().unwrap();
        let before = guard.consumed_tokens();
        let started = Instant::now();
        let task = tokio::time::timeout(REAL_TIMEOUT, session.governed_tool("task", serde_json::json!({"agent":"policy-reader","description":"policy question","prompt":prompt}))).await.unwrap().unwrap();
        assert_eq!(task.exit_code, 0, "{}", task.output);
        assert!(task.output.contains(&marker), "{}", task.output);
        let skill = tokio::time::timeout(
            REAL_TIMEOUT,
            session.governed_tool(
                "Skill",
                serde_json::json!({"skill_name":"selected-policy","prompt":prompt}),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(skill.exit_code, 0, "{}", skill.output);
        assert!(skill.output.contains(&marker), "{}", skill.output);
        let workflow = session.workflow_with_token_budget(Some(100_000));
        let step = tokio::time::timeout(
            REAL_TIMEOUT,
            workflow.agent(AgentStepSpec::new(
                format!("policy-{reopened}"),
                "policy-reader",
                "policy question",
                prompt,
            )),
        )
        .await
        .unwrap();
        assert!(step.success, "{}", step.output);
        assert!(step.output.contains(&marker), "{}", step.output);
        assert!(
            guard.consumed_tokens() > before,
            "child usage must reach the shared guard"
        );
        assert!(session.goal_definition().unwrap().is_none());
        session.save().await.unwrap();
        let after = store.load_snapshot("child-live").await.unwrap().unwrap();
        assert_eq!(
            after.session.durable_source,
            snapshot.session.durable_source
        );
        eprintln!("CHILD-POLICY-LIVE reopen={reopened} elapsed_ms={} shared_tokens_before={before} shared_tokens_after={}", started.elapsed().as_millis(), guard.consumed_tokens());
        session.close().await;
        agent.close().await;
    }
}

use std::path::Path;
use std::time::{Duration, Instant};

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, AgentResult, PlanningMode, SessionOptions, SystemPromptSlots};
use support::layer_c_model::load_pinned_layer_c_config;

const REAL_TIMEOUT: Duration = Duration::from_secs(300);
/// Generous sanity ceiling per task: provider prefill+generation for a
/// handful of rounds on flash is seconds, not minutes. A breach means the
/// run stalled, not that the model is "slow".
const SANITY_CEILING: Duration = Duration::from_secs(180);

#[derive(Default)]
struct CheckpointCapture {
    exports: std::sync::Mutex<Vec<a3s_code_core::session_checkpoint::SessionCheckpointExportV1>>,
}

#[async_trait::async_trait]
impl a3s_code_core::session_checkpoint::SessionCheckpointExportSink for CheckpointCapture {
    async fn export_checkpoint(
        &self,
        checkpoint: a3s_code_core::session_checkpoint::SessionCheckpointExportV1,
    ) -> anyhow::Result<()> {
        self.exports.lock().unwrap().push(checkpoint);
        Ok(())
    }
}

struct CrashCheckpointSink {
    store: std::sync::Arc<a3s_code_core::store::FileSessionStore>,
    ready: std::path::PathBuf,
}

#[async_trait::async_trait]
impl a3s_code_core::session_checkpoint::SessionCheckpointExportSink for CrashCheckpointSink {
    async fn export_checkpoint(
        &self,
        checkpoint: a3s_code_core::session_checkpoint::SessionCheckpointExportV1,
    ) -> anyhow::Result<()> {
        use a3s_code_core::store::SessionStore;
        let payload = checkpoint.open()?;
        let logical = payload
            .logical_resume
            .as_ref()
            .expect("actual tool boundary");
        assert_eq!(logical.schema_version, 2);
        assert_eq!(
            logical.durable_source,
            payload.snapshot.session.durable_source
        );
        self.store.save_snapshot(&payload.snapshot).await?;
        self.store
            .save_loop_checkpoint(&logical.run_id, logical)
            .await?;
        std::fs::write(&self.ready, &logical.run_id)?;
        // Parent terminates this process at the original completed-tool boundary.
        std::future::pending::<anyhow::Result<()>>().await
    }
}

fn pinned_agent() -> impl std::future::Future<Output = Agent> {
    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    Box::pin(async move { Agent::from_config(config).await.expect("agent") })
}

fn speed_options(session_id: &str) -> SessionOptions {
    let mut policy = PermissionPolicy::new().allow_all(&[
        "ls(*)",
        "read(*)",
        "search(*)",
        "write(src/**)",
        "edit(src/**)",
        "bash(node test.mjs)",
    ]);
    policy.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id(session_id)
        .with_permission_policy(policy)
        .with_default_security()
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(12)
        .with_llm_api_timeout(180_000)
        .with_temperature(0.0)
        .with_continuation(false)
        // Host manifest: this workspace's suite is `node test.mjs` and it
        // exercises src/** — a passing run verifies the mutated paths, so
        // repair-style tasks complete Verified without path-referencing
        // gymnastics (the LH4/GATE posture).
        .with_verification_contract(
            a3s_code_core::verification::VerificationContractV1::new(
                "node-suite",
                "node test.mjs",
                ["src/**"],
            )
            .expect("contract"),
        )
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(
            "Complete the task efficiently: take the shortest tool path that \
             establishes the answer, batch independent reads when possible, and \
             answer as soon as the evidence is established.",
        ))
}

/// The model's actual tool decisions on the record.
fn tool_uses(result: &AgentResult) -> Vec<(String, serde_json::Value)> {
    result
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            a3s_code_core::llm::ContentBlock::ToolUse { name, input, .. } => {
                Some((name.clone(), input.clone()))
            }
            _ => None,
        })
        .collect()
}

struct Measurement {
    task: &'static str,
    wall: Duration,
    tool_calls: usize,
    prompt_tokens: usize,
    completion_tokens: usize,
    cache_read_tokens: Option<usize>,
}

async fn timed_send(
    task: &'static str,
    session: &a3s_code_core::AgentSession,
    prompt: &str,
) -> (Measurement, AgentResult) {
    let started = Instant::now();
    let answer = tokio::time::timeout(REAL_TIMEOUT, session.send(prompt, None))
        .await
        .expect("task timed out");
    let answer = answer.unwrap_or_else(|error| {
        // Keep bounded tool evidence before TempDir cleanup; never print model prompts.
        if let Ok(facts) = a3s_code_core::fact_control::read_workspace_facts(
            session.workspace(),
            session.session_id(),
        ) {
            for fact in facts.iter().filter(|fact| fact.kind == "tool.result") {
                let output = fact
                    .payload
                    .get("output")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                eprintln!(
                    "{task} tool evidence seq={}: {}",
                    fact.seq,
                    output.chars().take(2048).collect::<String>()
                );
            }
        }
        panic!("task failed: {error}");
    });
    let wall = started.elapsed();
    assert!(
        wall < SANITY_CEILING,
        "{task} breached the sanity ceiling ({:?}); the run stalled",
        wall
    );
    let measurement = Measurement {
        task,
        wall,
        tool_calls: answer.tool_calls_count,
        prompt_tokens: answer.usage.prompt_tokens,
        completion_tokens: answer.usage.completion_tokens,
        cache_read_tokens: answer.usage.cache_read_tokens,
    };
    (measurement, answer)
}

fn print_row(m: &Measurement) {
    println!(
        "| {:<9} | {:>8.1}s | {:>5} | {:>7} | {:>7} | {:>14} |",
        m.task,
        m.wall.as_secs_f32(),
        m.tool_calls,
        m.prompt_tokens,
        m.completion_tokens,
        m.cache_read_tokens
            .map(|tokens| tokens.to_string())
            .unwrap_or_else(|| "NOT-REPORTED".to_string()),
    );
}

// --- LOOKUP: a marked value in a small file tree ---------------------------

const LOOKUP_ANSWER: &str = "LH6-LOOKUP-9f41c2";

fn assert_lookup_evidence(session: &a3s_code_core::AgentSession, answer: &AgentResult) {
    if !answer.text.contains(LOOKUP_ANSWER) {
        if let Ok(facts) = a3s_code_core::fact_control::read_workspace_facts(
            session.workspace(),
            session.session_id(),
        ) {
            for fact in facts
                .iter()
                .filter(|fact| fact.kind == "tool.result")
                .take(16)
            {
                let output = fact
                    .payload
                    .get("output")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                eprintln!(
                    "lookup failure original receipt seq={}: {}",
                    fact.seq,
                    output.chars().take(1024).collect::<String>()
                );
            }
        }
    }
    assert!(
        answer.text.contains(LOOKUP_ANSWER),
        "{}\nuses: {:?}",
        answer.text,
        tool_uses(answer)
    );
}

fn seed_lookup_workspace(root: &Path) {
    std::fs::create_dir_all(root.join("deploy/env/staging")).unwrap();
    std::fs::write(
        root.join("README.md"),
        "Deployment parameters live under deploy/. Env-specific values are one \
         level deeper. The active environment is recorded in deploy/env/current.md.",
    )
    .unwrap();
    std::fs::write(
        root.join("deploy/env/current.md"),
        "The active environment this week is `staging` (see staging/parameters.md).",
    )
    .unwrap();
    std::fs::write(
        root.join("deploy/env/staging/parameters.md"),
        format!("# staging parameters\n\nbuild_channel: fast\nbuild_stamp: {LOOKUP_ANSWER}\n"),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn speed_lookup_task_completes_with_the_marked_value() {
    let workspace = tempfile::tempdir().expect("workspace");
    seed_lookup_workspace(workspace.path());
    let agent = pinned_agent().await;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(speed_options("lh6-lookup")),
        )
        .await
        .expect("session");

    println!("| task      |   wall s | calls | prompt | compl. | cache-read    |");
    println!("|----------|---------:|------:|-------:|-------:|--------------|");
    let (m, answer) = timed_send(
        "LOOKUP",
        &session,
        "Following the deployment docs (start at README.md), what is the build \
         stamp for the active deployment environment? Reply with its value \
         verbatim and nothing else.",
    )
    .await;
    print_row(&m);
    assert!(
        answer.text.contains(LOOKUP_ANSWER),
        "the lookup task must surface the marked value; got: {}\nuses: {:?}",
        answer.text.trim(),
        tool_uses(&answer)
    );
    session.close().await;
    agent.close().await;
}

/// T006 runtime diagnostic; it does not qualify engineering quality or native execution.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn admitted_policy_survives_real_model_file_session_reopen() {
    use a3s_code_core::store::{FileSessionStore, SessionStore};
    use std::sync::Arc;

    const MARKER: &str = "M1-POLICY-4bd982";
    let workspace = tempfile::tempdir().expect("workspace");
    let store_directory = tempfile::tempdir().expect("session store");
    seed_lookup_workspace(workspace.path());
    let policy = a3s_code_core::WorkPolicyV1::new(
        "runtime-diagnostic",
        1,
        format!("For deployment answers, include the host reference `{MARKER}` after the evidenced value. Keep the answer concise."),
    )
    .expect("policy");
    let digest = policy.digest().expect("digest");
    let prompt = "Following the deployment docs (start at README.md), what is the build stamp for the active deployment environment?";
    for restored in [false, true] {
        let agent = pinned_agent().await;
        let store = Arc::new(
            FileSessionStore::new(store_directory.path())
                .await
                .expect("actual session-store reopen"),
        );
        if restored {
            let saved = store
                .load_snapshot("m1-policy-live")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(saved.schema_version, 2);
            assert!(saved.session.durable_source.is_some());
        }
        let exports = Arc::new(CheckpointCapture::default());
        let options = speed_options("m1-policy-live")
            .with_session_checkpoint_export_sink(exports.clone())
            .with_session_store(store.clone())
            .with_auto_save(true);
        let options = if restored {
            options
        } else {
            options.with_work_policy(policy.clone())
        };
        let session = if restored {
            let source =
                a3s_code_core::session_folder::effect_log_dir(workspace.path(), "m1-policy-live");
            let retained = workspace.path().join("retained-source");
            std::fs::rename(&source, &retained).unwrap();
            let error = agent
                .resume_session_async("m1-policy-live", options.clone())
                .await
                .expect_err("missing original source");
            assert!(error.to_string().contains("durable session"), "{error}");
            assert!(!source.exists());
            std::fs::rename(&retained, &source).unwrap();
            agent
                .resume_session_async("m1-policy-live", options)
                .await
                .expect("source-bound resume")
        } else {
            agent
                .session_async(workspace.path().display().to_string(), Some(options))
                .await
                .expect("session")
        };
        let (measurement, answer) = timed_send(
            if restored { "POLICY-R" } else { "POLICY" },
            &session,
            prompt,
        )
        .await;
        print_row(&measurement);
        assert_lookup_evidence(&session, &answer);
        assert!(answer.text.contains(MARKER), "{}", answer.text);
        let facts = a3s_code_core::fact_control::read_workspace_facts(
            session.workspace(),
            session.session_id(),
        )
        .expect("original fact log");
        let error = session.fork_session_at(0).await.unwrap_err();
        assert!(error.to_string().contains("cannot be truncated"), "{error}");
        assert_eq!(session.fork_session_at(u64::MAX).await.unwrap(), 0);
        assert_eq!(
            a3s_code_core::fact_control::read_workspace_facts(
                session.workspace(),
                session.session_id()
            )
            .unwrap(),
            facts
        );
        let policies = facts
            .iter()
            .filter(|fact| fact.kind == "a3s.work-policy.admitted")
            .collect::<Vec<_>>();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].payload["digest"], digest);
        assert_eq!(
            policies[0].payload["policy"]["instructions"],
            policy.instructions()
        );
        session.save().await.expect("save original session");
        let snapshot = store
            .load_snapshot(session.session_id())
            .await
            .unwrap()
            .unwrap();
        {
            let exports = exports.exports.lock().unwrap();
            // A restored answer may use retained evidence without another tool boundary.
            if !restored || measurement.tool_calls > 0 {
                assert!(
                    !exports.is_empty(),
                    "real tool rounds must export original source-bound checkpoints"
                );
            }
            for export in exports.iter() {
                let payload = export.open().unwrap();
                let checkpoint = payload.logical_resume.unwrap();
                assert_eq!(checkpoint.schema_version, 2);
                assert_eq!(checkpoint.durable_source, snapshot.session.durable_source);
                assert_eq!(
                    checkpoint.durable_source,
                    payload.snapshot.session.durable_source
                );
            }
        }
        session.close().await;
        agent.close().await;
    }
}

/// Metadata-only Goal definitions never bypass explicit plan admission.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn goal_definition_preserves_real_model_history_and_file_session_reopen() {
    use a3s_code_core::goal_control::{
        GoalCommandKindV1, GoalControlCommandV1, GoalDefinitionCommandV1, GoalState,
    };
    use a3s_code_core::store::FileSessionStore;
    use a3s_code_core::workspace::WorkspaceServices;
    use std::sync::Arc;

    let workspace = tempfile::tempdir().unwrap();
    let store_directory = tempfile::tempdir().unwrap();
    seed_lookup_workspace(workspace.path());
    let backend = WorkspaceServices::local(workspace.path());
    let policy = a3s_code_core::WorkPolicyV1::new(
        "goal-admission-diagnostic", 1,
        "For deployment answers, include the host reference M1-GOAL-83be21 after the evidenced value.",
    ).unwrap();
    let agent = pinned_agent().await;
    let store = Arc::new(FileSessionStore::new(store_directory.path()).await.unwrap());
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                speed_options("m1-goal-live")
                    .with_session_store(store)
                    .with_workspace_backend(backend.clone())
                    .with_work_policy(policy.clone()),
            ),
        )
        .await
        .unwrap();
    let (measurement, answer) = timed_send("GOAL-PRE", &session,
        "Following the deployment docs (start at README.md), what is the build stamp for the active deployment environment?",
    ).await;
    print_row(&measurement);
    assert_lookup_evidence(&session, &answer);
    assert!(answer.text.contains("M1-GOAL-83be21"), "{}", answer.text);
    let before = a3s_code_core::fact_control::read_workspace_facts(
        session.workspace(),
        session.session_id(),
    )
    .unwrap();
    let create = GoalDefinitionCommandV1 {
        kind: GoalCommandKindV1::Create,
        import_source: None,
        operation_id: "host-create".into(),
        goal_id: "deployment-audit".into(),
        session_id: session.session_id().into(),
        workspace: backend.workspace_ref().clone(),
        goal: a3s_code_core::planning::AgentGoal::new(
            "Record deployment evidence for subsequent review",
        )
        .with_criteria(vec![format!(
            "Independent deployment fixture equals {LOOKUP_ANSWER}"
        )]),
        non_goals: vec!["Do not change deployment files".into()],
        scope_fact_sequences: vec![before.last().unwrap().seq],
        work_policy_digest: Some(policy.digest().unwrap()),
        expected_goal_revision: None,
        expected_plan_revision: None,
        expected_goal_sequence: None,
    };
    let first = session.apply_goal_definition(create.clone()).unwrap();
    let bound = a3s_code_core::fact_control::read_workspace_facts(
        session.workspace(),
        session.session_id(),
    )
    .unwrap();
    assert_eq!(&bound[..before.len()], before.as_slice());
    assert_eq!(bound.len(), before.len() + 1);
    session.save().await.unwrap();
    session.close().await;
    agent.close().await;

    let agent = pinned_agent().await;
    let store = Arc::new(FileSessionStore::new(store_directory.path()).await.unwrap());
    let reopened = agent
        .resume_session_async(
            "m1-goal-live",
            speed_options("m1-goal-live")
                .with_session_store(store)
                .with_workspace_backend(backend.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        reopened.goal_definition().unwrap().unwrap().goal_sequence,
        first.goal_sequence
    );
    assert_eq!(
        reopened
            .apply_goal_definition(create.clone())
            .unwrap()
            .goal_sequence,
        first.goal_sequence
    );
    let mut revise = create;
    revise.kind = GoalCommandKindV1::Revise;
    revise.operation_id = "host-revise".into();
    revise.expected_goal_revision = Some(first.binding.goal_revision);
    revise.expected_goal_sequence = Some(first.goal_sequence);
    revise.goal.description = "Review both deployment references".into();
    let second = reopened.apply_goal_definition(revise).unwrap();
    assert_eq!(second.binding.goal_revision, 2);
    let revised = a3s_code_core::fact_control::read_workspace_facts(
        reopened.workspace(),
        reopened.session_id(),
    )
    .unwrap();
    assert_eq!(&revised[..bound.len()], bound.as_slice());
    assert_eq!(revised.len(), bound.len() + 1);
    let error = reopened
        .send("Do not bypass a bound Goal", None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("goal.host_driver_conflict"),
        "{error}"
    );
    assert_eq!(
        a3s_code_core::fact_control::read_workspace_facts(
            reopened.workspace(),
            reopened.session_id()
        )
        .unwrap(),
        revised
    );
    let usage = reopened.recorded_usage_tokens();
    let cancel = GoalControlCommandV1 {
        kind: GoalCommandKindV1::Cancel,
        operation_id: "host-cancel".into(),
        goal_id: second.binding.goal_id.clone(),
        session_id: second.binding.session_id.clone(),
        workspace: second.binding.workspace.clone(),
        work_policy_digest: second.binding.work_policy_digest.clone(),
        expected_goal_revision: second.binding.goal_revision,
        expected_plan_revision: second.plan_revision,
        expected_goal_sequence: second.goal_sequence,
    };
    let cancelled = reopened.apply_goal_control(cancel.clone()).unwrap();
    assert_eq!(cancelled.state, GoalState::Cancelled);
    assert_eq!(cancelled.binding, second.binding);
    let retained = a3s_code_core::fact_control::read_workspace_facts(
        reopened.workspace(),
        reopened.session_id(),
    )
    .unwrap();
    assert_eq!(&retained[..revised.len()], revised.as_slice());
    assert_eq!(retained.len(), revised.len() + 1);
    assert_eq!(retained.last().unwrap().kind, "a3s.goal.cancelled");
    assert_eq!(reopened.recorded_usage_tokens(), usage);
    reopened.save().await.unwrap();
    reopened.close().await;
    agent.close().await;

    let agent = pinned_agent().await;
    let store = Arc::new(FileSessionStore::new(store_directory.path()).await.unwrap());
    let reopened = agent
        .resume_session_async(
            "m1-goal-live",
            speed_options("m1-goal-live")
                .with_session_store(store)
                .with_workspace_backend(backend),
        )
        .await
        .unwrap();
    let current = reopened.goal_definition().unwrap().unwrap();
    assert_eq!(current.state, GoalState::Cancelled);
    assert_eq!(current.goal_sequence, cancelled.goal_sequence);
    assert_eq!(
        reopened.apply_goal_control(cancel).unwrap().goal_sequence,
        cancelled.goal_sequence
    );
    assert_eq!(reopened.recorded_usage_tokens(), usage);
    assert_eq!(
        a3s_code_core::fact_control::read_workspace_facts(
            reopened.workspace(),
            reopened.session_id()
        )
        .unwrap(),
        retained
    );
    reopened.close().await;
    agent.close().await;
}

/// T007 plan-association diagnostic; plan acceptance is not step-result acceptance.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn goal_plan_from_real_model_reopens_without_accepting_display_progress() {
    use a3s_code_core::goal_control::{
        GoalCommandKindV1, GoalDefinitionCommandV1, GoalPlanCommandV1,
    };
    use a3s_code_core::planning::TaskStatus;
    use a3s_code_core::store::FileSessionStore;
    use a3s_code_core::workspace::WorkspaceServices;
    use std::sync::Arc;

    let workspace = tempfile::tempdir().unwrap();
    let store_directory = tempfile::tempdir().unwrap();
    seed_lookup_workspace(workspace.path());
    let backend = WorkspaceServices::local(workspace.path());
    let agent = pinned_agent().await;
    let store = Arc::new(FileSessionStore::new(store_directory.path()).await.unwrap());
    let mut permission = PermissionPolicy::new().allow("read(*)");
    permission.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                speed_options("m1-plan-live")
                    .with_session_store(store)
                    .with_permission_policy(permission.clone())
                    // Keep the early plan event through a long streamed answer.
                    .with_retention_limits(
                        a3s_code_core::retention::SessionRetentionLimits::default()
                            .with_max_events_per_run(16_384),
                    )
                    .with_workspace_backend(backend.clone())
                    .with_planning_mode(PlanningMode::Enabled),
            ),
        )
        .await
        .unwrap();
    let prompt = "Following the deployment docs (start at README.md), what is the build stamp for the active deployment environment? Plan two bounded steps before investigating. Each plan step must have an explicit success_criteria field. The second step depends on the first. Do not modify files.";
    let (measurement, answer) = timed_send("PLAN-PRE", &session, prompt).await;
    print_row(&measurement);
    assert!(answer.text.contains(LOOKUP_ANSWER), "{}", answer.text);
    let runs = session.runs().await;
    assert_eq!(runs.len(), 1);
    // Qualify original receipt provenance before associating any durable Goal metadata.
    let source_facts = a3s_code_core::fact_control::read_workspace_facts(
        session.workspace(),
        session.session_id(),
    )
    .unwrap();
    let binding = runs[0]
        .capability_binding
        .as_ref()
        .expect("original Run binding");
    let binding_digest =
        a3s_code_core::digest_json("a3s.code.tool-admission-binding.v1", binding).unwrap();
    let mut preparations = std::collections::BTreeMap::new();
    let mut results = std::collections::BTreeSet::new();
    for fact in &source_facts {
        match fact.kind.as_str() {
            "tool.prepared" => {
                let prepared: a3s_effect::ToolPreparedV1 =
                    serde_json::from_value(fact.payload["prepared"].clone()).unwrap();
                prepared.validate().unwrap();
                let admission = prepared.identity.admission.as_ref().expect("Run admission");
                assert_eq!(admission.run_id, runs[0].id);
                assert_eq!(admission.thread_id, session.session_id());
                assert_eq!(admission.capability_binding_digest, binding_digest);
                assert!(preparations
                    .insert(prepared.identity.key().unwrap(), prepared)
                    .is_none());
            }
            "tool.result" => {
                let result: a3s_effect::ToolExecutionResultV1 =
                    serde_json::from_value(fact.payload["receipt"].clone()).unwrap();
                let key = result.identity.key().unwrap();
                result
                    .ensure_prepared(preparations.get(&key).expect("prior preparation"))
                    .unwrap();
                assert!(results.insert(key));
            }
            _ => {}
        }
    }
    assert!(
        !preparations.is_empty(),
        "the real model must produce original tool receipts"
    );
    assert_eq!(results.len(), preparations.len());
    assert!(!source_facts
        .iter()
        .any(|fact| fact.kind.starts_with("a3s.goal.")));
    let markers = source_facts
        .iter()
        .filter(|fact| fact.kind == "turn.complete")
        .collect::<Vec<_>>();
    assert!(!markers.is_empty());
    for marker in &markers {
        assert!(
            marker.payload.get("run_id").is_none(),
            "the marker does not bind a Run"
        );
        assert_eq!(marker.payload["terminal"], "narrative");
    }
    assert!(matches!(
        answer.completion,
        a3s_code_core::harness_loop::CompletionTerminal::Narrative
    ));
    println!(
        "original source: {} paired tool receipts, {} narrative markers; no durable Goal binding",
        results.len(),
        markers.len()
    );
    let events = session.run_events(&runs[0].id).await;
    let mut plan = events
        .into_iter()
        .find_map(|event| match event.event {
            a3s_code_core::AgentEvent::PlanningEnd { plan, .. } => Some(plan),
            _ => None,
        })
        .expect("original model planner emitted its retained plan");
    assert_eq!(plan.steps.len(), 2);
    a3s_code_core::goal_control::validate_goal_plan(&plan).unwrap();
    let expected_ready = plan
        .get_ready_steps()
        .iter()
        .map(|step| step.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(expected_ready.len(), 1);
    let identity = plan.definition_identity().unwrap();
    let original_steps = plan.steps.clone();
    for step in original_steps {
        plan.mark_status(&step.id, TaskStatus::Completed);
    }
    let goal = session
        .apply_goal_definition(GoalDefinitionCommandV1 {
            kind: GoalCommandKindV1::Create,
            import_source: None,
            operation_id: "create".into(),
            goal_id: "deployment-review".into(),
            session_id: session.session_id().into(),
            workspace: backend.workspace_ref().clone(),
            goal: a3s_code_core::planning::AgentGoal::new("Review deployment references")
                .with_criteria(vec![format!(
                    "Independent deployment fixture equals {LOOKUP_ANSWER}"
                )]),
            non_goals: vec!["Do not modify deployment files".into()],
            scope_fact_sequences: vec![],
            work_policy_digest: None,
            expected_goal_revision: None,
            expected_plan_revision: None,
            expected_goal_sequence: None,
        })
        .unwrap();
    let proposal = GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: "model-plan".into(),
        goal_id: goal.binding.goal_id.clone(),
        session_id: session.session_id().into(),
        workspace: backend.workspace_ref().clone(),
        work_policy_digest: None,
        expected_goal_revision: goal.binding.goal_revision,
        expected_plan_revision: None,
        expected_goal_sequence: goal.goal_sequence,
        plan: Some(plan),
        proposal_sequence: None,
    };
    let proposed = session.apply_goal_plan(proposal.clone()).unwrap();
    let mut acceptance = proposal.clone();
    acceptance.kind = GoalCommandKindV1::AcceptPlan;
    acceptance.operation_id = "host-accept".into();
    acceptance.expected_plan_revision = proposed.plan_revision;
    acceptance.expected_goal_sequence = proposed.goal_sequence;
    acceptance.plan = None;
    acceptance.proposal_sequence = proposed.plan_fact_sequence;
    let accepted = session.apply_goal_plan(acceptance.clone()).unwrap();
    assert!(accepted.plan_accepted);
    let before = a3s_code_core::fact_control::read_workspace_facts(
        session.workspace(),
        session.session_id(),
    )
    .unwrap();
    session.save().await.unwrap();
    session.close().await;
    agent.close().await;
    let agent = pinned_agent().await;
    let store = Arc::new(FileSessionStore::new(store_directory.path()).await.unwrap());
    let reopened = agent
        .resume_session_async(
            "m1-plan-live",
            speed_options("m1-plan-live")
                .with_session_store(store)
                .with_permission_policy(permission)
                .with_workspace_backend(backend),
        )
        .await
        .unwrap();
    let recovered = reopened.goal_definition().unwrap().unwrap();
    assert!(recovered.plan_accepted);
    assert_eq!(recovered.goal_sequence, accepted.goal_sequence);
    let plan = recovered.plan.unwrap();
    assert_eq!(plan.definition_identity().unwrap(), identity);
    assert!(plan
        .steps
        .iter()
        .all(|step| step.status == TaskStatus::Pending));
    assert_eq!(
        plan.get_ready_steps()
            .iter()
            .map(|step| step.id.clone())
            .collect::<Vec<_>>(),
        expected_ready
    );
    assert_eq!(
        reopened.apply_goal_plan(proposal).unwrap().goal_sequence,
        proposed.goal_sequence
    );
    assert_eq!(
        reopened.apply_goal_plan(acceptance).unwrap().goal_sequence,
        accepted.goal_sequence
    );
    let (measurement, answer) = timed_send(
        "PLAN-BOUND",
        &reopened,
        "Use the admitted goal and current deployment docs to recheck the active build stamp. Read current files, then return the stamp. Do not modify files.",
    ).await;
    print_row(&measurement);
    assert!(answer.text.contains(LOOKUP_ANSWER), "{}", answer.text);
    assert!(tool_uses(&answer).iter().any(|(name, _)| name == "read"));
    let after = a3s_code_core::fact_control::read_workspace_facts(
        reopened.workspace(),
        reopened.session_id(),
    )
    .unwrap();
    assert_eq!(&after[..before.len()], before.as_slice());
    assert!(after[before.len()..]
        .iter()
        .any(|f| f.kind == "tool.result"));
    let bindings = after
        .iter()
        .filter(|f| f.kind == "a3s.goal.slice.admitted")
        .collect::<Vec<_>>();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].payload["body"]["step_id"], expected_ready[0]);
    let current = reopened.goal_definition().unwrap().unwrap();
    assert_eq!(current.binding, accepted.binding);
    assert_eq!(
        current.state,
        a3s_code_core::goal_control::GoalState::Active
    );
    let plan = current.plan.unwrap();
    assert_eq!(plan.definition_identity().unwrap(), identity);
    assert!(plan
        .steps
        .iter()
        .all(|step| step.status == TaskStatus::Pending));
    assert!(!after
        .iter()
        .any(|f| f.kind == "a3s.goal.step.accepted" || f.kind == "a3s.goal.completed"));
    reopened.close().await;
    agent.close().await;
}

// --- COMPUTE: three disjoint files, one answer ------------------------------

fn seed_compute_workspace(root: &Path) {
    std::fs::create_dir_all(root.join("regions")).unwrap();
    for (name, units) in [("north", 12), ("south", 30), ("west", 57)] {
        std::fs::write(
            root.join(format!("regions/{name}.md")),
            format!("# {name} region\n\nmonthly units shipped: {units}\n"),
        )
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn speed_compute_task_sums_three_regions() {
    let workspace = tempfile::tempdir().expect("workspace");
    seed_compute_workspace(workspace.path());
    let agent = pinned_agent().await;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(speed_options("lh6-compute")),
        )
        .await
        .expect("session");

    println!("| task      |   wall s | calls | prompt | compl. | cache-read    |");
    println!("|----------|---------:|------:|-------:|-------:|--------------|");
    let (m, answer) = timed_send(
        "COMPUTE",
        &session,
        "Compute the total monthly units shipped across all region files under \
         regions/. Reply with just the number.",
    )
    .await;
    print_row(&m);
    assert!(
        answer.text.contains("99"),
        "the compute task must produce 12+30+57=99; got: {}",
        answer.text.trim()
    );
    session.close().await;
    agent.close().await;
}

// --- REPAIR: fix a broken function against its test and verify --------------

fn seed_repair_workspace(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("SPEC.md"),
        "`totalPrice(base, fee)` returns `base + fee`. The implementation is wrong.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/pricing.mjs"),
        "export function totalPrice(base, fee) {\n  return base * fee;\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("test.mjs"),
        "import assert from \"node:assert/strict\";\n\
         import { totalPrice } from \"./src/pricing.mjs\";\n\
         assert.equal(totalPrice(10, 3), 13);\n\
         console.log(\"LH6-REPAIR-OK\");\n",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn speed_repair_task_lands_a_verified_fix() {
    let workspace = tempfile::tempdir().expect("workspace");
    seed_repair_workspace(workspace.path());
    let agent = pinned_agent().await;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(speed_options("lh6-repair")),
        )
        .await
        .expect("session");

    println!("| task      |   wall s | calls | prompt | compl. | cache-read    |");
    println!("|----------|---------:|------:|-------:|-------:|--------------|");
    let (m, answer) = timed_send(
        "REPAIR",
        &session,
        "The pricing function is broken. Repair it per SPEC.md, prove it with the \
         test, and report the outcome.",
    )
    .await;
    print_row(&m);

    // Completion capability oracle #1: the fix is on disk, correct.
    let fixed = std::fs::read_to_string(workspace.path().join("src/pricing.mjs"))
        .expect("the implementation survives");
    assert!(
        fixed.contains("base + fee"),
        "the repair must replace multiplication with addition; got: {fixed}"
    );
    // Oracle #2: the verified test output is on the record.
    assert!(
        answer
            .messages
            .iter()
            .flat_map(|message| message.content.iter())
            .any(
                |block| matches!(block, a3s_code_core::llm::ContentBlock::ToolResult { content, .. }
                if content.as_text().contains("LH6-REPAIR-OK"))
            ),
        "the passing test run must be on the record"
    );
    // Oracle #3: the verified-completion posture (a mutating run completes
    // through the gate, not a bare narrative).
    assert!(
        matches!(
            answer.completion,
            a3s_code_core::harness_loop::CompletionTerminal::Verified { .. }
        ),
        "a verified repair must complete Verified; got {:?}",
        answer.completion
    );
    println!("LH6 repair verdict: fix=landed test=passed completion=verified");
    session.close().await;
    agent.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned actual model and credentials from .a3s/config.acl"]
async fn durable_loop_checkpoint_resumes_real_model_after_file_reopen() {
    use a3s_code_core::store::{FileSessionStore, SessionStore};
    use std::sync::Arc;
    const CHILD_WORKSPACE: &str = "A3S_CODE_TEST_LOOP_SOURCE_WORKSPACE";
    const CHILD_STORAGE: &str = "A3S_CODE_TEST_LOOP_SOURCE_STORAGE";
    let prompt = "Following the deployment docs (start at README.md), what is the build stamp for the active deployment environment?";
    if let Some(workspace) = std::env::var_os(CHILD_WORKSPACE) {
        let storage =
            std::path::PathBuf::from(std::env::var_os(CHILD_STORAGE).expect("parent storage"));
        let store = Arc::new(FileSessionStore::new(&storage).await.unwrap());
        let agent = pinned_agent().await;
        let sink = Arc::new(CrashCheckpointSink {
            store: store.clone(),
            ready: storage.join("boundary.ready"),
        });
        let policy = a3s_code_core::WorkPolicyV1::new("checkpoint-live", 1,
            "Preserve deployment evidence and include CHECKPOINT-SOURCE-OK after the evidenced answer.").unwrap();
        let session = agent
            .session_async(
                std::path::PathBuf::from(workspace).display().to_string(),
                Some(
                    speed_options("checkpoint-live")
                        .with_session_store(store)
                        .with_work_policy(policy)
                        .with_session_checkpoint_export_sink(sink),
                ),
            )
            .await
            .unwrap();
        let stream = session.stream_with_run(prompt, None).await.unwrap();
        let mut events = stream.events;
        while events.recv().await.is_some() {}
        stream.worker.await.unwrap();
        panic!("child must remain at its exported tool boundary until process termination");
    }
    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    seed_lookup_workspace(workspace.path());
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let started = Instant::now();
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "durable_loop_checkpoint_resumes_real_model_after_file_reopen",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD_WORKSPACE, workspace.path())
            .env(CHILD_STORAGE, storage.path())
            .spawn()
            .unwrap(),
    );
    let ready = storage.path().join("boundary.ready");
    tokio::time::timeout(REAL_TIMEOUT, async {
        while !ready.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "child exited before exporting its real tool boundary"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("actual child checkpoint boundary");
    child.0.kill().unwrap();
    let status = child.0.wait().unwrap();
    assert!(
        !status.success(),
        "source process must actually terminate before recovery"
    );
    let source_run = std::fs::read_to_string(&ready).unwrap();
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let checkpoint = store
        .load_loop_checkpoint(&source_run)
        .await
        .unwrap()
        .unwrap();
    let snapshot = store
        .load_snapshot("checkpoint-live")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.schema_version, 2);
    assert_eq!(checkpoint.durable_source, snapshot.session.durable_source);
    let agent = pinned_agent().await;
    let session = agent
        .resume_session_async(
            "checkpoint-live",
            speed_options("checkpoint-live").with_session_store(store),
        )
        .await
        .unwrap();
    let before =
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "checkpoint-live")
            .unwrap();
    let answer = tokio::time::timeout(REAL_TIMEOUT, session.resume_run(&checkpoint.run_id))
        .await
        .unwrap()
        .unwrap();
    assert_lookup_evidence(&session, &answer);
    assert!(
        answer.text.contains("CHECKPOINT-SOURCE-OK"),
        "{}",
        answer.text
    );
    let after =
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "checkpoint-live")
            .unwrap();
    assert!(
        after
            .iter()
            .filter(|fact| fact.kind == "model.turn")
            .count()
            > before
                .iter()
                .filter(|fact| fact.kind == "model.turn")
                .count(),
        "recovery must call the actual model"
    );
    assert_eq!(
        &after[..before.len()],
        before.as_slice(),
        "recovery preserves every original fact"
    );
    eprintln!("LOOP-RESTORE wall={:.1}s checkpoint_tools={} resumed_tools={} prompt={} completion={} cache={:?}", started.elapsed().as_secs_f64(), checkpoint.tool_calls_count, answer.tool_calls_count, answer.usage.prompt_tokens, answer.usage.completion_tokens, answer.usage.cache_read_tokens);
    session.close().await;
    agent.close().await;
}

/// T009: explicit policy admission carries historical evidence, never a source Goal/Run.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn fork_admission_preserves_real_history_policy_and_file_reopen() {
    use a3s_code_core::goal_control::{GoalCommandKindV1, GoalDefinitionCommandV1};
    use a3s_code_core::store::{FileSessionStore, SessionStore};
    use a3s_code_core::workspace::WorkspaceServices;
    use std::sync::Arc;

    let workspace = tempfile::tempdir().unwrap();
    let target_workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    seed_lookup_workspace(workspace.path());
    let stamp = format!("FORK-{}", uuid::Uuid::new_v4());
    std::fs::write(target_workspace.path().join("fork-current.txt"), &stamp).unwrap();
    let policy = a3s_code_core::WorkPolicyV1::new(
        "fork-live",
        1,
        "Include the host reference M1-FORK-31f942 in every final answer.",
    )
    .unwrap();
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let agent = pinned_agent().await;
    let source = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                speed_options("fork-live-source")
                    .with_session_store(store.clone())
                    .with_work_policy(policy.clone()),
            ),
        )
        .await
        .unwrap();
    let (measurement, answer) = timed_send("FORK-SOURCE", &source,
        "Following the deployment docs (start at README.md), what is the build stamp for the active deployment environment?").await;
    print_row(&measurement);
    assert_lookup_evidence(&source, &answer);
    assert!(answer.text.contains("M1-FORK-31f942"), "{}", answer.text);
    let before =
        a3s_code_core::fact_control::read_workspace_facts(source.workspace(), source.session_id())
            .unwrap();
    source
        .apply_goal_definition(GoalDefinitionCommandV1 {
            kind: GoalCommandKindV1::Create,
            import_source: None,
            operation_id: "create".into(),
            goal_id: "source-review".into(),
            session_id: source.session_id().into(),
            workspace: WorkspaceServices::local(workspace.path())
                .workspace_ref()
                .clone(),
            goal: a3s_code_core::planning::AgentGoal::new("Keep original deployment evidence")
                .with_criteria(vec![format!("Deployment stamp equals {LOOKUP_ANSWER}")]),
            non_goals: vec![],
            scope_fact_sequences: vec![before.last().unwrap().seq],
            work_policy_digest: Some(policy.digest().unwrap()),
            expected_goal_revision: None,
            expected_plan_revision: None,
            expected_goal_sequence: None,
        })
        .unwrap();
    source.save().await.unwrap();
    let snapshot = store
        .load_snapshot(source.session_id())
        .await
        .unwrap()
        .unwrap();
    let original_snapshot = serde_json::to_value(&snapshot).unwrap();
    let facts =
        a3s_code_core::fact_control::read_workspace_facts(source.workspace(), source.session_id())
            .unwrap();
    let usage = source.recorded_usage_tokens();
    source.close().await;
    agent.close().await;

    let agent = pinned_agent().await;
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let started = Instant::now();
    let target = agent
        .fork_session_async(
            snapshot,
            target_workspace.path().display().to_string(),
            speed_options("fork-live-target")
                .with_session_store(store.clone())
                .with_work_policy(policy),
        )
        .await
        .unwrap();
    eprintln!(
        "FORK-ADMISSION elapsed_ms={}",
        started.elapsed().as_millis()
    );
    assert_eq!(target.recorded_usage_tokens(), usage);
    assert!(target.goal_definition().unwrap().is_none());
    target.close().await;
    agent.close().await;

    let agent = pinned_agent().await;
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let target = agent
        .resume_session_async(
            "fork-live-target",
            speed_options("fork-live-target").with_session_store(store.clone()),
        )
        .await
        .unwrap();
    assert!(target.goal_definition().unwrap().is_none());
    assert_eq!(target.recorded_usage_tokens(), usage);
    let (measurement, answer) = timed_send("FORK-REOPEN", &target,
        "Read fork-current.txt in this workspace. Report its exact current stamp and the deployment build stamp established earlier in our conversation.").await;
    print_row(&measurement);
    assert!(answer.text.contains(&stamp), "{}", answer.text);
    assert!(answer.text.contains(LOOKUP_ANSWER), "{}", answer.text);
    assert!(answer.text.contains("M1-FORK-31f942"), "{}", answer.text);
    assert!(!tool_uses(&answer).is_empty(), "must obtain fresh evidence");
    assert!(target.recorded_usage_tokens() > usage);
    assert_eq!(
        a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "fork-live-source")
            .unwrap(),
        facts
    );
    assert_eq!(
        serde_json::to_value(
            store
                .load_snapshot("fork-live-source")
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        original_snapshot
    );
    target.save().await.unwrap();
    target.close().await;
    agent.close().await;
}

/// T012 explicit host mount; the provider and tools share one original manifest.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn navigation_real_model_uses_shared_recent_files_after_deletion() {
    use a3s_code_core::context::{
        ContextProvider, ContextQuery, RecentWorkspaceFilesContextProvider,
    };
    use a3s_code_core::workspace::{ManifestWorkspaceBackend, WorkspaceServices};
    use std::sync::Arc;

    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("notes")).unwrap();
    let backend = ManifestWorkspaceBackend::new_deferred(workspace.path());
    let manifest = backend.manifest();
    let mut updates = manifest.subscribe();
    let provider = Arc::new(RecentWorkspaceFilesContextProvider::new(manifest.clone()));
    let policy = a3s_code_core::WorkPolicyV1::new(
        "engineering-v1",
        1,
        include_str!("../../acp/prompts/engineering.md"),
    )
    .unwrap();
    let agent = pinned_agent().await;
    let mut previous = None;
    let mut previous_version = 0;
    for round in 0..2 {
        let path = format!("notes/发布-{}.txt", uuid::Uuid::new_v4());
        let value = format!("RELEASE-{}", uuid::Uuid::new_v4());
        if let Some(previous) = previous.as_ref() {
            std::fs::remove_file(workspace.path().join(previous)).unwrap();
        }
        std::fs::write(
            workspace.path().join(&path),
            format!("release marker: {value}\n"),
        )
        .unwrap();
        if round == 0 {
            assert!(manifest.activate());
        }
        let snapshot = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let snapshot = updates.recv().await.unwrap();
                if snapshot.files.iter().any(|file| file.path == path)
                    && !snapshot
                        .files
                        .iter()
                        .any(|file| previous.as_ref() == Some(&file.path))
                {
                    break snapshot;
                }
            }
        })
        .await
        .unwrap();
        assert!(snapshot.version > previous_version);
        manifest.touch_file(&path);
        let hints = provider
            .query(&ContextQuery::new("current file"))
            .await
            .unwrap();
        assert!(hints.to_xml().contains(&path));
        assert!(
            !hints.to_xml().contains(&value),
            "navigation does not read file contents"
        );
        if let Some(previous) = previous.as_ref() {
            assert!(!hints.to_xml().contains(previous));
        }
        let mut permission = PermissionPolicy::new().allow("read(*)");
        permission.default_decision = PermissionDecision::Deny;
        let session = agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(
                    SessionOptions::new()
                        .with_session_id(format!("navigation-live-{round}"))
                        .with_workspace_backend(WorkspaceServices::local_with_manifest_backend(
                            backend.clone(),
                        ))
                        .with_context_provider(provider.clone())
                        .with_work_policy(policy.clone())
                        .with_permission_policy(permission)
                        .with_default_security()
                        .with_planning_mode(PlanningMode::Disabled)
                        .with_auto_delegation_enabled(false)
                        .with_manual_delegation_enabled(false)
                        .with_continuation(false)
                        .with_max_tool_rounds(4)
                        .with_llm_api_timeout(180_000)
                        .with_temperature(0.0),
                ),
            )
            .await
            .unwrap();
        let (measurement, answer) = timed_send("NAVIGATION", &session,
            "What is the release marker in the file I just opened? Quote its current value exactly.").await;
        print_row(&measurement);
        assert!(
            answer.text.contains(&value),
            "{}; tools={:?}",
            answer.text,
            tool_uses(&answer)
        );
        let calls = tool_uses(&answer);
        assert!(
            calls
                .iter()
                .any(|(name, args)| name == "read" && args.to_string().contains(&path)),
            "{calls:?}"
        );
        assert!(calls.iter().all(|(name, _)| name == "read"), "{calls:?}");
        let facts = a3s_code_core::fact_control::read_workspace_facts(
            workspace.path(),
            session.session_id(),
        )
        .unwrap();
        assert!(facts
            .iter()
            .filter(|f| f.kind == "context.sections")
            .any(|f| f.payload["sections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["provider"] == "recent_workspace_files")));
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(&path)).unwrap(),
            format!("release marker: {value}\n")
        );
        eprintln!(
            "navigation round={round} manifest_version={} navigation_tokens={} path={path}",
            snapshot.version, hints.total_tokens
        );
        session.close().await;
        previous = Some(path);
        previous_version = snapshot.version;
    }
    agent.close().await;
    manifest.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn admitted_goal_uses_original_model_and_reopens_with_fresh_step_evidence() {
    use a3s_code_core::goal_control::{
        GoalCommandKindV1, GoalDefinitionCommandV1, GoalPlanCommandV1, GoalState,
    };
    use a3s_code_core::planning::{AgentGoal, Complexity, ExecutionPlan, Task, TaskStatus};
    use a3s_code_core::store::FileSessionStore;
    use a3s_code_core::workspace::WorkspaceServices;
    use std::sync::Arc;

    let workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let marker = format!("GOAL-POLICY-{}", uuid::Uuid::new_v4());
    let policy = a3s_code_core::WorkPolicyV1::new(
        "goal-live",
        1,
        format!("Include the exact policy reference {marker} in every final answer."),
    )
    .unwrap();
    let mut prior_facts = vec![];
    let mut prior_binding = None;
    for reopened in [false, true] {
        let value = format!("CURRENT-{}", uuid::Uuid::new_v4());
        std::fs::write(workspace.path().join("task-source.txt"), &value).unwrap();
        let mut config = load_pinned_layer_c_config();
        config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
        config.memory = Some(a3s_code_core::memory::MemoryConfig {
            llm_extraction: false,
            prune_policy: None,
            ..Default::default()
        });
        let agent = Agent::from_config(config).await.unwrap();
        let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
        let guard = Arc::new(compaction::UsageProbe::default());
        let mut permission = PermissionPolicy::new().allow("read(*)");
        permission.default_decision = PermissionDecision::Deny;
        let options = SessionOptions::new()
            .with_session_id("goal-run-live")
            .with_session_store(store)
            .with_planning_mode(PlanningMode::Enabled)
            .with_goal_tracking(true)
            .with_auto_compact(false)
            .with_auto_delegation_enabled(false)
            .with_manual_delegation_enabled(false)
            .with_continuation(false)
            .with_permission_policy(permission)
            .with_budget_guard(guard.clone())
            .with_max_tool_rounds(4)
            .with_llm_api_timeout(180_000)
            .with_temperature(0.0);
        let session = if reopened {
            agent
                .resume_session_async("goal-run-live", options)
                .await
                .unwrap()
        } else {
            agent
                .session_async(
                    workspace.path().display().to_string(),
                    Some(options.with_work_policy(policy.clone())),
                )
                .await
                .unwrap()
        };
        if !reopened {
            let definition = session
                .apply_goal_definition(GoalDefinitionCommandV1 {
                    kind: GoalCommandKindV1::Create,
                    import_source: None,
                    operation_id: "create".into(),
                    goal_id: "source-check".into(),
                    session_id: session.session_id().into(),
                    workspace: WorkspaceServices::local(workspace.path())
                        .workspace_ref()
                        .clone(),
                    goal: AgentGoal::new("Report the current source value")
                        .with_criteria(vec!["The answer quotes a fresh source read".into()]),
                    non_goals: vec!["Do not change any file".into()],
                    scope_fact_sequences: vec![],
                    work_policy_digest: Some(policy.digest().unwrap()),
                    expected_goal_revision: None,
                    expected_plan_revision: None,
                    expected_goal_sequence: None,
                })
                .unwrap();
            let mut plan =
                ExecutionPlan::new("Report the current source value", Complexity::Simple);
            plan.upsert_step(
                Task::new(
                    "inspect",
                    "Read task-source.txt freshly and quote its exact current value",
                )
                .with_success_criteria("The exact current file value appears in the answer"),
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
            session.apply_goal_plan(proposal).unwrap();
        }
        let before = a3s_code_core::fact_control::read_workspace_facts(
            workspace.path(),
            session.session_id(),
        )
        .unwrap();
        assert_eq!(&before[..prior_facts.len()], prior_facts.as_slice());
        let (measurement, answer) = timed_send(
            if reopened { "GOAL-REOPEN" } else { "GOAL-RUN" },
            &session,
            "Execute the current admitted step using fresh source evidence.",
        )
        .await;
        print_row(&measurement);
        assert!(answer.text.contains(&value), "{}", answer.text);
        assert!(answer.text.contains(&marker), "{}", answer.text);
        assert!(tool_uses(&answer)
            .iter()
            .any(|(name, args)| name == "read" && args.to_string().contains("task-source.txt")));
        let after = a3s_code_core::fact_control::read_workspace_facts(
            workspace.path(),
            session.session_id(),
        )
        .unwrap();
        assert_eq!(&after[..before.len()], before.as_slice());
        let new_facts = &after[before.len()..];
        assert_eq!(
            guard.0.lock().unwrap().len(),
            new_facts.iter().filter(|f| f.kind == "model.turn").count(),
            "admitted planning must not issue hidden model calls",
        );
        let bindings = new_facts
            .iter()
            .filter(|f| f.kind == "a3s.goal.slice.admitted")
            .collect::<Vec<_>>();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].payload["body"]["step_id"], "inspect");
        let run_id = bindings[0].payload["body"]["run_id"].as_str().unwrap();
        let run = session.run_snapshot(run_id).await.unwrap();
        let capability_digest = a3s_code_core::digest_json(
            "a3s.code.tool-admission-binding.v1",
            run.capability_binding.as_ref().unwrap(),
        )
        .unwrap();
        let mut preparations = std::collections::BTreeMap::new();
        let mut terminals = std::collections::BTreeSet::new();
        let mut current_value_observed = false;
        for fact in new_facts {
            if fact.kind == "tool.prepared" {
                assert!(fact.seq > bindings[0].seq);
                let prepared: a3s_effect::ToolPreparedV1 =
                    serde_json::from_value(fact.payload["prepared"].clone()).unwrap();
                prepared.validate().unwrap();
                let admission = prepared.identity.admission.as_ref().unwrap();
                assert_eq!(admission.run_id, run_id);
                assert_eq!(admission.thread_id, session.session_id());
                assert_eq!(admission.capability_binding_digest, capability_digest);
                assert!(preparations
                    .insert(prepared.identity.key().unwrap(), prepared)
                    .is_none());
            } else if fact.kind == "tool.result" {
                let result: a3s_effect::ToolExecutionResultV1 =
                    serde_json::from_value(fact.payload["receipt"].clone()).unwrap();
                let key = result.identity.key().unwrap();
                result
                    .ensure_prepared(preparations.get(&key).unwrap())
                    .unwrap();
                assert_eq!(result.outcome, a3s_effect::ToolExecutionOutcome::Succeeded);
                assert_eq!(
                    result.effects.completeness,
                    a3s_effect::ToolObservationCompleteness::Complete
                );
                assert!(result.effects.changes.is_empty());
                current_value_observed |= result.output.to_string().contains(&value);
                assert!(terminals.insert(key));
            }
        }
        assert!(!preparations.is_empty());
        assert_eq!(terminals.len(), preparations.len());
        assert!(
            current_value_observed,
            "the current value must come from this original Run's receipt"
        );
        let goal = session.goal_definition().unwrap().unwrap();
        if let Some(prior) = &prior_binding {
            assert_eq!(&goal.binding, prior);
        }
        prior_binding = Some(goal.binding.clone());
        assert_eq!(goal.state, GoalState::Active);
        assert_eq!(goal.plan_revision, Some(1));
        assert!(goal
            .plan
            .unwrap()
            .steps
            .iter()
            .all(|s| s.status == TaskStatus::Pending));
        assert!(!after
            .iter()
            .any(|f| f.kind == "a3s.goal.step.accepted" || f.kind == "a3s.goal.completed"));
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("task-source.txt")).unwrap(),
            value
        );
        session.save().await.unwrap();
        prior_facts = after;
        session.close().await;
        agent.close().await;
    }
}

mod compaction {
    use super::*;
    use a3s_code_core::budget::BudgetGuard;
    use a3s_code_core::llm::TokenUsage;
    use a3s_code_core::store::{FileSessionStore, SessionStore};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    pub(super) struct UsageProbe(pub(super) Mutex<Vec<TokenUsage>>);

    #[async_trait::async_trait]
    impl BudgetGuard for UsageProbe {
        async fn record_after_llm(&self, _: &str, usage: &TokenUsage) {
            self.0.lock().unwrap().push(usage.clone());
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires the pinned real provider"]
    async fn compact_twice_real_model_reopens_original_policy_and_uses_fresh_evidence() {
        let workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let stamp = format!("CURRENT-{}", uuid::Uuid::new_v4());
        let reference = format!("POLICY-{}", uuid::Uuid::new_v4());
        let memory = format!("TASK-{}", uuid::Uuid::new_v4());
        let policy = a3s_code_core::WorkPolicyV1::new("compact-live", 1, format!(
        "When asked for the policy reference, return exactly {reference}. Keep task references in conversation context. Use current file evidence for workspace questions."
    )).unwrap();
        let guard = Arc::new(UsageProbe::default());
        let agent = pinned_agent().await;
        let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
        let session = agent
            .session_async(
                workspace.path().display().to_string(),
                Some(
                    speed_options("compact-live")
                        .with_auto_compact(false)
                        .with_session_store(store.clone())
                        .with_work_policy(policy.clone())
                        .with_budget_guard(guard.clone()),
                ),
            )
            .await
            .unwrap();
        let (measurement, _) = timed_send("COMPACT-SEED", &session, &format!(
        "The task reference is {memory}. Keep it for our next question. Reply only noted. No tools are needed."
    )).await;
        print_row(&measurement);
        for round in 0..2 {
            if round == 1 {
                let (measurement, _) = timed_send(
                    "COMPACT-BETWEEN",
                    &session,
                    "Keep the task reference in scope. Reply only ready. No tools are needed.",
                )
                .await;
                print_row(&measurement);
            }
            let before = guard.0.lock().unwrap().len();
            let facts_before =
                a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "compact-live")
                    .unwrap();
            let started = Instant::now();
            tokio::time::timeout(REAL_TIMEOUT, session.compact_conversation(None))
                .await
                .unwrap()
                .unwrap();
            let usage = {
                let calls = guard.0.lock().unwrap();
                assert_eq!(
                    calls.len(),
                    before + 1,
                    "exactly one governed summary model call"
                );
                calls.last().unwrap().clone()
            };
            assert!(usage.prompt_tokens > 0 && usage.completion_tokens > 0);
            let facts_after =
                a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "compact-live")
                    .unwrap();
            assert_eq!(&facts_after[..facts_before.len()], facts_before.as_slice());
            assert_eq!(facts_after.len(), facts_before.len() + 1);
            assert_eq!(facts_after.last().unwrap().kind, "compaction.done");
            let summary = facts_after.last().unwrap().payload["summary"]
                .as_str()
                .unwrap();
            assert_eq!(summary.matches("## Durable source").count(), 1);
            assert!(!summary.contains(policy.instructions()));
            eprintln!(
                "COMPACT round={round} wall_ms={} prompt={} completion={} summary_bytes={}",
                started.elapsed().as_millis(),
                usage.prompt_tokens,
                usage.completion_tokens,
                summary.len()
            );
        }
        session.save().await.unwrap();
        let before = store.load_snapshot("compact-live").await.unwrap().unwrap();
        assert!(before.session.durable_source.is_some());
        session.close().await;
        agent.close().await;
        std::fs::write(workspace.path().join("current.txt"), &stamp).unwrap();
        let agent = pinned_agent().await;
        let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
        let resumed = agent
            .resume_session_async(
                "compact-live",
                speed_options("compact-live")
                    .with_auto_compact(false)
                    .with_session_store(store.clone())
                    .with_budget_guard(guard.clone()),
            )
            .await
            .unwrap();
        let (measurement, answer) = timed_send("COMPACT-REOPEN", &resumed,
        "Read current.txt. Return its exact current value, the task reference from our earlier conversation, and the policy reference.").await;
        print_row(&measurement);
        assert!(answer.text.contains(&stamp), "{}", answer.text);
        assert!(answer.text.contains(&memory), "{}", answer.text);
        assert!(answer.text.contains(&reference), "{}", answer.text);
        assert!(tool_uses(&answer)
            .iter()
            .any(|(name, args)| name == "read" && args.to_string().contains("current.txt")));
        resumed.save().await.unwrap();
        assert_eq!(
            store
                .load_snapshot("compact-live")
                .await
                .unwrap()
                .unwrap()
                .session
                .durable_source,
            before.session.durable_source
        );
        let facts =
            a3s_code_core::fact_control::read_workspace_facts(workspace.path(), "compact-live")
                .unwrap();
        assert_eq!(
            facts
                .iter()
                .filter(|f| f.kind == "a3s.work-policy.admitted")
                .count(),
            1
        );
        assert_eq!(
            facts.iter().filter(|f| f.kind == "compaction.done").count(),
            2
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("current.txt")).unwrap(),
            stamp
        );
        resumed.close().await;
        agent.close().await;
    }
}
