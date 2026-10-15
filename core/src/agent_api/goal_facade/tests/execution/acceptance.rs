#![cfg(test)]

use super::*;
use crate::tools::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use crate::verification::{VerificationCheck, VerificationReport, VerificationStatus};

pub(super) struct VerifySource(pub(super) String);

pub(super) fn script(client: &GoalClient, judgment: &str) {
    let mut responses = client.scripted.lock().unwrap();
    for message in [
        Message {
            role: "assistant".into(),
            content: vec![crate::llm::ContentBlock::ToolUse {
                id: "verify-current".into(),
                name: "verify_source".into(),
                input: serde_json::json!({}),
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: crate::llm::TranscriptVisibility::Wire,
        },
        Message::assistant("The original source is checked."),
        Message::assistant(judgment),
    ] {
        responses.push_back(LlmResponse {
            message,
            usage: TokenUsage {
                prompt_tokens: 5,
                completion_tokens: 2,
                total_tokens: 7,
                ..Default::default()
            },
            stop_reason: None,
            token_logprobs: vec![],
            meta: None,
        });
    }
}

#[async_trait::async_trait]
impl Tool for VerifySource {
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
        let observed = std::fs::read_to_string(ctx.workspace.join("source.txt"))?;
        let status = if observed == self.0 {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        };
        let report = VerificationReport::new(
            "host:source-check",
            vec![VerificationCheck::required(
                "inspect",
                "source-bytes",
                "Original source is checked",
            )
            .with_status(status)],
        );
        Ok(
            ToolOutput::success(observed).with_metadata(serde_json::json!({
                "verification_author":"host", "verification_report":report,
            })),
        )
    }
}

#[tokio::test]
async fn goal_step_current_source_and_original_judgment_accept_only_the_ready_step() {
    for file_store in [false, true] {
        prove_acceptance(file_store).await;
    }
}

async fn prove_acceptance(file_store: bool) {
    let root = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let store: Arc<dyn SessionStore> = if file_store {
        Arc::new(FileSessionStore::new(storage.path()).await.unwrap())
    } else {
        Arc::new(MemorySessionStore::new())
    };
    std::fs::write(
        root.path().join("source.txt"),
        "independent expected source",
    )
    .unwrap();
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run")
            .with_session_store(store.clone())
            .with_permission_policy(
                crate::permissions::PermissionPolicy::new().allow("verify_source(*)"),
            ),
    )
    .await;
    session
        .register_dynamic_tool(Arc::new(VerifySource("independent expected source".into())))
        .unwrap();
    script(
        &client,
        r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#,
    );
    let result = session
        .send("Execute the admitted current step.", None)
        .await
        .unwrap();
    assert_eq!(
        result.usage.total_tokens, 21,
        "include original judgment exactly once"
    );
    let goal = session.goal_definition().unwrap().unwrap();
    let plan = goal.plan.as_ref().unwrap();
    assert_eq!(plan.steps[0].status, crate::planning::TaskStatus::Completed);
    assert_eq!(plan.steps[1].status, crate::planning::TaskStatus::Pending);
    assert_eq!(plan.get_ready_steps()[0].id, "verify");
    assert_eq!(goal.state, GoalState::Active);
    let mut facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let settled = facts.pop().unwrap();
    assert_eq!(settled.kind, "a3s.goal.slice.settled");
    assert_eq!(settled.payload["body"]["outcome"], "completed");
    assert_eq!(
        settled.payload["body"]["usage"]["total_tokens"], 21,
        "the owner closes the slice with the Run's proven usage"
    );
    assert_eq!(
        settled.payload["body"]["step_sequence"],
        facts.last().unwrap().seq
    );
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.kind == "a3s.goal.step.accepted")
            .count(),
        1
    );
    assert!(!facts.iter().any(|f| f.kind == "a3s.goal.completed"));
    assert_eq!(client.calls.load(Ordering::SeqCst), 3);
    let inputs = client.inputs.lock().unwrap().clone();
    assert!(inputs[2].contains("Original source is checked"));
    assert!(!inputs[2].contains("All original checks pass"));
    assert!(inputs[2].contains("host:source-check"));
    assert!(
        inputs[2].contains("independent expected source"),
        "judge must see the original tool observation"
    );
    assert_eq!(
        serde_json::to_value(
            crate::goal_control::definition_from_facts(
                &facts,
                "goal-run",
                &goal.binding.workspace,
                None,
            )
            .unwrap()
            .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&goal).unwrap()
    );
    let accepted = facts.last().unwrap();
    assert_eq!(accepted.kind, "a3s.goal.step.accepted");
    let pending = a3s_effect::NewFact {
        kind: accepted.kind.clone(),
        key: accepted.key.clone(),
        payload: accepted.payload.clone(),
    };
    assert!(crate::goal_control::validate_step_append(
        &facts[..facts.len() - 1],
        &pending,
        "goal-run",
        &goal.binding.workspace,
        None,
    )
    .is_ok());
    let mut stale = facts[..facts.len() - 1].to_vec();
    stale.push(a3s_effect::Fact {
        seq: accepted.seq,
        kind: "user.message".into(),
        key: "late-input".into(),
        cause: None,
        payload: serde_json::json!({"text":"changed current source"}),
    });
    assert!(
        crate::goal_control::validate_step_append(
            &stale,
            &pending,
            "goal-run",
            &goal.binding.workspace,
            None,
        )
        .is_err(),
        "late input invalidates the judged prefix"
    );
    for field in ["run_id", "step_id", "source_digest"] {
        let mut tampered = facts.clone();
        tampered.last_mut().unwrap().payload["body"][field] = "foreign".into();
        assert!(
            crate::goal_control::definition_from_facts(
                &tampered,
                "goal-run",
                &goal.binding.workspace,
                None,
            )
            .is_err(),
            "reject altered {field}"
        );
    }
    session.save().await.unwrap();
    session.close().await;
    agent.close().await;
    let agent = Agent::from_config(test_config()).await.unwrap();
    let store: Arc<dyn SessionStore> = if file_store {
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
        serde_json::to_value(&goal).unwrap()
    );
    facts.push(settled);
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap(),
        facts
    );
    restored.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_step_failed_source_or_current_judgment_cannot_unlock_dependencies() {
    for (bytes, judgment, calls) in [
        (
            "independent expected source",
            r#"{"achieved":false,"progress":0.5,"remaining_criteria":["Original source is checked"]}"#,
            3,
        ),
        (
            "independent expected source",
            r#"{"achieved":true,"progress":1.0,"remaining_criteria":["Original source is checked"]}"#,
            3,
        ),
        (
            "unexpected source",
            r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#,
            2,
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("source.txt"), bytes).unwrap();
        let (agent, session, client) = fixture(
            root.path(),
            true,
            options("goal-run").with_permission_policy(
                crate::permissions::PermissionPolicy::new().allow("verify_source(*)"),
            ),
        )
        .await;
        session
            .register_dynamic_tool(Arc::new(VerifySource("independent expected source".into())))
            .unwrap();
        script(&client, judgment);
        let result = session
            .send("Execute the admitted current step.", None)
            .await
            .unwrap();
        assert_eq!(result.usage.total_tokens, calls * 7);
        assert_eq!(client.calls.load(Ordering::SeqCst), calls);
        let goal = session.goal_definition().unwrap().unwrap();
        let plan = goal.plan.unwrap();
        assert!(plan
            .steps
            .iter()
            .all(|step| step.status == crate::planning::TaskStatus::Pending));
        assert_eq!(plan.get_ready_steps()[0].id, "inspect");
        let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
        assert!(!facts.iter().any(|f| f.kind == "a3s.goal.step.accepted"));
        session.close().await;
        agent.close().await;
    }
}

#[tokio::test]
async fn goal_step_late_workspace_acceptance_still_requires_its_original_report() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("source.txt"),
        "independent expected source",
    )
    .unwrap();
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run").with_permission_policy(
            crate::permissions::PermissionPolicy::new().allow("verify_source(*)"),
        ),
    )
    .await;
    session
        .register_dynamic_tool(Arc::new(VerifySource("independent expected source".into())))
        .unwrap();
    script(
        &client,
        r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#,
    );
    let loop_dir = root.path().join(".a3s/loops/late-acceptance");
    *client.before_response.lock().unwrap() = Some(Box::new(move |call| {
        if call == 3 {
            std::fs::create_dir_all(&loop_dir).unwrap();
            std::fs::write(
                loop_dir.join("STATE.md"),
                "Status: running\nPhase: executing\n",
            )
            .unwrap();
            std::fs::write(
                loop_dir.join("ACCEPTANCE.md"),
                "- [ ] kind:command assert:`true` expect:exit=0\n",
            )
            .unwrap();
        }
    }));
    let error = session
        .send("Execute the admitted current step.", None)
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("goal.evidence_stale"),
        "{error:#}"
    );
    let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    assert!(!facts.iter().any(|f| f.kind == "a3s.goal.step.accepted"));
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_step_oversized_original_observation_cannot_be_silently_truncated() {
    let root = tempfile::tempdir().unwrap();
    let expected = "本次原始观察".repeat(3000);
    std::fs::write(root.path().join("source.txt"), &expected).unwrap();
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run")
            .with_tool_result_transform_policy(
                crate::tools::ToolResultTransformPolicyV1::conservative(),
            )
            .with_permission_policy(
                crate::permissions::PermissionPolicy::new().allow("verify_source(*)"),
            ),
    )
    .await;
    session
        .register_dynamic_tool(Arc::new(VerifySource(expected)))
        .unwrap();
    script(
        &client,
        r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#,
    );
    session
        .send("Execute the admitted current step.", None)
        .await
        .unwrap();
    assert_eq!(client.calls.load(Ordering::SeqCst), 2);
    let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let result = facts.iter().find(|f| f.kind == "tool.result").unwrap();
    let receipt: a3s_effect::ToolExecutionResultV1 =
        serde_json::from_value(result.payload["receipt"].clone()).unwrap();
    let envelope: serde_json::Value =
        serde_json::from_str(receipt.output.as_str().unwrap()).unwrap();
    assert_eq!(
        envelope["metadata"]["verification_report"]["status"],
        "passed",
        "outcome={:?}; metadata={}; output={}",
        receipt.outcome,
        envelope["metadata"],
        crate::text::truncate_utf8(envelope["text"].as_str().unwrap_or(""), 200)
    );
    assert!(!facts.iter().any(|f| f.kind == "a3s.goal.step.accepted"));
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_step_judges_a_bounded_observation_that_names_its_omission() {
    let root = tempfile::tempdir().unwrap();
    let expected = "本次原始观察".repeat(3000);
    std::fs::write(root.path().join("source.txt"), &expected).unwrap();
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run").with_permission_policy(
            crate::permissions::PermissionPolicy::new().allow("verify_source(*)"),
        ),
    )
    .await;
    session
        .register_dynamic_tool(Arc::new(VerifySource(expected)))
        .unwrap();
    script(
        &client,
        r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#,
    );
    session
        .send("Execute the admitted current step.", None)
        .await
        .unwrap();
    assert_eq!(client.calls.load(Ordering::SeqCst), 3);
    let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let result = facts.iter().find(|f| f.kind == "tool.result").unwrap();
    let receipt: a3s_effect::ToolExecutionResultV1 =
        serde_json::from_value(result.payload["receipt"].clone()).unwrap();
    let envelope: serde_json::Value =
        serde_json::from_str(receipt.output.as_str().unwrap()).unwrap();
    let text = envelope["text"].as_str().unwrap();
    assert!(text.len() <= 32 * 1024);
    assert!(text.contains("[tool output bounded by"), "{text:.200}");
    assert!(text.contains("a3s://tool-output/verify_source/"));
    assert_eq!(
        envelope["metadata"]["verification_report"]["status"],
        "passed"
    );
    assert!(facts.iter().any(|f| f.kind == "a3s.goal.step.accepted"));
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn goal_step_original_structured_repair_usage_is_included_once() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("source.txt"),
        "independent expected source",
    )
    .unwrap();
    let (agent, session, client) = fixture(
        root.path(),
        true,
        options("goal-run").with_permission_policy(
            crate::permissions::PermissionPolicy::new().allow("verify_source(*)"),
        ),
    )
    .await;
    session
        .register_dynamic_tool(Arc::new(VerifySource("independent expected source".into())))
        .unwrap();
    script(&client, "invalid structured decision");
    let mut repaired = client.scripted.lock().unwrap().back().unwrap().clone();
    repaired.message =
        Message::assistant(r#"{"achieved":true,"progress":1.0,"remaining_criteria":[]}"#);
    client.scripted.lock().unwrap().push_back(repaired);
    let result = session
        .send("Execute the admitted current step.", None)
        .await
        .unwrap();
    assert_eq!(client.calls.load(Ordering::SeqCst), 4);
    assert_eq!(result.usage.total_tokens, 28);
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
    session.close().await;
    agent.close().await;
}
