#![cfg(test)]

use super::{test_launch, A3sCodeAgent};
use a3s_code_core::goal_control::{
    GoalCommandKindV1, GoalDefinitionCommandV1, GoalImportSourceV1, GoalState,
};
use a3s_code_core::workspace::WorkspaceServices;
use agent_client_protocol::{self as acp, Agent as _};

async fn prompt(
    agent: &A3sCodeAgent,
    id: &acp::SessionId,
    text: &str,
) -> acp::Result<acp::PromptResponse> {
    agent
        .prompt(acp::PromptRequest::new(
            id.clone(),
            vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
        ))
        .await
}

async fn preview(
    agent: &A3sCodeAgent,
    id: &acp::SessionId,
    expected_source: serde_json::Value,
) -> acp::Result<serde_json::Value> {
    let raw = serde_json::value::to_raw_value(&serde_json::json!({
        "session_id":id.0.as_ref(), "expected_source":expected_source
    }))
    .unwrap();
    let response = agent
        .ext_method(acp::ExtRequest::new("a3s/goal/import/preview", raw.into()))
        .await?;
    Ok(serde_json::from_str(response.0.get()).unwrap())
}

async fn import(
    agent: &A3sCodeAgent,
    command: &GoalDefinitionCommandV1,
) -> acp::Result<serde_json::Value> {
    let raw = serde_json::value::to_raw_value(command).unwrap();
    let response = agent
        .ext_method(acp::ExtRequest::new("a3s/goal/import", raw.into()))
        .await?;
    Ok(serde_json::from_str(response.0.get()).unwrap())
}

async fn ext(
    agent: &A3sCodeAgent,
    method: &'static str,
    params: serde_json::Value,
) -> acp::Result<serde_json::Value> {
    let raw = serde_json::value::to_raw_value(&params).unwrap();
    let response = agent
        .ext_method(acp::ExtRequest::new(method, raw.into()))
        .await?;
    Ok(serde_json::from_str(response.0.get()).unwrap())
}

fn goal_ref(kind: &str, operation_id: &str, view: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "kind": kind, "operation_id": operation_id,
        "goal_id": view["binding"]["goal_id"], "session_id": view["binding"]["session_id"],
        "workspace": view["binding"]["workspace"], "work_policy_digest": null,
        "expected_goal_revision": view["binding"]["goal_revision"],
        "expected_plan_revision": view["plan_revision"],
        "expected_goal_sequence": view["goal_sequence"],
    })
}

#[tokio::test]
async fn bound_goal_verbs_and_typed_commands_share_core_and_never_wake_without_goal_control() {
    let root = tempfile::tempdir().unwrap();
    let mut launch = test_launch(Some(root.path().join("sessions")));
    launch.workspace = root.path().to_owned();
    assert!(launch.config.goal_control.is_none());
    let host = A3sCodeAgent::new(launch);
    let id = host
        .new_session(acp::NewSessionRequest::new(root.path()))
        .await
        .unwrap()
        .session_id;
    let session = host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    let thread = a3s_code_core::fact_control::thread_for_session(session.session_id());
    let facts = || a3s_code_core::fact_control::read_workspace_facts(root.path(), &thread).unwrap();
    let state = || async {
        ext(
            &host,
            "a3s/goal/state",
            serde_json::json!({"session_id": id.0}),
        )
        .await
        .unwrap()["definition"]
            .clone()
    };
    let unbound = ext(
        &host,
        "a3s/goal/state",
        serde_json::json!({"session_id": id.0}),
    )
    .await
    .unwrap();
    assert_eq!(
        unbound,
        serde_json::json!({"definition": null, "work_policy_digest": null})
    );
    assert!(ext(
        &host,
        "a3s/goal/state",
        serde_json::json!({"session_id": id.0, "x": 1})
    )
    .await
    .is_err());
    assert!(ext(
        &host,
        "a3s/goal/state",
        serde_json::json!({"session_id": "unknown"})
    )
    .await
    .is_err());

    let workspace = WorkspaceServices::local(session.workspace())
        .workspace_ref()
        .clone();
    let create = serde_json::json!({
        "kind": "create", "operation_id": "acp-create", "goal_id": "acp-goal",
        "session_id": id.0, "workspace": workspace,
        "goal": {
            "description": "Report the release code", "success_criteria": ["Reported exactly"],
            "progress": 0.0, "achieved": false, "created_at": 1, "achieved_at": null,
        },
        "non_goals": [], "scope_fact_sequences": [], "work_policy_digest": null,
        "expected_goal_revision": null, "expected_plan_revision": null,
        "expected_goal_sequence": null,
    });
    let mut unknown = create.clone();
    unknown["unexpected"] = true.into();
    let error = ext(&host, "a3s/goal/definition", unknown)
        .await
        .unwrap_err();
    assert!(format!("{error:?}").contains("unknown field"), "{error:?}");
    assert_eq!(state().await, serde_json::Value::Null);
    assert!(facts().is_empty(), "a refused command records nothing");

    host.goals.lock().await.insert(
        id.0.to_string(),
        crate::goal::DurableGoal {
            objective: "Stale legacy projection".into(),
            paused: false,
        },
    );
    let created = ext(&host, "a3s/goal/definition", create).await.unwrap();
    assert_eq!(created["state"], "active");
    assert_eq!(state().await, created);
    assert!(
        !host.goals.lock().await.contains_key(id.0.as_ref()),
        "binding retires the legacy projection, so GoalAchieved has nothing to retire"
    );
    let mut propose = goal_ref("propose_plan", "acp-propose", &created);
    propose["proposal_sequence"] = serde_json::Value::Null;
    propose["plan"] = serde_json::json!({
        "goal": "Release check", "complexity": "Simple", "required_tools": [], "estimated_steps": 1,
        "steps": [{"id": "report", "content": "Report the release code", "status": "pending",
                   "priority": "medium", "success_criteria": "Reported exactly"}],
    });
    let proposed = ext(&host, "a3s/goal/plan", propose).await.unwrap();
    let mut accept = goal_ref("accept_plan", "acp-accept", &proposed);
    accept["plan"] = serde_json::Value::Null;
    accept["proposal_sequence"] = proposed["plan_fact_sequence"].clone();
    let accepted = ext(&host, "a3s/goal/plan", accept).await.unwrap();
    assert_eq!(accepted["plan_accepted"], true);

    // No typed method is a wake: a tick is not a control command.
    let bound = facts();
    assert!(ext(
        &host,
        "a3s/goal/control",
        goal_ref("tick", "acp-tick", &accepted)
    )
    .await
    .is_err());
    let error = prompt(&host, &id, "/goal Replace the objective")
        .await
        .unwrap_err();
    assert!(format!("{error:?}").contains("goal.condition_unsupported"));
    assert!(
        crate::goal::apply_session_goal(&session, crate::goal::GoalOp::Set("Legacy".into()))
            .await
            .is_err(),
        "the legacy writer cannot advance a bound session"
    );
    assert!(prompt(&host, &id, "/goal status").await.is_ok());
    assert_eq!(facts(), bound, "refusals and status record nothing");

    let state_of = || async { session.goal_definition().unwrap().unwrap().state };
    assert!(prompt(&host, &id, "/goal pause").await.is_ok());
    assert_eq!(state_of().await, GoalState::Paused);
    let paused = facts();
    assert_eq!(paused.last().unwrap().kind, "a3s.goal.paused");
    assert!(prompt(&host, &id, "/goal pause").await.is_ok());
    assert_eq!(facts(), paused, "pausing a paused goal records nothing");

    // Resume asks Core for one slice; without trusted bounds Core starts none.
    assert!(prompt(&host, &id, "/goal resume").await.is_ok());
    assert_eq!(state_of().await, GoalState::Active);
    let resumed = facts();
    assert_eq!(resumed.len(), paused.len() + 1);
    assert_eq!(resumed.last().unwrap().kind, "a3s.goal.resumed");
    assert!(prompt(&host, &id, "/goal resume").await.is_ok());
    assert_eq!(facts(), resumed);
    assert!(session.runs().await.is_empty());
    assert_eq!(session.recorded_usage_tokens(), (0, 0, 0));

    assert!(prompt(&host, &id, "/goal clear").await.is_ok());
    assert_eq!(state_of().await, GoalState::Cancelled);
    let cancelled = facts();
    assert!(prompt(&host, &id, "/goal pause").await.is_err());
    assert!(prompt(&host, &id, "/goal resume").await.is_ok());
    assert_eq!(
        facts(),
        cancelled,
        "a finished goal neither resumes nor ticks"
    );
    assert!(!host.goals.lock().await.contains_key(id.0.as_ref()));
    session.close().await;
}

#[tokio::test]
async fn bound_goal_clear_uses_original_facts_and_keeps_legacy_source_after_acp_reload() {
    prove_bound_clear(false).await;
}

#[tokio::test]
#[ignore = "requires A3S_CONFIG_FILE and the live boyue/deepseek-v4-flash model"]
async fn bound_goal_clear_retains_real_model_history_through_acp_reload() {
    prove_bound_clear(true).await;
}

async fn prove_bound_clear(real_model: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut launch = if real_model {
        let config = std::env::var_os("A3S_CONFIG_FILE").expect("explicit live ACL path");
        let mut launch =
            crate::config::load_launch_config(root.path().to_owned(), Some(config.into())).unwrap();
        assert!(launch
            .config
            .find_provider("boyue")
            .unwrap()
            .models
            .iter()
            .any(|model| model.id == "deepseek-v4-flash"));
        launch.model_id = "boyue/deepseek-v4-flash".into();
        launch.config.default_model = Some(launch.model_id.clone());
        launch.config.sessions_dir = Some(root.path().join("sessions"));
        launch
    } else {
        test_launch(Some(root.path().join("sessions")))
    };
    launch.workspace = root.path().to_owned();
    let legacy = crate::goal::DurableGoal {
        objective: "Retain this legacy source".into(),
        // Keep the unrelated live turn from retiring the legacy source.
        paused: real_model,
    };
    std::fs::create_dir_all(root.path().join(".a3s")).unwrap();
    std::fs::write(
        root.path().join(".a3s/durable-goal.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    let path = root.path().join(".a3s/durable-goal.json");
    let original_file = std::fs::read(&path).unwrap();
    let host = A3sCodeAgent::new(launch.clone());
    let opened = host
        .new_session(acp::NewSessionRequest::new(root.path()))
        .await
        .unwrap();
    let id = opened.session_id;
    assert!(!host.goals.lock().await.contains_key(id.0.as_ref()));
    assert!(prompt(&host, &id, "/goal status").await.is_ok());
    let session = host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    assert!(
        session.goal_definition().unwrap().is_none(),
        "legacy workspace data is not adopted"
    );
    let initial_usage = session.recorded_usage_tokens();
    let thread = a3s_code_core::fact_control::thread_for_session(session.session_id());
    let initial_facts =
        a3s_code_core::fact_control::read_workspace_facts(root.path(), &thread).unwrap();
    let selected = preview(&host, &id, serde_json::Value::Null).await.unwrap();
    assert_eq!(selected["goal"]["objective"], legacy.objective);
    assert_eq!(selected["goal"]["paused"], legacy.paused);
    assert_eq!(
        preview(&host, &id, selected["source"].clone())
            .await
            .unwrap(),
        selected
    );
    assert!(preview(
        &host,
        &acp::SessionId::new("unknown"),
        serde_json::Value::Null
    )
    .await
    .is_err());
    let mut changed = original_file.clone();
    changed.push(b'\n');
    std::fs::write(&path, changed).unwrap();
    assert!(preview(&host, &id, selected["source"].clone())
        .await
        .is_err());
    std::fs::write(&path, &original_file).unwrap();
    assert!(session.goal_definition().unwrap().is_none());
    assert_eq!(session.recorded_usage_tokens(), initial_usage);
    assert_eq!(
        a3s_code_core::fact_control::read_workspace_facts(root.path(), &thread).unwrap(),
        initial_facts
    );
    if real_model {
        let marker = uuid::Uuid::new_v4().to_string();
        std::fs::write(root.path().join("goal-source.txt"), &marker).unwrap();
        let started = std::time::Instant::now();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(180),
            prompt(
                &host,
                &id,
                "Read goal-source.txt and quote its one-line value.",
            ),
        )
        .await
        .expect("bounded live turn")
        .unwrap_or_else(|error| {
            let tail = a3s_code_core::fact_control::read_workspace_facts(root.path(), &thread).map(
                |facts| {
                    facts
                        .into_iter()
                        .rev()
                        .take(16)
                        .map(|fact| (fact.seq, fact.kind))
                        .collect::<Vec<_>>()
                },
            );
            panic!(
                "live ACP failed: {error:?}; usage={:?}; fact tail={tail:?}",
                session.recorded_usage_tokens()
            );
        });
        assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
        assert!(session
            .durable_transcript()
            .iter()
            .any(|message| message.role == "assistant" && message.text().contains(&marker)));
        assert!(session.recorded_usage_tokens().0 > 0);
        eprintln!(
            "ACP-BOUND-CLEAR model={} elapsed_ms={} usage={:?}",
            launch.model_id,
            started.elapsed().as_millis(),
            session.recorded_usage_tokens()
        );
    }
    let usage = session.recorded_usage_tokens();
    let backend = WorkspaceServices::local(session.workspace());
    let command = GoalDefinitionCommandV1 {
        kind: GoalCommandKindV1::Create,
        import_source: Some(GoalImportSourceV1 {
            identity: serde_json::from_value(selected["source"].clone()).unwrap(),
            paused: legacy.paused,
        }),
        operation_id: "host-create".into(),
        goal_id: "host-bound".into(),
        session_id: session.session_id().into(),
        workspace: backend.workspace_ref().clone(),
        goal: a3s_code_core::planning::AgentGoal::new(legacy.objective.clone())
            .with_criteria(vec!["The original history remains available".into()]),
        non_goals: vec![],
        scope_fact_sequences: vec![],
        work_policy_digest: None,
        expected_goal_revision: None,
        expected_plan_revision: None,
        expected_goal_sequence: None,
    };
    let unbound_facts =
        a3s_code_core::fact_control::read_workspace_facts(root.path(), &thread).unwrap();
    assert!(session.apply_goal_definition(command.clone()).is_err());
    for variant in 0..4 {
        let mut bad = command.clone();
        match variant {
            0 => bad.goal.success_criteria.clear(),
            1 => bad.goal.description = "Different objective".into(),
            2 => bad.import_source.as_mut().unwrap().paused = !legacy.paused,
            _ => {
                let mut changed = original_file.clone();
                changed.push(b'\n');
                std::fs::write(&path, changed).unwrap();
            }
        }
        assert!(import(&host, &bad).await.is_err(), "variant {variant}");
        assert!(session.goal_definition().unwrap().is_none());
        assert_eq!(
            a3s_code_core::fact_control::read_workspace_facts(root.path(), &thread).unwrap(),
            unbound_facts
        );
        std::fs::write(&path, &original_file).unwrap();
    }
    let queue = host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .turn_queue
        .clone();
    let permit = queue.acquire_owned().await.unwrap();
    let mut admission = Box::pin(import(&host, &command));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut admission)
            .await
            .is_err(),
        "import must serialize behind the original ACP turn queue"
    );
    assert!(session.goal_definition().unwrap().is_none());
    drop(permit);
    let imported = admission.await.unwrap();
    let definition = session.goal_definition().unwrap().unwrap();
    assert_eq!(
        definition.state,
        if legacy.paused {
            GoalState::Paused
        } else {
            GoalState::Active
        }
    );
    assert_eq!(definition.import_source, command.import_source);
    assert_eq!(import(&host, &command).await.unwrap(), imported);
    assert!(!host.goals.lock().await.contains_key(id.0.as_ref()));
    assert!(preview(&host, &id, selected["source"].clone())
        .await
        .is_err());
    let error = prompt(&host, &id, "/goal replace the definition")
        .await
        .unwrap_err();
    assert!(format!("{error:?}").contains("goal.condition_unsupported"));
    // Pause and resume are original Core controls; without trusted bounds the
    // resume's tick starts no Run.
    for (command, state) in [
        ("/goal pause", GoalState::Paused),
        ("/goal resume", GoalState::Active),
    ] {
        assert!(prompt(&host, &id, command).await.is_ok(), "{command}");
        assert_eq!(session.goal_definition().unwrap().unwrap().state, state);
        assert_eq!(std::fs::read(&path).unwrap(), original_file);
    }
    let before = a3s_code_core::fact_control::read_workspace_facts(root.path(), &thread).unwrap();
    assert!(prompt(&host, &id, "/goal status").await.is_ok());
    assert!(prompt(&host, &id, "/goal clear").await.is_ok());
    let cancelled = session.goal_definition().unwrap().unwrap();
    assert_eq!(cancelled.state, GoalState::Cancelled);
    assert_eq!(cancelled.binding, definition.binding);
    let after = a3s_code_core::fact_control::read_workspace_facts(root.path(), &thread).unwrap();
    assert_eq!(&after[..before.len()], before.as_slice());
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(after.last().unwrap().kind, "a3s.goal.cancelled");
    std::fs::write(&path, "changed after admission").unwrap();
    assert_eq!(import(&host, &command).await.unwrap(), imported);
    assert_eq!(
        session.goal_definition().unwrap().unwrap().state,
        GoalState::Cancelled
    );
    std::fs::write(&path, &original_file).unwrap();
    assert!(prompt(&host, &id, "/goal clear").await.is_ok());
    assert_eq!(
        a3s_code_core::fact_control::read_workspace_facts(root.path(), &thread).unwrap(),
        after
    );
    assert_eq!(std::fs::read(&path).unwrap(), original_file);
    assert_eq!(session.recorded_usage_tokens(), usage);
    session.save().await.unwrap();
    session.close().await;
    drop(host);

    let host = A3sCodeAgent::new(launch);
    host.load_session(acp::LoadSessionRequest::new(id.clone(), root.path()))
        .await
        .unwrap();
    assert!(!host.goals.lock().await.contains_key(id.0.as_ref()));
    let session = host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    assert_eq!(
        session.goal_definition().unwrap().unwrap().state,
        GoalState::Cancelled
    );
    assert_eq!(import(&host, &command).await.unwrap(), imported);
    assert_eq!(
        session.goal_definition().unwrap().unwrap().import_source,
        command.import_source
    );
    assert!(prompt(&host, &id, "/goal clear").await.is_ok());
    assert_eq!(
        a3s_code_core::fact_control::read_workspace_facts(root.path(), &thread).unwrap(),
        after
    );
    assert_eq!(std::fs::read(&path).unwrap(), original_file);

    let source = a3s_code_core::session_folder::effect_log_dir(root.path(), session.session_id())
        .join(format!("{thread}.jsonl"));
    let mut corrupted = after;
    corrupted.last_mut().unwrap().payload["schema"] = serde_json::json!("invalid");
    let bytes = corrupted
        .iter()
        .map(|fact| serde_json::to_string(fact).unwrap() + "\n")
        .collect::<String>();
    std::fs::write(&source, &bytes).unwrap();
    host.goals.lock().await.insert(id.0.to_string(), legacy);
    host.reapply_active_goal(id.0.as_ref()).await;
    assert!(!host.goals.lock().await.contains_key(id.0.as_ref()));
    assert!(prompt(&host, &id, "/goal clear").await.is_err());
    assert!(import(&host, &command).await.is_err());
    assert!(preview(&host, &id, serde_json::Value::Null).await.is_err());
    assert_eq!(std::fs::read_to_string(&source).unwrap(), bytes);
    assert_eq!(std::fs::read(&path).unwrap(), original_file);
    assert_eq!(session.recorded_usage_tokens(), usage);
    session.close().await;
    assert!(import(&host, &command).await.is_err());
    assert!(preview(&host, &id, serde_json::Value::Null).await.is_err());
}

#[tokio::test]
async fn unowned_workspace_goal_is_not_automatically_claimed_by_new_sessions() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join(".a3s/durable-goal.json");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    let original = br#"{"objective":"Explicit import only","paused":false}"#;
    std::fs::write(&source, original).unwrap();
    let mut launch = test_launch(Some(root.path().join("sessions")));
    launch.workspace = root.path().to_owned();
    let host = A3sCodeAgent::new(launch);
    for _ in 0..2 {
        let id = host
            .new_session(acp::NewSessionRequest::new(root.path()))
            .await
            .unwrap()
            .session_id;
        assert!(
            !host.goals.lock().await.contains_key(id.0.as_ref()),
            "workspace source must not become a new session's goal"
        );
        assert!(preview(&host, &id, serde_json::Value::Null).await.is_ok());
        assert_eq!(std::fs::read(&source).unwrap(), original);
        host.sessions
            .lock()
            .await
            .get(id.0.as_ref())
            .unwrap()
            .session
            .close()
            .await;
    }
}

#[tokio::test]
async fn same_session_other_host_cannot_write_or_retire_legacy_goal_after_import() {
    let root = tempfile::tempdir().unwrap();
    let mut launch = test_launch(Some(root.path().join("sessions")));
    launch.workspace = root.path().to_owned();
    let first_host = A3sCodeAgent::new(launch.clone());
    let id = first_host
        .new_session(acp::NewSessionRequest::new(root.path()))
        .await
        .unwrap()
        .session_id;
    let first = first_host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    let legacy = crate::goal::apply_session_goal(
        &first,
        crate::goal::GoalOp::Set("Owned legacy goal".into()),
    )
    .await
    .unwrap()
    .next
    .unwrap();
    first.save().await.unwrap();
    assert!(prompt(&first_host, &id, "/goal status").await.is_ok());
    let folder = a3s_code_core::session_folder::find_session_dir(root.path(), id.0.as_ref())
        .unwrap()
        .unwrap();
    let history = serde_json::json!({"seq":1,"type":"user.message","key":"retained-history","payload":{"text":"Retained source history"}});
    std::fs::write(
        folder.join(format!(
            "{}.jsonl",
            a3s_code_core::fact_control::thread_for_session(id.0.as_ref())
        )),
        format!("{history}\n"),
    )
    .unwrap();
    let owned_file = folder.join("durable-goal.json");
    let owned_bytes = std::fs::read(&owned_file).unwrap();
    let source_file = root.path().join(".a3s/durable-goal.json");
    let original = br#"{"objective":"Explicitly selected source","paused":false}"#;
    std::fs::write(&source_file, original).unwrap();
    let second_host = A3sCodeAgent::new(launch);
    second_host
        .load_session(acp::LoadSessionRequest::new(id.clone(), root.path()))
        .await
        .unwrap();
    let selected = preview(&second_host, &id, serde_json::Value::Null)
        .await
        .unwrap();
    let command = GoalDefinitionCommandV1 {
        kind: GoalCommandKindV1::Create,
        import_source: Some(GoalImportSourceV1 {
            identity: serde_json::from_value(selected["source"].clone()).unwrap(),
            paused: false,
        }),
        operation_id: "other-host-import".into(),
        goal_id: "imported".into(),
        session_id: id.0.to_string(),
        workspace: WorkspaceServices::local(first.workspace())
            .workspace_ref()
            .clone(),
        goal: a3s_code_core::planning::AgentGoal::new("Explicitly selected source")
            .with_criteria(vec!["Keep both original files unchanged".into()]),
        non_goals: vec![],
        scope_fact_sequences: vec![],
        work_policy_digest: None,
        expected_goal_revision: None,
        expected_plan_revision: None,
        expected_goal_sequence: None,
    };
    first
        .with_legacy_goal_state(|| async {
            assert!(
                import(&second_host, &command).await.is_err(),
                "foreign import must share the original owner"
            );
            Ok(())
        })
        .await
        .unwrap();
    assert!(import(&second_host, &command).await.is_ok());
    assert!(
        crate::goal::apply_session_goal(&first, crate::goal::GoalOp::Clear)
            .await
            .is_err()
    );
    assert!(
        crate::goal::retire_achieved_durable_goal(&first, Some(&legacy))
            .await
            .is_err()
    );
    assert!(
        prompt(&first_host, &id, "/goal pause").await.is_ok(),
        "pause uses the original bound control"
    );
    assert!(
        prompt(&first_host, &id, "/goal clear").await.is_ok(),
        "clear uses the original bound cancel"
    );
    assert!(!first_host.goals.lock().await.contains_key(id.0.as_ref()));
    assert_eq!(std::fs::read(&source_file).unwrap(), original);
    assert_eq!(std::fs::read(&owned_file).unwrap(), owned_bytes);
    assert_eq!(first.recorded_usage_tokens(), (0, 0, 0));
    first.close().await;
    second_host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .close()
        .await;
}

#[tokio::test]
#[ignore = "requires A3S_CONFIG_FILE and the live boyue/deepseek-v4-flash model"]
async fn session_scoped_legacy_goal_reaches_real_model_after_new_host_reopen() {
    let root = tempfile::tempdir().unwrap();
    let config = std::env::var_os("A3S_CONFIG_FILE").expect("explicit live ACL");
    let mut launch =
        crate::config::load_launch_config(root.path().to_owned(), Some(config.into())).unwrap();
    launch.model_id = "boyue/deepseek-v4-flash".into();
    launch.config.default_model = Some(launch.model_id.clone());
    launch.config.sessions_dir = Some(root.path().join("sessions"));
    let first_host = A3sCodeAgent::new(launch.clone());
    let id = first_host
        .new_session(acp::NewSessionRequest::new(root.path()))
        .await
        .unwrap()
        .session_id;
    tokio::time::timeout(
        std::time::Duration::from_secs(180),
        prompt(&first_host, &id, "Reply ready. Do not use tools."),
    )
    .await
    .unwrap()
    .unwrap();
    let first = first_host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    let marker = format!("SCOPED-{}", uuid::Uuid::new_v4());
    let objective = format!("The selected objective marker is {marker}. When asked, quote the marker exactly. Do not use tools.");
    crate::goal::apply_session_goal(&first, crate::goal::GoalOp::Set(objective.clone()))
        .await
        .unwrap();
    first.save().await.unwrap();
    assert!(
        !first.history().iter().any(|m| m.text().contains(&marker)),
        "the model has never seen the new marker"
    );
    let before = first.recorded_usage_tokens();
    let root_source = root.path().join(".a3s/durable-goal.json");
    let unowned = br#"{"objective":"UNOWNED-WORKSPACE-MARKER","paused":false}"#;
    std::fs::write(&root_source, unowned).unwrap();
    first.close().await;
    drop(first_host);

    let next_host = A3sCodeAgent::new(launch);
    next_host
        .load_session(acp::LoadSessionRequest::new(id.clone(), root.path()))
        .await
        .unwrap();
    assert_eq!(
        next_host
            .goals
            .lock()
            .await
            .get(id.0.as_ref())
            .unwrap()
            .objective,
        objective
    );
    let next = next_host
        .sessions
        .lock()
        .await
        .get(id.0.as_ref())
        .unwrap()
        .session
        .clone();
    assert_eq!(next.recorded_usage_tokens(), before);
    assert!(
        next.goal_definition().unwrap().is_none(),
        "legacy restoration grants no bound Goal authority"
    );
    let started = std::time::Instant::now();
    tokio::time::timeout(
        std::time::Duration::from_secs(180),
        prompt(
            &next_host,
            &id,
            "What is the exact marker in my selected objective? Return it verbatim.",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let history = next.history();
    let answer = history
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .unwrap()
        .text();
    assert!(answer.contains(&marker), "{answer}");
    let facts =
        a3s_code_core::fact_control::read_workspace_facts(next.workspace(), next.session_id())
            .unwrap();
    let input = facts
        .iter()
        .rev()
        .find(|f| f.kind == "user.message")
        .unwrap();
    assert!(input.payload["text"].as_str().unwrap().contains(&marker));
    assert!(!input.payload["text"]
        .as_str()
        .unwrap()
        .contains("UNOWNED-WORKSPACE-MARKER"));
    assert_eq!(std::fs::read(&root_source).unwrap(), unowned);
    assert!(next.recorded_usage_tokens().2 > before.2);
    eprintln!("SCOPED-GOAL-REOPEN model=boyue/deepseek-v4-flash elapsed_ms={} before={before:?} after={:?}", started.elapsed().as_millis(), next.recorded_usage_tokens());
    next.close().await;
}
