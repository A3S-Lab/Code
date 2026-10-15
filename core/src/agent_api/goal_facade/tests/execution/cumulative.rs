#![cfg(test)]

use super::automatic::{now_ms, policy, request, slices};
use super::*;
use crate::agent_api::{AgentRunSpawn, GoalTickAdmission};

#[derive(Default)]
struct Probe(Mutex<Vec<TokenUsage>>);

#[async_trait::async_trait]
impl crate::budget::BudgetGuard for Probe {
    async fn record_after_llm(&self, _: &str, usage: &TokenUsage) {
        self.0.lock().unwrap().push(usage.clone());
    }
}

fn respond(client: &GoalClient, content: Vec<crate::llm::ContentBlock>) {
    client.scripted.lock().unwrap().push_back(LlmResponse {
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
    });
}

/// A child Task inside an automatic slice is billed once, through the original
/// guard; the slice closeout keeps the parent Run's own usage beside it.
#[tokio::test(flavor = "multi_thread")]
async fn an_automatic_slice_child_task_is_billed_once_through_the_original_guard() {
    let root = tempfile::tempdir().unwrap();
    let probe = Arc::new(Probe::default());
    let opts = options("goal-run")
        .with_manual_delegation_enabled(true)
        .with_auto_delegation_enabled(false)
        .with_worker_agent(crate::subagent::WorkerAgentSpec::read_only(
            "worker",
            "Read only",
        ))
        .with_permission_policy(crate::permissions::PermissionPolicy::new().allow_all(&["task"]))
        .with_budget_guard(probe.clone());
    let (agent, session, client) = fixture_with(
        root.path(),
        true,
        opts,
        Some(policy(true, 1, 4, now_ms() + 600_000)),
    )
    .await;
    respond(
        &client,
        vec![crate::llm::ContentBlock::ToolUse {
            id: "delegate".into(),
            name: "task".into(),
            input: serde_json::json!({
                "agent": "worker",
                "description": "read",
                "prompt": "Reply ready without tools."
            }),
        }],
    );
    respond(
        &client,
        vec![crate::llm::ContentBlock::Text {
            text: "ready".into(),
        }],
    );
    respond(
        &client,
        vec![crate::llm::ContentBlock::Text {
            text: "The worker replied ready.".into(),
        }],
    );
    let GoalTickAdmission::Admitted(AgentRunSpawn::Started { worker, .. }) = session
        .tick_goal(&request(&session, "wake-1"))
        .await
        .unwrap()
    else {
        panic!("the tick admits one Run");
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        3,
        "parent, child, parent"
    );
    let governed = probe.0.lock().unwrap().clone();
    assert_eq!(
        governed.len(),
        3,
        "the child call reaches the original guard"
    );
    let facts = crate::fact_control::read_workspace_facts(root.path(), "goal-run").unwrap();
    let closeout = facts
        .iter()
        .rfind(|fact| fact.kind == "a3s.goal.slice.settled")
        .unwrap();
    let body = &closeout.payload["body"];
    assert_eq!(body["outcome"], "completed");
    let parent = body["usage"]["total_tokens"].as_u64().unwrap() as usize;
    let billed = governed
        .iter()
        .map(|usage| usage.total_tokens)
        .sum::<usize>();
    eprintln!("CHILD-COST closeout={parent} governed={billed}");
    assert_eq!(parent, 14, "the closeout is the parent Run's own usage");
    assert_eq!(billed, parent + 7, "the child is billed once, in the guard");
    assert_eq!(slices(root.path()).len(), 1);
    assert!(matches!(
        session
            .tick_goal(&request(&session, "wake-2"))
            .await
            .unwrap(),
        GoalTickAdmission::Decided(crate::goal_control::GoalTickDecisionV1::BudgetExhausted)
    ));
    assert_eq!(client.calls.load(Ordering::SeqCst), 3);
    session.close().await;
    agent.close().await;
}
