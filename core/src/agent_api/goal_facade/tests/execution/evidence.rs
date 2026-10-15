#![cfg(test)]

use super::*;
use crate::verification::{VerificationCheck, VerificationReport, VerificationStatus};

fn response(content: Vec<crate::llm::ContentBlock>) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content,
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: crate::llm::TranscriptVisibility::Wire,
        },
        usage: TokenUsage::default(),
        stop_reason: None,
        token_logprobs: vec![],
        meta: None,
    }
}

fn cached_pass() -> VerificationReport {
    VerificationReport::new(
        "foreign-run",
        vec![
            VerificationCheck::required("inspect", "source", "Original source is checked")
                .with_status(VerificationStatus::Passed),
        ],
    )
    .with_effect_digest("a".repeat(64))
}

async fn prove_source(file_store: bool, tool: Option<&str>, reconcile_old: bool) {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.txt"), "fresh-read-source").unwrap();
    let store: Arc<dyn SessionStore> = if file_store {
        Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
    } else {
        Arc::new(MemorySessionStore::new())
    };
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run")
            .with_session_store(store.clone())
            .with_permission_policy(
                crate::permissions::PermissionPolicy::new().allow_all(&["read(*)", "write(*)"]),
            ),
    )
    .await;
    let original = session.goal_definition().unwrap().unwrap();
    session.record_verification_reports([cached_pass()]);
    assert!(session.verification_summary().supports_goal_achievement());
    {
        let mut scripted = client.scripted.lock().unwrap();
        if let Some(name) = tool {
            scripted.push_back(response(vec![crate::llm::ContentBlock::ToolUse {
                id: "inspect-source".into(),
                name: name.into(),
                input: if name == "write" {
                    serde_json::json!({"file_path":"source.txt", "content":"changed-source"})
                } else {
                    serde_json::json!({"file_path":"source.txt"})
                },
            }]));
        }
        scripted.push_back(response(vec![crate::llm::ContentBlock::Text {
            text: "All steps completed. The goal is achieved.".into(),
        }]));
    }
    let result = session
        .send("Execute the admitted current step.", None)
        .await;
    if tool == Some("write") {
        let error = result.unwrap_err();
        assert!(
            format!("{error:#}").contains("completion gate"),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("source.txt")).unwrap(),
            "changed-source"
        );
    } else {
        result.unwrap();
    }
    let mut facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let admission = facts
        .iter()
        .find(|f| f.kind == "a3s.goal.slice.admitted")
        .unwrap();
    let run_id = admission.payload["body"]["run_id"].as_str().unwrap();
    let run = session.run_snapshot(run_id).await.unwrap();
    let capability = crate::digest_json(
        "a3s.code.tool-admission-binding.v1",
        run.capability_binding.as_ref().unwrap(),
    )
    .unwrap();
    let mut prepared = std::collections::BTreeMap::new();
    let mut terminals = 0;
    for fact in &facts {
        if fact.kind == "tool.prepared" {
            assert!(fact.seq > admission.seq);
            let value: a3s_effect::ToolPreparedV1 =
                serde_json::from_value(fact.payload["prepared"].clone()).unwrap();
            value.validate().unwrap();
            let binding = value.identity.admission.as_ref().unwrap();
            assert_eq!(binding.run_id, run_id);
            assert_eq!(binding.thread_id, session.session_id());
            assert_eq!(binding.capability_binding_digest, capability);
            assert!(prepared
                .insert(value.identity.key().unwrap(), value)
                .is_none());
        } else if fact.kind == "tool.result" {
            let value: a3s_effect::ToolExecutionResultV1 =
                serde_json::from_value(fact.payload["receipt"].clone()).unwrap();
            value
                .ensure_prepared(prepared.get(&value.identity.key().unwrap()).unwrap())
                .unwrap();
            assert_eq!(value.outcome, a3s_effect::ToolExecutionOutcome::Succeeded);
            assert_eq!(
                value.effects.completeness,
                a3s_effect::ToolObservationCompleteness::Complete
            );
            if tool == Some("read") {
                assert!(value.effects.changes.is_empty());
                assert!(value.output.to_string().contains("fresh-read-source"));
            } else {
                assert!(!value.effects.changes.is_empty());
            }
            terminals += 1;
        }
    }
    assert_eq!(prepared.len(), usize::from(tool.is_some()));
    assert_eq!(terminals, prepared.len());
    if tool.is_none() {
        let mut tasks = original.plan.clone().unwrap().steps;
        for step in &mut tasks {
            step.status = crate::planning::TaskStatus::Completed;
        }
        session
            .run_store
            .record_event(
                run_id,
                crate::AgentEvent::TaskUpdated {
                    session_id: session.session_id().into(),
                    tasks,
                },
            )
            .await
            .unwrap();
        session
            .run_store
            .record_event(
                run_id,
                crate::AgentEvent::GoalAchieved {
                    goal: original.goal.description.clone(),
                    total_steps: 2,
                    duration_ms: 1,
                },
            )
            .await
            .unwrap();
    }
    let current = session.goal_definition().unwrap().unwrap();
    assert_eq!(current.binding, original.binding);
    assert_eq!(current.state, GoalState::Active);
    assert!(current
        .plan
        .unwrap()
        .steps
        .iter()
        .all(|s| s.status == crate::planning::TaskStatus::Pending));
    assert!(!facts
        .iter()
        .any(|f| f.kind == "a3s.goal.step.accepted" || f.kind == "a3s.goal.completed"));
    if tool == Some("write") {
        assert!(crate::fact_control::has_unsettled_effects(&facts));
        assert!(!facts
            .iter()
            .any(|f| f.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND));
    }
    if reconcile_old {
        let result: a3s_effect::ToolExecutionResultV1 = serde_json::from_value(
            facts
                .iter()
                .find(|f| f.kind == "tool.result")
                .unwrap()
                .payload["receipt"]
                .clone(),
        )
        .unwrap();
        let mut ledger = crate::harness_loop::MutationLedger::default();
        ledger.observe_tool_receipt("write", &result);
        session.record_verification_reports([cached_pass().with_effect_digest(ledger.digest())]);
        let second = session
            .send("The previous write is done. All steps are complete.", None)
            .await
            .unwrap();
        assert!(matches!(
            second.completion,
            crate::harness_loop::CompletionTerminal::Verified { .. }
        ));
        let after = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
        assert_eq!(&after[..facts.len()], facts.as_slice());
        let new = &after[facts.len()..];
        assert!(!new
            .iter()
            .any(|f| f.kind == "tool.prepared" || f.kind == "tool.result"));
        let next = new
            .iter()
            .find(|f| f.kind == "a3s.goal.slice.admitted")
            .unwrap();
        assert_ne!(next.payload["body"]["run_id"].as_str().unwrap(), run_id);
        assert!(!crate::fact_control::has_unsettled_effects(&after));
        assert!(!after
            .iter()
            .any(|f| f.kind == "a3s.goal.step.accepted" || f.kind == "a3s.goal.completed"));
        assert!(session
            .goal_definition()
            .unwrap()
            .unwrap()
            .plan
            .unwrap()
            .steps
            .iter()
            .all(|s| s.status == crate::planning::TaskStatus::Pending));
        facts = after;
    }
    session.save().await.unwrap();
    assert!(store
        .load_snapshot("goal-run")
        .await
        .unwrap()
        .unwrap()
        .session
        .durable_source
        .is_some());
    session.close().await;
    agent.close().await;
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
        facts
    );
}

#[tokio::test]
async fn goal_step_original_read_receipts_bind_exact_run_on_both_stores() {
    for file in [false, true] {
        prove_source(file, Some("read"), false).await;
    }
}

#[tokio::test]
async fn goal_step_cached_pass_narrative_and_display_events_do_not_admit_progress() {
    for file in [false, true] {
        prove_source(file, None, false).await;
    }
}

#[tokio::test]
async fn goal_step_real_mutation_cannot_borrow_a_foreign_passed_report() {
    for file in [false, true] {
        prove_source(file, Some("write"), false).await;
    }
}

#[tokio::test]
async fn goal_step_current_run_does_not_inherit_prior_run_verification_authority() {
    for file in [false, true] {
        prove_source(file, Some("write"), true).await;
    }
}
