#![cfg(test)]

use super::*;
use crate::verification::{VerificationCheck, VerificationReport, VerificationStatus};

struct ByteAttestor {
    root: std::path::PathBuf,
    calls: AtomicUsize,
}

impl crate::CompletionAttestor for ByteAttestor {
    fn attest(
        &self,
        digest: &str,
        paths: &[crate::MutatedPathRecord],
    ) -> Option<VerificationReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if paths.len() != 1 || paths[0].path != "source.txt" {
            return None;
        }
        let bytes = std::fs::read(self.root.join("source.txt")).ok()?;
        if bytes != b"independent expected source"
            || paths[0].content_digest != sha256::digest(&bytes)
        {
            return None;
        }
        Some(
            VerificationReport::new(
                "host:current-source",
                vec![VerificationCheck::required(
                    "inspect",
                    "host_source_bytes",
                    "Original source is checked against expected bytes and receipt digest",
                )
                .with_status(VerificationStatus::Passed)],
            )
            .with_effect_digest(digest),
        )
    }
}

struct WrongDigestAttestor(ByteAttestor);

impl crate::CompletionAttestor for WrongDigestAttestor {
    fn attest(
        &self,
        digest: &str,
        paths: &[crate::MutatedPathRecord],
    ) -> Option<VerificationReport> {
        let mut report = crate::CompletionAttestor::attest(&self.0, digest, paths)?;
        report.effect_digest = Some("a".repeat(64));
        Some(report)
    }
}

pub(super) fn script(client: &GoalClient, final_response: bool) {
    for (index, message) in [
        Message {
            role: "assistant".into(), content: vec![crate::llm::ContentBlock::ToolUse {
                id: "write-current".into(), name: "write".into(),
                input: serde_json::json!({"file_path":"source.txt", "content":"independent expected source"}),
            }], reasoning_content: None, transcript_text: None,
            transcript_visibility: crate::llm::TranscriptVisibility::Wire,
        },
        Message::assistant("Updated the original source."),
        Message::assistant(r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#),
    ].into_iter().enumerate() {
        if index == 1 && !final_response { continue; }
        client.scripted.lock().unwrap().push_back(LlmResponse {
            message, usage: TokenUsage::default(), stop_reason: None, token_logprobs: vec![], meta: None,
        });
    }
}

#[tokio::test]
async fn goal_step_fresh_host_attestation_accepts_write_and_preserves_original_probe() {
    for file in [false, true] {
        for final_response in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let storage = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("source.txt"), "old source").unwrap();
            let store: Arc<dyn SessionStore> = if file {
                Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
            } else {
                Arc::new(MemorySessionStore::new())
            };
            let attestor = Arc::new(ByteAttestor {
                root: root.path().into(),
                calls: AtomicUsize::new(0),
            });
            let (agent, session, client) = fixture(
                root.path(),
                true,
                options("goal-run")
                    .with_assistant_final_response(final_response)
                    .with_session_store(store.clone())
                    .with_completion_attestor(attestor.clone())
                    .with_permission_policy(
                        crate::permissions::PermissionPolicy::new().allow("write(*)"),
                    ),
            )
            .await;
            script(&client, final_response);
            let result = session
                .send("Execute the admitted current step.", None)
                .await
                .unwrap();
            assert!(matches!(
                result.completion,
                crate::harness_loop::CompletionTerminal::Verified { .. }
            ));
            let host_calls = if final_response { 1 } else { 2 };
            assert_eq!(attestor.calls.load(Ordering::SeqCst), host_calls);
            assert_eq!(
                std::fs::read(root.path().join("source.txt")).unwrap(),
                b"independent expected source"
            );
            let goal = session.goal_definition().unwrap().unwrap();
            assert_eq!(
                goal.plan.as_ref().unwrap().steps[0].status,
                crate::planning::TaskStatus::Completed
            );
            assert_eq!(
                goal.plan.as_ref().unwrap().get_ready_steps()[0].id,
                "verify"
            );
            assert_eq!(goal.state, GoalState::Active);
            assert_eq!(
                client.calls.load(Ordering::SeqCst),
                if final_response { 3 } else { 2 }
            );
            assert!(client
                .inputs
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .contains("host:current-source"));
            let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
            assert_eq!(
                facts
                    .iter()
                    .filter(|f| f.kind == "a3s.goal.step.accepted")
                    .count(),
                1
            );
            assert!(!facts.iter().any(|f| f.kind == "a3s.goal.completed"));
            session.save().await.unwrap();
            session.close().await;
            agent.close().await;
            let agent = Agent::from_config(test_config()).await.unwrap();
            let store: Arc<dyn SessionStore> = if file {
                Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
            } else {
                store
            };
            let restored = agent
                .resume_session_async("goal-run", options("goal-run").with_session_store(store))
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(restored.goal_definition().unwrap().unwrap()).unwrap(),
                serde_json::to_value(goal).unwrap()
            );
            assert_eq!(
                attestor.calls.load(Ordering::SeqCst),
                host_calls,
                "reopen must not call the old host"
            );
            assert_eq!(
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
                facts
            );
            restored.close().await;
            agent.close().await;
        }
    }
}

#[tokio::test]
async fn goal_step_denied_attempt_preserves_verified_current_write() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), "old source").unwrap();
    let mut permission = crate::permissions::PermissionPolicy::new().allow("write(source.txt)");
    permission.default_decision = crate::permissions::PermissionDecision::Deny;
    let attestor = Arc::new(ByteAttestor {
        root: root.path().into(),
        calls: AtomicUsize::new(0),
    });
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run")
            .with_assistant_final_response(true)
            .with_completion_attestor(attestor.clone())
            .with_permission_policy(permission),
    )
    .await;
    client.scripted.lock().unwrap().push_back(LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![crate::llm::ContentBlock::ToolUse {
                id: "denied-write".into(),
                name: "write".into(),
                input: serde_json::json!({"file_path":"forbidden.txt", "content":"must not exist"}),
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
    script(&client, true);
    let result = session
        .send("Execute the admitted current step.", None)
        .await
        .unwrap();
    assert!(matches!(
        result.completion,
        crate::harness_loop::CompletionTerminal::Verified { .. }
    ));
    assert!(!root.path().join("forbidden.txt").exists());
    let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let denied: a3s_effect::ToolExecutionResultV1 = serde_json::from_value(
        facts
            .iter()
            .find(|f| f.kind == "tool.result")
            .unwrap()
            .payload["receipt"]
            .clone(),
    )
    .unwrap();
    assert_eq!(denied.outcome, a3s_effect::ToolExecutionOutcome::Denied);
    assert!(denied.effects.changes.is_empty());
    assert!(denied.identity.admission.is_some());
    assert_eq!(
        session
            .goal_definition()
            .unwrap()
            .unwrap()
            .plan
            .unwrap()
            .steps[0]
            .status,
        crate::planning::TaskStatus::Completed
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 4);
    assert_eq!(attestor.calls.load(Ordering::SeqCst), 1);
    session.close().await;
    agent.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn goal_step_late_artifact_drift_refuses_original_completion_and_progress() {
    for change in ["bytes", "mode", "missing", "symlink"] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("source.txt"), "old source").unwrap();
        let attestor = Arc::new(ByteAttestor {
            root: root.path().into(),
            calls: AtomicUsize::new(0),
        });
        let (agent, session, client) = fixture(
            root.path(),
            true,
            options("goal-run")
                .with_assistant_final_response(true)
                .with_completion_attestor(attestor.clone())
                .with_permission_policy(
                    crate::permissions::PermissionPolicy::new().allow("write(*)"),
                ),
        )
        .await;
        script(&client, true);
        let file = root.path().join("source.txt");
        *client.before_response.lock().unwrap() = Some(Box::new(move |call| {
            if call != 3 {
                return;
            }
            match change {
                "bytes" => std::fs::write(&file, "changed during judgment").unwrap(),
                "mode" => {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
                    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode ^ 0o100))
                        .unwrap();
                }
                "missing" => std::fs::remove_file(&file).unwrap(),
                "symlink" => {
                    std::fs::remove_file(&file).unwrap();
                    std::os::unix::fs::symlink("missing-target", &file).unwrap();
                }
                _ => unreachable!(),
            }
        }));
        let error = session
            .send("Execute the admitted current step.", None)
            .await
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("goal.evidence_stale"),
            "{change}: {error:#}"
        );
        assert_eq!(attestor.calls.load(Ordering::SeqCst), 1);
        let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
        assert!(!facts
            .iter()
            .any(|f| f.kind == "a3s.goal.step.accepted" || f.kind == "turn.complete"));
        assert!(crate::fact_control::has_unsettled_effects(&facts));
        session.close().await;
        agent.close().await;
    }
}

#[tokio::test]
async fn goal_step_fresh_host_report_with_wrong_effect_digest_is_not_authority() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), "old source").unwrap();
    let attestor = Arc::new(WrongDigestAttestor(ByteAttestor {
        root: root.path().into(),
        calls: AtomicUsize::new(0),
    }));
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run")
            .with_assistant_final_response(true)
            .with_completion_attestor(attestor.clone())
            .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("write(*)")),
    )
    .await;
    script(&client, true);
    let error = session
        .send("Execute the admitted current step.", None)
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("completion gate"),
        "{error:#}"
    );
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        2,
        "do not spend a judgment on an unrelated effect"
    );
    assert_eq!(attestor.0.calls.load(Ordering::SeqCst), 1);
    let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    assert!(!facts
        .iter()
        .any(|f| f.kind == "a3s.goal.step.accepted" || f.kind == "turn.complete"));
    assert!(crate::fact_control::has_unsettled_effects(&facts));
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_step_replan_revalidates_retained_bytes_without_replaying_the_write() {
    for file in [false, true] {
        for changed_criterion in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let storage = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("source.txt"), "old source").unwrap();
            let store: Arc<dyn SessionStore> = if file {
                Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
            } else {
                Arc::new(MemorySessionStore::new())
            };
            let attestor = Arc::new(ByteAttestor {
                root: root.path().into(),
                calls: AtomicUsize::new(0),
            });
            let (agent, session, client) = fixture(
                root.path(),
                true,
                options("goal-run")
                    .with_session_store(store.clone())
                    .with_assistant_final_response(true)
                    .with_completion_attestor(attestor.clone())
                    .with_permission_policy(
                        crate::permissions::PermissionPolicy::new().allow("write(*)"),
                    ),
            )
            .await;
            script(&client, true);
            session
                .send("Execute the admitted current step.", None)
                .await
                .unwrap();
            let before = session.goal_definition().unwrap().unwrap();
            let expected = if changed_criterion {
                "new required source"
            } else {
                "independent expected source"
            };
            let criterion = format!("The retained source bytes equal {expected}");
            let mut plan = before.plan.unwrap();
            assert_eq!(plan.steps[0].status, crate::planning::TaskStatus::Completed);
            plan.steps[0].content =
                "Revalidate the retained source without writing it again".into();
            plan.steps[0].success_criteria = Some(criterion.clone());
            plan.steps[0].tool = Some("verify_source".into());
            session
                .register_dynamic_tool(Arc::new(super::acceptance::VerifySource(expected.into())))
                .unwrap();
            let mut permission =
                crate::permissions::PermissionPolicy::new().allow("verify_source(*)");
            permission.default_decision = crate::permissions::PermissionDecision::Deny;
            session.set_permission_policy(permission).unwrap();
            let mut command = crate::goal_control::GoalPlanCommandV1 {
                kind: GoalCommandKindV1::ProposePlan,
                operation_id: "replan".into(),
                goal_id: before.binding.goal_id,
                session_id: before.binding.session_id,
                workspace: before.binding.workspace,
                work_policy_digest: before.binding.work_policy_digest,
                expected_goal_revision: before.binding.goal_revision,
                expected_plan_revision: before.plan_revision,
                expected_goal_sequence: before.goal_sequence,
                plan: Some(plan),
                proposal_sequence: None,
            };
            let proposed = session.apply_goal_plan(command.clone()).unwrap();
            assert_eq!(proposed.plan_revision, Some(2));
            assert!(proposed
                .plan
                .as_ref()
                .unwrap()
                .steps
                .iter()
                .all(|step| step.status == crate::planning::TaskStatus::Pending));
            command.kind = GoalCommandKindV1::AcceptPlan;
            command.operation_id = "accept-replan".into();
            command.expected_plan_revision = proposed.plan_revision;
            command.expected_goal_sequence = proposed.goal_sequence;
            command.proposal_sequence = proposed.plan_fact_sequence;
            command.plan = None;
            session.apply_goal_plan(command).unwrap();
            let source =
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
            session
                .send("The earlier result was completed.", None)
                .await
                .unwrap();
            assert_eq!(
                session
                    .goal_definition()
                    .unwrap()
                    .unwrap()
                    .plan
                    .unwrap()
                    .steps[0]
                    .status,
                crate::planning::TaskStatus::Pending
            );
            super::acceptance::script(
                &client,
                r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#,
            );
            let calls = client.calls.load(Ordering::SeqCst);
            session
                .send(
                    "Explicitly revalidate the retained source against the current criterion.",
                    None,
                )
                .await
                .unwrap();
            assert_eq!(
                client.calls.load(Ordering::SeqCst) - calls,
                if changed_criterion { 2 } else { 3 }
            );
            let goal = session.goal_definition().unwrap().unwrap();
            assert_eq!(goal.plan_revision, Some(2));
            assert_eq!(
                goal.plan.as_ref().unwrap().steps[0].status,
                if changed_criterion {
                    crate::planning::TaskStatus::Pending
                } else {
                    crate::planning::TaskStatus::Completed
                }
            );
            if !changed_criterion {
                assert!(client
                    .inputs
                    .lock()
                    .unwrap()
                    .last()
                    .unwrap()
                    .contains(&criterion));
            }
            let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
            assert_eq!(&facts[..source.len()], source.as_slice());
            for fact in facts
                .iter()
                .skip(source.len())
                .filter(|f| f.kind == "tool.result")
            {
                let receipt: a3s_effect::ToolExecutionResultV1 =
                    serde_json::from_value(fact.payload["receipt"].clone()).unwrap();
                assert!(
                    receipt.effects.changes.is_empty(),
                    "retained modifications must never replay"
                );
            }
            assert_eq!(
                facts
                    .iter()
                    .filter(
                        |f| f.kind == "a3s.goal.step.accepted" && f.payload["plan_revision"] == 2
                    )
                    .count(),
                usize::from(!changed_criterion)
            );
            assert_eq!(
                std::fs::read(root.path().join("source.txt")).unwrap(),
                b"independent expected source"
            );
            assert_eq!(attestor.calls.load(Ordering::SeqCst), 1);
            session.save().await.unwrap();
            session.close().await;
            agent.close().await;
            let agent = Agent::from_config(test_config()).await.unwrap();
            let store: Arc<dyn SessionStore> = if file {
                Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
            } else {
                store
            };
            let restored = agent
                .resume_session_async("goal-run", options("goal-run").with_session_store(store))
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(restored.goal_definition().unwrap().unwrap()).unwrap(),
                serde_json::to_value(goal).unwrap()
            );
            assert_eq!(
                crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
                facts
            );
            restored.close().await;
            agent.close().await;
        }
    }
}
