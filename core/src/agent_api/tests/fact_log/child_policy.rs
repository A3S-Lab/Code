#![cfg(test)]

use super::*;

struct DelegatingClient {
    skill: bool,
    calls: AtomicUsize,
    systems: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl LlmClient for DelegatingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.systems
            .lock()
            .unwrap()
            .push(system.unwrap_or_default().to_owned());
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let mut response = streamed_response();
        if call == 0 {
            let (name, input) = if self.skill {
                (
                    "Skill",
                    serde_json::json!({"skill_name":"policy-skill","prompt":"Reply ready without tools."}),
                )
            } else {
                (
                    "task",
                    serde_json::json!({"agent":"policy-worker","description":"read","prompt":"Reply ready without tools."}),
                )
            };
            response.message.content = vec![crate::llm::ContentBlock::ToolUse {
                id: "delegate-policy".into(),
                name: name.into(),
                input,
            }];
            response.stop_reason = Some("tool_use".into());
        }
        Ok(response)
    }
    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[crate::llm::ToolDefinition],
        _cancel: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let response = self.complete(messages, system, tools).await?;
        let (sender, receiver) = mpsc::channel(1);
        sender.send(StreamEvent::Done(response)).await.unwrap();
        Ok(receiver)
    }
}

#[tokio::test]
async fn child_policy_actual_fact_run_restores_before_task_and_skill_delegation() {
    use crate::store::{FileSessionStore, MemorySessionStore, SessionStore};
    for file in [false, true] {
        for skill in [false, true] {
            let workspace = tempfile::tempdir().unwrap();
            let storage = tempfile::tempdir().unwrap();
            let store: Arc<dyn SessionStore> = if file {
                Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
            } else {
                Arc::new(MemorySessionStore::new())
            };
            let agent = super::durable_source::source_agent().await;
            let policy =
                crate::WorkPolicyV1::new("child-policy", 1, "PERSISTED-CHILD-POLICY").unwrap();
            let skills = Arc::new(crate::skills::SkillRegistry::new());
            skills.register_unchecked(Arc::new(crate::skills::Skill {
                name: "policy-skill".into(),
                description: "Scoped policy proof".into(),
                allowed_tools: Some("read(*)".into()),
                disable_model_invocation: false,
                kind: crate::skills::SkillKind::Instruction,
                content: "CHILD-ROLE-PRESERVED".into(),
                tags: vec![],
                version: None,
            }));
            let options = crate::SessionOptions::new()
                .with_session_id("restore-child-policy")
                .with_session_store(store.clone())
                .with_auto_save(true)
                .with_planning_mode(crate::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(true)
                .with_continuation(false)
                .with_permission_policy(
                    crate::permissions::PermissionPolicy::new()
                        .allow_all(&["task", "Skill", "read"]),
                )
                .with_skill_registry(skills)
                .with_worker_agent(
                    crate::subagent::WorkerAgentSpec::read_only("policy-worker", "Read only")
                        .with_prompt("CHILD-ROLE-PRESERVED"),
                )
                .with_prompt_slots(crate::SystemPromptSlots {
                    role: Some("PARENT-ONLY-ROLE".into()),
                    extra: Some("PARENT-ONLY-EXTRA".into()),
                    ..Default::default()
                });
            let first = agent
                .build_session(
                    workspace.path().display().to_string(),
                    Arc::new(SkillCapturingClient {
                        systems: Arc::default(),
                    }),
                    &options.clone().with_work_policy(policy.clone()),
                )
                .unwrap();
            first.send("Record initial history.", None).await.unwrap();
            first.save().await.unwrap();
            first.close().await;
            let store: Arc<dyn SessionStore> = if file {
                Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
            } else {
                store
            };
            let options = options.with_session_store(store.clone());
            assert!(options.work_policy.is_none());
            let client = Arc::new(DelegatingClient {
                skill,
                calls: AtomicUsize::new(0),
                systems: Default::default(),
            });
            let resumed = agent
                .build_session(
                    workspace.path().display().to_string(),
                    client.clone(),
                    &options,
                )
                .unwrap();
            resumed
                .send("Delegate one read-only step.", None)
                .await
                .unwrap();
            let systems = client.systems.lock().unwrap().clone();
            assert_eq!(systems.len(), 3, "parent, child, parent");
            for system in &systems {
                assert_eq!(system.matches(policy.instructions()).count(), 1);
            }
            assert!(systems[1].contains("CHILD-ROLE-PRESERVED"));
            assert!(!systems[1].contains("PARENT-ONLY"));
            let facts = read_workspace_facts(workspace.path(), "restore-child-policy").unwrap();
            assert_eq!(
                facts
                    .iter()
                    .filter(|fact| fact.kind == "a3s.work-policy.admitted")
                    .count(),
                1
            );
            assert!(!facts.iter().any(|fact| fact.kind.starts_with("a3s.goal.")));
            resumed.save().await.unwrap();
            resumed.close().await;
            let snapshot = store
                .load_snapshot("restore-child-policy")
                .await
                .unwrap()
                .unwrap();
            let restored = crate::agent_api::session_persistence::apply_persisted_runtime_options(
                options.clone(),
                &snapshot.session,
            )
            .unwrap();
            assert_eq!(restored.work_policy.as_ref(), Some(&policy));
            let error = crate::agent_api::session_persistence::apply_persisted_runtime_options(
                options.with_work_policy(
                    crate::WorkPolicyV1::new("different", 1, "Conflicting policy.").unwrap(),
                ),
                &snapshot.session,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("work_policy.conflict"),
                "{error}"
            );
            let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
            let workflow_session = agent
                .build_session(
                    workspace.path().display().to_string(),
                    Arc::new(SkillCapturingClient {
                        systems: captured.clone(),
                    }),
                    &restored,
                )
                .unwrap();
            let outcome = workflow_session
                .agent_executor()
                .execute_step(
                    crate::orchestration::AgentStepSpec::new(
                        "restored-step",
                        "policy-worker",
                        "read",
                        "Reply ready.",
                    ),
                    None,
                )
                .await;
            assert!(outcome.success, "{}", outcome.output);
            assert_eq!(
                captured.lock().unwrap()[0]
                    .matches(policy.instructions())
                    .count(),
                1
            );
            workflow_session.close().await;
            agent.close().await;
        }
    }
}

#[tokio::test]
async fn delegated_work_policy_reaches_original_workflow_child_prompt() {
    use crate::orchestration::AgentStepSpec;
    let workspace = tempfile::tempdir().unwrap();
    let agent = crate::Agent::from_config(test_config()).await.unwrap();
    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let policy = crate::WorkPolicyV1::new("child", 1, "CHILD-POLICY-EXACT-BYTES").unwrap();
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            Arc::new(SkillCapturingClient {
                systems: systems.clone(),
            }),
            &crate::SessionOptions::new()
                .with_session_id("child-policy")
                .with_planning_mode(crate::PlanningMode::Disabled)
                .with_work_policy(policy.clone())
                .with_prompt_slots(
                    crate::SystemPromptSlots::default().with_role("PARENT-ONLY-ROLE"),
                ),
        )
        .unwrap();
    let outcome = session
        .agent_executor()
        .execute_step(
            AgentStepSpec::new(
                "child",
                "general",
                "read only",
                "Reply ready, without tools.",
            ),
            None,
        )
        .await;
    assert!(outcome.success, "{}", outcome.output);
    let captured = systems.lock().unwrap().clone();
    assert!(!captured.is_empty());
    for system in captured {
        assert_eq!(
            system.matches(policy.instructions()).count(),
            1,
            "child dropped policy"
        );
        assert!(!system.contains("PARENT-ONLY-ROLE"));
    }
    session.close().await;
    agent.close().await;
}
