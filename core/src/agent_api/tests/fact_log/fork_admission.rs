#![cfg(test)]

use super::*;
use crate::goal_control::{GoalCommandKindV1, GoalDefinitionCommandV1};
use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::store::{FileSessionStore, MemorySessionStore, SessionSnapshotV1, SessionStore};
use crate::{Agent, SessionOptions, WorkPolicyV1};
use a3s_effect::LogStore;

fn options(id: &str, store: Arc<dyn SessionStore>, client: Arc<dyn LlmClient>) -> SessionOptions {
    SessionOptions::new()
        .with_session_id(id)
        .with_session_store(store)
        .with_llm_client(client)
        .with_planning_mode(crate::PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_continuation(false)
}

fn define_goal(session: &crate::AgentSession, policy: Option<&WorkPolicyV1>) {
    session
        .apply_goal_definition(GoalDefinitionCommandV1 {
            kind: GoalCommandKindV1::Create,
            import_source: None,
            operation_id: "create".into(),
            goal_id: "source-goal".into(),
            session_id: session.session_id().into(),
            workspace: session
                .tool_context
                .workspace_services
                .workspace_ref()
                .clone(),
            goal: crate::planning::AgentGoal::new("Keep source evidence")
                .with_criteria(vec!["Original evidence retained".into()]),
            non_goals: vec![],
            scope_fact_sequences: vec![],
            work_policy_digest: policy.map(|p| p.digest().unwrap()),
            expected_goal_revision: None,
            expected_plan_revision: None,
            expected_goal_sequence: None,
        })
        .unwrap();
}

async fn prove_fork(file: bool) {
    for (with_policy, with_goal) in [(false, false), (true, false), (false, true), (true, true)] {
        let workspace = tempfile::tempdir().unwrap();
        let target_workspace = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
        } else {
            Arc::new(MemorySessionStore::new())
        };
        let agent = durable_source::source_agent().await;
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = Arc::new(ForkRecordingClient {
            requests: requests.clone(),
        });
        let policy = WorkPolicyV1::new("fork-policy", 1, "Retain exact evidence.").unwrap();
        let mut source_options = options("fork-source", store.clone(), client.clone())
            .with_permission_policy(PermissionPolicy::new().allow_all(&["bash(*)"]));
        if with_policy {
            source_options = source_options.with_work_policy(policy.clone());
        }
        let source = agent
            .session_async(workspace.path().display().to_string(), Some(source_options))
            .await
            .unwrap();
        source
            .send("source history sentinel 52ef", None)
            .await
            .unwrap();
        if with_goal {
            define_goal(&source, with_policy.then_some(&policy));
        }
        source
            .tool_executor
            .artifact_store()
            .put(crate::tools::ToolArtifact {
                artifact_id: "fork-evidence".into(),
                artifact_uri: "a3s://tool-output/fork/evidence".into(),
                tool_name: "read".into(),
                content: "retained evidence".into(),
                original_bytes: 17,
                shown_bytes: 17,
            });
        source
            .tool_executor
            .artifact_store()
            .set_retained_uris(["a3s://tool-output/fork/evidence"]);
        source
            .session_review
            .write()
            .unwrap()
            .upsert(
                crate::session_review::SessionReviewFindingV1::new_transcript(
                    "fork-finding",
                    "fork-source",
                    crate::session_review::SessionReviewSeverityV1::Warning,
                    "evidence",
                    "Keep source evidence",
                    "retained evidence",
                    crate::session_review::SessionReviewAnchorV1::new("turn-1").unwrap(),
                    "review-1",
                    1,
                )
                .unwrap(),
            )
            .unwrap();
        source.save().await.unwrap();
        let snapshot = store.load_snapshot("fork-source").await.unwrap().unwrap();
        let source_binding = snapshot.session.durable_source.clone();
        let source_usage = serde_json::to_value(&snapshot.session.total_usage).unwrap();
        let original_snapshot = serde_json::to_value(&snapshot).unwrap();
        let original_facts = read_workspace_facts(workspace.path(), "fork-source").unwrap();
        let checkpoint: crate::loop_checkpoint::LoopCheckpoint = serde_json::from_value(serde_json::json!({
            "schema_version": snapshot.schema_version, "durable_source": snapshot.session.durable_source,
            "run_id":"source-loop", "session_id":"fork-source", "turn":1, "messages":[],
            "total_usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0},
            "tool_calls_count":0,"checkpoint_ms":1
        })).unwrap();
        store
            .save_loop_checkpoint("source-loop", &checkpoint)
            .await
            .unwrap();
        let mut permission = PermissionPolicy::new();
        permission.default_decision = PermissionDecision::Deny;
        let mut target_options = options("fork-target", store.clone(), client.clone())
            .with_permission_policy(permission);
        if with_policy {
            let error = agent
                .fork_session_async(
                    snapshot.clone(),
                    target_workspace.path().display().to_string(),
                    target_options.clone(),
                )
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("explicit identical acceptance"),
                "{error}"
            );
            target_options = target_options.with_work_policy(policy.clone());
        }
        let before = requests.lock().unwrap().len();
        let target = agent
            .fork_session_async(
                snapshot,
                target_workspace.path().display().to_string(),
                target_options.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            requests.lock().unwrap().len(),
            before,
            "fork must not call provider"
        );
        assert!(target.goal_definition().unwrap().is_none());
        assert_eq!(target.history().len(), source.history().len());
        let saved = store.load_snapshot("fork-target").await.unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&saved.session.total_usage).unwrap(),
            source_usage
        );
        assert_eq!(
            saved
                .session
                .config
                .permission_policy
                .as_ref()
                .unwrap()
                .default_decision,
            PermissionDecision::Deny
        );
        assert_eq!(saved.session.durable_source.is_some(), with_policy);
        if with_policy {
            assert_ne!(saved.session.durable_source, source_binding);
        }
        assert!(saved
            .run_records
            .iter()
            .all(|r| r.snapshot.session_id == "fork-target" && r.snapshot.status.is_terminal()));
        assert_eq!(
            saved.session_review.as_ref().unwrap().session_id,
            "fork-target"
        );
        let finding = &saved.session_review.as_ref().unwrap().findings[0];
        assert_eq!(finding.session_id, "fork-target");
        assert_eq!(finding.evidence, "retained evidence");
        assert_eq!(
            saved.retained_artifact_uris,
            ["a3s://tool-output/fork/evidence"]
        );
        assert_eq!(saved.artifacts[0].content, "retained evidence");
        assert!(read_workspace_facts(target_workspace.path(), "fork-target")
            .unwrap()
            .iter()
            .all(|f| !f.kind.starts_with("a3s.goal.")));
        target.close().await;
        source.close().await;
        agent.close().await;
        let store: Arc<dyn SessionStore> = if file {
            Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
        } else {
            store
        };
        let agent = durable_source::source_agent().await;
        target_options.session_store = Some(store.clone());
        target_options.work_policy = None;
        let target = agent
            .resume_session_async("fork-target", target_options)
            .await
            .unwrap();
        assert!(target.goal_definition().unwrap().is_none());
        target.send("continue fork", None).await.unwrap();
        assert!(
            requests
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .iter()
                .any(|m| m.contains("source history sentinel 52ef")),
            "fork history must reach the provider after real reopen"
        );
        assert_eq!(
            read_workspace_facts(workspace.path(), "fork-source").unwrap(),
            original_facts
        );
        assert_eq!(
            serde_json::to_value(store.load_snapshot("fork-source").await.unwrap().unwrap())
                .unwrap(),
            original_snapshot
        );
        assert_eq!(
            serde_json::to_value(
                store
                    .load_loop_checkpoint("source-loop")
                    .await
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(checkpoint).unwrap()
        );
        assert_eq!(requests.lock().unwrap().len(), before + 1);
        target.close().await;
        agent.close().await;
    }
}

#[tokio::test]
async fn fork_admission_four_modes_memory_store() {
    prove_fork(false).await;
}

#[tokio::test]
async fn fork_admission_four_modes_file_store() {
    prove_fork(true).await;
}

async fn refuses(
    agent: &Agent,
    snapshot: &SessionSnapshotV1,
    workspace: &std::path::Path,
    options: &SessionOptions,
    message: &str,
) {
    let error = agent
        .fork_session_async(
            snapshot.clone(),
            workspace.display().to_string(),
            options.clone(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains(message), "{message}: {error}");
    assert!(!agent
        .list_sessions()
        .await
        .contains(&"fork-target".to_owned()));
}

#[tokio::test]
async fn fork_admission_refuses_debt_busy_and_changed_source_without_execution() {
    let workspace = tempfile::tempdir().unwrap();
    let target_workspace = tempfile::tempdir().unwrap();
    let agent = durable_source::source_agent().await;
    let store = Arc::new(MemorySessionStore::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(CountingText {
        calls: calls.clone(),
    });
    let source = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options("fork-source", store.clone(), client.clone())),
        )
        .await
        .unwrap();
    define_goal(&source, None);
    source.save().await.unwrap();
    let snapshot = store.load_snapshot("fork-source").await.unwrap().unwrap();
    let target_options = options("fork-target", store, client);
    let directory = crate::session_folder::effect_log_dir(workspace.path(), "fork-source");
    let log = a3s_effect::FileLog::open(&directory).unwrap();
    let thread = crate::fact_control::thread_for_session("fork-source");
    let owner = log.try_run_owner(&thread).unwrap();
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &target_options,
        "busy",
    )
    .await;
    drop(owner);
    let writer = log.try_writer_fence().unwrap();
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &target_options,
        "busy",
    )
    .await;
    drop(writer);
    let retained = workspace.path().join("original-source");
    std::fs::rename(&directory, &retained).unwrap();
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &target_options,
        "missing",
    )
    .await;
    assert!(!directory.exists());
    std::fs::rename(retained, &directory).unwrap();
    let fact_file = directory.join(format!("{thread}.jsonl"));
    let original_bytes = std::fs::read(&fact_file).unwrap();
    let mut changed = log.read(&thread).unwrap();
    changed.last_mut().unwrap().payload["unaccepted_change"] = serde_json::json!(true);
    let bytes = changed
        .iter()
        .map(|fact| serde_json::to_string(fact).unwrap() + "\n")
        .collect::<String>();
    std::fs::write(&fact_file, bytes).unwrap();
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &target_options,
        "differs",
    )
    .await;
    std::fs::write(&fact_file, original_bytes).unwrap();
    let mut omitted = snapshot.clone();
    omitted.session.durable_source = None;
    omitted.schema_version = 1;
    refuses(
        &agent,
        &omitted,
        target_workspace.path(),
        &target_options,
        "omits",
    )
    .await;
    log.append(&thread, &[a3s_effect::NewFact {
        kind: "tool.result".into(), key: "orphan".into(),
        payload: serde_json::json!({"toolCallId":"orphan","ok":false,"output":"unknown","unresolvedLegacy":true}),
    }], None).unwrap();
    let before = log.read(&thread).unwrap();
    assert!(crate::fact_control::has_unsettled_effects(&before));
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &target_options,
        "unsettled effects",
    )
    .await;
    assert_eq!(log.read(&thread).unwrap(), before);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(read_workspace_facts(target_workspace.path(), "fork-target")
        .unwrap()
        .is_empty());
    source.close().await;
    agent.close().await;
}

#[tokio::test]
async fn fork_admission_refuses_nonterminal_and_existing_targets() {
    let workspace = tempfile::tempdir().unwrap();
    let target_workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let agent = durable_source::source_agent().await;
    let store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(CountingText {
        calls: calls.clone(),
    });
    let source = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options("fork-source", store.clone(), client.clone())),
        )
        .await
        .unwrap();
    source.send("source history", None).await.unwrap();
    source.save().await.unwrap();
    let snapshot = store.load_snapshot("fork-source").await.unwrap().unwrap();
    let target_options = options("fork-target", store.clone(), client.clone());
    let mut invalid = snapshot.clone();
    invalid.run_records[0].snapshot.status = crate::run::RunStatus::Executing;
    refuses(
        &agent,
        &invalid,
        target_workspace.path(),
        &target_options,
        "unfinished execution",
    )
    .await;
    invalid = snapshot.clone();
    invalid
        .subagent_tasks
        .push(crate::subagent_task_tracker::SubagentTaskSnapshot {
            task_id: "child-task".into(),
            parent_session_id: "fork-source".into(),
            child_session_id: "child".into(),
            agent: "worker".into(),
            description: "unfinished child".into(),
            status: crate::subagent_task_tracker::SubagentStatus::Running,
            started_ms: 1,
            updated_ms: 1,
            finished_ms: None,
            output: None,
            success: None,
            source_anchors: vec![],
            progress: vec![],
        });
    refuses(
        &agent,
        &invalid,
        target_workspace.path(),
        &target_options,
        "unfinished execution",
    )
    .await;
    let mut same_id = target_options.clone();
    same_id.session_id = Some("fork-source".into());
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &same_id,
        "different target",
    )
    .await;
    same_id.session_id = None;
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &same_id,
        "session id",
    )
    .await;
    let bad_policy = target_options
        .clone()
        .with_work_policy(WorkPolicyV1::new("invented", 1, "Unaccepted").unwrap());
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &bad_policy,
        "explicit identical acceptance",
    )
    .await;
    let existing = agent
        .session_async(
            target_workspace.path().display().to_string(),
            Some(target_options.clone()),
        )
        .await
        .unwrap();
    existing.save().await.unwrap();
    let saved_before =
        serde_json::to_value(store.load_snapshot("fork-target").await.unwrap()).unwrap();
    existing.close().await;
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &target_options,
        "saved session",
    )
    .await;
    assert_eq!(
        serde_json::to_value(store.load_snapshot("fork-target").await.unwrap()).unwrap(),
        saved_before
    );
    store.delete("fork-target").await.unwrap();
    let log = a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
        target_workspace.path(),
        "fork-target",
    ))
    .unwrap();
    let thread = crate::fact_control::thread_for_session("fork-target");
    log.append(
        &thread,
        &[a3s_effect::NewFact {
            kind: "user.message".into(),
            key: "existing".into(),
            payload: serde_json::json!({"text":"keep target"}),
        }],
        None,
    )
    .unwrap();
    let before = log.read(&thread).unwrap();
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &target_options,
        "already has facts",
    )
    .await;
    assert_eq!(log.read(&thread).unwrap(), before);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    source.close().await;
    agent.close().await;
}

#[tokio::test]
async fn fork_admission_failed_save_keeps_source_and_never_publishes_target() {
    let workspace = tempfile::tempdir().unwrap();
    let target_workspace = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let agent = durable_source::source_agent().await;
    let source_store = Arc::new(MemorySessionStore::new());
    let target_store = Arc::new(FileSessionStore::new(storage.path()).await.unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(CountingText {
        calls: calls.clone(),
    });
    let source = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options("fork-source", source_store.clone(), client.clone())),
        )
        .await
        .unwrap();
    define_goal(&source, None);
    source.save().await.unwrap();
    let snapshot = source_store
        .load_snapshot("fork-source")
        .await
        .unwrap()
        .unwrap();
    let before = read_workspace_facts(workspace.path(), "fork-source").unwrap();
    let target_options = options("fork-target", target_store.clone(), client);
    let wal = storage.path().join("v1/wal/session-store.ndjson");
    std::fs::create_dir(&wal).unwrap();
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &target_options,
        "Failed to save session",
    )
    .await;
    assert!(!target_store.exists("fork-target").await.unwrap());
    assert_eq!(
        read_workspace_facts(workspace.path(), "fork-source").unwrap(),
        before
    );
    let partial = read_workspace_facts(target_workspace.path(), "fork-target").unwrap();
    assert!(
        !partial.is_empty(),
        "original admitted metadata retained after I/O failure"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    std::fs::remove_dir(wal).unwrap();
    refuses(
        &agent,
        &snapshot,
        target_workspace.path(),
        &target_options,
        "already has facts",
    )
    .await;
    assert_eq!(
        read_workspace_facts(target_workspace.path(), "fork-target").unwrap(),
        partial
    );
    let source_log = a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
        workspace.path(),
        "fork-source",
    ))
    .unwrap();
    let _owner = source_log
        .try_run_owner(&crate::fact_control::thread_for_session("fork-source"))
        .unwrap();
    let _writer = source_log.try_writer_fence().unwrap();
    drop(_writer);
    drop(_owner);
    source.close().await;
    agent.close().await;
}

#[tokio::test]
async fn fork_admission_reuses_original_flat_log_for_same_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = durable_source::source_agent().await;
    let store = Arc::new(MemorySessionStore::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(CountingText {
        calls: calls.clone(),
    });
    let source = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options("flat-source", store.clone(), client.clone())),
        )
        .await
        .unwrap();
    define_goal(&source, None);
    source.save().await.unwrap();
    let before = read_workspace_facts(workspace.path(), "flat-source").unwrap();
    let snapshot = store.load_snapshot("flat-source").await.unwrap().unwrap();
    let target = agent
        .fork_session_async(
            snapshot,
            workspace.path().display().to_string(),
            options("flat-target", store, client),
        )
        .await
        .unwrap();
    assert!(target.goal_definition().unwrap().is_none());
    assert_eq!(
        read_workspace_facts(workspace.path(), "flat-source").unwrap(),
        before
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    target.send("New target history", None).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        read_workspace_facts(workspace.path(), "flat-source").unwrap(),
        before
    );
    target.close().await;
    source.close().await;
    agent.close().await;
}
