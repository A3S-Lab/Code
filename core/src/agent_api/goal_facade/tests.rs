#![cfg(test)]

use super::*;
use crate::agent_api::tests::{test_config, NonRetryableStreamingClient};
use crate::goal_control::{GoalCommandKindV1, GoalState};
use crate::store::{FileSessionStore, MemorySessionStore, SessionStore};
use std::sync::{atomic::Ordering, Arc};

mod execution;
mod import;
mod legacy;

fn options(id: &str) -> crate::SessionOptions {
    crate::SessionOptions::new()
        .with_session_id(id)
        .with_planning_mode(crate::PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_continuation(false)
}

fn command(session: &AgentSession, digest: Option<String>) -> GoalDefinitionCommandV1 {
    GoalDefinitionCommandV1 {
        kind: GoalCommandKindV1::Create,
        import_source: None,
        operation_id: "create".into(),
        goal_id: "repair".into(),
        session_id: session.session_id.clone(),
        workspace: session
            .tool_context
            .workspace_services
            .workspace_ref()
            .clone(),
        goal: crate::planning::AgentGoal::new("Repair existing callers")
            .with_criteria(vec!["All original checks pass".into()]),
        non_goals: vec!["No second scheduler".into()],
        scope_fact_sequences: vec![],
        work_policy_digest: digest,
        expected_goal_revision: None,
        expected_plan_revision: None,
        expected_goal_sequence: None,
    }
}

async fn prove_session(
    store: Arc<dyn SessionStore>,
    reopen: Option<std::path::PathBuf>,
    with_policy: bool,
) {
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(NonRetryableStreamingClient::default());
    let policy = crate::WorkPolicyV1::new("engineering", 1, "Reuse existing mechanisms.").unwrap();
    let mut opts = options("definition/session").with_session_store(store.clone());
    if with_policy {
        opts = opts.with_work_policy(policy.clone());
    }
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &opts,
        )
        .unwrap();
    let directory = crate::session_folder::effect_log_dir(workspace.path(), &session.session_id);
    let existed = directory.exists();
    assert!(session.goal_definition().unwrap().is_none());
    assert_eq!(directory.exists(), existed);
    let create = command(&session, with_policy.then(|| policy.digest().unwrap()));
    let first = session.apply_goal_definition(create.clone()).unwrap();
    assert_eq!(first.binding.goal_revision, 1);
    let thread = crate::fact_control::thread_for_session(&session.session_id);
    let log = a3s_effect::FileLog::open(&directory).unwrap();
    let before = log.read(&thread).unwrap();
    assert_eq!(
        before[0].kind,
        crate::standing_context::STANDING_CONTEXT_FACT_KIND
    );
    assert_eq!(before.last().unwrap().kind, "a3s.goal.created");
    assert_eq!(
        before
            .iter()
            .filter(|fact| fact.kind == "a3s.work-policy.admitted")
            .count(),
        usize::from(with_policy)
    );
    assert_eq!(
        session
            .apply_goal_definition(create.clone())
            .unwrap()
            .goal_sequence,
        first.goal_sequence
    );
    assert_eq!(log.read(&thread).unwrap(), before);
    let error = session
        .send("Ordinary execution must not ignore a bound Goal", None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("goal.host_driver_conflict"),
        "{error}"
    );
    assert_eq!(log.read(&thread).unwrap(), before);
    let unqualified = crate::agent_api::conversation_runtime::FactSession::from(&session)
        .open()
        .err()
        .expect("legacy opener must refuse a bound Goal");
    assert!(unqualified
        .to_string()
        .contains("goal.host_driver_conflict"));
    assert_eq!(log.read(&thread).unwrap(), before);
    session.save().await.unwrap();
    session.close().await;
    agent.close().await;
    let store: Arc<dyn SessionStore> = match reopen.as_ref() {
        Some(path) => Arc::new(FileSessionStore::new(path).await.unwrap()),
        None => store,
    };
    assert!(store.load("definition/session").await.unwrap().is_some());
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let resumed = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("definition/session").with_session_store(store.clone()),
        )
        .unwrap();
    assert_eq!(
        resumed.goal_definition().unwrap().unwrap().goal_sequence,
        first.goal_sequence
    );
    let mut revise = create.clone();
    revise.kind = GoalCommandKindV1::Revise;
    revise.operation_id = "revise".into();
    revise.expected_goal_revision = Some(1);
    revise.expected_goal_sequence = Some(first.goal_sequence);
    revise.goal.description = "Repair both existing callers".into();
    let second = resumed.apply_goal_definition(revise.clone()).unwrap();
    assert_eq!(second.binding.goal_revision, 2);
    assert!(second.goal_sequence > first.goal_sequence);
    assert_eq!(
        resumed.apply_goal_definition(create).unwrap().goal_sequence,
        first.goal_sequence
    );
    revise.operation_id = "stale".into();
    assert!(resumed
        .apply_goal_definition(revise)
        .unwrap_err()
        .to_string()
        .contains("goal.conflict"));
    let mut plan = crate::planning::ExecutionPlan::new(
        "Repair both callers",
        crate::planning::Complexity::Medium,
    );
    plan.upsert_step(
        crate::planning::Task::new("first", "Repair first caller")
            .with_success_criteria("First check passes")
            .with_status(crate::planning::TaskStatus::Completed),
    );
    plan.upsert_step(
        crate::planning::Task::new("second", "Repair second caller")
            .with_success_criteria("Second check passes")
            .with_dependencies(vec!["first".into()]),
    );
    let mut proposal = crate::goal_control::GoalPlanCommandV1 {
        kind: GoalCommandKindV1::ProposePlan,
        operation_id: "plan".into(),
        goal_id: second.binding.goal_id.clone(),
        session_id: second.binding.session_id.clone(),
        workspace: second.binding.workspace.clone(),
        work_policy_digest: second.binding.work_policy_digest.clone(),
        expected_goal_revision: second.binding.goal_revision,
        expected_plan_revision: second.plan_revision,
        expected_goal_sequence: second.goal_sequence,
        plan: Some(plan.clone()),
        proposal_sequence: None,
    };
    let proposed = resumed.apply_goal_plan(proposal.clone()).unwrap();
    assert!(!proposed.plan_accepted);
    let mut acceptance = proposal.clone();
    acceptance.kind = GoalCommandKindV1::AcceptPlan;
    acceptance.operation_id = "accept-plan".into();
    acceptance.expected_plan_revision = proposed.plan_revision;
    acceptance.expected_goal_sequence = proposed.goal_sequence;
    acceptance.plan = None;
    acceptance.proposal_sequence = proposed.plan_fact_sequence;
    let accepted = resumed.apply_goal_plan(acceptance.clone()).unwrap();
    assert!(accepted.plan_accepted);
    resumed
        .persistence_state
        .write()
        .unwrap()
        .replace_tasks(plan.steps);
    let projected = resumed.goal_definition().unwrap().unwrap().plan.unwrap();
    assert!(projected
        .steps
        .iter()
        .all(|step| step.status == crate::planning::TaskStatus::Pending));
    assert_eq!(
        projected
            .get_ready_steps()
            .iter()
            .map(|step| step.id.as_str())
            .collect::<Vec<_>>(),
        ["first"]
    );
    let before = log.read(&thread).unwrap();
    resumed.save().await.unwrap();
    resumed.close().await;
    agent.close().await;
    let store: Arc<dyn SessionStore> = match reopen.as_ref() {
        Some(path) => Arc::new(FileSessionStore::new(path).await.unwrap()),
        None => store,
    };
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let reopened = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("definition/session").with_session_store(store.clone()),
        )
        .unwrap();
    let recovered = reopened.goal_definition().unwrap().unwrap();
    assert!(recovered.plan_accepted);
    assert_eq!(recovered.plan_revision, Some(1));
    assert_eq!(recovered.goal_sequence, accepted.goal_sequence);
    assert_eq!(
        reopened
            .apply_goal_plan(proposal.clone())
            .unwrap()
            .goal_sequence,
        proposed.goal_sequence
    );
    assert_eq!(
        reopened.apply_goal_plan(acceptance).unwrap().goal_sequence,
        accepted.goal_sequence
    );
    assert_eq!(log.read(&thread).unwrap(), before);
    let cancel = GoalControlCommandV1 {
        kind: GoalCommandKindV1::Cancel,
        operation_id: "cancel".into(),
        goal_id: recovered.binding.goal_id.clone(),
        session_id: recovered.binding.session_id.clone(),
        workspace: recovered.binding.workspace.clone(),
        work_policy_digest: recovered.binding.work_policy_digest.clone(),
        expected_goal_revision: recovered.binding.goal_revision,
        expected_plan_revision: recovered.plan_revision,
        expected_goal_sequence: recovered.goal_sequence,
    };
    let owner = reopened
        .run_admission
        .try_acquire(&reopened.session_id)
        .unwrap();
    assert!(matches!(
        reopened.apply_goal_control(cancel.clone()),
        Err(CodeError::SessionBusy { .. })
    ));
    drop(owner);
    let writer = log.try_writer_fence().unwrap();
    assert!(reopened
        .apply_goal_control(cancel.clone())
        .unwrap_err()
        .to_string()
        .contains("goal.writer_unavailable"));
    drop(writer);
    assert_eq!(log.read(&thread).unwrap(), before);
    let cancelled = reopened.apply_goal_control(cancel.clone()).unwrap();
    assert_eq!(cancelled.state, GoalState::Cancelled);
    assert_eq!(cancelled.binding, recovered.binding);
    assert_eq!(cancelled.plan_revision, recovered.plan_revision);
    assert_eq!(cancelled.plan_fact_sequence, recovered.plan_fact_sequence);
    assert_eq!(cancelled.plan_accepted, recovered.plan_accepted);
    assert_eq!(
        serde_json::to_value(&cancelled.plan).unwrap(),
        serde_json::to_value(&recovered.plan).unwrap()
    );
    let after = log.read(&thread).unwrap();
    assert_eq!(&after[..before.len()], before.as_slice());
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(
        reopened
            .apply_goal_control(cancel.clone())
            .unwrap()
            .goal_sequence,
        cancelled.goal_sequence
    );
    assert_eq!(
        reopened.apply_goal_plan(proposal.clone()).unwrap().state,
        GoalState::Active
    );
    proposal.operation_id = "plan-after-cancel".into();
    proposal.expected_goal_sequence = cancelled.goal_sequence;
    proposal.expected_plan_revision = cancelled.plan_revision;
    assert!(reopened
        .apply_goal_plan(proposal)
        .unwrap_err()
        .to_string()
        .contains("goal.conflict"));
    assert_eq!(
        reopened.goal_definition().unwrap().unwrap().state,
        GoalState::Cancelled
    );
    assert_eq!(log.read(&thread).unwrap(), after);
    reopened.save().await.unwrap();
    assert_eq!(client.streaming_calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.complete_calls.load(Ordering::SeqCst), 0);
    reopened.close().await;
    assert!(matches!(
        reopened.apply_goal_control(cancel.clone()),
        Err(CodeError::SessionClosed { .. })
    ));
    agent.close().await;
    let store: Arc<dyn SessionStore> = match reopen.as_ref() {
        Some(path) => Arc::new(FileSessionStore::new(path).await.unwrap()),
        None => store,
    };
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let restored = agent
        .resume_session_async(
            "definition/session",
            options("definition/session")
                .with_session_store(store)
                .with_llm_client(client.clone()),
        )
        .await
        .unwrap();
    let current = restored.goal_definition().unwrap().unwrap();
    assert_eq!(current.state, GoalState::Cancelled);
    assert_eq!(current.goal_sequence, cancelled.goal_sequence);
    assert_eq!(
        restored.apply_goal_control(cancel).unwrap().goal_sequence,
        current.goal_sequence
    );
    assert_eq!(log.read(&thread).unwrap(), after);
    assert_eq!(client.streaming_calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.complete_calls.load(Ordering::SeqCst), 0);
    restored.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_definition_reuses_original_stores_and_reopens_with_and_without_policy() {
    for with_policy in [false, true] {
        prove_session(Arc::new(MemorySessionStore::new()), None, with_policy).await;
        let directory = tempfile::tempdir().unwrap();
        prove_session(
            Arc::new(FileSessionStore::new(directory.path()).await.unwrap()),
            Some(directory.path().to_path_buf()),
            with_policy,
        )
        .await;
    }
}

#[tokio::test]
async fn goal_reads_keep_the_one_log_handle_a_later_run_uses() {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(NonRetryableStreamingClient::default());
    let writer_agent = crate::Agent::from_config(test_config()).await.unwrap();
    let writer = writer_agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("shared-handle"),
        )
        .unwrap();
    writer
        .apply_goal_definition(command(&writer, None))
        .unwrap();
    writer.close().await;
    writer_agent.close().await;

    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("shared-handle"),
        )
        .unwrap();
    assert!(session.goal_definition().unwrap().is_some());
    let slot = session.run_admission.fact_log_slot();
    let read_handle = slot
        .lock()
        .unwrap()
        .clone()
        .expect("a second handle would contend for the log lock with the first Run");
    let lease = session.run_admission.try_acquire("shared-handle").unwrap();
    assert!(Arc::ptr_eq(
        &read_handle,
        slot.lock().unwrap().as_ref().unwrap()
    ));
    let fence = read_handle.try_writer_fence().unwrap();
    assert!(session.goal_definition().unwrap().is_some());
    drop((fence, lease));
    assert_eq!(client.streaming_calls.load(Ordering::SeqCst), 0);
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_definition_requires_original_owner_and_rejects_foreign_or_busy_before_writes() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(NonRetryableStreamingClient::default());
    let first = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("session"),
        )
        .unwrap();
    let second_agent = crate::Agent::from_config(test_config()).await.unwrap();
    let second = second_agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("session"),
        )
        .unwrap();
    let create = command(&first, None);
    let owner = first.run_admission.try_acquire("session").unwrap();
    assert!(matches!(
        second.apply_goal_definition(create.clone()),
        Err(CodeError::SessionBusy { .. })
    ));
    assert!(owner
        .nonexecuting_fact_log(&second.run_admission, "session")
        .is_err());
    drop(owner);
    let fallback = Arc::new(crate::agent_api::run_admission::RunAdmission::default());
    assert!(fallback
        .try_acquire("session")
        .unwrap()
        .nonexecuting_fact_log(&fallback, "session")
        .is_err());
    let mut foreign = create.clone();
    foreign.workspace.id = "foreign".into();
    assert!(first
        .apply_goal_definition(foreign)
        .unwrap_err()
        .to_string()
        .contains("goal.not_found"));
    let directory = crate::session_folder::effect_log_dir(workspace.path(), "session");
    let log = a3s_effect::FileLog::open(&directory).unwrap();
    assert!(log.read("session").unwrap().is_empty());
    let writer = log.try_writer_fence().unwrap();
    assert!(first
        .apply_goal_definition(create.clone())
        .unwrap_err()
        .to_string()
        .contains("goal.writer_unavailable"));
    drop(writer);
    let admitted = second.apply_goal_definition(create.clone()).unwrap();
    assert_eq!(
        first.apply_goal_definition(create).unwrap().goal_sequence,
        admitted.goal_sequence
    );
    assert_eq!(client.streaming_calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.complete_calls.load(Ordering::SeqCst), 0);
    first.close().await;
    second.close().await;
    agent.close().await;
    second_agent.close().await;
}

#[tokio::test]
async fn goal_definition_recovers_each_initialization_prefix_and_rejects_legacy_policy_rebinding() {
    for retained in [1, 2] {
        let workspace = tempfile::tempdir().unwrap();
        let agent = crate::Agent::from_config(test_config()).await.unwrap();
        let client = Arc::new(NonRetryableStreamingClient::default());
        let policy =
            crate::WorkPolicyV1::new("engineering", 1, "Preserve existing checks.").unwrap();
        let opts = options("prefix").with_work_policy(policy.clone());
        let session = agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &opts,
            )
            .unwrap();
        let create = command(&session, Some(policy.digest().unwrap()));
        session.apply_goal_definition(create.clone()).unwrap();
        session.close().await;
        let log = a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
            workspace.path(),
            "prefix",
        ))
        .unwrap();
        let complete = log.read("prefix").unwrap();
        assert_eq!(complete.len(), 3);
        log.truncate_after("prefix", complete[retained - 1].seq)
            .unwrap();
        let prefix = log.read("prefix").unwrap();
        let resumed = agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &opts,
            )
            .unwrap();
        resumed.apply_goal_definition(create.clone()).unwrap();
        assert_eq!(log.read("prefix").unwrap(), complete);
        assert_eq!(&complete[..retained], prefix.as_slice());
        resumed.close().await;
        // An old conversation prevents a new policy before standing or Goal writes.
        let old = a3s_effect::NewFact {
            kind: "user.message".into(),
            key: "old-message".into(),
            payload: serde_json::json!({"text":"old"}),
        };
        log.append("legacy", &[old], None).unwrap();
        let legacy_before = log.read("legacy").unwrap();
        let Err(refused) = agent.build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("legacy").with_work_policy(policy.clone()),
        ) else {
            panic!("an old conversation refuses a new policy at session admission");
        };
        assert!(
            refused.to_string().contains("work_policy.legacy_rebind"),
            "{refused}"
        );
        assert_eq!(log.read("legacy").unwrap(), legacy_before);
        assert_eq!(client.streaming_calls.load(Ordering::SeqCst), 0);
        assert_eq!(client.complete_calls.load(Ordering::SeqCst), 0);
        agent.close().await;
    }
}

#[tokio::test]
async fn competing_definition_revisions_have_one_original_winner() {
    let workspace = tempfile::tempdir().unwrap();
    let first_agent = crate::Agent::from_config(test_config()).await.unwrap();
    let second_agent = crate::Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(NonRetryableStreamingClient::default());
    let first = first_agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("race"),
        )
        .unwrap();
    let second = second_agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options("race"),
        )
        .unwrap();
    let create = command(&first, None);
    let initial = first.apply_goal_definition(create.clone()).unwrap();
    let mut left = create;
    left.kind = GoalCommandKindV1::Revise;
    left.operation_id = "left".into();
    left.expected_goal_revision = Some(1);
    left.expected_goal_sequence = Some(initial.goal_sequence);
    let mut right = left.clone();
    right.operation_id = "right".into();
    let barrier = std::sync::Barrier::new(2);
    let (left, right) = std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            barrier.wait();
            first.apply_goal_definition(left)
        });
        let right = scope.spawn(|| {
            barrier.wait();
            second.apply_goal_definition(right)
        });
        (left.join().unwrap(), right.join().unwrap())
    });
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let error = left.err().or_else(|| right.err()).unwrap();
    assert!(
        matches!(error, CodeError::SessionBusy { .. })
            || error.to_string().contains("goal.conflict"),
        "{error}"
    );
    assert_eq!(
        first
            .goal_definition()
            .unwrap()
            .unwrap()
            .binding
            .goal_revision,
        2
    );
    assert_eq!(client.streaming_calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.complete_calls.load(Ordering::SeqCst), 0);
    first.close().await;
    second.close().await;
    first_agent.close().await;
    second_agent.close().await;
}
